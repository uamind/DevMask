// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 设备注册 / 同步 / overview / apps 矩阵的集成测试（Mock axum 设备）。

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use common::{
    TestApp,
    mock_device::{MockApp, MockDevice, MockOptions, MockTemplate},
    send, send_json, start_app, start_app_with_timeouts,
};
use serde_json::{Value, json};

async fn add_device(app: &TestApp, mock: &MockDevice, name: &str) -> Value {
    let response = send_json(
        app,
        Method::POST,
        "/relay/api/v1/devices",
        Some(&app.token),
        json!({
            "name": name,
            "baseUrl": mock.base_url,
            "token": mock.token,
            "tags": ["mock"],
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    response.json()
}

async fn add_device_with(app: &TestApp, body: Value) -> common::TestResponse {
    send_json(
        app,
        Method::POST,
        "/relay/api/v1/devices",
        Some(&app.token),
        body,
    )
    .await
}

fn xiaomi_options() -> MockOptions {
    MockOptions {
        model: "Xiaomi 15".to_string(),
        brand: "Xiaomi".to_string(),
        android: "15".to_string(),
        sdk: 35,
        templates: vec![MockTemplate {
            name: "xiaomi_15".to_string(),
            packages: vec!["com.xingin.xhs".to_string()],
            config: json!({
                "brand": "Xiaomi",
                "model": "Xiaomi 15",
                "android_version": "15",
                "sdk_int": 35,
                "dpi": 440
            }),
        }],
        apps: vec![MockApp {
            package: "com.xingin.xhs".to_string(),
            template: "xiaomi_15".to_string(),
            model: "Xiaomi 15".to_string(),
            installed: true,
            warnings: Vec::new(),
        }],
        ..MockOptions::default()
    }
}

#[tokio::test]
async fn add_device_persists_metadata_and_never_returns_token() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    let response = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices",
        Some(&app.token),
        json!({
            "name": "Pixel 9",
            "baseUrl": mock.base_url,
            "token": mock.token,
            "tags": ["主力", "内网"],
        }),
    )
    .await;

    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    let body = response.json();
    assert_eq!(body["id"], "pixel-9");
    assert_eq!(body["online"], true);
    assert_eq!(body["model"], "Pixel 9");
    assert_eq!(body["brand"], "google");
    assert_eq!(body["android"], "17");
    assert_eq!(body["sdk"], 37);
    assert_eq!(body["moduleVersion"], "v1.9.0");
    assert_eq!(body["serverVersion"], "1.9.0");
    assert_eq!(body["root"], "kernelsu");
    assert!(body["fingerprint"].as_str().unwrap().contains("tokay"));
    assert!(body["lastSeen"].is_string());
    assert!(body["lastSyncedAt"].is_string());
    assert!(body["latencyMs"].is_number());
    assert!(body["lastError"].is_null());

    assert!(
        !response.text().contains("admin123"),
        "响应中不应出现设备 Token"
    );

    let record = app
        .state
        .db
        .get_device("pixel-9")
        .await
        .unwrap()
        .expect("设备已入库");
    assert!(record.token_cipher.starts_with("v1:"));
    assert_ne!(record.token_cipher, "admin123");
    assert_eq!(
        app.state
            .key
            .decrypt("pixel-9", &record.token_cipher)
            .unwrap()
            .as_str(),
        "admin123"
    );

    let list = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(list.status, StatusCode::OK);
    let list = list.json();
    assert_eq!(list["total"], 1);
    assert_eq!(list["online"], 1);
    assert_eq!(list["devices"][0]["id"], "pixel-9");
    assert!(list["devices"][0].get("token").is_none());
    assert!(list["devices"][0].get("tokenCipher").is_none());
}

#[tokio::test]
async fn duplicate_base_url_and_id_conflict() {
    let app = start_app(None).await;
    let first = MockDevice::start(MockOptions::default()).await;
    let second = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &first, "Pixel 9").await;

    let duplicate_url = add_device_with(
        &app,
        json!({ "name": "Another", "baseUrl": first.base_url, "token": first.token }),
    )
    .await;
    assert_eq!(duplicate_url.status, StatusCode::CONFLICT);
    let body = duplicate_url.json();
    assert_eq!(body["error"]["code"], "device_exists");
    assert_eq!(body["error"]["details"]["field"], "baseUrl");
    assert_eq!(body["error"]["details"]["deviceId"], "pixel-9");

    let duplicate_id = add_device_with(
        &app,
        json!({ "name": "Pixel 9", "baseUrl": second.base_url, "token": second.token, "id": "pixel-9" }),
    )
    .await;
    assert_eq!(duplicate_id.status, StatusCode::CONFLICT);
    assert_eq!(duplicate_id.json()["error"]["details"]["field"], "id");
}

#[tokio::test]
async fn probe_failures_return_422_without_persisting() {
    let app = start_app(None).await;

    let failing_mock = MockDevice::start(MockOptions {
        fail_status: Some(500),
        ..MockOptions::default()
    })
    .await;
    let response = add_device_with(
        &app,
        json!({ "name": "Broken", "baseUrl": failing_mock.base_url, "token": failing_mock.token }),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.json();
    assert_eq!(body["error"]["code"], "device_probe_failed");
    assert_eq!(body["error"]["details"]["reason"], "upstream_status");
    assert_eq!(body["error"]["details"]["upstream"]["status"], 500);

    let mock = MockDevice::start(MockOptions::default()).await;
    let wrong_token = add_device_with(
        &app,
        json!({ "name": "Wrong token", "baseUrl": mock.base_url, "token": "wrong-token" }),
    )
    .await;
    assert_eq!(wrong_token.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        wrong_token.json()["error"]["details"]["reason"],
        "unauthorized"
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = listener.local_addr().unwrap().port();
    drop(listener);
    let offline = add_device_with(
        &app,
        json!({ "name": "Offline", "baseUrl": format!("http://127.0.0.1:{dead_port}"), "token": "x" }),
    )
    .await;
    assert_eq!(offline.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(offline.json()["error"]["details"]["reason"], "connect");

    let list = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(list["total"], 0, "失败探测不得写入设备");
}

#[tokio::test]
async fn device_detail_and_overview_endpoints() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let detail = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK);
    let detail = detail.json();
    assert_eq!(detail["id"], "pixel-9");
    assert_eq!(detail["stale"], false);
    assert!(detail["cachedAt"].is_string());
    assert!(detail["overview"].is_object());
    assert_eq!(detail["templates"][0]["name"], "pixel_9");
    assert_eq!(detail["templates"][0]["sources"][0], "pixel-9");
    assert_eq!(detail["apps"][0]["package"], "com.xingin.xhs");
    assert_eq!(detail["apps"][0]["source"]["type"], "template");
    assert_eq!(detail["apps"][0]["model"], "Pixel 9");
    assert_eq!(detail["warnings"], json!([]));

    let live = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/overview",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(live.status, StatusCode::OK);
    let live = live.json();
    assert_eq!(live["deviceId"], "pixel-9");
    assert_eq!(live["online"], true);
    assert_eq!(live["stale"], false);
    assert_eq!(live["overview"]["device"]["model"], "Pixel 9");

    let cached = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/overview?cached=1",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(cached.status, StatusCode::OK);
    assert_eq!(cached.json()["overview"]["device"]["model"], "Pixel 9");

    let missing = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/nope",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json()["error"]["code"], "device_not_found");
}

#[tokio::test]
async fn sync_refreshes_model_and_cache() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    mock.set_model("Pixel 9 Pro");
    let sync = send(
        &app,
        Method::POST,
        "/relay/api/v1/devices/pixel-9/sync",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(sync.status, StatusCode::OK);
    let sync = sync.json();
    assert_eq!(sync["device"]["model"], "Pixel 9 Pro");
    assert_eq!(sync["device"]["online"], true);
    assert_eq!(sync["warnings"], json!([]));
    assert!(sync["device"]["lastSyncedAt"].is_string());

    let detail = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(detail["model"], "Pixel 9 Pro");
    assert_eq!(detail["apps"][0]["model"], "Pixel 9 Pro");
}

#[tokio::test]
async fn offline_device_is_marked_but_aggregation_still_works() {
    let app = start_app(None).await;
    let pixel = MockDevice::start(MockOptions::default()).await;
    let xiaomi = MockDevice::start(xiaomi_options()).await;
    add_device(&app, &pixel, "Pixel 9").await;
    add_device(&app, &xiaomi, "Xiaomi 15").await;

    pixel.stop();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let sync = send(
        &app,
        Method::POST,
        "/relay/api/v1/devices/pixel-9/sync",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(sync.status, StatusCode::OK, "{}", sync.text());
    let sync = sync.json();
    assert_eq!(sync["device"]["online"], false);
    assert_eq!(sync["warnings"][0]["code"], "device_offline");
    assert!(sync["overview"].is_object(), "离线时保留旧缓存");

    let sync_all = send(
        &app,
        Method::POST,
        "/relay/api/v1/devices/sync",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(sync_all.status, StatusCode::OK);
    let sync_all = sync_all.json();
    assert_eq!(sync_all["synced"], 1);
    assert_eq!(sync_all["failed"], 1);
    assert_eq!(sync_all["results"][0]["deviceId"], "pixel-9");
    assert_eq!(sync_all["results"][0]["status"], "offline");
    assert_eq!(sync_all["results"][1]["status"], "ok");

    let matrix = send(
        &app,
        Method::GET,
        "/relay/api/v1/apps",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(matrix.status, StatusCode::OK);
    let matrix = matrix.json();
    assert_eq!(matrix["totals"]["devices"], 2);
    assert_eq!(matrix["devices"][0]["id"], "pixel-9");
    assert_eq!(matrix["devices"][0]["online"], false);
    assert!(matrix["devices"][0]["stale"].as_bool().unwrap());
    assert_eq!(matrix["apps"][0]["cells"]["pixel-9"]["model"], "Pixel 9");
    assert_eq!(matrix["apps"][0]["offlineDevices"], json!(["pixel-9"]));
}

#[tokio::test]
async fn apps_matrix_flags_drift_and_aggregates_revision() {
    let app = start_app(None).await;
    let pixel = MockDevice::start(MockOptions::default()).await;
    let xiaomi = MockDevice::start(xiaomi_options()).await;
    add_device(&app, &pixel, "Pixel 9").await;
    add_device(&app, &xiaomi, "Xiaomi 15").await;

    let matrix = send(
        &app,
        Method::GET,
        "/relay/api/v1/apps",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(matrix.status, StatusCode::OK);
    let matrix = matrix.json();

    assert_eq!(matrix["totals"]["devices"], 2);
    assert_eq!(matrix["totals"]["apps"], 1);
    assert_eq!(matrix["totals"]["appsConfigured"], 2);
    assert_eq!(matrix["totals"]["driftApps"], 1);
    assert_eq!(matrix["revision"].as_str().unwrap().len(), 64);

    let row = &matrix["apps"][0];
    assert_eq!(row["package"], "com.xingin.xhs");
    assert_eq!(row["configured"], 2);
    assert_eq!(row["total"], 2);
    assert_eq!(row["inconsistent"], true);
    assert_eq!(row["models"], json!(["Pixel 9", "Xiaomi 15"]));
    assert_eq!(row["cells"]["pixel-9"]["source"], "template");
    assert_eq!(row["cells"]["pixel-9"]["template"], "pixel_9");
    assert_eq!(row["cells"]["pixel-9"]["drift"], false);
    assert_eq!(row["cells"]["xiaomi-15"]["model"], "Xiaomi 15");
    assert_eq!(row["cells"]["xiaomi-15"]["template"], "xiaomi_15");
    assert_eq!(row["cells"]["xiaomi-15"]["drift"], true);
    assert!(
        row["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "drift")
    );

    let drift_only = send(
        &app,
        Method::GET,
        "/relay/api/v1/apps?drift=true",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(drift_only["apps"].as_array().unwrap().len(), 1);

    let filtered = send(
        &app,
        Method::GET,
        "/relay/api/v1/apps?q=xingin&device=xiaomi-15",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(filtered["apps"].as_array().unwrap().len(), 1);

    let no_match = send(
        &app,
        Method::GET,
        "/relay/api/v1/apps?device=pixel-9&q=not-installed",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(no_match["apps"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn apps_without_devices_returns_empty_matrix_with_warning() {
    let app = start_app(None).await;
    let matrix = send(
        &app,
        Method::GET,
        "/relay/api/v1/apps",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(matrix.status, StatusCode::OK);
    let matrix = matrix.json();
    assert_eq!(matrix["devices"], json!([]));
    assert_eq!(matrix["apps"], json!([]));
    assert_eq!(matrix["totals"]["devices"], 0);
    assert!(matrix["revision"].is_null());
    assert!(
        matrix["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "no_devices")
    );
}

#[tokio::test]
async fn delete_removes_device_from_list_and_matrix() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let deleted = send(
        &app,
        Method::DELETE,
        "/relay/api/v1/devices/pixel-9",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);

    let list = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(list["total"], 0);

    let matrix = send(
        &app,
        Method::GET,
        "/relay/api/v1/apps",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(matrix["apps"], json!([]));
    assert_eq!(matrix["totals"]["devices"], 0);

    let detail = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_filters_and_pagination() {
    let app = start_app(None).await;
    let alpha = MockDevice::start(MockOptions::default()).await;
    let beta = MockDevice::start(xiaomi_options()).await;
    add_device(&app, &alpha, "Alpha").await;
    add_device(&app, &beta, "Beta").await;

    let by_tag = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices?tag=mock",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(by_tag["total"], 2);

    let no_tag = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices?tag=missing",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(no_tag["total"], 0);

    let by_query = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices?q=xiaomi",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(by_query["total"], 1);
    assert_eq!(by_query["devices"][0]["id"], "beta");

    let offline = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices?online=false",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(offline["total"], 0);

    let first_page = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices?limit=1",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(first_page["total"], 2);
    assert_eq!(first_page["devices"].as_array().unwrap().len(), 1);
    let cursor = first_page["nextCursor"].as_str().unwrap().to_string();

    let second_page = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/devices?limit=1&cursor={cursor}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(second_page["devices"].as_array().unwrap().len(), 1);
    assert_ne!(
        second_page["devices"][0]["id"],
        first_page["devices"][0]["id"]
    );
    assert!(second_page["nextCursor"].is_null());

    let invalid = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices?cursor=not-a-cursor",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(invalid.json()["error"]["code"], "validation_failed");
}

#[tokio::test]
async fn patch_updates_metadata_and_rejects_bad_token() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let patched = send_json(
        &app,
        Method::PATCH,
        "/relay/api/v1/devices/pixel-9",
        Some(&app.token),
        json!({ "name": "Renamed", "tags": ["x", "y"] }),
    )
    .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    assert_eq!(patched.json()["name"], "Renamed");
    assert_eq!(patched.json()["tags"], json!(["x", "y"]));

    let bad_token = send_json(
        &app,
        Method::PATCH,
        "/relay/api/v1/devices/pixel-9",
        Some(&app.token),
        json!({ "token": "wrong-token" }),
    )
    .await;
    assert_eq!(bad_token.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad_token.json()["error"]["code"], "device_probe_failed");

    let sync = send(
        &app,
        Method::POST,
        "/relay/api/v1/devices/pixel-9/sync",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(sync.status, StatusCode::OK);
    assert_eq!(sync.json()["device"]["online"], true, "旧 Token 仍然有效");
}

#[tokio::test]
async fn request_timeout_marks_device_offline() {
    let app = start_app_with_timeouts(None, 120).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    mock.set_delay(Some(Duration::from_millis(600)));
    let sync = send(
        &app,
        Method::POST,
        "/relay/api/v1/devices/pixel-9/sync",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(sync.status, StatusCode::OK);
    let sync = sync.json();
    assert_eq!(sync["device"]["online"], false);
    assert_eq!(sync["warnings"][0]["code"], "device_offline");

    mock.set_delay(None);
    let recovered = send(
        &app,
        Method::POST,
        "/relay/api/v1/devices/pixel-9/sync",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(recovered["device"]["online"], true);
}

#[tokio::test]
async fn unknown_fields_are_rejected() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    let response = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices",
        Some(&app.token),
        json!({
            "name": "Pixel 9",
            "baseUrl": mock.base_url,
            "token": mock.token,
            "endpoint": "unexpected",
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.json();
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "unknown_field"
    );
}

#[tokio::test]
async fn oversized_device_overview_is_rejected() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        oversized_overview: true,
        ..MockOptions::default()
    })
    .await;
    let response = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices",
        Some(&app.token),
        json!({
            "name": "Huge",
            "baseUrl": mock.base_url,
            "token": mock.token,
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.json()["error"]["code"], "device_probe_failed");
    assert_eq!(
        response.json()["error"]["details"]["reason"],
        "invalid_response"
    );

    let list = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(list["total"], 0);
}
