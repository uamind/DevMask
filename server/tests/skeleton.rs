// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 骨架端点的集成测试：认证、错误信封、healthz / status。

mod common;

use axum::http::{Method, StatusCode, header::CONTENT_TYPE};

use common::{
    ADMIN_TOKEN, READ_TOKEN, WRITE_TOKEN, assert_error_envelope, body_json, body_text, fixture,
    fixture_with_config, get, get_with_token, header, send,
};
use device_faker_server::util;

fn empty_sha256() -> String {
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned()
}

#[tokio::test]
async fn healthz_is_public_and_no_store() {
    let fixture = fixture();
    let response = get(&fixture.router, "/api/v1/healthz").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "cache-control"),
        Some("no-store")
    );
    assert!(
        header(response.headers(), "content-type")
            .unwrap()
            .starts_with("application/json")
    );
    let request_id = header(response.headers(), "x-request-id")
        .expect("x-request-id")
        .to_owned();
    assert!(request_id.starts_with("req_"), "{request_id}");

    let body = body_json(response).await;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], fixture.state.version());
    assert!(body["uptimeSec"].as_u64().is_some());
}

#[tokio::test]
async fn healthz_rejects_other_methods_with_envelope() {
    let fixture = fixture();
    let response = send(&fixture.router, Method::POST, "/api/v1/healthz", None, &[]).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "method_not_allowed");
    assert_eq!(error["details"]["method"], "POST");
    assert_eq!(error["details"]["path"], "/api/v1/healthz");
    assert_eq!(error["retryable"], false);
}

#[tokio::test]
async fn status_without_token_is_401() {
    let fixture = fixture();
    let response = get(&fixture.router, "/api/v1/status").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        header(response.headers(), "cache-control"),
        Some("no-store")
    );
    let header_request_id = header(response.headers(), "x-request-id")
        .expect("x-request-id")
        .to_owned();
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "unauthorized");
    assert_eq!(error["requestId"], header_request_id.as_str());
    assert_eq!(error["details"], serde_json::Value::Null);
    assert_eq!(error["retryable"], false);
}

#[tokio::test]
async fn status_with_unknown_token_is_401() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/status", "df_not_a_real_token").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(response).await;
    assert_error_envelope(&body, "unauthorized");
}

#[tokio::test]
async fn status_with_write_only_token_is_403() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/status", WRITE_TOKEN).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "forbidden_scope");
    assert_eq!(error["details"]["required"], "read");
    assert_eq!(error["details"]["granted"], serde_json::json!(["write"]));
}

#[tokio::test]
async fn status_with_read_token_returns_snapshot() {
    let fixture = fixture();
    let expected_revision = util::sha256_hex(fixture.config_text.as_bytes());

    let response = get_with_token(&fixture.router, "/api/v1/status", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "x-df-revision"),
        Some(expected_revision.as_str())
    );
    assert_eq!(
        header(response.headers(), "cache-control"),
        Some("no-store")
    );

    let body = body_json(response).await;
    assert_eq!(body["module"]["id"], "device_faker");
    assert_eq!(body["module"]["version"], "v1.7.0");
    assert_eq!(body["module"]["versionCode"], 20261109);

    assert_eq!(body["device"]["manufacturer"], "Google");
    assert_eq!(body["device"]["model"], "Pixel 9");
    assert_eq!(body["device"]["android"], "17");
    assert_eq!(body["device"]["sdk"], 37);
    assert_eq!(body["device"]["abi"], "arm64-v8a");

    assert_eq!(body["server"]["version"], fixture.state.version());
    assert_eq!(body["server"]["bind"], "127.0.0.1:8686");
    assert!(body["server"]["uptimeSec"].as_u64().is_some());
    assert!(body["server"]["pid"].as_u64().unwrap() > 0);
    let started_at = body["server"]["startedAt"].as_str().expect("startedAt");
    assert!(
        started_at.ends_with('Z') && started_at.len() == 20,
        "{started_at}"
    );

    assert_eq!(body["config"]["revision"], expected_revision.as_str());
    assert_eq!(body["config"]["exists"], true);
    assert_eq!(body["config"]["size"], fixture.config_text.len() as u64);
    assert_eq!(body["config"]["templates"], 2);
    assert_eq!(body["config"]["apps"], 1);
    assert_eq!(body["config"]["conflicts"], 1);
    assert!(body["config"]["mtime"].as_str().unwrap().ends_with('Z'));

    assert_eq!(
        body["capabilities"],
        serde_json::json!([
            "config",
            "templates",
            "apps",
            "packages",
            "device",
            "validate",
            "overview"
        ])
    );
}

#[tokio::test]
async fn status_with_admin_token_is_allowed() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/status", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn status_rejects_other_methods_with_envelope() {
    let fixture = fixture();
    let response = send(
        &fixture.router,
        Method::POST,
        "/api/v1/status",
        Some(ADMIN_TOKEN),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "method_not_allowed");
    assert_eq!(error["details"]["path"], "/api/v1/status");
}

#[tokio::test]
async fn unknown_api_path_returns_404_envelope_not_html() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/nope", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        header(response.headers(), "content-type")
            .unwrap()
            .starts_with("application/json")
    );
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "not_found");
    assert_eq!(error["details"]["resource"], "path");
    assert_eq!(error["details"]["name"], "/api/v1/nope");
}

#[tokio::test]
async fn api_prefix_without_v1_stays_an_api_error_not_spa() {
    let fixture = fixture();
    let response = get(&fixture.router, "/api/foo").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = header(response.headers(), "content-type").unwrap_or_default();
    assert!(
        content_type.starts_with("application/json"),
        "{content_type}"
    );
    let body = body_json(response).await;
    assert_error_envelope(&body, "not_found");
}

#[tokio::test]
async fn openapi_json_is_next_increment_and_returns_404() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/openapi.json", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert_error_envelope(&body, "not_found");
}

#[tokio::test]
async fn client_request_id_is_echoed_on_success_and_error() {
    let fixture = fixture();
    let ok = send(
        &fixture.router,
        Method::GET,
        "/api/v1/healthz",
        None,
        &[("x-request-id", "client.trace-01")],
    )
    .await;
    assert_eq!(
        header(ok.headers(), "x-request-id"),
        Some("client.trace-01")
    );

    let unauthorized = send(
        &fixture.router,
        Method::GET,
        "/api/v1/status",
        None,
        &[("x-request-id", "client.trace-02")],
    )
    .await;
    assert_eq!(
        header(unauthorized.headers(), "x-request-id"),
        Some("client.trace-02")
    );
    let body = body_json(unauthorized).await;
    let error = assert_error_envelope(&body, "unauthorized");
    assert_eq!(error["requestId"], "client.trace-02");
}

#[tokio::test]
async fn missing_config_file_still_reports_status() {
    let fixture = fixture_with_config(None);
    let response = get_with_token(&fixture.router, "/api/v1/status", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "x-df-revision"),
        Some(empty_sha256().as_str())
    );
    let body = body_json(response).await;
    assert_eq!(body["config"]["exists"], false);
    assert_eq!(body["config"]["revision"], empty_sha256().as_str());
    assert_eq!(body["config"]["size"], 0);
    assert_eq!(body["config"]["templates"], 0);
    assert_eq!(body["config"]["apps"], 0);
    assert_eq!(body["config"]["conflicts"], 0);
    assert_eq!(body["config"]["mtime"], serde_json::Value::Null);
}

#[tokio::test]
async fn invalid_config_yields_internal_error_envelope() {
    let fixture = fixture_with_config(Some("[templates.broken\npackages = [\n"));
    let response = get_with_token(&fixture.router, "/api/v1/status", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "internal_error");
    assert_eq!(error["retryable"], true);
    assert!(
        error["message"].as_str().unwrap().contains("配置解析失败"),
        "{error}"
    );
}

#[tokio::test]
async fn status_response_is_json_not_html() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/status", READ_TOKEN).await;
    let content_type = header(response.headers(), CONTENT_TYPE.as_str())
        .unwrap_or_default()
        .to_owned();
    let text = body_text(response).await;
    assert!(content_type.starts_with("application/json"));
    assert!(!text.to_ascii_lowercase().contains("<!doctype html>"));
}
