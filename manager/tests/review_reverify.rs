// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! t14 独立复验：R1-R9 修复与关键低危项。
//!
//! 这些用例不复用 t9 的测试文件，直接调用 Manager Router 并检查设备侧状态
//! （Mock 的 app→template、template.packages）或真实响应头/错误信封。

mod common;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::http::{Method, StatusCode};
use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};
use common::{
    TestApp,
    mock_device::{MockApp, MockDevice, MockOptions, MockTemplate},
    send, send_json, send_json_with_headers, send_raw_body, start_app, start_app_with_token,
};
use devmask_manager::state::{TokenSource, TokenState};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

const API: &str = "/relay/api/v1";
const DEVICE_ID: &str = "pixel-1";

fn pixel_config() -> Value {
    json!({
        "brand": "google",
        "model": "Pixel 9",
        "android_version": "17",
        "sdk_int": 37,
        "dpi": 420
    })
}

async fn add_mock(app: &TestApp, mock: &MockDevice, id: &str, name: &str) -> Value {
    let response = send_json(
        app,
        Method::POST,
        &format!("{API}/devices"),
        Some(&app.token),
        json!({
            "id": id,
            "name": name,
            "baseUrl": mock.base_url,
            "token": mock.token,
            "tags": ["reverify"],
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    response.json()
}

async fn apply(app: &TestApp, body: Value) -> Value {
    let response = send_json(
        app,
        Method::POST,
        &format!("{API}/apply"),
        Some(&app.token),
        body,
    )
    .await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    response.json()
}

async fn wait_job(app: &TestApp, job_id: &str) -> Value {
    for _ in 0..300 {
        let response = send(
            app,
            Method::GET,
            &format!("{API}/jobs/{job_id}"),
            Some(&app.token),
            None,
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.text());
        let body = response.json();
        match body["status"].as_str().unwrap_or_default() {
            "queued" | "running" => tokio::time::sleep(Duration::from_millis(20)).await,
            _ => return body,
        }
    }
    panic!("job {job_id} 未完成");
}

fn steps(result: &Value) -> Vec<Value> {
    result["steps"].as_array().cloned().unwrap_or_default()
}

fn step_names(result: &Value) -> Vec<String> {
    steps(result)
        .iter()
        .filter_map(|step| step["step"].as_str().map(str::to_owned))
        .collect()
}

async fn package_bindings(mock: &MockDevice, packages: &[&str]) -> Vec<(String, Option<String>)> {
    packages
        .iter()
        .map(|package| ((*package).to_string(), mock.app_template(package)))
        .collect()
}

// ---------------------------------------------------------------------------
// R1：真实形状 GET template（含 packages）；对象模板下发后其它包仍绑定。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r1_object_template_keeps_other_bindings_and_packages() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        templates: vec![MockTemplate {
            name: "shared".to_string(),
            packages: vec!["com.xingin.xhs".to_string(), "com.other.app".to_string()],
            config: pixel_config(),
        }],
        apps: vec![
            MockApp {
                package: "com.xingin.xhs".to_string(),
                template: "shared".to_string(),
                model: "Pixel 9".to_string(),
                installed: true,
                warnings: Vec::new(),
            },
            MockApp {
                package: "com.other.app".to_string(),
                template: "shared".to_string(),
                model: "Pixel 9".to_string(),
                installed: true,
                warnings: Vec::new(),
            },
        ],
        ..MockOptions::default()
    })
    .await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;

    // GET /templates/{name} 走真实设备形状：packages 独立于 config。
    let before = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/proxy/api/v1/templates/shared"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(before.status, StatusCode::OK, "{}", before.text());
    assert_eq!(
        before.json()["template"]["packages"],
        json!(["com.xingin.xhs", "com.other.app"])
    );

    // 语义相同且请求不带 packages：不 PUT 模板，直接绑定新包。
    let first = apply(
        &app,
        json!({
            "app": "com.third.app",
            "devices": [DEVICE_ID],
            "template": { "name": "shared", "config": pixel_config() },
            "restart": false,
        }),
    )
    .await;
    let job = wait_job(&app, first["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["status"], "ok");
    assert_eq!(result["attempts"], 1);
    assert!(!step_names(result).contains(&"template_put".to_string()));
    assert_eq!(
        package_bindings(&mock, &["com.xingin.xhs", "com.other.app", "com.third.app"]).await,
        vec![
            ("com.xingin.xhs".to_string(), Some("shared".to_string())),
            ("com.other.app".to_string(), Some("shared".to_string())),
            ("com.third.app".to_string(), Some("shared".to_string())),
        ]
    );
    let packages = mock.template_config("shared").unwrap()["packages"]
        .as_array()
        .unwrap()
        .clone();
    for package in ["com.xingin.xhs", "com.other.app", "com.third.app"] {
        assert!(
            packages.iter().any(|value| value == package),
            "{packages:?}"
        );
    }

    // 语义不同触发 PUT：仍合并原 packages，且原有绑定不被推翻。
    let mut changed = pixel_config();
    changed["dpi"] = json!(440);
    let second = apply(
        &app,
        json!({
            "app": "com.fourth.app",
            "devices": [DEVICE_ID],
            "template": { "name": "shared", "config": changed },
            "restart": false,
        }),
    )
    .await;
    let job = wait_job(&app, second["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["attempts"], 1);
    assert!(step_names(result).contains(&"template_put".to_string()));
    assert!(
        result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "template_overwritten")
    );
    assert_eq!(
        package_bindings(
            &mock,
            &[
                "com.xingin.xhs",
                "com.other.app",
                "com.third.app",
                "com.fourth.app"
            ]
        )
        .await
        .iter()
        .filter(|(_, template)| template.as_deref() == Some("shared"))
        .count(),
        4
    );
    let packages = mock.template_config("shared").unwrap()["packages"]
        .as_array()
        .unwrap()
        .clone();
    for package in [
        "com.xingin.xhs",
        "com.other.app",
        "com.third.app",
        "com.fourth.app",
    ] {
        assert!(
            packages.iter().any(|value| value == package),
            "{packages:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// R2：对象模板 PUT 后重取 revision，bind 一次成功；强制 412 只重试一次。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r2_object_template_binds_in_one_attempt() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": [DEVICE_ID],
            "template": { "name": "r2_fresh", "config": pixel_config() },
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["attempts"], 1, "{result}");
    let binds: Vec<Value> = steps(result)
        .into_iter()
        .filter(|step| step["step"] == "bind")
        .collect();
    assert_eq!(binds.len(), 1, "{binds:?}");
    assert_eq!(binds[0]["status"], "ok");
    assert!(step_names(result).contains(&"template_put".to_string()));
    // 模板 PUT 后必须先重新取 revision，再 bind。
    let revisions = steps(result)
        .iter()
        .filter(|step| step["step"] == "revision")
        .count();
    assert!(revisions >= 2, "{:?}", steps(result));
}

#[tokio::test]
async fn r2_forced_412_retries_bind_exactly_once() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        force_first_412: true,
        ..MockOptions::default()
    })
    .await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": [DEVICE_ID],
            "template": { "name": "r2_forced", "config": pixel_config() },
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["attempts"], 2, "{result}");
    let binds: Vec<Value> = steps(result)
        .into_iter()
        .filter(|step| step["step"] == "bind")
        .collect();
    assert_eq!(binds.len(), 2, "{binds:?}");
    assert_eq!(binds[0]["httpStatus"], 412);
    assert_eq!(binds[0]["code"], "config_revision_mismatch");
    assert_eq!(binds[1]["status"], "ok");
}

// ---------------------------------------------------------------------------
// R3：幂等键过期与并发 replay 都不 500，返回同一 job。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r3_expired_idempotency_key_replays_same_job() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;
    let body = json!({
        "app": "com.xingin.xhs",
        "devices": [DEVICE_ID],
        "template": "pixel_9",
    });

    let first = send_json_with_headers(
        &app,
        Method::POST,
        &format!("{API}/apply"),
        Some(&app.token),
        &[("idempotency-key", "r3-expired-key")],
        body.clone(),
    )
    .await;
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.text());
    let job_id = first.json()["jobId"].as_str().unwrap().to_string();
    wait_job(&app, &job_id).await;

    let id = job_id.clone();
    app.state
        .db
        .call(move |connection| {
            connection.execute(
                "UPDATE jobs SET created_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
                rusqlite::params![id],
            )
        })
        .await
        .expect("老化 job");

    let second = send_json_with_headers(
        &app,
        Method::POST,
        &format!("{API}/apply"),
        Some(&app.token),
        &[("idempotency-key", "r3-expired-key")],
        body,
    )
    .await;
    assert_eq!(second.status, StatusCode::ACCEPTED, "{}", second.text());
    assert_eq!(second.header("idempotent-replay"), Some("true"));
    assert_eq!(second.json()["jobId"], job_id);
}

#[tokio::test]
async fn r3_concurrent_idempotency_keys_share_one_job() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;
    let body = json!({
        "app": "com.xingin.xhs",
        "devices": [DEVICE_ID],
        "template": "pixel_9",
    });

    let apply_uri = format!("{API}/apply");
    let first = send_json_with_headers(
        &app,
        Method::POST,
        &apply_uri,
        Some(&app.token),
        &[("idempotency-key", "r3-concurrent-key")],
        body.clone(),
    );
    let second = send_json_with_headers(
        &app,
        Method::POST,
        &apply_uri,
        Some(&app.token),
        &[("idempotency-key", "r3-concurrent-key")],
        body.clone(),
    );
    let third = send_json_with_headers(
        &app,
        Method::POST,
        &apply_uri,
        Some(&app.token),
        &[("idempotency-key", "r3-concurrent-key")],
        body,
    );
    let (first, second, third) = tokio::join!(first, second, third);
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.text());
    assert_eq!(second.status, StatusCode::ACCEPTED, "{}", second.text());
    assert_eq!(third.status, StatusCode::ACCEPTED, "{}", third.text());

    let first_id = first.json()["jobId"].as_str().unwrap().to_string();
    assert_eq!(second.json()["jobId"], first_id);
    assert_eq!(third.json()["jobId"], first_id);
    assert!(first_id.starts_with("job_"));
}

// ---------------------------------------------------------------------------
// R4：200/202/401/405/413 与 token rotate 全部 Cache-Control: no-store。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r4_every_api_response_is_no_store() {
    let app = start_app_with_token(
        None,
        TokenState {
            value: "r4-file-token-1234".to_string(),
            source: TokenSource::File,
        },
    )
    .await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;

    let ok = send(
        &app,
        Method::GET,
        &format!("{API}/devices"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.header("cache-control"), Some("no-store"));

    let accepted = send_json(
        &app,
        Method::POST,
        &format!("{API}/apply"),
        Some(&app.token),
        json!({ "app": "com.xingin.xhs", "devices": [DEVICE_ID], "template": "pixel_9" }),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    assert_eq!(accepted.header("cache-control"), Some("no-store"));

    let unauthorized = send(&app, Method::GET, &format!("{API}/devices"), None, None).await;
    assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauthorized.header("cache-control"), Some("no-store"));

    let root_405 = send(&app, Method::POST, "/healthz", None, None).await;
    assert_eq!(root_405.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(root_405.header("cache-control"), Some("no-store"));
    assert_eq!(root_405.json()["error"]["code"], "method_not_allowed");

    let api_405 = send(
        &app,
        Method::POST,
        &format!("{API}/healthz"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(api_405.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(api_405.header("cache-control"), Some("no-store"));

    let big = vec![b'x'; 1024 * 1024 + 1024];
    let too_large = send_raw_body(
        &app,
        Method::POST,
        &format!("{API}/apply"),
        Some(&app.token),
        Some("application/json"),
        &big,
    )
    .await;
    assert_eq!(too_large.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(too_large.header("cache-control"), Some("no-store"));
    assert_eq!(too_large.json()["error"]["code"], "body_too_large");

    let rotated = send_json(
        &app,
        Method::POST,
        &format!("{API}/token/rotate"),
        Some(&app.token),
        json!({}),
    )
    .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.text());
    assert_eq!(rotated.header("cache-control"), Some("no-store"));
    assert!(
        rotated.json()["token"]
            .as_str()
            .unwrap()
            .starts_with("dfm_")
    );
}

// ---------------------------------------------------------------------------
// R5：rotate 畸形 body 均 4xx JSON，且绝不轮换。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r5_malformed_rotate_bodies_never_rotate() {
    let app = start_app_with_token(
        None,
        TokenState {
            value: "r5-old-token-123456".to_string(),
            source: TokenSource::File,
        },
    )
    .await;
    let token_file = app.data_dir.join("manager.token");

    let cases = [
        ("[]", "application/json", StatusCode::UNPROCESSABLE_ENTITY),
        (
            "{not-json",
            "application/json",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            r#"{"unknownField":true}"#,
            "application/json",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        ("{}", "text/plain", StatusCode::UNSUPPORTED_MEDIA_TYPE),
    ];
    for (body, content_type, expected) in cases {
        let response = send_raw_body(
            &app,
            Method::POST,
            &format!("{API}/token/rotate"),
            Some(&app.token),
            Some(content_type),
            body.as_bytes(),
        )
        .await;
        assert_eq!(response.status, expected, "{body}: {}", response.text());
        let json = response.json();
        assert!(json["error"]["code"].is_string(), "{body}: {json}");
        assert!(json["token"].is_null(), "{body}");
        assert_eq!(response.header("cache-control"), Some("no-store"));
        assert!(
            !token_file.exists(),
            "畸形请求不得写入 manager.token：{body}"
        );

        let still_valid = send(
            &app,
            Method::GET,
            &format!("{API}/devices"),
            Some(&app.token),
            None,
        )
        .await;
        assert_eq!(
            still_valid.status,
            StatusCode::OK,
            "畸形请求后旧 Token 必须仍有效：{body}"
        );
    }

    let rotated = send_json(
        &app,
        Method::POST,
        &format!("{API}/token/rotate"),
        Some(&app.token),
        json!({ "token": "r5-new-token-123456" }),
    )
    .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.text());
    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap().trim(),
        "r5-new-token-123456"
    );
    let old = send(
        &app,
        Method::GET,
        &format!("{API}/devices"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    let fresh = send(
        &app,
        Method::GET,
        &format!("{API}/devices"),
        Some("r5-new-token-123456"),
        None,
    )
    .await;
    assert_eq!(fresh.status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// R6：设备读接口 8 MiB 上限（探测与代理）。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r6_oversized_device_reads_are_rejected() {
    let app = start_app(None).await;
    let huge = MockDevice::start(MockOptions {
        oversized_overview: true,
        ..MockOptions::default()
    })
    .await;
    let probe = send_json(
        &app,
        Method::POST,
        &format!("{API}/devices"),
        Some(&app.token),
        json!({ "id": "huge-1", "name": "Huge", "baseUrl": huge.base_url, "token": huge.token }),
    )
    .await;
    assert_eq!(probe.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(probe.json()["error"]["code"], "device_probe_failed");
    assert_eq!(
        probe.json()["error"]["details"]["reason"],
        "invalid_response"
    );

    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;
    mock.set_oversized_overview(true);
    let proxied = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/proxy/api/v1/overview"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(
        proxied.status,
        StatusCode::BAD_GATEWAY,
        "{}",
        proxied.text()
    );
    let json = proxied.json();
    assert_eq!(json["error"]["code"], "device_error");
    assert_eq!(json["error"]["details"]["reason"], "response_too_large");
}

// ---------------------------------------------------------------------------
// R8：audit since 的 +08:00 与 Z 等价。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r8_audit_since_offset_equals_utc() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;
    let _ = send(
        &app,
        Method::GET,
        &format!("{API}/devices"),
        Some("bad-token"),
        None,
    )
    .await;

    let all = send(
        &app,
        Method::GET,
        &format!("{API}/audit?limit=100"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    let entries = all["entries"].as_array().unwrap();
    assert!(entries.len() >= 2, "{all}");
    let oldest = entries.last().unwrap()["ts"].as_str().unwrap();
    let cutoff = DateTime::parse_from_rfc3339(oldest)
        .unwrap()
        .with_timezone(&Utc)
        + chrono::Duration::seconds(1);
    let utc = cutoff.to_rfc3339_opts(SecondsFormat::Secs, true);
    let plus8 = cutoff
        .with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
        .to_rfc3339_opts(SecondsFormat::Secs, false);
    assert!(plus8.ends_with("+08:00"), "{plus8}");

    let by_utc = send(
        &app,
        Method::GET,
        &format!("{API}/audit?since={utc}&limit=100"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    let encoded_plus = plus8.replace('+', "%2B");
    let by_offset = send(
        &app,
        Method::GET,
        &format!("{API}/audit?since={encoded_plus}&limit=100"),
        Some(&app.token),
        None,
    )
    .await
    .json();

    assert_eq!(by_utc["total"], by_offset["total"]);
    let ids_utc: Vec<i64> = by_utc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_i64().unwrap())
        .collect();
    let ids_offset: Vec<i64> = by_offset["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids_utc, ids_offset);
    assert!(by_utc["total"].as_i64().unwrap() < all["total"].as_i64().unwrap());

    let invalid = send(
        &app,
        Method::GET,
        &format!("{API}/audit?since=not-a-time"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
}

// ---------------------------------------------------------------------------
// R9：/templates?device= 只返回目标设备的 config/sources。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r9_templates_device_filter_returns_that_device_config() {
    let app = start_app(None).await;
    let pixel = MockDevice::start(MockOptions {
        templates: vec![MockTemplate {
            name: "shared".to_string(),
            packages: vec!["com.xingin.xhs".to_string()],
            config: pixel_config(),
        }],
        apps: vec![MockApp {
            package: "com.xingin.xhs".to_string(),
            template: "shared".to_string(),
            model: "Pixel 9".to_string(),
            installed: true,
            warnings: Vec::new(),
        }],
        ..MockOptions::default()
    })
    .await;
    let xiaomi = MockDevice::start(MockOptions {
        model: "Xiaomi 15".to_string(),
        brand: "Xiaomi".to_string(),
        android: "15".to_string(),
        sdk: 35,
        templates: vec![MockTemplate {
            name: "shared".to_string(),
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
            template: "shared".to_string(),
            model: "Xiaomi 15".to_string(),
            installed: true,
            warnings: Vec::new(),
        }],
        ..MockOptions::default()
    })
    .await;
    add_mock(&app, &pixel, "pixel-1", "Pixel 9").await;
    add_mock(&app, &xiaomi, "xiaomi-1", "Xiaomi 15").await;

    let filtered = send(
        &app,
        Method::GET,
        &format!("{API}/templates?device={DEVICE_ID}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    let templates = filtered["templates"].as_array().unwrap();
    assert_eq!(templates.len(), 1, "{filtered}");
    assert_eq!(templates[0]["name"], "shared");
    assert_eq!(templates[0]["sources"], json!([DEVICE_ID]));
    assert_eq!(templates[0]["config"]["model"], "Pixel 9");
    assert_eq!(templates[0]["config"]["brand"], "google");
    assert!(
        templates[0]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|warning| warning["code"] != "template_conflict")
    );

    let all = send(
        &app,
        Method::GET,
        &format!("{API}/templates"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    let shared = all["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|template| template["name"] == "shared")
        .unwrap();
    assert!(
        shared["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "template_conflict")
    );
}

// ---------------------------------------------------------------------------
// 低危：模板 PUT 412 只重试一次；bind 404 template 记 skipped。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn low_template_put_412_retries_once() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        force_template_412_once: true,
        ..MockOptions::default()
    })
    .await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": [DEVICE_ID],
            "template": { "name": "put_retry", "config": pixel_config() },
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let result = &job["results"][0];
    let puts: Vec<Value> = steps(result)
        .into_iter()
        .filter(|step| step["step"] == "template_put")
        .collect();
    assert_eq!(puts.len(), 2, "{puts:?}");
    assert_eq!(puts[0]["httpStatus"], 412);
    assert_eq!(puts[1]["status"], "ok");
}

#[tokio::test]
async fn low_bind_404_template_is_skipped() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        bind_template_404: Some("pixel_9".to_string()),
        ..MockOptions::default()
    })
    .await;
    add_mock(&app, &mock, DEVICE_ID, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": [DEVICE_ID],
            "template": "pixel_9",
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "partial", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["status"], "skipped");
    assert_eq!(result["code"], "template_not_found");
    assert_eq!(result["httpStatus"], 404);
    assert_eq!(result["error"]["details"]["resource"], "template");
}

// ---------------------------------------------------------------------------
// 低危：proxy 保留原始编码路径（%3F 不能变成 query）。
// ---------------------------------------------------------------------------

struct RawDevice {
    base_url: String,
    targets: Arc<Mutex<Vec<String>>>,
    handle: JoinHandle<()>,
}

impl Drop for RawDevice {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn start_raw_device() -> RawDevice {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let targets = Arc::new(Mutex::new(Vec::new()));
    let recorded = targets.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => read,
                    };
                    buffer.extend_from_slice(&chunk[..read]);
                    let Some(header_end) = find_subslice(&buffer, b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                    let request_line = headers.lines().next().unwrap_or_default();
                    let target = request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(str::trim)
                                .map(str::parse::<usize>)
                                .and_then(Result::ok)
                        })
                        .unwrap_or(0);
                    if buffer.len() < header_end + 4 + content_length {
                        continue;
                    }
                    recorded.lock().unwrap().push(target.clone());
                    let path = target.split('?').next().unwrap_or_default();
                    let payload = if path.ends_with("/status") {
                        json!({
                            "module": {"id": "devmask", "version": "v1.9.0"},
                            "device": {
                                "manufacturer": "Google", "brand": "google",
                                "model": "Raw Device", "device": "raw",
                                "android": "17", "sdk": 37, "fingerprint": "raw"
                            },
                            "server": {
                                "version": "1.9.0", "bind": "127.0.0.1:0",
                                "uptimeSec": 1, "startedAt": "2026-11-24T00:00:00Z", "pid": 1
                            }
                        })
                    } else if path.ends_with("/overview") {
                        raw_overview()
                    } else {
                        json!({ "raw_target": target })
                    };
                    let body = payload.to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }
            });
        }
    });
    RawDevice {
        base_url: format!("http://{address}"),
        targets,
        handle,
    }
}

fn raw_overview() -> Value {
    json!({
        "revision": "rev-raw",
        "module": {"id": "devmask", "version": "v1.9.0"},
        "device": {
            "manufacturer": "Google", "brand": "google", "model": "Raw Device",
            "device": "raw", "android": "17", "sdk": 37, "fingerprint": "raw"
        },
        "server": {
            "version": "1.9.0", "bind": "127.0.0.1:0", "uptimeSec": 1,
            "startedAt": "2026-11-24T00:00:00Z", "pid": 1
        },
        "config": {
            "revision": "rev-raw", "mtime": "2026-11-24T00:00:00Z", "size": 1,
            "exists": true, "templates": 0, "apps": 0, "conflicts": 0
        },
        "templates": [],
        "apps": [],
        "warnings": []
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[tokio::test]
async fn low_proxy_preserves_encoded_question_mark_as_path() {
    let app = start_app(None).await;
    let raw = start_raw_device().await;
    let response = send_json(
        &app,
        Method::POST,
        &format!("{API}/devices"),
        Some(&app.token),
        json!({
            "id": "raw-1",
            "name": "Raw Device",
            "baseUrl": raw.base_url,
            "token": "raw-token",
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());

    let proxied = send(
        &app,
        Method::GET,
        &format!("{API}/devices/raw-1/proxy/api/v1/raw%3Fencoded?real=1"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(proxied.status, StatusCode::OK, "{}", proxied.text());
    let target = proxied.json()["raw_target"].as_str().unwrap().to_string();
    assert_eq!(target, "/api/v1/raw%3Fencoded?real=1");
    let recorded = raw.targets.lock().unwrap().clone();
    assert!(
        recorded
            .iter()
            .any(|target| target == "/api/v1/raw%3Fencoded?real=1"),
        "{recorded:?}"
    );
    assert!(
        !recorded
            .iter()
            .any(|target| target.starts_with("/api/v1/raw?")),
        "编码的 %3F 不得变成 query：{recorded:?}"
    );
}

// ---------------------------------------------------------------------------
// R6：无 Content-Length 的 chunked 64 MiB 响应必须提前中止。
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use devmask_manager::device_client::{DeviceCallError, DeviceHttp};

struct ChunkedDevice {
    base_url: String,
    sent: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Drop for ChunkedDevice {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl ChunkedDevice {
    async fn wait_done(&self) {
        for _ in 0..300 {
            if self.done.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("chunked 服务端未在 3s 内结束");
    }
}

async fn start_chunked_device() -> ChunkedDevice {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let sent = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let handle_sent = sent.clone();
    let handle_done = done.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let sent = handle_sent.clone();
            let done = handle_done.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => read,
                    };
                    buffer.extend_from_slice(&chunk[..read]);
                    let Some(header_end) = find_subslice(&buffer, b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                    let target = headers
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let path = target.split('?').next().unwrap_or_default();

                    if path.ends_with("/status") {
                        let body = "{}";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                        let _ = socket.shutdown().await;
                        return;
                    }

                    if path.ends_with("/overview") {
                        let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                        if socket.write_all(head.as_bytes()).await.is_err() {
                            done.store(true, Ordering::SeqCst);
                            return;
                        }
                        let payload = vec![b'x'; 64 * 1024];
                        let size = format!("{:x}\r\n", payload.len());
                        for _ in 0..1024 {
                            if socket.write_all(size.as_bytes()).await.is_err()
                                || socket.write_all(&payload).await.is_err()
                                || socket.write_all(b"\r\n").await.is_err()
                            {
                                done.store(true, Ordering::SeqCst);
                                return;
                            }
                            sent.fetch_add(payload.len(), Ordering::SeqCst);
                        }
                        let _ = socket.write_all(b"0\r\n\r\n").await;
                        done.store(true, Ordering::SeqCst);
                        return;
                    }

                    let body = "{}";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }
            });
        }
    });

    ChunkedDevice {
        base_url: format!("http://{address}"),
        sent,
        done,
        handle,
    }
}

#[tokio::test]
async fn r6_chunked_overview_over_limit_aborts_before_full_read() {
    let app = start_app(None).await;
    let raw = start_chunked_device().await;
    let response = send_json(
        &app,
        Method::POST,
        &format!("{API}/devices"),
        Some(&app.token),
        json!({
            "id": "chunked-huge",
            "name": "Chunked Huge",
            "baseUrl": raw.base_url,
            "token": "device-token",
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.json()["error"]["code"], "device_probe_failed");
    assert_eq!(
        response.json()["error"]["details"]["reason"],
        "invalid_response"
    );

    raw.wait_done().await;
    let sent = raw.sent.load(Ordering::SeqCst);
    assert!(sent > 0, "服务端未写入任何 chunk");
    assert!(
        sent < 64 * 1024 * 1024,
        "客户端读完了 {sent} 字节，超过 8 MiB 上限后仍未中止"
    );
}

#[tokio::test]
async fn r6_chunked_proxy_response_over_limit_is_too_large() {
    let raw = start_chunked_device().await;
    let http = DeviceHttp::new();
    let error = http
        .proxy(
            &raw.base_url,
            "device-token",
            reqwest::Method::GET,
            "overview",
            None,
            &[],
            None,
            Duration::from_secs(5),
            None,
        )
        .await
        .unwrap_err();
    match error {
        DeviceCallError::TooLarge { limit } => {
            assert_eq!(limit, 8 * 1024 * 1024);
        }
        other => panic!("期望 TooLarge，实际 {other:?}"),
    }

    raw.wait_done().await;
    let sent = raw.sent.load(Ordering::SeqCst);
    assert!(sent > 0);
    assert!(sent < 64 * 1024 * 1024, "proxy 读完了 {sent} 字节");
}
