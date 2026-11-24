// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! GET /relay/api/v1/audit：审计查询（只读数据库）。

use axum::{
    Json,
    extract::{RawQuery, State},
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::{
    api::query::QueryParams,
    db::{AuditFilter, AuditRow},
    error::{ApiError, ValidationError},
    paging,
    state::AppState,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTarget {
    #[serde(rename = "type")]
    pub target_type: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub ts: String,
    pub actor: Option<String>,
    pub action: String,
    pub target: AuditTarget,
    pub request_id: Option<String>,
    pub status: String,
    pub detail: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditListResponse {
    pub entries: Vec<AuditEntry>,
    pub total: i64,
    pub next_cursor: Option<String>,
}

/// GET /relay/api/v1/audit
pub async fn list(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<AuditListResponse>, ApiError> {
    let params = QueryParams::parse(raw.as_deref());

    let mut actions: Vec<String> = Vec::new();
    for value in params.multi("action") {
        for action in value.split(',') {
            let action = action.trim();
            if !action.is_empty() && !actions.iter().any(|existing| existing == action) {
                actions.push(action.to_string());
            }
        }
    }
    let target = params
        .string("target")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let since = match params.string("since") {
        Some(value) => match DateTime::parse_from_rfc3339(&value) {
            Ok(parsed) => Some(
                parsed
                    .with_timezone(&Utc)
                    .to_rfc3339_opts(SecondsFormat::Secs, true),
            ),
            Err(_) => {
                return Err(ApiError::validation(vec![ValidationError::new(
                    "since",
                    "invalid_value",
                    "since 必须是 RFC 3339 时间",
                )]));
            }
        },
        None => None,
    };

    let limit = params.usize("limit")?;
    let cursor = params.string("cursor");
    let page = paging::resolve(limit, cursor.as_deref())?;

    let filter = AuditFilter {
        actions,
        target,
        since,
        limit: page.limit,
        offset: page.offset,
    };
    let (rows, total) = state
        .db
        .query_audit(filter)
        .await
        .map_err(|error| internal("audit.query", error))?;

    let entries: Vec<AuditEntry> = rows.into_iter().map(AuditEntry::from).collect();
    let next_cursor = paging::next_cursor(total.max(0) as usize, page.offset, entries.len());

    Ok(Json(AuditListResponse {
        entries,
        total,
        next_cursor,
    }))
}

impl From<AuditRow> for AuditEntry {
    fn from(row: AuditRow) -> Self {
        Self {
            id: row.id,
            ts: row.ts,
            actor: row.actor,
            action: row.action,
            target: AuditTarget {
                target_type: row.target_type,
                id: row.target_id,
            },
            request_id: row.request_id,
            status: row.status,
            detail: row.detail,
        }
    }
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
}
