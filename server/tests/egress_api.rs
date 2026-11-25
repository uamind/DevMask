// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 出口代理（egress）端点的集成测试。
//!
//! 覆盖验收口径：PUT/DELETE 的 settings 写入与读回、四行判定真值表、egress revision
//! 与 If-Match、响应头（不得出现 X-DM-Revision / ETag）、0600 与权限自愈、幂等、
//! 探测成功/失败映射、启动自愈、以及「全局无密码明文」。

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use common::{
    ADMIN_TOKEN, OPS_TOKEN, READ_TOKEN, body_json, body_text, fixture_with_arc_runner, header,
    if_match, json_call, read_config, send,
};
use devmask_core::revision::FileLock;
use devmask_server::commands::FixtureRunner;
use devmask_server::commands::{CommandOutcome, CommandRunner, command_key};
use devmask_server::egress::reconcile_once;
use devmask_server::util;
use serde_json::{Value, json};

/// 带设备级 default 的配置。
const CONFIG_WITH_EGRESS: &str = "debug = false

[egress]
default = \"us_la_01\"
";

/// 对应 [egress].default 的代理定义（0600 文件）。
const EGRESS_TOML: &str = "version = 1

[proxies.us_la_01]
type = \"http\"
host = \"203.0.113.10\"
port = 1080

[probe]
url = \"http://checkip.amazonaws.com/\"
timeout_seconds = 5
fail_open = true
";

/// 模拟 Android settings 的可写状态（真实设备上 settings 会立即读回写入值）。
#[derive(Debug, Default)]
struct SimInner {
    proxy: Option<String>,
    host: Option<String>,
    exclusion: Option<String>,
    /// 模拟「写了但没生效」（SELinux 拒绝等）：记录调用但不改状态。
    ignore_puts: bool,
    /// 人为让 dumpsys 与 settings 主值分歧（L-9 用例）。
    dumpsys_override: Option<String>,
    calls: Vec<String>,
}

/// 共享的 settings 模拟执行器（测试侧也能读同一份状态）。
#[derive(Debug, Clone, Default)]
struct SettingsSim {
    inner: Arc<Mutex<SimInner>>,
}

impl SettingsSim {
    fn new() -> Self {
        Self::default()
    }

    fn with_proxy(value: Option<&str>) -> Self {
        let sim = Self::new();
        sim.set_proxy(value);
        sim
    }

    fn set_proxy(&self, value: Option<&str>) {
        let mut inner = self.lock();
        inner.proxy = value.map(str::to_owned);
        inner.host = value.and_then(proxy_host);
    }

    /// 写入不生效的执行器（readback 永远不匹配）。
    fn stubborn_with_proxy(value: Option<&str>) -> Self {
        let sim = Self::new();
        sim.set_proxy(value);
        sim.lock().ignore_puts = true;
        sim
    }

    /// 让 dumpsys 独立显示一个代理（与 settings 主值分歧），用于验证判定以交叉验证为准。
    fn set_dumpsys_proxy(&self, host: &str, port: u16) {
        self.lock().dumpsys_override = Some(format!("Global proxy: [{host}] port: {port}"));
    }

    /// 脏状态：http_proxy 为 null，但 global_http_proxy_host 仍有值。
    fn set_dirty(&self, host: &str) {
        let mut inner = self.lock();
        inner.proxy = None;
        inner.host = Some(host.to_owned());
    }

    fn proxy(&self) -> Option<String> {
        self.lock().proxy.clone()
    }

    fn calls(&self) -> Vec<String> {
        self.lock().calls.clone()
    }

    fn clear_calls(&self) {
        self.lock().calls.clear();
    }

    fn count_prefix(&self, prefix: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.starts_with(prefix))
            .count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SimInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn proxy_host(value: &str) -> Option<String> {
    if value == ":0" || value == "null" {
        return None;
    }
    let (host, _) = value.split_once(':')?;
    (!host.is_empty()).then(|| host.to_owned())
}

impl CommandRunner for SettingsSim {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
        let key = command_key(program, args);
        let mut inner = self.lock();
        inner.calls.push(key.clone());
        match (program, args) {
            ("settings", ["put", "global", "http_proxy", value]) => {
                if !inner.ignore_puts {
                    if *value == ":0" {
                        inner.proxy = Some(":0".to_owned());
                        inner.host = None;
                    } else {
                        inner.proxy = Some((*value).to_owned());
                        inner.host = proxy_host(value);
                    }
                }
                CommandOutcome::Ok(String::new())
            }
            ("settings", ["put", "global", "global_http_proxy_exclusion_list", value]) => {
                if !inner.ignore_puts {
                    inner.exclusion = Some((*value).to_owned());
                }
                CommandOutcome::Ok(String::new())
            }
            ("settings", ["get", "global", "http_proxy"]) => {
                CommandOutcome::Ok(inner.proxy.clone().unwrap_or_else(|| "null".to_owned()))
            }
            ("settings", ["get", "global", "global_http_proxy_host"]) => {
                CommandOutcome::Ok(inner.host.clone().unwrap_or_else(|| "null".to_owned()))
            }
            ("settings", ["get", "global", "global_http_proxy_exclusion_list"]) => {
                CommandOutcome::Ok(inner.exclusion.clone().unwrap_or_else(|| "null".to_owned()))
            }
            ("dumpsys", ["connectivity"]) => CommandOutcome::Ok(render_dumpsys(&inner)),
            _ => CommandOutcome::Unavailable {
                message: format!("测试未模拟命令: {key}"),
            },
        }
    }
}

fn render_dumpsys(inner: &SimInner) -> String {
    if let Some(line) = &inner.dumpsys_override {
        return format!("ConnectivityService:\n  {line}\n");
    }
    let line = match inner.proxy.as_deref() {
        None | Some("null") | Some(":0") => "Global proxy: none".to_owned(),
        Some(value) => match value.split_once(':') {
            Some((host, port)) => format!("Global proxy: [{host}] port: {port}"),
            None => "Global proxy: none".to_owned(),
        },
    };
    format!("ConnectivityService:\n  {line}\n")
}

fn fixture_with_sim(config_text: &str, sim: &SettingsSim) -> common::Fixture {
    fixture_with_arc_runner(Some(config_text), Arc::new(sim.clone()))
}

async fn eg_revision(router: &axum::Router) -> String {
    let response = common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK, "GET /egress 必须 200");
    let body = body_json(response).await;
    body["revision"].as_str().expect("revision").to_owned()
}

fn put_payload(default: &str) -> Value {
    json!({
        "default": default,
        "failOpen": true,
        "probeUrl": "http://checkip.amazonaws.com/",
        "proxies": [
            { "name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080, "username": "alice", "password": "s3cret" }
        ]
    })
}

#[tokio::test]
async fn put_applies_system_proxy_and_get_reports_applied() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    // 初始：没有 egress 配置、settings 是 null → 期望直连 = 已生效
    let response = common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(header(response.headers(), "x-dm-revision").is_none());
    assert!(header(response.headers(), "etag").is_none());
    assert!(header(response.headers(), "x-dm-egress-revision").is_some());
    let body = body_json(response).await;
    assert_eq!(body["mode"], "direct");
    assert_eq!(body["applied"], true);
    assert_eq!(body["configured"], false);
    let revision = body["revision"].as_str().unwrap().to_owned();

    // revision = sha256(configRevisionHex ‖ "\n" ‖ egress.toml bytes)
    let config_revision = util::sha256_hex(read_config(&fixture).as_bytes());
    let egress_bytes = std::fs::read(fixture.state.egress().store().path()).unwrap_or_default();
    let mut material = Vec::new();
    material.extend_from_slice(config_revision.as_bytes());
    material.push(b'\n');
    material.extend_from_slice(&egress_bytes);
    assert_eq!(revision, util::sha256_hex(&material));

    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(header(response.headers(), "x-dm-revision").is_none());
    assert!(header(response.headers(), "etag").is_none());
    assert!(header(response.headers(), "x-dm-egress-revision").is_some());
    let text = body_text(response).await;
    assert!(!text.contains("s3cret"), "响应里不得出现密码: {text}");
    assert!(!text.contains("alice"), "响应里不得出现用户名: {text}");
    assert!(
        !text.contains("password"),
        "响应里不得有 password 字段: {text}"
    );
    let body: Value = serde_json::from_str(&text).expect("json");
    assert_eq!(body["changed"], true);
    assert_eq!(body["applied"], true);
    assert_eq!(body["mode"], "proxy");
    assert_eq!(body["default"], "us_la_01");
    assert_eq!(body["upstream"]["hasCredentials"], true);
    assert_eq!(body["upstream"]["host"], "203.0.113.10");

    // settings 真的被写了，且读回一致
    assert_eq!(sim.proxy().as_deref(), Some("203.0.113.10:1080"));
    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(body["applied"], true);
    assert_eq!(body["readback"]["settingsValue"], "203.0.113.10:1080");
    assert_eq!(body["readback"]["matches"], true);
    assert_eq!(body["egressFile"]["exists"], true);

    // 文件分工：config.toml 只放引用，密码只在 0600 的 egress.toml 里
    let config_text = read_config(&fixture);
    assert!(config_text.contains("[egress]"), "{config_text}");
    assert!(
        config_text.contains("default = \"us_la_01\""),
        "{config_text}"
    );
    assert!(!config_text.contains("s3cret"), "{config_text}");
    let egress_path = fixture.state.egress().store().path().to_path_buf();
    let egress_text = std::fs::read_to_string(&egress_path).unwrap();
    assert!(
        egress_text.contains("password = \"s3cret\""),
        "{egress_text}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&egress_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "egress.toml 必须 0600");
        let config_mode = std::fs::metadata(&fixture.config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(config_mode, 0o644, "config.toml 保持 0644");
    }

    // 审计只有路径与状态码，没有密码
    let audit = std::fs::read_to_string(&fixture.audit_path).unwrap_or_default();
    assert!(audit.contains("/api/v1/egress"), "{audit}");
    assert!(!audit.contains("s3cret"), "{audit}");
}

#[tokio::test]
async fn delete_clears_system_proxy_with_colon_zero() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let payload = json!({
        "default": "us_la_01",
        "proxies": [{ "name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080 }]
    });
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &payload,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(sim.proxy().as_deref(), Some("203.0.113.10:1080"));
    sim.clear_calls();

    // 缺 confirm → 400 confirmation_required
    let revision = eg_revision(router).await;
    let response = send(
        router,
        Method::DELETE,
        "/api/v1/egress",
        Some(ADMIN_TOKEN),
        &[("if-match", &if_match(&revision))],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "confirmation_required");

    let response = send(
        router,
        Method::DELETE,
        "/api/v1/egress?confirm=true",
        Some(ADMIN_TOKEN),
        &[("if-match", &if_match(&revision))],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(header(response.headers(), "x-dm-revision").is_none());
    assert!(header(response.headers(), "etag").is_none());
    let body = body_json(response).await;
    assert_eq!(body["changed"], true);
    assert_eq!(body["mode"], "direct");
    assert_eq!(body["default"], Value::Null);
    assert_eq!(body["upstream"], Value::Null);
    assert_eq!(body["applied"], true);

    // 清除只允许写 :0（settings delete 无效）
    assert_eq!(sim.proxy().as_deref(), Some(":0"));
    assert!(
        sim.calls()
            .iter()
            .any(|call| call == "settings put global http_proxy :0"),
        "{:?}",
        sim.calls()
    );

    // 再删一次（新 revision）→ 幂等：changed=false 且不再写 settings
    sim.clear_calls();
    let revision = eg_revision(router).await;
    let body = body_json(
        send(
            router,
            Method::DELETE,
            "/api/v1/egress?confirm=true",
            Some(ADMIN_TOKEN),
            &[("if-match", &if_match(&revision))],
        )
        .await,
    )
    .await;
    assert_eq!(body["changed"], false);
    assert_eq!(
        sim.count_prefix("settings put"),
        0,
        "已直连且无改动时不得再写 settings: {:?}",
        sim.calls()
    );

    // 卸载脚本共用同一份「清直连」语义（脚本不能调用进程内函数，字面量必须一致）
    let script = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../module/uninstall.sh"
    ))
    .expect("uninstall.sh");
    assert!(
        script.contains("settings put global http_proxy :0"),
        "{script}"
    );
    assert!(
        !script.contains("settings delete global http_proxy"),
        "{script}"
    );
}

#[tokio::test]
async fn truth_table_rows_are_evaluated_from_readback() {
    // 第 1 行：settings 是 null + Global proxy: none → 期望直连时 applied=true
    // 第 2 行：:0 与 null 等价
    for value in [None, Some(":0")] {
        let sim = SettingsSim::with_proxy(value);
        let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
        let body =
            body_json(common::get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await)
                .await;
        assert_eq!(body["applied"], true, "settings={value:?}");
        assert_eq!(body["readback"]["matches"], true, "settings={value:?}");
    }

    // 第 3 行：h:p 与期望一致 → applied=true；不一致 → applied=false
    let sim = SettingsSim::with_proxy(Some("203.0.113.10:1080"));
    let fixture = fixture_with_sim(CONFIG_WITH_EGRESS, &sim);
    std::fs::write(fixture.state.egress().store().path(), EGRESS_TOML).unwrap();
    let body =
        body_json(common::get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await)
            .await;
    assert_eq!(body["mode"], "proxy");
    assert_eq!(body["applied"], true);
    assert_eq!(
        body["readback"]["dumpsysGlobalProxy"],
        "Global proxy: [203.0.113.10] port: 1080"
    );

    sim.set_proxy(Some("198.51.100.7:8080"));
    let body =
        body_json(common::get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await)
            .await;
    assert_eq!(body["applied"], false, "主值与期望不一致必须 applied=false");
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "egress_settings_mismatch"),
        "{body}"
    );

    // 第 4 行：脏状态（http_proxy 为 null 但 global_http_proxy_host 有值）→ applied=false
    sim.set_dirty("203.0.113.10");
    let body =
        body_json(common::get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await)
            .await;
    assert_eq!(
        body["applied"], false,
        "脏状态必须 applied=false（先写 :0）"
    );
}

#[tokio::test]
async fn if_match_is_required_and_compared_against_egress_revision() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    // 缺失 If-Match → 428
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_REQUIRED);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "precondition_required");
    assert_eq!(body["error"]["details"]["header"], "If-Match");

    // 不匹配 → 412 config_revision_mismatch + details.resource="egress"
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", "\"deadbeef\"")],
        &json!({ "default": "us_la_01" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "config_revision_mismatch");
    assert_eq!(body["error"]["details"]["resource"], "egress");
    assert_eq!(body["error"]["details"]["expected"], "deadbeef");
    assert!(body["error"]["details"]["actual"].as_str().is_some());

    // DELETE 同样要求 If-Match
    let response = send(
        router,
        Method::DELETE,
        "/api/v1/egress?confirm=true",
        Some(ADMIN_TOKEN),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_REQUIRED);

    // 写入后 revision 变化；用旧 revision 再写 → 412
    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["previousRevision"], revision.as_str());
    assert_ne!(body["revision"], revision.as_str());
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({ "default": null }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    // egress revision 不污染 config revision：PUT /config 仍用 config revision
    let config_revision = common::fetch_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&config_revision))],
        &json!({}),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /egress 后 PUT /config 不能因为 revision 被污染而 412"
    );
}

#[tokio::test]
async fn same_value_put_does_not_write_settings_twice() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        sim.count_prefix("settings put global http_proxy"),
        1,
        "首次写入必须只写一次 settings: {:?}",
        sim.calls()
    );

    sim.clear_calls();
    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &put_payload("us_la_01"),
        )
        .await,
    )
    .await;
    assert_eq!(
        body["changed"], false,
        "同一内容重复 PUT 必须 changed=false"
    );
    assert_eq!(
        body["revision"],
        revision.as_str(),
        "幂等短路不推进 revision"
    );
    assert_eq!(
        sim.count_prefix("settings put global http_proxy"),
        0,
        "值相同不得重复写 settings: {:?}",
        sim.calls()
    );
}

#[tokio::test]
async fn egress_toml_is_0600_self_healing_with_0600_backups() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let path = fixture.state.egress().store().path().to_path_buf();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // 人为放宽权限 → 读时自愈为 0600 + warning egress_perm_fixed
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let body =
            body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "读时必须自愈为 0600"
        );
        assert!(
            body["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning["code"] == "egress_perm_fixed"),
            "{body}"
        );

        // 再写一次 → 备份同样 0600
        let revision = eg_revision(router).await;
        let response = json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({ "probeUrl": "http://example.com/ip" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        // 备份目录与 config.toml 共用，只看 egress.toml.* 的备份
        let backups: Vec<_> = std::fs::read_dir(fixture.state.egress().store().backup_dir())
            .expect("备份目录")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("egress.toml.")
            })
            .collect();
        assert_eq!(backups.len(), 1, "写入前必须备份 egress.toml");
        for entry in backups {
            let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "备份里也有密码，必须 0600");
        }
    }
}

#[tokio::test]
async fn proxies_endpoint_is_masked_and_shows_references() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "default": "us_la_01",
            "proxies": [
                { "name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080, "username": "alice", "password": "s3cret" },
                { "name": "jp_tokyo", "type": "socks5", "host": "198.51.100.7", "port": 1080 }
            ],
            "apps": [{ "package": "com.example.inline", "egress": "jp_tokyo" }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = common::get_with_token(router, "/api/v1/egress/proxies", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(header(response.headers(), "x-dm-revision").is_none());
    assert!(header(response.headers(), "x-dm-egress-revision").is_some());
    let text = body_text(response).await;
    assert!(!text.contains("s3cret"), "{text}");
    assert!(!text.contains("alice"), "{text}");
    assert!(!text.contains("username"), "{text}");
    assert!(!text.contains("password"), "{text}");
    let body: Value = serde_json::from_str(&text).expect("json");
    assert_eq!(body["total"], 2);
    assert_eq!(body["items"][0]["name"], "us_la_01");
    assert_eq!(body["items"][0]["usedAsDefault"], true);
    assert_eq!(body["items"][0]["hasCredentials"], true);
    assert_eq!(body["items"][1]["name"], "jp_tokyo");
    assert_eq!(body["items"][1]["usedAsDefault"], false);
    assert_eq!(body["items"][1]["apps"][0], "com.example.inline");

    // 写权限断言：read token 不能写、ops token 不能读
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        READ_TOKEN,
        &[("if-match", "*")],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = common::get_with_token(router, "/api/v1/egress", OPS_TOKEN).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// 起一个只回一次固定响应的 HTTP stub，返回端口。
async fn spawn_http_stub(status_line: &'static str, body: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                let _ = stream.readable().await;
                let _ = stream.try_read(&mut buffer);
                let response = format!(
                    "{status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.writable().await;
                let _ = stream.try_write(response.as_bytes());
            });
        }
    });
    port
}

/// 拿一个「刚被释放」的端口，连接必然被拒（reason=connect）。
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

#[tokio::test]
async fn probe_reports_exit_ip_and_requires_ops_scope() {
    let port = spawn_http_stub("HTTP/1.1 200 OK", "203.0.113.7\n").await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    // 只放定义、不给 default → 设备级按直连探测
    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "probeUrl": format!("http://127.0.0.1:{port}/"),
            "proxies": [{ "name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080 }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // read token 不能探测（ops 作用域）
    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        READ_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "探测必须成功");
    assert!(header(response.headers(), "x-dm-revision").is_none());
    assert!(header(response.headers(), "x-dm-egress-revision").is_some());
    let body = body_json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["exitIp"], "203.0.113.7");
    assert_eq!(body["via"], "direct");
    assert_eq!(body["scope"], "device");
    assert_eq!(body["approximate"], false);
    assert_eq!(body["upstreamStatus"], 200);
    assert!(body["latencyMs"].as_u64().is_some());
    assert!(body["checkedAt"].as_str().unwrap().ends_with('Z'));

    // 最近一次探测进内存 health
    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(body["health"]["lastProbe"]["exitIp"], "203.0.113.7");
}

#[tokio::test]
async fn probe_app_scope_is_marked_approximate() {
    let port = spawn_http_stub("HTTP/1.1 200 OK", "198.51.100.9\n").await;
    let sim = SettingsSim::new();
    let config =
        "debug = false\n\n[[apps]]\npackage = \"com.example.inline\"\negress = \"__DIRECT__\"\n";
    let fixture = fixture_with_sim(config, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({ "probeUrl": format!("http://127.0.0.1:{port}/") }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(
        json_call(
            router,
            Method::POST,
            "/api/v1/egress/probe",
            OPS_TOKEN,
            &[],
            &json!({ "package": "com.example.inline" }),
        )
        .await,
    )
    .await;
    assert_eq!(body["exitIp"], "198.51.100.9");
    assert_eq!(body["scope"], "app");
    assert_eq!(body["approximate"], true);
    assert_eq!(body["via"], "direct");
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "egress_probe_approximate"),
        "{body}"
    );
}

#[tokio::test]
async fn probe_without_any_egress_is_409() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim("debug = false\n", &sim);
    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "egress.not_configured");
    assert_eq!(body["error"]["details"]["scope"], "device");
}

#[tokio::test]
async fn probe_failure_is_502_and_connect_failure_triggers_fail_open() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    // 代理本身不可达（127.0.0.1 的已关闭端口）→ 探测必然 reason=connect
    let proxy_port = closed_port().await;
    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "default": "us_la_01",
            "proxies": [{ "name": "us_la_01", "type": "http", "host": "127.0.0.1", "port": proxy_port }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        sim.proxy(),
        Some(format!("127.0.0.1:{proxy_port}")),
        "设备级期望值必须写成 host:port"
    );

    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "egress.probe_failed");
    assert_eq!(body["error"]["details"]["reason"], "connect");
    assert_eq!(body["error"]["details"]["timeoutMs"], 5000);
    assert_eq!(body["error"]["retryable"], true);

    // fail-open：流量切直连，但配置不动
    assert_eq!(
        sim.proxy().as_deref(),
        Some(":0"),
        "connect 失败必须触发 fail-open 切直连"
    );
    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(body["health"]["degraded"], true);
    assert_eq!(body["health"]["degradedReason"], "probe_failed");
    assert_eq!(body["effectiveMode"], "direct");
    assert_eq!(body["mode"], "proxy", "期望模式不变");
    assert_eq!(body["default"], "us_la_01", "配置必须保留");

    // http_status 不触发 fail-open（链路是通的）：切回直连链路再探测一个 503 目标
    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({ "default": null }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(sim.proxy().as_deref(), Some(":0"));
    sim.clear_calls();

    let port = spawn_http_stub("HTTP/1.1 503 Service Unavailable", "nope\n").await;
    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[],
        &json!({ "url": format!("http://127.0.0.1:{port}/") }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = body_json(response).await;
    assert_eq!(body["error"]["details"]["reason"], "http_status");
    assert_eq!(body["error"]["details"]["upstreamStatus"], 503);
    assert_eq!(
        sim.count_prefix("settings put global http_proxy"),
        0,
        "http_status 不应改动系统代理: {:?}",
        sim.calls()
    );
}

#[tokio::test]
async fn probe_url_must_be_plain_http() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({ "probeUrl": "https://checkip.amazonaws.com/" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "invalid_probe_url"
    );
    assert_eq!(body["error"]["details"]["errors"][0]["path"], "probeUrl");

    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[],
        &json!({ "url": "https://example.com/" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "invalid_probe_url"
    );
}

#[test]
fn startup_reconcile_skips_on_parse_error_and_fixes_mismatch() {
    // 解析失败：只记日志，不写 settings（宁可不动）
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    std::fs::write(
        fixture.state.egress().store().path(),
        "not = valid = toml\n",
    )
    .unwrap();
    let report = reconcile_once(&fixture.state);
    assert_eq!(report.reason, Some("egress_parse"));
    assert!(!report.wrote);
    assert_eq!(
        sim.count_prefix("settings put"),
        0,
        "解析失败不得写 settings"
    );

    // 读回与期望不一致 → 重写一次（期望直连时写 :0），修好即不降级
    let sim = SettingsSim::with_proxy(Some("198.51.100.7:8080"));
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let report = reconcile_once(&fixture.state);
    assert!(report.applied, "{report:?}");
    assert!(report.wrote);
    assert!(!report.degraded);
    assert_eq!(sim.proxy().as_deref(), Some(":0"));
    assert!(sim.count_prefix("settings put") >= 1);

    // 写了但没生效（SELinux 拒绝）→ 读回仍不一致 → fail-open 降级
    let sim = SettingsSim::stubborn_with_proxy(Some("198.51.100.7:8080"));
    let fixture = fixture_with_sim(CONFIG_WITH_EGRESS, &sim);
    std::fs::write(fixture.state.egress().store().path(), EGRESS_TOML).unwrap();
    let report = reconcile_once(&fixture.state);
    assert!(report.degraded, "{report:?}");
    assert_eq!(report.reason, Some("settings_mismatch"));
    assert!(!report.applied);

    // 已经一致：不写任何 settings
    let sim = SettingsSim::with_proxy(Some("203.0.113.10:1080"));
    let fixture = fixture_with_sim(CONFIG_WITH_EGRESS, &sim);
    std::fs::write(fixture.state.egress().store().path(), EGRESS_TOML).unwrap();
    let report = reconcile_once(&fixture.state);
    assert!(report.applied, "{report:?}");
    assert!(!report.wrote);
    assert_eq!(sim.count_prefix("settings put"), 0);

    // 空 egress.toml（文件不存在）也不该写 settings
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let report = reconcile_once(&fixture.state);
    assert!(report.applied, "{report:?}");
    assert_eq!(sim.count_prefix("settings put"), 0);
}

#[test]
fn startup_reconcile_runs_after_binding_and_off_the_runtime() {
    let app = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app.rs"))
        .expect("app.rs");
    let serve_at = app.find("pub async fn serve").expect("serve");
    // relay 增量：启动顺序是「先 relay（绑定 listener）→ 再 egress 自愈（写 settings）」，
    // 因此 serve 里启动的是 relay::spawn_startup（内部再跑 reconcile_once）。
    let spawn_at = app
        .find("relay::spawn_startup(state.clone())")
        .expect("serve 里必须启动一次异步自愈");
    assert!(serve_at < spawn_at, "自愈必须在 serve 内、监听之后启动");
    let state =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/egress/state.rs"))
            .expect("state.rs");
    assert!(
        state.contains("spawn_blocking"),
        "自愈必须跑在 blocking 线程池，不阻塞 healthz"
    );
}

fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn clearing_the_system_proxy_has_a_single_implementation() {
    let mut files = Vec::new();
    collect_rs(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut files,
    );
    assert!(!files.is_empty());
    let mut zero_holders = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).expect("read");
        assert!(
            !text.contains("settings delete global http_proxy"),
            "不允许出现删除系统代理的写法: {}",
            path.display()
        );
        assert!(
            !text.contains("\"delete\", \"global\""),
            "不允许出现删除系统代理的写法: {}",
            path.display()
        );
        if text.contains("\":0\"") {
            zero_holders.push(path.clone());
        }
    }
    assert_eq!(
        zero_holders.len(),
        1,
        "清直连（:0）只能有一处实现: {zero_holders:?}"
    );
    assert!(
        zero_holders[0].ends_with("egress/system.rs"),
        "{:?}",
        zero_holders[0]
    );
    // DELETE /egress、fail-open、启动自愈都走同一个 clear()
    let state =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/egress/state.rs"))
            .unwrap();
    assert!(state.contains("system.clear()"), "{state}");
    let api =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/api/egress.rs")).unwrap();
    assert!(api.contains(".clear()"), "{api}");
    assert!(api.contains("degrade_to_direct"), "{api}");
}

#[tokio::test]
async fn validation_errors_never_echo_credentials() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let secret = "s3cret".repeat(50);
    let revision = eg_revision(&fixture.router).await;
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "proxies": [{
                "name": "us_la_01",
                "type": "http",
                "host": "203.0.113.10",
                "port": 1080,
                "password": secret,
            }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let text = body_text(response).await;
    assert!(!text.contains("s3cret"), "错误信息绝不回显密码: {text}");
    assert!(text.contains("password 超过 256 字符"), "{text}");
    // 未写入任何文件
    assert!(!fixture.state.egress().store().path().exists());
}

#[tokio::test]
async fn broken_egress_toml_fails_closed_and_is_repairable() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;
    std::fs::write(
        fixture.state.egress().store().path(),
        "not = valid = toml\n",
    )
    .unwrap();

    // fail closed：GET 500 + details.op=egress_parse（不按空配置继续）
    let response = common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "internal_error");
    assert_eq!(body["error"]["details"]["op"], "egress_parse");

    // 修复通道：If-Match: * 重写
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", "*")],
        &json!({
            "default": "us_la_01",
            "proxies": [{ "name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080 }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["changed"], true);
    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(body["applied"], true);
    assert_eq!(body["default"], "us_la_01");
}

#[tokio::test]
async fn put_replays_through_idempotency_cache_but_probe_never_does() {
    let port = spawn_http_stub("HTTP/1.1 200 OK", "203.0.113.7\n").await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    // PUT 在 write 组里，复用既有 Idempotency-Key 重放
    let revision = eg_revision(router).await;
    let payload = json!({
        "probeUrl": format!("http://127.0.0.1:{port}/"),
        "proxies": [{ "name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080 }]
    });
    let first = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[
            ("if-match", &if_match(&revision)),
            ("idempotency-key", "egress-put-1"),
        ],
        &payload,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert!(header(first.headers(), "idempotent-replay").is_none());
    sim.clear_calls();
    let second = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[
            ("if-match", &if_match(&revision)),
            ("idempotency-key", "egress-put-1"),
        ],
        &payload,
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        header(second.headers(), "idempotent-replay"),
        Some("true"),
        "同 key 必须命中重放缓存"
    );
    assert_eq!(sim.count_prefix("settings put"), 0, "重放不得产生副作用");

    // probe 是 ops 组且**不挂**幂等中间件：两次都真发请求
    let first = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[("idempotency-key", "egress-probe-1")],
        &json!({}),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert!(header(first.headers(), "idempotent-replay").is_none());
    let second = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[("idempotency-key", "egress-probe-1")],
        &json!({}),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    assert!(
        header(second.headers(), "idempotent-replay").is_none(),
        "探测不得被重放"
    );
}

#[tokio::test]
async fn device_level_ipv6_literal_is_rejected() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let revision = eg_revision(&fixture.router).await;
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "default": "v6",
            "proxies": [{ "name": "v6", "type": "http", "host": "::1", "port": 1080 }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "invalid_egress_proxy"
    );
    assert_eq!(body["error"]["details"]["errors"][0]["path"], "default");
    assert_eq!(
        sim.count_prefix("settings put"),
        0,
        "校验失败不得写 settings"
    );
}

#[test]
fn tracing_never_formats_credentials() {
    let mut files = Vec::new();
    collect_rs(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/egress")),
        &mut files,
    );
    assert!(!files.is_empty());
    for path in files {
        let text = std::fs::read_to_string(&path).expect("read");
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if trimmed.contains("tracing::")
                || trimmed.contains("warn!(")
                || trimmed.contains("info!(")
            {
                assert!(
                    !trimmed.contains("password"),
                    "{}:{} 日志里不得出现 password: {line}",
                    path.display(),
                    index + 1
                );
                assert!(
                    !trimmed.contains("username"),
                    "{}:{} 日志里不得出现 username: {line}",
                    path.display(),
                    index + 1
                );
            }
        }
    }
}

#[tokio::test]
async fn config_write_passes_egress_validation_codes_through() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    // TOML 整写路径：apps 里的非法代理名必须是 invalid_egress_name（不是 invalid_value）
    let revision = common::fetch_revision(router).await;
    let toml = format!(
        "{}[[apps]]\npackage = \"com.example.egress\"\negress = \"bad name\"\n",
        common::CONFIG_TEXT
    );
    let response = common::send_body(
        router,
        Method::PUT,
        "/api/v1/config",
        Some(ADMIN_TOKEN),
        &[
            ("if-match", &if_match(&revision)),
            ("content-type", "application/toml"),
        ],
        Some(toml.into_bytes()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    let codes: Vec<&str> = body["error"]["details"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|error| error["code"].as_str())
        .collect();
    assert!(codes.contains(&"invalid_egress_name"), "{body}");

    // [egress].default = __DIRECT__ → invalid_egress_default
    let revision = common::fetch_revision(router).await;
    let toml = format!(
        "{}[egress]\ndefault = \"__DIRECT__\"\n",
        common::CONFIG_TEXT
    );
    let response = common::send_body(
        router,
        Method::PUT,
        "/api/v1/config",
        Some(ADMIN_TOKEN),
        &[
            ("if-match", &if_match(&revision)),
            ("content-type", "application/toml"),
        ],
        Some(toml.into_bytes()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    let codes: Vec<&str> = body["error"]["details"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|error| error["code"].as_str())
        .collect();
    assert!(codes.contains(&"invalid_egress_default"), "{body}");

    // JSON 路径同样透传（PUT /config 的 ConfigDocument 分支）
    let revision = common::fetch_revision(router).await;
    let doc = json!({
        "debug": false,
        "apps": [{ "package": "com.example.egress", "egress": "bad name" }]
    });
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &doc,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    let codes: Vec<&str> = body["error"]["details"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|error| error["code"].as_str())
        .collect();
    assert!(codes.contains(&"invalid_egress_name"), "{body}");
}

#[tokio::test]
async fn egress_section_credential_keys_are_upgraded_to_422_unknown_field() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    // PUT /config：0644 的 config.toml 里出现凭据键 → 422 unknown_field（不是 warning）
    let revision = common::fetch_revision(router).await;
    let toml = format!(
        "{}[egress]\ndefault = \"us_la_01\"\nhost = \"203.0.113.10\"\npassword = \"s3cret\"\n",
        common::CONFIG_TEXT
    );
    let response = common::send_body(
        router,
        Method::PUT,
        "/api/v1/config",
        Some(ADMIN_TOKEN),
        &[
            ("if-match", &if_match(&revision)),
            ("content-type", "application/toml"),
        ],
        Some(toml.into_bytes()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let text = body_text(response).await;
    assert!(!text.contains("s3cret"), "错误响应不得回显凭据: {text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    let errors: Vec<(String, String)> = body["error"]["details"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| {
            (
                error["path"].as_str().unwrap_or_default().to_owned(),
                error["code"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert!(
        errors.contains(&("egress.host".to_owned(), "unknown_field".to_owned())),
        "{errors:?}"
    );
    assert!(
        errors.contains(&("egress.password".to_owned(), "unknown_field".to_owned())),
        "{errors:?}"
    );

    // 已存在这种配置时 PUT /egress 也拒绝（不写 egress.toml、不写 settings）
    let bad_config = format!(
        "{}[egress]\ndefault = \"us_la_01\"\nhost = \"203.0.113.10\"\n",
        common::CONFIG_TEXT
    );
    let fixture = fixture_with_sim(&bad_config, &sim);
    // 这份 config.toml 连 core 都解析不了，所以 GET /egress 会是 500：
    // 用 If-Match: * 提交，验证的是「校验先于写盘」。
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", "*")],
        &json!({ "failOpen": false }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"], "unknown_field",
        "{body}"
    );
    assert!(!fixture.state.egress().store().path().exists());
    assert_eq!(sim.count_prefix("settings put"), 0);
}

fn auth_warnings_of(body: &Value) -> Vec<Value> {
    body["warnings"]
        .as_array()
        .map(|warnings| {
            warnings
                .iter()
                .filter(|warning| warning["code"] == "egress_auth_unsupported")
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn credentialed_http_device_proxy_reports_auth_unsupported_once() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    // 同一个带凭据的代理既做设备级 default、又被应用级引用：只报一次（device 优先）
    let revision = eg_revision(router).await;
    let payload = json!({
        "default": "us_la_01",
        "proxies": [{
            "name": "us_la_01",
            "type": "http",
            "host": "203.0.113.10",
            "port": 1080,
            "username": "alice",
            "password": "s3cret"
        }],
        "apps": [{ "package": "com.example.inline", "egress": "us_la_01" }]
    });
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &payload,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "带凭据不得阻断写入");
    let text = body_text(response).await;
    assert!(!text.contains("s3cret"), "响应不得回显密码: {text}");
    assert!(!text.contains("alice"), "响应不得回显用户名: {text}");
    let body: Value = serde_json::from_str(&text).expect("json");
    let auth = auth_warnings_of(&body);
    assert_eq!(auth.len(), 1, "同一代理只报一次: {body}");
    assert_eq!(auth[0]["details"]["scope"], "device");
    assert_eq!(auth[0]["details"]["proxy"], "us_la_01");
    assert_eq!(auth[0]["details"]["field"], "username");

    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    let auth = auth_warnings_of(&body);
    assert_eq!(auth.len(), 1, "GET 同样只报一次: {body}");
    assert_eq!(auth[0]["details"]["scope"], "device");
    assert_eq!(auth[0]["details"]["proxy"], "us_la_01");
    assert_eq!(auth[0]["details"]["field"], "username");
}

#[tokio::test]
async fn credentialed_socks5_app_proxy_reports_auth_unsupported() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let payload = json!({
        "proxies": [{
            "name": "jp_tokyo",
            "type": "socks5",
            "host": "198.51.100.7",
            "port": 1080,
            "password": "s3cret"
        }],
        "apps": [{ "package": "com.example.inline", "egress": "jp_tokyo" }]
    });
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &payload,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let auth = auth_warnings_of(&body);
    assert_eq!(auth.len(), 1, "{body}");
    assert_eq!(auth[0]["details"]["scope"], "app", "{body}");
    assert_eq!(auth[0]["details"]["proxy"], "jp_tokyo");
    assert_eq!(auth[0]["details"]["field"], "password");

    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    let auth = auth_warnings_of(&body);
    assert_eq!(auth.len(), 1, "{body}");
    assert_eq!(auth[0]["details"]["scope"], "app");
    assert_eq!(auth[0]["details"]["field"], "password");
}

#[tokio::test]
async fn plain_proxy_has_no_auth_unsupported_warning() {
    let port = spawn_http_stub("HTTP/1.1 200 OK", "203.0.113.7\n").await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let payload = json!({
        "default": "plain_01",
        "probeUrl": format!("http://127.0.0.1:{port}/"),
        "proxies": [{
            "name": "plain_01",
            "type": "http",
            "host": "127.0.0.1",
            "port": port
        }]
    });
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &payload,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(auth_warnings_of(&body).is_empty(), "{body}");

    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert!(auth_warnings_of(&body).is_empty(), "{body}");

    // 探测同样不该有该警告（本用例顺带覆盖「无凭据也能成功探测」）
    let body = body_json(
        json_call(
            router,
            Method::POST,
            "/api/v1/egress/probe",
            OPS_TOKEN,
            &[],
            &json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(body["exitIp"], "203.0.113.7", "{body}");
    assert!(auth_warnings_of(&body).is_empty(), "{body}");
}

#[tokio::test]
async fn probe_reports_auth_unsupported_for_credentialed_proxy() {
    // stub 对任何请求都回固定 IP，因此也能当「HTTP 代理」用（absolute-form 请求照收）
    let port = spawn_http_stub("HTTP/1.1 200 OK", "203.0.113.7\n").await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let payload = json!({
        "default": "creds_01",
        "proxies": [{
            "name": "creds_01",
            "type": "http",
            "host": "127.0.0.1",
            "port": port,
            "username": "alice",
            "password": "s3cret"
        }]
    });
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &payload,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "探测本身仍成功");
    let text = body_text(response).await;
    assert!(!text.contains("s3cret"), "{text}");
    assert!(!text.contains("alice"), "{text}");
    let body: Value = serde_json::from_str(&text).expect("json");
    let auth = auth_warnings_of(&body);
    assert_eq!(auth.len(), 1, "probe 响应同码同语义: {body}");
    assert_eq!(auth[0]["details"]["scope"], "device");
    assert_eq!(auth[0]["details"]["proxy"], "creds_01");
    assert_eq!(auth[0]["details"]["field"], "username");
}

// ---------------------------------------------------------------------------
// t23（t11 评审 findings）
// ---------------------------------------------------------------------------

fn warning_has_code(body: &Value, code: &str) -> bool {
    body["warnings"]
        .as_array()
        .is_some_and(|warnings| warnings.iter().any(|warning| warning["code"] == code))
}

#[tokio::test]
async fn dry_run_removal_diff_does_not_echo_stored_credentials() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // 只改 probeUrl 的 dry_run：响应不得含密码
    let revision = eg_revision(router).await;
    let text = body_text(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress?dry_run=true",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({ "probeUrl": "http://example.com/ip" }),
        )
        .await,
    )
    .await;
    assert!(!text.contains("s3cret"), "{text}");
    assert!(!text.contains("alice"), "{text}");

    // 删掉代理定义（diff 里必然出现 password 行的 -）也必须脱敏
    let revision = eg_revision(router).await;
    let text = body_text(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress?dry_run=true",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({ "proxies": [] }),
        )
        .await,
    )
    .await;
    assert!(!text.contains("s3cret"), "diff 不得回显旧密码: {text}");
    assert!(!text.contains("alice"), "diff 不得回显用户名: {text}");
    assert!(text.contains("<redacted>"), "凭据行必须脱敏: {text}");
}

#[tokio::test]
async fn diff_rotation_does_not_echo_old_or_new_password() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let revision = eg_revision(router).await;
    let text = body_text(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress?diff=true",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({
                "proxies": [{
                    "name": "us_la_01",
                    "type": "http",
                    "host": "203.0.113.10",
                    "port": 1080,
                    "username": "alice",
                    "password": "newpass"
                }]
            }),
        )
        .await,
    )
    .await;
    let body: Value = serde_json::from_str(&text).expect("json");
    let diff = body["diff"].as_str().expect("diff");
    assert!(!diff.contains("s3cret"), "不得回显旧密码: {diff}");
    assert!(!diff.contains("newpass"), "不得回显新密码: {diff}");
    assert!(diff.contains("<redacted>"), "{diff}");
    // 键名保留，便于看出「哪个键变了」
    assert!(diff.contains("password = \"<redacted>\""), "{diff}");
}

#[tokio::test]
async fn app_scope_probe_failure_does_not_degrade_device_proxy() {
    let stub = spawn_http_stub("HTTP/1.1 200 OK", "203.0.113.7\n").await;
    let dead = closed_port().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "default": "good",
            "proxies": [
                { "name": "good", "type": "http", "host": "127.0.0.1", "port": stub },
                { "name": "dead", "type": "socks5", "host": "127.0.0.1", "port": dead }
            ],
            "apps": [{ "package": "com.example.inline", "egress": "dead" }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        sim.proxy(),
        Some(format!("127.0.0.1:{stub}")),
        "设备级代理已生效"
    );
    sim.clear_calls();

    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[],
        &json!({ "package": "com.example.inline" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "egress.probe_failed");
    assert_eq!(body["error"]["details"]["scope"], "app");
    assert_eq!(body["error"]["details"]["reason"], "connect");
    assert_eq!(
        body["error"]["details"]["warnings"][0]["code"], "egress_probe_scope_app",
        "{body}"
    );
    // 应用级失败不得动设备级 settings、不得降级
    assert_eq!(sim.count_prefix("settings put"), 0, "{:?}", sim.calls());
    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(body["health"]["degraded"], false, "{body}");
    assert_eq!(body["effectiveMode"], "proxy");
}

#[tokio::test]
async fn stubborn_settings_report_actual_mode_and_need_real_clear() {
    // 模拟「settings 写了但没生效」（SELinux 拒绝），且设备上已有一个别的代理：
    // 降级动作清不掉它 → cleared=false → effectiveMode 必须报实际模式 proxy
    let sim = SettingsSim::stubborn_with_proxy(Some("198.51.100.7:8080"));
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({
                "default": "us_la_01",
                "proxies": [{ "name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080 }]
            }),
        )
        .await,
    )
    .await;
    assert_eq!(body["applied"], false, "{body}");
    assert_eq!(
        body["effectiveMode"], "proxy",
        "清直连没落地就不能报 direct"
    );
    assert_eq!(body["health"]["degraded"], true);
    assert_eq!(body["health"]["degradedReason"], "settings_unavailable");
    assert!(
        !body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "egress_degraded_to_direct"),
        "没切成功就不能报「已切直连」: {body}"
    );
}

#[tokio::test]
async fn probe_success_does_not_recover_when_reapply_fails() {
    let stub = spawn_http_stub("HTTP/1.1 200 OK", "203.0.113.7\n").await;
    let sim = SettingsSim::stubborn_with_proxy(Some("198.51.100.7:8080"));
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "default": "p",
            "proxies": [{ "name": "p", "type": "http", "host": "127.0.0.1", "port": stub }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(
        json_call(
            router,
            Method::POST,
            "/api/v1/egress/probe",
            OPS_TOKEN,
            &[],
            &json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(body["ok"], true, "探测本身成功: {body}");
    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(
        body["health"]["degraded"], true,
        "恢复路径必须先 readback 校验成功才 recover: {body}"
    );
    assert_eq!(body["effectiveMode"], "proxy");
}

#[test]
fn reconcile_waits_for_the_write_lock() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let store = fixture.state.store().clone();
    let lock = FileLock::acquire(&store.lock_path()).expect("config 写锁");
    let state = fixture.state.clone();
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    let handle = std::thread::spawn(move || {
        let _ = reconcile_once(&state);
        flag.store(true, Ordering::SeqCst);
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !done.load(Ordering::SeqCst),
        "reconcile 必须按写路径的锁序等待，而不是无锁读快照"
    );
    drop(lock);
    handle.join().expect("reconcile 线程");
    assert!(done.load(Ordering::SeqCst), "释放锁后 reconcile 必须完成");
}

#[tokio::test]
async fn settings_failure_returns_503_and_keeps_files() {
    let runner = FixtureRunner::new()
        .with_output("settings get global http_proxy", "null")
        .with_output("settings get global global_http_proxy_host", "null")
        .with_output("dumpsys connectivity", "  Global proxy: none\n")
        .with_failure("settings put global http_proxy 203.0.113.10:1080", 1);
    let fixture = fixture_with_arc_runner(Some(common::CONFIG_TEXT), Arc::new(runner));
    let revision = eg_revision(&fixture.router).await;
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "upstream_unavailable");
    assert_eq!(body["error"]["details"]["component"], "settings");
    assert!(
        fixture.state.egress().store().path().exists(),
        "配置是期望态：settings 失败也要先落盘"
    );
}

#[tokio::test]
async fn settings_unavailable_returns_503_but_writes_files() {
    let fixture =
        fixture_with_arc_runner(Some(common::CONFIG_TEXT), Arc::new(FixtureRunner::new()));
    let revision = eg_revision(&fixture.router).await;
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(fixture.state.egress().store().path().exists());

    // readback 不可用时 GET 也要给出原因明确的警告
    let body =
        body_json(common::get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await)
            .await;
    let mismatch: Vec<&Value> = body["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|warning| warning["code"] == "egress_settings_mismatch")
        .collect();
    assert_eq!(mismatch.len(), 1, "{body}");
    assert_eq!(mismatch[0]["details"]["reason"], "settings_unavailable");
    assert_eq!(body["applied"], false);
}

#[tokio::test]
async fn replay_reapplies_settings_when_readback_drifted() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;
    let payload = put_payload("us_la_01");

    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &payload,
        )
        .await,
    )
    .await;
    assert_eq!(body["changed"], true);
    assert_eq!(sim.proxy().as_deref(), Some("203.0.113.10:1080"));

    // 设备被外部改回直连：文件没变，但重放必须把 settings 补回去
    sim.set_proxy(Some(":0"));
    sim.clear_calls();
    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &payload,
        )
        .await,
    )
    .await;
    assert_eq!(body["changed"], false, "文件没变：changed 必须报 false");
    assert_eq!(body["revision"], revision.as_str(), "revision 不变");
    assert_eq!(
        sim.proxy().as_deref(),
        Some("203.0.113.10:1080"),
        "settings 必须被补回期望值"
    );
    assert!(sim.count_prefix("settings put global http_proxy") >= 1);
}

#[tokio::test]
async fn fail_closed_does_not_clear_settings() {
    let sim = SettingsSim::stubborn_with_proxy(None);
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({
                "default": "us_la_01",
                "failOpen": false,
                "proxies": [{ "name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080 }]
            }),
        )
        .await,
    )
    .await;
    assert_eq!(body["failOpen"], false);
    assert_eq!(body["applied"], false);
    assert_eq!(body["effectiveMode"], "proxy");
    assert_eq!(body["health"]["degraded"], true);
    assert_eq!(body["health"]["degradedReason"], "settings_mismatch");
    assert_eq!(
        sim.count_prefix("settings put global http_proxy :0"),
        0,
        "fail-closed 不得清直连: {:?}",
        sim.calls()
    );
}

#[tokio::test]
async fn fail_closed_probe_failure_does_not_touch_settings() {
    let dead = closed_port().await;
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({
            "default": "dead",
            "failOpen": false,
            "proxies": [{ "name": "dead", "type": "http", "host": "127.0.0.1", "port": dead }]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    sim.clear_calls();

    let response = json_call(
        router,
        Method::POST,
        "/api/v1/egress/probe",
        OPS_TOKEN,
        &[],
        &json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        sim.count_prefix("settings put"),
        0,
        "fail-closed 探测失败不得改 settings: {:?}",
        sim.calls()
    );
    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(body["health"]["degraded"], true);
    assert_eq!(body["health"]["degradedReason"], "probe_failed");
    assert_eq!(body["effectiveMode"], "proxy");
}

#[tokio::test]
async fn undefined_app_proxy_falls_back_to_device_mode() {
    // 设备级有默认：app 引用未定义名字 → effective 跟随设备级，proxy 给设备级默认名
    let sim = SettingsSim::new();
    let config = "debug = false\n\n[egress]\ndefault = \"us_la_01\"\n\n[[apps]]\npackage = \"com.example.inline\"\negress = \"missing\"\n";
    let fixture = fixture_with_sim(config, &sim);
    std::fs::write(fixture.state.egress().store().path(), EGRESS_TOML).unwrap();
    let body =
        body_json(common::get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await)
            .await;
    let apps = body["apps"].as_array().unwrap();
    assert_eq!(apps[0]["egress"], "missing");
    assert_eq!(apps[0]["effective"], "proxy", "{body}");
    assert_eq!(apps[0]["proxy"], "us_la_01", "{body}");
    assert!(warning_has_code(&body, "egress_proxy_not_found"), "{body}");

    // 设备级没有默认：effective 应为 direct、proxy 为 null
    let sim = SettingsSim::new();
    let config =
        "debug = false\n\n[[apps]]\npackage = \"com.example.inline\"\negress = \"missing\"\n";
    let fixture = fixture_with_sim(config, &sim);
    std::fs::write(fixture.state.egress().store().path(), EGRESS_TOML).unwrap();
    let body =
        body_json(common::get_with_token(&fixture.router, "/api/v1/egress", ADMIN_TOKEN).await)
            .await;
    let apps = body["apps"].as_array().unwrap();
    assert_eq!(apps[0]["effective"], "direct", "{body}");
    assert_eq!(apps[0]["proxy"], Value::Null, "{body}");
}

#[tokio::test]
async fn backup_id_matches_contract_shape() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let revision = eg_revision(router).await;
    let response = json_call(
        router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({ "probeUrl": "http://example.com/ip" }),
        )
        .await,
    )
    .await;
    let id = body["backupId"].as_str().expect("backupId");
    // <YYYYMMDDTHHMMSS.nnnnnnnnnZ>-<revision 前 8 位>
    assert_eq!(id.len(), 35, "backupId 形状不对: {id}");
    assert!(id.contains('T'), "{id}");
    let (stamp, revision_part) = id.split_once('-').expect("stamp-rev");
    assert!(stamp.ends_with('Z'), "{stamp}");
    assert_eq!(revision_part.len(), 8);
    assert!(revision_part.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
    assert!(!id.contains("toml"), "backupId 不得包含文件名: {id}");
}

const APPS_EGRESS_CONFIG: &str = "debug = false\n\n[[apps]]\npackage = \"com.example.a\"\negress = \"us_la_01\"\nmodel = \"Pixel 9\"\n\n[[apps]]\npackage = \"com.example.b\"\negress = \"__DIRECT__\"\nmodel = \"Pixel 8\"\n";

#[tokio::test]
async fn apps_empty_array_clears_all_overrides() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(APPS_EGRESS_CONFIG, &sim);
    std::fs::write(fixture.state.egress().store().path(), EGRESS_TOML).unwrap();
    assert!(
        read_config(&fixture).contains("egress"),
        "前置：配置里有覆盖"
    );

    let revision = eg_revision(&fixture.router).await;
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({ "apps": [] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["changed"], true);
    assert!(body["apps"].as_array().unwrap().is_empty(), "{body}");

    // 落盘后的 config.toml 不能有 egress 残留，但其它机型字段必须保留
    let text = read_config(&fixture);
    assert!(!text.contains("egress"), "{text}");
    assert!(text.contains("model = \"Pixel 9\""), "{text}");
    assert!(text.contains("model = \"Pixel 8\""), "{text}");
}

#[tokio::test]
async fn apps_null_and_omitted_follow_contract() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(APPS_EGRESS_CONFIG, &sim);
    std::fs::write(fixture.state.egress().store().path(), EGRESS_TOML).unwrap();
    let router = &fixture.router;

    // null = 清空所有覆盖
    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({ "apps": null }),
        )
        .await,
    )
    .await;
    assert_eq!(body["changed"], true);
    assert!(!read_config(&fixture).contains("egress"));

    // 显式写回一条
    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({ "apps": [{ "package": "com.example.a", "egress": "us_la_01" }] }),
        )
        .await,
    )
    .await;
    assert_eq!(body["changed"], true);
    let text = read_config(&fixture);
    assert!(text.contains("package = \"com.example.a\""), "{text}");
    assert!(text.contains("egress = \"us_la_01\""), "{text}");

    // 省略 apps = 不改任何覆盖（只改 probeUrl 时这条必须仍在）
    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({ "failOpen": true }),
        )
        .await,
    )
    .await;
    assert_eq!(body["changed"], false);
    assert!(read_config(&fixture).contains("egress = \"us_la_01\""));
}

#[tokio::test]
async fn apps_array_replaces_unlisted_overrides() {
    let sim = SettingsSim::new();
    let fixture = fixture_with_sim(APPS_EGRESS_CONFIG, &sim);
    std::fs::write(fixture.state.egress().store().path(), EGRESS_TOML).unwrap();

    let revision = eg_revision(&fixture.router).await;
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &json!({ "apps": [{ "package": "com.example.a", "egress": "us_la_01" }] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // 整体替换：com.example.b 不在列表里 → 它的 egress 必须被删掉
    let text = read_config(&fixture);
    let b_block = text
        .split("package = \"com.example.b\"")
        .nth(1)
        .expect("com.example.b 记录");
    assert!(
        !b_block.contains("egress"),
        "未列出的应用必须删掉 egress: {text}"
    );
    assert!(text.contains("egress = \"us_la_01\""), "{text}");
    assert!(
        b_block.contains("model = \"Pixel 8\""),
        "其它字段保留: {text}"
    );
}

#[tokio::test]
async fn settings_zero_but_dumpsys_still_proxy_is_a_mismatch() {
    // L-9：settings 主值是 :0，但 dumpsys 仍显示代理 —— 判定必须以交叉验证为准
    let sim = SettingsSim::new();
    sim.set_proxy(Some(":0"));
    sim.set_dumpsys_proxy("203.0.113.10", 1080);
    let fixture = fixture_with_sim(common::CONFIG_TEXT, &sim);
    let router = &fixture.router;

    let body = body_json(common::get_with_token(router, "/api/v1/egress", ADMIN_TOKEN).await).await;
    assert_eq!(body["mode"], "direct");
    assert_eq!(body["applied"], false, "{body}");
    assert_eq!(body["readback"]["settingsValue"], ":0");
    assert_eq!(body["readback"]["matches"], false);
    let dumpsys = body["readback"]["dumpsysGlobalProxy"]
        .as_str()
        .expect("dumpsys");
    assert!(dumpsys.contains("203.0.113.10"), "{dumpsys}");
    let mismatch: Vec<&Value> = body["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|warning| warning["code"] == "egress_settings_mismatch")
        .collect();
    assert_eq!(mismatch.len(), 1, "{body}");
    assert_eq!(mismatch[0]["details"]["expected"], ":0");
    assert_eq!(mismatch[0]["details"]["actual"], ":0");

    // 写路径同样判定为不一致：PUT 会补写 :0，但 dumpsys 仍说代理 → applied=false + warning
    let revision = eg_revision(router).await;
    let body = body_json(
        json_call(
            router,
            Method::PUT,
            "/api/v1/egress",
            ADMIN_TOKEN,
            &[("if-match", &if_match(&revision))],
            &json!({ "failOpen": true }),
        )
        .await,
    )
    .await;
    assert_eq!(body["applied"], false, "{body}");
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "egress_settings_mismatch"),
        "{body}"
    );
}

#[tokio::test]
async fn config_parse_errors_hide_source_lines() {
    // L-5：磁盘上的 config.toml 语法错误时，500 响应不得回显出错的原始行（可能含敏感值）
    let broken = "debug = \"s3cret\n";
    let fixture = common::fixture_with_config(Some(broken));
    for path in ["/api/v1/templates", "/api/v1/config"] {
        let response = common::get_with_token(&fixture.router, path, ADMIN_TOKEN).await;
        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{path}"
        );
        let text = body_text(response).await;
        assert!(!text.contains("s3cret"), "{path} 不得回显原始行: {text}");
        assert!(
            text.contains("TOML parse error") || text.contains("配置解析失败"),
            "{path}: {text}"
        );
    }
}

#[derive(Debug, Default, Clone)]
struct TimeoutSettingsRunner;

impl CommandRunner for TimeoutSettingsRunner {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutcome {
        match (program, args) {
            ("settings", ["get", "global", "http_proxy"]) => CommandOutcome::Ok("null".to_owned()),
            ("settings", ["get", "global", "global_http_proxy_host"]) => {
                CommandOutcome::Ok("null".to_owned())
            }
            ("settings", ["get", "global", "global_http_proxy_exclusion_list"]) => {
                CommandOutcome::Ok("null".to_owned())
            }
            ("dumpsys", ["connectivity"]) => {
                CommandOutcome::Ok("  Global proxy: none\n".to_owned())
            }
            ("settings", ["put", ..]) => CommandOutcome::Failed {
                code: None,
                stderr: "timeout after 5000ms".to_owned(),
            },
            _ => CommandOutcome::Unavailable {
                message: "测试未模拟命令".to_owned(),
            },
        }
    }
}

#[tokio::test]
async fn settings_timeout_is_normalized_to_503() {
    // M-7：执行器超时（Failed{code:None, stderr=timeout}）必须归一成 503 upstream_unavailable
    let fixture =
        fixture_with_arc_runner(Some(common::CONFIG_TEXT), Arc::new(TimeoutSettingsRunner));
    let revision = eg_revision(&fixture.router).await;
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/egress",
        ADMIN_TOKEN,
        &[("if-match", &if_match(&revision))],
        &put_payload("us_la_01"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "upstream_unavailable");
    assert_eq!(body["error"]["details"]["component"], "settings");
    assert_eq!(body["error"]["retryable"], true);
}
