// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 系统命令执行抽象。
//!
//! 只用于契约白名单里的语义化命令（getprop / pm / am / dumpsys / monkey），
//! 不暴露任意 exec 端点（docs/dev/server-api-plan.md 3.2、10.2）。
//! 抽象成 trait 是为了 host 测试能用 fixture 注入 `pm` / `dumpsys` 样例输出。

use std::collections::HashMap;
use std::fmt::Debug;
use std::process::Command;

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

/// 真机执行器：直接 spawn（不做 shell 解析，不拼接字符串）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
        match Command::new(program).args(args).output() {
            Ok(output) => {
                if output.status.success() {
                    CommandOutcome::Ok(String::from_utf8_lossy(&output.stdout).into_owned())
                } else {
                    CommandOutcome::Failed {
                        code: output.status.code(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    }
                }
            }
            Err(error) => CommandOutcome::Unavailable {
                message: format!("执行 {program} 失败: {error}"),
            },
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
        let key = command_key(program, args);
        self.responses
            .get(&key)
            .cloned()
            .unwrap_or(CommandOutcome::Unavailable {
                message: format!("fixture 未配置命令: {key}"),
            })
    }
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
        let outcome = SystemRunner.run("device-faker-definitely-not-a-binary", &[]);
        assert!(matches!(outcome, CommandOutcome::Unavailable { .. }));
    }
}
