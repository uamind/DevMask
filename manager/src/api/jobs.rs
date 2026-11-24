// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! GET /relay/api/v1/jobs/{id} 与 GET /relay/api/v1/jobs。

use axum::{
    Json,
    extract::{Path, RawQuery, State},
};

use crate::{
    api::{
        job_dto::{JobResponse, JobSummaryResponse, JobsListResponse, job_response},
        query::QueryParams,
    },
    db::JobFilter,
    error::{ApiError, ValidationError},
    paging,
    state::AppState,
};

const JOB_STATUSES: [&str; 5] = ["queued", "running", "succeeded", "partial", "failed"];

/// GET /relay/api/v1/jobs/{id}
pub async fn get(
    Path(job_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<JobResponse>, ApiError> {
    let job = state
        .db
        .get_job(&job_id)
        .await
        .map_err(|error| internal("jobs.get", error))?
        .ok_or_else(|| ApiError::not_found("job", &job_id))?;
    let results = state
        .db
        .job_results(&job_id)
        .await
        .map_err(|error| internal("jobs.results", error))?;

    Ok(Json(job_response(job, results)))
}

/// GET /relay/api/v1/jobs?limit&cursor&status&app&device
pub async fn list(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<JobsListResponse>, ApiError> {
    let params = QueryParams::parse(raw.as_deref());

    let mut statuses: Vec<String> = Vec::new();
    for value in params.multi("status") {
        for status in value.split(',') {
            let status = status.trim();
            if status.is_empty() {
                continue;
            }
            if !JOB_STATUSES.contains(&status) {
                return Err(ApiError::validation(vec![ValidationError::new(
                    "status",
                    "invalid_value",
                    format!("status 只能是 {}", JOB_STATUSES.join(",")),
                )]));
            }
            if !statuses.iter().any(|existing| existing == status) {
                statuses.push(status.to_string());
            }
        }
    }

    let kind = match params.string("kind").as_deref() {
        Some("bulk") => Some("bulk".to_string()),
        Some("assignments") => Some("assignments".to_string()),
        Some(other) => {
            return Err(ApiError::validation(vec![ValidationError::new(
                "kind",
                "invalid_value",
                format!("kind 只能是 bulk 或 assignments，当前为 {other}"),
            )]));
        }
        None => None,
    };

    let app = params
        .string("app")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let device = params
        .string("device")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let limit = params.usize("limit")?;
    let cursor = params.string("cursor");
    let page = paging::resolve(limit, cursor.as_deref())?;

    let (rows, total) = state
        .db
        .list_jobs(
            JobFilter {
                statuses,
                kind,
                app,
                device,
            },
            page.limit,
            page.offset,
        )
        .await
        .map_err(|error| internal("jobs.list", error))?;

    let items: Vec<JobSummaryResponse> = rows.into_iter().map(JobSummaryResponse::from).collect();
    let next_cursor = paging::next_cursor(total.max(0) as usize, page.offset, items.len());

    Ok(Json(JobsListResponse {
        jobs: items.clone(),
        items,
        total,
        next_cursor,
    }))
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
}
