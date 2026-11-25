// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! uid 归属、`pkg@uid` 解析与并发限流（契约 §4.6 / §5.3）。
//!
//! - **归属**：TCP 回环连接没有 `SO_PEERCRED`，只能读内核视图 `/proc/net/tcp`
//!   （IPv4）与 `/proc/net/tcp6`（IPv6 回环）的 `uid` 列；
//! - **解析**：`pm list packages -U --user <id> <pkg>`（后备 `cmd package`），
//!   失败按 fail-closed 处理（restricted 端口不监听 / 不符就 close）；
//! - **限流**：全局 `max_connections` + 单 uid `max_connections_per_uid`（`0` = 不限）。
//!
//! 非 Linux（host 测试）上 `/proc/net/tcp` 不存在 → 归属返回 `None`，按「未知 uid」处理：
//! restricted 端口拒绝、设备级端口只跳过 per-uid 计数（契约 §4.6 第 4 条）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::commands::{CommandOutcome, CommandRunner};

/// 归属重试次数（契约 §4.6：3 次，每次 50 ms）。
const UID_RETRIES: usize = 3;
/// 归属重试间隔。
const UID_RETRY_DELAY: Duration = Duration::from_millis(50);

/// `pkg@uid` 解析失败（契约 §5.3 / §6.4 的 `egress_relay_uid_unknown`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UidError {
    /// 包未安装（`pm` 输出为空）。
    PackageNotInstalled,
    /// `userId` 不存在 / 用户枚举为空。
    UserNotFound,
    /// `pm` / `cmd package` 不可用（`Unavailable`、非零退出、超时）。
    PmUnavailable,
    /// 输出无法解析出 `uid:`。
    ParseFailed,
}

impl UidError {
    /// `details.reason` 的枚举值。
    pub fn reason(self) -> &'static str {
        match self {
            UidError::PackageNotInstalled => "package_not_installed",
            UidError::UserNotFound => "user_not_found",
            UidError::PmUnavailable => "pm_unavailable",
            UidError::ParseFailed => "parse_failed",
        }
    }
}

/// 解析 `pm list packages -U` 输出里的 uid（`package:com.example.a uid:10123`）。
pub fn parse_package_uids(output: &str) -> Vec<u32> {
    let mut uids = Vec::new();
    for line in output.lines() {
        let Some(index) = line.find("uid:") else {
            continue;
        };
        let rest = &line[index + 4..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(uid) = digits.parse::<u32>()
            && !uids.contains(&uid)
        {
            uids.push(uid);
        }
    }
    uids.sort_unstable();
    uids
}

/// 解析 `cmd user list` / `pm list users` 输出的 userId（`UserInfo{0:Owner:13}`）。
pub fn parse_user_ids(output: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    for line in output.lines() {
        let Some(start) = line.find("UserInfo{") else {
            continue;
        };
        let rest = &line[start + "UserInfo{".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(id) = digits.parse::<u32>()
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    ids
}

/// 把 `pkg@userId` 拆成 (包名, Some(userId))；不带后缀时为 None。
pub fn split_package(package: &str) -> (&str, Option<u32>) {
    match package.rsplit_once('@') {
        Some((base, suffix)) => match suffix.parse::<u32>() {
            Ok(user) => (base, Some(user)),
            // 非法后缀：按原文当包名（既有包名规则不允许 '@'，这里只在容错时出现）
            Err(_) => (package, None),
        },
        None => (package, None),
    }
}

fn run(commands: &dyn CommandRunner, program: &str, args: &[&str]) -> Result<String, UidError> {
    match commands.run(program, args) {
        CommandOutcome::Ok(stdout) => Ok(stdout),
        CommandOutcome::Failed { .. } | CommandOutcome::Unavailable { .. } => {
            Err(UidError::PmUnavailable)
        }
    }
}

/// 解析某个包（可带 `@userId`）在各用户下的 uid 集合（去重升序）。
///
/// 失败即 fail-closed（调用方让端口不监听），不做任何猜测。
pub fn resolve_package_uids(
    commands: &dyn CommandRunner,
    package: &str,
) -> Result<Vec<u32>, UidError> {
    let (base, explicit_user) = split_package(package);
    if base.is_empty() {
        return Err(UidError::PackageNotInstalled);
    }
    let users: Vec<u32> = match explicit_user {
        Some(user) => vec![user],
        None => {
            // 用户枚举：cmd user list 优先，pm list users 后备。
            let output = run(commands, "cmd", &["user", "list"])
                .or_else(|_| run(commands, "pm", &["list", "users"]))?;
            let ids = parse_user_ids(&output);
            if ids.is_empty() {
                return Err(UidError::UserNotFound);
            }
            ids
        }
    };
    let mut uids: Vec<u32> = Vec::new();
    let mut saw_package = false;
    for user in users {
        let user_text = user.to_string();
        let args = ["list", "packages", "-U", "--user", user_text.as_str(), base];
        let output = match run(commands, "pm", &args) {
            Ok(output) => Ok(output),
            Err(_) => run(
                commands,
                "cmd",
                &[
                    "package",
                    "list",
                    "packages",
                    "-U",
                    "--user",
                    user_text.as_str(),
                    base,
                ],
            ),
        }?;
        let found = parse_package_uids(&output);
        if !found.is_empty() {
            saw_package = true;
        }
        for uid in found {
            if !uids.contains(&uid) {
                uids.push(uid);
            }
        }
    }
    if !saw_package {
        // 显式 userId 时「查不到包」既可能是没装、也可能是该用户没有它：
        // 后者按 user_not_found 更贴近调用方的修复动作（换用户 / 装包）。
        // 查不到包：无论是否显式 userId，都是「这个包在该用户下不存在」。
        return Err(UidError::PackageNotInstalled);
    }
    uids.sort_unstable();
    Ok(uids)
}

/// 读 `/proc/net/tcp` 文本，找「本端 `local_port` ← 对端 `peer_port`」那一行的 uid。
///
/// 字段顺序（内核固定）：`sl local_address rem_address st tx_queue:rx_queue tr:tm->when
/// retrnsmt uid timeout inode`，因此 uid 是第 8 列（0 基下标 7）。十六进制端口。
pub fn parse_proc_net_tcp(text: &str, listen_port: u16, peer_port: u16) -> Option<u32> {
    parse_proc_net_tcp_detailed(text, listen_port, peer_port).map(|(uid, _)| uid)
}

/// 解析 /proc/net/tcp{,6} 的一行：sl local_address rem_address st ... uid ...。
///
/// 关键点（真机踩过）：
///
/// - 端口与地址都是**十六进制**，端口取低 16 位；
/// - st 必须是 01（TCP_ESTABLISHED）——06（TIME_WAIT）之类的残留行不算；
/// - 地址按每 32 位字小端存储：IPv4 8 个 hex 字符、tcp6 32 个 hex 字符；
/// - 只接受**两端都是回环**的行（127.0.0.1 / ::1 / v4-mapped ::ffff:127.0.0.1）。
///
/// 返回 (uid, 脱敏摘要)：摘要只含端口/uid/st，用于失败日志定位。
pub fn parse_proc_net_tcp_detailed(
    text: &str,
    listen_port: u16,
    peer_port: u16,
) -> Option<(u32, String)> {
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }
        let Some((local_addr, local_hex)) = fields[1].rsplit_once(':') else {
            continue;
        };
        let Some((remote_addr, remote_hex)) = fields[2].rsplit_once(':') else {
            continue;
        };
        let (Ok(local_port), Ok(remote_port)) = (
            u16::from_str_radix(local_hex, 16),
            u16::from_str_radix(remote_hex, 16),
        ) else {
            continue;
        };
        // 客户端行：local = 对端(地址, 源端口)，rem = 监听(地址, 监听端口)。
        if local_port != peer_port || remote_port != listen_port {
            continue;
        }
        if fields[3] != "01" {
            continue;
        }
        if !is_loopback_hex(local_addr) || !is_loopback_hex(remote_addr) {
            continue;
        }
        let Ok(uid) = fields[7].parse::<u32>() else {
            continue;
        };
        let summary = format!(
            "local=127.0.0.1:{} rem=127.0.0.1:{} uid={} st={}",
            local_port, remote_port, uid, fields[3]
        );
        return Some((uid, summary));
    }
    None
}

/// 十六进制地址是否回环（IPv4 127.0.0.0/8、IPv6 ::1、v4-mapped ::ffff:127.0.0.0/8）。
fn is_loopback_hex(hex: &str) -> bool {
    match hex.len() {
        8 => match u32::from_str_radix(hex, 16) {
            Ok(word) => word.to_le_bytes()[0] == 127,
            Err(_) => false,
        },
        32 => {
            let mut bytes = [0u8; 16];
            for word in 0..4 {
                let Ok(value) = u32::from_str_radix(&hex[word * 8..word * 8 + 8], 16) else {
                    return false;
                };
                bytes[word * 4..word * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            if bytes == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1] {
                return true;
            }
            bytes[..10].iter().all(|byte| *byte == 0)
                && bytes[10] == 0xFF
                && bytes[11] == 0xFF
                && bytes[12] == 127
        }
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn read_table(path: &str) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(not(target_os = "linux"))]
fn read_table(_path: &str) -> std::io::Result<String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "非 Linux 平台没有 /proc/net/tcp",
    ))
}

/// 读一张表并解析；读失败（SELinux/权限/表不存在）把错误记进日志。
fn probe_table(path: &str, listen_port: u16, peer_port: u16, attempt: usize) -> Option<u32> {
    match read_table(path) {
        Ok(text) => match parse_proc_net_tcp_detailed(&text, listen_port, peer_port) {
            Some((uid, summary)) => {
                tracing::debug!(table = path, attempt, line = %summary, "relay uid 归属命中");
                Some(uid)
            }
            None => None,
        },
        Err(error) => {
            tracing::debug!(
                table = path,
                attempt,
                error = %error,
                "读 /proc 失败（SELinux 拒绝或该表不存在）"
            );
            None
        }
    }
}

/// 失败时打印候选行（脱敏：只保留端口/uid/st）。
fn log_candidates(listen_port: u16, peer_port: u16) {
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = read_table(path) else {
            continue;
        };
        let mut shown = 0usize;
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 8 {
                continue;
            }
            let port_of = |address: &str| {
                address
                    .rsplit_once(':')
                    .and_then(|(_, port)| u16::from_str_radix(port, 16).ok())
            };
            let local = port_of(fields[1]);
            let remote = port_of(fields[2]);
            if local != Some(peer_port) && remote != Some(listen_port) && local != Some(listen_port)
            {
                continue;
            }
            tracing::debug!(
                table = path,
                local_port = local.unwrap_or(0),
                remote_port = remote.unwrap_or(0),
                uid = fields.get(7).unwrap_or(&"?"),
                state = fields[3],
                "relay uid 候选行"
            );
            shown += 1;
            if shown >= 5 {
                break;
            }
        }
    }
}

/// 归属一个已 accept 的连接（3 次重试 × 50 ms）。
///
/// SO_PEERCRED 在 TCP 上不可用（Linux 只对 AF_UNIX 实现，TCP 会返回 EOPNOTSUPP），
/// 所以只走内核视图：查**客户端那一行**（local=对端地址:对端源端口、rem=监听端口、
/// st=01）。反过来查会拿到内核为本进程 accept 出来的 socket（uid 恒 root）。
pub async fn resolve_peer_uid(local_port: u16, peer_port: u16) -> Option<u32> {
    for attempt in 0..UID_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(UID_RETRY_DELAY).await;
        }
        if let Some(uid) = probe_table("/proc/net/tcp", local_port, peer_port, attempt) {
            return Some(uid);
        }
        if let Some(uid) = probe_table("/proc/net/tcp6", local_port, peer_port, attempt) {
            return Some(uid);
        }
    }
    log_candidates(local_port, peer_port);
    tracing::debug!(
        listen_port = local_port,
        peer_port,
        "relay uid 归属失败（没有匹配的 ESTABLISHED 回环客户端行）"
    );
    None
}

/// 连接限流器（全局 + 单 uid）。
#[derive(Debug)]
pub struct Limiter {
    max: AtomicU32,
    per_uid_max: AtomicU32,
    active: AtomicU32,
    per_uid: Mutex<HashMap<u32, u32>>,
}

impl Default for Limiter {
    fn default() -> Self {
        Self {
            max: AtomicU32::new(crate::relay::port::DEFAULT_MAX_CONNECTIONS),
            per_uid_max: AtomicU32::new(crate::relay::port::DEFAULT_MAX_CONNECTIONS_PER_UID),
            active: AtomicU32::new(0),
            per_uid: Mutex::new(HashMap::new()),
        }
    }
}

impl Limiter {
    /// 新建（默认 256 / 64）。
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 应用新的上限（每次 reconcile 刷新）。
    pub fn configure(&self, max: u32, per_uid_max: u32) {
        self.max.store(max.max(1), Ordering::SeqCst);
        self.per_uid_max.store(per_uid_max, Ordering::SeqCst);
    }

    /// 当前全局连接数。
    pub fn active(&self) -> u32 {
        self.active.load(Ordering::SeqCst)
    }

    /// 尝试占用一个连接名额；超限返回 `None`（调用方回 503 并关闭）。
    pub fn try_acquire(self: &Arc<Self>, uid: Option<u32>) -> Option<LimitGuard> {
        let max = self.max.load(Ordering::SeqCst);
        let current = self.active.fetch_add(1, Ordering::SeqCst);
        if current >= max {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        if let Some(uid) = uid {
            let per_uid_max = self.per_uid_max.load(Ordering::SeqCst);
            if per_uid_max > 0 {
                let mut counters = self
                    .per_uid
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let used = counters.entry(uid).or_insert(0);
                if *used >= per_uid_max {
                    drop(counters);
                    self.active.fetch_sub(1, Ordering::SeqCst);
                    return None;
                }
                *used += 1;
            }
        }
        Some(LimitGuard {
            limiter: Arc::clone(self),
            uid,
            counted: uid.is_some() && self.per_uid_max.load(Ordering::SeqCst) > 0,
        })
    }

    /// 读取某 uid 的当前计数（测试用）。
    pub fn uid_active(&self, uid: u32) -> u32 {
        self.per_uid
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&uid)
            .copied()
            .unwrap_or(0)
    }
}

/// 一个连接的名额（`Drop` 归还）。
#[derive(Debug)]
pub struct LimitGuard {
    limiter: Arc<Limiter>,
    uid: Option<u32>,
    counted: bool,
}

impl Drop for LimitGuard {
    fn drop(&mut self) {
        self.limiter.active.fetch_sub(1, Ordering::SeqCst);
        if let (true, Some(uid)) = (self.counted, self.uid) {
            let mut counters = self
                .limiter
                .per_uid
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(used) = counters.get_mut(&uid) {
                *used = used.saturating_sub(1);
                if *used == 0 {
                    counters.remove(&uid);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::FixtureRunner;

    #[test]
    fn proc_net_tcp_parsing_matches_kernel_columns() {
        let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:A2C8 0100007F:8F1A 01 00000000:00000000 00:00000000 00000000  10123        0 1234 1 0000000000000000 20 4 30 10 -1
   1: 0100007F:44A8 0100007F:8F1A 01 00000000:00000000 00:00000000 00000000  2000        0 1235 1 0000000000000000 20 4 30 10 -1
";
        // 新语义：参数是 (listen_port, peer_port)，匹配「local=对端端口, rem=监听端口」的客户端行。
        // 第 0 行：local=A2C8（客户端源端口）、rem=8F1A（监听端口）→ 监听 0x8F1A、对端 0xA2C8
        assert_eq!(parse_proc_net_tcp(text, 0x8F1A, 0xA2C8), Some(10123));
        // 第 1 行：local=44A8、rem=8F1A → 同一监听端口、另一个客户端
        assert_eq!(parse_proc_net_tcp(text, 0x8F1A, 0x44A8), Some(2000));
        assert_eq!(parse_proc_net_tcp(text, 0xA2C8, 0x1111), None);
        assert_eq!(parse_proc_net_tcp("", 1, 2), None);
    }

    #[test]
    fn parses_device_dash_proc_lines_strictly() {
        let table = concat!(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
",
            // 客户端行：local=127.0.0.1:9C40(40000) rem=127.0.0.1:45AB(17835) st=01 uid=10325
            "   0: 0100007F:9C40 0100007F:45AB 01 00000000:00000000 00:00000000 00000000 10325        0 1 1 0 0
",
            // 我们自己 accept 出来的行（local=监听端口）：不能拿来当归属
            "   1: 0100007F:45AB 0100007F:9C40 01 00000000:00000000 00:00000000 00000000     0        0 2 1 0 0
",
        );
        assert_eq!(parse_proc_net_tcp(table, 17835, 40000), Some(10325));
        assert_eq!(
            parse_proc_net_tcp_detailed(table, 17835, 40000).map(|(uid, _)| uid),
            Some(10325)
        );
        // 反过来按「local=监听端口」查不到（那行不是客户端行）
        assert_eq!(parse_proc_net_tcp(table, 40000, 17835), Some(0));

        // st != 01（06 = TIME_WAIT）不算
        let time_wait = concat!(
            "header
",
            "   0: 0100007F:9C40 0100007F:45AB 06 00000000:00000000 00:00000000 00000000 10325        0 1 1 0 0
",
        );
        assert_eq!(parse_proc_net_tcp(time_wait, 17835, 40000), None);

        // 非回环地址不算（10.0.0.5 = 0500000A）
        let non_loopback = concat!(
            "header
",
            "   0: 0500000A:9C40 0100007F:45AB 01 00000000:00000000 00:00000000 00000000 10325        0 1 1 0 0
",
        );
        assert_eq!(parse_proc_net_tcp(non_loopback, 17835, 40000), None);
    }

    #[test]
    fn parses_tcp6_v4_mapped_loopback() {
        // tcp6 行：local 是 v4-mapped 127.0.0.1:9C40，rem 是 v4-mapped 127.0.0.1:45AB
        let table = concat!(
            "header
",
            "   0: 0000000000000000FFFF00000100007F:9C40 0000000000000000FFFF00000100007F:45AB 01 00000000:00000000 00:00000000 00000000 10325        0 1 1 0 0
",
        );
        assert_eq!(parse_proc_net_tcp(table, 17835, 40000), Some(10325));
        // ::1（最后 8 个 hex 是 01000000）
        let table_v6 = concat!(
            "header
",
            "   0: 00000000000000000000000001000000:9C40 00000000000000000000000001000000:45AB 01 00000000:00000000 00:00000000 00000000 10123        0 1 1 0 0
",
        );
        assert_eq!(parse_proc_net_tcp(table_v6, 17835, 40000), Some(10123));
    }

    #[test]
    fn package_uid_output_parsing() {
        let output = "package:com.example.a uid:10123
package:com.example.b uid:10200
";
        assert_eq!(parse_package_uids(output), vec![10123, 10200]);
        assert!(
            parse_package_uids(
                "package:com.example.a
"
            )
            .is_empty()
        );
        assert_eq!(
            parse_user_ids(
                "Users:
	UserInfo{0:Owner:13} running
	UserInfo{999:Work profile:30} running
"
            ),
            vec![0, 999]
        );
    }

    #[test]
    fn package_split_handles_user_suffix() {
        assert_eq!(split_package("com.example.a"), ("com.example.a", None));
        assert_eq!(
            split_package("com.example.a@999"),
            ("com.example.a", Some(999))
        );
    }

    #[test]
    fn resolve_package_reports_contract_reasons() {
        let commands = FixtureRunner::new()
            .with_output(
                "cmd user list",
                "UserInfo{0:Owner:13} running
",
            )
            .with_output(
                "pm list packages -U --user 0 com.example.app",
                "package:com.example.app uid:10123
",
            )
            .with_output("pm list packages -U --user 0 com.example.missing", "");
        assert_eq!(
            resolve_package_uids(&commands, "com.example.app"),
            Ok(vec![10123])
        );
        assert_eq!(
            resolve_package_uids(&commands, "com.example.missing"),
            Err(UidError::PackageNotInstalled)
        );
        let unavailable = FixtureRunner::new();
        assert_eq!(
            resolve_package_uids(&unavailable, "com.example.app"),
            Err(UidError::PmUnavailable)
        );
        let no_users = FixtureRunner::new().with_output("cmd user list", "");
        assert_eq!(
            resolve_package_uids(&no_users, "com.example.app"),
            Err(UidError::UserNotFound)
        );
    }

    #[test]
    fn resolve_package_uids_covers_every_user_and_dedupes() {
        let commands = FixtureRunner::new()
            .with_output(
                "cmd user list",
                "Users:
	UserInfo{0:Owner:13} running
	UserInfo{999:Work profile:30} running
",
            )
            .with_output(
                "pm list packages -U --user 0 com.example.app",
                "package:com.example.app uid:10123
",
            )
            .with_output(
                "pm list packages -U --user 999 com.example.app",
                "package:com.example.app uid:10199
",
            );
        // 包名不带 @userId：对每个已存在用户解析一次，取并集（升序）
        assert_eq!(
            resolve_package_uids(&commands, "com.example.app"),
            Ok(vec![10123, 10199])
        );
        // 带 @999：只解析该用户
        assert_eq!(
            resolve_package_uids(&commands, "com.example.app@999"),
            Ok(vec![10199])
        );
        // 共享 uid（同一应用多用户同 uid）：去重只留一个
        let shared = FixtureRunner::new()
            .with_output(
                "cmd user list",
                "UserInfo{0:Owner:13}
UserInfo{999:Work profile:30}
",
            )
            .with_output(
                "pm list packages -U --user 0 com.example.app",
                "package:com.example.app uid:10123
",
            )
            .with_output(
                "pm list packages -U --user 999 com.example.app",
                "package:com.example.app uid:10123
",
            );
        assert_eq!(
            resolve_package_uids(&shared, "com.example.app"),
            Ok(vec![10123])
        );
    }

    #[test]
    fn limiter_enforces_global_and_per_uid() {
        let limiter = Limiter::new();
        limiter.configure(2, 1);
        let first = limiter.try_acquire(Some(1000));
        assert!(first.is_some());
        assert!(
            limiter.try_acquire(Some(1000)).is_none(),
            "单 uid 上限 1 必须生效"
        );
        let other = limiter.try_acquire(Some(2000));
        assert!(other.is_some());
        assert!(
            limiter.try_acquire(Some(3000)).is_none(),
            "全局上限 2 必须生效"
        );
        drop(first);
        drop(other);
        assert_eq!(limiter.active(), 0);
        assert_eq!(limiter.uid_active(1000), 0);
        assert!(limiter.try_acquire(Some(3000)).is_some());
    }

    #[test]
    fn unlimited_per_uid_when_zero() {
        let limiter = Limiter::new();
        limiter.configure(8, 0);
        let guards: Vec<_> = (0..8).map(|_| limiter.try_acquire(Some(1000))).collect();
        assert!(guards.iter().all(Option::is_some));
        assert!(
            limiter.try_acquire(Some(1000)).is_none(),
            "全局上限仍然生效"
        );
    }
}
