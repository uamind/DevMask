// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

import { toast } from 'kernelsu-alt'
import { backend } from '../../backend'
import { useLazyMessage } from '../../utils/elementPlus'

/**
 * 跨传输的用户提示。
 *
 * - 管理器模式：沿用 `kernelsu-alt` 的原生 Android toast（行为零改动）；
 * - 原生模式：`window.ksu` 不存在，`toast()` 只会写 console，因此改用 Element Plus
 *   的 ElMessage（挂到 body，不依赖 App.vue 也能显示）。
 */
export type NotifyKind = 'info' | 'success' | 'warning' | 'error'

export async function notify(message: string, kind: NotifyKind = 'info'): Promise<void> {
  const text = message.trim()
  if (!text) {
    return
  }

  if (backend.runtime === 'ksu') {
    toast(text)
    return
  }

  try {
    const ElMessage = await useLazyMessage()()
    ElMessage({
      message: text,
      type: kind,
      duration: kind === 'error' ? 6000 : 3500,
      showClose: true,
      grouping: true,
    })
  } catch (error) {
    // ElMessage 加载失败（极端情况）时至少留下痕迹
    console.warn('[notify]', text, error)
  }
}
