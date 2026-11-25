/// <reference types="vite/client" />

// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

interface ImportMetaEnv {
  /** Relay API 前缀，默认同源 /relay/api/v1。 */
  readonly VITE_API_BASE?: string
  /** 置为 "1" 时只加载 src/mock 的纯视觉预览数据。 */
  readonly VITE_MOCK?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
