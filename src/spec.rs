// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 预渲染的 per-package TOML 配置（全内存，不落盘）。
//!
//! Zygisk Next 1.5.3 起，模块的 `pre_app_specialize` 运行在 zygote 域，读 `/data/adb`
//! （`adb_data_file`）会被 SELinux 拒绝。为不改动系统策略，改由 companion（root 域）
//! 把用户配置**按包预合并**成 TOML 文本放在内存里，模块每次启动通过 companion IPC
//! 取自己那份；不产生任何文件、不需要任何 sepolicy 规则。
//!
//! 设计要点：
//! - companion 持有 `HashMap<pkg, TOML>`，`config.toml` 的 mtime 变化时重渲染；
//! - 重渲染由 inotify 事件驱动（不轮询）；
//! - 模块侧只做一次 `GetSpec{keys}` 往返 + 一次 TOML 解析，无磁盘 IO。

use crate::config::{Config, MergedAppConfig, NumberSetting};
use crate::egress::{self, EgressProxy};
use log::{info, warn};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// 已渲染的 spec 表：包名 → TOML 文本。
static SPECS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
/// 最近一次渲染依据的 mtime 快照（config.toml + egress.toml）。
static LAST_RENDER: Mutex<Option<RenderStamp>> = Mutex::new(None);
static WATCHER: OnceLock<()> = OnceLock::new();

/// 按顺序返回第一个命中的 spec 文本（`pkg@uid` → 裸包名）。
pub fn lookup_first(keys: &[String]) -> Option<String> {
    let guard = SPECS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let table = guard.as_ref()?;
    keys.iter().find_map(|key| table.get(key).cloned())
}

fn push_opt(table: &mut toml::map::Map<String, toml::Value>, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        table.insert(key.to_string(), toml::Value::String(value.clone()));
    }
}

fn app_table(merged: &MergedAppConfig, package: &str) -> toml::Value {
    let mut app = toml::map::Map::new();
    app.insert(
        "package".to_string(),
        toml::Value::String(package.to_string()),
    );

    push_opt(&mut app, "manufacturer", &merged.manufacturer);
    push_opt(&mut app, "brand", &merged.brand);
    push_opt(&mut app, "marketname", &merged.marketname);
    push_opt(&mut app, "model", &merged.model);
    push_opt(&mut app, "name", &merged.name);
    push_opt(&mut app, "device", &merged.device);
    push_opt(&mut app, "product", &merged.product);
    push_opt(&mut app, "hardware", &merged.hardware);
    push_opt(&mut app, "board", &merged.board);
    push_opt(&mut app, "soc_model", &merged.soc_model);
    push_opt(&mut app, "fingerprint", &merged.fingerprint);
    push_opt(&mut app, "build_id", &merged.build_id);
    push_opt(&mut app, "display_id", &merged.display_id);
    push_opt(&mut app, "incremental", &merged.incremental);
    push_opt(&mut app, "security_patch", &merged.security_patch);
    push_opt(&mut app, "characteristics", &merged.characteristics);
    push_opt(&mut app, "android_version", &merged.android_version);

    match &merged.sdk_int {
        Some(NumberSetting::Value(value)) => {
            app.insert(
                "sdk_int".to_string(),
                toml::Value::Integer(i64::from(*value)),
            );
        }
        Some(NumberSetting::Delete) => {
            app.insert(
                "sdk_int".to_string(),
                toml::Value::String("__DELETE__".to_string()),
            );
        }
        None => {}
    }

    if let Some(dpi) = merged.dpi {
        app.insert("dpi".to_string(), toml::Value::Integer(i64::from(dpi)));
    }

    if let Some(props) = &merged.custom_props {
        let mut table = toml::map::Map::new();
        for (key, value) in props {
            // 派生键（指纹解析出的 ro.build.type/tags）不写进 spec：模块解析 spec 后
            // 会再用 fingerprint 重新派生一次，写成显式条目会改变 patch-only 门控语义。
            if merged.derived_props.contains(key) {
                continue;
            }
            table.insert(key.clone(), toml::Value::String(value.clone()));
        }
        if !table.is_empty() {
            app.insert("custom_props".to_string(), toml::Value::Table(table));
        }
    }

    app.insert(
        "force_denylist_unmount".to_string(),
        toml::Value::Boolean(merged.force_denylist_unmount),
    );
    app.insert(
        "hide_maps".to_string(),
        toml::Value::Array(
            merged
                .hide_maps
                .iter()
                .map(|value| toml::Value::String(value.clone()))
                .collect(),
        ),
    );
    app.insert(
        "companion_resetprop".to_string(),
        toml::Value::Boolean(merged.companion_resetprop),
    );

    toml::Value::Table(app)
}

fn render_doc(
    config: &Config,
    merged: &MergedAppConfig,
    package: &str,
    proxy: Option<&EgressProxy>,
) -> String {
    let mut root = toml::map::Map::new();
    root.insert("debug".to_string(), toml::Value::Boolean(config.debug));
    root.insert(
        "default_force_denylist_unmount".to_string(),
        toml::Value::Boolean(config.default_force_denylist_unmount),
    );
    root.insert(
        "default_hide_maps".to_string(),
        toml::Value::Array(
            config
                .default_hide_maps
                .iter()
                .map(|value| toml::Value::String(value.clone()))
                .collect(),
        ),
    );
    root.insert(
        "apps".to_string(),
        toml::Value::Array(vec![app_table(merged, package)]),
    );
    // 出口代理（方案 B，契约 §5.1）：**只有真的解析出代理才写这个小节**。
    // 「未配置」必须表现为「没有 [egress_proxy]」，模块侧据此完全跳过注入——
    // 老配置（没有 [egress] / [[apps]].egress）的 spec 与改动前逐字节一致。
    if let Some(proxy) = proxy {
        egress::insert_spec_section(&mut root, proxy);
    }
    toml::to_string(&toml::Value::Table(root)).unwrap_or_default()
}

/// 把所有配置包渲染成 `pkg → TOML` 的内存表。
///
/// `egress_file` 由调用方读一次（[egress::load_egress]）后传进来：代理定义只解析一遍，
/// 「带凭据 → 本地中继 / fail-closed 哨兵」的改写（[egress::spec_proxies]）也只算一遍，
/// 因此回退与 fail-closed 的告警每次重渲染只出现一次（而不是每个包一条）。
fn render_map(config: &Config, egress_file: &egress::EgressFile) -> HashMap<String, String> {
    let mut packages: Vec<String> = config.apps.iter().map(|app| app.package.clone()).collect();
    for template in config.templates.values() {
        packages.extend(template.packages.iter().cloned());
    }
    packages.sort();
    packages.dedup();

    // 代理视图（含 relay 改写与降级日志）只算一次：egress.toml 是 0600，只有 companion
    // 所在的 root 域读得到；模块进程不碰这个文件，见 egress 模块的文档。
    let view = egress::spec_proxies(egress_file);
    for line in &view.warnings {
        // 只报代理名与上游地址，不含凭据（契约 §3.3 日志边界）。
        warn!("{line}");
    }

    let mut table = HashMap::with_capacity(packages.len());
    for package in packages {
        if let Some(merged) = config.get_merged_config(&package) {
            let proxy = egress::resolve_for_package(config, &view.proxies, &package);
            table.insert(
                package.clone(),
                render_doc(config, &merged, &package, proxy.as_ref()),
            );
        }
    }
    table
}

/// 渲染依据的 mtime 快照：config.toml 必有，egress.toml 可能不存在。
#[derive(Clone, Copy, PartialEq, Eq)]
struct RenderStamp {
    config: SystemTime,
    egress: Option<SystemTime>,
}

/// 取当前的 mtime 快照；config.toml 读不到（还没写下来）时返回 `None`。
fn render_stamp() -> Option<RenderStamp> {
    let config = std::fs::metadata(crate::CONFIG_PATH)
        .ok()?
        .modified()
        .ok()?;
    let egress = std::fs::metadata(egress::EGRESS_PATH)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    Some(RenderStamp { config, egress })
}

/// companion 侧调用：config.toml / egress.toml 的 mtime 变了才重新渲染内存表。
pub fn maybe_render() {
    let Some(stamp) = render_stamp() else {
        return;
    };
    {
        let mut last = LAST_RENDER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.as_ref() == Some(&stamp) {
            return;
        }
        *last = Some(stamp);
    }

    match crate::load_config() {
        Ok(Some(config)) => {
            let egress_file = egress::load_egress();
            let table = render_map(&config, &egress_file);
            let count = table.len();
            let mut guard = SPECS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *guard = Some(table);
            info!("Rendered {count} per-package spec(s) in memory");
        }
        Ok(None) => {}
        Err(err) => {
            warn!("Spec render skipped, config load failed: {err:#}");
        }
    }
}

/// companion 侧调用一次：起一个 inotify 监听线程，配置文件被写入/替换时立刻重渲染。
/// 纯事件驱动，不做周期轮询。
pub fn ensure_watcher() {
    WATCHER.get_or_init(|| {
        std::thread::spawn(|| {
            #[cfg(target_os = "android")]
            watch_config_events();
        });
    });
}

#[cfg(target_os = "android")]
fn watch_config_events() {
    use std::os::fd::FromRawFd;

    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    if fd < 0 {
        warn!("inotify_init1 failed: {}", std::io::Error::last_os_error());
        return;
    }
    let _file = unsafe { std::fs::File::from_raw_fd(fd) };

    let mask = libc::IN_CLOSE_WRITE
        | libc::IN_MOVED_TO
        | libc::IN_MODIFY
        | libc::IN_ATTRIB
        | libc::IN_CREATE
        | libc::IN_DELETE;
    let mut watched = false;
    for dir in ["/data/adb/devmask/config", "/data/adb/devmask", "/data/adb"] {
        let Ok(path) = std::ffi::CString::new(dir) else {
            continue;
        };
        if unsafe { libc::inotify_add_watch(fd, path.as_ptr(), mask) } >= 0 {
            watched = true;
            break;
        }
    }
    if !watched {
        warn!("inotify_add_watch failed, config hot reload disabled");
        return;
    }

    let mut buf = [0u8; 4096];
    loop {
        let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if read <= 0 {
            break;
        }
        maybe_render();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::{EgressFile, EgressProxy, EgressType, RelaySettings};

    /// 固定配置：一个 `__DIRECT__`、一个未配置、一个带凭据中继、一个无凭据上游、
    /// 一个「带凭据但缺 local_port」（中继开着 → fail-closed 哨兵）。
    const CONFIG_TOML: &str = r#"
debug = false
default_force_denylist_unmount = true
default_hide_maps = ["/data/local/tmp/df"]

[[apps]]
package = "com.example.direct"
model = "Pixel 9"
brand = "google"
egress = "__DIRECT__"

[[apps]]
package = "com.example.unconfigured"
model = "Pixel 9"

[[apps]]
package = "com.example.relay"
model = "Pixel 9"
egress = "hk_resi"

[[apps]]
package = "com.example.plain"
model = "Pixel 9"
egress = "us_la"

[[apps]]
package = "com.example.legacy"
model = "Pixel 9"
egress = "hk_legacy"
"#;

    // DIRECT_SPEC / UNCONFIGURED_SPEC / PLAIN_UPSTREAM_SPEC 是 **v1.10.0 实测输出**
    //（改动前用同一份配置从 render_doc 导出）：无 [egress_proxy] 的两种情形与无凭据上游，
    // 一个字节都不许变。
    //
    // 带凭据的上游**不再**有 v1.10.0 基线：v1.2 起 spec 里根本不写 username/password
    //（§9.2 B13），回退窗口的输出与 v1.10.0 有意不同——「凭据不进应用进程」优先于兼容。
    const DIRECT_SPEC: &str = r#"debug = false
default_force_denylist_unmount = true
default_hide_maps = ["/data/local/tmp/df"]

[[apps]]
package = "com.example.direct"
brand = "google"
model = "Pixel 9"
force_denylist_unmount = true
hide_maps = ["/data/local/tmp/df"]
companion_resetprop = false

[egress_proxy]
type = "direct"
"#;

    const UNCONFIGURED_SPEC: &str = r#"debug = false
default_force_denylist_unmount = true
default_hide_maps = ["/data/local/tmp/df"]

[[apps]]
package = "com.example.unconfigured"
model = "Pixel 9"
force_denylist_unmount = true
hide_maps = ["/data/local/tmp/df"]
companion_resetprop = false
"#;

    const PLAIN_UPSTREAM_SPEC: &str = r#"debug = false
default_force_denylist_unmount = true
default_hide_maps = ["/data/local/tmp/df"]

[[apps]]
package = "com.example.plain"
model = "Pixel 9"
force_denylist_unmount = true
hide_maps = ["/data/local/tmp/df"]
companion_resetprop = false

[egress_proxy]
type = "socks5"
host = "198.51.100.7"
port = 1080
"#;

    /// relay **明确关闭**时的回退：上游 host/port，且**不带**凭据载体（v1.2，§9.2 B13）。
    const UPSTREAM_NO_CREDS_SPEC: &str = r#"debug = false
default_force_denylist_unmount = true
default_hide_maps = ["/data/local/tmp/df"]

[[apps]]
package = "com.example.relay"
model = "Pixel 9"
force_denylist_unmount = true
hide_maps = ["/data/local/tmp/df"]
companion_resetprop = false

[egress_proxy]
type = "http"
host = "203.0.113.10"
port = 8080
"#;

    /// relay 开着、文件已迁移、却缺 local_port → fail-closed 哨兵（R13）。
    const FAIL_CLOSED_SPEC: &str = r#"debug = false
default_force_denylist_unmount = true
default_hide_maps = ["/data/local/tmp/df"]

[[apps]]
package = "com.example.legacy"
model = "Pixel 9"
force_denylist_unmount = true
hide_maps = ["/data/local/tmp/df"]
companion_resetprop = false

[egress_proxy]
type = "http"
host = "127.0.0.1"
port = 9
relay = true
"#;

    /// 中继生效时的 spec（契约 §3.3 字段表：只留 type/host/port + `relay = true`）。
    const RELAY_SPEC: &str = r#"debug = false
default_force_denylist_unmount = true
default_hide_maps = ["/data/local/tmp/df"]

[[apps]]
package = "com.example.relay"
model = "Pixel 9"
force_denylist_unmount = true
hide_maps = ["/data/local/tmp/df"]
companion_resetprop = false

[egress_proxy]
type = "http"
host = "127.0.0.1"
port = 17800
relay = true
"#;

    fn config() -> Config {
        Config::from_toml(CONFIG_TOML).expect("test config parses")
    }

    fn credential_proxy(
        kind: EgressType,
        host: &str,
        port: u16,
        local_port: Option<u16>,
    ) -> EgressProxy {
        let mut proxy = EgressProxy::endpoint_proxy(kind, host, port);
        proxy.username = Some("relay-user".to_string());
        proxy.password = Some("relay-secret".to_string());
        proxy.local_port = local_port;
        proxy
    }

    fn egress_file(enabled: bool) -> EgressFile {
        let mut proxies = HashMap::new();
        proxies.insert(
            "hk_resi".to_string(),
            credential_proxy(EgressType::Http, "203.0.113.10", 8080, Some(17800)),
        );
        proxies.insert(
            "us_la".to_string(),
            EgressProxy::endpoint_proxy(EgressType::Socks5, "198.51.100.7", 1080),
        );
        proxies.insert(
            "hk_legacy".to_string(),
            credential_proxy(EgressType::Http, "203.0.113.10", 8080, None),
        );
        EgressFile {
            relay: RelaySettings { enabled },
            relay_present: true,
            proxies,
        }
    }

    /// v1.10.0 的老文件：既没有 `[relay]` 键，也没有任何 `local_port`。
    fn unmigrated_file() -> EgressFile {
        let mut proxies = HashMap::new();
        proxies.insert(
            "hk_resi".to_string(),
            credential_proxy(EgressType::Http, "203.0.113.10", 8080, None),
        );
        EgressFile {
            relay: RelaySettings::default(),
            relay_present: false,
            proxies,
        }
    }

    fn render(file: &EgressFile, package: &str) -> String {
        render_map(&config(), file)
            .remove(package)
            .unwrap_or_else(|| panic!("no spec rendered for {package}"))
    }

    #[test]
    fn direct_and_unconfigured_specs_are_byte_identical_to_v1_10_0() {
        // relay 开着也不影响：__DIRECT__ 永远直连，未配置永远没有 [egress_proxy]。
        let file = egress_file(true);
        assert_eq!(render(&file, "com.example.direct"), DIRECT_SPEC);
        assert_eq!(render(&file, "com.example.unconfigured"), UNCONFIGURED_SPEC);

        // relay 关掉之后同样逐字节一致（两种 relay 状态都不改变这两类包的输出）。
        let off = egress_file(false);
        assert_eq!(render(&off, "com.example.direct"), DIRECT_SPEC);
        assert_eq!(render(&off, "com.example.unconfigured"), UNCONFIGURED_SPEC);
    }

    #[test]
    fn credentialed_proxy_is_rewritten_to_the_local_relay_spec() {
        let document = render(&egress_file(true), "com.example.relay");

        assert_eq!(document, RELAY_SPEC);
        // 三项硬要求：回环 host、local_port、relay 标记
        assert!(document.contains("host = \"127.0.0.1\""), "{document}");
        assert!(document.contains("port = 17800"), "{document}");
        assert!(document.contains("relay = true"), "{document}");
        // 凭据与上游地址一个字节都不许出现在 spec 里
        assert!(!document.contains("relay-user"), "{document}");
        assert!(!document.contains("relay-secret"), "{document}");
        assert!(!document.contains("203.0.113.10"), "{document}");
        assert!(!document.contains("127.0.0.1:0"), "{document}");

        // 带 relay 的 spec 仍然是合法 Config（模块侧 Config::from_toml 要能解析）
        let parsed = Config::from_toml(&document).expect("relay spec still parses as Config");
        assert_eq!(parsed.apps.len(), 1);
        assert_eq!(parsed.apps[0].package, "com.example.relay");
    }

    #[test]
    fn proxy_without_credentials_keeps_the_upstream_address() {
        let document = render(&egress_file(true), "com.example.plain");
        assert_eq!(
            document, PLAIN_UPSTREAM_SPEC,
            "无凭据必须与 v1.10.0 逐字节一致"
        );
        assert!(!document.contains("relay"), "{document}");
        assert!(!document.contains("127.0.0.1"), "{document}");

        // relay 关闭时输出不变（无凭据从来不参与中继）
        assert_eq!(
            render(&egress_file(false), "com.example.plain"),
            PLAIN_UPSTREAM_SPEC
        );
    }

    #[test]
    fn local_port_missing_while_relay_enabled_fails_closed() {
        // R13：中继开着、文件已迁移（本 fixture 有 [relay]），缺 local_port = 故障。
        let file = egress_file(true);
        let document = render(&file, "com.example.legacy");

        assert_eq!(document, FAIL_CLOSED_SPEC);
        assert!(document.contains("host = \"127.0.0.1\""), "{document}");
        assert!(document.contains("port = 9"), "{document}");
        assert!(document.contains("relay = true"), "{document}");
        // 绝不回退上游、也绝不写 0 端口
        assert!(!document.contains("203.0.113.10"), "{document}");
        assert!(!document.contains("127.0.0.1:0"), "{document}");

        // 必须留一行 warn，且不含凭据原文
        let view = egress::spec_proxies(&file);
        assert_eq!(view.warnings.len(), 1, "{:?}", view.warnings);
        let line = &view.warnings[0];
        assert!(line.contains("fail-closed"), "{line}");
        assert!(line.contains("port_missing"), "{line}");
        assert!(line.contains("hk_legacy"), "{line}");
        assert!(!line.contains("203.0.113.10"), "{line}");
        assert!(!line.contains("relay-secret"), "{line}");
        assert!(!line.contains("relay-user"), "{line}");
    }

    #[test]
    fn relay_disabled_falls_back_upstream_without_credentials() {
        // 同一条带凭据定义：relay 明确关掉 → 回退上游，且**不写凭据载体**（v1.2，§9.2 B13）。
        let document = render(&egress_file(false), "com.example.relay");
        assert_eq!(document, UPSTREAM_NO_CREDS_SPEC);
        assert!(!document.contains("relay = true"), "{document}");
        assert!(!document.contains("username"), "{document}");
        assert!(!document.contains("password"), "{document}");
        assert!(!document.contains("relay-user"), "{document}");
        assert!(!document.contains("relay-secret"), "{document}");
    }

    #[test]
    fn unmigrated_egress_file_falls_back_upstream_without_credentials() {
        // v1.10.0 的老文件（无 [relay]、无 local_port）：中继视为明确未启用 → 回退上游。
        // 这是 R13 唯一保留的可用性窗口；且同样不写凭据载体。
        let file = unmigrated_file();
        let document = render(&file, "com.example.relay");
        assert_eq!(document, UPSTREAM_NO_CREDS_SPEC);
        assert!(!document.contains("relay = true"), "{document}");
        assert!(!document.contains("username"), "{document}");

        let view = egress::spec_proxies(&file);
        assert_eq!(view.warnings.len(), 1, "{:?}", view.warnings);
        assert!(
            view.warnings[0].contains("not migrated"),
            "{:?}",
            view.warnings
        );
        // 未迁移不是故障：绝不能断网
        assert!(!document.contains("port = 9"), "{document}");
    }

    #[test]
    fn relay_never_rewrites_direct_or_undefined_proxies() {
        // 未定义代理名：不注入、也没有 [egress_proxy]（既有语义）
        let config = Config::from_toml(
            r#"
            [[apps]]
            package = "com.example.missing"
            egress = "nope_proxy"
            "#,
        )
        .expect("config");
        let specs = render_map(&config, &egress_file(true));
        let document = &specs["com.example.missing"];
        assert!(!document.contains("[egress_proxy]"), "{document}");
    }

    #[test]
    fn contract_sample_egress_toml_drives_the_spec_end_to_end() {
        // 直接吃契约 §2.1 的样例文件：解析 → spec 视图 → 渲染，验证 local_port 真的
        // 决定了注入地址（不是只测内部视图）。
        let file = egress::parse_egress(
            r#"
version = 1

[relay]
enabled                  = true
base_port                = 17800
firewall                 = true
max_connections          = 256
max_connections_per_uid  = 64

[proxies.hk_resi]
type       = "http"
host       = "203.0.113.10"
port       = 8080
username   = "user"
password   = "<PASSWORD>"
local_port = 17800

[proxies.jp_socks]
type       = "socks5"
host       = "198.51.100.20"
port       = 1080
username   = "u2"
password   = "p2"
local_port = 17831

[proxies.us_la_01]
type = "http"
host = "203.0.113.10"
port = 1080
"#,
        );
        assert!(file.relay.enabled);

        let config = Config::from_toml(
            r#"
            [[apps]]
            package = "com.example.hk"
            egress = "hk_resi"

            [[apps]]
            package = "com.example.jp"
            egress = "jp_socks"

            [[apps]]
            package = "com.example.us"
            egress = "us_la_01"
            "#,
        )
        .expect("config");
        let specs = render_map(&config, &file);

        // http 定义 → 本地 HTTP 中继端口，凭据与上游地址都不出现
        let http = &specs["com.example.hk"];
        assert!(http.contains("host = \"127.0.0.1\""), "{http}");
        assert!(http.contains("port = 17800"), "{http}");
        assert!(http.contains("relay = true"), "{http}");
        assert!(!http.contains("<PASSWORD>"), "{http}");
        assert!(!http.contains("203.0.113.10"), "{http}");

        // socks5 定义 → 该定义自己的 local_port（17831，不是 base_port）
        let socks = &specs["com.example.jp"];
        assert!(socks.contains("type = \"socks5\""), "{socks}");
        assert!(socks.contains("host = \"127.0.0.1\""), "{socks}");
        assert!(socks.contains("port = 17831"), "{socks}");
        assert!(socks.contains("relay = true"), "{socks}");
        assert!(!socks.contains("198.51.100.20"), "{socks}");

        // 无凭据定义 → 上游直连，**不**因为文件里有 relay 段就被改写
        let plain = &specs["com.example.us"];
        assert!(plain.contains("host = \"203.0.113.10\""), "{plain}");
        assert!(plain.contains("port = 1080"), "{plain}");
        assert!(!plain.contains("127.0.0.1"), "{plain}");
        assert!(!plain.contains("relay = true"), "{plain}");
    }
}
