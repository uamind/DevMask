// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

//! 配置模型统一转发到 [`devmask_core::config`]。
//!
//! 配置模型（Config / DeviceTemplate / AppConfig / NumberSetting / MergedAppConfig）、
//! 模板优先规则、属性族展开与 `__DELETE__` 语义已下沉到共享 crate，供 server 与测试
//! 复用。这里保留 `crate::config::*` 的既有调用方式，模块侧代码一行不用改。

pub use devmask_core::config::*;
