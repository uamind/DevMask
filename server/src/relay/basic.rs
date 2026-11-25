// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! HTTP Basic 的 base64（契约 §4.5：标准字母表、带 `=` 填充、不换行）。
//!
//! 只为这一个用途实现编码，不引入新依赖（12 行 vs 一个 crate）。

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 标准 base64 编码（带填充）。
pub fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).copied().map(u32::from).unwrap_or(0);
        let third = chunk.get(2).copied().map(u32::from).unwrap_or(0);
        let triple = (first << 16) | (second << 8) | third;
        out.push(char::from(ALPHABET[((triple >> 18) & 0x3f) as usize]));
        out.push(char::from(ALPHABET[((triple >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(ALPHABET[((triple >> 6) & 0x3f) as usize]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(ALPHABET[(triple & 0x3f) as usize]));
        } else {
            out.push('=');
        }
    }
    out
}

/// `Basic ` + base64(`username:password`)，即 `Proxy-Authorization` 的完整值。
pub fn authorization_value(username: &str, password: &str) -> String {
    let mut raw = Vec::with_capacity(username.len() + 1 + password.len());
    raw.extend_from_slice(username.as_bytes());
    raw.push(b':');
    raw.extend_from_slice(password.as_bytes());
    let encoded = encode(&raw);
    raw.iter_mut().for_each(|byte| *byte = 0);
    format!("Basic {encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(input.as_bytes()), expected, "input={input:?}");
        }
    }

    #[test]
    fn basic_value_matches_rfc7617_sample() {
        // RFC 7617 的经典样例：Aladdin:open sesame
        assert_eq!(
            authorization_value("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        // 空 user-id 合法
        assert_eq!(authorization_value("", "p"), "Basic OnA=");
        // UTF-8 参与编码
        assert_eq!(
            authorization_value("用户", "密码"),
            "Basic 55So5oi3OuWvhueggQ=="
        );
    }
}
