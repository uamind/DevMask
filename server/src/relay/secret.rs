// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 凭据的内存载体（契约 §5.2 / §4.5 / §4.8）。
//!
//! 两条硬规则：
//!
//! 1. 值只在内存里，[\`Secret\`] 的 `Drop` 会把字节清零（重载 = 旧值 zeroize）；
//! 2. `Debug` 永远输出 `<redacted>`，日志与错误 details 拿不到明文。

use std::sync::atomic::{Ordering, compiler_fence};

use crate::egress::file::ProxyDefinition;
use crate::relay::basic;

/// 一段需要清零的内存。`Debug` 只输出长度。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    bytes: Vec<u8>,
}

impl Secret {
    /// 从字节构造。
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    /// 只读访问（仅用于拼 Basic 头 / 比较，不要记日志）。
    pub fn expose(&self) -> &[u8] {
        &self.bytes
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// 主动清零（`Drop` 会调用；单测直接调它断言）。
    pub fn zeroize(&mut self) {
        for byte in self.bytes.iter_mut() {
            // 用 volatile 写，避免被优化掉。
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        compiler_fence(Ordering::SeqCst);
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Secret(<redacted>, {} bytes)", self.bytes.len())
    }
}

/// 一条上游凭据：用户名 + 密码 + 预计算好的 `Proxy-Authorization` 值。
///
/// 预计算是为了每个连接不再重新编码；值本身也是 [\`Secret\`]，随 listener 释放一起清零。
pub struct Credentials {
    username: Secret,
    password: Secret,
    authorization: Secret,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Credentials {
    /// 由定义构造；无凭据（两项都空）时返回 `None`。
    pub fn from_definition(proxy: &ProxyDefinition) -> Option<Self> {
        if !proxy.has_credentials() {
            return None;
        }
        Some(Self::new(
            proxy.username.as_deref().unwrap_or(""),
            proxy.password.as_deref().unwrap_or(""),
        ))
    }

    /// 由明文构造（空串参与拼接，RFC 7617 允许空 user-id）。
    pub fn new(username: &str, password: &str) -> Self {
        let authorization = basic::authorization_value(username, password);
        Self {
            username: Secret::new(username.as_bytes().to_vec()),
            password: Secret::new(password.as_bytes().to_vec()),
            authorization: Secret::new(authorization.into_bytes()),
        }
    }

    /// `Proxy-Authorization` 的**值**（形如 `Basic dXNlcjpwYXNz`，不含头名与 CRLF）。
    pub fn authorization(&self) -> &Secret {
        &self.authorization
    }

    /// 用户名（只为比较，不用于日志）。
    pub fn username(&self) -> &Secret {
        &self.username
    }

    /// 密码（只为比较，不用于日志）。
    pub fn password(&self) -> &Secret {
        &self.password
    }
}

impl PartialEq for Credentials {
    fn eq(&self, other: &Self) -> bool {
        self.username == other.username
            && self.password == other.password
            && self.authorization == other.authorization
    }
}

impl Eq for Credentials {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_zeroizes_on_request_and_drop() {
        let mut secret = Secret::new(b"p@ss".to_vec());
        assert_eq!(secret.expose(), b"p@ss");
        secret.zeroize();
        assert!(
            secret.expose().iter().all(|byte| *byte == 0),
            "zeroize 后必须全 0"
        );
        // Drop 路径：不 panic 即可；值不泄漏到 Debug。
        let dropped = Secret::new(b"s3cret".to_vec());
        assert!(!format!("{dropped:?}").contains("s3cret"));
    }

    #[test]
    fn credentials_never_debug_the_plaintext() {
        let credentials = Credentials::new("alice", "s3cret");
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("alice"), "{debug}");
        assert!(!debug.contains("s3cret"), "{debug}");
        assert_eq!(
            std::str::from_utf8(credentials.authorization().expose()).unwrap(),
            "Basic YWxpY2U6czNjcmV0"
        );
    }

    #[test]
    fn credentials_require_at_least_one_field() {
        let mut proxy = ProxyDefinition {
            name: "p".to_owned(),
            kind: crate::egress::file::ProxyType::Http,
            host: "203.0.113.10".to_owned(),
            port: 8080,
            username: None,
            password: None,
            local_port: None,
        };
        assert!(Credentials::from_definition(&proxy).is_none());
        proxy.password = Some(String::new());
        assert!(
            Credentials::from_definition(&proxy).is_some(),
            "password 键存在即算带凭据"
        );
    }
}
