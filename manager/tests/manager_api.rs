// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! Manager 骨架验收：健康检查、Bearer 鉴权、错误信封、静态托管与 SPA fallback。

mod common;

use axum::http::{Method, StatusCode};
use common::{send, start_app};
use serde_json::{Value, json};

fn assert_error_envelope(body: &Value, code: &str) {
    let error = body.get("error").expect("应含 error 对象");
    assert_eq!(error["code"], json!(code));
    assert!(error["message"].is_string());
    assert!(error["retryable"].is_boolean());
    let request_id = error["requestId"].as_str().expect("requestId 字符串");
    assert!(request_id.starts_with("req_"), "{request_id}");
}

#[tokio::test]
async fn healthz_is_public_and_reports_ok() {
    let app = start_app(None).await;
    let response = send(&app, Method::GET, "/relay/api/v1/healthz", None, None).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.header("cache-control"), Some("no-store"));

    let body = response.json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["apiVersion"], "v1");
    assert_eq!(body["db"]["status"], "ok");
    assert_eq!(body["db"]["schemaVersion"], 2);
    assert_eq!(body["devices"]["total"], 0);
    assert_eq!(body["devices"]["online"], 0);
    assert_eq!(body["devices"]["offline"], 0);
    assert!(body["uptimeSec"].is_u64());
    assert!(body["startedAt"].as_str().unwrap().ends_with('Z'));
}

#[tokio::test]
async fn root_healthz_alias_works() {
    let app = start_app(None).await;
    let response = send(&app, Method::GET, "/healthz", None, None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json()["status"], "ok");
}

#[tokio::test]
async fn protected_route_without_token_returns_unified_401() {
    let app = start_app(None).await;
    let response = send(&app, Method::GET, "/relay/api/v1/devices", None, None).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.header("www-authenticate"), Some("Bearer"));
    assert!(
        response
            .header("content-type")
            .unwrap_or_default()
            .starts_with("application/json")
    );
    assert_eq!(response.header("cache-control"), Some("no-store"));

    let body = response.json();
    assert_error_envelope(&body, "unauthorized");
    assert!(body["error"]["details"].is_null());
    assert_eq!(body["error"]["retryable"], false);

    let request_id = body["error"]["requestId"].as_str().unwrap();
    assert_eq!(response.header("x-request-id"), Some(request_id));
}

#[tokio::test]
async fn wrong_token_is_rejected() {
    let app = start_app(None).await;
    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some("not-the-token"),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_error_envelope(&response.json(), "unauthorized");
}

#[tokio::test]
async fn valid_token_lists_empty_devices() {
    let app = start_app(None).await;
    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some(&app.token),
        None,
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    assert_eq!(body["devices"], json!([]));
    assert_eq!(body["total"], 0);
    assert_eq!(body["online"], 0);
    assert_eq!(body["offline"], 0);
    assert!(body["nextCursor"].is_null());
}

#[tokio::test]
async fn unknown_device_returns_device_not_found() {
    let app = start_app(None).await;
    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9",
        Some(&app.token),
        None,
    )
    .await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let body = response.json();
    assert_error_envelope(&body, "device_not_found");
    assert_eq!(body["error"]["details"]["deviceId"], "pixel-9");
}

#[tokio::test]
async fn request_id_is_echoed_into_header_and_envelope() {
    let app = start_app(None).await;
    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        None,
        Some("req_client-123.test"),
    )
    .await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.header("x-request-id"), Some("req_client-123.test"));
    assert_eq!(response.json()["error"]["requestId"], "req_client-123.test");
}

#[tokio::test]
async fn invalid_client_request_id_is_replaced() {
    let app = start_app(None).await;
    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        None,
        Some("bad request id"),
    )
    .await;

    let request_id = response.header("x-request-id").unwrap_or_default();
    assert!(request_id.starts_with("req_"), "{request_id}");
    assert_eq!(response.json()["error"]["requestId"], request_id);
}

#[tokio::test]
async fn unknown_api_route_returns_json_404() {
    let app = start_app(None).await;
    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/nope",
        Some(&app.token),
        None,
    )
    .await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(
        response
            .header("content-type")
            .unwrap_or_default()
            .starts_with("application/json")
    );
    let body = response.json();
    assert_error_envelope(&body, "not_found");
    assert_eq!(body["error"]["details"]["resource"], "route");
}

#[tokio::test]
async fn method_not_allowed_returns_envelope() {
    let app = start_app(None).await;
    let response = send(
        &app,
        Method::POST,
        "/relay/api/v1/healthz",
        Some(&app.token),
        None,
    )
    .await;

    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    let body = response.json();
    assert_error_envelope(&body, "method_not_allowed");
    assert_eq!(body["error"]["details"]["method"], "POST");
}

#[tokio::test]
async fn missing_dist_returns_readable_build_hint() {
    let app = start_app(None).await;

    let root = send(&app, Method::GET, "/", None, None).await;
    assert_eq!(root.status, StatusCode::OK);
    assert_eq!(root.header("cache-control"), Some("no-cache"));
    let html = root.text();
    assert!(html.contains("WebUI 尚未构建"), "{html}");
    assert!(html.contains("npm run build"), "{html}");

    let asset = send(&app, Method::GET, "/assets/app.js", None, None).await;
    assert_eq!(asset.status, StatusCode::NOT_FOUND);
    assert!(asset.text().contains("尚未构建"));
}

#[tokio::test]
async fn serves_index_assets_and_spa_fallback() {
    let dist = tempfile::tempdir().expect("dist");
    std::fs::write(
        dist.path().join("index.html"),
        "<!doctype html><title>manager-index</title>",
    )
    .unwrap();
    std::fs::create_dir_all(dist.path().join("assets")).unwrap();
    std::fs::write(dist.path().join("assets/app-123.js"), "console.log(1)").unwrap();

    let app = start_app(Some(dist.path().to_path_buf())).await;

    let index = send(&app, Method::GET, "/", None, None).await;
    assert_eq!(index.status, StatusCode::OK);
    assert!(index.text().contains("manager-index"));
    assert_eq!(index.header("cache-control"), Some("no-cache"));
    assert_eq!(index.header("x-content-type-options"), Some("nosniff"));

    let asset = send(&app, Method::GET, "/assets/app-123.js", None, None).await;
    assert_eq!(asset.status, StatusCode::OK);
    assert_eq!(
        asset.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        asset.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(asset.text(), "console.log(1)");

    let head = send(&app, Method::HEAD, "/assets/app-123.js", None, None).await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());

    let spa = send(&app, Method::GET, "/apps", None, None).await;
    assert_eq!(spa.status, StatusCode::OK);
    assert!(spa.text().contains("manager-index"));

    let traversal = send(&app, Method::GET, "/assets/../index.html", None, None).await;
    assert_eq!(traversal.status, StatusCode::OK);
    assert!(!traversal.text().contains("workspace"));
}

#[tokio::test]
async fn static_route_rejects_non_get_methods() {
    let app = start_app(None).await;
    let response = send(&app, Method::POST, "/apps", None, None).await;
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_error_envelope(&response.json(), "method_not_allowed");
}

#[tokio::test]
async fn auth_failure_is_audited_without_token() {
    let app = start_app(None).await;
    let _ = send(&app, Method::GET, "/relay/api/v1/devices", None, None).await;

    let (action, detail): (String, Option<String>) = app
        .state
        .db
        .call(|connection| {
            connection.query_row(
                "SELECT action, detail FROM audit ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .await
        .expect("审计查询");

    assert_eq!(action, "auth.failed");
    let detail = detail.expect("detail");
    assert!(detail.contains("missing"));
    assert!(!detail.contains(&app.token));
}

#[tokio::test]
async fn key_file_is_generated_private() {
    let app = start_app(None).await;
    let key_path = app.state.cfg.data_dir.join("manager.key");
    let bytes = std::fs::read(&key_path).expect("密钥文件");
    assert_eq!(bytes.len(), 32);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode={mode:o}");

        let dir_mode = std::fs::metadata(&app.state.cfg.data_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "dir={dir_mode:o}");

        let db_path = app.data_dir.join("manager.db");
        let db_mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(db_mode, 0o600, "db={db_mode:o}");
    }
}

#[tokio::test]
async fn api_responses_are_no_store_and_root_healthz_is_json_405() {
    let app = start_app(None).await;

    let ok = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.header("cache-control"), Some("no-store"));

    let not_found = send(
        &app,
        Method::GET,
        "/relay/api/v1/nope",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(not_found.status, StatusCode::NOT_FOUND);
    assert_eq!(not_found.header("cache-control"), Some("no-store"));

    let root = send(&app, Method::POST, "/healthz", None, None).await;
    assert_eq!(root.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(root.json()["error"]["code"], "method_not_allowed");
    assert_eq!(root.header("cache-control"), Some("no-store"));
}

#[tokio::test]
async fn device_token_encryption_roundtrips_through_state_key() {
    let app = start_app(None).await;
    let stored = app.state.key.encrypt("pixel-9", "admin123").unwrap();
    assert!(stored.starts_with("v1:"));
    assert!(!stored.contains("admin123"));
    assert_eq!(
        app.state.key.decrypt("pixel-9", &stored).unwrap().as_str(),
        "admin123"
    );
    assert!(
        app.state.key.decrypt("xiaomi-15", &stored).is_err(),
        "AAD 绑定设备 id，换设备必须解不开"
    );
}
