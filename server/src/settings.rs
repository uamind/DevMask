// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! server.toml、命令行覆盖与启动前安全校验（docs/API.md 2.1、3.4；计划书附录 A）。
//!
//! 优先级：命令行 > server.toml > 设备默认值。
//!
//! 硬性约束：
//!
//! - 默认只绑 loopback；bind 不是 loopback 时必须同时 allow_lan = true 且至少一个 Token，
//!   否则拒绝启动并在日志里写明原因（不给「无认证监听局域网」的机会）；
//! - 未知作用域名、重复 Token id 都视为配置错误，启动阶段直接失败；
//! - 不把 Token 明文写进 server.toml，只写 sha256。

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::args::CliArgs;
use crate::auth::{Scope, TokenEntry};

/// 默认监听地址：只对设备本机可见。
pub const DEFAULT_BIND: &str = "127.0.0.1:8686";
/// 默认 `/data/adb` 根目录（root 管理器探测）。
pub const DEFAULT_ADB_DIR: &str = "/data/adb";
/// 默认静态资源目录（模块安装位置）。
pub const DEFAULT_STATIC_DIR: &str = "/data/adb/modules/device_faker/webroot";
/// 默认配置路径。
pub const DEFAULT_CONFIG_PATH: &str = "/data/adb/device_faker/config/config.toml";
/// 默认请求体上限：2 MiB。
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
/// 默认读请求限流（本增量只记录，执行留给后续增量）。
pub const DEFAULT_READ_RATE_PER_MIN: u32 = 120;
/// 默认写请求限流。
pub const DEFAULT_WRITE_RATE_PER_MIN: u32 = 30;

/// server.toml 的 [[tokens]] 条目。
#[derive(Debug, Clone, Deserialize)]
pub struct TokenFile {
    /// 稳定 id；缺省为 default。
    #[serde(default)]
    pub id: Option<String>,
    /// 人类可读名字；缺省与 id 相同。
    #[serde(default)]
    pub name: Option<String>,
    /// sha256(token)，允许带 sha256: 前缀。
    pub hash: String,
    /// 作用域列表；空列表视为配置错误。
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// server.toml 的磁盘结构。未知键忽略，便于后续增量加字段。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerFile {
    /// 监听地址。
    #[serde(default)]
    pub bind: Option<String>,
    /// WebUI 静态目录。
    #[serde(default)]
    pub static_dir: Option<PathBuf>,
    /// 配置 config.toml 路径。
    #[serde(default)]
    pub config_path: Option<PathBuf>,
    /// 备份目录（本增量只透传，写入由后续增量使用）。
    #[serde(default)]
    pub backup_dir: Option<PathBuf>,
    /// 审计日志路径（本增量只透传）。
    #[serde(default)]
    pub audit_log: Option<PathBuf>,
    /// 是否允许监听非 loopback 地址。
    #[serde(default)]
    pub allow_lan: Option<bool>,
    /// 请求体上限。
    #[serde(default)]
    pub body_limit_bytes: Option<usize>,
    /// 读请求限流（次/分钟/Token）。
    #[serde(default)]
    pub read_rate_per_min: Option<u32>,
    /// 写请求限流（次/分钟/Token）。
    #[serde(default)]
    pub write_rate_per_min: Option<u32>,
    /// Token 表。
    #[serde(default)]
    pub tokens: Vec<TokenFile>,
    /// `/data/adb` 根目录（root 管理器探测用；host 测试可指向 tempdir）。
    #[serde(default)]
    pub adb_dir: Option<PathBuf>,
}

/// 解析后的运行设置。
#[derive(Debug, Clone)]
pub struct Settings {
    /// 监听地址（--bind / server.toml / 默认）。
    pub bind: String,
    /// bind 是否 loopback。
    pub bind_is_loopback: bool,
    /// 是否显式允许局域网监听。
    pub allow_lan: bool,
    /// WebUI 静态目录。
    pub static_dir: PathBuf,
    /// config.toml 路径。
    pub config_path: PathBuf,
    /// server.toml 路径。
    pub server_config_path: PathBuf,
    /// module.prop 路径（默认静态目录的父目录）。
    pub module_prop_path: PathBuf,
    /// 备份目录（透传）。
    pub backup_dir: Option<PathBuf>,
    /// 审计日志（透传）。
    pub audit_log: Option<PathBuf>,
    /// 请求体上限。
    pub body_limit_bytes: usize,
    /// 读请求限流。
    pub read_rate_per_min: u32,
    /// 写请求限流。
    pub write_rate_per_min: u32,
    /// Token 表（已归一化哈希）。
    pub tokens: Vec<TokenEntry>,
    /// 首次生成 server.toml 时要写入的 bind；命令行覆盖时为 None（避免把临时参数固化）。
    pub bind_for_new_file: Option<String>,
    /// `/data/adb` 根目录：GET /device 的 root 管理器探测用。
    pub adb_dir: PathBuf,
}

impl Settings {
    /// 测试与 fixture 用的最小设置（loopback + 给定 Token）。
    pub fn for_test(config_path: PathBuf, static_dir: PathBuf, tokens: Vec<TokenEntry>) -> Self {
        let module_prop_path = static_dir
            .parent()
            .map(|parent| parent.join("module.prop"))
            .unwrap_or_else(|| PathBuf::from("module.prop"));
        let server_config_path = config_path.with_file_name("server.toml");
        let adb_dir = adb_dir_for_test(&static_dir);
        Self {
            bind: DEFAULT_BIND.to_owned(),
            bind_is_loopback: true,
            allow_lan: false,
            static_dir,
            config_path,
            server_config_path,
            module_prop_path,
            backup_dir: None,
            audit_log: None,
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            read_rate_per_min: DEFAULT_READ_RATE_PER_MIN,
            write_rate_per_min: DEFAULT_WRITE_RATE_PER_MIN,
            tokens,
            bind_for_new_file: None,
            adb_dir,
        }
    }

    /// 审计日志路径：显式配置优先，否则 `<数据目录>/logs/api-audit.jsonl`。
    ///
    /// 设备上 config.toml 在 `/data/adb/device_faker/config/`，因此数据目录是它的父目录；
    /// host 上如果配置不在 `config/` 子目录里，就放在配置同目录的 `logs/` 下。
    pub fn audit_path(&self) -> PathBuf {
        if let Some(path) = &self.audit_log {
            return path.clone();
        }
        let base = match self.config_path.parent() {
            Some(parent)
                if parent
                    .file_name()
                    .is_some_and(|name| name == std::ffi::OsStr::new("config")) =>
            {
                parent.parent().unwrap_or(parent)
            }
            Some(parent) => parent,
            None => Path::new("."),
        };
        base.join("logs").join("api-audit.jsonl")
    }
}

fn adb_dir_for_test(static_dir: &Path) -> PathBuf {
    static_dir
        .parent()
        .map(|parent| parent.join("adb"))
        .unwrap_or_else(|| PathBuf::from("adb"))
}

/// bind 是否是 loopback（支持 [::1]:8686 与 localhost:8686）。
pub fn is_loopback_bind(bind: &str) -> bool {
    if let Ok(address) = bind.parse::<SocketAddr>() {
        return address.ip().is_loopback();
    }
    if let Some((host, _port)) = bind.rsplit_once(':') {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.eq_ignore_ascii_case("localhost") {
            return true;
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            return ip.is_loopback();
        }
    }
    false
}

/// 从磁盘读取 server.toml；文件不存在返回默认值（首次启动的常态）。
pub fn load_server_file(path: &Path) -> Result<ServerFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            parse_server_file(&text).with_context(|| format!("解析 {} 失败", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ServerFile::default()),
        Err(error) => Err(error).with_context(|| format!("读取 {} 失败", path.display())),
    }
}

/// 解析 server.toml 文本。
pub fn parse_server_file(text: &str) -> Result<ServerFile> {
    if text.trim().is_empty() {
        return Ok(ServerFile::default());
    }
    Ok(toml::from_str::<ServerFile>(text)?)
}

/// 把 [[tokens]] 条目转成运行期 TokenEntry，并做启动期校验。
pub fn parse_tokens(entries: &[TokenFile]) -> Result<Vec<TokenEntry>> {
    let mut tokens = Vec::with_capacity(entries.len());
    let mut seen_ids: Vec<String> = Vec::new();
    for entry in entries {
        let id = entry.id.clone().unwrap_or_else(|| "default".to_owned());
        let name = entry.name.clone().unwrap_or_else(|| id.clone());
        if seen_ids.iter().any(|seen| seen == &id) {
            bail!("server.toml 中 Token id 重复: {id}");
        }
        if entry.scopes.is_empty() {
            bail!("server.toml 中 Token {id} 没有 scopes；至少需要一个（read/write/ops/admin）");
        }
        let mut scopes = Vec::with_capacity(entry.scopes.len());
        for raw in &entry.scopes {
            let Some(scope) = Scope::parse(raw.trim()) else {
                bail!("server.toml 中 Token {id} 含未知作用域: {raw}");
            };
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }
        let hash = crate::auth::normalize_hash(&entry.hash);
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!(
                "server.toml 中 Token {id} 的 hash 不是 64 位十六进制 sha256（只存哈希，不要写明文）: {hash}"
            );
        }
        seen_ids.push(id.clone());
        tokens.push(TokenEntry::new(id, name, &hash, scopes));
    }
    Ok(tokens)
}

/// Token 生成命令提示（拒绝启动时输出，避免用户去猜格式）。
pub fn token_generation_hint() -> &'static str {
    "TOKEN=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=') && echo \"$TOKEN\" && printf '%s' \"$TOKEN\" | sha256sum"
}

/// 合并命令行与 server.toml，得到运行设置并做启动期校验。
pub fn resolve(cli: &CliArgs) -> Result<Settings> {
    let preliminary_config_path = cli
        .config_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
    let server_config_path = cli
        .server_config
        .clone()
        .unwrap_or_else(|| preliminary_config_path.with_file_name("server.toml"));
    let file = load_server_file(&server_config_path)?;

    let config_path = cli
        .config_path
        .clone()
        .or_else(|| file.config_path.clone())
        .unwrap_or(preliminary_config_path);
    let static_dir = cli
        .static_dir
        .clone()
        .or_else(|| file.static_dir.clone())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STATIC_DIR));
    let module_prop_path = cli.module_prop.clone().unwrap_or_else(|| {
        static_dir
            .parent()
            .map(|parent| parent.join("module.prop"))
            .unwrap_or_else(|| PathBuf::from("module.prop"))
    });

    let bind_from_cli = cli.bind.is_some();
    let bind = cli
        .bind
        .clone()
        .or_else(|| file.bind.clone())
        .unwrap_or_else(|| DEFAULT_BIND.to_owned());
    let bind_for_new_file = if !bind_from_cli && file.bind.is_none() {
        Some(bind.clone())
    } else {
        None
    };

    let settings = Settings {
        bind_is_loopback: is_loopback_bind(&bind),
        bind,
        allow_lan: file.allow_lan.unwrap_or(false),
        static_dir,
        config_path,
        server_config_path,
        module_prop_path,
        backup_dir: file.backup_dir.clone(),
        audit_log: file.audit_log.clone(),
        body_limit_bytes: file.body_limit_bytes.unwrap_or(DEFAULT_BODY_LIMIT_BYTES),
        read_rate_per_min: file.read_rate_per_min.unwrap_or(DEFAULT_READ_RATE_PER_MIN),
        write_rate_per_min: file
            .write_rate_per_min
            .unwrap_or(DEFAULT_WRITE_RATE_PER_MIN),
        tokens: parse_tokens(&file.tokens)?,
        bind_for_new_file,
        adb_dir: cli
            .adb_dir
            .clone()
            .or_else(|| file.adb_dir.clone())
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ADB_DIR)),
    };
    settings.validate()?;
    Ok(settings)
}

impl Settings {
    /// 启动前安全校验（不合格直接拒绝启动）。
    pub fn validate(&self) -> Result<()> {
        if self.bind.parse::<SocketAddr>().is_err() {
            bail!(
                "bind 必须是 ip:port 形式，当前为 {:?}（例: 127.0.0.1:8686）",
                self.bind
            );
        }
        if self.body_limit_bytes == 0 {
            bail!("body_limit_bytes 必须大于 0");
        }
        if !self.bind_is_loopback {
            if !self.allow_lan {
                bail!(
                    "bind = {} 不是 loopback，但 server.toml 的 allow_lan 不是 true；\
                     为避免误把管理面暴露到网络，服务拒绝启动。确认要监听局域网时把 allow_lan 改为 true。",
                    self.bind
                );
            }
            if self.tokens.is_empty() {
                bail!(
                    "bind = {} 不是 loopback，但没有配置任何 Token；为避免无认证暴露，服务拒绝启动。\
                     生成 Token 的方式：{}；把输出的 sha256 写进 {} 的 [[tokens]] 表（scopes 至少 read）。",
                    self.bind,
                    token_generation_hint(),
                    self.server_config_path.display()
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util;

    fn token_entry(token: &str) -> TokenEntry {
        TokenEntry::new(
            "default",
            "owner",
            &util::sha256_hex(token.as_bytes()),
            vec![Scope::Read],
        )
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_bind("127.0.0.1:8686"));
        assert!(is_loopback_bind("127.1.2.3:0"));
        assert!(is_loopback_bind("[::1]:8686"));
        assert!(is_loopback_bind("localhost:8686"));
        assert!(!is_loopback_bind("0.0.0.0:8686"));
        assert!(!is_loopback_bind("[::]:8686"));
        assert!(!is_loopback_bind("192.168.1.10:8686"));
        assert!(!is_loopback_bind("garbage"));
    }

    #[test]
    fn parse_server_file_accepts_minimal_and_full_documents() {
        let empty = parse_server_file("").unwrap();
        assert!(empty.bind.is_none() && empty.tokens.is_empty());
        let text = r#"
bind = "127.0.0.1:8686"
allow_lan = false
body_limit_bytes = 2097152
read_rate_per_min = 120
write_rate_per_min = 30

[[tokens]]
id = "owner"
name = "phone"
hash = "sha256:ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
scopes = ["read", "write", "ops", "admin"]
"#;
        let file = parse_server_file(text).unwrap();
        assert_eq!(file.bind.as_deref(), Some("127.0.0.1:8686"));
        let tokens = parse_tokens(&file.tokens).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].id, "owner");
        assert_eq!(tokens[0].name, "phone");
        assert_eq!(tokens[0].hash.len(), 64);
        assert_eq!(tokens[0].scopes.len(), 4);
    }

    #[test]
    fn parse_tokens_rejects_unknown_scope_and_bad_hash() {
        let unknown_scope = TokenFile {
            id: Some("a".to_owned()),
            name: None,
            hash: "a".repeat(64),
            scopes: vec!["root".to_owned()],
        };
        assert!(parse_tokens(&[unknown_scope]).is_err());

        let bad_hash = TokenFile {
            id: Some("b".to_owned()),
            name: None,
            hash: "df_plaintext_token".to_owned(),
            scopes: vec!["read".to_owned()],
        };
        assert!(parse_tokens(&[bad_hash]).is_err());

        let empty_scopes = TokenFile {
            id: Some("c".to_owned()),
            name: None,
            hash: "a".repeat(64),
            scopes: Vec::new(),
        };
        assert!(parse_tokens(&[empty_scopes]).is_err());
    }

    #[test]
    fn non_loopback_without_token_is_rejected() {
        let mut settings = Settings::for_test(
            PathBuf::from("/tmp/df/config.toml"),
            PathBuf::from("/tmp/df/webroot"),
            Vec::new(),
        );
        settings.bind = "0.0.0.0:8686".to_owned();
        settings.bind_is_loopback = false;
        settings.allow_lan = true;
        let error = settings.validate().unwrap_err().to_string();
        assert!(error.contains("没有配置任何 Token"), "{error}");
        assert!(error.contains("head -c 32 /dev/urandom"), "{error}");
    }

    #[test]
    fn non_loopback_without_allow_lan_is_rejected() {
        let mut settings = Settings::for_test(
            PathBuf::from("/tmp/df/config.toml"),
            PathBuf::from("/tmp/df/webroot"),
            vec![token_entry("df_x")],
        );
        settings.bind = "0.0.0.0:8686".to_owned();
        settings.bind_is_loopback = false;
        let error = settings.validate().unwrap_err().to_string();
        assert!(error.contains("allow_lan"), "{error}");
    }

    #[test]
    fn lan_with_token_and_allow_lan_passes() {
        let mut settings = Settings::for_test(
            PathBuf::from("/tmp/df/config.toml"),
            PathBuf::from("/tmp/df/webroot"),
            vec![token_entry("df_x")],
        );
        settings.bind = "0.0.0.0:8686".to_owned();
        settings.bind_is_loopback = false;
        settings.allow_lan = true;
        settings.validate().unwrap();
    }

    #[test]
    fn invalid_bind_is_rejected() {
        let mut settings = Settings::for_test(
            PathBuf::from("/tmp/df/config.toml"),
            PathBuf::from("/tmp/df/webroot"),
            vec![token_entry("df_x")],
        );
        settings.bind = "localhost".to_owned();
        let error = settings.validate().unwrap_err().to_string();
        assert!(error.contains("ip:port"), "{error}");
    }
}
