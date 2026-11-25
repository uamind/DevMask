// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! uid 防火墙（契约 §5）：三链 `DM_EGRESS_RLY` / `_ALLOW` / `_DENY`，
//! `relay.firewall.applied` 由一次 `iptables -S` **现算**（不缓存、不猜）。
//!
//! 冻结口径（memo R1/R3/R4/R14）：
//!
//! 1. 入口链固定三条跳转且顺序不可变：`-j ALLOW` → `-j DENY` → `-j RETURN`；
//! 2. `ALLOW` 只有 ACCEPT，重建 = `-F` + 按序 `-A`（空链期间全拒，fail-closed）；
//! 3. `DENY` 每端口一条 REJECT，只按 spec 增删，**永不 `-F`**；
//! 4. `applied` / `allowRules` / `denyRules` 来自**同一次**采样（三条链一次快照）；
//! 5. 采样与重建互斥（同一把读写锁）：restart 期间的 `GET` 不会被误报成 `false`；
//! 6. 单次 `-S` 上限 1 s，失败**不** 500；命令成功但内容与期望不符 → `reason="state_mismatch"`
//!    （不用 `command_failed` 冒充：命令其实成功了）。

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::commands::{CommandOutcome, CommandRunner};

/// 入口/分派链（名字 K2 冻结）。
pub const ENTRY_CHAIN: &str = "DM_EGRESS_RLY";
/// 只有 ACCEPT 的叶子链（可重建）。
pub const ALLOW_CHAIN: &str = "DM_EGRESS_RLY_ALLOW";
/// 只有 REJECT 的叶子链（只增删、永不 -F）。
pub const DENY_CHAIN: &str = "DM_EGRESS_RLY_DENY";
/// iptables 程序名。
const PROGRAM: &str = "iptables";
/// 拿锁等待（毫秒）。
const WEIGHT: [&str; 2] = ["-w", "2"];
/// 单次 `-S` 的上限（契约要求 ≤1 s，不吃 CommandRunner 的 5 s）。
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(1);
/// 回环目的地址匹配（只做 IPv4）。
const LOOPBACK: [&str; 3] = ["-o", "lo", "-d"];

/// 一条 ACCEPT：`uid = None` 表示放行全部 uid（设备级端口）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptRule {
    /// 端口。
    pub port: u16,
    /// 允许的 uid（`None` = 全部）。
    pub uid: Option<u32>,
}

/// 期望规则集（由端口表 + 已监听端口推导）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleSpec {
    /// `P_deny`：端口表里全部 `local_port`（升序去重）。
    pub deny_ports: Vec<u16>,
    /// `P_all`：真正在监听的端口及其允许 uid。
    pub accepts: Vec<AcceptRule>,
}

impl RuleSpec {
    /// 端口表为空（中继关闭 / 没有定义）。
    pub fn is_empty(&self) -> bool {
        self.deny_ports.is_empty() && self.accepts.is_empty()
    }

    /// 入口链期望内容（顺序即语义）。
    pub fn entry_rules(&self) -> Vec<String> {
        [
            format!("-j {ALLOW_CHAIN}"),
            format!("-j {DENY_CHAIN}"),
            "-j RETURN".to_owned(),
        ]
        .to_vec()
    }

    /// `ALLOW` 期望内容（root → 设备级 → 应用级逐 uid，端口升序）。
    pub fn allow_rules(&self) -> Vec<String> {
        let mut ports: Vec<u16> = self.accepts.iter().map(|rule| rule.port).collect();
        ports.sort_unstable();
        ports.dedup();
        let mut rules: Vec<String> = Vec::new();
        let mut push = |rule: String| {
            // 去重：allowedUids 含 0（B3）会与 B1 的 root 规则重合，
            // 重复条目会让 -S 与期望不一致 → applied=false → 端口不监听（t9 真机 blocker）。
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        };
        // B1：root 永远放行（排障/自测；也是显式事实）
        for port in &ports {
            push(accept_rule(*port, Some(0)));
        }
        // B2：设备级端口放行全部 uid
        for rule in &self.accepts {
            if rule.uid.is_none() {
                push(accept_rule(rule.port, None));
            }
        }
        // B3：应用级端口按 uid 逐条（uid 升序；uid 0 已由 B1 覆盖）
        let mut per_uid: Vec<(u16, u32)> = self
            .accepts
            .iter()
            .filter_map(|rule| rule.uid.map(|uid| (rule.port, uid)))
            .collect();
        per_uid.sort_unstable();
        for (port, uid) in per_uid {
            push(accept_rule(port, Some(uid)));
        }
        rules
    }

    /// `DENY` 期望内容（每端口一条 REJECT）。
    pub fn deny_rules(&self) -> Vec<String> {
        let mut ports = self.deny_ports.clone();
        ports.sort_unstable();
        ports.dedup();
        ports.iter().map(|port| deny_rule(*port)).collect()
    }
}

fn accept_rule(port: u16, uid: Option<u32>) -> String {
    let mut rule = format!("{} 127.0.0.0/8 -p tcp --dport {port}", LOOPBACK.join(" "));
    if let Some(uid) = uid {
        rule.push_str(&format!(" -m owner --uid-owner {uid}"));
    }
    rule.push_str(" -j ACCEPT");
    rule
}

fn deny_rule(port: u16) -> String {
    format!("-p tcp --dport {port} -j REJECT --reject-with icmp-port-unreachable")
}

/// 防火墙失败（`egress_relay_firewall_failed` 的 details 来源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirewallFailure {
    /// 恒为 `egress_relay_firewall_failed`。
    pub code: &'static str,
    /// `iptables_missing` / `permission_denied` / `command_failed` / `state_mismatch`。
    pub reason: &'static str,
    /// `apply` / `clear`。
    pub op: &'static str,
    /// 失败时刻（RFC 3339 UTC）。
    pub at: String,
}

impl FirewallFailure {
    fn new(reason: &'static str, op: &'static str) -> Self {
        Self {
            code: "egress_relay_firewall_failed",
            reason,
            op,
            at: crate::util::now_rfc3339(),
        }
    }
}

/// `relay.firewall` 的运行时快照（同一次 `-S` 采样）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FirewallState {
    /// 三条链是否与期望逐条一致。
    pub applied: bool,
    /// 三条链规则总条数（入口 3 + ALLOW + DENY，不含 OUTPUT 跳转）。
    pub rules: usize,
    /// `ALLOW` 条数。
    pub allow_rules: usize,
    /// `DENY` 条数。
    pub deny_rules: usize,
    /// 最近一次失败。
    pub last_error: Option<FirewallFailure>,
}

/// 防火墙：规则安装 + `-S` 采样。
#[derive(Debug, Clone, Default)]
pub struct Firewall {
    state: Arc<Mutex<FirewallState>>,
    /// 重建（写）与采样（读）互斥：避免 restart 期间 GET 误报 `applied=false`。
    rebuild: Arc<RwLock<()>>,
}

impl Firewall {
    /// 新建（无规则）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 采样：`GET /egress` 用（读锁 + 三条链一次快照）。
    pub fn sample(&self, commands: &dyn CommandRunner, spec: &RuleSpec) -> FirewallState {
        let _guard = self
            .rebuild
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.sample_locked(commands, spec)
    }

    fn sample_locked(&self, commands: &dyn CommandRunner, spec: &RuleSpec) -> FirewallState {
        let mut state = FirewallState::default();
        let Some((entry, allow, deny)) = self.snapshot(commands, &mut state) else {
            *lock(&self.state) = state.clone();
            return state;
        };
        let expected_entry = spec.entry_rules();
        let expected_allow = spec.allow_rules();
        let expected_deny = spec.deny_rules();
        state.allow_rules = allow.len();
        state.deny_rules = deny.len();
        state.rules = entry.len() + allow.len() + deny.len();
        // 入口链顺序即语义：逐条按元组比较（仍需顺序一致）。
        let entry_ok = entry.len() == expected_entry.len()
            && entry
                .iter()
                .zip(expected_entry.iter())
                .all(|(actual, expected)| rule_tuple(actual) == rule_tuple(expected));
        let allow_ok = rules_set_equivalent(&allow, &expected_allow);
        let deny_ok = rules_set_equivalent(&deny, &expected_deny);
        state.applied = entry_ok && allow_ok && deny_ok && state.last_error.is_none();
        if !state.applied && state.last_error.is_none() {
            // 命令成功、内容不符 = 规则被外部改过（手工 -F / 手工增删）。
            state.last_error = Some(FirewallFailure::new("state_mismatch", "apply"));
        }
        if state.applied {
            state.last_error = None;
        }
        *lock(&self.state) = state.clone();
        state
    }

    /// 读三条链（一次快照）；失败时写 last_error 并返回 None。
    fn snapshot(
        &self,
        commands: &dyn CommandRunner,
        state: &mut FirewallState,
    ) -> Option<(Vec<String>, Vec<String>, Vec<String>)> {
        let mut chains = Vec::new();
        for chain in [ENTRY_CHAIN, ALLOW_CHAIN, DENY_CHAIN] {
            let outcome = commands.run_with_timeout(
                PROGRAM,
                &[WEIGHT[0], WEIGHT[1], "-S", chain],
                SAMPLE_TIMEOUT,
            );
            match outcome {
                CommandOutcome::Ok(stdout) => chains.push(parse_chain(&stdout, chain)),
                CommandOutcome::Failed { stderr, .. } => {
                    let reason = if stderr.to_ascii_lowercase().contains("permission denied") {
                        "permission_denied"
                    } else {
                        "command_failed"
                    };
                    state.last_error = Some(FirewallFailure::new(reason, "apply"));
                    return None;
                }
                CommandOutcome::Unavailable { .. } => {
                    state.last_error = Some(FirewallFailure::new("iptables_missing", "apply"));
                    return None;
                }
            }
        }
        Some((chains[0].clone(), chains[1].clone(), chains[2].clone()))
    }

    /// 安装规则（写锁）：链/跳转幂等 → DENY 增补 → ALLOW 全量重建 → 采样校验。
    pub fn apply(&self, commands: &dyn CommandRunner, spec: &RuleSpec) -> FirewallState {
        let _guard = self
            .rebuild
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if spec.is_empty() {
            return self.clear_locked(commands);
        }
        let mut failure: Option<FirewallFailure> = None;
        for chain in [ENTRY_CHAIN, ALLOW_CHAIN, DENY_CHAIN] {
            self.exec(commands, &["-N", chain], &mut failure);
        }
        if !self.check(commands, &["-C", "OUTPUT", "-j", ENTRY_CHAIN], &mut failure) {
            self.exec(
                commands,
                &["-I", "OUTPUT", "1", "-j", ENTRY_CHAIN],
                &mut failure,
            );
        }
        for target in [ALLOW_CHAIN, DENY_CHAIN, "RETURN"] {
            if !self.check(commands, &["-C", ENTRY_CHAIN, "-j", target], &mut failure) {
                self.exec(commands, &["-A", ENTRY_CHAIN, "-j", target], &mut failure);
            }
        }
        // DENY 先补齐：新端口必须先有 REJECT，才允许出现 ACCEPT。
        for rule in spec.deny_rules() {
            let args = split_rule(&rule);
            let mut check = vec!["-C", DENY_CHAIN];
            check.extend(args.iter().map(String::as_str));
            if !self.check(commands, &check, &mut failure) {
                let mut append = vec!["-A", DENY_CHAIN];
                append.extend(args.iter().map(String::as_str));
                self.exec(commands, &append, &mut failure);
            }
        }
        // ALLOW 全量重建（空链期间全拒 = fail-closed）。
        self.exec(commands, &["-F", ALLOW_CHAIN], &mut failure);
        for rule in spec.allow_rules() {
            let args = split_rule(&rule);
            let mut append = vec!["-A", ALLOW_CHAIN];
            append.extend(args.iter().map(String::as_str));
            self.exec(commands, &append, &mut failure);
        }
        // §5.4 第 10 步：已释放端口的 DENY spec 要删（只按 spec 精确 -D，不做 -F）。
        self.prune_deny(commands, spec);
        let mut state = self.sample_locked(commands, spec);
        if let Some(failure) = failure {
            state.applied = false;
            state.last_error = Some(failure);
        }
        *lock(&self.state) = state.clone();
        state
    }

    /// 卸载（服务退出 / 中继关闭）：先摘跳转，再按「叶子链 → 入口链」删。
    pub fn clear(&self, commands: &dyn CommandRunner) -> FirewallState {
        let _guard = self
            .rebuild
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.clear_locked(commands)
    }

    fn clear_locked(&self, commands: &dyn CommandRunner) -> FirewallState {
        let mut failure: Option<FirewallFailure> = None;
        // 清理是幂等的：链/跳转本来就不存在（-D/-F/-X 退出码 1）不算故障，
        // 只有「命令不可用 / 权限 / 其它非零」才记 firewall_failed(op=clear)。
        self.exec_tolerant(commands, &["-D", "OUTPUT", "-j", ENTRY_CHAIN], &mut failure);
        for chain in [ALLOW_CHAIN, DENY_CHAIN, ENTRY_CHAIN] {
            self.exec_tolerant(commands, &["-F", chain], &mut failure);
            self.exec_tolerant(commands, &["-X", chain], &mut failure);
        }
        let state = FirewallState {
            applied: false,
            rules: 0,
            allow_rules: 0,
            deny_rules: 0,
            last_error: failure.map(|mut failure| {
                failure.op = "clear";
                failure
            }),
        };
        *lock(&self.state) = state.clone();
        state
    }

    /// 删除不在期望端口表里的 DENY 规则（-D 精确删除，永不 -F）。
    fn prune_deny(&self, commands: &dyn CommandRunner, spec: &RuleSpec) {
        let output = match commands.run_with_timeout(
            PROGRAM,
            &[WEIGHT[0], WEIGHT[1], "-S", DENY_CHAIN],
            SAMPLE_TIMEOUT,
        ) {
            CommandOutcome::Ok(stdout) => stdout,
            _ => return,
        };
        for rule in parse_chain(&output, DENY_CHAIN) {
            let Some(port) = dport_of(&rule) else {
                continue;
            };
            if spec.deny_ports.contains(&port) {
                continue;
            }
            let mut full = vec![WEIGHT[0], WEIGHT[1], "-D", DENY_CHAIN];
            let owned = split_rule(&rule);
            full.extend(owned.iter().map(String::as_str));
            let _ = commands.run_with_timeout(PROGRAM, &full, SAMPLE_TIMEOUT);
        }
    }

    /// 幂等清理用：退出码 1（不存在）不算故障。
    fn exec_tolerant(
        &self,
        commands: &dyn CommandRunner,
        args: &[&str],
        failure: &mut Option<FirewallFailure>,
    ) -> bool {
        let mut full = vec![WEIGHT[0], WEIGHT[1]];
        full.extend_from_slice(args);
        match commands.run_with_timeout(PROGRAM, &full, SAMPLE_TIMEOUT) {
            CommandOutcome::Ok(_) => true,
            CommandOutcome::Failed { code, stderr } => {
                if !matches!(code, Some(1)) && failure.is_none() {
                    let reason = if stderr.to_ascii_lowercase().contains("permission denied") {
                        "permission_denied"
                    } else {
                        "command_failed"
                    };
                    *failure = Some(FirewallFailure::new(reason, "clear"));
                }
                false
            }
            CommandOutcome::Unavailable { .. } => {
                if failure.is_none() {
                    *failure = Some(FirewallFailure::new("iptables_missing", "clear"));
                }
                false
            }
        }
    }

    fn exec(
        &self,
        commands: &dyn CommandRunner,
        args: &[&str],
        failure: &mut Option<FirewallFailure>,
    ) -> bool {
        let mut full = vec![WEIGHT[0], WEIGHT[1]];
        full.extend_from_slice(args);
        match commands.run_with_timeout(PROGRAM, &full, SAMPLE_TIMEOUT) {
            CommandOutcome::Ok(_) => true,
            CommandOutcome::Failed { stderr, .. } => {
                if failure.is_none() {
                    let reason = if stderr.to_ascii_lowercase().contains("permission denied") {
                        "permission_denied"
                    } else {
                        "command_failed"
                    };
                    *failure = Some(FirewallFailure::new(reason, "apply"));
                }
                false
            }
            CommandOutcome::Unavailable { .. } => {
                if failure.is_none() {
                    *failure = Some(FirewallFailure::new("iptables_missing", "apply"));
                }
                false
            }
        }
    }

    /// `-C` 判定：0 = 已存在；非 0 = 不存在（不是故障）。
    fn check(
        &self,
        commands: &dyn CommandRunner,
        args: &[&str],
        failure: &mut Option<FirewallFailure>,
    ) -> bool {
        let mut full = vec![WEIGHT[0], WEIGHT[1]];
        full.extend_from_slice(args);
        match commands.run_with_timeout(PROGRAM, &full, SAMPLE_TIMEOUT) {
            CommandOutcome::Ok(_) => true,
            CommandOutcome::Failed { code, .. } => {
                // 1 = 规则不存在（正常分支）；其它退出码才算故障
                if !matches!(code, Some(1) | None) && failure.is_none() {
                    *failure = Some(FirewallFailure::new("command_failed", "apply"));
                }
                false
            }
            CommandOutcome::Unavailable { .. } => {
                if failure.is_none() {
                    *failure = Some(FirewallFailure::new("iptables_missing", "apply"));
                }
                false
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 从规则文本里取 --dport N。
pub fn dport_of(rule: &str) -> Option<u16> {
    let mut tokens = rule.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == "--dport" {
            return tokens.next().and_then(|value| value.parse().ok());
        }
    }
    None
}

/// 解析 `iptables -S <chain>` 输出里属于该链的规则（去掉 `-A <chain> ` 前缀）。
pub fn parse_chain(output: &str, chain: &str) -> Vec<String> {
    let prefix = format!("-A {chain} ");
    output
        .lines()
        .map(str::trim_end)
        .filter_map(|line| line.strip_prefix(&prefix))
        .map(|rule: &str| rule.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// 一条规则解析出的语义元组（与选项顺序、隐式模块标记无关）。
///
/// 真机 iptables -S 与我们的期望串有两类差异：
///
/// 1. 隐式模块标记：-p tcp --dport 会打印成 -p tcp -m tcp --dport，
///    --uid-owner 会打印成 -m owner --uid-owner；
/// 2. 选项顺序：-d/-o/-p 的先后由 iptables 自己决定（真机上 -d 在 -o 之前）。
///
/// 因此比较必须解析成 (target, proto, in/out iface, source, dest, dport, uid-owner,
/// reject-with) 元组集合，而不是逐字比较。改回字面比较会让真机永远 state_mismatch。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuleTuple {
    target: String,
    proto: String,
    in_iface: String,
    out_iface: String,
    source: String,
    dest: String,
    dport: String,
    uid_owner: String,
    reject_with: String,
}

impl RuleTuple {
    fn is_empty(&self) -> bool {
        self.target.is_empty() && self.dport.is_empty() && self.uid_owner.is_empty()
    }
}

/// 把一条 -S 规则文本（已去掉 -A CHAIN 前缀）解析成语义元组。
pub fn rule_tuple(rule: &str) -> RuleTuple {
    let tokens: Vec<&str> = rule.split_whitespace().collect();
    let mut tuple = RuleTuple {
        target: String::new(),
        proto: String::new(),
        in_iface: String::new(),
        out_iface: String::new(),
        source: String::new(),
        dest: String::new(),
        dport: String::new(),
        uid_owner: String::new(),
        reject_with: String::new(),
    };
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        let value = tokens.get(index + 1).copied().unwrap_or("");
        match token {
            // -m tcp / -m owner 只是模块标记，语义由后面的选项承载
            "-m" => {
                index += 2;
                continue;
            }
            "-j" => tuple.target = value.to_owned(),
            "-p" => tuple.proto = value.to_owned(),
            "-i" => tuple.in_iface = value.to_owned(),
            "-o" => tuple.out_iface = value.to_owned(),
            "-s" => tuple.source = value.to_owned(),
            "-d" => tuple.dest = value.to_owned(),
            "--dport" => tuple.dport = value.to_owned(),
            "--uid-owner" => tuple.uid_owner = value.to_owned(),
            "--reject-with" => tuple.reject_with = value.to_owned(),
            _ => {}
        }
        index += 1;
    }
    // --reject-with 的默认值：REJECT 不带 --reject-with 时内核等价于 icmp-port-unreachable
    if tuple.target == "REJECT" && tuple.reject_with.is_empty() {
        tuple.reject_with = "icmp-port-unreachable".to_owned();
    }
    tuple
}

fn rule_tuples(rules: &[String]) -> Vec<RuleTuple> {
    let mut tuples: Vec<RuleTuple> = rules
        .iter()
        .map(|rule| rule_tuple(rule))
        .filter(|tuple| !tuple.is_empty())
        .collect();
    tuples.sort();
    tuples
}

/// 两组规则是否语义等价（按元组集合比较，顺序无关）。
fn rules_set_equivalent(left: &[String], right: &[String]) -> bool {
    rule_tuples(left) == rule_tuples(right)
}

/// 把期望规则文本拆回 argv（简单空白切分：规则里没有需要保留的空格）。
pub fn split_rule(rule: &str) -> Vec<String> {
    rule.split_whitespace().map(str::to_owned).collect()
}

/// 限频复核器：把「最近一次 apply 的期望态」与实时 -S 采样绑在一起。
///
/// accept 期遇到 uid 未知的连接时，**不能**永久信任激活时的 applied 快照：
/// 规则可能被手工 -F、被第三方脚本改掉，或本来就没落地。这里按 ≤1 次/秒
/// 重新 sample（每次 -S 都有 1 s 超时），用最新结果更新共享旗标后再决定。
pub struct Revalidator {
    firewall: Arc<Firewall>,
    commands: Arc<dyn CommandRunner>,
    spec: Arc<std::sync::Mutex<Option<RuleSpec>>>,
    applied: Arc<std::sync::atomic::AtomicBool>,
    last: std::sync::Mutex<Option<std::time::Instant>>,
    min_interval: std::time::Duration,
}

impl std::fmt::Debug for Revalidator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Revalidator")
            .field("min_interval", &self.min_interval)
            .finish_non_exhaustive()
    }
}

impl Revalidator {
    /// 构造：共享 Firewall、命令执行器、期望态与旗标。
    pub fn new(
        firewall: Arc<Firewall>,
        commands: Arc<dyn CommandRunner>,
        spec: Arc<std::sync::Mutex<Option<RuleSpec>>>,
        applied: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            firewall,
            commands,
            spec,
            applied,
            last: std::sync::Mutex::new(None),
            min_interval: std::time::Duration::from_secs(1),
        }
    }

    /// 复核一次（≤1/s）。返回最新的 applied。
    pub fn check(&self) -> bool {
        if let Ok(mut guard) = self.last.lock() {
            let now = std::time::Instant::now();
            let fresh = guard
                .map(|last| now.duration_since(last) < self.min_interval)
                .unwrap_or(false);
            if fresh {
                return self.applied.load(std::sync::atomic::Ordering::Relaxed);
            }
            *guard = Some(now);
        }
        let spec = self
            .spec
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(spec) = spec else {
            // 还没成功 apply 过：没有权威期望态可比，保守返回当前旗标。
            return self.applied.load(std::sync::atomic::Ordering::Relaxed);
        };
        let state = self.firewall.sample(&*self.commands, &spec);
        self.applied
            .store(state.applied, std::sync::atomic::Ordering::SeqCst);
        if !state.applied {
            tracing::warn!(
                reason = state.last_error.as_ref().map(|failure| failure.reason),
                "relay 防火墙复核失败：uid 未知的连接将按 fail-closed 关闭"
            );
        }
        state.applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::FixtureRunner;

    fn spec() -> RuleSpec {
        RuleSpec {
            deny_ports: vec![17800, 17831],
            accepts: vec![
                AcceptRule {
                    port: 17800,
                    uid: None,
                },
                AcceptRule {
                    port: 17831,
                    uid: Some(10123),
                },
            ],
        }
    }

    #[test]
    fn entry_rules_keep_the_frozen_order() {
        assert_eq!(
            spec().entry_rules(),
            vec![
                "-j DM_EGRESS_RLY_ALLOW".to_owned(),
                "-j DM_EGRESS_RLY_DENY".to_owned(),
                "-j RETURN".to_owned()
            ]
        );
    }

    #[test]
    fn allow_rules_are_root_then_device_then_uid_ascending() {
        let rules = spec().allow_rules();
        assert_eq!(rules.len(), 4, "{rules:?}");
        assert_eq!(
            rules[0],
            "-o lo -d 127.0.0.0/8 -p tcp --dport 17800 -m owner --uid-owner 0 -j ACCEPT"
        );
        assert_eq!(
            rules[1],
            "-o lo -d 127.0.0.0/8 -p tcp --dport 17831 -m owner --uid-owner 0 -j ACCEPT"
        );
        assert_eq!(
            rules[2], "-o lo -d 127.0.0.0/8 -p tcp --dport 17800 -j ACCEPT",
            "设备级端口放行全部 uid"
        );
        assert_eq!(
            rules[3],
            "-o lo -d 127.0.0.0/8 -p tcp --dport 17831 -m owner --uid-owner 10123 -j ACCEPT"
        );
        assert_eq!(
            spec().deny_rules(),
            vec![
                "-p tcp --dport 17800 -j REJECT --reject-with icmp-port-unreachable".to_owned(),
                "-p tcp --dport 17831 -j REJECT --reject-with icmp-port-unreachable".to_owned()
            ]
        );
    }

    #[test]
    fn chain_parsing_strips_chain_prefix_and_normalizes() {
        let output = "-N DM_EGRESS_RLY_ALLOW
-A DM_EGRESS_RLY_ALLOW -o lo -d 127.0.0.0/8 -p tcp --dport 17800 -j ACCEPT
-A OTHER_CHAIN -j DROP
";
        assert_eq!(
            parse_chain(output, ALLOW_CHAIN),
            vec!["-o lo -d 127.0.0.0/8 -p tcp --dport 17800 -j ACCEPT".to_owned()]
        );
        assert!(parse_chain(output, DENY_CHAIN).is_empty());
    }

    /// 极简假 iptables：只实现 relay 用到的 -N/-F/-X/-S/-C/-A/-I/-D，用于纯逻辑单测。
    #[derive(Debug, Default)]
    struct FakeIptables {
        chains: std::sync::Mutex<std::collections::BTreeMap<String, Vec<String>>>,
    }

    impl FakeIptables {
        fn new() -> Self {
            Self::default()
        }

        fn rules(&self, chain: &str) -> Vec<String> {
            self.chains
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(chain)
                .cloned()
                .unwrap_or_default()
        }

        fn flush(&self, chain: &str) {
            self.chains
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(chain.to_owned(), Vec::new());
        }
    }

    impl CommandRunner for FakeIptables {
        fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
            assert_eq!(program, PROGRAM);
            let mut chains = self
                .chains
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let rest = if args.len() >= 2 && args[0] == "-w" {
                &args[2..]
            } else {
                args
            };
            let failure = || CommandOutcome::Failed {
                code: Some(1),
                stderr: String::new(),
            };
            let (Some(verb), Some(chain)) = (rest.first().copied(), rest.get(1).copied()) else {
                return failure();
            };
            match verb {
                "-N" => {
                    chains.entry(chain.to_owned()).or_default();
                    CommandOutcome::Ok(String::new())
                }
                "-F" => {
                    chains.insert(chain.to_owned(), Vec::new());
                    CommandOutcome::Ok(String::new())
                }
                "-X" => {
                    chains.remove(chain);
                    CommandOutcome::Ok(String::new())
                }
                "-S" => match chains.get(chain) {
                    Some(rules) => {
                        let mut out = format!(
                            "-N {chain}
"
                        );
                        for rule in rules {
                            out.push_str(&format!(
                                "-A {chain} {rule}
"
                            ));
                        }
                        CommandOutcome::Ok(out)
                    }
                    None => failure(),
                },
                "-C" => {
                    let rule = rest[2..].join(" ");
                    if chains.get(chain).is_some_and(|rules| rules.contains(&rule)) {
                        CommandOutcome::Ok(String::new())
                    } else {
                        failure()
                    }
                }
                "-A" => {
                    chains
                        .entry(chain.to_owned())
                        .or_default()
                        .push(rest[2..].join(" "));
                    CommandOutcome::Ok(String::new())
                }
                "-I" => {
                    let tail = rest[2..].join(" ");
                    let rule = tail.split_once(' ').map(|(_, rule)| rule).unwrap_or(&tail);
                    chains
                        .entry(chain.to_owned())
                        .or_default()
                        .insert(0, rule.to_owned());
                    CommandOutcome::Ok(String::new())
                }
                "-D" => {
                    let rule = rest[2..].join(" ");
                    if let Some(rules) = chains.get_mut(chain) {
                        rules.retain(|existing| *existing != rule);
                    }
                    CommandOutcome::Ok(String::new())
                }
                _ => failure(),
            }
        }
    }

    #[test]
    fn app_port_only_allows_root_and_bound_uid_everyone_else_hits_deny() {
        let app = RuleSpec {
            deny_ports: vec![17831],
            accepts: vec![AcceptRule {
                port: 17831,
                uid: Some(10123),
            }],
        };
        let allow = app.allow_rules();
        assert_eq!(
            allow.len(),
            2,
            "应用级端口只有 root + 绑定 uid 两条 ACCEPT: {allow:?}"
        );
        assert!(allow[0].contains("--uid-owner 0"), "{allow:?}");
        assert!(allow[1].contains("--uid-owner 10123"), "{allow:?}");
        assert!(
            !allow.iter().any(|rule| !rule.contains("--uid-owner")),
            "应用级端口不得出现「放行全部 uid」的规则: {allow:?}"
        );
        // 其它 uid 落不到 ALLOW → 命中 DENY 的 REJECT（且不限定目的地址，防经公网目的地址绕开）
        let deny = app.deny_rules();
        assert_eq!(
            deny,
            vec!["-p tcp --dport 17831 -j REJECT --reject-with icmp-port-unreachable".to_owned()]
        );
        assert!(
            !deny[0].contains("-o lo"),
            "DENY 必须覆盖非回环目的地址: {deny:?}"
        );
    }

    /// 模拟真机 -S 的渲染（-d 在前、隐式 -m tcp / -m owner）。
    #[derive(Debug)]
    struct DeviceS {
        allow: String,
        deny: String,
        entry: String,
    }

    impl CommandRunner for DeviceS {
        fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
            assert_eq!(program, PROGRAM);
            if args.contains(&"-S") {
                let chain = args[args.len() - 1];
                let body = match chain {
                    ALLOW_CHAIN => &self.allow,
                    DENY_CHAIN => &self.deny,
                    ENTRY_CHAIN => &self.entry,
                    _ => {
                        return CommandOutcome::Failed {
                            code: Some(1),
                            stderr: String::new(),
                        };
                    }
                };
                return CommandOutcome::Ok(format!(
                    "-N {chain}
{body}"
                ));
            }
            CommandOutcome::Ok(String::new())
        }
    }

    fn device_s(allow: &str) -> DeviceS {
        DeviceS {
            allow: allow.to_owned(),
            deny: "-A DM_EGRESS_RLY_DENY -p tcp -m tcp --dport 17835 -j REJECT --reject-with icmp-port-unreachable
".to_owned(),
            entry: concat!(
                "-A DM_EGRESS_RLY -j DM_EGRESS_RLY_ALLOW
",
                "-A DM_EGRESS_RLY -j DM_EGRESS_RLY_DENY
",
                "-A DM_EGRESS_RLY -j RETURN
",
            )
            .to_owned(),
        }
    }

    fn app_spec() -> RuleSpec {
        RuleSpec {
            deny_ports: vec![17835],
            accepts: vec![
                AcceptRule {
                    port: 17835,
                    uid: Some(0),
                },
                AcceptRule {
                    port: 17835,
                    uid: Some(10325),
                },
            ],
        }
    }

    #[test]
    fn device_dash_s_is_semantically_equal_to_expected() {
        // 真机形态：-d 在 -o 之前，且 --dport/--uid-owner 前有隐式模块标记
        let commands = device_s(concat!(
            "-A DM_EGRESS_RLY_ALLOW -d 127.0.0.0/8 -o lo -p tcp -m owner --uid-owner 0 -m tcp --dport 17835 -j ACCEPT
",
            "-A DM_EGRESS_RLY_ALLOW -d 127.0.0.0/8 -o lo -p tcp -m owner --uid-owner 10325 -m tcp --dport 17835 -j ACCEPT
",
        ));
        let state = Firewall::new().sample(&commands, &app_spec());
        assert!(state.applied, "真机 -S 必须语义等价: {state:?}");
        assert_eq!(state.allow_rules, 2);
        assert_eq!(state.deny_rules, 1);
        assert!(state.last_error.is_none());
    }

    #[test]
    fn semantic_compare_still_catches_a_real_difference() {
        // 少一条 root 规则：语义比较必须仍判不一致
        let commands = device_s(
            "-A DM_EGRESS_RLY_ALLOW -d 127.0.0.0/8 -o lo -p tcp -m owner --uid-owner 10325 -m tcp --dport 17835 -j ACCEPT
",
        );
        let state = Firewall::new().sample(&commands, &app_spec());
        assert!(!state.applied, "{state:?}");
        assert_eq!(
            state.last_error.as_ref().map(|failure| failure.reason),
            Some("state_mismatch")
        );
    }

    #[test]
    fn revalidator_without_applied_spec_falls_back_to_flag() {
        let commands = Arc::new(FakeIptables::new());
        let applied = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let revalidator = Revalidator::new(
            Arc::new(Firewall::new()),
            commands,
            Arc::new(std::sync::Mutex::new(None)),
            Arc::clone(&applied),
        );
        assert!(
            revalidator.check(),
            "没有 applied_spec 时沿用当前旗标，不能凭空判 false 把连接全关掉"
        );
    }

    #[test]
    fn revalidator_rate_limits_sampling_to_once_per_second() {
        #[derive(Debug)]
        struct Counting {
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl CommandRunner for Counting {
            fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
                assert_eq!(program, PROGRAM);
                if args.contains(&"-S") {
                    self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                CommandOutcome::Failed {
                    code: Some(1),
                    stderr: String::new(),
                }
            }
        }
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let revalidator = Revalidator::new(
            Arc::new(Firewall::new()),
            Arc::new(Counting {
                calls: Arc::clone(&calls),
            }),
            Arc::new(std::sync::Mutex::new(Some(app_spec()))),
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        );
        assert!(!revalidator.check(), "采样失败必须返回 false");
        let first = calls.load(std::sync::atomic::Ordering::SeqCst);
        assert!(first > 0, "第一次必须真的采样");
        assert!(!revalidator.check(), "1s 内第二次沿用上次结果");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            first,
            "限频 ≤1/s：1 秒内的重复调用不得再调 iptables"
        );
    }

    #[test]
    fn root_rule_is_emitted_once_even_when_allowed_uids_contain_zero() {
        // t9 真机 blocker：B1 的 root 规则 + B3 的 allowedUids={0,...} 会重复，
        // 导致 -S 与期望不一致 → applied=false → 应用级端口不监听。
        let app = RuleSpec {
            deny_ports: vec![17835],
            accepts: (0..3)
                .map(|_| AcceptRule {
                    port: 17835,
                    uid: Some(0),
                })
                .chain(std::iter::once(AcceptRule {
                    port: 17835,
                    uid: Some(10325),
                }))
                .collect(),
        };
        let allow = app.allow_rules();
        assert_eq!(allow.len(), 2, "root + 绑定 uid 各一条: {allow:?}");
        assert_eq!(
            allow
                .iter()
                .filter(|rule| rule.contains("--uid-owner 0 -j"))
                .count(),
            1,
            "root 规则只能出现一次: {allow:?}"
        );
        // 同一份期望重复 apply 也保持稳定
        let commands = FakeIptables::new();
        let firewall = Firewall::new();
        let first = firewall.apply(&commands, &app);
        assert!(first.applied, "{first:?}");
        assert_eq!(commands.rules(ALLOW_CHAIN).len(), 2);
    }

    #[test]
    fn device_port_allows_every_uid_but_only_towards_loopback() {
        let device = RuleSpec {
            deny_ports: vec![17800],
            accepts: vec![AcceptRule {
                port: 17800,
                uid: None,
            }],
        };
        let allow = device.allow_rules();
        assert_eq!(allow.len(), 2, "{allow:?}");
        for rule in &allow {
            assert!(
                rule.contains("-o lo -d 127.0.0.0/8"),
                "设备级规则只对 lo 生效: {rule}"
            );
        }
        // 第二条是「放行全部 uid」：没有 --uid-owner
        assert!(!allow[1].contains("--uid-owner"), "{allow:?}");
        assert_eq!(
            device.deny_rules(),
            vec!["-p tcp --dport 17800 -j REJECT --reject-with icmp-port-unreachable".to_owned()]
        );
    }

    #[test]
    fn released_ports_are_pruned_from_deny_without_flushing_it() {
        let commands = FakeIptables::new();
        let firewall = Firewall::new();
        let full = spec();
        assert!(firewall.apply(&commands, &full).applied);
        assert!(
            commands
                .rules(DENY_CHAIN)
                .iter()
                .any(|rule| rule.contains("--dport 17831")),
            "初始 DENY 应包含两个端口"
        );

        // 17831 被释放：新期望只剩 17800
        let reduced = RuleSpec {
            deny_ports: vec![17800],
            accepts: vec![AcceptRule {
                port: 17800,
                uid: None,
            }],
        };
        let state = firewall.apply(&commands, &reduced);
        assert!(state.applied, "{state:?}");
        let deny = commands.rules(DENY_CHAIN);
        assert!(
            deny.iter().all(|rule| !rule.contains("--dport 17831")),
            "已释放端口必须按 spec 从 DENY 删掉（不 -F）: {deny:?}"
        );
        assert!(
            deny.iter().any(|rule| rule.contains("--dport 17800")),
            "{deny:?}"
        );
        let allow = commands.rules(ALLOW_CHAIN);
        assert!(
            allow.iter().all(|rule| !rule.contains("--dport 17831")),
            "ALLOW 全量重建后不应再有已释放端口: {allow:?}"
        );
    }

    #[test]
    fn apply_is_idempotent_and_clear_removes_the_three_chains() {
        let commands = FakeIptables::new();
        let firewall = Firewall::new();
        let spec = spec();
        let first = firewall.apply(&commands, &spec);
        assert!(first.applied, "{first:?}");
        let allow_after_first = commands.rules(ALLOW_CHAIN);
        let deny_after_first = commands.rules(DENY_CHAIN);
        let entry_after_first = commands.rules(ENTRY_CHAIN);

        // 同一份期望再 apply：内容逐条相同（幂等），不产生重复规则
        let second = firewall.apply(&commands, &spec);
        assert!(second.applied, "{second:?}");
        assert_eq!(commands.rules(ALLOW_CHAIN), allow_after_first);
        assert_eq!(commands.rules(DENY_CHAIN), deny_after_first);
        assert_eq!(commands.rules(ENTRY_CHAIN), entry_after_first);
        assert_eq!(second.allow_rules, first.allow_rules);
        assert_eq!(second.deny_rules, first.deny_rules);

        // 采样与期望一致 → applied=true；手工 -F 后立刻掉成 state_mismatch（无缓存）
        assert!(firewall.sample(&commands, &spec).applied);
        commands.flush(ALLOW_CHAIN);
        let mismatch = firewall.sample(&commands, &spec);
        assert!(!mismatch.applied);
        assert_eq!(
            mismatch.last_error.as_ref().map(|failure| failure.reason),
            Some("state_mismatch")
        );

        // 退出/卸载清理：三链被删，-S 再也读不到
        firewall.clear(&commands);
        assert!(commands.rules(ALLOW_CHAIN).is_empty());
        assert!(commands.rules(DENY_CHAIN).is_empty());
        assert!(commands.rules(ENTRY_CHAIN).is_empty());
        let after_clear = firewall.sample(&commands, &spec);
        assert!(!after_clear.applied);
        assert_eq!(
            after_clear
                .last_error
                .as_ref()
                .map(|failure| failure.reason),
            Some("command_failed"),
            "链不存在时 -S 非零退出 → command_failed"
        );
    }

    #[test]
    fn sampling_without_iptables_reports_missing_and_never_panics() {
        let commands = FixtureRunner::new(); // iptables 不存在
        let firewall = Firewall::new();
        let state = firewall.sample(&commands, &spec());
        assert!(!state.applied);
        assert_eq!(state.rules, 0);
        assert_eq!(
            state.last_error.as_ref().map(|failure| failure.reason),
            Some("iptables_missing")
        );
        let state = firewall.apply(&commands, &spec());
        assert!(!state.applied, "装不上规则时绝不能报 applied");
        assert_eq!(
            state.last_error.as_ref().map(|failure| failure.code),
            Some("egress_relay_firewall_failed")
        );
    }
}
