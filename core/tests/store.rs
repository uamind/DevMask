// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! revision / 原子写 / flock / 备份轮转回归。

use std::fs;
use std::path::Path;

use device_faker_core::config::{Config, DeviceTemplate};
use device_faker_core::edit::upsert_template;
use device_faker_core::revision::{
    BACKUP_KEEP, ConfigStore, StoreError, StoreOptions, atomic_write, format_utc_compact,
    revision_of, sha256_hex,
};

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

fn template(name: &str) -> DeviceTemplate {
    DeviceTemplate {
        model: Some(name.to_string()),
        brand: Some("test".to_string()),
        ..Default::default()
    }
}

#[test]
fn sha256_matches_known_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(sha256_hex(b""), revision_of(""));
}

#[test]
fn utc_stamp_is_compact_and_monotonic() {
    assert_eq!(format_utc_compact(0), "19700101T000000Z");
    assert_eq!(format_utc_compact(1_700_000_000), "20231114T221320Z");
    assert_eq!(format_utc_compact(1_700_000_001), "20231114T221321Z");
    assert!(format_utc_compact(1_800_000_000).starts_with("2027"));
}

#[test]
fn atomic_write_creates_private_temp_and_final_mode() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("nested/config.toml");

    atomic_write(&target, b"debug = true\n", 0o644).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "debug = true\n");

    let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("device_faker.tmp"))
        .collect();
    assert!(leftovers.is_empty(), "临时文件必须清理干净: {leftovers:?}");

    #[cfg(unix)]
    assert_eq!(mode_of(&target), 0o644);
}

#[test]
fn missing_file_reads_as_empty_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(dir.path().join("config.toml"));

    let snapshot = store.read().unwrap();
    assert!(!snapshot.exists);
    assert_eq!(snapshot.text, "");
    assert_eq!(snapshot.revision, revision_of(""));
}

#[test]
fn write_creates_file_backup_and_644_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config/config.toml");
    let store = ConfigStore::new(&path);

    let first = store.write("[templates.a]\nmodel = \"1\"\n", None).unwrap();
    assert!(first.changed);
    assert_eq!(first.previous_revision, None);
    assert!(first.backup.is_none(), "首次创建没有可备份的旧文件");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "[templates.a]\nmodel = \"1\"\n"
    );

    #[cfg(unix)]
    assert_eq!(mode_of(&path), 0o644);

    let second = store
        .write("[templates.a]\nmodel = \"2\"\n", Some(&first.revision))
        .unwrap();
    assert!(second.changed);
    assert_eq!(
        second.previous_revision.as_deref(),
        Some(first.revision.as_str())
    );

    let backup = second.backup.expect("second write must create backup");
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        "[templates.a]\nmodel = \"1\"\n",
        "备份必须是写入前的内容"
    );
    #[cfg(unix)]
    {
        assert_eq!(mode_of(&backup), 0o600);
        assert_eq!(mode_of(&store.backup_dir()), 0o700);
    }
}

#[test]
fn identical_write_is_noop_and_creates_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(dir.path().join("config.toml"));
    let text = "[templates.a]\nmodel = \"1\"\n";

    let first = store.write(text, None).unwrap();
    let second = store.write(text, None).unwrap();

    assert!(first.changed);
    assert!(!second.changed);
    assert_eq!(first.revision, second.revision);
    assert!(second.backup.is_none());
    assert!(store.backups().unwrap().is_empty());
}

#[test]
fn if_match_mismatch_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(dir.path().join("config.toml"));
    let first = store.write("[templates.a]\nmodel = \"1\"\n", None).unwrap();

    store
        .write("[templates.a]\nmodel = \"2\"\n", Some(&first.revision))
        .unwrap();

    let error = store
        .write("[templates.a]\nmodel = \"3\"\n", Some(&first.revision))
        .unwrap_err();
    assert!(matches!(error, StoreError::RevisionMismatch { .. }));
    assert_eq!(
        fs::read_to_string(store.path()).unwrap(),
        "[templates.a]\nmodel = \"2\"\n",
        "冲突写入不能落盘"
    );
}

#[test]
fn backups_rotate_to_keep_limit() {
    let dir = tempfile::tempdir().unwrap();
    let options = StoreOptions {
        keep_backups: BACKUP_KEEP,
        ..Default::default()
    };
    let store = ConfigStore::with_options(dir.path().join("config.toml"), options);

    store.write("model = \"0\"\n", None).unwrap();
    for index in 1..=15 {
        store
            .write(&format!("model = \"{index}\"\n"), None)
            .unwrap();
    }

    let backups = store.backups().unwrap();
    assert_eq!(backups.len(), BACKUP_KEEP);
    // 名字按时间排序：15 份备份（内容 0..14）裁到 10 份，最早保留的应是 model = "5"
    let oldest = fs::read_to_string(&backups[0]).unwrap();
    assert_eq!(oldest, "model = \"5\"\n");
    let newest = fs::read_to_string(backups.last().unwrap()).unwrap();
    assert_eq!(newest, "model = \"14\"\n");
}

#[test]
fn update_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(dir.path().join("config.toml"));
    store.write("", None).unwrap();

    let first = store
        .update(|document| {
            upsert_template(document, "pixel", &template("Pixel 9"))?;
            Ok(())
        })
        .unwrap();
    assert!(first.changed);

    let second = store
        .update(|document| {
            upsert_template(document, "pixel", &template("Pixel 9"))?;
            Ok(())
        })
        .unwrap();
    assert!(!second.changed);
    assert_eq!(first.revision, second.revision);
}

#[test]
fn concurrent_updates_do_not_lose_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(dir.path().join("config.toml"));
    store.write("", None).unwrap();

    let mut revisions = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let store = store.clone();
                scope.spawn(move || {
                    store
                        .update(|document| {
                            let name = format!("t{index}");
                            upsert_template(document, &name, &template(&name))?;
                            Ok(())
                        })
                        .expect("concurrent update")
                })
            })
            .collect();

        for handle in handles {
            revisions.push(handle.join().expect("thread"));
        }
    });

    assert!(revisions.iter().all(|outcome| outcome.changed));
    let mut unique: Vec<&str> = revisions
        .iter()
        .map(|outcome| outcome.revision.as_str())
        .collect();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        revisions.len(),
        "每次写入都必须产生新的 revision"
    );

    let config = Config::from_toml(&fs::read_to_string(store.path()).unwrap()).unwrap();
    assert_eq!(config.templates.len(), 8, "并发读改写不能丢模板");

    // 线程 join 顺序不等于写入完成顺序，因此最终 revision 只能断言「是其中一次写入」。
    let snapshot = store.read().unwrap();
    assert!(
        revisions
            .iter()
            .any(|outcome| outcome.revision == snapshot.revision),
        "文件内容必须与某一次成功写入一致"
    );
    assert_eq!(
        snapshot.revision,
        device_faker_core::revision::revision_of(&snapshot.text)
    );
    assert!(store.backups().unwrap().len() <= BACKUP_KEEP);
}

#[test]
fn update_creates_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(dir.path().join("config.toml"));

    let outcome = store
        .update(|document| {
            upsert_template(document, "fresh", &template("Fresh"))?;
            Ok(())
        })
        .unwrap();
    assert!(outcome.changed);
    assert!(store.path().exists());
    assert!(
        Config::from_toml(&fs::read_to_string(store.path()).unwrap())
            .unwrap()
            .templates
            .contains_key("fresh")
    );
}
