// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 出口代理（egress）控制面：存储、系统代理应用、出口 IP 探测与启动自愈。
//!
//! 契约：docs/plans/egress-contract.md（§1 端点、§2 egress.toml、§3 设备级语义、
//! §4 探测、§7.2 权限与凭据边界）。本模块只做设备端一侧，与 Manager 无关。
//!
//! 模块划分：
//!
//! - [`file`]：egress.toml 模型、解析/渲染与 0600 原子写存储（flock + 备份）；
//! - [`system`]：Android settings 系统代理的**唯一**应用/清除实现与读回判定真值表；
//! - [`probe`]：最小 HTTP/1.1 客户端（设备无 curl/wget），支持 HTTP 代理与 SOCKS5；
//! - [`request`]：PUT /egress 请求体、字段校验与「省略 = 不改」的合并规则；
//! - [`state`]：内存健康状态（health/lastProbe）、启动自愈与 fail-open 动作；
//! - [`dto`]：camelCase 响应视图（**没有任何凭据字段**）。
//!
//! 硬约束（与契约一一对应）：
//!
//! 1. `egress.toml` 是 0600、`O_CREAT|O_EXCL` 一步创建、tmp→fsync→rename 落盘、
//!    读时自愈权限、备份同样 0600；
//! 2. 清除系统代理只有 [`system::SystemProxy::clear`] 一份实现（写 `:0`，**禁止** settings delete）；
//! 3. 密码不进响应 / 日志 / 审计 / 错误 details，响应只有 `hasCredentials`；
//! 4. egress 端点不设置 `X-DF-Revision` / `ETag`，只用 `X-DF-Egress-Revision`。

pub mod dto;
pub mod file;
pub mod probe;
pub mod request;
pub mod state;
pub mod system;

use device_faker_core::config::{Config, DIRECT_MARKER};
use device_faker_core::revision::sha256_hex;
use toml_edit::{DocumentMut, Item};

pub use file::{
    EgressError, EgressFile, EgressSnapshot, EgressStore, EgressWriteOutcome, ProbeSettings,
    ProxyDefinition, ProxyType,
};
pub use request::{AppEgressInput, EgressUpdateRequest, ProxyInput};
pub use state::{
    EgressRuntime, EgressService, HealthSnapshot, LastProbe, ReconcileReport, reconcile_once,
    spawn_startup_reconcile,
};
pub use system::{ApplyOutcome, Expected, Readback, SettingsError, SystemProxy};

/// 探测 URL 默认值（契约 §4.2：明文 HTTP，避免引入 TLS 栈）。
pub const DEFAULT_PROBE_URL: &str = "http://checkip.amazonaws.com/";
/// 探测超时默认值（秒）。
pub const DEFAULT_PROBE_TIMEOUT_SECONDS: u64 = 5;
/// 探测超时硬上限（秒）。
pub const MAX_PROBE_TIMEOUT_SECONDS: u64 = 30;
/// 代理定义条数上限。
pub const MAX_PROXIES: usize = 64;
/// 排除列表条数上限。
pub const MAX_EXCLUSIONS: usize = 32;
/// 排除列表单条长度上限。
pub const MAX_EXCLUSION_LEN: usize = 64;
/// probe URL 长度上限。
pub const MAX_PROBE_URL_LEN: usize = 512;
/// 代理 host 长度上限。
pub const MAX_HOST_LEN: usize = 253;
/// 代理用户名长度上限。
pub const MAX_USERNAME_LEN: usize = 128;
/// 代理密码长度上限。
pub const MAX_PASSWORD_LEN: usize = 256;
/// egress.toml 文件名（与 config.toml 同目录）。
pub const EGRESS_FILE_NAME: &str = "egress.toml";
/// egress.toml 权限：0600（与 config.toml 的 0644 刻意不同）。
pub const EGRESS_FILE_MODE: u32 = 0o600;
/// 备份保留份数。
pub const EGRESS_KEEP_BACKUPS: usize = 10;
/// config.toml 权限（沿用既有语义）。
pub const CONFIG_FILE_MODE: u32 = 0o644;

/// egress revision = sha256(configRevisionHex ‖ "\n" ‖ egress.toml bytes)（契约 §1.2）。
///
/// 客户端把结果当不透明字符串使用；服务端每次都用当前两个文件的真实内容重算。
pub fn egress_revision(config_revision: &str, egress_bytes: &[u8]) -> String {
    let mut material = Vec::with_capacity(config_revision.len() + 1 + egress_bytes.len());
    material.extend_from_slice(config_revision.as_bytes());
    material.push(b'\n');
    material.extend_from_slice(egress_bytes);
    sha256_hex(&material)
}

/// 设备级（方案 A）默认代理的**定义**；未配置 default、default 为空/__DIRECT__、或定义
/// 缺失时返回 None（契约 §2.3 硬约束 2：允许引用未定义的名字，运行期按直连处理）。
pub fn default_proxy<'a>(config: &Config, model: &'a EgressFile) -> Option<&'a ProxyDefinition> {
    let name = config.egress.as_ref()?.default.as_deref()?;
    if name.trim().is_empty() || name == DIRECT_MARKER {
        return None;
    }
    model.proxy(name)
}

/// 设备级期望链路：default 能解析成定义就是代理，否则直连。
pub fn expected_for(config: &Config, model: &EgressFile) -> Expected {
    match default_proxy(config, model) {
        Some(proxy) => Expected::Proxy {
            host: proxy.host.clone(),
            port: proxy.port,
        },
        None => Expected::Direct,
    }
}

/// 在 config.toml 的 [[apps]] 上定点设置/删除 egress 键（契约 §2.4）。
///
/// - Some(名字)：改已有记录的该键，或在数组末尾追加一条只含 package + egress 的内联记录；
/// - None：从已有记录删除该键；没有记录时是空操作。
///
/// 其它字段、注释与键顺序原样保留（只动 egress 一个键）。返回实际改动数。
pub fn apply_app_egress(document: &mut DocumentMut, entries: &[(String, Option<String>)]) -> usize {
    let mut changed = 0usize;
    let mut missing: Vec<&(String, Option<String>)> = Vec::new();
    if let Some(apps) = document
        .get_mut("apps")
        .and_then(Item::as_array_of_tables_mut)
    {
        for entry in entries {
            let mut hit = false;
            for table in apps.iter_mut() {
                let Some(package) = table.get("package").and_then(Item::as_str) else {
                    continue;
                };
                if package != entry.0 {
                    continue;
                }
                hit = true;
                changed += set_table_string(table, "egress", entry.1.as_deref());
            }
            if !hit {
                missing.push(entry);
            }
        }
    } else {
        missing.extend(entries.iter());
    }
    for entry in missing {
        let Some(value) = entry.1.as_deref() else {
            continue;
        };
        let mut table = toml_edit::Table::new();
        table.insert("package", toml_edit::value(entry.0.clone()));
        table.insert("egress", toml_edit::value(value));
        ensure_apps(document).push(table);
        changed += 1;
    }
    changed
}

/// 清除**所有** [[apps]].egress（DELETE /egress 清单第 3 条），返回改动数。
pub fn clear_all_app_egress(document: &mut DocumentMut) -> usize {
    let Some(apps) = document
        .get_mut("apps")
        .and_then(Item::as_array_of_tables_mut)
    else {
        return 0;
    };
    let mut changed = 0usize;
    for table in apps.iter_mut() {
        if table.remove("egress").is_some() {
            changed += 1;
        }
    }
    changed
}

fn set_table_string(table: &mut toml_edit::Table, key: &str, value: Option<&str>) -> usize {
    match value {
        Some(value) => {
            if table.get(key).and_then(Item::as_str) == Some(value) {
                return 0;
            }
            table.insert(key, toml_edit::value(value));
            1
        }
        None => usize::from(table.remove(key).is_some()),
    }
}

fn ensure_apps(document: &mut DocumentMut) -> &mut toml_edit::ArrayOfTables {
    if document
        .get("apps")
        .is_none_or(|item| !item.is_array_of_tables())
    {
        document.insert("apps", Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    }
    document
        .get_mut("apps")
        .and_then(Item::as_array_of_tables_mut)
        .expect("apps 刚插入为数组表")
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_faker_core::config::EgressSection;

    #[test]
    fn revision_is_sha256_of_config_revision_newline_and_bytes() {
        let config_revision = "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5";
        let bytes = b"version = 1\n";
        let mut material = Vec::new();
        material.extend_from_slice(config_revision.as_bytes());
        material.push(b'\n');
        material.extend_from_slice(bytes);
        assert_eq!(
            egress_revision(config_revision, bytes),
            sha256_hex(&material)
        );
        assert_ne!(
            egress_revision(config_revision, bytes),
            egress_revision(config_revision, b"version = 1\n\n")
        );
        assert_ne!(
            egress_revision(config_revision, bytes),
            egress_revision("00", bytes)
        );
    }

    #[test]
    fn app_egress_is_set_and_removed_in_place() {
        let mut document: DocumentMut =
            "# 注释要保留\n[[apps]]\npackage = \"com.example.a\"\nmodel = \"Pixel 9\"\n"
                .parse()
                .unwrap();
        assert_eq!(
            apply_app_egress(
                &mut document,
                &[("com.example.a".to_owned(), Some("jp_tokyo".to_owned()))]
            ),
            1
        );
        let text = document.to_string();
        assert!(text.contains("# 注释要保留"), "{text}");
        assert!(text.contains("model = \"Pixel 9\""), "{text}");
        assert!(text.contains("egress = \"jp_tokyo\""), "{text}");

        assert_eq!(
            apply_app_egress(
                &mut document,
                &[("com.example.a".to_owned(), Some("jp_tokyo".to_owned()))]
            ),
            0
        );
        assert_eq!(
            apply_app_egress(&mut document, &[("com.example.a".to_owned(), None)]),
            1
        );
        assert!(!document.to_string().contains("egress"));
    }

    #[test]
    fn app_egress_creates_inline_record_for_unconfigured_package() {
        let mut document: DocumentMut = "debug = false\n".parse().unwrap();
        assert_eq!(
            apply_app_egress(
                &mut document,
                &[("com.example.b".to_owned(), Some(DIRECT_MARKER.to_owned()))]
            ),
            1
        );
        let text = document.to_string();
        assert!(text.contains("[[apps]]"), "{text}");
        assert!(text.contains("package = \"com.example.b\""), "{text}");
        assert!(text.contains("egress = \"__DIRECT__\""), "{text}");
    }

    #[test]
    fn clear_all_app_egress_only_touches_egress() {
        let mut document: DocumentMut = "[[apps]]\npackage = \"com.example.a\"\negress = \"jp_tokyo\"\nmodel = \"Pixel 9\"\n\n[[apps]]\npackage = \"com.example.b\"\nmodel = \"Pixel 8\"\n"
            .parse()
            .unwrap();
        assert_eq!(clear_all_app_egress(&mut document), 1);
        let text = document.to_string();
        assert!(!text.contains("egress"), "{text}");
        assert!(text.contains("model = \"Pixel 9\""), "{text}");
        assert!(text.contains("model = \"Pixel 8\""), "{text}");
    }

    #[test]
    fn expected_channel_follows_config_default() {
        let model = EgressFile::for_test("us_la_01", "203.0.113.10", 1080);
        let mut config = Config::from_toml("").expect("空配置可解析");
        assert_eq!(expected_for(&config, &model), Expected::Direct);
        config.egress = Some(EgressSection {
            default: Some("us_la_01".to_owned()),
        });
        assert_eq!(
            expected_for(&config, &model),
            Expected::Proxy {
                host: "203.0.113.10".to_owned(),
                port: 1080
            }
        );
        config.egress = Some(EgressSection {
            default: Some("missing".to_owned()),
        });
        assert_eq!(expected_for(&config, &model), Expected::Direct);
    }
}
