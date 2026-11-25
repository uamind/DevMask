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
    egress::EgressService,
    idempotency::{self, IdempotencyStore},
    packages::PackageService,
    relay::{self, RelayService},
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
    egress: EgressService,
    relay: RelayService,
}

/// 共享应用状态（内部 Arc，clone 很便宜）。
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

impl AppState {
    /// 生产构造：系统命令执行器。
    pub fn new(settings: Settings, bind: String, device: DeviceSummary) -> Self {
        Self::build(
            settings,
            bind,
            device,
            Arc::new(SystemRunner::default()),
            true,
        )
    }

    /// 指定命令执行器（host 测试注入 fixture）。
    ///
    /// **不自动起 relay listener**：host 测试默认不开监听（避免并行测试抢占
    /// 17800–17899 与真实 settings）；需要真实 listener 的测试用
    /// [`Self::with_runner_and_relay`]。生产走 [`Self::new`]，自动起。
    pub fn with_runner(
        settings: Settings,
        bind: String,
        device: DeviceSummary,
        commands: Arc<dyn CommandRunner>,
    ) -> Self {
        Self::build(settings, bind, device, commands, false)
    }

    /// 指定命令执行器，并允许 relay 自动起监听（host 测试用）。
    pub fn with_runner_and_relay(
        settings: Settings,
        bind: String,
        device: DeviceSummary,
        commands: Arc<dyn CommandRunner>,
    ) -> Self {
        Self::build(settings, bind, device, commands, true)
    }

    fn build(
        settings: Settings,
        bind: String,
        device: DeviceSummary,
        commands: Arc<dyn CommandRunner>,
        relay_binding: bool,
    ) -> Self {
        let mut options = StoreOptions::default();
        if let Some(backup_dir) = &settings.backup_dir {
            options.backup_dir = Some(backup_dir.clone());
        }
        let store = ConfigStore::with_options(settings.config_path.clone(), options);
        let site = StaticSite::new(settings.static_dir.clone());
        let audit = AuditLog::new(settings.audit_path());
        let packages = Arc::new(PackageService::new(commands.clone()));
        // egress.toml 与 config.toml 同目录（0600）；备份目录沿用 server.toml 的配置。
        let egress = EgressService::new(&settings.config_path, settings.backup_dir.clone());
        // 中继也要用同一个命令执行器（iptables -S/-A 走它），因此先 clone 好再 move。
        let relay = RelayService::with_runner(relay_binding, Arc::clone(&commands));
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
                egress,
                relay,
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

    /// 出口代理（egress.toml 存储 + 运行时健康状态）。
    pub fn egress(&self) -> &EgressService {
        &self.inner.egress
    }

    /// 设备内认证中继（listener 运行时状态）。
    pub fn relay(&self) -> &RelayService {
        &self.inner.relay
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
            .route("/device/users", get(api::device::get_users))
            .route("/egress", get(api::egress::get_egress))
            .route("/egress/proxies", get(api::egress::list_proxies)),
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
            .route(
                "/egress",
                put(api::egress::put_egress).delete(api::egress::delete_egress),
            )
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

    // POST /egress/probe 是 ops 作用域，但**不挂** Idempotency-Key：
    // 探测每次都要真发请求（而且可能触发 fail-open），本身不是幂等操作（契约 §1.6）。
    // POST /egress/probe 与 POST /egress/relay/restart 是动作而不是配置写入：
    // **不挂** Idempotency-Key（契约 §6.3：它与 probe 同口径）。
    let egress_ops_routes = guarded(
        Router::new()
            .route("/egress/probe", post(api::egress::probe_egress))
            .route("/egress/relay/restart", post(api::egress::restart_relay))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                audit::middleware,
            )),
        state.clone(),
        Scope::Ops,
    );

    let api = Router::new()
        .route("/healthz", get(api::healthz))
        .merge(read_routes)
        .merge(write_routes)
        .merge(ops_routes)
        .merge(egress_ops_routes)
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
///
/// 端口已经绑定（调用方 bind 过），这里先起一次**异步** egress 自愈再开始服务：
/// 自愈跑在 blocking 线程池里，不阻塞 healthz，也不阻塞监听（契约 §3.4）。
pub async fn serve(state: AppState, listener: tokio::net::TcpListener) -> std::io::Result<()> {
    // 中继先起来（绑定 listener），再让 egress 自愈去写 settings：
    // 反序会让 settings 短暂指向还没人监听的端口（契约 §7.1）。
    relay::spawn_startup(state.clone());
    let result = axum::serve(listener, router(state.clone()))
        .with_graceful_shutdown(shutdown_signal())
        .await;
    // 优雅停止：关闭全部 relay listener（不动 settings，契约 §5.6）。
    state.relay().shutdown().await;
    result
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
