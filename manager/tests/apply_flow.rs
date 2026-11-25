// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 批量下发 /apply 与 /jobs/{id} 的集成测试（Mock axum 设备）。

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use common::{
    TestApp,
    mock_device::{MockApp, MockDevice, MockOptions, MockTemplate},
    send, send_json, send_json_with_headers, start_app,
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
        }),
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
            packages: vec!["com.tencent.mm".to_string()],
            config: json!({
                "brand": "Xiaomi",
                "model": "Xiaomi 15",
                "android_version": "15",
                "sdk_int": 35,
                "dpi": 440
            }),
        }],
        apps: vec![MockApp {
            package: "com.tencent.mm".to_string(),
            template: "xiaomi_15".to_string(),
            model: "Xiaomi 15".to_string(),
            installed: true,
            warnings: Vec::new(),
        }],
        ..MockOptions::default()
    }
}

fn pixel_template_config() -> Value {
    json!({
        "brand": "google",
        "model": "Pixel 9",
        "android_version": "17",
        "sdk_int": 37,
        "dpi": 420
    })
}

async fn apply(app: &TestApp, body: Value) -> Value {
    let response = send_json(
        app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        body,
    )
    .await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    response.json()
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
        assert_eq!(response.status, StatusCode::OK, "{}", response.text());
        let body = response.json();
        match body["status"].as_str().unwrap_or_default() {
            "queued" | "running" => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            _ => return body,
        }
    }
    panic!("job {job_id} 未在超时内完成");
}

fn step_names(result: &Value) -> Vec<String> {
    result["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|step| step["step"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn bulk_apply_creates_template_then_binds() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": "all",
            "template": { "name": "new_pixel", "config": pixel_template_config() },
            "restart": true,
        }),
    )
    .await;
    assert_eq!(accepted["status"], "queued");
    assert_eq!(accepted["kind"], "bulk");
    assert_eq!(accepted["total"], 1);
    assert_eq!(accepted["queued"], 1);
    assert!(
        accepted["pollUrl"]
            .as_str()
            .unwrap()
            .starts_with("/relay/api/v1/jobs/")
    );

    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded");
    assert_eq!(job["total"], 1);
    assert_eq!(job["succeeded"], 1);
    assert_eq!(job["skipped"], 0);
    assert_eq!(job["failed"], 0);
    assert_eq!(job["app"], "com.xingin.xhs");
    assert_eq!(job["template"], "new_pixel");
    assert_eq!(job["restart"], true);
    assert!(job["startedAt"].is_string());
    assert!(job["finishedAt"].is_string());
    assert!(job["durationMs"].is_number());

    let result = &job["results"][0];
    assert_eq!(result["deviceId"], "pixel-9");
    assert_eq!(result["deviceName"], "Pixel 9");
    assert_eq!(result["app"], "com.xingin.xhs");
    assert_eq!(result["status"], "ok");
    assert!(result["code"].is_null());
    assert_eq!(result["httpStatus"], 200);
    // R2：模板 PUT 后立即重取 revision，bind 首次即成功
    assert_eq!(result["attempts"], 1);
    assert!(result["revision"].is_string());
    assert_eq!(result["restart"]["status"], "ok");
    let steps = step_names(result);
    for expected in [
        "revision",
        "template_get",
        "template_put",
        "bind",
        "restart",
    ] {
        assert!(steps.contains(&expected.to_string()), "{steps:?}");
    }
}

#[tokio::test]
async fn revision_conflict_refetches_and_retries_once() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        force_first_412: true,
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &mock, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": ["pixel-9"],
            "template": "pixel_9",
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{}", job);

    let result = &job["results"][0];
    assert_eq!(result["status"], "ok");
    assert_eq!(result["attempts"], 2);
    let steps = result["steps"].as_array().unwrap();
    let binds: Vec<&Value> = steps.iter().filter(|step| step["step"] == "bind").collect();
    assert_eq!(binds.len(), 2, "{steps:?}");
    assert_eq!(binds[0]["httpStatus"], 412);
    assert_eq!(binds[0]["code"], "config_revision_mismatch");
    assert_eq!(binds[1]["status"], "ok");
}

#[tokio::test]
async fn package_not_installed_is_skipped() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        missing_packages: vec!["com.missing.app".to_string()],
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &mock, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.missing.app",
            "devices": "all",
            "template": "pixel_9",
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;

    assert_eq!(job["status"], "partial");
    assert_eq!(job["succeeded"], 0);
    assert_eq!(job["skipped"], 1);
    assert_eq!(job["failed"], 0);
    let result = &job["results"][0];
    assert_eq!(result["status"], "skipped");
    assert_eq!(result["code"], "package_not_installed");
    assert_eq!(result["httpStatus"], 404);
    assert_eq!(result["error"]["details"]["resource"], "package");
}

#[tokio::test]
async fn offline_device_is_skipped_without_failing_request() {
    let app = start_app(None).await;
    let pixel = MockDevice::start(MockOptions::default()).await;
    let second = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &pixel, "Pixel 9").await;
    add_device(&app, &second, "Xiaomi 15").await;

    pixel.stop();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": "all",
            "template": "pixel_9",
        }),
    )
    .await;
    assert_eq!(accepted["total"], 2);

    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "partial");
    assert_eq!(job["succeeded"], 1);
    assert_eq!(job["skipped"], 1);
    assert_eq!(job["failed"], 0);

    let results = job["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["deviceId"], "pixel-9");
    assert_eq!(results[0]["status"], "skipped");
    assert_eq!(results[0]["code"], "device_offline");
    assert_eq!(results[1]["deviceId"], "xiaomi-15");
    assert_eq!(results[1]["status"], "ok");
}

#[tokio::test]
async fn assignments_support_per_device_templates() {
    let app = start_app(None).await;
    let pixel = MockDevice::start(MockOptions::default()).await;
    let xiaomi = MockDevice::start(xiaomi_options()).await;
    add_device(&app, &pixel, "Pixel 9").await;
    add_device(&app, &xiaomi, "Xiaomi 15").await;

    let accepted = apply(
        &app,
        json!({
            "assignments": [
                { "device": "pixel-9", "app": "com.xingin.xhs", "template": "pixel_9" },
                { "device": "xiaomi-15", "app": "com.tencent.mm", "template": { "name": "xiaomi_15", "config": { "brand": "Xiaomi", "model": "Xiaomi 15", "android_version": "15", "sdk_int": 35, "dpi": 440 } } }
            ],
            "restart": false,
        }),
    )
    .await;
    assert_eq!(accepted["kind"], "assignments");
    assert_eq!(accepted["total"], 2);

    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["kind"], "assignments");
    assert!(job["app"].is_null());
    assert!(job["template"].is_null());
    assert_eq!(job["status"], "succeeded");
    assert_eq!(job["succeeded"], 2);
    assert_eq!(job["results"][0]["deviceId"], "pixel-9");
    assert_eq!(job["results"][0]["status"], "ok");
    assert_eq!(job["results"][1]["deviceId"], "xiaomi-15");
    assert_eq!(job["results"][1]["status"], "ok");
    assert!(job["results"][1]["restart"].is_null());
}

#[tokio::test]
async fn missing_template_name_is_skipped() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "assignments": [
                { "device": "pixel-9", "app": "com.xingin.xhs", "template": "missing_template" }
            ],
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;

    assert_eq!(job["status"], "partial");
    assert_eq!(job["skipped"], 1);
    let result = &job["results"][0];
    assert_eq!(result["status"], "skipped");
    assert_eq!(result["code"], "template_not_found");
    assert_eq!(result["error"]["details"]["resource"], "template");
}

#[tokio::test]
async fn template_object_overwrites_and_warns() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": "all",
            "template": {
                "name": "pixel_9",
                "config": { "brand": "google", "model": "Pixel 9 Pro", "android_version": "17", "sdk_int": 37, "dpi": 440 }
            },
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded");
    let result = &job["results"][0];
    assert_eq!(result["status"], "ok");
    let warnings = result["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "template_overwritten"),
        "{warnings:?}"
    );
    let steps = step_names(result);
    assert!(steps.contains(&"template_get".to_string()));
    assert!(steps.contains(&"template_put".to_string()));
}

#[tokio::test]
async fn restart_forbidden_only_warns() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        restart_status: Some(403),
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &mock, "Pixel 9").await;

    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": "all",
            "template": "pixel_9",
            "restart": true,
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;

    assert_eq!(job["status"], "succeeded");
    let result = &job["results"][0];
    assert_eq!(result["status"], "ok");
    assert_eq!(result["restart"]["status"], "skipped");
    assert_eq!(result["restart"]["httpStatus"], 403);
    assert!(
        result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "restart_failed")
    );
}

#[tokio::test]
async fn idempotency_key_replays_same_job() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let body = json!({
        "app": "com.xingin.xhs",
        "devices": "all",
        "template": "pixel_9",
    });

    let first = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        &[("idempotency-key", "apply-once")],
        body.clone(),
    )
    .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    let first_job = first.json()["jobId"].as_str().unwrap().to_string();
    let _ = wait_job(&app, &first_job).await;

    let second = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        &[("idempotency-key", "apply-once")],
        body,
    )
    .await;
    assert_eq!(second.status, StatusCode::ACCEPTED);
    assert_eq!(second.header("idempotent-replay"), Some("true"));
    assert_eq!(second.json()["jobId"], first_job);
}

#[tokio::test]
async fn unknown_job_and_unknown_device_are_rejected() {
    let app = start_app(None).await;
    let missing = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs/job_missing",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json()["error"]["code"], "not_found");

    let invalid = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        json!({
            "app": "com.xingin.xhs",
            "devices": ["unknown-device"],
            "template": "pixel_9",
        }),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(invalid.json()["error"]["code"], "validation_failed");
}

#[tokio::test]
async fn apply_writes_bulk_and_assignment_audits() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    // bulk + devices=all
    let all_job = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": "all",
            "template": "pixel_9",
        }),
    )
    .await;
    let all_job_id = all_job["jobId"].as_str().unwrap().to_string();
    wait_job(&app, &all_job_id).await;

    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?action=apply.bulk",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    let text = response.text();
    assert!(!text.contains("admin123"), "审计不得包含设备 Token");
    assert!(!text.contains("test-token"), "审计不得包含 Manager Token");
    let body = response.json();
    let entry = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["detail"]["jobId"].as_str() == Some(all_job_id.as_str()))
        .expect("apply.bulk 审计记录");
    assert_eq!(entry["action"], "apply.bulk");
    assert_eq!(entry["target"]["type"], "device");
    assert_eq!(entry["target"]["id"], "all");
    assert_eq!(entry["detail"]["total"], 1);
    assert_eq!(entry["detail"]["app"], "com.xingin.xhs");
    assert_eq!(entry["detail"]["template"], "pixel_9");
    assert_eq!(entry["status"], "ok");

    // bulk + 显式设备数组
    let multiple_job = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": ["pixel-9"],
            "template": "pixel_9",
        }),
    )
    .await;
    let multiple_job_id = multiple_job["jobId"].as_str().unwrap().to_string();
    wait_job(&app, &multiple_job_id).await;
    let body = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?action=apply.bulk",
        Some(&app.token),
        None,
    )
    .await
    .json();
    let entry = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["detail"]["jobId"].as_str() == Some(multiple_job_id.as_str()))
        .expect("bulk multiple 审计记录");
    assert_eq!(entry["target"]["id"], "multiple");

    // assignments
    let assignment_job = apply(
        &app,
        json!({
            "assignments": [
                { "device": "pixel-9", "app": "com.xingin.xhs", "template": "pixel_9" }
            ],
            "restart": false,
        }),
    )
    .await;
    let assignment_job_id = assignment_job["jobId"].as_str().unwrap().to_string();
    wait_job(&app, &assignment_job_id).await;

    let response = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?action=apply.assignments",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(!response.text().contains("admin123"));
    let body = response.json();
    let entry = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["detail"]["jobId"].as_str() == Some(assignment_job_id.as_str()))
        .expect("apply.assignments 审计记录");
    assert_eq!(entry["action"], "apply.assignments");
    assert_eq!(entry["target"]["type"], "device");
    assert_eq!(entry["target"]["id"], "mixed");
    assert_eq!(entry["detail"]["total"], 1);
    assert_eq!(entry["detail"]["assignments"], 1);
    assert_eq!(entry["status"], "ok");
}

#[tokio::test]
async fn template_object_preserves_existing_packages() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        templates: vec![MockTemplate {
            name: "pixel_9".to_string(),
            packages: vec!["com.other.app".to_string()],
            config: json!({
                "brand": "google",
                "model": "Pixel 9",
                "android_version": "17",
                "sdk_int": 37,
                "dpi": 420
            }),
        }],
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &mock, "Pixel 9").await;

    // Mock 使用真实形状：GET /templates/{name} 的 template 含 packages
    let before = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/templates/pixel_9",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(
        before.json()["template"]["packages"],
        json!(["com.other.app"])
    );

    // 语义相同且未显式给 packages：不应 PUT，也不应覆盖现有 packages
    let accepted = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": "all",
            "template": {
                "name": "pixel_9",
                "config": {
                    "brand": "google",
                    "model": "Pixel 9",
                    "android_version": "17",
                    "sdk_int": 37,
                    "dpi": 420
                }
            },
        }),
    )
    .await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded");
    let result = &job["results"][0];
    assert!(
        !result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "template_overwritten")
    );
    let steps = step_names(result);
    assert!(!steps.contains(&"template_put".to_string()), "{steps:?}");

    let after = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/templates/pixel_9",
        Some(&app.token),
        None,
    )
    .await
    .json();
    let packages = after["template"]["packages"].as_array().unwrap();
    assert!(packages.iter().any(|value| value == "com.other.app"));
    assert!(packages.iter().any(|value| value == "com.xingin.xhs"));

    // 语义不同触发 PUT：仍须保留原 packages 并绑定新应用
    let second = apply(
        &app,
        json!({
            "app": "com.xingin.xhs",
            "devices": "all",
            "template": {
                "name": "pixel_9",
                "config": {
                    "brand": "google",
                    "model": "Pixel 9",
                    "android_version": "17",
                    "sdk_int": 37,
                    "dpi": 440
                }
            },
        }),
    )
    .await;
    let job = wait_job(&app, second["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded");
    let result = &job["results"][0];
    assert!(
        result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "template_overwritten")
    );
    let steps = step_names(result);
    assert!(steps.contains(&"template_put".to_string()), "{steps:?}");

    let after = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/proxy/api/v1/templates/pixel_9",
        Some(&app.token),
        None,
    )
    .await
    .json();
    let packages = after["template"]["packages"].as_array().unwrap();
    assert!(packages.iter().any(|value| value == "com.other.app"));
    assert!(packages.iter().any(|value| value == "com.xingin.xhs"));
}

#[tokio::test]
async fn expired_idempotency_key_replays_instead_of_500() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let body = json!({
        "app": "com.xingin.xhs",
        "devices": "all",
        "template": "pixel_9",
    });

    let first = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        &[("idempotency-key", "old-key")],
        body.clone(),
    )
    .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    let first_job = first.json()["jobId"].as_str().unwrap().to_string();
    wait_job(&app, &first_job).await;

    // 把 job 变老，模拟超过 24h TTL 的重复键
    let job_id = first_job.clone();
    app.state
        .db
        .call(move |connection| {
            connection.execute(
                "UPDATE jobs SET created_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
                rusqlite::params![job_id],
            )
        })
        .await
        .expect("老化 job");

    let second = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        &[("idempotency-key", "old-key")],
        body,
    )
    .await;
    assert_eq!(
        second.status,
        StatusCode::ACCEPTED,
        "过期幂等键不得 500：{}",
        second.text()
    );
    assert_eq!(second.header("idempotent-replay"), Some("true"));
    assert_eq!(second.json()["jobId"], first_job);
}

#[tokio::test]
async fn concurrent_same_idempotency_key_returns_same_job() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let body = json!({
        "app": "com.xingin.xhs",
        "devices": "all",
        "template": "pixel_9",
    });

    let first = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        &[("idempotency-key", "concurrent-key")],
        body.clone(),
    );
    let second = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/apply",
        Some(&app.token),
        &[("idempotency-key", "concurrent-key")],
        body,
    );
    let (first, second) = tokio::join!(first, second);

    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.text());
    assert_eq!(second.status, StatusCode::ACCEPTED, "{}", second.text());
    assert_eq!(first.json()["jobId"], second.json()["jobId"]);
}
