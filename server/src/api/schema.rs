// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! GET /api/v1/schema/config（docs/API.md 5.3、t10 修订）。
//!
//! 直接返回 core 的 schemars 产物：draft-07 + `definitions`，不包壳、不改方言、
//! 不把 definitions 改名成 $defs。

use axum::{extract::Extension, http::StatusCode, response::Response};
use device_faker_core::schema;

use crate::api::json;
use crate::requests::RequestId;

/// 返回 Config 的 JSON Schema。
pub async fn get_config_schema(Extension(_request_id): Extension<RequestId>) -> Response {
    json(StatusCode::OK, schema::config_schema())
}
