// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 批量下发 runner：逐设备 revision → 模板 → bind → restart，结果持久化。

use std::time::Instant;

use serde_json::{Value, json};
use tokio::task::JoinSet;

use crate::{
    api::{
        apply::{ApplyPlan, PlanAssignment, error_payload, request_timeout, write_timeout},
        job_dto::TemplateRef,
    },
    db::{JobResultUpdate, PendingAssignment},
    device_client::DeviceCallError,
    ids,
    state::AppState,
};

/// 启动一个 job：标记 running、并发执行 assignment、汇总终态、审计与刷新缓存。
pub async fn run(state: AppState, job_id: String, plan: ApplyPlan) {
    let job_permit = state.job_sem.clone().acquire_owned().await.ok();

    match state.db.start_job(&job_id).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(job_id, "job 不是 queued，跳过执行");
            return;
        }
        Err(error) => {
            tracing::error!(error = %error, job_id, "启动 job 失败");
            return;
        }
    }

    let pending = match state.db.pending_assignments(&job_id).await {
        Ok(pending) => pending,
        Err(error) => {
            tracing::error!(error = %error, job_id, "读取 pending assignment 失败");
            finalize(&state, &job_id).await;
            return;
        }
    };

    let restart = plan.restart;
    let request_id = plan.request_id.clone();
    let mut tasks = JoinSet::new();
    for pending_assignment in pending {
        let Some(plan_assignment) = plan
            .assignments
            .get(pending_assignment.ordinal as usize)
            .cloned()
        else {
            tracing::warn!(
                job_id,
                ordinal = pending_assignment.ordinal,
                "缺少 assignment 计划，跳过"
            );
            continue;
        };

        let state = state.clone();
        let job_id = job_id.clone();
        let request_id = request_id.clone();
        tasks.spawn(async move {
            let _permit = state.apply_sem.clone().acquire_owned().await.ok();
            let lock = state.device_lock(&pending_assignment.device_id);
            let _guard = lock.lock().await;

            let update = execute_assignment(
                &state,
                &job_id,
                &pending_assignment,
                &plan_assignment,
                restart,
                request_id.clone(),
            )
            .await;
            if let Err(error) = state.db.write_job_result(update).await {
                tracing::error!(
                    error = %error,
                    job_id,
                    device_id = %pending_assignment.device_id,
                    "写入 job 结果失败"
                );
            }
        });
    }
    while tasks.join_next().await.is_some() {}

    finalize(&state, &job_id).await;
    // 缓存刷新不占用 job 并发槽位，避免慢设备阻塞后续 job。
    drop(job_permit);
    refresh_ok_devices(&state, &job_id).await;
}

async fn finalize(state: &AppState, job_id: &str) {
    let finished_at = ids::now_rfc3339();
    if let Err(error) = state.db.finalize_job(job_id, &finished_at).await {
        tracing::error!(error = %error, job_id, "finalize job 失败");
    }

    let Ok(Some(job)) = state.db.get_job(job_id).await else {
        return;
    };

    let audit_status = if job.status == "failed" {
        "failed"
    } else {
        "ok"
    };
    crate::api::devices::write_audit(
        state,
        "job.finished",
        Some("job"),
        Some(job_id),
        Some(&job.request_id),
        audit_status,
        json!({
            "status": job.status,
            "succeeded": job.succeeded,
            "skipped": job.skipped,
            "failed": job.failed,
        }),
    )
    .await;
}

/// job 完成后 best-effort 刷新 ok 设备的 overview 缓存。
async fn refresh_ok_devices(state: &AppState, job_id: &str) {
    let Ok(Some(job)) = state.db.get_job(job_id).await else {
        return;
    };
    if job.status == "queued" || job.status == "running" {
        return;
    }
    if let Ok(results) = state.db.job_results(job_id).await {
        for result in results.into_iter().filter(|result| result.status == "ok") {
            crate::api::devices::refresh_overview(state, &result.device_id).await;
        }
    }
}

async fn execute_assignment(
    state: &AppState,
    job_id: &str,
    pending: &PendingAssignment,
    plan: &PlanAssignment,
    restart_requested: bool,
    request_id: Option<String>,
) -> JobResultUpdate {
    let started = Instant::now();
    let mut steps: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();
    let device_id = pending.device_id.as_str();
    let app = plan.app.as_str();

    let device = match state.db.get_device(device_id).await {
        Ok(Some(device)) => device,
        Ok(None) => {
            return simple_failure(
                job_id,
                pending,
                started,
                "device_error",
                "设备已不在注册表",
                json!({ "deviceId": device_id }),
                steps,
                warnings,
            );
        }
        Err(error) => {
            tracing::error!(error = %error, device_id, "读取设备失败");
            return simple_failure(
                job_id,
                pending,
                started,
                "device_error",
                "读取设备失败",
                json!({ "deviceId": device_id }),
                steps,
                warnings,
            );
        }
    };

    let token = match state.key.decrypt(device_id, &device.token_cipher) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(error = %error, device_id, "设备 Token 解密失败");
            return simple_failure(
                job_id,
                pending,
                started,
                "device_error",
                "设备 Token 解密失败",
                json!({ "deviceId": device_id }),
                steps,
                warnings,
            );
        }
    };

    // 1) revision
    let revision_started = Instant::now();
    let revision = match state
        .http
        .get_revision(
            &device.base_url,
            token.as_str(),
            request_timeout(state),
            request_id.as_deref(),
        )
        .await
    {
        Ok(value) => match value.get("revision").and_then(Value::as_str) {
            Some(revision) => {
                steps.push(step("revision", "ok", Some(200), None, revision_started));
                revision.to_string()
            }
            None => {
                steps.push(step(
                    "revision",
                    "failed",
                    Some(200),
                    Some("device_error"),
                    revision_started,
                ));
                return simple_failure(
                    job_id,
                    pending,
                    started,
                    "device_error",
                    "设备 revision 响应缺少 revision",
                    json!({ "deviceId": device_id }),
                    steps,
                    warnings,
                );
            }
        },
        Err(error) => {
            let failure = Failure::from_error(&error);
            steps.push(step(
                "revision",
                failure.result,
                failure.http_status,
                Some(failure.code),
                revision_started,
            ));
            return failure_update(
                job_id, pending, started, failure, steps, warnings, None, 1, None,
            );
        }
    };

    // 2) 模板：字符串只确认存在；对象缺失则创建、存在但不同则覆盖
    let mut bind_revision = revision.clone();
    let template_name = match &plan.template {
        TemplateRef::Name(name) => name.clone(),
        TemplateRef::Object(object) => object.name.clone(),
    };

    match &plan.template {
        TemplateRef::Name(name) => {
            let template_started = Instant::now();
            match state
                .http
                .get_template(
                    &device.base_url,
                    token.as_str(),
                    name,
                    request_timeout(state),
                    request_id.as_deref(),
                )
                .await
            {
                Ok(_) => steps.push(step(
                    "template_get",
                    "ok",
                    Some(200),
                    None,
                    template_started,
                )),
                Err(error) if is_http(&error, 404) => {
                    steps.push(step(
                        "template_get",
                        "skipped",
                        Some(404),
                        Some("template_not_found"),
                        template_started,
                    ));
                    return simple_skipped(
                        job_id,
                        pending,
                        started,
                        "template_not_found",
                        format!("设备上没有模板 {name}"),
                        json!({ "template": name, "resource": "template" }),
                        Some(404),
                        false,
                        steps,
                        warnings,
                    );
                }
                Err(error) => {
                    let failure = Failure::from_error(&error);
                    steps.push(step(
                        "template_get",
                        failure.result,
                        failure.http_status,
                        Some(failure.code),
                        template_started,
                    ));
                    return failure_update(
                        job_id, pending, started, failure, steps, warnings, None, 1, None,
                    );
                }
            }
        }
        TemplateRef::Object(object) => {
            let template_started = Instant::now();
            let existing = match state
                .http
                .get_template(
                    &device.base_url,
                    token.as_str(),
                    &object.name,
                    request_timeout(state),
                    request_id.as_deref(),
                )
                .await
            {
                Ok(value) => {
                    steps.push(step(
                        "template_get",
                        "ok",
                        Some(200),
                        None,
                        template_started,
                    ));
                    Some(value)
                }
                Err(error) if is_http(&error, 404) => {
                    steps.push(step(
                        "template_get",
                        "skipped",
                        Some(404),
                        None,
                        template_started,
                    ));
                    None
                }
                Err(error) => {
                    let failure = Failure::from_error(&error);
                    steps.push(step(
                        "template_get",
                        failure.result,
                        failure.http_status,
                        Some(failure.code),
                        template_started,
                    ));
                    return failure_update(
                        job_id, pending, started, failure, steps, warnings, None, 1, None,
                    );
                }
            };

            let existing_template = existing.as_ref().and_then(|value| value.get("template"));
            let same_semantic = existing_template.is_some_and(|current| {
                template_config_without_packages(current)
                    == template_config_without_packages(&object.config)
            });
            let packages_same = match (
                object.config.get("packages"),
                existing_template.and_then(|template| template.get("packages")),
            ) {
                (Some(requested), Some(current)) => requested == current,
                (Some(_), None) => false,
                (None, _) => true,
            };
            let needs_put = !same_semantic || !packages_same;

            if needs_put {
                if existing.is_some() && !same_semantic {
                    warnings.push(json!({
                        "code": "template_overwritten",
                        "message": format!("模板 {} 已按请求覆盖", object.name),
                        "details": { "deviceId": device_id, "name": object.name },
                    }));
                }
                let put_config = merged_template_config(&object.config, existing_template);
                let mut put_attempts = 0;
                loop {
                    put_attempts += 1;
                    let put_started = Instant::now();
                    match state
                        .http
                        .put_template(
                            &device.base_url,
                            token.as_str(),
                            &object.name,
                            &put_config,
                            write_timeout(state),
                            request_id.as_deref(),
                        )
                        .await
                    {
                        Ok((status, _)) => {
                            steps.push(step("template_put", "ok", Some(status), None, put_started));
                            break;
                        }
                        Err(error) if is_http(&error, 412) && put_attempts < 2 => {
                            steps.push(step(
                                "template_put",
                                "failed",
                                Some(412),
                                Some("config_revision_mismatch"),
                                put_started,
                            ));
                            continue;
                        }
                        Err(error) => {
                            let failure = Failure::from_error(&error);
                            steps.push(step(
                                "template_put",
                                failure.result,
                                failure.http_status,
                                Some(failure.code),
                                put_started,
                            ));
                            return failure_update(
                                job_id, pending, started, failure, steps, warnings, None, 1, None,
                            );
                        }
                    }
                }

                // 模板 PUT 会推进设备 revision；bind 前必须重新取
                let refresh_started = Instant::now();
                match state
                    .http
                    .get_revision(
                        &device.base_url,
                        token.as_str(),
                        request_timeout(state),
                        request_id.as_deref(),
                    )
                    .await
                {
                    Ok(value) => match value.get("revision").and_then(Value::as_str) {
                        Some(next) => {
                            bind_revision = next.to_string();
                            steps.push(step("revision", "ok", Some(200), None, refresh_started));
                        }
                        None => {
                            steps.push(step(
                                "revision",
                                "failed",
                                Some(200),
                                Some("device_error"),
                                refresh_started,
                            ));
                            return simple_failure(
                                job_id,
                                pending,
                                started,
                                "device_error",
                                "模板写入后重取 revision 失败：响应缺少 revision",
                                json!({ "deviceId": device_id }),
                                steps,
                                warnings,
                            );
                        }
                    },
                    Err(error) => {
                        let failure = Failure::from_error(&error);
                        steps.push(step(
                            "revision",
                            failure.result,
                            failure.http_status,
                            Some(failure.code),
                            refresh_started,
                        ));
                        return failure_update(
                            job_id,
                            pending,
                            started,
                            failure,
                            steps,
                            warnings,
                            Some(bind_revision),
                            1,
                            None,
                        );
                    }
                }
            }
        }
    }

    // 3) bind：412 重新取 revision 后重试一次
    let mut attempts = 0i64;
    let (bind_status, bind_revision) = loop {
        attempts += 1;
        let bind_started = Instant::now();
        match state
            .http
            .bind_app(
                &device.base_url,
                token.as_str(),
                app,
                &template_name,
                &bind_revision,
                write_timeout(state),
                request_id.as_deref(),
            )
            .await
        {
            Ok((status, value)) => {
                steps.push(step("bind", "ok", Some(status), None, bind_started));
                let next_revision = value
                    .get("revision")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| bind_revision.clone());
                break (Some(i64::from(status)), next_revision);
            }
            Err(error) if is_http(&error, 412) && attempts < 2 => {
                steps.push(step(
                    "bind",
                    "failed",
                    Some(412),
                    Some("config_revision_mismatch"),
                    bind_started,
                ));
                let refresh_started = Instant::now();
                match state
                    .http
                    .get_revision(
                        &device.base_url,
                        token.as_str(),
                        request_timeout(state),
                        request_id.as_deref(),
                    )
                    .await
                {
                    Ok(value) => match value.get("revision").and_then(Value::as_str) {
                        Some(next) => {
                            bind_revision = next.to_string();
                            steps.push(step("revision", "ok", Some(200), None, refresh_started));
                            continue;
                        }
                        None => {
                            steps.push(step(
                                "revision",
                                "failed",
                                Some(200),
                                Some("device_error"),
                                refresh_started,
                            ));
                            return simple_failure(
                                job_id,
                                pending,
                                started,
                                "device_error",
                                "重取 revision 失败：响应缺少 revision",
                                json!({ "deviceId": device_id }),
                                steps,
                                warnings,
                            );
                        }
                    },
                    Err(error) => {
                        let failure = Failure::from_error(&error);
                        steps.push(step(
                            "revision",
                            failure.result,
                            failure.http_status,
                            Some(failure.code),
                            refresh_started,
                        ));
                        return failure_update(
                            job_id,
                            pending,
                            started,
                            failure,
                            steps,
                            warnings,
                            Some(bind_revision),
                            attempts,
                            None,
                        );
                    }
                }
            }
            Err(error) if is_http(&error, 404) => {
                let resource = error
                    .upstream_details()
                    .and_then(|details| details.get("resource"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if resource == "package" {
                    steps.push(step(
                        "bind",
                        "skipped",
                        Some(404),
                        Some("package_not_installed"),
                        bind_started,
                    ));
                    return simple_skipped(
                        job_id,
                        pending,
                        started,
                        "package_not_installed",
                        format!("设备未安装 {app}"),
                        json!({ "package": app, "resource": "package" }),
                        Some(404),
                        false,
                        steps,
                        warnings,
                    );
                }
                steps.push(step(
                    "bind",
                    "skipped",
                    Some(404),
                    Some("template_not_found"),
                    bind_started,
                ));
                return simple_skipped(
                    job_id,
                    pending,
                    started,
                    "template_not_found",
                    format!("设备上模板 {template_name} 不存在"),
                    json!({ "template": template_name, "resource": "template" }),
                    Some(404),
                    false,
                    steps,
                    warnings,
                );
            }
            Err(error) => {
                let failure = Failure::from_error(&error);
                steps.push(step(
                    "bind",
                    failure.result,
                    failure.http_status,
                    Some(failure.code),
                    bind_started,
                ));
                return failure_update(
                    job_id,
                    pending,
                    started,
                    failure,
                    steps,
                    warnings,
                    Some(bind_revision),
                    attempts,
                    None,
                );
            }
        }
    };

    // 4) restart：失败只记警告，不改变结果
    let mut restart: Option<Value> = None;
    if restart_requested {
        let restart_started = Instant::now();
        let idempotency_key = format!("{job_id}:{device_id}:{app}");
        match state
            .http
            .restart_app(
                &device.base_url,
                token.as_str(),
                app,
                &idempotency_key,
                request_timeout(state),
                request_id.as_deref(),
            )
            .await
        {
            Ok((status, _)) => {
                restart = Some(json!({
                    "status": "ok",
                    "httpStatus": status,
                    "code": null,
                    "durationMs": elapsed_ms(restart_started),
                }));
                steps.push(step("restart", "ok", Some(status), None, restart_started));
            }
            Err(error) => {
                let status = if is_http(&error, 403) {
                    "skipped"
                } else {
                    "failed"
                };
                let code = if is_http(&error, 403) {
                    "forbidden_scope"
                } else {
                    error.manager_code()
                };
                restart = Some(json!({
                    "status": status,
                    "httpStatus": error.http_status(),
                    "code": code,
                    "durationMs": elapsed_ms(restart_started),
                }));
                warnings.push(json!({
                    "code": "restart_failed",
                    "message": error.summary(),
                    "details": { "deviceId": device_id, "package": app, "code": code },
                }));
                steps.push(step(
                    "restart",
                    status,
                    error.http_status(),
                    Some(code),
                    restart_started,
                ));
            }
        }
    }

    Outcome {
        status: "ok",
        code: None,
        http_status: bind_status,
        revision: Some(bind_revision),
        attempts,
        restart,
        steps,
        warnings,
        error: None,
    }
    .into_update(job_id, pending, started)
}

struct Failure {
    result: &'static str,
    code: &'static str,
    message: String,
    details: Value,
    retryable: bool,
    http_status: Option<u16>,
}

impl Failure {
    fn from_error(error: &DeviceCallError) -> Self {
        match error {
            DeviceCallError::Offline { .. } => Self {
                result: "skipped",
                code: "device_offline",
                message: error.summary(),
                details: error.details(),
                retryable: true,
                http_status: None,
            },
            DeviceCallError::Http { status, code, .. } => {
                let (result, mapped) = match (*status, code.as_str()) {
                    (401 | 403, _) => ("failed", "forbidden_scope"),
                    (412, _) => ("failed", "config_revision_mismatch"),
                    (422, _) => ("failed", "validation_failed"),
                    (503, "upstream_unavailable") => ("failed", "upstream_unavailable"),
                    _ => ("failed", "device_error"),
                };
                Self {
                    result,
                    code: mapped,
                    message: error.summary(),
                    details: error.details(),
                    retryable: *status >= 500,
                    http_status: Some(*status),
                }
            }
            DeviceCallError::Decode { .. } | DeviceCallError::TooLarge { .. } => Self {
                result: "failed",
                code: "device_error",
                message: error.summary(),
                details: error.details(),
                retryable: false,
                http_status: None,
            },
        }
    }
}

struct Outcome {
    status: &'static str,
    code: Option<&'static str>,
    http_status: Option<i64>,
    revision: Option<String>,
    attempts: i64,
    restart: Option<Value>,
    steps: Vec<Value>,
    warnings: Vec<Value>,
    error: Option<Value>,
}

impl Outcome {
    fn into_update(
        self,
        job_id: &str,
        pending: &PendingAssignment,
        started: Instant,
    ) -> JobResultUpdate {
        JobResultUpdate {
            job_id: job_id.to_string(),
            device_id: pending.device_id.clone(),
            app: pending.app.clone(),
            status: self.status.to_string(),
            code: self.code.map(str::to_owned),
            http_status: self.http_status,
            revision: self.revision,
            attempts: self.attempts.max(1),
            duration_ms: elapsed_ms(started),
            restart: self.restart,
            steps: self.steps,
            warnings: self.warnings,
            error: self.error,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn failure_update(
    job_id: &str,
    pending: &PendingAssignment,
    started: Instant,
    failure: Failure,
    steps: Vec<Value>,
    warnings: Vec<Value>,
    revision: Option<String>,
    attempts: i64,
    restart: Option<Value>,
) -> JobResultUpdate {
    let error = Some(error_payload(
        failure.code,
        &failure.message,
        failure.details.clone(),
        failure.retryable,
    ));
    Outcome {
        status: failure.result,
        code: Some(failure.code),
        http_status: failure.http_status.map(i64::from),
        revision,
        attempts,
        restart,
        steps,
        warnings,
        error,
    }
    .into_update(job_id, pending, started)
}

#[allow(clippy::too_many_arguments)]
fn simple_failure(
    job_id: &str,
    pending: &PendingAssignment,
    started: Instant,
    code: &'static str,
    message: impl Into<String>,
    details: Value,
    steps: Vec<Value>,
    warnings: Vec<Value>,
) -> JobResultUpdate {
    let error = Some(error_payload(code, &message.into(), details, false));
    Outcome {
        status: "failed",
        code: Some(code),
        http_status: None,
        revision: None,
        attempts: 1,
        restart: None,
        steps,
        warnings,
        error,
    }
    .into_update(job_id, pending, started)
}

#[allow(clippy::too_many_arguments)]
fn simple_skipped(
    job_id: &str,
    pending: &PendingAssignment,
    started: Instant,
    code: &'static str,
    message: impl Into<String>,
    details: Value,
    http_status: Option<i64>,
    retryable: bool,
    steps: Vec<Value>,
    warnings: Vec<Value>,
) -> JobResultUpdate {
    let error = Some(error_payload(code, &message.into(), details, retryable));
    Outcome {
        status: "skipped",
        code: Some(code),
        http_status,
        revision: None,
        attempts: 1,
        restart: None,
        steps,
        warnings,
        error,
    }
    .into_update(job_id, pending, started)
}

/// 比较模板语义时忽略 packages（packages 是绑定关系，不是机型字段）。
fn template_config_without_packages(config: &Value) -> Value {
    let mut normalized = config.clone();
    if let Some(object) = normalized.as_object_mut() {
        object.remove("packages");
    }
    normalized
}

/// PUT 前合并：请求未显式给 packages 时保留设备原 packages，避免解绑其它应用。
fn merged_template_config(request: &Value, existing_template: Option<&Value>) -> Value {
    let mut merged = request.clone();
    if merged.get("packages").is_none()
        && let Some(packages) = existing_template.and_then(|template| template.get("packages"))
        && let Some(object) = merged.as_object_mut()
    {
        object.insert("packages".to_string(), packages.clone());
    }
    merged
}

fn step(
    name: &str,
    status: &str,
    http_status: Option<u16>,
    code: Option<&str>,
    started: Instant,
) -> Value {
    json!({
        "step": name,
        "status": status,
        "httpStatus": http_status,
        "code": code,
        "durationMs": elapsed_ms(started),
    })
}

fn elapsed_ms(started: Instant) -> i64 {
    started.elapsed().as_millis().min(i64::MAX as u128) as i64
}

fn is_http(error: &DeviceCallError, status: u16) -> bool {
    matches!(error, DeviceCallError::Http { status: actual, .. } if *actual == status)
}
