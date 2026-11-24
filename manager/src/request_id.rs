// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! request id 中间件：生成 / 回填 X-Request-Id，并通过 task-local 暴露给错误响应。

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use tokio::task_local;

use crate::ids;

/// 响应与错误信封共用的请求 id 头。
pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// 一次请求内的 request id。
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

task_local! {
    static CURRENT: String;
}

/// 当前请求的 request id；不在请求作用域内时为 None。
pub fn current() -> Option<String> {
    CURRENT.try_with(|value| value.clone()).ok()
}

/// 在 task-local 作用域中运行 future。
pub async fn scope<F>(id: String, future: F) -> F::Output
where
    F: std::future::Future,
{
    CURRENT.scope(id, future).await
}

/// 中间件：接受客户端 X-Request-Id（校验字符集），否则生成新值；
/// 请求处理结束后把同一个值写回响应头。
pub async fn middleware(mut request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| ids::is_valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(ids::new_request_id);

    request.extensions_mut().insert(RequestId(id.clone()));

    let mut response = scope(id.clone(), next.run(request)).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(HEADER, value);
    }
    response
}
