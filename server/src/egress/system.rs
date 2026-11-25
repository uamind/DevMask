// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! Android 系统代理（方案 A）的应用、读回校验与**唯一一份**清直连实现。
//!
//! 契约：docs/plans/egress-contract.md §3.1–§3.5。
//!
//! 两条硬规则（真机实测得出的，不要「优化」）：
//!
//! 1. 设置：把 global http_proxy 写成 host:port（只写 host:port，不做 DNS 解析）；
//! 2. 清除：**只能**写 global http_proxy :0。删除该键不会让代理失效，
//!    因此代码里不允许出现删除写法；「:0」与「null」在判定上等价。
//!
//! 所有写 settings 的地方都经过 [SystemProxy]：DELETE /egress、fail-open 切直连、
//! 启动自愈都调用同一个 [SystemProxy::clear]；module/uninstall.sh 是同一语义的
//! 离线副本（脚本不能调用进程内函数），两处字面量保持一致。

use crate::commands::{self, CommandOutcome, CommandRunner};

/// settings 命名空间。
pub const GLOBAL_NAMESPACE: &str = "global";
/// 系统代理键（契约 §3.2 的相关键之一）。
pub const PROXY_KEY: &str = "http_proxy";
/// 排除列表键（DELETE /egress 与 fail-open 都会写空串）。
pub const EXCLUSION_KEY: &str = "global_http_proxy_exclusion_list";
/// 脏状态诊断键：http_proxy 为 null 但它有值即为脏状态（契约 §3.3）。
pub const PROXY_HOST_KEY: &str = "global_http_proxy_host";
/// **清除系统代理的唯一表达**：settings put global http_proxy :0。
pub const DIRECT_VALUE: &str = ":0";
/// 读回交叉验证用的 dumpsys 服务。
pub const DUMPSYS_COMPONENT: &str = "connectivity";

/// 期望的设备级链路。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    /// 直连（清除系统代理）。
    Direct,
    /// 经代理。
    Proxy {
        /// 主机名或 IP 字面量。
        host: String,
        /// 端口。
        port: u16,
    },
}

impl Expected {
    /// 写入 http_proxy 的字面量：直连是 :0，代理是 host:port。
    pub fn value(&self) -> String {
        match self {
            Expected::Direct => DIRECT_VALUE.to_owned(),
            Expected::Proxy { host, port } => format!("{host}:{port}"),
        }
    }

    /// 只读展示用的简短描述。
    pub fn describe(&self) -> String {
        match self {
            Expected::Direct => "direct".to_owned(),
            Expected::Proxy { .. } => self.value(),
        }
    }
}

/// http_proxy 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyState {
    /// 未设置（null / 空串 / :0）。
    Unset,
    /// 指向某个代理。
    Proxy {
        /// 主机。
        host: String,
        /// 端口。
        port: u16,
    },
    /// 无法解析（判定为不一致，不猜测）。
    Unknown,
}

/// 一次读回的结果（契约 §3.3）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Readback {
    /// settings get global http_proxy 的原文（含 null 字符串）。
    pub settings_value: Option<String>,
    /// dumpsys connectivity 里 Global proxy: 行的原文。
    pub dumpsys_global_proxy: Option<String>,
    /// 脏状态：http_proxy 为未设置，但 global_http_proxy_host 仍有值。
    pub dirty: bool,
    /// 本次读回时间（UTC RFC 3339）。
    pub at: String,
    /// settings 命令是否可用（false 表示读回不可信，applied 一定为 false）。
    pub available: bool,
}

/// settings 命令失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    /// 二进制缺失 / 无法执行。
    Unavailable,
    /// 非零退出（SELinux 拒绝等）。
    Failed {
        /// 退出码。
        code: Option<i32>,
        /// 截断后的标准错误（≤[MAX_STDERR] 字符；settings 命令不含凭据，可安全落日志）。
        stderr: String,
    },
}

/// stderr 保留上限：足够定位 SELinux/参数问题，又不会把长输出灌进日志。
pub const MAX_STDERR: usize = 256;

impl SettingsError {
    /// 构造带截断 stderr 的失败。
    pub fn failed(code: Option<i32>, stderr: &str) -> Self {
        Self::Failed {
            code,
            stderr: stderr.chars().take(MAX_STDERR).collect(),
        }
    }

    /// 错误 details 里的组件名。
    pub fn component(&self) -> &'static str {
        "settings"
    }

    /// 退出码（Unavailable 时为 None）。
    pub fn code(&self) -> Option<i32> {
        match self {
            SettingsError::Failed { code, .. } => *code,
            SettingsError::Unavailable => None,
        }
    }

    /// 截断后的 stderr（Unavailable 时为空）。
    pub fn stderr(&self) -> &str {
        match self {
            SettingsError::Failed { stderr, .. } => stderr,
            SettingsError::Unavailable => "",
        }
    }
}

/// 一次应用的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// 应用后的读回。
    pub readback: Readback,
    /// 实际执行的 settings put 次数（幂等断言用）。
    pub write_count: usize,
    /// 应用后是否与期望一致（applied 的唯一来源）。
    pub matches: bool,
}

impl ApplyOutcome {
    /// 是否发生过写入。
    pub fn wrote(&self) -> bool {
        self.write_count > 0
    }
}

/// 系统代理的应用入口：所有 settings 读写都走这里。
#[derive(Debug, Clone, Copy)]
pub struct SystemProxy<'a> {
    commands: &'a dyn CommandRunner,
}

impl<'a> SystemProxy<'a> {
    /// 绑定命令执行器。
    pub fn new(commands: &'a dyn CommandRunner) -> Self {
        Self { commands }
    }

    fn put(&self, key: &str, value: &str) -> Result<(), SettingsError> {
        match commands::settings_put(self.commands, GLOBAL_NAMESPACE, key, value) {
            CommandOutcome::Ok(_) => Ok(()),
            CommandOutcome::Failed { code, stderr } => Err(SettingsError::failed(code, &stderr)),
            CommandOutcome::Unavailable { .. } => Err(SettingsError::Unavailable),
        }
    }

    fn get(&self, key: &str) -> Result<Option<String>, SettingsError> {
        match commands::settings_get(self.commands, GLOBAL_NAMESPACE, key) {
            CommandOutcome::Ok(stdout) => {
                let trimmed = stdout.trim();
                Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
            }
            CommandOutcome::Failed { code, stderr } => Err(SettingsError::failed(code, &stderr)),
            CommandOutcome::Unavailable { .. } => Err(SettingsError::Unavailable),
        }
    }

    /// 读回：主值 + 脏状态诊断 + dumpsys 交叉验证（契约 §3.3）。
    pub fn readback(&self) -> Result<Readback, SettingsError> {
        let settings_value = self.get(PROXY_KEY)?;
        let host_value = self.get(PROXY_HOST_KEY)?;
        let dumpsys = match commands::dumpsys(self.commands, DUMPSYS_COMPONENT) {
            CommandOutcome::Ok(stdout) => find_global_proxy_line(&stdout),
            _ => None,
        };
        let dirty = matches!(
            parse_settings_state(settings_value.as_deref()),
            ProxyState::Unset
        ) && host_value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty() && value.trim() != "null");
        Ok(Readback {
            settings_value,
            dumpsys_global_proxy: dumpsys,
            dirty,
            at: crate::util::now_rfc3339(),
            available: true,
        })
    }

    /// 读回排除列表（逗号连接），用于 GET /egress 的 exclusionList。
    pub fn exclusions(&self) -> Result<Vec<String>, SettingsError> {
        let raw = self.get(EXCLUSION_KEY)?;
        Ok(match raw.as_deref() {
            // settings 未设置时输出字符串 "null"，不能当成一个叫 null 的排除项
            None => Vec::new(),
            Some(value) if value.trim() == "null" => Vec::new(),
            Some(value) => split_exclusions(value),
        })
    }

    /// 应用期望值：幂等（已一致只刷新读回、不重复写）。
    ///
    /// - 脏状态先写一次 :0（契约 §3.3 第四行）；
    /// - exclusions 为 Some 时同步写排除列表（Some(&[]) 写空串）。
    pub fn apply(
        &self,
        expected: &Expected,
        exclusions: Option<&[String]>,
    ) -> Result<ApplyOutcome, SettingsError> {
        let mut state = self.readback()?;
        let mut write_count = 0usize;

        if state.dirty {
            // 脏状态：先清成直连，再按期望写（避免残留的 host/port 让设置「看似生效」）
            self.put(PROXY_KEY, DIRECT_VALUE)?;
            write_count += 1;
            state = self.readback()?;
        }
        if !matches(expected, &state) {
            self.put(PROXY_KEY, &expected.value())?;
            write_count += 1;
        }
        if let Some(list) = exclusions {
            let wanted = list.join(",");
            let current = self.get(EXCLUSION_KEY)?.unwrap_or_default();
            let current = if current.trim() == "null" {
                String::new()
            } else {
                current.trim().to_owned()
            };
            if current != wanted {
                self.put(EXCLUSION_KEY, &wanted)?;
                write_count += 1;
            }
        }

        let readback = self.readback()?;
        Ok(ApplyOutcome {
            matches: matches(expected, &readback),
            readback,
            write_count,
        })
    }

    /// **清除系统代理的唯一实现**（DELETE /egress、fail-open、启动自愈共用）。
    ///
    /// 语义固定为：写 :0 + 排除列表写空串。
    pub fn clear(&self) -> Result<ApplyOutcome, SettingsError> {
        self.apply(&Expected::Direct, Some(&[]))
    }
}

/// 判定真值表（契约 §3.3，冻结）：
///
/// | settings 主值 | dumpsys 行 | 期望=直连 | 期望=h:p |
/// | --- | --- | --- | --- |
/// | null / 空 / :0 | Global proxy: none | true | false |
/// | h:p | Global proxy: [h] port: p | false | true |
/// | null（host 仍有值，脏状态） | Global proxy: none | false（先写 :0） | false（写期望） |
///
/// dumpsys 不可用时只按主值判定（少一条交叉验证，不影响真值表前三行）。
pub fn matches(expected: &Expected, readback: &Readback) -> bool {
    if !readback.available || readback.dirty {
        return false;
    }
    let settings_state = parse_settings_state(readback.settings_value.as_deref());
    let dumpsys_state = parse_dumpsys_state(readback.dumpsys_global_proxy.as_deref());
    match expected {
        Expected::Direct => {
            // dumpsys 说「有代理」时即使主值是 null 也算不一致（脏状态的一种）
            settings_state == ProxyState::Unset
                && !matches!(dumpsys_state, ProxyState::Proxy { .. })
        }
        Expected::Proxy { host, port } => {
            settings_state
                == (ProxyState::Proxy {
                    host: host.clone(),
                    port: *port,
                })
                && dumpsys_state != ProxyState::Unset
        }
    }
}

/// 解析 settings get global http_proxy 的原文。
pub fn parse_settings_state(raw: Option<&str>) -> ProxyState {
    let Some(raw) = raw else {
        return ProxyState::Unset;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" || trimmed == DIRECT_VALUE {
        return ProxyState::Unset;
    }
    match split_host_port(trimmed) {
        Some((host, port)) => ProxyState::Proxy { host, port },
        None => ProxyState::Unknown,
    }
}

/// 解析 dumpsys 的 Global proxy: 行。
///
/// 真机（Pixel 9 / Android 17）与不同 ROM 的写法不一致，因此同时接受：
///
/// - `Global proxy: none`
/// - `Global proxy: [203.0.113.10] port: 1080`
/// - `Global proxy: [203.0.113.10] 1080`（无 `port:`）
/// - 上述任一形态 + 尾部属性（例如 `… 1080 xl=1`，只取第一个数字端口）
pub fn parse_dumpsys_state(line: Option<&str>) -> ProxyState {
    let Some(line) = line else {
        return ProxyState::Unknown;
    };
    let trimmed = line.trim();
    let Some(rest) = strip_prefix_ignore_case(trimmed, DUMPSYS_MARKER_WITH_COLON) else {
        return ProxyState::Unknown;
    };
    let rest = rest.trim();
    if rest.eq_ignore_ascii_case("none") {
        return ProxyState::Unset;
    }
    let Some(rest) = rest.strip_prefix('[') else {
        return ProxyState::Unknown;
    };
    let Some((host, tail)) = rest.split_once(']') else {
        return ProxyState::Unknown;
    };
    let tail = tail.trim();
    let port_text = tail.strip_prefix("port:").map(str::trim).unwrap_or(tail);
    let Some(port) = port_text.split_whitespace().next() else {
        return ProxyState::Unknown;
    };
    match port.parse::<u16>() {
        Ok(port) if !host.is_empty() => ProxyState::Proxy {
            host: host.to_owned(),
            port,
        },
        _ => ProxyState::Unknown,
    }
}

/// `Global proxy:` 前缀（大小写不敏感比较用）。
const DUMPSYS_MARKER_WITH_COLON: &str = "Global proxy:";

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// 在 dumpsys 输出里找全局代理行（原文，已 trim）。
///
/// **只认行首的 `Global proxy:`**：dumpsys 输出里还有 `Proxy:`、`global proxy` 之类的
/// 其它行，宽松的子串匹配会把它们当成交叉验证依据（误判）。
pub fn find_global_proxy_line(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| strip_prefix_ignore_case(line, DUMPSYS_MARKER_WITH_COLON).is_some())
        .map(str::to_owned)
}

fn split_host_port(text: &str) -> Option<(String, u16)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':')?.parse::<u16>().ok()?;
        return Some((host.to_owned(), port));
    }
    let (host, port) = text.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_owned(), port.parse::<u16>().ok()?))
}

fn split_exclusions(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::FixtureRunner;

    fn readback(settings: Option<&str>, dumpsys: Option<&str>, dirty: bool) -> Readback {
        Readback {
            settings_value: settings.map(str::to_owned),
            dumpsys_global_proxy: dumpsys.map(str::to_owned),
            dirty,
            at: "2026-11-24T12:00:00Z".to_owned(),
            available: true,
        }
    }

    #[test]
    fn truth_table_rows_match_contract() {
        let proxy = Expected::Proxy {
            host: "203.0.113.10".to_owned(),
            port: 1080,
        };
        let direct = Expected::Direct;

        // 第 1 行：null + Global proxy: none
        let row = readback(Some("null"), Some("Global proxy: none"), false);
        assert!(matches(&direct, &row));
        assert!(!matches(&proxy, &row));

        // 第 2 行：:0 与 null 等价
        let row = readback(Some(":0"), Some("Global proxy: none"), false);
        assert!(matches(&direct, &row));
        assert!(!matches(&proxy, &row));

        // 第 3 行：h:p + Global proxy: [h] port: p
        let row = readback(
            Some("203.0.113.10:1080"),
            Some("Global proxy: [203.0.113.10] port: 1080"),
            false,
        );
        assert!(matches(&proxy, &row));
        assert!(!matches(&direct, &row));

        // 第 4 行：null 但 host 有值 → 脏状态，两边都是 false（需要写 :0）
        let row = readback(Some("null"), Some("Global proxy: none"), true);
        assert!(!matches(&direct, &row));
        assert!(!matches(&proxy, &row));

        // 主值一致但 dumpsys 说 none → 不一致
        let row = readback(Some("203.0.113.10:1080"), Some("Global proxy: none"), false);
        assert!(!matches(&proxy, &row));

        // settings 已是 :0，但 dumpsys 仍显示代理 → 期望直连时必须判不一致（t11 追加）
        let row = readback(
            Some(":0"),
            Some("Global proxy: [203.0.113.10] port: 1080"),
            false,
        );
        assert!(!matches(&direct, &row), "dumpsys 仍有代理时不能算直连");

        // dumpsys 缺失时只按主值判定
        let row = readback(Some("203.0.113.10:1080"), None, false);
        assert!(matches(&proxy, &row));
        let row = readback(None, None, false);
        assert!(matches(&direct, &row));
    }

    #[test]
    fn parse_helpers_handle_edge_cases() {
        assert_eq!(parse_settings_state(None), ProxyState::Unset);
        assert_eq!(parse_settings_state(Some("  ")), ProxyState::Unset);
        assert_eq!(parse_settings_state(Some("null")), ProxyState::Unset);
        assert_eq!(parse_settings_state(Some(":0")), ProxyState::Unset);
        assert_eq!(
            parse_settings_state(Some("203.0.113.10:1080")),
            ProxyState::Proxy {
                host: "203.0.113.10".to_owned(),
                port: 1080
            }
        );
        assert_eq!(
            parse_settings_state(Some("[::1]:8080")),
            ProxyState::Proxy {
                host: "::1".to_owned(),
                port: 8080
            }
        );
        assert_eq!(parse_settings_state(Some("garbage")), ProxyState::Unknown);

        assert_eq!(parse_dumpsys_state(None), ProxyState::Unknown);
        assert_eq!(
            parse_dumpsys_state(Some("Global proxy: none")),
            ProxyState::Unset
        );
        assert_eq!(
            parse_dumpsys_state(Some("Global proxy: [198.51.100.7] port: 1080")),
            ProxyState::Proxy {
                host: "198.51.100.7".to_owned(),
                port: 1080
            }
        );
        // 真机/ROM 变体：无 port: 前缀、带尾部属性、IPv6 字面量
        assert_eq!(
            parse_dumpsys_state(Some("Global proxy: [203.0.113.10] 1080")),
            ProxyState::Proxy {
                host: "203.0.113.10".to_owned(),
                port: 1080
            }
        );
        assert_eq!(
            parse_dumpsys_state(Some("Global proxy: [203.0.113.10] 1080 xl=1")),
            ProxyState::Proxy {
                host: "203.0.113.10".to_owned(),
                port: 1080
            }
        );
        assert_eq!(
            parse_dumpsys_state(Some("Global proxy: [2001:db8::1] port: 8080 extra=abc")),
            ProxyState::Proxy {
                host: "2001:db8::1".to_owned(),
                port: 8080
            }
        );
        // 只认 Global proxy: 前缀
        assert_eq!(
            parse_dumpsys_state(Some("Global proxy type: HTTP")),
            ProxyState::Unknown
        );
        assert_eq!(
            parse_dumpsys_state(Some("Proxy: Global proxy: [1.2.3.4] port: 1")),
            ProxyState::Unknown
        );
    }

    #[test]
    fn global_proxy_line_only_matches_prefixed_line() {
        let real =
            "ConnectivityService:\n  Global proxy: [203.0.113.10] port: 1080 xl=1\n  MTU: 1500\n";
        assert_eq!(
            find_global_proxy_line(real).as_deref(),
            Some("Global proxy: [203.0.113.10] port: 1080 xl=1")
        );
        assert_eq!(
            find_global_proxy_line("  Global proxy: none\n"),
            Some("Global proxy: none".to_owned())
        );
        // 干扰行不能被当成全局代理行
        assert_eq!(
            find_global_proxy_line("  Proxy: Global proxy: [1.2.3.4] port: 1\n"),
            None
        );
        assert_eq!(find_global_proxy_line("  Global Proxy Info: x\n"), None);
    }

    #[test]
    fn settings_failure_keeps_truncated_stderr() {
        let long = "avc: denied { write } for pid=1 comm=\"settings\" ".repeat(40);
        let error = SettingsError::failed(Some(1), &long);
        assert_eq!(error.code(), Some(1));
        assert_eq!(error.stderr().chars().count(), MAX_STDERR);
        assert_eq!(error.component(), "settings");
        assert!(!error.stderr().contains("password"));
    }

    #[test]
    fn apply_writes_once_when_value_differs() {
        let runner = FixtureRunner::new()
            .with_output("settings get global http_proxy", "null")
            .with_output("settings get global global_http_proxy_host", "null")
            .with_output("dumpsys connectivity", "  Global proxy: none\n")
            .with_output("settings put global http_proxy 203.0.113.10:1080", "");
        let system = SystemProxy::new(&runner);
        let expected = Expected::Proxy {
            host: "203.0.113.10".to_owned(),
            port: 1080,
        };
        // fixture 的 get 永远返回 null（不模拟写入），因此 matches 必须如实为 false。
        let outcome = system.apply(&expected, None).unwrap();
        assert_eq!(outcome.write_count, 1);
        assert!(
            !outcome.matches,
            "fixture 不模拟读回，matches 必须如实为 false"
        );
        assert_eq!(outcome.readback.settings_value.as_deref(), Some("null"));
    }

    #[test]
    fn apply_skips_write_when_already_matching() {
        let runner = FixtureRunner::new()
            .with_output("settings get global http_proxy", "203.0.113.10:1080")
            .with_output("settings get global global_http_proxy_host", "203.0.113.10")
            .with_output(
                "dumpsys connectivity",
                "Global proxy: [203.0.113.10] port: 1080\n",
            );
        let system = SystemProxy::new(&runner);
        let outcome = system
            .apply(
                &Expected::Proxy {
                    host: "203.0.113.10".to_owned(),
                    port: 1080,
                },
                None,
            )
            .unwrap();
        assert_eq!(outcome.write_count, 0, "值相同不重复写");
        assert!(outcome.matches);
    }

    #[test]
    fn clear_uses_colon_zero_and_empties_exclusions() {
        let runner = FixtureRunner::new()
            .with_output("settings get global http_proxy", "203.0.113.10:1080")
            .with_output("settings get global global_http_proxy_host", "203.0.113.10")
            .with_output(
                "dumpsys connectivity",
                "Global proxy: [203.0.113.10] port: 1080\n",
            )
            .with_output("settings put global http_proxy :0", "")
            .with_output(
                "settings get global global_http_proxy_exclusion_list",
                "10.0.0.0/8",
            )
            .with_output("settings put global global_http_proxy_exclusion_list ", "");
        let system = SystemProxy::new(&runner);
        let outcome = system.clear().unwrap();
        // 一次 http_proxy :0 + 一次排除列表清空
        assert_eq!(outcome.write_count, 2);
        assert!(!outcome.matches, "fixture 不模拟读回");
    }

    #[test]
    fn unavailable_settings_is_reported() {
        let runner = FixtureRunner::new();
        let system = SystemProxy::new(&runner);
        assert_eq!(system.readback().unwrap_err(), SettingsError::Unavailable);
        assert_eq!(
            system.clear().unwrap_err(),
            SettingsError::Unavailable,
            "settings 不可用时不得假装成功"
        );
    }
}
