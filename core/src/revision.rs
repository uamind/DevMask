// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! revision、原子写、flock 与备份轮转。
//!
//! 设备端约束（docs/dev/server-api-plan.md 8.3 / 9.2）：
//!
//! - 配置文件 `/data/adb/devmask/config/config.toml` 必须是 0644，
//!   否则 zygisk 模块与 companion 可能读不到；
//! - 写配置必须**原子**：先写同目录临时文件、`fsync`、`rename` 覆盖、再 `fsync` 目录，
//!   避免断电/并发读到半截 TOML；
//! - 写之前自动备份到 `config/backups/`，保留最近 10 份；
//! - 并发写用 `flock` 串行化，revision（sha256）用于 `If-Match` 乐观并发控制。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

use crate::edit::{self, EditError};

/// 自动备份默认保留份数。
pub const BACKUP_KEEP: usize = 10;
/// 配置文件权限：Android 运行时要求 0644。
pub const CONFIG_FILE_MODE: u32 = 0o644;
/// 备份目录权限。
pub const BACKUP_DIR_MODE: u32 = 0o700;
/// 备份文件权限（仅 root 可读）。
pub const BACKUP_FILE_MODE: u32 = 0o600;
/// 锁文件权限。
pub const LOCK_FILE_MODE: u32 = 0o600;
/// 临时文件后缀前缀（便于排障与清理）。
const TEMP_PREFIX: &str = ".devmask.tmp";
/// 备份文件后缀。
const BACKUP_SUFFIX: &str = ".bak";

/// 字节流的 sha256 十六进制小写表示。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 文本 revision（sha256，与 `sha256_hex(text.as_bytes())` 等价）。
pub fn revision_of(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

/// `Unix 时间戳 → `YYYYMMDDTHHMMSSZ`（UTC，秒级；备份文件名用）。
pub fn format_utc_compact(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds_of_day = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// 当前 UTC 时间戳的 `YYYYMMDDTHHMMSSZ` 形式。
pub fn now_utc_compact() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_utc_compact(seconds)
}

/// 备份名用的时间戳：秒 + 纳秒（`YYYYMMDDTHHMMSS.nnnnnnnnnZ`）。
///
/// 只用秒会在「同一秒内多次写入」时退化成按 revision 前缀排序，备份轮转就会删错文件；
/// 定宽纳秒让文件名的字典序严格等于写入顺序。
fn now_utc_nanos() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = format_utc_compact(now.as_secs());
    let nanos = now.subsec_nanos();
    format!(
        "{}.{nanos:09}Z",
        seconds.strip_suffix('Z').unwrap_or(&seconds)
    )
}

/// Howard Hinnant 的 civil_from_days：把 1970-01-01 起的天数换算成 (年, 月, 日)。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 在 Unix 上给文件设置权限；其他平台是空操作。
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// 进程内共享的 `flock` 守卫，drop 时解锁。
#[derive(Debug)]
pub struct FileLock {
    file: File,
}

impl FileLock {
    /// 取排他锁（写路径）。
    pub fn acquire(path: &Path) -> io::Result<Self> {
        Self::acquire_with(path, true)
    }

    /// 取共享锁（读路径）。
    pub fn acquire_shared(path: &Path) -> io::Result<Self> {
        Self::acquire_with(path, false)
    }

    fn acquire_with(path: &Path, exclusive: bool) -> io::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(LOCK_FILE_MODE);
        }
        let file = options.open(path)?;
        lock_file(&file, exclusive)?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
}

#[cfg(unix)]
fn lock_file(file: &File, exclusive: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let operation = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    loop {
        // SAFETY: fd 来自存活的 File，flock 只操作该 fd 的内核锁，不涉及指针。
        let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(not(unix))]
fn lock_file(_file: &File, _exclusive: bool) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn unlock_file(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: 同 lock_file。
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn unlock_file(_file: &File) -> io::Result<()> {
    Ok(())
}

/// 原子写：同目录临时文件 → fsync → chmod → rename → fsync 目录。
pub fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = temp_path_for(path);
    let result = (|| -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(LOCK_FILE_MODE);
        }
        let mut file = options.open(&temp_path)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);

        // umask 可能把 create 时的 0600 收紧，这里显式 chmod 到目标权限。
        set_mode(&temp_path, mode)?;
        fs::rename(&temp_path, path)?;
        sync_dir(parent);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// 目录 fsync：让 rename 的结果在掉电后仍然可见。尽力而为（部分文件系统不支持）。
fn sync_dir(directory: &Path) {
    if let Ok(file) = File::open(directory) {
        let _ = file.sync_all();
    }
}

fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    path.with_file_name(format!(
        "{TEMP_PREFIX}.{file_name}.{}.{nanos}",
        std::process::id()
    ))
}

/// 备份当前文件到 `directory`，返回备份路径（文件不存在时返回 `Ok(None)`）。
///
/// 备份名形如 `config.toml.20261124T031002Z-9f2c1a3b.bak`，按名字排序即时间顺序。
pub fn create_backup(
    path: &Path,
    directory: &Path,
    keep: usize,
    revision: Option<&str>,
) -> io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    fs::create_dir_all(directory)?;
    set_mode(directory, BACKUP_DIR_MODE)?;

    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let stamp = now_utc_nanos();
    let short_revision = revision
        .map(|value| &value[..value.len().min(8)])
        .unwrap_or("norev");

    let mut candidate = directory.join(format!(
        "{file_name}.{stamp}-{short_revision}{BACKUP_SUFFIX}"
    ));
    let mut counter = 1;
    while candidate.exists() {
        candidate = directory.join(format!(
            "{file_name}.{stamp}-{short_revision}-{counter}{BACKUP_SUFFIX}"
        ));
        counter += 1;
    }

    fs::copy(path, &candidate)?;
    set_mode(&candidate, BACKUP_FILE_MODE)?;
    prune_backups(directory, &file_name, keep)?;
    Ok(Some(candidate))
}

/// 只保留最近 `keep` 份备份（按文件名排序，新的在后），返回删除数量。
pub fn prune_backups(directory: &Path, file_name: &str, keep: usize) -> io::Result<usize> {
    let mut entries = list_backups(directory, file_name)?;
    entries.sort();
    if entries.len() <= keep {
        return Ok(0);
    }

    let mut removed = 0;
    for path in &entries[..entries.len() - keep] {
        fs::remove_file(path)?;
        removed += 1;
    }
    Ok(removed)
}

/// 列出某个配置文件对应的全部备份（未排序）。
pub fn list_backups(directory: &Path, file_name: &str) -> io::Result<Vec<PathBuf>> {
    let prefix = format!("{file_name}.");
    let mut backups = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(backups),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name.ends_with(BACKUP_SUFFIX) {
            backups.push(entry.path());
        }
    }
    Ok(backups)
}

/// `ConfigStore` 的可调参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreOptions {
    /// 备份保留份数（默认 [`BACKUP_KEEP`]）。
    pub keep_backups: usize,
    /// 配置文件权限（默认 [`CONFIG_FILE_MODE`]）。
    pub file_mode: u32,
    /// 备份目录；默认 `<配置文件目录>/backups`。
    pub backup_dir: Option<PathBuf>,
    /// 锁文件；默认 `<配置文件>.lock`。
    pub lock_path: Option<PathBuf>,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            keep_backups: BACKUP_KEEP,
            file_mode: CONFIG_FILE_MODE,
            backup_dir: None,
            lock_path: None,
        }
    }
}

/// 读取到的配置快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// 文件是否存在。
    pub exists: bool,
    /// 原始文本（不存在时为空串）。
    pub text: String,
    /// 文本 revision（sha256 小写十六进制）。
    pub revision: String,
}

/// 写入结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    /// 内容是否发生变化（未变化时不落盘、不产生备份）。
    pub changed: bool,
    /// 写入前 revision；文件原本不存在时为 `None`。
    pub previous_revision: Option<String>,
    /// 写入后 revision。
    pub revision: String,
    /// 本次产生的备份路径。
    pub backup: Option<PathBuf>,
}

/// 读-改-写结果（与 [`WriteOutcome`] 同形）。
pub type UpdateOutcome = WriteOutcome;

/// 存储层错误。
#[derive(Debug)]
pub enum StoreError {
    /// 文件系统错误。
    Io(io::Error),
    /// 文档编辑错误。
    Edit(EditError),
    /// `If-Match` 不匹配。
    RevisionMismatch {
        expected: String,
        actual: Option<String>,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "io error: {error}"),
            Self::Edit(error) => write!(formatter, "edit error: {error}"),
            Self::RevisionMismatch { expected, actual } => match actual {
                Some(actual) => write!(
                    formatter,
                    "revision mismatch: expected {expected}, actual {actual}"
                ),
                None => write!(
                    formatter,
                    "revision mismatch: expected {expected}, file missing"
                ),
            },
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Edit(error) => Some(error),
            Self::RevisionMismatch { .. } => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<EditError> for StoreError {
    fn from(error: EditError) -> Self {
        Self::Edit(error)
    }
}

/// `config.toml` 的读写入口：flock 串行化 + 原子写 + 自动备份 + revision。
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    options: StoreOptions,
}

impl ConfigStore {
    /// 使用默认参数打开（备份保留 10 份、权限 0644、备份到同目录 `backups/`）。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_options(path, StoreOptions::default())
    }

    /// 自定义参数打开。
    pub fn with_options(path: impl Into<PathBuf>, options: StoreOptions) -> Self {
        Self {
            path: path.into(),
            options,
        }
    }

    /// 配置文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 备份目录。
    pub fn backup_dir(&self) -> PathBuf {
        if let Some(dir) = &self.options.backup_dir {
            return dir.clone();
        }
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        parent.join("backups")
    }

    /// 锁文件路径。
    pub fn lock_path(&self) -> PathBuf {
        if let Some(path) = &self.options.lock_path {
            return path.clone();
        }
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config.toml".to_owned());
        self.path.with_file_name(format!("{file_name}.lock"))
    }

    /// 读取当前快照（共享锁，保证不与写路径交错）。
    pub fn read(&self) -> Result<Snapshot, StoreError> {
        let _lock = FileLock::acquire_shared(&self.lock_path())?;
        self.read_unlocked()
    }

    /// 读取全部备份（按名字排序，旧 → 新）。
    pub fn backups(&self) -> io::Result<Vec<PathBuf>> {
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config.toml".to_owned());
        let mut backups = list_backups(&self.backup_dir(), &file_name)?;
        backups.sort();
        Ok(backups)
    }

    /// 整体写入新文本；`expected_revision` 用于 `If-Match`（None 表示不做并发校验）。
    pub fn write(
        &self,
        text: &str,
        expected_revision: Option<&str>,
    ) -> Result<WriteOutcome, StoreError> {
        let _lock = FileLock::acquire(&self.lock_path())?;
        let current = self.read_unlocked()?;
        self.check_revision(&current, expected_revision)?;

        if current.exists && current.text == text {
            return Ok(WriteOutcome {
                changed: false,
                previous_revision: Some(current.revision.clone()),
                revision: current.revision,
                backup: None,
            });
        }

        self.commit(&current, text)
    }

    /// 在排他锁内读-改-写：`apply` 只拿到内存文档，落盘/备份/锁由本方法负责。
    pub fn update<F>(&self, apply: F) -> Result<UpdateOutcome, StoreError>
    where
        F: FnOnce(&mut DocumentMut) -> Result<(), EditError>,
    {
        let _lock = FileLock::acquire(&self.lock_path())?;
        let current = self.read_unlocked()?;

        let mut document = edit::parse_document(&current.text)?;
        apply(&mut document)?;
        let new_text = document.to_string();

        if new_text == current.text {
            return Ok(WriteOutcome {
                changed: false,
                previous_revision: current.exists.then(|| current.revision.clone()),
                revision: current.revision,
                backup: None,
            });
        }

        self.commit(&current, &new_text)
    }

    fn check_revision(
        &self,
        current: &Snapshot,
        expected_revision: Option<&str>,
    ) -> Result<(), StoreError> {
        let Some(expected) = expected_revision else {
            return Ok(());
        };
        let actual = current.exists.then_some(current.revision.as_str());
        if actual == Some(expected) {
            return Ok(());
        }
        Err(StoreError::RevisionMismatch {
            expected: expected.to_owned(),
            actual: actual.map(str::to_owned),
        })
    }

    fn read_unlocked(&self) -> Result<Snapshot, StoreError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(Snapshot {
                exists: true,
                revision: revision_of(&text),
                text,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Snapshot {
                exists: false,
                revision: revision_of(""),
                text: String::new(),
            }),
            Err(error) => Err(StoreError::Io(error)),
        }
    }

    fn commit(&self, current: &Snapshot, new_text: &str) -> Result<WriteOutcome, StoreError> {
        let backup = if current.exists {
            create_backup(
                &self.path,
                &self.backup_dir(),
                self.options.keep_backups,
                Some(&current.revision),
            )?
        } else {
            None
        };

        atomic_write(&self.path, new_text.as_bytes(), self.options.file_mode)?;

        Ok(WriteOutcome {
            changed: true,
            previous_revision: current.exists.then(|| current.revision.clone()),
            revision: revision_of(new_text),
            backup,
        })
    }
}
