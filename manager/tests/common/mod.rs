// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 集成测试公共设施：临时数据目录 + 进程内 Router + 请求助手 + Mock 设备。

#![allow(dead_code)]

pub mod mock_device;

use std::path::PathBuf;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode},
};
use devmask_manager::{
    AppState, build_router,
    config::Config,
    state::{TokenSource, TokenState},
};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

/// 测试用 Manager 实例。
pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub token: String,
    pub data_dir: std::path::PathBuf,
    pub _temp: TempDir,
}

/// 启动一个使用临时数据目录的 Manager（默认 10 s 设备超时）。
pub async fn start_app(static_dir: Option<PathBuf>) -> TestApp {
    start_app_with_timeouts(static_dir, 10_000).await
}

/// 指定设备请求超时的 Manager。
pub async fn start_app_with_timeouts(
    static_dir: Option<PathBuf>,
    request_timeout_ms: u64,
) -> TestApp {
    start_app_with_token_source(
        static_dir,
        request_timeout_ms,
        TokenState {
            value: "test-token".to_string(),
            source: TokenSource::Cli,
        },
    )
    .await
}

/// 指定 Manager Token 与来源的测试实例（token rotate 用例使用）。
pub async fn start_app_with_token(static_dir: Option<PathBuf>, token: TokenState) -> TestApp {
    start_app_with_token_source(static_dir, 10_000, token).await
}

async fn start_app_with_token_source(
    static_dir: Option<PathBuf>,
    request_timeout_ms: u64,
    token: TokenState,
) -> TestApp {
    let temp = tempfile::tempdir().expect("创建临时目录");
    let data_dir = temp.path().join("data");
    let candidates = static_dir.iter().cloned().collect();
    let token_value = token.value.clone();

    let config = Config {
        bind: "127.0.0.1:0".parse().expect("地址"),
        data_dir: data_dir.clone(),
        static_dir,
        static_dir_candidates: candidates,
        token,
        token_file: data_dir.join("manager.token"),
        key_file: data_dir.join("devmask-manager.key"),
        config_file: data_dir.join("manager.toml"),
        apply_concurrency: 4,
        sync_concurrency: 4,
        request_timeout_ms,
        write_timeout_ms: 15_000,
    };

    let state = devmask_manager::initialize(config)
        .await
        .expect("初始化 Manager");
    let router = build_router(state.clone());

    TestApp {
        router,
        state,
        token: token_value,
        data_dir,
        _temp: temp,
    }
}

/// 一次测试响应。
pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl TestResponse {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("响应应为 JSON")
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// 发送无请求体请求；token / request_id 为 None 时不附加对应头。
pub async fn send(
    app: &TestApp,
    method: Method,
    uri: &str,
    token: Option<&str>,
    request_id: Option<&str>,
) -> TestResponse {
    send_with_body(app, method, uri, token, request_id, None).await
}

/// 发送 JSON 请求体。
pub async fn send_json(
    app: &TestApp,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> TestResponse {
    let bytes = serde_json::to_vec(&body).expect("序列化请求体");
    send_with_body(app, method, uri, token, None, Some(bytes)).await
}

/// 发送带自定义头的 JSON 请求（幂等键等）。
pub async fn send_json_with_headers(
    app: &TestApp,
    method: Method,
    uri: &str,
    token: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: Value,
) -> TestResponse {
    let bytes = serde_json::to_vec(&body).expect("序列化请求体");
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .header("content-type", "application/json")
        .body(Body::from(bytes))
        .expect("构造请求");
    let response = app
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("Router 不应失败");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("读取响应体")
        .to_vec();
    TestResponse {
        status,
        headers,
        body,
    }
}

/// 发送原始字节体并指定 Content-Type（token rotate 异常 body 测试）。
pub async fn send_raw_with_content_type(
    app: &TestApp,
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: &[u8],
) -> TestResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let request = builder.body(Body::from(body.to_vec())).expect("构造请求");
    let response = app
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("Router 不应失败");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("读取响应体")
        .to_vec();
    TestResponse {
        status,
        headers,
        body,
    }
}

/// 发送任意 Content-Type 的原始请求体（rotate 畸形 body、非 JSON 用例）。
pub async fn send_raw_body(
    app: &TestApp,
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: &[u8],
) -> TestResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let request = builder.body(Body::from(body.to_vec())).expect("构造请求");
    let response = app
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("Router 不应失败");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("读取响应体")
        .to_vec();
    TestResponse {
        status,
        headers,
        body,
    }
}

/// 发送原始 JSON 文本（用于非法 / 未知字段用例）。
pub async fn send_raw_json(
    app: &TestApp,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: &str,
) -> TestResponse {
    send_with_body(
        app,
        method,
        uri,
        token,
        None,
        Some(body.as_bytes().to_vec()),
    )
    .await
}

async fn send_with_body(
    app: &TestApp,
    method: Method,
    uri: &str,
    token: Option<&str>,
    request_id: Option<&str>,
    body: Option<Vec<u8>>,
) -> TestResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(request_id) = request_id {
        builder = builder.header("x-request-id", request_id);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }

    let request = builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .expect("构造请求");
    let response = app
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("Router 不应失败");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("读取响应体")
        .to_vec();

    TestResponse {
        status,
        headers,
        body,
    }
}
