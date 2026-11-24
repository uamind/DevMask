// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 设备端点（docs/API.md 5.21-5.23）。

use std::collections::HashMap;

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    response::Response,
};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::api::{ServerInfo, json as json_response};
use crate::app::AppState;
use crate::commands::CommandOutcome;
use crate::device::{self, DeviceSummary, RootInfo};
use crate::error::ApiError;
use crate::module;
use crate::params::{self, Params};
use crate::requests::RequestId;

/// getprop 白名单前缀（docs/API.md 5.22）。
pub const PROP_WHITELIST: [&str; 6] = [
    "ro.product.",
    "ro.build.",
    "ro.soc.",
    "ro.hardware",
    "ro.board.",
    "ro.vendor.",
];

/// GET /device 里的 device 块：在摘要上补 fingerprint。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetail {
    /// Build 摘要字段。
    #[serde(flatten)]
    pub summary: DeviceSummary,
    /// Build.FINGERPRINT（getprop 读不到时为 null）。
    pub fingerprint: Option<String>,
}

/// GET /device 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceResponse {
    /// 模块信息。
    pub module: module::ModuleInfo,
    /// 设备信息。
    pub device: DeviceDetail,
    /// 服务信息。
    pub server: ServerInfo,
    /// root 管理器。
    pub root: RootInfo,
}

/// GET /api/v1/device
pub async fn get_device(
    State(state): State<AppState>,
    Extension(_request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let summary = state.device().clone();
    let fingerprint = match state.commands().run("getprop", &["ro.build.fingerprint"]) {
        CommandOutcome::Ok(stdout) => {
            let value = stdout.trim();
            (!value.is_empty()).then(|| value.to_owned())
        }
        _ => None,
    };

    let body = DeviceResponse {
        module: module::read(&state.settings().module_prop_path),
        device: DeviceDetail {
            summary,
            fingerprint,
        },
        server: ServerInfo {
            version: state.version(),
            bind: state.bind().to_owned(),
            uptime_sec: state.uptime_sec(),
            started_at: state.started_at().to_owned(),
            pid: std::process::id(),
        },
        root: device::root_info(&state.settings().adb_dir),
    };
    Ok(json_response(StatusCode::OK, body))
}

/// GET /api/v1/device/props?keys=...
pub async fn get_props(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let Some(raw) = params.raw("keys") else {
        return Err(params::field_error(
            request_id.as_str(),
            "keys",
            "missing_required",
            "缺少 keys（逗号分隔，1..=50 个）",
        ));
    };
    let keys = params::comma_list(raw);
    if keys.is_empty() || keys.len() > 50 {
        return Err(params::field_error(
            request_id.as_str(),
            "keys",
            "out_of_range",
            "keys 必须是 1..=50 个",
        ));
    }
    for key in &keys {
        if key.len() > 128 || !PROP_WHITELIST.iter().any(|prefix| key.starts_with(prefix)) {
            return Err(params::field_error(
                request_id.as_str(),
                "keys",
                "invalid_value",
                format!(
                    "属性 {key:?} 不在白名单内（ro.product./ro.build./ro.soc./ro.hardware/ro.board./ro.vendor.）"
                ),
            ));
        }
    }

    let stdout = match state.commands().run("getprop", &[]) {
        CommandOutcome::Ok(stdout) => stdout,
        _ => {
            return Err(ApiError::upstream_unavailable(
                request_id.as_str(),
                "getprop",
            ));
        }
    };
    let all = parse_getprop(&stdout);

    let mut props = Map::new();
    let mut missing = Vec::new();
    for key in &keys {
        match all.get(key) {
            Some(value) => {
                props.insert(key.clone(), Value::String(value.clone()));
            }
            None => missing.push(key.clone()),
        }
    }

    Ok(json_response(
        StatusCode::OK,
        serde_json::json!({
            "props": props,
            "missing": missing,
            "source": "getprop",
        }),
    ))
}

/// GET /api/v1/device/users
pub async fn get_users(
    State(state): State<AppState>,
    Extension(_request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let index = state.packages().index();
    let current = index.current_user;
    let users: Vec<Value> = index
        .users
        .iter()
        .map(|user| {
            serde_json::json!({
                "id": user.id,
                "name": user.name,
                "running": user.running,
                "isCurrent": current == Some(user.id),
            })
        })
        .collect();
    Ok(json_response(
        StatusCode::OK,
        serde_json::json!({ "users": users }),
    ))
}

/// 解析 `getprop` 全量输出（`[key]: [value]`）。
pub fn parse_getprop(stdout: &str) -> HashMap<String, String> {
    let mut props = HashMap::new();
    for line in stdout.lines() {
        let Some((key, rest)) = line
            .trim()
            .strip_prefix('[')
            .and_then(|line| line.split_once(']'))
        else {
            continue;
        };
        let Some(value) = rest.trim().strip_prefix(':').map(str::trim) else {
            continue;
        };
        let value = value.trim_start().trim_start_matches('[');
        let value = value.strip_suffix(']').unwrap_or(value).trim();
        props.insert(key.to_owned(), value.to_owned());
    }
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_getprop_lines() {
        let stdout = "[ro.product.model]: [Pixel 9]\n[ro.build.fingerprint]: [google/tokay:17/x:user/release-keys]\n[ro.hardware]: [tensor]\n";
        let props = parse_getprop(stdout);
        assert_eq!(
            props.get("ro.product.model").map(String::as_str),
            Some("Pixel 9")
        );
        assert_eq!(
            props.get("ro.build.fingerprint").map(String::as_str),
            Some("google/tokay:17/x:user/release-keys")
        );
        assert_eq!(props.get("ro.hardware").map(String::as_str), Some("tensor"));
    }

    #[test]
    fn whitelist_covers_contract_prefixes() {
        for key in [
            "ro.product.model",
            "ro.build.fingerprint",
            "ro.soc.model",
            "ro.hardware",
            "ro.board.platform",
            "ro.vendor.custom",
        ] {
            assert!(
                PROP_WHITELIST.iter().any(|prefix| key.starts_with(prefix)),
                "{key}"
            );
        }
        assert!(
            !PROP_WHITELIST
                .iter()
                .any(|prefix| "persist.foo".starts_with(prefix))
        );
    }
}
