// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! GET /api/v1/packages（docs/API.md 5.20）。

use std::collections::HashMap;

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    response::Response,
};
use devmask_core::validate::is_valid_package_key;
use serde::Serialize;

use crate::api::json as json_response;
use crate::app::AppState;
use crate::dto::Warning;
use crate::error::ApiError;
use crate::packages::{MAX_DUMPSYS, base_package};
use crate::params::{self, Params};
use crate::requests::RequestId;

/// 单个包的信息（openapi PackageInfo）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInfo {
    /// 包名。
    pub package: String,
    /// 已安装的用户 id。
    pub user_ids: Vec<u32>,
    /// 是否已安装。
    pub installed: bool,
    /// 是否系统应用。
    pub is_system: bool,
    /// versionName（仅在 names 查询且数量较少时补齐）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
    /// versionCode。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_code: Option<u64>,
    /// 应用名（本增量恒为 null，P4 提供）。
    pub label: Option<String>,
}

/// GET /packages 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackagesResponse {
    /// 过滤后的总数。
    pub total: usize,
    /// 下一页游标。
    pub next_cursor: Option<String>,
    /// `pm` 或 `unavailable`。
    pub source: &'static str,
    /// 警告。
    pub warnings: Vec<Warning>,
    /// 当前页。
    pub items: Vec<PackageInfo>,
}

/// GET /api/v1/packages
pub async fn list(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let params = Params::new(query);
    let page = params.page(request_id.as_str())?;

    let names = match params.raw("names") {
        None => None,
        Some(raw) => {
            let names = params::comma_list(raw);
            if names.is_empty() || names.len() > 500 {
                return Err(params::field_error(
                    request_id.as_str(),
                    "names",
                    "out_of_range",
                    "names 必须是 1..=500 个包名",
                ));
            }
            for name in &names {
                if !is_valid_package_key(name) {
                    return Err(params::field_error(
                        request_id.as_str(),
                        "names",
                        "invalid_package",
                        format!("不是合法包名: {name:?}"),
                    ));
                }
            }
            Some(names)
        }
    };
    let user = params.usize_param("user", request_id.as_str())?;
    if let Some(user) = user
        && user > u32::MAX as usize
    {
        return Err(params::field_error(
            request_id.as_str(),
            "user",
            "out_of_range",
            "user 超出 Android 用户 id 范围",
        ));
    }

    let index = state.packages().index();
    if !index.available {
        let body = PackagesResponse {
            total: 0,
            next_cursor: None,
            source: "unavailable",
            warnings: vec![Warning::new(
                "pm_unavailable",
                "PackageManager 不可用，无法列出已安装包",
                Some(serde_json::json!({})),
            )],
            items: Vec::new(),
        };
        return Ok(json_response(StatusCode::OK, body));
    }

    let allow_dumpsys = names
        .as_ref()
        .is_some_and(|names| names.len() <= MAX_DUMPSYS);
    let mut items: Vec<PackageInfo> = Vec::new();
    match &names {
        Some(names) => {
            for name in names {
                let base = base_package(name);
                let entry = index.packages.get(base);
                let version_name = if allow_dumpsys {
                    state.packages().version_name(base)
                } else {
                    None
                };
                items.push(PackageInfo {
                    package: name.clone(),
                    user_ids: entry
                        .map(|entry| entry.user_ids.clone())
                        .unwrap_or_default(),
                    installed: entry.is_some(),
                    is_system: entry.is_some_and(|entry| entry.system),
                    version_name,
                    version_code: entry.and_then(|entry| entry.version_code),
                    label: None,
                });
            }
        }
        None => {
            for (package, entry) in &index.packages {
                items.push(PackageInfo {
                    package: package.clone(),
                    user_ids: entry.user_ids.clone(),
                    installed: true,
                    is_system: entry.system,
                    version_name: None,
                    version_code: entry.version_code,
                    label: None,
                });
            }
        }
    }

    if let Some(user) = user {
        let user = user as u32;
        items.retain(|item| !item.installed || item.user_ids.contains(&user));
    }

    let total = items.len();
    let (slice, next_cursor) = page.slice(&items);
    let body = PackagesResponse {
        total,
        next_cursor,
        source: "pm",
        warnings: Vec::new(),
        items: slice.to_vec(),
    };
    Ok(json_response(StatusCode::OK, body))
}
