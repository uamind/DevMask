// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! GET /relay/api/v1/healthz（与 /healthz 别名共用）。

use axum::{
    Json,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    api_version: &'static str,
    uptime_sec: u64,
    started_at: String,
    db: DbInfo,
    devices: DeviceCounts,
    jobs: JobCounts,
}

#[derive(Debug, Serialize)]
struct DbInfo {
    status: &'static str,
    #[serde(rename = "schemaVersion")]
    schema_version: i64,
    path: String,
}

#[derive(Debug, Serialize)]
struct DeviceCounts {
    total: i64,
    online: i64,
    offline: i64,
}

#[derive(Debug, Serialize)]
struct JobCounts {
    queued: i64,
    running: i64,
}

/// 存活探针：无鉴权；DB 故障时降级为 degraded，但 HTTP 仍 200。
pub async fn healthz(State(state): State<AppState>) -> Response {
    let path = state.db.file_name();

    let (status, db, devices, jobs) = match state.db.health().await {
        Ok(health) => {
            let offline = (health.devices_total - health.devices_online).max(0);
            (
                "ok",
                DbInfo {
                    status: "ok",
                    schema_version: health.schema_version,
                    path,
                },
                DeviceCounts {
                    total: health.devices_total,
                    online: health.devices_online,
                    offline,
                },
                JobCounts {
                    queued: health.jobs_queued,
                    running: health.jobs_running,
                },
            )
        }
        Err(error) => {
            tracing::error!(error = %error, "healthz 数据库检查失败");
            (
                "degraded",
                DbInfo {
                    status: "error",
                    schema_version: 0,
                    path,
                },
                DeviceCounts {
                    total: 0,
                    online: 0,
                    offline: 0,
                },
                JobCounts {
                    queued: 0,
                    running: 0,
                },
            )
        }
    };

    let body = HealthResponse {
        status,
        version: crate::VERSION,
        api_version: "v1",
        uptime_sec: state.uptime_secs(),
        started_at: state.started_at_rfc3339.clone(),
        db,
        devices,
        jobs,
    };

    let mut response = (StatusCode::OK, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
