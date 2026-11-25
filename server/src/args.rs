// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 命令行参数。
//!
//! 不引入 clap：设备端二进制体积优先，参数表只有 8 个。
//! 支持 --flag value 与 --flag=value 两种写法。

use std::path::PathBuf;

use anyhow::{Result, bail};

/// 命令行参数（均为可选；server.toml 与默认值兜底）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliArgs {
    /// config.toml 路径。
    pub config_path: Option<PathBuf>,
    /// WebUI 静态目录。
    pub static_dir: Option<PathBuf>,
    /// 监听地址。
    pub bind: Option<String>,
    /// server.toml 路径。
    pub server_config: Option<PathBuf>,
    /// module.prop 路径。
    pub module_prop: Option<PathBuf>,
    /// 设备信息 fixture（host 测试用）。
    pub device_fixture: Option<PathBuf>,
    /// `/data/adb` 根目录（root 管理器探测；host 测试可指向 tempdir）。
    pub adb_dir: Option<PathBuf>,
    /// 打印帮助。
    pub help: bool,
    /// 打印版本。
    pub version: bool,
}

/// --help 文本。
pub const USAGE: &str = concat!(
    "devmask_server - DevMask 原生 Web 服务\n\n",
    "用法: devmask_server [选项]\n\n",
    "选项:\n",
    "  --config-path <PATH>     配置 config.toml 路径（默认 /data/adb/devmask/config/config.toml）\n",
    "  --static-dir <PATH>      WebUI 静态目录（默认 /data/adb/modules/devmask/webroot）\n",
    "  --bind <ADDR>            监听地址（默认 127.0.0.1:8686）\n",
    "  --server-config <PATH>   server.toml 路径（默认与 config.toml 同目录）\n",
    "  --module-prop <PATH>     module.prop 路径（默认静态目录的父目录）\n",
    "  --device-fixture <PATH>  设备信息 JSON fixture（host 测试用）\n",
    "  --adb-dir <PATH>         /data/adb 根目录（root 管理器探测，默认 /data/adb）\n",
    "  -h, --help               打印本帮助\n",
    "  -V, --version            打印版本\n",
);

impl CliArgs {
    /// 从进程参数（不含 argv[0]）解析。
    pub fn parse<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(argument) = args.next() {
            let (flag, inline) = match argument.split_once('=') {
                Some((flag, value)) => (flag.to_owned(), Some(value.to_owned())),
                None => (argument.clone(), None),
            };
            match flag.as_str() {
                "-h" | "--help" => parsed.help = true,
                "-V" | "--version" => parsed.version = true,
                "--config-path" => {
                    parsed.config_path = Some(PathBuf::from(take_value(&mut args, inline, &flag)?));
                }
                "--static-dir" => {
                    parsed.static_dir = Some(PathBuf::from(take_value(&mut args, inline, &flag)?));
                }
                "--bind" => {
                    parsed.bind = Some(take_value(&mut args, inline, &flag)?);
                }
                "--server-config" => {
                    parsed.server_config =
                        Some(PathBuf::from(take_value(&mut args, inline, &flag)?));
                }
                "--module-prop" => {
                    parsed.module_prop = Some(PathBuf::from(take_value(&mut args, inline, &flag)?));
                }
                "--device-fixture" => {
                    parsed.device_fixture =
                        Some(PathBuf::from(take_value(&mut args, inline, &flag)?));
                }
                "--adb-dir" => {
                    parsed.adb_dir = Some(PathBuf::from(take_value(&mut args, inline, &flag)?));
                }
                other => bail!("未知参数: {other}\n\n{USAGE}"),
            }
        }
        Ok(parsed)
    }

    /// 从 std::env::args 解析（跳过 argv[0]）。
    pub fn from_env() -> Result<Self> {
        Self::parse(std::env::args().skip(1))
    }
}

fn take_value<I>(args: &mut I, inline: Option<String>, flag: &str) -> Result<String>
where
    I: Iterator<Item = String>,
{
    if let Some(value) = inline {
        if value.is_empty() {
            bail!("参数 {flag} 缺少取值");
        }
        return Ok(value);
    }
    match args.next() {
        Some(value) if !value.is_empty() => Ok(value),
        _ => bail!("参数 {flag} 缺少取值"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<CliArgs> {
        CliArgs::parse(args.iter().map(|argument| (*argument).to_owned()))
    }

    #[test]
    fn parses_long_flags_with_space_and_equals() {
        let parsed = parse(&[
            "--config-path",
            "/tmp/df/config.toml",
            "--static-dir=/tmp/df/webroot",
            "--bind",
            "127.0.0.1:0",
        ])
        .unwrap();
        assert_eq!(
            parsed.config_path,
            Some(PathBuf::from("/tmp/df/config.toml"))
        );
        assert_eq!(parsed.static_dir, Some(PathBuf::from("/tmp/df/webroot")));
        assert_eq!(parsed.bind.as_deref(), Some("127.0.0.1:0"));
        assert!(!parsed.help);
    }

    #[test]
    fn help_and_version_flags() {
        assert!(parse(&["--help"]).unwrap().help);
        assert!(parse(&["-h"]).unwrap().help);
        assert!(parse(&["--version"]).unwrap().version);
    }

    #[test]
    fn unknown_flag_is_error() {
        assert!(parse(&["--nope"]).is_err());
    }

    #[test]
    fn missing_value_is_error() {
        assert!(parse(&["--bind"]).is_err());
        assert!(parse(&["--bind="]).is_err());
    }
}
