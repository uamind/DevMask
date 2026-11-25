// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 设备 Token 加密：AES-256-GCM，密文格式 v1: + base64(nonce || ciphertext || tag)。
//! 密钥文件为 32 字节原始二进制，权限 0600；AAD 绑定设备 id，防止密文行搬迁。

use std::{
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use zeroize::Zeroizing;

/// 密文版本前缀。
pub const VERSION_PREFIX: &str = "v1:";
/// 密钥长度（AES-256）。
pub const KEY_LEN: usize = 32;
/// GCM nonce 长度。
pub const NONCE_LEN: usize = 12;

/// egress 凭据 AAD 前缀：在「绑定 proxy id」之上再绑字段名，防止两列密文互换。
pub const EGRESS_AAD_PREFIX: &str = "egress_proxy:";

/// egress 凭据字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressField {
    Username,
    Password,
}

impl EgressField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Username => "username",
            Self::Password => "password",
        }
    }
}

/// AAD = "egress_proxy:" ‖ proxy_id ‖ ":" ‖ field。
fn egress_aad(proxy_id: &str, field: EgressField) -> Vec<u8> {
    format!("{EGRESS_AAD_PREFIX}{proxy_id}:{}", field.as_str()).into_bytes()
}

/// 密钥文件与加解密错误。
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("密钥文件读写失败：{path}（{source}）")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("密钥文件 {path} 长度应为 {expected} 字节，实际 {actual} 字节")]
    InvalidLength {
        path: PathBuf,
        expected: usize,
        actual: usize,
    },
    #[error("Token / 代理凭据加密失败")]
    Encrypt,
    #[error("Token / 代理凭据解密失败（密钥不匹配或密文损坏）")]
    Decrypt,
}

impl KeyError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// 设备 Token 密钥；Debug 不输出密钥内容。
#[derive(Clone)]
pub struct DeviceKey {
    key: [u8; KEY_LEN],
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceKey(<redacted>)")
    }
}

impl DeviceKey {
    /// 读取已有密钥；不存在时生成 32 字节并写入（Unix 权限 0600）。
    pub fn load_or_create(path: &Path) -> Result<Self, KeyError> {
        match fs::read(path) {
            Ok(bytes) => Self::from_bytes(path, &bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::generate(path),
            Err(error) => Err(KeyError::io(path, error)),
        }
    }

    fn from_bytes(path: &Path, bytes: &[u8]) -> Result<Self, KeyError> {
        if bytes.len() != KEY_LEN {
            return Err(KeyError::InvalidLength {
                path: path.to_path_buf(),
                expected: KEY_LEN,
                actual: bytes.len(),
            });
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(bytes);
        Self::ensure_private(path);
        Ok(Self { key })
    }

    fn generate(path: &Path) -> Result<Self, KeyError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| KeyError::io(parent, error))?;
        }

        let generated = Aes256Gcm::generate_key(&mut OsRng);
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(generated.as_slice());

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = options
            .open(path)
            .map_err(|error| KeyError::io(path, error))?;
        file.write_all(&key)
            .map_err(|error| KeyError::io(path, error))?;
        file.sync_all().map_err(|error| KeyError::io(path, error))?;

        Self::ensure_private(path);
        tracing::info!(file = %path.display(), "已生成 Manager 设备 Token 密钥");
        Ok(Self { key })
    }

    fn ensure_private(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(path) {
                let mode = metadata.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    tracing::warn!(
                        file = %path.display(),
                        mode = format!("{mode:o}"),
                        "密钥文件权限过宽，已收紧为 0600"
                    );
                    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.key).expect("32 字节密钥总是合法")
    }

    /// 加密设备 Token；AAD 为设备 id。
    pub fn encrypt(&self, device_id: &str, token: &str) -> Result<String, KeyError> {
        self.encrypt_with_aad(device_id.as_bytes(), token)
    }

    /// 解密设备 Token；返回 Zeroizing<String>，drop 时清零。
    pub fn decrypt(&self, device_id: &str, stored: &str) -> Result<Zeroizing<String>, KeyError> {
        self.decrypt_with_aad(device_id.as_bytes(), stored)
    }

    /// 加密出口代理凭据；AAD = egress_proxy:<proxy_id>:<field>。
    pub fn encrypt_egress(
        &self,
        proxy_id: &str,
        field: EgressField,
        value: &str,
    ) -> Result<String, KeyError> {
        self.encrypt_with_aad(&egress_aad(proxy_id, field), value)
    }

    /// 解密出口代理凭据；AAD 与写入时一致（同 id 换字段也解不开）。
    pub fn decrypt_egress(
        &self,
        proxy_id: &str,
        field: EgressField,
        stored: &str,
    ) -> Result<Zeroizing<String>, KeyError> {
        self.decrypt_with_aad(&egress_aad(proxy_id, field), stored)
    }

    fn encrypt_with_aad(&self, aad: &[u8], value: &str) -> Result<String, KeyError> {
        let cipher = self.cipher();
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: value.as_bytes(),
                    aad,
                },
            )
            .map_err(|_| KeyError::Encrypt)?;

        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(nonce.as_slice());
        blob.extend_from_slice(&ciphertext);
        Ok(format!("{VERSION_PREFIX}{}", BASE64.encode(blob)))
    }

    fn decrypt_with_aad(&self, aad: &[u8], stored: &str) -> Result<Zeroizing<String>, KeyError> {
        let encoded = stored
            .strip_prefix(VERSION_PREFIX)
            .ok_or(KeyError::Decrypt)?;
        let blob = BASE64.decode(encoded).map_err(|_| KeyError::Decrypt)?;
        if blob.len() <= NONCE_LEN {
            return Err(KeyError::Decrypt);
        }
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        let plaintext = self
            .cipher()
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| KeyError::Decrypt)?;

        let value = String::from_utf8(plaintext).map_err(|_| KeyError::Decrypt)?;
        Ok(Zeroizing::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn encrypts_and_decrypts_roundtrip() {
        let dir = tempdir().unwrap();
        let key = DeviceKey::load_or_create(&dir.path().join("devmask-manager.key")).unwrap();
        let stored = key.encrypt("pixel-9", "admin123").unwrap();
        assert!(stored.starts_with(VERSION_PREFIX), "{stored}");
        assert!(!stored.contains("admin123"));

        let plaintext = key.decrypt("pixel-9", &stored).unwrap();
        assert_eq!(plaintext.as_str(), "admin123");
    }

    #[test]
    fn key_file_is_32_bytes_and_reloaded() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("devmask-manager.key");
        let key = DeviceKey::load_or_create(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap().len(), KEY_LEN);

        let reloaded = DeviceKey::load_or_create(&path).unwrap();
        let stored = key.encrypt("xiaomi-15", "secret-token").unwrap();
        assert_eq!(
            reloaded.decrypt("xiaomi-15", &stored).unwrap().as_str(),
            "secret-token"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "mode={mode:o}");
        }
    }

    #[test]
    fn wrong_device_id_fails() {
        let dir = tempdir().unwrap();
        let key = DeviceKey::load_or_create(&dir.path().join("devmask-manager.key")).unwrap();
        let stored = key.encrypt("pixel-9", "admin123").unwrap();
        assert!(key.decrypt("redmi-k80", &stored).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let dir = tempdir().unwrap();
        let key = DeviceKey::load_or_create(&dir.path().join("devmask-manager.key")).unwrap();
        let stored = key.encrypt("pixel-9", "admin123").unwrap();
        let mut blob = BASE64.decode(&stored[VERSION_PREFIX.len()..]).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        let tampered = format!("{VERSION_PREFIX}{}", BASE64.encode(blob));
        assert!(key.decrypt("pixel-9", &tampered).is_err());
    }

    #[test]
    fn rejects_unknown_prefix() {
        let dir = tempdir().unwrap();
        let key = DeviceKey::load_or_create(&dir.path().join("devmask-manager.key")).unwrap();
        assert!(key.decrypt("pixel-9", "v2:abcd").is_err());
        assert!(key.decrypt("pixel-9", "not-base64").is_err());
    }

    #[test]
    fn rejects_bad_key_length() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("devmask-manager.key");
        fs::write(&path, b"short").unwrap();
        let error = DeviceKey::load_or_create(&path).unwrap_err();
        assert!(matches!(
            error,
            KeyError::InvalidLength {
                expected: KEY_LEN,
                actual: 5,
                ..
            }
        ));
    }

    #[test]
    fn egress_credentials_roundtrip_and_bind_identity() {
        let dir = tempdir().unwrap();
        let key = DeviceKey::load_or_create(&dir.path().join("devmask-manager.key")).unwrap();
        let stored = key
            .encrypt_egress("egp_abc", EgressField::Password, "hunter2")
            .unwrap();
        assert!(stored.starts_with(VERSION_PREFIX), "{stored}");
        assert_eq!(
            key.decrypt_egress("egp_abc", EgressField::Password, &stored)
                .unwrap()
                .as_str(),
            "hunter2"
        );

        // 换 id / 换字段都解不开。
        assert!(
            key.decrypt_egress("egp_other", EgressField::Password, &stored)
                .is_err()
        );
        assert!(
            key.decrypt_egress("egp_abc", EgressField::Username, &stored)
                .is_err()
        );

        // AAD 与设备 Token 的 AAD 空间互不重叠。
        let token = key.encrypt("egp_abc", "hunter2").unwrap();
        assert!(
            key.decrypt_egress("egp_abc", EgressField::Username, &token)
                .is_err()
        );
    }

    #[test]
    fn debug_does_not_leak_key_material() {
        let dir = tempdir().unwrap();
        let key = DeviceKey::load_or_create(&dir.path().join("devmask-manager.key")).unwrap();
        let rendered = format!("{key:?}");
        assert_eq!(rendered, "DeviceKey(<redacted>)");
    }
}
