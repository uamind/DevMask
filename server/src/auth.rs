// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! Bearer Token 认证与作用域判定（docs/API.md 3）。
//!
//! - 落盘只存 sha256(token)，比较用常量时间；
//! - 作用域不隐含：write 不含 read，ops 不含 read；admin 是其余三个的超集；
//! - 失败不区分「缺 Token / 格式错 / 未知 / 已撤销」，统一 401，避免枚举。

use axum::{
    extract::{Request, State},
    http::{HeaderMap, header::AUTHORIZATION},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::{app::AppState, error::ApiError, requests, util};

/// 作用域（docs/API.md 3.2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Read,
    Write,
    Ops,
    Admin,
}

impl Scope {
    /// 全部作用域（写 server.toml 与文档一致）。
    pub const ALL: [Scope; 4] = [Scope::Read, Scope::Write, Scope::Ops, Scope::Admin];

    /// 线上名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Ops => "ops",
            Scope::Admin => "admin",
        }
    }

    /// 解析作用域名；未知值返回 None（启动时报错，避免静默降权/提权）。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Scope::Read),
            "write" => Some(Scope::Write),
            "ops" => Some(Scope::Ops),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }
}

/// server.toml 里的一个 Token（只保存哈希）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenEntry {
    /// 稳定 id（审计与日志用）。
    pub id: String,
    /// 人类可读名字。
    pub name: String,
    /// 归一化后的 sha256 十六进制（无 sha256: 前缀）。
    pub hash: String,
    /// 授予的作用域。
    pub scopes: Vec<Scope>,
}

impl TokenEntry {
    /// 构造并归一化哈希。
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        hash: &str,
        scopes: Vec<Scope>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            hash: normalize_hash(hash),
            scopes,
        }
    }

    /// 是否允许该作用域：admin 是超集，其余不隐含。
    pub fn allows(&self, needed: Scope) -> bool {
        self.scopes.contains(&Scope::Admin) || self.scopes.contains(&needed)
    }

    /// 写回 server.toml 的哈希形式（带 sha256: 前缀）。
    pub fn hash_for_file(&self) -> String {
        format!("sha256:{}", self.hash)
    }

    /// 作用域名列表（错误 details 与日志用）。
    pub fn scope_names(&self) -> Vec<String> {
        self.scopes
            .iter()
            .map(|scope| scope.as_str().to_owned())
            .collect()
    }
}

/// 归一化哈希：去掉可选的 sha256: 前缀并转小写。
pub fn normalize_hash(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_prefix = trimmed
        .strip_prefix("sha256:")
        .or_else(|| trimmed.strip_prefix("SHA256:"))
        .unwrap_or(trimmed);
    without_prefix.trim().to_ascii_lowercase()
}

/// 计算 Token 的 sha256（小写十六进制）。
pub fn hash_token(token: &str) -> String {
    util::sha256_hex(token.as_bytes())
}

/// 认证上下文：注入到请求扩展，供审计与后续端点使用。
#[derive(Clone, Debug, Serialize)]
pub struct AuthContext {
    /// Token id。
    pub token_id: String,
    /// Token 名字。
    pub token_name: String,
    /// 授予的作用域（名字形式，便于审计）。
    pub scopes: Vec<String>,
}

/// 从 Authorization 头解析 Bearer Token（scheme 大小写不敏感）。
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, value) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = value.trim();
    if token.is_empty() {
        return None;
    }
    Some(token)
}

/// 常量时间比较（长度不同直接 false；哈希长度固定，不泄露内容）。
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// 校验请求头里的 Token；成功返回认证上下文。
pub fn authenticate(tokens: &[TokenEntry], headers: &HeaderMap) -> Option<AuthContext> {
    let presented = bearer_token(headers)?;
    let digest = hash_token(presented);
    let mut matched: Option<&TokenEntry> = None;
    for entry in tokens {
        // 不用提前 break：遍历全部条目，避免按位置泄露匹配信息。
        if constant_time_eq(entry.hash.as_bytes(), digest.as_bytes()) {
            matched = Some(entry);
        }
    }
    let entry = matched?;
    Some(AuthContext {
        token_id: entry.id.clone(),
        token_name: entry.name.clone(),
        scopes: entry.scope_names(),
    })
}

/// 挂到某个作用域上的守卫状态。
#[derive(Clone)]
pub struct ScopeGuard {
    /// 应用状态（含 Token 表）。
    pub state: AppState,
    /// 该路由组要求的作用域。
    pub required: Scope,
}

/// 认证 + 作用域中间件。
pub async fn guard(State(guard): State<ScopeGuard>, mut request: Request, next: Next) -> Response {
    let request_id = requests::extract(&request);
    let tokens = &guard.state.settings().tokens;
    let Some(context) = authenticate(tokens, request.headers()) else {
        return ApiError::unauthorized(request_id.as_str()).into_response();
    };

    let allowed = context
        .scopes
        .iter()
        .filter_map(|scope| Scope::parse(scope))
        .any(|scope| scope == Scope::Admin || scope == guard.required);
    if !allowed {
        return ApiError::forbidden_scope(
            request_id.as_str(),
            guard.required.as_str(),
            &context.scopes,
        )
        .into_response();
    }

    request.extensions_mut().insert(context);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    use super::*;

    fn headers_with(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    fn entry(token: &str, scopes: Vec<Scope>) -> TokenEntry {
        TokenEntry::new("default", "owner", &hash_token(token), scopes)
    }

    #[test]
    fn scope_names_round_trip() {
        for scope in Scope::ALL {
            assert_eq!(Scope::parse(scope.as_str()), Some(scope));
        }
        assert_eq!(Scope::parse("root"), None);
    }

    #[test]
    fn admin_is_superset_but_write_is_not_read() {
        let admin = entry("df_a", vec![Scope::Admin]);
        for scope in Scope::ALL {
            assert!(admin.allows(scope));
        }
        let writer = entry("df_w", vec![Scope::Write]);
        assert!(writer.allows(Scope::Write));
        assert!(!writer.allows(Scope::Read));
        assert!(!writer.allows(Scope::Ops));
    }

    #[test]
    fn authenticate_accepts_only_known_token() {
        let tokens = vec![entry("df_right", vec![Scope::Read])];
        let context = authenticate(&tokens, &headers_with("df_right")).expect("should match");
        assert_eq!(context.token_id, "default");
        assert_eq!(context.scopes, vec!["read".to_string()]);
        assert!(authenticate(&tokens, &headers_with("df_wrong")).is_none());
        assert!(authenticate(&tokens, &HeaderMap::new()).is_none());
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let tokens = vec![entry("df_right", vec![Scope::Read])];
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("bearer df_right"));
        assert!(authenticate(&tokens, &headers).is_some());
    }

    #[test]
    fn normalize_hash_strips_optional_prefix() {
        let digest = util::sha256_hex(b"token");
        assert_eq!(normalize_hash(&format!("sha256:{digest}")), digest);
        assert_eq!(normalize_hash(&format!("  {digest}  ")), digest);
        assert_eq!(normalize_hash(&digest.to_ascii_uppercase()), digest);
    }
}
