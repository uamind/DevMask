// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! schemars 生成的 JSON Schema（docs/dev/server-api-plan.md 9.2 / 5.8）。
//!
//! server 侧 @@/api/v1/schema@@ 直接返回这里的产物；字段名与 TOML 完全一致（snake_case），
//! @@sdk_int@@ 用 @@anyOf@@ 描述「整数 或字符串 `__DELETE__`」；egress 的代理名规则
//! （`^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`、应用级另允许 `__DIRECT__`）随模型 derive 自动带出。
//!
//! 版本约定：TOML / JSON 都对着同一份模型，所以 **不要**为了 API 另写一套 DTO——
//! 否则两份定义迟早漂移。

use serde_json::{Map, Value};

use crate::config::{AppConfig, Config, DeviceTemplate};

/// @@Config@@ 的 JSON Schema。
pub fn config_schema() -> Value {
    schema_for::<Config>("DeviceFakerConfig")
}

/// @@DeviceTemplate@@ 的 JSON Schema。
pub fn template_schema() -> Value {
    schema_for::<DeviceTemplate>("DeviceFakerTemplate")
}

/// @@AppConfig@@ 的 JSON Schema。
pub fn app_schema() -> Value {
    schema_for::<AppConfig>("DeviceFakerApp")
}

/// 三份 schema 打包，便于一次性下发给 WebUI / AI。
pub fn bundle() -> Value {
    let mut definitions = Map::new();
    definitions.insert("config".to_owned(), config_schema());
    definitions.insert("template".to_owned(), template_schema());
    definitions.insert("app".to_owned(), app_schema());

    let mut root = Map::new();
    root.insert(
        "$schema".to_owned(),
        Value::String("http://json-schema.org/draft-07/schema#".to_owned()),
    );
    root.insert(
        "title".to_owned(),
        Value::String("DeviceFakerSchemas".to_owned()),
    );
    root.insert("definitions".to_owned(), Value::Object(definitions));
    Value::Object(root)
}

fn schema_for<T: schemars::JsonSchema>(title: &str) -> Value {
    let root = schemars::schema_for!(T);
    let mut value = serde_json::to_value(root).unwrap_or(Value::Null);
    if let Value::Object(object) = &mut value {
        object.insert("title".to_owned(), Value::String(title.to_owned()));
    }
    value
}
