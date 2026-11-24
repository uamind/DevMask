// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 分页：limit 1-500，cursor 为 Base64URL 的不透明偏移量。

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::error::{ApiError, ValidationError};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 500;
const CURSOR_PREFIX: &str = "v1:";

/// 解析后的分页参数。
#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub limit: usize,
    pub offset: usize,
}

/// 解析 limit / cursor；非法值返回 422 校验错误。
pub fn resolve(limit: Option<usize>, cursor: Option<&str>) -> Result<Page, ApiError> {
    let limit = match limit {
        Some(limit) if (1..=MAX_LIMIT).contains(&limit) => limit,
        Some(_) => {
            return Err(ApiError::validation(vec![ValidationError::new(
                "limit",
                "out_of_range",
                format!("limit 必须在 1-{MAX_LIMIT} 之间"),
            )]));
        }
        None => DEFAULT_LIMIT,
    };

    let offset = match cursor {
        Some(cursor) => decode_cursor(cursor)?,
        None => 0,
    };

    Ok(Page { limit, offset })
}

/// 生成下一页 cursor；没有下一页时返回 None。
pub fn next_cursor(total: usize, offset: usize, taken: usize) -> Option<String> {
    let next = offset.saturating_add(taken);
    if next >= total {
        return None;
    }
    Some(URL_SAFE_NO_PAD.encode(format!("{CURSOR_PREFIX}{next}")))
}

fn decode_cursor(cursor: &str) -> Result<usize, ApiError> {
    let invalid = || {
        ApiError::validation(vec![ValidationError::new(
            "cursor",
            "invalid_value",
            "cursor 不是合法的分页游标",
        )])
    };

    let decoded = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
    let text = String::from_utf8(decoded).map_err(|_| invalid())?;
    let offset = text
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(invalid)?
        .parse::<usize>()
        .map_err(|_| invalid())?;
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_defaults_and_ranges() {
        let page = resolve(None, None).unwrap();
        assert_eq!(page.limit, DEFAULT_LIMIT);
        assert_eq!(page.offset, 0);

        assert!(resolve(Some(0), None).is_err());
        assert!(resolve(Some(MAX_LIMIT + 1), None).is_err());
        assert!(resolve(Some(10), None).is_ok());
    }

    #[test]
    fn cursor_roundtrip() {
        let cursor = next_cursor(100, 0, 50).expect("下一页");
        let page = resolve(Some(50), Some(&cursor)).unwrap();
        assert_eq!(page.offset, 50);

        assert!(next_cursor(50, 0, 50).is_none());
        assert!(resolve(Some(50), Some("not-a-cursor")).is_err());
    }
}
