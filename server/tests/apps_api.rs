// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 应用端点集成测试（docs/API.md 5.15-5.19）。

mod common;

use axum::http::{Method, StatusCode};
use device_faker_core::Config;
use device_faker_server::commands::FixtureRunner;
use serde_json::json;

use common::{
    ADMIN_TOKEN, OPS_TOKEN, READ_TOKEN, WRITE_TOKEN, assert_error_envelope, body_json,
    current_revision, fixture, fixture_with_runner, get_with_token, header, if_match, json_call,
    pm_fixture, read_config, send_body,
};

fn pm_only_fixture() -> FixtureRunner {
    FixtureRunner::new()
        .with_output(
            "pm list packages -u --show-versioncode",
            "package:com.example.app versionCode:420\n",
        )
        .with_output("pm list packages -s", "")
        .with_output("pm list users", "Users:\n\tUserInfo{0:Owner:13} running\n")
        .with_output("am get-current-user", "0\n")
        .with_output("pm list packages --user 0", "package:com.example.app\n")
}

#[tokio::test]
async fn list_apps_returns_inline_records_only() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/apps", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["package"], "com.example.inline");
    assert_eq!(body["items"][0]["position"], 0);
    assert_eq!(body["items"][0]["source"]["type"], "inline");
    assert_eq!(
        body["items"][0]["source"]["template"],
        serde_json::Value::Null
    );
    assert_eq!(body["items"][0]["fieldsSet"], json!(["model"]));

    let response =
        get_with_token(&fixture.router, "/api/v1/apps?source=template", READ_TOKEN).await;
    let body = body_json(response).await;
    assert_eq!(body["total"], 0);
    assert_eq!(body["items"], json!([]));

    let response = get_with_token(&fixture.router, "/api/v1/apps?source=weird", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn get_app_explains_inline_template_and_none() {
    let fixture = fixture();
    let response = get_with_token(
        &fixture.router,
        "/api/v1/apps/com.example.inline",
        READ_TOKEN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["package"], "com.example.inline");
    assert_eq!(body["source"]["type"], "inline");
    assert_eq!(body["source"]["template"], serde_json::Value::Null);
    assert_eq!(body["installed"], serde_json::Value::Null);
    assert_eq!(body["resolved"]["model"], "2509FPN0BC");
    assert_eq!(body["resolved"]["hide_maps"], json!([]));
    assert_eq!(body["resolved"]["force_denylist_unmount"], false);
    assert_eq!(body["resolved"]["companion_resetprop"], false);
    assert!(body["resolved"].get("packages").is_none());

    let response =
        get_with_token(&fixture.router, "/api/v1/apps/com.example.app", READ_TOKEN).await;
    let body = body_json(response).await;
    assert_eq!(body["source"]["type"], "template");
    assert_eq!(body["source"]["template"], "pixel_9");
    assert_eq!(body["source"]["alsoInline"], false);
    assert_eq!(body["resolved"]["model"], "Pixel 9");
    assert_eq!(body["resolved"]["brand"], "google");
    assert_eq!(body["resolved"]["dpi"], 420);

    let response =
        get_with_token(&fixture.router, "/api/v1/apps/com.example.none", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["source"]["type"], "none");
    assert_eq!(body["resolved"], json!({}));
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "no_config")
    );

    let response = get_with_token(&fixture.router, "/api/v1/apps/not-a-package", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "invalid_package"
    );
}

#[tokio::test]
async fn put_app_template_mode_binds_and_removes_inline() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/apps/com.example.inline",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "template": "pixel_9" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "inline_record_removed")
    );

    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert!(
        config.templates["pixel_9"]
            .packages
            .contains(&"com.example.inline".to_owned())
    );
    assert!(
        config
            .apps
            .iter()
            .all(|app| app.package != "com.example.inline")
    );

    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/apps/com.example.shared",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "template": "pixel_9" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert!(
        config.templates["pixel_9"]
            .packages
            .contains(&"com.example.shared".to_owned())
    );
    assert!(
        !config.templates["pixel_9_pro"]
            .packages
            .contains(&"com.example.shared".to_owned())
    );
}

#[tokio::test]
async fn put_app_rejects_bad_shapes_and_missing_template() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/apps/com.example.two",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "template": "pixel_9", "config": { "model": "X" } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["path"],
        "template|config"
    );

    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/apps/com.example.two",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/apps/com.example.two",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "template": "missing_template" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "not_found");
    assert_eq!(error["details"]["resource"], "template");
}

#[tokio::test]
async fn put_app_config_mode_creates_and_updates_inline_record() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/apps/com.example.three",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "config": { "model": "2509FPN0BC", "dpi": 420 } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    let app = config
        .apps
        .iter()
        .find(|app| app.package == "com.example.three")
        .expect("inline record");
    assert_eq!(app.model.as_deref(), Some("2509FPN0BC"));

    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/apps/com.example.three",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "config": { "package": "com.example.three", "model": "X" } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    let app = config
        .apps
        .iter()
        .find(|app| app.package == "com.example.three")
        .unwrap();
    assert_eq!(app.model.as_deref(), Some("X"));
    assert_eq!(app.dpi, None, "整记录替换");

    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/apps/com.example.three",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "config": { "package": "com.other.app", "model": "X" } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn delete_app_requires_confirm_and_supports_detach() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/apps/com.example.inline",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "confirmation_required");
    assert_eq!(error["details"]["action"], "delete_app");

    let response = send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/apps/com.example.inline?confirm=true",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert!(config.apps.is_empty());

    let revision = current_revision(&fixture);
    let response = send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/apps/com.example.app?confirm=true&detach_templates=true",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert!(
        !config.templates["pixel_9"]
            .packages
            .contains(&"com.example.app".to_owned())
    );

    let revision = current_revision(&fixture);
    let response = send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/apps/com.example.none?confirm=true",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "not_found");
    assert_eq!(error["details"]["resource"], "app_config");
}

#[tokio::test]
async fn restart_app_runs_am_and_supports_idempotency() {
    let fixture = fixture_with_runner(Some(common::CONFIG_TEXT), pm_fixture());
    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/apps/com.example.app/restart",
        OPS_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["package"], "com.example.app");
    assert_eq!(body["forceStopped"], true);
    assert_eq!(body["launched"], false);
    assert!(body["durationMs"].as_u64().is_some());

    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/apps/com.example.app/restart",
        OPS_TOKEN,
        &[("idempotency-key", "restart-1")],
        &json!({ "forceStop": true, "launch": false }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(response.headers(), "idempotent-replay"), None);
    let first_body = body_json(response).await;

    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/apps/com.example.app/restart",
        OPS_TOKEN,
        &[("idempotency-key", "restart-1")],
        &json!({ "forceStop": true, "launch": false }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "idempotent-replay"),
        Some("true")
    );
    assert_eq!(body_json(response).await, first_body);
}

#[tokio::test]
async fn restart_app_launch_and_failures() {
    let fixture = fixture_with_runner(Some(common::CONFIG_TEXT), pm_fixture());
    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/apps/com.example.app/restart",
        OPS_TOKEN,
        &[],
        &json!({ "forceStop": true, "launch": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["launched"], true);

    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/apps/com.example.zzz/restart",
        OPS_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let broken = fixture_with_runner(Some(common::CONFIG_TEXT), pm_only_fixture());
    let response = json_call(
        &broken.router,
        Method::POST,
        "/api/v1/apps/com.example.app/restart",
        OPS_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "upstream_unavailable");
    assert_eq!(error["details"]["component"], "am");
}

#[tokio::test]
async fn restart_requires_ops_scope() {
    let fixture = fixture_with_runner(Some(common::CONFIG_TEXT), pm_fixture());
    for token in [READ_TOKEN, WRITE_TOKEN] {
        let response = json_call(
            &fixture.router,
            Method::POST,
            "/api/v1/apps/com.example.app/restart",
            token,
            &[],
            &json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{token}");
    }
}
