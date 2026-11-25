// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! DevMask Manager / Relay：多设备管理端的库入口（lib + bin 拆分便于集成测试）。

pub mod api;
pub mod args;
pub mod auth;
pub mod config;
pub mod crypto;
pub mod db;
pub mod device_client;
pub mod error;
pub mod ids;
pub mod jobs;
pub mod paging;
pub mod request_id;
pub mod state;
pub mod static_files;

pub use api::build_router;
pub use config::Config;
pub use state::AppState;

/// Manager 版本（来自 Cargo.toml）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn harden_database_permissions(database: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut candidates = vec![database.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        let mut path = std::ffi::OsString::from(database.as_os_str());
        path.push(suffix);
        candidates.push(std::path::PathBuf::from(path));
    }
    for path in candidates {
        if path.exists()
            && let Err(error) =
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        {
            tracing::warn!(file = %path.display(), error = %error, "设置数据库文件权限 0600 失败");
        }
    }
}

#[cfg(not(unix))]
fn harden_database_permissions(database: &std::path::Path) {
    let _ = database;
}

/// 初始化数据目录、密钥、SQLite（含迁移与崩溃恢复）、静态目录，返回可服务的状态。
pub async fn initialize(config: Config) -> anyhow::Result<AppState> {
    use anyhow::Context as _;

    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("创建数据目录失败：{}", config.data_dir.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) =
            std::fs::set_permissions(&config.data_dir, std::fs::Permissions::from_mode(0o700))
        {
            tracing::warn!(
                dir = %config.data_dir.display(),
                error = %error,
                "设置数据目录权限 0700 失败"
            );
        }
    }

    let key = crypto::DeviceKey::load_or_create(&config.key_file)
        .with_context(|| format!("加载设备 Token 密钥失败：{}", config.key_file.display()))?;

    let db_file = config.db_file();
    let db =
        db::Db::open(&db_file).with_context(|| format!("打开数据库失败：{}", db_file.display()))?;
    harden_database_permissions(&db_file);

    let recovered = db
        .recover_interrupted_jobs()
        .await
        .context("恢复中断的 job 失败")?;
    if recovered > 0 {
        tracing::warn!(recovered, "启动时将中断的 job 标记为 failed");
    }

    let (jobs_deleted, audits_deleted) = db.prune().await.context("清理历史数据失败")?;
    if jobs_deleted + audits_deleted > 0 {
        tracing::info!(jobs_deleted, audits_deleted, "已清理过期 job / 审计");
    }

    let static_site = config.static_dir.as_ref().and_then(state::StaticSite::new);
    match &static_site {
        Some(site) => tracing::info!(dir = %site.dir.display(), "静态托管已启用"),
        None => tracing::warn!("WebUI dist 未就绪：静态页面将显示构建提示，API 不受影响"),
    }

    Ok(AppState::new(config, db, key, static_site))
}
