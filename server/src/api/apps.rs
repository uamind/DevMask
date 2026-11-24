// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 应用端点（docs/API.md 5.15-5.19）。
//!
//! `GET /apps/{pkg}` 同时承担「解释为什么生效/不生效」：返回来源（inline / template / none）、
//! 是否安装与最终生效值（core 的 `get_merged_config`，去掉指纹派生属性）。

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use device_faker_core::Config;
use device_faker_core::config::{AppConfig, is_known_app_key};
use device_faker_core::edit::{bind_package, delete_app, set_app, unbind_package};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::api::{self, json as json_response, with_revision, with_revision_etag};
use crate::app::AppState;
use crate::dto::{
    AppListItem, AppListResponse, AppResolved, AppResponse, AppSource, Warning, app_fields_set,
    document_warnings, duplicate_package_warnings,
};
use crate::error::ApiError;
use crate::packages::{base_package, split_user};
use crate::params::{self, Params};
use crate::requests::RequestId;
use crate::store::{self, TxError};

/// PUT /apps/{pkg} 的请求体。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpsertRequest {
    /// 模式 A：绑定模板名。
    #[serde(default)]
    pub template: Option<String>,
    /// 模式 B：内联配置。
    #[serde(default)]
    pub config: Option<Value>,
    /// 是否把该包从其它模板解绑（A 默认 true，B 默认 false）。
    #[serde(default)]
    pub detach_from_templates: Option<bool>,
}

/// POST /apps/{pkg}/restart 的请求体（字段全可选）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartRequest {
    /// 是否 force-stop（默认 true）。
    #[serde(default)]
    pub force_stop: Option<bool>,
    /// 是否重新拉起（默认 false）。
    #[serde(default)]
    pub launch: Option<bool>,
    /// Android 用户 id（默认 0；路径里的 @uid 优先）。
    #[serde(default)]
    pub user: Option<u32>,
}

/// GET /api/v1/apps
pub async fn list(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let page = params.page(request_id.as_str())?;
    let source_filter = match params.raw("source") {
        None => None,
        Some("inline") => Some("inline"),
        Some("template") => Some("template"),
        Some(other) => {
            return Err(params::field_error(
                request_id.as_str(),
                "source",
                "invalid_value",
                format!("source 只支持 inline 或 template，收到 {other:?}"),
            ));
        }
    };

    let snapshot = store::read_snapshot(state.store())
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error}")))?;
    let config = Config::from_toml(&snapshot.text)
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error:#}")))?;

    // 列表只包含 [[apps]] 内联记录；source=template 时按契约返回空列表。
    let items: Vec<AppListItem> = if source_filter == Some("template") {
        Vec::new()
    } else {
        config
            .apps
            .iter()
            .enumerate()
            .map(|(position, app)| AppListItem {
                package: app.package.clone(),
                position,
                source: AppSource {
                    source_type: "inline",
                    template: None,
                    also_inline: false,
                },
                fields_set: app_fields_set(app),
            })
            .collect()
    };

    let total = items.len();
    let (slice, next_cursor) = page.slice(&items);
    let body = AppListResponse {
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

/// GET /api/v1/apps/{pkg}
pub async fn get(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(package): Path<String>,
) -> Result<Response, ApiError> {
    crate::api::templates::ensure_package(&package, "package", request_id.as_str())?;
    let snapshot = store::read_snapshot(state.store())
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error}")))?;
    let config = Config::from_toml(&snapshot.text)
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error:#}")))?;

    let inline = config.apps.iter().any(|app| app.package == package);
    let templates: Vec<&str> = config.conflicting_templates_for_package(&package);
    let source = if inline {
        AppSource {
            source_type: "inline",
            template: None,
            also_inline: !templates.is_empty(),
        }
    } else if let Some(name) = templates.first() {
        AppSource {
            source_type: "template",
            template: Some((*name).to_owned()),
            also_inline: false,
        }
    } else {
        AppSource {
            source_type: "none",
            template: None,
            also_inline: false,
        }
    };

    let installed = state.packages().is_installed(base_package(&package));
    let resolved = match config.get_merged_config(&package) {
        Some(merged) => {
            serde_json::to_value(AppResolved::from_merged(&merged)).unwrap_or_else(|_| json!({}))
        }
        None => json!({}),
    };

    let mut warnings = Vec::new();
    if source.source_type == "none" {
        warnings.push(Warning::new(
            "no_config",
            format!("{package} 没有任何内联配置或模板绑定"),
            Some(json!({ "package": package })),
        ));
    }
    if installed == Some(false) {
        warnings.push(Warning::new(
            "package_not_installed",
            format!("包名 {package} 当前未安装，配置仍会保留"),
            Some(json!({ "package": package })),
        ));
    }
    if templates.len() > 1 {
        warnings.push(Warning::new(
            "duplicate_package",
            format!("{package} 同时被 {} 声明", templates.join(" 与 ")),
            Some(json!({
                "package": package,
                "templates": templates,
            })),
        ));
    }

    let body = AppResponse {
        package: package.clone(),
        revision: snapshot.revision.clone(),
        installed,
        source,
        resolved,
        warnings,
    };
    Ok(with_revision_etag(
        json_response(StatusCode::OK, body),
        &snapshot.revision,
    ))
}

/// PUT /api/v1/apps/{pkg}
pub async fn put(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(package): Path<String>,
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
    crate::api::templates::ensure_package(&package, "pkg", request_id.as_str())?;

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
        |key| matches!(key, "template" | "config" | "detachFromTemplates"),
        "",
        request_id.as_str(),
    )?;
    let request: AppUpsertRequest = params::typed(&value, request_id.as_str())?;
    if request.template.is_some() == request.config.is_some() {
        return Err(params::field_error(
            request_id.as_str(),
            "template|config",
            "invalid_value",
            "template 与 config 必须恰好提供一个",
        ));
    }

    let request_id_str = request_id.as_str().to_owned();
    let package_for_tx = package.clone();
    if let Some(template_name) = request.template.clone() {
        let detach = request.detach_from_templates.unwrap_or(true);
        let (removed_inline, outcome) = store::transact(
            state.store(),
            &precondition,
            dry_run,
            move |document, config| {
                if !config.templates.contains_key(&template_name) {
                    return Err(TxError::Domain(ApiError::not_found(
                        &request_id_str,
                        "template",
                        &template_name,
                    )));
                }
                let removed_inline = config.apps.iter().any(|app| app.package == package_for_tx);
                if removed_inline {
                    delete_app(document, &package_for_tx)?;
                }
                if detach {
                    for (name, template) in &config.templates {
                        if name != &template_name
                            && template
                                .packages
                                .iter()
                                .any(|entry| entry == &package_for_tx)
                        {
                            unbind_package(document, name, &package_for_tx)?;
                        }
                    }
                }
                let already_bound = config
                    .templates
                    .get(&template_name)
                    .is_some_and(|template| {
                        template
                            .packages
                            .iter()
                            .any(|entry| entry == &package_for_tx)
                    });
                bind_package(document, &template_name, &package_for_tx)?;

                let candidate = Config::from_toml(&document.to_string()).map_err(|error| {
                    TxError::Domain(ApiError::internal_error(
                        &request_id_str,
                        format!("{error:#}"),
                    ))
                })?;
                if !allow_duplicate && !already_bound {
                    let others: Vec<String> = candidate
                        .conflicting_templates_for_package(&package_for_tx)
                        .into_iter()
                        .filter(|name| *name != template_name)
                        .map(str::to_owned)
                        .collect();
                    if !others.is_empty() {
                        return Err(TxError::Domain(ApiError::conflict_duplicate_package(
                            &request_id_str,
                            &package_for_tx,
                            &others,
                        )));
                    }
                }
                Ok(removed_inline)
            },
        )
        .map_err(|error| error.into_api(request_id.as_str()))?;

        let mut warnings = document_warnings(&outcome.new_text);
        if removed_inline {
            warnings.push(Warning::new(
                "inline_record_removed",
                format!("已删除 {package} 的内联记录，让模板绑定生效"),
                Some(json!({ "package": package })),
            ));
        }
        append_package_warnings(&state, &outcome.new_text, &package, &mut warnings);
        let result = api::write_result(&outcome, dry_run, want_diff, warnings);
        return Ok(with_revision(
            json_response(StatusCode::OK, result),
            &outcome.revision,
        ));
    }

    let config_value = request.config.clone().unwrap_or(Value::Null);
    let app = parse_app_config(&config_value, &package, request_id.as_str())?;
    let detach = request.detach_from_templates.unwrap_or(false);
    let (created, outcome) = store::transact(
        state.store(),
        &precondition,
        dry_run,
        move |document, config| {
            let created = !config
                .apps
                .iter()
                .any(|entry| entry.package == package_for_tx);
            if detach {
                for (name, template) in &config.templates {
                    if template
                        .packages
                        .iter()
                        .any(|entry| entry == &package_for_tx)
                    {
                        unbind_package(document, name, &package_for_tx)?;
                    }
                }
            }
            set_app(document, &app)?;
            Ok(created)
        },
    )
    .map_err(|error| error.into_api(request_id.as_str()))?;

    let mut warnings = document_warnings(&outcome.new_text);
    append_package_warnings(&state, &outcome.new_text, &package, &mut warnings);
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

/// DELETE /api/v1/apps/{pkg}
pub async fn delete(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(package): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let dry_run = params.flag("dry_run", request_id.as_str())?;
    let want_diff = params.flag("diff", request_id.as_str())?;
    let detach_templates = params.flag("detach_templates", request_id.as_str())?;
    params.confirm("delete_app", request_id.as_str())?;
    let precondition = store::require_if_match(&headers, request_id.as_str())?;

    let request_id_str = request_id.as_str().to_owned();
    let package_for_tx = package.clone();
    let ((), outcome) = store::transact(
        state.store(),
        &precondition,
        dry_run,
        move |document, config| {
            let mut removed = delete_app(document, &package_for_tx)?;
            if detach_templates {
                for (name, template) in &config.templates {
                    if template
                        .packages
                        .iter()
                        .any(|entry| entry == &package_for_tx)
                    {
                        unbind_package(document, name, &package_for_tx)?;
                        removed = true;
                    }
                }
            }
            if !removed {
                return Err(TxError::Domain(ApiError::not_found(
                    &request_id_str,
                    "app_config",
                    &package_for_tx,
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

/// POST /api/v1/apps/{pkg}/restart（作用域 ops）
pub async fn restart(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(package): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request = parse_restart_request(&headers, &body, request_id.as_str())?;
    let (base, package_user) = split_user(&package);
    let user = package_user.or(request.user).unwrap_or(0);

    if state.packages().is_installed(base) == Some(false) {
        return Err(ApiError::not_found(request_id.as_str(), "package", base));
    }

    let started = std::time::Instant::now();
    let force_stop = request.force_stop.unwrap_or(true);
    let launch = request.launch.unwrap_or(false);
    let user_arg = user.to_string();

    if force_stop {
        let outcome = state
            .commands()
            .run("am", &["force-stop", "--user", &user_arg, base]);
        if !outcome.is_ok() {
            return Err(ApiError::upstream_unavailable(request_id.as_str(), "am"));
        }
    }
    if launch {
        let outcome = state.commands().run(
            "monkey",
            &["-p", base, "-c", "android.intent.category.LAUNCHER", "1"],
        );
        if !outcome.is_ok() {
            return Err(ApiError::upstream_unavailable(
                request_id.as_str(),
                "monkey",
            ));
        }
    }

    let response = json_response(
        StatusCode::OK,
        json!({
            "package": base,
            "forceStopped": force_stop,
            "launched": launch,
            "durationMs": started.elapsed().as_millis() as u64,
        }),
    );
    Ok(response)
}

fn parse_app_config(value: &Value, package: &str, request_id: &str) -> Result<AppConfig, ApiError> {
    let Value::Object(map) = value else {
        return Err(params::field_error(
            request_id,
            "config",
            "invalid_type",
            "config 必须是 JSON 对象",
        ));
    };
    params::reject_unknown_keys(map, is_known_app_key, "config", request_id)?;
    let mut object: Map<String, Value> = map.clone();
    if let Some(declared) = object.get("package").and_then(Value::as_str)
        && declared != package
    {
        return Err(params::field_error(
            request_id,
            "config.package",
            "invalid_value",
            format!("config.package ({declared}) 必须与路径包名 ({package}) 一致"),
        ));
    }
    object.insert("package".to_owned(), Value::String(package.to_owned()));
    params::typed(&Value::Object(object), request_id)
}

fn parse_restart_request(
    headers: &HeaderMap,
    body: &Bytes,
    request_id: &str,
) -> Result<RestartRequest, ApiError> {
    if body.is_empty() {
        return Ok(RestartRequest::default());
    }
    params::require_json(headers, request_id)?;
    let value = params::parse_json(body, request_id)?;
    let Value::Object(map) = &value else {
        return Err(params::field_error(
            request_id,
            "$",
            "invalid_type",
            "请求体必须是 JSON 对象",
        ));
    };
    params::reject_unknown_keys(
        map,
        |key| matches!(key, "forceStop" | "launch" | "user"),
        "",
        request_id,
    )?;
    params::typed(&value, request_id)
}

fn append_package_warnings(
    state: &AppState,
    new_text: &str,
    package: &str,
    warnings: &mut Vec<Warning>,
) {
    if let Ok(config) = Config::from_toml(new_text) {
        for duplicate in duplicate_package_warnings(&config) {
            let matches = duplicate
                .details
                .as_ref()
                .and_then(|details| details.get("package"))
                .and_then(Value::as_str)
                == Some(package);
            if matches {
                warnings.push(duplicate);
            }
        }
    }
    if state.packages().is_installed(base_package(package)) == Some(false) {
        warnings.push(Warning::new(
            "package_not_installed",
            format!("包名 {package} 当前未安装，配置仍会保留"),
            Some(json!({ "package": package })),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_package_must_match_path() {
        let value = json!({ "package": "com.other.app", "model": "X" });
        let error = parse_app_config(&value, "com.example.app", "req").unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::ValidationFailed);
    }

    #[test]
    fn config_package_is_injected_when_absent() {
        let value = json!({ "model": "X" });
        let app = parse_app_config(&value, "com.example.app", "req").unwrap();
        assert_eq!(app.package, "com.example.app");
        assert_eq!(app.model.as_deref(), Some("X"));
    }

    #[test]
    fn restart_request_defaults_when_body_missing() {
        let request = parse_restart_request(&HeaderMap::new(), &Bytes::new(), "req").unwrap();
        assert!(request.force_stop.is_none());
        assert!(request.launch.is_none());
    }
}
