// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 校验规则与 JSON Schema 回归。

use devmask_core::config::{AppConfig, Config, DeviceTemplate, NumberSetting};
use devmask_core::edit::parse_document;
use devmask_core::schema::{app_schema, bundle, config_schema, template_schema};
use devmask_core::validate::{
    Severity, ValidationOptions, duplicate_packages, is_valid_package_key, unknown_fields,
    validate_app, validate_config, validate_document, validate_package_key, validate_template,
};

fn codes(report: &devmask_core::validate::ValidationReport) -> Vec<&'static str> {
    report.issues.iter().map(|issue| issue.code).collect()
}

#[test]
fn package_name_rules() {
    for valid in [
        "com.example.app",
        "com.tencent.tmgp.sgame",
        "com.example.app@10",
        "a.b",
        "com.example._private",
    ] {
        assert!(is_valid_package_key(valid), "{valid} 应当合法");
    }

    for invalid in [
        "",
        "com",
        "com..example",
        "com.example.",
        ".com.example",
        "com.1example.app",
        "com.example.app@x",
        "com.example.app@1000",
        "com example.app",
    ] {
        assert!(!is_valid_package_key(invalid), "{invalid} 应当非法");
        assert!(validate_package_key(invalid).is_some());
    }
}

#[test]
fn dpi_and_sdk_and_android_version_rules() {
    let ok = DeviceTemplate {
        packages: vec!["com.example.app".to_string()],
        dpi: Some(420),
        sdk_int: Some(NumberSetting::Value(35)),
        android_version: Some("15".to_string()),
        ..Default::default()
    };
    let report = validate_template("ok", &ok);
    assert!(report.is_ok(), "{:?}", report.issues);
    assert!(report.issues.is_empty());

    let bad = DeviceTemplate {
        packages: vec!["com.example.app".to_string()],
        dpi: Some(641),
        sdk_int: Some(NumberSetting::Value(0)),
        android_version: Some("fifteen".to_string()),
        ..Default::default()
    };
    let report = validate_template("bad", &bad);
    assert!(!report.is_ok());
    assert!(codes(&report).contains(&"dpi.out_of_range"));
    assert!(codes(&report).contains(&"sdk_int.out_of_range"));
    assert!(codes(&report).contains(&"android_version.invalid"));

    // 边界值合法
    for dpi in [120u32, 640] {
        let template = DeviceTemplate {
            dpi: Some(dpi),
            ..Default::default()
        };
        assert!(validate_template("edge", &template).is_ok());
    }
    // 删除标记合法
    let delete = DeviceTemplate {
        android_version: Some("__DELETE__".to_string()),
        sdk_int: Some(NumberSetting::Delete),
        ..Default::default()
    };
    assert!(validate_template("delete", &delete).is_ok());
}

#[test]
fn custom_props_whitelist_and_lengths() {
    let mut props = std::collections::HashMap::new();
    props.insert("ro.product.model".to_string(), "Pixel 9".to_string());
    props.insert("persist.vendor.debug".to_string(), "1".to_string());
    let good = DeviceTemplate {
        custom_props: Some(props),
        ..Default::default()
    };
    assert!(validate_template("good", &good).is_ok());

    let mut props = std::collections::HashMap::new();
    props.insert("vendor.product.model".to_string(), "x".to_string());
    props.insert("ro.ok".to_string(), "y".repeat(93));
    let warn = DeviceTemplate {
        custom_props: Some(props),
        ..Default::default()
    };
    let report = validate_template("warn", &warn);
    assert!(!report.is_ok());
    assert!(codes(&report).contains(&"custom_props.key_not_allowed"));
    assert_eq!(report.warning_count(), 1);
    assert!(codes(&report).contains(&"custom_props.value_too_long"));

    let mut props = std::collections::HashMap::new();
    props.insert("ro.huge".to_string(), "z".repeat(5000));
    let huge = DeviceTemplate {
        custom_props: Some(props),
        ..Default::default()
    };
    assert!(!validate_template("huge", &huge).is_ok());
}

#[test]
fn duplicate_packages_follow_configured_severity() {
    let source = r#"
[templates.first]
packages = ["com.example.shared", "com.example.only"]

[templates.second]
packages = ["com.example.shared"]
"#;
    let config = Config::from_toml(source).unwrap();
    let duplicates = duplicate_packages(&config);
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0].package, "com.example.shared");
    assert_eq!(duplicates[0].templates, vec!["first", "second"]);

    let lenient = validate_config(&config, &ValidationOptions::default());
    assert!(lenient.is_ok(), "默认重复包名只是 warning");
    assert!(codes(&lenient).contains(&"package.duplicate"));
    assert_eq!(lenient.warning_count(), 1);

    let strict = validate_config(
        &config,
        &ValidationOptions {
            duplicate_package_severity: Severity::Error,
        },
    );
    assert!(!strict.is_ok(), "严格模式下重复包名是 error");
}

#[test]
fn app_overrides_template_is_reported() {
    let source = r#"
[templates.pixel]
packages = ["com.example.app"]
brand = "google"

[[apps]]
package = "com.example.app"
brand = "inline"

[[apps]]
package = "com.example.app"
brand = "duplicate"
"#;
    let config = Config::from_toml(source).unwrap();
    let report = validate_config(&config, &ValidationOptions::default());
    assert!(codes(&report).contains(&"package.app_overrides_template"));
    assert!(codes(&report).contains(&"package.duplicate_app"));
    assert!(report.is_ok());
}

#[test]
fn app_validation_requires_package() {
    let app = AppConfig {
        package: String::new(),
        ..Default::default()
    };
    let report = validate_app(&app);
    assert!(!report.is_ok());
    assert!(codes(&report).contains(&"package.invalid"));
}

#[test]
fn unknown_fields_are_detected_but_kept() {
    let source = r#"
# 顶层未知表
[cpu_presets.kirin]
name = "Kirin 9000"

[templates.pixel]
model = "Pixel 9"
author = "someone"
vendor_extra = "keep me"

[[apps]]
package = "com.example.app"
brand = "google"
extra_app_key = 1
"#;
    let document = parse_document(source).unwrap();
    let fields = unknown_fields(&document);
    let keys: Vec<(&str, &str)> = fields
        .iter()
        .map(|field| (field.path.as_str(), field.key.as_str()))
        .collect();
    assert!(keys.contains(&("config", "cpu_presets")));
    assert!(
        !keys.iter().any(|(_, key)| *key == "author"),
        "author 是项目约定字段"
    );
    assert!(keys.contains(&("templates.pixel", "vendor_extra")));
    assert!(keys.contains(&("apps[0]", "extra_app_key")));

    let report = validate_document(&document, &ValidationOptions::default());
    assert!(report.is_ok());
    let unknown_count = report
        .issues
        .iter()
        .filter(|issue| issue.code == "config.unknown_field")
        .count();
    assert_eq!(unknown_count, fields.len());
}

#[test]
fn validate_document_reports_parse_errors() {
    let document = parse_document("[templates.x]\nmodel = 1\n").unwrap();
    // model 是字符串字段，整数会让 Config 反序列化失败
    let report = validate_document(&document, &ValidationOptions::default());
    assert!(!report.is_ok());
    assert!(codes(&report).contains(&"config.parse"));
}

#[test]
fn config_schema_describes_toml_shape() {
    let schema = config_schema();
    let properties = &schema["properties"];
    assert!(properties.get("templates").is_some());
    assert!(properties.get("apps").is_some());
    assert!(properties.get("debug").is_some());

    let template = template_schema();
    let template_properties = &template["properties"];
    for field in [
        "manufacturer",
        "brand",
        "model",
        "fingerprint",
        "dpi",
        "sdk_int",
        "custom_props",
        "packages",
    ] {
        assert!(
            template_properties.get(field).is_some(),
            "模板 schema 缺少字段 {field}"
        );
    }
    // sdk_int 通过 $ref 指向 NumberSetting，后者用 anyOf 描述「整数 或 __DELETE__」
    let sdk_int = template_properties["sdk_int"].to_string();
    assert!(
        sdk_int.contains("NumberSetting"),
        "sdk_int schema: {sdk_int}"
    );
    let number_setting = template["definitions"]["NumberSetting"]["anyOf"]
        .as_array()
        .expect("NumberSetting 必须有 anyOf");
    assert!(
        number_setting
            .iter()
            .any(|branch| branch["type"] == "integer")
    );
    assert!(
        number_setting
            .iter()
            .any(|branch| branch["const"] == "__DELETE__")
    );

    let app = app_schema();
    assert!(app["properties"].get("package").is_some());
    assert!(
        app["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|value| value == "package"))
    );

    let bundle = bundle();
    assert!(bundle["definitions"]["config"].is_object());
    assert!(bundle["definitions"]["template"].is_object());
    assert!(bundle["definitions"]["app"].is_object());
}
