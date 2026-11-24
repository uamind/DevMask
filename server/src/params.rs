// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 查询参数、请求体与 Content-Type 的契约化解析（docs/API.md 2）。
//!
//! 不用 axum 的强类型 `Query<T>`：它解析失败会返回 400 纯文本，而契约要求
//! 422 + 错误信封 + 字段路径。

use std::collections::HashMap;

use axum::http::{HeaderMap, header::CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::dto::ValidationError;
use crate::error::ApiError;

/// 默认分页大小。
pub const DEFAULT_LIMIT: usize = 50;
/// 最大分页大小。
pub const MAX_LIMIT: usize = 500;

/// 查询参数表（保留原始字符串，按端点语义解释）。
#[derive(Debug, Clone, Default)]
pub struct Params {
    map: HashMap<String, String>,
}

impl Params {
    /// 从 axum 解析好的 map 构造。
    pub fn new(map: HashMap<String, String>) -> Self {
        Self { map }
    }

    /// 原始字符串值。
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(String::as_str)
    }

    /// 布尔参数：缺省 false；`true/1` 与 `false/0` 之外的值 → 422。
    pub fn flag(&self, name: &str, request_id: &str) -> Result<bool, ApiError> {
        match self.raw(name) {
            None => Ok(false),
            Some("true") | Some("1") => Ok(true),
            Some("false") | Some("0") | Some("") => Ok(false),
            Some(other) => Err(invalid_param(
                request_id,
                name,
                format!("{name} 只接受 true/false，收到 {other:?}"),
            )),
        }
    }

    /// confirm=true 必填（缺失或 false → 400 confirmation_required）。
    pub fn confirm(&self, action: &str, request_id: &str) -> Result<(), ApiError> {
        if self.flag("confirm", request_id)? {
            Ok(())
        } else {
            Err(ApiError::confirmation_required(request_id, action))
        }
    }

    /// 非负整数参数；非法 → 422 invalid_type。
    pub fn usize_param(&self, name: &str, request_id: &str) -> Result<Option<usize>, ApiError> {
        let Some(raw) = self.raw(name) else {
            return Ok(None);
        };
        raw.parse::<usize>().map(Some).map_err(|_| {
            invalid_param(
                request_id,
                name,
                format!("{name} 必须是非负整数，收到 {raw:?}"),
            )
        })
    }

    /// 分页参数（limit 1..=500、cursor 不透明字符串）。
    pub fn page(&self, request_id: &str) -> Result<Page, ApiError> {
        let limit = self
            .usize_param("limit", request_id)?
            .unwrap_or(DEFAULT_LIMIT);
        if limit == 0 || limit > MAX_LIMIT {
            return Err(ApiError::validation_failed(
                request_id,
                json!([ValidationError::new(
                    "limit",
                    "out_of_range",
                    format!("limit 必须在 1..={MAX_LIMIT}"),
                )
                .to_value()]),
            ));
        }
        let offset = match self.raw("cursor") {
            None => 0,
            Some("") => 0,
            Some(cursor) => cursor.parse::<usize>().map_err(|_| {
                ApiError::validation_failed(
                    request_id,
                    json!([ValidationError::new(
                        "cursor",
                        "invalid_value",
                        "cursor 无法解析（必须原样回传上一页的 nextCursor）",
                    )
                    .to_value()]),
                )
            })?,
        };
        Ok(Page { offset, limit })
    }
}

/// 解析后的分页窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// 起始下标（0 基）。
    pub offset: usize,
    /// 页大小。
    pub limit: usize,
}

impl Page {
    /// 取当前页切片与 nextCursor。
    pub fn slice<'a, T>(&self, items: &'a [T]) -> (&'a [T], Option<String>) {
        let total = items.len();
        let start = self.offset.min(total);
        let end = (start + self.limit).min(total);
        let next = (end < total).then(|| end.to_string());
        (&items[start..end], next)
    }
}

fn invalid_param(request_id: &str, name: &str, message: String) -> ApiError {
    ApiError::validation_failed(
        request_id,
        json!([ValidationError::new(name, "invalid_value", message).to_value()]),
    )
}

/// 请求体类型（PUT /config 同时接受 JSON 与 TOML 原文）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// application/json
    Json,
    /// application/toml 或 text/plain
    Toml,
}

/// 只接受 JSON 的端点。
pub fn require_json(headers: &HeaderMap, request_id: &str) -> Result<(), ApiError> {
    let kind = content_kind(headers);
    if kind == Some(BodyKind::Json) {
        Ok(())
    } else {
        Err(ApiError::unsupported_media_type(
            request_id,
            &["application/json"],
        ))
    }
}

/// PUT /config 的两种请求体类型。
pub fn config_body_kind(headers: &HeaderMap, request_id: &str) -> Result<BodyKind, ApiError> {
    content_kind(headers).ok_or_else(|| {
        ApiError::unsupported_media_type(
            request_id,
            &["application/json", "application/toml", "text/plain"],
        )
    })
}

fn content_kind(headers: &HeaderMap) -> Option<BodyKind> {
    let raw = headers.get(CONTENT_TYPE)?.to_str().ok()?;
    let media_type = raw
        .split(';')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    match media_type.as_str() {
        "application/json" => Some(BodyKind::Json),
        "application/toml" | "text/plain" | "application/x-toml" => Some(BodyKind::Toml),
        _ => None,
    }
}

/// 解析 JSON 请求体；语法错误 → 422 syntax_error（path=`$`）。
pub fn parse_json(bytes: &[u8], request_id: &str) -> Result<Value, ApiError> {
    serde_json::from_slice(bytes).map_err(|error| {
        ApiError::validation_failed(
            request_id,
            json!([
                ValidationError::new("$", "syntax_error", format!("JSON 解析失败: {error}"),)
                    .to_value()
            ]),
        )
    })
}

/// 把 JSON 值反序列化成类型；失败返回一条带 serde 路径的 [`ValidationError`]。
pub fn typed_result<T: DeserializeOwned>(value: &Value) -> Result<T, ValidationError> {
    let text = value.to_string();
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    serde_path_to_error::deserialize::<_, T>(&mut deserializer).map_err(|error| {
        let path = error.path().to_string();
        let path = if path.is_empty() || path == "." {
            "$".to_owned()
        } else {
            path
        };
        ValidationError::new(
            path,
            "invalid_type",
            format!("字段类型不合法: {}", error.inner()),
        )
    })
}

/// 把 JSON 值反序列化成类型；失败 → 422 invalid_type（带 serde 路径）。
pub fn typed<T: DeserializeOwned>(value: &Value, request_id: &str) -> Result<T, ApiError> {
    typed_result(value).map_err(|error| validation_failed(request_id, vec![error]))
}

/// 拒绝未知键（docs/API.md 2.2 / 6.1）。
pub fn reject_unknown_keys(
    object: &Map<String, Value>,
    known: impl Fn(&str) -> bool,
    path_prefix: &str,
    request_id: &str,
) -> Result<(), ApiError> {
    let unknown: Vec<&String> = object.keys().filter(|key| !known(key)).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let errors: Vec<Value> = unknown
        .iter()
        .map(|key| {
            let path = if path_prefix.is_empty() {
                (*key).clone()
            } else {
                format!("{path_prefix}.{key}")
            };
            ValidationError::new(path, "unknown_field", format!("未知字段 {key}")).to_value()
        })
        .collect();
    Err(ApiError::validation_failed(
        request_id,
        Value::Array(errors),
    ))
}

/// 组装 422 details.errors。
pub fn validation_failed(request_id: &str, errors: Vec<ValidationError>) -> ApiError {
    ApiError::validation_failed(
        request_id,
        Value::Array(errors.iter().map(ValidationError::to_value).collect()),
    )
}

/// 单个字段错误 422。
pub fn field_error(
    request_id: &str,
    path: &str,
    code: &'static str,
    message: impl Into<String>,
) -> ApiError {
    validation_failed(request_id, vec![ValidationError::new(path, code, message)])
}

/// 逗号分隔列表（`names=`, `keys=`）。
pub fn comma_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Params {
        Params::new(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn flags_accept_contract_values() {
        assert!(
            params(&[("dry_run", "true")])
                .flag("dry_run", "req")
                .unwrap()
        );
        assert!(
            !params(&[("dry_run", "false")])
                .flag("dry_run", "req")
                .unwrap()
        );
        assert!(!params(&[]).flag("dry_run", "req").unwrap());
        assert_eq!(
            params(&[("dry_run", "yes")])
                .flag("dry_run", "req")
                .unwrap_err()
                .code(),
            crate::error::ErrorCode::ValidationFailed
        );
    }

    #[test]
    fn page_defaults_and_bounds() {
        let page = params(&[]).page("req").unwrap();
        assert_eq!(
            page,
            Page {
                offset: 0,
                limit: DEFAULT_LIMIT
            }
        );
        assert!(params(&[("limit", "0")]).page("req").is_err());
        assert!(params(&[("limit", "501")]).page("req").is_err());
        assert!(params(&[("cursor", "abc")]).page("req").is_err());
        assert_eq!(params(&[("cursor", "25")]).page("req").unwrap().offset, 25);
    }

    #[test]
    fn page_slice_reports_next_cursor() {
        let items = [0, 1, 2, 3, 4];
        let page = Page {
            offset: 0,
            limit: 2,
        };
        let (slice, next) = page.slice(&items);
        assert_eq!(slice, &[0, 1]);
        assert_eq!(next.as_deref(), Some("2"));
        let last = Page {
            offset: 4,
            limit: 2,
        };
        let (slice, next) = last.slice(&items);
        assert_eq!(slice, &[4]);
        assert_eq!(next, None);
    }

    #[test]
    fn content_kind_parses_media_types() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert_eq!(content_kind(&headers), Some(BodyKind::Json));
        headers.insert(CONTENT_TYPE, "text/plain".parse().unwrap());
        assert_eq!(content_kind(&headers), Some(BodyKind::Toml));
        headers.insert(CONTENT_TYPE, "application/xml".parse().unwrap());
        assert_eq!(content_kind(&headers), None);
    }
}
