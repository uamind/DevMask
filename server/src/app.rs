// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 应用状态、路由装配与服务启动。
//!
//! 路由按作用域分三组（docs/API.md 3.2 / 4）：
//!
//! - 公开：GET /api/v1/healthz；
//! - read：status、schema、config 读、config/validate、templates 读、apps 读、packages、device；
//! - write：PUT /config、templates 写、apps 写；
//! - ops：POST /apps/{pkg}/restart；
//! - /api/v1 未匹配：404 + 错误信封；方法不匹配：405 + 错误信封；
//! - 其余路径：静态资源 / SPA 回退（/api 前缀除外）。
//!
//! write 组的中间件从外到内是：作用域守卫 → Idempotency-Key 重放 → 审计。

use std::sync::Arc;
use std::time::Instant;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{Extensions, HeaderMap, StatusCode, Version, header::CONTENT_TYPE},
    middleware,
    routing::{delete, get, post, put},
};
use device_faker_core::revision::{ConfigStore, StoreOptions};
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

use crate::{
    api,
    audit::{self, AuditLog},
    auth::{self, Scope, ScopeGuard},
    commands::{CommandRunner, SystemRunner},
    device::DeviceSummary,
    idempotency::{self, IdempotencyStore},
    packages::PackageService,
    requests,
    settings::Settings,
    static_files::{self, StaticSite},
    util,
};

struct Inner {
    settings: Settings,
    store: ConfigStore,
    device: DeviceSummary,
    site: StaticSite,
    bind: String,
    started: Instant,
    started_at: String,
    version: &'static str,
    commands: Arc<dyn CommandRunner>,
    packages: Arc<PackageService>,
    audit: AuditLog,
    idempotency: IdempotencyStore,
}

/// 共享应用状态（内部 Arc，clone 很便宜）。
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

impl AppState {
    /// 生产构造：系统命令执行器。
    pub fn new(settings: Settings, bind: String, device: DeviceSummary) -> Self {
        Self::with_runner(settings, bind, device, Arc::new(SystemRunner))
    }

    /// 指定命令执行器（host 测试注入 fixture）。
    pub fn with_runner(
        settings: Settings,
        bind: String,
        device: DeviceSummary,
        commands: Arc<dyn CommandRunner>,
    ) -> Self {
        let mut options = StoreOptions::default();
        if let Some(backup_dir) = &settings.backup_dir {
            options.backup_dir = Some(backup_dir.clone());
        }
        let store = ConfigStore::with_options(settings.config_path.clone(), options);
        let site = StaticSite::new(settings.static_dir.clone());
        let audit = AuditLog::new(settings.audit_path());
        let packages = Arc::new(PackageService::new(commands.clone()));
        Self {
            inner: Arc::new(Inner {
                settings,
                store,
                device,
                site,
                bind,
                started: Instant::now(),
                started_at: util::now_rfc3339(),
                version: env!("CARGO_PKG_VERSION"),
                commands,
                packages,
                audit,
                idempotency: IdempotencyStore::default(),
            }),
        }
    }

    /// 运行设置。
    pub fn settings(&self) -> &Settings {
        &self.inner.settings
    }

    /// 配置仓库（revision + 锁 + 备份路径）。
    pub fn store(&self) -> &ConfigStore {
        &self.inner.store
    }

    /// 设备摘要。
    pub fn device(&self) -> &DeviceSummary {
        &self.inner.device
    }

    /// 静态站点。
    pub fn site(&self) -> &StaticSite {
        &self.inner.site
    }

    /// 实际监听地址。
    pub fn bind(&self) -> &str {
        &self.inner.bind
    }

    /// 已运行秒数。
    pub fn uptime_sec(&self) -> u64 {
        self.inner.started.elapsed().as_secs()
    }

    /// 启动时间（UTC RFC 3339）。
    pub fn started_at(&self) -> &str {
        &self.inner.started_at
    }

    /// 服务版本。
    pub fn version(&self) -> &'static str {
        self.inner.version
    }

    /// 命令执行器。
    pub fn commands(&self) -> &dyn CommandRunner {
        self.inner.commands.as_ref()
    }

    /// 包查询服务。
    pub fn packages(&self) -> &PackageService {
        &self.inner.packages
    }

    /// 审计日志。
    pub fn audit(&self) -> &AuditLog {
        &self.inner.audit
    }

    /// 幂等重放缓存。
    pub fn idempotency(&self) -> &IdempotencyStore {
        &self.inner.idempotency
    }
}

/// 组装路由。
pub fn router(state: AppState) -> Router {
    let read_routes = guarded(
        Router::new()
            .route("/status", get(api::status))
            .route("/overview", get(api::overview::get_overview))
            .route("/schema/config", get(api::schema::get_config_schema))
            .route("/config", get(api::config::get_config))
            .route("/config/revision", get(api::config::get_revision))
            .route("/config/validate", post(api::config::validate_config))
            .route("/templates", get(api::templates::list))
            .route("/templates/{name}", get(api::templates::get))
            .route("/apps", get(api::apps::list))
            .route("/apps/{pkg}", get(api::apps::get))
            .route("/packages", get(api::packages::list))
            .route("/device", get(api::device::get_device))
            .route("/device/props", get(api::device::get_props))
            .route("/device/users", get(api::device::get_users)),
        state.clone(),
        Scope::Read,
    );

    let write_routes = guarded(
        Router::new()
            .route("/config", put(api::config::put_config))
            .route(
                "/templates/{name}",
                put(api::templates::put)
                    .patch(api::templates::patch)
                    .delete(api::templates::delete),
            )
            .route(
                "/templates/{name}/packages",
                post(api::templates::bind_packages),
            )
            .route(
                "/templates/{name}/packages/{pkg}",
                delete(api::templates::unbind_package_handler),
            )
            .route("/apps/{pkg}", put(api::apps::put).delete(api::apps::delete))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                audit::middleware,
            ))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                idempotency::middleware,
            )),
        state.clone(),
        Scope::Write,
    );

    let ops_routes = guarded(
        Router::new()
            .route("/apps/{pkg}/restart", post(api::apps::restart))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                audit::middleware,
            ))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                idempotency::middleware,
            )),
        state.clone(),
        Scope::Ops,
    );

    let api = Router::new()
        .route("/healthz", get(api::healthz))
        .merge(read_routes)
        .merge(write_routes)
        .merge(ops_routes)
        .method_not_allowed_fallback(api::method_not_allowed)
        .fallback(api::not_found);

    let body_limit = state.settings().body_limit_bytes;

    Router::new()
        .nest("/api/v1", api)
        .fallback(static_files::handle)
        .with_state(state)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(middleware::from_fn_with_state(
            body_limit,
            requests::envelope_oversize,
        ))
        .layer(CompressionLayer::new().compress_when(compressible))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(requests::attach))
}

fn guarded(router: Router<AppState>, state: AppState, scope: Scope) -> Router<AppState> {
    router.route_layer(middleware::from_fn_with_state(
        ScopeGuard {
            state,
            required: scope,
        },
        auth::guard,
    ))
}

/// 压缩判定：只压缩文本/脚本类资源；index.html（text/html）按契约不压缩。
fn compressible(
    _status: StatusCode,
    _version: Version,
    headers: &HeaderMap,
    _extensions: &Extensions,
) -> bool {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    const COMPRESSIBLE: [&str; 9] = [
        "text/css",
        "text/javascript",
        "application/javascript",
        "application/json",
        "image/svg+xml",
        "application/wasm",
        "text/plain",
        "font/woff2",
        "application/octet-stream",
    ];
    COMPRESSIBLE
        .iter()
        .any(|prefix| content_type.starts_with(prefix))
}

/// 启动 HTTP 服务（SIGTERM / Ctrl-C 优雅退出）。
pub async fn serve(state: AppState, listener: tokio::net::TcpListener) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
