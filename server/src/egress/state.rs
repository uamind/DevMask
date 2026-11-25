// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 运行时健康状态（只有内存，不落盘）与启动自愈（契约 §3.4 / §3.5 / §4.6）。
//!
//! - health / lastProbe 只存在内存，进程重启即清空（设备端没有数据库）；
//! - fail-open 只在三个时点判定：显式探测失败、写入后读回不一致、启动自愈不一致；
//!   动作统一是 [SystemProxy::clear]（清直连），并且**保留全部配置**；
//! - 启动自愈在开始监听之后异步执行一次，不阻塞 healthz，也不探测。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use device_faker_core::config::Config;
use device_faker_core::revision::FileLock;

use crate::app::AppState;
use crate::egress::file::EgressStore;
use crate::egress::system::{SystemProxy, matches};
use crate::egress::{EGRESS_FILE_NAME, expected_for};
use crate::store;

/// 最近一次探测结果（响应里的 health.lastProbe）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastProbe {
    /// 是否成功。
    pub ok: bool,
    /// 出口 IP（失败时 None）。
    pub exit_ip: Option<String>,
    /// 毫秒耗时。
    pub latency_ms: u64,
    /// 探测时刻（RFC 3339 UTC）。
    pub checked_at: String,
    /// device | app。
    pub scope: &'static str,
    /// proxy | direct。
    pub via: &'static str,
    /// 实际使用的代理名。
    pub proxy: Option<String>,
    /// probe URL 的状态码。
    pub upstream_status: Option<u16>,
    /// 失败时的错误码（egress.probe_failed）。
    pub error_code: Option<&'static str>,
}

/// 健康状态快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthSnapshot {
    /// 是否已降级。
    pub degraded: bool,
    /// 进入降级的时间（首次，不随后续刷新重置）。
    pub degraded_since: Option<String>,
    /// 降级原因：probe_failed | settings_mismatch | settings_unavailable。
    pub degraded_reason: Option<&'static str>,
    /// 降级时「清直连」是否真的落地（ApplyOutcome.matches）。
    ///
    /// 只有 true 才能把 effectiveMode 报成 direct；false 时链路其实还是原样，
    /// 只能靠 degradedReason=settings_unavailable 让调用方知道「降级动作没生效」。
    pub cleared: bool,
    /// 最近一次探测。
    pub last_probe: Option<LastProbe>,
}

/// 进程内的 egress 健康状态。
#[derive(Debug, Default)]
pub struct EgressRuntime {
    health: Mutex<HealthSnapshot>,
}

impl EgressRuntime {
    /// 读取快照。
    pub fn snapshot(&self) -> HealthSnapshot {
        self.lock().clone()
    }

    /// 记录一次探测结果。
    pub fn record_probe(&self, probe: LastProbe) {
        self.lock().last_probe = Some(probe);
    }

    /// 标记降级（degradedSince 只在首次进入时写；cleared 每次按实际结果刷新）。
    ///
    /// cleared 来自清直连的 ApplyOutcome.matches：只有真的切到直连，effectiveMode 才是 direct。
    pub fn degrade(&self, reason: &'static str, cleared: bool) {
        let mut health = self.lock();
        if !health.degraded {
            health.degraded = true;
            health.degraded_since = Some(crate::util::now_rfc3339());
            health.degraded_reason = Some(reason);
        } else if !cleared && reason == "settings_unavailable" {
            // 后续一次降级动作没落地：原因要能解释「为什么还报 proxy」，覆盖为 settings_unavailable
            // （fail-closed 的 probe 失败 reason 仍是 probe_failed，不走这条分支）。
            health.degraded_reason = Some("settings_unavailable");
        }
        health.cleared = cleared;
    }

    /// 恢复（一次成功探测或一次成功的读回校验之后）。
    pub fn recover(&self) {
        let mut health = self.lock();
        health.degraded = false;
        health.degraded_since = None;
        health.degraded_reason = None;
        health.cleared = false;
    }

    /// 清空（DELETE /egress 的第 7 条）。
    pub fn reset(&self) {
        let mut health = self.lock();
        health.degraded = false;
        health.degraded_since = None;
        health.degraded_reason = None;
        health.cleared = false;
        health.last_probe = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HealthSnapshot> {
        self.health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// egress 的存储 + 运行时状态（挂在 [AppState] 上）。
#[derive(Debug, Clone)]
pub struct EgressService {
    store: EgressStore,
    runtime: Arc<EgressRuntime>,
}

impl EgressService {
    /// 由 config.toml 路径推导出 egress.toml（同目录）与备份目录。
    pub fn new(config_path: &Path, backup_dir: Option<PathBuf>) -> Self {
        let path = config_path.with_file_name(EGRESS_FILE_NAME);
        Self {
            store: EgressStore::new(path, backup_dir),
            runtime: Arc::new(EgressRuntime::default()),
        }
    }

    /// egress.toml 存储。
    pub fn store(&self) -> &EgressStore {
        &self.store
    }

    /// 运行时健康状态。
    pub fn runtime(&self) -> &EgressRuntime {
        &self.runtime
    }
}

/// 启动自愈 / 一次对账的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// 读回是否与期望一致。
    pub applied: bool,
    /// 本次是否真的写过 settings。
    pub wrote: bool,
    /// 是否降级。
    pub degraded: bool,
    /// 跳过或降级的原因（只用于日志与测试）。
    pub reason: Option<&'static str>,
}

/// fail-open 动作：清直连 + 记账（**唯一实现**，探测/写端点/自愈共用）。
///
/// 返回是否成功执行了清直连（settings 不可用时为 false，但仍然记账）。
pub fn degrade_to_direct(state: &AppState, reason: &'static str) -> bool {
    let system = SystemProxy::new(state.commands());
    let outcome = system.clear();
    let (cleared, readback_has_proxy) = match &outcome {
        Ok(outcome) => (outcome.matches, readback_has_proxy(&outcome.readback)),
        Err(_) => (false, false),
    };
    // 清直连没落地时不能谎报「已切直连」：原因记 settings_unavailable，effectiveMode 报实际模式
    let degraded_reason = if cleared {
        reason
    } else {
        "settings_unavailable"
    };
    state.egress().runtime().degrade(degraded_reason, cleared);
    if cleared {
        tracing::warn!(
            reason = degraded_reason,
            effective_mode = "direct",
            cleared,
            "egress degraded（fail-open 已切直连；配置保持不变）"
        );
    } else {
        tracing::warn!(
            reason = degraded_reason,
            cleared,
            readback_has_proxy,
            "egress degraded（fail-open 清直连未生效：settings 不可用或写入未落地）"
        );
    }
    cleared
}

/// 读回值里是否仍指向某个代理（**只回布尔**：契约 §3.5 要求降级日志不含 host/port）。
///
/// 判定复用 [crate::egress::system::parse_settings_state]，避免在别处再写一份「直连值」清单。
fn readback_has_proxy(readback: &crate::egress::Readback) -> bool {
    matches!(
        crate::egress::system::parse_settings_state(readback.settings_value.as_deref()),
        crate::egress::system::ProxyState::Proxy { .. }
    )
}

/// 启动自愈：按 egress.toml + config.toml 重算期望值，读回不一致就重写一次。
///
/// 解析失败只记日志、**不写 settings**（宁可不动）。不探测（探测只由显式请求触发）。
pub fn reconcile_once(state: &AppState) -> ReconcileReport {
    let service = state.egress();
    // 与写路径相同的锁序：config 共享锁 → egress 共享锁（保证两文件是同一时刻的一致快照）
    let _config_lock = match FileLock::acquire_shared(&state.store().lock_path()) {
        Ok(lock) => lock,
        Err(error) => {
            tracing::warn!(error = %error, "egress reconcile 跳过：config.toml 加锁失败");
            return ReconcileReport::skipped("config_lock");
        }
    };
    let snapshot = match store::read_snapshot_locked(state.store()) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::warn!(error = %error, "egress reconcile 跳过：config.toml 读取失败");
            return ReconcileReport::skipped("config_read");
        }
    };
    let config = if snapshot.exists {
        match Config::from_toml(&snapshot.text) {
            Ok(config) => config,
            Err(_) => {
                tracing::warn!("egress reconcile 跳过：config.toml 无法解析");
                return ReconcileReport::skipped("config_parse");
            }
        }
    } else {
        match Config::from_toml("") {
            Ok(config) => config,
            Err(_) => {
                tracing::warn!("egress reconcile 跳过：空配置无法建模");
                return ReconcileReport::skipped("config_parse");
            }
        }
    };

    let _egress_lock = match FileLock::acquire_shared(&service.store().lock_path()) {
        Ok(lock) => lock,
        Err(error) => {
            tracing::warn!(error = %error, "egress reconcile 跳过：egress.toml 加锁失败");
            return ReconcileReport::skipped("egress_lock");
        }
    };
    let egress = match service.store().read_locked() {
        Ok(egress) => egress,
        Err(error) => {
            tracing::warn!(error = %error, "egress reconcile 跳过：egress.toml 读取失败");
            return ReconcileReport::skipped("egress_read");
        }
    };
    if let Some(error) = &egress.parse_error {
        tracing::warn!(error = %error, "egress reconcile 跳过：egress.toml 无法解析");
        return ReconcileReport::skipped("egress_parse");
    }

    let expected = expected_for(&config, &egress.model);
    let system = SystemProxy::new(state.commands());
    let outcome = match system.apply(&expected, None) {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::warn!(
                component = error.component(),
                "egress reconcile 跳过：settings 不可用"
            );
            return ReconcileReport::skipped("settings_unavailable");
        }
    };
    if outcome.matches {
        service.runtime().recover();
        tracing::info!(
            wrote = outcome.wrote(),
            effective_mode = "from_config",
            "egress reconcile 完成（读回与期望一致）"
        );
        return ReconcileReport {
            applied: true,
            wrote: outcome.wrote(),
            degraded: false,
            reason: None,
        };
    }

    // 读回不一致：fail-open 清直连，fail-closed 只记账（不动 settings）
    let fail_open = egress.model.probe.fail_open;
    if fail_open {
        degrade_to_direct(state, "settings_mismatch");
    } else {
        service.runtime().degrade("settings_mismatch", false);
        tracing::warn!(
            reason = "settings_mismatch",
            cleared = false,
            "egress reconcile：读回与期望不一致（fail-closed，未改 settings）"
        );
    }
    ReconcileReport {
        applied: false,
        wrote: outcome.wrote(),
        degraded: true,
        reason: Some("settings_mismatch"),
    }
}

impl ReconcileReport {
    fn skipped(reason: &'static str) -> Self {
        Self {
            applied: false,
            wrote: false,
            degraded: false,
            reason: Some(reason),
        }
    }
}

/// 开始监听后异步跑一次自愈：不阻塞 healthz，也不阻塞启动。
pub fn spawn_startup_reconcile(state: AppState) {
    tokio::task::spawn_blocking(move || {
        let _ = reconcile_once(&state);
    });
}

/// 只读判定：期望值与当前读回是否一致（GET /egress 的 applied）。
pub fn readback_matches(state: &AppState, expected: &crate::egress::Expected) -> bool {
    match SystemProxy::new(state.commands()).readback() {
        Ok(readback) => matches(expected, &readback),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrade_keeps_first_since_and_recover_clears() {
        let runtime = EgressRuntime::default();
        assert!(!runtime.snapshot().degraded);
        runtime.degrade("probe_failed", true);
        let first = runtime.snapshot();
        assert!(first.degraded);
        assert!(first.cleared, "清直连落地才算切直连");
        assert_eq!(first.degraded_reason, Some("probe_failed"));
        assert!(first.degraded_since.is_some());
        // 后续降级动作没落地：cleared 必须跟着实际结果刷新，reason/since 保持首次
        runtime.degrade("settings_mismatch", false);
        let second = runtime.snapshot();
        assert_eq!(
            second.degraded_since, first.degraded_since,
            "since 不随后续刷新重置"
        );
        assert_eq!(
            second.degraded_reason,
            Some("probe_failed"),
            "原因也保持首次"
        );
        assert!(!runtime.snapshot().cleared, "cleared 也要按最后一次结果");
        runtime.recover();
        assert!(!runtime.snapshot().degraded);
        assert!(!runtime.snapshot().cleared);
        assert_eq!(runtime.snapshot().degraded_since, None);
    }

    #[test]
    fn record_probe_and_reset() {
        let runtime = EgressRuntime::default();
        runtime.record_probe(LastProbe {
            ok: true,
            exit_ip: Some("203.0.113.7".to_owned()),
            latency_ms: 412,
            checked_at: "2026-11-24T12:00:00Z".to_owned(),
            scope: "device",
            via: "proxy",
            proxy: Some("us_la_01".to_owned()),
            upstream_status: Some(200),
            error_code: None,
        });
        assert!(runtime.snapshot().last_probe.is_some());
        runtime.reset();
        assert!(runtime.snapshot().last_probe.is_none());
        assert!(!runtime.snapshot().degraded);
    }
}
