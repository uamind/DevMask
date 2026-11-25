// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 设备内认证中继（relay）：只监听 `127.0.0.1` 的本地代理 + 上游凭据注入。
//!
//! 契约：docs/plans/relay-contract.md（§1 端口、§2 egress.toml、§3 生效规则、§4 数据面、
//! §5.1–§5.4 绑定面与生命周期、§6 API、§7 失败与降级）。模块划分：
//!
//! - [`port`]：端口分配（FNV-1a 32 + 线性探测，17800–17899）与保留/重分配判定；
//! - [`secret`]：凭据的内存载体（Drop 即 zeroize，Debug 永不出明文）；
//! - [`basic`]：HTTP Basic 的 base64 编码（不引入新依赖）；
//! - [`http`]：本地 HTTP 代理数据面（CONNECT + absolute-URI、32 KiB 头、覆盖式注入）；
//! - [`uid`]：`/proc/net/tcp` 的 uid 归属、`pkg@uid` 解析与并发限流；
//! - [`service`]：listener 生命周期、运行态与重建流水线；
//! - [`dto`]：`GET /egress.relay` 的响应视图、4 个警告码的 details 形状。
//!
//! 硬约束（逐条对应契约，改动前先读契约）：
//!
//! 1. bind 只接受 IPv4 字面量 `127.0.0.1:<local_port>`，bind 之后用 `local_addr()` 复核
//!    AF_INET + `127.0.0.1` + 端口相符，不符就关闭 socket、不监听（`reason="not_loopback"`）；
//! 2. 凭据只在内存（[`secret::Secret`]），重载即 zeroize；日志只记代理名与错误类别；
//! 3. 重建顺序固定为 **先停旧 listener → 再 bind paused → 写端口表 → 才 accept**：
//!    先建后关会在换端口时自撞 `EADDRINUSE`（误重分配），也会让 `GET /egress` 短暂
//!    展示两个 listener；
//! 4. restricted 端口 fail-closed：`pkg@uid` 解析失败 → 不监听；accept 期再校验一次
//!    uid（`uid_unresolved` / `uid_not_allowed` 直接 close）；
//! 5. 设备级端口绑定失败 → **不写** `settings` 指向中继（由 [`DeviceRelayStatus`] 表达）。
//!
//! 本轮（t2）**不含** `iptables` 防火墙（t3）与 SOCKS5 服务端（t5）：
//! `type="socks5"` 的定义暂不分配中继端口（否则注入侧会指向没人监听的端口），
//! `relay.firewall` 只回显 `applied=false` / `rules=0`。

pub mod basic;
pub mod dto;
pub mod firewall;
pub mod http;
pub mod net;
pub mod pipe;
pub mod port;
pub mod secret;
pub mod service;
pub mod socks5;
pub mod uid;

use std::sync::{Arc, Mutex};

pub use dto::{
    PortChangeView, RelayErrorView, RelayExhausted, RelayFirewallView, RelayLimitsView,
    RelayListenerView, RelayUidFailure, RelayUpstreamFailure, RelayView,
};
pub use firewall::{AcceptRule, Firewall, FirewallState, RuleSpec};
pub use port::{
    DEFAULT_BASE_PORT, DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_CONNECTIONS_PER_UID,
    MAX_CONNECTIONS_LIMIT, PortChange, RELAY_POOL_SIZE, RELAY_PORT_COUNT, RELAY_PORT_MAX,
    RELAY_PORT_MIN, Scope,
};
pub use secret::{Credentials, Secret};
pub use service::{DeviceRelayStatus, RelayService, prepare_from_disk, startup};

/// listener 的协议类型（由上游 `type` 决定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerKind {
    /// 上游 `http` → 本地 HTTP 代理（CONNECT / absolute-URI）。
    Http,
    /// 上游 `socks5` → 本地 SOCKS5 服务端（无本地认证）。
    Socks5,
}

impl ListenerKind {
    /// 由上游类型映射。
    pub fn from_proxy_type(kind: crate::egress::file::ProxyType) -> Self {
        match kind {
            crate::egress::file::ProxyType::Http => ListenerKind::Http,
            crate::egress::file::ProxyType::Socks5 => ListenerKind::Socks5,
        }
    }

    /// 日志用的字面量。
    pub fn as_str(self) -> &'static str {
        match self {
            ListenerKind::Http => "http",
            ListenerKind::Socks5 => "socks5",
        }
    }
}

/// 一个 listener 最近一次绑定/退出失败（`listeners[].lastError`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerError {
    /// `egress_relay_bind_failed`（绑定失败）或 `egress_relay_uid_unknown`（uid 解析失败）。
    pub code: &'static str,
    /// `addr_in_use` / `permission_denied` / `address_not_available` / `not_loopback` /
    /// `pool_exhausted` / `persist_failed` / `listener_exited` / `other`。
    pub reason: &'static str,
    /// 失败时刻（RFC 3339 UTC）。
    pub at: String,
}

impl ListenerError {
    /// 构造绑定/退出失败。
    pub fn new(reason: &'static str) -> Self {
        Self {
            code: "egress_relay_bind_failed",
            reason,
            at: crate::util::now_rfc3339(),
        }
    }
}

/// 数据面最近一次到上游的握手失败（`egress_relay_upstream_unreachable`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamFailure {
    /// `connect` / `dns` / `timeout` / `auth_failed` / `protocol_error`。
    pub reason: &'static str,
    /// 失败时刻（RFC 3339 UTC）。
    pub at: String,
}

impl UpstreamFailure {
    /// 构造一次上游失败。
    pub fn new(reason: &'static str) -> Self {
        Self {
            reason,
            at: crate::util::now_rfc3339(),
        }
    }
}

/// 每个 listener 的运行时信号（accept 循环写、`GET /egress` 读）。
///
/// 与 listener 的绑定/握手状态放在一起，避免 accept 循环反向持有 [`RelayService`] 的锁。
#[derive(Debug, Default)]
pub struct ListenerSignalsInner {
    listener_error: Mutex<Option<ListenerError>>,
    upstream_failure: Mutex<Option<UpstreamFailure>>,
    limit_log: Mutex<Option<std::time::Instant>>,
}

/// 共享句柄。
pub type ListenerSignals = Arc<ListenerSignalsInner>;

/// 同一条限流/拒绝日志的最小间隔（契约 §4.6：同类事件 10 s 最多一行）。
pub const LOG_QUIET_PERIOD: std::time::Duration = std::time::Duration::from_secs(10);

impl ListenerSignalsInner {
    /// 记录绑定/退出失败。
    pub fn set_listener_error(&self, error: ListenerError) {
        *lock(&self.listener_error) = Some(error);
    }

    /// 清除绑定失败（重新监听成功）。
    pub fn clear_listener_error(&self) {
        *lock(&self.listener_error) = None;
    }

    /// 读取绑定失败。
    pub fn listener_error(&self) -> Option<ListenerError> {
        lock(&self.listener_error).clone()
    }

    /// 记录一次上游失败。
    pub fn set_upstream_failure(&self, failure: UpstreamFailure) {
        *lock(&self.upstream_failure) = Some(failure);
    }

    /// 上游握手成功 → 警告消失（契约 §6.4）。
    pub fn clear_upstream_failure(&self) {
        *lock(&self.upstream_failure) = None;
    }

    /// 读取上游失败。
    pub fn upstream_failure(&self) -> Option<UpstreamFailure> {
        lock(&self.upstream_failure).clone()
    }

    /// 限速判定：距上次同类日志超过 [`LOG_QUIET_PERIOD`] 才返回 true。
    pub fn allow_log(&self) -> bool {
        let mut last = lock(&self.limit_log);
        match last.as_ref() {
            Some(at) if at.elapsed() < LOG_QUIET_PERIOD => false,
            _ => {
                *last = Some(std::time::Instant::now());
                true
            }
        }
    }
}

/// 启动顺序入口（`serve()` 调用）：先把 listener 拉起来，再交给 egress 自愈去写 `settings`。
///
/// 反序会让 `settings` 短暂指向还没人监听的端口（契约 §7.1 决策 #1）。
pub fn spawn_startup(state: crate::app::AppState) {
    tokio::spawn(async move {
        service::startup(state.clone()).await;
        let egress_state = state;
        let _ = tokio::task::spawn_blocking(move || {
            crate::egress::reconcile_once(&egress_state);
        })
        .await;
    });
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
