// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 出口代理（egress）集成测试：代理池 CRUD、绑定、探测与批量下发。

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use common::{
    TestApp,
    mock_device::{MockDevice, MockOptions, MockProxy, closed_port},
    send, send_json, send_json_with_headers, start_app,
};
use serde_json::{Value, json};

const PROBE_URL: &str = "http://checkip.example/";
const SECRET: &str = "hunter2-super-secret";

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

async fn create_proxy(app: &TestApp, body: Value) -> Value {
    let response = send_json(
        app,
        Method::POST,
        "/relay/api/v1/egress-proxies",
        Some(&app.token),
        body,
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    response.json()
}

fn http_proxy_body(name: &str, port: u16) -> Value {
    json!({
        "name": name,
        "type": "http",
        "host": "127.0.0.1",
        "port": port,
        "region": "US-LA",
        "tags": ["us", "mobile"],
        "username": "proxy-user",
        "password": SECRET,
    })
}

async fn put_bindings(app: &TestApp, device_id: &str, bindings: Value) -> Value {
    let response = send_json(
        app,
        Method::PUT,
        &format!("/relay/api/v1/devices/{device_id}/egress"),
        Some(&app.token),
        json!({ "bindings": bindings }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.json()
}

async fn wait_job(app: &TestApp, job_id: &str) -> Value {
    for _ in 0..300 {
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
            "queued" | "running" => tokio::time::sleep(Duration::from_millis(20)).await,
            _ => return body,
        }
    }
    panic!("job {job_id} 未在超时内完成");
}

/// 递归检查 JSON 里是否出现凭据键。
fn contains_secret_key(value: &Value, sample: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.keys()
                .any(|key| key == "password" || key == "username" || key == "password_cipher")
                || map.values().any(|value| contains_secret_key(value, sample))
        }
        Value::Array(items) => items.iter().any(|item| contains_secret_key(item, sample)),
        Value::String(text) => text.contains(sample),
        _ => false,
    }
}

fn step_status<'a>(result: &'a Value, name: &str) -> Option<&'a Value> {
    result["steps"]
        .as_array()?
        .iter()
        .find(|step| step["step"].as_str() == Some(name))
}

#[tokio::test]
async fn proxy_crud_is_secret_free_and_blocks_bound_delete() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let created = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    assert!(!contains_secret_key(&created, SECRET), "{created}");
    let proxy_id = created["id"].as_str().unwrap().to_string();
    assert!(proxy_id.starts_with("egp_"), "{proxy_id}");
    assert_eq!(created["name"], "us_la_01");
    assert_eq!(created["type"], "http");
    assert_eq!(created["hasCredentials"], true);
    assert_eq!(created["enabled"], true);
    assert_eq!(created["boundDevices"], 0);
    assert_eq!(created["tags"], json!(["us", "mobile"]));
    assert_eq!(created["region"], "US-LA");

    // 列表：分页 + 过滤，且不含任何凭据键。
    let list = send(
        &app,
        Method::GET,
        "/relay/api/v1/egress-proxies?q=us_la",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text());
    assert!(!list.text().contains(SECRET));
    let body = list.json();
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["name"], "us_la_01");
    assert!(!contains_secret_key(&body, SECRET), "{body}");

    // 名字冲突 → 409。
    let conflict = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/egress-proxies",
        Some(&app.token),
        http_proxy_body("us_la_01", 1081),
    )
    .await;
    assert_eq!(conflict.status, StatusCode::CONFLICT, "{}", conflict.text());
    assert_eq!(conflict.json()["error"]["code"], "egress_proxy_exists");

    // 非法代理名 / 保留值 → 422。
    for name in ["__DIRECT__", "-bad"] {
        let invalid = send_json(
            &app,
            Method::POST,
            "/relay/api/v1/egress-proxies",
            Some(&app.token),
            http_proxy_body(name, 1080),
        )
        .await;
        assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY, "{name}");
        assert_eq!(
            invalid.json()["error"]["details"]["errors"][0]["code"],
            "invalid_egress_name"
        );
    }

    // PATCH：改名 + 清除凭据（双态）。
    let patched = send_json(
        &app,
        Method::PATCH,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}"),
        Some(&app.token),
        json!({ "name": "us_la_02", "username": null, "password": null }),
    )
    .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let patched_json = patched.json();
    assert_eq!(patched_json["name"], "us_la_02");
    assert_eq!(patched_json["hasCredentials"], false);
    assert!(!contains_secret_key(&patched_json, SECRET));

    // 不可置空字段 → 422。
    let null_name = send_json(
        &app,
        Method::PATCH,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}"),
        Some(&app.token),
        json!({ "name": null }),
    )
    .await;
    assert_eq!(null_name.status, StatusCode::UNPROCESSABLE_ENTITY);

    // 被绑定 → 删除 409。
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;
    let blocked = send(
        &app,
        Method::DELETE,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(blocked.status, StatusCode::CONFLICT, "{}", blocked.text());
    let blocked_body = blocked.json();
    assert_eq!(blocked_body["error"]["code"], "egress_proxy_in_use");
    assert_eq!(blocked_body["error"]["details"]["total"], 1);
    assert_eq!(
        blocked_body["error"]["details"]["devices"],
        json!(["pixel-9"])
    );

    // 解绑 → 204。
    put_bindings(&app, "pixel-9", json!([])).await;
    let deleted = send(
        &app,
        Method::DELETE,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text());

    // 审计：只有 actor / 时间 / 名称，没有凭据。
    let audit = send(
        &app,
        Method::GET,
        "/relay/api/v1/audit?action=egress.proxy_create,egress.proxy_update,egress.proxy_delete,egress.bind",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(audit.status, StatusCode::OK, "{}", audit.text());
    let audit_text = audit.text();
    assert!(
        !audit_text.contains(SECRET),
        "审计不能出现密码：{audit_text}"
    );
    assert!(!audit_text.contains("proxy-user"), "审计不能出现用户名");
    let audit_body = audit.json();
    assert!(
        audit_body["total"].as_i64().unwrap_or(0) >= 4,
        "{audit_body}"
    );
    let actions: Vec<String> = audit_body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["action"].as_str().unwrap_or_default().to_string())
        .collect();
    for expected in [
        "egress.proxy_create",
        "egress.proxy_update",
        "egress.bind",
        "egress.proxy_delete",
    ] {
        assert!(actions.contains(&expected.to_string()), "{actions:?}");
    }
    let create_entry = audit_body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["action"] == "egress.proxy_create")
        .expect("create 审计");
    assert_eq!(create_entry["target"]["type"], "egress_proxy");
    assert!(create_entry["detail"]["proxyId"].as_str().is_some());
    assert!(create_entry["detail"]["name"].as_str().is_some());
    assert_eq!(create_entry["actor"], "manager-token");
}

#[tokio::test]
async fn device_binding_put_get_roundtrip_and_device_state() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();

    let first = put_bindings(
        &app,
        "pixel-9",
        json!([
            { "app": "", "proxyId": proxy_id },
            { "app": "com.xingin.xhs", "proxyId": "__DIRECT__" },
            { "app": "com.tencent.mm", "proxyId": null },
        ]),
    )
    .await;
    assert_eq!(first["changed"], true);
    assert_eq!(first["pendingApply"], true);
    // 整机行 + 直连应用行；null 行不落库。
    let bindings = first["bindings"].as_array().unwrap();
    assert_eq!(bindings.len(), 2, "{first}");
    assert_eq!(bindings[0]["app"], "");
    assert_eq!(bindings[0]["mode"], "proxy");
    assert_eq!(bindings[0]["proxyName"], "us_la_01");
    assert_eq!(bindings[1]["app"], "com.xingin.xhs");
    assert_eq!(bindings[1]["mode"], "direct");
    assert!(bindings[1]["proxyId"].is_null());

    // 幂等重放。
    let again = put_bindings(
        &app,
        "pixel-9",
        json!([
            { "app": "", "proxyId": proxy_id },
            { "app": "com.xingin.xhs", "proxyId": "__DIRECT__" },
            { "app": "com.tencent.mm", "proxyId": null },
        ]),
    )
    .await;
    assert_eq!(again["changed"], false);

    // 读回同一个状态，并透传设备端 /egress。
    let fetched = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/egress",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    let fetched_body = fetched.json();
    assert_eq!(fetched_body["bindings"].as_array().unwrap().len(), 2);
    assert!(
        fetched_body["deviceState"]["revision"]
            .as_str()
            .unwrap_or_default()
            .starts_with("egress-"),
        "{fetched_body}"
    );
    assert_eq!(fetched_body["warnings"], json!([]));

    // 未知代理 → 422。
    let invalid = send_json(
        &app,
        Method::PUT,
        "/relay/api/v1/devices/pixel-9/egress",
        Some(&app.token),
        json!({ "bindings": [{ "app": "", "proxyId": "egp_unknown" }] }),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        invalid.json()["error"]["details"]["errors"][0]["code"],
        "invalid_value"
    );

    // 设备离线时 deviceState 为 null + device_offline 警告（不报错）。
    mock.stop();
    let offline = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/egress",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(offline.status, StatusCode::OK, "{}", offline.text());
    let offline_body = offline.json();
    assert!(offline_body["deviceState"].is_null());
    assert_eq!(offline_body["warnings"][0]["code"], "device_offline");
}

#[tokio::test]
async fn apply_pushes_egress_and_records_per_device_results() {
    let app = start_app(None).await;
    let online = MockDevice::start(MockOptions::default()).await;
    let offline = MockDevice::start(MockOptions {
        model: "Xiaomi 15".to_string(),
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &online, "Pixel 9").await;
    add_device(&app, &offline, "Xiaomi 15").await;

    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    for device_id in ["pixel-9", "xiaomi-15"] {
        put_bindings(&app, device_id, json!([{ "app": "", "proxyId": proxy_id }])).await;
    }
    offline.stop();

    let accepted = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        json!({ "devices": "all", "probe": true }),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text());
    let accepted_body = accepted.json();
    assert_eq!(accepted_body["kind"], "egress");
    assert_eq!(accepted_body["total"], 2);
    assert!(accepted_body["requestId"].as_str().is_some());
    let job_id = accepted_body["jobId"].as_str().unwrap().to_string();

    let job = wait_job(&app, &job_id).await;
    assert_eq!(job["kind"], "egress");
    assert_eq!(job["total"], 2);
    assert_eq!(job["status"], "partial");
    assert_eq!(job["succeeded"], 1);
    assert_eq!(job["skipped"], 1);
    assert_eq!(job["failed"], 0);
    assert!(job["app"].is_null());
    assert!(job["template"].is_null());
    assert_eq!(job["restart"], false);

    let results = job["results"].as_array().unwrap();
    let ok_result = results
        .iter()
        .find(|result| result["deviceId"] == "pixel-9")
        .expect("在线设备结果");
    assert_eq!(ok_result["status"], "ok");
    assert_eq!(ok_result["app"], "");
    assert_eq!(ok_result["httpStatus"], 200);
    assert!(
        ok_result["revision"]
            .as_str()
            .unwrap_or_default()
            .starts_with("egress-")
    );
    assert_eq!(
        step_status(ok_result, "egress_get").unwrap()["status"],
        "ok"
    );
    assert_eq!(
        step_status(ok_result, "egress_put").unwrap()["status"],
        "ok"
    );
    assert_eq!(
        step_status(ok_result, "egress_probe").unwrap()["status"],
        "ok"
    );

    let skipped_result = results
        .iter()
        .find(|result| result["deviceId"] == "xiaomi-15")
        .expect("离线设备结果");
    assert_eq!(skipped_result["status"], "skipped");
    assert_eq!(skipped_result["code"], "device_offline");
    assert_eq!(
        step_status(skipped_result, "egress_get").unwrap()["status"],
        "skipped"
    );

    // 设备端真的收到了代理定义与默认代理。
    assert_eq!(online.egress_default().as_deref(), Some("us_la_01"));
    let proxies = online.egress_proxies();
    assert_eq!(proxies.len(), 1);
    assert_eq!(proxies[0]["name"], "us_la_01");
    assert_eq!(proxies[0]["hasCredentials"], true);

    // 设备列表带最近一次 exitIp 与 proxyName。
    let devices = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices?limit=50",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(devices.status, StatusCode::OK, "{}", devices.text());
    let device_body = devices.json();
    let pixel = device_body["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|device| device["id"] == "pixel-9")
        .expect("pixel 设备");
    assert_eq!(pixel["egress"]["proxyName"], "us_la_01");
    assert_eq!(pixel["egress"]["exitIp"], "203.0.113.7");
    assert_eq!(pixel["egress"]["ok"], true);

    // /overview 同样带出口摘要。
    let overview = send(
        &app,
        Method::GET,
        "/relay/api/v1/overview",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(overview.status, StatusCode::OK, "{}", overview.text());
    let overview_body = overview.json();
    let pixel = overview_body["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|device| device["id"] == "pixel-9")
        .expect("overview pixel 设备");
    assert_eq!(pixel["egress"]["proxyName"], "us_la_01");
    assert_eq!(pixel["egress"]["exitIp"], "203.0.113.7");

    // 幂等重放：同 Idempotency-Key 返回同一个 job。
    let replay = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        &[("idempotency-key", "egress-key-1")],
        json!({ "devices": ["pixel-9"] }),
    )
    .await;
    assert_eq!(replay.status, StatusCode::ACCEPTED, "{}", replay.text());
    let replay_body = replay.json();
    let replay_id = replay_body["jobId"].as_str().unwrap().to_string();
    wait_job(&app, &replay_id).await;
    let replay_again = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        &[("idempotency-key", "egress-key-1")],
        json!({ "devices": ["pixel-9"] }),
    )
    .await;
    assert_eq!(replay_again.status, StatusCode::ACCEPTED);
    assert_eq!(replay_again.json()["jobId"], replay_id);
    assert_eq!(
        replay_again.header("idempotent-replay"),
        Some("true"),
        "命中幂等键要带 Idempotent-Replay"
    );

    // dryRun：只算计划，不建 job。
    let before = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs?limit=1",
        Some(&app.token),
        None,
    )
    .await
    .json()["total"]
        .as_i64()
        .unwrap();
    let dry = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        json!({ "devices": ["pixel-9"], "dryRun": true }),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text());
    let dry_body = dry.json();
    assert_eq!(dry_body["dryRun"], true);
    assert_eq!(dry_body["plan"][0]["proxyName"], "us_la_01");
    let after = send(
        &app,
        Method::GET,
        "/relay/api/v1/jobs?limit=1",
        Some(&app.token),
        None,
    )
    .await
    .json()["total"]
        .as_i64()
        .unwrap();
    assert_eq!(before, after, "dryRun 不能建 job");
}

#[tokio::test]
async fn repeated_apply_is_idempotent_and_reports_unchanged() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    let accepted = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        json!({ "devices": ["pixel-9"] }),
    )
    .await;
    let job = wait_job(&app, accepted.json()["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded");
    assert_eq!(job["results"][0]["status"], "ok");

    let accepted = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        json!({ "devices": ["pixel-9"] }),
    )
    .await;
    let job = wait_job(&app, accepted.json()["jobId"].as_str().unwrap()).await;
    // 幂等短路不能让 job 变 partial：结果仍是 ok，只是标了 egress_unchanged。
    assert_eq!(job["status"], "succeeded");
    assert_eq!(job["succeeded"], 1);
    assert_eq!(job["skipped"], 0);
    let result = &job["results"][0];
    assert_eq!(result["status"], "ok");
    assert_eq!(result["code"], "egress_unchanged");
    let put_step = step_status(result, "egress_put").expect("put 步骤");
    assert_eq!(put_step["status"], "skipped");
    assert_eq!(put_step["code"], "egress_unchanged");
}

#[tokio::test]
async fn probe_through_proxy_records_check_and_failure_is_structured() {
    let app = start_app(None).await;
    let proxy = MockProxy::start("198.51.100.7").await;

    let created = create_proxy(&app, http_proxy_body("us_la_01", proxy.port)).await;
    let proxy_id = created["id"].as_str().unwrap().to_string();

    let probed = send_json(
        &app,
        Method::POST,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}/probe"),
        Some(&app.token),
        json!({ "url": PROBE_URL }),
    )
    .await;
    assert_eq!(probed.status, StatusCode::OK, "{}", probed.text());
    let probed_body = probed.json();
    assert_eq!(probed_body["proxyId"], proxy_id);
    assert_eq!(probed_body["ok"], true);
    assert_eq!(probed_body["exitIp"], "198.51.100.7");
    assert_eq!(probed_body["url"], PROBE_URL);
    assert_eq!(probed_body["upstreamStatus"], 200);
    assert!(probed_body["latencyMs"].as_i64().unwrap() >= 0);

    // 请求确实经过代理，且凭据走 Basic 头（不进 URL / 日志）。
    let requests = proxy.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0]["method"], "GET");
    assert_eq!(requests[0]["uri"], PROBE_URL);
    let credentials = requests[0]["proxyAuthorization"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(credentials.starts_with("Basic "), "{requests:?}");
    // 头里是 base64，不是明文；明文只存在于请求体内的 Basic 载荷。
    assert!(!credentials.contains(SECRET), "{credentials}");
    assert!(!credentials.contains("proxy-user"), "{credentials}");

    // 探测结果落库：详情页 lastCheck / recentChecks 可读。
    let detail = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.text());
    let detail_body = detail.json();
    assert_eq!(detail_body["lastCheck"]["ok"], true);
    assert_eq!(detail_body["lastCheck"]["exitIp"], "198.51.100.7");
    assert_eq!(detail_body["lastCheck"]["source"], "manager");
    assert_eq!(detail_body["recentChecks"].as_array().unwrap().len(), 1);
    assert!(!contains_secret_key(&detail_body, SECRET), "{detail_body}");

    // 不可达代理 → 502 结构化错误码 + ok=0 落库。
    let broken = create_proxy(&app, http_proxy_body("jp_tokyo_02", closed_port())).await;
    let broken_id = broken["id"].as_str().unwrap().to_string();
    let failed = send_json(
        &app,
        Method::POST,
        &format!("/relay/api/v1/egress-proxies/{broken_id}/probe"),
        Some(&app.token),
        json!({ "url": PROBE_URL }),
    )
    .await;
    assert_eq!(failed.status, StatusCode::BAD_GATEWAY, "{}", failed.text());
    let failed_body = failed.json();
    assert_eq!(failed_body["error"]["code"], "egress_probe_failed");
    assert_eq!(failed_body["error"]["retryable"], true);
    assert_eq!(failed_body["error"]["details"]["proxyId"], broken_id);
    assert_eq!(failed_body["error"]["details"]["url"], PROBE_URL);
    assert_eq!(failed_body["error"]["details"]["reason"], "connect");
    assert_eq!(failed_body["error"]["details"]["timeoutMs"], 5000);

    let broken_detail = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/egress-proxies/{broken_id}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(broken_detail["lastCheck"]["ok"], false);
    assert_eq!(broken_detail["lastCheck"]["errorCode"], "connect");

    // socks5 且不可达 → 与 http 同口径的结构化 502（不再是 422 unsupported_proxy_type）。
    let socks = create_proxy(
        &app,
        json!({
            "name": "socks_01",
            "type": "socks5",
            "host": "127.0.0.1",
            "port": closed_port(),
        }),
    )
    .await;
    let socks_id = socks["id"].as_str().unwrap().to_string();
    let socks_probe = send_json(
        &app,
        Method::POST,
        &format!("/relay/api/v1/egress-proxies/{socks_id}/probe"),
        Some(&app.token),
        json!({ "url": PROBE_URL }),
    )
    .await;
    assert_eq!(
        socks_probe.status,
        StatusCode::BAD_GATEWAY,
        "{}",
        socks_probe.text()
    );
    let socks_body = socks_probe.json();
    assert_eq!(socks_body["error"]["code"], "egress_probe_failed");
    assert_eq!(socks_body["error"]["details"]["reason"], "connect");
    assert_eq!(socks_body["error"]["details"]["proxyId"], socks_id);
    let socks_detail = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/egress-proxies/{socks_id}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(socks_detail["lastCheck"]["ok"], false);
    assert_eq!(socks_detail["lastCheck"]["errorCode"], "connect");

    // 非法探测 URL → 422 invalid_probe_url。
    let bad_url = send_json(
        &app,
        Method::POST,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}/probe"),
        Some(&app.token),
        json!({ "url": "https://checkip.example/" }),
    )
    .await;
    assert_eq!(bad_url.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        bad_url.json()["error"]["details"]["errors"][0]["code"],
        "invalid_probe_url"
    );
}

#[tokio::test]
async fn not_configured_and_revision_conflict_paths() {
    // 1) 设备返回 409 egress.not_configured → 结构化失败码。
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        egress_put_status: Some(409),
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    let accepted = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        json!({ "devices": ["pixel-9"] }),
    )
    .await;
    let job = wait_job(&app, accepted.json()["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "failed", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["status"], "failed");
    assert_eq!(result["code"], "egress_not_configured");
    assert_eq!(result["httpStatus"], 409);
    assert_eq!(result["error"]["code"], "egress_not_configured");
    assert_eq!(
        result["error"]["details"]["upstreamCode"],
        "egress.not_configured"
    );

    // 2) 412 → 重取 revision 重试一次后成功。
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        force_egress_412_once: true,
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    let accepted = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        json!({ "devices": ["pixel-9"] }),
    )
    .await;
    let job = wait_job(&app, accepted.json()["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["status"], "ok");
    assert_eq!(result["attempts"], 2, "{result}");
    let mut saw_412 = false;
    for step in result["steps"].as_array().unwrap() {
        if step["step"] == "egress_put" && step["httpStatus"] == 412 {
            assert_eq!(step["code"], "config_revision_mismatch");
            saw_412 = true;
        }
    }
    assert!(saw_412, "{result}");
    assert_eq!(mock.egress_default().as_deref(), Some("us_la_01"));

    // 3) 旧版本设备（无 /egress）→ skipped egress_unsupported。
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        egress_supported: false,
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    let accepted = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        json!({ "devices": ["pixel-9"] }),
    )
    .await;
    let job = wait_job(&app, accepted.json()["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "partial", "{job}");
    assert_eq!(job["results"][0]["status"], "skipped");
    assert_eq!(job["results"][0]["code"], "egress_unsupported");

    // 设备出口详情页同样给出警告而不是错误。
    let detail = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/egress",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.text());
    assert_eq!(detail.json()["warnings"][0]["code"], "egress_unsupported");
}
/// 审计条数（按 action 过滤）。
async fn audit_total(app: &TestApp, action: &str) -> i64 {
    let response = send(
        app,
        Method::GET,
        &format!("/relay/api/v1/audit?action={action}&limit=1"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.json()["total"].as_i64().unwrap_or_default()
}

/// job 总数。
async fn jobs_total(app: &TestApp) -> i64 {
    let response = send(
        app,
        Method::GET,
        "/relay/api/v1/jobs?limit=1",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.json()["total"].as_i64().unwrap_or_default()
}

async fn apply_egress(app: &TestApp, body: Value) -> Value {
    let response = send_json(
        app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        body,
    )
    .await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    response.json()
}

#[tokio::test]
async fn dry_run_assignments_have_no_side_effects() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();

    let binds_before = audit_total(&app, "egress.bind").await;
    let jobs_before = jobs_total(&app).await;

    let dry = send_json(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        json!({
            "assignments": [
                { "deviceId": "pixel-9", "app": "", "proxyId": proxy_id },
            ],
            "dryRun": true,
        }),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text());
    let dry_body = dry.json();
    assert_eq!(dry_body["dryRun"], true);
    assert_eq!(dry_body["total"], 1);
    // assignments 形态的 dryRun 计划来自「既有 + 本次」的合并结果，但一行都不落库。
    assert_eq!(dry_body["plan"][0]["deviceId"], "pixel-9");
    assert_eq!(dry_body["plan"][0]["app"], "");
    assert_eq!(dry_body["plan"][0]["mode"], "proxy");
    assert_eq!(dry_body["plan"][0]["proxyName"], "us_la_01");

    // 绑定表没变、没有新增 egress.bind 审计、也没建 job。
    let fetched = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/egress",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    assert_eq!(fetched.json()["bindings"], json!([]));
    assert_eq!(audit_total(&app, "egress.bind").await, binds_before);
    assert_eq!(jobs_total(&app).await, jobs_before);
}

#[tokio::test]
async fn credential_rotation_and_clear_reach_device() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    // 第一次下发：请求体带原始凭据。
    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"] })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let body = mock.egress_last_put_body().expect("最后一次 PUT body");
    assert_eq!(body["default"], "us_la_01");
    assert_eq!(body["proxies"][0]["username"], "proxy-user");
    assert_eq!(body["proxies"][0]["password"], SECRET);

    // 轮换密码：新凭据必须真的进请求体（本地比较不能把它当成「未变更」）。
    let rotated = "rotated-secret-2";
    let patched = send_json(
        &app,
        Method::PATCH,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}"),
        Some(&app.token),
        json!({ "password": rotated }),
    )
    .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"] })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    // 内容变了 → 设备 changed=true，不是 egress_unchanged。
    assert_eq!(job["results"][0]["code"], Value::Null, "{job}");
    let body = mock.egress_last_put_body().expect("最后一次 PUT body");
    assert_eq!(body["proxies"][0]["password"], rotated);
    assert_eq!(body["proxies"][0]["username"], "proxy-user");

    // 清除凭据：请求体里不再出现 username / password 键。
    let cleared = send_json(
        &app,
        Method::PATCH,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}"),
        Some(&app.token),
        json!({ "username": null, "password": null }),
    )
    .await;
    assert_eq!(cleared.status, StatusCode::OK, "{}", cleared.text());
    assert_eq!(cleared.json()["hasCredentials"], false);
    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"] })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let body = mock.egress_last_put_body().expect("最后一次 PUT body");
    let definition = &body["proxies"][0];
    // 显式 null（而不是省略）才能真正清掉设备上已存的凭据。
    assert!(definition["username"].is_null(), "{body}");
    assert!(definition["password"].is_null(), "{body}");
    assert_eq!(mock.egress_proxies()[0]["hasCredentials"], false);
}

#[tokio::test]
async fn unbinding_apps_clears_device_state_and_sends_null() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();

    put_bindings(
        &app,
        "pixel-9",
        json!([
            { "app": "", "proxyId": proxy_id },
            { "app": "com.xingin.xhs", "proxyId": "__DIRECT__" },
        ]),
    )
    .await;
    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"] })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(mock.egress_default().as_deref(), Some("us_la_01"));
    assert_eq!(
        mock.egress_apps(),
        vec![("com.xingin.xhs".to_string(), "__DIRECT__".to_string())]
    );
    let body = mock.egress_last_put_body().expect("最后一次 PUT body");
    assert!(body["apps"].is_array(), "{body}");

    // 解绑全部：Manager 发 apps:null（清除所有应用级覆盖），设备侧 egress 归零。
    put_bindings(&app, "pixel-9", json!([])).await;
    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"] })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["total"], 1, "零绑定时按整机一行下发");
    let body = mock.egress_last_put_body().expect("最后一次 PUT body");
    assert!(body["apps"].is_null(), "{body}");
    assert!(body["default"].is_null(), "{body}");
    assert_eq!(body["proxies"], json!([]));
    assert_eq!(mock.egress_apps(), Vec::new());
    assert!(mock.egress_default().is_none());
}

#[tokio::test]
async fn concurrent_same_idempotency_key_yields_one_job() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    let jobs_before = jobs_total(&app).await;
    let body = json!({ "devices": ["pixel-9"] });
    let headers = [("idempotency-key", "egress-concurrent-1")];
    let (first, second) = tokio::join!(
        send_json_with_headers(
            &app,
            Method::POST,
            "/relay/api/v1/devices/egress/apply",
            Some(&app.token),
            &headers,
            body.clone(),
        ),
        send_json_with_headers(
            &app,
            Method::POST,
            "/relay/api/v1/devices/egress/apply",
            Some(&app.token),
            &headers,
            body.clone(),
        ),
    );
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.text());
    assert_eq!(second.status, StatusCode::ACCEPTED, "{}", second.text());
    let first_body = first.json();
    let second_body = second.json();
    assert_eq!(
        first_body["jobId"], second_body["jobId"],
        "同键并发必须复用同一个 job"
    );
    assert_eq!(jobs_total(&app).await, jobs_before + 1);
    wait_job(&app, first_body["jobId"].as_str().unwrap()).await;
}

#[tokio::test]
async fn credential_decrypt_failure_is_its_own_result_code() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    // 模拟「devmask-manager.key 变了」：已存密文与当前密钥不匹配（等价于换 key 后旧密文解不开）。
    let db =
        devmask_manager::db::Db::open(app.data_dir.join("devmask-manager.db")).expect("重开数据库");
    db.call(|connection| {
        connection.execute(
            "UPDATE egress_proxies SET password_cipher = 'v1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'",
            [],
        )
    })
    .await
    .expect("破坏已存密文");

    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"] })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "failed", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["status"], "failed");
    assert_eq!(result["code"], "decrypt_failed", "{result}");
    assert_eq!(result["error"]["code"], "decrypt_failed");
    assert_eq!(result["error"]["details"]["op"], "decrypt");
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("devmask-manager.key"),
        "{result}"
    );
    // 设备端不应该收到任何 egress 写入。
    assert!(mock.egress_last_put_body().is_none());
}

#[tokio::test]
async fn invalid_idempotency_key_is_rejected_before_any_write() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    let jobs_before = jobs_total(&app).await;
    let response = send_json_with_headers(
        &app,
        Method::POST,
        "/relay/api/v1/devices/egress/apply",
        Some(&app.token),
        &[("idempotency-key", "with space")],
        json!({ "devices": ["pixel-9"] }),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        response.text()
    );
    let body = response.json();
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(
        body["error"]["details"]["errors"][0]["path"],
        "Idempotency-Key"
    );
    assert_eq!(jobs_total(&app).await, jobs_before);
}
#[tokio::test]
async fn probe_check_proxy_id_only_for_proxy_rows() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(
        &app,
        "pixel-9",
        json!([
            { "app": "", "proxyId": proxy_id },
            { "app": "com.xingin.xhs", "proxyId": "__DIRECT__" },
        ]),
    )
    .await;

    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"], "probe": true })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");

    let db =
        devmask_manager::db::Db::open(app.data_dir.join("devmask-manager.db")).expect("重开数据库");
    let rows = db
        .call(|connection| {
            let mut statement = connection.prepare(
                "SELECT app, scope, proxy_id FROM egress_checks
                  WHERE device_id = 'pixel-9' ORDER BY app ASC",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .await
        .expect("读取 egress_checks");

    assert_eq!(rows.len(), 2, "{rows:?}");
    // 整机行：proxy_id 是设备级绑定的代理。
    assert_eq!(rows[0].0, "");
    assert_eq!(rows[0].1, "device");
    assert_eq!(rows[0].2.as_deref(), Some(proxy_id.as_str()));
    // 直连的 app 行：proxy_id 必须 NULL（不能回填整机代理）。
    assert_eq!(rows[1].0, "com.xingin.xhs");
    assert_eq!(rows[1].1, "app");
    assert!(rows[1].2.is_none(), "{rows:?}");
}

#[tokio::test]
async fn device_token_decrypt_failure_has_own_code() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;

    // 模拟 devmask-manager.key 变更：设备 Token 密文与当前密钥不匹配。
    let db =
        devmask_manager::db::Db::open(app.data_dir.join("devmask-manager.db")).expect("重开数据库");
    db.call(|connection| {
        connection.execute(
            "UPDATE devices SET token_cipher = 'v1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'",
            [],
        )
    })
    .await
    .expect("破坏设备 Token 密文");

    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"] })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "failed", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["code"], "device_token_invalid", "{result}");
    assert_eq!(result["error"]["details"]["op"], "decrypt");
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("devmask-manager.key"),
        "{result}"
    );
    // 设备端全程没被请求过。
    assert!(mock.egress_last_put_body().is_none());
}
#[tokio::test]
async fn device_token_invalid_on_both_get_and_job_paths() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions::default()).await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    // 模拟 devmask-manager.key 变更：设备 Token 密文与当前密钥不匹配。
    let db =
        devmask_manager::db::Db::open(app.data_dir.join("devmask-manager.db")).expect("重开数据库");
    db.call(|connection| {
        connection.execute(
            "UPDATE devices SET token_cipher = 'v1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'",
            [],
        )
    })
    .await
    .expect("破坏设备 Token 密文");

    // GET /devices/{id}/egress：Token 解不开 → device_token_invalid（不是 device_offline）。
    let fetched = send(
        &app,
        Method::GET,
        "/relay/api/v1/devices/pixel-9/egress",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    let fetched_body = fetched.json();
    assert!(fetched_body["deviceState"].is_null(), "{fetched_body}");
    // 绑定来自 Manager 数据库，不因 Token 解不开而消失。
    assert_eq!(fetched_body["bindings"].as_array().unwrap().len(), 1);
    let warning = &fetched_body["warnings"][0];
    assert_eq!(warning["code"], "device_token_invalid", "{fetched_body}");
    assert_eq!(warning["details"]["op"], "decrypt", "{fetched_body}");
    assert_eq!(warning["details"]["deviceId"], "pixel-9");
    assert!(
        warning["message"]
            .as_str()
            .unwrap_or_default()
            .contains("devmask-manager.key"),
        "{fetched_body}"
    );
    assert!(
        fetched_body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|warning| warning["code"] != "device_offline"),
        "Token 解密失败不能报成 device_offline：{fetched_body}"
    );

    // job 路径：同一个错误码。
    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"] })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    assert_eq!(job["status"], "failed", "{job}");
    assert_eq!(job["results"][0]["code"], "device_token_invalid", "{job}");
}

#[tokio::test]
async fn probe_failure_response_reports_check_not_recorded_when_write_fails() {
    let app = start_app(None).await;
    let broken = create_proxy(&app, http_proxy_body("jp_tokyo_02", closed_port())).await;
    let proxy_id = broken["id"].as_str().unwrap().to_string();

    // 让 egress_checks 写入失败：直接删表（等价于探测结果没法落库）。
    let db =
        devmask_manager::db::Db::open(app.data_dir.join("devmask-manager.db")).expect("重开数据库");
    db.call(|connection| connection.execute("DROP TABLE egress_checks", []))
        .await
        .expect("删除 egress_checks");

    let failed = send_json(
        &app,
        Method::POST,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}/probe"),
        Some(&app.token),
        json!({ "url": PROBE_URL }),
    )
    .await;
    assert_eq!(failed.status, StatusCode::BAD_GATEWAY, "{}", failed.text());
    let body = failed.json();
    assert_eq!(body["error"]["code"], "egress_probe_failed", "{body}");
    assert_eq!(body["error"]["details"]["reason"], "connect", "{body}");
    // 失败分支同样要报告「探测结果没落库」。
    let warnings = body["warnings"].as_array().expect("502 响应应带 warnings");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "egress_check_not_recorded"),
        "{body}"
    );
    let warning = warnings
        .iter()
        .find(|warning| warning["code"] == "egress_check_not_recorded")
        .unwrap();
    assert_eq!(warning["details"]["proxyId"], proxy_id, "{body}");
}

#[tokio::test]
async fn job_probe_failure_reports_check_not_recorded_when_write_fails() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        egress_probe_status: Some(502),
        ..MockOptions::default()
    })
    .await;
    add_device(&app, &mock, "Pixel 9").await;
    let proxy = create_proxy(&app, http_proxy_body("us_la_01", 1080)).await;
    let proxy_id = proxy["id"].as_str().unwrap().to_string();
    put_bindings(&app, "pixel-9", json!([{ "app": "", "proxyId": proxy_id }])).await;

    let db =
        devmask_manager::db::Db::open(app.data_dir.join("devmask-manager.db")).expect("重开数据库");
    db.call(|connection| connection.execute("DROP TABLE egress_checks", []))
        .await
        .expect("删除 egress_checks");

    let accepted = apply_egress(&app, json!({ "devices": ["pixel-9"], "probe": true })).await;
    let job = wait_job(&app, accepted["jobId"].as_str().unwrap()).await;
    // 探测失败不改结果状态。
    assert_eq!(job["status"], "succeeded", "{job}");
    let result = &job["results"][0];
    assert_eq!(result["status"], "ok", "{result}");
    let codes: Vec<&str> = result["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .filter_map(|warning| warning["code"].as_str())
        .collect();
    assert!(codes.contains(&"egress_probe_failed"), "{codes:?}");
    assert!(codes.contains(&"egress_check_not_recorded"), "{codes:?}");
}
