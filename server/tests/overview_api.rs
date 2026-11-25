// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! GET /api/v1/overview 的集成测试：设备指纹、模板与每个包的最终生效值。

mod common;

use axum::http::StatusCode;
use common::{
    ADMIN_TOKEN, CONFIG_TEXT, READ_TOKEN, WRITE_TOKEN, body_json, current_revision, fixture,
    fixture_with_runner, get_with_token, header, pm_fixture,
};

#[tokio::test]
async fn overview_returns_device_templates_and_apps() {
    let fixture = fixture_with_runner(Some(CONFIG_TEXT), pm_fixture());
    let response = get_with_token(&fixture.router, "/api/v1/overview", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "x-dm-revision"),
        Some(current_revision(&fixture).as_str())
    );

    let body = body_json(response).await;
    assert_eq!(body["revision"], current_revision(&fixture));
    assert_eq!(body["device"]["model"], "Pixel 9");
    assert_eq!(
        body["device"]["fingerprint"],
        "google/tokay/tokay:17/BP2A/13000000:user/release-keys"
    );
    assert!(body["root"].is_object());
    assert_eq!(body["templates"].as_array().unwrap().len(), 2);
    assert_eq!(body["templates"][0]["name"], "pixel_9");
    assert_eq!(body["templates"][0]["config"]["model"], "Pixel 9");

    let apps = body["apps"].as_array().expect("apps");
    let names: Vec<&str> = apps
        .iter()
        .map(|app| app["package"].as_str().expect("package"))
        .collect();
    assert_eq!(
        names,
        vec![
            "com.example.app",
            "com.example.inline",
            "com.example.shared"
        ]
    );

    let app = apps
        .iter()
        .find(|app| app["package"] == "com.example.app")
        .expect("template app");
    assert_eq!(app["source"]["type"], "template");
    assert_eq!(app["source"]["template"], "pixel_9");
    assert_eq!(app["resolved"]["model"], "Pixel 9");
    assert_eq!(app["installed"], true);

    let inline = apps
        .iter()
        .find(|app| app["package"] == "com.example.inline")
        .expect("inline app");
    assert_eq!(inline["source"]["type"], "inline");
    assert_eq!(inline["resolved"]["model"], "2509FPN0BC");

    let shared = apps
        .iter()
        .find(|app| app["package"] == "com.example.shared")
        .expect("shared app");
    assert!(
        shared["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "duplicate_package")
    );
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "duplicate_package")
    );
}

#[tokio::test]
async fn overview_requires_read_scope() {
    let fixture = fixture();
    let anonymous = common::get(&fixture.router, "/api/v1/overview").await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let wrong_scope = get_with_token(&fixture.router, "/api/v1/overview", WRITE_TOKEN).await;
    assert_eq!(wrong_scope.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn overview_is_listed_in_capabilities() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/status", ADMIN_TOKEN).await;
    let body = body_json(response).await;
    assert!(
        body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "overview")
    );
}
