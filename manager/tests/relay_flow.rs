// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! relay（设备内本地认证中继）在 Manager 侧的透传与 socks5 真实探测。
//!
//! 覆盖两件事：
//! 1) `DeviceSummary.egress.relay` 的三态（在线 / 离线 / 旧设备）与代理池的 `relayActive` 透传；
//! 2) `POST /egress-proxies/{id}/probe` 走真正的 SOCKS5 隧道（RFC 1929 凭据），
//!    失败仍是结构化 502 且响应里没有凭据。

mod common;

use axum::http::{Method, StatusCode};
use common::{
    TestApp,
    mock_device::{MockDevice, MockOptions, MockSocks5, closed_port},
    send, send_json, start_app,
};
use serde_json::{Value, json};

const PROBE_URL: &str = "http://checkip.example/";
const SECRET: &str = "hunter2-super-secret";
const PROXY_USER: &str = "proxy-user";

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

fn http_proxy_body(name: &str) -> Value {
    json!({
        "name": name,
        "type": "http",
        "host": "127.0.0.1",
        "port": 1080,
        "username": PROXY_USER,
        "password": SECRET,
    })
}

fn socks5_body(name: &str, port: u16, password: &str) -> Value {
    json!({
        "name": name,
        "type": "socks5",
        "host": "127.0.0.1",
        "port": port,
        "username": PROXY_USER,
        "password": password,
    })
}

/// 一条 `relay.listeners[]`（契约 §6.1 字段表）。
fn listener(proxy: &str, port: u16, scope: &str, listening: bool) -> Value {
    let last_error = if listening {
        Value::Null
    } else {
        json!({
            "code": "egress_relay_bind_failed",
            "reason": "addr_in_use",
            "at": "2026-11-25T10:00:00Z",
        })
    };
    let allowed_uids = if scope == "device" {
        json!([])
    } else {
        json!([10123])
    };
    json!({
        "proxy": proxy,
        "port": port,
        "scope": scope,
        "listening": listening,
        "uidPolicy": if scope == "device" { "all" } else { "restricted" },
        "allowedUids": allowed_uids,
        "activeConnections": 0,
        "lastError": last_error,
    })
}

/// 设备端 `GET /egress` 的 `relay` 对象。
fn relay_payload(enabled: bool, listeners: Value) -> Value {
    let rules = if enabled { 14 } else { 0 };
    json!({
        "enabled": enabled,
        "basePort": 17800,
        "at": "2026-11-25T10:00:00Z",
        "limits": { "maxConnections": 256, "maxConnectionsPerUid": 64 },
        "listeners": listeners,
        "firewall": { "applied": enabled, "rules": rules, "lastError": Value::Null },
        "portChanges": [],
        "restartRequiredApps": [],
    })
}

/// 设备列表里某台设备的 `DeviceSummary`。
async fn device_summary(app: &TestApp, device_id: &str) -> Value {
    let response = send(
        app,
        Method::GET,
        "/relay/api/v1/devices?limit=50",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.json()["devices"]
        .as_array()
        .expect("devices 数组")
        .iter()
        .find(|device| device["id"] == device_id)
        .cloned()
        .unwrap_or_else(|| panic!("设备列表里没有 {device_id}"))
}

async fn probe(app: &TestApp, proxy_id: &str) -> common::TestResponse {
    send_json(
        app,
        Method::POST,
        &format!("/relay/api/v1/egress-proxies/{proxy_id}/probe"),
        Some(&app.token),
        json!({ "url": PROBE_URL }),
    )
    .await
}

fn leaks(value: &Value, secret: &str) -> bool {
    value.to_string().contains(secret)
}

#[tokio::test]
async fn device_summary_relay_is_three_state() {
    let app = start_app(None).await;

    // 1) 在线设备：relay 原样透传（listeners 只数在监听的端口）。
    let online = MockDevice::start(MockOptions {
        egress_relay: Some(relay_payload(
            true,
            json!([listener("us_la_01", 17800, "device", true)]),
        )),
        ..MockOptions::default()
    })
    .await;
    let device = add_device(&app, &online, "Pixel 9").await;
    let device_id = device["id"].as_str().unwrap_or_default().to_string();
    let proxy = create_proxy(&app, http_proxy_body("us_la_01")).await;
    let proxy_id = proxy["id"].as_str().unwrap_or_default().to_string();
    put_bindings(
        &app,
        &device_id,
        json!([{ "app": "", "proxyId": proxy_id }]),
    )
    .await;

    let summary = device_summary(&app, &device_id).await;
    assert_eq!(
        summary["egress"]["relay"],
        json!({
            "enabled": true,
            "active": true,
            "listeners": 1,
            "firewallApplied": true,
        }),
        "{summary}"
    );

    // 详情页（flatten 的 DeviceSummary）同样带 relay。
    let detail = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/devices/{device_id}"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.text());
    assert_eq!(detail.json()["egress"]["relay"]["active"], true);

    // 2) 旧设备：`GET /egress` 没有 relay 字段 → egress 对象在、relay 为 null。
    let legacy = MockDevice::start(MockOptions::default()).await;
    let legacy_device = add_device(&app, &legacy, "Legacy").await;
    let legacy_id = legacy_device["id"].as_str().unwrap_or_default().to_string();
    let legacy_proxy = create_proxy(&app, http_proxy_body("jp_tokyo")).await;
    let legacy_proxy_id = legacy_proxy["id"].as_str().unwrap_or_default().to_string();
    put_bindings(
        &app,
        &legacy_id,
        json!([{ "app": "", "proxyId": legacy_proxy_id }]),
    )
    .await;

    let legacy_summary = device_summary(&app, &legacy_id).await;
    assert!(
        legacy_summary["egress"].is_object(),
        "旧设备的出口摘要仍应存在：{legacy_summary}"
    );
    assert!(
        legacy_summary["egress"]["relay"].is_null(),
        "旧设备没有 relay 字段 → null：{legacy_summary}"
    );

    // 3) 离线：设备不可达 → null（不拿旧快照冒充在线）。
    online.stop();
    let synced = send(
        &app,
        Method::POST,
        &format!("/relay/api/v1/devices/{device_id}/sync"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(synced.status, StatusCode::OK, "{}", synced.text());
    assert_eq!(synced.json()["device"]["online"], false);

    let offline_summary = device_summary(&app, &device_id).await;
    assert!(
        offline_summary["egress"]["relay"].is_null(),
        "离线设备 → null：{offline_summary}"
    );
}

#[tokio::test]
async fn proxy_pool_exposes_relay_active() {
    let app = start_app(None).await;
    let mock = MockDevice::start(MockOptions {
        egress_relay: Some(relay_payload(
            true,
            json!([
                listener("us_la_01", 17800, "device", true),
                listener("jp_socks", 17831, "app", false),
            ]),
        )),
        ..MockOptions::default()
    })
    .await;
    let device = add_device(&app, &mock, "Pixel 9").await;
    let device_id = device["id"].as_str().unwrap_or_default().to_string();

    let device_level = create_proxy(&app, http_proxy_body("us_la_01")).await;
    let device_level_id = device_level["id"].as_str().unwrap_or_default().to_string();
    let app_level = create_proxy(&app, socks5_body("jp_socks", 1080, SECRET)).await;
    let app_level_id = app_level["id"].as_str().unwrap_or_default().to_string();
    let no_port = create_proxy(&app, http_proxy_body("hk_resi")).await;
    let no_port_id = no_port["id"].as_str().unwrap_or_default().to_string();

    put_bindings(
        &app,
        &device_id,
        json!([
            { "app": "", "proxyId": device_level_id },
            { "app": "com.xingin.xhs", "proxyId": app_level_id },
            { "app": "com.ss.android.ugc.aweme", "proxyId": no_port_id },
        ]),
    )
    .await;

    let list = send(
        &app,
        Method::GET,
        "/relay/api/v1/egress-proxies?limit=50",
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text());
    let body = list.json();
    let items = body["items"].as_array().expect("items");
    let find = |proxy_id: &str| {
        items
            .iter()
            .find(|item| item["id"] == proxy_id)
            .cloned()
            .unwrap_or_else(|| panic!("代理池里没有 {proxy_id}"))
    };

    // listener 在监听 → true。
    assert_eq!(find(&device_level_id)["relayActive"], true);
    // 有 localPort 但 listener 没在监听 → false（不是 null）。
    assert_eq!(find(&app_level_id)["relayActive"], false);
    // 设备没有给这个定义分配端口 → null。
    assert!(find(&no_port_id)["relayActive"].is_null());

    // 详情同样透传；创建响应（还没探测过设备）是 null。
    let detail = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/egress-proxies/{device_level_id}"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.text());
    assert_eq!(detail.json()["relayActive"], true);
    assert!(device_level["relayActive"].is_null());

    // 设备端被标离线后不再给值（不缓存旧状态）。
    mock.stop();
    let synced = send(
        &app,
        Method::POST,
        &format!("/relay/api/v1/devices/{device_id}/sync"),
        Some(&app.token),
        None,
    )
    .await;
    assert_eq!(synced.json()["device"]["online"], false);
    let offline = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/egress-proxies/{device_level_id}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert!(
        offline["relayActive"].is_null(),
        "离线设备 → null：{offline}"
    );
}

#[tokio::test]
async fn socks5_probe_tunnels_with_rfc1929_credentials() {
    let app = start_app(None).await;
    let socks = MockSocks5::start("198.51.100.9", Some((PROXY_USER, SECRET))).await;

    let created = create_proxy(
        &app,
        json!({
            "name": "jp_socks",
            "type": "socks5",
            "host": socks.host,
            "port": socks.port,
            "username": PROXY_USER,
            "password": SECRET,
        }),
    )
    .await;
    assert!(!leaks(&created, SECRET), "{created}");
    let proxy_id = created["id"].as_str().unwrap_or_default().to_string();

    let probed = probe(&app, &proxy_id).await;
    assert_eq!(probed.status, StatusCode::OK, "{}", probed.text());
    let body = probed.json();
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["exitIp"], "198.51.100.9");
    assert_eq!(body["upstreamStatus"], 200);
    assert_eq!(body["url"], PROBE_URL);
    assert!(!leaks(&body, SECRET), "{body}");
    assert!(!leaks(&body, PROXY_USER), "{body}");

    // 握手细节：用户名/密码认证（RFC 1929），目标是 socks5h 交给代理解析的域名。
    let exchanges = socks.exchanges();
    assert_eq!(exchanges.len(), 1, "{exchanges:?}");
    assert_eq!(exchanges[0]["method"], 2, "{exchanges:?}");
    assert_eq!(exchanges[0]["accepted"], true, "{exchanges:?}");
    assert_eq!(exchanges[0]["username"], PROXY_USER);
    assert_eq!(exchanges[0]["password"], SECRET);
    assert_eq!(
        exchanges[0]["target"], "checkip.example:80",
        "{exchanges:?}"
    );
    // 凭据只用于 socks 握手，绝不作为 Proxy-Authorization 漏给目标站点。
    assert_eq!(exchanges[0]["proxyAuthorization"], false, "{exchanges:?}");
    assert_eq!(exchanges[0]["authorization"], false, "{exchanges:?}");

    // 探测结果落库（详情页可读），且不含凭据。
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
    assert_eq!(detail_body["lastCheck"]["ok"], true, "{detail_body}");
    assert_eq!(detail_body["lastCheck"]["exitIp"], "198.51.100.9");
    assert_eq!(detail_body["lastCheck"]["source"], "manager");
    assert!(!leaks(&detail_body, SECRET), "{detail_body}");
}

#[tokio::test]
async fn socks5_probe_failures_are_structured_and_secret_free() {
    let app = start_app(None).await;

    // 1) 认证失败：mock 只接受别的口令 → 502 结构化，且不回显任何凭据。
    let strict = MockSocks5::start("198.51.100.9", Some((PROXY_USER, "the-right-one"))).await;
    let denied = create_proxy(&app, socks5_body("jp_socks", strict.port, SECRET)).await;
    let denied_id = denied["id"].as_str().unwrap_or_default().to_string();
    let failed = probe(&app, &denied_id).await;
    assert_eq!(failed.status, StatusCode::BAD_GATEWAY, "{}", failed.text());
    let body = failed.json();
    assert_eq!(body["error"]["code"], "egress_probe_failed", "{body}");
    assert_eq!(body["error"]["retryable"], true);
    assert_eq!(body["error"]["details"]["proxyId"], denied_id);
    assert_eq!(body["error"]["details"]["url"], PROBE_URL);
    assert_eq!(body["error"]["details"]["timeoutMs"], 5000);
    assert!(!leaks(&body, SECRET), "{body}");
    assert!(!leaks(&body, PROXY_USER), "{body}");

    // 认证确实被拒绝（走到过 RFC 1929 交换），失败也落一条检查记录。
    let exchanges = strict.exchanges();
    assert_eq!(exchanges.len(), 1, "{exchanges:?}");
    assert_eq!(exchanges[0]["accepted"], false, "{exchanges:?}");
    assert_eq!(exchanges[0]["username"], PROXY_USER);
    let denied_detail = send(
        &app,
        Method::GET,
        &format!("/relay/api/v1/egress-proxies/{denied_id}"),
        Some(&app.token),
        None,
    )
    .await
    .json();
    assert_eq!(denied_detail["lastCheck"]["ok"], false);
    assert!(!leaks(&denied_detail, SECRET), "{denied_detail}");

    // 2) 端口不可达 → reason=connect，仍是结构化 502。
    let broken = create_proxy(&app, socks5_body("hk_socks", closed_port(), SECRET)).await;
    let broken_id = broken["id"].as_str().unwrap_or_default().to_string();
    let unreachable = probe(&app, &broken_id).await;
    assert_eq!(
        unreachable.status,
        StatusCode::BAD_GATEWAY,
        "{}",
        unreachable.text()
    );
    let body = unreachable.json();
    assert_eq!(body["error"]["code"], "egress_probe_failed");
    assert_eq!(body["error"]["details"]["reason"], "connect");
    assert!(!leaks(&body, SECRET), "{body}");
}
