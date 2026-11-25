// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 查询参数解析：手工处理重复键与类型错误，保证 422 统一信封。

use std::collections::HashMap;

use url::form_urlencoded;

use crate::error::{ApiError, ValidationError};

/// 解析后的查询参数（保留重复键）。
#[derive(Debug, Default, Clone)]
pub struct QueryParams {
    pairs: Vec<(String, String)>,
}

impl QueryParams {
    /// 解析原始 query 字符串（不含问号）。
    pub fn parse(raw: Option<&str>) -> Self {
        let pairs = form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        Self { pairs }
    }

    /// 第一个同名参数。
    pub fn first(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// 所有同名参数（如重复的 tag）。
    pub fn multi(&self, name: &str) -> Vec<String> {
        self.pairs
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }

    /// 布尔参数：true / false / 1 / 0；缺省为 None。
    pub fn boolean(&self, name: &str) -> Result<Option<bool>, ApiError> {
        let Some(value) = self.first(name) else {
            return Ok(None);
        };
        match value {
            "true" | "1" => Ok(Some(true)),
            "false" | "0" => Ok(Some(false)),
            _ => Err(invalid_value(name, format!("{name} 必须是 true 或 false"))),
        }
    }

    /// usize 参数；非法值 → 422。
    pub fn usize(&self, name: &str) -> Result<Option<usize>, ApiError> {
        let Some(value) = self.first(name) else {
            return Ok(None);
        };
        value
            .parse::<usize>()
            .map(Some)
            .map_err(|_| invalid_value(name, format!("{name} 必须是正整数")))
    }

    /// 任意字符串参数。
    pub fn string(&self, name: &str) -> Option<String> {
        self.first(name).map(str::to_owned)
    }

    /// 是否包含某个参数。
    pub fn contains(&self, name: &str) -> bool {
        self.pairs.iter().any(|(key, _)| key == name)
    }

    /// 转成键值映射（调试 / 审计用）。
    pub fn to_map(&self) -> HashMap<String, String> {
        self.pairs.iter().cloned().collect()
    }
}

fn invalid_value(path: &str, message: impl Into<String>) -> ApiError {
    ApiError::validation(vec![ValidationError::new(
        path,
        "invalid_value",
        message.into(),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repeated_and_typed_values() {
        let params = QueryParams::parse(Some("tag=a&tag=b&online=true&limit=10&q=pixel"));
        assert_eq!(params.multi("tag"), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(params.boolean("online").unwrap(), Some(true));
        assert_eq!(params.usize("limit").unwrap(), Some(10));
        assert_eq!(params.string("q").as_deref(), Some("pixel"));
        assert!(params.contains("q"));
    }

    #[test]
    fn rejects_invalid_booleans_and_numbers() {
        assert!(
            QueryParams::parse(Some("online=maybe"))
                .boolean("online")
                .is_err()
        );
        assert!(
            QueryParams::parse(Some("limit=abc"))
                .usize("limit")
                .is_err()
        );
    }

    #[test]
    fn missing_values_are_none() {
        let params = QueryParams::parse(None);
        assert_eq!(params.boolean("online").unwrap(), None);
        assert_eq!(params.usize("limit").unwrap(), None);
        assert!(params.string("q").is_none());
    }
}
