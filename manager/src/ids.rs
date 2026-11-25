// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 标识符与时间戳：device id slug、job id、request id、RFC 3339 时间。

use chrono::{SecondsFormat, Utc};
use uuid::Uuid;

/// 生成 request id：req_ + 32 位小写十六进制（uuid v4）。
pub fn new_request_id() -> String {
    format!("req_{}", Uuid::new_v4().simple())
}

/// 生成 job id：job_ + 32 位小写十六进制（uuid v7，时间有序）。
pub fn new_job_id() -> String {
    format!("job_{}", Uuid::now_v7().simple())
}

/// 生成代理池 id：egp_ + 32 位小写十六进制。
pub fn new_egress_proxy_id() -> String {
    format!("egp_{}", Uuid::new_v4().simple())
}

/// 当前 UTC 时间，RFC 3339、秒精度、以 Z 结尾。
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// 设备 id 是否合法：^[a-z0-9][a-z0-9_-]{0,63}$。
pub fn is_valid_device_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
}

/// 由设备名生成 id：小写、非字母数字折叠为 -、去首尾 -、截断 48；
/// 结果为空时退回 device-<6 位 hex>。
pub fn slugify_device_id(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if out.len() > 48 {
        out.truncate(48);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        let suffix = Uuid::new_v4().simple().to_string();
        format!("device-{}", &suffix[..6])
    } else {
        out
    }
}

/// 客户端提供的 request id 是否可接受：1-128 字符，[A-Za-z0-9._:-]。
pub fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_job_ids_have_expected_shape() {
        let request = new_request_id();
        assert!(request.starts_with("req_"), "{request}");
        assert_eq!(request.len(), 4 + 32);
        assert!(request[4..].bytes().all(|b| b.is_ascii_hexdigit()));

        let job = new_job_id();
        assert!(job.starts_with("job_"), "{job}");
        assert_eq!(job.len(), 4 + 32);

        let proxy = new_egress_proxy_id();
        assert!(proxy.starts_with("egp_"), "{proxy}");
        assert_eq!(proxy.len(), 4 + 32);
        assert!(proxy[4..].bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn slugifies_names() {
        assert_eq!(slugify_device_id("Pixel 9 · 主力机"), "pixel-9");
        assert_eq!(slugify_device_id("  Xiaomi__15  "), "xiaomi-15");
        assert_eq!(slugify_device_id("a--b"), "a-b");
    }

    #[test]
    fn slug_falls_back_for_non_ascii_names() {
        let id = slugify_device_id("主力机");
        assert!(id.starts_with("device-"), "{id}");
        assert!(is_valid_device_id(&id), "{id}");
    }

    #[test]
    fn validates_device_ids() {
        assert!(is_valid_device_id("pixel-9"));
        assert!(is_valid_device_id("a"));
        assert!(is_valid_device_id(&"x".repeat(64)));
        assert!(!is_valid_device_id(""));
        assert!(!is_valid_device_id("Pixel-9"));
        assert!(!is_valid_device_id("-pixel"));
        assert!(!is_valid_device_id(&"x".repeat(65)));
        assert!(!is_valid_device_id("pixel 9"));
    }

    #[test]
    fn validates_request_ids() {
        assert!(is_valid_request_id("req_abc-123.v1:ok"));
        assert!(!is_valid_request_id(""));
        assert!(!is_valid_request_id("with space"));
        assert!(!is_valid_request_id(&"x".repeat(129)));
    }

    #[test]
    fn timestamp_is_rfc3339_z() {
        let timestamp = now_rfc3339();
        assert!(timestamp.ends_with('Z'), "{timestamp}");
        assert_eq!(timestamp.len(), 20, "{timestamp}");
    }
}
