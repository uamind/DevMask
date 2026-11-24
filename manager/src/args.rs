// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 极简参数解析：只有 6 个开关，手写即可，避免引入 clap。

use std::path::PathBuf;

/// 命令行参数（未经环境变量与配置文件合并）。
#[derive(Default, Clone)]
pub struct Args {
    pub bind: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub static_dir: Option<PathBuf>,
    pub token: Option<String>,
    pub config: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
    pub token_file: Option<PathBuf>,
    pub log_level: Option<String>,
    pub help: bool,
    pub version: bool,
}

/// 参数解析错误。
#[derive(Debug, thiserror::Error)]
pub enum ArgError {
    #[error("未知参数：{0}（用 --help 查看用法）")]
    Unknown(String),
    #[error("参数 {0} 缺少值")]
    MissingValue(String),
}

/// 解析参数（不含程序名）。
pub fn parse<I>(iter: I) -> Result<Args, ArgError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = Args::default();
    let mut iter = iter.into_iter();

    while let Some(argument) = iter.next() {
        if !argument.starts_with('-') {
            return Err(ArgError::Unknown(argument));
        }

        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) => (name.to_string(), Some(value.to_string())),
            None => (argument.clone(), None),
        };

        match name.as_str() {
            "-h" | "--help" => args.help = true,
            "-V" | "--version" => args.version = true,
            "--bind" => args.bind = Some(take_value(&mut iter, inline, "--bind")?),
            "--data-dir" => {
                args.data_dir = Some(PathBuf::from(take_value(&mut iter, inline, "--data-dir")?))
            }
            "--static-dir" => {
                args.static_dir = Some(PathBuf::from(take_value(
                    &mut iter,
                    inline,
                    "--static-dir",
                )?))
            }
            "--token" => args.token = Some(take_value(&mut iter, inline, "--token")?),
            "--config" => {
                args.config = Some(PathBuf::from(take_value(&mut iter, inline, "--config")?))
            }
            "--key-file" => {
                args.key_file = Some(PathBuf::from(take_value(&mut iter, inline, "--key-file")?))
            }
            "--token-file" => {
                args.token_file = Some(PathBuf::from(take_value(
                    &mut iter,
                    inline,
                    "--token-file",
                )?))
            }
            "--log-level" => args.log_level = Some(take_value(&mut iter, inline, "--log-level")?),
            _ => return Err(ArgError::Unknown(name)),
        }
    }

    Ok(args)
}

fn take_value<I>(iter: &mut I, inline: Option<String>, flag: &str) -> Result<String, ArgError>
where
    I: Iterator<Item = String>,
{
    if let Some(value) = inline {
        return Ok(value);
    }
    iter.next()
        .ok_or_else(|| ArgError::MissingValue(flag.to_string()))
}

/// 帮助文本。
pub fn help_text() -> String {
    let mut text = String::new();
    text.push_str("device_faker_manager ");
    text.push_str(env!("CARGO_PKG_VERSION"));
    text.push('\n');
    text.push_str("Device Faker 多设备管理端 / Relay\n\n");
    text.push_str("用法：device_faker_manager [选项]\n\n");
    text.push_str("选项：\n");
    text.push_str("  --bind <ADDR>        监听地址（默认 127.0.0.1:8765）\n");
    text.push_str("  --data-dir <PATH>    数据目录（默认按平台推导）\n");
    text.push_str("  --static-dir <PATH>  WebUI dist 目录（默认自动查找）\n");
    text.push_str("  --token <TOKEN>      Manager Token（默认 admin123，仅限 loopback）\n");
    text.push_str("  --token-file <PATH>  Token 文件（默认 <data-dir>/manager.token）\n");
    text.push_str("  --key-file <PATH>    设备 Token 密钥文件（默认 <data-dir>/manager.key）\n");
    text.push_str("  --config <PATH>      配置文件（默认 <data-dir>/manager.toml）\n");
    text.push_str("  --log-level <LEVEL>  日志级别（默认 info；也可用 DFM_LOG / RUST_LOG）\n");
    text.push_str("  -h, --help           显示帮助\n");
    text.push_str("  -V, --version        显示版本\n\n");
    text.push_str(
        "环境变量：DFM_BIND、DFM_DATA_DIR、DFM_STATIC_DIR、DFM_TOKEN、DFM_TOKEN_FILE、\n",
    );
    text.push_str("          DFM_KEY_FILE、DFM_CONFIG、DFM_LOG、DFM_APPLY_CONCURRENCY、DFM_SYNC_CONCURRENCY\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(items: &[&str]) -> Result<Args, ArgError> {
        parse(items.iter().map(|item| item.to_string()))
    }

    #[test]
    fn parses_flags_and_values() {
        let args = parse_args(&[
            "--bind",
            "0.0.0.0:9999",
            "--data-dir=/tmp/dfm",
            "--token",
            "secret",
            "--help",
        ])
        .unwrap();
        assert_eq!(args.bind.as_deref(), Some("0.0.0.0:9999"));
        assert_eq!(args.data_dir, Some(PathBuf::from("/tmp/dfm")));
        assert_eq!(args.token.as_deref(), Some("secret"));
        assert!(args.help);
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(matches!(parse_args(&["--nope"]), Err(ArgError::Unknown(_))));
    }

    #[test]
    fn rejects_missing_value() {
        assert!(matches!(
            parse_args(&["--token"]),
            Err(ArgError::MissingValue(_))
        ));
    }

    #[test]
    fn help_text_mentions_defaults() {
        let text = help_text();
        assert!(text.contains("127.0.0.1:8765"));
        assert!(text.contains("admin123"));
        assert!(text.contains("DFM_TOKEN"));
    }
}
