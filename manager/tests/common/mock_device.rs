// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 进程内 Mock 设备：实现 Manager 需要的 /api/v1/status 与 /api/v1/overview。

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get, post, put},
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

/// Mock 模板。
#[derive(Debug, Clone)]
pub struct MockTemplate {
    pub name: String,
    pub packages: Vec<String>,
    pub config: Value,
}

/// Mock 应用配置。
#[derive(Debug, Clone)]
pub struct MockApp {
    pub package: String,
    pub template: String,
    pub model: String,
    pub installed: bool,
    pub warnings: Vec<Value>,
}

/// Mock 设备行为。
#[derive(Debug, Clone)]
pub struct MockOptions {
    pub token: String,
    pub model: String,
    pub brand: String,
    pub android: String,
    pub sdk: i64,
    pub fingerprint: String,
    pub templates: Vec<MockTemplate>,
    pub apps: Vec<MockApp>,
    /// 非 2xx 的固定失败（探测 / 同步用）。
    pub fail_status: Option<u16>,
    /// 每个请求的人为延迟（超时测试用）。
    pub delay: Option<Duration>,
    /// 首次 PUT /apps/{pkg} 强制返回 412（并推进 revision）。
    pub force_first_412: bool,
    /// 这些包被 PUT /apps/{pkg} 视为未安装（404 resource=package）。
    pub missing_packages: Vec<String>,
    /// POST /apps/{pkg}/restart 的固定状态（如 403）。
    pub restart_status: Option<u16>,
    /// overview 返回超过 8 MiB 的响应（设备响应上限回归测试）。
    pub oversized_overview: bool,
    /// 首次 PUT /templates/{name} 返回 412（模板写入重试回归）。
    pub force_template_412_once: bool,
    /// PUT /apps/{pkg} 绑定的模板名等于该值时，返回 404 resource=template。
    pub bind_template_404: Option<String>,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            token: "admin123".to_string(),
            model: "Pixel 9".to_string(),
            brand: "google".to_string(),
            android: "17".to_string(),
            sdk: 37,
            fingerprint: "google/tokay/tokay:17/BP2A.250605.031/13000000:user/release-keys"
                .to_string(),
            templates: vec![MockTemplate {
                name: "pixel_9".to_string(),
                packages: vec!["com.xingin.xhs".to_string()],
                config: json!({
                    "brand": "google",
                    "model": "Pixel 9",
                    "android_version": "17",
                    "sdk_int": 37,
                    "dpi": 420
                }),
            }],
            apps: vec![MockApp {
                package: "com.xingin.xhs".to_string(),
                template: "pixel_9".to_string(),
                model: "Pixel 9".to_string(),
                installed: true,
                warnings: Vec::new(),
            }],
            fail_status: None,
            delay: None,
            force_first_412: false,
            missing_packages: Vec::new(),
            restart_status: None,
            oversized_overview: false,
            force_template_412_once: false,
            bind_template_404: None,
        }
    }
}

struct MockState {
    options: Mutex<MockOptions>,
    revision: Mutex<u64>,
    templates: Mutex<HashMap<String, Value>>,
    apps: Mutex<HashMap<String, String>>,
    forced_412: AtomicBool,
    forced_template_412: AtomicBool,
}

/// 运行中的 Mock 设备。
pub struct MockDevice {
    pub base_url: String,
    pub token: String,
    state: Arc<MockState>,
    handle: JoinHandle<()>,
}

impl MockDevice {
    pub async fn start(options: MockOptions) -> Self {
        let token = options.token.clone();
        let templates: HashMap<String, Value> = options
            .templates
            .iter()
            .map(|template| {
                let mut config = template.config.clone();
                if let Some(object) = config.as_object_mut() {
                    object.insert("packages".to_string(), json!(template.packages));
                }
                (template.name.clone(), config)
            })
            .collect();
        let apps: HashMap<String, String> = options
            .apps
            .iter()
            .map(|app| (app.package.clone(), app.template.clone()))
            .collect();
        let forced_412 = options.force_first_412;
        let forced_template_412 = options.force_template_412_once;
        let state = Arc::new(MockState {
            options: Mutex::new(options),
            revision: Mutex::new(1),
            templates: Mutex::new(templates),
            apps: Mutex::new(apps),
            forced_412: AtomicBool::new(forced_412),
            forced_template_412: AtomicBool::new(forced_template_412),
        });

        let app = Router::new()
            .route("/api/v1/status", get(status))
            .route("/api/v1/overview", get(overview))
            .route("/api/v1/config/revision", get(revision))
            .route(
                "/api/v1/templates/{name}",
                get(get_template).put(put_template),
            )
            .route("/api/v1/apps/{pkg}", put(put_app))
            .route("/api/v1/apps/{pkg}/restart", post(restart_app))
            .route("/api/v1/echo", any(echo))
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("绑定 Mock 设备");
        let address = listener.local_addr().expect("Mock 地址");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self {
            base_url: format!("http://{address}"),
            token,
            state,
            handle,
        }
    }

    pub fn set_model(&self, model: &str) {
        self.options_mut(|options| options.model = model.to_string());
        self.options_mut(|options| {
            for app in &mut options.apps {
                if app.template == "pixel_9" {
                    app.model = model.to_string();
                }
            }
        });
    }

    /// 运行中更换设备 Token（模拟 Token 被设备端轮换，用于 401 重映射测试）。
    pub fn set_token(&self, token: &str) {
        self.options_mut(|options| options.token = token.to_string());
    }

    pub fn set_delay(&self, delay: Option<Duration>) {
        self.options_mut(|options| options.delay = delay);
    }

    pub fn set_fail_status(&self, status: Option<u16>) {
        self.options_mut(|options| options.fail_status = status);
    }

    /// 运行中开启/关闭超大 overview（读接口 8 MiB 上限回归）。
    pub fn set_oversized_overview(&self, enabled: bool) {
        self.options_mut(|options| options.oversized_overview = enabled);
    }

    /// 读取模板当前配置（真实形状，含 packages）。
    pub fn template_config(&self, name: &str) -> Option<Value> {
        self.state
            .templates
            .lock()
            .expect("Mock templates 锁")
            .get(name)
            .cloned()
    }

    /// 读取某个包当前绑定的模板名。
    pub fn app_template(&self, package: &str) -> Option<String> {
        self.state
            .apps
            .lock()
            .expect("Mock apps 锁")
            .get(package)
            .cloned()
    }

    /// 停止服务，后续请求按连接失败处理。
    pub fn stop(&self) {
        self.handle.abort();
    }

    fn options_mut(&self, apply: impl FnOnce(&mut MockOptions)) {
        let mut guard = self.state.options.lock().expect("Mock 状态锁");
        apply(&mut guard);
    }

    fn snapshot(&self) -> MockOptions {
        self.state.options.lock().expect("Mock 状态锁").clone()
    }
}

impl Drop for MockDevice {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn status(State(state): State<Arc<MockState>>, headers: HeaderMap) -> Response {
    let options = snapshot(&state);
    if let Some(response) = common_guard(&options, &headers).await {
        return response;
    }
    let overview = overview_payload(&options);
    Json(json!({
        "module": overview["module"],
        "device": overview["device"],
        "server": overview["server"],
        "config": overview["config"],
        "capabilities": ["config", "templates", "apps", "overview"],
    }))
    .into_response()
}

async fn overview(State(state): State<Arc<MockState>>, headers: HeaderMap) -> Response {
    let options = snapshot(&state);
    if let Some(response) = common_guard(&options, &headers).await {
        return response;
    }
    if options.oversized_overview {
        return Json(json!({
            "device": {},
            "blob": "x".repeat(9 * 1024 * 1024),
        }))
        .into_response();
    }
    Json(overview_payload(&options)).into_response()
}

async fn revision(State(state): State<Arc<MockState>>, headers: HeaderMap) -> Response {
    let options = snapshot(&state);
    if let Some(response) = common_guard(&options, &headers).await {
        return response;
    }
    Json(json!({
        "revision": current_revision(&state),
        "mtime": "2026-11-24T08:00:00Z",
        "size": 256,
        "exists": true,
    }))
    .into_response()
}

fn current_revision(state: &MockState) -> String {
    let revision = state.revision.lock().expect("Mock revision 锁");
    format!("rev-{revision}")
}

fn bump_revision(state: &MockState) -> String {
    let mut revision = state.revision.lock().expect("Mock revision 锁");
    *revision += 1;
    format!("rev-{revision}")
}

async fn get_template(
    State(state): State<Arc<MockState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let options = snapshot(&state);
    if let Some(response) = common_guard(&options, &headers).await {
        return response;
    }
    let template = state
        .templates
        .lock()
        .expect("Mock templates 锁")
        .get(&name)
        .cloned();
    match template {
        Some(mut config) => {
            if let Some(object) = config.as_object_mut() {
                object.entry("packages").or_insert_with(|| json!([]));
            }
            Json(json!({
                "revision": current_revision(&state),
                "position": 0,
                "name": name,
                "template": config,
            }))
            .into_response()
        }
        None => error_response(
            404,
            "not_found",
            "mock template missing",
            json!({ "resource": "template", "name": name }),
        ),
    }
}

async fn put_template(
    State(state): State<Arc<MockState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(config): Json<Value>,
) -> Response {
    let options = snapshot(&state);
    if let Some(response) = common_guard(&options, &headers).await {
        return response;
    }
    if state.forced_template_412.swap(false, Ordering::SeqCst) {
        return error_response(
            412,
            "config_revision_mismatch",
            "forced template 412",
            json!({ "expected": current_revision(&state), "actual": "rev-stale" }),
        );
    }
    state
        .templates
        .lock()
        .expect("Mock templates 锁")
        .insert(name, config);
    let revision = bump_revision(&state);
    Json(json!({ "revision": revision, "warnings": [] })).into_response()
}

async fn put_app(
    State(state): State<Arc<MockState>>,
    Path(package): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let options = snapshot(&state);
    if let Some(response) = common_guard(&options, &headers).await {
        return response;
    }
    if options
        .missing_packages
        .iter()
        .any(|missing| missing == &package)
    {
        return error_response(
            404,
            "not_found",
            "mock package not installed",
            json!({ "resource": "package", "name": package }),
        );
    }
    if state.forced_412.swap(false, Ordering::SeqCst) {
        let revision = bump_revision(&state);
        return error_response(
            412,
            "config_revision_mismatch",
            "forced 412",
            json!({ "expected": revision, "actual": "rev-stale" }),
        );
    }

    let current = current_revision(&state);
    let if_match = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim_matches('"')
        .to_string();
    if if_match != current {
        return error_response(
            412,
            "config_revision_mismatch",
            "revision mismatch",
            json!({ "expected": current, "actual": if_match }),
        );
    }

    let template = body
        .get("template")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if options.bind_template_404.as_deref() == Some(template.as_str()) {
        return error_response(
            404,
            "not_found",
            "mock template missing on bind",
            json!({ "resource": "template", "name": template }),
        );
    }
    if !state
        .templates
        .lock()
        .expect("Mock templates 锁")
        .contains_key(&template)
    {
        return error_response(
            404,
            "not_found",
            "mock template missing",
            json!({ "resource": "template", "name": template }),
        );
    }

    state
        .apps
        .lock()
        .expect("Mock apps 锁")
        .insert(package.clone(), template.clone());
    if let Some(config) = state
        .templates
        .lock()
        .expect("Mock templates 锁")
        .get_mut(&template)
        && let Some(object) = config.as_object_mut()
    {
        let packages = object.entry("packages").or_insert_with(|| json!([]));
        if let Some(packages) = packages.as_array_mut()
            && !packages
                .iter()
                .any(|value| value.as_str() == Some(&package))
        {
            packages.push(json!(package.clone()));
        }
    }
    let revision = bump_revision(&state);
    Json(json!({
        "revision": revision,
        "previousRevision": current,
        "dryRun": false,
        "backupId": null,
        "warnings": [],
    }))
    .into_response()
}

async fn restart_app(
    State(state): State<Arc<MockState>>,
    Path(package): Path<String>,
    headers: HeaderMap,
) -> Response {
    let options = snapshot(&state);
    if let Some(response) = common_guard(&options, &headers).await {
        return response;
    }
    if let Some(status) = options.restart_status {
        return failure(status);
    }
    Json(json!({
        "package": package,
        "forceStopped": true,
        "launched": false,
        "durationMs": 5,
    }))
    .into_response()
}

async fn echo(State(state): State<Arc<MockState>>, request: Request) -> Response {
    let options = snapshot(&state);
    if let Some(response) = common_guard(&options, &request.headers().clone()).await {
        return response;
    }

    let method = request.method().as_str().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();
    let path = request.uri().path().to_string();
    let (_, body) = request.into_parts();
    let bytes = to_bytes(body, 2 * 1024 * 1024)
        .await
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default();

    if query.contains("status=404") {
        return error_response(404, "not_found", "mock 404", json!({ "resource": "mock" }));
    }
    if query.contains("status=400") {
        return error_response(
            400,
            "validation_failed",
            "mock 400",
            json!({ "errors": [] }),
        );
    }

    let body_json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes).into_owned()))
    };

    Json(json!({
        "method": method,
        "path": path,
        "query": query,
        "body": body_json,
    }))
    .into_response()
}

fn snapshot(state: &Arc<MockState>) -> MockOptions {
    state.options.lock().expect("Mock 状态锁").clone()
}

/// 返回 Some(response) 表示提前失败。
async fn common_guard(options: &MockOptions, headers: &HeaderMap) -> Option<Response> {
    if let Some(delay) = options.delay {
        tokio::time::sleep(delay).await;
    }
    if let Some(status) = options.fail_status {
        return Some(failure(status));
    }
    let expected = format!("Bearer {}", options.token);
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if provided != Some(expected.as_str()) {
        return Some(failure(401));
    }
    None
}

fn failure(status: u16) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let code = match status.as_u16() {
        401 => "unauthorized",
        403 => "forbidden_scope",
        404 => "not_found",
        _ => "internal_error",
    };
    (
        status,
        Json(json!({
            "error": {
                "code": code,
                "message": "mock device failure",
                "details": null,
                "retryable": true,
                "requestId": "req_mock",
            }
        })),
    )
        .into_response()
}

fn error_response(status: u16, code: &str, message: &str, details: Value) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        Json(json!({
            "error": {
                "code": code,
                "message": message,
                "details": details,
                "retryable": false,
                "requestId": "req_mock",
            }
        })),
    )
        .into_response()
}

fn revision_of(options: &MockOptions) -> String {
    format!(
        "rev-{}-{}-{}",
        options.model,
        options.android,
        options.apps.len()
    )
}

fn overview_payload(options: &MockOptions) -> Value {
    let revision = revision_of(options);
    json!({
        "revision": revision,
        "module": { "id": "device_faker", "version": "v1.9.0", "versionCode": 20261125 },
        "device": {
            "manufacturer": "Google",
            "brand": options.brand,
            "model": options.model,
            "device": "tokay",
            "android": options.android,
            "sdk": options.sdk,
            "abi": "arm64-v8a",
            "fingerprint": options.fingerprint,
        },
        "root": { "manager": "kernelsu", "version": "1.0.7" },
        "server": {
            "version": "1.9.0",
            "bind": "127.0.0.1:8686",
            "uptimeSec": 3600,
            "startedAt": "2026-11-24T08:00:00Z",
            "pid": 4321,
        },
        "config": {
            "revision": revision,
            "mtime": "2026-11-24T08:00:00Z",
            "size": 256,
            "exists": true,
            "templates": options.templates.len(),
            "apps": options.apps.len(),
            "conflicts": 0,
        },
        "templates": options
            .templates
            .iter()
            .enumerate()
            .map(|(position, template)| {
                json!({
                    "name": template.name,
                    "position": position,
                    "packages": template.packages,
                    "config": template.config,
                })
            })
            .collect::<Vec<_>>(),
        "apps": options
            .apps
            .iter()
            .map(|app| {
                json!({
                    "package": app.package,
                    "installed": app.installed,
                    "source": {
                        "type": "template",
                        "template": app.template,
                        "alsoInline": false,
                    },
                    "resolved": { "model": app.model, "brand": options.brand, "dpi": 420 },
                    "warnings": app.warnings,
                })
            })
            .collect::<Vec<_>>(),
        "warnings": [],
    })
}
