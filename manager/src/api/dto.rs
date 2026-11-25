// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 设备 / 模板 / 矩阵 / 同步响应的共享 DTO，以及设备 overview 的只读投影。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::db::DeviceRecord;

/// 缓存过期阈值（秒）。
pub const CACHE_TTL_SECS: i64 = 300;

/// 设备出口摘要：最近一次探测的 exitIp 与绑定的 proxyName（只读聚合）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEgressSummary {
    /// proxy / direct；设备级没有任何绑定时为 null。
    pub mode: Option<String>,
    pub proxy_id: Option<String>,
    pub proxy_name: Option<String>,
    pub exit_ip: Option<String>,
    pub checked_at: Option<String>,
    pub ok: Option<bool>,
    pub error_code: Option<String>,
    /// 设备内本地认证中继（relay）状态；离线 / 旧设备（`GET /egress` 无 `relay` 字段）
    /// / 设备不可达时保持 null（字段只增不改）。
    pub relay: Option<DeviceRelaySummary>,
}

/// 设备内本地认证中继（relay）的状态投影，来源是设备 `GET /egress` 的 `relay` 对象。
///
/// `listeners` 只数**正在 accept** 的 listener（`listening=true`），因此
/// `active == enabled && listeners > 0`——`relay.listeners` 里绑定失败（`listening=false`）
/// 的端口不会让设备卡片亮起「经设备中继」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRelaySummary {
    /// `[relay].enabled` 的实际生效值（设备端降级后的结果）。
    pub enabled: bool,
    /// 是否真的在经设备内中继转发。
    pub active: bool,
    /// 正在监听的中继端口数量。
    pub listeners: usize,
    /// `relay.firewall.applied`：uid 防火墙规则是否已落地。
    pub firewall_applied: bool,
}

/// 设备摘要（API.md 5.1）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub online: bool,
    pub tags: Vec<String>,
    pub model: Option<String>,
    pub brand: Option<String>,
    pub android: Option<String>,
    pub sdk: Option<i64>,
    pub fingerprint: Option<String>,
    pub root: Option<String>,
    pub module_version: Option<String>,
    pub server_version: Option<String>,
    pub last_seen: Option<String>,
    pub last_synced_at: Option<String>,
    pub latency_ms: Option<i64>,
    pub created_at: String,
    pub last_error: Option<Value>,
    /// 设备级出口摘要（未知时为 null，字段只增不改）。
    pub egress: Option<DeviceEgressSummary>,
}

impl DeviceSummary {
    /// 附带出口摘要的构造（列表 / 详情 / 总览使用）。
    pub fn with_egress(record: DeviceRecord, egress: Option<DeviceEgressSummary>) -> Self {
        Self {
            egress,
            ..Self::from(record)
        }
    }
}

impl From<DeviceRecord> for DeviceSummary {
    fn from(record: DeviceRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            base_url: record.base_url,
            online: record.online,
            tags: record.tags,
            model: record.model,
            brand: record.brand,
            android: record.android,
            sdk: record.sdk,
            fingerprint: record.fingerprint,
            root: record.root_manager,
            module_version: record.module_version,
            server_version: record.server_version,
            last_seen: record.last_seen,
            last_synced_at: record.last_synced_at,
            latency_ms: record.latency_ms,
            created_at: record.created_at,
            last_error: record.last_error,
            egress: None,
        }
    }
}

/// 汇总设备级出口摘要：绑定（app 为空）叠加最近一条整机探测结果。
pub fn device_egress_summaries(
    bindings: &[crate::db::DeviceEgressRow],
    checks: &std::collections::HashMap<String, crate::db::EgressCheckRow>,
) -> std::collections::HashMap<String, DeviceEgressSummary> {
    let mut summaries: std::collections::HashMap<String, DeviceEgressSummary> =
        std::collections::HashMap::new();
    for binding in bindings.iter().filter(|binding| binding.app.is_empty()) {
        summaries.insert(
            binding.device_id.clone(),
            DeviceEgressSummary {
                mode: Some(binding.mode.clone()),
                proxy_id: binding.proxy_id.clone(),
                proxy_name: binding.proxy_name.clone(),
                exit_ip: None,
                checked_at: None,
                ok: None,
                error_code: None,
                relay: None,
            },
        );
    }
    for (device_id, check) in checks {
        let entry = summaries
            .entry(device_id.clone())
            .or_insert_with(|| DeviceEgressSummary {
                mode: None,
                proxy_id: None,
                proxy_name: None,
                exit_ip: None,
                checked_at: None,
                ok: None,
                error_code: None,
                relay: None,
            });
        entry.exit_ip = check.exit_ip.clone();
        entry.checked_at = Some(check.checked_at.clone());
        entry.ok = Some(check.ok);
        entry.error_code = check.error_code.clone();
    }
    summaries
}

/// 警告（code / message / details）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Warning {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl Warning {
    pub fn new(code: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details,
        }
    }
}

/// 生效来源。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub template: Option<String>,
    pub also_inline: Option<bool>,
}

/// 模板摘要（详情 / 模板聚合共用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSummary {
    pub name: String,
    pub config: Value,
    pub fields_set: Vec<String>,
    pub sources: Vec<String>,
    pub complete: bool,
    pub warnings: Vec<Warning>,
}

/// 设备上某个应用的配置摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAppSummary {
    pub package: String,
    pub installed: Option<bool>,
    pub source: AppSource,
    pub model: Option<String>,
    pub warnings: Vec<Warning>,
}

/// 设备详情（API.md 4.4）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetail {
    #[serde(flatten)]
    pub summary: DeviceSummary,
    pub overview: Option<Value>,
    pub templates: Vec<TemplateSummary>,
    pub apps: Vec<DeviceAppSummary>,
    pub cached_at: Option<String>,
    pub stale: bool,
    pub warnings: Vec<Warning>,
}

/// 设备列表（API.md 4.2）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListResponse {
    pub devices: Vec<DeviceSummary>,
    pub total: usize,
    pub online: usize,
    pub offline: usize,
    pub next_cursor: Option<String>,
}

/// 单台 overview 响应（API.md 4.9）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOverviewResponse {
    pub device_id: String,
    pub online: bool,
    pub cached_at: Option<String>,
    pub stale: bool,
    pub overview: Value,
}

/// 单台同步响应（API.md 4.7）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDeviceResponse {
    pub device: DeviceSummary,
    pub overview: Option<Value>,
    pub warnings: Vec<Warning>,
}

/// 全量同步的单台结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAllResult {
    pub device_id: String,
    pub status: String,
    pub online: bool,
    pub latency_ms: Option<i64>,
    pub code: Option<String>,
    pub message: Option<String>,
}

/// 全量同步响应（API.md 4.8）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAllResponse {
    pub synced: usize,
    pub failed: usize,
    pub results: Vec<SyncAllResult>,
    pub warnings: Vec<Warning>,
}

/// 添加设备请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddDeviceRequest {
    pub name: String,
    pub base_url: String,
    pub token: String,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub id: Option<String>,
}

/// 修改设备请求；全部字段可选，至少给一项。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDeviceRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

/// 设备 overview 里管理器关心的字段。
#[derive(Debug, Clone, Default)]
pub struct OverviewMeta {
    pub model: Option<String>,
    pub brand: Option<String>,
    pub android: Option<String>,
    pub sdk: Option<i64>,
    pub fingerprint: Option<String>,
    pub root_manager: Option<String>,
    pub module_version: Option<String>,
    pub server_version: Option<String>,
    pub revision: Option<String>,
}

/// 缓存新鲜度。
#[derive(Debug, Clone, Copy)]
pub struct StaleState {
    pub stale: bool,
    pub age_secs: Option<i64>,
}

/// 解析 overview 元数据。
pub fn overview_meta(overview: &Value) -> OverviewMeta {
    OverviewMeta {
        model: string_at(overview, "/device/model"),
        brand: string_at(overview, "/device/brand"),
        android: string_at(overview, "/device/android"),
        sdk: i64_at(overview, "/device/sdk"),
        fingerprint: string_at(overview, "/device/fingerprint"),
        root_manager: string_at(overview, "/root/manager"),
        module_version: string_at(overview, "/module/version"),
        server_version: string_at(overview, "/server/version"),
        revision: string_at(overview, "/config/revision")
            .or_else(|| string_at(overview, "/revision")),
    }
}

/// 聚合 revision 使用的设备 revision。
pub fn overview_revision(overview: &Value) -> Option<String> {
    string_at(overview, "/config/revision").or_else(|| string_at(overview, "/revision"))
}

/// 根据 fetched_at 与在线状态计算是否过期。
pub fn stale_state(fetched_at: Option<&str>, online: bool) -> StaleState {
    let age_secs = fetched_at
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|timestamp| (Utc::now() - timestamp.with_timezone(&Utc)).num_seconds());
    let stale = !online || age_secs.is_none_or(|age| age > CACHE_TTL_SECS);
    StaleState { stale, age_secs }
}

/// overview.templates -> TemplateSummary 列表。
pub fn overview_templates(overview: &Value, device_id: &str) -> Vec<TemplateSummary> {
    let Some(items) = overview.get("templates").and_then(Value::as_array) else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(Value::as_str)?.to_string();
            let mut config = item.get("config").cloned().unwrap_or_else(|| json!({}));
            if let Some(packages) = item.get("packages")
                && let Some(object) = config.as_object_mut()
            {
                object.insert("packages".to_string(), packages.clone());
            }
            let mut fields_set: Vec<String> = config
                .as_object()
                .map(|object| object.keys().cloned().collect())
                .unwrap_or_default();
            fields_set.sort();
            fields_set.dedup();

            let complete = ["model", "brand", "android_version", "sdk_int", "dpi"]
                .iter()
                .all(|field| config.get(*field).is_some());

            Some(TemplateSummary {
                name,
                config,
                fields_set,
                sources: vec![device_id.to_string()],
                complete,
                warnings: Vec::new(),
            })
        })
        .collect()
}

/// overview.apps -> DeviceAppSummary 列表。
pub fn overview_apps(overview: &Value) -> Vec<DeviceAppSummary> {
    let Some(items) = overview.get("apps").and_then(Value::as_array) else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let package = item.get("package").and_then(Value::as_str)?.to_string();
            let installed = item.get("installed").and_then(Value::as_bool);
            let source = app_source(item.get("source"));
            let model = item
                .pointer("/resolved/model")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let warnings = warnings_at(item.get("warnings"));
            Some(DeviceAppSummary {
                package,
                installed,
                source,
                model,
                warnings,
            })
        })
        .collect()
}

/// 解析 source 对象。
pub fn app_source(value: Option<&Value>) -> AppSource {
    let Some(value) = value else {
        return AppSource {
            source_type: "none".to_string(),
            template: None,
            also_inline: None,
        };
    };
    AppSource {
        source_type: value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string(),
        template: value
            .get("template")
            .and_then(Value::as_str)
            .map(str::to_owned),
        also_inline: value.get("alsoInline").and_then(Value::as_bool),
    }
}

/// overview 顶层 warnings。
pub fn overview_warnings(overview: &Value) -> Vec<Warning> {
    warnings_at(overview.get("warnings"))
}

/// 任意 warning 数组（app 级 warnings 使用）。
pub fn warnings_from(value: Option<&Value>) -> Vec<Warning> {
    warnings_at(value)
}

fn warnings_at(value: Option<&Value>) -> Vec<Warning> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let code = item.get("code").and_then(Value::as_str)?;
                    Some(Warning::new(
                        code,
                        item.get("message").and_then(Value::as_str).unwrap_or(code),
                        item.get("details").cloned().unwrap_or(Value::Null),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn i64_at(value: &Value, pointer: &str) -> Option<i64> {
    value.pointer(pointer).and_then(Value::as_i64)
}

/// 解析缓存的 overview 原文。
pub fn parse_overview(payload: &str) -> Option<Value> {
    serde_json::from_str(payload).ok()
}
