// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 出口代理（egress）：代理池 CRUD、设备绑定、Manager 侧探测与批量下发。
//!
//! 契约见 docs/plans/egress-contract.md §6 与 §7。三条硬约束：
//! 1) 响应 / 日志 / 审计里永远没有凭据值，唯一信号是 hasCredentials；
//! 2) 密码复用 DeviceKey（AES-256-GCM），AAD 绑定 proxy id 与字段名；
//! 3) 探测 URL 只允许 http://（与设备端一致，避免引入 TLS 语义）。
//!
//! Manager 侧探测 URL 的默认值来自常量 DEFAULT_PROBE_URL，可被请求体的 url 覆盖。

use std::{
    net::IpAddr,
    str::FromStr,
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, RawQuery, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    api::{
        apply::is_valid_package,
        dto::Warning,
        egress_dto::{
            CreateEgressProxyRequest, DeviceEgressBindingDto, DeviceEgressBindingInput,
            DeviceEgressPutRequest, DeviceEgressPutResponse, DeviceEgressResponse,
            EgressApplyAcceptedResponse, EgressApplyDryRunResponse, EgressApplyPlanItem,
            EgressApplyRequest, EgressAssignmentInput, EgressProbeRequest, EgressProbeResponse,
            EgressProxyBindingDto, EgressProxyDetailResponse, EgressProxyDto,
            EgressProxyListResponse, UpdateEgressProxyRequest,
        },
        job_dto::DevicesSpec,
        query::QueryParams,
    },
    crypto::EgressField,
    db::{
        DeviceEgressRow, DeviceEgressWrite, EgressProxyRecord, NewEgressCheck, NewJob,
        PendingAssignment,
    },
    error::{ApiError, ValidationError},
    ids, paging, request_id,
    state::AppState,
};

/// 保留的「强制直连」代理名。
pub const DIRECT_PROXY: &str = "__DIRECT__";
/// Manager 侧探测的默认目标（设备端默认值相同）。
pub const DEFAULT_PROBE_URL: &str = "http://checkip.amazonaws.com/";
/// 探测超时（契约 §6.7 固定 5 s）。
pub const PROBE_TIMEOUT_MS: u64 = 5_000;
/// 探测响应体上限（4 KiB）。
pub const PROBE_BODY_LIMIT: usize = 4 * 1024;
/// 单台设备的绑定行上限。
pub const MAX_BINDINGS: usize = 200;
/// 代理名长度上限。
const MAX_PROXY_NAME: usize = 64;
/// 标签数量 / 单项长度上限。
const MAX_TAGS: usize = 16;
const MAX_TAG_LEN: usize = 32;
/// 审计里 devices / apps 明细的截断长度。
const DETAIL_LIMIT: usize = 20;

/// 一个待下发目标：(device, app) 对；app 为空字符串表示整机。
#[derive(Debug, Clone)]
pub struct EgressTarget {
    pub device_id: String,
    pub app: String,
}

/// egress 下发计划（runner 用）。
#[derive(Debug, Clone)]
pub struct EgressPlan {
    pub request_id: Option<String>,
    pub probe: bool,
    pub targets: Vec<EgressTarget>,
}

// ── 校验 ─────────────────────────────────────────────────────────────────

/// 代理名：^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$，且不得是保留值 __DIRECT__。
pub fn is_valid_proxy_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_PROXY_NAME || name == DIRECT_PROXY {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphanumeric() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
}

/// 代理协议。
pub fn is_valid_proxy_type(value: &str) -> bool {
    matches!(value, "http" | "socks5")
}

/// 主机：≤253，不含 scheme / 路径 / 端口 / 用户信息。
///
/// 含 ':' 时只接受合法 IPv6 字面量——"host:1080" / "203.0.113.10:1080" 这类
/// 把端口写进 host 的输入必须拒绝（端口有独立字段）。
pub fn is_valid_proxy_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.contains("://")
        || host.contains('/')
        || host.contains('@')
        || host.contains('?')
        || host.contains('#')
        || host.chars().any(char::is_whitespace)
    {
        return false;
    }
    if host.contains(':') {
        return matches!(
            std::net::IpAddr::from_str(host),
            Ok(std::net::IpAddr::V6(_))
        );
    }
    true
}

/// 探测 URL：必须 http://、≤512、无用户信息。
pub fn is_valid_probe_url(url: &str) -> bool {
    url.len() <= 512 && url.len() > 7 && url.starts_with("http://") && !url[7..].contains('@')
}

fn validate_tags(raw: Vec<String>, path: &str, errors: &mut Vec<ValidationError>) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for (index, tag) in raw.into_iter().enumerate() {
        let tag = tag.trim().to_string();
        if tag.is_empty() || tag.chars().count() > MAX_TAG_LEN {
            errors.push(ValidationError::new(
                format!("{path}[{index}]"),
                "out_of_range",
                format!("标签需为 1-{MAX_TAG_LEN} 字符"),
            ));
            continue;
        }
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    if tags.len() > MAX_TAGS {
        errors.push(ValidationError::new(
            path,
            "too_many_items",
            format!("标签最多 {MAX_TAGS} 个"),
        ));
        tags.truncate(MAX_TAGS);
    }
    tags
}

fn validate_credentials(
    username: Option<&str>,
    password: Option<&str>,
    errors: &mut Vec<ValidationError>,
) {
    if username.is_some_and(|value| value.chars().count() > 128) {
        errors.push(ValidationError::new(
            "username",
            "invalid_egress_proxy",
            "username 最多 128 字符",
        ));
    }
    if password.is_some_and(|value| value.chars().count() > 256) {
        errors.push(ValidationError::new(
            "password",
            "invalid_egress_proxy",
            "password 最多 256 字符",
        ));
    }
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
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

async fn write_audit(
    state: &AppState,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    status: &str,
    detail: Value,
) {
    crate::api::devices::write_audit(
        state,
        action,
        target_type,
        target_id,
        request_id::current().as_deref(),
        status,
        detail,
    )
    .await;
}

async fn load_proxy(state: &AppState, proxy_id: &str) -> Result<EgressProxyRecord, ApiError> {
    state
        .db
        .get_egress_proxy(proxy_id)
        .await
        .map_err(|error| internal("egress.get_proxy", error))?
        .ok_or_else(|| ApiError::not_found("egress_proxy", proxy_id))
}

fn proxy_dto(
    record: EgressProxyRecord,
    usage: Option<&(i64, i64)>,
    last_check: Option<crate::db::EgressCheckRow>,
) -> EgressProxyDto {
    let (devices, apps) = usage.copied().unwrap_or((0, 0));
    EgressProxyDto::new(record, devices, apps, last_check)
}

// ── 代理池 CRUD ──────────────────────────────────────────────────────────

/// GET /relay/api/v1/egress-proxies
pub async fn list_proxies(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<EgressProxyListResponse>, ApiError> {
    let params = QueryParams::parse(raw.as_deref());
    let enabled = params.boolean("enabled")?;
    let tags = params.multi("tag");
    let keyword = params.string("q").map(|value| value.to_lowercase());
    let limit = params.usize("limit")?;
    let cursor = params.string("cursor");
    let page = paging::resolve(limit, cursor.as_deref())?;

    let records = state
        .db
        .list_egress_proxies()
        .await
        .map_err(|error| internal("egress.list", error))?;
    let usage = state
        .db
        .egress_proxy_usage()
        .await
        .map_err(|error| internal("egress.usage", error))?;
    let checks = state
        .db
        .latest_egress_checks()
        .await
        .map_err(|error| internal("egress.checks", error))?;

    let filtered: Vec<EgressProxyRecord> = records
        .into_iter()
        .filter(|record| {
            if let Some(enabled) = enabled
                && record.enabled != enabled
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
                    record.host.to_lowercase(),
                    record.region.clone().unwrap_or_default().to_lowercase()
                );
                if !haystack.contains(keyword) {
                    return false;
                }
            }
            true
        })
        .collect();

    let total = filtered.len();
    let items: Vec<EgressProxyDto> = filtered
        .into_iter()
        .skip(page.offset)
        .take(page.limit)
        .map(|record| {
            let id = record.id.clone();
            proxy_dto(record, usage.get(&id), checks.get(&id).cloned())
        })
        .collect();
    let next_cursor = paging::next_cursor(total, page.offset, items.len());

    Ok(Json(EgressProxyListResponse {
        total: total as i64,
        next_cursor,
        items,
    }))
}

/// POST /relay/api/v1/egress-proxies
pub async fn create_proxy(
    State(state): State<AppState>,
    payload: Result<Json<CreateEgressProxyRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let mut request = json_body(payload, "body")?;
    let mut errors = Vec::new();

    let name = request.name.trim().to_string();
    if !is_valid_proxy_name(&name) {
        errors.push(ValidationError::new(
            "name",
            "invalid_egress_name",
            "代理名需匹配 ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$ 且不得为 __DIRECT__",
        ));
    }
    if !is_valid_proxy_type(&request.proxy_type) {
        errors.push(ValidationError::new(
            "type",
            "invalid_egress_proxy",
            "type 只能是 http 或 socks5",
        ));
    }
    let host = request.host.trim().to_string();
    if !is_valid_proxy_host(&host) {
        errors.push(ValidationError::new(
            "host",
            "invalid_egress_proxy",
            "host 需为 1-253 字符，不含 scheme / 端口 / 路径 / 用户信息",
        ));
    }
    if !(1..=65535).contains(&request.port) {
        errors.push(ValidationError::new(
            "port",
            "invalid_egress_proxy",
            "port 需在 1-65535 之间",
        ));
    }
    validate_credentials(
        request.username.as_deref(),
        request.password.as_deref(),
        &mut errors,
    );
    if request
        .region
        .as_deref()
        .is_some_and(|region| region.chars().count() > 64)
    {
        errors.push(ValidationError::new(
            "region",
            "out_of_range",
            "region 最多 64 字符",
        ));
    }
    let tags = validate_tags(
        request.tags.clone().unwrap_or_default(),
        "tags",
        &mut errors,
    );

    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if let Some(existing) = state
        .db
        .get_egress_proxy_by_name(&name)
        .await
        .map_err(|error| internal("egress.create.duplicate", error))?
    {
        return Err(ApiError::egress_proxy_exists(&name, &existing.id));
    }

    let proxy_id = ids::new_egress_proxy_id();
    let now = ids::now_rfc3339();
    let username_cipher = encrypt_credential(
        &state,
        &proxy_id,
        EgressField::Username,
        &mut request.username,
    )
    .map_err(|error| internal("encrypt", error))?;
    let password_cipher = encrypt_credential(
        &state,
        &proxy_id,
        EgressField::Password,
        &mut request.password,
    )
    .map_err(|error| internal("encrypt", error))?;
    // 加密已经完成，请求体里的明文立刻清零（避免长时间驻留在内存里）。
    zeroize_credentials(&mut request.username, &mut request.password);
    let record = EgressProxyRecord {
        id: proxy_id.clone(),
        name: name.clone(),
        kind: request.proxy_type,
        host,
        port: request.port,
        username_cipher,
        password_cipher,
        region: request.region,
        tags,
        enabled: request.enabled.unwrap_or(true),
        created_at: now.clone(),
        updated_at: now,
    };
    state
        .db
        .insert_egress_proxy(record.clone())
        .await
        .map_err(|error| internal("egress.create.insert", error))?;

    write_audit(
        &state,
        "egress.proxy_create",
        Some("egress_proxy"),
        Some(&proxy_id),
        "ok",
        json!({ "proxyId": proxy_id, "name": name, "changed": true }),
    )
    .await;

    Ok((StatusCode::CREATED, Json(proxy_dto(record, None, None))).into_response())
}

/// GET /relay/api/v1/egress-proxies/{id}
pub async fn get_proxy(
    Path(proxy_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<EgressProxyDetailResponse>, ApiError> {
    let record = load_proxy(&state, &proxy_id).await?;
    let bindings = state
        .db
        .egress_proxy_bindings(&proxy_id)
        .await
        .map_err(|error| internal("egress.bindings", error))?;
    let usage = state
        .db
        .egress_proxy_usage()
        .await
        .map_err(|error| internal("egress.usage", error))?;
    let checks = state
        .db
        .recent_egress_checks(&proxy_id, 20)
        .await
        .map_err(|error| internal("egress.checks", error))?;

    let last_check = checks.first().cloned();
    let proxy = proxy_dto(record, usage.get(&proxy_id), last_check);
    Ok(Json(EgressProxyDetailResponse {
        proxy,
        bindings: bindings
            .into_iter()
            .map(EgressProxyBindingDto::from)
            .collect(),
        recent_checks: checks.into_iter().map(Into::into).collect(),
    }))
}

/// PATCH /relay/api/v1/egress-proxies/{id}
pub async fn update_proxy(
    Path(proxy_id): Path<String>,
    State(state): State<AppState>,
    payload: Result<Json<UpdateEgressProxyRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let mut request = json_body(payload, "body")?;
    if request.is_empty() {
        return Err(ApiError::validation(vec![ValidationError::new(
            "body",
            "missing_required",
            "至少提供一个字段",
        )]));
    }
    let current = load_proxy(&state, &proxy_id).await?;
    let mut errors = Vec::new();
    let mut changed: Vec<&str> = Vec::new();
    let mut record = current.clone();

    match &request.name {
        None => {}
        Some(None) => errors.push(ValidationError::new(
            "name",
            "invalid_value",
            "name 不可置空",
        )),
        Some(Some(raw)) => {
            let name = raw.trim().to_string();
            if !is_valid_proxy_name(&name) {
                errors.push(ValidationError::new(
                    "name",
                    "invalid_egress_name",
                    "代理名需匹配 ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$ 且不得为 __DIRECT__",
                ));
            } else if name != record.name {
                changed.push("name");
                record.name = name;
            }
        }
    }
    match &request.proxy_type {
        None => {}
        Some(None) => errors.push(ValidationError::new(
            "type",
            "invalid_value",
            "type 不可置空",
        )),
        Some(Some(value)) => {
            if !is_valid_proxy_type(value) {
                errors.push(ValidationError::new(
                    "type",
                    "invalid_egress_proxy",
                    "type 只能是 http 或 socks5",
                ));
            } else if *value != record.kind {
                changed.push("type");
                record.kind = value.clone();
            }
        }
    }
    match &request.host {
        None => {}
        Some(None) => errors.push(ValidationError::new(
            "host",
            "invalid_value",
            "host 不可置空",
        )),
        Some(Some(raw)) => {
            let host = raw.trim().to_string();
            if !is_valid_proxy_host(&host) {
                errors.push(ValidationError::new(
                    "host",
                    "invalid_egress_proxy",
                    "host 需为 1-253 字符，不含 scheme / 端口 / 路径 / 用户信息",
                ));
            } else if host != record.host {
                changed.push("host");
                record.host = host;
            }
        }
    }
    match request.port {
        None => {}
        Some(None) => errors.push(ValidationError::new(
            "port",
            "invalid_value",
            "port 不可置空",
        )),
        Some(Some(port)) => {
            if !(1..=65535).contains(&port) {
                errors.push(ValidationError::new(
                    "port",
                    "invalid_egress_proxy",
                    "port 需在 1-65535 之间",
                ));
            } else if port != record.port {
                changed.push("port");
                record.port = port;
            }
        }
    }
    match &request.enabled {
        None => {}
        Some(None) => errors.push(ValidationError::new(
            "enabled",
            "invalid_value",
            "enabled 不可置空",
        )),
        Some(Some(enabled)) if *enabled != record.enabled => {
            changed.push("enabled");
            record.enabled = *enabled;
        }
        Some(Some(_)) => {}
    }
    match &request.region {
        None => {}
        Some(None) => {
            if record.region.is_some() {
                changed.push("region");
                record.region = None;
            }
        }
        Some(Some(raw)) => {
            let region = raw.trim().to_string();
            if region.chars().count() > 64 {
                errors.push(ValidationError::new(
                    "region",
                    "out_of_range",
                    "region 最多 64 字符",
                ));
            } else if record.region.as_ref() != Some(&region) {
                changed.push("region");
                record.region = Some(region);
            }
        }
    }
    match &request.tags {
        None => {}
        Some(None) => {
            if !record.tags.is_empty() {
                changed.push("tags");
                record.tags = Vec::new();
            }
        }
        Some(Some(raw)) => {
            let tags = validate_tags(raw.clone(), "tags", &mut errors);
            if tags != record.tags {
                changed.push("tags");
                record.tags = tags;
            }
        }
    }

    // 凭据双态：省略 = 不改；null = 清除；有值 = 重新加密。
    // 明文从请求体里 take 出来后立刻加密，加密完即清零。
    match request.username.take() {
        None => {}
        Some(None) => {
            if record.username_cipher.take().is_some() {
                changed.push("credentials");
            }
        }
        Some(Some(mut raw)) => {
            if raw.chars().count() > 128 {
                errors.push(ValidationError::new(
                    "username",
                    "invalid_egress_proxy",
                    "username 最多 128 字符",
                ));
            } else {
                record.username_cipher = Some(
                    state
                        .key
                        .encrypt_egress(&proxy_id, EgressField::Username, &raw)
                        .map_err(|error| internal("encrypt", error))?,
                );
                changed.push("credentials");
            }
            raw.zeroize();
        }
    }
    match request.password.take() {
        None => {}
        Some(None) => {
            if record.password_cipher.take().is_some() {
                changed.push("credentials");
            }
        }
        Some(Some(mut raw)) => {
            if raw.chars().count() > 256 {
                errors.push(ValidationError::new(
                    "password",
                    "invalid_egress_proxy",
                    "password 最多 256 字符",
                ));
            } else {
                record.password_cipher = Some(
                    state
                        .key
                        .encrypt_egress(&proxy_id, EgressField::Password, &raw)
                        .map_err(|error| internal("encrypt", error))?,
                );
                changed.push("credentials");
            }
            raw.zeroize();
        }
    }

    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if record.name != current.name
        && let Some(existing) = state
            .db
            .get_egress_proxy_by_name(&record.name)
            .await
            .map_err(|error| internal("egress.update.duplicate", error))?
        && existing.id != proxy_id
    {
        return Err(ApiError::egress_proxy_exists(&record.name, &existing.id));
    }

    record.updated_at = ids::now_rfc3339();
    let updated = state
        .db
        .update_egress_proxy(record.clone())
        .await
        .map_err(|error| internal("egress.update", error))?;
    if updated == 0 {
        return Err(ApiError::not_found("egress_proxy", &proxy_id));
    }

    let mut warnings: Vec<Value> = Vec::new();
    if record.name != current.name {
        let bindings = state
            .db
            .egress_proxy_bindings(&proxy_id)
            .await
            .map_err(|error| internal("egress.bindings", error))?;
        let mut devices: Vec<String> = bindings.iter().map(|row| row.device_id.clone()).collect();
        devices.dedup();
        if !devices.is_empty() {
            warnings.push(json!({
                "code": "egress_rename_pending",
                "message": format!("{} 台设备仍绑定旧名 {}", devices.len(), current.name),
                "details": {
                    "oldName": current.name,
                    "newName": record.name,
                    "devices": devices.len(),
                },
            }));
        }
    }

    write_audit(
        &state,
        "egress.proxy_update",
        Some("egress_proxy"),
        Some(&proxy_id),
        "ok",
        json!({
            "proxyId": proxy_id,
            "name": record.name,
            "changed": !changed.is_empty(),
        }),
    )
    .await;

    let usage = state
        .db
        .egress_proxy_usage()
        .await
        .map_err(|error| internal("egress.usage", error))?;
    let checks = state
        .db
        .latest_egress_checks()
        .await
        .map_err(|error| internal("egress.checks", error))?;
    let dto = proxy_dto(record, usage.get(&proxy_id), checks.get(&proxy_id).cloned());

    let mut body = serde_json::to_value(dto).map_err(|error| internal("egress.update", error))?;
    if let Some(object) = body.as_object_mut() {
        object.insert("warnings".to_string(), Value::Array(warnings));
    }
    Ok(Json(body))
}

/// DELETE /relay/api/v1/egress-proxies/{id}
pub async fn delete_proxy(
    Path(proxy_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let record = load_proxy(&state, &proxy_id).await?;
    let bindings = state
        .db
        .egress_proxy_bindings(&proxy_id)
        .await
        .map_err(|error| internal("egress.bindings", error))?;
    if !bindings.is_empty() {
        let mut devices: Vec<String> = bindings.iter().map(|row| row.device_id.clone()).collect();
        devices.dedup();
        let apps: Vec<String> = bindings
            .iter()
            .map(|row| row.app.clone())
            .filter(|app| !app.is_empty())
            .take(DETAIL_LIMIT)
            .collect();
        let total = bindings.len() as i64;
        let devices: Vec<String> = devices.into_iter().take(DETAIL_LIMIT).collect();
        return Err(ApiError::egress_proxy_in_use(
            &proxy_id,
            &record.name,
            total,
            devices,
            apps,
        ));
    }

    // 预检查之后仍可能被并发绑定：FK RESTRICT 会报约束错误，这里重新查绑定再映射 409。
    if let Err(error) = state.db.delete_egress_proxy(&proxy_id).await {
        if error.is_foreign_key_violation() {
            let bindings = state
                .db
                .egress_proxy_bindings(&proxy_id)
                .await
                .map_err(|error| internal("egress.bindings", error))?;
            let mut devices: Vec<String> =
                bindings.iter().map(|row| row.device_id.clone()).collect();
            devices.dedup();
            let apps: Vec<String> = bindings
                .iter()
                .map(|row| row.app.clone())
                .filter(|app| !app.is_empty())
                .take(DETAIL_LIMIT)
                .collect();
            return Err(ApiError::egress_proxy_in_use(
                &proxy_id,
                &record.name,
                bindings.len() as i64,
                devices.into_iter().take(DETAIL_LIMIT).collect(),
                apps,
            ));
        }
        return Err(internal("egress.delete", error));
    }

    write_audit(
        &state,
        "egress.proxy_delete",
        Some("egress_proxy"),
        Some(&proxy_id),
        "ok",
        json!({ "proxyId": proxy_id, "name": record.name }),
    )
    .await;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// 加密一条凭据；空串等价于未设置。加密后立即清零明文。
fn encrypt_credential(
    state: &AppState,
    proxy_id: &str,
    field: EgressField,
    value: &mut Option<String>,
) -> Result<Option<String>, crate::crypto::KeyError> {
    let Some(raw) = value.as_mut() else {
        return Ok(None);
    };
    if raw.is_empty() {
        raw.zeroize();
        return Ok(None);
    }
    let cipher = state.key.encrypt_egress(proxy_id, field, raw)?;
    // 明文只在这一跳的内存里停留：加密完立刻清零。
    raw.zeroize();
    Ok(Some(cipher))
}

/// 请求体里残留的凭据明文清零（处理完即调用）。
fn zeroize_credentials(username: &mut Option<String>, password: &mut Option<String>) {
    use zeroize::Zeroize;
    if let Some(value) = username.as_mut() {
        value.zeroize();
    }
    if let Some(value) = password.as_mut() {
        value.zeroize();
    }
}

// ── Manager 侧探测 ───────────────────────────────────────────────────────

/// POST /relay/api/v1/egress-proxies/{id}/probe
pub async fn probe_proxy(
    Path(proxy_id): Path<String>,
    State(state): State<AppState>,
    payload: Result<Json<EgressProbeRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let request = match payload {
        Ok(Json(value)) => value,
        // 请求体可省略（空体 / 无 Content-Type）。
        Err(rejection)
            if rejection.status() == StatusCode::UNSUPPORTED_MEDIA_TYPE
                || rejection.status() == StatusCode::BAD_REQUEST =>
        {
            EgressProbeRequest::default()
        }
        Err(rejection) => {
            return Err(json_body::<EgressProbeRequest>(Err(rejection), "body").unwrap_err());
        }
    };
    let record = load_proxy(&state, &proxy_id).await?;
    let url = request
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PROBE_URL)
        .to_string();
    if !is_valid_probe_url(&url) {
        return Err(ApiError::validation(vec![ValidationError::new(
            "url",
            "invalid_probe_url",
            "探测 URL 必须以 http:// 开头、≤512 字符且不含用户信息",
        )]));
    }

    let checked_at = ids::now_rfc3339();
    if record.kind != "http" {
        write_check(
            &state,
            NewEgressCheck {
                source: "manager".to_string(),
                device_id: None,
                app: String::new(),
                scope: "device".to_string(),
                proxy_id: Some(record.id.clone()),
                exit_ip: None,
                latency_ms: None,
                ok: false,
                error_code: Some("unsupported_proxy_type".to_string()),
                checked_at,
            },
        )
        .await;
        return Err(ApiError::validation(vec![ValidationError::new(
            "proxyId",
            "unsupported_proxy_type",
            "Manager 侧探测只支持 HTTP 代理",
        )]));
    }

    let username = decrypt_credential(&state, &record, EgressField::Username)?;
    let password = decrypt_credential(&state, &record, EgressField::Password)?;

    let started = Instant::now();
    let outcome = probe_through_proxy(
        &record,
        &url,
        username.as_ref().map(|value| value.as_str()),
        password.as_ref().map(|value| value.as_str()),
    )
    .await;
    let latency_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
    let mut warnings: Vec<Warning> = Vec::new();

    match outcome {
        Ok((exit_ip, upstream_status)) => {
            let recorded = write_check(
                &state,
                NewEgressCheck {
                    source: "manager".to_string(),
                    device_id: None,
                    app: String::new(),
                    scope: "device".to_string(),
                    proxy_id: Some(record.id.clone()),
                    exit_ip: Some(exit_ip.clone()),
                    latency_ms: Some(latency_ms),
                    ok: true,
                    error_code: None,
                    checked_at: checked_at.clone(),
                },
            )
            .await;
            if !recorded {
                warnings.push(check_not_recorded_warning_for_probe(&record.id));
            }
            write_audit(
                &state,
                "egress.proxy_probe",
                Some("egress_proxy"),
                Some(&record.id),
                "ok",
                json!({ "proxyId": record.id, "name": record.name, "changed": false }),
            )
            .await;
            Ok(Json(EgressProbeResponse {
                proxy_id: record.id,
                ok: true,
                exit_ip,
                latency_ms,
                checked_at,
                url,
                upstream_status,
                warnings,
            })
            .into_response())
        }
        Err(failure) => {
            // 日志只记 host 与 reason（契约 §7.1 第 6 条），细节降到 debug。
            tracing::warn!(
                proxy_id = %record.id,
                host = %record.host,
                reason = failure.reason,
                "Manager 侧经代理探测出口失败"
            );
            tracing::debug!(detail = %failure.detail, "探测失败细节");
            let recorded = write_check(
                &state,
                NewEgressCheck {
                    source: "manager".to_string(),
                    device_id: None,
                    app: String::new(),
                    scope: "device".to_string(),
                    proxy_id: Some(record.id.clone()),
                    exit_ip: None,
                    latency_ms: Some(latency_ms),
                    ok: false,
                    error_code: Some(failure.reason.to_string()),
                    checked_at: checked_at.clone(),
                },
            )
            .await;
            if !recorded {
                // 失败分支同样不能静默丢掉「没写进 egress_checks」这件事。
                warnings.push(check_not_recorded_warning_for_probe(&record.id));
            }
            write_audit(
                &state,
                "egress.proxy_probe",
                Some("egress_proxy"),
                Some(&record.id),
                "failed",
                json!({ "proxyId": record.id, "name": record.name, "changed": false }),
            )
            .await;
            Ok(error_with_warnings(
                ApiError::egress_probe_failed(
                    &record.id,
                    &url,
                    failure.reason,
                    PROBE_TIMEOUT_MS,
                    failure.upstream_status,
                ),
                &warnings,
            ))
        }
    }
}

/// 统一错误信封 + warnings（契约外附加字段，客户端安全忽略）。
///
/// 502 的 egress_probe_failed 需要在同一响应里告诉调用方「探测结果没有落库」，
/// 而标准错误信封没有 warnings 槽位，因此在这里附加一个顶层 warnings 数组。
fn error_with_warnings(error: ApiError, warnings: &[Warning]) -> Response {
    let request_id = request_id::current().unwrap_or_else(ids::new_request_id);
    let body = json!({
        "error": {
            "code": error.code,
            "message": error.message,
            "details": error.details,
            "retryable": error.retryable,
            "requestId": request_id,
        },
        "warnings": warnings,
    });
    let mut response = (error.status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// 写入探测结果；返回是否真的落库（false 时调用方要给出 egress_check_not_recorded 警告）。
async fn write_check(state: &AppState, check: NewEgressCheck) -> bool {
    match state.db.insert_egress_check(check).await {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(error = %error, "写入 egress_checks 失败");
            false
        }
    }
}

/// 探测结果未能落库时的警告（响应体层面；job 侧用 jobs::check_not_recorded_warning）。
fn check_not_recorded_warning_for_probe(proxy_id: &str) -> Warning {
    Warning::new(
        "egress_check_not_recorded",
        "探测结果未能写入 egress_checks（数据库写入失败）",
        json!({ "proxyId": proxy_id, "source": "manager" }),
    )
}

fn decrypt_credential(
    state: &AppState,
    record: &EgressProxyRecord,
    field: EgressField,
) -> Result<Option<Zeroizing<String>>, ApiError> {
    let stored = match field {
        EgressField::Username => record.username_cipher.as_deref(),
        EgressField::Password => record.password_cipher.as_deref(),
    };
    let Some(stored) = stored else {
        return Ok(None);
    };
    state
        .key
        .decrypt_egress(&record.id, field, stored)
        .map(Some)
        .map_err(|error| {
            tracing::error!(error = %error, proxy_id = %record.id, "代理凭据解密失败");
            ApiError::internal("decrypt")
        })
}

/// 经 HTTP 代理发一次 GET，返回 (出口 IP, 上游状态码)。
async fn probe_through_proxy(
    record: &EgressProxyRecord,
    url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<(String, Option<u16>), ProbeFailure> {
    let proxy_url = format!("http://{}:{}", record.host, record.port);
    let mut proxy = reqwest::Proxy::http(&proxy_url).map_err(|error| ProbeFailure {
        reason: "connect",
        upstream_status: None,
        detail: error.to_string(),
    })?;
    if let (Some(username), Some(password)) = (username, password) {
        proxy = proxy.basic_auth(username, password);
    }
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .connect_timeout(Duration::from_millis(PROBE_TIMEOUT_MS))
        .timeout(Duration::from_millis(PROBE_TIMEOUT_MS))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(crate::device_client::USER_AGENT)
        .build()
        .map_err(|error| ProbeFailure {
            reason: "connect",
            upstream_status: None,
            detail: error.to_string(),
        })?;

    let mut request = client
        .get(url)
        .header(reqwest::header::ACCEPT, "text/plain, application/json");
    if let (Some(username), Some(password)) = (username, password) {
        // reqwest 的 basic_auth 只在收到 407 后才附带凭据；这里主动预置代理认证头。
        request = request.header("proxy-authorization", basic_auth_header(username, password));
    }
    let response = request.send().await.map_err(classify_probe_error)?;

    let status = response.status();
    let body = read_limited_body(response).await?;
    if !status.is_success() {
        return Err(ProbeFailure {
            reason: "http_status",
            upstream_status: Some(status.as_u16()),
            detail: format!("HTTP {}", status.as_u16()),
        });
    }
    match parse_exit_ip(&body) {
        Some(exit_ip) => Ok((exit_ip, Some(status.as_u16()))),
        None => Err(ProbeFailure {
            reason: "invalid_response",
            upstream_status: Some(status.as_u16()),
            detail: "响应体不是 IP 字面量".to_string(),
        }),
    }
}

/// 代理认证头；凭据只出现在这一次请求里，不进日志。
fn basic_auth_header(username: &str, password: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    format!(
        "Basic {}",
        BASE64.encode(format!("{username}:{password}").as_bytes())
    )
}

async fn read_limited_body(mut response: reqwest::Response) -> Result<String, ProbeFailure> {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len().saturating_add(chunk.len()) > PROBE_BODY_LIMIT {
                    return Err(ProbeFailure {
                        reason: "invalid_response",
                        upstream_status: Some(status),
                        detail: format!("响应体超过 {PROBE_BODY_LIMIT} 字节"),
                    });
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => return Err(classify_probe_error(error)),
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn classify_probe_error(error: reqwest::Error) -> ProbeFailure {
    let text = error.to_string().to_ascii_lowercase();
    let reason = if error.is_timeout() {
        "timeout"
    } else if text.contains("tls") || text.contains("certificate") {
        "tls"
    } else if text.contains("dns") || text.contains("resolve") {
        "dns"
    } else {
        "connect"
    };
    ProbeFailure {
        reason,
        upstream_status: None,
        detail: error.to_string(),
    }
}

/// 解析探测响应体：纯 IP → JSON 的 ip/origin/query → 其余视为非法。
fn parse_exit_ip(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if let Some(ip) = parse_ip_literal(trimmed) {
        return Some(ip);
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    for key in ["ip", "origin", "query"] {
        if let Some(text) = value.get(key).and_then(Value::as_str)
            && let Some(ip) = parse_ip_literal(text.trim())
        {
            return Some(ip);
        }
    }
    None
}

/// 单个候选值：与设备端同一规则——出现 ':' 就必须是合法 IPv6 字面量，
/// "203.0.113.7:80" / "host:443" 这类带端口的文本一律拒绝。
fn parse_ip_literal(text: &str) -> Option<String> {
    if text.contains(':') {
        return match IpAddr::from_str(text) {
            Ok(ip @ IpAddr::V6(_)) => Some(ip.to_string()),
            _ => None,
        };
    }
    IpAddr::from_str(text).ok().map(|ip| ip.to_string())
}

struct ProbeFailure {
    reason: &'static str,
    upstream_status: Option<u16>,
    /// 失败细节：只写 debug 日志，绝不进响应或 warn 级日志。
    detail: String,
}

/// 设备级出口摘要（设备列表 / 详情 / 总览用）。
pub async fn device_egress_summaries(
    state: &AppState,
) -> Result<std::collections::HashMap<String, crate::api::dto::DeviceEgressSummary>, ApiError> {
    let bindings = state
        .db
        .list_all_device_egress()
        .await
        .map_err(|error| internal("egress.summaries", error))?;
    let checks = state
        .db
        .latest_device_egress_checks()
        .await
        .map_err(|error| internal("egress.summaries", error))?;
    Ok(crate::api::dto::device_egress_summaries(&bindings, &checks))
}

// ── 设备绑定 ─────────────────────────────────────────────────────────────

/// GET /relay/api/v1/devices/{id}/egress
pub async fn get_device_egress(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<DeviceEgressResponse>, ApiError> {
    let device = state
        .db
        .get_device(&device_id)
        .await
        .map_err(|error| internal("egress.device", error))?
        .ok_or_else(|| ApiError::device_not_found(&device_id))?;

    let bindings = state
        .db
        .list_device_egress(&device_id)
        .await
        .map_err(|error| internal("egress.device_bindings", error))?;

    let mut warnings = Vec::new();
    let mut device_state = None;
    match state.key.decrypt(&device_id, &device.token_cipher) {
        Ok(token) => match state
            .http
            .get_egress(
                &device.base_url,
                token.as_str(),
                Duration::from_millis(state.cfg.request_timeout_ms),
                request_id::current().as_deref(),
            )
            .await
        {
            Ok(value) => device_state = Some(value),
            Err(error) => {
                let code = if error.http_status() == Some(404) {
                    "egress_unsupported"
                } else {
                    "device_offline"
                };
                warnings.push(Warning::new(
                    code,
                    error.summary(),
                    json!({ "deviceId": device_id, "code": code }),
                ));
            }
        },
        Err(error) => {
            // 与 job 路径同一口径：Token 解不开不是「离线」，而是密钥/密文问题。
            warnings.push(Warning::new(
                "device_token_invalid",
                format!(
                    "设备 Token 解密失败（manager.key 可能已变更，需要重新登记设备的 Token）：{error}"
                ),
                json!({
                    "deviceId": device_id,
                    "code": "device_token_invalid",
                    "op": "decrypt",
                }),
            ));
        }
    }

    Ok(Json(DeviceEgressResponse {
        device_id,
        bindings: bindings
            .into_iter()
            .map(DeviceEgressBindingDto::from)
            .collect(),
        device_state,
        warnings,
    }))
}

/// PUT /relay/api/v1/devices/{id}/egress（只落库，不下发）
pub async fn put_device_egress(
    Path(device_id): Path<String>,
    State(state): State<AppState>,
    payload: Result<Json<DeviceEgressPutRequest>, JsonRejection>,
) -> Result<Json<DeviceEgressPutResponse>, ApiError> {
    let request = json_body(payload, "body")?;
    state
        .db
        .get_device(&device_id)
        .await
        .map_err(|error| internal("egress.device", error))?
        .ok_or_else(|| ApiError::device_not_found(&device_id))?;

    let prepared = prepare_bindings(&state, &device_id, request.bindings).await?;
    let changed = state
        .db
        .replace_device_egress(&device_id, prepared, &ids::now_rfc3339())
        .await
        .map_err(|error| internal("egress.bind", error))?;

    let bindings = state
        .db
        .list_device_egress(&device_id)
        .await
        .map_err(|error| internal("egress.device_bindings", error))?;

    write_audit(
        &state,
        "egress.bind",
        Some("device"),
        Some(&device_id),
        "ok",
        json!({
            "deviceId": device_id,
            "changed": changed,
            "total": bindings.len(),
        }),
    )
    .await;

    Ok(Json(DeviceEgressPutResponse {
        device_id,
        changed,
        bindings: bindings
            .into_iter()
            .map(DeviceEgressBindingDto::from)
            .collect(),
        pending_apply: true,
        warnings: Vec::new(),
    }))
}

/// 把请求体的绑定行校验 / 归一化成待写库的行。
async fn prepare_bindings(
    state: &AppState,
    device_id: &str,
    bindings: Vec<DeviceEgressBindingInput>,
) -> Result<Vec<DeviceEgressWrite>, ApiError> {
    if bindings.len() > MAX_BINDINGS {
        return Err(ApiError::validation(vec![ValidationError::new(
            "bindings",
            "too_many_items",
            format!("bindings 最多 {MAX_BINDINGS} 项"),
        )]));
    }

    let proxies = state
        .db
        .list_egress_proxies()
        .await
        .map_err(|error| internal("egress.list", error))?;
    let existing: Vec<DeviceEgressWrite> = state
        .db
        .list_device_egress(device_id)
        .await
        .map_err(|error| internal("egress.device_bindings", error))?
        .into_iter()
        .map(|row| DeviceEgressWrite {
            app: row.app,
            mode: row.mode,
            proxy_id: row.proxy_id,
        })
        .collect();

    let mut errors = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut prepared: Vec<DeviceEgressWrite> = Vec::new();

    for (index, binding) in bindings.into_iter().enumerate() {
        let path = format!("bindings[{index}]");
        let app = binding.app.trim().to_string();
        if !app.is_empty() && !is_valid_package(&app) {
            errors.push(ValidationError::new(
                format!("{path}.app"),
                "invalid_package",
                format!("包名不合法：{app}"),
            ));
            continue;
        }
        if seen.iter().any(|value| value == &app) {
            errors.push(ValidationError::new(
                format!("{path}.app"),
                "invalid_value",
                format!("同一 app 重复：{app}"),
            ));
            continue;
        }
        seen.push(app.clone());

        match binding.proxy_id.as_deref() {
            // 整机直连 = 没有绑定；应用级 null = 删除该行。
            None => {}
            Some(DIRECT_PROXY) => {
                if app.is_empty() {
                    continue;
                }
                prepared.push(DeviceEgressWrite {
                    app,
                    mode: "direct".to_string(),
                    proxy_id: None,
                });
            }
            Some(proxy_id) => {
                let Some(proxy) = proxies.iter().find(|proxy| proxy.id == proxy_id) else {
                    errors.push(ValidationError::new(
                        format!("{path}.proxyId"),
                        "invalid_value",
                        format!("代理不存在：{proxy_id}"),
                    ));
                    continue;
                };
                if !proxy.enabled {
                    let unchanged = existing
                        .iter()
                        .any(|row| row.app == app && row.proxy_id.as_deref() == Some(proxy_id));
                    if !unchanged {
                        errors.push(ValidationError::new(
                            format!("{path}.proxyId"),
                            "invalid_value",
                            "代理已停用，不能新建绑定",
                        ));
                        continue;
                    }
                }
                prepared.push(DeviceEgressWrite {
                    app,
                    mode: "proxy".to_string(),
                    proxy_id: Some(proxy_id.to_string()),
                });
            }
        }
    }

    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    Ok(prepared)
}

// ── 批量下发 ─────────────────────────────────────────────────────────────

/// POST /relay/api/v1/devices/egress/apply
pub async fn apply_egress(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<EgressApplyRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let request = json_body(payload, "body")?;
    let probe = request.probe.unwrap_or(false);
    let dry_run = request.dry_run.unwrap_or(false);

    if request.devices.is_some() && request.assignments.is_some() {
        return Err(ApiError::validation(vec![ValidationError::new(
            "assignments",
            "invalid_value",
            "devices 与 assignments 只能二选一",
        )]));
    }

    // 幂等键先校验、先重放：重放不需要再走校验 / 落库。
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(key) = idempotency_key.as_deref() {
        if !valid_idempotency_key(key) {
            return Err(ApiError::validation(vec![ValidationError::new(
                "Idempotency-Key",
                "invalid_value",
                "Idempotency-Key 需为 1-128 字符，且不含空白/控制字符",
            )]));
        }
        if let Some(existing) = state
            .db
            .find_job_by_idempotency(key)
            .await
            .map_err(|error| internal("egress.apply.idempotency", error))?
        {
            return Ok(replay_response(existing));
        }
    }

    // 纯校验 / 归一化：dryRun 只走这一步，不写库、不写审计、不建 job。
    let planned = plan_egress_targets(&state, request.devices, request.assignments).await?;
    let targets = planned.targets();
    if targets.is_empty() {
        return Err(ApiError::validation(vec![ValidationError::new(
            "devices",
            "invalid_value",
            "没有可下发的设备",
        )]));
    }
    let device_count = planned.devices.len();

    if dry_run {
        let plan = build_dry_run_plan(&state, &planned).await?;
        return Ok(Json(EgressApplyDryRunResponse {
            dry_run: true,
            total: plan.len() as i64,
            plan,
            warnings: Vec::new(),
        })
        .into_response());
    }

    // assignments 形态：校验与落库是两步，只有真实下发才在这里写库 + 写审计。
    if planned.persist {
        persist_egress_bindings(&state, &planned.devices).await?;
    }

    let now = ids::now_rfc3339();
    let job_id = ids::new_job_id();
    let req_id = request_id::current().unwrap_or_else(ids::new_request_id);
    let total = targets.len() as i64;
    let replay_key = idempotency_key.clone();
    let job = NewJob {
        id: job_id.clone(),
        kind: "egress".to_string(),
        status: "queued".to_string(),
        app: None,
        template: None,
        restart: false,
        total,
        request_id: req_id.clone(),
        idempotency_key,
        created_at: now.clone(),
    };
    let pending: Vec<PendingAssignment> = targets
        .iter()
        .enumerate()
        .map(|(index, target)| PendingAssignment {
            ordinal: index as i64,
            device_id: target.device_id.clone(),
            app: target.app.clone(),
        })
        .collect();

    if let Err(error) = state.db.insert_job_with_assignments(job, pending).await {
        // 并发同键：UNIQUE 冲突时回查并重放，避免 500。
        if let Some(key) = replay_key.as_deref()
            && let Ok(Some(existing)) = state.db.find_job_by_idempotency(key).await
        {
            return Ok(replay_response(existing));
        }
        return Err(internal("egress.apply.insert", error));
    }

    write_audit(
        &state,
        "egress.apply",
        Some("job"),
        Some(&job_id),
        "ok",
        json!({
            "jobId": job_id,
            "changed": true,
            "total": total,
            "devices": device_count,
        }),
    )
    .await;

    let state_for_runner = state.clone();
    let job_id_for_runner = job_id.clone();
    let plan = EgressPlan {
        request_id: Some(req_id.clone()),
        probe,
        targets,
    };
    tokio::spawn(async move {
        crate::jobs::run_egress(state_for_runner, job_id_for_runner, plan).await;
    });

    let body = EgressApplyAcceptedResponse {
        job_id,
        status: "queued".to_string(),
        kind: "egress".to_string(),
        total,
        created_at: now,
        request_id: req_id,
    };
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}

/// 一台设备的下发计划（纯内存：此时还没落库）。
struct PlannedDeviceEgress {
    device_id: String,
    /// 归一化后的完整绑定；assignments 形态是「既有绑定 + 本次覆盖」的合并结果。
    rows: Vec<DeviceEgressWrite>,
}

/// 已校验的下发计划。
struct PlannedEgress {
    devices: Vec<PlannedDeviceEgress>,
    /// assignments 形态需要落库（dryRun 永远不落库）。
    persist: bool,
}

impl PlannedEgress {
    /// 展开成 (device, app) 目标；没有任何绑定的设备产出一行整机目标（= 清空该设备出口）。
    fn targets(&self) -> Vec<EgressTarget> {
        let mut targets = Vec::new();
        for device in &self.devices {
            if device.rows.is_empty() {
                targets.push(EgressTarget {
                    device_id: device.device_id.clone(),
                    app: String::new(),
                });
                continue;
            }
            for row in &device.rows {
                targets.push(EgressTarget {
                    device_id: device.device_id.clone(),
                    app: row.app.clone(),
                });
            }
        }
        targets
    }
}

/// 既有绑定 → 请求体形态（direct 行用保留值 __DIRECT__ 表示）。
fn existing_bindings(rows: Vec<DeviceEgressRow>) -> Vec<DeviceEgressBindingInput> {
    rows.into_iter()
        .map(|row| DeviceEgressBindingInput {
            app: row.app,
            proxy_id: match (row.mode.as_str(), row.proxy_id) {
                ("direct", _) => Some(DIRECT_PROXY.to_string()),
                (_, Some(proxy_id)) => Some(proxy_id),
                _ => None,
            },
        })
        .collect()
}

/// 读某设备当前的绑定（已经是落库形态）。
async fn current_device_rows(
    state: &AppState,
    device_id: &str,
) -> Result<Vec<DeviceEgressWrite>, ApiError> {
    let rows = state
        .db
        .list_device_egress(device_id)
        .await
        .map_err(|error| internal("egress.device_bindings", error))?;
    Ok(rows
        .into_iter()
        .map(|row| DeviceEgressWrite {
            app: row.app,
            mode: row.mode,
            proxy_id: row.proxy_id,
        })
        .collect())
}

/// 纯校验 / 归一化：只读 DB，绝不写库、不写审计、不建 job（dryRun 与真实下发共用）。
async fn plan_egress_targets(
    state: &AppState,
    devices: Option<DevicesSpec>,
    assignments: Option<Vec<EgressAssignmentInput>>,
) -> Result<PlannedEgress, ApiError> {
    if let Some(assignments) = assignments {
        if assignments.is_empty() || assignments.len() > MAX_BINDINGS {
            return Err(ApiError::validation(vec![ValidationError::new(
                "assignments",
                "too_many_items",
                format!("assignments 数量需为 1-{MAX_BINDINGS}"),
            )]));
        }
        let registered = state
            .db
            .list_devices()
            .await
            .map_err(|error| internal("egress.apply.devices", error))?;
        let mut errors = Vec::new();
        let mut grouped: Vec<(String, Vec<DeviceEgressBindingInput>)> = Vec::new();
        for (index, assignment) in assignments.into_iter().enumerate() {
            let path = format!("assignments[{index}]");
            if !registered
                .iter()
                .any(|device| device.id == assignment.device_id)
            {
                errors.push(ValidationError::new(
                    format!("{path}.deviceId"),
                    "invalid_device_id",
                    format!("设备未注册：{}", assignment.device_id),
                ));
                continue;
            }
            let position = grouped
                .iter()
                .position(|(device_id, _)| device_id == &assignment.device_id);
            let bindings = match position {
                Some(index) => &mut grouped[index].1,
                None => {
                    grouped.push((assignment.device_id.clone(), Vec::new()));
                    &mut grouped.last_mut().expect("刚插入的设备分组").1
                }
            };
            bindings.push(DeviceEgressBindingInput {
                app: assignment.app,
                proxy_id: assignment.proxy_id,
            });
        }
        if !errors.is_empty() {
            return Err(ApiError::validation(errors));
        }

        // 同一设备的 assignments 与既有绑定合并后整体替换；这里只做归一化，不落库。
        let mut planned = Vec::new();
        for (device_id, incoming) in grouped {
            let mut merged = existing_bindings(
                state
                    .db
                    .list_device_egress(&device_id)
                    .await
                    .map_err(|error| internal("egress.device_bindings", error))?,
            );
            for binding in incoming {
                if let Some(existing) = merged
                    .iter_mut()
                    .find(|existing| existing.app == binding.app)
                {
                    existing.proxy_id = binding.proxy_id;
                } else {
                    merged.push(binding);
                }
            }
            let prepared = prepare_bindings(state, &device_id, merged).await?;
            planned.push(PlannedDeviceEgress {
                device_id,
                rows: prepared,
            });
        }
        return Ok(PlannedEgress {
            devices: planned,
            persist: true,
        });
    }

    let Some(devices) = devices else {
        return Err(ApiError::validation(vec![ValidationError::new(
            "devices",
            "missing_required",
            "devices 与 assignments 必须给一个",
        )]));
    };
    let registered = state
        .db
        .list_devices()
        .await
        .map_err(|error| internal("egress.apply.devices", error))?;
    let device_ids = match devices {
        DevicesSpec::All(literal) if literal == "all" => {
            registered.into_iter().map(|device| device.id).collect()
        }
        DevicesSpec::All(_) => {
            return Err(ApiError::validation(vec![ValidationError::new(
                "devices",
                "invalid_value",
                "devices 只能是 \"all\" 或设备 id 数组",
            )]));
        }
        DevicesSpec::List(ids_list) => {
            if ids_list.is_empty() || ids_list.len() > MAX_BINDINGS {
                return Err(ApiError::validation(vec![ValidationError::new(
                    "devices",
                    "too_many_items",
                    format!("devices 数量需为 1-{MAX_BINDINGS}"),
                )]));
            }
            if let Some(missing) = ids_list
                .iter()
                .find(|id| !registered.iter().any(|device| &device.id == *id))
            {
                return Err(ApiError::validation(vec![ValidationError::new(
                    "devices",
                    "invalid_device_id",
                    format!("设备未注册：{missing}"),
                )]));
            }
            ids_list
        }
    };

    let mut planned = Vec::new();
    for device_id in device_ids {
        let rows = current_device_rows(state, &device_id).await?;
        planned.push(PlannedDeviceEgress { device_id, rows });
    }
    Ok(PlannedEgress {
        devices: planned,
        persist: false,
    })
}

/// assignments 形态的落库：整体替换绑定 + 写 egress.bind 审计（dryRun 不走这里）。
async fn persist_egress_bindings(
    state: &AppState,
    devices: &[PlannedDeviceEgress],
) -> Result<(), ApiError> {
    for device in devices {
        let changed = state
            .db
            .replace_device_egress(&device.device_id, device.rows.clone(), &ids::now_rfc3339())
            .await
            .map_err(|error| internal("egress.bind", error))?;
        write_audit(
            state,
            "egress.bind",
            Some("device"),
            Some(&device.device_id),
            "ok",
            json!({
                "deviceId": device.device_id,
                "changed": changed,
                "total": device.rows.len(),
            }),
        )
        .await;
    }
    Ok(())
}

async fn build_dry_run_plan(
    state: &AppState,
    planned: &PlannedEgress,
) -> Result<Vec<EgressApplyPlanItem>, ApiError> {
    let proxies = state
        .db
        .list_egress_proxies()
        .await
        .map_err(|error| internal("egress.list", error))?;
    let mut plan = Vec::new();
    for device in &planned.devices {
        if device.rows.is_empty() {
            plan.push(EgressApplyPlanItem {
                device_id: device.device_id.clone(),
                app: String::new(),
                mode: "direct".to_string(),
                proxy_name: None,
            });
            continue;
        }
        for row in &device.rows {
            let proxy_name = row.proxy_id.as_deref().and_then(|proxy_id| {
                proxies
                    .iter()
                    .find(|proxy| proxy.id == proxy_id)
                    .map(|proxy| proxy.name.clone())
            });
            plan.push(EgressApplyPlanItem {
                device_id: device.device_id.clone(),
                app: row.app.clone(),
                mode: row.mode.clone(),
                proxy_name,
            });
        }
    }
    Ok(plan)
}

/// Idempotency-Key：1-128 字符，不含空白/控制字符（与 POST /apply 同一规则）。
fn valid_idempotency_key(value: &str) -> bool {
    let length = value.chars().count();
    (1..=128).contains(&length)
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

/// 幂等重放：同键直接返回首次 job（沿用 §2.11，不设 TTL）。
fn replay_response(job: crate::db::JobRow) -> Response {
    let body = EgressApplyAcceptedResponse {
        job_id: job.id,
        status: "queued".to_string(),
        kind: job.kind,
        total: job.total,
        created_at: job.created_at,
        request_id: job.request_id,
    };
    let mut response = (StatusCode::ACCEPTED, Json(body)).into_response();
    response
        .headers_mut()
        .insert("idempotent-replay", HeaderValue::from_static("true"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_proxy_names() {
        assert!(is_valid_proxy_name("us_la_01"));
        assert!(is_valid_proxy_name("US-LA.01"));
        assert!(is_valid_proxy_name("_x"));
        assert!(!is_valid_proxy_name(""));
        assert!(!is_valid_proxy_name("__DIRECT__"));
        assert!(!is_valid_proxy_name("-x"));
        assert!(!is_valid_proxy_name("has space"));
        assert!(!is_valid_proxy_name(&"x".repeat(65)));
    }

    #[test]
    fn validates_probe_urls() {
        assert!(is_valid_probe_url("http://checkip.amazonaws.com/"));
        assert!(is_valid_probe_url("http://127.0.0.1:8080/ip"));
        assert!(!is_valid_probe_url("https://checkip.amazonaws.com/"));
        assert!(!is_valid_probe_url("http://user:pass@host/ip"));
        assert!(!is_valid_probe_url("http://"));
        assert!(!is_valid_probe_url(&format!(
            "http://host/{}",
            "x".repeat(600)
        )));
    }

    #[test]
    fn parses_exit_ips() {
        assert_eq!(
            parse_exit_ip("203.0.113.7\n").as_deref(),
            Some("203.0.113.7")
        );
        assert_eq!(
            parse_exit_ip("{\"ip\":\"198.51.100.7\"}").as_deref(),
            Some("198.51.100.7")
        );
        assert_eq!(
            parse_exit_ip("{\"origin\":\"198.51.100.9, 10.0.0.1\"}").as_deref(),
            None
        );
        assert_eq!(parse_exit_ip("<html>nope</html>"), None);
        assert_eq!(parse_exit_ip(""), None);
        assert_eq!(parse_exit_ip("1.2.3.4, 5.6.7.8"), None);
        // 合法的 IPv6 字面量可以带 ':'。
        assert_eq!(parse_exit_ip("2001:db8::1").as_deref(), Some("2001:db8::1"));
        assert_eq!(parse_exit_ip("[2001:db8::1]"), None);
    }

    #[test]
    fn rejects_colon_text_unless_ipv6_literal() {
        // 与设备端同规则：出现 ':' 但解析不成 IPv6 → 一律拒绝（不能把 host:port 当出口 IP）。
        for body in [
            "203.0.113.7:80",
            "checkip.example.com:443",
            "{\"ip\":\"203.0.113.7:80\"}",
            "{\"query\":\"host:1080\"}",
            "::not-an-ip",
        ] {
            assert_eq!(parse_exit_ip(body), None, "{body}");
        }
        assert_eq!(parse_exit_ip("::1").as_deref(), Some("::1"));
    }

    #[test]
    fn validates_idempotency_keys() {
        assert!(valid_idempotency_key("egress-key-1"));
        assert!(valid_idempotency_key(&"x".repeat(128)));
        assert!(!valid_idempotency_key(""));
        assert!(!valid_idempotency_key("with space"));
        assert!(!valid_idempotency_key("line\nbreak"));
        assert!(!valid_idempotency_key(&"x".repeat(129)));
    }

    #[test]
    fn zeroizes_request_credentials_after_encryption() {
        let mut username = Some("proxy-user".to_string());
        let mut password = Some("hunter2-super-secret".to_string());

        // 加密一跳：取到密文后明文立即清零。
        let text = password.clone().unwrap();
        let _cipher = format!("v1:{}", text.len());
        password.as_mut().expect("password").zeroize();

        zeroize_credentials(&mut username, &mut password);

        assert_eq!(username.as_deref(), Some(""));
        assert_eq!(password.as_deref(), Some(""));
        assert!(!format!("{username:?}{password:?}").contains("hunter2"));
        assert!(!format!("{username:?}{password:?}").contains("proxy-user"));
    }

    #[test]
    fn validates_hosts_and_types() {
        assert!(is_valid_proxy_host("203.0.113.10"));
        assert!(is_valid_proxy_host("proxy.example.com"));
        assert!(!is_valid_proxy_host("http://host"));
        assert!(!is_valid_proxy_host("host:1080/path"));
        assert!(!is_valid_proxy_host("user@host"));
        assert!(!is_valid_proxy_host(""));
        // 含 ':' 时只允许 IPv6 字面量（端口有独立字段）。
        assert!(!is_valid_proxy_host("host:1080"));
        assert!(!is_valid_proxy_host("203.0.113.10:1080"));
        assert!(!is_valid_proxy_host("[2001:db8::1]"));
        assert!(is_valid_proxy_host("2001:db8::1"));
        assert!(is_valid_proxy_type("http"));
        assert!(is_valid_proxy_type("socks5"));
        assert!(!is_valid_proxy_type("socks4"));
    }
}
