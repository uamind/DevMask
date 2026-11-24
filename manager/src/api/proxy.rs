// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! ANY /devices/{id}/proxy/api/v1/{*path}：带设备 Token 的单设备透传。

use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    extract::{OriginalUri, Path, Request, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::Response,
};
use reqwest::Method;
use serde_json::Value;

use crate::{
    api::BODY_LIMIT_BYTES,
    device_client::{DeviceCallError, OfflineReason, ProxyResponse},
    error::ApiError,
    request_id,
    state::AppState,
};

const ALLOWED_HEADERS: [&str; 6] = [
    "content-type",
    "accept",
    "if-match",
    "idempotency-key",
    "x-request-id",
    "range",
];

/// ANY /relay/api/v1/devices/{id}/proxy/api/v1/{*path}
pub async fn proxy(
    State(state): State<AppState>,
    OriginalUri(original): OriginalUri,
    Path((device_id, path)): Path<(String, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    let _decoded_path = sanitize_path(&path)?;
    let prefix = format!("/relay/api/v1/devices/{device_id}/proxy/api/v1/");
    let raw_path = original
        .path()
        .strip_prefix(&prefix)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ApiError::not_found("route", &path))?;

    let device = state
        .db
        .get_device(&device_id)
        .await
        .map_err(|error| internal("proxy.device", error))?
        .ok_or_else(|| ApiError::device_not_found(&device_id))?;
    let token = state
        .key
        .decrypt(&device_id, &device.token_cipher)
        .map_err(|error| internal("proxy.decrypt", error))?;

    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, BODY_LIMIT_BYTES)
        .await
        .map_err(|_| ApiError::body_too_large(BODY_LIMIT_BYTES))?;
    let method = Method::from_bytes(parts.method.as_str().as_bytes())
        .map_err(|_| ApiError::method_not_allowed(parts.method.as_str(), parts.uri.path()))?;

    let mut extra_headers: Vec<(String, String)> = Vec::new();
    for name in ALLOWED_HEADERS {
        if let Some(value) = parts.headers.get(name)
            && let Ok(value) = value.to_str()
        {
            extra_headers.push((name.to_string(), value.to_string()));
        }
    }

    let timeout = if matches!(method, Method::GET | Method::HEAD) {
        Duration::from_millis(state.cfg.request_timeout_ms)
    } else {
        Duration::from_millis(state.cfg.write_timeout_ms)
    };
    let query = parts.uri.query().map(str::to_owned);
    let request_id = request_id::current();

    match state
        .http
        .proxy(
            &device.base_url,
            token.as_str(),
            method,
            &raw_path,
            query.as_deref(),
            &extra_headers,
            Some(bytes.to_vec()),
            timeout,
            request_id.as_deref(),
        )
        .await
    {
        Ok(response) => {
            if response.status == 401 || response.status == 403 {
                let code = upstream_error_code(&response.body).unwrap_or_else(|| {
                    if response.status == 401 {
                        "unauthorized".to_string()
                    } else {
                        "forbidden_scope".to_string()
                    }
                });
                return Err(ApiError::device_error(&device_id, response.status, &code));
            }
            Ok(proxy_response(response))
        }
        Err(error) => Err(map_error(&device_id, &state, &error)),
    }
}

/// 拒绝 .. / . / 空段 / 反斜杠 / NUL，避免路径穿越。
fn sanitize_path(path: &str) -> Result<String, ApiError> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Err(ApiError::not_found("route", path));
    }
    for segment in trimmed.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains('\\')
            || segment.contains('\0')
        {
            return Err(ApiError::not_found("route", path));
        }
    }
    Ok(trimmed.to_string())
}

fn upstream_error_code(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn proxy_response(proxy: ProxyResponse) -> Response {
    let status = StatusCode::from_u16(proxy.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::new(Body::from(proxy.body));
    *response.status_mut() = status;
    let headers = response.headers_mut();

    let content_type = proxy.content_type.as_deref().unwrap_or("application/json");
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/json")),
    );
    response_header(headers, "x-df-revision", proxy.revision);
    response_header(headers, "etag", proxy.etag);
    response_header(headers, "retry-after", proxy.retry_after);
    response_header(
        headers,
        "x-df-upstream-request-id",
        proxy.upstream_request_id,
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn response_header(headers: &mut header::HeaderMap, name: &'static str, value: Option<String>) {
    let Some(value) = value else {
        return;
    };
    let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(&value) {
        headers.insert(name, value);
    }
}

fn map_error(device_id: &str, state: &AppState, error: &DeviceCallError) -> ApiError {
    match error {
        DeviceCallError::Offline {
            reason: OfflineReason::Timeout,
        } => ApiError::upstream_timeout(device_id, state.cfg.request_timeout_ms),
        DeviceCallError::Offline { reason } => ApiError::device_offline(device_id, reason.as_str()),
        DeviceCallError::TooLarge { limit } => {
            ApiError::device_response_too_large(device_id, *limit)
        }
        DeviceCallError::Decode { .. } => ApiError::device_error(device_id, 0, "invalid_response"),
        DeviceCallError::Http { status, code, .. } => {
            ApiError::device_error(device_id, *status, code)
        }
    }
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_segments() {
        assert!(sanitize_path("config/revision").is_ok());
        assert!(sanitize_path("../config").is_err());
        assert!(sanitize_path("apps/../secret").is_err());
        assert!(sanitize_path("apps//x").is_err());
        assert!(sanitize_path("apps\\x").is_err());
        assert!(sanitize_path("").is_err());
    }

    #[test]
    fn extracts_upstream_error_code() {
        let body = br#"{"error":{"code":"package_not_installed","message":"x"}}"#;
        assert_eq!(
            upstream_error_code(body).as_deref(),
            Some("package_not_installed")
        );
        assert!(upstream_error_code(b"not json").is_none());
    }
}
