// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 文档级编辑回归：注释、空行、键顺序、未知字段必须原样保留；
//! 重复执行同一操作必须是无操作（幂等）。

use device_faker_core::config::{AppConfig, Config, DeviceTemplate};
use device_faker_core::edit::{
    EditError, bind_package, delete_app, delete_template, parse_document, set_app, unbind_package,
    upsert_template,
};

/// 随模块发布的默认配置：带大量注释与多行数组，是最贴近用户的写路径样本。
const MODULE_CONFIG: &str = include_str!("../../module/config.toml");

const SMALL: &str = r#"# 顶部注释：必须保留
debug = false   # 行内注释

[templates.pixel]
# 包名列表注释
packages = [ "com.example.one", "com.example.two" ]
model = "Pixel 9"     # 机型
author = "someone"

[templates."一加15"]
model = "PLK110"

[templates.with_props]
model = "Props"

[templates.with_props.custom_props]
# 属性注释
"ro.product.model" = "Keep"   # 行内
"ro.build.type" = "user"

[[apps]]
package = "com.example.one"
brand = "google"   # 应用注释

# 未知顶层表
[cpu_presets.kirin]
name = "Kirin 9000"
"#;

fn comment_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('#'))
        .collect()
}

fn upsert_all(document: &mut toml_edit::DocumentMut, config: &Config) -> bool {
    let mut changed = false;
    for (name, template) in &config.templates {
        changed |= upsert_template(document, name, template).expect("upsert template");
    }
    for app in &config.apps {
        changed |= set_app(document, app).expect("set app");
    }
    changed
}

#[test]
fn module_config_rewrite_is_byte_identical() {
    let config = Config::from_toml(MODULE_CONFIG).expect("shipped config must parse");
    let mut document = parse_document(MODULE_CONFIG).expect("parse shipped config");
    let changed = upsert_all(&mut document, &config);

    assert!(!changed, "把解析结果原样写回必须是无操作");
    assert_eq!(document.to_string(), MODULE_CONFIG);
}

#[test]
fn upsert_new_template_keeps_comments_and_unknown_fields() {
    let mut document = parse_document(SMALL).unwrap();
    let template = DeviceTemplate {
        packages: vec!["com.example.brand_new".to_string()],
        brand: Some("samsung".to_string()),
        model: Some("SM-S9280".to_string()),
        ..Default::default()
    };

    assert!(upsert_template(&mut document, "brand_new", &template).unwrap());
    let text = document.to_string();

    for comment in comment_lines(SMALL) {
        assert!(
            text.lines().map(str::trim).any(|line| line == comment),
            "注释 {comment:?} 丢失"
        );
    }
    assert!(text.contains("author = \"someone\""), "未知字段必须保留");
    assert!(text.contains("[cpu_presets.kirin]"), "未知表必须保留");
    assert!(text.contains("# 行内注释"));

    let config = Config::from_toml(&text).expect("edited document must still parse");
    assert_eq!(
        config.templates.keys().cloned().collect::<Vec<_>>(),
        vec!["pixel", "一加15", "with_props", "brand_new"]
    );
    let created = config.templates.get("brand_new").expect("new template");
    assert_eq!(created.brand.as_deref(), Some("samsung"));
    assert_eq!(created.packages, vec!["com.example.brand_new".to_string()]);

    // 幂等：第二次必须是无操作
    let mut again = parse_document(&text).unwrap();
    assert!(!upsert_template(&mut again, "brand_new", &template).unwrap());
    assert_eq!(again.to_string(), text);
}

#[test]
fn upsert_existing_template_preserves_key_order_and_inline_comment() {
    let mut document = parse_document(SMALL).unwrap();
    let mut template = Config::from_toml(SMALL)
        .unwrap()
        .templates
        .get("pixel")
        .cloned()
        .unwrap();
    template.model = Some("Pixel 10".to_string());

    assert!(upsert_template(&mut document, "pixel", &template).unwrap());
    let text = document.to_string();

    assert!(
        text.contains("model = \"Pixel 10\"     # 机型"),
        "行内注释必须跟随新值保留：\n{text}"
    );
    let packages_at = text.find("packages = [").expect("packages kept");
    let model_at = text.find("model = \"Pixel 10\"").expect("model kept");
    assert!(packages_at < model_at, "已存在的键必须保持原位");
    assert!(text.contains("author = \"someone\""), "未知字段必须保留");
}

#[test]
fn upsert_quoted_template_key_stays_quoted() {
    let mut document = parse_document(SMALL).unwrap();
    assert!(
        upsert_template(
            &mut document,
            "一加15",
            &DeviceTemplate {
                model: Some("PLK120".to_string()),
                ..Default::default()
            }
        )
        .unwrap()
    );

    let text = document.to_string();
    assert!(
        text.contains("[templates.\"一加15\"]"),
        "带非 ASCII 的模板名必须保持 TOML 引号写法：\n{text}"
    );
}

#[test]
fn upsert_replaces_known_fields_and_drops_missing_ones() {
    let mut document = parse_document(SMALL).unwrap();
    // 只留 model：packages 清空、其余已知字段删除，未知 author 保留
    assert!(
        upsert_template(
            &mut document,
            "pixel",
            &DeviceTemplate {
                model: Some("Pixel 11".to_string()),
                ..Default::default()
            }
        )
        .unwrap()
    );

    let text = document.to_string();
    let config = Config::from_toml(&text).unwrap();
    let template = config.templates.get("pixel").unwrap();
    assert_eq!(template.model.as_deref(), Some("Pixel 11"));
    assert!(template.packages.is_empty());
    assert_eq!(template.brand, None);
    assert!(text.contains("author = \"someone\""));
    assert!(
        !text.contains("packages ="),
        "空 packages 必须删除键：\n{text}"
    );
}

#[test]
fn bind_package_is_idempotent_and_keeps_indent() {
    let mut document = parse_document(MODULE_CONFIG).unwrap();
    let template = config_template("redmagic_11_pro");

    assert!(bind_package(&mut document, &template, "com.example.df_test").unwrap());
    let text = document.to_string();
    assert!(
        text.contains("\n    \"com.example.df_test\",\n"),
        "多行数组的新元素必须沿用 4 空格缩进：\n{text}"
    );
    assert_eq!(
        Config::from_toml(&text)
            .unwrap()
            .templates
            .get(&template)
            .unwrap()
            .packages
            .last()
            .map(String::as_str),
        Some("com.example.df_test")
    );

    let mut again = parse_document(&text).unwrap();
    assert!(
        !bind_package(&mut again, &template, "com.example.df_test").unwrap(),
        "重复绑定必须是无操作"
    );
    assert_eq!(again.to_string(), text);
}

#[test]
fn bind_package_creates_array_when_missing() {
    let mut document = parse_document(SMALL).unwrap();
    assert!(bind_package(&mut document, "一加15", "com.example.new").unwrap());
    let text = document.to_string();
    assert!(
        text.contains("packages = [ \"com.example.new\" ]"),
        "{text}"
    );
    assert_eq!(
        Config::from_toml(&text)
            .unwrap()
            .templates
            .get("一加15")
            .unwrap()
            .packages,
        vec!["com.example.new".to_string()]
    );
}

#[test]
fn unbind_package_removes_all_occurrences() {
    let replaced = SMALL.replace(
        "packages = [ \"com.example.one\", \"com.example.two\" ]",
        "packages = [ \"com.example.one\", \"com.example.two\", \"com.example.two\" ]",
    );
    let mut document = parse_document(&replaced).unwrap();

    assert!(unbind_package(&mut document, "pixel", "com.example.two").unwrap());
    let text = document.to_string();
    let config = Config::from_toml(&text).unwrap();
    assert_eq!(
        config.templates.get("pixel").unwrap().packages,
        vec!["com.example.one".to_string()],
        "重复项必须全部删除"
    );

    assert!(!unbind_package(&mut document, "pixel", "com.example.two").unwrap());
    // 未绑定包名不改变文档
    assert!(!unbind_package(&mut document, "pixel", "com.example.absent").unwrap());
}

#[test]
fn bind_missing_template_reports_not_found() {
    let mut document = parse_document(SMALL).unwrap();
    let error = bind_package(&mut document, "nope", "com.example.x").unwrap_err();
    assert!(matches!(error, EditError::NotFound { .. }));
    assert!(unbind_package(&mut document, "nope", "com.example.x").is_err());
}

#[test]
fn delete_template_keeps_unknown_fields_and_is_idempotent() {
    let mut document = parse_document(SMALL).unwrap();
    assert!(delete_template(&mut document, "pixel"));
    let text = document.to_string();
    assert!(!text.contains("[templates.pixel]"));
    assert!(text.contains("[cpu_presets.kirin]"));
    assert!(text.contains("[templates.\"一加15\"]"));
    assert!(!delete_template(&mut document, "pixel"));
}

#[test]
fn set_app_updates_in_place() {
    let mut document = parse_document(SMALL).unwrap();
    let mut app = Config::from_toml(SMALL).unwrap().apps[0].clone();
    app.brand = Some("samsung".to_string());

    assert!(set_app(&mut document, &app).unwrap());
    let text = document.to_string();
    assert!(text.contains("brand = \"samsung\"   # 应用注释"), "{text}");

    let config = Config::from_toml(&text).unwrap();
    assert_eq!(config.apps.len(), 1);
    assert_eq!(config.apps[0].brand.as_deref(), Some("samsung"));

    let mut again = parse_document(&text).unwrap();
    assert!(!set_app(&mut again, &app).unwrap());
}

#[test]
fn set_app_appends_new_entry() {
    let mut document = parse_document(SMALL).unwrap();
    let app = AppConfig {
        package: "com.example.added".to_string(),
        brand: Some("google".to_string()),
        ..Default::default()
    };

    assert!(set_app(&mut document, &app).unwrap());
    let text = document.to_string();
    let config = Config::from_toml(&text).unwrap();
    assert_eq!(config.apps.len(), 2);
    assert_eq!(config.apps[1].package, "com.example.added");
    assert!(text.contains("[[apps]]"));
    // 新条目追加到文档末尾（未知表之前没有 apps 段）
    assert!(text.find("com.example.added").unwrap() > text.find("com.example.one").unwrap());
}

#[test]
fn delete_app_removes_matching_entry() {
    let mut document = parse_document(SMALL).unwrap();
    assert!(delete_app(&mut document, "com.example.one").unwrap());
    let config = Config::from_toml(&document.to_string()).unwrap();
    assert!(config.apps.is_empty());
    assert!(!delete_app(&mut document, "com.example.one").unwrap());
}

#[test]
fn inline_apps_array_is_converted_to_array_of_tables() {
    let source = "apps = [ { package = \"com.example.old\", brand = \"old\" } ]\n";
    let mut document = parse_document(source).unwrap();
    let app = AppConfig {
        package: "com.example.new".to_string(),
        brand: Some("new".to_string()),
        ..Default::default()
    };

    assert!(set_app(&mut document, &app).unwrap());
    let text = document.to_string();
    assert!(text.contains("[[apps]]"), "{text}");

    let config = Config::from_toml(&text).unwrap();
    assert_eq!(config.apps.len(), 2);
    assert_eq!(config.apps[0].package, "com.example.old");
    assert_eq!(config.apps[1].package, "com.example.new");
}

#[test]
fn custom_props_section_keeps_comments_and_order() {
    let mut document = parse_document(SMALL).unwrap();
    let mut template = Config::from_toml(SMALL)
        .unwrap()
        .templates
        .get("with_props")
        .cloned()
        .unwrap();
    let props = template.custom_props.as_mut().unwrap();
    props.insert("ro.product.model".to_string(), "Changed".to_string());
    props.insert("ro.vendor.debug".to_string(), "1".to_string());
    props.remove("ro.build.type");

    assert!(upsert_template(&mut document, "with_props", &template).unwrap());
    let text = document.to_string();

    assert!(text.contains("[templates.with_props.custom_props]"));
    assert!(text.contains("# 属性注释"));
    assert!(
        text.contains("\"ro.product.model\" = \"Changed\"   # 行内"),
        "已有属性必须保留行内注释：\n{text}"
    );
    assert!(!text.contains("ro.build.type"), "模型里没有的属性要删除");

    let config = Config::from_toml(&text).unwrap();
    let props = config
        .templates
        .get("with_props")
        .unwrap()
        .custom_props
        .as_ref()
        .unwrap();
    assert_eq!(
        props.get("ro.product.model").map(String::as_str),
        Some("Changed")
    );
    assert_eq!(props.get("ro.vendor.debug").map(String::as_str), Some("1"));
}

#[test]
fn custom_props_created_as_inline_table() {
    let mut document = parse_document(SMALL).unwrap();
    let template = DeviceTemplate {
        custom_props: Some(std::collections::HashMap::from([
            ("ro.product.model".to_string(), "X".to_string()),
            ("persist.vendor.a".to_string(), "1".to_string()),
        ])),
        ..Default::default()
    };

    assert!(upsert_template(&mut document, "with_inline_props", &template).unwrap());
    let text = document.to_string();
    assert!(text.contains("custom_props"), "{text}");

    let config = Config::from_toml(&text).unwrap();
    let props = config
        .templates
        .get("with_inline_props")
        .unwrap()
        .custom_props
        .as_ref()
        .unwrap();
    assert_eq!(props.len(), 2);
}

#[test]
fn invalid_toml_is_reported() {
    let error = parse_document("[templates.x\nmodel = 1").unwrap_err();
    assert!(matches!(error, EditError::Parse { .. }));
    assert!(error.to_string().contains("invalid TOML"));
}

fn config_template(name: &str) -> String {
    name.to_string()
}

#[test]
fn creates_entries_in_empty_document() {
    let mut document = parse_document("").unwrap();
    let app = AppConfig {
        package: "com.example.first".to_string(),
        brand: Some("google".to_string()),
        ..Default::default()
    };

    assert!(set_app(&mut document, &app).unwrap());
    let text = document.to_string();
    assert!(text.starts_with("[[apps]]"), "{text}");

    let config = Config::from_toml(&text).unwrap();
    assert_eq!(config.apps.len(), 1);
    assert_eq!(config.apps[0].package, "com.example.first");
}

#[test]
fn template_name_with_dot_stays_single_key() {
    let mut document = parse_document("").unwrap();
    assert!(
        upsert_template(
            &mut document,
            "honor_pad_5_10.1",
            &DeviceTemplate {
                model: Some("ELP-AN00".to_string()),
                ..Default::default()
            }
        )
        .unwrap()
    );

    let text = document.to_string();
    assert!(text.contains("[templates.\"honor_pad_5_10.1\"]"), "{text}");

    let config = Config::from_toml(&text).unwrap();
    assert_eq!(
        config.templates.keys().cloned().collect::<Vec<_>>(),
        vec!["honor_pad_5_10.1"]
    );
    assert_eq!(
        config.templates["honor_pad_5_10.1"].model.as_deref(),
        Some("ELP-AN00")
    );
}
