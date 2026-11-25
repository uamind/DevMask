// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 统一错误信封（docs/API.md 7）。
//!
//! 所有 4xx / 5xx 只有一个格式：
//!
//! ```json
//! {"error":{"code":"unauthorized","message":"...","details":null,"retryable":false,"requestId":"req_..."}}
//! ```
//!
//! 错误码是稳定契约：客户端按 code 分支，不解析 message。

use axum::{
    Json,
    http::{HeaderName, HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// 契约冻结的错误码（docs/API.md 7.2；egress 增量 +2，docs/plans/egress-contract.md §1.8）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Unauthorized,
    ForbiddenScope,
    NotFound,
    MethodNotAllowed,
    ConfirmationRequired,
    ConflictDuplicatePackage,
    ConfigRevisionMismatch,
    PreconditionRequired,
    ValidationFailed,
    BodyTooLarge,
    UnsupportedMediaType,
    RateLimited,
    InternalError,
    UpstreamUnavailable,
    EgressNotConfigured,
    EgressProbeFailed,
}

impl ErrorCode {
    /// 全部错误码：用于契约自检（错误码表与实现必须一致）。
    pub const ALL: [ErrorCode; 16] = [
        ErrorCode::Unauthorized,
        ErrorCode::ForbiddenScope,
        ErrorCode::NotFound,
        ErrorCode::MethodNotAllowed,
        ErrorCode::ConfirmationRequired,
        ErrorCode::ConflictDuplicatePackage,
        ErrorCode::ConfigRevisionMismatch,
        ErrorCode::PreconditionRequired,
        ErrorCode::ValidationFailed,
        ErrorCode::BodyTooLarge,
        ErrorCode::UnsupportedMediaType,
        ErrorCode::RateLimited,
        ErrorCode::InternalError,
        ErrorCode::UpstreamUnavailable,
        ErrorCode::EgressNotConfigured,
        ErrorCode::EgressProbeFailed,
    ];

    /// 稳定的线上错误码。
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::ForbiddenScope => "forbidden_scope",
            ErrorCode::NotFound => "not_found",
            ErrorCode::MethodNotAllowed => "method_not_allowed",
            ErrorCode::ConfirmationRequired => "confirmation_required",
            ErrorCode::ConflictDuplicatePackage => "conflict_duplicate_package",
            ErrorCode::ConfigRevisionMismatch => "config_revision_mismatch",
            ErrorCode::PreconditionRequired => "precondition_required",
            ErrorCode::ValidationFailed => "validation_failed",
            ErrorCode::BodyTooLarge => "body_too_large",
            ErrorCode::UnsupportedMediaType => "unsupported_media_type",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::InternalError => "internal_error",
            ErrorCode::UpstreamUnavailable => "upstream_unavailable",
            // egress 领域用点分命名空间（刻意与既有扁平码不同，见契约「阅读约定」）
            ErrorCode::EgressNotConfigured => "egress.not_configured",
            ErrorCode::EgressProbeFailed => "egress.probe_failed",
        }
    }

    /// 对应的 HTTP 状态码。
    pub const fn status(self) -> StatusCode {
        match self {
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::ForbiddenScope => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            ErrorCode::ConfirmationRequired => StatusCode::BAD_REQUEST,
            ErrorCode::ConflictDuplicatePackage => StatusCode::CONFLICT,
            ErrorCode::ConfigRevisionMismatch => StatusCode::PRECONDITION_FAILED,
            ErrorCode::PreconditionRequired => StatusCode::PRECONDITION_REQUIRED,
            ErrorCode::ValidationFailed => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::UpstreamUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::EgressNotConfigured => StatusCode::CONFLICT,
            ErrorCode::EgressProbeFailed => StatusCode::BAD_GATEWAY,
        }
    }

    /// 同一请求原样重试是否可能成功。
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::RateLimited
                | ErrorCode::InternalError
                | ErrorCode::UpstreamUnavailable
                | ErrorCode::EgressProbeFailed
        )
    }
}

/// 带统一信封的错误。
#[derive(Debug, Clone)]
pub struct ApiError {
    code: ErrorCode,
    message: String,
    details: Option<Value>,
    request_id: String,
}

impl ApiError {
    /// 构造错误；request_id 会同时出现在响应体与 X-Request-Id 头。
    pub fn new(request_id: impl Into<String>, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
            request_id: request_id.into(),
        }
    }

    /// 附加结构化 details。
    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// 401：缺少 / 格式错误 / 未知 Token（不区分原因，避免枚举）。
    pub fn unauthorized(request_id: &str) -> Self {
        Self::new(
            request_id,
            ErrorCode::Unauthorized,
            "缺少或无效的 Bearer Token",
        )
    }

    /// 403：作用域不足。
    pub fn forbidden_scope(request_id: &str, required: &str, granted: &[String]) -> Self {
        Self::new(request_id, ErrorCode::ForbiddenScope, "Token 作用域不足")
            .with_details(json!({ "required": required, "granted": granted }))
    }

    /// 404：资源或路径不存在。
    pub fn not_found(request_id: &str, resource: &str, name: &str) -> Self {
        Self::new(request_id, ErrorCode::NotFound, "资源不存在")
            .with_details(json!({ "resource": resource, "name": name }))
    }

    /// 405：路径存在但不支持该方法。
    pub fn method_not_allowed(request_id: &str, method: &str, path: &str) -> Self {
        Self::new(
            request_id,
            ErrorCode::MethodNotAllowed,
            "路径存在但不支持该 HTTP 方法",
        )
        .with_details(json!({ "method": method, "path": path }))
    }

    /// 428：写请求缺少 If-Match。
    pub fn precondition_required(request_id: &str) -> Self {
        Self::new(
            request_id,
            ErrorCode::PreconditionRequired,
            "写请求必须携带 If-Match（当前配置 revision；首次创建可用 *）",
        )
        .with_details(json!({ "header": "If-Match" }))
    }

    /// 412：If-Match 与当前 revision 不符，服务端不自动合并。
    pub fn config_revision_mismatch(
        request_id: &str,
        expected: &str,
        actual: Option<&str>,
    ) -> Self {
        Self::new(
            request_id,
            ErrorCode::ConfigRevisionMismatch,
            "配置已被其他客户端修改",
        )
        .with_details(json!({
            "expected": expected,
            "actual": actual,
        }))
    }

    /// 400：破坏性操作缺少 confirm=true。
    pub fn confirmation_required(request_id: &str, action: &str) -> Self {
        Self::new(
            request_id,
            ErrorCode::ConfirmationRequired,
            "破坏性操作需要 confirm=true",
        )
        .with_details(json!({ "action": action }))
    }

    /// 409：会写出重复包名，需要 allow_duplicate_package=true。
    pub fn conflict_duplicate_package(
        request_id: &str,
        package: &str,
        templates: &[String],
    ) -> Self {
        Self::new(
            request_id,
            ErrorCode::ConflictDuplicatePackage,
            format!("{package} 已被其它模板声明"),
        )
        .with_details(json!({ "package": package, "templates": templates }))
    }

    /// 422：details 里带 errors 数组。
    pub fn validation_failed(request_id: &str, errors: serde_json::Value) -> Self {
        Self::new(
            request_id,
            ErrorCode::ValidationFailed,
            "请求体或配置语义校验失败",
        )
        .with_details(json!({ "errors": errors }))
    }

    /// 415：Content-Type 不支持。
    pub fn unsupported_media_type(request_id: &str, supported: &[&str]) -> Self {
        Self::new(
            request_id,
            ErrorCode::UnsupportedMediaType,
            "不支持的 Content-Type",
        )
        .with_details(json!({ "supported": supported }))
    }

    /// 413：请求体超过上限。
    pub fn body_too_large(request_id: &str, limit_bytes: usize) -> Self {
        Self::new(
            request_id,
            ErrorCode::BodyTooLarge,
            format!("请求体超过 {limit_bytes} 字节上限"),
        )
        .with_details(json!({ "limitBytes": limit_bytes }))
    }
    /// 500：未预期错误。
    pub fn internal_error(request_id: &str, message: impl Into<String>) -> Self {
        Self::new(request_id, ErrorCode::InternalError, message)
    }

    /// 503：系统命令（pm / am / getprop）不可用。
    pub fn upstream_unavailable(request_id: &str, component: &str) -> Self {
        Self::new(
            request_id,
            ErrorCode::UpstreamUnavailable,
            format!("{component} 命令执行失败或超时"),
        )
        .with_details(json!({ "component": component }))
    }

    /// 409：没有任何可用出口（egress.not_configured，契约 §1.8）。
    pub fn egress_not_configured(request_id: &str, scope: &str, package: Option<&str>) -> Self {
        let mut details = json!({ "scope": scope });
        if let Some(package) = package {
            details["package"] = json!(package);
        }
        Self::new(
            request_id,
            ErrorCode::EgressNotConfigured,
            "没有任何可用的出口代理，无法按当前链路探测",
        )
        .with_details(details)
    }

    /// 502：探测失败（egress.probe_failed）。details 固定四个键（契约 §4.4）。
    pub fn egress_probe_failed(
        request_id: &str,
        url: &str,
        reason: &str,
        timeout_ms: u64,
        upstream_status: Option<u16>,
    ) -> Self {
        Self::new(
            request_id,
            ErrorCode::EgressProbeFailed,
            format!("出口探测失败（{reason}）"),
        )
        .with_details(json!({
            "url": url,
            "reason": reason,
            "timeoutMs": timeout_ms,
            "upstreamStatus": upstream_status,
        }))
    }

    /// 412：egress revision 不匹配。
    ///
    /// 复用既有 config_revision_mismatch（Manager 的「412 重试一次」逻辑无需分支），
    /// details 追加 resource="egress" 让客户端能区分是哪个 revision 过期。
    pub fn egress_revision_mismatch(
        request_id: &str,
        expected: &str,
        actual: Option<&str>,
    ) -> Self {
        Self::new(
            request_id,
            ErrorCode::ConfigRevisionMismatch,
            "egress 已被其他客户端修改",
        )
        .with_details(json!({
            "expected": expected,
            "actual": actual,
            "resource": "egress",
        }))
    }

    /// 错误码。
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// requestId（与 X-Request-Id 头一致）。
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} ({}): {}",
            self.code.as_str(),
            self.code.status(),
            self.message
        )
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "code": self.code.as_str(),
                "message": self.message,
                "details": self.details,
                "retryable": self.code.retryable(),
                "requestId": self.request_id,
            }
        });
        let mut response = (self.code.status(), Json(body)).into_response();
        let headers = response.headers_mut();
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if let Ok(value) = HeaderValue::from_str(&self.request_id) {
            headers.insert(X_REQUEST_ID, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_table_matches_contract() {
        let codes: Vec<&str> = ErrorCode::ALL.iter().map(|code| code.as_str()).collect();
        assert_eq!(
            codes,
            vec![
                "unauthorized",
                "forbidden_scope",
                "not_found",
                "method_not_allowed",
                "confirmation_required",
                "conflict_duplicate_package",
                "config_revision_mismatch",
                "precondition_required",
                "validation_failed",
                "body_too_large",
                "unsupported_media_type",
                "rate_limited",
                "internal_error",
                "upstream_unavailable",
                "egress.not_configured",
                "egress.probe_failed",
            ]
        );
        for code in ErrorCode::ALL {
            assert!(!code.as_str().is_empty());
        }
    }

    #[test]
    fn retryable_codes_are_only_429_500_502_503() {
        for code in ErrorCode::ALL {
            let expected = matches!(
                code.as_str(),
                "rate_limited" | "internal_error" | "upstream_unavailable" | "egress.probe_failed"
            );
            assert_eq!(code.retryable(), expected, "{}", code.as_str());
        }
    }

    #[test]
    fn egress_error_shapes_match_contract() {
        let not_configured =
            ApiError::egress_not_configured("req_egress", "app", Some("com.example.a"));
        assert_eq!(not_configured.code().status(), StatusCode::CONFLICT);
        let response = not_configured.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let mismatch = ApiError::egress_revision_mismatch("req_egress", "abc", Some("def"));
        assert_eq!(mismatch.code().as_str(), "config_revision_mismatch");
        assert_eq!(mismatch.code().status(), StatusCode::PRECONDITION_FAILED);

        let failed = ApiError::egress_probe_failed(
            "req_egress",
            "http://checkip.amazonaws.com/",
            "connect",
            5000,
            None,
        );
        assert_eq!(failed.code().as_str(), "egress.probe_failed");
        assert_eq!(failed.code().status(), StatusCode::BAD_GATEWAY);
        assert!(failed.code().retryable());
    }

    #[test]
    fn envelope_shape() {
        let error = ApiError::forbidden_scope("req_test", "read", &["write".to_string()]);
        assert_eq!(error.code(), ErrorCode::ForbiddenScope);
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");
        assert_eq!(response.headers().get(&X_REQUEST_ID).unwrap(), "req_test");
    }
}
