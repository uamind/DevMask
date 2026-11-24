// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 打印 JSON Schema（server 的 /api/v1/schema 与文档生成可直接复用）。
//!
//! cargo run -p device_faker_core --example schema
//! cargo run -p device_faker_core --example schema -- config

fn main() {
    let target = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bundle".to_owned());
    let value = match target.as_str() {
        "config" => device_faker_core::schema::config_schema(),
        "template" => device_faker_core::schema::template_schema(),
        "app" => device_faker_core::schema::app_schema(),
        _ => device_faker_core::schema::bundle(),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("schema must serialize")
    );
}
