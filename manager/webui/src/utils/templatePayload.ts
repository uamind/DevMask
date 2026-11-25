// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask
//
// 模板下发载荷构造（t8 review R1）：
// /templates 返回的聚合 config 里 packages 只用于 WebUI 展示/筛选，
// 它是「某台设备上这个模板绑定了哪些包」的快照，不能回流进 apply 的对象模板，
// 否则会把 A 设备的包绑定写到 B 设备上。apply 前统一在这里剔除。

import type { TemplateInput, TemplateName, TemplateRef } from '../api/types'

/**
 * 复制模板 config 并移除展示性字段 packages，返回可直接放进 apply 的配置。
 * 不修改入参（TemplateSummary 仍需要保留 packages 用于展示）。
 */
export function templateConfigForApply(config: TemplateInput): TemplateInput {
  const next: TemplateInput = { ...config }
  delete next.packages
  return next
}

/**
 * 构造 apply 的 TemplateRef 对象形态：{ name: 模板名, config: 设备字段 }。
 * 两条下发路径（bulk 批量页、assignments 矩阵单格）都必须走这里。
 */
export function templateRefForApply(name: TemplateName, config: TemplateInput): TemplateRef {
  return { name, config: templateConfigForApply(config) }
}
