// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 应用 × 设备矩阵聚合（API.md 4.12）。

use std::collections::HashMap;

use axum::{
    Json,
    extract::{RawQuery, State},
};
use serde::Serialize;

use crate::{
    api::{aggregate::build_matrix_snapshot, dto::Warning, query::QueryParams},
    db::StoredOverview,
    error::ApiError,
    ids, paging,
    state::AppState,
};

/// 矩阵列（设备）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixDevice {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub model: Option<String>,
    pub android: Option<String>,
    pub last_synced_at: Option<String>,
    pub stale: bool,
}

/// 矩阵单元。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixCell {
    pub model: Option<String>,
    pub template: Option<String>,
    pub source: String,
    pub installed: Option<bool>,
    pub drift: bool,
    pub warnings: Vec<Warning>,
}

/// 矩阵行（应用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixApp {
    pub package: String,
    pub cells: std::collections::BTreeMap<String, Option<MatrixCell>>,
    pub configured: usize,
    pub total: usize,
    pub inconsistent: bool,
    pub models: Vec<String>,
    pub templates: Vec<String>,
    pub offline_devices: Vec<String>,
    pub warnings: Vec<Warning>,
}

/// 矩阵汇总。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixTotals {
    pub apps: usize,
    pub apps_configured: usize,
    pub drift_apps: usize,
    pub devices: usize,
}

/// GET /relay/api/v1/apps
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppsMatrixResponse {
    pub generated_at: String,
    pub revision: Option<String>,
    pub devices: Vec<MatrixDevice>,
    pub apps: Vec<MatrixApp>,
    pub totals: MatrixTotals,
    pub warnings: Vec<Warning>,
    pub next_cursor: Option<String>,
}

/// 聚合应用矩阵；只读 SQLite 缓存，不访问设备。
pub async fn matrix(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<AppsMatrixResponse>, ApiError> {
    let params = QueryParams::parse(raw.as_deref());
    let keyword = params.string("q").map(|value| value.to_lowercase());
    let drift_only = params.boolean("drift")?.unwrap_or(false);
    let device_filter = params.string("device");
    let limit = params.usize("limit")?;
    let cursor = params.string("cursor");
    let page = paging::resolve(limit, cursor.as_deref())?;

    let records = state
        .db
        .list_devices()
        .await
        .map_err(|error| internal("apps.list_devices", error))?;
    let stored = state
        .db
        .list_overviews()
        .await
        .map_err(|error| internal("apps.list_overviews", error))?;
    let overviews: HashMap<String, StoredOverview> = stored
        .into_iter()
        .map(|overview| (overview.device_id.clone(), overview))
        .collect();

    let snapshot = build_matrix_snapshot(&records, &overviews);

    let filtered: Vec<MatrixApp> = snapshot
        .apps
        .into_iter()
        .filter(|app| {
            if let Some(keyword) = keyword.as_deref()
                && !app.package.to_lowercase().contains(keyword)
            {
                return false;
            }
            if drift_only && !app.inconsistent {
                return false;
            }
            if let Some(device) = device_filter.as_deref()
                && app
                    .cells
                    .get(device)
                    .and_then(|cell| cell.as_ref())
                    .is_none()
            {
                return false;
            }
            true
        })
        .collect();

    let total_rows = filtered.len();
    let start = page.offset.min(total_rows);
    let page_apps: Vec<MatrixApp> = filtered.into_iter().skip(start).take(page.limit).collect();
    let next_cursor = paging::next_cursor(total_rows, page.offset, page_apps.len());

    Ok(Json(AppsMatrixResponse {
        generated_at: ids::now_rfc3339(),
        revision: snapshot.revision,
        devices: snapshot.columns,
        apps: page_apps,
        totals: snapshot.totals,
        warnings: snapshot.warnings,
        next_cursor,
    }))
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
}
