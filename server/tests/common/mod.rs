// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 集成测试共用的 fixture：临时 webroot / config.toml / module.prop / Token 表 / 命令执行器。
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode},
    response::Response,
};
use std::sync::Arc;

use devmask_server::{
    app::{AppState, router},
    auth::{Scope, TokenEntry},
    commands::{CommandRunner, FixtureRunner},
    device::DeviceSummary,
    settings::Settings,
    util,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::util::ServiceExt;

/// 只读 Token 明文。
pub const READ_TOKEN: &str = "df_test_read_token";
/// 只写 Token 明文（用于验证作用域不隐含）。
pub const WRITE_TOKEN: &str = "df_test_write_token";
/// admin Token 明文。
pub const ADMIN_TOKEN: &str = "df_test_admin_token";
/// ops Token 明文（只能重启应用）。
pub const OPS_TOKEN: &str = "df_test_ops_token";

/// 测试夹具。
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub state: AppState,
    pub router: Router,
    pub config_path: PathBuf,
    pub config_text: String,
    pub audit_path: PathBuf,
    pub adb_dir: PathBuf,
}

/// 样例 config.toml：2 个模板 + 1 条内联应用，com.example.shared 被两个模板声明（conflicts = 1）。
pub const CONFIG_TEXT: &str = "
# 测试用配置，注释必须能保留（结构化写路径只改目标键）
[templates.pixel_9]
packages = [\"com.example.app\", \"com.example.shared\"]
brand = \"google\"
model = \"Pixel 9\"
dpi = 420
author = \"tester\"
xyz_unknown = 1

[templates.pixel_9_pro]
packages = [\"com.example.shared\"]
model = \"Pixel 9 Pro\"

[[apps]]
package = \"com.example.inline\"
model = \"2509FPN0BC\"
";

/// module.prop 内容。
pub const MODULE_PROP: &str =
    "id=devmask\nname=DevMask\nversion=v1.7.0\nversionCode=20261109\nauthor=test\n";

/// index.html（带 <head>，用于验证注入位置）。
pub const INDEX_HTML: &str = "<!DOCTYPE html>\n<html lang=\"zh-CN\"><head><meta charset=\"UTF-8\"><title>DevMask</title></head><body><div id=\"app\"></div></body></html>\n";

fn token_entry(token: &str, scopes: Vec<Scope>) -> TokenEntry {
    TokenEntry::new(
        "default",
        "test",
        &util::sha256_hex(token.as_bytes()),
        scopes,
    )
}

/// 构造默认夹具（有 config.toml、有 webroot、无 pm fixture）。
pub fn fixture() -> Fixture {
    fixture_with_runner(Some(CONFIG_TEXT), FixtureRunner::new())
}

/// 构造夹具；config_text = None 表示配置文件不存在。
pub fn fixture_with_config(config_text: Option<&str>) -> Fixture {
    fixture_with_runner(config_text, FixtureRunner::new())
}

/// 构造夹具并指定命令执行器。
pub fn fixture_with_runner(config_text: Option<&str>, runner: FixtureRunner) -> Fixture {
    build_fixture(config_text, Arc::new(runner), |_| {})
}

/// 构造夹具并允许改设置（body_limit、audit_log、adb_dir 等）。
pub fn fixture_with_settings(
    config_text: Option<&str>,
    runner: FixtureRunner,
    customize: impl FnOnce(&mut Settings),
) -> Fixture {
    build_fixture(config_text, Arc::new(runner), customize)
}

/// 构造**允许真实绑定 relay 端口**的夹具（host 测试专用）。
///
/// 默认夹具不绑定端口（`AppState::with_runner`），避免并行测试抢 17800–17899；
/// 需要验证 listener / 数据面的测试用这个构造函数，并自行串行化（见 relay_api.rs）。
pub fn fixture_with_relay_and_runner(
    config_text: Option<&str>,
    runner: Arc<dyn CommandRunner>,
) -> Fixture {
    build_fixture_with_relay(config_text, runner, true, |_| {})
}

/// 构造夹具并注入**共享**的命令执行器。
///
/// 测试侧需要知道「写了几次 settings」或模拟读回值时必须拿到同一个 Arc
/// （egress 的幂等/读回校验断言）。
pub fn fixture_with_arc_runner(
    config_text: Option<&str>,
    runner: Arc<dyn CommandRunner>,
) -> Fixture {
    build_fixture(config_text, runner, |_| {})
}

fn build_fixture(
    config_text: Option<&str>,
    runner: Arc<dyn CommandRunner>,
    customize: impl FnOnce(&mut Settings),
) -> Fixture {
    build_fixture_with_relay(config_text, runner, false, customize)
}

fn build_fixture_with_relay(
    config_text: Option<&str>,
    runner: Arc<dyn CommandRunner>,
    relay_binding: bool,
    customize: impl FnOnce(&mut Settings),
) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let webroot = root.join("webroot");
    std::fs::create_dir_all(webroot.join("assets")).expect("webroot");
    std::fs::write(webroot.join("index.html"), INDEX_HTML).expect("index.html");
    std::fs::write(
        webroot.join("assets/app-abc.js"),
        "console.log('devmask')\n",
    )
    .expect("app.js");
    std::fs::write(webroot.join("assets/site.css"), "body{color:#000}\n").expect("site.css");
    std::fs::write(root.join("module.prop"), MODULE_PROP).expect("module.prop");

    let config_path = root.join("config.toml");
    if let Some(text) = config_text {
        std::fs::write(&config_path, text).expect("config.toml");
    }

    let mut settings = Settings::for_test(
        config_path.clone(),
        webroot.clone(),
        vec![
            token_entry(READ_TOKEN, vec![Scope::Read]),
            token_entry(WRITE_TOKEN, vec![Scope::Write]),
            token_entry(ADMIN_TOKEN, vec![Scope::Admin]),
            token_entry(OPS_TOKEN, vec![Scope::Ops]),
        ],
    );
    settings.audit_log = Some(root.join("logs/api-audit.jsonl"));
    settings.adb_dir = root.join("adb");
    customize(&mut settings);

    let device = DeviceSummary {
        manufacturer: "Google".to_owned(),
        brand: "google".to_owned(),
        model: "Pixel 9".to_owned(),
        device: "tokay".to_owned(),
        android: "17".to_owned(),
        sdk: 37,
        abi: "arm64-v8a".to_owned(),
    };

    let audit_path = settings.audit_path();
    let adb_dir = settings.adb_dir.clone();
    let state = if relay_binding {
        AppState::with_runner_and_relay(settings, "127.0.0.1:8686".to_owned(), device, runner)
    } else {
        AppState::with_runner(settings, "127.0.0.1:8686".to_owned(), device, runner)
    };
    let router = router(state.clone());
    Fixture {
        dir,
        state,
        router,
        config_path,
        config_text: config_text.unwrap_or("").to_owned(),
        audit_path,
        adb_dir,
    }
}

/// 常用命令 fixture：pm / am / monkey / getprop。
pub fn pm_fixture() -> FixtureRunner {
    FixtureRunner::new()
        .with_output(
            "pm list packages -u --show-versioncode",
            "package:com.android.chrome versionCode:677820000\npackage:com.example.app versionCode:420\n",
        )
        .with_output("pm list packages -s", "package:com.android.chrome\n")
        .with_output(
            "pm list users",
            "Users:\n\tUserInfo{0:Owner:13} running\n\tUserInfo{999:Work profile:30} running\n",
        )
        .with_output("am get-current-user", "0\n")
        .with_output(
            "pm list packages --user 0",
            "package:com.android.chrome\npackage:com.example.app\n",
        )
        .with_output("pm list packages --user 999", "package:com.example.app\n")
        .with_output(
            "dumpsys package com.example.app",
            "Packages:\n  Package [com.example.app]:\n    versionName=1.2.3\n",
        )
        .with_output("am force-stop --user 0 com.example.app", "")
        .with_output("am force-stop --user 999 com.example.app", "")
        .with_output(
            "monkey -p com.example.app -c android.intent.category.LAUNCHER 1",
            "",
        )
        .with_output(
            "getprop",
            "[ro.product.model]: [Pixel 9]\n[ro.build.fingerprint]: [google/tokay/tokay:17/BP2A/13000000:user/release-keys]\n[ro.vendor.missing]: []\n",
        )
        .with_output(
            "getprop ro.build.fingerprint",
            "google/tokay/tokay:17/BP2A/13000000:user/release-keys",
        )
}

/// 发一个请求（GET 无 token）。
pub async fn get(router: &Router, uri: &str) -> Response {
    send(router, Method::GET, uri, None, &[]).await
}

/// 发一个带 Bearer Token 的 GET。
pub async fn get_with_token(router: &Router, uri: &str, token: &str) -> Response {
    send(router, Method::GET, uri, Some(token), &[]).await
}

/// 通用请求构造（空 body）。
pub async fn send(
    router: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> Response {
    send_body(router, method, uri, token, extra_headers, None).await
}

/// 通用请求构造（可带 body）。
pub async fn send_body(
    router: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(body.map_or_else(Body::empty, Body::from))
        .expect("request");
    router.clone().oneshot(request).await.expect("infallible")
}

/// 发 JSON 请求。
pub async fn json_call(
    router: &Router,
    method: Method,
    uri: &str,
    token: &str,
    extra_headers: &[(&str, &str)],
    body: &Value,
) -> Response {
    let mut headers = vec![("content-type", "application/json")];
    headers.extend_from_slice(extra_headers);
    send_body(
        router,
        method,
        uri,
        Some(token),
        &headers,
        Some(serde_json::to_vec(body).expect("json body")),
    )
    .await
}

/// 读完响应体。
pub async fn body_bytes(response: Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec()
}

/// 响应体按 JSON 解析。
pub async fn body_json(response: Response) -> Value {
    let bytes = body_bytes(response).await;
    serde_json::from_slice(&bytes).expect("json body")
}

/// 响应体按 UTF-8 文本读取。
pub async fn body_text(response: Response) -> String {
    String::from_utf8(body_bytes(response).await).expect("utf-8 body")
}

/// 断言统一错误信封的形状，并返回 error 对象。
pub fn assert_error_envelope<'a>(value: &'a Value, expected_code: &str) -> &'a Value {
    let error = value.get("error").expect("error envelope");
    assert_eq!(error["code"], expected_code, "error code: {value}");
    assert!(error.get("message").is_some(), "message 必填");
    assert!(error.get("details").is_some(), "details 必填（可为 null）");
    assert!(error.get("retryable").is_some(), "retryable 必填");
    assert!(error.get("requestId").is_some(), "requestId 必填");
    assert_eq!(
        value.as_object().map(|object| object.len()),
        Some(1),
        "错误信封只能有 error 顶层字段"
    );
    error
}

/// 取响应头字符串。
pub fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// 常用断言：状态码。
pub fn assert_status(response: &Response, expected: StatusCode) {
    assert_eq!(response.status(), expected, "unexpected status");
}

/// 文件内容 → revision（与 server 的 revision 定义一致）。
pub fn revision_of(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => util::sha256_hex(&bytes),
        Err(_) => util::sha256_hex(b""),
    }
}

/// 当前配置 revision。
pub fn current_revision(fixture: &Fixture) -> String {
    revision_of(&fixture.config_path)
}

/// 读配置文件文本。
pub fn read_config(fixture: &Fixture) -> String {
    std::fs::read_to_string(&fixture.config_path).unwrap_or_default()
}

/// 带 If-Match 的请求头（ETag 带引号，测试两种写法都覆盖）。
pub fn if_match(revision: &str) -> String {
    format!("\"{revision}\"")
}

/// GET /config/revision 拿当前 revision。
pub async fn fetch_revision(router: &Router) -> String {
    let response = get_with_token(router, "/api/v1/config/revision", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    body["revision"].as_str().expect("revision").to_owned()
}
