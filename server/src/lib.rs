// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! DevMask 原生 Web 服务。
//!
//! 第一增量（骨架）职责：
//!
//! - 托管 module/webroot（SPA 回退、index.html 运行时注入、assets 长缓存）；
//! - 认证（Bearer Token + read/write/ops/admin 作用域）与统一错误信封；
//! - GET /api/v1/healthz（公开）与 GET /api/v1/status（read 作用域）。
//!
//! 配置写入、模板与应用 CRUD、packages / device 端点由后续增量接入；未实现的
//! /api/v1/* 一律返回 404 + 错误信封，不回退 index.html（docs/API.md 1.2、11）。
//!
//! 模块划分：
//!
//! - app：AppState 与路由装配；
//! - api：healthz / status 处理器与 DTO；
//! - auth：Token 校验与作用域守卫；
//! - error：统一错误信封与错误码表（14 个稳定错误码）；
//! - requests：X-Request-Id、Cache-Control: no-store 等通用中间件；
//! - static_files：静态资源与 SPA 回退；
//! - settings：server.toml、命令行覆盖与 loopback 安全约束；
//! - token：Token 哈希、首次启动生成与 server.toml 落盘；
//! - args / device / module / util：命令行、设备信息、module.prop、时间与随机数。

pub mod api;
pub mod app;
pub mod args;
pub mod audit;
pub mod auth;
pub mod commands;
pub mod device;
pub mod diff;
pub mod dto;
pub mod egress;
pub mod error;
pub mod idempotency;
pub mod module;
pub mod packages;
pub mod params;
pub mod relay;
pub mod requests;
pub mod settings;
pub mod static_files;
pub mod store;
pub mod token;
pub mod util;

pub use app::{AppState, router, serve};
pub use error::{ApiError, ErrorCode};
