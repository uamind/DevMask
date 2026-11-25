// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 首次启动的 Token 引导（docs/API.md 3.4）。
//!
//! - server.toml 没有任何 Token 且 bind 是 loopback 时，写入默认 Token
//!   `admin123`（name=owner，scopes=read/write/ops/admin），只在 server.toml
//!   里保存 sha256（0600），明文一次性写 logs/server.log 的横幅；
//! - 默认值只用于「内网、首次启动」的便利，用户可在管理器 WebUI 的
//!   「外部访问」里换成任意 Token（同样只写 sha256）；
//! - bind 不是 loopback 时**不自动生成**：无认证暴露局域网比「服务起不来」更糟，
//!   启动校验会直接拒绝（见 settings::validate）。

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::auth::{Scope, TokenEntry, hash_token};
use crate::settings::{self, Settings};

/// 首次启动写入的默认 Token 明文（内网便利优先；可在 WebUI 修改）。
pub const DEFAULT_TOKEN: &str = "admin123";
/// 自动生成 Token 的稳定 id。
pub const INITIAL_TOKEN_ID: &str = "default";
/// 自动生成 Token 的名字。
pub const INITIAL_TOKEN_NAME: &str = "owner";
/// 明文横幅前缀（logs/server.log 中 grep 这一行）。
pub const BANNER_PREFIX: &str = "[devmask-server] initial token = ";

/// 引导结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapOutcome {
    /// 新生成的 Token 明文（仅当本次生成时存在；之后永远拿不到）。
    pub plaintext: Option<String>,
    /// server.toml 路径。
    pub server_config_path: PathBuf,
    /// 本次是否新建了 server.toml。
    pub created_file: bool,
}

impl BootstrapOutcome {
    /// 写日志用的横幅行（不含换行）。
    pub fn banner(&self) -> Option<String> {
        self.plaintext
            .as_ref()
            .map(|plaintext| format!("{BANNER_PREFIX}{plaintext}"))
    }
}

/// 确保有可用的 Token；返回本次生成的明文（若有）。
pub fn bootstrap(settings: &mut Settings) -> Result<BootstrapOutcome> {
    let server_config_path = settings.server_config_path.clone();
    if !settings.tokens.is_empty() {
        return Ok(BootstrapOutcome {
            plaintext: None,
            server_config_path,
            created_file: false,
        });
    }
    if !settings.bind_is_loopback {
        bail!(
            "bind = {} 不是 loopback；不会自动生成 Token（无认证暴露局域网风险过高）。\
             请先按 docs/API.md 3.4 在 {} 里配置 [[tokens]] 再启动。",
            settings.bind,
            server_config_path.display()
        );
    }

    let plaintext = DEFAULT_TOKEN.to_owned();
    let entry = TokenEntry::new(
        INITIAL_TOKEN_ID,
        INITIAL_TOKEN_NAME,
        &hash_token(&plaintext),
        Scope::ALL.to_vec(),
    );

    let created_file = !server_config_path.exists();
    let mut text = match std::fs::read_to_string(&server_config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => default_file_text(settings),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 {} 失败", server_config_path.display()));
        }
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&token_block(&entry));

    // 写前自检：拼接后的文件必须是合法 TOML，且能解析出刚写入的 Token。
    let file = settings::parse_server_file(&text).with_context(|| {
        format!(
            "写入前的 {} 自检失败，已放弃写入",
            server_config_path.display()
        )
    })?;
    let parsed = settings::parse_tokens(&file.tokens).with_context(|| {
        format!(
            "写入前的 {} Token 自检失败，已放弃写入",
            server_config_path.display()
        )
    })?;
    if !parsed.iter().any(|token| token.id == INITIAL_TOKEN_ID) {
        bail!("写入前的 Token 自检失败：没有找到 id = {INITIAL_TOKEN_ID}");
    }

    if let Some(parent) = server_config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建 {} 失败", parent.display()))?;
    }
    // 0600：只有 root 能读；原子写避免出现半截文件。
    devmask_core::revision::atomic_write(&server_config_path, text.as_bytes(), 0o600)
        .with_context(|| format!("写入 {} 失败", server_config_path.display()))?;

    settings.tokens.push(entry);
    Ok(BootstrapOutcome {
        plaintext: Some(plaintext),
        server_config_path,
        created_file,
    })
}

/// 新建 server.toml 时的默认内容。
fn default_file_text(settings: &Settings) -> String {
    let mut text = String::from(
        "# devmask_server 服务设置（首次启动自动生成）。\n\
         # 修改后重启服务生效；Token 只存 sha256，明文仅在生成时输出一次。\n\
         # 局域网访问需要 allow_lan = true 且 bind 不是 loopback（且必须配置 Token）。\n",
    );
    if let Some(bind) = &settings.bind_for_new_file {
        text.push_str(&format!("bind = \"{bind}\"\n"));
    }
    text.push_str(&format!("allow_lan = {}\n", settings.allow_lan));
    text.push_str(&format!(
        "body_limit_bytes = {}\n",
        settings.body_limit_bytes
    ));
    text.push_str(&format!(
        "read_rate_per_min = {}\n",
        settings.read_rate_per_min
    ));
    text.push_str(&format!(
        "write_rate_per_min = {}\n",
        settings.write_rate_per_min
    ));
    text
}

/// [[tokens]] 文本块（只含 sha256）。
fn token_block(entry: &TokenEntry) -> String {
    let scopes = entry
        .scopes
        .iter()
        .map(|scope| format!("\"{}\"", scope.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n# 首次启动自动生成的 Token：明文在 logs/server.log 的横幅里，这里只存 sha256。\n\
         [[tokens]]\n\
         id = \"{}\"\n\
         name = \"{}\"\n\
         hash = \"{}\"\n\
         scopes = [{}]\n",
        entry.id,
        entry.name,
        entry.hash_for_file(),
        scopes
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn loopback_settings(dir: &tempfile::TempDir) -> Settings {
        Settings::for_test(
            dir.path().join("config.toml"),
            dir.path().join("webroot"),
            Vec::new(),
        )
    }

    #[test]
    fn generates_token_and_persists_only_hash() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = loopback_settings(&dir);
        let outcome = bootstrap(&mut settings).unwrap();
        let plaintext = outcome
            .plaintext
            .clone()
            .expect("first run must return plaintext");
        assert_eq!(plaintext, DEFAULT_TOKEN);
        assert_eq!(plaintext, "admin123");
        assert!(outcome.created_file);

        let path = dir.path().join("server.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&plaintext), "明文不得落盘");
        assert!(text.contains("[[tokens]]"));
        assert!(text.contains("hash = \"sha256:"));

        let parsed = settings::parse_server_file(&text).unwrap();
        let tokens = settings::parse_tokens(&parsed.tokens).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].id, INITIAL_TOKEN_ID);
        assert_eq!(tokens[0].name, INITIAL_TOKEN_NAME);
        assert_eq!(tokens[0].hash, hash_token(&plaintext));
        assert_eq!(tokens[0].scopes.len(), 4);
        assert_eq!(settings.tokens.len(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "server.toml 必须 0600");
        }
    }

    #[test]
    fn second_run_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = loopback_settings(&dir);
        bootstrap(&mut settings).unwrap();
        let second = bootstrap(&mut settings).unwrap();
        assert!(second.plaintext.is_none());
        assert!(!second.created_file);
        assert_eq!(settings.tokens.len(), 1);
    }

    #[test]
    fn appends_to_existing_settings_without_losing_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = loopback_settings(&dir);
        let path = dir.path().join("server.toml");
        std::fs::write(
            &path,
            "bind = \"127.0.0.1:9999\"\nallow_lan = false\nbody_limit_bytes = 1024\n",
        )
        .unwrap();
        bootstrap(&mut settings).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("bind = \"127.0.0.1:9999\""), "既有设置不能丢");
        assert!(text.contains("body_limit_bytes = 1024"));
        let parsed = settings::parse_server_file(&text).unwrap();
        assert_eq!(parsed.body_limit_bytes, Some(1024));
        assert_eq!(parsed.tokens.len(), 1);
    }

    #[test]
    fn refuses_to_generate_for_non_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = loopback_settings(&dir);
        settings.bind = "0.0.0.0:8686".to_owned();
        settings.bind_is_loopback = false;
        let error = bootstrap(&mut settings).unwrap_err().to_string();
        assert!(error.contains("不是 loopback"), "{error}");
        assert!(!dir.path().join("server.toml").exists());
    }

    #[test]
    fn banner_contains_plaintext_once() {
        let outcome = BootstrapOutcome {
            plaintext: Some("df_test".to_owned()),
            server_config_path: PathBuf::from("/tmp/server.toml"),
            created_file: true,
        };
        assert_eq!(
            outcome.banner().unwrap(),
            "[devmask-server] initial token = df_test"
        );
        let none = BootstrapOutcome {
            plaintext: None,
            server_config_path: PathBuf::from("/tmp/server.toml"),
            created_file: false,
        };
        assert!(none.banner().is_none());
    }
}
