// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 契约读 / 运维端点：templates、overview、audit、token rotate、proxy、jobs 列表。

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use chrono::{DateTime, FixedOffset, SecondsFormat};
use common::{
    TestApp,
    mock_device::{MockApp, MockDevice, MockOptions, MockTemplate},
    send, send_json, send_json_with_headers, send_raw_with_content_type, start_app,
    start_app_with_token,
};
use devmask_manager::state::{TokenSource, TokenState};
use serde_json::{Value, json};

async fn add_device(app: &TestApp, mock: &MockDevice, name: &str) -> Value {
    let response = send_json(
        app,
        Method::POST,
        "/relay/api/v1/devices",
        Some(&app.token),
        json!({ "name": name, "baseUrl": mock.base_url, "token": mock.token }),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    response.json()
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

async fn apply(app: &TestApp, body: Value) -> String {
    let response = send_json(
        app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        body,
    )
    .await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    response.json()["jobId"].as_str().unwrap().to_string()
}

async fn wait_job(app: &TestApp, job_id: &str) -> Value {
    for _ in 0..200 {
        let response = send(
            app,
            Method::GET,
            &format!("/relay/api/v1/jobs/{job_id}"),
            Some(&app.token),
            None,
        )
        .await;
        let body = response.json();
        match body["status"].as_str().unwrap_or_default() {
            "queued" | "running" => tokio::time::sleep(Duration::from_millis(20)).await,
            _ => return body,
        }
    }
    panic!("job {job_id} 未完成");
}

#[tokio::test]
async fn templates_aggregate_shape_filters_and_conflict() {
    let app = start_app(None).await;
    let first = MockDevice::start(MockOptions::default()).await;
    let second = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &first, "Pixel 9").await;
    add_device(&app, &second, "Pixel 9 B").await;

    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/templates",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    assert_eq!(body["total"], 1);
    assert_eq!(body["revision"].as_str().unwrap().len(), 64);
    let template = &body["templates"][0];
    assert_eq!(template["name"], "pixel_9");
    assert_eq!(template["complete"], true);
    assert_eq!(template["config"]["packages"], json!(["com.xingin.xhs"]));
    assert!(template["sources"].as_array().unwrap().len() == 2);
    assert!(
        template["fieldsSet"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "packages")
    );
    assert_eq!(template["warnings"], json!([]));

    let by_device = send(
        &app,
        Method::GET,
        "/relay/api/v1/templates?device=pixel-9",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(by_device["templates"][0]["sources"], json!(["pixel-9"]));

    let by_package = send(
        &app,
        Method::GET,
        "/relay/api/v1/templates?package=com.xingin.xhs",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(by_package["total"], 1);

    let missing = send(
        &app,
        Method::GET,
        "/relay/api/v1/templates?package=com.none.app",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(missing["total"], 0);

    let invalid = send(
        &app,
        Method::GET,
        "/relay/api/v1/templates?limit=0",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn templates_report_conflicts_between_devices() {
    let app = start_app(None).await;
    let first = MockDevice::start(MockOptions::default()).await;
    let conflicting = MockOptions {
        templates: vec![MockTemplate {
            name: "pixel_9".to_string(),
            packages: vec!["com.xingin.xhs".to_string()],
            config: json!({
                "brand": "google",
                "model": "Pixel 9 Pro",
                "android_version": "17",
                "sdk_int": 37,
                "dpi": 440
            }),
        }],
        ..MockOptions::default()
    };
    let second = MockDevice::start(conflicting).await;
    add_device(&app, &first, "Pixel 9").await;
    add_device(&app, &second, "Pixel 9 Pro").await;

    let body = send(
        &app,
        Method::GET,
        "/relay/api/v1/templates",
        Some(&app.token),
        None,
    )
    .await
    .json();

    assert_eq!(body["total"], 1);
    let template = &body["templates"][0];
    assert_eq!(template["name"], "pixel_9");
    assert_eq!(template["warnings"][0]["code"], "template_conflict");
    assert_eq!(
        template["warnings"][0]["details"]["values"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(template["sources"].as_array().unwrap().len(), 2);

    // R9：device filter 必须使用该设备自己的 config/sources，而不是首个设备
    let by_device = send(
        &app,
        Method::GET,
        "/relay/api/v1/templates?device=pixel-9-pro",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(by_device["total"], 1);
    assert_eq!(by_device["templates"][0]["config"]["model"], "Pixel 9 Pro");
    assert_eq!(by_device["templates"][0]["sources"], json!(["pixel-9-pro"]));
    assert_eq!(by_device["templates"][0]["warnings"], json!([]));
}

#[tokio::test]
async fn overview_shape_and_fleet_counts() {
    let app = start_app(None).await;

    let empty = send(
        &app,
        Method::GET,
        "/relay/api/v1/overview",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(empty["fleet"]["devices"], 0);
    assert!(empty["revision"].is_null());
    assert_eq!(empty["devices"], json!([]));
    assert_eq!(empty["drift"], json!([]));
    assert!(
        empty["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "no_devices")
    );

    let pixel = MockDevice::start(MockOptions::default()).await;
    let xiaomi = MockDevice::start(xiaomi_options()).await;
    add_device(&app, &pixel, "Pixel 9").await;
    add_device(&app, &xiaomi, "Xiaomi 15").await;

    let body = send(
        &app,
        Method::GET,
        "/relay/api/v1/overview",
        Some(&app.token),
        None,
    )
    .await
    .json();

    let fleet = &body["fleet"];
    assert_eq!(fleet["devices"], 2);
    assert_eq!(fleet["online"], 2);
    assert_eq!(fleet["offline"], 0);
    assert_eq!(fleet["apps"], 1);
    assert_eq!(fleet["appsConfigured"], 2);
    assert_eq!(fleet["driftApps"], 1);
    assert_eq!(fleet["templates"], 2);
    assert!(fleet["averageLatencyMs"].is_number());
    assert_eq!(fleet["jobs"]["queued"], 0);
    assert_eq!(fleet["jobs"]["running"], 0);
    assert_eq!(fleet["jobs"]["last24h"], 0);

    assert_eq!(body["devices"].as_array().unwrap().len(), 2);
    assert_eq!(body["drift"][0]["package"], "com.xingin.xhs");
    assert_eq!(body["drift"][0]["driftDevices"], json!(["xiaomi-15"]));
    assert_eq!(body["revision"].as_str().unwrap().len(), 64);
    assert_eq!(body["recentJobs"], json!([]));
}

#[tokio::test]
async fn audit_supports_action_target_and_cursor_filters() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let unauthorized = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some("wrong-token"),
        None,
    )
    .await;
    assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);

    let body = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert!(body["total"].as_i64().unwrap() >= 2);
    let entries = body["entries"].as_array().unwrap();
    assert!(entries[0]["id"].as_i64().unwrap() > entries[1]["id"].as_i64().unwrap());
    assert_eq!(entries[0]["action"], "auth.failed");
    assert_eq!(entries[0]["target"]["type"], "token");
    assert!(entries[0]["requestId"].is_string());
    assert_eq!(entries[0]["status"], "failed");

    let by_action = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?action=device.add",
        Some(&app.token),
        None,
    )
    .await
    .json();
    let entries = by_action["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|entry| entry["action"] == "device.add"));
    assert_eq!(entries[0]["target"]["type"], "device");
    assert_eq!(entries[0]["target"]["id"], "pixel-9");
    assert_eq!(entries[0]["status"], "ok");
    assert_eq!(entries[0]["detail"]["online"], true);

    let by_target = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?target=device:pixel-9",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert!(!by_target["entries"].as_array().unwrap().is_empty());
    assert!(
        by_target["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["target"]["id"] == "pixel-9")
    );

    let first_page = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?limit=1",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(first_page["entries"].as_array().unwrap().len(), 1);
    let cursor = first_page["nextCursor"].as_str().unwrap().to_string();
    let second_page = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/audit?limit=1&cursor={cursor}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(second_page["entries"].as_array().unwrap().len(), 1);
    assert_ne!(
        second_page["entries"][0]["id"],
        first_page["entries"][0]["id"]
    );

    let invalid_cursor = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?cursor=not-a-cursor",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(invalid_cursor.status, StatusCode::UNPROCESSABLE_ENTITY);

    let invalid_since = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?since=not-a-time",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(invalid_since.status, StatusCode::UNPROCESSABLE_ENTITY);

    // R8：+08:00 与 Z 表示同一时刻，过滤结果必须一致
    let all = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?limit=50",
        Some(&app.token),
        None,
    )
    .await
    .json();
    let ts = all["entries"][0]["ts"].as_str().unwrap();
    let parsed = DateTime::parse_from_rfc3339(ts).unwrap();
    let plus8 = parsed
        .with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
        .to_rfc3339_opts(SecondsFormat::Secs, false);
    let z_query = ts.replace(':', "%3A");
    let plus_query = plus8.replace('+', "%2B").replace(':', "%3A");
    let by_z = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/audit?limit=50&since={z_query}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    let by_plus = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/audit?limit=50&since={plus_query}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert!(by_z["total"].as_i64().unwrap() > 0);
    assert_eq!(by_z["total"], by_plus["total"]);
}

#[tokio::test]
async fn token_rotate_persists_and_invalidates_old_token() {
    let app = start_app_with_token(
        None,
        TokenState {
            value: "admin123".to_string(),
            source: TokenSource::Default,
        },
    )
    .await;

    let token_file = app.data_dir.join("manager.token");
    let invalid_cases: [(Option<&str>, &[u8], StatusCode); 4] = [
        (
            Some("application/json"),
            b"[]",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            Some("application/json"),
            b"{",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            Some("application/json"),
            br#"{"foo":1}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (Some("text/plain"), b"x", StatusCode::UNSUPPORTED_MEDIA_TYPE),
    ];
    for (content_type, payload, expected) in invalid_cases {
        let invalid = send_raw_with_content_type(
            &app,
            Method::POST,
            "/relay/api/v1/token/rotate",
            Some("admin123"),
            content_type,
            payload,
        )
        .await;
        assert_eq!(invalid.status, expected, "{}", invalid.text());
        assert!(invalid.json()["error"]["code"].is_string());
        assert!(!token_file.exists(), "异常 body 不得触发 Token 轮换");
        let still_valid = send(
            &app,
            Method::GET,
            "/relay/api/v1/devices",
            Some("admin123"),
            None,
        )
        .await;
        assert_eq!(still_valid.status, StatusCode::OK);
    }

    let response = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/token/rotate",
        Some("admin123"),
        &[],
        json!({}),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.header("cache-control"), Some("no-store"));
    let body = response.json();
    let new_token = body["token"].as_str().unwrap().to_string();
    assert!(new_token.starts_with("dfm_"));
    assert_eq!(new_token.len(), 36);
    assert_eq!(body["tokenPersisted"], true);
    assert_eq!(body["source"], "rotated");

    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap().trim(),
        new_token
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode={mode:o}");
    }

    let old = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some("admin123"),
        None,
    )
    .await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    let fresh = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices",
        Some(&new_token),
        None,
    )
    .await;
    assert_eq!(fresh.status, StatusCode::OK);

    let custom = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/token/rotate",
        Some(&new_token),
        &[],
        json!({ "token": "my-long-random-token-123" }),
    )
    .await;
    assert_eq!(custom.status, StatusCode::OK);
    assert_eq!(custom.json()["token"], "my-long-random-token-123");

    let generated = send(
        &app,
        Method::POST,
        "/relay/api/v1/token/rotate",
        Some("my-long-random-token-123"),
        None,
    )
    .await;
    assert_eq!(generated.status, StatusCode::OK, "{}", generated.text());
    assert_eq!(generated.json()["tokenPersisted"], true);

    let audit = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?action=token.rotate",
        Some(generated.json()["token"].as_str().unwrap()),
        None,
    )
    .await
    .json();
    assert!(audit["total"].as_i64().unwrap() >= 3);
}

#[tokio::test]
async fn token_rotate_rejects_cli_or_env_source() {
    let app = start_app(None).await;
    let response = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/token/rotate",
        Some(&app.token),
        &[],
        json!({}),
    )
    .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    let body = response.json();
    assert_eq!(body["error"]["code"], "invalid_state");
    assert_eq!(body["error"]["details"]["reason"], "token_overridden");
}

#[tokio::test]
async fn jobs_list_supports_pagination_and_filters() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let first = apply(
        &app,
        json!({ "app": "com.xingin.xhs", "devices": "all", "template": "pixel_9" }),
    )
    .await;
    wait_job(&app, &first).await;
    let second = apply(
        &app,
        json!({ "app": "com.xingin.xhs", "devices": "all", "template": "pixel_9" }),
    )
    .await;
    wait_job(&app, &second).await;

    let body = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(body["total"], 2);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
    assert_eq!(body["jobs"].as_array().unwrap().len(), 2);
    assert!(body["nextCursor"].is_null());
    assert_eq!(body["items"][0]["status"], "succeeded");

    let first_page = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs?limit=1",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(first_page["items"].as_array().unwrap().len(), 1);
    let cursor = first_page["nextCursor"].as_str().unwrap().to_string();
    let second_page = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/jobs?limit=1&cursor={cursor}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(second_page["items"].as_array().unwrap().len(), 1);
    assert_ne!(second_page["items"][0]["id"], first_page["items"][0]["id"]);

    let filtered = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs?status=succeeded&app=com.xingin.xhs&device=pixel-9",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(filtered["total"], 2);

    let by_kind = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs?kind=bulk",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(by_kind["total"], 2);

    let missing = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs?app=com.none.app",
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(missing["total"], 0);

    let invalid = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs?status=bogus",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn proxy_forwards_methods_bodies_and_status_codes() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let unauthorized = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/echo",
        None,
        None,
    )
    .await;
    assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);

    let get = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/echo?x=1",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(get.status, StatusCode::OK, "{}", get.text());
    assert_eq!(get.header("cache-control"), Some("no-store"));
    let body = get.json();
    assert_eq!(body["method"], "GET");
    assert_eq!(body["query"], "x=1");
    assert!(body["body"].is_null());

    let post = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/echo",
        Some(&app.token),
        &[],
        json!({ "a": 1 }),
    )
    .await;
    assert_eq!(post.status, StatusCode::OK, "{}", post.text());
    let body = post.json();
    assert_eq!(body["method"], "POST");
    assert_eq!(body["body"], json!({ "a": 1 }));

    let delete = send(
        &app,
        Method::DELETE,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/echo",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(delete.status, StatusCode::OK);
    assert_eq!(delete.json()["method"], "DELETE");

    let device_404 = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/templates/nope",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(device_404.status, StatusCode::NOT_FOUND);
    assert_eq!(device_404.json()["error"]["code"], "not_found");

    let device_400 = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/echo?status=400",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(device_400.status, StatusCode::BAD_REQUEST);
    assert_eq!(device_400.json()["error"]["code"], "validation_failed");

    let unknown = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/nope/proxy/api/v1/echo",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.json()["error"]["code"], "device_not_found");

    let traversal = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/../config",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(traversal.status, StatusCode::NOT_FOUND);

    // R10：%3F 必须保持编码在路径中，不能被解码成 query
    let encoded = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/echo%3Fdry_run=1",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(encoded.status, StatusCode::NOT_FOUND, "{}", encoded.text());

    let oversized = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/echo",
        Some(&app.token),
        &[],
        json!({ "blob": "x".repeat(1024 * 1024 + 32) }),
    )
    .await;
    assert_eq!(oversized.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(oversized.json()["error"]["code"], "body_too_large");
}
