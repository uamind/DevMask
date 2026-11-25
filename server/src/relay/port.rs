// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 端口分配（契约 §1.3 / §1.5）：FNV-1a 32 位哈希 + 线性探测，17800–17899。
//!
//! 这里只有**可复算的部分**：候选序列与「保留优先」判定。真正的绑定由
//! [`crate::relay::service`] 直接做成 **paused listener**（契约 §1.3 步骤 4：分配与绑定合并，
//! 不再「探测后关闭再重绑」，消除 TOCTOU），因此本模块不碰 socket。

use std::collections::BTreeSet;

use devmask_core::config::{Config, DIRECT_MARKER};

use crate::egress::file::{EgressFile, ProxyDefinition, RelaySettings};

/// 保留区间下界（含）。
pub const RELAY_PORT_MIN: u16 = 17800;
/// 保留区间上界（含）。
pub const RELAY_PORT_MAX: u16 = 17899;
/// 区间容量（100）。
pub const RELAY_PORT_COUNT: u16 = RELAY_PORT_MAX - RELAY_PORT_MIN + 1;
/// 应用级端口池容量：去掉 `base_port` 后的 99 个。
pub const RELAY_POOL_SIZE: u16 = RELAY_PORT_COUNT - 1;
/// `[relay].base_port` 默认值。
pub const DEFAULT_BASE_PORT: u16 = RELAY_PORT_MIN;
/// 全局并发默认值（契约 §4.6）。
pub const DEFAULT_MAX_CONNECTIONS: u32 = 256;
/// 单 uid 并发默认值（`0` = 不限）。
pub const DEFAULT_MAX_CONNECTIONS_PER_UID: u32 = 64;
/// `max_connections` 硬上限（契约 §2.2）。
pub const MAX_CONNECTIONS_LIMIT: u32 = 1024;

/// 32 位 FNV-1a（契约 §1.3 步骤 2，逐字节定义，可复算）。
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

/// 首选下标 `i0 = h mod 99`。
pub fn preferred_index(name: &str) -> u16 {
    (fnv1a32(name.as_bytes()) % u32::from(RELAY_POOL_SIZE)) as u16
}

/// `port(i) = 17800 + ((B - 17800 + 1 + i) mod 100)`（契约 §1.3 步骤 3）。
///
/// `i` 最大 98，因此 `port(i)` 恒不等于 `B`，无需额外排除。
pub fn candidate_port(base_port: u16, index: u16) -> u16 {
    let base = base_port.wrapping_sub(RELAY_PORT_MIN);
    RELAY_PORT_MIN + ((base + 1 + index) % RELAY_PORT_COUNT)
}

/// 某定义名的完整候选序列（99 个，按探测顺序）。
pub fn candidate_ports(name: &str, base_port: u16) -> Vec<u16> {
    let start = preferred_index(name);
    (0..RELAY_POOL_SIZE)
        .map(|step| candidate_port(base_port, (start + step) % RELAY_POOL_SIZE))
        .collect()
}

/// 「保留优先」判定（契约 §1.3 步骤 1 / §1.5 C1/C2/C4）。
///
/// 返回 `Some(port)` = 沿用；`None` = 需要重分配。判定只看文件与占用集，不看 socket：
/// 可绑定性由调用方在 bind 阶段决定（C3）。
pub fn retained_port(
    previous: Option<u16>,
    base_port: u16,
    is_device: bool,
    taken: &BTreeSet<u16>,
) -> Option<u16> {
    let port = previous?;
    if !(RELAY_PORT_MIN..=RELAY_PORT_MAX).contains(&port) {
        return None; // C2：越界
    }
    if taken.contains(&port) {
        return None; // C1：同轮已被前面的定义占用
    }
    if is_device {
        // 角色变化：成为设备级就必须占 base_port，不再是它就必须回池。
        return (port == base_port).then_some(port);
    }
    if port == base_port {
        return None; // C2：应用级不得占用 base_port
    }
    Some(port)
}

/// 一次端口变化（`relay.portChanges[]`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortChange {
    /// 代理名。
    pub proxy: String,
    /// 变化前（`None` = 首次分配 / 之前没有端口）。
    pub from: Option<u16>,
    /// 变化后（`None` = 释放）。
    pub to: Option<u16>,
    /// `initial` / `collision` / `out_of_range` / `addr_in_use` / `role_change` / `credentials_change`。
    pub reason: &'static str,
}

/// 该定义是否需要中继（契约 §0 D1）。
///
/// 两条同时成立：`[relay].enabled` 且带凭据。`http` 与 `socks5` 都参与：
/// 本地 listener 的协议跟着上游 `type` 走（http → HTTP 代理，socks5 → SOCKS5 服务端）。
pub fn needs_relay(proxy: &ProxyDefinition, settings: &RelaySettings) -> bool {
    settings.enabled && proxy.has_credentials()
}

/// 设备级默认定义名（`[egress].default`；空串与 `__DIRECT__` 视为没有）。
pub fn device_default_name(config: &Config) -> Option<&str> {
    config
        .egress
        .as_ref()
        .and_then(|section| section.default.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != DIRECT_MARKER)
}

/// 引用某定义的应用包名（`[[apps]].egress == name`，保持配置文件顺序）。
pub fn apps_referencing<'a>(config: &'a Config, name: &str) -> Vec<&'a str> {
    config
        .apps
        .iter()
        .filter(|app| app.egress.as_deref().map(str::trim) == Some(name))
        .map(|app| app.package.as_str())
        .collect()
}

/// `restartRequiredApps`：端口发生变化、且被 `[[apps]].egress` 引用的包名（升序去重）。
///
/// 契约 §1.7 第 3 条：设备级引用**不**要求重启应用，所以这里只收集应用级引用。
pub fn restart_required_apps(
    config: &Config,
    model: &EgressFile,
    changes: &[PortChange],
) -> Vec<String> {
    let mut apps: Vec<String> = Vec::new();
    for change in changes {
        // 端口值真的变化才算（释放到 None 也是变化：应用必须重启才知道不再经中继）。
        if change.from == change.to {
            continue;
        }
        if model.proxy(&change.proxy).is_none() {
            continue;
        }
        for package in apps_referencing(config, &change.proxy) {
            let package = package.to_owned();
            if !apps.contains(&package) {
                apps.push(package);
            }
        }
    }
    apps.sort();
    apps
}

/// 期望的监听集合里的一条（不含端口）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredListener {
    /// 代理名。
    pub proxy: String,
    /// `device` / `app`。
    pub scope: Scope,
    /// 是否被引用（未被引用的定义只保留端口表，不监听，契约 §6.1）。
    pub referenced: bool,
}

/// 端口作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// 设备级（`port == base_port`，uid 策略 `all`）。
    Device,
    /// 应用级（uid 策略 `restricted`）。
    App,
}

impl Scope {
    /// 线上字面量。
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Device => "device",
            Scope::App => "app",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::file::ProxyType;

    fn taken(ports: &[u16]) -> BTreeSet<u16> {
        ports.iter().copied().collect()
    }

    #[test]
    fn contract_hash_samples_are_reproducible() {
        // 契约 §1.3 复算样例（B = 17800）。
        let samples = [
            ("hk_resi", 3_732_955_428u32, 48u16, 17849u16),
            ("jp_socks", 1_353_994_617, 30, 17831),
            ("us_la_01", 1_484_863_389, 9, 17810),
            ("jp_tokyo_02", 2_664_631_713, 84, 17885),
        ];
        for (name, hash, index, port) in samples {
            assert_eq!(fnv1a32(name.as_bytes()), hash, "{name} 的 h 与契约不符");
            assert_eq!(preferred_index(name), index, "{name} 的 i0 与契约不符");
            assert_eq!(
                candidate_port(17800, index),
                port,
                "{name} 的首选端口与契约不符"
            );
        }
        assert_eq!(fnv1a32(b"px_12") % 99, 33);
        assert_eq!(fnv1a32(b"px_24") % 99, 33);
        assert_eq!(candidate_port(17800, 33), 17834);
        assert_eq!(candidate_port(17800, 34), 17835);
        // 换 base_port 复算：B=17850、hk_resi（i0=48）→ 17899
        assert_eq!(candidate_port(17850, 48), 17899);
    }

    #[test]
    fn candidate_ports_never_return_base_port_and_are_unique() {
        for base in [17800u16, 17850, 17899] {
            let ports = candidate_ports("hk_resi", base);
            assert_eq!(ports.len(), usize::from(RELAY_POOL_SIZE));
            assert!(!ports.contains(&base), "候选不得等于 base_port: {base}");
            let unique: BTreeSet<u16> = ports.iter().copied().collect();
            assert_eq!(unique.len(), ports.len(), "候选必须互不相同");
            assert!(
                ports
                    .iter()
                    .all(|port| (RELAY_PORT_MIN..=RELAY_PORT_MAX).contains(port))
            );
        }
    }

    #[test]
    fn collision_walks_to_the_next_candidate() {
        let ports = candidate_ports("px_24", 17800);
        assert_eq!(ports[0], 17834);
        assert_eq!(ports[1], 17835, "px_12 占用 17834 后 px_24 应探测到 17835");
    }

    #[test]
    fn retention_prefers_previous_value_within_rules() {
        let empty = taken(&[]);
        // 原值合法 → 沿用（跨重启稳定）
        assert_eq!(
            retained_port(Some(17831), 17800, false, &empty),
            Some(17831)
        );
        // C1：本轮已被别人占用 → 重分配
        assert_eq!(
            retained_port(Some(17831), 17800, false, &taken(&[17831])),
            None
        );
        // C2：越界 / 等于 base_port → 重分配
        assert_eq!(retained_port(Some(17800), 17800, false, &empty), None);
        assert_eq!(retained_port(Some(17900), 17800, false, &empty), None);
        assert_eq!(retained_port(Some(17000), 17800, false, &empty), None);
        // 没有原值 → 需要分配
        assert_eq!(retained_port(None, 17800, false, &empty), None);
        // 设备级必须等于 base_port
        assert_eq!(retained_port(Some(17800), 17800, true, &empty), Some(17800));
        assert_eq!(retained_port(Some(17831), 17800, true, &empty), None);
    }

    #[test]
    fn needs_relay_requires_enabled_and_credentials_for_both_kinds() {
        let mut settings = RelaySettings::default();
        let mut proxy = ProxyDefinition {
            name: "p".to_owned(),
            kind: ProxyType::Http,
            host: "203.0.113.10".to_owned(),
            port: 8080,
            username: None,
            password: None,
            local_port: None,
        };
        assert!(!needs_relay(&proxy, &settings), "无凭据不中继");
        proxy.username = Some("u".to_owned());
        assert!(needs_relay(&proxy, &settings));
        settings.enabled = false;
        assert!(!needs_relay(&proxy, &settings), "关闭中继后不分配");
        settings.enabled = true;
        proxy.kind = ProxyType::Socks5;
        assert!(needs_relay(&proxy, &settings), "SOCKS5 同样走中继（t5）");
    }
}
