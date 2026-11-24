// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 进程状态：配置、数据库、密钥、Token、并发信号量与静态站点。

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use tokio::sync::{RwLock, Semaphore};

use crate::{config::Config, crypto::DeviceKey, db::Db, device_client::DeviceHttp};

/// Manager Token 的来源；优先级 CLI > env > 文件 > 配置文件 > 默认值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenSource {
    Cli,
    Env,
    File,
    Config,
    Default,
}

impl TokenSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Env => "env",
            Self::File => "file",
            Self::Config => "config",
            Self::Default => "default",
        }
    }
}

/// 当前生效的 Manager Token 与来源。
#[derive(Clone)]
pub struct TokenState {
    pub value: String,
    pub source: TokenSource,
}

impl std::fmt::Debug for TokenState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenState")
            .field("value", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

/// 已解析并校验过 index.html 的 WebUI 静态目录。
#[derive(Debug, Clone)]
pub struct StaticSite {
    pub dir: PathBuf,
    pub index: PathBuf,
}

impl StaticSite {
    pub fn new(dir: impl Into<PathBuf>) -> Option<Self> {
        let dir = dir.into();
        let index = dir.join("index.html");
        index.is_file().then_some(Self { dir, index })
    }
}

/// 全进程共享状态；克隆成本仅为 Arc 计数。
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: Db,
    pub key: Arc<DeviceKey>,
    pub http: DeviceHttp,
    pub token: Arc<RwLock<TokenState>>,
    pub started_at: Instant,
    pub started_at_rfc3339: String,
    pub static_site: Option<Arc<StaticSite>>,
    pub sync_sem: Arc<Semaphore>,
    pub apply_sem: Arc<Semaphore>,
    pub job_sem: Arc<Semaphore>,
    pub device_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl AppState {
    pub fn new(cfg: Config, db: Db, key: DeviceKey, static_site: Option<StaticSite>) -> Self {
        let token = cfg.token.clone();
        let sync_sem = Arc::new(Semaphore::new(cfg.sync_concurrency));
        let apply_sem = Arc::new(Semaphore::new(cfg.apply_concurrency));
        let job_sem = Arc::new(Semaphore::new(2));
        let http = DeviceHttp::new();

        Self {
            cfg: Arc::new(cfg),
            db,
            key: Arc::new(key),
            http,
            token: Arc::new(RwLock::new(token)),
            started_at: Instant::now(),
            started_at_rfc3339: crate::ids::now_rfc3339(),
            static_site: static_site.map(Arc::new),
            sync_sem,
            apply_sem,
            job_sem,
            device_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 进程已运行秒数。
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// 取（或创建）某台设备的串行锁，避免同一设备的同步 / 下发并发。
    pub fn device_lock(&self, device_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .device_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(device_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// 设备被移除时清理对应锁。
    pub fn forget_device_lock(&self, device_id: &str) {
        let mut locks = self
            .device_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.remove(device_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_state_debug_redacts_secret() {
        let token = TokenState {
            value: "super-secret-token".to_string(),
            source: TokenSource::File,
        };
        let rendered = format!("{token:?}");
        assert!(!rendered.contains("super-secret-token"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }
}
