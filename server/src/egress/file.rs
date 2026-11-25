// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! egress.toml 的模型、解析/渲染与 0600 原子写存储（契约 §2.1 / §2.2）。
//!
//! 存储语义与 config.toml 同构，差别只有两处（刻意不同，不要「统一」）：
//!
//! - 文件权限 0600（含备份），而不是 0644；
//! - 读时自愈：mode & 0o077 != 0 立即 chmod 0600，并给响应 warning egress_perm_fixed。
//!
//! 写路径复用 core 的公开原语：FileLock（flock 串行化）、atomic_write
//! （tmp → fsync → rename → fsync(dir)，临时文件 O_CREAT|O_EXCL 一步 0600）
//! 与 create_backup（备份 0600、保留最近 10 份）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use device_faker_core::revision::{FileLock, atomic_write, create_backup, set_mode, sha256_hex};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::egress::{
    DEFAULT_PROBE_TIMEOUT_SECONDS, DEFAULT_PROBE_URL, EGRESS_FILE_MODE, EGRESS_FILE_NAME,
    EGRESS_KEEP_BACKUPS, MAX_HOST_LEN, MAX_PASSWORD_LEN, MAX_PROBE_TIMEOUT_SECONDS, MAX_PROXIES,
    MAX_USERNAME_LEN,
};
use crate::util;

/// egress.toml 的当前结构版本（只接受 1）。
pub const VERSION: u32 = 1;

/// 代理协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyType {
    /// HTTP 代理（绝对 URI / CONNECT）。
    Http,
    /// SOCKS5 代理。
    Socks5,
}

impl ProxyType {
    /// 线上/TOML 字面量。
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyType::Http => "http",
            ProxyType::Socks5 => "socks5",
        }
    }

    /// 解析枚举（大小写敏感，与契约一致）。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "http" => Some(ProxyType::Http),
            "socks5" => Some(ProxyType::Socks5),
            _ => None,
        }
    }
}

/// 一条代理定义（契约 §2.1 的 [proxies.<name>]）。
///
/// Debug 手写脱敏：username / password 只输出是否设置，值永远不出现（契约 §7.1 第 4 条）。
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyDefinition {
    /// 代理名（同时是 TOML 表名）。
    pub name: String,
    /// 协议。
    pub kind: ProxyType,
    /// 主机名或 IP 字面量（不含 scheme/端口/路径）。
    pub host: String,
    /// 端口（1..=65535）。
    pub port: u16,
    /// 用户名（可选，永不出现在响应/日志）。
    pub username: Option<String>,
    /// 密码（可选，永不出现在响应/日志）。
    pub password: Option<String>,
}

impl ProxyDefinition {
    /// 是否带凭据（响应里唯一的凭据信号）。
    pub fn has_credentials(&self) -> bool {
        self.username.is_some() || self.password.is_some()
    }

    /// host:port（设备级写入的期望值与只读展示用）。
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

impl std::fmt::Debug for ProxyDefinition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyDefinition")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username.as_ref().map(|_| "<redacted>"))
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// [probe] 段（探测策略，不是出口本身）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSettings {
    /// 探测 URL（必须 http://）。
    pub url: String,
    /// 超时秒数（1..=30）。
    pub timeout_seconds: u64,
    /// 代理不可用时是否 fail-open 切直连。
    pub fail_open: bool,
}

impl Default for ProbeSettings {
    fn default() -> Self {
        Self {
            url: DEFAULT_PROBE_URL.to_owned(),
            timeout_seconds: DEFAULT_PROBE_TIMEOUT_SECONDS,
            fail_open: true,
        }
    }
}

/// egress.toml 的完整内存模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressFile {
    /// 结构版本。
    pub version: u32,
    /// 代理定义（保持文件书写顺序）。
    pub proxies: Vec<ProxyDefinition>,
    /// 探测策略。
    pub probe: ProbeSettings,
    /// 未识别的键（忽略但保留；路径形如 proxies.x.foo）。
    pub unknown_fields: Vec<String>,
}

impl Default for EgressFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            proxies: Vec::new(),
            probe: ProbeSettings::default(),
            unknown_fields: Vec::new(),
        }
    }
}

impl EgressFile {
    /// 空模型（只有默认 probe 策略）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 测试夹具：一条 http 代理 + 默认 probe。
    pub fn for_test(name: &str, host: &str, port: u16) -> Self {
        Self {
            version: VERSION,
            proxies: vec![ProxyDefinition {
                name: name.to_owned(),
                kind: ProxyType::Http,
                host: host.to_owned(),
                port,
                username: None,
                password: None,
            }],
            probe: ProbeSettings::default(),
            unknown_fields: Vec::new(),
        }
    }

    /// 按名字取定义。
    pub fn proxy(&self, name: &str) -> Option<&ProxyDefinition> {
        self.proxies.iter().find(|proxy| proxy.name == name)
    }

    /// 是否没有任何代理定义。
    pub fn is_empty(&self) -> bool {
        self.proxies.is_empty()
    }
}

/// 存储层错误（不含任何字段值，避免密码经错误信息泄漏）。
#[derive(Debug)]
pub enum EgressError {
    /// 文件系统错误。
    Io(io::Error),
    /// egress.toml 无法解析（只回报键路径/行号，不回显内容）。
    Parse(String),
}

impl std::fmt::Display for EgressError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EgressError::Io(error) => write!(formatter, "io error: {error}"),
            EgressError::Parse(message) => write!(formatter, "egress.toml 解析失败: {message}"),
        }
    }
}

impl std::error::Error for EgressError {}

impl From<io::Error> for EgressError {
    fn from(error: io::Error) -> Self {
        EgressError::Io(error)
    }
}

/// 读取到的 egress.toml 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressSnapshot {
    /// 文件是否存在。
    pub exists: bool,
    /// 原始文本。
    pub text: String,
    /// 原始字节（egress revision 的输入之一）。
    pub bytes: Vec<u8>,
    /// 字节级 sha256（备份名用）。
    pub revision: String,
    /// 解析结果；解析失败时是空模型。
    pub model: EgressFile,
    /// 字节数。
    pub size: u64,
    /// mtime（UTC RFC 3339）。
    pub mtime: Option<String>,
    /// 读时自愈前的权限位（Some 表示本次修过权限）。
    pub perm_fixed: Option<u32>,
    /// 解析失败原因（Some 时调用方必须 fail closed：GET 500 / 写入以空文档为基线）。
    pub parse_error: Option<String>,
}

impl EgressSnapshot {
    fn missing() -> Self {
        Self {
            exists: false,
            text: String::new(),
            bytes: Vec::new(),
            revision: sha256_hex(b""),
            model: EgressFile::empty(),
            size: 0,
            mtime: None,
            perm_fixed: None,
            parse_error: None,
        }
    }
}

/// 一次 egress.toml 写入的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressWriteOutcome {
    /// 内容是否变化（false 时不写盘、不备份）。
    pub changed: bool,
    /// 写入前 revision（文件原本不存在时为 None）。
    pub previous_revision: Option<String>,
    /// 写入后 revision。
    pub revision: String,
    /// 备份 ID（<时间>-<revision 前 8 位>）。
    pub backup_id: Option<String>,
    /// 写入前文本（diff 用）。
    pub old_text: String,
    /// 写入后文本（diff 用）。
    pub new_text: String,
}

impl EgressWriteOutcome {
    fn unchanged(current: &EgressSnapshot) -> Self {
        Self {
            changed: false,
            previous_revision: current.exists.then(|| current.revision.clone()),
            revision: current.revision.clone(),
            backup_id: None,
            old_text: current.text.clone(),
            new_text: current.text.clone(),
        }
    }
}

/// egress.toml 的读写入口：flock + 0600 原子写 + 0600 备份 + 读时自愈。
#[derive(Debug, Clone)]
pub struct EgressStore {
    path: PathBuf,
    backup_dir: PathBuf,
    lock_path: PathBuf,
}

impl EgressStore {
    /// 用 egress.toml 路径与（可选的）备份目录构造。
    pub fn new(path: impl Into<PathBuf>, backup_dir: Option<PathBuf>) -> Self {
        let path = path.into();
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "egress.toml".to_owned());
        let lock_path = path.with_file_name(format!("{file_name}.lock"));
        let backup_dir = backup_dir.unwrap_or_else(|| {
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join("backups")
        });
        Self {
            path,
            backup_dir,
            lock_path,
        }
    }

    /// 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 锁文件路径。
    pub fn lock_path(&self) -> PathBuf {
        self.lock_path.clone()
    }

    /// 备份目录。
    pub fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    /// 取排他锁（跨文件事务用：config 锁 → egress 锁，顺序固定）。
    pub fn acquire(&self) -> Result<FileLock, EgressError> {
        FileLock::acquire(&self.lock_path).map_err(EgressError::Io)
    }

    /// 读取快照（共享锁）。
    pub fn read(&self) -> Result<EgressSnapshot, EgressError> {
        let _lock = FileLock::acquire_shared(&self.lock_path).map_err(EgressError::Io)?;
        self.read_locked()
    }

    /// 在调用方已持排他锁的前提下读取快照。
    pub fn read_locked(&self) -> Result<EgressSnapshot, EgressError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(EgressSnapshot::missing());
            }
            Err(error) => return Err(EgressError::Io(error)),
        };
        let perm_fixed = heal_mode(&self.path)?;
        let metadata = fs::metadata(&self.path).ok();
        let size = metadata
            .as_ref()
            .map_or(bytes.len() as u64, fs::Metadata::len);
        let mtime = metadata
            .and_then(|meta| meta.modified().ok())
            .map(util::rfc3339);
        let text = match String::from_utf8(bytes.clone()) {
            Ok(text) => text,
            Err(_) => {
                return Ok(EgressSnapshot {
                    exists: true,
                    text: String::new(),
                    revision: sha256_hex(&bytes),
                    bytes,
                    model: EgressFile::empty(),
                    size,
                    mtime,
                    perm_fixed,
                    parse_error: Some("egress.toml 不是合法 UTF-8".to_owned()),
                });
            }
        };
        let (model, parse_error) = match parse(&text) {
            Ok(model) => (model, None),
            Err(error) => (EgressFile::empty(), Some(error.to_string())),
        };
        Ok(EgressSnapshot {
            exists: true,
            revision: sha256_hex(&bytes),
            bytes,
            text,
            model,
            size,
            mtime,
            perm_fixed,
            parse_error,
        })
    }

    /// 单独写入（自己取锁）；跨文件事务请用 [Self::write_locked]。
    pub fn write(&self, model: &EgressFile) -> Result<EgressWriteOutcome, EgressError> {
        let _lock = self.acquire()?;
        let current = self.read_locked()?;
        self.write_locked(&current, model)
    }

    /// 在调用方已持排他锁的前提下写入：渲染 → 备份 → 原子写（0600）。
    pub fn write_locked(
        &self,
        current: &EgressSnapshot,
        model: &EgressFile,
    ) -> Result<EgressWriteOutcome, EgressError> {
        let base: DocumentMut = if current.exists {
            current.text.parse().unwrap_or_default()
        } else {
            DocumentMut::new()
        };
        let new_text = render(&base, model)?;
        if current.exists && current.text == new_text {
            return Ok(EgressWriteOutcome::unchanged(current));
        }
        let backup = if current.exists {
            create_backup(
                &self.path,
                &self.backup_dir,
                EGRESS_KEEP_BACKUPS,
                Some(&current.revision),
            )
            .map_err(EgressError::Io)?
        } else {
            None
        };
        atomic_write(&self.path, new_text.as_bytes(), EGRESS_FILE_MODE)?;
        Ok(EgressWriteOutcome {
            changed: true,
            previous_revision: current.exists.then(|| current.revision.clone()),
            revision: sha256_hex(new_text.as_bytes()),
            backup_id: backup
                .as_deref()
                .and_then(|path| backup_id(path, EGRESS_FILE_NAME)),
            old_text: current.text.clone(),
            new_text,
        })
    }
}

/// 解析 egress.toml 文本（不回显任何字段值）。
pub fn parse(text: &str) -> Result<EgressFile, EgressError> {
    if text.trim().is_empty() {
        return Ok(EgressFile::empty());
    }
    let document: DocumentMut = text
        .parse()
        .map_err(|error| EgressError::Parse(short_toml_error(text, &error)))?;

    let version = match document.get("version") {
        None => VERSION,
        Some(Item::Value(found)) => match found.as_integer() {
            Some(1) => 1,
            Some(_) => return Err(EgressError::Parse("version 只支持 1".to_owned())),
            None => return Err(EgressError::Parse("version 必须是整数".to_owned())),
        },
        Some(_) => return Err(EgressError::Parse("version 必须是整数".to_owned())),
    };

    let mut proxies = Vec::new();
    if let Some(item) = document.get("proxies") {
        let table = item
            .as_table()
            .ok_or_else(|| EgressError::Parse("proxies 必须是表".to_owned()))?;
        if table.len() > MAX_PROXIES {
            return Err(EgressError::Parse(format!("proxies 最多 {MAX_PROXIES} 条")));
        }
        for (name, item) in table.iter() {
            let table = item
                .as_table()
                .ok_or_else(|| EgressError::Parse(format!("proxies.{name} 必须是表")))?;
            proxies.push(parse_proxy(name, table)?);
        }
    }

    let probe = match document.get("probe") {
        None => ProbeSettings::default(),
        Some(item) => parse_probe(
            item.as_table()
                .ok_or_else(|| EgressError::Parse("probe 必须是表".to_owned()))?,
        )?,
    };

    let mut unknown_fields = Vec::new();
    for (key, _) in document.iter() {
        if !matches!(key, "version" | "proxies" | "probe") {
            unknown_fields.push(key.to_owned());
        }
    }
    if let Some(table) = document.get("probe").and_then(Item::as_table) {
        for (key, _) in table.iter() {
            if !matches!(key, "url" | "timeout_seconds" | "fail_open") {
                unknown_fields.push(format!("probe.{key}"));
            }
        }
    }
    if let Some(table) = document.get("proxies").and_then(Item::as_table) {
        for (name, item) in table.iter() {
            let Some(proxy) = item.as_table() else {
                continue;
            };
            for (key, _) in proxy.iter() {
                if !matches!(key, "type" | "host" | "port" | "username" | "password") {
                    unknown_fields.push(format!("proxies.{name}.{key}"));
                }
            }
        }
    }

    Ok(EgressFile {
        version,
        proxies,
        probe,
        unknown_fields,
    })
}

fn parse_proxy(name: &str, table: &Table) -> Result<ProxyDefinition, EgressError> {
    let kind = table
        .get("type")
        .and_then(Item::as_str)
        .and_then(ProxyType::parse)
        .ok_or_else(|| EgressError::Parse(format!("proxies.{name}.type 必须是 http 或 socks5")))?;
    let host = table
        .get("host")
        .and_then(Item::as_str)
        .ok_or_else(|| EgressError::Parse(format!("proxies.{name}.host 必须是字符串")))?;
    if host.is_empty() || host.chars().count() > MAX_HOST_LEN {
        return Err(EgressError::Parse(format!(
            "proxies.{name}.host 长度不合法"
        )));
    }
    let port = table
        .get("port")
        .and_then(Item::as_integer)
        .filter(|value| (1..=65535).contains(value))
        .ok_or_else(|| {
            EgressError::Parse(format!("proxies.{name}.port 必须是 1..=65535 的整数"))
        })?;
    let username = optional_string(table, "username", name, MAX_USERNAME_LEN)?;
    let password = optional_string(table, "password", name, MAX_PASSWORD_LEN)?;
    Ok(ProxyDefinition {
        name: name.to_owned(),
        kind,
        host: host.to_owned(),
        port: port as u16,
        username,
        password,
    })
}

fn optional_string(
    table: &Table,
    key: &str,
    name: &str,
    max_len: usize,
) -> Result<Option<String>, EgressError> {
    let Some(item) = table.get(key) else {
        return Ok(None);
    };
    let Some(raw) = item.as_str() else {
        return Err(EgressError::Parse(format!(
            "proxies.{name}.{key} 必须是字符串"
        )));
    };
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.chars().count() > max_len {
        return Err(EgressError::Parse(format!(
            "proxies.{name}.{key} 超过 {max_len} 字符"
        )));
    }
    Ok(Some(raw.to_owned()))
}

fn parse_probe(table: &Table) -> Result<ProbeSettings, EgressError> {
    let mut probe = ProbeSettings::default();
    if let Some(item) = table.get("url") {
        let url = item
            .as_str()
            .ok_or_else(|| EgressError::Parse("probe.url 必须是字符串".to_owned()))?;
        if !url.starts_with("http://") {
            return Err(EgressError::Parse(
                "probe.url 必须以 http:// 开头".to_owned(),
            ));
        }
        probe.url = url.to_owned();
    }
    if let Some(item) = table.get("timeout_seconds") {
        let seconds = item
            .as_integer()
            .filter(|value| (1..=MAX_PROBE_TIMEOUT_SECONDS as i64).contains(value))
            .ok_or_else(|| {
                EgressError::Parse(format!(
                    "probe.timeout_seconds 必须是 1..={MAX_PROBE_TIMEOUT_SECONDS} 的整数"
                ))
            })?;
        probe.timeout_seconds = seconds as u64;
    }
    if let Some(item) = table.get("fail_open") {
        probe.fail_open = item
            .as_bool()
            .ok_or_else(|| EgressError::Parse("probe.fail_open 必须是布尔值".to_owned()))?;
    }
    Ok(probe)
}

/// 把模型渲染成 egress.toml 文本：保留 base 里未识别的键与其它顶层项（契约 §2.1）。
pub fn render(base: &DocumentMut, model: &EgressFile) -> Result<String, EgressError> {
    let mut document = base.clone();
    // 只在值不同的时候改 version：直接 insert 会丢掉首行的注释装饰。
    let version = i64::from(model.version);
    if document.get("version").and_then(Item::as_integer) != Some(version) {
        document.insert("version", value(version));
    }

    let mut proxies = Table::new();
    for proxy in &model.proxies {
        let mut table = base
            .get("proxies")
            .and_then(Item::as_table)
            .and_then(|table| table.get(&proxy.name))
            .and_then(Item::as_table)
            .cloned()
            .unwrap_or_default();
        table.insert("type", value(proxy.kind.as_str()));
        table.insert("host", value(proxy.host.as_str()));
        table.insert("port", value(i64::from(proxy.port)));
        match &proxy.username {
            Some(username) => {
                table.insert("username", value(username.as_str()));
            }
            None => {
                table.remove("username");
            }
        }
        match &proxy.password {
            Some(password) => {
                table.insert("password", value(password.as_str()));
            }
            None => {
                table.remove("password");
            }
        }
        proxies.insert(&proxy.name, Item::Table(table));
    }
    // [proxies] 里的非表项（未识别的标量键）原样保留；有它们时不能把表标成 implicit
    let mut has_unknown_scalars = false;
    if let Some(base_proxies) = base.get("proxies").and_then(Item::as_table) {
        for (key, item) in base_proxies.iter() {
            if item.is_table() {
                continue;
            }
            proxies.insert(key, item.clone());
            has_unknown_scalars = true;
        }
    }
    if proxies.is_empty() {
        document.remove("proxies");
    } else {
        // 只写 [proxies.<name>] 子表，不额外写一个空的 [proxies] 头
        proxies.set_implicit(!has_unknown_scalars);
        document.insert("proxies", Item::Table(proxies));
    }

    let mut probe = base
        .get("probe")
        .and_then(Item::as_table)
        .cloned()
        .unwrap_or_default();
    probe.insert("url", value(model.probe.url.as_str()));
    probe.insert("timeout_seconds", value(model.probe.timeout_seconds as i64));
    probe.insert("fail_open", value(model.probe.fail_open));
    document.insert("probe", Item::Table(probe));

    Ok(document.to_string())
}

fn short_toml_error(text: &str, error: &toml_edit::TomlError) -> String {
    let line = error
        .span()
        .map(|span| text[..span.start.min(text.len())].lines().count())
        .unwrap_or(0);
    if line == 0 {
        error.message().to_owned()
    } else {
        format!("{}（第 {line} 行）", error.message())
    }
}

/// 读时自愈：mode & 0o077 != 0 → chmod 0600，返回自愈前权限。
fn heal_mode(path: &Path) -> io::Result<Option<u32>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            set_mode(path, EGRESS_FILE_MODE)?;
            // 自愈必须留痕（t11 F15）：这条日志与响应 warning egress_perm_fixed 一一对应。
            tracing::warn!(
                path = %path.display(),
                mode = format!("{mode:o}"),
                "egress.toml 权限过宽，已自愈为 0600"
            );
            return Ok(Some(mode));
        }
        Ok(None)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(None)
    }
}

/// 备份文件名 → 备份 ID：`<file>.<YYYYMMDDTHHMMSS.nnnnnnnnnZ>-<revision 前 8 位>.bak` → 中间那段。
///
/// 契约（§1.4 的 `backupId`）只要求「时间戳-rev8」；这里显式按文件名前缀剥离，
/// 避免 `config.toml` / `egress.toml` 里的点被当成第一段分隔符。
pub fn backup_id(path: &Path, file_name: &str) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let without_suffix = name.strip_suffix(".bak")?;
    let id = without_suffix.strip_prefix(&format!("{file_name}."))?;
    (!id.is_empty()).then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# egress.toml
version = 1

[proxies.us_la_01]
type = "http"
host = "203.0.113.10"
port = 1080
username = "user"
password = "pass"

[proxies.jp_tokyo_02]
type = "socks5"
host = "198.51.100.7"
port = 1080

[probe]
url = "http://checkip.amazonaws.com/"
timeout_seconds = 5
fail_open = true
"#;

    #[test]
    fn parse_reads_full_document_in_file_order() {
        let model = parse(SAMPLE).unwrap();
        assert_eq!(model.version, 1);
        assert_eq!(model.proxies.len(), 2);
        assert_eq!(model.proxies[0].name, "us_la_01");
        assert_eq!(model.proxies[0].kind, ProxyType::Http);
        assert_eq!(model.proxies[0].port, 1080);
        assert!(model.proxies[0].has_credentials());
        assert_eq!(model.proxies[1].name, "jp_tokyo_02");
        assert_eq!(model.proxies[1].kind, ProxyType::Socks5);
        assert!(!model.proxies[1].has_credentials());
        assert_eq!(model.probe, ProbeSettings::default());
        assert!(model.unknown_fields.is_empty());
    }

    #[test]
    fn render_round_trips_and_preserves_unknown_keys() {
        let document: DocumentMut = SAMPLE.parse().unwrap();
        let mut model = parse(SAMPLE).unwrap();
        let text = render(&document, &model).unwrap();
        let reparsed = parse(&text).unwrap();
        assert_eq!(reparsed, model);

        let with_unknown: DocumentMut = SAMPLE
            .replace(
                "username = \"user\"",
                "username = \"user\"\nproxy_host = \"x\"",
            )
            .parse()
            .unwrap();
        let text = render(&with_unknown, &model).unwrap();
        assert!(text.contains("proxy_host = \"x\""), "{text}");

        model.proxies[0].password = Some("newpass".to_owned());
        let text = render(&with_unknown, &model).unwrap();
        assert!(text.contains("password = \"newpass\""), "{text}");
        assert!(text.contains("# egress.toml"), "{text}");

        let empty = EgressFile::empty();
        let text = render(&document, &empty).unwrap();
        assert!(!text.contains("[proxies"), "{text}");
        assert!(text.contains("[probe]"), "{text}");
    }

    #[test]
    fn parse_rejects_bad_version_and_bad_fields() {
        assert!(parse("version = 2\n").is_err());
        assert!(parse("version = \"1\"\n").is_err());
        assert!(parse("[proxies.a]\ntype = \"ftp\"\nhost = \"h\"\nport = 1\n").is_err());
        assert!(parse("[proxies.a]\ntype = \"http\"\nhost = \"h\"\nport = 0\n").is_err());
        assert!(parse("[proxies.a]\ntype = \"http\"\nport = 1\n").is_err());
        assert!(parse("[probe]\nurl = \"https://x/\"\n").is_err());
        assert!(parse("[probe]\ntimeout_seconds = 31\n").is_err());
        let broken = "[proxies.a]\ntype = \"http\"\npassword = \"s3cret\"\nhost = \n";
        let error = parse(broken).unwrap_err().to_string();
        assert!(!error.contains("s3cret"), "{error}");
    }

    #[test]
    fn debug_output_never_contains_credentials() {
        let model = parse(SAMPLE).unwrap();
        let debug = format!("{model:?}");
        assert!(!debug.contains("\"user\""), "{debug}");
        assert!(!debug.contains("\"pass\""), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
    }

    #[test]
    fn store_writes_0600_and_backs_up_with_same_mode() {
        let dir = tempfile::tempdir().unwrap();
        let store = EgressStore::new(dir.path().join("egress.toml"), None);
        let model = parse(SAMPLE).unwrap();
        let outcome = store.write(&model).unwrap();
        assert!(outcome.changed);
        assert_eq!(outcome.previous_revision, None);
        let written = store.read().unwrap();
        assert_eq!(written.model, parse(SAMPLE).unwrap(), "语义必须一致");
        assert!(
            written.text.contains("[proxies.us_la_01]"),
            "{}",
            written.text
        );
        assert!(
            !written.text.contains("[proxies]\n"),
            "不应写出空的 [proxies] 头: {}",
            written.text
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "新建文件必须一步 0600");
        }

        let mut changed = model.clone();
        changed.probe.fail_open = false;
        let outcome = store.write(&changed).unwrap();
        assert!(outcome.changed);
        assert!(outcome.backup_id.is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let backups: Vec<_> = fs::read_dir(store.backup_dir()).unwrap().collect();
            assert_eq!(backups.len(), 1);
            for entry in backups {
                let mode = entry.unwrap().metadata().unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "备份也必须 0600");
            }
        }
    }

    #[test]
    fn store_heals_loose_permissions_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("egress.toml");
        let store = EgressStore::new(path.clone(), None);
        store.write(&EgressFile::empty()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            let snapshot = store.read().unwrap();
            assert_eq!(snapshot.perm_fixed, Some(0o644));
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            assert_eq!(store.read().unwrap().perm_fixed, None, "第二次读不再自愈");
        }
    }

    #[test]
    fn parse_error_is_reported_but_write_falls_back_to_empty_base() {
        let dir = tempfile::tempdir().unwrap();
        let store = EgressStore::new(dir.path().join("egress.toml"), None);
        fs::write(store.path(), "not = valid = toml\n").unwrap();
        let snapshot = store.read().unwrap();
        assert!(snapshot.parse_error.is_some());
        assert!(snapshot.exists);
        let outcome = store.write(&EgressFile::empty()).unwrap();
        assert!(outcome.changed);
        assert!(store.read().unwrap().parse_error.is_none());
    }

    #[test]
    fn too_many_proxies_is_a_parse_error() {
        let mut text = String::from("version = 1\n");
        for index in 0..=MAX_PROXIES {
            text.push_str(&format!(
                "\n[proxies.p{index}]\ntype = \"http\"\nhost = \"h\"\nport = 1\n"
            ));
        }
        assert!(parse(&text).is_err());
    }
}
