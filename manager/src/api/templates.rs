// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! GET /relay/api/v1/templates：跨设备模板并集（只读缓存）。

use std::collections::{BTreeMap, HashMap};

use axum::{
    Json,
    extract::{RawQuery, State},
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    api::{
        aggregate::aggregate_revision,
        dto::{
            CACHE_TTL_SECS, TemplateSummary, Warning, overview_templates, parse_overview,
            stale_state,
        },
        query::QueryParams,
    },
    db::StoredOverview,
    error::ApiError,
    ids, paging,
    state::AppState,
};

/// GET /templates 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatesResponse {
    pub generated_at: String,
    pub revision: Option<String>,
    pub templates: Vec<TemplateSummary>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub warnings: Vec<Warning>,
}

#[derive(Default)]
struct Accumulator {
    config: Option<Value>,
    values: Vec<Value>,
    sources: Vec<String>,
}

/// GET /relay/api/v1/templates
pub async fn list(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<TemplatesResponse>, ApiError> {
    let params = QueryParams::parse(raw.as_deref());
    let package_filter = params.string("package");
    let device_filter = params.string("device");
    let limit = params.usize("limit")?;
    let cursor = params.string("cursor");
    let page = paging::resolve(limit, cursor.as_deref())?;

    let records = state
        .db
        .list_devices()
        .await
        .map_err(|error| internal("templates.list_devices", error))?;
    let stored = state
        .db
        .list_overviews()
        .await
        .map_err(|error| internal("templates.list_overviews", error))?;
    let overviews: HashMap<String, StoredOverview> = stored
        .into_iter()
        .map(|overview| (overview.device_id.clone(), overview))
        .collect();

    let mut warnings: Vec<Warning> = Vec::new();
    let mut accumulators: BTreeMap<String, Accumulator> = BTreeMap::new();

    for record in &records {
        if let Some(device) = device_filter.as_deref()
            && record.id != device
        {
            continue;
        }
        let Some(stored) = overviews.get(&record.id) else {
            warnings.push(missing_cache(&record.id));
            continue;
        };
        let Some(value) = parse_overview(&stored.payload) else {
            warnings.push(missing_cache(&record.id));
            continue;
        };
        let freshness = stale_state(Some(&stored.fetched_at), record.online);
        if freshness.age_secs.is_some_and(|age| age > CACHE_TTL_SECS) {
            warnings.push(Warning::new(
                "stale_cache",
                format!("{} 的缓存已过期", record.name),
                json!({ "deviceId": record.id, "ageSec": freshness.age_secs }),
            ));
        }

        for template in overview_templates(&value, &record.id) {
            let accumulator = accumulators.entry(template.name.clone()).or_default();
            if !accumulator.sources.contains(&record.id) {
                accumulator.sources.push(record.id.clone());
            }
            match &accumulator.config {
                None => {
                    accumulator.config = Some(template.config.clone());
                    accumulator.values.push(template.config);
                }
                Some(current) => {
                    if current != &template.config
                        && !accumulator
                            .values
                            .iter()
                            .any(|value| value == &template.config)
                    {
                        accumulator.values.push(template.config);
                    }
                }
            }
        }
    }

    if records.is_empty() {
        warnings.push(Warning::new("no_devices", "尚未注册设备", json!({})));
    }

    let mut templates: Vec<TemplateSummary> = Vec::with_capacity(accumulators.len());
    for (name, accumulator) in accumulators {
        let config = accumulator.config.unwrap_or_else(|| json!({}));
        let mut fields_set: Vec<String> = config
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        fields_set.sort();
        fields_set.dedup();
        let complete = ["model", "brand", "android_version", "sdk_int", "dpi"]
            .iter()
            .all(|field| config.get(*field).is_some());

        let mut template_warnings = Vec::new();
        if accumulator.values.len() > 1 {
            template_warnings.push(Warning::new(
                "template_conflict",
                format!("模板 {name} 在多台设备上的配置不一致"),
                json!({
                    "name": name,
                    "devices": accumulator.sources,
                    "values": accumulator.values,
                }),
            ));
        }

        if let Some(package) = package_filter.as_deref()
            && !config
                .get("packages")
                .and_then(Value::as_array)
                .is_some_and(|packages| {
                    packages.iter().any(|value| value.as_str() == Some(package))
                })
        {
            continue;
        }
        let sources = match device_filter.as_deref() {
            Some(device) if accumulator.sources.iter().any(|source| source == device) => {
                vec![device.to_string()]
            }
            Some(_) => continue,
            None => accumulator.sources,
        };

        templates.push(TemplateSummary {
            name,
            config,
            fields_set,
            sources,
            complete,
            warnings: template_warnings,
        });
    }

    let total = templates.len();
    let start = page.offset.min(total);
    let page_templates: Vec<TemplateSummary> =
        templates.into_iter().skip(start).take(page.limit).collect();
    let next_cursor = paging::next_cursor(total, page.offset, page_templates.len());

    Ok(Json(TemplatesResponse {
        generated_at: ids::now_rfc3339(),
        revision: aggregate_revision(&records, &overviews),
        templates: page_templates,
        total,
        next_cursor,
        warnings,
    }))
}

fn missing_cache(device_id: &str) -> Warning {
    Warning::new(
        "device_cache_missing",
        format!("{device_id} 尚未同步 overview"),
        json!({ "deviceId": device_id }),
    )
}

fn internal(op: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %error, op, "内部错误");
    ApiError::internal(op)
}
