// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! Bearer 鉴权中间件：常量时间比较，失败写审计并返回统一 401 信封。

use axum::{
    extract::{OriginalUri, Request, State},
    http::{Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use subtle::ConstantTimeEq;

use crate::{error::ApiError, ids, request_id, state::AppState};

/// 校验 Authorization: Bearer <manager-token>。
pub async fn require_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    // 唯一的公开端点是 GET /relay/api/v1/healthz（根路径 /healthz 不经过本中间件）。
    // nest 之后 request.uri() 是去掉前缀的路径，因此优先读 OriginalUri。
    let original_path = request
        .extensions()
        .get::<OriginalUri>()
        .map(|original| original.0.path().to_string());
    let path = original_path
        .as_deref()
        .unwrap_or_else(|| request.uri().path());
    let is_public_health =
        request.method() == Method::GET && (path == "/relay/api/v1/healthz" || path == "/healthz");
    if is_public_health {
        return next.run(request).await;
    }

    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_bearer);

    let current = state.token.read().await.value.clone();
    let authorized = provided
        .as_deref()
        .is_some_and(|token| constant_time_eq(token.as_bytes(), current.as_bytes()));

    if !authorized {
        let reason = if provided.is_some() {
            "mismatch"
        } else {
            "missing"
        };
        audit_failure(&state, request.uri().path(), reason).await;
        return ApiError::unauthorized().into_response();
    }

    next.run(request).await
}

fn parse_bearer(value: &str) -> Option<String> {
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        let _ = left.ct_eq(left);
        return false;
    }
    left.ct_eq(right).into()
}

async fn audit_failure(state: &AppState, path: &str, reason: &str) {
    let detail = serde_json::json!({ "path": path, "reason": reason });
    if let Err(error) = state
        .db
        .insert_audit(
            "auth.failed",
            Some("token"),
            None,
            request_id::current().as_deref(),
            "failed",
            Some(detail),
            &ids::now_rfc3339(),
        )
        .await
    {
        tracing::warn!(error = %error, "写入 auth.failed 审计失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bearer_header() {
        assert_eq!(parse_bearer("Bearer abc").as_deref(), Some("abc"));
        assert_eq!(parse_bearer("bearer  abc  ").as_deref(), Some("abc"));
        assert!(parse_bearer("Basic abc").is_none());
        assert!(parse_bearer("Bearer").is_none());
        assert!(parse_bearer("Bearer   ").is_none());
    }

    #[test]
    fn constant_time_comparison_matches_equality() {
        assert!(constant_time_eq(b"admin123", b"admin123"));
        assert!(!constant_time_eq(b"admin123", b"admin124"));
        assert!(!constant_time_eq(b"admin123", b"admin"));
        assert!(constant_time_eq(b"", b""));
    }
}
