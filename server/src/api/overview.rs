// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! GET /api/v1/overview：一次返回设备状态、模板与每个应用最终生效的配置。
//!
//! 与 `GET /status` 的区别：`/status` 只给配置计数，本端点把所有模板和
//! 「包名 → 来源 → 最终生效指纹」摊平，供 AI 或脚本一次读完，不必对每个包
//! 再发 `GET /apps/{pkg}`。未知包不会出现在 apps 中，避免把整台设备的
//! 应用列表塞进响应。

use std::collections::BTreeSet;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::Response,
};
use device_faker_core::Config;
use serde::Serialize;
use serde_json::{Value, json};

use crate::api::device::DeviceDetail;
use crate::api::{ConfigStatus, ServerInfo, json as json_response, with_revision};
use crate::app::AppState;
use crate::commands::CommandOutcome;
use crate::device::{self, RootInfo};
use crate::dto::{AppResolved, AppSource, Warning, duplicate_package_warnings};
use crate::error::ApiError;
use crate::module;
use crate::packages::base_package;
use crate::requests::RequestId;
use crate::store;

/// GET /api/v1/overview 响应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewResponse {
    /// 当前配置 revision。
    pub revision: String,
    /// 模块信息（module.prop）。
    pub module: module::ModuleInfo,
    /// 设备摘要 + Build.FINGERPRINT。
    pub device: DeviceDetail,
    /// root 管理器。
    pub root: RootInfo,
    /// 服务运行时信息。
    pub server: ServerInfo,
    /// 配置文件的 revision 与统计。
    pub config: ConfigStatus,
    /// 模板及其原始字段。
    pub templates: Vec<TemplateOverview>,
    /// 每个已配置包名的来源与最终生效值。
    pub apps: Vec<AppOverview>,
    /// 配置级警告（如同一包名被多个模板声明）。
    pub warnings: Vec<Warning>,
}

/// 模板概览：名称、书写位置、绑定的包名与模板原始字段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateOverview {
    /// 模板名。
    pub name: String,
    /// 0 基书写位置。
    pub position: usize,
    /// 绑定包名。
    pub packages: Vec<String>,
    /// 模板字段（不含 packages）。
    pub config: Value,
}

/// 应用概览：来源与最终生效值。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppOverview {
    /// 包名。
    pub package: String,
    /// 是否已安装；pm 不可用时为 null。
    pub installed: Option<bool>,
    /// 配置来源。
    pub source: AppSource,
    /// 最终生效值；未配置时为 {}。
    pub resolved: Value,
    /// 该包的警告。
    pub warnings: Vec<Warning>,
}

/// GET /api/v1/overview：需要 read 作用域（由路由层守卫）。
pub async fn get_overview(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let snapshot = store::read_snapshot(state.store())
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error}")))?;
    let config = Config::from_toml(&snapshot.text)
        .map_err(|error| ApiError::internal_error(request_id.as_str(), format!("{error:#}")))?;
    let config_status = crate::api::config_status_for(&snapshot, &config);

    let fingerprint = match state.commands().run("getprop", &["ro.build.fingerprint"]) {
        CommandOutcome::Ok(stdout) => {
            let value = stdout.trim();
            (!value.is_empty()).then(|| value.to_owned())
        }
        _ => None,
    };

    let templates: Vec<TemplateOverview> = config
        .templates
        .iter()
        .enumerate()
        .map(|(position, (name, template))| {
            let mut config = serde_json::to_value(template).unwrap_or_else(|_| json!({}));
            if let Some(object) = config.as_object_mut() {
                object.remove("packages");
            }
            TemplateOverview {
                name: name.clone(),
                position,
                packages: template.packages.clone(),
                config,
            }
        })
        .collect();

    let mut packages: BTreeSet<String> = BTreeSet::new();
    for app in &config.apps {
        packages.insert(app.package.clone());
    }
    for template in config.templates.values() {
        for package in &template.packages {
            packages.insert(package.clone());
        }
    }

    let apps: Vec<AppOverview> = packages
        .into_iter()
        .map(|package| {
            let installed = state.packages().is_installed(base_package(&package));
            let source = crate::api::apps::source_for(&config, &package);
            let resolved = config
                .get_merged_config(&package)
                .map(|merged| {
                    serde_json::to_value(AppResolved::from_merged(&merged))
                        .unwrap_or_else(|_| json!({}))
                })
                .unwrap_or_else(|| json!({}));
            let warnings = crate::api::apps::warnings_for(&config, &package, installed);
            AppOverview {
                package,
                installed,
                source,
                resolved,
                warnings,
            }
        })
        .collect();

    let body = OverviewResponse {
        revision: snapshot.revision.clone(),
        module: module::read(&state.settings().module_prop_path),
        device: DeviceDetail {
            summary: state.device().clone(),
            fingerprint,
        },
        root: device::root_info(&state.settings().adb_dir),
        server: ServerInfo {
            version: state.version(),
            bind: state.bind().to_owned(),
            uptime_sec: state.uptime_sec(),
            started_at: state.started_at().to_owned(),
            pid: std::process::id(),
        },
        config: config_status,
        templates,
        apps,
        warnings: duplicate_package_warnings(&config),
    };

    Ok(with_revision(
        json_response(StatusCode::OK, body),
        &snapshot.revision,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_response_serializes_camel_case() {
        let value = serde_json::to_value(TemplateOverview {
            name: "pixel_9".to_owned(),
            position: 0,
            packages: vec!["com.example.app".to_owned()],
            config: json!({ "model": "Pixel 9" }),
        })
        .unwrap();
        assert_eq!(value["name"], "pixel_9");
        assert_eq!(value["packages"][0], "com.example.app");
    }
}
