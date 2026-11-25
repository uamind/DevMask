// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 通用请求中间件：X-Request-Id 与 API 的 no-store。
//!
//! - 客户端可用 X-Request-Id（<=128 字符，[A-Za-z0-9._-]）关联日志；合法则原样回填，
//!   否则服务端生成 req_<24 chars>（docs/API.md 2.4）；
//! - 所有 /api/v1/* 响应（含错误）带 Cache-Control: no-store（docs/API.md 2.1）。

use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue, StatusCode, header::CACHE_CONTROL},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{error::ApiError, util};

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
/// 客户端提供的 request id 最大长度。
const MAX_REQUEST_ID_LEN: usize = 128;

/// 请求标识：同时用于响应头与错误信封。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestId(String);

impl RequestId {
    /// 生成新的 request id。
    pub fn generate() -> Self {
        Self(util::generate_request_id())
    }

    /// 从请求头解析；缺失或不合法时生成新的。
    pub fn from_header(value: Option<&HeaderValue>) -> Self {
        if let Some(value) = value
            && let Ok(text) = value.to_str()
            && is_valid(text)
        {
            return Self(text.to_owned());
        }
        Self::generate()
    }

    /// 字符串形式。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_ID_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// 从请求上下文取 request id（外层中间件已注入；兜底生成，避免 panic）。
pub fn extract(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .cloned()
        .unwrap_or_else(RequestId::generate)
}

/// 中间件：注入 RequestId 扩展、回填响应头、给 API 响应加 no-store。
pub async fn attach(mut request: Request, next: Next) -> Response {
    let request_id = RequestId::from_header(request.headers().get(&X_REQUEST_ID));
    let is_api = request.uri().path().starts_with("/api/v1");
    request.extensions_mut().insert(request_id.clone());

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(request_id.as_str()) {
        headers.insert(X_REQUEST_ID, value);
    }
    if is_api {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// 把 axum 的 413 纯文本响应改成契约要求的 `body_too_large` 错误信封。
pub async fn envelope_oversize(
    State(limit_bytes): State<usize>,
    request: Request,
    next: Next,
) -> Response {
    let request_id = extract(&request);
    let response = next.run(request).await;
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::body_too_large(request_id.as_str(), limit_bytes).into_response();
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn accepts_valid_client_id() {
        let value = HeaderValue::from_static("client.req-01_A");
        assert_eq!(
            RequestId::from_header(Some(&value)).as_str(),
            "client.req-01_A"
        );
    }

    #[test]
    fn rejects_invalid_client_id() {
        let too_long = HeaderValue::from_str(&"a".repeat(MAX_REQUEST_ID_LEN + 1)).unwrap();
        assert!(
            RequestId::from_header(Some(&too_long))
                .as_str()
                .starts_with("req_")
        );
        let invalid = HeaderValue::from_static("bad id with space");
        assert!(
            RequestId::from_header(Some(&invalid))
                .as_str()
                .starts_with("req_")
        );
        assert!(RequestId::from_header(None).as_str().starts_with("req_"));
    }
}
