// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! POST /relay/api/v1/token/rotate：轮换 Manager Token 并持久化。

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    api::devices::write_audit,
    error::{ApiError, ValidationError},
    ids, request_id,
    state::{AppState, TokenSource},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RotateTokenRequest {
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotateTokenResponse {
    pub token: String,
    pub rotated_at: String,
    pub token_persisted: bool,
    pub source: &'static str,
}

/// POST /relay/api/v1/token/rotate
pub async fn rotate(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<RotateTokenResponse>, ApiError> {
    // 先解析请求体：畸形 body 绝不允许触发 Token 轮换。
    let requested = parse_rotate_body(&headers, &body)?;
    let current = state.token.read().await.clone();
    if matches!(current.source, TokenSource::Cli | TokenSource::Env) {
        write_audit(
            &state,
            "token.rotate",
            Some("token"),
            None,
            request_id::current().as_deref(),
            "failed",
            json!({ "reason": "token_overridden", "source": current.source.as_str() }),
        )
        .await;
        return Err(ApiError::invalid_state("token_overridden"));
    }

    let token = match requested {
        Some(token) => {
            if !valid_token(&token) {
                return Err(ApiError::validation(vec![ValidationError::new(
                    "token",
                    "invalid_value",
                    "Token 需为 16-128 字符且不含空白",
                )]));
            }
            token
        }
        None => generate_token(),
    };

    let persisted = match persist_token(&state.cfg.token_file, &token) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(
                error = %error,
                file = %state.cfg.token_file.display(),
                "Token 轮换已生效，但写入 token 文件失败"
            );
            false
        }
    };

    {
        let mut guard = state.token.write().await;
        guard.value = token.clone();
        guard.source = TokenSource::File;
    }

    let rotated_at = ids::now_rfc3339();
    write_audit(
        &state,
        "token.rotate",
        Some("token"),
        None,
        request_id::current().as_deref(),
        "ok",
        json!({ "tokenPersisted": persisted, "source": "rotated" }),
    )
    .await;

    Ok(Json(RotateTokenResponse {
        token,
        rotated_at,
        token_persisted: persisted,
        source: "rotated",
    }))
}

fn parse_rotate_body(headers: &HeaderMap, body: &[u8]) -> Result<Option<String>, ApiError> {
    let trimmed = body.trim_ascii();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !content_type
        .to_ascii_lowercase()
        .starts_with("application/json")
    {
        return Err(ApiError::unsupported_media_type());
    }
    let value: serde_json::Value = serde_json::from_slice(trimmed).map_err(|error| {
        ApiError::validation(vec![ValidationError::new(
            "body",
            "syntax_error",
            format!("请求体不是合法 JSON：{error}"),
        )])
    })?;
    if !value.is_object() {
        return Err(ApiError::validation(vec![ValidationError::new(
            "body",
            "invalid_type",
            "请求体必须是 JSON 对象",
        )]));
    }
    let request: RotateTokenRequest = serde_json::from_value(value).map_err(|error| {
        let message = error.to_string();
        let code = if message.contains("unknown field") {
            "unknown_field"
        } else if message.contains("missing field") {
            "missing_required"
        } else {
            "invalid_type"
        };
        ApiError::validation(vec![ValidationError::new("body", code, message)])
    })?;
    Ok(request
        .token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty()))
}

fn generate_token() -> String {
    let mut bytes = Vec::with_capacity(32);
    for _ in 0..2 {
        bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    }
    format!("dfm_{}", URL_SAFE_NO_PAD.encode(&bytes[..24]))
}

fn valid_token(token: &str) -> bool {
    let length = token.chars().count();
    (16..=128).contains(&length)
        && !token
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn persist_token(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_file_name(format!(".manager.token.tmp-{}", Uuid::new_v4().simple()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_token_has_contract_shape() {
        let token = generate_token();
        assert!(token.starts_with("dfm_"));
        assert_eq!(token.len(), 4 + 32);
        assert!(valid_token(&token));
    }

    #[test]
    fn validates_custom_tokens() {
        assert!(valid_token("a-strong-manager-token"));
        assert!(!valid_token("short"));
        assert!(!valid_token("with space and length"));
        assert!(!valid_token(&"x".repeat(129)));
    }
}
