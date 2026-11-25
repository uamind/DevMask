// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 出口代理端点：GET/PUT/DELETE /egress、POST /egress/probe、GET /egress/proxies。
//!
//! 契约：docs/plans/egress-contract.md §1（端点）、§3（方案 A）、§4（探测）、§7（凭据）。
//!
//! 三条容易踩的规则：
//!
//! 1. 这些端点**不设置** X-DF-Revision / ETag，只设置 X-DF-Egress-Revision
//!    （否则既有 WebUI 的拦截器会把 config revision 覆盖成 egress revision，
//!    下一次 PUT /config 必然 412）；
//! 2. 写路径的 If-Match 比较对象是 egress revision（config revision 与 egress.toml 字节的合成值）；
//! 3. 响应/日志/审计/错误 details 里没有密码，GET 只回 hasCredentials。

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Extension, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use device_faker_core::config::{Config, DIRECT_MARKER, EgressSection};
use device_faker_core::revision::{FileLock, atomic_write, create_backup};
use device_faker_core::validate::{self, ValidationOptions};
use serde_json::{Value, json};
use toml_edit::DocumentMut;

use crate::api::{self, json as json_response};
use crate::app::AppState;
use crate::dto::Warning;
use crate::egress::dto::{
    AppEgressView, EgressFileView, EgressResponse, EgressStateView, EgressWriteResult, HealthView,
    ProbeResponse, ProxiesResponse, ProxyListItem, ProxySummary, ReadbackView,
    RelayRestartResponse,
};
use crate::egress::file::{self, EgressFile, EgressSnapshot};
use crate::egress::probe::{self, Channel};
use crate::egress::request::{self as egress_request, AppUpdates, EgressUpdateRequest};
use crate::egress::state::{self as egress_state, LastProbe};
use crate::egress::system::{self as egress_system, Expected, Readback, SystemProxy};
use crate::egress::{
    CONFIG_FILE_MODE, default_proxy, egress_revision, expected_for, expected_for_relay,
};
use crate::error::ApiError;
use crate::params::{self, Params};
use crate::relay::{self, RelayView};
use crate::requests::RequestId;
use crate::store;

/// 两个文件的当前状态（一次请求内只读一次）。
struct Situation {
    config_snapshot: store::Snapshot,
    config: Config,
    egress: EgressSnapshot,
    /// egress revision（config revision + egress.toml 字节）。
    revision: String,
    /// 文件级警告（权限自愈、未知字段、引用缺失）。
    warnings: Vec<Warning>,
}

fn load_situation(state: &AppState, request_id: &str) -> Result<Situation, ApiError> {
    let config_snapshot = store::read_snapshot(state.store())
        .map_err(|error| ApiError::internal_error(request_id, format!("读取配置失败: {error}")))?;
    let text = if config_snapshot.exists {
        config_snapshot.text.as_str()
    } else {
        ""
    };
    let config = Config::from_toml(text).map_err(|error| {
        ApiError::internal_error(
            request_id,
            format!("配置解析失败: {}", crate::util::parse_error_message(&error)),
        )
    })?;
    let egress = state.egress().store().read().map_err(|error| {
        ApiError::internal_error(request_id, format!("读取 egress.toml 失败: {error}"))
    })?;
    if let Some(error) = &egress.parse_error {
        // fail closed：损坏的 egress.toml 绝不按「空配置」继续，客户端用 If-Match: * 重写修复。
        return Err(ApiError::internal_error(
            request_id,
            "egress.toml 无法解析（用 If-Match: * 的 PUT /egress 重写可修复）",
        )
        .with_details(json!({ "op": "egress_parse", "message": error })));
    }
    let revision = egress_revision(&config_snapshot.revision, &egress.bytes);
    let mut warnings = Vec::new();
    if let Some(mode) = egress.perm_fixed {
        warnings.push(Warning::new(
            "egress_perm_fixed",
            "egress.toml 权限过宽，已自愈为 0600",
            Some(json!({
                "path": state.egress().store().path().display().to_string(),
                "mode": format!("{mode:04o}"),
            })),
        ));
    }
    if !egress.model.unknown_fields.is_empty() {
        warnings.push(Warning::new(
            "unknown_field_preserved",
            format!(
                "egress.toml 里有 {} 个未知字段，已原样保留",
                egress.model.unknown_fields.len()
            ),
            Some(json!({ "fields": egress.model.unknown_fields })),
        ));
    }
    // 契约 §9.1 K3：`egress_credentials_ignored` 收窄为「relay 关闭时的设备级带凭据」。
    let relay_effective = state.relay().effective_enabled(&egress.model);
    warnings.extend(reference_warnings(&config, &egress.model, relay_effective));
    Ok(Situation {
        config_snapshot,
        config,
        egress,
        revision,
        warnings,
    })
}

/// 引用类警告：代理名未定义 / 设备级带凭据被忽略 / 内联覆盖模板。
fn reference_warnings(config: &Config, model: &EgressFile, relay_effective: bool) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let default = config
        .egress
        .as_ref()
        .and_then(|section| section.default.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(name) = default {
        match model.proxy(name) {
            None => warnings.push(Warning::new(
                "egress_proxy_not_found",
                format!("设备级默认代理 {name} 未在 egress.toml 定义，当前按直连处理"),
                Some(json!({ "name": name, "scope": "device" })),
            )),
            // 契约 §9.1 K3：relay 生效时设备级**承载认证**，不再报「凭据被忽略」；
            // 只有中继关闭（配置选择）时才回到 v1.10.0 的语义。
            Some(proxy) if proxy.has_credentials() && !relay_effective => {
                warnings.push(Warning::new(
                    "egress_credentials_ignored",
                    "设备级代理带凭据且中继未启用：方案 A 只写 host:port，不承载认证",
                    Some(json!({ "name": name })),
                ))
            }
            Some(_) => {}
        }
    }
    for app in &config.apps {
        let Some(value) = app
            .egress
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if value != DIRECT_MARKER && model.proxy(value).is_none() {
            warnings.push(Warning::new(
                "egress_proxy_not_found",
                format!("{} 引用的代理 {value} 未在 egress.toml 定义", app.package),
                Some(json!({ "name": value, "scope": "app", "package": app.package })),
            ));
        }
        for (name, template) in &config.templates {
            if template
                .packages
                .iter()
                .any(|package| package == &app.package)
            {
                warnings.push(Warning::new(
                    "egress_inline_shadows_template",
                    format!(
                        "{} 的内联 egress 会为该包建立内联记录，整记录覆盖模板 {name}",
                        app.package
                    ),
                    Some(json!({ "package": app.package, "template": name })),
                ));
            }
        }
    }
    warnings
}

/// 带凭据代理的显式警告码。
///
/// 背景（t15 真机结论）：libcore 不读取 java.net.socks.* 的凭据，应用级 HTTP 认证也未实现，
/// 因此「配了用户名/密码」目前是**静默不生效**——比明确报错更糟。这里在 GET/PUT/DELETE 的
/// warnings 与 probe 响应里统一暴露它，写操作仍然成功（不阻断）。
///
/// details 固定三个键：`{scope:"device"|"app", proxy:<代理名>, field:"username"|"password"}`；
/// 同一代理只报一次（device 优先，其次按 [[apps]] 顺序的第一条引用者）。
pub const AUTH_UNSUPPORTED: &str = "egress_auth_unsupported";

fn credential_field(username: &Option<String>, password: &Option<String>) -> Option<&'static str> {
    if username.is_some() {
        Some("username")
    } else if password.is_some() {
        Some("password")
    } else {
        None
    }
}

fn auth_warning(scope: &str, proxy: &str, field: &str) -> Warning {
    Warning::new(
        AUTH_UNSUPPORTED,
        format!(
            "代理 {proxy} 带凭据，但设备端注入不支持认证（libcore 不读取 java.net.socks.* 凭据、HTTP 认证未实现），该凭据不会生效"
        ),
        Some(json!({ "scope": scope, "proxy": proxy, "field": field })),
    )
}

/// 生效代理（设备级 default + 应用级覆盖）里带凭据、且**中继未生效**的那些各给一条提醒。
///
/// 契约 §9.1 K1：relay 生效时凭据由中继承担，应用侧不需要也不该看到凭据，
/// 因此 `egress_auth_unsupported` 收窄为「relay 未生效且带凭据才报」。
/// default 指向带凭据的 socks5 时给一条明确告警。
///
/// 设备级 scope 只对 http 成立（settings 的 http_proxy 只能承载 HTTP 代理语义），
/// socks5 只会按 app scope 生效；如果用户把它设成 default，中继不会建 listener，
/// 必须让 UI 看到「这条配置不会以设备级方式生效」，而不是静默无效。
fn device_socks5_warning(config: &Config, model: &EgressFile) -> Option<Warning> {
    // 条件与 expected_for_relay 的黑洞判定严格一致：default 不是 http 就告警，
    // 与 relay 是否启用、是否带凭据都无关（启用 → 黑洞；未启用 → 按未配置处理，仍要告警）。
    let name = crate::relay::port::device_default_name(config)?;
    let proxy = model.proxy(name)?;
    if proxy.kind == crate::egress::file::ProxyType::Http {
        return None;
    }
    Some(Warning::new(
        "egress_relay_device_socks5_unsupported",
        format!(
            "代理 {name} 是 socks5：设备级中继只支持 http，该定义不会以设备级方式监听（可绑定到应用按 app scope 使用）"
        ),
        Some(serde_json::json!({
            "proxy": name,
            "scope": "device",
            "reason": "socks5_device_scope_unsupported",
        })),
    ))
}

fn auth_warnings(
    config: &Config,
    model: &EgressFile,
    relay_active: &std::collections::BTreeMap<String, bool>,
    relay_enabled: bool,
) -> Vec<Warning> {
    // 契约 §9.1 K1：relay 生效时凭据由中继承载，**不要**再报「凭据不生效」，
    // 否则 UI 会误显示（t9 blocker F3）。
    if relay_enabled {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut push = |scope: &str, proxy: &crate::egress::ProxyDefinition| {
        let Some(field) = credential_field(&proxy.username, &proxy.password) else {
            return;
        };
        if relay_active.get(&proxy.name).copied().unwrap_or(false) {
            return;
        }
        if seen.iter().any(|name| name == &proxy.name) {
            return;
        }
        seen.push(proxy.name.clone());
        warnings.push(auth_warning(scope, &proxy.name, field));
    };
    if let Some(proxy) = default_proxy(config, model) {
        push("device", proxy);
    }
    for app in app_views(config, model) {
        if let Some(name) = app.proxy.as_deref()
            && let Some(proxy) = model.proxy(name)
        {
            push("app", proxy);
        }
    }
    warnings
}

fn app_views(config: &Config, model: &EgressFile) -> Vec<AppEgressView> {
    config
        .apps
        .iter()
        .filter_map(|app| {
            let value = app
                .egress
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            // 定义缺失时按「无覆盖」计算（t11 F9）：effective 跟随设备级，proxy 给设备级默认名或 null，
            // 不能谎报 effective=proxy 却给不出代理名。
            let (effective, proxy) = if value == DIRECT_MARKER {
                ("direct", None)
            } else if model.proxy(value).is_some() {
                ("proxy", Some(value.to_owned()))
            } else if mode_of(config) == "proxy" {
                (
                    "proxy",
                    default_proxy(config, model).map(|proxy| proxy.name.clone()),
                )
            } else {
                ("direct", None)
            };
            Some(AppEgressView {
                package: app.package.clone(),
                egress: value.to_owned(),
                effective,
                proxy,
            })
        })
        .collect()
}

fn file_view(egress: &EgressSnapshot) -> EgressFileView {
    EgressFileView {
        exists: egress.exists,
        size: egress.size,
        mtime: egress.mtime.clone(),
    }
}

fn mode_of(config: &Config) -> &'static str {
    let configured = config
        .egress
        .as_ref()
        .and_then(|section| section.default.as_deref())
        .is_some_and(|name| !name.trim().is_empty());
    if configured { "proxy" } else { "direct" }
}

fn readback_view(readback: &Readback, matched: bool) -> ReadbackView {
    ReadbackView {
        settings_value: readback.settings_value.clone(),
        dumpsys_global_proxy: readback.dumpsys_global_proxy.clone(),
        matches: matched,
        at: readback.at.clone(),
    }
}

fn empty_readback() -> Readback {
    Readback {
        settings_value: None,
        dumpsys_global_proxy: None,
        dirty: false,
        at: crate::util::now_rfc3339(),
        available: false,
    }
}

/// 组装状态块（GET 与写端点响应共用）。
fn state_view(
    state: &AppState,
    situation: &Situation,
    relay_override: Option<RelayView>,
) -> EgressStateView {
    let model = &situation.egress.model;
    // 中继生效时设备级期望值是 127.0.0.1:<base_port>（契约 §3.1 规则 4）；
    // 绑定失败 → 清直连（规则 6），读回比较随之变成 :0。
    let relay_status = state.relay().device_status(model);
    let expected = expected_for_relay(&situation.config, model, Some(&relay_status));
    let relay_view = relay_override.unwrap_or_else(|| state.relay().view(model));
    let relay_active = state.relay().relay_active(model);
    let relay_enabled = state.relay().effective_enabled(model);
    let system = SystemProxy::new(state.commands());
    let readback = system.readback().unwrap_or_else(|_| empty_readback());
    let matched = egress_system::matches(&expected, &readback);
    let exclusions = system.exclusions().unwrap_or_default();
    let health = state.egress().runtime().snapshot();

    let mut warnings = situation.warnings.clone();
    // relay 运行态告警（4 个码里的 3 个在 t2；firewall 由 t3 接管）。
    warnings.extend(relay::dto::relay_warnings(&relay_view));
    // 设备级中继只承载 http：default 指向带凭据的 socks5 时不会建 listener，
    // 必须显式告警，不能静默「配了但不可用」（t25 F1）。
    if let Some(warning) = device_socks5_warning(&situation.config, model) {
        warnings.push(warning);
    }
    // 带凭据的生效代理：**只在中继未生效时**才报「凭据不生效」（契约 §9.1 K1 收窄）。
    warnings.extend(auth_warnings(
        &situation.config,
        model,
        &relay_active,
        relay_enabled,
    ));
    if !readback.available {
        // settings 命令不可用：读回不可信，明确告知原因（t11 F6）
        warnings.push(Warning::new(
            "egress_settings_mismatch",
            "无法读回系统代理（settings 命令不可用），applied 只能报 false",
            Some(json!({ "reason": "settings_unavailable" })),
        ));
    } else if !matched {
        warnings.push(Warning::new(
            "egress_settings_mismatch",
            "写入后的读回值与期望不一致",
            Some(json!({
                "expected": expected.value(),
                "actual": readback.settings_value.clone(),
            })),
        ));
    }
    // 只有真的切到直连才报 egress_degraded_to_direct；没落地时由上面的 settings_unavailable 解释
    if health.degraded && health.cleared {
        warnings.push(Warning::new(
            "egress_degraded_to_direct",
            "已按 fail-open 切为直连（配置保持不变）",
            Some(json!({ "reason": health.degraded_reason })),
        ));
    }

    let mode = mode_of(&situation.config);
    // effectiveMode 报「实际」模式：降级且清直连落地才是 direct
    let effective_mode = if health.degraded && health.cleared {
        "direct"
    } else {
        mode
    };
    let configured = !model.proxies.is_empty()
        || mode == "proxy"
        || !app_views(&situation.config, model).is_empty();

    EgressStateView {
        configured,
        mode,
        effective_mode,
        default: situation
            .config
            .egress
            .as_ref()
            .and_then(|section| section.default.clone())
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty()),
        fail_open: model.probe.fail_open,
        probe_url: model.probe.url.clone(),
        probe_timeout_seconds: model.probe.timeout_seconds,
        exclusion_list: exclusions,
        upstream: default_proxy(&situation.config, model)
            .map(|proxy| ProxySummary::from_definition(proxy, &relay_active, relay_enabled)),
        proxies: model
            .proxies
            .iter()
            .map(|proxy| ProxySummary::from_definition(proxy, &relay_active, relay_enabled))
            .collect(),
        apps: app_views(&situation.config, model),
        applied: matched,
        readback: readback_view(&readback, matched),
        health: HealthView::from_snapshot(&health),
        egress_file: file_view(&situation.egress),
        relay: relay_view,
        warnings,
    }
}

fn channel_for(proxy: &crate::egress::ProxyDefinition) -> Channel {
    Channel::Proxy {
        name: proxy.name.clone(),
        kind: proxy.kind,
        host: proxy.host.clone(),
        port: proxy.port,
        username: proxy.username.clone(),
        password: proxy.password.clone(),
    }
}

/// 解析探测链路（设备级 / 应用级）。
fn resolve_channel(
    config: &Config,
    model: &EgressFile,
    package: Option<&str>,
    request_id: &str,
) -> Result<(Channel, &'static str), ApiError> {
    let scope = if package.is_some() { "app" } else { "device" };
    if let Some(package) = package {
        let base = crate::packages::base_package(package);
        let app = config
            .apps
            .iter()
            .find(|app| app.package == package)
            .or_else(|| {
                config
                    .apps
                    .iter()
                    .find(|app| crate::packages::base_package(&app.package) == base)
            });
        let value = app
            .and_then(|app| app.egress.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match value {
            Some(DIRECT_MARKER) => return Ok((Channel::Direct, scope)),
            Some(name) => match model.proxy(name) {
                Some(proxy) => return Ok((channel_for(proxy), scope)),
                None => {
                    return Err(ApiError::egress_not_configured(
                        request_id,
                        scope,
                        Some(package),
                    ));
                }
            },
            None => {}
        }
    }
    if let Some(proxy) = default_proxy(config, model) {
        return Ok((channel_for(proxy), scope));
    }
    if model.proxies.is_empty() {
        return Err(ApiError::egress_not_configured(request_id, scope, package));
    }
    // 有定义但设备级没有 default：方案 A 不写代理，探测按直连做。
    Ok((Channel::Direct, scope))
}

/// 解析 `via="relay"` 的通道：按 package（或设备级默认）选中定义的中继端口。
///
/// 契约 §3.5 第 1–4 条：没有可用中继端口 → `409 egress.not_configured`（details 带
/// `via:"relay"`）；端口在端口表里但 listener 没在监听 → `502 egress.probe_failed`
/// （`reason="relay_not_listening"`）。
fn resolve_relay_channel(
    state: &AppState,
    situation: &Situation,
    package: Option<&str>,
    url: &str,
    timeout_ms: u64,
    request_id: &str,
) -> Result<(Channel, &'static str), ApiError> {
    let scope = if package.is_some() { "app" } else { "device" };
    let model = &situation.egress.model;
    let not_configured = || {
        ApiError::egress_not_configured(request_id, scope, package).with_details(json!({
            "scope": scope,
            "package": package,
            "via": "relay",
        }))
    };
    let name = match package {
        Some(package) => {
            let base = crate::packages::base_package(package);
            let app = situation
                .config
                .apps
                .iter()
                .find(|app| app.package == package)
                .or_else(|| {
                    situation
                        .config
                        .apps
                        .iter()
                        .find(|app| crate::packages::base_package(&app.package) == base)
                });
            match app
                .and_then(|app| app.egress.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(DIRECT_MARKER) => return Err(not_configured()),
                Some(name) => Some(name.to_owned()),
                None => {
                    crate::relay::port::device_default_name(&situation.config).map(str::to_owned)
                }
            }
        }
        None => crate::relay::port::device_default_name(&situation.config).map(str::to_owned),
    };
    let Some(name) = name else {
        return Err(not_configured());
    };
    let Some(proxy) = model.proxy(&name) else {
        return Err(not_configured());
    };
    // 判定顺序（评审口径）：先判「中继到底有没有启用」，再判「端口在不在监听」。
    // 把「中继根本没启用」报成「端口没监听」会误导排障。
    if !model.relay.enabled || !proxy.has_credentials() {
        return Err(not_configured());
    }
    let Some(port) = proxy.local_port else {
        return Err(not_configured());
    };
    let active = state
        .relay()
        .relay_active(model)
        .get(&name)
        .copied()
        .unwrap_or(false);
    if !active {
        return Err(ApiError::egress_probe_failed(
            request_id,
            url,
            "relay_not_listening",
            timeout_ms,
            None,
        )
        .with_details(json!({
            "url": url,
            "reason": "relay_not_listening",
            "timeoutMs": timeout_ms,
            "upstreamStatus": null,
            "scope": scope,
            "via": "relay",
            "relayPort": port,
            "warnings": [],
        })));
    }
    Ok((Channel::Relay { name, port }, scope))
}

/// GET /api/v1/egress（read 作用域）。
pub async fn get_egress(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let situation = load_situation(&state, request_id.as_str())?;
    let mut view = state_view(&state, &situation, None);
    for app in &view.apps {
        let installed = state
            .packages()
            .is_installed(crate::packages::base_package(&app.package));
        if installed == Some(false) {
            view.warnings.push(Warning::new(
                "egress_app_not_installed",
                format!("{} 当前未安装，配置仍会保留", app.package),
                Some(json!({ "package": app.package })),
            ));
        }
    }
    let body = EgressResponse {
        revision: situation.revision.clone(),
        config_revision: situation.config_snapshot.revision.clone(),
        state: view,
    };
    Ok(api::with_egress_revision(
        json_response(StatusCode::OK, body),
        &situation.revision,
    ))
}

/// GET /api/v1/egress/proxies（read 作用域，脱敏）。
pub async fn list_proxies(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let situation = load_situation(&state, request_id.as_str())?;
    let model = &situation.egress.model;
    let default = situation
        .config
        .egress
        .as_ref()
        .and_then(|section| section.default.as_deref())
        .map(str::trim);
    let relay_active = state.relay().relay_active(model);
    let relay_enabled = state.relay().effective_enabled(model);
    let items: Vec<ProxyListItem> = model
        .proxies
        .iter()
        .map(|proxy| {
            let apps: Vec<String> = situation
                .config
                .apps
                .iter()
                .filter(|app| app.egress.as_deref().map(str::trim) == Some(proxy.name.as_str()))
                .map(|app| app.package.clone())
                .collect();
            ProxyListItem::new(
                proxy,
                default == Some(proxy.name.as_str()),
                apps,
                relay_active.get(&proxy.name).copied().unwrap_or(false),
                relay_enabled,
            )
        })
        .collect();
    let body = ProxiesResponse {
        revision: situation.revision.clone(),
        config_revision: situation.config_snapshot.revision.clone(),
        total: items.len(),
        egress_file: file_view(&situation.egress),
        items,
        warnings: situation.warnings.clone(),
    };
    Ok(api::with_egress_revision(
        json_response(StatusCode::OK, body),
        &situation.revision,
    ))
}

/// 删除**除 keep 之外**所有 `[[apps]].egress`：PUT 的 apps 数组是「整体替换」，
/// 没列出的应用必须把 egress 键删掉（其它机型字段一律不动）。
fn clear_app_egress_except(document: &mut DocumentMut, keep: &[&str]) -> usize {
    let Some(apps) = document
        .get_mut("apps")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
    else {
        return 0;
    };
    let mut changed = 0usize;
    for table in apps.iter_mut() {
        let Some(package) = table.get("package").and_then(toml_edit::Item::as_str) else {
            continue;
        };
        if keep.contains(&package) {
            continue;
        }
        if table.remove("egress").is_some() {
            changed += 1;
        }
    }
    changed
}

/// 契约 §6.5：`localPort`/`local_port` 由 server 分配，客户端只读。
///
/// 必须在 serde 反序列化**之前**拦：`ProxyInput` 的 `deny_unknown_fields` 只会给出
/// `invalid_type`，而契约要求 `unknown_field` + 固定 message。
fn reject_readonly_fields(
    map: &serde_json::Map<String, Value>,
    request_id: &str,
) -> Result<(), ApiError> {
    let mut errors = Vec::new();
    if let Some(Value::Array(proxies)) = map.get("proxies") {
        for (index, item) in proxies.iter().enumerate() {
            let Some(object) = item.as_object() else {
                continue;
            };
            for key in ["localPort", "local_port"] {
                if object.contains_key(key) {
                    errors.push(crate::dto::ValidationError::new(
                        format!("proxies[{index}].{key}"),
                        "unknown_field",
                        "localPort 由服务端分配，客户端只读",
                    ));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(params::validation_failed(request_id, errors))
    }
}

fn is_known_update_key(key: &str) -> bool {
    matches!(
        key,
        "default"
            | "failOpen"
            | "probeUrl"
            | "probeTimeoutSeconds"
            | "exclusionList"
            | "proxies"
            | "apps"
    )
}

/// settings 命令失败 → 503 + 一行诊断日志（保留截断 stderr，不含凭据）。
///
/// 日志的断言在 `#[cfg(test)] mod tests`（同步作用域，不受并行测试影响）。
fn settings_api_error(request_id: &str, error: &crate::egress::SettingsError) -> ApiError {
    match error {
        crate::egress::SettingsError::Failed { code, stderr } => {
            tracing::warn!(
                component = "settings",
                code = ?code,
                stderr = %stderr,
                "settings 命令执行失败"
            );
        }
        crate::egress::SettingsError::Unavailable => {
            tracing::warn!(component = "settings", "settings 命令不可用");
        }
    }
    ApiError::upstream_unavailable(request_id, error.component())
}

fn combined_diff(
    old_config: &str,
    new_config: &str,
    old_egress: &str,
    new_egress: &str,
) -> (String, bool) {
    let mut text = crate::diff::unified_diff("config.toml", old_config, new_config);
    text.push_str(&crate::diff::unified_diff(
        "egress.toml",
        old_egress,
        new_egress,
    ));
    // diff 也是响应体：egress.toml 的 username/password 行必须在返回前脱敏（t11 F1）
    crate::diff::cap(redact_diff_credentials(&text))
}

/// 把 diff 里的凭据行值替换为 `<redacted>`（保留键名与前缀，便于看清「哪个键变了」）。
fn redact_diff_credentials(diff: &str) -> String {
    let mut out = String::with_capacity(diff.len());
    for line in diff.split_inclusive('\n') {
        let (content, newline) = match line.strip_suffix('\n') {
            Some(content) => (content, "\n"),
            None => (line, ""),
        };
        out.push_str(&redact_diff_line(content));
        out.push_str(newline);
    }
    out
}

fn redact_diff_line(line: &str) -> String {
    // unified diff 的行前缀：'+'、'-'、' '（也兼容 "\ No newline" 之类）
    let (prefix, body) = match line.chars().next() {
        Some(mark @ ('+' | '-' | ' ')) => (&line[..mark.len_utf8()], &line[mark.len_utf8()..]),
        _ => ("", line),
    };
    let trimmed = body.trim_start();
    let indent = &body[..body.len() - trimmed.len()];
    for key in ["username", "password"] {
        if let Some(rest) = trimmed.strip_prefix(key)
            && rest.trim_start().starts_with('=')
        {
            return format!("{prefix}{indent}{key} = \"<redacted>\"");
        }
    }
    line.to_owned()
}

/// config.toml 备份的 backupId（`<stamp>-<rev8>`，与 [crate::egress::file::backup_id] 同一口径）。
fn config_backup_id(state: &AppState, path: &std::path::Path) -> Option<String> {
    let file_name = state
        .store()
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    crate::egress::file::backup_id(path, file_name)
}

/// PUT /api/v1/egress（write 作用域，If-Match 必填）。
pub async fn put_egress(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request_id = request_id.as_str().to_owned();
    let params = Params::new(query);
    let dry_run = params.flag("dry_run", &request_id)?;
    let want_diff = params.flag("diff", &request_id)?;

    let value = if body.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        params::require_json(&headers, &request_id)?;
        params::parse_json(&body, &request_id)?
    };
    let Value::Object(map) = &value else {
        return Err(params::field_error(
            &request_id,
            "$",
            "invalid_type",
            "请求体必须是 JSON 对象",
        ));
    };
    params::reject_unknown_keys(map, is_known_update_key, "", &request_id)?;
    // relay：localPort / local_port / relay* 属 server 拥有字段，客户端只读（契约 §6.5）。
    reject_readonly_fields(map, &request_id)?;
    let request: EgressUpdateRequest = params::typed(&value, &request_id)?;
    let errors = egress_request::validate(&request);
    if !errors.is_empty() {
        return Err(params::validation_failed(&request_id, errors));
    }
    let precondition =
        store::if_match(&headers).ok_or_else(|| ApiError::precondition_required(&request_id))?;

    // 锁顺序固定：config 锁 → egress 锁（避免与其它写端点交叉死锁）。
    let config_lock = FileLock::acquire(&state.store().lock_path()).map_err(|error| {
        ApiError::internal_error(&request_id, format!("获取配置锁失败: {error}"))
    })?;
    let egress_lock = state.egress().store().acquire().map_err(|error| {
        ApiError::internal_error(&request_id, format!("获取 egress 锁失败: {error}"))
    })?;

    // 已持有排他 flock：必须用 read_snapshot_locked，再取共享锁会自我死锁
    let config_snapshot = store::read_snapshot_locked(state.store())
        .map_err(|error| ApiError::internal_error(&request_id, format!("读取配置失败: {error}")))?;
    let egress = state.egress().store().read_locked().map_err(|error| {
        ApiError::internal_error(&request_id, format!("读取 egress.toml 失败: {error}"))
    })?;
    let current_revision = egress_revision(&config_snapshot.revision, &egress.bytes);
    match &precondition {
        store::Precondition::Any => {}
        store::Precondition::Exact(expected) if *expected == current_revision => {}
        store::Precondition::Exact(expected) => {
            return Err(ApiError::egress_revision_mismatch(
                &request_id,
                expected,
                Some(&current_revision),
            ));
        }
    }

    // egress.toml 损坏时以空文档为基线（这就是修复通道）。
    let current_model = if egress.parse_error.is_some() {
        EgressFile::empty()
    } else {
        egress.model.clone()
    };
    let merged = egress_request::merge_egress(&current_model, &request);

    let old_config_text = if config_snapshot.exists {
        config_snapshot.text.clone()
    } else {
        String::new()
    };
    let mut document: DocumentMut = old_config_text
        .parse()
        .map_err(|error| ApiError::internal_error(&request_id, format!("配置解析失败: {error}")))?;
    if let Some(default) = egress_request::default_update(&request) {
        device_faker_core::edit::set_egress(&mut document, &EgressSection { default }).map_err(
            |error| ApiError::internal_error(&request_id, format!("写入 [egress] 失败: {error}")),
        )?;
    }
    match egress_request::app_updates(&request) {
        AppUpdates::None => {}
        AppUpdates::ClearAll => {
            crate::egress::clear_all_app_egress(&mut document);
        }
        AppUpdates::Entries(entries) => {
            // 整体替换：先删掉不在本次列表里的 egress，再写列出的（其它字段一律不动）。
            let keep: Vec<&str> = entries
                .iter()
                .map(|(package, _)| package.as_str())
                .collect();
            clear_app_egress_except(&mut document, &keep);
            crate::egress::apply_app_egress(&mut document, &entries);
        }
    }
    let new_config_text = document.to_string();

    let report = validate::validate_document(&document, &ValidationOptions::default());
    if !report.is_ok() {
        let (errors, _) = crate::dto::map_validation_report(&report);
        return Err(params::validation_failed(&request_id, errors));
    }
    let section_errors = crate::dto::egress_section_issues(&document);
    if !section_errors.is_empty() {
        return Err(params::validation_failed(&request_id, section_errors));
    }
    let parsed = Config::from_toml(&new_config_text).map_err(|error| {
        ApiError::internal_error(
            &request_id,
            format!("配置解析失败: {}", crate::util::parse_error_message(&error)),
        )
    })?;
    // 设备级中继只支持 http：按**合并后的有效 default** 校验（t30 L1）。
    let effective_default = crate::relay::port::device_default_name(&parsed).map(str::to_owned);
    let scope_errors =
        egress_request::validate_default_scope(&merged, effective_default.as_deref());
    if !scope_errors.is_empty() {
        return Err(params::validation_failed(&request_id, scope_errors));
    }
    let limit_errors = crate::dto::limit_issues(&parsed);
    if !limit_errors.is_empty() {
        return Err(params::validation_failed(&request_id, limit_errors));
    }
    // 设备级只支持 IPv4/域名（方案 A 的 settings 形态），应用级仍可用 IPv6。
    if let Some(proxy) = default_proxy(&parsed, &merged)
        && proxy.host.parse::<std::net::Ipv6Addr>().is_ok()
    {
        return Err(params::field_error(
            &request_id,
            "default",
            "invalid_egress_proxy",
            "设备级仅支持 IPv4/域名（IPv6 字面量请用应用级覆盖）",
        ));
    }

    let base: DocumentMut = if egress.exists {
        egress.text.parse().unwrap_or_default()
    } else {
        DocumentMut::new()
    };
    let new_egress_text = file::render(&base, &merged).map_err(|error| {
        ApiError::internal_error(&request_id, format!("渲染 egress.toml 失败: {error}"))
    })?;
    let egress_changed = !egress.exists || egress.text != new_egress_text;
    let config_changed = new_config_text != old_config_text;
    let changed = egress_changed || config_changed;

    let mut backup_id: Option<String> = None;
    if changed && !dry_run {
        // 先写 egress.toml，再写 config.toml（中断只会留下未被引用的定义）。
        let outcome = state
            .egress()
            .store()
            .write_locked(&egress, &merged)
            .map_err(|error| {
                ApiError::internal_error(&request_id, format!("写入 egress.toml 失败: {error}"))
            })?;
        backup_id = outcome.backup_id;
        if config_changed {
            let backup = if config_snapshot.exists {
                create_backup(
                    state.store().path(),
                    &state.store().backup_dir(),
                    store::BACKUP_KEEP,
                    Some(&config_snapshot.revision),
                )
                .map_err(|error| {
                    ApiError::internal_error(&request_id, format!("备份配置失败: {error}"))
                })?
            } else {
                None
            };
            if backup_id.is_none() {
                backup_id = backup
                    .as_deref()
                    .and_then(|path| config_backup_id(&state, path));
            }
            atomic_write(
                state.store().path(),
                new_config_text.as_bytes(),
                CONFIG_FILE_MODE,
            )
            .map_err(|error| {
                ApiError::internal_error(&request_id, format!("写入配置失败: {error}"))
            })?;
        }
    }
    drop(egress_lock);
    drop(config_lock);

    // relay 重建必须在释放文件锁之后：activate 会回写端口表（同一个 egress store），
    // 持锁调用会自我死锁。
    let mut relay_view: Option<RelayView> = None;
    if changed && !dry_run {
        match relay::prepare_from_disk(&state, false).await {
            Ok((rebuild, persisted)) => {
                relay_view = Some(state.relay().activate(rebuild, persisted).await);
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "PUT 后重建中继失败（定义已写入，可用 POST /egress/relay/restart 修复）"
                );
            }
        }
    }

    // settings 只在 relay 重建**之后**写一次（t30 H1）：重建前 device_status 还是旧的，
    // 首次 PUT 会算出 Direct 并写 http_proxy=:0，窗口覆盖 bind + pkg@uid 解析 + iptables
    // apply —— 这段时间全设备直连，等于出口泄漏。exclusionList 必须在这一次里带上。
    if !dry_run {
        let fresh = load_situation(&state, &request_id)?;
        let status = state.relay().device_status(&fresh.egress.model);
        let expected = expected_for_relay(&fresh.config, &fresh.egress.model, Some(&status));
        let exclusions: Option<Vec<String>> = match &request.exclusion_list {
            None => None,
            Some(None) => Some(Vec::new()),
            Some(Some(list)) => Some(list.clone()),
        };
        let system = SystemProxy::new(state.commands());
        let outcome = system
            .apply(&expected, exclusions.as_deref())
            .map_err(|error| settings_api_error(&request_id, &error))?;
        if outcome.matches {
            state.egress().runtime().recover();
        } else if fresh.egress.model.probe.fail_open {
            egress_state::degrade_to_direct(&state, "settings_mismatch");
        } else {
            state.egress().runtime().degrade("settings_mismatch", false);
            tracing::warn!(
                reason = "settings_mismatch",
                cleared = false,
                "egress 写入后读回不一致（fail-closed，未改 settings）"
            );
        }
    }

    let situation = load_situation(&state, &request_id)?;
    let view = state_view(&state, &situation, relay_view);
    let diff = if want_diff || dry_run {
        Some(combined_diff(
            &old_config_text,
            &new_config_text,
            &egress.text,
            &new_egress_text,
        ))
    } else {
        None
    };
    let (diff, diff_truncated) = match diff {
        Some((text, truncated)) => (Some(text), truncated),
        None => (None, false),
    };
    let body = EgressWriteResult {
        revision: situation.revision.clone(),
        previous_revision: (egress.exists || config_snapshot.exists)
            .then(|| current_revision.clone()),
        config_revision: situation.config_snapshot.revision.clone(),
        dry_run,
        changed: changed && !dry_run,
        backup_id,
        diff,
        diff_truncated,
        state: view,
    };
    Ok(api::with_egress_revision(
        json_response(StatusCode::OK, body),
        &situation.revision,
    ))
}

pub async fn delete_egress(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let request_id = request_id.as_str().to_owned();
    let params = Params::new(query);
    params.confirm("egress_clear", &request_id)?;
    let precondition =
        store::if_match(&headers).ok_or_else(|| ApiError::precondition_required(&request_id))?;

    let config_lock = FileLock::acquire(&state.store().lock_path()).map_err(|error| {
        ApiError::internal_error(&request_id, format!("获取配置锁失败: {error}"))
    })?;
    let egress_lock = state.egress().store().acquire().map_err(|error| {
        ApiError::internal_error(&request_id, format!("获取 egress 锁失败: {error}"))
    })?;
    // 已持有排他 flock：必须用 read_snapshot_locked，再取共享锁会自我死锁
    let config_snapshot = store::read_snapshot_locked(state.store())
        .map_err(|error| ApiError::internal_error(&request_id, format!("读取配置失败: {error}")))?;
    let egress = state.egress().store().read_locked().map_err(|error| {
        ApiError::internal_error(&request_id, format!("读取 egress.toml 失败: {error}"))
    })?;
    let current_revision = egress_revision(&config_snapshot.revision, &egress.bytes);
    match &precondition {
        store::Precondition::Any => {}
        store::Precondition::Exact(expected) if *expected == current_revision => {}
        store::Precondition::Exact(expected) => {
            return Err(ApiError::egress_revision_mismatch(
                &request_id,
                expected,
                Some(&current_revision),
            ));
        }
    }

    let current_model = if egress.parse_error.is_some() {
        EgressFile::empty()
    } else {
        egress.model.clone()
    };
    // 清除清单第 1 条：删掉全部 [proxies.*]；[probe] 保留（策略不是出口）。
    let mut cleared = current_model.clone();
    cleared.proxies.clear();
    cleared
        .unknown_fields
        .retain(|path| !path.starts_with("proxies."));

    let old_config_text = if config_snapshot.exists {
        config_snapshot.text.clone()
    } else {
        String::new()
    };
    let mut document: DocumentMut = old_config_text
        .parse()
        .map_err(|error| ApiError::internal_error(&request_id, format!("配置解析失败: {error}")))?;
    // 第 2、3 条：删 [egress].default、清所有 [[apps]].egress（不动其它机型字段）。
    device_faker_core::edit::set_egress(&mut document, &EgressSection { default: None }).map_err(
        |error| ApiError::internal_error(&request_id, format!("清除 [egress] 失败: {error}")),
    )?;
    crate::egress::clear_all_app_egress(&mut document);
    let new_config_text = document.to_string();

    let base: DocumentMut = if egress.exists {
        egress.text.parse().unwrap_or_default()
    } else {
        DocumentMut::new()
    };
    let new_egress_text = file::render(&base, &cleared).map_err(|error| {
        ApiError::internal_error(&request_id, format!("渲染 egress.toml 失败: {error}"))
    })?;
    let egress_changed = egress.exists && egress.text != new_egress_text;
    let config_changed = new_config_text != old_config_text;
    let changed = egress_changed || config_changed;

    let mut backup_id: Option<String> = None;
    if changed {
        if egress_changed {
            let outcome = state
                .egress()
                .store()
                .write_locked(&egress, &cleared)
                .map_err(|error| {
                    ApiError::internal_error(&request_id, format!("写入 egress.toml 失败: {error}"))
                })?;
            backup_id = outcome.backup_id;
        }
        if config_changed {
            let backup = if config_snapshot.exists {
                create_backup(
                    state.store().path(),
                    &state.store().backup_dir(),
                    store::BACKUP_KEEP,
                    Some(&config_snapshot.revision),
                )
                .map_err(|error| {
                    ApiError::internal_error(&request_id, format!("备份配置失败: {error}"))
                })?
            } else {
                None
            };
            if backup_id.is_none() {
                backup_id = backup
                    .as_deref()
                    .and_then(|path| config_backup_id(&state, path));
            }
            atomic_write(
                state.store().path(),
                new_config_text.as_bytes(),
                CONFIG_FILE_MODE,
            )
            .map_err(|error| {
                ApiError::internal_error(&request_id, format!("写入配置失败: {error}"))
            })?;
        }
    }
    // settings 清直连也放进同一临界区（t11 F5）：否则并发写会让「清掉的」与「刚写上的」交错。
    let system = SystemProxy::new(state.commands());
    let already_direct = system
        .readback()
        .map(|readback| egress_system::matches(&Expected::Direct, &readback))
        .unwrap_or(false);
    if changed || !already_direct {
        system
            .clear()
            .map_err(|error| settings_api_error(&request_id, &error))?;
    }
    state.egress().runtime().reset();
    drop(egress_lock);
    drop(config_lock);

    // 端口表随 proxies 一起被清掉：重建中继（释放 listener、写回端口表、撤链）。
    let mut relay_view: Option<RelayView> = None;
    if changed {
        match relay::prepare_from_disk(&state, false).await {
            Ok((rebuild, persisted)) => {
                relay_view = Some(state.relay().activate(rebuild, persisted).await);
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "DELETE 后重建中继失败（已清配置，可用 restart 修复）"
                );
            }
        }
    }

    let situation = load_situation(&state, &request_id)?;
    let view = state_view(&state, &situation, relay_view);
    let body = EgressWriteResult {
        revision: situation.revision.clone(),
        previous_revision: (egress.exists || config_snapshot.exists).then_some(current_revision),
        config_revision: situation.config_snapshot.revision.clone(),
        dry_run: false,
        changed,
        backup_id,
        diff: None,
        diff_truncated: false,
        state: view,
    };
    Ok(api::with_egress_revision(
        json_response(StatusCode::OK, body),
        &situation.revision,
    ))
}

/// POST /api/v1/egress/probe（ops 作用域，不支持 Idempotency-Key）。
pub async fn probe_egress(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request_id = request_id.as_str().to_owned();
    let value = if body.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        params::require_json(&headers, &request_id)?;
        params::parse_json(&body, &request_id)?
    };
    let Value::Object(map) = &value else {
        return Err(params::field_error(
            &request_id,
            "$",
            "invalid_type",
            "请求体必须是 JSON 对象",
        ));
    };
    params::reject_unknown_keys(
        map,
        |key| matches!(key, "package" | "url" | "via"),
        "",
        &request_id,
    )?;
    // 契约 §3.5：`via` 缺省 = "upstream"（逐字节兼容 v1.10.0），"relay" = 经设备内中继。
    let via_relay = match map.get("via") {
        None | Some(Value::Null) => false,
        Some(Value::String(via)) => match via.trim() {
            "upstream" => false,
            "relay" => true,
            other => {
                return Err(params::field_error(
                    &request_id,
                    "via",
                    "invalid_value",
                    format!("via 只接受 upstream 或 relay，收到 {other:?}"),
                ));
            }
        },
        Some(_) => {
            return Err(params::field_error(
                &request_id,
                "via",
                "invalid_type",
                "via 必须是字符串或 null",
            ));
        }
    };
    let package = match map.get("package") {
        None | Some(Value::Null) => None,
        Some(Value::String(package)) => {
            let package = package.trim();
            (!package.is_empty()).then_some(package)
        }
        Some(_) => {
            return Err(params::field_error(
                &request_id,
                "package",
                "invalid_type",
                "package 必须是字符串或 null",
            ));
        }
    };
    if let Some(package) = package {
        crate::api::templates::ensure_package(package, "package", &request_id)?;
    }
    let url_override = match map.get("url") {
        None | Some(Value::Null) => None,
        Some(Value::String(url)) => Some(url.as_str()),
        Some(_) => {
            return Err(params::field_error(
                &request_id,
                "url",
                "invalid_type",
                "url 必须是字符串或 null",
            ));
        }
    };

    let situation = load_situation(&state, &request_id)?;
    let probe_settings = egress_request::probe_settings(&situation.egress.model, url_override);
    let url = probe::parse_url(&probe_settings.url).map_err(|error| {
        params::field_error(&request_id, "url", "invalid_probe_url", error.message())
    })?;
    let (channel, scope) = if via_relay {
        resolve_relay_channel(
            &state,
            &situation,
            package,
            &url.raw,
            probe_settings.timeout_seconds * 1000,
            &request_id,
        )?
    } else {
        resolve_channel(
            &situation.config,
            &situation.egress.model,
            package,
            &request_id,
        )?
    };
    let timeout_ms = probe_settings.timeout_seconds * 1000;

    match probe::probe(&url, &channel, probe_settings.timeout_seconds).await {
        Ok(success) => {
            state.egress().runtime().record_probe(LastProbe {
                ok: true,
                exit_ip: Some(success.exit_ip.to_string()),
                latency_ms: success.latency_ms,
                checked_at: success.checked_at.clone(),
                scope,
                via: channel.via(),
                proxy: channel.name().map(str::to_owned),
                upstream_status: Some(success.upstream_status),
                error_code: None,
            });
            if state.egress().runtime().snapshot().degraded {
                // 恢复：先重写期望值并**读回校验**，只有真的回到期望态才 recover（t11 F3）
                let expected = expected_for(&situation.config, &situation.egress.model);
                let recovery = SystemProxy::new(state.commands()).apply(&expected, None);
                if recovery
                    .as_ref()
                    .map(|outcome| outcome.matches)
                    .unwrap_or(false)
                {
                    state.egress().runtime().recover();
                    tracing::info!("egress recovered（探测成功且读回一致，已恢复期望值）");
                } else {
                    tracing::warn!(
                        "egress recovery 未生效：探测成功但读回仍与期望不一致，保持 degraded"
                    );
                }
            }
            let mut warnings = Vec::new();
            if scope == "app" {
                warnings.push(Warning::new(
                    "egress_probe_approximate",
                    "应用级探测在服务进程内完成，不代表目标应用进程内的真实链路",
                    Some(json!({ "package": package })),
                ));
            }
            // 带凭据的生效代理：探测能吃下这份凭据，但应用进程里的注入拿不到它（t20），
            // 所以成功响应也要提醒「凭据不生效」，避免把 able-to-probe 当成配置正确。
            if let Channel::Proxy {
                name,
                username,
                password,
                ..
            } = &channel
                && let Some(field) = credential_field(username, password)
            {
                warnings.push(auth_warning(scope, name, field));
            }
            let body = ProbeResponse {
                ok: true,
                exit_ip: success.exit_ip.to_string(),
                latency_ms: success.latency_ms,
                checked_at: success.checked_at,
                scope,
                via: channel.via(),
                proxy: channel.name().map(str::to_owned),
                relay_port: channel.relay_port(),
                url: url.raw.clone(),
                upstream_status: success.upstream_status,
                approximate: scope == "app",
                warnings,
            };
            Ok(api::with_egress_revision(
                json_response(StatusCode::OK, body),
                &situation.revision,
            ))
        }
        Err(failure) => {
            state.egress().runtime().record_probe(LastProbe {
                ok: false,
                exit_ip: None,
                latency_ms: 0,
                checked_at: crate::util::now_rfc3339(),
                scope,
                via: channel.via(),
                proxy: channel.name().map(str::to_owned),
                upstream_status: failure.upstream_status(),
                error_code: Some("egress.probe_failed"),
            });
            // 只有设备级链路（channel 来自 [egress].default）才允许 fail-open 切直连；
            // 应用级探测失败只记 lastProbe + 在 details.warnings 里说明，不动设备级 settings（t11 F2）。
            let mut notes: Vec<Value> = Vec::new();
            // 契约 §3.5 第 5 条（安全关键）：只有 via="upstream" 的失败才允许触发 fail-open。
            // via="relay" 的失败一律不动 settings——否则「中继死 → 探测红 → 清系统代理」
            // 会把 fail-closed 变成 IP 泄漏。
            if failure.triggers_fail_open() && !via_relay {
                if scope == "device" {
                    if situation.egress.model.probe.fail_open {
                        let cleared = egress_state::degrade_to_direct(&state, "probe_failed");
                        notes.push(json!({
                            "code": "egress_degraded_to_direct",
                            "message": if cleared {
                                "已按 fail-open 切为直连（配置保持不变）"
                            } else {
                                "fail-open 判定为降级，但清直连未生效（settings 不可用或写入未落地）"
                            },
                            "details": { "reason": failure.reason(), "scope": scope, "cleared": cleared },
                        }));
                    } else {
                        state.egress().runtime().degrade("probe_failed", false);
                        tracing::warn!(
                            reason = failure.reason(),
                            cleared = false,
                            "egress 探测失败（fail-closed，未改 settings）"
                        );
                        notes.push(json!({
                            "code": "egress_degraded_to_direct",
                            "message": "探测失败但 fail-closed 生效：未改系统代理",
                            "details": { "reason": failure.reason(), "scope": scope, "cleared": false },
                        }));
                    }
                } else {
                    tracing::warn!(
                        reason = failure.reason(),
                        scope = "app",
                        "egress 应用级探测失败（不影响设备级系统代理）"
                    );
                    notes.push(json!({
                        "code": "egress_probe_scope_app",
                        "message": "应用级探测失败不会触发 fail-open（设备级系统代理保持不变）",
                        "details": { "scope": "app", "reason": failure.reason(), "proxy": channel.name() },
                    }));
                }
            }
            let error = ApiError::egress_probe_failed(
                &request_id,
                &url.raw,
                failure.reason(),
                timeout_ms,
                failure.upstream_status(),
            )
            .with_details(json!({
                "url": url.raw,
                "reason": failure.reason(),
                "timeoutMs": timeout_ms,
                "upstreamStatus": failure.upstream_status(),
                "scope": scope,
                "via": channel.via(),
                "relayPort": channel.relay_port(),
                "warnings": notes,
            }));
            Err(error)
        }
    }
}

/// POST /api/v1/egress/relay/restart（ops 作用域；无 If-Match、不支持 Idempotency-Key）。
///
/// 契约 §6.3：**强制**重建（memo R14：不被幂等短路吃掉，手工清掉规则后 restart 必须能修好），
/// 端口与 uid 集合未变的 listener 不关闭、不中断连接；并重放设备级 `settings`。
pub async fn restart_relay(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request_id = request_id.as_str().to_owned();
    if !body.is_empty() {
        params::require_json(&headers, &request_id)?;
        let value = params::parse_json(&body, &request_id)?;
        let Value::Object(map) = &value else {
            return Err(params::field_error(
                &request_id,
                "$",
                "invalid_type",
                "请求体必须是 JSON 对象",
            ));
        };
        if let Some(key) = map.keys().next() {
            return Err(params::field_error(
                &request_id,
                key,
                "unknown_field",
                format!("未知字段 {key}（本端点不接受任何字段）"),
            ));
        }
    }

    let rebuild = match relay::prepare_from_disk(&state, true).await {
        Ok((rebuild, persisted)) => (rebuild, persisted),
        Err(error) => {
            return Err(ApiError::internal_error(
                &request_id,
                "egress.toml 无法解析，无法重建中继",
            )
            .with_details(json!({ "op": "egress_parse", "message": error })));
        }
    };
    let (rebuild, persisted) = rebuild;
    let relay_view = state.relay().activate(rebuild, persisted).await;

    // 重放设备级 settings（不清配置）；设备级走中继时 settings 不可用 → 503。
    let situation = load_situation(&state, &request_id)?;
    let relay_status = state.relay().device_status(&situation.egress.model);
    let expected = expected_for_relay(
        &situation.config,
        &situation.egress.model,
        Some(&relay_status),
    );
    let system = SystemProxy::new(state.commands());
    if let Err(error) = system.apply(&expected, None) {
        return Err(settings_api_error(&request_id, &error));
    }

    let listening = relay_view
        .listeners
        .iter()
        .filter(|listener| listener.listening)
        .count();
    tracing::info!(listeners = listening, "relay restarted");

    let mut warnings = situation.warnings.clone();
    warnings.extend(relay::dto::relay_warnings(&relay_view));
    if let Some(warning) = device_socks5_warning(&situation.config, &situation.egress.model) {
        warnings.push(warning);
    }
    let restarted_at = relay_view.at.clone();
    let body = RelayRestartResponse {
        restarted_at,
        relay: relay_view,
        warnings,
    };
    Ok(api::with_egress_revision(
        json_response(StatusCode::OK, body),
        &situation.revision,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// 把 tracing 输出收进内存，供同步断言（不受并行测试影响）。
    #[derive(Clone, Default)]
    struct CaptureLog(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureLog {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureLog {
        type Writer = CaptureLog;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_lines() -> (CaptureLog, Arc<Mutex<Vec<u8>>>) {
        let capture = CaptureLog::default();
        let buffer = Arc::clone(&capture.0);
        (capture, buffer)
    }

    #[test]
    fn settings_failure_maps_to_503_and_one_warn_line() {
        let (capture, buffer) = capture_lines();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(capture)
            .finish();
        let error = tracing::subscriber::with_default(subscriber, || {
            settings_api_error(
                "req_test",
                &crate::egress::SettingsError::failed(
                    Some(1),
                    "avc: denied { write } for settings",
                ),
            )
        });
        assert_eq!(error.code(), crate::error::ErrorCode::UpstreamUnavailable);

        let text = String::from_utf8(
            buffer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        )
        .expect("utf-8 log");
        let lines: Vec<&str> = text.lines().filter(|line| line.contains("WARN")).collect();
        assert_eq!(lines.len(), 1, "settings 失败必须且只记一行 warn: {text}");
        assert!(lines[0].contains("settings"), "{text}");
        assert!(
            lines[0].contains("stderr") && lines[0].contains("avc: denied"),
            "诊断行要带截断 stderr: {text}"
        );
    }

    #[test]
    fn unavailable_settings_logs_without_stderr() {
        let (capture, buffer) = capture_lines();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(capture)
            .finish();
        let error = tracing::subscriber::with_default(subscriber, || {
            settings_api_error("req_test", &crate::egress::SettingsError::Unavailable)
        });
        assert_eq!(error.code(), crate::error::ErrorCode::UpstreamUnavailable);
        let text = String::from_utf8(
            buffer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        )
        .expect("utf-8 log");
        assert!(text.contains("不可用"), "{text}");
    }

    #[test]
    fn diff_redaction_replaces_only_credential_values() {
        let diff = "--- a/egress.toml\n+++ b/egress.toml\n@@ -1,3 +1,3 @@\n host = \"203.0.113.10\"\n-username = \"alice\"\n+username = \"bob\"\n-password = \"old\"\n+password = \"new\"\n";
        let redacted = redact_diff_credentials(diff);
        assert!(!redacted.contains("alice"), "{redacted}");
        assert!(!redacted.contains("bob"), "{redacted}");
        assert!(!redacted.contains("\"old\""), "{redacted}");
        assert!(!redacted.contains("\"new\""), "{redacted}");
        assert_eq!(redacted.matches("<redacted>").count(), 4, "{redacted}");
        // 非凭据行原样保留（diff 头部与 host 行）
        assert!(redacted.contains("--- a/egress.toml"), "{redacted}");
        assert!(redacted.contains(" host = \"203.0.113.10\""), "{redacted}");
    }
}

#[cfg(test)]
mod combined_diff_tests {
    use super::*;

    /// B-1 回归：egress 段脱敏的同时，config.toml 的 diff 行为不变（既有断言见 config_api.rs）。
    #[test]
    fn combined_diff_redacts_egress_but_keeps_config_changes() {
        let old_config = "debug = false\n";
        let new_config = "debug = false\n\n[egress]\ndefault = \"us_la_01\"\n";
        let old_egress = "version = 1\n\n[proxies.us_la_01]\ntype = \"http\"\nhost = \"203.0.113.10\"\nport = 1080\npassword = \"oldpass\"\n";
        let new_egress = "version = 1\n\n[proxies.us_la_01]\ntype = \"http\"\nhost = \"203.0.113.10\"\nport = 1080\npassword = \"newpass\"\n";
        let (diff, truncated) = combined_diff(old_config, new_config, old_egress, new_egress);
        assert!(!truncated);
        assert!(diff.contains("--- a/config.toml"), "{diff}");
        assert!(diff.contains("--- a/egress.toml"), "{diff}");
        assert!(diff.contains("+default = \"us_la_01\""), "{diff}");
        assert!(!diff.contains("oldpass"), "{diff}");
        assert!(!diff.contains("newpass"), "{diff}");
        assert!(diff.contains("password = \"<redacted>\""), "{diff}");
    }
}
