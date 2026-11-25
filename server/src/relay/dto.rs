// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! `GET /egress.relay` 的响应视图与 4 个 relay 警告码（契约 §6.1 / §6.4）。
//!
//! 这里的每个结构体都**没有凭据字段**（契约 §4.8）：代理名、端口、scope、reason 是全部。

use serde::Serialize;

use crate::dto::Warning;
use crate::relay::port::PortChange;

/// 并发上限视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayLimitsView {
    /// 全局并发上限。
    pub max_connections: u32,
    /// 单 uid 并发上限（`0` = 不限）。
    pub max_connections_per_uid: u32,
}

/// `listeners[].lastError`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayErrorView {
    /// 恒为 `egress_relay_bind_failed`。
    pub code: &'static str,
    /// `addr_in_use` / ... （契约 §6.4）。
    pub reason: &'static str,
    /// 失败时刻（RFC 3339 UTC）。
    pub at: String,
}

/// `relay.firewall.lastError`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayFirewallErrorView {
    /// 恒为 `egress_relay_firewall_failed`。
    pub code: &'static str,
    /// 失败原因。
    pub reason: &'static str,
    /// `apply` / `clear`。
    pub op: &'static str,
    /// 失败时刻。
    pub at: String,
}

/// `relay.firewall`（t3 落地前恒为未落地）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayFirewallView {
    /// 期望规则是否已落地。
    pub applied: bool,
    /// 三条链规则总条数。
    pub rules: usize,
    /// `DM_EGRESS_RLY_ALLOW` 条数。
    pub allow_rules: usize,
    /// `DM_EGRESS_RLY_DENY` 条数。
    pub deny_rules: usize,
    /// 最近一次失败。
    pub last_error: Option<RelayFirewallErrorView>,
}

/// `relay.listeners[]`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayListenerView {
    /// 代理名。
    pub proxy: String,
    /// 监听端口（等于 `local_port`）。
    pub port: u16,
    /// `device` / `app`。
    pub scope: &'static str,
    /// 是否正在 accept。
    pub listening: bool,
    /// `all` / `restricted`。
    pub uid_policy: &'static str,
    /// `restricted` 时允许的 uid（升序，含 0）；`all` 时为 `[]`。
    pub allowed_uids: Vec<u32>,
    /// 当前连接数（含握手中）。
    pub active_connections: usize,
    /// 最近一次绑定/退出失败。
    pub last_error: Option<RelayErrorView>,
}

/// `relay.portChanges[]`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortChangeView {
    /// 代理名。
    pub proxy: String,
    /// 变化前（`null` = 首次分配）。
    pub from: Option<u16>,
    /// 变化后（`null` = 释放）。
    pub to: Option<u16>,
    /// `initial` / `collision` / `out_of_range` / `addr_in_use` / `role_change` / `credentials_change`。
    pub reason: &'static str,
}

impl From<&PortChange> for PortChangeView {
    fn from(change: &PortChange) -> Self {
        Self {
            proxy: change.proxy.clone(),
            from: change.from,
            to: change.to,
            reason: change.reason,
        }
    }
}

/// 数据面最近一次上游失败（不出现在 JSON 里，只喂 `warnings`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayUpstreamFailure {
    /// 代理名。
    pub proxy: String,
    /// 端口。
    pub port: u16,
    /// `device` / `app`。
    pub scope: &'static str,
    /// `connect` / `dns` / `timeout` / `auth_failed` / `protocol_error`。
    pub reason: &'static str,
    /// 失败时刻。
    pub at: String,
}

/// 应用级端口的 `pkg@uid` 解析失败（契约 §5.3 / §6.4 `egress_relay_uid_unknown`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayUidFailure {
    /// 代理名。
    pub proxy: String,
    /// 端口（0 表示本轮没有分配）。
    pub port: u16,
    /// 恒为 `app`。
    pub scope: &'static str,
    /// 解析失败的包名（可带 `@userId`）。
    pub package: String,
    /// 包名里带的 `userId`（没带则 `null`）。
    pub user_id: Option<u32>,
    /// `package_not_installed` / `user_not_found` / `pm_unavailable` / `parse_failed`。
    pub reason: &'static str,
}

/// 池耗尽（端口表里没有 `local_port`）的定义（不出现在 JSON 里，只喂 `warnings`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayExhausted {
    /// 代理名。
    pub proxy: String,
    /// `device` / `app`。
    pub scope: &'static str,
}

/// `GET /egress.relay`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayView {
    /// `[relay].enabled` 的实际生效值。
    pub enabled: bool,
    /// 实际生效的 `base_port`。
    pub base_port: u16,
    /// `[probe].fail_open` 的实际生效值。
    pub fail_open: bool,
    /// 本快照时刻（RFC 3339 UTC）。
    pub at: String,
    /// 并发上限。
    pub limits: RelayLimitsView,
    /// 每个「应该监听」的中继端口一条（设备级在前，其余按端口升序）。
    pub listeners: Vec<RelayListenerView>,
    /// 防火墙状态。
    pub firewall: RelayFirewallView,
    /// 端口变化（仅写响应可能非空）。
    pub port_changes: Vec<PortChangeView>,
    /// 需要重启的应用（仅写响应可能非空）。
    pub restart_required_apps: Vec<String>,
    /// `[relay].firewall` 是否开启（不进 JSON；`false` 时不产生防火墙警告，契约 §5.5）。
    #[serde(skip)]
    pub firewall_enabled: bool,
    /// 数据面上游失败（不进 JSON）。
    #[serde(skip)]
    pub upstream_failures: Vec<RelayUpstreamFailure>,
    /// 池耗尽（不进 JSON）。
    #[serde(skip)]
    pub exhausted: Vec<RelayExhausted>,
    /// 应用级端口 uid 解析失败（不进 JSON）。
    #[serde(skip)]
    pub uid_failures: Vec<RelayUidFailure>,
}

impl RelayView {
    /// 没有任何监听端口的空视图（`[relay].enabled=false` 或尚未重建时）。
    pub fn empty(enabled: bool, base_port: u16, fail_open: bool) -> Self {
        Self {
            enabled,
            base_port,
            fail_open,
            at: crate::util::now_rfc3339(),
            limits: RelayLimitsView {
                max_connections: crate::relay::port::DEFAULT_MAX_CONNECTIONS,
                max_connections_per_uid: crate::relay::port::DEFAULT_MAX_CONNECTIONS_PER_UID,
            },
            listeners: Vec::new(),
            firewall: RelayFirewallView::default(),
            port_changes: Vec::new(),
            restart_required_apps: Vec::new(),
            firewall_enabled: false,
            upstream_failures: Vec::new(),
            exhausted: Vec::new(),
            uid_failures: Vec::new(),
        }
    }

    /// 该代理当前是否经中继（`relayActive`，契约 §6.2）。
    pub fn relay_active(&self, proxy: &str) -> bool {
        self.enabled
            && self
                .listeners
                .iter()
                .any(|listener| listener.proxy == proxy && listener.listening)
    }

    /// 某个代理的监听端口（`relayActive` 为真时才有意义）。
    pub fn listening_port(&self, proxy: &str) -> Option<u16> {
        self.listeners
            .iter()
            .find(|listener| listener.proxy == proxy && listener.listening)
            .map(|listener| listener.port)
    }

    /// `relay.listeners[].port` 是否等于 `base_port`（设备级）。
    pub fn device_listening(&self) -> bool {
        self.listeners
            .iter()
            .any(|listener| listener.scope == "device" && listener.listening)
    }
}

/// 由 relay 视图生成 `warnings`（契约 §6.4；`enabled=false` 时静默）。
pub fn relay_warnings(view: &RelayView) -> Vec<Warning> {
    if !view.enabled {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    for listener in &view.listeners {
        if let Some(error) = &listener.last_error
            // uid 解析失败由 uid_failures 报（带 package/userId）、防火墙失败由
            // relay.firewall.lastError 报（带 op/chain/ports），这里都不重复（§6.4 去重）。
            && error.code != "egress_relay_uid_unknown"
            && error.code != "egress_relay_firewall_failed"
        {
            warnings.push(Warning::new(
                error.code,
                format!(
                    "代理 {} 的中继端口 {} 未能监听（{}）",
                    listener.proxy, listener.port, error.reason
                ),
                Some(serde_json::json!({
                    "proxy": listener.proxy,
                    "port": listener.port,
                    "scope": listener.scope,
                    "reason": error.reason,
                })),
            ));
        }
    }
    for exhausted in &view.exhausted {
        warnings.push(Warning::new(
            "egress_relay_bind_failed",
            format!(
                "代理 {} 没有可用的中继端口（端口池耗尽），该定义不监听",
                exhausted.proxy
            ),
            Some(serde_json::json!({
                "proxy": exhausted.proxy,
                "port": null,
                "scope": exhausted.scope,
                "reason": "pool_exhausted",
            })),
        ));
    }
    // 防火墙未能落地 / 与期望不符（`state_mismatch` 是「命令成功但规则被改过」）。
    if view.firewall_enabled
        && !view.firewall.applied
        && let Some(error) = &view.firewall.last_error
    {
        let ports: Vec<u16> = {
            let mut ports: Vec<u16> = view
                .listeners
                .iter()
                .map(|listener| listener.port)
                .filter(|port| *port != 0)
                .collect();
            ports.sort_unstable();
            ports.dedup();
            ports
        };
        warnings.push(Warning::new(
            error.code,
            format!("中继防火墙未就绪（{}）", error.reason),
            Some(serde_json::json!({
                "op": error.op,
                "reason": error.reason,
                "chain": crate::relay::firewall::ENTRY_CHAIN,
                "ports": ports,
                "exitCode": null,
            })),
        ));
    }
    for failure in &view.uid_failures {
        warnings.push(Warning::new(
            "egress_relay_uid_unknown",
            format!(
                "代理 {} 的应用级端口无法解析 {} 的 uid（{}），该端口不监听",
                failure.proxy, failure.package, failure.reason
            ),
            Some(serde_json::json!({
                "proxy": failure.proxy,
                "port": failure.port,
                "scope": "app",
                "package": failure.package,
                "userId": failure.user_id,
                "reason": failure.reason,
            })),
        ));
    }
    for failure in &view.upstream_failures {
        warnings.push(Warning::new(
            "egress_relay_upstream_unreachable",
            format!(
                "代理 {} 的中继端口 {} 最近一次到上游的握手失败（{}）",
                failure.proxy, failure.port, failure.reason
            ),
            Some(serde_json::json!({
                "proxy": failure.proxy,
                "port": failure.port,
                "scope": failure.scope,
                "reason": failure.reason,
                "at": failure.at,
            })),
        ));
    }
    warnings
}
