// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask
//
// WebUI 展示层类型。真实数据来自 api/types.ts（API v1），这里只做视图映射：
// - 应用名称 / 图标 / 颜色是 UI 元数据（API.md §6：package 是唯一键）
// - 矩阵行、单元格、日志行由 store 从 API 响应投影而来

import type {
  DeviceSummary,
  TemplateInput,
  TemplateSummary,
  Warning,
} from './api/types'

/** 设备展示别名（字段与 DeviceSummary 完全一致，避免 UI 再造模型）。 */
export type Device = DeviceSummary

/** 应用定义（UI 侧元数据 + API 的 package）。 */
export interface AppDef {
  /** 与 package 相同，保留 id 便于现有页面/路由复用。 */
  id: string
  name: string
  package: string
  short: string
  color: string
}

/** 某个应用在某台设备上的生效配置（矩阵单元格的视图投影）。 */
export interface AppConfig {
  appId: string
  deviceId: string
  model: string
  /** 生效模板名；内联配置为 null。 */
  template: string | null
  source: 'template' | 'inline'
  /** 多条 warnings 合并成一句话，供 tooltip 展示。 */
  warning?: string
}

/** 模板选项（TemplateSummary 的扁平视图，保留 config 以便下发时重建 TemplateRef）。 */
export interface TemplateDef {
  id: string
  name: string
  model: string
  brand: string
  android: string
  dpi: number | null
  config: TemplateInput
  sources: string[]
  complete: boolean
}

/** 操作日志（AuditEntry 的视图投影）。 */
export interface LogEntry {
  id: string
  type: 'add' | 'remove' | 'sync' | 'batch' | 'error'
  title: string
  detail: string
  time: string
  status: 'success' | 'error' | 'info'
  requestId?: string | null
}

/** 应用矩阵的一行。 */
export interface MatrixRow {
  app: AppDef
  cells: Record<string, AppConfig | undefined>
  configured: number
  total: number
  inconsistent: boolean
  models: string[]
  offlineDevices: string[]
  warnings: Warning[]
}

/** 添加设备表单（baseUrl + 设备 Token，探测由 Relay 执行）。 */
export interface NewDeviceInput {
  name: string
  baseUrl: string
  token: string
  tags: string[]
}

/** 下发目标：全部设备或显式 id 列表。 */
export type ApplyTargets = 'all' | string[]

/** 批量下发入参（bulk 形态）。 */
export interface BatchApplyInput {
  app: string
  devices: ApplyTargets
  template: TemplateDef
  restart: boolean
}

export type { TemplateSummary, Warning }
