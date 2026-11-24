// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! Device Faker 共享配置内核。
//!
//! 这一层被三处复用：zygisk 模块（cdylib）、device_faker_server（HTTP / AI API）与测试。
//! 抽取的唯一目标是**行为不变**——解析、模板优先规则、属性族展开与删除语义与模块内联
//! 版本逐条一致；新增能力只出现在写路径（[`edit`] / [`revision`]）与校验（[`validate`]）。
//!
//! 模块划分（对应 docs/dev/server-api-plan.md 第 9 节）：
//!
//! - [`config`]：Config / DeviceTemplate / AppConfig / NumberSetting / MergedAppConfig
//!   与合并、冲突检测；
//! - [`edit`]：基于 toml_edit 的文档级增量编辑，保留注释、空行、键顺序与未知字段；
//! - [`revision`]：sha256 revision、flock、临时文件 + rename + fsync、chmod 0644、
//!   自动备份（默认保留 10 份）；
//! - [`validate`]：包名、dpi、sdk_int、android_version、custom_props 键白名单与重复包名冲突；
//! - [`schema`]：schemars 生成的 JSON Schema。
//!
//! Feature 划分（zygisk 模块用 `default-features = false`，不引入 toml_edit / schemars / sha2）：
//!
//! - `edit`：文档级编辑（toml_edit）；
//! - `store`：原子写 + flock + 备份轮转（依赖 `edit`）；
//! - `schema`：JSON Schema（schemars）。

pub mod config;
pub mod validate;

#[cfg(feature = "edit")]
pub mod edit;
#[cfg(feature = "store")]
pub mod revision;
#[cfg(feature = "schema")]
pub mod schema;

pub use config::{AppConfig, Config, DeviceTemplate, MergedAppConfig, NumberSetting};
pub use validate::{DuplicatePackage, Issue, Severity, ValidationOptions, ValidationReport};

#[cfg(feature = "edit")]
pub use edit::{
    EditError, bind_package, delete_app, delete_template, parse_document, set_app, unbind_package,
    upsert_template,
};
#[cfg(feature = "store")]
pub use revision::{ConfigStore, FileLock, Snapshot, StoreError, StoreOptions};
