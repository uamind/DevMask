// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker
//
// 应用展示元数据。API 只返回 package（API.md §6：AppDef.id/name/short/color 无对应字段），
// 因此中文名、首字与配色都在 UI 侧维护；未知包名回落到包名本身。

import type { AppDef } from './types'

interface CatalogEntry {
  package: string
  name: string
  short: string
  color: string
}

const CATALOG: CatalogEntry[] = [
  { package: 'com.ss.android.ugc.aweme', name: '抖音', short: '抖', color: '#111827' },
  { package: 'com.xingin.xhs', name: '小红书', short: '红', color: '#ff2442' },
  { package: 'com.tencent.tmgp.pubgmhd', name: '和平精英', short: '和', color: '#f59e0b' },
  { package: 'com.tencent.mm', name: '微信', short: '微', color: '#07c160' },
  { package: 'com.taobao.taobao', name: '淘宝', short: '淘', color: '#ff5000' },
  { package: 'tv.danmaku.bili', name: '哔哩哔哩', short: 'B', color: '#fb7299' },
  { package: 'com.miHoYo.Yuanshen', name: '原神', short: '原', color: '#6d5efc' },
  { package: 'com.eg.android.AlipayGphone', name: '支付宝', short: '支', color: '#1677ff' },
  { package: 'com.sankuai.meituan', name: '美团', short: '美', color: '#ffc300' },
  { package: 'com.jingdong.app.mall', name: '京东', short: '京', color: '#e1251b' },
]

const FALLBACK_COLORS = [
  '#6d5efc',
  '#0ea5e9',
  '#10b981',
  '#f97316',
  '#ec4899',
  '#8b5cf6',
  '#ef4444',
  '#14b8a6',
]

/** 包名 → 展示定义；未知包名用包名末段首字母和稳定配色。 */
export function appDefFor(packageName: string): AppDef {
  const known = CATALOG.find((item) => item.package === packageName)
  if (known) {
    return { id: known.package, name: known.name, package: known.package, short: known.short, color: known.color }
  }
  let hash = 0
  for (const char of packageName) hash = (hash * 31 + char.charCodeAt(0)) >>> 0
  const tail = packageName.split('.').filter(Boolean).pop() ?? packageName
  return {
    id: packageName,
    name: packageName,
    package: packageName,
    short: (tail[0] ?? '?').toUpperCase(),
    color: FALLBACK_COLORS[hash % FALLBACK_COLORS.length],
  }
}

/** 目录里的已知包名（批量页可选项的补充来源）。 */
export function knownAppPackages(): string[] {
  return CATALOG.map((item) => item.package)
}

/** 目录里的全部应用定义。 */
export function knownAppDefs(): AppDef[] {
  return CATALOG.map((item) => appDefFor(item.package))
}

/** 包名 → 中文名（未知时原样返回包名）。 */
export function appDisplayName(packageName: string): string {
  return CATALOG.find((item) => item.package === packageName)?.name ?? packageName
}

/** API.md §2.7 invalid_package 的同一正则，前端先做一次即时校验。 */
export const PACKAGE_PATTERN = /^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+(@[0-9]{1,3})?$/

/** 设备 id 正则（API.md §2.7 invalid_device_id）。 */
export const DEVICE_ID_PATTERN = /^[a-z0-9][a-z0-9_-]{0,63}$/
