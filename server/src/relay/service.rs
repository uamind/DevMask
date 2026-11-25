// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! listener 生命周期与运行时状态（契约 §1.5 / §5.4 / §6.1 / §7）。
//!
//! 重建流水线（t2 版；防火墙两步由 t3 插入在「绑定」与「accept」之间）：
//!
//! 1. 算期望态（端口表 / 每个端口的 scope / upstream / allowedUids）；
//! 2. **先停**端口变化或身份变化的旧 listener（先关后开，防自撞 `EADDRINUSE`）；
//! 3. 按 §1.3 直接把新端口 bind 成 **paused listener**（分配与绑定合并，无 TOCTOU）；
//! 4. 调用方写端口表（`egress.toml` 0600）；写失败 → 新 listener **不 accept**；
//! 5. 通过之后才 spawn accept 循环（accept 前已做完 uid 双检的前置：解析 allowedUids）。
//!
//! 幂等短路：期望态与当前已生效态完全一致且不是强制重建 → 不动 listener、不写文件。

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devmask_core::config::Config;
use devmask_core::revision::FileLock;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::app::AppState;
use crate::commands::CommandRunner;
use crate::egress::file::{EgressFile, RelaySettings};
use crate::relay::dto::{
    RelayErrorView, RelayExhausted, RelayFirewallErrorView, RelayFirewallView, RelayLimitsView,
    RelayListenerView, RelayUidFailure, RelayUpstreamFailure, RelayView,
};
use crate::relay::firewall::{AcceptRule, Firewall, FirewallState, RuleSpec};
use crate::relay::http::{self, ProxyContext, Upstream};
use crate::relay::port::{self, PortChange, Scope};
use crate::relay::secret::Credentials;
use crate::relay::uid::{self, Limiter, UidError};
use crate::relay::{
    ListenerError, ListenerKind, ListenerSignals, ListenerSignalsInner, UpstreamFailure,
};
use crate::store;

/// 退避重绑间隔（契约 §7.2：1 s / 5 s / 15 s，最多 3 次）。
const REBIND_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// 设备级中继的运行态（喂给 `expected_for_relay`，决定 `settings` 写什么）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceRelayStatus {
    /// `[relay].enabled` 实际值。
    pub enabled: bool,
    /// `base_port` 实际值。
    pub base_port: u16,
    /// 设备级 listener 是否正在 accept。
    pub listening: bool,
}

impl DeviceRelayStatus {
    /// `[relay].enabled=false` 的状态（不参与任何中继判断）。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            base_port: port::DEFAULT_BASE_PORT,
            listening: false,
        }
    }
}

/// 一条已生效的 listener。
struct LiveListener {
    proxy: String,
    port: u16,
    scope: Scope,
    upstream: Arc<Upstream>,
    allowed_uids: Option<Arc<Vec<u32>>>,
    active: Arc<AtomicUsize>,
    signals: ListenerSignals,
    handle: Option<JoinHandle<()>>,
}

impl LiveListener {
    fn listening(&self) -> bool {
        self.handle.is_some() && self.signals.listener_error().is_none()
    }

    fn view(&self) -> RelayListenerView {
        RelayListenerView {
            proxy: self.proxy.clone(),
            port: self.port,
            scope: self.scope.as_str(),
            listening: self.listening(),
            uid_policy: if self.scope == Scope::Device {
                "all"
            } else {
                "restricted"
            },
            allowed_uids: self
                .allowed_uids
                .as_ref()
                .map(|uids| uids.as_ref().clone())
                .unwrap_or_default(),
            active_connections: self.active.load(Ordering::Relaxed),
            last_error: self.signals.listener_error().map(|error| RelayErrorView {
                code: error.code,
                reason: error.reason,
                at: error.at,
            }),
        }
    }

    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

#[derive(Default)]
struct RelayState {
    settings: RelaySettings,
    at: Option<String>,
    listeners: BTreeMap<String, LiveListener>,
    exhausted: Vec<RelayExhausted>,
    uid_failures: Vec<RelayUidFailure>,
}

/// relay 服务（挂在 [AppState] 上）。
#[derive(Clone)]
pub struct RelayService {
    inner: Arc<Mutex<RelayState>>,
    limiter: Arc<Limiter>,
    /// 是否真的绑定端口。host 测试里为 `false`（并行测试不抢 17800–17899、不碰真实
    /// settings）；此时中继对 API 表现为「未启用」，与 `[relay].enabled=false` 等价。
    binding: bool,
    /// 命令执行器（`iptables -S` / `iptables -A` 全走它，测试可注入假 iptables）。
    commands: Arc<dyn CommandRunner>,
    /// uid 防火墙（三链 + `-S` 现算）。
    firewall: Arc<Firewall>,
    /// 最近一次 apply 后 -S 是否确认规则生效（accept 期分层策略用）。
    firewall_applied: Arc<std::sync::atomic::AtomicBool>,
    /// 最近一次成功 apply 的期望态（t10：view 与复核都用它，不重新推导）。
    applied_spec: Arc<std::sync::Mutex<Option<RuleSpec>>>,
}

impl std::fmt::Debug for RelayService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        formatter
            .debug_struct("RelayService")
            .field("listeners", &state.listeners.len())
            .field("settings", &state.settings)
            .field("binding", &self.binding)
            .finish()
    }
}

impl Default for RelayService {
    fn default() -> Self {
        Self::with_binding(true)
    }
}

impl RelayService {
    /// 新建（无 listener，真实绑定端口）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定是否需要真实绑定端口与命令执行器（host 测试传 `false` + fixture runner）。
    pub fn with_binding(binding: bool) -> Self {
        Self::with_runner(binding, Arc::new(crate::commands::SystemRunner::default()))
    }

    /// 指定命令执行器（生产是 SystemRunner，测试可注入 fixture）。
    pub fn with_runner(binding: bool, commands: Arc<dyn CommandRunner>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RelayState::default())),
            limiter: Limiter::new(),
            binding,
            commands,
            firewall: Arc::new(Firewall::new()),
            firewall_applied: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            applied_spec: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// 不绑定端口的实例（host 测试；中继对 API 表现为未启用）。
    pub fn without_binding() -> Self {
        Self::with_binding(false)
    }

    /// 中继对本进程是否生效（`[relay].enabled` **且** 本实例允许绑定）。
    pub fn effective_enabled(&self, model: &EgressFile) -> bool {
        self.binding && model.relay.enabled
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RelayState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 当前已绑定端口集合（分配时视为「本进程自己的」，契约 §1.3 步骤 4）。
    pub fn bound_ports(&self) -> BTreeSet<u16> {
        self.lock()
            .listeners
            .values()
            .filter(|listener| listener.handle.is_some())
            .map(|listener| listener.port)
            .collect()
    }

    /// 期望防火墙规则集（`P_deny` = 端口表全部 local_port；`P_all` = 正在监听的端口）。
    pub fn rule_spec(&self, model: &EgressFile) -> RuleSpec {
        let mut deny_ports: Vec<u16> = model
            .proxies
            .iter()
            .filter_map(|proxy| proxy.local_port)
            .collect();
        deny_ports.sort_unstable();
        deny_ports.dedup();
        let mut accepts: Vec<AcceptRule> = Vec::new();
        {
            let state = self.lock();
            for listener in state
                .listeners
                .values()
                .filter(|listener| listener.handle.is_some())
            {
                push_accepts(
                    &mut accepts,
                    listener.scope,
                    listener.port,
                    &listener.allowed_uids,
                );
            }
        }
        RuleSpec {
            deny_ports,
            accepts,
        }
    }

    /// 运行时快照（`GET /egress.relay`）。
    ///
    /// `applied` / `allowRules` / `denyRules` 必须是**同一次** `iptables -S` 快照，
    /// 因此这里现采样（无 TTL 缓存；手工 `-F` 后立刻 GET 必须看到 `false`）。
    pub fn view(&self, model: &EgressFile) -> RelayView {
        // 期望态优先用「最近一次成功 apply 的 RuleSpec」：restart/startup 期间模型
        // 可能已经变了而规则还没重建，用模型现推会得出 applied=false 的假窗口（M1）。
        // 没有成功 apply 过时才回退到按模型推导。
        let spec = self
            .applied_spec
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| self.rule_spec(model));
        let firewall_enabled = model.relay.firewall && self.effective_enabled(model);
        let firewall_state = if firewall_enabled {
            let sampled = self.firewall.sample(&*self.commands, &spec);
            // 采样结果同步写回共享旗标：accept 期的分层策略读的就是它，
            // 不能只在 activate 时写一次（规则被手工改掉后要能反映出来）。
            self.firewall_applied
                .store(sampled.applied, Ordering::SeqCst);
            sampled
        } else {
            self.firewall_applied.store(false, Ordering::SeqCst);
            FirewallState::default()
        };
        let state = self.lock();
        let mut listeners: Vec<RelayListenerView> =
            state.listeners.values().map(LiveListener::view).collect();
        listeners.sort_by_key(|listener| (listener.scope != "device", listener.port));
        let upstream_failures = state
            .listeners
            .values()
            .filter_map(|listener| {
                listener
                    .signals
                    .upstream_failure()
                    .map(|failure: UpstreamFailure| RelayUpstreamFailure {
                        proxy: listener.proxy.clone(),
                        port: listener.port,
                        scope: listener.scope.as_str(),
                        reason: failure.reason,
                        at: failure.at,
                    })
            })
            .collect();
        RelayView {
            enabled: self.effective_enabled(model),
            base_port: model.relay.base_port,
            fail_open: model.probe.fail_open,
            at: state.at.clone().unwrap_or_else(crate::util::now_rfc3339),
            limits: RelayLimitsView {
                max_connections: model.relay.max_connections,
                max_connections_per_uid: model.relay.max_connections_per_uid,
            },
            listeners,
            firewall: RelayFirewallView {
                applied: firewall_state.applied,
                rules: firewall_state.rules,
                allow_rules: firewall_state.allow_rules,
                deny_rules: firewall_state.deny_rules,
                last_error: firewall_state.last_error.as_ref().map(|failure| {
                    RelayFirewallErrorView {
                        code: failure.code,
                        reason: failure.reason,
                        op: failure.op,
                        at: failure.at.clone(),
                    }
                }),
            },
            firewall_enabled,
            port_changes: Vec::new(),
            restart_required_apps: Vec::new(),
            upstream_failures,
            exhausted: state.exhausted.clone(),
            uid_failures: state.uid_failures.clone(),
        }
    }

    /// 设备级运行态。
    pub fn device_status(&self, model: &EgressFile) -> DeviceRelayStatus {
        let state = self.lock();
        DeviceRelayStatus {
            enabled: self.effective_enabled(model),
            base_port: model.relay.base_port,
            listening: state
                .listeners
                .values()
                .any(|listener| listener.scope == Scope::Device && listener.listening()),
        }
    }

    /// 该定义当前是否经中继（`relayActive`）。
    pub fn relay_active(&self, model: &EgressFile) -> BTreeMap<String, bool> {
        let state = self.lock();
        let mut active = BTreeMap::new();
        let enabled = self.effective_enabled(model);
        for name in model.proxies.iter().map(|proxy| proxy.name.clone()) {
            let is_active = enabled
                && state
                    .listeners
                    .get(&name)
                    .is_some_and(LiveListener::listening);
            active.insert(name, is_active);
        }
        active
    }

    /// 期望态 → paused listener（尚未 accept）。
    pub async fn prepare(
        &self,
        config: &Config,
        model: &EgressFile,
        commands: &dyn CommandRunner,
        force: bool,
    ) -> Rebuild {
        if force {
            // 显式重建：本函数没有「幂等短路」，管线一定会重跑一遍（t3 在这里重装规则）。
            tracing::debug!("relay 强制重建（POST /egress/relay/restart）");
        }
        if !self.binding {
            // host 测试：不绑定任何端口，也不改端口表。
            return Rebuild {
                model: model.clone(),
                ports_changed: false,
                changes: Vec::new(),
                restart_required_apps: Vec::new(),
                spec: RuleSpec::default(),
                paused: Vec::new(),
                kept: Vec::new(),
                failures: Vec::new(),
                exhausted: Vec::new(),
                blocked: Vec::new(),
                at: crate::util::now_rfc3339(),
            };
        }
        let settings = model.relay;
        self.limiter
            .configure(settings.max_connections, settings.max_connections_per_uid);
        let device_name = port::device_default_name(config).map(str::to_owned);

        // 1) 期望集合（文件顺序）+ 每条的 upstream / allowedUids。
        let mut specs: Vec<Spec> = Vec::new();
        let mut blocked: Vec<RelayUidFailure> = Vec::new();
        for proxy in model
            .proxies
            .iter()
            .filter(|proxy| port::needs_relay(proxy, &settings))
        {
            // 设备级 scope 只对 http 上游成立：settings 的 http_proxy 只能承载 HTTP 代理语义，
            // socks5 若占 base_port 就等于向所有 uid 暴露了一个 SOCKS 入口（契约 D1/P8）。
            // socks5 只按 app scope 生效（带 uid 白名单）。
            let is_device = device_name.as_deref() == Some(proxy.name.as_str())
                && proxy.kind == crate::egress::file::ProxyType::Http;
            let scope = if is_device { Scope::Device } else { Scope::App };
            let referenced = is_device || !port::apps_referencing(config, &proxy.name).is_empty();
            let upstream = Arc::new(Upstream {
                label: proxy.name.clone(),
                host: proxy.host.clone(),
                port: proxy.port,
                credentials: Credentials::from_definition(proxy).map(Arc::new),
            });
            let allowed_uids = if is_device {
                None
            } else {
                match resolve_allowed_uids(commands, config, &proxy.name) {
                    Ok(uids) => Some(Arc::new(uids)),
                    Err(failure) => {
                        blocked.push(failure);
                        continue;
                    }
                }
            };
            specs.push(Spec {
                proxy: proxy.name.clone(),
                previous: proxy.local_port,
                scope,
                referenced,
                upstream,
                allowed_uids,
                kind: ListenerKind::from_proxy_type(proxy.kind),
            });
        }

        // 2) 保留判定 → 哪些旧 listener 可以原样留着。
        let mut kept: BTreeMap<String, LiveListener> = BTreeMap::new();
        let kept_proxies: Vec<String>;
        let mut stopping: Vec<LiveListener> = Vec::new();
        // 本轮已确定（保留或新分配）的端口 → 代理名。
        let mut assignments: BTreeMap<String, u16> = BTreeMap::new();
        // 保留判定用独立集合：不能把设备级端口先塞进绑定阶段的 taken，否则它会被
        // 自己的保留集挡掉（表现为「一个定义都没 bind」的假失败）。
        let mut reserved: BTreeSet<u16> = BTreeSet::new();
        let mut desired: BTreeMap<String, (u16, Scope)> = BTreeMap::new();
        for spec in &specs {
            let port = if spec.scope == Scope::Device {
                settings.base_port
            } else {
                port::retained_port(spec.previous, settings.base_port, false, &reserved)
                    .unwrap_or(0)
            };
            if port != 0 {
                reserved.insert(port);
                desired.insert(spec.proxy.clone(), (port, spec.scope));
            }
        }
        // 绑定阶段真正要跳过的端口（只含「别人的」：kept + 本轮已分配）。
        let mut taken: BTreeSet<u16> = BTreeSet::new();
        {
            let mut state = self.lock();
            let existing = std::mem::take(&mut state.listeners);
            for (name, listener) in existing {
                // 契约 §6.3：restart 的副作用是「**未变**的 listener 不关闭、不中断连接」，
                // 强制重建（memo R14）指的是跳过幂等短路、重新校验规则，而不是打断连接。
                let keeps = specs.iter().any(|spec| {
                    spec.proxy == name
                        && desired.get(&name).is_some_and(|(port, scope)| {
                            *port == listener.port && *scope == listener.scope
                        })
                        && spec.upstream == listener.upstream
                        && spec.allowed_uids == listener.allowed_uids
                        && listener.listening()
                });
                if keeps {
                    kept.insert(name, listener);
                } else {
                    stopping.push(listener);
                }
            }
            state.settings = settings;
            // 先记录下来（后面要拿它算 taken / assignments），再把 map 交回状态。
            for listener in kept.values() {
                assignments.insert(listener.proxy.clone(), listener.port);
                taken.insert(listener.port);
            }
            kept_proxies = kept.keys().cloned().collect();
            state.listeners = kept;
        }
        for listener in stopping {
            listener.stop().await;
        }

        // 3) 绑定：kept 端口算占用；其余按候选序列 bind paused（分配即绑定）。
        let mut paused: Vec<PausedListener> = Vec::new();
        let mut failures: Vec<FailedListener> = Vec::new();
        let mut exhausted: Vec<RelayExhausted> = Vec::new();
        for spec in &specs {
            if assignments.contains_key(&spec.proxy) {
                continue;
            }
            if !spec.referenced {
                // 未被引用：只保留端口表，不监听（契约 §6.1 / §6.2）。
                if let Some(port) = port::retained_port(
                    spec.previous,
                    settings.base_port,
                    spec.scope == Scope::Device,
                    &taken,
                ) {
                    taken.insert(port);
                    assignments.insert(spec.proxy.clone(), port);
                }
                continue;
            }
            let candidates: Vec<u16> = if spec.scope == Scope::Device {
                vec![settings.base_port]
            } else {
                let mut list: Vec<u16> = Vec::new();
                if let Some(port) =
                    port::retained_port(spec.previous, settings.base_port, false, &taken)
                {
                    list.push(port);
                }
                for candidate in port::candidate_ports(&spec.proxy, settings.base_port) {
                    if taken.contains(&candidate) || list.contains(&candidate) {
                        continue;
                    }
                    list.push(candidate);
                }
                list
            };
            let mut bound = None;
            let mut last_error: &'static str = "other";
            for candidate in candidates {
                if taken.contains(&candidate) {
                    continue;
                }
                match bind_loopback(candidate).await {
                    Ok(listener) => {
                        bound = Some((candidate, listener));
                        break;
                    }
                    Err(error_reason) => {
                        last_error = error_reason;
                        continue;
                    }
                }
            }
            match bound {
                Some((port, listener)) => {
                    taken.insert(port);
                    assignments.insert(spec.proxy.clone(), port);
                    paused.push(PausedListener {
                        proxy: spec.proxy.clone(),
                        port,
                        scope: spec.scope,
                        upstream: Arc::clone(&spec.upstream),
                        allowed_uids: spec.allowed_uids.clone(),
                        signals: Arc::new(ListenerSignalsInner::default()),
                        listener,
                        kind: spec.kind,
                    });
                }
                None => {
                    if spec.scope == Scope::Device {
                        // 设备级只有一个候选（base_port）：报真实原因（addr_in_use 等）。
                        failures.push(FailedListener {
                            proxy: spec.proxy.clone(),
                            port: Some(settings.base_port),
                            scope: spec.scope,
                            error: ListenerError::new(last_error),
                        });
                    } else {
                        // 应用级池耗尽：不分配端口、不监听（契约 §1.3 步骤 5）。
                        exhausted.push(RelayExhausted {
                            proxy: spec.proxy.clone(),
                            scope: spec.scope.as_str(),
                        });
                        failures.push(FailedListener {
                            proxy: spec.proxy.clone(),
                            port: None,
                            scope: spec.scope,
                            error: ListenerError::new("pool_exhausted"),
                        });
                    }
                }
            }
        }
        // uid 解析失败的定义：端口表保留（不重分配），不监听。
        for failure in &blocked {
            if let Some(port) = port::retained_port(
                specs
                    .iter()
                    .find(|spec| spec.proxy == failure.proxy)
                    .and_then(|spec| spec.previous)
                    .or_else(|| {
                        model
                            .proxy(&failure.proxy)
                            .and_then(|proxy| proxy.local_port)
                    }),
                settings.base_port,
                false,
                &taken,
            ) {
                taken.insert(port);
                assignments.insert(failure.proxy.clone(), port);
            }
        }

        // 4) 端口表 + 端口变化。
        //    契约 §1.6：`[relay].enabled=false` 时**不释放、不重算**（端口表保留），
        //    只停 listener；因此这一整段在关闭态下跳过（文件一个字节都不动）。
        let mut next = model.clone();
        let mut changes: Vec<PortChange> = Vec::new();
        for proxy in next.proxies.iter_mut().filter(|_| settings.enabled) {
            let wanted_port = if port::needs_relay(proxy, &settings) {
                assignments.get(&proxy.name).copied()
            } else {
                None
            };
            if proxy.local_port != wanted_port {
                changes.push(PortChange {
                    proxy: proxy.name.clone(),
                    from: proxy.local_port,
                    to: wanted_port,
                    reason: reason_for(
                        model.proxy(&proxy.name).and_then(|proxy| proxy.local_port),
                        wanted_port,
                        proxy.has_credentials(),
                    ),
                });
                proxy.local_port = wanted_port;
            }
        }
        let restart_required_apps = port::restart_required_apps(config, &next, &changes);

        // 期望规则集：P_deny = 端口表全部 local_port；P_all = 真正会 accept 的端口
        // （kept 的 + 本轮新绑的）。t3 的 pkg@uid 解析已在 specs 阶段做过。
        let base_spec = self.rule_spec(&next);
        let mut accepts = base_spec.accepts.clone();
        for entry in &paused {
            push_accepts(&mut accepts, entry.scope, entry.port, &entry.allowed_uids);
        }
        let spec = RuleSpec {
            deny_ports: base_spec.deny_ports,
            accepts,
        };

        Rebuild {
            model: next,
            ports_changed: !changes.is_empty(),
            changes,
            restart_required_apps,
            spec,
            paused,
            kept: kept_proxies,
            failures,
            exhausted,
            blocked,
            at: crate::util::now_rfc3339(),
        }
    }

    /// 让 paused listener 开始 accept（`persisted=false` 时全部丢弃）。
    pub async fn activate(&self, rebuild: Rebuild, persisted: bool) -> RelayView {
        let Rebuild {
            model,
            changes,
            restart_required_apps,
            spec,
            paused,
            failures,
            exhausted,
            blocked,
            at,
            ..
        } = rebuild;
        let mut failure_list = failures;
        if persisted {
            // 契约 §5.4 第 7–9 步：先装规则（DENY 先增补 → ALLOW 全量重建）并做同一把
            // 写锁下的 -S 逐条校验，**通过之后才 accept**。§5.5：防火墙失败时应用级端口
            // 全部不监听（宁可不服务，也不裸奔），设备级照常（不依赖防火墙，§0 D2）。
            let firewall_state = if model.relay.firewall {
                let state = self.firewall.apply(&*self.commands, &spec);
                self.firewall_applied.store(state.applied, Ordering::SeqCst);
                if state.applied {
                    *self
                        .applied_spec
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(spec.clone());
                }
                tracing::info!(
                    applied = state.applied,
                    allow_rules = state.allow_rules,
                    deny_rules = state.deny_rules,
                    "relay firewall"
                );
                Some(state)
            } else {
                // 显式关闭：不建链、不建规则（配置选择，不是故障）。
                self.firewall.clear(&*self.commands);
                self.firewall_applied.store(false, Ordering::SeqCst);
                None
            };
            let firewall_blocked = firewall_state.as_ref().is_some_and(|state| !state.applied);
            let firewall_reason = firewall_state
                .as_ref()
                .and_then(|state| state.last_error.as_ref())
                .map(|error| error.reason)
                .unwrap_or("state_mismatch");
            // 限频复核器：accept 期 uid 未知时重新采样 -S（≤1/s），
            // 不能永久信任激活时的那份 applied 快照。
            let revalidator = Arc::new(crate::relay::firewall::Revalidator::new(
                Arc::clone(&self.firewall),
                Arc::clone(&self.commands),
                Arc::clone(&self.applied_spec),
                Arc::clone(&self.firewall_applied),
            ));
            let mut state = self.lock();
            for entry in paused {
                if firewall_blocked && entry.scope == Scope::App {
                    failure_list.push(FailedListener {
                        proxy: entry.proxy,
                        port: Some(entry.port),
                        scope: entry.scope,
                        error: ListenerError {
                            code: "egress_relay_firewall_failed",
                            reason: firewall_reason,
                            at: crate::util::now_rfc3339(),
                        },
                    });
                    drop(entry.listener);
                    continue;
                }
                let signals = Arc::clone(&entry.signals);
                let context = ProxyContext::new(
                    Arc::clone(&entry.upstream),
                    Arc::clone(&self.limiter),
                    Arc::new(AtomicUsize::new(0)),
                    Arc::clone(&signals),
                    entry.allowed_uids.clone(),
                    entry.scope.as_str(),
                    entry.port,
                    entry.kind,
                    Arc::clone(&self.firewall_applied),
                    Some(Arc::clone(&revalidator)),
                );
                let handle =
                    spawn_accept(entry.listener, Arc::clone(&context), Arc::clone(&signals));
                tracing::info!(
                    proxy = %entry.proxy,
                    port = entry.port,
                    scope = entry.scope.as_str(),
                    kind = entry.kind.as_str(),
                    uid_policy = if entry.scope == Scope::Device { "all" } else { "restricted" },
                    "relay listen"
                );
                state.listeners.insert(
                    entry.proxy.clone(),
                    LiveListener {
                        proxy: entry.proxy,
                        port: entry.port,
                        scope: entry.scope,
                        upstream: entry.upstream,
                        allowed_uids: entry.allowed_uids,
                        active: context.active.clone(),
                        signals,
                        handle: Some(handle),
                    },
                );
            }
            state.settings = model.relay;
            state.at = Some(at);
            state.exhausted = exhausted;
            state.uid_failures = blocked;
        } else {
            // 端口表没写成功：socket 全部关闭，绝不 accept（契约 §1.5 步骤 4）。
            for entry in paused {
                failure_list.push(FailedListener {
                    proxy: entry.proxy,
                    port: Some(entry.port),
                    scope: entry.scope,
                    error: ListenerError::new("persist_failed"),
                });
                drop(entry.listener);
            }
            let mut state = self.lock();
            state.at = Some(at);
            state.exhausted = exhausted;
            state.uid_failures = blocked;
        }
        // 绑定失败/uid 失败登记成占位 listener（listening=false + lastError）。
        {
            let mut state = self.lock();
            for failure in failure_list {
                if state.listeners.contains_key(&failure.proxy) {
                    continue;
                }
                let signals: ListenerSignals = Arc::new(ListenerSignalsInner::default());
                signals.set_listener_error(failure.error.clone());
                state.listeners.insert(
                    failure.proxy.clone(),
                    LiveListener {
                        proxy: failure.proxy,
                        port: failure.port.unwrap_or(0),
                        scope: failure.scope,
                        upstream: Arc::new(Upstream {
                            label: String::new(),
                            host: String::new(),
                            port: 0,
                            credentials: None,
                        }),
                        allowed_uids: None,
                        active: Arc::new(AtomicUsize::new(0)),
                        signals,
                        handle: None,
                    },
                );
            }
            for failure in state.uid_failures.clone() {
                let error = ListenerError {
                    code: "egress_relay_uid_unknown",
                    reason: failure.reason,
                    at: crate::util::now_rfc3339(),
                };
                if let Some(listener) = state.listeners.get_mut(&failure.proxy) {
                    listener.signals.set_listener_error(error);
                    continue;
                }
                // 契约 §6.1：listeners[] 要给「应该监听但失败」的那一条（listening=false）。
                let port = model
                    .proxy(&failure.proxy)
                    .and_then(|proxy| proxy.local_port)
                    .unwrap_or(0);
                let signals: ListenerSignals = Arc::new(ListenerSignalsInner::default());
                signals.set_listener_error(error);
                state.listeners.insert(
                    failure.proxy.clone(),
                    LiveListener {
                        proxy: failure.proxy,
                        port,
                        scope: Scope::App,
                        upstream: Arc::new(Upstream {
                            label: String::new(),
                            host: String::new(),
                            port: 0,
                            credentials: None,
                        }),
                        allowed_uids: None,
                        active: Arc::new(AtomicUsize::new(0)),
                        signals,
                        handle: None,
                    },
                );
            }
        }
        let mut view = self.view(&model);
        view.port_changes = changes.iter().map(Into::into).collect();
        view.restart_required_apps = restart_required_apps;
        view
    }

    /// 停止全部 listener 并撤规则（服务退出 / 关闭中继；契约 §5.6）。
    pub async fn shutdown(&self) {
        let stopping: Vec<LiveListener> = {
            let mut state = self.lock();
            std::mem::take(&mut state.listeners).into_values().collect()
        };
        for listener in stopping {
            listener.stop().await;
        }
        self.firewall.clear(&*self.commands);
    }
}

fn push_accepts(
    accepts: &mut Vec<AcceptRule>,
    scope: Scope,
    port: u16,
    allowed_uids: &Option<Arc<Vec<u32>>>,
) {
    match (scope, allowed_uids) {
        (Scope::App, Some(uids)) => {
            for uid in uids.iter() {
                accepts.push(AcceptRule {
                    port,
                    uid: Some(*uid),
                });
            }
        }
        _ => accepts.push(AcceptRule { port, uid: None }),
    }
}

/// 期望态里的一条。
struct Spec {
    proxy: String,
    previous: Option<u16>,
    scope: Scope,
    referenced: bool,
    upstream: Arc<Upstream>,
    allowed_uids: Option<Arc<Vec<u32>>>,
    /// 本地 listener 协议（跟着上游 type 走）。
    kind: ListenerKind,
}

/// 已 bind、尚未 accept 的 listener。
pub struct PausedListener {
    proxy: String,
    port: u16,
    scope: Scope,
    upstream: Arc<Upstream>,
    allowed_uids: Option<Arc<Vec<u32>>>,
    signals: ListenerSignals,
    listener: TcpListener,
    /// 本地 listener 协议（http / socks5）。
    kind: ListenerKind,
}

/// 一次重建的结果。
pub struct Rebuild {
    /// 带新 `local_port` 的模型（调用方负责落盘）。
    pub model: EgressFile,
    /// 端口表是否有变化（有变化就必须写回文件）。
    pub ports_changed: bool,
    /// 端口变化明细。
    pub changes: Vec<PortChange>,
    /// 需要重启的应用。
    pub restart_required_apps: Vec<String>,
    /// 本次的期望防火墙规则集（`activate` 里安装并按 `-S` 校验）。
    spec: RuleSpec,
    paused: Vec<PausedListener>,
    kept: Vec<String>,
    failures: Vec<FailedListener>,
    exhausted: Vec<RelayExhausted>,
    blocked: Vec<RelayUidFailure>,
    at: String,
}

impl Rebuild {
    /// 本次复用了多少个旧 listener（不改端口、不中断连接）。
    pub fn kept_count(&self) -> usize {
        self.kept.len()
    }
}

struct FailedListener {
    proxy: String,
    port: Option<u16>,
    scope: Scope,
    error: ListenerError,
}

fn reason_for(previous: Option<u16>, next: Option<u16>, has_credentials: bool) -> &'static str {
    match (previous, next) {
        (None, Some(_)) => "initial",
        (Some(_), None) => {
            if has_credentials {
                "role_change"
            } else {
                "credentials_change"
            }
        }
        (Some(from), Some(to)) if from == to => "initial",
        (Some(from), Some(to)) => {
            if from == 0 || to == 0 {
                "initial"
            } else if !(port::RELAY_PORT_MIN..=port::RELAY_PORT_MAX).contains(&from) {
                "out_of_range"
            } else {
                "collision"
            }
        }
        (None, None) => "initial",
    }
}

/// 应用级端口的 allowedUids = {0} ∪ 解析出的 uid（升序去重）。
fn resolve_allowed_uids(
    commands: &dyn CommandRunner,
    config: &Config,
    proxy: &str,
) -> Result<Vec<u32>, RelayUidFailure> {
    let mut uids = vec![0u32];
    for package in port::apps_referencing(config, proxy) {
        match uid::resolve_package_uids(commands, package) {
            Ok(found) => {
                for found_uid in found {
                    if !uids.contains(&found_uid) {
                        uids.push(found_uid);
                    }
                }
            }
            Err(error) => {
                return Err(RelayUidFailure {
                    proxy: proxy.to_owned(),
                    port: 0,
                    scope: "app",
                    package: package.to_owned(),
                    user_id: uid::split_package(package).1,
                    reason: error.reason(),
                });
            }
        }
    }
    uids.sort_unstable();
    uids.dedup();
    Ok(uids)
}

/// 绑定面硬断言（契约 §5.1 第 1–2 条）：只绑 IPv4 `127.0.0.1`，绑后复核。
pub async fn bind_loopback(port: u16) -> Result<TcpListener, &'static str> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = TcpListener::bind(address)
        .await
        .map_err(classify_bind_error)?;
    let local = listener.local_addr().map_err(|_| "other")?;
    if local.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || local.port() != port {
        // 明明绑的是字面量，这里不符只能是内核/平台异常：关闭、不监听。
        drop(listener);
        return Err("not_loopback");
    }
    Ok(listener)
}

fn classify_bind_error(error: std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::AddrInUse => "addr_in_use",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::AddrNotAvailable => "address_not_available",
        _ => "other",
    }
}

/// accept 循环（错误 → 退避重绑 1 s / 5 s / 15 s，最多 3 次）。
fn spawn_accept(
    mut listener: TcpListener,
    context: Arc<ProxyContext>,
    signals: ListenerSignals,
) -> JoinHandle<()> {
    let port = context.port;
    tokio::spawn(async move {
        let mut attempts = 0usize;
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    attempts = 0;
                    spawn_connection(stream, peer, Arc::clone(&context));
                }
                Err(error) => {
                    tracing::warn!(
                        proxy = %context.upstream.label,
                        port,
                        error = ?error.kind(),
                        "relay listener exited"
                    );
                    signals.set_listener_error(ListenerError::new("listener_exited"));
                    if attempts >= REBIND_BACKOFF.len() {
                        break;
                    }
                    tokio::time::sleep(REBIND_BACKOFF[attempts]).await;
                    attempts += 1;
                    match bind_loopback(port).await {
                        Ok(rebound) => {
                            signals.clear_listener_error();
                            listener = rebound;
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    })
}

/// 一条连接：限流 → uid 双检（restricted）→ 协议解析。
fn spawn_connection(stream: TcpStream, peer: SocketAddr, context: Arc<ProxyContext>) {
    let active = Arc::clone(&context.active);
    active.fetch_add(1, Ordering::Relaxed);
    tokio::spawn(async move {
        let guard = ActiveGuard(Arc::clone(&active));
        let _ = stream.set_nodelay(true);
        let uid = uid::resolve_peer_uid(context.port, peer.port()).await;
        if let Some(allowed) = &context.allowed_uids {
            // 契约 §4.6 第 4–5 条 + t21 分层策略：
            // - uid 已知且不在 allowedUids -> 关（uid_not_allowed）；
            // - uid 拿不到（/proc 解析失败或受限 ROM）且 firewall.applied=true ->
            //   内核 owner match 已按 uid 过滤过，双检只是第二道防线，
            //   记限频 warning 后**放行**（否则一个 ROM 的 /proc 差异就让整个应用级中继全废）；
            // - uid 拿不到且防火墙未生效 / firewall=false -> 仍 fail-closed 关闭。
            let admitted = uid.is_some_and(|uid| allowed.contains(&uid));
            // 先看旗标，再做一次限频复核（≤1/s）：旗标可能是激活时的旧快照，
            // 规则被手工 -F 之后必须能反映出来（t10 验收要求「不得永久信任旧值」）。
            let uid_unknown_but_enforced = uid.is_none()
                && context.firewall_enforced.load(Ordering::Relaxed)
                && context
                    .revalidate
                    .as_ref()
                    .map(|revalidator| revalidator.check())
                    .unwrap_or(true);
            if !admitted && uid_unknown_but_enforced {
                if context.signals.allow_log() {
                    tracing::warn!(
                        proxy = %context.upstream.label,
                        port = context.port,
                        reason = "uid_unresolved_firewall_enforced",
                        "relay uid 归属失败但防火墙已生效：放行（内核层已按 uid 过滤）"
                    );
                }
            } else if !admitted {
                let reason = if uid.is_some() {
                    "uid_not_allowed"
                } else {
                    "uid_unresolved"
                };
                if context.signals.allow_log() {
                    tracing::warn!(
                        proxy = %context.upstream.label,
                        port = context.port,
                        uid = uid.map_or_else(|| "unknown".to_owned(), |uid| uid.to_string()),
                        reason,
                        "relay client rejected"
                    );
                }
                // drop 即关闭（fail-closed：不读协议字节）
                return;
            }
        }
        let Some(limit) = Arc::clone(&context.limiter).try_acquire(uid) else {
            if context.signals.allow_log() {
                tracing::warn!(
                    proxy = %context.upstream.label,
                    port = context.port,
                    "relay limit reached"
                );
            }
            let mut stream = stream;
            match context.kind {
                ListenerKind::Http => http::reject(&mut stream, 503, "Service Unavailable").await,
                ListenerKind::Socks5 => {
                    crate::relay::socks5::reject(&mut stream, crate::relay::socks5::REP_FAILURE)
                        .await
                }
            }
            return;
        };
        match context.kind {
            ListenerKind::Http => http::serve(stream, Arc::clone(&context)).await,
            ListenerKind::Socks5 => crate::relay::socks5::serve(stream, Arc::clone(&context)).await,
        }
        drop(limit);
        drop(guard);
    });
}

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 从磁盘重建（启动 reconcile / `POST /egress/relay/restart` 共用）。
///
/// 返回 `(rebuild, persisted)`；`persisted=false` 表示端口表写回失败，
/// 调用方必须让新 listener 保持关闭（契约 §1.5 步骤 4）。
pub async fn prepare_from_disk(state: &AppState, force: bool) -> Result<(Rebuild, bool), String> {
    if !state.relay().binding {
        // host 测试不去读文件、不绑定端口。
        let model = EgressFile::empty();
        return Ok((
            Rebuild {
                model,
                ports_changed: false,
                changes: Vec::new(),
                restart_required_apps: Vec::new(),
                spec: RuleSpec::default(),
                paused: Vec::new(),
                kept: Vec::new(),
                failures: Vec::new(),
                exhausted: Vec::new(),
                blocked: Vec::new(),
                at: crate::util::now_rfc3339(),
            },
            true,
        ));
    }
    let _config_lock = FileLock::acquire(&state.store().lock_path())
        .map_err(|error| format!("获取配置锁失败: {error}"))?;
    let config_snapshot = store::read_snapshot_locked(state.store())
        .map_err(|error| format!("读取配置失败: {error}"))?;
    let config_text = if config_snapshot.exists {
        config_snapshot.text.as_str()
    } else {
        ""
    };
    let config = Config::from_toml(config_text)
        .map_err(|error| format!("配置解析失败: {}", crate::util::parse_error_message(&error)))?;
    let _egress_lock = state
        .egress()
        .store()
        .acquire()
        .map_err(|error| format!("获取 egress 锁失败: {error}"))?;
    let snapshot = state
        .egress()
        .store()
        .read_locked()
        .map_err(|error| format!("读取 egress.toml 失败: {error}"))?;
    if let Some(error) = &snapshot.parse_error {
        return Err(error.clone());
    }
    let rebuild = state
        .relay()
        .prepare(&config, &snapshot.model, state.commands(), force)
        .await;
    let mut persisted = true;
    if rebuild.ports_changed {
        match state
            .egress()
            .store()
            .write_locked(&snapshot, &rebuild.model)
        {
            Ok(outcome) => {
                if outcome.changed {
                    tracing::info!(
                        local_ports = rebuild.changes.len(),
                        "relay 端口表已写回 egress.toml"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "relay 端口表写回失败（不 accept）");
                persisted = false;
            }
        }
    }
    Ok((rebuild, persisted))
}

/// 启动入口：按文件重建 listener，等待 accept 起来，再交给 egress 自愈写 `settings`。
pub async fn startup(state: AppState) {
    if !state.relay().binding {
        return;
    }
    match prepare_from_disk(&state, false).await {
        Ok((rebuild, persisted)) => {
            let model = rebuild.model.clone();
            if !model.relay.enabled {
                state.relay().shutdown().await;
                tracing::info!("relay 未启用（[relay].enabled=false）");
                return;
            }
            let view = state.relay().activate(rebuild, persisted).await;
            let listening = view
                .listeners
                .iter()
                .filter(|listener| listener.listening)
                .count();
            tracing::info!(listeners = listening, enabled = view.enabled, "relay ready");
        }
        Err(error) => {
            tracing::warn!(error = %error, "relay 启动重建跳过");
        }
    }
}

/// 供测试与排障：当前允许 uid 的解析错误原因（`UidError`）。
pub fn uid_reason(error: UidError) -> &'static str {
    error.reason()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bind_loopback_asserts_ipv4_loopback_and_reports_conflicts() {
        // 借 OS 拿一个空闲端口，再释放
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("probe bind");
        let port = probe.local_addr().expect("addr").port();
        drop(probe);

        let listener = bind_loopback(port).await.expect("必须能绑定回环字面量");
        let address = listener.local_addr().expect("local_addr");
        assert!(address.is_ipv4(), "只能 AF_INET: {address}");
        assert_eq!(
            address.ip().to_string(),
            "127.0.0.1",
            "只能 127.0.0.1: {address}"
        );
        assert_eq!(address.port(), port);

        // 同端口第二次绑定必须失败（addr_in_use）——这是「本进程已持有/被外部占用」的判据
        assert_eq!(bind_loopback(port).await.err(), Some("addr_in_use"));
        drop(listener);
        // 释放后可以再次绑定（同端口重建不依赖 SO_REUSEPORT）
        let again = bind_loopback(port).await.expect("释放后必须能重新绑定");
        drop(again);
    }
}
