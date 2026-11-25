// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 契约测试：路由注册必须与 docs/openapi.yaml 的 30 个操作**完全一致**。
//!
//! 两个方向都校验：
//!
//! 1. 文档里的每个操作都必须在静态表 `api::CONTRACT_OPERATIONS` 里（不能少），
//!    表里的每个操作也必须在文档里（不能多）；egress 的 5 个操作已在 egress 增量补齐；
//! 2. 每个操作实际存在（不是 405；404 必须是资源级错误，而不是 /api/v1 兜底）。

mod common;

use std::collections::BTreeSet;

use axum::http::{Method, StatusCode};
use devmask_server::api::CONTRACT_OPERATIONS;
use serde_json::{Value, json};

use common::{ADMIN_TOKEN, body_json, fixture, header};

fn openapi_operations() -> BTreeSet<(String, String)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/openapi.yaml");
    let text = std::fs::read_to_string(path).expect("docs/openapi.yaml");
    let document: serde_yaml::Value = serde_yaml::from_str(&text).expect("openapi yaml");
    let paths = document
        .get("paths")
        .and_then(serde_yaml::Value::as_mapping)
        .expect("paths");
    let mut operations = BTreeSet::new();
    for (path, item) in paths {
        let path = path.as_str().expect("path string").to_owned();
        let item = item.as_mapping().expect("path item");
        for (method, operation) in item {
            let Some(operation) = operation.as_mapping() else {
                continue;
            };
            if !operation.contains_key(serde_yaml::Value::String("operationId".to_owned())) {
                continue;
            }
            operations.insert((
                method.as_str().expect("method").to_ascii_uppercase(),
                path.clone(),
            ));
        }
    }
    operations
}

#[test]
fn contract_table_matches_openapi() {
    let documented = openapi_operations();
    let implemented: BTreeSet<(String, String)> = CONTRACT_OPERATIONS
        .iter()
        .map(|(method, path)| ((*method).to_owned(), (*path).to_owned()))
        .collect();
    // 双向严格相等：路由表与文档必须逐条一致（多一条、少一条都失败）。
    assert_eq!(implemented.len(), 30, "路由表操作数必须是 30");
    assert_eq!(documented.len(), 30, "OpenAPI 文档操作数必须是 30");
    assert_eq!(
        documented, implemented,
        "OpenAPI 文档与路由表必须逐条一致（文档缺操作 = 漏写，路由多操作 = 违约）"
    );
}

fn body_for(method: &str, path: &str) -> Option<Value> {
    match (method, path) {
        ("POST", "/api/v1/config/validate") => Some(json!({ "format": "toml", "content": "" })),
        ("POST", "/api/v1/templates/{name}/packages") => {
            Some(json!({ "packages": ["com.example.contract"] }))
        }
        ("POST", "/api/v1/apps/{pkg}/restart") => Some(json!({})),
        ("PUT", "/api/v1/config") => Some(json!({})),
        ("PUT", "/api/v1/templates/{name}") => Some(json!({ "packages": [] })),
        ("PATCH", "/api/v1/templates/{name}") => Some(json!({ "dpi": 420 })),
        ("PUT", "/api/v1/apps/{pkg}") => Some(json!({ "config": { "model": "Contract" } })),
        // egress：PUT 空对象是合法的「什么都不改」；probe 空对象按设备级链路探测
        ("PUT", "/api/v1/egress") => Some(json!({})),
        ("POST", "/api/v1/egress/probe") => Some(json!({})),
        _ => None,
    }
}

#[tokio::test]
async fn every_documented_operation_is_routed() {
    let fixture = fixture();
    for (method, path) in CONTRACT_OPERATIONS {
        let mut uri = path
            .replace("{name}", "demo")
            .replace("{pkg}", "com.example.app");
        if method == "DELETE" {
            uri.push_str("?confirm=true");
        }
        let parsed_method = Method::from_bytes(method.as_bytes()).expect("method");
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if method != "GET" {
            headers.push(("if-match", "*"));
        }
        let body = body_for(method, path);
        let response = if let Some(body) = body {
            headers.push(("content-type", "application/json"));
            common::send_body(
                &fixture.router,
                parsed_method,
                &uri,
                Some(ADMIN_TOKEN),
                &headers,
                Some(serde_json::to_vec(&body).unwrap()),
            )
            .await
        } else {
            common::send(
                &fixture.router,
                parsed_method,
                &uri,
                Some(ADMIN_TOKEN),
                &headers,
            )
            .await
        };

        let status = response.status();
        assert!(
            status != StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri} 返回 405：路由未注册该方法"
        );
        assert!(
            header(response.headers(), "content-type").is_some(),
            "{method} {uri} 缺少 Content-Type"
        );
        if status == StatusCode::NOT_FOUND {
            let body = body_json(response).await;
            assert_ne!(
                body["error"]["details"]["resource"], "path",
                "{method} {uri} 落到 /api/v1 兜底，说明没有注册路由"
            );
        }
    }
}

#[tokio::test]
async fn unimplemented_openapi_json_stays_404() {
    let fixture = fixture();
    let response =
        common::get_with_token(&fixture.router, "/api/v1/openapi.json", ADMIN_TOKEN).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "not_found");
    assert_eq!(body["error"]["details"]["resource"], "path");
}
