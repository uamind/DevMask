// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 出口代理（egress）请求 / 响应 DTO。
//!
//! 硬约束：任何 DTO 都没有 username / password 字段，凭据的唯一信号是 hasCredentials。

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{
    api::dto::Warning,
    db::{DeviceEgressRow, EgressCheckRow, EgressProxyRecord},
};

/// 反序列化「省略 / null / 有值」三态：调用方据此区分「不改」与「清除」。
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// 一次探测结果的线上形状（lastCheck / recentChecks）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressCheckDto {
    pub source: String,
    pub device_id: Option<String>,
    pub app: String,
    pub ok: bool,
    pub exit_ip: Option<String>,
    pub latency_ms: Option<i64>,
    pub error_code: Option<String>,
    pub checked_at: String,
}

impl From<EgressCheckRow> for EgressCheckDto {
    fn from(row: EgressCheckRow) -> Self {
        Self {
            source: row.source,
            device_id: row.device_id,
            app: row.app,
            ok: row.ok,
            exit_ip: row.exit_ip,
            latency_ms: row.latency_ms,
            error_code: row.error_code,
            checked_at: row.checked_at,
        }
    }
}

/// EgressProxy（所有代理端点共用；永远不含凭据值）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressProxyDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub proxy_type: String,
    pub host: String,
    pub port: i64,
    pub has_credentials: bool,
    pub region: Option<String>,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub bound_devices: i64,
    pub bound_apps: i64,
    pub last_check: Option<EgressCheckDto>,
    pub created_at: String,
    pub updated_at: String,
}

impl EgressProxyDto {
    pub fn new(
        record: EgressProxyRecord,
        bound_devices: i64,
        bound_apps: i64,
        last_check: Option<EgressCheckRow>,
    ) -> Self {
        let has_credentials = record.has_credentials();
        Self {
            id: record.id,
            name: record.name,
            proxy_type: record.kind,
            host: record.host,
            port: record.port,
            has_credentials,
            region: record.region,
            tags: record.tags,
            enabled: record.enabled,
            bound_devices,
            bound_apps,
            last_check: last_check.map(EgressCheckDto::from),
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

/// GET /egress-proxies
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressProxyListResponse {
    pub total: i64,
    pub next_cursor: Option<String>,
    pub items: Vec<EgressProxyDto>,
}

/// 详情页的绑定行。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressProxyBindingDto {
    pub device_id: String,
    pub device_name: String,
    pub app: String,
    pub updated_at: String,
}

impl From<DeviceEgressRow> for EgressProxyBindingDto {
    fn from(row: DeviceEgressRow) -> Self {
        Self {
            device_name: row.device_name.unwrap_or_else(|| row.device_id.clone()),
            device_id: row.device_id,
            app: row.app,
            updated_at: row.updated_at,
        }
    }
}

/// GET /egress-proxies/{id}：EgressProxy + bindings + recentChecks。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressProxyDetailResponse {
    #[serde(flatten)]
    pub proxy: EgressProxyDto,
    pub bindings: Vec<EgressProxyBindingDto>,
    pub recent_checks: Vec<EgressCheckDto>,
}

/// POST /egress-proxies 请求体。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateEgressProxyRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub proxy_type: String,
    pub host: String,
    pub port: i64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// PATCH /egress-proxies/{id} 请求体：省略 = 不改，null = 清除（仅可空字段）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateEgressProxyRequest {
    #[serde(default, deserialize_with = "double_option")]
    pub name: Option<Option<String>>,
    #[serde(rename = "type", default, deserialize_with = "double_option")]
    pub proxy_type: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub host: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub port: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub username: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub password: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub region: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub tags: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "double_option")]
    pub enabled: Option<Option<bool>>,
}

impl UpdateEgressProxyRequest {
    /// 是否一个字段都没给。
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.proxy_type.is_none()
            && self.host.is_none()
            && self.port.is_none()
            && self.username.is_none()
            && self.password.is_none()
            && self.region.is_none()
            && self.tags.is_none()
            && self.enabled.is_none()
    }
}

/// POST /egress-proxies/{id}/probe 请求体（可省略）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressProbeRequest {
    #[serde(default)]
    pub url: Option<String>,
}

/// 探测成功响应（失败走 502 egress_probe_failed）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressProbeResponse {
    pub proxy_id: String,
    pub ok: bool,
    pub exit_ip: String,
    pub latency_ms: i64,
    pub checked_at: String,
    pub url: String,
    pub upstream_status: Option<u16>,
    /// 非致命提醒（如 egress_check_not_recorded：探测成功但结果没能落库）。
    pub warnings: Vec<Warning>,
}

/// 设备级 / 应用级绑定（GET / PUT 共用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEgressBindingDto {
    pub app: String,
    pub mode: String,
    pub proxy_id: Option<String>,
    pub proxy_name: Option<String>,
    pub updated_at: String,
}

impl From<DeviceEgressRow> for DeviceEgressBindingDto {
    fn from(row: DeviceEgressRow) -> Self {
        Self {
            app: row.app,
            mode: row.mode,
            proxy_id: row.proxy_id,
            proxy_name: row.proxy_name,
            updated_at: row.updated_at,
        }
    }
}

/// GET /devices/{id}/egress。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEgressResponse {
    pub device_id: String,
    pub bindings: Vec<DeviceEgressBindingDto>,
    pub device_state: Option<Value>,
    pub warnings: Vec<Warning>,
}

/// PUT /devices/{id}/egress 的单行输入。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceEgressBindingInput {
    pub app: String,
    /// 代理 id / "__DIRECT__" / null（该行不保留）。
    #[serde(default)]
    pub proxy_id: Option<String>,
}

/// PUT /devices/{id}/egress 请求体（全量替换）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceEgressPutRequest {
    pub bindings: Vec<DeviceEgressBindingInput>,
}

/// PUT /devices/{id}/egress 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEgressPutResponse {
    pub device_id: String,
    pub changed: bool,
    pub bindings: Vec<DeviceEgressBindingDto>,
    pub pending_apply: bool,
    pub warnings: Vec<Warning>,
}

/// POST /devices/egress/apply 的 assignments 形态单行。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressAssignmentInput {
    pub device_id: String,
    pub app: String,
    #[serde(default)]
    pub proxy_id: Option<String>,
}

/// POST /devices/egress/apply 请求体。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressApplyRequest {
    #[serde(default)]
    pub devices: Option<crate::api::job_dto::DevicesSpec>,
    #[serde(default)]
    pub assignments: Option<Vec<EgressAssignmentInput>>,
    #[serde(default)]
    pub probe: Option<bool>,
    #[serde(default)]
    pub dry_run: Option<bool>,
}

/// 202 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressApplyAcceptedResponse {
    pub job_id: String,
    pub status: String,
    pub kind: String,
    pub total: i64,
    pub created_at: String,
    pub request_id: String,
}

/// dryRun 时逐目标的计划行。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressApplyPlanItem {
    pub device_id: String,
    pub app: String,
    pub mode: String,
    pub proxy_name: Option<String>,
}

/// dryRun 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressApplyDryRunResponse {
    pub dry_run: bool,
    pub total: i64,
    pub plan: Vec<EgressApplyPlanItem>,
    pub warnings: Vec<Warning>,
}
