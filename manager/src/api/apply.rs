// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! POST /relay/api/v1/apply：校验、归一化、入队并启动 runner。

use std::collections::HashSet;
use std::time::Duration;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::{
    api::job_dto::{ApplyAcceptedResponse, ApplyRequest, DevicesSpec, TemplateObject, TemplateRef},
    db::{JobRow, NewJob, PendingAssignment},
    error::{ApiError, ValidationError},
    ids, request_id,
    state::AppState,
};

/// 一条已校验的 assignment。
#[derive(Debug, Clone)]
pub struct PlanAssignment {
    pub device_id: String,
    pub app: String,
    pub template: TemplateRef,
}

/// 已校验的 apply 计划；runner 使用内存计划，pending 行用于进度持久化。
#[derive(Debug, Clone)]
pub struct ApplyPlan {
    pub kind: &'static str,
    pub app: Option<String>,
    pub template_name: Option<String>,
    pub restart: bool,
    pub request_id: Option<String>,
    /// 审计 target_id：bulk 为 all / multiple，assignments 为 mixed。
    pub audit_target: &'static str,
    pub assignments: Vec<PlanAssignment>,
}

const MAX_ASSIGNMENTS: usize = 200;

/// POST /relay/api/v1/apply
pub async fn apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<ApplyRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let request = json_body(payload)?;
    let request_id = request_id::current();
    let mut plans = build_plan(&state, request).await?;
    plans.request_id = request_id.clone();

    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(key) = idempotency_key.as_deref()
        && !valid_idempotency_key(key)
    {
        return Err(ApiError::validation(vec![ValidationError::new(
            "Idempotency-Key",
            "invalid_value",
            "Idempotency-Key 需为 1-128 字符，且不含空白/控制字符",
        )]));
    }

    if let Some(key) = idempotency_key.as_deref()
        && let Some(existing) = state
            .db
            .find_job_by_idempotency(key)
            .await
            .map_err(|error| internal("apply.idempotency", error))?
    {
        return Ok(replay_response(existing));
    }

    let now = ids::now_rfc3339();
    let job_id = ids::new_job_id();
    let total = plans.assignments.len() as i64;
    let replay_key = idempotency_key.clone();
    let job = NewJob {
        id: job_id.clone(),
        kind: plans.kind.to_string(),
        status: "queued".to_string(),
        app: plans.app.clone(),
        template: plans.template_name.clone(),
        restart: plans.restart,
        total,
        request_id: request_id.clone().unwrap_or_else(ids::new_request_id),
        idempotency_key,
        created_at: now.clone(),
    };
    let pending: Vec<PendingAssignment> = plans
        .assignments
        .iter()
        .enumerate()
        .map(|(index, assignment)| PendingAssignment {
            ordinal: index as i64,
            device_id: assignment.device_id.clone(),
            app: assignment.app.clone(),
        })
        .collect();

    if let Err(error) = state.db.insert_job_with_assignments(job, pending).await {
        // 并发同 key：UNIQUE 冲突时回查并重放，避免 500。
        if let Some(key) = replay_key.as_deref()
            && let Ok(Some(existing)) = state.db.find_job_by_idempotency(key).await
        {
            return Ok(replay_response(existing));
        }
        return Err(internal("apply.insert_job", error));
    }

    let (audit_action, audit_detail) = if plans.kind == "bulk" {
        (
            "apply.bulk",
            json!({
                "jobId": &job_id,
                "total": total,
                "app": &plans.app,
                "template": &plans.template_name,
            }),
        )
    } else {
        (
            "apply.assignments",
            json!({
                "jobId": &job_id,
                "total": total,
                "assignments": total,
            }),
        )
    };
    crate::api::devices::write_audit(
        &state,
        audit_action,
        Some("device"),
        Some(plans.audit_target),
        request_id.as_deref(),
        "ok",
        audit_detail,
    )
    .await;

    let state_for_runner = state.clone();
    let job_id_for_runner = job_id.clone();
    let plan_for_runner = plans.clone();
    tokio::spawn(async move {
        crate::jobs::run(state_for_runner, job_id_for_runner, plan_for_runner).await;
    });

    let body = ApplyAcceptedResponse {
        job_id: job_id.clone(),
        status: "queued".to_string(),
        kind: plans.kind.to_string(),
        total,
        queued: total,
        created_at: now,
        poll_url: format!("/relay/api/v1/jobs/{job_id}"),
    };
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}

/// 解析并校验请求体，生成计划。
async fn build_plan(state: &AppState, request: ApplyRequest) -> Result<ApplyPlan, ApiError> {
    let restart = request.restart.unwrap_or(false);
    let has_bulk_fields =
        request.app.is_some() || request.devices.is_some() || request.template.is_some();

    if request.assignments.is_some() && has_bulk_fields {
        return Err(ApiError::validation(vec![ValidationError::new(
            "assignments",
            "invalid_value",
            "assignments 与 app/devices/template 只能二选一",
        )]));
    }

    let devices = state
        .db
        .list_devices()
        .await
        .map_err(|error| internal("apply.list_devices", error))?;
    let known: HashSet<&str> = devices.iter().map(|device| device.id.as_str()).collect();

    if let Some(assignments) = request.assignments {
        return build_assignments_plan(assignments, &known, restart);
    }

    let Some(app) = request.app else {
        return Err(ApiError::validation(vec![ValidationError::new(
            "app",
            "missing_required",
            "bulk 请求需要 app",
        )]));
    };
    let Some(devices_spec) = request.devices else {
        return Err(ApiError::validation(vec![ValidationError::new(
            "devices",
            "missing_required",
            "bulk 请求需要 devices（\"all\" 或 id 数组）",
        )]));
    };
    let Some(template) = request.template else {
        return Err(ApiError::validation(vec![ValidationError::new(
            "template",
            "missing_required",
            "bulk 请求需要 template（模板名或 {name, config}）",
        )]));
    };

    let mut errors = Vec::new();
    validate_package(&app, "app", &mut errors);
    let template_name = validate_template(&template, "template", &mut errors);
    let audit_target = match &devices_spec {
        DevicesSpec::All(literal) if literal == "all" => "all",
        DevicesSpec::All(_) | DevicesSpec::List(_) => "multiple",
    };
    let target_ids = match devices_spec {
        DevicesSpec::All(literal) => {
            if literal != "all" {
                errors.push(ValidationError::new(
                    "devices",
                    "invalid_value",
                    "devices 只能是 \"all\" 或设备 id 数组",
                ));
            }
            devices.iter().map(|device| device.id.clone()).collect()
        }
        DevicesSpec::List(ids_list) => {
            if ids_list.is_empty() || ids_list.len() > MAX_ASSIGNMENTS {
                errors.push(ValidationError::new(
                    "devices",
                    "too_many_items",
                    format!("devices 数量需为 1-{MAX_ASSIGNMENTS}"),
                ));
            }
            let mut seen = HashSet::new();
            for (index, device_id) in ids_list.iter().enumerate() {
                if !known.contains(device_id.as_str()) {
                    errors.push(ValidationError::new(
                        format!("devices[{index}]"),
                        "invalid_device_id",
                        format!("设备未注册：{device_id}"),
                    ));
                } else if !seen.insert(device_id.clone()) {
                    errors.push(ValidationError::new(
                        format!("devices[{index}]"),
                        "duplicate_assignment",
                        format!("设备重复：{device_id}"),
                    ));
                }
            }
            ids_list
        }
    };

    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    if target_ids.is_empty() {
        return Err(ApiError::validation(vec![ValidationError::new(
            "devices",
            "invalid_value",
            "没有可下发的设备",
        )]));
    }

    Ok(ApplyPlan {
        kind: "bulk",
        app: Some(app.clone()),
        template_name,
        restart,
        request_id: None,
        audit_target,
        assignments: target_ids
            .into_iter()
            .map(|device_id| PlanAssignment {
                device_id,
                app: app.clone(),
                template: template.clone(),
            })
            .collect(),
    })
}

fn build_assignments_plan(
    assignments: Vec<crate::api::job_dto::AssignmentRequest>,
    known: &HashSet<&str>,
    restart: bool,
) -> Result<ApplyPlan, ApiError> {
    if assignments.is_empty() || assignments.len() > MAX_ASSIGNMENTS {
        return Err(ApiError::validation(vec![ValidationError::new(
            "assignments",
            "too_many_items",
            format!("assignments 数量需为 1-{MAX_ASSIGNMENTS}"),
        )]));
    }

    let mut errors = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut plan = Vec::with_capacity(assignments.len());

    for (index, assignment) in assignments.into_iter().enumerate() {
        let path = format!("assignments[{index}]");
        if !known.contains(assignment.device.as_str()) {
            errors.push(ValidationError::new(
                format!("{path}.device"),
                "invalid_device_id",
                format!("设备未注册：{}", assignment.device),
            ));
        }
        validate_package(&assignment.app, &format!("{path}.app"), &mut errors);
        validate_template(
            &assignment.template,
            &format!("{path}.template"),
            &mut errors,
        );
        if !seen.insert((assignment.device.clone(), assignment.app.clone())) {
            errors.push(ValidationError::new(
                path.clone(),
                "duplicate_assignment",
                "同一 (device, app) 重复",
            ));
        }
        plan.push(PlanAssignment {
            device_id: assignment.device,
            app: assignment.app,
            template: assignment.template,
        });
    }

    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    Ok(ApplyPlan {
        kind: "assignments",
        app: None,
        template_name: None,
        restart,
        request_id: None,
        audit_target: "mixed",
        assignments: plan,
    })
}

fn validate_template(
    template: &TemplateRef,
    path: &str,
    errors: &mut Vec<ValidationError>,
) -> Option<String> {
    match template {
        TemplateRef::Name(name) => {
            if !is_valid_template_name(name) {
                errors.push(ValidationError::new(
                    format!("{path}.name"),
                    "invalid_template_name",
                    "模板名不合法",
                ));
            }
            Some(name.clone())
        }
        TemplateRef::Object(TemplateObject { name, config }) => {
            if !is_valid_template_name(name) {
                errors.push(ValidationError::new(
                    format!("{path}.name"),
                    "invalid_template_name",
                    "模板名不合法",
                ));
            }
            if !config.is_object() {
                errors.push(ValidationError::new(
                    format!("{path}.config"),
                    "invalid_type",
                    "config 必须是对象",
                ));
            } else if config
                .get("model")
                .and_then(serde_json::Value::as_str)
                .is_none_or(str::is_empty)
            {
                errors.push(ValidationError::new(
                    format!("{path}.config.model"),
                    "missing_required",
                    "template.config.model 必填",
                ));
            }
            Some(name.clone())
        }
    }
}

/// 包名校验（与设备端一致，支持 @userId）。
pub fn is_valid_package(package: &str) -> bool {
    let (name, user) = match package.rsplit_once('@') {
        Some((name, user)) => {
            if user.is_empty() || user.len() > 3 || !user.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            (name, Some(user))
        }
        None => (package, None),
    };
    let _ = user;

    let mut segments = name.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    if !is_package_segment(first) {
        return false;
    }
    let mut count = 1;
    for segment in segments {
        if !is_package_segment(segment) {
            return false;
        }
        count += 1;
    }
    count >= 2
}

fn is_package_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic() && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_valid_template_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name.len() <= 64
        && (first.is_ascii_alphanumeric() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
}

fn validate_package(package: &str, path: &str, errors: &mut Vec<ValidationError>) {
    if !is_valid_package(package) {
        errors.push(ValidationError::new(
            path,
            "invalid_package",
            format!("包名不合法：{package}"),
        ));
    }
}

fn replay_response(job: JobRow) -> Response {
    let body = ApplyAcceptedResponse::from(job);
    let mut response = (StatusCode::ACCEPTED, Json(body)).into_response();
    response
        .headers_mut()
        .insert("idempotent-replay", HeaderValue::from_static("true"));
    response
}

/// Idempotency-Key：1-128 字符，不含空白/控制字符（不再限制为建议字符集）。
fn valid_idempotency_key(value: &str) -> bool {
    let length = value.chars().count();
    (1..=128).contains(&length)
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn json_body(payload: Result<Json<ApplyRequest>, JsonRejection>) -> Result<ApplyRequest, ApiError> {
    payload.map(|Json(value)| value).map_err(|rejection| {
        let status = rejection.status();
        if status == StatusCode::UNSUPPORTED_MEDIA_TYPE {
            return ApiError::unsupported_media_type();
        }
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            return ApiError::body_too_large(crate::api::BODY_LIMIT_BYTES);
        }
        let message = rejection.body_text();
        let code = if message.contains("unknown field") {
            "unknown_field"
        } else if message.contains("missing field") {
            "missing_required"
        } else if message.contains("invalid type") {
            "invalid_type"
        } else {
            "invalid_value"
        };
        ApiError::validation(vec![ValidationError::new("body", code, message)])
    })
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
}

/// 供 runner / 测试复用的超时。
pub fn request_timeout(state: &AppState) -> Duration {
    Duration::from_millis(state.cfg.request_timeout_ms)
}

/// 供 runner / 测试复用的写超时。
pub fn write_timeout(state: &AppState) -> Duration {
    Duration::from_millis(state.cfg.write_timeout_ms)
}

/// 把 job 结果错误对象统一成 API.md 第 5.7 节的形状。
pub fn error_payload(
    code: &str,
    message: &str,
    details: serde_json::Value,
    retryable: bool,
) -> serde_json::Value {
    json!({
        "code": code,
        "message": message,
        "details": details,
        "retryable": retryable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_package_names() {
        assert!(is_valid_package("com.xingin.xhs"));
        assert!(is_valid_package("com.tencent.mm@999"));
        assert!(is_valid_package("a.b"));
        assert!(!is_valid_package("com"));
        assert!(!is_valid_package("com..xhs"));
        assert!(!is_valid_package("1com.xhs"));
        assert!(!is_valid_package("com.xingin.xhs@"));
        assert!(!is_valid_package("com.xingin.xhs@1000"));
        assert!(!is_valid_package("com.xingin.xhs@a"));
    }

    #[test]
    fn validates_idempotency_keys() {
        assert!(valid_idempotency_key("abc"));
        assert!(valid_idempotency_key(&"x".repeat(128)));
        assert!(!valid_idempotency_key(""));
        assert!(!valid_idempotency_key("with space"));
        assert!(!valid_idempotency_key(&"x".repeat(129)));
    }

    #[test]
    fn validates_template_names() {
        assert!(is_valid_template_name("pixel_9"));
        assert!(is_valid_template_name("Pixel-9.pro"));
        assert!(!is_valid_template_name(""));
        assert!(!is_valid_template_name("-pixel"));
        assert!(!is_valid_template_name("pixel 9"));
        assert!(!is_valid_template_name(&"x".repeat(65)));
    }

    #[test]
    fn error_payload_shape() {
        let payload = error_payload("device_offline", "超时", json!({"reason": "timeout"}), true);
        assert_eq!(payload["code"], "device_offline");
        assert_eq!(payload["details"]["reason"], "timeout");
        assert_eq!(payload["retryable"], true);
    }
}
