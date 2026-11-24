// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 设备注册、探测、同步、详情与 overview（API.md 4.2-4.9）。

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, RawQuery, State, rejection::JsonRejection},
    http::StatusCode,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::{
    api::{
        dto::{
            AddDeviceRequest, DeviceAppSummary, DeviceDetail, DeviceListResponse,
            DeviceOverviewResponse, DeviceSummary, SyncAllResponse, SyncAllResult,
            SyncDeviceResponse, TemplateSummary, UpdateDeviceRequest, Warning, overview_apps,
            overview_meta, overview_templates, overview_warnings, parse_overview, stale_state,
        },
        query::QueryParams,
    },
    db::{DeviceProbeUpdate, DeviceRecord, StoredOverview},
    device_client::{DeviceCallError, OfflineReason, normalize_base_url},
    error::{ApiError, ValidationError},
    ids, paging, request_id,
    state::AppState,
};

fn request_timeout(state: &AppState) -> Duration {
    Duration::from_millis(state.cfg.request_timeout_ms)
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
}

fn log_db_write<T>(result: Result<T, crate::db::DbError>, op: &'static str, device_id: &str) {
    if let Err(error) = result {
        tracing::error!(error = %error, op, device_id, "数据库写入失败");
    }
}

async fn load_device(state: &AppState, device_id: &str) -> Result<DeviceRecord, ApiError> {
    state
        .db
        .get_device(device_id)
        .await
        .map_err(|error| internal("devices.get", error))?
        .ok_or_else(|| ApiError::device_not_found(device_id))
}

pub(crate) async fn write_audit(
    state: &AppState,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    request_id: Option<&str>,
    status: &str,
    detail: Value,
) {
    if let Err(error) = state
        .db
        .insert_audit(
            action,
            target_type,
            target_id,
            request_id,
            status,
            Some(detail),
            &ids::now_rfc3339(),
        )
        .await
    {
        tracing::warn!(error = %error, action, "写入审计失败");
    }
}

fn json_body<T>(payload: Result<Json<T>, JsonRejection>, path: &str) -> Result<T, ApiError> {
    payload.map(|Json(value)| value).map_err(|rejection| {
        let status = rejection.status();
        if status == StatusCode::UNSUPPORTED_MEDIA_TYPE {
            return ApiError::unsupported_media_type();
        }
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            return ApiError::body_too_large(crate::api::BODY_LIMIT_BYTES);
        }
        let message = rejection.body_text();
        let code = if message.contains("unknown field") {
            "unknown_field"
        } else if message.contains("missing field") {
            "missing_required"
        } else if message.contains("invalid type") {
            "invalid_type"
        } else {
            "invalid_value"
        };
        ApiError::validation(vec![ValidationError::new(path, code, message)])
    })
}

/// 校验后的添加设备输入。
struct PreparedDevice {
    id: String,
    name: String,
    base_url: String,
    token: Zeroizing<String>,
    tags: Vec<String>,
}

fn validate_tags(raw: Vec<String>, errors: &mut Vec<ValidationError>) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for (index, tag) in raw.into_iter().enumerate() {
        let tag = tag.trim().to_string();
        if tag.is_empty() || tag.chars().count() > 32 {
            errors.push(ValidationError::new(
                format!("tags[{index}]"),
                "out_of_range",
                "标签需为 1-32 字符",
            ));
            continue;
        }
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    if tags.len() > 8 {
        errors.push(ValidationError::new(
            "tags",
            "too_many_items",
            "标签最多 8 个",
        ));
        tags.truncate(8);
    }
    tags
}

fn validate_token(token: &str, errors: &mut Vec<ValidationError>) {
    if token.is_empty()
        || token.len() > 256
        || token.contains('\n')
        || token.contains('\r')
        || token.chars().any(char::is_control)
    {
        errors.push(ValidationError::new(
            "token",
            "invalid_token",
            "Token 需为 1-256 字符且不含换行",
        ));
    }
}

fn prepare_add(request: AddDeviceRequest) -> Result<PreparedDevice, ApiError> {
    let mut errors = Vec::new();

    let name = request.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        errors.push(ValidationError::new(
            "name",
            if name.is_empty() {
                "missing_required"
            } else {
                "out_of_range"
            },
            "设备名称需为 1-64 字符",
        ));
    }

    validate_token(&request.token, &mut errors);
    let tags = validate_tags(request.tags.unwrap_or_default(), &mut errors);

    let base_url = match normalize_base_url(&request.base_url) {
        Ok(base_url) => base_url,
        Err(error) => {
            errors.push(ValidationError::new(
                "baseUrl",
                "invalid_base_url",
                error.to_string(),
            ));
            String::new()
        }
    };

    let id = match request.id {
        Some(raw) => {
            let id = raw.trim().to_string();
            if !ids::is_valid_device_id(&id) {
                errors.push(ValidationError::new(
                    "id",
                    "invalid_device_id",
                    "id 需匹配 ^[a-z0-9][a-z0-9_-]{0,63}$",
                ));
            }
            id
        }
        None => ids::slugify_device_id(&name),
    };

    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    Ok(PreparedDevice {
        id,
        name,
        base_url,
        token: Zeroizing::new(request.token),
        tags,
    })
}

/// GET /relay/api/v1/devices
pub async fn list(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<DeviceListResponse>, ApiError> {
    let params = QueryParams::parse(raw.as_deref());
    let online = params.boolean("online")?;
    let tags = params.multi("tag");
    let keyword = params.string("q").map(|value| value.to_lowercase());
    let limit = params.usize("limit")?;
    let cursor = params.string("cursor");
    let page = paging::resolve(limit, cursor.as_deref())?;

    let records = state
        .db
        .list_devices()
        .await
        .map_err(|error| internal("devices.list", error))?;

    let filtered: Vec<DeviceRecord> = records
        .into_iter()
        .filter(|record| {
            if let Some(online) = online
                && record.online != online
            {
                return false;
            }
            if !tags.iter().all(|tag| record.tags.contains(tag)) {
                return false;
            }
            if let Some(keyword) = keyword.as_deref() {
                let haystack = format!(
                    "{} {} {}",
                    record.name.to_lowercase(),
                    record.model.clone().unwrap_or_default().to_lowercase(),
                    record.base_url.to_lowercase()
                );
                if !haystack.contains(keyword) {
                    return false;
                }
            }
            true
        })
        .collect();

    let total = filtered.len();
    let online_count = filtered.iter().filter(|record| record.online).count();
    let start = page.offset.min(total);
    let devices: Vec<DeviceSummary> = filtered
        .into_iter()
        .skip(start)
        .take(page.limit)
        .map(DeviceSummary::from)
        .collect();
    let next_cursor = paging::next_cursor(total, page.offset, devices.len());

    Ok(Json(DeviceListResponse {
        devices,
        total,
        online: online_count,
        offline: total - online_count,
        next_cursor,
    }))
}

/// POST /relay/api/v1/devices
pub async fn create(
    State(state): State<AppState>,
    payload: Result<Json<AddDeviceRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<DeviceSummary>), ApiError> {
    let request = json_body(payload, "body")?;
    let prepared = prepare_add(request)?;
    let request_id = request_id::current();

    if let Some(existing) = state
        .db
        .get_device_by_base_url(&prepared.base_url)
        .await
        .map_err(|error| internal("devices.create.duplicate", error))?
    {
        return Err(ApiError::device_exists(
            "baseUrl",
            &prepared.base_url,
            &existing.id,
        ));
    }
    if let Some(existing) = state
        .db
        .get_device(&prepared.id)
        .await
        .map_err(|error| internal("devices.create.duplicate", error))?
    {
        return Err(ApiError::device_exists("id", &prepared.id, &existing.id));
    }

    let probe = match state
        .http
        .probe(
            &prepared.base_url,
            prepared.token.as_str(),
            request_timeout(&state),
            request_id.as_deref(),
        )
        .await
    {
        Ok(probe) => probe,
        Err(failure) => {
            write_audit(
                &state,
                "device.add",
                Some("device"),
                Some(&prepared.id),
                request_id.as_deref(),
                "failed",
                json!({ "baseUrl": prepared.base_url, "reason": failure.reason }),
            )
            .await;
            return Err(ApiError::device_probe_failed(
                &prepared.base_url,
                failure.reason,
                failure.upstream,
                failure.message,
            ));
        }
    };

    let now = ids::now_rfc3339();
    let meta = overview_meta(&probe.overview);
    let token_cipher = state
        .key
        .encrypt(&prepared.id, prepared.token.as_str())
        .map_err(|error| internal("devices.create.encrypt", error))?;

    let record = DeviceRecord {
        id: prepared.id.clone(),
        name: prepared.name.clone(),
        base_url: prepared.base_url.clone(),
        token_cipher,
        tags: prepared.tags.clone(),
        online: true,
        model: meta.model,
        brand: meta.brand,
        android: meta.android,
        sdk: meta.sdk,
        fingerprint: meta.fingerprint,
        root_manager: meta.root_manager,
        module_version: meta.module_version,
        server_version: meta.server_version,
        last_seen: Some(now.clone()),
        last_synced_at: Some(now.clone()),
        latency_ms: Some(probe.latency_ms),
        last_error: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let overview = StoredOverview {
        device_id: prepared.id.clone(),
        revision: meta.revision,
        fetched_at: now,
        payload: probe.overview.to_string(),
    };

    state
        .db
        .insert_device_with_overview(record.clone(), overview)
        .await
        .map_err(|error| internal("devices.create.insert", error))?;

    write_audit(
        &state,
        "device.add",
        Some("device"),
        Some(&prepared.id),
        request_id.as_deref(),
        "ok",
        json!({
            "baseUrl": prepared.base_url,
            "online": true,
            "model": record.model,
        }),
    )
    .await;

    Ok((StatusCode::CREATED, Json(DeviceSummary::from(record))))
}

/// GET /relay/api/v1/devices/{id}
pub async fn get(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<DeviceDetail>, ApiError> {
    let record = load_device(&state, &device_id).await?;
    let stored = state
        .db
        .get_overview(&device_id)
        .await
        .map_err(|error| internal("devices.get.overview", error))?;

    let (overview, templates, apps, cached_at, stale, mut warnings) = match stored {
        Some(stored) => {
            let Some(value) = parse_overview(&stored.payload) else {
                return Err(internal(
                    "devices.get.overview",
                    "缓存的 overview JSON 已损坏",
                ));
            };
            let freshness = stale_state(Some(&stored.fetched_at), record.online);
            let warnings = overview_warnings(&value);
            let templates: Vec<TemplateSummary> = overview_templates(&value, &device_id);
            let apps: Vec<DeviceAppSummary> = overview_apps(&value);
            (
                Some(value),
                templates,
                apps,
                Some(stored.fetched_at),
                freshness.stale,
                warnings,
            )
        }
        None => (
            None,
            Vec::new(),
            Vec::new(),
            None,
            true,
            vec![Warning::new(
                "device_cache_missing",
                "设备尚未同步 overview",
                json!({ "deviceId": device_id }),
            )],
        ),
    };

    if stale && let Some(cached_at) = cached_at.as_deref() {
        let age = stale_state(Some(cached_at), record.online).age_secs;
        warnings.push(Warning::new(
            "stale_cache",
            "设备缓存已过期",
            json!({ "deviceId": device_id, "ageSec": age }),
        ));
    }
    if !record.online {
        let reason = record
            .last_error
            .as_ref()
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        warnings.push(Warning::new(
            "device_offline",
            format!("{} 当前离线", record.name),
            json!({ "deviceId": device_id, "reason": reason }),
        ));
    }

    Ok(Json(DeviceDetail {
        summary: DeviceSummary::from(record),
        overview,
        templates,
        apps,
        cached_at,
        stale,
        warnings,
    }))
}

/// PATCH /relay/api/v1/devices/{id}
pub async fn update(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
    payload: Result<Json<UpdateDeviceRequest>, JsonRejection>,
) -> Result<Json<DeviceSummary>, ApiError> {
    let request = json_body(payload, "body")?;
    let record = load_device(&state, &device_id).await?;
    let request_id = request_id::current();

    if request.name.is_none()
        && request.tags.is_none()
        && request.token.is_none()
        && request.base_url.is_none()
    {
        return Err(ApiError::validation(vec![ValidationError::new(
            "body",
            "missing_required",
            "至少提供一个字段：name / tags / token / baseUrl",
        )]));
    }

    let mut errors = Vec::new();
    let name = match &request.name {
        Some(raw) => {
            let name = raw.trim().to_string();
            if name.is_empty() || name.chars().count() > 64 {
                errors.push(ValidationError::new(
                    "name",
                    "out_of_range",
                    "设备名称需为 1-64 字符",
                ));
            }
            name
        }
        None => record.name.clone(),
    };
    let tags = match request.tags.clone() {
        Some(raw) => validate_tags(raw, &mut errors),
        None => record.tags.clone(),
    };
    if let Some(token) = request.token.as_deref() {
        validate_token(token, &mut errors);
    }
    let base_url = match request.base_url.as_deref() {
        Some(raw) => match normalize_base_url(raw) {
            Ok(base_url) => base_url,
            Err(error) => {
                errors.push(ValidationError::new(
                    "baseUrl",
                    "invalid_base_url",
                    error.to_string(),
                ));
                record.base_url.clone()
            }
        },
        None => record.base_url.clone(),
    };
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if base_url != record.base_url
        && let Some(existing) = state
            .db
            .get_device_by_base_url(&base_url)
            .await
            .map_err(|error| internal("devices.update.duplicate", error))?
        && existing.id != device_id
    {
        return Err(ApiError::device_exists("baseUrl", &base_url, &existing.id));
    }

    let token_changed = request.token.is_some();
    let connection_changed = token_changed || base_url != record.base_url;
    let mut fields = Vec::new();
    if name != record.name {
        fields.push("name");
    }
    if tags != record.tags {
        fields.push("tags");
    }
    if token_changed {
        fields.push("token");
    }
    if base_url != record.base_url {
        fields.push("baseUrl");
    }

    let now = ids::now_rfc3339();

    if connection_changed {
        let token = match request.token.clone() {
            Some(token) => Zeroizing::new(token),
            None => state
                .key
                .decrypt(&device_id, &record.token_cipher)
                .map_err(|error| internal("devices.update.decrypt", error))?,
        };

        match state
            .http
            .probe(
                &base_url,
                token.as_str(),
                request_timeout(&state),
                request_id.as_deref(),
            )
            .await
        {
            Ok(probe) => {
                let meta = overview_meta(&probe.overview);
                if token_changed || base_url != record.base_url {
                    let cipher = if token_changed {
                        state
                            .key
                            .encrypt(&device_id, token.as_str())
                            .map_err(|error| internal("devices.update.encrypt", error))?
                    } else {
                        record.token_cipher.clone()
                    };
                    state
                        .db
                        .update_device_connection(&device_id, &base_url, &cipher, &now)
                        .await
                        .map_err(|error| internal("devices.update.connection", error))?;
                }
                state
                    .db
                    .update_device_probe(DeviceProbeUpdate {
                        id: device_id.clone(),
                        online: true,
                        model: meta.model,
                        brand: meta.brand,
                        android: meta.android,
                        sdk: meta.sdk,
                        fingerprint: meta.fingerprint,
                        root_manager: meta.root_manager,
                        module_version: meta.module_version,
                        server_version: meta.server_version,
                        last_seen: now.clone(),
                        last_synced_at: now.clone(),
                        latency_ms: Some(probe.latency_ms),
                        updated_at: now.clone(),
                    })
                    .await
                    .map_err(|error| internal("devices.update.probe", error))?;
                state
                    .db
                    .upsert_overview(StoredOverview {
                        device_id: device_id.clone(),
                        revision: meta.revision,
                        fetched_at: now.clone(),
                        payload: probe.overview.to_string(),
                    })
                    .await
                    .map_err(|error| internal("devices.update.overview", error))?;
            }
            Err(failure) => {
                write_audit(
                    &state,
                    "device.update",
                    Some("device"),
                    Some(&device_id),
                    request_id.as_deref(),
                    "failed",
                    json!({ "baseUrl": base_url, "reason": failure.reason }),
                )
                .await;
                return Err(ApiError::device_probe_failed(
                    &base_url,
                    failure.reason,
                    failure.upstream,
                    failure.message,
                ));
            }
        }
    }

    if name != record.name || tags != record.tags {
        state
            .db
            .update_device_profile(&device_id, &name, &tags, &now)
            .await
            .map_err(|error| internal("devices.update.profile", error))?;
    }

    let updated = load_device(&state, &device_id).await?;
    write_audit(
        &state,
        "device.update",
        Some("device"),
        Some(&device_id),
        request_id.as_deref(),
        "ok",
        json!({ "fields": fields }),
    )
    .await;

    Ok(Json(DeviceSummary::from(updated)))
}

/// DELETE /relay/api/v1/devices/{id}
pub async fn remove(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    let record = load_device(&state, &device_id).await?;
    let request_id = request_id::current();

    let deleted = state
        .db
        .delete_device(&device_id)
        .await
        .map_err(|error| internal("devices.remove", error))?;
    if !deleted {
        return Err(ApiError::device_not_found(&device_id));
    }
    state.forget_device_lock(&device_id);
    write_audit(
        &state,
        "device.remove",
        Some("device"),
        Some(&device_id),
        request_id.as_deref(),
        "ok",
        json!({ "name": record.name, "baseUrl": record.base_url }),
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

struct SyncOutcome {
    record: DeviceRecord,
    overview: Option<Value>,
    warnings: Vec<Warning>,
    success: bool,
    code: Option<&'static str>,
    message: Option<String>,
    latency_ms: Option<i64>,
}

fn failure_code(reason: &str) -> &'static str {
    match reason {
        "connect" | "dns" | "timeout" => "device_offline",
        "unauthorized" | "forbidden_scope" => "forbidden_scope",
        _ => "device_error",
    }
}

async fn sync_record(
    state: &AppState,
    record: DeviceRecord,
    request_id: Option<String>,
) -> SyncOutcome {
    let device_id = record.id.clone();

    let token = match state.key.decrypt(&device_id, &record.token_cipher) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(error = %error, device_id, "设备 Token 解密失败");
            let now = ids::now_rfc3339();
            let message = "设备 Token 解密失败";
            log_db_write(
                state
                    .db
                    .mark_device_offline(
                        &device_id,
                        json!({ "code": "internal_error", "message": message }),
                        None,
                        &now,
                    )
                    .await,
                "devices.sync.mark_offline",
                &device_id,
            );
            let record = state
                .db
                .get_device(&device_id)
                .await
                .ok()
                .flatten()
                .unwrap_or(record);
            return SyncOutcome {
                record,
                overview: None,
                warnings: vec![Warning::new(
                    "internal_error",
                    message,
                    json!({ "deviceId": device_id }),
                )],
                success: false,
                code: Some("internal_error"),
                message: Some(message.to_string()),
                latency_ms: None,
            };
        }
    };

    match state
        .http
        .probe(
            &record.base_url,
            token.as_str(),
            request_timeout(state),
            request_id.as_deref(),
        )
        .await
    {
        Ok(probe) => {
            let now = ids::now_rfc3339();
            let meta = overview_meta(&probe.overview);
            log_db_write(
                state
                    .db
                    .update_device_probe(DeviceProbeUpdate {
                        id: device_id.clone(),
                        online: true,
                        model: meta.model,
                        brand: meta.brand,
                        android: meta.android,
                        sdk: meta.sdk,
                        fingerprint: meta.fingerprint,
                        root_manager: meta.root_manager,
                        module_version: meta.module_version,
                        server_version: meta.server_version,
                        last_seen: now.clone(),
                        last_synced_at: now.clone(),
                        latency_ms: Some(probe.latency_ms),
                        updated_at: now.clone(),
                    })
                    .await,
                "devices.sync.probe",
                &device_id,
            );
            log_db_write(
                state
                    .db
                    .upsert_overview(StoredOverview {
                        device_id: device_id.clone(),
                        revision: meta.revision,
                        fetched_at: now.clone(),
                        payload: probe.overview.to_string(),
                    })
                    .await,
                "devices.sync.overview",
                &device_id,
            );
            let record = state
                .db
                .get_device(&device_id)
                .await
                .ok()
                .flatten()
                .unwrap_or(record);
            write_audit(
                state,
                "device.sync",
                Some("device"),
                Some(&device_id),
                request_id.as_deref(),
                "ok",
                json!({ "online": true, "latencyMs": probe.latency_ms }),
            )
            .await;

            SyncOutcome {
                record,
                overview: Some(probe.overview),
                warnings: Vec::new(),
                success: true,
                code: None,
                message: None,
                latency_ms: Some(probe.latency_ms),
            }
        }
        Err(failure) => {
            let code = failure_code(failure.reason);
            let now = ids::now_rfc3339();
            let message = failure.message.clone();
            log_db_write(
                state
                    .db
                    .mark_device_offline(
                        &device_id,
                        json!({ "code": code, "message": message }),
                        None,
                        &now,
                    )
                    .await,
                "devices.sync.mark_offline",
                &device_id,
            );
            let cached = state
                .db
                .get_overview(&device_id)
                .await
                .ok()
                .flatten()
                .and_then(|stored| parse_overview(&stored.payload));
            let record = state
                .db
                .get_device(&device_id)
                .await
                .ok()
                .flatten()
                .unwrap_or(record);
            write_audit(
                state,
                "device.sync",
                Some("device"),
                Some(&device_id),
                request_id.as_deref(),
                "failed",
                json!({ "code": code, "reason": failure.reason }),
            )
            .await;

            SyncOutcome {
                record,
                overview: cached,
                warnings: vec![Warning::new(
                    code,
                    message.clone(),
                    json!({ "deviceId": device_id, "reason": failure.reason }),
                )],
                success: false,
                code: Some(code),
                message: Some(message),
                latency_ms: None,
            }
        }
    }
}

/// POST /relay/api/v1/devices/{id}/sync
pub async fn sync_one(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<SyncDeviceResponse>, ApiError> {
    let record = load_device(&state, &device_id).await?;
    let request_id = request_id::current();
    let _permit = state.sync_sem.clone().acquire_owned().await.ok();
    let lock = state.device_lock(&device_id);
    let _guard = lock.lock().await;
    let outcome = sync_record(&state, record, request_id).await;

    Ok(Json(SyncDeviceResponse {
        device: DeviceSummary::from(outcome.record),
        overview: outcome.overview,
        warnings: outcome.warnings,
    }))
}

/// POST /relay/api/v1/devices/sync
pub async fn sync_all(State(state): State<AppState>) -> Result<Json<SyncAllResponse>, ApiError> {
    let records = state
        .db
        .list_devices()
        .await
        .map_err(|error| internal("devices.sync_all", error))?;
    let ordered_ids: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
    let request_id = request_id::current();

    let mut tasks = tokio::task::JoinSet::new();
    for record in records {
        let state = state.clone();
        let request_id = request_id.clone();
        tasks.spawn(async move {
            let device_id = record.id.clone();
            let permit = state.sync_sem.clone().acquire_owned().await.ok();
            let lock = state.device_lock(&device_id);
            let _guard = lock.lock().await;
            let outcome = sync_record(&state, record, request_id).await;
            drop(permit);
            (device_id, outcome)
        });
    }

    let mut outcomes: HashMap<String, SyncOutcome> = HashMap::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((device_id, outcome)) => {
                outcomes.insert(device_id, outcome);
            }
            Err(error) => tracing::warn!(error = %error, "全量同步任务异常结束"),
        }
    }

    let mut results = Vec::new();
    let mut synced = 0usize;
    for device_id in &ordered_ids {
        if let Some(outcome) = outcomes.remove(device_id) {
            if outcome.success {
                synced += 1;
            }
            let status = if outcome.success {
                "ok"
            } else if outcome.code == Some("device_offline") {
                "offline"
            } else {
                "error"
            };
            results.push(SyncAllResult {
                device_id: device_id.clone(),
                status: status.to_string(),
                online: outcome.record.online,
                latency_ms: outcome.latency_ms,
                code: outcome.code.map(str::to_owned),
                message: outcome.message,
            });
        }
    }

    let failed = ordered_ids.len().saturating_sub(synced);
    write_audit(
        &state,
        "device.sync_all",
        Some("device"),
        Some("all"),
        request_id.as_deref(),
        if failed == 0 { "ok" } else { "failed" },
        json!({ "synced": synced, "failed": failed }),
    )
    .await;

    Ok(Json(SyncAllResponse {
        synced,
        failed,
        results,
        warnings: Vec::new(),
    }))
}

/// GET /relay/api/v1/devices/{id}/overview
pub async fn overview(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<DeviceOverviewResponse>, ApiError> {
    let params = QueryParams::parse(raw.as_deref());
    let cached = params.boolean("cached")?.unwrap_or(false);
    let record = load_device(&state, &device_id).await?;

    if cached {
        let stored = state
            .db
            .get_overview(&device_id)
            .await
            .map_err(|error| internal("devices.overview.cached", error))?
            .ok_or_else(|| ApiError::not_found("device_overview", &device_id))?;
        let Some(value) = parse_overview(&stored.payload) else {
            return Err(internal(
                "devices.overview.cached",
                "缓存的 overview JSON 已损坏",
            ));
        };
        let freshness = stale_state(Some(&stored.fetched_at), record.online);
        return Ok(Json(DeviceOverviewResponse {
            device_id,
            online: record.online,
            cached_at: Some(stored.fetched_at),
            stale: freshness.stale,
            overview: value,
        }));
    }

    let request_id = request_id::current();
    let lock = state.device_lock(&device_id);
    let _guard = lock.lock().await;
    let token = state
        .key
        .decrypt(&device_id, &record.token_cipher)
        .map_err(|error| internal("devices.overview.decrypt", error))?;

    let started = Instant::now();
    match state
        .http
        .fetch_overview(
            &record.base_url,
            token.as_str(),
            request_timeout(&state),
            request_id.as_deref(),
        )
        .await
    {
        Ok(value) => {
            let now = ids::now_rfc3339();
            let meta = overview_meta(&value);
            let latency = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
            log_db_write(
                state
                    .db
                    .update_device_probe(DeviceProbeUpdate {
                        id: device_id.clone(),
                        online: true,
                        model: meta.model,
                        brand: meta.brand,
                        android: meta.android,
                        sdk: meta.sdk,
                        fingerprint: meta.fingerprint,
                        root_manager: meta.root_manager,
                        module_version: meta.module_version,
                        server_version: meta.server_version,
                        last_seen: now.clone(),
                        last_synced_at: now.clone(),
                        latency_ms: Some(latency),
                        updated_at: now.clone(),
                    })
                    .await,
                "devices.overview.probe",
                &device_id,
            );
            log_db_write(
                state
                    .db
                    .upsert_overview(StoredOverview {
                        device_id: device_id.clone(),
                        revision: meta.revision,
                        fetched_at: now.clone(),
                        payload: value.to_string(),
                    })
                    .await,
                "devices.overview.cache",
                &device_id,
            );

            Ok(Json(DeviceOverviewResponse {
                device_id,
                online: true,
                cached_at: Some(now),
                stale: false,
                overview: value,
            }))
        }
        Err(error) => {
            let now = ids::now_rfc3339();
            log_db_write(
                state
                    .db
                    .mark_device_offline(
                        &device_id,
                        json!({ "code": error.manager_code(), "message": error.summary() }),
                        None,
                        &now,
                    )
                    .await,
                "devices.overview.mark_offline",
                &device_id,
            );
            Err(device_call_error(&device_id, &state, &error))
        }
    }
}

/// 下发完成后对 ok 设备做一次 best-effort overview 刷新（失败只记日志）。
pub(crate) async fn refresh_overview(state: &AppState, device_id: &str) {
    let record = match state.db.get_device(device_id).await {
        Ok(Some(record)) => record,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(error = %error, device_id, "刷新 overview 前读取设备失败");
            return;
        }
    };
    let token = match state.key.decrypt(device_id, &record.token_cipher) {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(error = %error, device_id, "刷新 overview 前解密设备 Token 失败");
            return;
        }
    };
    let value = match state
        .http
        .fetch_overview(
            &record.base_url,
            token.as_str(),
            request_timeout(state),
            None,
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(error = %error, device_id, "下发后刷新 overview 失败");
            return;
        }
    };

    let now = ids::now_rfc3339();
    let meta = overview_meta(&value);
    log_db_write(
        state
            .db
            .update_device_probe(DeviceProbeUpdate {
                id: device_id.to_string(),
                online: true,
                model: meta.model,
                brand: meta.brand,
                android: meta.android,
                sdk: meta.sdk,
                fingerprint: meta.fingerprint,
                root_manager: meta.root_manager,
                module_version: meta.module_version,
                server_version: meta.server_version,
                last_seen: now.clone(),
                last_synced_at: now.clone(),
                latency_ms: record.latency_ms,
                updated_at: now.clone(),
            })
            .await,
        "devices.refresh.probe",
        device_id,
    );
    log_db_write(
        state
            .db
            .upsert_overview(StoredOverview {
                device_id: device_id.to_string(),
                revision: meta.revision,
                fetched_at: now,
                payload: value.to_string(),
            })
            .await,
        "devices.refresh.overview",
        device_id,
    );
}

fn device_call_error(device_id: &str, state: &AppState, error: &DeviceCallError) -> ApiError {
    match error {
        DeviceCallError::Offline {
            reason: OfflineReason::Timeout,
        } => ApiError::upstream_timeout(device_id, state.cfg.request_timeout_ms),
        DeviceCallError::Offline { reason } => ApiError::device_offline(device_id, reason.as_str()),
        DeviceCallError::Http { status, code, .. } => {
            ApiError::device_error(device_id, *status, code)
        }
        DeviceCallError::Decode { .. } => ApiError::device_error(device_id, 0, "invalid_response"),
        DeviceCallError::TooLarge { .. } => {
            ApiError::device_error(device_id, 0, "response_too_large")
        }
    }
}
