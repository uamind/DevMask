// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 写操作审计（docs/API.md 3.6、计划书 5.7）。
//!
//! 只挂在 write 路由组上：每次写尝试（含 412/422 失败）追加一行 JSONL；
//! 不记录 Token 明文与配置全文。审计写失败只记 warn，不影响请求结果。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use serde::Serialize;

use crate::{app::AppState, auth::AuthContext, requests, store, util};

/// 审计文件（append-only JSONL）。
#[derive(Debug, Clone)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    /// 指定文件路径。
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 追加一条记录（父目录不存在则创建；文件 0600）。
    pub fn append(&self, entry: &AuditEntry) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_owned());
        line.push('\n');
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.flush()
    }
}

/// 一条审计记录。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    /// UTC RFC 3339。
    pub ts: String,
    /// 与 X-Request-Id 一致。
    pub request_id: String,
    /// Token id。
    pub token_id: String,
    /// Token 名字。
    pub token_name: String,
    /// 来源地址（当前未接入 ConnectInfo，恒为 null）。
    pub remote: Option<String>,
    /// HTTP 方法。
    pub method: String,
    /// 请求路径。
    pub path: String,
    /// 响应状态码。
    pub status: u16,
    /// 写入前 revision（文件不存在时为 null）。
    pub revision_before: Option<String>,
    /// 写入后 revision。
    pub revision_after: Option<String>,
    /// 处理耗时（毫秒）。
    pub duration_ms: u64,
    /// User-Agent。
    pub user_agent: Option<String>,
}

/// write 路由组的审计中间件（放在权限守卫之内、幂等中间件之内）。
pub async fn middleware(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let started = Instant::now();
    let request_id = requests::extract(&request).as_str().to_owned();
    let auth = request.extensions().get::<AuthContext>().cloned();
    let method = request.method().as_str().to_owned();
    // 嵌套路由里的 uri() 已经去掉 /api/v1 前缀；审计要记录客户端看到的完整路径。
    let path = request
        .extensions()
        .get::<axum::extract::OriginalUri>()
        .map(|original| original.0.path().to_owned())
        .unwrap_or_else(|| {
            let path = request.uri().path();
            if path.starts_with("/api/v1") {
                path.to_owned()
            } else {
                format!("/api/v1{path}")
            }
        });
    let user_agent = request
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let revision_before = store::read_snapshot(state.store())
        .ok()
        .and_then(|snapshot| snapshot.exists.then_some(snapshot.revision));

    let response = next.run(request).await;

    let revision_after = store::read_snapshot(state.store())
        .ok()
        .and_then(|snapshot| snapshot.exists.then_some(snapshot.revision));
    let entry = AuditEntry {
        ts: util::now_rfc3339(),
        request_id,
        token_id: auth
            .as_ref()
            .map(|auth| auth.token_id.clone())
            .unwrap_or_default(),
        token_name: auth
            .as_ref()
            .map(|auth| auth.token_name.clone())
            .unwrap_or_default(),
        remote: None,
        method,
        path,
        status: response.status().as_u16(),
        revision_before,
        revision_after,
        duration_ms: started.elapsed().as_millis() as u64,
        user_agent,
    };
    if let Err(error) = state.audit().append(&entry) {
        tracing::warn!(error = %error, "写审计日志失败（不影响请求结果）");
    }
    response
}
