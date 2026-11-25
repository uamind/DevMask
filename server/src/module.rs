// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! module.prop 解析（GET /api/v1/status 的 module 块）。

use std::path::Path;

use serde::Serialize;

/// 模块信息（docs/API.md 6 / openapi ModuleInfo）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleInfo {
    /// 模块 id。
    pub id: String,
    /// 模块版本（带 v 前缀）。
    pub version: String,
    /// 模块版本号。
    pub version_code: u64,
}

/// module.prop 缺失或损坏时的 id。
pub const FALLBACK_ID: &str = "devmask";
/// module.prop 缺失或损坏时的版本。
pub const UNKNOWN_VERSION: &str = "unknown";

impl ModuleInfo {
    /// 缺省值：文件不存在时仍能给出稳定 JSON 形状。
    pub fn fallback() -> Self {
        Self {
            id: FALLBACK_ID.to_owned(),
            version: UNKNOWN_VERSION.to_owned(),
            version_code: 0,
        }
    }
}

/// 读取 module.prop；读不到时返回 fallback。
pub fn read(path: &Path) -> ModuleInfo {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(_) => ModuleInfo::fallback(),
    }
}

/// 解析 module.prop 的 id / version / versionCode。
pub fn parse(text: &str) -> ModuleInfo {
    let mut info = ModuleInfo::fallback();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "id" => info.id = value.to_owned(),
            "version" => info.version = value.to_owned(),
            "versionCode" => {
                if let Ok(code) = value.parse::<u64>() {
                    info.version_code = code;
                }
            }
            _ => {}
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_module_prop() {
        let text = "id=devmask\nname=DevMask\nversion=v1.7.0\nversionCode=20261109\nauthor=x\n";
        let info = parse(text);
        assert_eq!(info.id, "devmask");
        assert_eq!(info.version, "v1.7.0");
        assert_eq!(info.version_code, 20261109);
    }

    #[test]
    fn missing_file_falls_back() {
        let info = read(Path::new("/nonexistent/module.prop"));
        assert_eq!(info, ModuleInfo::fallback());
    }

    #[test]
    fn malformed_lines_are_ignored() {
        let info = parse("garbage\nversionCode=not-a-number\nversion = v9.9.9\n");
        assert_eq!(info.id, FALLBACK_ID);
        assert_eq!(info.version, "v9.9.9");
        assert_eq!(info.version_code, 0);
    }

    #[test]
    fn serializes_camel_case() {
        let json = serde_json::to_value(ModuleInfo::fallback()).unwrap();
        assert_eq!(json["versionCode"], 0);
        assert_eq!(json["id"], FALLBACK_ID);
    }
}
