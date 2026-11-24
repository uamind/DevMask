// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

/** RFC 3339 → 相对时间（刚刚 / 12 秒前 / 3 小时前 / 2026-11-24 08:12）。 */
export function relativeTime(iso: string | null | undefined): string {
  if (!iso) return '--'
  const timestamp = Date.parse(iso)
  if (Number.isNaN(timestamp)) return iso
  const seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000))
  if (seconds < 5) return '刚刚'
  if (seconds < 60) return `${seconds} 秒前`
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes} 分钟前`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} 小时前`
  const days = Math.round(hours / 24)
  if (days < 30) return `${days} 天前`
  return formatDateTime(iso)
}

/** RFC 3339 → 本地 YYYY-MM-DD HH:mm。 */
export function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return '--'
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso
  const pad = (value: number): string => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours()
  )}:${pad(date.getMinutes())}`
}

/** uptime 秒 → 1 天 2 小时 / 3 分钟。 */
export function uptimeLabel(seconds: number | null | undefined): string {
  if (typeof seconds !== 'number' || seconds < 0) return '--'
  if (seconds < 60) return `${Math.round(seconds)} 秒`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} 分钟`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时 ${minutes % 60} 分`
  const days = Math.floor(hours / 24)
  return `${days} 天 ${hours % 24} 小时`
}

/** 截断长文本（指纹、详情等）。 */
export function truncate(text: string | null | undefined, max = 64): string {
  if (!text) return '--'
  return text.length > max ? `${text.slice(0, max)}…` : text
}
