// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 模板端点（docs/API.md 5.8-5.14）。
//!
//! 写路径全部走 [`crate::store::transact`]：同一把 flock 内完成 If-Match、toml_edit 定点修改、
//! 语义校验、备份与原子写；注释、键顺序与未知字段由 core 的编辑函数保留。

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use device_faker_core::Config;
use device_faker_core::config::{DeviceTemplate, is_known_template_key};
use device_faker_core::edit::{delete_template, unbind_package, upsert_template};
use device_faker_core::validate::is_valid_package_key;
use serde_json::{Map, Value, json};

use crate::api::{self, json as json_response, with_revision, with_revision_etag};
use crate::app::AppState;
use crate::dto::{
    TemplateListItem, TemplateListResponse, TemplateResponse, Warning, document_warnings,
    duplicate_package_warnings, template_fields_set,
};
use crate::error::ApiError;
use crate::params::{self, Params};
use crate::requests::RequestId;
use crate::store::{self, TxError};

/// 契约允许的模板名（`^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`，docs/API.md 8.1）。
pub fn is_valid_template_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_alphanumeric() || first == b'_') {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// GET /api/v1/templates
pub async fn list(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let page = params.page(request_id.as_str())?;
    let filter = match params.raw("package") {
        Some(package) if !is_valid_package_key(package) => {
            return Err(params::field_error(
                request_id.as_str(),
                "package",
                "invalid_package",
                format!("不是合法包名: {package:?}"),
            ));
        }
        Some(package) => Some(package.to_owned()),
        None => None,
    };

    let snapshot = store::read_snapshot(state.store())
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error}")))?;
    let config = Config::from_toml(&snapshot.text)
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error:#}")))?;

    let items: Vec<TemplateListItem> = config
        .templates
        .iter()
        .enumerate()
        .filter(|(_, (_, template))| match &filter {
            Some(package) => template.packages.iter().any(|entry| entry == package),
            None => true,
        })
        .map(|(position, (name, template))| TemplateListItem {
            name: name.clone(),
            position,
            packages: template.packages.clone(),
            package_count: template.packages.len(),
            fields_set: template_fields_set(template),
        })
        .collect();

    let total = items.len();
    let (slice, next_cursor) = page.slice(&items);
    let body = TemplateListResponse {
        revision: snapshot.revision.clone(),
        total,
        next_cursor,
        items: slice.to_vec(),
    };
    Ok(with_revision(
        json_response(StatusCode::OK, body),
        &snapshot.revision,
    ))
}

/// GET /api/v1/templates/{name}
pub async fn get(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let snapshot = store::read_snapshot(state.store())
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error}")))?;
    let config = Config::from_toml(&snapshot.text)
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error:#}")))?;
    let Some((position, _, template)) = config.templates.get_full(&name) else {
        return Err(ApiError::not_found(request_id.as_str(), "template", &name));
    };
    let body = TemplateResponse {
        revision: snapshot.revision.clone(),
        position,
        name: name.clone(),
        template: template.clone(),
    };
    Ok(with_revision_etag(
        json_response(StatusCode::OK, body),
        &snapshot.revision,
    ))
}

/// PUT /api/v1/templates/{name}
pub async fn put(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let dry_run = params.flag("dry_run", request_id.as_str())?;
    let want_diff = params.flag("diff", request_id.as_str())?;
    let allow_duplicate = params.flag("allow_duplicate_package", request_id.as_str())?;
    let precondition = store::require_if_match(&headers, request_id.as_str())?;
    if !is_valid_template_name(&name) {
        return Err(params::field_error(
            request_id.as_str(),
            "name",
            "invalid_template_name",
            format!("模板名不合法: {name:?}（允许 A-Za-z0-9_.-，首字符不能是 . 或 -，最长 64）"),
        ));
    }
    params::require_json(&headers, request_id.as_str())?;
    let value = params::parse_json(&body, request_id.as_str())?;
    let template = parse_template_input(&value, request_id.as_str())?;

    let request_id_str = request_id.as_str().to_owned();
    let name_for_tx = name.clone();
    let template_for_tx = template.clone();
    let (created, outcome) = store::transact(
        state.store(),
        &precondition,
        dry_run,
        move |document, config| {
            let existed = config.templates.contains_key(&name_for_tx);
            let previous: Vec<String> = config
                .templates
                .get(&name_for_tx)
                .map(|template| template.packages.clone())
                .unwrap_or_default();
            upsert_template(document, &name_for_tx, &template_for_tx)?;

            let candidate = candidate_config(document, &request_id_str)?;
            let newly_added: Vec<String> = template_for_tx
                .packages
                .iter()
                .filter(|package| !previous.contains(package))
                .cloned()
                .collect();
            if !allow_duplicate
                && let Some((package, templates)) =
                    first_conflict(&candidate, &newly_added, &name_for_tx)
            {
                return Err(TxError::Domain(ApiError::conflict_duplicate_package(
                    &request_id_str,
                    &package,
                    &templates,
                )));
            }
            Ok(!existed)
        },
    )
    .map_err(|error| error.into_api(request_id.as_str()))?;

    let mut warnings = document_warnings(&outcome.new_text);
    warnings.extend(template_warnings(
        &state,
        &outcome.new_text,
        &name,
        &template,
    ));
    let result = api::write_result(&outcome, dry_run, want_diff, warnings);
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok(with_revision(
        json_response(status, result),
        &outcome.revision,
    ))
}

/// PATCH /api/v1/templates/{name}
pub async fn patch(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let dry_run = params.flag("dry_run", request_id.as_str())?;
    let want_diff = params.flag("diff", request_id.as_str())?;
    let allow_duplicate = params.flag("allow_duplicate_package", request_id.as_str())?;
    let precondition = store::require_if_match(&headers, request_id.as_str())?;
    params::require_json(&headers, request_id.as_str())?;
    let patch = params::parse_json(&body, request_id.as_str())?;

    let request_id_str = request_id.as_str().to_owned();
    let name_for_tx = name.clone();
    let (template, outcome) = store::transact(
        state.store(),
        &precondition,
        dry_run,
        move |document, config| {
            let Some(existing) = config.templates.get(&name_for_tx) else {
                return Err(TxError::Domain(ApiError::not_found(
                    &request_id_str,
                    "template",
                    &name_for_tx,
                )));
            };
            let previous = existing.packages.clone();
            let merged = merge_patch(existing, &patch, &request_id_str)?;
            upsert_template(document, &name_for_tx, &merged)?;

            let candidate = candidate_config(document, &request_id_str)?;
            let newly_added: Vec<String> = merged
                .packages
                .iter()
                .filter(|package| !previous.contains(package))
                .cloned()
                .collect();
            if !allow_duplicate
                && let Some((package, templates)) =
                    first_conflict(&candidate, &newly_added, &name_for_tx)
            {
                return Err(TxError::Domain(ApiError::conflict_duplicate_package(
                    &request_id_str,
                    &package,
                    &templates,
                )));
            }
            Ok(merged)
        },
    )
    .map_err(|error| error.into_api(request_id.as_str()))?;

    let mut warnings = document_warnings(&outcome.new_text);
    warnings.extend(template_warnings(
        &state,
        &outcome.new_text,
        &name,
        &template,
    ));
    let result = api::write_result(&outcome, dry_run, want_diff, warnings);
    Ok(with_revision(
        json_response(StatusCode::OK, result),
        &outcome.revision,
    ))
}

/// DELETE /api/v1/templates/{name}
pub async fn delete(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let dry_run = params.flag("dry_run", request_id.as_str())?;
    let want_diff = params.flag("diff", request_id.as_str())?;
    params.confirm("delete_template", request_id.as_str())?;
    let precondition = store::require_if_match(&headers, request_id.as_str())?;

    let request_id_str = request_id.as_str().to_owned();
    let name_for_tx = name.clone();
    let ((), outcome) = store::transact(
        state.store(),
        &precondition,
        dry_run,
        move |document, _config| {
            if !delete_template(document, &name_for_tx) {
                return Err(TxError::Domain(ApiError::not_found(
                    &request_id_str,
                    "template",
                    &name_for_tx,
                )));
            }
            Ok(())
        },
    )
    .map_err(|error| error.into_api(request_id.as_str()))?;

    let warnings = document_warnings(&outcome.new_text);
    let result = api::write_result(&outcome, dry_run, want_diff, warnings);
    Ok(with_revision(
        json_response(StatusCode::OK, result),
        &outcome.revision,
    ))
}

/// POST /api/v1/templates/{name}/packages
pub async fn bind_packages(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let dry_run = params.flag("dry_run", request_id.as_str())?;
    let want_diff = params.flag("diff", request_id.as_str())?;
    let allow_duplicate = params.flag("allow_duplicate_package", request_id.as_str())?;
    let precondition = store::require_if_match(&headers, request_id.as_str())?;
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
        |key| matches!(key, "packages" | "mode"),
        "",
        request_id.as_str(),
    )?;
    let packages = parse_packages(map.get("packages"), request_id.as_str())?;
    let mode = match map.get("mode").and_then(Value::as_str) {
        None => "append".to_owned(),
        Some("append") => "append".to_owned(),
        Some("replace") => "replace".to_owned(),
        Some(other) => {
            return Err(params::field_error(
                request_id.as_str(),
                "mode",
                "invalid_value",
                format!("mode 只支持 append 或 replace，收到 {other:?}"),
            ));
        }
    };

    let request_id_str = request_id.as_str().to_owned();
    let name_for_tx = name.clone();
    let packages_for_tx = packages.clone();
    let mode_for_tx = mode.clone();
    let (final_packages, outcome) = store::transact(
        state.store(),
        &precondition,
        dry_run,
        move |document, config| {
            let Some(existing) = config.templates.get(&name_for_tx) else {
                return Err(TxError::Domain(ApiError::not_found(
                    &request_id_str,
                    "template",
                    &name_for_tx,
                )));
            };
            let mut updated = existing.clone();
            if mode_for_tx == "replace" {
                updated.packages = packages_for_tx.clone();
            } else {
                for package in &packages_for_tx {
                    if !updated.packages.contains(package) {
                        updated.packages.push(package.clone());
                    }
                }
            }
            upsert_template(document, &name_for_tx, &updated)?;

            let candidate = candidate_config(document, &request_id_str)?;
            let newly_added: Vec<String> = updated
                .packages
                .iter()
                .filter(|package| !existing.packages.contains(package))
                .cloned()
                .collect();
            if !allow_duplicate
                && let Some((package, templates)) =
                    first_conflict(&candidate, &newly_added, &name_for_tx)
            {
                return Err(TxError::Domain(ApiError::conflict_duplicate_package(
                    &request_id_str,
                    &package,
                    &templates,
                )));
            }
            Ok(updated.packages.clone())
        },
    )
    .map_err(|error| error.into_api(request_id.as_str()))?;

    let _ = final_packages;
    let template = store::read_snapshot(state.store())
        .ok()
        .and_then(|snapshot| Config::from_toml(&snapshot.text).ok())
        .and_then(|config| config.templates.get(&name).cloned())
        .unwrap_or_default();
    let mut warnings = document_warnings(&outcome.new_text);
    warnings.extend(template_warnings(
        &state,
        &outcome.new_text,
        &name,
        &template,
    ));
    let result = api::write_result(&outcome, dry_run, want_diff, warnings);
    Ok(with_revision(
        json_response(StatusCode::OK, result),
        &outcome.revision,
    ))
}

/// DELETE /api/v1/templates/{name}/packages/{pkg}
pub async fn unbind_package_handler(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path((name, package)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let dry_run = params.flag("dry_run", request_id.as_str())?;
    let want_diff = params.flag("diff", request_id.as_str())?;
    let precondition = store::require_if_match(&headers, request_id.as_str())?;

    let request_id_str = request_id.as_str().to_owned();
    let name_for_tx = name.clone();
    let package_for_tx = package.clone();
    let ((), outcome) = store::transact(
        state.store(),
        &precondition,
        dry_run,
        move |document, config| {
            let Some(existing) = config.templates.get(&name_for_tx) else {
                return Err(TxError::Domain(ApiError::not_found(
                    &request_id_str,
                    "template",
                    &name_for_tx,
                )));
            };
            if !existing
                .packages
                .iter()
                .any(|entry| entry == &package_for_tx)
            {
                return Err(TxError::Domain(ApiError::not_found(
                    &request_id_str,
                    "template_package",
                    &package_for_tx,
                )));
            }
            unbind_package(document, &name_for_tx, &package_for_tx)?;
            Ok(())
        },
    )
    .map_err(|error| error.into_api(request_id.as_str()))?;

    let mut warnings = document_warnings(&outcome.new_text);
    let template = store::read_snapshot(state.store())
        .ok()
        .and_then(|snapshot| Config::from_toml(&snapshot.text).ok())
        .and_then(|config| config.templates.get(&name).cloned())
        .unwrap_or_default();
    if template.packages.is_empty() {
        warnings.push(Warning::new(
            "empty_packages",
            format!("模板 {name} 没有绑定任何包名"),
            Some(json!({ "template": name })),
        ));
    }
    let result = api::write_result(&outcome, dry_run, want_diff, warnings);
    Ok(with_revision(
        json_response(StatusCode::OK, result),
        &outcome.revision,
    ))
}

fn candidate_config(
    document: &toml_edit::DocumentMut,
    request_id: &str,
) -> Result<Config, TxError> {
    Config::from_toml(&document.to_string()).map_err(|error| {
        TxError::Domain(ApiError::internal_error(request_id, format!("{error:#}")))
    })
}

fn first_conflict(
    candidate: &Config,
    packages: &[String],
    own_template: &str,
) -> Option<(String, Vec<String>)> {
    for package in packages {
        let templates: Vec<String> = candidate
            .conflicting_templates_for_package(package)
            .into_iter()
            .filter(|name| *name != own_template)
            .map(str::to_owned)
            .collect();
        if !templates.is_empty() {
            return Some((package.clone(), templates));
        }
    }
    None
}

fn template_warnings(
    state: &AppState,
    new_text: &str,
    name: &str,
    template: &DeviceTemplate,
) -> Vec<Warning> {
    let mut warnings = Vec::new();
    if template.packages.is_empty() {
        warnings.push(Warning::new(
            "empty_packages",
            format!("模板 {name} 没有绑定任何包名"),
            Some(json!({ "template": name })),
        ));
    }
    if let Ok(config) = Config::from_toml(new_text) {
        for duplicate in duplicate_package_warnings(&config) {
            let Some(details) = duplicate.details.as_ref() else {
                continue;
            };
            let templates = details
                .get("templates")
                .and_then(Value::as_array)
                .map(|entries| entries.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default();
            if templates.contains(&name) {
                warnings.push(duplicate);
            }
        }
    }
    warnings.extend(crate::dto::package_not_installed_warnings(
        template.packages.clone(),
        |package| state.packages().is_installed(package),
    ));
    warnings
}

fn parse_template_input(value: &Value, request_id: &str) -> Result<DeviceTemplate, ApiError> {
    let Value::Object(map) = value else {
        return Err(params::field_error(
            request_id,
            "$",
            "invalid_type",
            "TemplateInput 必须是 JSON 对象",
        ));
    };
    params::reject_unknown_keys(map, is_known_template_key, "template", request_id)?;
    params::typed(value, request_id)
}

fn merge_patch(
    existing: &DeviceTemplate,
    patch: &Value,
    request_id: &str,
) -> Result<DeviceTemplate, ApiError> {
    let Value::Object(patch) = patch else {
        return Err(params::field_error(
            request_id,
            "$",
            "invalid_type",
            "PATCH 请求体必须是 JSON 对象",
        ));
    };
    params::reject_unknown_keys(patch, is_known_template_key, "template", request_id)?;
    let mut base: Map<String, Value> = match serde_json::to_value(existing) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for (key, value) in patch {
        if value.is_null() {
            base.remove(key);
        } else {
            base.insert(key.clone(), value.clone());
        }
    }
    params::typed(&Value::Object(base), request_id)
}

fn parse_packages(value: Option<&Value>, request_id: &str) -> Result<Vec<String>, ApiError> {
    let Some(Value::Array(entries)) = value else {
        return Err(params::field_error(
            request_id,
            "packages",
            "missing_required",
            "packages 必须是字符串数组",
        ));
    };
    if entries.is_empty() || entries.len() > 200 {
        return Err(params::field_error(
            request_id,
            "packages",
            "out_of_range",
            "packages 必须是 1..=200 项",
        ));
    }
    let mut packages: Vec<String> = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(package) = entry.as_str() else {
            return Err(params::field_error(
                request_id,
                "packages",
                "invalid_type",
                "packages 每一项都必须是字符串",
            ));
        };
        if !is_valid_package_key(package) {
            return Err(params::field_error(
                request_id,
                "packages",
                "invalid_package",
                format!("不是合法包名: {package:?}"),
            ));
        }
        if !packages.iter().any(|entry| entry == package) {
            packages.push(package.to_owned());
        }
    }
    Ok(packages)
}

/// 供其他端点复用：校验包名并给出 422。
pub fn ensure_package(package: &str, path: &str, request_id: &str) -> Result<(), ApiError> {
    if is_valid_package_key(package) {
        Ok(())
    } else {
        Err(params::field_error(
            request_id,
            path,
            "invalid_package",
            format!("不是合法包名: {package:?}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_name_rule_matches_contract() {
        assert!(is_valid_template_name("pixel_9"));
        assert!(is_valid_template_name("a.b-c_1"));
        assert!(is_valid_template_name("_x"));
        assert!(!is_valid_template_name(""));
        assert!(!is_valid_template_name(".hidden"));
        assert!(!is_valid_template_name("-dash"));
        assert!(!is_valid_template_name("has space"));
        assert!(!is_valid_template_name(&"a".repeat(65)));
    }

    #[test]
    fn merge_patch_deletes_null_and_keeps_absent() {
        let existing: DeviceTemplate = serde_json::from_value(json!({
            "packages": ["com.example.app"],
            "model": "Pixel 9",
            "dpi": 420
        }))
        .unwrap();
        let patch = json!({ "dpi": 440, "model": null, "brand": "google" });
        let merged = merge_patch(&existing, &patch, "req").unwrap();
        assert_eq!(merged.dpi, Some(440));
        assert_eq!(merged.model, None);
        assert_eq!(merged.brand.as_deref(), Some("google"));
        assert_eq!(merged.packages, vec!["com.example.app".to_owned()]);
    }

    #[test]
    fn merge_patch_rejects_unknown_keys() {
        let existing = DeviceTemplate::default();
        let error = merge_patch(&existing, &json!({ "bogus": 1 }), "req").unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::ValidationFailed);
    }

    #[test]
    fn packages_must_be_deduped_and_valid() {
        let packages =
            parse_packages(Some(&json!(["com.example.app", "com.example.app"])), "req").unwrap();
        assert_eq!(packages, vec!["com.example.app".to_owned()]);
        assert!(parse_packages(Some(&json!([])), "req").is_err());
        assert!(parse_packages(Some(&json!(["not a package"])), "req").is_err());
    }

    #[test]
    fn first_conflict_ignores_own_template() {
        let config = Config::from_toml(
            "[templates.a]\npackages = [\"com.example.one\"]\n[templates.b]\npackages = [\"com.example.one\"]\n",
        )
        .unwrap();
        let conflict = first_conflict(&config, &["com.example.one".to_owned()], "a").unwrap();
        assert_eq!(conflict.0, "com.example.one");
        assert_eq!(conflict.1, vec!["b".to_owned()]);
        assert!(first_conflict(&config, &["com.example.one".to_owned()], "b").is_some());
        assert!(first_conflict(&config, &["com.example.two".to_owned()], "a").is_none());
    }
}
