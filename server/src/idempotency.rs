// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! Idempotency-Key 重放（docs/API.md 2.8）。
//!
//! 只缓存 2xx：同键重放在 24h 内直接返回首次的状态码与响应体，并加
//! `Idempotent-Replay: true`；失败的请求不缓存（客户端可以安全重试）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;

use crate::{app::AppState, auth::AuthContext, error::ApiError, params, requests};

/// 重放保留时长。
pub const TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// 键最大长度。
pub const MAX_KEY_LEN: usize = 128;

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");
const IDEMPOTENT_REPLAY: HeaderName = HeaderName::from_static("idempotent-replay");

#[derive(Debug)]
struct Cached {
    at: Instant,
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

/// 进程内重放缓存。
#[derive(Debug, Default)]
pub struct IdempotencyStore {
    entries: Mutex<HashMap<String, Cached>>,
}

impl IdempotencyStore {
    fn get(&self, key: &str) -> Option<(StatusCode, HeaderMap, Bytes)> {
        let mut guard = self.entries.lock().expect("idempotency cache poisoned");
        guard.retain(|_, cached| cached.at.elapsed() < TTL);
        guard
            .get(key)
            .map(|cached| (cached.status, cached.headers.clone(), cached.body.clone()))
    }

    fn put(&self, key: String, status: StatusCode, headers: HeaderMap, body: Bytes) {
        let mut guard = self.entries.lock().expect("idempotency cache poisoned");
        guard.retain(|_, cached| cached.at.elapsed() < TTL);
        guard.insert(
            key,
            Cached {
                at: Instant::now(),
                status,
                headers,
                body,
            },
        );
    }

    /// 当前缓存条数（测试用）。
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("idempotency cache poisoned")
            .len()
    }

    /// 缓存是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// write 路由组的幂等中间件（放在权限守卫之内、审计之外）。
pub async fn middleware(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(raw) = request.headers().get(&IDEMPOTENCY_KEY) else {
        return next.run(request).await;
    };
    let request_id = requests::extract(&request);
    let key = match raw.to_str() {
        Ok(value) if value.trim().is_empty() => {
            return params::field_error(
                request_id.as_str(),
                "Idempotency-Key",
                "invalid_value",
                "Idempotency-Key 不能为空",
            )
            .into_response();
        }
        Ok(value) if value.len() > MAX_KEY_LEN => {
            return ApiError::validation_failed(
                request_id.as_str(),
                serde_json::json!([crate::dto::ValidationError::new(
                    "Idempotency-Key",
                    "invalid_value",
                    format!("Idempotency-Key 不超过 {MAX_KEY_LEN} 字符"),
                )
                .to_value()]),
            )
            .into_response();
        }
        Ok(value) => value.trim().to_owned(),
        Err(_) => {
            return ApiError::validation_failed(
                request_id.as_str(),
                serde_json::json!([crate::dto::ValidationError::new(
                    "Idempotency-Key",
                    "invalid_value",
                    "Idempotency-Key 必须是可见 ASCII",
                )
                .to_value()]),
            )
            .into_response();
        }
    };

    let token_id = request
        .extensions()
        .get::<AuthContext>()
        .map(|auth| auth.token_id.clone())
        .unwrap_or_default();
    let cache_key = format!(
        "{token_id}:{}:{}:{key}",
        request.method(),
        request.uri().path()
    );

    if let Some((status, headers, body)) = state.idempotency().get(&cache_key) {
        let mut response = Response::builder()
            .status(status)
            .body(Body::from(body))
            .expect("cached response is valid");
        *response.headers_mut() = headers;
        response
            .headers_mut()
            .insert(IDEMPOTENT_REPLAY, HeaderValue::from_static("true"));
        return response;
    }

    let response = next.run(request).await;
    if !response.status().is_success() {
        return response;
    }
    let (parts, body) = response.into_parts();
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };
    let mut headers = parts.headers.clone();
    headers.remove(axum::http::header::CONTENT_LENGTH);
    state
        .idempotency()
        .put(cache_key, parts.status, headers, bytes.clone());
    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_replays_first_result() {
        let store = IdempotencyStore::default();
        assert!(store.get("k").is_none());
        store.put(
            "k".to_owned(),
            StatusCode::OK,
            HeaderMap::new(),
            Bytes::from_static(b"{}"),
        );
        let (status, _, body) = store.get("k").expect("cached");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, Bytes::from_static(b"{}"));
        assert_eq!(store.len(), 1);
    }
}
