// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! egress 响应视图（契约 §1.3 / §1.4 / §1.6 / §1.7）。
//!
//! **凭据边界（契约 §7.1）**：这里的每个结构体都没有 username / password 字段，
//! 唯一信号是 hasCredentials。任何新增字段都必须先过这一条。

use serde::Serialize;

use crate::dto::Warning;
use crate::egress::file::ProxyDefinition;
use crate::egress::state::{HealthSnapshot, LastProbe};

/// 脱敏代理摘要：固定 5 个键（契约 §1.3 upstream / proxies[]）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySummary {
    /// 代理名。
    pub name: String,
    /// http | socks5。
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// 主机。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 是否带凭据（永不带值）。
    pub has_credentials: bool,
}

impl ProxySummary {
    /// 由定义构造（只取 5 个字段）。
    pub fn from_definition(proxy: &ProxyDefinition) -> Self {
        Self {
            name: proxy.name.clone(),
            kind: proxy.kind.as_str(),
            host: proxy.host.clone(),
            port: proxy.port,
            has_credentials: proxy.has_credentials(),
        }
    }
}

/// 应用级 egress 视图（只列显式写了 [[apps]].egress 的记录）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEgressView {
    /// 包名（可带 @userId）。
    pub package: String,
    /// 配置值：代理名或 __DIRECT__。
    pub egress: String,
    /// proxy | direct。
    pub effective: &'static str,
    /// 生效代理名；直连或定义缺失时为 null。
    pub proxy: Option<String>,
}

/// 读回视图（契约 §1.3 readback）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadbackView {
    /// settings get global http_proxy 的原文。
    pub settings_value: Option<String>,
    /// dumpsys 的 Global proxy 行原文。
    pub dumpsys_global_proxy: Option<String>,
    /// 是否与期望一致。
    pub matches: bool,
    /// 读回时刻（RFC 3339 UTC）。
    pub at: String,
}

/// health.lastProbe 视图。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastProbeView {
    /// 是否成功。
    pub ok: bool,
    /// 出口 IP。
    pub exit_ip: Option<String>,
    /// 毫秒耗时。
    pub latency_ms: u64,
    /// 探测时刻。
    pub checked_at: String,
    /// device | app。
    pub scope: &'static str,
    /// proxy | direct。
    pub via: &'static str,
    /// 实际使用的代理名。
    pub proxy: Option<String>,
    /// 上游状态码。
    pub upstream_status: Option<u16>,
    /// 失败错误码。
    pub error_code: Option<&'static str>,
}

impl LastProbeView {
    /// 由运行时记录构造。
    pub fn from_probe(probe: &LastProbe) -> Self {
        Self {
            ok: probe.ok,
            exit_ip: probe.exit_ip.clone(),
            latency_ms: probe.latency_ms,
            checked_at: probe.checked_at.clone(),
            scope: probe.scope,
            via: probe.via,
            proxy: probe.proxy.clone(),
            upstream_status: probe.upstream_status,
            error_code: probe.error_code,
        }
    }
}

/// health 视图（契约 §1.3 health）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    /// 是否降级。
    pub degraded: bool,
    /// 进入降级的时间。
    pub degraded_since: Option<String>,
    /// probe_failed | settings_mismatch。
    pub degraded_reason: Option<&'static str>,
    /// 最近一次探测。
    pub last_probe: Option<LastProbeView>,
}

impl HealthView {
    /// 由运行时快照构造。
    pub fn from_snapshot(snapshot: &HealthSnapshot) -> Self {
        Self {
            degraded: snapshot.degraded,
            degraded_since: snapshot.degraded_since.clone(),
            degraded_reason: snapshot.degraded_reason,
            last_probe: snapshot.last_probe.as_ref().map(LastProbeView::from_probe),
        }
    }
}

/// egress.toml 元数据（只给存在性/大小/时间，不给内容）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressFileView {
    /// 文件是否存在。
    pub exists: bool,
    /// 字节数。
    pub size: u64,
    /// mtime（RFC 3339 UTC）。
    pub mtime: Option<String>,
}

/// GET /egress（以及 PUT/DELETE 响应）里的状态块。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressStateView {
    /// 是否配置过任何出口。
    pub configured: bool,
    /// 期望模式：proxy | direct。
    pub mode: &'static str,
    /// 实际模式（计入 fail-open）。
    pub effective_mode: &'static str,
    /// 设备级默认代理名。
    pub default: Option<String>,
    /// fail-open 开关。
    pub fail_open: bool,
    /// 探测 URL。
    pub probe_url: String,
    /// 探测超时（秒）。
    pub probe_timeout_seconds: u64,
    /// 排除列表（来自 Android settings，不落 TOML）。
    pub exclusion_list: Vec<String>,
    /// 设备级默认代理的脱敏摘要。
    pub upstream: Option<ProxySummary>,
    /// 全部代理定义（脱敏，保持书写顺序）。
    pub proxies: Vec<ProxySummary>,
    /// 应用级覆盖。
    pub apps: Vec<AppEgressView>,
    /// 方案 A 是否已生效（读回与期望一致）。
    pub applied: bool,
    /// 读回。
    pub readback: ReadbackView,
    /// 健康状态。
    pub health: HealthView,
    /// egress.toml 元数据。
    pub egress_file: EgressFileView,
    /// 警告码。
    pub warnings: Vec<Warning>,
}

/// GET /api/v1/egress 响应。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressResponse {
    /// egress revision（If-Match 用）。
    pub revision: String,
    /// config.toml 的 revision（仅排障/展示）。
    pub config_revision: String,
    /// 状态块。
    #[serde(flatten)]
    pub state: EgressStateView,
}

/// PUT / DELETE /api/v1/egress 响应（WriteResult 的超集）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressWriteResult {
    /// 写入后的 egress revision。
    pub revision: String,
    /// 写入前的 egress revision。
    pub previous_revision: Option<String>,
    /// 写入后的 config revision。
    pub config_revision: String,
    /// 是否演练。
    pub dry_run: bool,
    /// 是否真的落盘。
    pub changed: bool,
    /// 备份 ID。
    pub backup_id: Option<String>,
    /// diff（diff=true 或 dry_run=true 时）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// diff 是否被截断。
    pub diff_truncated: bool,
    /// 状态块。
    #[serde(flatten)]
    pub state: EgressStateView,
}

/// GET /api/v1/egress/proxies 的列表项（脱敏 + 引用关系）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyListItem {
    /// 代理名。
    pub name: String,
    /// http | socks5。
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// 主机。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 是否带凭据。
    pub has_credentials: bool,
    /// 是否为 [egress].default 指向的定义。
    pub used_as_default: bool,
    /// 引用该定义的包名（按 [[apps]] 顺序）。
    pub apps: Vec<String>,
}

impl ProxyListItem {
    /// 由定义 + 引用关系构造。
    pub fn new(proxy: &ProxyDefinition, used_as_default: bool, apps: Vec<String>) -> Self {
        Self {
            name: proxy.name.clone(),
            kind: proxy.kind.as_str(),
            host: proxy.host.clone(),
            port: proxy.port,
            has_credentials: proxy.has_credentials(),
            used_as_default,
            apps,
        }
    }
}

/// GET /api/v1/egress/proxies 响应。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxiesResponse {
    /// egress revision。
    pub revision: String,
    /// config revision。
    pub config_revision: String,
    /// 定义数量。
    pub total: usize,
    /// egress.toml 元数据。
    pub egress_file: EgressFileView,
    /// 列表。
    pub items: Vec<ProxyListItem>,
    /// 警告。
    pub warnings: Vec<Warning>,
}

/// POST /api/v1/egress/probe 响应（契约 §1.6）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResponse {
    /// 成功时恒 true。
    pub ok: bool,
    /// 出口 IP。
    pub exit_ip: String,
    /// 毫秒耗时。
    pub latency_ms: u64,
    /// 探测时刻。
    pub checked_at: String,
    /// device | app。
    pub scope: &'static str,
    /// proxy | direct。
    pub via: &'static str,
    /// 实际使用的代理名。
    pub proxy: Option<String>,
    /// 实际请求的 URL。
    pub url: String,
    /// 上游状态码。
    pub upstream_status: u16,
    /// 应用级探测恒 true。
    pub approximate: bool,
    /// 警告码。
    pub warnings: Vec<Warning>,
}
