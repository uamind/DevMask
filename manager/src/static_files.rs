// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 静态托管：托管 manager/webui/dist，并为前端路由提供 SPA fallback。

use std::path::{Component, Path, PathBuf};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{error::ApiError, state::AppState};

/// index.html 与 SPA fallback 的缓存策略。
pub const INDEX_CACHE: &str = "no-cache";
/// Vite 带 hash 的 assets 缓存策略。
pub const ASSET_CACHE: &str = "public, max-age=31536000, immutable";
/// 其它根级静态文件。
pub const FILE_CACHE: &str = "public, max-age=3600";

const CSP: &str =
    "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'";

/// dist 未构建时的可读提示页（API 文档第 11.3 节）。
pub const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head><meta charset="utf-8"><title>Device Faker Manager</title></head>
<body style="font-family: system-ui, sans-serif; padding: 40px; line-height: 1.6">
  <h1>Device Faker Manager</h1>
  <p>WebUI 尚未构建。运行：</p>
  <pre style="background:#f5f5f5;padding:12px;border-radius:6px">cd manager/webui &amp;&amp; npm install &amp;&amp; npm run build</pre>
  <p>API 已就绪：<code>GET /relay/api/v1/healthz</code></p>
</body>
</html>
"#;

/// 按候选顺序找到第一个含 index.html 的目录。
pub fn resolve_static_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|dir| dir.join("index.html").is_file())
        .cloned()
}

/// 静态路由 fallback：先尝试真实文件，未命中时回落到 index.html。
pub async fn serve(State(state): State<AppState>, request: Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    if method != Method::GET && method != Method::HEAD {
        return ApiError::method_not_allowed(method.as_str(), &path).into_response();
    }
    let head_only = method == Method::HEAD;

    let Some(site) = state.static_site.as_ref() else {
        return placeholder(&path, head_only);
    };

    if let Some(relative) = safe_relative(&path)
        && !relative.as_os_str().is_empty()
        && let Some(file) = existing_file(&site.dir, &relative)
    {
        return file_response(file, cache_for(&path), head_only).await;
    }

    file_response(site.index.clone(), INDEX_CACHE, head_only).await
}

/// 把 URL 路径转成安全的相对路径；含 ..、绝对路径或盘符时返回 None。
fn safe_relative(path: &str) -> Option<PathBuf> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some(PathBuf::new());
    }
    let mut relative = PathBuf::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            _ => return None,
        }
    }
    Some(relative)
}

fn existing_file(root: &Path, relative: &Path) -> Option<PathBuf> {
    let candidate = root.join(relative);
    let canonical = candidate.canonicalize().ok()?;
    let canonical_root = root.canonicalize().ok()?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
        return None;
    }
    Some(canonical)
}

fn cache_for(path: &str) -> &'static str {
    if path.starts_with("/assets/") {
        ASSET_CACHE
    } else if path == "/" || path == "/index.html" {
        INDEX_CACHE
    } else {
        FILE_CACHE
    }
}

async fn file_response(path: PathBuf, cache: &'static str, head_only: bool) -> Response {
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(file = %path.display(), error = %error, "读取静态文件失败");
            return text_response(
                StatusCode::NOT_FOUND,
                "静态文件不存在或不可读；若刚部署请确认 manager/webui/dist 完整",
                INDEX_CACHE,
            );
        }
    };

    let content_type = content_type(&path);
    let body = if head_only {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    base_response(StatusCode::OK, content_type, cache, body)
}

fn placeholder(path: &str, head_only: bool) -> Response {
    if path == "/" || path == "/index.html" {
        let body = if head_only {
            Body::empty()
        } else {
            Body::from(PLACEHOLDER_HTML)
        };
        base_response(
            StatusCode::OK,
            "text/html; charset=utf-8",
            INDEX_CACHE,
            body,
        )
    } else {
        text_response(
            StatusCode::NOT_FOUND,
            "WebUI 尚未构建；请运行 cd manager/webui && npm install && npm run build",
            INDEX_CACHE,
        )
    }
}

fn text_response(status: StatusCode, message: &'static str, cache: &'static str) -> Response {
    let body = if status == StatusCode::NOT_FOUND && message.is_empty() {
        Body::empty()
    } else {
        Body::from(message)
    };
    base_response(status, "text/plain; charset=utf-8", cache, body)
}

fn base_response(
    status: StatusCode,
    content_type: &'static str,
    cache: &'static str,
    body: Body,
) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static(CSP));
    response
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_rejects_traversal() {
        assert!(safe_relative("/../etc/passwd").is_none());
        assert!(safe_relative("/assets/../../Cargo.toml").is_none());
        assert!(safe_relative("/assets/app.js").is_some());
        assert_eq!(safe_relative("/").unwrap(), PathBuf::new());
    }

    #[test]
    fn resolves_first_candidate_with_index() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(second.path().join("index.html"), "<html></html>").unwrap();

        let candidates = vec![first.path().to_path_buf(), second.path().to_path_buf()];
        let resolved = resolve_static_dir(&candidates).unwrap();
        assert_eq!(resolved, second.path());
    }

    #[test]
    fn maps_content_types() {
        assert_eq!(
            content_type(Path::new("index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("app.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("logo.png")), "image/png");
        assert_eq!(
            content_type(Path::new("unknown.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn asset_cache_is_immutable() {
        assert_eq!(cache_for("/assets/app-123.js"), ASSET_CACHE);
        assert_eq!(cache_for("/"), INDEX_CACHE);
        assert_eq!(cache_for("/favicon.ico"), FILE_CACHE);
    }
}
