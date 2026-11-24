// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 配置端点集成测试：GET/PUT /config、/config/revision、/config/validate、/schema/config。

mod common;

use axum::http::{Method, StatusCode};
use device_faker_core::Config;
use serde_json::{Value, json};

use device_faker_server::commands::FixtureRunner;

use common::{
    ADMIN_TOKEN, READ_TOKEN, assert_error_envelope, body_json, body_text, current_revision,
    fixture, fixture_with_config, fixture_with_settings, get_with_token, header, if_match,
    read_config, send_body,
};

const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[tokio::test]
async fn get_config_returns_document_and_headers() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = get_with_token(&fixture.router, "/api/v1/config", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        header(response.headers(), "content-type")
            .unwrap()
            .starts_with("application/json")
    );
    assert_eq!(
        header(response.headers(), "x-df-revision"),
        Some(revision.as_str())
    );
    assert_eq!(
        header(response.headers(), "etag"),
        Some(format!("\"{revision}\"").as_str())
    );

    let body = body_json(response).await;
    assert_eq!(body["revision"], revision.as_str());
    assert_eq!(body["exists"], true);
    assert_eq!(body["size"], fixture.config_text.len());
    assert!(body["mtime"].as_str().unwrap().ends_with('Z'));
    assert_eq!(body["config"]["debug"], false);
    assert_eq!(body["config"]["default_hide_maps"], json!([]));
    assert_eq!(body["config"]["templates"]["pixel_9"]["model"], "Pixel 9");
    assert_eq!(body["config"]["apps"][0]["package"], "com.example.inline");
    // CONFIG_TEXT 里模板带未知字段 author → 保留并给出警告。
    let warnings = body["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "unknown_field_preserved")
    );
}

#[tokio::test]
async fn get_config_toml_returns_raw_text_with_comments() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/config?format=toml", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        header(response.headers(), "x-df-revision"),
        Some(current_revision(&fixture).as_str())
    );
    let text = body_text(response).await;
    assert_eq!(text, fixture.config_text);
    assert!(text.contains("# 测试用配置"));
}

#[tokio::test]
async fn missing_config_reports_exists_false_and_null_mtime() {
    let fixture = fixture_with_config(None);
    let response = get_with_token(&fixture.router, "/api/v1/config", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(response.headers(), "x-df-revision"), Some(EMPTY_SHA));
    let body = body_json(response).await;
    assert_eq!(body["exists"], false);
    assert_eq!(body["mtime"], serde_json::Value::Null);
    assert_eq!(body["size"], 0);
    assert_eq!(body["revision"], EMPTY_SHA);
    assert_eq!(body["config"]["templates"], json!({}));
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "config_missing")
    );

    let toml = get_with_token(&fixture.router, "/api/v1/config?format=toml", ADMIN_TOKEN).await;
    assert_eq!(toml.status(), StatusCode::OK);
    assert_eq!(body_text(toml).await, "");
}

#[tokio::test]
async fn get_config_rejects_unknown_format() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/config?format=xml", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "validation_failed");
    assert_eq!(error["details"]["errors"][0]["path"], "format");
}

#[tokio::test]
async fn config_revision_endpoint_matches_file_sha256() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/config/revision", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "etag"),
        Some(format!("\"{}\"", current_revision(&fixture)).as_str())
    );
    let body = body_json(response).await;
    assert_eq!(body["revision"], current_revision(&fixture).as_str());
    assert_eq!(body["exists"], true);
    assert_eq!(body["size"], fixture.config_text.len());
    assert!(body["mtime"].as_str().unwrap().ends_with('Z'));
}

#[tokio::test]
async fn put_config_requires_if_match() {
    let fixture = fixture();
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[],
        &json!({ "debug": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_REQUIRED);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "precondition_required");
    assert_eq!(error["details"]["header"], "If-Match");
}

#[tokio::test]
async fn put_config_rejects_stale_revision() {
    let fixture = fixture();
    let stale = if_match(&"a".repeat(64));
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", stale.as_str())],
        &json!({ "debug": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "config_revision_mismatch");
    assert_eq!(error["details"]["expected"], "a".repeat(64));
    assert_eq!(error["details"]["actual"], current_revision(&fixture));
    assert_eq!(read_config(&fixture), fixture.config_text, "412 不得改文件");
}

#[tokio::test]
async fn put_config_replaces_document_and_creates_backup() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let document = json!({
        "debug": false,
        "default_force_denylist_unmount": true,
        "default_hide_maps": ["df_*"],
        "templates": {
            "pixel_9": {
                "packages": ["com.example.app"],
                "brand": "google",
                "model": "Pixel 9",
                "sdk_int": 35,
                "dpi": 420
            }
        },
        "apps": [{ "package": "com.example.two", "manufacturer": "Xiaomi" }]
    });
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config?diff=true",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &document,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let new_revision = header(response.headers(), "x-df-revision")
        .expect("revision header")
        .to_owned();
    assert_ne!(new_revision, revision);
    assert_eq!(header(response.headers(), "etag"), None, "写响应不带 ETag");

    let body = body_json(response).await;
    assert_eq!(body["dryRun"], false);
    assert_eq!(body["revision"], new_revision.as_str());
    assert_eq!(body["previousRevision"], revision.as_str());
    assert!(body["backupId"].as_str().unwrap().contains('-'));
    let diff = body["diff"].as_str().expect("diff");
    assert!(diff.starts_with("--- a/config.toml"), "{diff}");
    assert_eq!(body["warnings"], json!([]));

    let text = read_config(&fixture);
    let parsed = Config::from_toml(&text).expect("written config parses");
    assert_eq!(parsed.templates.len(), 1);
    assert_eq!(
        parsed.templates["pixel_9"].model.as_deref(),
        Some("Pixel 9")
    );
    assert_eq!(parsed.apps.len(), 1);
    assert!(parsed.default_force_denylist_unmount);
    assert_eq!(parsed.default_hide_maps, vec!["df_*".to_owned()]);

    let backups: Vec<_> = std::fs::read_dir(fixture.config_path.parent().unwrap().join("backups"))
        .expect("backup dir")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(backups.len(), 1, "每次成功写入前自动备份一份");
}

#[tokio::test]
async fn put_config_rejects_unknown_fields_and_bad_types() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "debug": true, "bogus": 1 }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_error_envelope(&body, "validation_failed");
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "unknown_field"
    );

    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "templates": { "demo": { "dpi": 900 } } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_error_envelope(&body, "validation_failed");
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "out_of_range"
    );
    assert_eq!(read_config(&fixture), fixture.config_text, "422 不得改文件");
}

#[tokio::test]
async fn put_config_dry_run_does_not_write() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config?dry_run=true",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "debug": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["dryRun"], true);
    assert_eq!(
        body["revision"],
        revision.as_str(),
        "dry run 返回当前 revision"
    );
    assert_eq!(body["backupId"], serde_json::Value::Null);
    assert!(body["diff"].as_str().unwrap().contains("+debug = true"));
    assert_eq!(read_config(&fixture), fixture.config_text);
}

#[tokio::test]
async fn put_config_if_match_star_creates_missing_file() {
    let fixture = fixture_with_config(None);
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", "*")],
        &json!({ "debug": false }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["previousRevision"], serde_json::Value::Null);
    assert_eq!(body["backupId"], serde_json::Value::Null);
    assert!(fixture.config_path.exists());
}

#[tokio::test]
async fn put_config_accepts_toml_text_and_keeps_comments() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let toml = "# 手写注释保留\ndebug = true\n\n[cpu_presets]\nvalue = 1\n\n[templates.demo]\npackages = [\"com.example.app\"]\ndpi = 420\n";
    let response = send_body(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        Some(ADMIN_TOKEN),
        &[
            ("content-type", "text/plain"),
            ("if-match", if_match(&revision).as_str()),
        ],
        Some(toml.as_bytes().to_vec()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read_config(&fixture), toml);

    let warning_body = body_json(response).await;
    assert!(
        warning_body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "unknown_field_preserved")
    );
}

#[tokio::test]
async fn put_config_rejects_unsupported_media_type() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = send_body(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        Some(ADMIN_TOKEN),
        &[
            ("content-type", "application/xml"),
            ("if-match", if_match(&revision).as_str()),
        ],
        Some(b"<xml/>".to_vec()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "unsupported_media_type");
    assert_eq!(
        error["details"]["supported"],
        json!(["application/json", "application/toml", "text/plain"])
    );
}

#[tokio::test]
async fn validate_toml_returns_200_with_errors() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let good = json!({
        "format": "toml",
        "content": "[templates.demo]\npackages = [\"com.example.app\"]\ndpi = 420\n"
    });
    let response = common::json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/config/validate",
        READ_TOKEN,
        &[],
        &good,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["valid"], true);
    assert_eq!(body["revision"], revision.as_str());
    assert_eq!(body["errors"], json!([]));

    let bad = json!({
        "format": "toml",
        "content": "[templates.demo]\npackages = [\"com.example.app\"]\ndpi = 900\n"
    });
    let response = common::json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/config/validate",
        READ_TOKEN,
        &[],
        &bad,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["valid"], false);
    assert_eq!(body["errors"][0]["path"], "templates.demo.dpi");
    assert_eq!(body["errors"][0]["code"], "out_of_range");

    let broken = json!({ "format": "toml", "content": "[templates.demo\n" });
    let response = common::json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/config/validate",
        READ_TOKEN,
        &[],
        &broken,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["valid"], false);
    assert_eq!(body["errors"][0]["code"], "syntax_error");
    assert_eq!(
        read_config(&fixture),
        fixture.config_text,
        "validate 不落盘"
    );
}

#[tokio::test]
async fn validate_json_document_accepts_object_and_string() {
    let fixture = fixture();
    let body = json!({
        "format": "json",
        "content": { "templates": { "demo": { "packages": ["com.example.app"], "dpi": 420 } } }
    });
    let response = common::json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/config/validate",
        READ_TOKEN,
        &[],
        &body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let parsed = body_json(response).await;
    assert_eq!(parsed["valid"], true);

    let unknown = json!({
        "format": "json",
        "content": "{\"bogus\": true}"
    });
    let response = common::json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/config/validate",
        READ_TOKEN,
        &[],
        &unknown,
    )
    .await;
    let parsed = body_json(response).await;
    assert_eq!(parsed["valid"], false);
    assert_eq!(parsed["errors"][0]["code"], "unknown_field");
}

#[tokio::test]
async fn validate_rejects_bad_format_and_missing_content() {
    let fixture = fixture();
    let response = common::json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/config/validate",
        READ_TOKEN,
        &[],
        &json!({ "format": "yaml", "content": "" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = common::json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/config/validate",
        READ_TOKEN,
        &[],
        &json!({ "format": "toml" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["error"]["details"]["errors"][0]["path"], "content");
}

#[tokio::test]
async fn schema_config_is_core_generated_draft07() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/schema/config", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let expected = serde_json::to_value(device_faker_core::schema::config_schema()).unwrap();
    assert_eq!(body, expected, "必须原样返回 core 的 schema");
    assert_eq!(body["$schema"], "http://json-schema.org/draft-07/schema#");
    assert!(body.get("definitions").is_some());
    assert!(body.get("$defs").is_none());
    assert!(body["properties"].get("templates").is_some());
}

#[tokio::test]
async fn oversized_body_returns_413_envelope() {
    let fixture = fixture_with_settings(
        Some(common::CONFIG_TEXT),
        FixtureRunner::new(),
        |settings| {
            settings.body_limit_bytes = 32;
        },
    );
    let response = send_body(
        &fixture.router,
        Method::POST,
        "/api/v1/config/validate",
        Some(READ_TOKEN),
        &[("content-type", "application/json")],
        Some(vec![b'x'; 256]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "body_too_large");
    assert_eq!(error["details"]["limitBytes"], 32);
}

#[tokio::test]
async fn schema_and_config_require_authentication() {
    let fixture = fixture();
    for uri in [
        "/api/v1/config",
        "/api/v1/schema/config",
        "/api/v1/config/revision",
    ] {
        let response = common::get(&fixture.router, uri).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
    let response =
        common::get_with_token(&fixture.router, "/api/v1/config", common::WRITE_TOKEN).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn put_config_rejects_nested_unknown_and_duplicate_apps() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "templates": { "demo": { "bogus_nested": 1 } } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["path"],
        "templates.demo.bogus_nested"
    );
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "unknown_field"
    );

    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "apps": [
            { "package": "com.example.app" },
            { "package": "com.example.app" }
        ] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert!(
        body["error"]["details"]["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error["code"] == "duplicate_app_record"),
        "{body}"
    );
    assert_eq!(read_config(&fixture), fixture.config_text);
}
#[tokio::test]
async fn write_paths_enforce_contract_size_limits() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let apps: Vec<Value> = (0..501)
        .map(|index| json!({ "package": format!("com.example.app{index}") }))
        .collect();
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/config",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "apps": apps }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    let errors = body["error"]["details"]["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|error| error["path"] == "apps" && error["code"] == "out_of_range"),
        "{body}"
    );
    assert_eq!(read_config(&fixture), fixture.config_text);

    let packages: Vec<Value> = (0..201)
        .map(|index| json!(format!("com.example.pkg{index}")))
        .collect();
    let response = common::json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": packages }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    let errors = body["error"]["details"]["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|error| error["path"] == "templates.demo.packages"),
        "{body}"
    );
    assert_eq!(read_config(&fixture), fixture.config_text);
}
