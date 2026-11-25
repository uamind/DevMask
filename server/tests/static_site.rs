// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 静态托管与 SPA 回退的集成测试（docs/API.md 10.1、11）。

mod common;

use axum::http::{Method, StatusCode};

use common::{body_text, fixture, get, header, send};
use devmask_server::static_files::{ASSET_CACHE_CONTROL, RUNTIME_SCRIPT};

#[tokio::test]
async fn root_serves_index_with_runtime_injection() {
    let fixture = fixture();
    let response = get(&fixture.router, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        header(response.headers(), "cache-control"),
        Some("no-store")
    );
    assert_eq!(
        header(response.headers(), "x-content-type-options"),
        Some("nosniff")
    );

    let text = body_text(response).await;
    assert!(text.contains(RUNTIME_SCRIPT), "必须注入运行时配置");
    assert!(text.contains("window.__DF_SERVER__"));
    assert!(text.contains("\"transport\": \"http\"") || text.contains("transport: \"http\""));
    assert!(
        !text.to_ascii_lowercase().contains("token"),
        "注入不得包含 Token"
    );
    assert!(!text.to_ascii_lowercase().contains("authorization"));
}

#[tokio::test]
async fn assets_are_long_cached_with_mime() {
    let fixture = fixture();

    let js = get(&fixture.router, "/assets/app-abc.js").await;
    assert_eq!(js.status(), StatusCode::OK);
    assert_eq!(
        header(js.headers(), "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        header(js.headers(), "cache-control"),
        Some(ASSET_CACHE_CONTROL)
    );
    assert_eq!(body_text(js).await, "console.log('devmask')\n");

    let css = get(&fixture.router, "/assets/site.css").await;
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(
        header(css.headers(), "content-type"),
        Some("text/css; charset=utf-8")
    );
}

#[tokio::test]
async fn extensionless_paths_fall_back_to_spa() {
    let fixture = fixture();
    let response = get(&fixture.router, "/templates/pixel_9").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        header(response.headers(), "cache-control"),
        Some("no-store")
    );
    let text = body_text(response).await;
    assert!(text.contains(RUNTIME_SCRIPT));
}

#[tokio::test]
async fn missing_asset_with_extension_is_plain_404() {
    let fixture = fixture();
    let response = get(&fixture.router, "/assets/missing-xyz.js").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        header(response.headers(), "content-type"),
        Some("text/plain; charset=utf-8")
    );
    let text = body_text(response).await;
    assert_eq!(text, "Not Found");
    assert!(
        !text.to_ascii_lowercase().contains("<!doctype"),
        "不能回退 index.html"
    );
}

#[tokio::test]
async fn path_traversal_is_blocked() {
    let fixture = fixture();
    for uri in [
        "/assets/../../Cargo.toml",
        "/%2e%2e/Cargo.toml",
        "/assets/%2e%2e%2f%2e%2e%2fCargo.toml",
    ] {
        let response = get(&fixture.router, uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        let text = body_text(response).await;
        assert_eq!(text, "Not Found", "{uri}");
    }
}

#[tokio::test]
async fn head_requests_have_empty_body() {
    let fixture = fixture();
    let response = send(
        &fixture.router,
        Method::HEAD,
        "/assets/app-abc.js",
        None,
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(body_text(response).await, "");
}

#[tokio::test]
async fn already_injected_index_is_not_double_injected() {
    let fixture = fixture();
    let index_path = fixture.state.site().index_path().to_path_buf();
    std::fs::write(
        &index_path,
        format!("<!doctype html><html><head>{RUNTIME_SCRIPT}</head><body></body></html>"),
    )
    .unwrap();

    let response = get(&fixture.router, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let text = body_text(response).await;
    assert_eq!(text.matches("__DF_SERVER__").count(), 1);
}

#[tokio::test]
async fn missing_static_dir_still_serves_api() {
    let fixture = fixture();
    std::fs::remove_dir_all(fixture.state.site().root()).unwrap();
    let root = get(&fixture.router, "/").await;
    assert_eq!(root.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_text(root).await, "Not Found");

    let health = get(&fixture.router, "/api/v1/healthz").await;
    assert_eq!(health.status(), StatusCode::OK);
}
