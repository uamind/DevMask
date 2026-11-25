// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! /apply 与 /jobs 的请求 / 响应 DTO。

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{JobResultRow, JobRow};

/// devices 字段：字面量 "all" 或设备 id 数组。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DevicesSpec {
    All(String),
    List(Vec<String>),
}

/// 模板引用：模板名，或 { name, config }。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TemplateRef {
    Name(String),
    Object(TemplateObject),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateObject {
    pub name: String,
    pub config: Value,
}

/// 逐设备 assignment。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentRequest {
    pub device: String,
    pub app: String,
    pub template: TemplateRef,
}

/// POST /apply 请求体；bulk 与 assignments 只能二选一。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyRequest {
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub devices: Option<DevicesSpec>,
    #[serde(default)]
    pub template: Option<TemplateRef>,
    #[serde(default)]
    pub assignments: Option<Vec<AssignmentRequest>>,
    #[serde(default)]
    pub restart: Option<bool>,
}

/// 202 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyAcceptedResponse {
    pub job_id: String,
    pub status: String,
    pub kind: String,
    pub total: i64,
    pub queued: i64,
    pub created_at: String,
    pub poll_url: String,
}

/// job 摘要（列表 / 总览使用，不含 results）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSummaryResponse {
    pub id: String,
    pub status: String,
    pub kind: String,
    pub app: Option<String>,
    pub template: Option<String>,
    pub restart: bool,
    pub total: i64,
    pub succeeded: i64,
    pub skipped: i64,
    pub failed: i64,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
}

impl From<JobRow> for JobSummaryResponse {
    fn from(job: JobRow) -> Self {
        let duration_ms = duration_between(job.started_at.as_deref(), job.finished_at.as_deref());
        Self {
            id: job.id,
            status: job.status,
            kind: job.kind,
            app: job.app,
            template: job.template,
            restart: job.restart,
            total: job.total,
            succeeded: job.succeeded,
            skipped: job.skipped,
            failed: job.failed,
            created_at: job.created_at,
            started_at: job.started_at,
            finished_at: job.finished_at,
            duration_ms,
        }
    }
}

/// GET /jobs 列表响应；items 为规范字段，jobs 为兼容旧客户端的别名。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsListResponse {
    pub items: Vec<JobSummaryResponse>,
    pub jobs: Vec<JobSummaryResponse>,
    pub total: i64,
    pub next_cursor: Option<String>,
}

/// GET /jobs/{id} 的逐设备结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResultResponse {
    pub device_id: String,
    pub device_name: String,
    pub app: String,
    pub status: String,
    pub code: Option<String>,
    pub http_status: Option<i64>,
    pub revision: Option<String>,
    pub attempts: i64,
    pub duration_ms: i64,
    pub steps: Vec<Value>,
    pub restart: Option<Value>,
    pub warnings: Vec<Value>,
    pub error: Option<Value>,
}

/// GET /jobs/{id} 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResponse {
    pub id: String,
    pub status: String,
    pub kind: String,
    pub app: Option<String>,
    pub template: Option<String>,
    pub restart: bool,
    pub total: i64,
    pub succeeded: i64,
    pub skipped: i64,
    pub failed: i64,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub request_id: String,
    pub idempotency_key: Option<String>,
    pub warnings: Vec<Value>,
    pub results: Vec<JobResultResponse>,
}

impl From<JobRow> for ApplyAcceptedResponse {
    /// 幂等重放使用首次 202 的快照语义：status=queued、queued=total。
    fn from(job: JobRow) -> Self {
        Self {
            poll_url: format!("/relay/api/v1/jobs/{}", job.id),
            job_id: job.id,
            status: "queued".to_string(),
            kind: job.kind,
            total: job.total,
            queued: job.total,
            created_at: job.created_at,
        }
    }
}

impl From<JobResultRow> for JobResultResponse {
    fn from(row: JobResultRow) -> Self {
        Self {
            device_name: row.device_name.unwrap_or_else(|| row.device_id.clone()),
            device_id: row.device_id,
            app: row.app,
            status: row.status,
            code: row.code,
            http_status: row.http_status,
            revision: row.revision,
            attempts: row.attempts,
            duration_ms: row.duration_ms.unwrap_or(0),
            steps: row.steps,
            restart: row.restart,
            warnings: row.warnings,
            error: row.error,
        }
    }
}

/// 组装 GET /jobs/{id} 响应。
pub fn job_response(job: JobRow, results: Vec<JobResultRow>) -> JobResponse {
    let duration_ms = duration_between(job.started_at.as_deref(), job.finished_at.as_deref());
    JobResponse {
        id: job.id,
        status: job.status,
        kind: job.kind,
        app: job.app,
        template: job.template,
        restart: job.restart,
        total: job.total,
        succeeded: job.succeeded,
        skipped: job.skipped,
        failed: job.failed,
        created_at: job.created_at,
        started_at: job.started_at,
        finished_at: job.finished_at,
        duration_ms,
        request_id: job.request_id,
        idempotency_key: job.idempotency_key,
        warnings: Vec::new(),
        results: results.into_iter().map(JobResultResponse::from).collect(),
    }
}

fn duration_between(start: Option<&str>, end: Option<&str>) -> Option<i64> {
    let start = DateTime::parse_from_rfc3339(start?).ok()?;
    let end = DateTime::parse_from_rfc3339(end?).ok()?;
    Some((end - start).num_milliseconds().max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::JobResultRow;
    use serde_json::json;

    fn sample_job() -> JobRow {
        JobRow {
            id: "job_test".to_string(),
            kind: "bulk".to_string(),
            status: "partial".to_string(),
            app: Some("com.xingin.xhs".to_string()),
            template: Some("pixel_9".to_string()),
            restart: false,
            total: 2,
            succeeded: 1,
            skipped: 1,
            failed: 0,
            request_id: "req_test".to_string(),
            idempotency_key: None,
            error: None,
            created_at: "2026-11-24T08:00:00Z".to_string(),
            started_at: Some("2026-11-24T08:00:00Z".to_string()),
            finished_at: Some("2026-11-24T08:00:02Z".to_string()),
        }
    }

    #[test]
    fn accepted_response_exposes_queued() {
        let response = ApplyAcceptedResponse::from(sample_job());
        assert_eq!(response.job_id, "job_test");
        assert_eq!(response.status, "queued");
        assert_eq!(response.queued, 2);
        assert_eq!(response.poll_url, "/relay/api/v1/jobs/job_test");
    }

    #[test]
    fn job_response_includes_duration_and_results() {
        let result = JobResultRow {
            ordinal: 0,
            device_id: "pixel-9".to_string(),
            device_name: Some("Pixel 9".to_string()),
            app: "com.xingin.xhs".to_string(),
            status: "ok".to_string(),
            code: None,
            http_status: Some(200),
            revision: Some("rev-2".to_string()),
            attempts: 2,
            duration_ms: Some(120),
            restart: None,
            steps: vec![json!({"step": "bind"})],
            warnings: Vec::new(),
            error: None,
        };
        let response = job_response(sample_job(), vec![result]);
        assert_eq!(response.status, "partial");
        assert_eq!(response.duration_ms, Some(2000));
        assert_eq!(response.results[0].device_name, "Pixel 9");
        assert_eq!(response.results[0].attempts, 2);
    }
}
