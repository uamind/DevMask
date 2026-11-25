// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 系统命令执行抽象。
//!
//! 只用于契约白名单里的语义化命令（getprop / pm / am / dumpsys / monkey / settings），
//! 不暴露任意 exec 端点（docs/dev/server-api-plan.md 3.2、10.2）。
//! 抽象成 trait 是为了 host 测试能用 fixture 注入 `pm` / `dumpsys` / `settings` 样例输出。
//!
//! `settings` / `dumpsys` 的语义化封装见 [settings_put] / [settings_get] / [dumpsys]：
//! 出口代理（egress）只能通过这些函数读写系统代理，禁止在别处拼接任意命令。

use std::collections::HashMap;
use std::fmt::Debug;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// 一次命令执行的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// 正常退出（code 0）。
    Ok(String),
    /// 命令存在但执行失败（非零退出）。
    Failed {
        /// 退出码（被信号终止时为 None）。
        code: Option<i32>,
        /// 标准错误（可能为空）。
        stderr: String,
    },
    /// 命令不存在 / 无法执行。
    Unavailable {
        /// 原因（用于日志，不返回给客户端）。
        message: String,
    },
}

impl CommandOutcome {
    /// 成功时取标准输出。
    pub fn stdout(&self) -> Option<&str> {
        match self {
            CommandOutcome::Ok(stdout) => Some(stdout),
            _ => None,
        }
    }

    /// 是否成功。
    pub fn is_ok(&self) -> bool {
        matches!(self, CommandOutcome::Ok(_))
    }
}

/// 命令执行器。
pub trait CommandRunner: Send + Sync + Debug {
    /// 执行 `program args...`。
    fn run(&self, program: &str, args: &[&str]) -> CommandOutcome;
}

/// 默认命令超时。
///
/// settings / dumpsys / pm 正常都是毫秒级；卡住时必须有限时间内失败，否则持有 config
/// 排他 flock 的写端点（PUT/DELETE /egress）会把整条配置 API（含 PUT /config）一起挂住。
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// 等待子进程退出时的轮询间隔。
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 真机执行器：直接 spawn（不做 shell 解析，不拼接字符串），并带硬超时。
///
/// 超时后 kill 子进程并返回 [CommandOutcome::Failed]（code=None，stderr 说明超时），
/// 上层（egress）统一映射成 503 upstream_unavailable(component="settings")。
#[derive(Debug, Clone, Copy)]
pub struct SystemRunner {
    timeout: Duration,
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
}

impl SystemRunner {
    /// 默认超时的执行器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 自定义超时（测试与排障用）。
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// 当前超时。
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
        let mut child = match Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return CommandOutcome::Unavailable {
                    message: format!("执行 {program} 失败: {error}"),
                };
            }
        };
        let deadline = Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    return match child.wait_with_output() {
                        Ok(output) => outcome_from_output(output),
                        Err(error) => CommandOutcome::Unavailable {
                            message: format!("读取 {program} 输出失败: {error}"),
                        },
                    };
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return CommandOutcome::Failed {
                            code: None,
                            stderr: format!("timeout after {}ms", self.timeout.as_millis()),
                        };
                    }
                    std::thread::sleep(COMMAND_POLL_INTERVAL);
                }
                Err(error) => {
                    return CommandOutcome::Unavailable {
                        message: format!("等待 {program} 失败: {error}"),
                    };
                }
            }
        }
    }
}

fn outcome_from_output(output: Output) -> CommandOutcome {
    if output.status.success() {
        CommandOutcome::Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        CommandOutcome::Failed {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// 测试执行器：按完整命令行（空格分隔）查表；未配置的命令返回 Unavailable。
#[derive(Debug, Default, Clone)]
pub struct FixtureRunner {
    responses: HashMap<String, CommandOutcome>,
}

impl FixtureRunner {
    /// 空表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一条命令输出。
    #[must_use]
    pub fn with_output(mut self, command: &str, stdout: &str) -> Self {
        self.responses.insert(
            normalize_key(command),
            CommandOutcome::Ok(stdout.to_owned()),
        );
        self
    }

    /// 注册一条失败命令。
    #[must_use]
    pub fn with_failure(mut self, command: &str, code: i32) -> Self {
        self.responses.insert(
            normalize_key(command),
            CommandOutcome::Failed {
                code: Some(code),
                stderr: String::new(),
            },
        );
        self
    }
}

impl CommandRunner for FixtureRunner {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
        // 与 with_output 一致地归一化空白，否则带空参数的命令（如清空排除列表）查不到
        let key = normalize_key(&command_key(program, args));
        self.responses
            .get(&key)
            .cloned()
            .unwrap_or(CommandOutcome::Unavailable {
                message: format!("fixture 未配置命令: {key}"),
            })
    }
}

/// `settings` 可执行文件名（Android 系统设置服务 CLI）。
pub const SETTINGS: &str = "settings";
/// `dumpsys` 可执行文件名。
pub const DUMPSYS: &str = "dumpsys";

/// `settings put <namespace> <key> <value>`。
///
/// 出口代理唯一允许的写方式；清除系统代理时 value 必须是 `:0`
/// （见 `egress::system::DIRECT_VALUE`），`settings delete` 无效。
pub fn settings_put(
    runner: &dyn CommandRunner,
    namespace: &str,
    key: &str,
    value: &str,
) -> CommandOutcome {
    runner.run(SETTINGS, &["put", namespace, key, value])
}

/// `settings get <namespace> <key>`（未设置时 Android 输出字符串 `null`）。
pub fn settings_get(runner: &dyn CommandRunner, namespace: &str, key: &str) -> CommandOutcome {
    runner.run(SETTINGS, &["get", namespace, key])
}

/// `dumpsys <service>`：用于全局代理的交叉验证（契约 §3.3）。
pub fn dumpsys(runner: &dyn CommandRunner, service: &str) -> CommandOutcome {
    runner.run(DUMPSYS, &[service])
}

/// 生成命令键（测试与 fixture 共用）。
pub fn command_key(program: &str, args: &[&str]) -> String {
    let mut key = program.to_owned();
    for arg in args {
        key.push(' ');
        key.push_str(arg);
    }
    key
}

fn normalize_key(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_returns_registered_output() {
        let runner = FixtureRunner::new().with_output("pm list packages -u", "package:com.a\n");
        assert_eq!(
            runner.run("pm", &["list", "packages", "-u"]),
            CommandOutcome::Ok("package:com.a\n".to_owned())
        );
        assert!(matches!(
            runner.run("getprop", &[]),
            CommandOutcome::Unavailable { .. }
        ));
    }

    #[test]
    fn missing_binary_is_unavailable() {
        let outcome = SystemRunner::default().run("device-faker-definitely-not-a-binary", &[]);
        assert!(matches!(outcome, CommandOutcome::Unavailable { .. }));
    }

    #[test]
    fn default_timeout_is_bounded_and_positive() {
        assert!(SystemRunner::default().timeout() >= Duration::from_secs(1));
        assert_eq!(Duration::from_secs(5), DEFAULT_COMMAND_TIMEOUT);
    }

    #[test]
    fn system_runner_times_out_instead_of_blocking_forever() {
        // sleep 5 秒，但执行器只等 150ms：必须在有限时间内返回 Failed（code=None，stderr 说明超时）
        let runner = SystemRunner::with_timeout(Duration::from_millis(150));
        let started = Instant::now();
        let outcome = runner.run("sleep", &["5"]);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(3),
            "必须在超时附近返回，实际 {elapsed:?}"
        );
        match outcome {
            CommandOutcome::Failed { code, stderr } => {
                assert_eq!(code, None);
                assert!(stderr.contains("timeout"), "{stderr}");
            }
            other => panic!("超时应返回 Failed，实际 {other:?}"),
        }
    }

    #[test]
    fn system_runner_returns_output_before_timeout() {
        let outcome = SystemRunner::with_timeout(Duration::from_secs(5)).run("echo", &["hello"]);
        assert_eq!(outcome, CommandOutcome::Ok("hello\n".to_owned()));
    }
}
