// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! SQLite：单写连接 + WAL；schema v1（见 manager/docs/ARCH.md 第 5 节）。

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::Value;

use crate::ids;

/// 当前 schema 版本。
pub const SCHEMA_VERSION: i64 = 1;

/// schema v1 全量 DDL；迁移只追加、不修改历史。
pub const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
  version    INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS devices (
  id             TEXT PRIMARY KEY,
  name           TEXT NOT NULL,
  base_url       TEXT NOT NULL UNIQUE,
  token_cipher   TEXT NOT NULL,
  tags           TEXT NOT NULL DEFAULT '[]',
  online         INTEGER NOT NULL DEFAULT 0,
  model          TEXT,
  brand          TEXT,
  android        TEXT,
  sdk            INTEGER,
  fingerprint    TEXT,
  root_manager   TEXT,
  module_version TEXT,
  server_version TEXT,
  last_seen      TEXT,
  last_synced_at TEXT,
  latency_ms     INTEGER,
  last_error     TEXT,
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_devices_online  ON devices(online);
CREATE INDEX IF NOT EXISTS idx_devices_created ON devices(created_at);

CREATE TABLE IF NOT EXISTS device_overviews (
  device_id  TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
  revision   TEXT,
  fetched_at TEXT NOT NULL,
  payload    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs (
  id              TEXT PRIMARY KEY,
  kind            TEXT NOT NULL CHECK (kind IN ('bulk','assignments')),
  status          TEXT NOT NULL CHECK (status IN ('queued','running','succeeded','partial','failed')),
  app             TEXT,
  template        TEXT,
  restart         INTEGER NOT NULL DEFAULT 0,
  total           INTEGER NOT NULL,
  succeeded       INTEGER NOT NULL DEFAULT 0,
  skipped         INTEGER NOT NULL DEFAULT 0,
  failed          INTEGER NOT NULL DEFAULT 0,
  request_id      TEXT NOT NULL,
  idempotency_key TEXT UNIQUE,
  actor           TEXT NOT NULL DEFAULT 'manager-token',
  error           TEXT,
  created_at      TEXT NOT NULL,
  started_at      TEXT,
  finished_at     TEXT
);
CREATE INDEX IF NOT EXISTS idx_jobs_created ON jobs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_jobs_status  ON jobs(status);

CREATE TABLE IF NOT EXISTS job_results (
  job_id      TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  ordinal     INTEGER NOT NULL,
  device_id   TEXT NOT NULL,
  app         TEXT NOT NULL,
  status      TEXT NOT NULL CHECK (status IN ('pending','ok','skipped','failed')),
  code        TEXT,
  http_status INTEGER,
  revision    TEXT,
  attempts    INTEGER NOT NULL DEFAULT 1,
  duration_ms INTEGER,
  restart     TEXT,
  steps       TEXT NOT NULL DEFAULT '[]',
  warnings    TEXT NOT NULL DEFAULT '[]',
  error       TEXT,
  PRIMARY KEY (job_id, device_id, app)
);
CREATE INDEX IF NOT EXISTS idx_job_results_job ON job_results(job_id, ordinal);

CREATE TABLE IF NOT EXISTS audit (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  ts          TEXT NOT NULL,
  actor       TEXT,
  action      TEXT NOT NULL,
  target_type TEXT,
  target_id   TEXT,
  request_id  TEXT,
  status      TEXT NOT NULL DEFAULT 'ok',
  detail      TEXT
);
CREATE INDEX IF NOT EXISTS idx_audit_action ON audit(action);
CREATE INDEX IF NOT EXISTS idx_audit_ts     ON audit(id DESC);
"#;

/// 数据库错误。
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite 错误：{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("数据库目录不可写：{path}（{source}）")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("数据库连接锁已损坏")]
    Poisoned,
    #[error("数据库任务执行失败：{0}")]
    Join(#[from] tokio::task::JoinError),
}

/// 健康检查快照。
#[derive(Debug, Clone)]
pub struct DbHealth {
    pub schema_version: i64,
    pub devices_total: i64,
    pub devices_online: i64,
    pub jobs_queued: i64,
    pub jobs_running: i64,
}

/// 设备行。
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub token_cipher: String,
    pub tags: Vec<String>,
    pub online: bool,
    pub model: Option<String>,
    pub brand: Option<String>,
    pub android: Option<String>,
    pub sdk: Option<i64>,
    pub fingerprint: Option<String>,
    pub root_manager: Option<String>,
    pub module_version: Option<String>,
    pub server_version: Option<String>,
    pub last_seen: Option<String>,
    pub last_synced_at: Option<String>,
    pub latency_ms: Option<i64>,
    pub last_error: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

/// 缓存的设备 overview。
#[derive(Debug, Clone)]
pub struct StoredOverview {
    pub device_id: String,
    pub revision: Option<String>,
    pub fetched_at: String,
    pub payload: String,
}

/// 新 job（入队时写入）。
#[derive(Debug, Clone)]
pub struct NewJob {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub app: Option<String>,
    pub template: Option<String>,
    pub restart: bool,
    pub total: i64,
    pub request_id: String,
    pub idempotency_key: Option<String>,
    pub created_at: String,
}

/// 待执行的 assignment（job_results 中的 pending 行）。
#[derive(Debug, Clone)]
pub struct PendingAssignment {
    pub ordinal: i64,
    pub device_id: String,
    pub app: String,
}

/// 一次 assignment 的最终结果。
#[derive(Debug, Clone)]
pub struct JobResultUpdate {
    pub job_id: String,
    pub device_id: String,
    pub app: String,
    pub status: String,
    pub code: Option<String>,
    pub http_status: Option<i64>,
    pub revision: Option<String>,
    pub attempts: i64,
    pub duration_ms: i64,
    pub restart: Option<Value>,
    pub steps: Vec<Value>,
    pub warnings: Vec<Value>,
    pub error: Option<Value>,
}

/// jobs 行。
#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub app: Option<String>,
    pub template: Option<String>,
    pub restart: bool,
    pub total: i64,
    pub succeeded: i64,
    pub skipped: i64,
    pub failed: i64,
    pub request_id: String,
    pub idempotency_key: Option<String>,
    pub error: Option<Value>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// job_results 行（含设备名快照）。
#[derive(Debug, Clone)]
pub struct JobResultRow {
    pub ordinal: i64,
    pub device_id: String,
    pub device_name: Option<String>,
    pub app: String,
    pub status: String,
    pub code: Option<String>,
    pub http_status: Option<i64>,
    pub revision: Option<String>,
    pub attempts: i64,
    pub duration_ms: Option<i64>,
    pub restart: Option<Value>,
    pub steps: Vec<Value>,
    pub warnings: Vec<Value>,
    pub error: Option<Value>,
}

/// 审计行。
#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: i64,
    pub ts: String,
    pub actor: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub request_id: Option<String>,
    pub status: String,
    pub detail: Option<Value>,
}

/// 审计查询条件。
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub actions: Vec<String>,
    pub target: Option<String>,
    pub since: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

/// job 计数（总览用）。
#[derive(Debug, Clone, Copy, Default)]
pub struct JobStats {
    pub queued: i64,
    pub running: i64,
    pub last24h: i64,
    pub failed_last24h: i64,
}

/// job 列表过滤。
#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    pub statuses: Vec<String>,
    pub kind: Option<String>,
    pub app: Option<String>,
    pub device: Option<String>,
}

/// job 进度。
#[derive(Debug, Clone, Copy)]
pub struct JobProgress {
    pub total: i64,
    pub succeeded: i64,
    pub skipped: i64,
    pub failed: i64,
    pub pending: i64,
}

/// 插入设备行。
const INSERT_DEVICE_SQL: &str = "INSERT INTO devices (
    id, name, base_url, token_cipher, tags, online,
    model, brand, android, sdk, fingerprint, root_manager,
    module_version, server_version, last_seen, last_synced_at,
    latency_ms, last_error, created_at, updated_at
 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
           ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)";

/// 写入 / 覆盖设备 overview 缓存。
const UPSERT_OVERVIEW_SQL: &str =
    "INSERT INTO device_overviews (device_id, revision, fetched_at, payload)
 VALUES (?1, ?2, ?3, ?4)
 ON CONFLICT(device_id) DO UPDATE SET
    revision = excluded.revision,
    fetched_at = excluded.fetched_at,
    payload = excluded.payload";

/// 一次成功探测 / 同步后要回写的设备字段。
#[derive(Debug, Clone)]
pub struct DeviceProbeUpdate {
    pub id: String,
    pub online: bool,
    pub model: Option<String>,
    pub brand: Option<String>,
    pub android: Option<String>,
    pub sdk: Option<i64>,
    pub fingerprint: Option<String>,
    pub root_manager: Option<String>,
    pub module_version: Option<String>,
    pub server_version: Option<String>,
    pub last_seen: String,
    pub last_synced_at: String,
    pub latency_ms: Option<i64>,
    pub updated_at: String,
}

/// SQLite 访问层；克隆成本为 Arc 计数。
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl Db {
    /// 打开（必要时创建）数据库并执行迁移。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DbError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let connection = Connection::open(&path)?;
        let db = Self {
            conn: Arc::new(Mutex::new(connection)),
            path,
        };
        db.configure()?;
        db.migrate()?;
        Ok(db)
    }

    /// 仅用于单元测试的内存库。
    pub fn open_in_memory() -> Result<Self, DbError> {
        let connection = Connection::open_in_memory()?;
        let db = Self {
            conn: Arc::new(Mutex::new(connection)),
            path: PathBuf::from(":memory:"),
        };
        db.configure()?;
        db.migrate()?;
        Ok(db)
    }

    fn configure(&self) -> Result<(), DbError> {
        let connection = self.conn.lock().map_err(|_| DbError::Poisoned)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;
        Ok(())
    }

    fn migrate(&self) -> Result<(), DbError> {
        let mut connection = self.conn.lock().map_err(|_| DbError::Poisoned)?;
        connection.execute_batch(DDL)?;

        let current: i64 = connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;

        if current < SCHEMA_VERSION {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT OR REPLACE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![SCHEMA_VERSION, ids::now_rfc3339()],
            )?;
            transaction.commit()?;
            connection.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }
        Ok(())
    }

    /// 数据库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 健康检查里展示的文件名（不泄露绝对路径）。
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "manager.db".to_string())
    }

    /// 在阻塞线程池中执行数据库操作，避免占用 async 执行器。
    pub async fn call<T, F>(&self, operation: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let connection = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = connection.lock().map_err(|_| DbError::Poisoned)?;
            operation(&mut guard).map_err(DbError::Sqlite)
        })
        .await?
    }

    /// 健康检查：SELECT 1、schema 版本与设备 / job 计数。
    pub async fn health(&self) -> Result<DbHealth, DbError> {
        self.call(|connection| {
            connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?;
            let schema_version = connection.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )?;
            let devices_total =
                connection.query_row("SELECT COUNT(*) FROM devices", [], |row| row.get(0))?;
            let devices_online = connection.query_row(
                "SELECT COUNT(*) FROM devices WHERE online = 1",
                [],
                |row| row.get(0),
            )?;
            let jobs_queued = connection.query_row(
                "SELECT COUNT(*) FROM jobs WHERE status = 'queued'",
                [],
                |row| row.get(0),
            )?;
            let jobs_running = connection.query_row(
                "SELECT COUNT(*) FROM jobs WHERE status = 'running'",
                [],
                |row| row.get(0),
            )?;
            Ok(DbHealth {
                schema_version,
                devices_total,
                devices_online,
                jobs_queued,
                jobs_running,
            })
        })
        .await
    }

    /// 插入设备行。
    pub async fn insert_device(&self, device: DeviceRecord) -> Result<(), DbError> {
        self.call(move |connection| insert_device_row(connection, &device))
            .await
    }

    /// 同一事务内写入设备行与 overview 缓存（添加设备使用）。
    pub async fn insert_device_with_overview(
        &self,
        device: DeviceRecord,
        overview: StoredOverview,
    ) -> Result<(), DbError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            insert_device_row(&transaction, &device)?;
            upsert_overview_row(&transaction, &overview)?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    /// 全部设备，按 created_at 升序、id 升序。
    pub async fn list_devices(&self) -> Result<Vec<DeviceRecord>, DbError> {
        self.call(|connection| {
            let mut statement =
                connection.prepare("SELECT * FROM devices ORDER BY created_at ASC, id ASC")?;
            let rows = statement
                .query_map([], row_to_device)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// 单台设备。
    pub async fn get_device(&self, device_id: &str) -> Result<Option<DeviceRecord>, DbError> {
        let device_id = device_id.to_string();
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT * FROM devices WHERE id = ?1",
                    params![device_id],
                    row_to_device,
                )
                .optional()
        })
        .await
    }

    /// 删除设备；返回是否删到行。
    pub async fn delete_device(&self, device_id: &str) -> Result<bool, DbError> {
        let device_id = device_id.to_string();
        self.call(move |connection| {
            let affected =
                connection.execute("DELETE FROM devices WHERE id = ?1", params![device_id])?;
            Ok(affected > 0)
        })
        .await
    }

    /// 写入 / 覆盖设备 overview 缓存。
    pub async fn upsert_overview(&self, overview: StoredOverview) -> Result<(), DbError> {
        self.call(move |connection| upsert_overview_row(connection, &overview))
            .await
    }

    /// 读取设备 overview 缓存。
    pub async fn get_overview(&self, device_id: &str) -> Result<Option<StoredOverview>, DbError> {
        let device_id = device_id.to_string();
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT device_id, revision, fetched_at, payload
                       FROM device_overviews WHERE device_id = ?1",
                    params![device_id],
                    |row| {
                        Ok(StoredOverview {
                            device_id: row.get(0)?,
                            revision: row.get(1)?,
                            fetched_at: row.get(2)?,
                            payload: row.get(3)?,
                        })
                    },
                )
                .optional()
        })
        .await
    }

    /// 按 baseUrl 查重（传入已归一化的地址）。
    pub async fn get_device_by_base_url(
        &self,
        base_url: &str,
    ) -> Result<Option<DeviceRecord>, DbError> {
        let base_url = base_url.to_string();
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT * FROM devices WHERE base_url = ?1",
                    params![base_url],
                    row_to_device,
                )
                .optional()
        })
        .await
    }

    /// 全部 overview 缓存（apps 矩阵 / 总览使用）。
    pub async fn list_overviews(&self) -> Result<Vec<StoredOverview>, DbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT device_id, revision, fetched_at, payload
                   FROM device_overviews ORDER BY device_id ASC",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok(StoredOverview {
                        device_id: row.get(0)?,
                        revision: row.get(1)?,
                        fetched_at: row.get(2)?,
                        payload: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// 探测 / 同步成功后回写设备元数据与在线状态（并清空 last_error）。
    pub async fn update_device_probe(&self, update: DeviceProbeUpdate) -> Result<usize, DbError> {
        self.call(move |connection| {
            connection.execute(
                "UPDATE devices SET
                    online = ?2, model = ?3, brand = ?4, android = ?5, sdk = ?6,
                    fingerprint = ?7, root_manager = ?8, module_version = ?9,
                    server_version = ?10, last_seen = ?11, last_synced_at = ?12,
                    latency_ms = ?13, last_error = NULL, updated_at = ?14
                  WHERE id = ?1",
                params![
                    update.id,
                    update.online as i64,
                    update.model,
                    update.brand,
                    update.android,
                    update.sdk,
                    update.fingerprint,
                    update.root_manager,
                    update.module_version,
                    update.server_version,
                    update.last_seen,
                    update.last_synced_at,
                    update.latency_ms,
                    update.updated_at,
                ],
            )
        })
        .await
    }

    /// 标记设备离线并记录最近错误（保留上次成功时间与缓存）。
    pub async fn mark_device_offline(
        &self,
        device_id: &str,
        error: Value,
        latency_ms: Option<i64>,
        updated_at: &str,
    ) -> Result<usize, DbError> {
        let device_id = device_id.to_string();
        let error = error.to_string();
        let updated_at = updated_at.to_string();
        self.call(move |connection| {
            connection.execute(
                "UPDATE devices SET
                    online = 0, last_error = ?2,
                    latency_ms = COALESCE(?3, latency_ms), updated_at = ?4
                  WHERE id = ?1",
                params![device_id, error, latency_ms, updated_at],
            )
        })
        .await
    }

    /// 更新名称与标签（PATCH 使用）。
    pub async fn update_device_profile(
        &self,
        device_id: &str,
        name: &str,
        tags: &[String],
        updated_at: &str,
    ) -> Result<usize, DbError> {
        let device_id = device_id.to_string();
        let name = name.to_string();
        let tags = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());
        let updated_at = updated_at.to_string();
        self.call(move |connection| {
            connection.execute(
                "UPDATE devices SET name = ?2, tags = ?3, updated_at = ?4 WHERE id = ?1",
                params![device_id, name, tags, updated_at],
            )
        })
        .await
    }

    /// 更新 baseUrl 与设备 Token 密文（PATCH 使用）。
    pub async fn update_device_connection(
        &self,
        device_id: &str,
        base_url: &str,
        token_cipher: &str,
        updated_at: &str,
    ) -> Result<usize, DbError> {
        let device_id = device_id.to_string();
        let base_url = base_url.to_string();
        let token_cipher = token_cipher.to_string();
        let updated_at = updated_at.to_string();
        self.call(move |connection| {
            connection.execute(
                "UPDATE devices SET base_url = ?2, token_cipher = ?3, updated_at = ?4
                  WHERE id = ?1",
                params![device_id, base_url, token_cipher, updated_at],
            )
        })
        .await
    }

    /// 写入 job（入队）。
    pub async fn insert_job(&self, job: NewJob) -> Result<(), DbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO jobs (
                    id, kind, status, app, template, restart, total,
                    request_id, idempotency_key, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    job.id,
                    job.kind,
                    job.status,
                    job.app,
                    job.template,
                    job.restart as i64,
                    job.total,
                    job.request_id,
                    job.idempotency_key,
                    job.created_at,
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// 同一事务写入 job 与全部 pending assignment。
    pub async fn insert_job_with_assignments(
        &self,
        job: NewJob,
        assignments: Vec<PendingAssignment>,
    ) -> Result<(), DbError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO jobs (
                    id, kind, status, app, template, restart, total,
                    request_id, idempotency_key, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    job.id,
                    job.kind,
                    job.status,
                    job.app,
                    job.template,
                    job.restart as i64,
                    job.total,
                    job.request_id,
                    job.idempotency_key,
                    job.created_at,
                ],
            )?;
            for assignment in &assignments {
                transaction.execute(
                    "INSERT INTO job_results (job_id, ordinal, device_id, app, status)
                     VALUES (?1, ?2, ?3, ?4, 'pending')",
                    params![
                        &job.id,
                        assignment.ordinal,
                        &assignment.device_id,
                        &assignment.app
                    ],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    /// 把 queued job 置为 running；已被其它执行者启动时返回 false。
    pub async fn start_job(&self, job_id: &str) -> Result<bool, DbError> {
        let job_id = job_id.to_string();
        let started_at = ids::now_rfc3339();
        self.call(move |connection| {
            let affected = connection.execute(
                "UPDATE jobs SET status = 'running', started_at = ?2
                  WHERE id = ?1 AND status = 'queued'",
                params![job_id, started_at],
            )?;
            Ok(affected > 0)
        })
        .await
    }

    /// 尚未执行的 assignment，按 ordinal 升序。
    pub async fn pending_assignments(
        &self,
        job_id: &str,
    ) -> Result<Vec<PendingAssignment>, DbError> {
        let job_id = job_id.to_string();
        self.call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT ordinal, device_id, app FROM job_results
                  WHERE job_id = ?1 AND status = 'pending' ORDER BY ordinal ASC",
            )?;
            let rows = statement
                .query_map(params![job_id], |row| {
                    Ok(PendingAssignment {
                        ordinal: row.get(0)?,
                        device_id: row.get(1)?,
                        app: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// 写入一台设备的最终结果并累加 job 计数（同一事务）。
    pub async fn write_job_result(&self, update: JobResultUpdate) -> Result<(), DbError> {
        let steps = serde_json::to_string(&update.steps).unwrap_or_else(|_| "[]".to_string());
        let warnings = serde_json::to_string(&update.warnings).unwrap_or_else(|_| "[]".to_string());
        let restart = update.restart.as_ref().map(Value::to_string);
        let error = update.error.as_ref().map(Value::to_string);
        self.call(move |connection| {
            let counter = match update.status.as_str() {
                "ok" => "succeeded",
                "skipped" => "skipped",
                "failed" => "failed",
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            let transaction = connection.transaction()?;
            let affected = transaction.execute(
                "UPDATE job_results SET
                    status = ?3, code = ?4, http_status = ?5, revision = ?6,
                    attempts = ?7, duration_ms = ?8, restart = ?9, steps = ?10,
                    warnings = ?11, error = ?12
                  WHERE job_id = ?1 AND device_id = ?2 AND app = ?13 AND status = 'pending'",
                params![
                    update.job_id,
                    update.device_id,
                    update.status,
                    update.code,
                    update.http_status,
                    update.revision,
                    update.attempts,
                    update.duration_ms,
                    restart,
                    steps,
                    warnings,
                    error,
                    update.app,
                ],
            )?;
            if affected == 1 {
                transaction.execute(
                    &format!("UPDATE jobs SET {counter} = {counter} + 1 WHERE id = ?1"),
                    params![update.job_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    /// job 进度（含 pending 数）。
    pub async fn job_progress(&self, job_id: &str) -> Result<Option<JobProgress>, DbError> {
        let job_id = job_id.to_string();
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT j.total, j.succeeded, j.skipped, j.failed,
                            (SELECT COUNT(*) FROM job_results r
                              WHERE r.job_id = j.id AND r.status = 'pending')
                       FROM jobs j WHERE j.id = ?1",
                    params![job_id],
                    |row| {
                        Ok(JobProgress {
                            total: row.get(0)?,
                            succeeded: row.get(1)?,
                            skipped: row.get(2)?,
                            failed: row.get(3)?,
                            pending: row.get(4)?,
                        })
                    },
                )
                .optional()
        })
        .await
    }

    /// 依据计数与 pending 数计算终态并写 finished_at；返回最终状态。
    pub async fn finalize_job(
        &self,
        job_id: &str,
        finished_at: &str,
    ) -> Result<Option<String>, DbError> {
        let job_id = job_id.to_string();
        let finished_at = finished_at.to_string();
        self.call(move |connection| {
            connection.execute(
                "UPDATE jobs SET
                    status = CASE
                        WHEN succeeded + skipped + failed < total THEN 'running'
                        WHEN succeeded = total THEN 'succeeded'
                        WHEN succeeded = 0 AND failed > 0 THEN 'failed'
                        ELSE 'partial'
                    END,
                    finished_at = CASE
                        WHEN succeeded + skipped + failed >= total THEN ?2
                        ELSE finished_at
                    END
                  WHERE id = ?1",
                params![job_id, finished_at],
            )?;
            connection
                .query_row(
                    "SELECT status FROM jobs WHERE id = ?1",
                    params![job_id],
                    |row| row.get(0),
                )
                .optional()
        })
        .await
    }

    /// 按 id 查 job。
    pub async fn get_job(&self, job_id: &str) -> Result<Option<JobRow>, DbError> {
        let job_id = job_id.to_string();
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT * FROM jobs WHERE id = ?1",
                    params![job_id],
                    row_to_job,
                )
                .optional()
        })
        .await
    }

    /// 幂等键命中查询（不受 TTL 限制：旧键重放返回原 job，绝不落库冲突）。
    pub async fn find_job_by_idempotency(&self, key: &str) -> Result<Option<JobRow>, DbError> {
        let key = key.to_string();
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT * FROM jobs WHERE idempotency_key = ?1
                      ORDER BY created_at DESC LIMIT 1",
                    params![key],
                    row_to_job,
                )
                .optional()
        })
        .await
    }

    /// 审计查询：action 列表 / target / since + 分页。
    pub async fn query_audit(&self, filter: AuditFilter) -> Result<(Vec<AuditRow>, i64), DbError> {
        self.call(move |connection| {
            let mut clauses: Vec<String> = Vec::new();
            let mut values: Vec<Box<dyn rusqlite::ToSql + Send>> = Vec::new();

            if !filter.actions.is_empty() {
                let placeholders = (1..=filter.actions.len())
                    .map(|index| format!("?{index}"))
                    .collect::<Vec<_>>()
                    .join(",");
                clauses.push(format!("action IN ({placeholders})"));
                for action in &filter.actions {
                    values.push(Box::new(action.clone()));
                }
            }
            if let Some(target) = &filter.target {
                if let Some((target_type, target_id)) = target.split_once(':') {
                    clauses.push("target_type = ?".to_string());
                    values.push(Box::new(target_type.to_string()));
                    clauses.push("target_id = ?".to_string());
                    values.push(Box::new(target_id.to_string()));
                } else {
                    clauses.push("target_id = ?".to_string());
                    values.push(Box::new(target.clone()));
                }
            }
            if let Some(since) = &filter.since {
                clauses.push("ts >= ?".to_string());
                values.push(Box::new(since.clone()));
            }

            let where_sql = if clauses.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", clauses.join(" AND "))
            };

            let total: i64 = connection.query_row(
                &format!("SELECT COUNT(*) FROM audit{where_sql}"),
                rusqlite::params_from_iter(values.iter().map(|value| value.as_ref())),
                |row| row.get(0),
            )?;

            let mut statement = connection.prepare(&format!(
                "SELECT id, ts, actor, action, target_type, target_id, request_id, status, detail
                   FROM audit{where_sql}
                  ORDER BY id DESC
                  LIMIT {} OFFSET {}",
                filter.limit.max(1),
                filter.offset
            ))?;
            let rows = statement
                .query_map(
                    rusqlite::params_from_iter(values.iter().map(|value| value.as_ref())),
                    row_to_audit,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((rows, total))
        })
        .await
    }

    /// 总览用的 job 计数（含 24h 窗口）。
    pub async fn job_stats(&self) -> Result<JobStats, DbError> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::hours(24))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.call(move |connection| {
            connection.query_row(
                "SELECT
                    COALESCE(SUM(CASE WHEN status = 'queued' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN created_at >= ?1 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'failed' AND created_at >= ?1 THEN 1 ELSE 0 END), 0)
                   FROM jobs",
                params![cutoff],
                |row| {
                    Ok(JobStats {
                        queued: row.get(0)?,
                        running: row.get(1)?,
                        last24h: row.get(2)?,
                        failed_last24h: row.get(3)?,
                    })
                },
            )
        })
        .await
    }

    /// 最近 N 个 job（总览用），按创建时间倒序。
    pub async fn recent_jobs(&self, limit: usize) -> Result<Vec<JobRow>, DbError> {
        self.call(move |connection| {
            let mut statement = connection
                .prepare("SELECT * FROM jobs ORDER BY created_at DESC, id DESC LIMIT ?1")?;
            let rows = statement
                .query_map(params![limit as i64], row_to_job)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// job 列表：status 列表 / app / device 过滤 + 分页，按创建时间倒序。
    pub async fn list_jobs(
        &self,
        filter: JobFilter,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<JobRow>, i64), DbError> {
        self.call(move |connection| {
            let mut clauses: Vec<String> = Vec::new();
            let mut values: Vec<Box<dyn rusqlite::ToSql + Send>> = Vec::new();

            if !filter.statuses.is_empty() {
                let placeholders = (1..=filter.statuses.len())
                    .map(|index| format!("?{index}"))
                    .collect::<Vec<_>>()
                    .join(",");
                clauses.push(format!("j.status IN ({placeholders})"));
                for status in &filter.statuses {
                    values.push(Box::new(status.clone()));
                }
            }
            if let Some(kind) = &filter.kind {
                clauses.push("j.kind = ?".to_string());
                values.push(Box::new(kind.clone()));
            }
            if let Some(app) = &filter.app {
                clauses.push("j.app = ?".to_string());
                values.push(Box::new(app.clone()));
            }
            if let Some(device) = &filter.device {
                clauses.push(
                    "EXISTS (SELECT 1 FROM job_results r WHERE r.job_id = j.id AND r.device_id = ?)"
                        .to_string(),
                );
                values.push(Box::new(device.clone()));
            }

            let where_sql = if clauses.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", clauses.join(" AND "))
            };

            let total: i64 = connection.query_row(
                &format!("SELECT COUNT(*) FROM jobs j{where_sql}"),
                rusqlite::params_from_iter(values.iter().map(|value| value.as_ref())),
                |row| row.get(0),
            )?;

            let mut statement = connection.prepare(&format!(
                "SELECT j.* FROM jobs j{where_sql}
                  ORDER BY j.created_at DESC, j.id DESC
                  LIMIT {} OFFSET {}",
                limit.max(1),
                offset
            ))?;
            let rows = statement
                .query_map(
                    rusqlite::params_from_iter(values.iter().map(|value| value.as_ref())),
                    row_to_job,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((rows, total))
        })
        .await
    }

    /// 已完成的逐设备结果（忽略 pending），按 assignment 顺序。
    pub async fn job_results(&self, job_id: &str) -> Result<Vec<JobResultRow>, DbError> {
        let job_id = job_id.to_string();
        self.call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT r.ordinal, r.device_id, d.name AS device_name, r.app, r.status,
                        r.code, r.http_status, r.revision, r.attempts, r.duration_ms,
                        r.restart, r.steps, r.warnings, r.error
                   FROM job_results r
                   LEFT JOIN devices d ON d.id = r.device_id
                  WHERE r.job_id = ?1 AND r.status <> 'pending'
                  ORDER BY r.ordinal ASC",
            )?;
            let rows = statement
                .query_map(params![job_id], |row| {
                    Ok(JobResultRow {
                        ordinal: row.get(0)?,
                        device_id: row.get(1)?,
                        device_name: row.get(2)?,
                        app: row.get(3)?,
                        status: row.get(4)?,
                        code: row.get(5)?,
                        http_status: row.get(6)?,
                        revision: row.get(7)?,
                        attempts: row.get(8)?,
                        duration_ms: row.get(9)?,
                        restart: decode_json(row.get(10)?),
                        steps: decode_json(row.get(11)?).unwrap_or_default(),
                        warnings: decode_json(row.get(12)?).unwrap_or_default(),
                        error: decode_json(row.get(13)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// 启动时把遗留的 queued / running job 标记为失败（interrupted）。
    pub async fn recover_interrupted_jobs(&self) -> Result<usize, DbError> {
        let finished_at = ids::now_rfc3339();
        let error = serde_json::json!({
            "code": "interrupted",
            "message": "Manager 重启，任务已中断",
        })
        .to_string();
        self.call(move |connection| {
            let affected = connection.execute(
                "UPDATE jobs SET status = 'failed', error = ?1, finished_at = ?2
                  WHERE status IN ('queued', 'running')",
                params![error, finished_at],
            )?;
            Ok(affected)
        })
        .await
    }

    /// 清理过期 job（7 天 / 200 条）与审计（5000 条）。
    pub async fn prune(&self) -> Result<(usize, usize), DbError> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(7))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.call(move |connection| {
            let mut jobs_deleted =
                connection.execute("DELETE FROM jobs WHERE created_at < ?1", params![cutoff])?;
            jobs_deleted += connection.execute(
                "DELETE FROM jobs WHERE id NOT IN (
                    SELECT id FROM jobs ORDER BY created_at DESC, id DESC LIMIT 200
                 )",
                [],
            )?;
            let audits_deleted = connection.execute(
                "DELETE FROM audit WHERE id NOT IN (
                    SELECT id FROM audit ORDER BY id DESC LIMIT 5000
                 )",
                [],
            )?;
            Ok((jobs_deleted, audits_deleted))
        })
        .await
    }

    /// 追加审计行；失败由调用方决定是否忽略。
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_audit(
        &self,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        request_id: Option<&str>,
        status: &str,
        detail: Option<Value>,
        ts: &str,
    ) -> Result<(), DbError> {
        let action = action.to_string();
        let target_type = target_type.map(str::to_owned);
        let target_id = target_id.map(str::to_owned);
        let request_id = request_id.map(str::to_owned);
        let status = status.to_string();
        let detail = detail.map(|value| value.to_string());
        let ts = ts.to_string();
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO audit (ts, actor, action, target_type, target_id, request_id, status, detail)
                 VALUES (?1, 'manager-token', ?2, ?3, ?4, ?5, ?6, ?7)",
                params![ts, action, target_type, target_id, request_id, status, detail],
            )?;
            Ok(())
        })
        .await
    }
}

fn row_to_audit(row: &Row<'_>) -> rusqlite::Result<AuditRow> {
    Ok(AuditRow {
        id: row.get("id")?,
        ts: row.get("ts")?,
        actor: row.get("actor")?,
        action: row.get("action")?,
        target_type: row.get("target_type")?,
        target_id: row.get("target_id")?,
        request_id: row.get("request_id")?,
        status: row.get("status")?,
        detail: decode_json(row.get("detail")?),
    })
}

fn row_to_job(row: &Row<'_>) -> rusqlite::Result<JobRow> {
    let restart: i64 = row.get("restart")?;
    Ok(JobRow {
        id: row.get("id")?,
        kind: row.get("kind")?,
        status: row.get("status")?,
        app: row.get("app")?,
        template: row.get("template")?,
        restart: restart != 0,
        total: row.get("total")?,
        succeeded: row.get("succeeded")?,
        skipped: row.get("skipped")?,
        failed: row.get("failed")?,
        request_id: row.get("request_id")?,
        idempotency_key: row.get("idempotency_key")?,
        error: decode_json(row.get("error")?),
        created_at: row.get("created_at")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
    })
}

fn decode_json<T: serde::de::DeserializeOwned>(raw: Option<String>) -> Option<T> {
    raw.and_then(|text| serde_json::from_str(&text).ok())
}

fn insert_device_row(connection: &Connection, device: &DeviceRecord) -> rusqlite::Result<()> {
    let tags = serde_json::to_string(&device.tags).unwrap_or_else(|_| "[]".to_string());
    let last_error = device.last_error.as_ref().map(Value::to_string);
    connection.execute(
        INSERT_DEVICE_SQL,
        params![
            &device.id,
            &device.name,
            &device.base_url,
            &device.token_cipher,
            tags,
            device.online as i64,
            device.model.as_deref(),
            device.brand.as_deref(),
            device.android.as_deref(),
            device.sdk,
            device.fingerprint.as_deref(),
            device.root_manager.as_deref(),
            device.module_version.as_deref(),
            device.server_version.as_deref(),
            device.last_seen.as_deref(),
            device.last_synced_at.as_deref(),
            device.latency_ms,
            last_error,
            &device.created_at,
            &device.updated_at,
        ],
    )?;
    Ok(())
}

fn upsert_overview_row(connection: &Connection, overview: &StoredOverview) -> rusqlite::Result<()> {
    connection.execute(
        UPSERT_OVERVIEW_SQL,
        params![
            &overview.device_id,
            overview.revision.as_deref(),
            &overview.fetched_at,
            &overview.payload,
        ],
    )?;
    Ok(())
}

fn row_to_device(row: &Row<'_>) -> rusqlite::Result<DeviceRecord> {
    let tags_json: String = row.get("tags")?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    let last_error_json: Option<String> = row.get("last_error")?;
    let last_error = last_error_json.and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    let online: i64 = row.get("online")?;

    Ok(DeviceRecord {
        id: row.get("id")?,
        name: row.get("name")?,
        base_url: row.get("base_url")?,
        token_cipher: row.get("token_cipher")?,
        tags,
        online: online != 0,
        model: row.get("model")?,
        brand: row.get("brand")?,
        android: row.get("android")?,
        sdk: row.get("sdk")?,
        fingerprint: row.get("fingerprint")?,
        root_manager: row.get("root_manager")?,
        module_version: row.get("module_version")?,
        server_version: row.get("server_version")?,
        last_seen: row.get("last_seen")?,
        last_synced_at: row.get("last_synced_at")?,
        latency_ms: row.get("latency_ms")?,
        last_error,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_device(id: &str) -> DeviceRecord {
        DeviceRecord {
            id: id.to_string(),
            name: format!("设备 {id}"),
            base_url: format!("http://127.0.0.1:8686/{id}"),
            token_cipher: "v1:test".to_string(),
            tags: vec!["测试".to_string()],
            online: true,
            model: Some("Pixel 9".to_string()),
            brand: Some("google".to_string()),
            android: Some("17".to_string()),
            sdk: Some(37),
            fingerprint: Some("google/tokay/tokay:17".to_string()),
            root_manager: Some("kernelsu".to_string()),
            module_version: Some("v1.9.0".to_string()),
            server_version: Some("1.9.0".to_string()),
            last_seen: Some(ids::now_rfc3339()),
            last_synced_at: Some(ids::now_rfc3339()),
            latency_ms: Some(21),
            last_error: None,
            created_at: ids::now_rfc3339(),
            updated_at: ids::now_rfc3339(),
        }
    }

    #[tokio::test]
    async fn migrates_and_reports_health() {
        let db = Db::open_in_memory().unwrap();
        let health = db.health().await.unwrap();
        assert_eq!(health.schema_version, SCHEMA_VERSION);
        assert_eq!(health.devices_total, 0);
        assert_eq!(health.devices_online, 0);
    }

    #[tokio::test]
    async fn device_crud_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.insert_device(sample_device("pixel-9")).await.unwrap();
        db.insert_device(sample_device("xiaomi-15")).await.unwrap();

        let devices = db.list_devices().await.unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].tags, vec!["测试".to_string()]);
        assert!(devices[0].online);

        let found = db.get_device("pixel-9").await.unwrap().unwrap();
        assert_eq!(found.model.as_deref(), Some("Pixel 9"));

        assert!(db.delete_device("pixel-9").await.unwrap());
        assert!(db.get_device("pixel-9").await.unwrap().is_none());
        assert!(!db.delete_device("pixel-9").await.unwrap());
    }

    #[tokio::test]
    async fn overview_cache_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.insert_device(sample_device("pixel-9")).await.unwrap();
        db.upsert_overview(StoredOverview {
            device_id: "pixel-9".to_string(),
            revision: Some("abc".to_string()),
            fetched_at: ids::now_rfc3339(),
            payload: "{\"revision\":\"abc\"}".to_string(),
        })
        .await
        .unwrap();

        let stored = db.get_overview("pixel-9").await.unwrap().unwrap();
        assert_eq!(stored.revision.as_deref(), Some("abc"));
        assert!(stored.payload.contains("abc"));
    }

    #[tokio::test]
    async fn recovers_interrupted_jobs() {
        let db = Db::open_in_memory().unwrap();
        db.insert_job(NewJob {
            id: "job_test".to_string(),
            kind: "bulk".to_string(),
            status: "running".to_string(),
            app: Some("com.xingin.xhs".to_string()),
            template: Some("pixel_9".to_string()),
            restart: false,
            total: 1,
            request_id: "req_test".to_string(),
            idempotency_key: None,
            created_at: ids::now_rfc3339(),
        })
        .await
        .unwrap();

        let recovered = db.recover_interrupted_jobs().await.unwrap();
        assert_eq!(recovered, 1);

        let status: String = db
            .call(|connection| {
                connection.query_row("SELECT status FROM jobs WHERE id = 'job_test'", [], |row| {
                    row.get(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(status, "failed");
    }

    #[tokio::test]
    async fn writes_audit_rows() {
        let db = Db::open_in_memory().unwrap();
        db.insert_audit(
            "auth.failed",
            Some("token"),
            None,
            Some("req_test"),
            "failed",
            Some(serde_json::json!({"path": "/relay/api/v1/devices"})),
            &ids::now_rfc3339(),
        )
        .await
        .unwrap();

        let count: i64 = db
            .call(|connection| {
                connection.query_row("SELECT COUNT(*) FROM audit", [], |row| row.get(0))
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
