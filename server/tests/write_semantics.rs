// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 写路径语义：If-Match、并发、审计、幂等、405、dry run（docs/API.md 2、9）。

mod common;

use axum::http::{Method, StatusCode};
use device_faker_core::Config;
use device_faker_core::config::DeviceTemplate;
use device_faker_core::edit::upsert_template;
use device_faker_server::store::{Precondition, TxError, transact};
use serde_json::json;

use common::{
    ADMIN_TOKEN, assert_error_envelope, body_json, current_revision, fixture, header, if_match,
    json_call, read_config, send_body,
};

#[tokio::test]
async fn every_write_endpoint_requires_if_match() {
    let fixture = fixture();
    let cases: Vec<(Method, &str, Option<&str>)> = vec![
        (Method::PUT, "/api/v1/config", Some("application/json")),
        (
            Method::PUT,
            "/api/v1/templates/demo",
            Some("application/json"),
        ),
        (
            Method::PATCH,
            "/api/v1/templates/pixel_9",
            Some("application/json"),
        ),
        (
            Method::DELETE,
            "/api/v1/templates/pixel_9?confirm=true",
            None,
        ),
        (
            Method::POST,
            "/api/v1/templates/pixel_9/packages",
            Some("application/json"),
        ),
        (
            Method::DELETE,
            "/api/v1/templates/pixel_9/packages/com.example.app",
            None,
        ),
        (
            Method::PUT,
            "/api/v1/apps/com.example.two",
            Some("application/json"),
        ),
        (
            Method::DELETE,
            "/api/v1/apps/com.example.inline?confirm=true",
            None,
        ),
    ];
    for (method, uri, content_type) in cases {
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(content_type) = content_type {
            headers.push(("content-type", content_type));
        }
        let body = content_type.map(|_| b"{}".to_vec());
        let response = send_body(
            &fixture.router,
            method.clone(),
            uri,
            Some(ADMIN_TOKEN),
            &headers,
            body,
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::PRECONDITION_REQUIRED,
            "{method} {uri}"
        );
        let body = body_json(response).await;
        let error = assert_error_envelope(&body, "precondition_required");
        assert_eq!(error["details"]["header"], "If-Match");
    }
    assert_eq!(read_config(&fixture), fixture.config_text, "全部未落盘");
}

#[tokio::test]
async fn if_match_accepts_quoted_unquoted_prefixed_and_star() {
    let fixture = fixture();
    let mut revision = current_revision(&fixture);
    for index in 0..4 {
        let header = match index {
            0 => format!("\"{revision}\""),
            1 => revision.clone(),
            2 => format!("sha256:{revision}"),
            _ => "*".to_owned(),
        };
        let response = json_call(
            &fixture.router,
            Method::PUT,
            "/api/v1/templates/demo",
            ADMIN_TOKEN,
            &[("if-match", header.as_str())],
            &json!({ "packages": ["com.example.one"], "model": format!("M{index}") }),
        )
        .await;
        assert!(
            matches!(response.status(), StatusCode::OK | StatusCode::CREATED),
            "variant {index}: {header} -> {}",
            response.status()
        );
        revision = current_revision(&fixture);
    }
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert_eq!(config.templates["demo"].model.as_deref(), Some("M3"));
}

#[tokio::test]
async fn stale_if_match_returns_412_with_current_revision() {
    let fixture = fixture();
    let stale = if_match(&"0".repeat(64));
    let response = json_call(
        &fixture.router,
        Method::PATCH,
        "/api/v1/templates/pixel_9",
        ADMIN_TOKEN,
        &[("if-match", stale.as_str())],
        &json!({ "dpi": 440 }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "config_revision_mismatch");
    assert_eq!(error["details"]["expected"], "0".repeat(64));
    assert_eq!(error["details"]["actual"], current_revision(&fixture));
}

#[tokio::test]
async fn concurrent_writes_with_same_revision_leave_one_winner() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let store = fixture.state.store().clone();
    let barrier = std::sync::Barrier::new(2);

    let results: Vec<Result<(), String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let store = store.clone();
                let revision = revision.clone();
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    let template = DeviceTemplate {
                        model: Some(format!("concurrent-{index}")),
                        ..DeviceTemplate::default()
                    };
                    transact(
                        &store,
                        &Precondition::Exact(revision),
                        false,
                        move |document, _config| {
                            upsert_template(document, "concurrent", &template)
                                .map_err(TxError::from)?;
                            Ok(())
                        },
                    )
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}"))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect()
    });

    let successes = results.iter().filter(|result| result.is_ok()).count();
    let conflicts = results
        .iter()
        .filter(|result| {
            result
                .as_ref()
                .err()
                .is_some_and(|error| error.contains("RevisionMismatch"))
        })
        .count();
    assert_eq!(successes, 1, "{results:?}");
    assert_eq!(conflicts, 1, "{results:?}");

    let text = read_config(&fixture);
    let config = Config::from_toml(&text).expect("并发写后文件仍然完整");
    assert!(config.templates.get("concurrent").is_some());
    assert!(text.contains("# 测试用配置"), "注释仍在");
}

#[tokio::test]
async fn audit_log_records_writes_without_secrets() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/audited",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.audit"] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let after_first = current_revision(&fixture);

    let stale = if_match(&revision);
    let response = json_call(
        &fixture.router,
        Method::PATCH,
        "/api/v1/templates/audited",
        ADMIN_TOKEN,
        &[("if-match", stale.as_str())],
        &json!({ "dpi": 440 }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let text = std::fs::read_to_string(&fixture.audit_path).expect("audit file");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "每次写尝试一行：{text}");
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["method"], "PUT");
    assert_eq!(first["path"], "/api/v1/templates/audited");
    assert_eq!(first["status"], 201, "PUT 新建模板 201");
    assert_eq!(first["tokenId"], "default");
    assert_eq!(first["revisionBefore"], revision.as_str());
    assert_eq!(first["revisionAfter"], after_first.as_str());
    assert!(first["durationMs"].as_u64().is_some());
    assert!(first["requestId"].as_str().unwrap().starts_with("req_"));
    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["status"], 412);
    assert!(!text.contains(common::ADMIN_TOKEN), "审计不得记录 Token");
    assert!(!text.contains("com.example.audit"), "审计不得记录配置内容");
}

#[tokio::test]
async fn idempotency_key_replays_without_second_write() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/idem",
        ADMIN_TOKEN,
        &[
            ("if-match", if_match(&revision).as_str()),
            ("idempotency-key", "tpl-1"),
        ],
        &json!({ "packages": ["com.example.idem"] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(header(response.headers(), "idempotent-replay"), None);
    let first_body = body_json(response).await;
    let first_revision = first_body["revision"].as_str().unwrap().to_owned();
    assert_eq!(current_revision(&fixture), first_revision);

    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/idem",
        ADMIN_TOKEN,
        &[
            ("if-match", if_match(&revision).as_str()),
            ("idempotency-key", "tpl-1"),
        ],
        &json!({ "packages": ["com.example.idem"] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        header(response.headers(), "idempotent-replay"),
        Some("true")
    );
    let second_body = body_json(response).await;
    assert_eq!(second_body["revision"], first_body["revision"]);
    assert_eq!(
        current_revision(&fixture),
        first_revision,
        "重放不产生第二次写入"
    );
}

#[tokio::test]
async fn method_not_allowed_on_write_paths_uses_envelope() {
    let fixture = fixture();
    let response = send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/config",
        Some(ADMIN_TOKEN),
        &[],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "method_not_allowed");
    assert_eq!(error["details"]["method"], "DELETE");

    let response =
        common::get_with_token(&fixture.router, "/api/v1/device", common::WRITE_TOKEN).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn template_dry_run_reports_diff_without_writing() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo?dry_run=true",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.dry"], "model": "Dry Run" }),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "dry run 仍区分 201/200"
    );
    let body = body_json(response).await;
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["revision"], revision.as_str());
    assert_eq!(body["backupId"], serde_json::Value::Null);
    assert!(body["diff"].as_str().unwrap().contains("demo"));
    assert_eq!(read_config(&fixture), fixture.config_text);
}

#[tokio::test]
async fn write_response_revision_header_matches_file() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/header",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": [] }),
    )
    .await;
    let header_revision = header(response.headers(), "x-df-revision")
        .expect("x-df-revision")
        .to_owned();
    assert_eq!(current_revision(&fixture), header_revision);
    let body = body_json(response).await;
    assert_eq!(body["revision"], header_revision.as_str());
}
