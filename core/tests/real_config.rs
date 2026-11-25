// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 用 adb pull 下来的真实 config.toml 做解析 / 回写语义对比。
//!
//! 真机样本不进仓库（含用户模板），通过环境变量传入：
//!
//!     adb pull /data/adb/devmask/config/config.toml /tmp/df_real_config.toml
//!     DM_REAL_CONFIG=/tmp/df_real_config.toml cargo test -p devmask_core --test real_config -- --nocapture
//!
//! 未设置该变量时本测试直接跳过，保证 @@cargo test -p devmask_core@@ 在干净环境里全绿。

use std::env;
use std::fs;

use devmask_core::config::{Config, DeviceTemplate};
use devmask_core::edit::{bind_package, delete_app, parse_document, set_app, upsert_template};
use devmask_core::validate::{ValidationOptions, unknown_fields, validate_document};

fn real_config() -> Option<String> {
    let path = env::var("DM_REAL_CONFIG").ok()?;
    Some(fs::read_to_string(&path).expect("DM_REAL_CONFIG 指向的文件必须可读"))
}

fn template_names(config: &Config) -> Vec<String> {
    config.templates.keys().cloned().collect()
}

#[test]
fn real_config_roundtrip_is_byte_identical() {
    let Some(original) = real_config() else {
        eprintln!("DM_REAL_CONFIG 未设置，跳过真机配置对比");
        return;
    };
    let config = Config::from_toml(&original).expect("真机配置必须可解析");
    eprintln!(
        "真机配置：{} 个模板 / {} 个应用 / {} 字节",
        config.templates.len(),
        config.apps.len(),
        original.len()
    );

    // 1) 用解析结果整体回写：必须逐字节一致（含注释、空行、键顺序、未知字段）。
    let mut document = parse_document(&original).expect("真机配置必须可编辑");
    let before = document.to_string();
    for (name, template) in &config.templates {
        assert!(
            !upsert_template(&mut document, name, template).expect("upsert template"),
            "模板 {name} 回写时发生了意外改动"
        );
    }
    for app in &config.apps {
        assert!(
            !set_app(&mut document, app).expect("set app"),
            "应用 {} 回写时发生了意外改动",
            app.package
        );
    }
    assert_eq!(document.to_string(), before, "按模型回写必须逐字节一致");

    // 2) 语义对比：重新解析后与原配置等价，且模板书写顺序不变。
    let reread = Config::from_toml(&document.to_string()).expect("回写结果必须可解析");
    assert_eq!(reread, config);
    assert_eq!(template_names(&reread), template_names(&config));
    assert_eq!(reread.apps, config.apps);

    // 3) 未知字段保留 + 校验结果（未知字段只是 warning）。
    let unknown = unknown_fields(&document);
    eprintln!(
        "未知字段（原样保留）：{:?}",
        unknown
            .iter()
            .map(|field| (field.path.clone(), field.key.clone()))
            .collect::<Vec<_>>()
    );
    for field in &unknown {
        assert!(before.contains(&field.key), "未知字段 {} 丢失", field.key);
    }

    let report = validate_document(&document, &ValidationOptions::default());
    eprintln!(
        "校验：{} error / {} warning，codes={:?}",
        report.error_count(),
        report.warning_count(),
        report
            .issues
            .iter()
            .map(|issue| issue.code)
            .collect::<Vec<_>>()
    );
    assert!(
        report.is_ok(),
        "真机配置不应产生 error：{:?}",
        report.errors().collect::<Vec<_>>()
    );

    // 4) 做过编辑后再对比：新增条目之外，所有原有条目必须完全等价。
    let mut edited_document = parse_document(&original).expect("parse for edit");
    let new_template = DeviceTemplate {
        packages: vec!["com.example.devmask_test".to_string()],
        brand: Some("Test".to_string()),
        model: Some("Test Model".to_string()),
        sdk_int: Some(devmask_core::config::NumberSetting::Value(35)),
        dpi: Some(420),
        ..Default::default()
    };
    assert!(upsert_template(&mut edited_document, "df_roundtrip_test", &new_template).unwrap());
    assert!(
        bind_package(
            &mut edited_document,
            "df_roundtrip_test",
            "com.example.devmask_test2"
        )
        .unwrap()
    );

    let removed_app = config
        .apps
        .last()
        .map(|app| app.package.clone())
        .expect("真机配置至少有一条 apps");
    assert!(delete_app(&mut edited_document, &removed_app).unwrap());

    let edited = Config::from_toml(&edited_document.to_string()).expect("编辑结果必须可解析");
    assert_eq!(edited.templates.len(), config.templates.len() + 1);
    for (name, template) in &config.templates {
        assert_eq!(
            edited.templates.get(name),
            Some(template),
            "模板 {name} 被意外改动"
        );
    }
    let created = edited.templates.get("df_roundtrip_test").expect("新模板");
    assert_eq!(
        created.packages,
        vec![
            "com.example.devmask_test".to_string(),
            "com.example.devmask_test2".to_string()
        ]
    );
    assert_eq!(created.dpi, Some(420));
    assert_eq!(
        created.sdk_int,
        Some(devmask_core::config::NumberSetting::Value(35))
    );

    let expected_apps: Vec<_> = config
        .apps
        .iter()
        .filter(|app| app.package != removed_app)
        .cloned()
        .collect();
    assert_eq!(edited.apps, expected_apps);

    // 未知字段与注释在编辑后仍然存在。
    for field in &unknown {
        if field.path == "config" {
            assert!(
                edited_document
                    .to_string()
                    .contains(&format!("[{}", field.key)),
                "顶层未知表 {} 丢失",
                field.key
            );
        }
    }
}

#[test]
fn real_config_store_roundtrip_creates_backup_and_keeps_tail() {
    let Some(original) = real_config() else {
        eprintln!("DM_REAL_CONFIG 未设置，跳过真机配置落盘对比");
        return;
    };

    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("config/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &original).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }

    let store = devmask_core::revision::ConfigStore::new(&path);
    let before_snapshot = store.read().expect("read snapshot");
    let config = Config::from_toml(&original).expect("parse");
    let template_name = config
        .templates
        .keys()
        .next()
        .cloned()
        .expect("至少一个模板");

    let outcome = store
        .update(|document| {
            bind_package(document, &template_name, "com.example.df_store_test")?;
            Ok(())
        })
        .expect("store update");

    assert!(outcome.changed);
    assert_eq!(
        outcome.previous_revision.as_deref(),
        Some(before_snapshot.revision.as_str())
    );
    assert_eq!(
        outcome.revision,
        devmask_core::revision::revision_of(&fs::read_to_string(&path).unwrap())
    );

    let backup = outcome.backup.expect("必须产生备份");
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        original,
        "备份内容必须是写入前的原文件"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644,
            "配置文件必须保持 0644"
        );
        assert_eq!(
            fs::metadata(store.backup_dir())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    let written = fs::read_to_string(&path).unwrap();
    let after = Config::from_toml(&written).expect("写回内容必须可解析");
    assert_eq!(after.templates.len(), config.templates.len());
    for (name, template) in &config.templates {
        if name == &template_name {
            let mut expected = template.packages.clone();
            expected.push("com.example.df_store_test".to_string());
            assert_eq!(after.templates.get(name).unwrap().packages, expected);
        } else {
            assert_eq!(
                after.templates.get(name),
                Some(template),
                "模板 {name} 被意外改动"
            );
        }
    }
    assert_eq!(after.apps, config.apps, "apps 段必须完全不变");

    // 未知顶层表仍在；新绑定仍写在 apps 段之前
    assert!(written.contains("[cpu_presets]"), "未知顶层表丢失");
    let bound_at = written.find("com.example.df_store_test").expect("新绑定");
    if let Some(apps_at) = written.find("[[apps]]") {
        assert!(bound_at < apps_at, "新增绑定必须写在 [[apps]] 之前");
    }
}
