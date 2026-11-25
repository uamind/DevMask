// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 配置写事务：同一个 flock 内完成「读 → If-Match → 编辑 → 语义校验 → 备份 → 原子写」。
//!
//! core 的 `ConfigStore` 有 `write(text, expected)`（整写带 If-Match）与 `update(apply)`（读改写，
//! 不带 If-Match）；结构化端点需要「带 If-Match 的读改写」，因此这里用 core 的公开原语
//! （[`FileLock`] / [`create_backup`] / [`atomic_write`]）组成同一把锁内的事务，语义与
//! `ConfigStore::commit` 完全一致（备份名、保留份数、0644、fsync 顺序）。

use std::fs;
use std::path::Path;

use axum::http::{HeaderMap, header::IF_MATCH};
use devmask_core::config::Config;
use devmask_core::edit::{self, EditError};
use devmask_core::revision::{self, ConfigStore, FileLock, StoreError};
use devmask_core::validate::{self, ValidationOptions, ValidationReport};
use toml_edit::DocumentMut;

use crate::error::ApiError;
use crate::util;

/// 配置文件权限：Android 运行时要求 0644。
pub const CONFIG_FILE_MODE: u32 = 0o644;
/// 自动备份保留份数（与 core 默认一致）。
pub const BACKUP_KEEP: usize = 10;

/// 读取到的配置快照（含文件元数据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// 文件是否存在。
    pub exists: bool,
    /// 原始文本。
    pub text: String,
    /// revision（sha256 小写十六进制）。
    pub revision: String,
    /// 字节数。
    pub size: u64,
    /// mtime（UTC RFC 3339）；不存在时为 None。
    pub mtime: Option<String>,
}

/// 读取快照：共享 flock + 文件元数据。
pub fn read_snapshot(store: &ConfigStore) -> Result<Snapshot, StoreError> {
    let snapshot = store.read()?;
    let (size, mtime) = if snapshot.exists {
        match fs::metadata(store.path()) {
            Ok(metadata) => (metadata.len(), metadata.modified().ok().map(util::rfc3339)),
            Err(_) => (0, None),
        }
    } else {
        (0, None)
    };
    Ok(Snapshot {
        exists: snapshot.exists,
        text: snapshot.text,
        revision: snapshot.revision,
        size,
        mtime,
    })
}

/// 在调用方**已持有排他 flock** 时读取快照。
///
/// 不能再调用 [read_snapshot]：它会再取一次共享 flock，而 flock 绑定的是「打开的文件描述」，
/// 同一进程内的两个 fd 之间也会互斥 → 自我死锁。跨文件事务（egress 的 config + egress.toml）
/// 必须用这个函数读 config.toml。
pub fn read_snapshot_locked(store: &ConfigStore) -> Result<Snapshot, StoreError> {
    let (exists, text) = match fs::read_to_string(store.path()) {
        Ok(text) => (true, text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, String::new()),
        Err(error) => return Err(StoreError::Io(error)),
    };
    let (size, mtime) = if exists {
        match fs::metadata(store.path()) {
            Ok(metadata) => (metadata.len(), metadata.modified().ok().map(util::rfc3339)),
            Err(_) => (0, None),
        }
    } else {
        (0, None)
    };
    Ok(Snapshot {
        exists,
        revision: revision::revision_of(&text),
        text,
        size,
        mtime,
    })
}

/// If-Match 的两种形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// `If-Match: *`：无条件写。
    Any,
    /// 指定 revision（已去掉引号与可选 `sha256:` 前缀）。
    Exact(String),
}

/// 归一化 revision：去引号、去可选 `sha256:` 前缀；其余逐字符保留。
pub fn normalize_revision(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    let without_prefix = unquoted
        .strip_prefix("sha256:")
        .or_else(|| unquoted.strip_prefix("SHA256:"))
        .unwrap_or(unquoted);
    without_prefix.trim().to_owned()
}

/// 解析 If-Match：缺失或空值返回 None（调用方给 428）。
pub fn if_match(headers: &HeaderMap) -> Option<Precondition> {
    let raw = headers.get(IF_MATCH)?.to_str().ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    let first = raw.split(',').next().unwrap_or(raw).trim();
    if first == "*" {
        return Some(Precondition::Any);
    }
    Some(Precondition::Exact(normalize_revision(first)))
}

/// 写端点取 If-Match；缺失 → 428 precondition_required。
pub fn require_if_match(headers: &HeaderMap, request_id: &str) -> Result<Precondition, ApiError> {
    if_match(headers).ok_or_else(|| ApiError::precondition_required(request_id))
}

/// 事务结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxOutcome {
    /// 写入前 revision；文件原本不存在时为 None。
    pub previous_revision: Option<String>,
    /// 写入后 revision；dry run 或不落盘时为当前值。
    pub revision: String,
    /// 内容是否真的变化。
    pub changed: bool,
    /// 本次备份 ID（文件名里的 `<时间>-<revision 前 8 位>`）。
    pub backup_id: Option<String>,
    /// 旧文本（diff 用）。
    pub old_text: String,
    /// 新文本（diff 用）。
    pub new_text: String,
}

/// 事务错误。
#[derive(Debug)]
pub enum TxError {
    /// If-Match 不匹配。
    RevisionMismatch {
        /// 客户端给的 revision。
        expected: String,
        /// 服务端当前 revision；文件不存在时为 None。
        actual: Option<String>,
    },
    /// 语义校验失败（422）。
    Validation(ValidationReport),
    /// API 层规模/长度校验失败（422，错误码已是契约值）。
    ValidationErrors(Vec<crate::dto::ValidationError>),
    /// 文档编辑失败。
    Edit(EditError),
    /// 文件系统/存储错误。
    Store(StoreError),
    /// 磁盘上的现有配置无法解析（500，不是客户端错误）。
    InvalidCurrent(String),
    /// 处理器在锁内产生的领域错误（404/409/422 等）。
    Domain(ApiError),
}

impl TxError {
    /// 转成统一错误信封。
    pub fn into_api(self, request_id: &str) -> ApiError {
        match self {
            TxError::Domain(error) => error,
            TxError::RevisionMismatch { expected, actual } => {
                ApiError::config_revision_mismatch(request_id, &expected, actual.as_deref())
            }
            TxError::ValidationErrors(errors) => ApiError::validation_failed(
                request_id,
                serde_json::Value::Array(errors.iter().map(|error| error.to_value()).collect()),
            ),
            TxError::Validation(report) => {
                let (errors, _) = crate::dto::map_validation_report(&report);
                ApiError::validation_failed(
                    request_id,
                    serde_json::Value::Array(errors.iter().map(|error| error.to_value()).collect()),
                )
            }
            TxError::Edit(error) => match error {
                EditError::NotFound { table, name } => {
                    ApiError::not_found(request_id, table, &name)
                }
                EditError::Invalid { field, message } => ApiError::validation_failed(
                    request_id,
                    serde_json::json!([{ "path": field, "code": "invalid_value", "message": message }]),
                ),
                other => ApiError::internal_error(request_id, format!("配置编辑失败: {other}")),
            },
            TxError::Store(error) => {
                ApiError::internal_error(request_id, format!("配置读写失败: {error}"))
            }
            TxError::InvalidCurrent(message) => {
                ApiError::internal_error(request_id, format!("磁盘上的配置无法解析: {message}"))
            }
        }
    }
}

impl From<StoreError> for TxError {
    fn from(error: StoreError) -> Self {
        TxError::Store(error)
    }
}

impl From<EditError> for TxError {
    fn from(error: EditError) -> Self {
        TxError::Edit(error)
    }
}

impl From<ApiError> for TxError {
    fn from(error: ApiError) -> Self {
        TxError::Domain(error)
    }
}

/// 在同一把排他 flock 内执行「读 → If-Match → 编辑 → 校验 → 备份 → 原子写」。
///
/// `apply` 拿到的 `Config` 是**当前磁盘内容**的模型，供领域检查（模板是否存在、
/// 重复包名等）使用；它返回的业务值原样透传。`dry_run=true` 时不备份、不落盘，
/// `revision` 保持当前值。
pub fn transact<T, F>(
    store: &ConfigStore,
    precondition: &Precondition,
    dry_run: bool,
    apply: F,
) -> Result<(T, TxOutcome), TxError>
where
    F: FnOnce(&mut DocumentMut, &Config) -> Result<T, TxError>,
{
    let lock_path = store.lock_path();
    let _lock = FileLock::acquire(&lock_path).map_err(StoreError::Io)?;
    let (exists, text) = read_text(store.path())?;
    let current_revision = revision::revision_of(&text);
    check_precondition(precondition, &current_revision, exists)?;

    let mut document =
        edit::parse_document(&text).map_err(|error| TxError::InvalidCurrent(format!("{error}")))?;
    let config = Config::from_toml(&text)
        .map_err(|error| TxError::InvalidCurrent(crate::util::parse_error_message(&error)))?;

    let value = apply(&mut document, &config)?;
    let new_text = document.to_string();

    if new_text == text {
        return Ok((
            value,
            TxOutcome {
                previous_revision: exists.then(|| current_revision.clone()),
                revision: current_revision,
                changed: false,
                backup_id: None,
                old_text: text.clone(),
                new_text,
            },
        ));
    }

    let report = validate::validate_document(&document, &ValidationOptions::default());
    if !report.is_ok() {
        return Err(TxError::Validation(report));
    }
    // [egress] 里出现凭据键 → 422 unknown_field（core 只报 warning，这里升级）
    let section_errors = crate::dto::egress_section_issues(&document);
    if !section_errors.is_empty() {
        return Err(TxError::ValidationErrors(section_errors));
    }
    let parsed = Config::from_toml(&new_text)
        .map_err(|error| TxError::InvalidCurrent(crate::util::parse_error_message(&error)))?;
    let limit_errors = crate::dto::limit_issues(&parsed);
    if !limit_errors.is_empty() {
        return Err(TxError::ValidationErrors(limit_errors));
    }
    if dry_run {
        return Ok((
            value,
            TxOutcome {
                previous_revision: exists.then(|| current_revision.clone()),
                revision: current_revision,
                changed: true,
                backup_id: None,
                old_text: text,
                new_text,
            },
        ));
    }

    Ok((
        value,
        commit(store, exists, &current_revision, text, new_text)?,
    ))
}

/// 整体替换文本（PUT /config）：不需要解析旧文档，也不会读取旧模型。
pub fn transact_text(
    store: &ConfigStore,
    precondition: &Precondition,
    dry_run: bool,
    new_text: String,
) -> Result<TxOutcome, TxError> {
    let lock_path = store.lock_path();
    let _lock = FileLock::acquire(&lock_path).map_err(StoreError::Io)?;
    let (exists, text) = read_text(store.path())?;
    let current_revision = revision::revision_of(&text);
    check_precondition(precondition, &current_revision, exists)?;

    let document = edit::parse_document(&new_text).map_err(|error| {
        TxError::Validation({
            let mut report = ValidationReport::new();
            report.error("config.parse", "config", format!("{error}"));
            report
        })
    })?;
    let report = validate::validate_document(&document, &ValidationOptions::default());
    if !report.is_ok() {
        return Err(TxError::Validation(report));
    }
    let section_errors = crate::dto::egress_section_issues(&document);
    if !section_errors.is_empty() {
        return Err(TxError::ValidationErrors(section_errors));
    }
    // core 把「同一 package 多条 [[apps]]」定为 warning；契约（API.md 6.3）要求 422
    // duplicate_app_record，因此整写路径在这里补一条硬错误。
    let parsed = Config::from_toml(&new_text)
        .map_err(|error| TxError::InvalidCurrent(crate::util::parse_error_message(&error)))?;
    let limit_errors = crate::dto::limit_issues(&parsed);
    if !limit_errors.is_empty() {
        return Err(TxError::ValidationErrors(limit_errors));
    }
    if let Ok(config) = Config::from_toml(&new_text) {
        let mut seen = std::collections::HashSet::new();
        for app in &config.apps {
            if !seen.insert(app.package.as_str()) {
                let mut report = ValidationReport::new();
                report.error(
                    "package.duplicate_app",
                    format!("apps.{}", app.package),
                    format!("包名 {} 有多条 [[apps]] 条目，只有第一条生效", app.package),
                );
                return Err(TxError::Validation(report));
            }
        }
    }

    if new_text == text {
        return Ok(TxOutcome {
            previous_revision: exists.then(|| current_revision.clone()),
            revision: current_revision,
            changed: false,
            backup_id: None,
            old_text: text.clone(),
            new_text,
        });
    }
    if dry_run {
        return Ok(TxOutcome {
            previous_revision: exists.then(|| current_revision.clone()),
            revision: current_revision,
            changed: true,
            backup_id: None,
            old_text: text,
            new_text,
        });
    }
    commit(store, exists, &current_revision, text, new_text)
}

fn commit(
    store: &ConfigStore,
    exists: bool,
    current_revision: &str,
    old_text: String,
    new_text: String,
) -> Result<TxOutcome, TxError> {
    let backup = if exists {
        revision::create_backup(
            store.path(),
            &store.backup_dir(),
            BACKUP_KEEP,
            Some(current_revision),
        )
        .map_err(StoreError::Io)?
    } else {
        None
    };
    let backup_id = backup.as_deref().and_then(backup_id_from);
    let new_revision = revision::revision_of(&new_text);
    revision::atomic_write(store.path(), new_text.as_bytes(), CONFIG_FILE_MODE)
        .map_err(StoreError::Io)?;
    Ok(TxOutcome {
        previous_revision: exists.then(|| current_revision.to_owned()),
        revision: new_revision,
        changed: true,
        backup_id,
        old_text,
        new_text,
    })
}

fn check_precondition(
    precondition: &Precondition,
    current_revision: &str,
    exists: bool,
) -> Result<(), TxError> {
    match precondition {
        Precondition::Any => Ok(()),
        Precondition::Exact(expected) if expected == current_revision => Ok(()),
        Precondition::Exact(expected) => Err(TxError::RevisionMismatch {
            expected: expected.clone(),
            actual: exists.then(|| current_revision.to_owned()),
        }),
    }
}

fn read_text(path: &Path) -> Result<(bool, String), TxError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok((true, text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((false, String::new())),
        Err(error) => Err(TxError::Store(StoreError::Io(error))),
    }
}

/// 备份文件名里的 ID：`config.toml.<stamp>-<rev8>.bak` → `<stamp>-<rev8>`。
fn backup_id_from(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let without_suffix = name.strip_suffix(".bak")?;
    let (_, id) = without_suffix.split_once('.')?;
    Some(id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_revision_strips_quotes_and_prefix() {
        let digest = "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5";
        assert_eq!(normalize_revision(digest), digest);
        assert_eq!(normalize_revision(&format!("\"{digest}\"")), digest);
        assert_eq!(normalize_revision(&format!("'sha256:{digest}'")), digest);
        assert_eq!(normalize_revision(&format!("sha256:{digest}")), digest);
    }

    #[test]
    fn if_match_parses_wildcard_and_values() {
        let mut headers = HeaderMap::new();
        assert_eq!(if_match(&headers), None);
        headers.insert(IF_MATCH, "*".parse().unwrap());
        assert_eq!(if_match(&headers), Some(Precondition::Any));
        headers.insert(IF_MATCH, "\"abc\"".parse().unwrap());
        assert_eq!(
            if_match(&headers),
            Some(Precondition::Exact("abc".to_owned()))
        );
    }
}
