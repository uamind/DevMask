// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 统一错误信封：所有 4xx / 5xx 的唯一样式（见 manager/docs/API.md 第 2.6 节）。

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::request_id;

/// 请求体 / 查询参数校验错误项（details.errors[]）。
#[derive(Debug, Clone, Serialize)]
pub struct ValidationError {
    pub path: String,
    pub code: String,
    pub message: String,
}

impl ValidationError {
    pub fn new(
        path: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            code: code.into(),
            message: message.into(),
        }
    }
}

/// 一次失败请求的结构化描述；IntoResponse 时补上 requestId。
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub details: Option<Value>,
    pub retryable: bool,
}

impl ApiError {
    fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        details: Option<Value>,
        retryable: bool,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details,
            retryable,
        }
    }

    /// 401：缺失 / 格式错误 / 无效 Manager Token。
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "缺少或无效的 Manager Token",
            None,
            false,
        )
    }

    /// 403：Token 作用域不足（本增量保留，单一 Token 拥有全部权限）。
    pub fn forbidden_scope(required: &str, granted: &[&str]) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden_scope",
            "Token 作用域不足",
            Some(json!({ "required": required, "granted": granted })),
            false,
        )
    }

    /// 404：通用资源不存在。
    pub fn not_found(resource: &str, name: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("{resource} 不存在：{name}"),
            Some(json!({ "resource": resource, "name": name })),
            false,
        )
    }

    /// 405：路径存在但方法不支持。
    pub fn method_not_allowed(method: &str, path: &str) -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            format!("{path} 不支持 {method}"),
            Some(json!({ "method": method, "path": path })),
            false,
        )
    }

    /// 404：设备未注册 / 已移除。
    pub fn device_not_found(device_id: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "device_not_found",
            format!("设备未注册：{device_id}"),
            Some(json!({ "deviceId": device_id })),
            false,
        )
    }

    /// 409：baseUrl 或 id 已被占用。
    pub fn device_exists(field: &str, value: &str, existing_id: &str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "device_exists",
            format!("{field} 已被设备 {existing_id} 使用"),
            Some(json!({ "field": field, "value": value, "deviceId": existing_id })),
            false,
        )
    }

    /// 422：请求体 / 查询参数校验失败。
    pub fn validation(errors: Vec<ValidationError>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "请求参数不合法",
            Some(json!({ "errors": errors })),
            false,
        )
    }

    /// 413：请求体超过 1 MiB。
    pub fn body_too_large(limit_bytes: usize) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "请求体超过上限",
            Some(json!({ "limitBytes": limit_bytes })),
            false,
        )
    }

    /// 415：不支持的 Content-Type。
    pub fn unsupported_media_type() -> Self {
        Self::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "仅支持 application/json",
            Some(json!({ "supported": ["application/json"] })),
            false,
        )
    }

    /// 409：当前配置不允许该操作（如 Token 被 CLI / env 固定）。
    pub fn invalid_state(reason: &str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "invalid_state",
            "当前配置不允许该操作",
            Some(json!({ "reason": reason })),
            false,
        )
    }

    /// 500：未预期错误；requestId 用于查日志。
    pub fn internal(op: &str) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "服务内部错误",
            Some(json!({ "op": op })),
            true,
        )
    }

    /// 422：添加 / 修改设备时探测失败。
    pub fn device_probe_failed(
        base_url: &str,
        reason: &str,
        upstream: Option<Value>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "device_probe_failed",
            message,
            Some(json!({ "baseUrl": base_url, "reason": reason, "upstream": upstream })),
            false,
        )
    }

    /// 502：设备返回非预期响应。
    pub fn device_error(device_id: &str, upstream_status: u16, upstream_code: &str) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "device_error",
            format!("设备返回错误（HTTP {upstream_status}）"),
            Some(json!({
                "deviceId": device_id,
                "upstreamStatus": upstream_status,
                "upstreamCode": upstream_code,
            })),
            true,
        )
    }

    /// 504：设备请求超时。
    pub fn upstream_timeout(device_id: &str, timeout_ms: u64) -> Self {
        Self::new(
            StatusCode::GATEWAY_TIMEOUT,
            "upstream_timeout",
            format!("设备请求超时（{timeout_ms} ms）"),
            Some(json!({ "deviceId": device_id, "timeoutMs": timeout_ms })),
            true,
        )
    }

    /// 502：设备响应超过代理上限。
    pub fn device_response_too_large(device_id: &str, limit: usize) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "device_error",
            format!("设备响应超过 {limit} 字节上限"),
            Some(json!({
                "deviceId": device_id,
                "reason": "response_too_large",
                "limitBytes": limit,
            })),
            true,
        )
    }

    /// 409：代理名已被占用。
    pub fn egress_proxy_exists(name: &str, id: &str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "egress_proxy_exists",
            format!("代理名已被占用：{name}"),
            Some(json!({ "field": "name", "value": name, "id": id })),
            false,
        )
    }

    /// 409：代理仍被绑定引用，拒绝删除。
    pub fn egress_proxy_in_use(
        proxy_id: &str,
        name: &str,
        total: i64,
        devices: Vec<String>,
        apps: Vec<String>,
    ) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "egress_proxy_in_use",
            format!("代理 {name} 仍被 {total} 条绑定引用"),
            Some(json!({
                "proxyId": proxy_id,
                "name": name,
                "total": total,
                "devices": devices,
                "apps": apps,
            })),
            false,
        )
    }

    /// 502：Manager 侧经代理探测出口失败。
    pub fn egress_probe_failed(
        proxy_id: &str,
        url: &str,
        reason: &str,
        timeout_ms: u64,
        upstream_status: Option<u16>,
    ) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "egress_probe_failed",
            format!("经代理探测出口失败（{reason}）"),
            Some(json!({
                "proxyId": proxy_id,
                "url": url,
                "reason": reason,
                "timeoutMs": timeout_ms,
                "upstreamStatus": upstream_status,
            })),
            true,
        )
    }

    /// 502：设备不可达。
    pub fn device_offline(device_id: &str, reason: &str) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "device_offline",
            format!("设备不可达：{device_id}"),
            Some(json!({ "deviceId": device_id, "reason": reason })),
            true,
        )
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}): {}",
            self.code,
            self.status.as_u16(),
            self.message
        )
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let request_id = request_id::current().unwrap_or_else(crate::ids::new_request_id);
        let body = json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "details": self.details,
                "retryable": self.retryable,
                "requestId": request_id,
            }
        });

        let mut response = (self.status, Json(body)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}
