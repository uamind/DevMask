// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 配置端点：GET/PUT /config、GET /config/revision、POST /config/validate（docs/API.md 5.4-5.7）。

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Extension, Query, State},
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::Response,
};
use device_faker_core::Config;
use device_faker_core::config::{is_known_app_key, is_known_config_key, is_known_template_key};
use serde_json::Value;

use crate::api::{self, json, with_revision_etag};
use crate::app::AppState;
use crate::dto::{
    ConfigResponse, ConfigRevisionResponse, ValidateResponse, ValidationError, Warning,
    document_from_config, document_warnings, map_validation_report,
};
use crate::error::ApiError;
use crate::params::{self, BodyKind, Params};
use crate::requests::RequestId;
use crate::store;

/// GET /api/v1/config
pub async fn get_config(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let format = params.raw("format").unwrap_or("json").trim();
    if format != "json" && format != "toml" {
        return Err(params::field_error(
            request_id.as_str(),
            "format",
            "invalid_value",
            format!("format 只支持 json 或 toml，收到 {format:?}"),
        ));
    }

    let snapshot = store::read_snapshot(state.store()).map_err(|error| {
        ApiError::internal_error(request_id.as_str(), format!("读取配置失败: {error}"))
    })?;

    if format == "toml" {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(axum::http::header::CACHE_CONTROL, "no-store")
            .body(axum::body::Body::from(snapshot.text.clone()))
            .expect("toml response is valid");
        response = with_revision_etag(response, &snapshot.revision);
        return Ok(response);
    }

    let config_text = if snapshot.exists {
        snapshot.text.as_str()
    } else {
        ""
    };
    let config = Config::from_toml(config_text).map_err(|error| {
        ApiError::internal_error(
            request_id.as_str(),
            format!("配置解析失败: {}", crate::util::parse_error_message(&error)),
        )
    })?;

    let mut warnings = Vec::new();
    if !snapshot.exists {
        warnings.push(Warning::new(
            "config_missing",
            "配置文件不存在，返回空配置",
            Some(serde_json::json!({})),
        ));
    }
    warnings.extend(document_warnings(config_text));

    let body = ConfigResponse {
        revision: snapshot.revision.clone(),
        mtime: snapshot.mtime,
        size: snapshot.size,
        exists: snapshot.exists,
        warnings,
        config,
    };
    Ok(with_revision_etag(
        json(StatusCode::OK, body),
        &snapshot.revision,
    ))
}

/// GET /api/v1/config/revision
pub async fn get_revision(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let snapshot = store::read_snapshot(state.store()).map_err(|error| {
        ApiError::internal_error(request_id.as_str(), format!("读取配置失败: {error}"))
    })?;
    let body = ConfigRevisionResponse {
        revision: snapshot.revision.clone(),
        mtime: snapshot.mtime,
        size: snapshot.size,
        exists: snapshot.exists,
    };
    Ok(with_revision_etag(
        json(StatusCode::OK, body),
        &snapshot.revision,
    ))
}

/// PUT /api/v1/config
pub async fn put_config(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let dry_run = params.flag("dry_run", request_id.as_str())?;
    let want_diff = params.flag("diff", request_id.as_str())?;
    let precondition = store::require_if_match(&headers, request_id.as_str())?;
    let kind = params::config_body_kind(&headers, request_id.as_str())?;

    let new_text = match kind {
        BodyKind::Json => {
            let value = params::parse_json(&body, request_id.as_str())?;
            build_document_text(&value, request_id.as_str())?
        }
        BodyKind::Toml => String::from_utf8(body.to_vec()).map_err(|error| {
            params::field_error(
                request_id.as_str(),
                "$",
                "syntax_error",
                format!("请求体不是合法 UTF-8: {error}"),
            )
        })?,
    };

    let outcome = store::transact_text(state.store(), &precondition, dry_run, new_text)
        .map_err(|error| error.into_api(request_id.as_str()))?;
    let warnings = document_warnings(&outcome.new_text);
    let result = api::write_result(&outcome, dry_run, want_diff, warnings);
    Ok(crate::api::with_revision(
        json(StatusCode::OK, result),
        &outcome.revision,
    ))
}

/// POST /api/v1/config/validate（校验失败也是 200）。
pub async fn validate_config(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    params::require_json(&headers, request_id.as_str())?;
    let value = params::parse_json(&body, request_id.as_str())?;
    let Value::Object(map) = &value else {
        return Err(params::field_error(
            request_id.as_str(),
            "$",
            "invalid_type",
            "请求体必须是 JSON 对象",
        ));
    };
    params::reject_unknown_keys(
        map,
        |key| matches!(key, "format" | "content"),
        "",
        request_id.as_str(),
    )?;

    let format = map.get("format").and_then(Value::as_str).ok_or_else(|| {
        params::field_error(
            request_id.as_str(),
            "format",
            "missing_required",
            "缺少 format（toml 或 json）",
        )
    })?;
    if format != "toml" && format != "json" {
        return Err(params::field_error(
            request_id.as_str(),
            "format",
            "invalid_value",
            format!("format 只支持 toml 或 json，收到 {format:?}"),
        ));
    }
    let Some(content) = map.get("content") else {
        return Err(params::field_error(
            request_id.as_str(),
            "content",
            "missing_required",
            "缺少 content",
        ));
    };

    let (errors, warnings) = match format {
        "toml" => {
            let Some(text) = content.as_str() else {
                return Err(params::field_error(
                    request_id.as_str(),
                    "content",
                    "invalid_type",
                    "format=toml 时 content 必须是字符串",
                ));
            };
            validate_toml(text, request_id.as_str())
        }
        _ => validate_json_document(content, request_id.as_str())?,
    };

    let snapshot = store::read_snapshot(state.store()).map_err(|error| {
        ApiError::internal_error(request_id.as_str(), format!("读取配置失败: {error}"))
    })?;
    let body = ValidateResponse {
        valid: errors.is_empty(),
        revision: snapshot.revision,
        errors,
        warnings,
    };
    Ok(json(StatusCode::OK, body))
}

fn build_document_text(value: &Value, request_id: &str) -> Result<String, ApiError> {
    let Value::Object(map) = value else {
        return Err(params::field_error(
            request_id,
            "$",
            "invalid_type",
            "ConfigDocument 必须是 JSON 对象",
        ));
    };
    params::reject_unknown_keys(map, is_known_config_key, "", request_id)?;
    let nested = nested_unknown_keys(value);
    if !nested.is_empty() {
        return Err(params::validation_failed(request_id, nested));
    }
    let config: Config = params::typed(value, request_id)?;
    // 契约 API.md 6.3：同一 package 多条内联记录 → 422 duplicate_app_record。
    let mut seen = std::collections::HashSet::new();
    for app in &config.apps {
        if !seen.insert(app.package.as_str()) {
            return Err(params::field_error(
                request_id,
                &format!("apps.{}", app.package),
                "duplicate_app_record",
                format!("包名 {} 有多条 [[apps]] 条目", app.package),
            ));
        }
    }
    document_from_config(&config)
        .map_err(|error| ApiError::internal_error(request_id, format!("配置序列化失败: {error}")))
}

/// ConfigDocument 里 templates.<name> / apps[i] 的未知键（docs/API.md 6.1）。
fn nested_unknown_keys(value: &Value) -> Vec<ValidationError> {
    let Value::Object(map) = value else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    if let Some(Value::Object(templates)) = map.get("templates") {
        for (name, template) in templates {
            let Value::Object(fields) = template else {
                continue;
            };
            for key in fields.keys().filter(|key| !is_known_template_key(key)) {
                errors.push(ValidationError::new(
                    format!("templates.{name}.{key}"),
                    "unknown_field",
                    format!("未知字段 {key}"),
                ));
            }
        }
    }
    if let Some(Value::Array(apps)) = map.get("apps") {
        for (index, app) in apps.iter().enumerate() {
            let Value::Object(fields) = app else {
                continue;
            };
            for key in fields.keys().filter(|key| !is_known_app_key(key)) {
                errors.push(ValidationError::new(
                    format!("apps[{index}].{key}"),
                    "unknown_field",
                    format!("未知字段 {key}"),
                ));
            }
        }
    }
    errors
}

fn validate_toml(text: &str, _request_id: &str) -> (Vec<ValidationError>, Vec<Warning>) {
    match text.parse::<toml_edit::DocumentMut>() {
        Ok(document) => {
            let report = device_faker_core::validate::validate_document(
                &document,
                &device_faker_core::validate::ValidationOptions::default(),
            );
            let (mut errors, mut warnings) = map_validation_report(&report);
            if let Ok(config) = Config::from_toml(text) {
                errors.extend(crate::dto::limit_issues(&config));
                warnings.extend(crate::dto::duplicate_package_warnings(&config));
            }
            (errors, warnings)
        }
        Err(error) => (
            vec![ValidationError::new(
                "$",
                "syntax_error",
                format!("TOML 解析失败: {error}"),
            )],
            Vec::new(),
        ),
    }
}

fn validate_json_document(
    content: &Value,
    request_id: &str,
) -> Result<(Vec<ValidationError>, Vec<Warning>), ApiError> {
    let value = match content {
        Value::String(text) => params::parse_json(text.as_bytes(), request_id)?,
        other => other.clone(),
    };
    let Value::Object(map) = &value else {
        return Ok((
            vec![ValidationError::new(
                "$",
                "invalid_type",
                "ConfigDocument 必须是 JSON 对象",
            )],
            Vec::new(),
        ));
    };

    let mut errors = nested_unknown_keys(&value);
    let unknown: Vec<String> = map
        .keys()
        .filter(|key| !is_known_config_key(key))
        .cloned()
        .collect();
    for key in unknown {
        errors.push(ValidationError::new(
            key.clone(),
            "unknown_field",
            format!("未知字段 {key}"),
        ));
    }
    if !errors.is_empty() {
        return Ok((errors, Vec::new()));
    }

    match params::typed_result::<Config>(&value) {
        Ok(config) => {
            let report = device_faker_core::validate::validate_config(
                &config,
                &device_faker_core::validate::ValidationOptions::default(),
            );
            let (mut mapped_errors, mut warnings) = map_validation_report(&report);
            mapped_errors.extend(crate::dto::limit_issues(&config));
            warnings.extend(crate::dto::duplicate_package_warnings(&config));
            Ok((mapped_errors, warnings))
        }
        Err(error) => Ok((vec![error], Vec::new())),
    }
}
