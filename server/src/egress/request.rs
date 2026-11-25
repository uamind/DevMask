// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! PUT /egress 的请求体、字段校验与「省略 = 不改」的合并规则（契约 §1.4 / §2.6）。
//!
//! 三态语义（冻结）：字段**省略** = 不改；字段为 **null** = 清空/恢复默认；
//! 给了**数组** = 整体替换该数组。为了实现这个语义，可空字段一律用 Option<Option<T>>：
//! None = 省略，Some(None) = null，Some(Some(v)) = 显式值。
//!
//! 校验只产出契约 §2.6 的 ValidationError 码，**message 里不带任何字段值**
//! （密码长度超限时只说「超过 256 字符」）。

use std::collections::HashSet;
use std::net::IpAddr;

use device_faker_core::config::{DIRECT_MARKER, is_valid_egress_app_value};
use device_faker_core::validate::{is_valid_package_key, validate_egress_proxy_name};
use serde::{Deserialize, Deserializer};

use crate::dto::ValidationError;
use crate::egress::file::{EgressFile, ProbeSettings, ProxyDefinition, ProxyType};
use crate::egress::probe;
use crate::egress::{
    DEFAULT_PROBE_URL, MAX_EXCLUSION_LEN, MAX_EXCLUSIONS, MAX_HOST_LEN, MAX_PASSWORD_LEN,
    MAX_PROBE_TIMEOUT_SECONDS, MAX_PROXIES, MAX_USERNAME_LEN,
};

/// 区分「字段省略」与「字段为 null」。
///
/// serde 把 `Option<Option<T>>` 的 null 也解析成 None，表达不了三态；配合
/// `#[serde(default, deserialize_with = "present_option")]`：字段缺失时不调用本函数
/// （得到 None），字段出现时把值包一层（null → Some(None)，值 → Some(Some(v))）。
fn present_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// PUT /api/v1/egress 请求体（字段全部可选）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressUpdateRequest {
    /// 设备级默认代理名；null / 空串 = 清除。
    #[serde(default, deserialize_with = "present_option")]
    pub default: Option<Option<String>>,
    /// fail-open 开关。
    #[serde(default)]
    pub fail_open: Option<bool>,
    /// 探测 URL；null = 恢复默认值。
    #[serde(default, deserialize_with = "present_option")]
    pub probe_url: Option<Option<String>>,
    /// 探测超时（1..=30）。
    #[serde(default)]
    pub probe_timeout_seconds: Option<i64>,
    /// 排除列表；null = 清除该键。
    #[serde(default, deserialize_with = "present_option")]
    pub exclusion_list: Option<Option<Vec<String>>>,
    /// 代理定义；整体替换（[] 清空全部）。
    #[serde(default)]
    pub proxies: Option<Vec<ProxyInput>>,
    /// 应用级覆盖；null = 清除所有 [[apps]].egress。
    #[serde(default, deserialize_with = "present_option")]
    pub apps: Option<Option<Vec<AppEgressInput>>>,
}

/// 一条代理定义的入参。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyInput {
    /// 代理名（必填）。
    #[serde(default)]
    pub name: Option<String>,
    /// 协议：http / socks5（必填）。
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// 主机（必填）。
    #[serde(default)]
    pub host: Option<String>,
    /// 端口（必填，1..=65535）。
    #[serde(default)]
    pub port: Option<i64>,
    /// 用户名；省略 = 保留已存值，null / 空串 = 清除。
    #[serde(default, deserialize_with = "present_option")]
    pub username: Option<Option<String>>,
    /// 密码；省略 = 保留已存值，null / 空串 = 清除（永不出现在响应里）。
    #[serde(default, deserialize_with = "present_option")]
    pub password: Option<Option<String>>,
}

/// 一条应用级覆盖的入参。
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppEgressInput {
    /// 包名（可带 @userId）。
    #[serde(default)]
    pub package: Option<String>,
    /// 代理名或 __DIRECT__；null / 空串 = 清除该应用的覆盖。
    #[serde(default, deserialize_with = "present_option")]
    pub egress: Option<Option<String>>,
}

/// 应用级覆盖的合并方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppUpdates {
    /// 不改（字段省略）。
    None,
    /// 清除所有 [[apps]].egress（字段为 null）。
    ClearAll,
    /// 逐条设置/清除。
    Entries(Vec<(String, Option<String>)>),
}

/// 校验请求体；返回空数组表示通过。
pub fn validate(request: &EgressUpdateRequest) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    validate_default(request, &mut errors);
    validate_probe(request, &mut errors);
    validate_exclusions(request, &mut errors);
    validate_proxies(request, &mut errors);
    validate_apps(request, &mut errors);
    errors
}

fn validate_default(request: &EgressUpdateRequest, errors: &mut Vec<ValidationError>) {
    let Some(default) = &request.default else {
        return;
    };
    match default.as_deref() {
        None => {}
        Some(name) if name.trim().is_empty() => {}
        Some(DIRECT_MARKER) => errors.push(ValidationError::new(
            "default",
            "invalid_egress_default",
            format!("设备级 default 不能是 {DIRECT_MARKER}；直连请删除该键"),
        )),
        Some(name) if name != name.trim() => errors.push(ValidationError::new(
            "default",
            "invalid_egress_name",
            "代理名不允许首尾空白",
        )),
        Some(name) => {
            if let Some(message) = validate_egress_proxy_name(name) {
                errors.push(ValidationError::new(
                    "default",
                    "invalid_egress_name",
                    message,
                ));
            }
        }
    }
}

fn validate_probe(request: &EgressUpdateRequest, errors: &mut Vec<ValidationError>) {
    if let Some(timeout) = request.probe_timeout_seconds
        && !(1..=MAX_PROBE_TIMEOUT_SECONDS as i64).contains(&timeout)
    {
        errors.push(ValidationError::new(
            "probeTimeoutSeconds",
            "out_of_range",
            format!("probeTimeoutSeconds 必须在 1..={MAX_PROBE_TIMEOUT_SECONDS}"),
        ));
    }
    if let Some(url) = &request.probe_url
        && let Some(url) = url.as_deref().map(str::trim).filter(|url| !url.is_empty())
        && let Err(error) = probe::parse_url(url)
    {
        errors.push(ValidationError::new(
            "probeUrl",
            "invalid_probe_url",
            error.message(),
        ));
    }
}

fn validate_exclusions(request: &EgressUpdateRequest, errors: &mut Vec<ValidationError>) {
    let Some(Some(list)) = &request.exclusion_list else {
        return;
    };
    if list.len() > MAX_EXCLUSIONS {
        errors.push(ValidationError::new(
            "exclusionList",
            "out_of_range",
            format!("exclusionList 最多 {MAX_EXCLUSIONS} 项"),
        ));
    }
    for entry in list {
        if entry.chars().count() > MAX_EXCLUSION_LEN {
            errors.push(ValidationError::new(
                "exclusionList",
                "invalid_value",
                format!("exclusionList 每项不超过 {MAX_EXCLUSION_LEN} 字符"),
            ));
            break;
        }
        if !is_valid_exclusion(entry) {
            errors.push(ValidationError::new(
                "exclusionList",
                "invalid_value",
                "排除项必须是 CIDR、IP 字面量或主机名",
            ));
            break;
        }
    }
}

fn validate_proxies(request: &EgressUpdateRequest, errors: &mut Vec<ValidationError>) {
    let Some(proxies) = &request.proxies else {
        return;
    };
    if proxies.len() > MAX_PROXIES {
        errors.push(ValidationError::new(
            "proxies",
            "out_of_range",
            format!("proxies 最多 {MAX_PROXIES} 条"),
        ));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for (index, proxy) in proxies.iter().enumerate() {
        let path = format!("proxies[{index}]");
        validate_proxy(proxy, &path, errors);
        if let Some(name) = proxy
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            && !seen.insert(name)
        {
            errors.push(ValidationError::new(
                format!("{path}.name"),
                "invalid_value",
                "同一请求内代理名必须唯一",
            ));
        }
    }
}

fn validate_proxy(proxy: &ProxyInput, path: &str, errors: &mut Vec<ValidationError>) {
    match proxy.name.as_deref().map(str::trim) {
        None | Some("") => errors.push(ValidationError::new(
            format!("{path}.name"),
            "missing_required",
            "缺少代理名 name",
        )),
        Some(name) => {
            if let Some(message) = validate_egress_proxy_name(name) {
                errors.push(ValidationError::new(
                    format!("{path}.name"),
                    "invalid_egress_name",
                    message,
                ));
            }
        }
    }
    match proxy.kind.as_deref() {
        None => errors.push(ValidationError::new(
            format!("{path}.type"),
            "missing_required",
            "缺少协议 type（http 或 socks5）",
        )),
        Some(kind) if ProxyType::parse(kind).is_none() => errors.push(ValidationError::new(
            format!("{path}.type"),
            "invalid_egress_proxy",
            "type 只支持 http 或 socks5",
        )),
        Some(_) => {}
    }
    match proxy.host.as_deref().map(str::trim) {
        None | Some("") => errors.push(ValidationError::new(
            format!("{path}.host"),
            "missing_required",
            "缺少主机 host",
        )),
        Some(host) => {
            if let Err(message) = validate_host(host) {
                errors.push(ValidationError::new(
                    format!("{path}.host"),
                    "invalid_egress_proxy",
                    message,
                ));
            }
        }
    }
    match proxy.port {
        None => errors.push(ValidationError::new(
            format!("{path}.port"),
            "missing_required",
            "缺少端口 port",
        )),
        Some(port) if !(1..=65535).contains(&port) => errors.push(ValidationError::new(
            format!("{path}.port"),
            "invalid_egress_proxy",
            "port 必须在 1..=65535",
        )),
        Some(_) => {}
    }
    if let Some(Some(username)) = &proxy.username
        && username.chars().count() > MAX_USERNAME_LEN
    {
        errors.push(ValidationError::new(
            format!("{path}.username"),
            "invalid_egress_proxy",
            format!("username 超过 {MAX_USERNAME_LEN} 字符"),
        ));
    }
    if let Some(Some(password)) = &proxy.password
        && password.chars().count() > MAX_PASSWORD_LEN
    {
        errors.push(ValidationError::new(
            format!("{path}.password"),
            "invalid_egress_proxy",
            format!("password 超过 {MAX_PASSWORD_LEN} 字符"),
        ));
    }
}

fn validate_apps(request: &EgressUpdateRequest, errors: &mut Vec<ValidationError>) {
    let Some(Some(apps)) = &request.apps else {
        return;
    };
    let mut seen: HashSet<&str> = HashSet::new();
    for (index, app) in apps.iter().enumerate() {
        let path = format!("apps[{index}]");
        let package = app.package.as_deref().map(str::trim);
        match package {
            None | Some("") => errors.push(ValidationError::new(
                format!("{path}.package"),
                "missing_required",
                "缺少包名 package",
            )),
            Some(package) => {
                if !is_valid_package_key(package) {
                    errors.push(ValidationError::new(
                        format!("{path}.package"),
                        "invalid_package",
                        format!("不是合法包名: {package:?}"),
                    ));
                }
                if !seen.insert(package) {
                    errors.push(ValidationError::new(
                        format!("{path}.package"),
                        "invalid_value",
                        "同一请求内 package 必须唯一",
                    ));
                }
            }
        }
        match &app.egress {
            None => errors.push(ValidationError::new(
                format!("{path}.egress"),
                "missing_required",
                "缺少 egress（要清除请显式给 null）",
            )),
            Some(None) => {}
            Some(Some(value)) if value.trim().is_empty() => {}
            Some(Some(value)) if !is_valid_egress_app_value(value.trim()) => {
                errors.push(ValidationError::new(
                    format!("{path}.egress"),
                    "invalid_egress_name",
                    format!(
                        "egress 只能是代理名（{}）或 {DIRECT_MARKER}",
                        device_faker_core::config::EGRESS_PROXY_NAME_PATTERN
                    ),
                ));
            }
            Some(Some(_)) => {}
        }
    }
}

fn validate_host(host: &str) -> Result<(), &'static str> {
    if host.chars().count() > MAX_HOST_LEN {
        return Err("host 超过 253 字符");
    }
    if host.contains("://") || host.contains('/') || host.contains('@') {
        return Err("host 不得包含 scheme、路径或用户信息");
    }
    if host.parse::<IpAddr>().is_ok() {
        // IPv6 字面量在定义层允许（应用级可用）；设备级会在 handler 里再拒一次。
        return Ok(());
    }
    if host.contains(':') {
        return Err("host 不得包含端口（端口请用 port 字段）");
    }
    if host.starts_with('.') || host.ends_with('.') || host.contains("..") {
        return Err("host 不是合法主机名");
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return Err("host 只允许字母、数字、点、减号（或 IP 字面量）");
    }
    Ok(())
}

fn is_valid_exclusion(entry: &str) -> bool {
    let entry = entry.trim();
    if entry.is_empty() {
        return false;
    }
    if entry.parse::<IpAddr>().is_ok() {
        return true;
    }
    if let Some((address, prefix)) = entry.split_once('/') {
        let Ok(prefix) = prefix.parse::<u8>() else {
            return false;
        };
        return match address.parse::<IpAddr>() {
            Ok(IpAddr::V4(_)) => prefix <= 32,
            Ok(IpAddr::V6(_)) => prefix <= 128,
            Err(_) => false,
        };
    }
    entry
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '*'))
}

/// 合并代理定义与 probe 策略（省略 = 不改、null = 清空/恢复默认）。
/// 设备级中继只支持 http：**有效 default**（合并后的模型 + 新配置里的 default 名）
/// 指向非 http 定义时给出 422 校验错误。
///
/// 注意不能只看 request.default：把已存在定义的 type 改成 socks5 时请求体里根本没有
/// default，但设备级已经指向它了，同样必须拒绝（t30 L1）。
pub fn validate_default_scope(
    model: &EgressFile,
    default_name: Option<&str>,
) -> Vec<crate::dto::ValidationError> {
    let Some(name) = default_name else {
        return Vec::new();
    };
    let Some(proxy) = model.proxy(name) else {
        return Vec::new();
    };
    if proxy.kind == ProxyType::Http {
        return Vec::new();
    }
    vec![crate::dto::ValidationError::new(
        "default",
        "invalid_egress_default",
        "设备级中继只支持 http 上游；socks5 请绑定到应用使用",
    )]
}

pub fn merge_egress(current: &EgressFile, request: &EgressUpdateRequest) -> EgressFile {
    let mut model = current.clone();
    if let Some(proxies) = &request.proxies {
        model.proxies = proxies
            .iter()
            .map(|input| {
                let name = input
                    .name
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_owned();
                let existing = current.proxy(&name);
                ProxyDefinition {
                    name,
                    kind: input
                        .kind
                        .as_deref()
                        .and_then(ProxyType::parse)
                        .unwrap_or(ProxyType::Http),
                    host: input
                        .host
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_owned(),
                    port: input
                        .port
                        .and_then(|port| u16::try_from(port).ok())
                        .unwrap_or(0),
                    username: merge_credential(
                        &input.username,
                        existing.and_then(|proxy| proxy.username.clone()),
                    ),
                    password: merge_credential(
                        &input.password,
                        existing.and_then(|proxy| proxy.password.clone()),
                    ),
                    // 端口表是 **server 拥有**字段（契约 §1.8 / D3）：请求体从不携带，
                    // 这里按名字从当前模型继承，保证「PUT 不改变已分配端口」且渲染幂等。
                    local_port: existing.and_then(|proxy| proxy.local_port),
                }
            })
            .collect();
    }
    if let Some(fail_open) = request.fail_open {
        model.probe.fail_open = fail_open;
    }
    if let Some(url) = &request.probe_url {
        model.probe.url = match url.as_deref().map(str::trim) {
            Some(url) if !url.is_empty() => url.to_owned(),
            _ => DEFAULT_PROBE_URL.to_owned(),
        };
    }
    if let Some(timeout) = request.probe_timeout_seconds
        && (1..=MAX_PROBE_TIMEOUT_SECONDS as i64).contains(&timeout)
    {
        model.probe.timeout_seconds = timeout as u64;
    }
    model
}

fn merge_credential(input: &Option<Option<String>>, existing: Option<String>) -> Option<String> {
    match input {
        // 省略：保留该条已存凭据（不是清空）
        None => existing,
        // null 或空串：清除
        Some(None) => None,
        Some(Some(value)) if value.is_empty() => None,
        Some(Some(value)) => Some(value.clone()),
    }
}

/// 设备级 default 的写入值：None = 不改，Some(None) = 清除。
pub fn default_update(request: &EgressUpdateRequest) -> Option<Option<String>> {
    request.default.as_ref().map(|value| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

/// 应用级覆盖的合并方式。
pub fn app_updates(request: &EgressUpdateRequest) -> AppUpdates {
    match &request.apps {
        None => AppUpdates::None,
        Some(None) => AppUpdates::ClearAll,
        Some(Some(apps)) => AppUpdates::Entries(
            apps.iter()
                .map(|app| {
                    let package = app
                        .package
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_owned();
                    let value = match &app.egress {
                        Some(Some(value)) if !value.trim().is_empty() => {
                            Some(value.trim().to_owned())
                        }
                        _ => None,
                    };
                    (package, value)
                })
                .collect(),
        ),
    }
}

/// 探测 URL 的最终值（请求体覆盖 > [probe].url）。
pub fn probe_settings(model: &EgressFile, request_url: Option<&str>) -> ProbeSettings {
    let mut probe = model.probe.clone();
    if let Some(url) = request_url.map(str::trim).filter(|url| !url.is_empty()) {
        probe.url = url.to_owned();
    }
    probe
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_request(value: serde_json::Value) -> EgressUpdateRequest {
        serde_json::from_value(value).expect("request")
    }

    #[test]
    fn omitted_null_and_value_are_distinguished() {
        let request = parse_request(json!({ "default": null }));
        assert_eq!(request.default, Some(None));
        let request = parse_request(json!({ "default": "us_la_01" }));
        assert_eq!(request.default, Some(Some("us_la_01".to_owned())));
        let request = parse_request(json!({}));
        assert_eq!(request.default, None);
        let request = parse_request(json!({ "apps": null }));
        assert_eq!(request.apps, Some(None));
    }

    #[test]
    fn unknown_fields_are_rejected_by_serde() {
        let error = serde_json::from_value::<EgressUpdateRequest>(json!({ "proxyHost": "x" }));
        assert!(
            error.is_err(),
            "未知字段必须拒绝（handler 另行映射 422 unknown_field）"
        );
    }

    #[test]
    fn validation_reports_contract_codes() {
        let request = parse_request(json!({
            "default": "__DIRECT__",
            "probeUrl": "https://example.com/",
            "probeTimeoutSeconds": 60,
            "exclusionList": ["not a cidr"],
            "proxies": [
                { "name": "bad name", "type": "ftp", "host": "http://x/y", "port": 0 },
                { "name": "ok", "type": "http", "host": "203.0.113.10", "port": 1080 }
            ],
            "apps": [
                { "package": "com.example.a" },
                { "package": "bad package", "egress": "bad name" }
            ]
        }));
        let errors = validate(&request);
        let codes: Vec<(&str, &str)> = errors
            .iter()
            .map(|error| (error.path.as_str(), error.code))
            .collect();
        assert!(
            codes.contains(&("default", "invalid_egress_default")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("probeUrl", "invalid_probe_url")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("probeTimeoutSeconds", "out_of_range")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("exclusionList", "invalid_value")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("proxies[0].name", "invalid_egress_name")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("proxies[0].type", "invalid_egress_proxy")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("proxies[0].host", "invalid_egress_proxy")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("proxies[0].port", "invalid_egress_proxy")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("apps[0].egress", "missing_required")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("apps[1].package", "invalid_package")),
            "{codes:?}"
        );
        assert!(
            codes.contains(&("apps[1].egress", "invalid_egress_name")),
            "{codes:?}"
        );
        for error in &errors {
            assert!(!error.message.contains("s3cret"), "错误信息不得回显值");
        }
    }

    #[test]
    fn merge_keeps_omitted_proxy_credentials() {
        let current = EgressFile {
            version: 1,
            proxies: vec![ProxyDefinition {
                name: "us_la_01".to_owned(),
                kind: ProxyType::Http,
                host: "203.0.113.10".to_owned(),
                port: 1080,
                username: Some("alice".to_owned()),
                password: Some("s3cret".to_owned()),
                local_port: Some(17831),
            }],
            probe: ProbeSettings::default(),
            relay: crate::egress::file::RelaySettings::default(),
            relay_degraded: Vec::new(),
            unknown_fields: Vec::new(),
        };
        // 省略 username/password → 保留
        let request = parse_request(json!({
            "proxies": [{ "name": "us_la_01", "type": "http", "host": "203.0.113.11", "port": 8080 }]
        }));
        assert!(validate(&request).is_empty());
        let merged = merge_egress(&current, &request);
        assert_eq!(merged.proxies[0].host, "203.0.113.11");
        assert_eq!(merged.proxies[0].username.as_deref(), Some("alice"));
        assert_eq!(merged.proxies[0].password.as_deref(), Some("s3cret"));
        assert_eq!(
            merged.proxies[0].local_port,
            Some(17831),
            "端口表由 server 拥有：PUT 必须原样保留"
        );

        // null → 清除
        let request = parse_request(json!({
            "proxies": [{ "name": "us_la_01", "type": "http", "host": "h", "port": 1, "password": null }]
        }));
        let merged = merge_egress(&current, &request);
        assert_eq!(merged.proxies[0].username.as_deref(), Some("alice"));
        assert_eq!(merged.proxies[0].password, None);
    }

    #[test]
    fn merge_handles_probe_and_apps_three_states() {
        let current = EgressFile::empty();
        let request =
            parse_request(json!({ "failOpen": false, "probeUrl": "http://example.com/ip" }));
        let merged = merge_egress(&current, &request);
        assert!(!merged.probe.fail_open);
        assert_eq!(merged.probe.url, "http://example.com/ip");

        let request = parse_request(json!({ "probeUrl": null }));
        let merged = merge_egress(&current, &request);
        assert_eq!(merged.probe.url, DEFAULT_PROBE_URL);

        let request = parse_request(
            json!({ "apps": [{ "package": "com.example.a", "egress": "__DIRECT__" }, { "package": "com.example.b", "egress": null }] }),
        );
        assert_eq!(
            app_updates(&request),
            AppUpdates::Entries(vec![
                ("com.example.a".to_owned(), Some("__DIRECT__".to_owned())),
                ("com.example.b".to_owned(), None),
            ])
        );
        let request = parse_request(json!({}));
        assert_eq!(app_updates(&request), AppUpdates::None);
        let request = parse_request(json!({ "apps": null }));
        assert_eq!(app_updates(&request), AppUpdates::ClearAll);
    }

    #[test]
    fn default_update_normalizes_empty_to_clear() {
        assert_eq!(default_update(&parse_request(json!({}))), None);
        assert_eq!(
            default_update(&parse_request(json!({ "default": null }))),
            Some(None)
        );
        assert_eq!(
            default_update(&parse_request(json!({ "default": "  " }))),
            Some(None)
        );
        assert_eq!(
            default_update(&parse_request(json!({ "default": "us_la_01" }))),
            Some(Some("us_la_01".to_owned()))
        );
    }
}
