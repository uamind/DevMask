// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 时间、随机数与哈希小工具。
//!
//! 设备端不引入额外时间/随机数依赖：RFC 3339 由 civil-from-days 直接算，
//! 随机数优先 /dev/urandom（Android 与 Linux/macOS 都有），失败时退化为
//! 时间 + pid 的 xorshift，保证「不会 panic、不会返回全零」。

use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// Token 明文前缀（docs/API.md 3.1）。
pub const TOKEN_PREFIX: &str = "df_";
/// 生成 Token 的随机字节数：32 字节 -> 43 字符 base64url（无填充）。
pub const TOKEN_SECRET_BYTES: usize = 32;

/// sha256 的十六进制小写表示。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64url（RFC 4648 5，无填充）。
pub fn base64url_no_pad(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let bits = (first << 16) | (second << 8) | third;
        out.push(BASE64URL_ALPHABET[(bits >> 18) as usize & 0x3f] as char);
        out.push(BASE64URL_ALPHABET[(bits >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(BASE64URL_ALPHABET[(bits >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(BASE64URL_ALPHABET[bits as usize & 0x3f] as char);
        }
    }
    out
}

/// 读取 len 字节随机数；优先 /dev/urandom。
pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut buffer = vec![0u8; len];
    if len == 0 {
        return buffer;
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut file) = std::fs::File::open("/dev/urandom")
            && file.read_exact(&mut buffer).is_ok()
        {
            return buffer;
        }
    }
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
        ^ ((std::process::id() as u64) << 32);
    for slot in &mut buffer {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *slot = (seed >> 24) as u8;
    }
    buffer
}

/// 生成一个 Token 明文：df_ + 43 字符 base64url。
pub fn generate_token() -> String {
    format!(
        "{TOKEN_PREFIX}{}",
        base64url_no_pad(&random_bytes(TOKEN_SECRET_BYTES))
    )
}

/// 生成 X-Request-Id：req_ + 24 字符 base64url（ULID 风格，客户端按不透明字符串处理）。
pub fn generate_request_id() -> String {
    format!("req_{}", base64url_no_pad(&random_bytes(18)))
}

/// 公历日期（Howard Hinnant civil_from_days）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Unix 秒 -> RFC 3339（UTC，秒精度，如 2026-11-24T03:00:00Z）。
pub fn rfc3339_from_unix(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60
    )
}

/// 配置解析错误的安全文本：**只保留首行**。
///
/// toml_edit / toml 的 Display 会把出错行原文渲染在后续行里（含该行的键值），而配置/响应
/// 里可能有敏感值，因此对外错误与日志只用首行（形如「TOML parse error at line 3, column 5」）。
pub fn parse_error_message(error: &anyhow::Error) -> String {
    let text = error.to_string();
    let first = text.lines().next().unwrap_or_default().trim();
    if first.is_empty() {
        "解析失败（细节已隐藏）".to_owned()
    } else {
        first.to_owned()
    }
}

/// SystemTime -> RFC 3339（UTC）。早于 1970 时返回 1970-01-01T00:00:00Z。
pub fn rfc3339(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    rfc3339_from_unix(seconds)
}

/// 当前 UTC 时间的 RFC 3339 表示。
pub fn now_rfc3339() -> String {
    rfc3339(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_rfc4648_vectors() {
        assert_eq!(base64url_no_pad(b""), "");
        assert_eq!(base64url_no_pad(b"f"), "Zg");
        assert_eq!(base64url_no_pad(b"fo"), "Zm8");
        assert_eq!(base64url_no_pad(b"foo"), "Zm9v");
        assert_eq!(base64url_no_pad(b"foob"), "Zm9vYg");
        assert_eq!(base64url_no_pad(b"fooba"), "Zm9vYmE");
        assert_eq!(base64url_no_pad(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64url_uses_url_safe_alphabet() {
        let encoded = base64url_no_pad(&[0xfb, 0xff, 0xbf]);
        assert!(!encoded.contains('+') && !encoded.contains('/') && !encoded.contains('='));
    }

    #[test]
    fn sha256_hex_matches_known_digest() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(86_400), "1970-01-02T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_764_000_000), "2025-11-24T16:00:00Z");
        assert_eq!(rfc3339_from_unix(1_703_980_800), "2023-12-31T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_704_067_200), "2024-01-01T00:00:00Z");
        // 契约示例时间（docs/API.md 5.2 startedAt / mtime）。
        assert_eq!(rfc3339_from_unix(1_795_489_200), "2026-11-24T03:00:00Z");
    }

    #[test]
    fn generated_token_shape() {
        let token = generate_token();
        assert!(token.starts_with(TOKEN_PREFIX));
        assert_eq!(token.len(), TOKEN_PREFIX.len() + 43);
        assert!(
            token[TOKEN_PREFIX.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
        assert_ne!(token, generate_token());
    }

    #[test]
    fn request_id_shape() {
        let id = generate_request_id();
        assert!(id.starts_with("req_"));
        assert_eq!(id.len(), 28);
    }
}
