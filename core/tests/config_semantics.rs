// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 抽取前后「解析 / 合并」语义回归：这些断言直接锁住模块原来的行为，
//! core 与 zygisk 共用同一份实现，任何一条改动都意味着线上伪装结果会变。

use devmask_core::config::{Config, NumberSetting, is_patch_only_key};

const CONFIG: &str = r#"debug = true
default_force_denylist_unmount = true
default_hide_maps = ["libc.so"]

# 书写顺序：z_first 在前
[templates.z_first]
packages = ["com.example.shared", "com.example.a"]
brand = "first"
model = "First"
dpi = 999

[templates.a_second]
packages = ["com.example.shared"]
brand = "second"

[templates.fp]
packages = ["com.example.fp"]
fingerprint = "google/tokay/tokay:15/AP4A.250105.002/1234567:user/release-keys"

[templates.dup_binding]
packages = ["com.example.app"]
brand = "template-brand"

[[apps]]
package = "com.example.app"
brand = "inline-brand"
sdk_int = "__DELETE__"

[[apps]]
package = "com.example.withprops"
custom_props = { "ro.product.model" = "Custom", "persist.vendor.debug" = "__EMPTY__" }

# 顶层未知字段：解析时忽略，但写路径必须保留
[cpu_presets.kirin]
name = "Kirin 9000"
"#;

fn config() -> Config {
    Config::from_toml(CONFIG).expect("fixture must parse")
}

#[test]
fn template_lookup_uses_written_order() {
    let config = config();
    let template = config
        .find_template_for_package("com.example.shared")
        .expect("shared package must resolve");
    assert_eq!(template.brand.as_deref(), Some("first"));
    assert_eq!(
        config.conflicting_templates_for_package("com.example.shared"),
        vec!["z_first", "a_second"]
    );
    // 顺序即 IndexMap 的插入顺序
    assert_eq!(
        config.templates.keys().cloned().collect::<Vec<_>>(),
        vec!["z_first", "a_second", "fp", "dup_binding"]
    );
}

#[test]
fn direct_app_config_wins_over_template() {
    let config = config();
    let merged = config
        .get_merged_config("com.example.app")
        .expect("app entry must resolve");
    assert_eq!(merged.brand.as_deref(), Some("inline-brand"));
    assert_eq!(merged.sdk_int, Some(NumberSetting::Delete));
    // 默认值继承
    assert!(merged.force_denylist_unmount);
    assert_eq!(merged.hide_maps, vec!["libc.so".to_string()]);
}

#[test]
fn out_of_range_dpi_is_ignored_on_merge() {
    let config = config();
    let merged = config
        .get_merged_config("com.example.a")
        .expect("template package must resolve");
    assert_eq!(merged.dpi, None, "dpi 999 超出 120-640，合并时必须忽略");
    assert!(merged.force_denylist_unmount);
}

#[test]
fn fingerprint_fills_only_missing_fields() {
    let config = config();
    let merged = config
        .get_merged_config("com.example.fp")
        .expect("template must resolve");
    assert_eq!(merged.brand.as_deref(), Some("google"));
    assert_eq!(merged.product.as_deref(), Some("tokay"));
    assert_eq!(merged.name.as_deref(), Some("tokay"));
    assert_eq!(merged.device.as_deref(), Some("tokay"));
    assert_eq!(merged.android_version.as_deref(), Some("15"));
    assert_eq!(merged.build_id.as_deref(), Some("AP4A.250105.002"));
    assert_eq!(merged.incremental.as_deref(), Some("1234567"));

    let custom = merged.custom_props.as_ref().expect("derived props");
    assert_eq!(
        custom.get("ro.build.type").map(String::as_str),
        Some("user")
    );
    assert_eq!(
        custom.get("ro.build.tags").map(String::as_str),
        Some("release-keys")
    );
    assert!(merged.derived_props.contains("ro.build.type"));
    assert!(merged.derived_props.contains("ro.build.tags"));
}

#[test]
fn property_families_are_expanded() {
    let config = config();
    let merged = config.get_merged_config("com.example.fp").unwrap();
    let (props, explicit) = Config::build_merged_property_map_with_explicit(&merged);

    assert_eq!(
        props.get("ro.product.brand").map(String::as_str),
        Some("google")
    );
    assert_eq!(
        props.get("ro.product.system_ext.brand").map(String::as_str),
        Some("google")
    );
    assert_eq!(
        props.get("ro.build.fingerprint").map(String::as_str),
        Some("google/tokay/tokay:15/AP4A.250105.002/1234567:user/release-keys")
    );
    assert_eq!(
        props.get("ro.vendor.build.fingerprint").map(String::as_str),
        Some("google/tokay/tokay:15/AP4A.250105.002/1234567:user/release-keys")
    );
    assert_eq!(
        props.get("ro.build.version.release").map(String::as_str),
        Some("15")
    );
    // 指纹派生键不算显式
    assert!(explicit.is_empty());
}

#[test]
fn custom_props_expand_but_stay_explicit() {
    let config = config();
    let merged = config.get_merged_config("com.example.withprops").unwrap();
    let (props, explicit) = Config::build_merged_property_map_with_explicit(&merged);

    assert_eq!(
        props.get("ro.product.model").map(String::as_str),
        Some("Custom")
    );
    assert_eq!(
        props.get("ro.product.vendor.model").map(String::as_str),
        Some("Custom"),
        "已知属性族必须整族展开，覆盖 bionic prefix routing 的任一副本"
    );
    assert_eq!(
        props.get("persist.vendor.debug").map(String::as_str),
        Some("")
    );
    assert!(explicit.contains("ro.product.model"));
    assert!(explicit.contains("persist.vendor.debug"));
}

#[test]
fn delete_markers_become_delete_lists() {
    let config = config();
    let mut merged = config.get_merged_config("com.example.app").unwrap();
    merged.brand = Some("__DELETE__".to_string());
    merged.marketname = Some("__DELETE__".to_string());

    let deletes = Config::build_delete_props_list(&merged);
    assert!(deletes.contains(&"ro.product.brand".to_string()));
    assert!(deletes.contains(&"ro.product.odm.brand".to_string()));
    assert!(deletes.contains(&"ro.vendor.oplus.market.name".to_string()));
    assert!(deletes.contains(&"ro.build.version.sdk".to_string()));
    assert!(deletes.contains(&"ro.vendor.build.version.sdk".to_string()));
}

#[test]
fn patch_only_keys_follow_rom_availability() {
    assert!(is_patch_only_key("ro.product.system_dlkm.model"));
    assert!(is_patch_only_key("ro.product.marketname"));
    assert!(is_patch_only_key("ro.vendor.oplus.market.name"));
    assert!(!is_patch_only_key("ro.product.model"));
    assert!(!is_patch_only_key("persist.vendor.debug"));
}

#[test]
fn number_setting_parses_int_and_delete_marker() {
    #[derive(serde::Deserialize)]
    struct Holder {
        sdk_int: Option<NumberSetting>,
    }

    let value: Holder = toml::from_str("sdk_int = 35").unwrap();
    assert_eq!(value.sdk_int, Some(NumberSetting::Value(35)));
    assert_eq!(value.sdk_int.and_then(NumberSetting::value), Some(35));

    let delete: Holder = toml::from_str(r#"sdk_int = "__DELETE__""#).unwrap();
    assert_eq!(delete.sdk_int, Some(NumberSetting::Delete));
    assert!(delete.sdk_int.unwrap().is_delete());

    let broken: Result<Holder, _> = toml::from_str(r#"sdk_int = "abc""#);
    assert!(broken.is_err());
}

#[test]
fn json_roundtrip_keeps_semantics() {
    let config = config();
    let json = serde_json::to_value(&config).expect("serialize config");
    // None 字段不出现（skip_serializing_if），API 输出更干净
    let template = &json["templates"]["z_first"];
    assert!(template.get("manufacturer").is_none());
    assert_eq!(template["packages"][0], "com.example.shared");
    // NumberSetting 保持 TOML 的写法
    assert_eq!(json["apps"][0]["sdk_int"], "__DELETE__");
    assert_eq!(
        json["apps"][1]["custom_props"]["ro.product.model"],
        "Custom"
    );

    let back: Config = serde_json::from_value(json).expect("deserialize config");
    assert_eq!(back, config);
}
