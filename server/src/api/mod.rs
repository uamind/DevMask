// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! /api/v1 的端点实现与共享响应工具。
//!
//! 契约：docs/API.md 5（端点明细）、6（数据结构）、7（错误信封）。
//! 所有响应头约定集中在 [`with_revision`] / [`with_etag`]：
//! - `X-DF-Revision`：不带引号的 revision；
//! - `ETag`：带引号的强校验值，可直接作为 If-Match 回传。

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Extension, OriginalUri, State},
    http::{HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use device_faker_core::Config;
use serde::Serialize;

use crate::app::AppState;
use crate::device::DeviceSummary;
use crate::error::ApiError;
use crate::module::{self, ModuleInfo};
use crate::requests::RequestId;
use crate::store;

pub mod apps;
pub mod config;
pub mod device;
pub mod packages;
pub mod schema;
pub mod templates;

/// 契约里 23 个操作的 (method, path)，用于契约测试与路由双向校验。
pub const CONTRACT_OPERATIONS: [(&str, &str); 23] = [
    ("GET", "/api/v1/healthz"),
    ("GET", "/api/v1/status"),
    ("GET", "/api/v1/schema/config"),
    ("GET", "/api/v1/config"),
    ("GET", "/api/v1/config/revision"),
    ("PUT", "/api/v1/config"),
    ("POST", "/api/v1/config/validate"),
    ("GET", "/api/v1/templates"),
    ("GET", "/api/v1/templates/{name}"),
    ("PUT", "/api/v1/templates/{name}"),
    ("PATCH", "/api/v1/templates/{name}"),
    ("DELETE", "/api/v1/templates/{name}"),
    ("POST", "/api/v1/templates/{name}/packages"),
    ("DELETE", "/api/v1/templates/{name}/packages/{pkg}"),
    ("GET", "/api/v1/apps"),
    ("GET", "/api/v1/apps/{pkg}"),
    ("PUT", "/api/v1/apps/{pkg}"),
    ("DELETE", "/api/v1/apps/{pkg}"),
    ("POST", "/api/v1/apps/{pkg}/restart"),
    ("GET", "/api/v1/packages"),
    ("GET", "/api/v1/device"),
    ("GET", "/api/v1/device/props"),
    ("GET", "/api/v1/device/users"),
];

/// 本增量已实现的能力标签（契约固定顺序）。
pub const CAPABILITIES: [&str; 6] = [
    "config",
    "templates",
    "apps",
    "packages",
    "device",
    "validate",
];

const X_DF_REVISION: HeaderName = HeaderName::from_static("x-df-revision");
const ETAG: HeaderName = HeaderName::from_static("etag");

/// 给响应加 `X-DF-Revision`（不带引号）。
pub fn with_revision(mut response: Response, revision: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(revision) {
        response.headers_mut().insert(X_DF_REVISION, value);
    }
    response
}

/// 给响应加 `ETag`（带引号的强校验值）。
pub fn with_etag(mut response: Response, revision: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(&format!("\"{revision}\"")) {
        response.headers_mut().insert(ETAG, value);
    }
    response
}

/// 同时加 `X-DF-Revision` 与 `ETag`。
pub fn with_revision_etag(response: Response, revision: &str) -> Response {
    with_etag(with_revision(response, revision), revision)
}

/// JSON 响应（带状态码）。
pub fn json<T: Serialize>(status: StatusCode, body: T) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}

/// GET /api/v1/healthz 响应（openapi HealthResponse）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    /// 固定 ok。
    pub status: &'static str,
    /// 服务版本。
    pub version: &'static str,
    /// 已运行秒数。
    pub uptime_sec: u64,
}

/// GET /api/v1/status 响应（openapi StatusResponse）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    /// 模块信息（module.prop）。
    pub module: ModuleInfo,
    /// 设备摘要（getprop / fixture）。
    pub device: DeviceSummary,
    /// 服务运行时信息。
    pub server: ServerInfo,
    /// 当前配置的 revision 与统计。
    pub config: ConfigStatus,
    /// 已实现能力标签。
    pub capabilities: Vec<&'static str>,
}

/// 服务运行时信息（openapi ServerInfo）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// device_faker_server 版本。
    pub version: &'static str,
    /// 实际监听地址。
    pub bind: String,
    /// 已运行秒数。
    pub uptime_sec: u64,
    /// 启动时间（UTC RFC 3339）。
    pub started_at: String,
    /// 进程 pid。
    pub pid: u32,
}

/// 配置文件的 revision 与统计（openapi ConfigStatus）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    /// 配置字节的 sha256（64 位小写十六进制）。
    pub revision: String,
    /// 文件 mtime（UTC RFC 3339）；文件不存在时为 null。
    pub mtime: Option<String>,
    /// 文件字节数。
    pub size: u64,
    /// 配置文件是否存在。
    pub exists: bool,
    /// 模板数量。
    pub templates: usize,
    /// 内联应用记录数量。
    pub apps: usize,
    /// 同一包名被多个模板声明的去重包数。
    pub conflicts: usize,
}

/// GET /api/v1/healthz：无需认证。
pub async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: state.version(),
        uptime_sec: state.uptime_sec(),
    })
}

/// GET /api/v1/status：需要 read 作用域（由路由层守卫）。
pub async fn status(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let settings = state.settings();
    let config = config_status(&state, request_id.as_str())?;
    let body = StatusResponse {
        module: module::read(&settings.module_prop_path),
        device: state.device().clone(),
        server: ServerInfo {
            version: state.version(),
            bind: state.bind().to_owned(),
            uptime_sec: state.uptime_sec(),
            started_at: state.started_at().to_owned(),
            pid: std::process::id(),
        },
        capabilities: CAPABILITIES.to_vec(),
        config: config.clone(),
    };

    Ok(with_revision(json(StatusCode::OK, body), &config.revision))
}

/// /api/v1 下未匹配路径：404 + 错误信封（不回退 index.html）。
pub async fn not_found(
    Extension(request_id): Extension<RequestId>,
    OriginalUri(uri): OriginalUri,
) -> ApiError {
    ApiError::not_found(request_id.as_str(), "path", uri.path())
}

/// /api/v1 下方法不匹配：405 + 错误信封。
pub async fn method_not_allowed(
    Extension(request_id): Extension<RequestId>,
    method: Method,
    OriginalUri(uri): OriginalUri,
) -> ApiError {
    ApiError::method_not_allowed(request_id.as_str(), method.as_str(), uri.path())
}

fn config_status(state: &AppState, request_id: &str) -> Result<ConfigStatus, ApiError> {
    let snapshot = store::read_snapshot(state.store())
        .map_err(|error| ApiError::internal_error(request_id, format!("读取配置失败: {error}")))?;
    let text = if snapshot.exists {
        snapshot.text.as_str()
    } else {
        ""
    };
    let config = Config::from_toml(text)
        .map_err(|error| ApiError::internal_error(request_id, format!("配置解析失败: {error}")))?;

    Ok(ConfigStatus {
        revision: snapshot.revision,
        mtime: snapshot.mtime,
        size: snapshot.size,
        exists: snapshot.exists,
        templates: config.templates.len(),
        apps: config.apps.len(),
        conflicts: count_conflicts(&config),
    })
}

/// 组装写端点响应：按需生成 unified diff（dry_run 时总是给）。
pub fn write_result(
    outcome: &store::TxOutcome,
    dry_run: bool,
    want_diff: bool,
    warnings: Vec<crate::dto::Warning>,
) -> crate::dto::WriteResult {
    let diff = if want_diff || dry_run {
        let text = crate::diff::unified_diff("config.toml", &outcome.old_text, &outcome.new_text);
        Some(crate::diff::cap(text))
    } else {
        None
    };
    crate::dto::WriteResult::new(outcome, dry_run, diff, warnings)
}

/// 冲突数：同一包名出现在 >= 2 个模板的 packages 中（模板内重复只算一次）。
pub fn count_conflicts(config: &Config) -> usize {
    let mut declarations: HashMap<&str, usize> = HashMap::new();
    for template in config.templates.values() {
        let unique: HashSet<&str> = template.packages.iter().map(String::as_str).collect();
        for package in unique {
            *declarations.entry(package).or_insert(0) += 1;
        }
    }
    declarations.values().filter(|count| **count >= 2).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicts_count_distinct_packages_declared_twice() {
        let text = r#"
[templates.a]
packages = ["com.example.one", "com.example.two", "com.example.two"]

[templates.b]
packages = ["com.example.two", "com.example.three"]

[templates.c]
packages = ["com.example.three"]
"#;
        let config = Config::from_toml(text).unwrap();
        assert_eq!(config.templates.len(), 3);
        assert_eq!(count_conflicts(&config), 2, "two 与 three 各被两个模板声明");
    }

    #[test]
    fn contract_operations_are_unique() {
        let mut seen = HashSet::new();
        for (method, path) in CONTRACT_OPERATIONS {
            assert!(seen.insert((method, path)), "{method} {path} 重复");
        }
        assert_eq!(seen.len(), 23);
    }
}
