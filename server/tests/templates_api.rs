// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 模板端点集成测试（docs/API.md 5.8-5.14）。

mod common;

use axum::http::{Method, StatusCode};
use devmask_core::Config;
use serde_json::json;

use common::{
    ADMIN_TOKEN, READ_TOKEN, WRITE_TOKEN, assert_error_envelope, body_json, current_revision,
    fixture, get_with_token, header, if_match, json_call, read_config,
};

#[tokio::test]
async fn list_templates_keeps_order_and_fields_set() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/templates", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "x-dm-revision"),
        Some(current_revision(&fixture).as_str())
    );
    let body = body_json(response).await;
    assert_eq!(body["revision"], current_revision(&fixture).as_str());
    assert_eq!(body["total"], 2);
    assert_eq!(body["nextCursor"], serde_json::Value::Null);
    assert_eq!(body["items"][0]["name"], "pixel_9");
    assert_eq!(body["items"][0]["position"], 0);
    assert_eq!(body["items"][0]["packageCount"], 2);
    assert_eq!(
        body["items"][0]["fieldsSet"],
        json!(["brand", "dpi", "model"])
    );
    assert_eq!(body["items"][1]["name"], "pixel_9_pro");
    assert_eq!(body["items"][1]["position"], 1);
}

#[tokio::test]
async fn list_templates_filters_by_package_and_paginates() {
    let fixture = fixture();
    let response = get_with_token(
        &fixture.router,
        "/api/v1/templates?package=com.example.shared",
        READ_TOKEN,
    )
    .await;
    let body = body_json(response).await;
    assert_eq!(body["total"], 2);

    let response = get_with_token(&fixture.router, "/api/v1/templates?limit=1", READ_TOKEN).await;
    let body = body_json(response).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["nextCursor"], "1");

    let response = get_with_token(
        &fixture.router,
        "/api/v1/templates?limit=1&cursor=1",
        READ_TOKEN,
    )
    .await;
    let body = body_json(response).await;
    assert_eq!(body["items"][0]["name"], "pixel_9_pro");
    assert_eq!(body["nextCursor"], serde_json::Value::Null);

    let response = get_with_token(&fixture.router, "/api/v1/templates?limit=0", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn get_template_returns_wrapper_without_template_name_inside() {
    let fixture = fixture();
    let response = get_with_token(&fixture.router, "/api/v1/templates/pixel_9", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(response.headers(), "etag"),
        Some(format!("\"{}\"", current_revision(&fixture)).as_str())
    );
    let body = body_json(response).await;
    assert_eq!(body["name"], "pixel_9");
    assert_eq!(body["position"], 0);
    assert_eq!(body["revision"], current_revision(&fixture).as_str());
    assert_eq!(body["template"]["model"], "Pixel 9");
    assert!(body["template"].get("name").is_none(), "模板内不含模板名");
    assert_eq!(
        body["template"]["author"],
        serde_json::Value::Null,
        "未知字段不进模型"
    );

    let response = get_with_token(&fixture.router, "/api/v1/templates/nope", READ_TOKEN).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "not_found");
    assert_eq!(error["details"]["resource"], "template");
}

#[tokio::test]
async fn put_template_creates_then_updates_and_preserves_comments() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let input = json!({
        "packages": ["com.example.one"],
        "brand": "google",
        "model": "Pixel 9",
        "dpi": 420
    });
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &input,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED, "新模板 201");
    let body = body_json(response).await;
    assert_eq!(body["previousRevision"], revision.as_str());
    assert!(body["backupId"].as_str().is_some());

    let text = read_config(&fixture);
    assert!(text.contains("# 测试用配置"), "注释必须保留");
    assert!(text.contains("author = \"tester\""), "未知字段必须保留");
    let config = Config::from_toml(&text).unwrap();
    assert_eq!(config.templates["demo"].model.as_deref(), Some("Pixel 9"));

    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.one"], "model": "Pixel 10" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "已存在 200");
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert_eq!(config.templates["demo"].model.as_deref(), Some("Pixel 10"));
    assert_eq!(config.templates["demo"].brand, None, "PUT 是整体替换");
    assert_eq!(config.templates["demo"].dpi, None);
}

#[tokio::test]
async fn put_template_validates_name_unknown_fields_and_duplicates() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/-bad",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": [] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "invalid_template_name"
    );

    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": [], "bogus": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["details"]["errors"][0]["code"],
        "unknown_field"
    );

    // com.example.shared 已被 pixel_9_pro 声明 → 默认 409。
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.shared"] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "conflict_duplicate_package");
    assert_eq!(error["details"]["package"], "com.example.shared");
    assert_eq!(
        error["details"]["templates"],
        json!(["pixel_9", "pixel_9_pro"])
    );

    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo?allow_duplicate_package=true",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.shared"] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = body_json(response).await;
    let warnings = body["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "duplicate_package")
    );
}

#[tokio::test]
async fn put_template_without_packages_warns_empty() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/empty",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "model": "X" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = body_json(response).await;
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "empty_packages")
    );
}

#[tokio::test]
async fn patch_template_merges_and_deletes_null() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PATCH,
        "/api/v1/templates/pixel_9",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "dpi": 440, "model": null, "brand": "google" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    let template = &config.templates["pixel_9"];
    assert_eq!(template.dpi, Some(440));
    assert_eq!(template.model, None, "null 删除字段");
    assert_eq!(template.brand.as_deref(), Some("google"));
    assert!(read_config(&fixture).contains("author = \"tester\""));

    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PATCH,
        "/api/v1/templates/nope",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "dpi": 440 }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "PATCH 不创建");
}

#[tokio::test]
async fn delete_template_requires_confirm_and_if_match() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = common::send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/templates/pixel_9_pro",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "confirmation_required");
    assert_eq!(error["details"]["action"], "delete_template");
    assert_eq!(read_config(&fixture), fixture.config_text);

    let response = common::send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/templates/pixel_9_pro?confirm=true",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert!(config.templates.get("pixel_9_pro").is_none());
    assert!(read_config(&fixture).contains("# 测试用配置"));

    let revision = current_revision(&fixture);
    let response = common::send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/templates/pixel_9_pro?confirm=true",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bind_packages_append_and_replace() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/templates/pixel_9/packages",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.three", "com.example.three"], "mode": "append" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert_eq!(
        config.templates["pixel_9"].packages,
        vec![
            "com.example.app".to_owned(),
            "com.example.shared".to_owned(),
            "com.example.three".to_owned()
        ]
    );

    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/templates/pixel_9/packages",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.four"], "mode": "replace" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert_eq!(
        config.templates["pixel_9"].packages,
        vec!["com.example.four".to_owned()]
    );
}

#[tokio::test]
async fn bind_packages_rejects_bad_mode_and_missing_template() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/templates/pixel_9/packages",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.x"], "mode": "merge" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = json_call(
        &fixture.router,
        Method::POST,
        "/api/v1/templates/nope/packages",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.x"] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unbind_package_removes_exact_entry() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = common::send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/templates/pixel_9/packages/com.example.shared",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert_eq!(
        config.templates["pixel_9"].packages,
        vec!["com.example.app".to_owned()]
    );

    let revision = current_revision(&fixture);
    let response = common::send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/templates/pixel_9/packages/com.example.missing",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    let error = assert_error_envelope(&body, "not_found");
    assert_eq!(error["details"]["resource"], "template_package");
}

#[tokio::test]
async fn template_writes_require_write_scope_and_if_match() {
    let fixture = fixture();
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo",
        READ_TOKEN,
        &[("if-match", if_match(&current_revision(&fixture)).as_str())],
        &json!({ "packages": [] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo",
        WRITE_TOKEN,
        &[],
        &json!({ "packages": [] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_REQUIRED);
}
#[tokio::test]
async fn package_path_accepts_percent_encoded_user_suffix() {
    let fixture = fixture();
    let revision = current_revision(&fixture);
    let response = json_call(
        &fixture.router,
        Method::PUT,
        "/api/v1/templates/demo",
        ADMIN_TOKEN,
        &[("if-match", if_match(&revision).as_str())],
        &json!({ "packages": ["com.example.app@999"] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let revision = current_revision(&fixture);
    let response = common::send_body(
        &fixture.router,
        Method::DELETE,
        "/api/v1/templates/demo/packages/com.example.app%40999",
        Some(ADMIN_TOKEN),
        &[("if-match", if_match(&revision).as_str())],
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "@ 需要百分号编码");
    let config = Config::from_toml(&read_config(&fixture)).unwrap();
    assert!(config.templates["demo"].packages.is_empty());
}
