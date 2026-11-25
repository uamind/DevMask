// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 路由组装：/relay/api/v1 控制面 + /healthz 别名 + 静态托管 fallback。

pub mod aggregate;
pub mod apply;
pub mod apps;
pub mod audit;
pub mod devices;
pub mod dto;
pub mod egress;
pub mod egress_dto;
pub mod health;
pub mod job_dto;
pub mod jobs;
pub mod overview;
pub mod proxy;
pub mod query;
pub mod templates;
pub mod token;

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use tower_http::{compression::CompressionLayer, limit::RequestBodyLimitLayer, trace::TraceLayer};

use crate::{auth, error::ApiError, request_id, state::AppState, static_files};

/// 请求体上限：1 MiB。
pub const BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// 构造完整应用路由（测试直接复用）。
pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/devices", get(devices::list).post(devices::create))
        .route("/devices/sync", post(devices::sync_all))
        .route(
            "/devices/{id}",
            get(devices::get)
                .patch(devices::update)
                .delete(devices::remove),
        )
        .route("/devices/{id}/sync", post(devices::sync_one))
        .route("/devices/{id}/overview", get(devices::overview))
        .route("/apps", get(apps::matrix))
        .route("/templates", get(templates::list))
        .route("/overview", get(overview::overview))
        .route("/audit", get(audit::list))
        .route("/token/rotate", post(token::rotate))
        .route("/apply", post(apply::apply))
        .route(
            "/egress-proxies",
            get(egress::list_proxies).post(egress::create_proxy),
        )
        .route(
            "/egress-proxies/{id}",
            get(egress::get_proxy)
                .patch(egress::update_proxy)
                .delete(egress::delete_proxy),
        )
        .route("/egress-proxies/{id}/probe", post(egress::probe_proxy))
        .route("/devices/egress/apply", post(egress::apply_egress))
        .route(
            "/devices/{id}/egress",
            get(egress::get_device_egress).put(egress::put_device_egress),
        )
        .route("/jobs", get(jobs::list))
        .route("/jobs/{id}", get(jobs::get))
        .route("/devices/{id}/proxy/api/v1/{*path}", any(proxy::proxy))
        .fallback(api_not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT_BYTES))
        .layer(middleware::from_fn(map_body_limit))
        .layer(middleware::from_fn(no_store));

    Router::new()
        .nest("/relay/api/v1", api)
        .route("/healthz", get(health::healthz))
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(static_files::serve)
        .layer(middleware::from_fn(request_id::middleware))
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .with_state(state)
}

async fn api_not_found(request: Request) -> Response {
    ApiError::not_found("route", request.uri().path()).into_response()
}

async fn method_not_allowed(request: Request) -> Response {
    let method = request.method().clone();
    ApiError::method_not_allowed(method.as_str(), request.uri().path()).into_response()
}

/// 所有 /relay/api/v1 响应统一 Cache-Control: no-store（含 202/401/405/413）。
async fn no_store(request: Request, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// tower-http 的 body limit 返回空 413；这里转换成统一错误信封。
async fn map_body_limit(request: Request, next: middleware::Next) -> Response {
    let response = next.run(request).await;
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::body_too_large(BODY_LIMIT_BYTES).into_response();
    }
    response
}
