// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 聚合快照：设备列 + 应用行 + 计数 + 警告 + 聚合 revision。
//! 供 /apps、/overview、/templates 共用，保证规则一致。

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write as _,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    api::{
        apps::{MatrixApp, MatrixCell, MatrixDevice, MatrixTotals},
        dto::{
            CACHE_TTL_SECS, Warning, app_source, overview_revision, parse_overview, stale_state,
            warnings_from,
        },
    },
    db::{DeviceRecord, StoredOverview},
};

/// 一次聚合的结果。
pub(crate) struct MatrixSnapshot {
    pub columns: Vec<MatrixDevice>,
    pub apps: Vec<MatrixApp>,
    pub totals: MatrixTotals,
    pub warnings: Vec<Warning>,
    pub revision: Option<String>,
    pub template_names: Vec<String>,
}

struct Row {
    cells: BTreeMap<String, Option<MatrixCell>>,
    warnings: Vec<Warning>,
}

/// 构建矩阵快照（只读缓存数据，不访问网络）。
pub(crate) fn build_matrix_snapshot(
    records: &[DeviceRecord],
    overviews: &HashMap<String, StoredOverview>,
) -> MatrixSnapshot {
    let mut columns = Vec::with_capacity(records.len());
    let mut parsed: HashMap<String, Value> = HashMap::new();
    let mut warnings: Vec<Warning> = Vec::new();
    let mut template_names: BTreeSet<String> = BTreeSet::new();

    for record in records {
        let Some(stored) = overviews.get(&record.id) else {
            warnings.push(missing_cache(&record.id));
            columns.push(column(record, true, None));
            continue;
        };
        let Some(value) = parse_overview(&stored.payload) else {
            warnings.push(missing_cache(&record.id));
            columns.push(column(record, true, None));
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
        if let Some(templates) = value.get("templates").and_then(Value::as_array) {
            for template in templates {
                if let Some(name) = template.get("name").and_then(Value::as_str) {
                    template_names.insert(name.to_string());
                }
            }
        }
        columns.push(column(
            record,
            freshness.stale,
            Some(stored.fetched_at.clone()),
        ));
        parsed.insert(record.id.clone(), value);
    }

    if records.is_empty() {
        warnings.push(Warning::new("no_devices", "尚未注册设备", json!({})));
    }

    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for record in records {
        let Some(overview) = parsed.get(&record.id) else {
            continue;
        };
        let Some(apps) = overview.get("apps").and_then(Value::as_array) else {
            continue;
        };
        for app in apps {
            let Some(package) = app.get("package").and_then(Value::as_str) else {
                continue;
            };
            let row = rows.entry(package.to_string()).or_insert_with(|| Row {
                cells: BTreeMap::new(),
                warnings: Vec::new(),
            });
            let source = app_source(app.get("source"));
            let cell = MatrixCell {
                model: app
                    .pointer("/resolved/model")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                template: source.template,
                source: source.source_type,
                installed: app.get("installed").and_then(Value::as_bool),
                drift: false,
                warnings: warnings_from(app.get("warnings")),
            };
            row.cells.insert(record.id.clone(), Some(cell));
        }
    }

    let device_ids: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
    let offline_devices: Vec<String> = records
        .iter()
        .filter(|record| !record.online)
        .map(|record| record.id.clone())
        .collect();
    let total_devices = records.len();

    let mut apps: Vec<MatrixApp> = Vec::with_capacity(rows.len());
    for (package, mut row) in rows {
        for device_id in &device_ids {
            row.cells.entry(device_id.clone()).or_insert(None);
        }

        let configured = row.cells.values().filter(|cell| cell.is_some()).count();
        let models = collect_models(&device_ids, &row.cells);
        let templates = collect_templates(&device_ids, &row.cells);
        let inconsistent = total_devices > 0 && (configured < total_devices || models.len() > 1);

        let first_model = models.first().cloned();
        for cell in row.cells.values_mut().flatten() {
            cell.drift = inconsistent && cell.model != first_model;
        }

        for cell in row.cells.values().flatten() {
            for warning in &cell.warnings {
                push_unique(&mut row.warnings, warning.clone());
            }
        }
        if inconsistent {
            push_unique(
                &mut row.warnings,
                Warning::new(
                    "drift",
                    format!("{package} 存在 {} 种机型", models.len()),
                    json!({ "package": package, "models": models }),
                ),
            );
        }

        apps.push(MatrixApp {
            package,
            cells: row.cells,
            configured,
            total: total_devices,
            inconsistent,
            models,
            templates,
            offline_devices: offline_devices.clone(),
            warnings: row.warnings,
        });
    }

    let totals = MatrixTotals {
        apps: apps.len(),
        apps_configured: apps.iter().map(|app| app.configured).sum(),
        drift_apps: apps.iter().filter(|app| app.inconsistent).count(),
        devices: total_devices,
    };

    MatrixSnapshot {
        columns,
        apps,
        totals,
        warnings,
        revision: aggregate_revision(records, overviews),
        template_names: template_names.into_iter().collect(),
    }
}

fn column(record: &DeviceRecord, stale: bool, last_synced_at: Option<String>) -> MatrixDevice {
    MatrixDevice {
        id: record.id.clone(),
        name: record.name.clone(),
        online: record.online,
        model: record.model.clone(),
        android: record.android.clone(),
        last_synced_at: last_synced_at.or_else(|| record.last_synced_at.clone()),
        stale,
    }
}

fn missing_cache(device_id: &str) -> Warning {
    Warning::new(
        "device_cache_missing",
        format!("{device_id} 尚未同步 overview"),
        json!({ "deviceId": device_id }),
    )
}

fn collect_models(
    device_ids: &[String],
    cells: &BTreeMap<String, Option<MatrixCell>>,
) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    for device_id in device_ids {
        if let Some(Some(cell)) = cells.get(device_id)
            && let Some(model) = &cell.model
            && !models.contains(model)
        {
            models.push(model.clone());
        }
    }
    models
}

fn collect_templates(
    device_ids: &[String],
    cells: &BTreeMap<String, Option<MatrixCell>>,
) -> Vec<String> {
    let mut templates: Vec<String> = Vec::new();
    for device_id in device_ids {
        if let Some(Some(cell)) = cells.get(device_id)
            && let Some(template) = &cell.template
            && !templates.contains(template)
        {
            templates.push(template.clone());
        }
    }
    templates
}

fn push_unique(target: &mut Vec<Warning>, warning: Warning) {
    if !target.contains(&warning) {
        target.push(warning);
    }
}

/// sha256(sort("deviceId:deviceRevision\n"))；没有缓存时为 None。
pub(crate) fn aggregate_revision(
    records: &[DeviceRecord],
    overviews: &HashMap<String, StoredOverview>,
) -> Option<String> {
    let mut pairs: Vec<String> = Vec::new();
    for record in records {
        let Some(stored) = overviews.get(&record.id) else {
            continue;
        };
        let revision = stored.revision.clone().or_else(|| {
            parse_overview(&stored.payload).and_then(|value| overview_revision(&value))
        });
        if let Some(revision) = revision {
            pairs.push(format!("{}:{}", record.id, revision));
        }
    }
    if pairs.is_empty() {
        return None;
    }
    pairs.sort();

    let mut hasher = Sha256::new();
    for pair in pairs {
        hasher.update(pair.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    Some(hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::StoredOverview;

    #[test]
    fn aggregate_revision_changes_with_device_set() {
        let mut device = DeviceRecord {
            id: "pixel-9".to_string(),
            name: "Pixel".to_string(),
            base_url: "http://host:8686".to_string(),
            token_cipher: "v1:x".to_string(),
            tags: Vec::new(),
            online: true,
            model: None,
            brand: None,
            android: None,
            sdk: None,
            fingerprint: None,
            root_manager: None,
            module_version: None,
            server_version: None,
            last_seen: None,
            last_synced_at: None,
            latency_ms: None,
            last_error: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let overview = StoredOverview {
            device_id: "pixel-9".to_string(),
            revision: Some("rev-1".to_string()),
            fetched_at: "2026-01-01T00:00:00Z".to_string(),
            payload: "{}".to_string(),
        };
        let mut overviews = HashMap::new();
        overviews.insert("pixel-9".to_string(), overview.clone());
        let first = aggregate_revision(&[device.clone()], &overviews).unwrap();
        assert_eq!(first.len(), 64);

        device.id = "xiaomi-15".to_string();
        overviews.insert(
            "xiaomi-15".to_string(),
            StoredOverview {
                device_id: "xiaomi-15".to_string(),
                revision: Some("rev-2".to_string()),
                ..overview
            },
        );
        let second = aggregate_revision(&[device], &overviews).unwrap();
        assert_ne!(first, second);
    }
}
