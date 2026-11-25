// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 批量下发 runner：逐设备 revision → 模板 → bind → restart，结果持久化。

use std::time::Instant;

use serde_json::{Value, json};
use tokio::task::JoinSet;

use std::collections::HashMap;

use crate::{
    api::{
        apply::{ApplyPlan, PlanAssignment, error_payload, request_timeout, write_timeout},
        egress::EgressPlan,
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

// ── egress 下发 runner ──────────────────────────────────────────────────

/// 启动一个 egress job：逐设备串行、跨设备并发、逐 (device, app) 落结果。
pub async fn run_egress(state: AppState, job_id: String, plan: EgressPlan) {
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

    // 同一设备的多个 (device, app) 目标由一次 GET + PUT 覆盖，因此按设备分组。
    let mut grouped: Vec<(String, Vec<PendingAssignment>)> = Vec::new();
    for row in pending {
        match grouped
            .iter()
            .position(|(device_id, _)| device_id == &row.device_id)
        {
            Some(index) => grouped[index].1.push(row),
            None => grouped.push((row.device_id.clone(), vec![row])),
        }
    }

    let request_id = plan.request_id.clone();
    let probe = plan.probe;
    let mut tasks = JoinSet::new();
    for (device_id, rows) in grouped {
        let state = state.clone();
        let job_id = job_id.clone();
        let request_id = request_id.clone();
        tasks.spawn(async move {
            let _permit = state.apply_sem.clone().acquire_owned().await.ok();
            let lock = state.device_lock(&device_id);
            let _guard = lock.lock().await;

            let updates = execute_egress_device(
                &state,
                &job_id,
                &device_id,
                &rows,
                probe,
                request_id.clone(),
            )
            .await;
            for update in updates {
                if let Err(error) = state.db.write_job_result(update).await {
                    tracing::error!(error = %error, job_id, device_id = %device_id, "写入 job 结果失败");
                }
            }
        });
    }
    while tasks.join_next().await.is_some() {}

    finalize(&state, &job_id).await;
    drop(job_permit);
    refresh_ok_devices(&state, &job_id).await;
}

/// 一次设备级 egress 下发：GET 现状 → 组装期望态 → 比较 → PUT（412 重试一次）→ 可选探测。
async fn execute_egress_device(
    state: &AppState,
    job_id: &str,
    device_id: &str,
    rows: &[PendingAssignment],
    probe: bool,
    request_id: Option<String>,
) -> Vec<JobResultUpdate> {
    let started = Instant::now();
    let mut steps: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();

    let device = match state.db.get_device(device_id).await {
        Ok(Some(device)) => device,
        Ok(None) => {
            return rows
                .iter()
                .map(|row| {
                    egress_failure_update(
                        job_id,
                        row,
                        started,
                        "failed",
                        "device_error",
                        "设备已不在注册表",
                        json!({ "deviceId": device_id }),
                        steps.clone(),
                        warnings.clone(),
                        None,
                        1,
                    )
                })
                .collect();
        }
        Err(error) => {
            tracing::error!(error = %error, device_id, "读取设备失败");
            return rows
                .iter()
                .map(|row| {
                    egress_failure_update(
                        job_id,
                        row,
                        started,
                        "failed",
                        "device_error",
                        "读取设备失败",
                        json!({ "deviceId": device_id }),
                        steps.clone(),
                        warnings.clone(),
                        None,
                        1,
                    )
                })
                .collect();
        }
    };

    let token = match state.key.decrypt(device_id, &device.token_cipher) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(error = %error, device_id, "设备 Token 解密失败");
            return rows
                .iter()
                .map(|row| {
                    egress_failure_update(
                        job_id,
                        row,
                        started,
                        "failed",
                        "device_token_invalid",
                        "设备 Token 解密失败（devmask-manager.key 可能已变更，需要重新登记设备的 Token）",
                        json!({ "deviceId": device_id, "op": "decrypt" }),
                        steps.clone(),
                        warnings.clone(),
                        None,
                        1,
                    )
                })
                .collect();
        }
    };

    // 1) 取现状
    let get_started = Instant::now();
    let current = match state
        .http
        .get_egress(
            &device.base_url,
            token.as_str(),
            request_timeout(state),
            request_id.as_deref(),
        )
        .await
    {
        Ok(value) => {
            steps.push(step("egress_get", "ok", Some(200), None, get_started));
            value
        }
        Err(error) => {
            let failure = EgressFailure::from_error(&error);
            steps.push(step(
                "egress_get",
                failure.result,
                failure.http_status,
                Some(failure.code),
                get_started,
            ));
            return rows
                .iter()
                .map(|row| {
                    egress_failure_update(
                        job_id,
                        row,
                        started,
                        failure.result,
                        failure.code,
                        &failure.message,
                        failure.details.clone(),
                        steps.clone(),
                        warnings.clone(),
                        None,
                        1,
                    )
                })
                .collect();
        }
    };

    // 设备可达：best-effort 回写在线状态（不覆盖其它元数据）。
    if !device.online {
        let now = ids::now_rfc3339();
        if let Err(error) = state
            .db
            .update_device_probe(crate::db::DeviceProbeUpdate {
                id: device_id.to_string(),
                online: true,
                model: device.model.clone(),
                brand: device.brand.clone(),
                android: device.android.clone(),
                sdk: device.sdk,
                fingerprint: device.fingerprint.clone(),
                root_manager: device.root_manager.clone(),
                module_version: device.module_version.clone(),
                server_version: device.server_version.clone(),
                last_seen: now.clone(),
                last_synced_at: device.last_synced_at.clone().unwrap_or_else(|| now.clone()),
                latency_ms: device.latency_ms,
                updated_at: now,
            })
            .await
        {
            tracing::warn!(error = %error, device_id, "回写设备在线状态失败");
        }
    }

    // 2) 组装期望态（解密失败与 DB 失败要给出不同结果码）
    let desired = match build_desired_egress(state, device_id).await {
        Ok(desired) => desired,
        Err(error) => {
            if matches!(error, DesiredError::Decrypt(_)) {
                tracing::error!(
                    error = %error.message(),
                    device_id,
                    "代理凭据解密失败（devmask-manager.key 可能已变更）"
                );
            } else {
                tracing::error!(error = %error.message(), device_id, "组装期望出口状态失败");
            }
            return rows
                .iter()
                .map(|row| {
                    egress_failure_update(
                        job_id,
                        row,
                        started,
                        "failed",
                        error.code(),
                        &error.message(),
                        error.details(device_id),
                        steps.clone(),
                        warnings.clone(),
                        None,
                        1,
                    )
                })
                .collect();
        }
    };
    warnings.extend(desired.warnings.clone());

    let current_revision = current
        .get("revision")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut revision = current_revision.clone();
    let mut attempts = 1i64;
    let mut unchanged = false;

    // 3) 总是 PUT：设备端在文本一致时幂等（changed:false、不写盘），
    //    因此 egress_unchanged 由设备响应的 changed 判定，配置漂移与凭据轮换都能自然收敛。
    let put_status: Option<u16> = {
        let mut if_match = current_revision
            .as_deref()
            .map(|value| format!("\"{value}\""))
            .unwrap_or_else(|| "*".to_string());
        loop {
            let put_started = Instant::now();
            match state
                .http
                .put_egress(
                    &device.base_url,
                    token.as_str(),
                    &desired.body,
                    &if_match,
                    write_timeout(state),
                    request_id.as_deref(),
                )
                .await
            {
                Ok((status, value)) => {
                    let device_changed = value
                        .get("changed")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    if device_changed {
                        steps.push(step("egress_put", "ok", Some(status), None, put_started));
                    } else {
                        unchanged = true;
                        steps.push(step(
                            "egress_put",
                            "skipped",
                            Some(status),
                            Some("egress_unchanged"),
                            put_started,
                        ));
                    }
                    if let Some(next) = value.get("revision").and_then(Value::as_str) {
                        revision = Some(next.to_string());
                    }
                    break Some(status);
                }
                Err(error) if is_http(&error, 412) && attempts < 2 => {
                    attempts += 1;
                    steps.push(step(
                        "egress_put",
                        "failed",
                        Some(412),
                        Some("config_revision_mismatch"),
                        put_started,
                    ));
                    let refresh_started = Instant::now();
                    match state
                        .http
                        .get_egress(
                            &device.base_url,
                            token.as_str(),
                            request_timeout(state),
                            request_id.as_deref(),
                        )
                        .await
                    {
                        Ok(value) => {
                            steps.push(step("egress_get", "ok", Some(200), None, refresh_started));
                            revision = value
                                .get("revision")
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                            if let Some(next) = revision.as_deref() {
                                if_match = format!("\"{next}\"");
                            } else {
                                if_match = "*".to_string();
                            }
                        }
                        Err(error) => {
                            let failure = EgressFailure::from_error(&error);
                            steps.push(step(
                                "egress_get",
                                failure.result,
                                failure.http_status,
                                Some(failure.code),
                                refresh_started,
                            ));
                            return rows
                                .iter()
                                .map(|row| {
                                    egress_failure_update(
                                        job_id,
                                        row,
                                        started,
                                        failure.result,
                                        failure.code,
                                        &failure.message,
                                        failure.details.clone(),
                                        steps.clone(),
                                        warnings.clone(),
                                        revision.clone(),
                                        attempts,
                                    )
                                })
                                .collect();
                        }
                    }
                }
                Err(error) => {
                    let failure = EgressFailure::from_error(&error);
                    steps.push(step(
                        "egress_put",
                        failure.result,
                        failure.http_status,
                        Some(failure.code),
                        put_started,
                    ));
                    return rows
                        .iter()
                        .map(|row| {
                            egress_failure_update(
                                job_id,
                                row,
                                started,
                                failure.result,
                                failure.code,
                                &failure.message,
                                failure.details.clone(),
                                steps.clone(),
                                warnings.clone(),
                                revision.clone(),
                                attempts,
                            )
                        })
                        .collect();
                }
            }
        }
    };

    // 5) 可选探测：失败只记 warning，不改结果
    if probe {
        for row in rows {
            let probe_started = Instant::now();
            let body = if row.app.is_empty() {
                None
            } else {
                Some(json!({ "package": row.app }))
            };
            // 只有 app 行 mode=proxy 时才回填该行的 proxy_id；direct 行留 NULL。
            // 设备级行（row.app 为空）才回落到 default_proxy_id。
            let check_proxy_id = match desired.row_proxy.get(&row.app) {
                Some(explicit) => explicit.clone(),
                None => desired.default_proxy_id.clone(),
            };
            let scope = if row.app.is_empty() { "device" } else { "app" };
            match state
                .http
                .probe_egress(
                    &device.base_url,
                    token.as_str(),
                    body.as_ref(),
                    request_timeout(state),
                    request_id.as_deref(),
                )
                .await
            {
                Ok(value) => {
                    steps.push(step("egress_probe", "ok", Some(200), None, probe_started));
                    let checked_at = value
                        .get("checkedAt")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(ids::now_rfc3339);
                    record_check(
                        state,
                        crate::db::NewEgressCheck {
                            source: "device".to_string(),
                            device_id: Some(device_id.to_string()),
                            app: row.app.clone(),
                            scope: scope.to_string(),
                            proxy_id: check_proxy_id,
                            exit_ip: value
                                .get("exitIp")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            latency_ms: value.get("latencyMs").and_then(Value::as_i64),
                            ok: true,
                            error_code: None,
                            checked_at,
                        },
                        &mut warnings,
                        device_id,
                    )
                    .await;
                }
                Err(error) => {
                    let failure = EgressFailure::from_error(&error);
                    steps.push(step(
                        "egress_probe",
                        "failed",
                        failure.http_status,
                        Some(failure.code),
                        probe_started,
                    ));
                    warnings.push(json!({
                        "code": "egress_probe_failed",
                        "message": failure.message,
                        "details": {
                            "deviceId": device_id,
                            "app": row.app,
                            "reason": failure.details.get("reason").cloned().unwrap_or(Value::Null),
                        },
                    }));
                    record_check(
                        state,
                        crate::db::NewEgressCheck {
                            source: "device".to_string(),
                            device_id: Some(device_id.to_string()),
                            app: row.app.clone(),
                            scope: scope.to_string(),
                            proxy_id: check_proxy_id,
                            exit_ip: None,
                            latency_ms: None,
                            ok: false,
                            error_code: failure
                                .details
                                .get("reason")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            checked_at: ids::now_rfc3339(),
                        },
                        &mut warnings,
                        device_id,
                    )
                    .await;
                }
            }
        }
    }

    // 6) 收尾：每个 (device, app) 一行结果
    rows.iter()
        .map(|row| JobResultUpdate {
            job_id: job_id.to_string(),
            device_id: row.device_id.clone(),
            app: row.app.clone(),
            // 幂等短路：结果仍是 ok（全部 skipped 会把 job 算成 partial），
            // 只把 code 标成 egress_unchanged 让调用方知道没有真下发。
            status: "ok".to_string(),
            code: unchanged.then(|| "egress_unchanged".to_string()),
            http_status: put_status.map(i64::from),
            revision: revision.clone(),
            attempts: attempts.max(1),
            duration_ms: elapsed_ms(started),
            restart: None,
            steps: steps.clone(),
            warnings: warnings.clone(),
            error: None,
        })
        .collect()
}

/// 写入一条探测结果；失败时把 egress_check_not_recorded 追加到 job warnings。
async fn record_check(
    state: &AppState,
    check: crate::db::NewEgressCheck,
    warnings: &mut Vec<Value>,
    device_id: &str,
) {
    let app = check.app.clone();
    let scope = check.scope.clone();
    let proxy_id = check.proxy_id.clone();
    if let Err(error) = state.db.insert_egress_check(check).await {
        tracing::warn!(error = %error, device_id, "写入 egress_checks 失败");
        warnings.push(check_not_recorded_warning(
            device_id,
            &app,
            &scope,
            proxy_id.as_deref(),
        ));
    }
}

/// egress_check_not_recorded：探测成功/失败但结果没能落库。
pub fn check_not_recorded_warning(
    device_id: &str,
    app: &str,
    scope: &str,
    proxy_id: Option<&str>,
) -> Value {
    json!({
        "code": "egress_check_not_recorded",
        "message": "探测结果未能写入 egress_checks（数据库写入失败）",
        "details": {
            "deviceId": device_id,
            "app": app,
            "scope": scope,
            "proxyId": proxy_id,
        },
    })
}

/// 期望的出口状态：一次 PUT 覆盖该设备的全部绑定。
struct DesiredEgress {
    /// PUT /api/v1/egress 请求体。
    body: Value,
    /// 整机绑定的代理 id（egress_checks 用）。
    default_proxy_id: Option<String>,
    /// app -> 代理 id（egress_checks 用）。
    row_proxy: HashMap<String, Option<String>>,
    warnings: Vec<Value>,
}

async fn build_desired_egress(
    state: &AppState,
    device_id: &str,
) -> Result<DesiredEgress, DesiredError> {
    let bindings = state
        .db
        .list_device_egress(device_id)
        .await
        .map_err(|error| DesiredError::Db(error.to_string()))?;

    let mut proxy_defs: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();
    let mut row_proxy: HashMap<String, Option<String>> = HashMap::new();
    let mut default_name: Option<String> = None;
    let mut default_proxy_id: Option<String> = None;
    let mut apps: Vec<(String, String)> = Vec::new();

    for binding in bindings {
        let app = binding.app.clone();
        match (binding.mode.as_str(), binding.proxy_id.clone()) {
            ("direct", _) => {
                if app.is_empty() {
                    default_name = None;
                    default_proxy_id = None;
                } else {
                    apps.push((app.clone(), crate::api::egress::DIRECT_PROXY.to_string()));
                    row_proxy.insert(app, None);
                }
            }
            (_, Some(proxy_id)) => {
                let Some(proxy) = state
                    .db
                    .get_egress_proxy(&proxy_id)
                    .await
                    .map_err(|error| DesiredError::Db(error.to_string()))?
                else {
                    warnings.push(json!({
                        "code": "egress_proxy_not_found",
                        "message": format!("代理定义已不存在：{proxy_id}"),
                        "details": { "deviceId": device_id, "app": app, "name": proxy_id },
                    }));
                    continue;
                };
                if !proxy.enabled {
                    warnings.push(json!({
                        "code": "egress_proxy_disabled",
                        "message": format!("代理 {} 已停用，仍按旧绑定下发", proxy.name),
                        "details": { "proxyId": proxy.id, "deviceId": device_id },
                    }));
                }
                if !seen.contains(&proxy.name) {
                    let mut definition = json!({
                        "name": proxy.name,
                        "type": proxy.kind,
                        "host": proxy.host,
                        "port": proxy.port,
                    });
                    // 凭据字段总是显式给出：未设置时发 null（= 清除设备侧残留）；
                    // 省略在设备端语义是「不改」，会让上一次的密码留在设备上。
                    definition["username"] = match proxy.username_cipher.as_deref() {
                        Some(cipher) => {
                            let username = state
                                .key
                                .decrypt_egress(
                                    &proxy.id,
                                    crate::crypto::EgressField::Username,
                                    cipher,
                                )
                                .map_err(|error| {
                                    DesiredError::Decrypt(format!(
                                        "{} / username: {error}",
                                        proxy.id
                                    ))
                                })?;
                            json!(username.as_str())
                        }
                        None => Value::Null,
                    };
                    definition["password"] = match proxy.password_cipher.as_deref() {
                        Some(cipher) => {
                            let password = state
                                .key
                                .decrypt_egress(
                                    &proxy.id,
                                    crate::crypto::EgressField::Password,
                                    cipher,
                                )
                                .map_err(|error| {
                                    DesiredError::Decrypt(format!(
                                        "{} / password: {error}",
                                        proxy.id
                                    ))
                                })?;
                            json!(password.as_str())
                        }
                        None => Value::Null,
                    };
                    seen.push(proxy.name.clone());
                    proxy_defs.push(definition);
                }
                if app.is_empty() {
                    default_name = Some(proxy.name.clone());
                    default_proxy_id = Some(proxy.id.clone());
                } else {
                    apps.push((app.clone(), proxy.name.clone()));
                    row_proxy.insert(app, Some(proxy.id.clone()));
                }
            }
            (_, None) => {}
        }
    }

    // Manager 是绑定关系的真实来源：default / apps 总是显式给出，移除绑定才不会残留。
    // 设备端语义（契约 §1.4）：给数组 = 整体替换该数组；null = 清除所有 [[apps]].egress；
    // 零应用级绑定时发 null，避免依赖「空数组 == 清空」这一设备实现细节。
    let apps_value = if apps.is_empty() {
        Value::Null
    } else {
        json!(
            apps.iter()
                .map(|(package, egress)| json!({ "package": package, "egress": egress }))
                .collect::<Vec<_>>()
        )
    };
    let body = json!({
        "proxies": proxy_defs,
        "default": default_name,
        "apps": apps_value,
    });

    Ok(DesiredEgress {
        body,
        default_proxy_id,
        row_proxy,
        warnings,
    })
}

/// 组装期望态的错误：解密失败必须给出独立结果码，不能混成 device_error。
enum DesiredError {
    /// 代理凭据解密失败（devmask-manager.key 变更 / 密文损坏）。
    Decrypt(String),
    /// 读取绑定 / 代理定义失败。
    Db(String),
}

impl DesiredError {
    fn code(&self) -> &'static str {
        match self {
            Self::Decrypt(_) => "decrypt_failed",
            Self::Db(_) => "device_error",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Decrypt(detail) => {
                format!("代理凭据解密失败（devmask-manager.key 可能已变更）：{detail}")
            }
            Self::Db(_) => "读取代理绑定 / 定义失败".to_string(),
        }
    }

    fn details(&self, device_id: &str) -> Value {
        match self {
            Self::Decrypt(detail) => json!({
                "deviceId": device_id,
                "op": "decrypt",
                "reason": "credential_decrypt_failed",
                "message": "代理凭据解密失败（devmask-manager.key 可能已变更）",
                "detail": detail,
            }),
            Self::Db(detail) => json!({
                "deviceId": device_id,
                "op": "db",
                "reason": detail,
            }),
        }
    }
}

/// egress 结果码映射（契约 §6.12）。
struct EgressFailure {
    result: &'static str,
    code: &'static str,
    message: String,
    details: Value,
    http_status: Option<u16>,
}

impl EgressFailure {
    fn from_error(error: &DeviceCallError) -> Self {
        match error {
            DeviceCallError::Offline { reason } => {
                let (result, code) = if *reason == crate::device_client::OfflineReason::Timeout {
                    ("skipped", "upstream_timeout")
                } else {
                    ("skipped", "device_offline")
                };
                Self {
                    result,
                    code,
                    message: error.summary(),
                    details: error.details(),
                    http_status: None,
                }
            }
            DeviceCallError::Http {
                status,
                code: upstream_code,
                ..
            } => {
                let (result, mapped) = match (*status, upstream_code.as_str()) {
                    (404, _) => ("skipped", "egress_unsupported"),
                    (401 | 403, _) => ("failed", "forbidden_scope"),
                    (409, code) if code.starts_with("egress.") => {
                        ("failed", "egress_not_configured")
                    }
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
                    http_status: Some(*status),
                }
            }
            DeviceCallError::Decode { .. } | DeviceCallError::TooLarge { .. } => Self {
                result: "failed",
                code: "device_error",
                message: error.summary(),
                details: error.details(),
                http_status: None,
            },
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn egress_failure_update(
    job_id: &str,
    row: &PendingAssignment,
    started: Instant,
    status: &'static str,
    code: &'static str,
    message: &str,
    details: Value,
    steps: Vec<Value>,
    warnings: Vec<Value>,
    revision: Option<String>,
    attempts: i64,
) -> JobResultUpdate {
    JobResultUpdate {
        job_id: job_id.to_string(),
        device_id: row.device_id.clone(),
        app: row.app.clone(),
        status: status.to_string(),
        code: Some(code.to_string()),
        http_status: details
            .get("upstreamStatus")
            .and_then(Value::as_u64)
            .map(|value| value as i64),
        revision,
        attempts: attempts.max(1),
        duration_ms: elapsed_ms(started),
        restart: None,
        steps,
        warnings,
        error: Some(error_payload(code, message, details, status == "skipped")),
    }
}

fn is_http(error: &DeviceCallError, status: u16) -> bool {
    matches!(error, DeviceCallError::Http { status: actual, .. } if *actual == status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egress_check_not_recorded_warning_shape() {
        let warning = check_not_recorded_warning("pixel-9", "com.xingin.xhs", "app", Some("egp_x"));
        assert_eq!(warning["code"], "egress_check_not_recorded");
        assert_eq!(warning["details"]["deviceId"], "pixel-9");
        assert_eq!(warning["details"]["app"], "com.xingin.xhs");
        assert_eq!(warning["details"]["scope"], "app");
        assert_eq!(warning["details"]["proxyId"], "egp_x");

        let warning = check_not_recorded_warning("pixel-9", "", "device", None);
        assert!(warning["details"]["proxyId"].is_null());
    }
}
