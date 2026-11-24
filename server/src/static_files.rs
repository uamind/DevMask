// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 静态资源托管与 SPA 回退（docs/API.md 10.1、11）。
//!
//! 规则：
//!
//! | 路径 | 行为 |
//! | --- | --- |
//! | / | index.html + 运行时注入，no-store |
//! | /assets/<file> | 静态文件，文件名带哈希，长缓存 |
//! | /<无扩展名路径> | SPA 回退到 index.html |
//! | /<缺失的带扩展名路径> | 404 纯文本，不进 SPA 回退 |
//! | /api/v1/* | 只走 API（未匹配由 api 路由 404 错误信封） |
//!
//! 目录穿越：路径先百分号解码，再逐段拒绝 `.` / `..` / 反斜杠 / NUL；
//! resolve 时对真实路径 canonicalize 并校验仍在 webroot 内（挡住符号链接逃逸）。

use std::path::{Path, PathBuf};

use axum::{
    body::Body,
    extract::{OriginalUri, State},
    http::{
        HeaderName, HeaderValue, Method, StatusCode, header::CACHE_CONTROL, header::CONTENT_TYPE,
    },
    response::{IntoResponse, Response},
};
use percent_encoding::percent_decode_str;

use crate::{app::AppState, error::ApiError, requests::RequestId};

/// 注入到 index.html 的运行时配置：不含 Token（docs/API.md 10.1）。
pub const RUNTIME_SCRIPT: &str = r#"<script>window.__DF_SERVER__ = { api: "/api/v1", version: "1", transport: "http" };</script>"#;
/// 带哈希文件名的 assets 长缓存。
pub const ASSET_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";
/// 其它静态文件的缓存策略。
pub const DEFAULT_CACHE_CONTROL: &str = "no-cache";

const X_CONTENT_TYPE_OPTIONS: HeaderName = HeaderName::from_static("x-content-type-options");

/// 静态站点：webroot 根目录 + 已 canonicalize 的根（用于防逃逸）。
#[derive(Clone, Debug)]
pub struct StaticSite {
    root: PathBuf,
    canonical_root: Option<PathBuf>,
    index: PathBuf,
}

impl StaticSite {
    /// 以 webroot 目录构造。目录不存在时仍可构造，请求期返回 404。
    pub fn new(root: PathBuf) -> Self {
        let index = root.join("index.html");
        let canonical_root = std::fs::canonicalize(&root).ok();
        Self {
            root,
            canonical_root,
            index,
        }
    }

    /// webroot 根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// index.html 路径。
    pub fn index_path(&self) -> &Path {
        &self.index
    }

    /// webroot 是否可用（启动日志用）。
    pub fn exists(&self) -> bool {
        self.canonical_root
            .as_ref()
            .is_some_and(|root| root.is_dir())
    }

    /// 把相对路径解析成 webroot 内的真实路径；目录穿越或符号链接逃逸返回 None。
    pub fn resolve(&self, relative: &Path) -> Option<PathBuf> {
        let root = self.canonical_root.as_ref()?;
        let canonical = std::fs::canonicalize(root.join(relative)).ok()?;
        canonical.starts_with(root).then_some(canonical)
    }
}

/// 百分号解码 + 逐段校验；任何可疑段返回 None（调用方按 404 处理）。
pub fn decode_path(raw: &str) -> Option<PathBuf> {
    let decoded = percent_decode_str(raw).decode_utf8().ok()?;
    let mut relative = PathBuf::new();
    for segment in decoded.split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." || segment.contains('\\') || segment.contains('\0') {
            return None;
        }
        relative.push(segment);
    }
    Some(relative)
}

/// 在 index.html 的 <head> 后注入 runtime 脚本；已注入则原样返回（幂等）。
pub fn inject_runtime(html: &str) -> String {
    if html.contains("__DF_SERVER__") {
        return html.to_owned();
    }
    let lower = html.to_ascii_lowercase();
    if let Some(position) = lower.find("<head>") {
        let insert_at = position + "<head>".len();
        let mut out = String::with_capacity(html.len() + RUNTIME_SCRIPT.len() + 8);
        out.push_str(&html[..insert_at]);
        out.push('\n');
        out.push_str("    ");
        out.push_str(RUNTIME_SCRIPT);
        out.push_str(&html[insert_at..]);
        return out;
    }
    format!("{RUNTIME_SCRIPT}\n{html}")
}

/// MIME 表：覆盖 js / css / json / svg / png / webp / woff2 / wasm / ico / txt。
pub fn content_type(relative: &Path) -> &'static str {
    let extension = relative
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") | Some("map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// assets/ 下带哈希文件名长缓存，其余 no-cache。
pub fn cache_control(relative: &Path) -> &'static str {
    let first = relative
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        });
    match first {
        Some("assets") => ASSET_CACHE_CONTROL,
        _ => DEFAULT_CACHE_CONTROL,
    }
}

/// 非 API 路径的兜底处理器。
pub async fn handle(
    State(state): State<AppState>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
    method: Method,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let raw_path = uri.path();

    // /api/v1 之外没有别的 API；/api 前缀一律按「未实现路径」处理，不进 SPA 回退。
    if raw_path == "/api" || raw_path.starts_with("/api/") {
        return ApiError::not_found(request_id.as_str(), "path", raw_path).into_response();
    }

    let Some(relative) = decode_path(raw_path) else {
        return text_not_found();
    };
    if relative.as_os_str().is_empty() {
        return index_response(&state, &method).await;
    }

    if let Some(path) = state.site().resolve(&relative)
        && path.is_file()
    {
        return file_response(&path, &relative, &method).await;
    }

    if relative.extension().is_some() {
        // 缺失的带扩展名路径：404 纯文本，不进 SPA 回退。
        return text_not_found();
    }
    index_response(&state, &method).await
}

async fn index_response(state: &AppState, method: &Method) -> Response {
    let html = match tokio::fs::read_to_string(state.site().index_path()).await {
        Ok(html) => html,
        Err(_) => return text_not_found(),
    };
    let body = inject_runtime(&html);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(body))
        .expect("static index response is valid");
    if method == Method::HEAD {
        *response.body_mut() = Body::empty();
    }
    response
}

async fn file_response(path: &Path, relative: &Path, method: &Method) -> Response {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(_) => return text_not_found(),
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type(relative))
        .header(CACHE_CONTROL, cache_control(relative))
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .expect("static file response is valid");
    if method == Method::HEAD {
        *response.body_mut() = Body::empty();
    }
    response
}

fn text_response(status: StatusCode, message: &'static str) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(Body::from(message))
        .expect("text response is valid");
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

/// 404 纯文本（缺失的静态文件；与 API 的错误信封区分）。
pub fn text_not_found() -> Response {
    text_response(StatusCode::NOT_FOUND, "Not Found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rejects_traversal_and_keeps_plain_paths() {
        assert_eq!(
            decode_path("/assets/app.js"),
            Some(PathBuf::from("assets/app.js"))
        );
        assert_eq!(decode_path("/"), Some(PathBuf::new()));
        assert_eq!(decode_path("/a/../etc/passwd"), None);
        assert_eq!(decode_path("/%2e%2e/etc/passwd"), None);
        assert_eq!(decode_path("/assets/%2e%2e%2fsecret"), None);
        assert_eq!(decode_path("/a\\b"), None);
        assert_eq!(
            decode_path("//double//slash"),
            Some(PathBuf::from("double/slash"))
        );
    }

    #[test]
    fn injection_is_idempotent_and_goes_after_head() {
        let html = "<!doctype html><html><head><title>t</title></head><body></body></html>";
        let injected = inject_runtime(html);
        assert!(injected.contains(RUNTIME_SCRIPT));
        assert!(injected.contains("window.__DF_SERVER__"));
        let head_position = injected.find("<head>").unwrap();
        let script_position = injected.find(RUNTIME_SCRIPT).unwrap();
        assert!(script_position > head_position);
        assert_eq!(inject_runtime(&injected), injected, "重复注入必须幂等");
    }

    #[test]
    fn injection_without_head_prepends() {
        let html = "<html><body>x</body></html>";
        let injected = inject_runtime(html);
        assert!(injected.starts_with(RUNTIME_SCRIPT));
        assert!(injected.ends_with(html));
    }

    #[test]
    fn runtime_script_matches_contract_and_has_no_token() {
        assert_eq!(
            RUNTIME_SCRIPT,
            "<script>window.__DF_SERVER__ = { api: \"/api/v1\", version: \"1\", transport: \"http\" };</script>"
        );
        assert!(!RUNTIME_SCRIPT.to_ascii_lowercase().contains("token"));
        assert!(!RUNTIME_SCRIPT.contains("Authorization"));
    }

    #[test]
    fn mime_table_covers_contract_types() {
        let cases = [
            ("app.js", "text/javascript; charset=utf-8"),
            ("app.css", "text/css; charset=utf-8"),
            ("data.json", "application/json; charset=utf-8"),
            ("logo.svg", "image/svg+xml"),
            ("logo.png", "image/png"),
            ("logo.webp", "image/webp"),
            ("font.woff2", "font/woff2"),
            ("mod.wasm", "application/wasm"),
            ("favicon.ico", "image/x-icon"),
            ("robots.txt", "text/plain; charset=utf-8"),
            ("blob.bin", "application/octet-stream"),
        ];
        for (name, expected) in cases {
            assert_eq!(content_type(Path::new(name)), expected, "{name}");
        }
    }

    #[test]
    fn assets_get_long_cache_others_no_cache() {
        assert_eq!(
            cache_control(Path::new("assets/index-abc.js")),
            ASSET_CACHE_CONTROL
        );
        assert_eq!(
            cache_control(Path::new("favicon.ico")),
            DEFAULT_CACHE_CONTROL
        );
    }

    #[test]
    fn resolve_blocks_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("assets")).unwrap();
        std::fs::write(root.path().join("assets/app.js"), "console.log(1)").unwrap();
        let site = StaticSite::new(root.path().to_path_buf());
        assert!(site.exists());
        assert!(site.resolve(Path::new("assets/app.js")).is_some());
        assert!(site.resolve(Path::new("assets/missing.js")).is_none());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("secret.txt"),
                root.path().join("assets/link.txt"),
            )
            .unwrap();
            assert!(
                site.resolve(Path::new("assets/link.txt")).is_none(),
                "指向 webroot 外的符号链接必须拒绝"
            );
        }
    }
}
