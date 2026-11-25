// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 端到端验收：一个 Manager 实例 + Mock 设备，按 RUN.md 的顺序走完全部 18 个端点。
//!
//! 覆盖点：
//! - 添加 / 探测 / 列表 / 详情 / 单台同步 / 全量同步 / overview（实时 + 缓存）
//! - 应用矩阵、模板并集、总览
//! - bulk apply（模板对象 → PUT 模板 → 首次 bind 412 → 重试一次）与 job 轮询
//! - GET /jobs 的 items 规范字段与 jobs 兼容别名
//! - 代理 GET / POST / DELETE（Mock 的 /api/v1/echo）
//! - 审计（device.add、device.sync_all、job.finished、token.rotate）
//! - Token 轮换后旧 Token 401、新 Token 200
//! - 删除设备与删除后的空聚合
//! - 设备离线时 overview 实时 502 / 代理 502 / 缓存仍 200 且 stale
//! - apply 参数校验（空目标、重复设备、未注册设备、模板缺 model）

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use common::{
    TestApp,
    mock_device::{MockDevice, MockOptions},
    send, send_json, start_app, start_app_with_token,
};
use devmask_manager::state::{TokenSource, TokenState};
use serde_json::{Value, json};

const API: &str = "/relay/api/v1";
const DEVICE_ID: &str = "mock-pixel-9";

async fn add_mock_device(app: &TestApp, mock: &MockDevice) -> Value {
    let response = send_json(
        app,
        Method::POST,
        &format!("{API}/devices"),
        Some(&app.token),
        json!({
            "name": "Mock Pixel 9",
            "baseUrl": mock.base_url,
            "token": mock.token,
            "tags": ["mock", "e2e"],
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
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
    panic!("job {job_id} 未在超时内完成");
}

async fn wait_audit_action(app: &TestApp, token: &str, action: &str) -> Value {
    for _ in 0..100 {
        let body = send(
            app,
            Method::GET,
            &format!("{API}/audit?action={action}&limit=10"),
            Some(token),
            None,
        )
        .await
        .json();
        if body["total"].as_i64().unwrap_or(0) > 0 {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("审计中未出现 action={action}");
}

fn step_names(result: &Value) -> Vec<String> {
    result["steps"]
        .as_array()
        .expect("steps 数组")
        .iter()
        .filter_map(|step| step["step"].as_str().map(str::to_owned))
        .collect()
}

/// 全流程：添加 → 同步 → 聚合 → 下发 → job → 代理 → 审计 → 轮换 → 删除。
#[tokio::test]
async fn full_lifecycle_covers_all_relay_endpoints() {
    // source=File 允许 /token/rotate（Cli/Env 来源按契约返回 409）。
    let app = start_app_with_token(
        None,
        TokenState {
            value: "test-token".to_string(),
            source: TokenSource::File,
        },
    )
    .await;
    let mock = MockDevice::start(MockOptions::default()).await;

    // 1) /healthz：无鉴权、空注册表。
    let health = send(&app, Method::GET, &format!("{API}/healthz"), None, None).await;
    assert_eq!(health.status, StatusCode::OK);
    let health = health.json();
    assert_eq!(health["status"], "ok");
    assert_eq!(health["apiVersion"], "v1");
    assert_eq!(health["db"]["schemaVersion"], 2);
    assert_eq!(health["devices"]["total"], 0);
    let root_health = send(&app, Method::GET, "/healthz", None, None).await;
    assert_eq!(root_health.status, StatusCode::OK);

    // 2) 未带 Token：统一 401 信封。
    let unauthorized = send(&app, Method::GET, &format!("{API}/devices"), None, None).await;
    assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauthorized.json()["error"]["code"], "unauthorized");
    assert!(unauthorized.header("www-authenticate").is_some());

    // 3) 添加设备：探测 status + overview，201，Token 不回显。
    let created = add_mock_device(&app, &mock).await;
    assert_eq!(created["id"], DEVICE_ID);
    assert_eq!(created["name"], "Mock Pixel 9");
    assert_eq!(created["online"], true);
    assert_eq!(created["model"], "Pixel 9");
    assert_eq!(created["brand"], "google");
    assert_eq!(created["android"], "17");
    assert_eq!(created["sdk"], 37);
    assert_eq!(created["tags"], json!(["mock", "e2e"]));
    assert!(created["latencyMs"].as_i64().is_some());
    assert!(!created.to_string().contains(&mock.token), "Token 不得回显");

    // 4) 列表与详情。
    let list = send(
        &app,
        Method::GET,
        &format!("{API}/devices"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(list["total"], 1);
    assert_eq!(list["online"], 1);
    assert_eq!(list["offline"], 0);
    assert_eq!(list["devices"][0]["id"], DEVICE_ID);

    let detail = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK);
    let detail = detail.json();
    assert_eq!(detail["id"], DEVICE_ID);
    assert_eq!(detail["stale"], false);
    assert!(detail["cachedAt"].is_string());
    assert_eq!(detail["templates"][0]["name"], "pixel_9");
    assert_eq!(detail["templates"][0]["sources"][0], DEVICE_ID);
    assert_eq!(detail["apps"][0]["package"], "com.xingin.xhs");
    assert_eq!(detail["apps"][0]["source"]["type"], "template");

    // 5) PATCH 元数据（标签），不触发重新探测。
    let patched = send_json(
        &app,
        Method::PATCH,
        &format!("{API}/devices/{DEVICE_ID}"),
        Some(&app.token),
        json!({ "tags": ["mock", "e2e", "patched"] }),
    )
    .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    assert_eq!(patched.json()["tags"], json!(["mock", "e2e", "patched"]));

    // 6) 单台同步与全量同步。
    let sync_one = send(
        &app,
        Method::POST,
        &format!("{API}/devices/{DEVICE_ID}/sync"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(sync_one.status, StatusCode::OK);
    let sync_one = sync_one.json();
    assert_eq!(sync_one["device"]["online"], true);
    assert_eq!(sync_one["warnings"], json!([]));

    let sync_all = send(
        &app,
        Method::POST,
        &format!("{API}/devices/sync"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(sync_all.status, StatusCode::OK);
    let sync_all = sync_all.json();
    assert_eq!(sync_all["synced"], 1);
    assert_eq!(sync_all["failed"], 0);
    assert_eq!(sync_all["results"][0]["deviceId"], DEVICE_ID);
    assert_eq!(sync_all["results"][0]["status"], "ok");

    // 7) overview：实时与缓存两条路径一致。
    let live = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/overview"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(live.status, StatusCode::OK);
    let live = live.json();
    assert_eq!(live["online"], true);
    assert_eq!(live["stale"], false);
    assert_eq!(live["overview"]["device"]["model"], "Pixel 9");
    let cached = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/overview?cached=1"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(cached["overview"]["device"]["model"], "Pixel 9");

    // 8) 应用矩阵与模板并集。
    let matrix = send(
        &app,
        Method::GET,
        &format!("{API}/apps"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(matrix["totals"]["devices"], 1);
    assert_eq!(matrix["totals"]["apps"], 1);
    assert_eq!(matrix["apps"][0]["package"], "com.xingin.xhs");
    assert_eq!(matrix["apps"][0]["configured"], 1);
    assert_eq!(matrix["apps"][0]["total"], 1);
    assert_eq!(matrix["apps"][0]["inconsistent"], false);
    let cell = &matrix["apps"][0]["cells"][DEVICE_ID];
    assert_eq!(cell["source"], "template");
    assert_eq!(cell["template"], "pixel_9");
    assert_eq!(cell["model"], "Pixel 9");

    let templates = send(
        &app,
        Method::GET,
        &format!("{API}/templates"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(templates["total"], 1);
    assert_eq!(templates["templates"][0]["name"], "pixel_9");
    assert_eq!(templates["templates"][0]["complete"], true);
    assert_eq!(templates["templates"][0]["sources"], json!([DEVICE_ID]));

    // 9) 总览：下发前没有 job。
    let overview = send(
        &app,
        Method::GET,
        &format!("{API}/overview"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(overview["fleet"]["devices"], 1);
    assert_eq!(overview["fleet"]["online"], 1);
    assert_eq!(overview["fleet"]["apps"], 1);
    assert_eq!(overview["fleet"]["templates"], 1);
    assert_eq!(overview["drift"], json!([]));
    assert_eq!(overview["recentJobs"], json!([]));

    // 10) bulk apply：新模板对象 → PUT 模板推进 revision → 首次 bind 412 → 重试成功。
    let accepted = send_json(
        &app,
        Method::POST,
        &format!("{API}/apply"),
        Some(&app.token),
        json!({
            "app": "com.xingin.xhs",
            "devices": "all",
            "template": {
                "name": "pixel_9_pro",
                "config": {
                    "brand": "google",
                    "model": "Pixel 9 Pro",
                    "android_version": "17",
                    "sdk_int": 37,
                    "dpi": 440
                }
            },
            "restart": false,
        }),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text());
    let accepted = accepted.json();
    assert_eq!(accepted["status"], "queued");
    assert_eq!(accepted["kind"], "bulk");
    assert_eq!(accepted["total"], 1);
    assert_eq!(accepted["queued"], 1);
    assert!(
        accepted["pollUrl"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{API}/jobs/"))
    );

    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["succeeded"], 1);
    assert_eq!(job["skipped"], 0);
    assert_eq!(job["failed"], 0);
    assert_eq!(job["app"], "com.xingin.xhs");
    assert_eq!(job["template"], "pixel_9_pro");
    assert!(job["requestId"].is_string());
    let result = &job["results"][0];
    assert_eq!(result["deviceId"], DEVICE_ID);
    assert_eq!(result["deviceName"], "Mock Pixel 9");
    assert_eq!(result["status"], "ok");
    // R2 修复后：模板 PUT 完成即重取 revision，bind 首次成功，不再无谓 412 重试。
    assert_eq!(result["attempts"], 1, "{result}");
    let steps = step_names(result);
    for expected in ["revision", "template_get", "template_put", "bind"] {
        assert!(steps.contains(&expected.to_string()), "{steps:?}");
    }

    // 11) job 列表：items 为规范字段，jobs 为兼容别名，内容一致。
    let jobs = send(
        &app,
        Method::GET,
        &format!("{API}/jobs?status=succeeded&limit=5"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(jobs["total"], 1);
    assert_eq!(jobs["items"].as_array().unwrap().len(), 1);
    assert_eq!(jobs["items"], jobs["jobs"]);
    assert_eq!(jobs["items"][0]["id"], job["id"]);
    assert_eq!(jobs["items"][0]["status"], "succeeded");

    let unknown_job = send(
        &app,
        Method::GET,
        &format!("{API}/jobs/job_missing"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(unknown_job.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown_job.json()["error"]["code"], "not_found");

    // 12) 代理：GET / POST / DELETE 透传。
    let proxied = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/proxy/api/v1/echo?x=1"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(proxied.status, StatusCode::OK, "{}", proxied.text());
    let proxied = proxied.json();
    assert_eq!(proxied["method"], "GET");
    assert_eq!(proxied["query"], "x=1");

    let posted = send_json(
        &app,
        Method::POST,
        &format!("{API}/devices/{DEVICE_ID}/proxy/api/v1/echo"),
        Some(&app.token),
        json!({ "hello": "device" }),
    )
    .await;
    assert_eq!(posted.status, StatusCode::OK, "{}", posted.text());
    assert_eq!(posted.json()["body"], json!({ "hello": "device" }));

    let deleted = send(
        &app,
        Method::DELETE,
        &format!("{API}/devices/{DEVICE_ID}/proxy/api/v1/echo"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(deleted.json()["method"], "DELETE");

    // 13) 审计：添加 / PATCH / 全量同步 / 下发 / job 完成各一条，且不含 Token。
    let added = wait_audit_action(&app, &app.token, "device.add").await;
    assert_eq!(added["entries"][0]["target"]["id"], DEVICE_ID);
    assert_eq!(added["entries"][0]["status"], "ok");
    let synced = wait_audit_action(&app, &app.token, "device.sync_all").await;
    assert_eq!(synced["entries"][0]["status"], "ok");
    let updated = wait_audit_action(&app, &app.token, "device.update").await;
    assert_eq!(updated["entries"][0]["target"]["id"], DEVICE_ID);
    let finished = wait_audit_action(&app, &app.token, "job.finished").await;
    assert_eq!(finished["entries"][0]["target"]["type"], "job");
    assert_eq!(finished["entries"][0]["detail"]["status"], "succeeded");
    assert!(
        !finished.to_string().contains(&mock.token),
        "审计不得含设备 Token"
    );
    let bulk_audit = wait_audit_action(&app, &app.token, "apply.bulk").await;
    assert_eq!(bulk_audit["entries"][0]["target"]["type"], "device");
    assert_eq!(bulk_audit["entries"][0]["target"]["id"], "all");
    assert_eq!(bulk_audit["entries"][0]["detail"]["jobId"], job["id"]);
    assert_eq!(bulk_audit["entries"][0]["detail"]["total"], 1);

    // 14) Token 轮换：响应只返回一次新值，旧 Token 立即 401。
    let rotated = send_json(
        &app,
        Method::POST,
        &format!("{API}/token/rotate"),
        Some(&app.token),
        json!({}),
    )
    .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.text());
    let rotated = rotated.json();
    let new_token = rotated["token"].as_str().unwrap().to_string();
    assert!(new_token.starts_with("dfm_"));
    assert_eq!(new_token.len(), 36);
    assert_eq!(rotated["tokenPersisted"], true);
    assert_eq!(rotated["source"], "rotated");
    let token_file = app.data_dir.join("manager.token");
    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap().trim(),
        new_token
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
        Some(&new_token),
        None,
    )
    .await;
    assert_eq!(fresh.status, StatusCode::OK);

    let custom = send_json(
        &app,
        Method::POST,
        &format!("{API}/token/rotate"),
        Some(&new_token),
        json!({ "token": "my-long-random-token-123" }),
    )
    .await;
    assert_eq!(custom.status, StatusCode::OK);
    assert_eq!(custom.json()["token"], "my-long-random-token-123");
    let too_short = send_json(
        &app,
        Method::POST,
        &format!("{API}/token/rotate"),
        Some("my-long-random-token-123"),
        json!({ "token": "short" }),
    )
    .await;
    assert_eq!(too_short.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(too_short.json()["error"]["code"], "validation_failed");

    let rotated_audit = wait_audit_action(&app, "my-long-random-token-123", "token.rotate").await;
    assert!(rotated_audit["total"].as_i64().unwrap() >= 2);
    assert!(
        !rotated_audit
            .to_string()
            .contains("my-long-random-token-123"),
        "审计不得含 Manager Token"
    );

    // 15) 删除设备：204，聚合回到空态。
    let removed = send(
        &app,
        Method::DELETE,
        &format!("{API}/devices/{DEVICE_ID}"),
        Some("my-long-random-token-123"),
        None,
    )
    .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    let after_delete = send(
        &app,
        Method::GET,
        &format!("{API}/devices"),
        Some("my-long-random-token-123"),
        None,
    )
    .await
    .json();
    assert_eq!(after_delete["total"], 0);
    let empty_matrix = send(
        &app,
        Method::GET,
        &format!("{API}/apps"),
        Some("my-long-random-token-123"),
        None,
    )
    .await
    .json();
    assert_eq!(empty_matrix["totals"]["apps"], 0);
    assert!(
        empty_matrix["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "no_devices")
    );
    let final_health = send(&app, Method::GET, &format!("{API}/healthz"), None, None)
        .await
        .json();
    assert_eq!(final_health["devices"]["total"], 0);
}

/// 设备离线：实时 overview / 代理都返回 502，缓存仍可读且 stale。
#[tokio::test]
async fn offline_device_degrades_to_502_but_keeps_cache_readable() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock_device(&app, &mock).await;

    mock.stop();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let live = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/overview"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(live.status, StatusCode::BAD_GATEWAY, "{}", live.text());
    let live = live.json();
    assert_eq!(live["error"]["code"], "device_offline");
    assert_eq!(live["error"]["details"]["deviceId"], DEVICE_ID);
    assert!(live["error"]["retryable"].as_bool().unwrap());

    let cached = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/overview?cached=1"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(cached.status, StatusCode::OK, "{}", cached.text());
    let cached = cached.json();
    assert_eq!(cached["online"], false);
    assert_eq!(cached["stale"], true);
    assert_eq!(cached["overview"]["device"]["model"], "Pixel 9");

    let proxy = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/proxy/api/v1/echo"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(proxy.status, StatusCode::BAD_GATEWAY, "{}", proxy.text());
    assert_eq!(proxy.json()["error"]["code"], "device_offline");

    // 聚合视图不因单台离线而 5xx。
    let matrix = send(
        &app,
        Method::GET,
        &format!("{API}/apps"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(matrix.status, StatusCode::OK, "{}", matrix.text());
    let matrix = matrix.json();
    assert_eq!(matrix["devices"][0]["online"], false);
    assert_eq!(matrix["devices"][0]["stale"], true);
    assert_eq!(matrix["apps"][0]["offlineDevices"], json!([DEVICE_ID]));

    let sync_all = send(
        &app,
        Method::POST,
        &format!("{API}/devices/sync"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(sync_all.status, StatusCode::OK);
    let sync_all = sync_all.json();
    assert_eq!(sync_all["synced"], 0);
    assert_eq!(sync_all["failed"], 1);
    assert_eq!(sync_all["results"][0]["status"], "offline");
    assert_eq!(sync_all["results"][0]["code"], "device_offline");
}

/// 设备 Token 失配：代理把设备 401 重映射为 502 device_error，避免前端误判 Manager 登录失效。
#[tokio::test]
async fn device_token_mismatch_is_remapped_by_proxy() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock_device(&app, &mock).await;

    mock.set_token("rotated-device-token");

    let proxy = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/proxy/api/v1/echo"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(proxy.status, StatusCode::BAD_GATEWAY, "{}", proxy.text());
    let proxy = proxy.json();
    assert_eq!(proxy["error"]["code"], "device_error");
    assert_eq!(proxy["error"]["details"]["deviceId"], DEVICE_ID);
    assert_eq!(proxy["error"]["details"]["upstreamStatus"], 401);
    assert_eq!(proxy["error"]["details"]["upstreamCode"], "unauthorized");
    assert_eq!(proxy["error"]["retryable"], true);

    // 实时 overview 同样映射为 device_error，并标记离线；聚合继续读缓存。
    let live = send(
        &app,
        Method::GET,
        &format!("{API}/devices/{DEVICE_ID}/overview"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(live.status, StatusCode::BAD_GATEWAY);
    assert_eq!(live.json()["error"]["code"], "device_error");
    let matrix = send(
        &app,
        Method::GET,
        &format!("{API}/apps"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(matrix.status, StatusCode::OK);
}

/// apply 入参归一化：空目标 / 重复设备 / 未注册设备 / 模板缺 model 全部 422。
#[tokio::test]
async fn apply_validation_rejects_empty_duplicate_and_unknown_targets() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_mock_device(&app, &mock).await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "empty list",
            json!({
                "app": "com.xingin.xhs",
                "devices": [],
                "template": "pixel_9",
            }),
            "devices",
        ),
        (
            "not all literal",
            json!({
                "app": "com.xingin.xhs",
                "devices": "everyone",
                "template": "pixel_9",
            }),
            "devices",
        ),
        (
            "unknown device",
            json!({
                "app": "com.xingin.xhs",
                "devices": ["ghost"],
                "template": "pixel_9",
            }),
            "devices[0]",
        ),
        (
            "duplicate device",
            json!({
                "app": "com.xingin.xhs",
                "devices": [DEVICE_ID, DEVICE_ID],
                "template": "pixel_9",
            }),
            "devices[1]",
        ),
        (
            "template without model",
            json!({
                "app": "com.xingin.xhs",
                "devices": "all",
                "template": { "name": "pixel_9", "config": { "brand": "google" } },
            }),
            "template.config.model",
        ),
        (
            "empty assignments",
            json!({ "assignments": [] }),
            "assignments",
        ),
        (
            "unknown assignment device",
            json!({
                "assignments": [
                    { "device": "ghost", "app": "com.xingin.xhs", "template": "pixel_9" }
                ],
            }),
            "assignments[0].device",
        ),
    ];

    for (label, body, path) in cases {
        let response = send_json(
            &app,
            Method::POST,
            &format!("{API}/apply"),
            Some(&app.token),
            body,
        )
        .await;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label}: {}",
            response.text()
        );
        let body = response.json();
        assert_eq!(body["error"]["code"], "validation_failed", "{label}");
        let errors = body["error"]["details"]["errors"].as_array().unwrap();
        assert!(
            errors.iter().any(|error| error["path"] == path),
            "{label}: {errors:?}"
        );
    }

    let no_token = send_json(
        &app,
        Method::POST,
        &format!("{API}/apply"),
        None,
        json!({ "app": "com.xingin.xhs", "devices": "all", "template": "pixel_9" }),
    )
    .await;
    assert_eq!(no_token.status, StatusCode::UNAUTHORIZED);

    // 失败请求不得留下 job。
    let jobs = send(
        &app,
        Method::GET,
        &format!("{API}/jobs"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(jobs["total"], 0);
}
