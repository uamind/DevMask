// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 配置解析：CLI / 环境变量 / manager.toml / 默认值的优先级合并。

use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    args::Args,
    state::{TokenSource, TokenState},
    static_files,
};

/// 默认监听地址。
pub const DEFAULT_BIND: &str = "127.0.0.1:8765";
/// 默认 Manager Token。
pub const DEFAULT_TOKEN: &str = "admin123";
/// 默认下发并发。
pub const DEFAULT_APPLY_CONCURRENCY: usize = 4;
/// 默认同步并发。
pub const DEFAULT_SYNC_CONCURRENCY: usize = 4;
/// 默认读请求超时。
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 10_000;
/// 默认写请求超时。
pub const DEFAULT_WRITE_TIMEOUT_MS: u64 = 15_000;

const APP_DIR_NAME: &str = "device-faker-manager";

/// 配置加载错误。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Message(String),
    #[error("读取 {path} 失败：{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("解析 {path} 失败：{source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

impl ConfigError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

/// 解析后的运行配置。
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub static_dir: Option<PathBuf>,
    pub static_dir_candidates: Vec<PathBuf>,
    pub token: TokenState,
    pub token_file: PathBuf,
    pub key_file: PathBuf,
    pub config_file: PathBuf,
    pub apply_concurrency: usize,
    pub sync_concurrency: usize,
    pub request_timeout_ms: u64,
    pub write_timeout_ms: u64,
}

impl Config {
    /// SQLite 主库路径。
    pub fn db_file(&self) -> PathBuf {
        self.data_dir.join("manager.db")
    }

    /// 按优先级加载配置，并校验监听地址与 Token 的组合是否安全。
    pub fn load(args: Args) -> Result<Self, ConfigError> {
        let data_dir = resolve_data_dir(&args)?;
        let config_file = args
            .config
            .clone()
            .or_else(|| env_non_empty("DFM_CONFIG").map(PathBuf::from))
            .unwrap_or_else(|| data_dir.join("manager.toml"));

        let file_config = load_file_config(&config_file)?;

        let bind_text = args
            .bind
            .clone()
            .or_else(|| env_non_empty("DFM_BIND"))
            .or_else(|| file_config.bind.clone())
            .unwrap_or_else(|| DEFAULT_BIND.to_string());
        let bind: SocketAddr = bind_text.parse().map_err(|_| {
            ConfigError::message(format!(
                "监听地址不合法：{bind_text}（应形如 127.0.0.1:8765）"
            ))
        })?;

        let token_file = args
            .token_file
            .clone()
            .or_else(|| env_non_empty("DFM_TOKEN_FILE").map(PathBuf::from))
            .unwrap_or_else(|| data_dir.join("manager.token"));

        let file_token = read_token_file(&token_file)?;
        let config_token = file_config
            .manager
            .as_ref()
            .and_then(|section| section.token.clone())
            .or_else(|| file_config.token.clone());
        let token = resolve_token(
            args.token.clone(),
            env_non_empty("DFM_TOKEN"),
            file_token,
            config_token,
        );
        Self::validate_bind_token(bind, &token)?;

        let key_file = args
            .key_file
            .clone()
            .or_else(|| env_non_empty("DFM_KEY_FILE").map(PathBuf::from))
            .unwrap_or_else(|| data_dir.join("manager.key"));

        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(explicit) = args
            .static_dir
            .clone()
            .or_else(|| env_non_empty("DFM_STATIC_DIR").map(PathBuf::from))
            .or_else(|| file_config.static_dir.clone())
        {
            candidates.push(explicit);
        }
        if let Ok(exe) = env::current_exe()
            && let Some(parent) = exe.parent()
        {
            candidates.push(parent.join("webui/dist"));
        }
        if let Ok(cwd) = env::current_dir() {
            candidates.push(cwd.join("manager/webui/dist"));
            candidates.push(cwd.join("webui/dist"));
        }
        candidates.dedup();

        let static_dir = static_files::resolve_static_dir(&candidates);
        if static_dir.is_none() {
            tracing::warn!(
                candidates = ?candidates,
                "未找到 WebUI dist（缺少 index.html）；静态页面将显示构建提示"
            );
        }

        let apply_concurrency = resolve_concurrency(
            "DFM_APPLY_CONCURRENCY",
            file_config.apply_concurrency,
            DEFAULT_APPLY_CONCURRENCY,
        )?;
        let sync_concurrency = resolve_concurrency(
            "DFM_SYNC_CONCURRENCY",
            file_config.sync_concurrency,
            DEFAULT_SYNC_CONCURRENCY,
        )?;

        Ok(Self {
            bind,
            data_dir,
            static_dir,
            static_dir_candidates: candidates,
            token,
            token_file,
            key_file,
            config_file,
            apply_concurrency,
            sync_concurrency,
            request_timeout_ms: file_config
                .request_timeout_ms
                .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS),
            write_timeout_ms: file_config
                .write_timeout_ms
                .unwrap_or(DEFAULT_WRITE_TIMEOUT_MS),
        })
    }

    /// 非 loopback 监听时，拒绝默认值或过短的 Token（防止把 admin123 暴露到局域网）。
    pub fn validate_bind_token(bind: SocketAddr, token: &TokenState) -> Result<(), ConfigError> {
        if bind.ip().is_loopback() {
            return Ok(());
        }
        if token.value == DEFAULT_TOKEN || token.value.len() < 8 {
            return Err(ConfigError::message(format!(
                "拒绝在非 loopback 地址 {bind} 上使用默认或过短的 Manager Token；\
                 请用 --token / DFM_TOKEN / manager.token 设置至少 8 字符的随机 Token"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    bind: Option<String>,
    static_dir: Option<PathBuf>,
    token: Option<String>,
    apply_concurrency: Option<usize>,
    sync_concurrency: Option<usize>,
    request_timeout_ms: Option<u64>,
    write_timeout_ms: Option<u64>,
    manager: Option<ManagerSection>,
}

#[derive(Debug, Default, Deserialize)]
struct ManagerSection {
    token: Option<String>,
}

const ALLOWED_KEYS: &[&str] = &[
    "bind",
    "static_dir",
    "token",
    "apply_concurrency",
    "sync_concurrency",
    "request_timeout_ms",
    "write_timeout_ms",
    "manager",
];

fn load_file_config(path: &Path) -> Result<FileConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileConfig::default());
        }
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let parsed: FileConfig = toml::from_str(&text).map_err(|source| ConfigError::Toml {
        path: path.to_path_buf(),
        source,
    })?;

    if let Ok(value) = text.parse::<toml::Value>()
        && let Some(table) = value.as_table()
    {
        for key in table.keys() {
            if !ALLOWED_KEYS.contains(&key.as_str()) {
                tracing::warn!(file = %path.display(), key = %key, "忽略未知配置项");
            }
        }
    }

    Ok(parsed)
}

fn resolve_data_dir(args: &Args) -> Result<PathBuf, ConfigError> {
    if let Some(dir) = args.data_dir.clone() {
        return Ok(dir);
    }
    if let Some(dir) = env_non_empty("DFM_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }

    #[cfg(target_os = "macos")]
    let default_dir =
        home_dir().map(|home| home.join("Library/Application Support").join(APP_DIR_NAME));

    #[cfg(target_os = "windows")]
    let default_dir =
        env_non_empty("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join(APP_DIR_NAME));

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let default_dir = if let Some(dir) = env_non_empty("XDG_DATA_HOME") {
        Some(PathBuf::from(dir).join(APP_DIR_NAME))
    } else {
        home_dir().map(|home| home.join(".local/share").join(APP_DIR_NAME))
    };

    default_dir.ok_or_else(|| {
        ConfigError::message("无法确定数据目录，请用 --data-dir 或 DFM_DATA_DIR 指定")
    })
}

fn home_dir() -> Option<PathBuf> {
    env_non_empty("HOME")
        .or_else(|| env_non_empty("USERPROFILE"))
        .map(PathBuf::from)
}

fn env_non_empty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 读取 manager.token：不存在视为未配置；存在但不可读必须 fail closed。
fn read_token_file(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.trim().to_string()).filter(|text| !text.is_empty())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Token 优先级：CLI > env > 文件 > 配置文件 > 默认值。
fn resolve_token(
    cli: Option<String>,
    env_token: Option<String>,
    file_token: Option<String>,
    config_token: Option<String>,
) -> TokenState {
    let candidates = [
        (cli, TokenSource::Cli),
        (env_token, TokenSource::Env),
        (file_token, TokenSource::File),
        (config_token, TokenSource::Config),
    ];
    for (value, source) in candidates {
        if let Some(value) = value {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return TokenState { value, source };
            }
        }
    }
    TokenState {
        value: DEFAULT_TOKEN.to_string(),
        source: TokenSource::Default,
    }
}

fn resolve_concurrency(
    env_name: &str,
    file_value: Option<usize>,
    default: usize,
) -> Result<usize, ConfigError> {
    let value = match env_non_empty(env_name) {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| ConfigError::message(format!("{env_name} 不是合法整数：{raw}")))?,
        None => file_value.unwrap_or(default),
    };
    if !(1..=16).contains(&value) {
        return Err(ConfigError::message(format!(
            "{env_name} 必须在 1-16 之间，当前为 {value}"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StaticSite;

    fn token_state(value: &str, source: TokenSource) -> TokenState {
        TokenState {
            value: value.to_string(),
            source,
        }
    }

    #[test]
    fn token_precedence_is_cli_env_file_config_default() {
        let resolved = resolve_token(
            Some("from-cli".into()),
            Some("from-env".into()),
            Some("from-file".into()),
            Some("from-config".into()),
        );
        assert_eq!(resolved.value, "from-cli");
        assert_eq!(resolved.source, TokenSource::Cli);

        let resolved = resolve_token(
            None,
            Some("from-env".into()),
            Some("from-file".into()),
            Some("from-config".into()),
        );
        assert_eq!(resolved.source, TokenSource::Env);

        let resolved = resolve_token(
            None,
            None,
            Some("from-file".into()),
            Some("from-config".into()),
        );
        assert_eq!(resolved.source, TokenSource::File);

        let resolved = resolve_token(None, None, None, Some("from-config".into()));
        assert_eq!(resolved.source, TokenSource::Config);

        let resolved = resolve_token(None, None, None, None);
        assert_eq!(resolved.value, DEFAULT_TOKEN);
        assert_eq!(resolved.source, TokenSource::Default);

        let resolved = resolve_token(Some("  ".into()), None, None, None);
        assert_eq!(resolved.source, TokenSource::Default);
    }

    #[test]
    fn rejects_default_token_on_non_loopback() {
        let bind: SocketAddr = "0.0.0.0:8765".parse().unwrap();
        let token = token_state(DEFAULT_TOKEN, TokenSource::Default);
        assert!(Config::validate_bind_token(bind, &token).is_err());

        let short = token_state("short", TokenSource::Cli);
        assert!(Config::validate_bind_token(bind, &short).is_err());

        let strong = token_state("a-strong-manager-token", TokenSource::Cli);
        assert!(Config::validate_bind_token(bind, &strong).is_ok());
    }

    #[test]
    fn allows_default_token_on_loopback() {
        let bind: SocketAddr = "127.0.0.1:8765".parse().unwrap();
        let token = token_state(DEFAULT_TOKEN, TokenSource::Default);
        assert!(Config::validate_bind_token(bind, &token).is_ok());
        let bind: SocketAddr = "[::1]:8765".parse().unwrap();
        assert!(Config::validate_bind_token(bind, &token).is_ok());
    }

    #[test]
    fn concurrency_range_is_enforced() {
        assert_eq!(
            resolve_concurrency("DFM_TEST_UNSET", Some(3), 4).unwrap(),
            3
        );
        assert_eq!(resolve_concurrency("DFM_TEST_UNSET", None, 4).unwrap(), 4);
    }

    #[test]
    fn token_file_missing_is_none_but_unreadable_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("manager.token");
        assert!(matches!(read_token_file(&missing), Ok(None)));

        let readable = dir.path().join("readable.token");
        std::fs::write(&readable, "  file-token  \n").unwrap();
        assert_eq!(
            read_token_file(&readable).unwrap().as_deref(),
            Some("file-token")
        );

        // 目录不是合法 token 文件：读取失败必须返回错误，不能静默回退。
        assert!(matches!(
            read_token_file(dir.path()),
            Err(ConfigError::Io { .. })
        ));
    }

    #[test]
    fn static_site_requires_index() {
        let dir = tempfile::tempdir().unwrap();
        assert!(StaticSite::new(dir.path()).is_none());
        std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        assert!(StaticSite::new(dir.path()).is_some());
    }
}
