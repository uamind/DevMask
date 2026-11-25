// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 设备摘要（GET /api/v1/status 的 device 块）。
//!
//! 取值优先级：环境变量覆盖 > fixture JSON > getprop > unknown。
//! 设备上只读 getprop 白名单里的 6 个键；host 测试用 --device-fixture 或
//! DF_GETPROP_BIN 指向一个假的 getprop 脚本，避免依赖真实 Android 环境。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// 读不到的字段统一用 unknown（而不是空串，便于客户端区分）。
pub const UNKNOWN: &str = "unknown";
/// Android 的 getprop 可执行文件。
pub const GETPROP: &str = "getprop";

/// 字段 -> getprop 键（白名单，见 docs/API.md 5.22）。
pub const GETPROP_KEYS: [(&str, &str); 6] = [
    ("manufacturer", "ro.product.manufacturer"),
    ("brand", "ro.product.brand"),
    ("model", "ro.product.model"),
    ("device", "ro.product.device"),
    ("android", "ro.build.version.release"),
    ("sdk", "ro.build.version.sdk"),
];

/// 设备 Build 摘要（openapi DeviceSummary）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    /// Build.MANUFACTURER
    pub manufacturer: String,
    /// Build.BRAND
    pub brand: String,
    /// Build.MODEL
    pub model: String,
    /// Build.DEVICE
    pub device: String,
    /// Build.VERSION.RELEASE
    pub android: String,
    /// Build.VERSION.SDK_INT
    pub sdk: u32,
    /// 服务运行的 ABI
    pub abi: String,
}

impl DeviceSummary {
    /// 未知设备：getprop 不可用时仍给出稳定形状。
    pub fn unknown() -> Self {
        Self {
            manufacturer: UNKNOWN.to_owned(),
            brand: UNKNOWN.to_owned(),
            model: UNKNOWN.to_owned(),
            device: UNKNOWN.to_owned(),
            android: UNKNOWN.to_owned(),
            sdk: 0,
            abi: abi().to_owned(),
        }
    }

    fn set_text(&mut self, field: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        match field {
            "manufacturer" => self.manufacturer = value.to_owned(),
            "brand" => self.brand = value.to_owned(),
            "model" => self.model = value.to_owned(),
            "device" => self.device = value.to_owned(),
            "android" => self.android = value.to_owned(),
            "sdk" => {
                if let Ok(sdk) = value.trim().parse::<u32>() {
                    self.sdk = sdk;
                }
            }
            _ => {}
        }
    }
}

/// 运行服务的 ABI：模块只构建 arm64-v8a，host 测试按目标架构映射同类名字。
pub const fn abi() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64-v8a"
    } else if cfg!(target_arch = "arm") {
        "armeabi-v7a"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        UNKNOWN
    }
}

/// 设备信息 fixture（host 测试与本地联调）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct DeviceFixture {
    /// Build.MANUFACTURER
    #[serde(default)]
    pub manufacturer: Option<String>,
    /// Build.BRAND
    #[serde(default)]
    pub brand: Option<String>,
    /// Build.MODEL
    #[serde(default)]
    pub model: Option<String>,
    /// Build.DEVICE
    #[serde(default)]
    pub device: Option<String>,
    /// Build.VERSION.RELEASE
    #[serde(default)]
    pub android: Option<String>,
    /// Build.VERSION.SDK_INT
    #[serde(default)]
    pub sdk: Option<u32>,
}

impl DeviceFixture {
    /// 叠加到设备摘要上（fixture 优先于 getprop）。
    pub fn apply(&self, summary: &mut DeviceSummary) {
        if let Some(value) = &self.manufacturer {
            summary.manufacturer = value.clone();
        }
        if let Some(value) = &self.brand {
            summary.brand = value.clone();
        }
        if let Some(value) = &self.model {
            summary.model = value.clone();
        }
        if let Some(value) = &self.device {
            summary.device = value.clone();
        }
        if let Some(value) = &self.android {
            summary.android = value.clone();
        }
        if let Some(value) = self.sdk {
            summary.sdk = value;
        }
    }
}

/// 设备信息数据源：fixture 文件 + getprop 可执行文件。
#[derive(Clone, Debug, Default)]
pub struct DeviceSource {
    /// fixture JSON 路径（可选）。
    pub fixture: Option<PathBuf>,
    /// getprop 可执行文件路径（可选；设备上默认用 PATH 里的 getprop）。
    pub getprop: Option<PathBuf>,
}

impl DeviceSource {
    /// 显式指定数据源（测试用）。
    pub fn new(fixture: Option<PathBuf>, getprop: Option<PathBuf>) -> Self {
        Self { fixture, getprop }
    }

    /// 从 --device-fixture 与 DF_DEVICE_FIXTURE / DF_GETPROP_BIN 构造。
    pub fn from_env(cli_fixture: Option<PathBuf>) -> Self {
        let fixture =
            cli_fixture.or_else(|| std::env::var_os("DF_DEVICE_FIXTURE").map(PathBuf::from));
        let getprop = std::env::var_os("DF_GETPROP_BIN").map(PathBuf::from);
        Self { fixture, getprop }
    }

    fn getprop_bin(&self) -> Option<PathBuf> {
        if let Some(path) = &self.getprop {
            return Some(path.clone());
        }
        if cfg!(target_os = "android") {
            return Some(PathBuf::from(GETPROP));
        }
        None
    }

    /// 读取设备摘要。
    pub fn load(&self) -> DeviceSummary {
        let mut summary = DeviceSummary::unknown();
        if let Some(binary) = self.getprop_bin() {
            for (field, key) in GETPROP_KEYS {
                if let Some(value) = getprop(&binary, key) {
                    summary.set_text(field, &value);
                }
            }
        }
        if let Some(fixture) = self.fixture.as_deref().and_then(read_fixture) {
            fixture.apply(&mut summary);
        }
        apply_env_overrides(&mut summary);
        summary
    }
}

/// 读取 fixture JSON；失败返回 None（不阻断启动）。
pub fn read_fixture(path: &Path) -> Option<DeviceFixture> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<DeviceFixture>(&text).ok()
}

fn getprop(binary: &Path, key: &str) -> Option<String> {
    let output = Command::new(binary).arg(key).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

fn apply_env_overrides(summary: &mut DeviceSummary) {
    const ENV_KEYS: [(&str, &str); 6] = [
        ("DF_DEVICE_MANUFACTURER", "manufacturer"),
        ("DF_DEVICE_BRAND", "brand"),
        ("DF_DEVICE_MODEL", "model"),
        ("DF_DEVICE_DEVICE", "device"),
        ("DF_DEVICE_ANDROID", "android"),
        ("DF_DEVICE_SDK", "sdk"),
    ];
    for (variable, field) in ENV_KEYS {
        if let Ok(value) = std::env::var(variable)
            && !value.trim().is_empty()
        {
            summary.set_text(field, value.trim());
        }
    }
}

/// root 管理器信息（docs/API.md 5.21 的 root 块）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootInfo {
    /// kernelsu | apatch | magisk | null。
    pub manager: Option<&'static str>,
    /// 管理器版本（读不到时为 null）。
    pub version: Option<String>,
}

/// `/data/adb` 下的管理器目录（按优先级）。
pub const ROOT_CANDIDATES: [(&str, &str); 3] =
    [("kernelsu", "ksu"), ("apatch", "ap"), ("magisk", "magisk")];

/// 探测 root 管理器：只读文件系统，不执行任何命令。
pub fn root_info(adb_dir: &Path) -> RootInfo {
    for (manager, directory) in ROOT_CANDIDATES {
        let path = adb_dir.join(directory);
        if !path.is_dir() {
            continue;
        }
        let version = ["version", "version.txt"]
            .iter()
            .find_map(|name| std::fs::read_to_string(path.join(name)).ok())
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty());
        return RootInfo {
            manager: Some(manager),
            version,
        };
    }
    RootInfo {
        manager: None,
        version: None,
    }
}
#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write_script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("getprop");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn abi_is_never_empty() {
        assert!(!abi().is_empty());
    }

    #[test]
    fn unknown_source_yields_unknown_fields() {
        let summary = DeviceSource::default().load();
        assert_eq!(summary.manufacturer, UNKNOWN);
        assert_eq!(summary.sdk, 0);
        assert_eq!(summary.abi, abi());
    }

    #[test]
    fn getprop_script_is_used_when_configured() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "#!/bin/sh\ncase \"$1\" in\n  ro.product.manufacturer) echo Google;;\n  ro.product.brand) echo google;;\n  ro.product.model) echo 'Pixel 9';;\n  ro.product.device) echo tokay;;\n  ro.build.version.release) echo 17;;\n  ro.build.version.sdk) echo 37;;\n  *) echo ;;\nesac\n",
        );
        let summary = DeviceSource::new(None, Some(script)).load();
        assert_eq!(summary.manufacturer, "Google");
        assert_eq!(summary.model, "Pixel 9");
        assert_eq!(summary.android, "17");
        assert_eq!(summary.sdk, 37);
    }

    #[test]
    fn fixture_overrides_getprop_and_keeps_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "#!/bin/sh\ncase \"$1\" in\n  ro.product.manufacturer) echo Google;;\n  ro.product.model) echo 'Pixel 9';;\n  *) echo ;;\nesac\n",
        );
        let fixture_path = dir.path().join("device.json");
        std::fs::write(
            &fixture_path,
            r#"{"manufacturer":"Xiaomi","model":"2509FPN0BC","sdk":35}"#,
        )
        .unwrap();
        let summary = DeviceSource::new(Some(fixture_path), Some(script)).load();
        assert_eq!(summary.manufacturer, "Xiaomi");
        assert_eq!(summary.model, "2509FPN0BC");
        assert_eq!(summary.sdk, 35);
        assert_eq!(summary.brand, UNKNOWN);
    }

    #[test]
    fn missing_fixture_is_ignored() {
        let summary =
            DeviceSource::new(Some(PathBuf::from("/nonexistent/device.json")), None).load();
        assert_eq!(summary.manufacturer, UNKNOWN);
    }

    #[test]
    fn root_info_detects_kernelsu_and_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ksu")).unwrap();
        std::fs::write(dir.path().join("ksu/version"), "1.0.7\n").unwrap();
        let info = root_info(dir.path());
        assert_eq!(info.manager, Some("kernelsu"));
        assert_eq!(info.version.as_deref(), Some("1.0.7"));
    }

    #[test]
    fn root_info_without_manager() {
        let dir = tempfile::tempdir().unwrap();
        let info = root_info(dir.path());
        assert_eq!(info.manager, None);
        assert_eq!(info.version, None);
    }
}
