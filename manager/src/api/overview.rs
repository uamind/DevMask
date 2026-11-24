// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! GET /relay/api/v1/overview：管理端总览（只读缓存与数据库）。

use std::collections::HashMap;

use axum::{Json, extract::State};
use serde::Serialize;

use crate::{
    api::{aggregate::build_matrix_snapshot, dto::DeviceSummary, job_dto::JobSummaryResponse},
    db::StoredOverview,
    error::ApiError,
    ids,
    state::AppState,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetJobCounts {
    pub queued: i64,
    pub running: i64,
    pub last24h: i64,
    pub failed_last24h: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetCounts {
    pub devices: usize,
    pub online: usize,
    pub offline: usize,
    pub apps: usize,
    pub apps_configured: usize,
    pub drift_apps: usize,
    pub templates: usize,
    pub average_latency_ms: Option<i64>,
    pub jobs: FleetJobCounts,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftRow {
    pub package: String,
    pub configured: usize,
    pub total: usize,
    pub models: Vec<String>,
    pub drift_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerOverviewResponse {
    pub generated_at: String,
    pub revision: Option<String>,
    pub fleet: FleetCounts,
    pub devices: Vec<DeviceSummary>,
    pub drift: Vec<DriftRow>,
    pub recent_jobs: Vec<JobSummaryResponse>,
    pub warnings: Vec<crate::api::dto::Warning>,
}

/// GET /relay/api/v1/overview
pub async fn overview(
    State(state): State<AppState>,
) -> Result<Json<ManagerOverviewResponse>, ApiError> {
    let records = state
        .db
        .list_devices()
        .await
        .map_err(|error| internal("overview.list_devices", error))?;
    let stored = state
        .db
        .list_overviews()
        .await
        .map_err(|error| internal("overview.list_overviews", error))?;
    let overviews: HashMap<String, StoredOverview> = stored
        .into_iter()
        .map(|overview| (overview.device_id.clone(), overview))
        .collect();
    let stats = state
        .db
        .job_stats()
        .await
        .map_err(|error| internal("overview.job_stats", error))?;
    let recent_jobs = state
        .db
        .recent_jobs(5)
        .await
        .map_err(|error| internal("overview.recent_jobs", error))?;

    let snapshot = build_matrix_snapshot(&records, &overviews);

    let online = records.iter().filter(|record| record.online).count();
    let latencies: Vec<i64> = records
        .iter()
        .filter(|record| record.online)
        .filter_map(|record| record.latency_ms)
        .collect();
    let average_latency_ms = if latencies.is_empty() {
        None
    } else {
        let sum: i64 = latencies.iter().sum();
        Some((sum as f64 / latencies.len() as f64).round() as i64)
    };

    let drift: Vec<DriftRow> = snapshot
        .apps
        .iter()
        .filter(|app| app.inconsistent)
        .take(20)
        .map(|app| {
            let mut drift_devices = Vec::new();
            for record in &records {
                if let Some(Some(cell)) = app.cells.get(&record.id)
                    && cell.drift
                {
                    drift_devices.push(record.id.clone());
                }
            }
            DriftRow {
                package: app.package.clone(),
                configured: app.configured,
                total: app.total,
                models: app.models.clone(),
                drift_devices,
            }
        })
        .collect();

    let devices: Vec<DeviceSummary> = records.into_iter().map(DeviceSummary::from).collect();
    let recent_jobs: Vec<JobSummaryResponse> = recent_jobs
        .into_iter()
        .map(JobSummaryResponse::from)
        .collect();

    let response = ManagerOverviewResponse {
        generated_at: ids::now_rfc3339(),
        revision: snapshot.revision,
        fleet: FleetCounts {
            devices: snapshot.totals.devices,
            online,
            offline: snapshot.totals.devices.saturating_sub(online),
            apps: snapshot.totals.apps,
            apps_configured: snapshot.totals.apps_configured,
            drift_apps: snapshot.totals.drift_apps,
            templates: snapshot.template_names.len(),
            average_latency_ms,
            jobs: FleetJobCounts {
                queued: stats.queued,
                running: stats.running,
                last24h: stats.last24h,
                failed_last24h: stats.failed_last24h,
            },
        },
        devices,
        drift,
        recent_jobs,
        warnings: snapshot.warnings,
    };

    Ok(Json(response))
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
}
