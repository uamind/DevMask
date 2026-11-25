// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! packages / device 端点集成测试（docs/API.md 5.20-5.23）。

mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{READ_TOKEN, body_json, fixture, fixture_with_runner, get_with_token, pm_fixture};

fn pm_fixture_device() -> common::Fixture {
    fixture_with_runner(Some(common::CONFIG_TEXT), pm_fixture())
}

#[tokio::test]
async fn packages_lists_pm_index_with_users_and_versions() {
    let fixture = pm_fixture_device();
    let response = get_with_token(&fixture.router, "/api/v1/packages", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["source"], "pm");
    assert_eq!(body["total"], 2);
    assert_eq!(body["nextCursor"], serde_json::Value::Null);
    assert_eq!(body["warnings"], json!([]));

    let chrome = &body["items"][0];
    assert_eq!(chrome["package"], "com.android.chrome");
    assert_eq!(chrome["userIds"], json!([0]));
    assert_eq!(chrome["installed"], true);
    assert_eq!(chrome["isSystem"], true);
    assert_eq!(chrome["versionCode"], 677820000);
    assert_eq!(chrome["label"], serde_json::Value::Null);

    let example = &body["items"][1];
    assert_eq!(example["package"], "com.example.app");
    assert_eq!(example["userIds"], json!([0, 999]));
    assert_eq!(example["isSystem"], false);
    assert_eq!(example["versionCode"], 420);
    assert_eq!(
        example["versionName"],
        serde_json::Value::Null,
        "未按名字查询时不补 versionName"
    );
}

#[tokio::test]
async fn packages_names_filter_enriches_and_reports_missing() {
    let fixture = pm_fixture_device();
    let response = get_with_token(
        &fixture.router,
        "/api/v1/packages?names=com.example.app,com.missing.app",
        READ_TOKEN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["total"], 2);
    assert_eq!(body["items"][0]["package"], "com.example.app");
    assert_eq!(body["items"][0]["versionName"], "1.2.3");
    assert_eq!(body["items"][1]["package"], "com.missing.app");
    assert_eq!(body["items"][1]["installed"], false);
    assert_eq!(body["items"][1]["userIds"], json!([]));

    let response = get_with_token(&fixture.router, "/api/v1/packages?names=1bad", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn packages_user_filter_and_pagination() {
    let fixture = pm_fixture_device();
    let response = get_with_token(&fixture.router, "/api/v1/packages?user=999", READ_TOKEN).await;
    let body = body_json(response).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["package"], "com.example.app");

    let response = get_with_token(&fixture.router, "/api/v1/packages?limit=1", READ_TOKEN).await;
    let body = body_json(response).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["nextCursor"], "1");

    let response = get_with_token(&fixture.router, "/api/v1/packages?limit=501", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn packages_unavailable_without_pm() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/packages", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["source"], "unavailable");
    assert_eq!(body["total"], 0);
    assert_eq!(body["items"], json!([]));
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "pm_unavailable")
    );
}

#[tokio::test]
async fn device_reports_module_device_server_and_root() {
    let fixture = pm_fixture_device();
    let response = get_with_token(&fixture.router, "/api/v1/device", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["module"]["version"], "v1.7.0");
    assert_eq!(body["device"]["model"], "Pixel 9");
    assert_eq!(
        body["device"]["fingerprint"],
        "google/tokay/tokay:17/BP2A/13000000:user/release-keys"
    );
    assert_eq!(body["server"]["bind"], "127.0.0.1:8686");
    assert_eq!(body["root"]["manager"], serde_json::Value::Null);

    std::fs::create_dir_all(fixture.adb_dir.join("ksu")).unwrap();
    std::fs::write(fixture.adb_dir.join("ksu/version"), "1.0.7\n").unwrap();
    let response = get_with_token(&fixture.router, "/api/v1/device", READ_TOKEN).await;
    let body = body_json(response).await;
    assert_eq!(body["root"]["manager"], "kernelsu");
    assert_eq!(body["root"]["version"], "1.0.7");
}

#[tokio::test]
async fn device_props_respects_whitelist_and_reports_missing() {
    let fixture = pm_fixture_device();
    let response = get_with_token(
        &fixture.router,
        "/api/v1/device/props?keys=ro.product.model,ro.build.fingerprint,ro.product.absent",
        READ_TOKEN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["source"], "getprop");
    assert_eq!(body["props"]["ro.product.model"], "Pixel 9");
    assert!(
        body["props"]["ro.build.fingerprint"]
            .as_str()
            .unwrap()
            .contains("google/tokay")
    );
    assert_eq!(body["missing"], json!(["ro.product.absent"]));

    let response = get_with_token(
        &fixture.router,
        "/api/v1/device/props?keys=persist.sys.foo",
        READ_TOKEN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["error"]["details"]["errors"][0]["path"], "keys");

    let response = get_with_token(&fixture.router, "/api/v1/device/props", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn device_props_returns_503_without_getprop() {
    let fixture = fixture();
    let response = get_with_token(
        &fixture.router,
        "/api/v1/device/props?keys=ro.product.model",
        READ_TOKEN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_json(response).await;
    assert_eq!(body["error"]["details"]["component"], "getprop");
}

#[tokio::test]
async fn device_users_reports_current_user() {
    let fixture = pm_fixture_device();
    let response = get_with_token(&fixture.router, "/api/v1/device/users", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["users"].as_array().unwrap().len(), 2);
    assert_eq!(body["users"][0]["id"], 0);
    assert_eq!(body["users"][0]["name"], "Owner");
    assert_eq!(body["users"][0]["running"], true);
    assert_eq!(body["users"][0]["isCurrent"], true);
    assert_eq!(body["users"][1]["id"], 999);
    assert_eq!(body["users"][1]["isCurrent"], false);
}

#[tokio::test]
async fn packages_and_device_require_read_scope() {
    let fixture = fixture();
    for uri in ["/api/v1/packages", "/api/v1/device", "/api/v1/device/users"] {
        let response = common::get(&fixture.router, uri).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}
