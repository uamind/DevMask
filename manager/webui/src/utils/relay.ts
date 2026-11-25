// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

// 设备内本地认证中继（relay）的展示口径，契约：docs/plans/relay-contract.md §6。
//
// 设备卡 / 设备详情 / 代理池三处共用同一套「四态 + 占位文案」，
// 避免同一个 null 在不同页面被解释成「关闭」「离线」「旧设备」三种说法。

import type {
  DeviceEgressDeviceState,
  DeviceRelayState,
  DeviceRelaySummary,
} from '../api/types'

/** active=正在转发；idle=已启用但没有 listener 在 accept；disabled=关闭；unknown=设备没上报。 */
export type RelayViewState = 'active' | 'idle' | 'disabled' | 'unknown'

/** 设备没上报 relay 时的统一占位文案（离线 / 旧设备 / 设备不可达）。 */
export const RELAY_UNKNOWN_LABEL = '设备未上报中继状态'

/**
 * 设备端原始 `relay` 对象 → Manager 摘要口径。
 * 与 manager/src/api/egress.rs 的 relay_summary 完全一致：enabled && listeners>0 才算 active。
 * 缺 enabled（旧设备 / 字段形状不对）时返回 null，界面显示占位而非「未启用」。
 */
export function relaySummaryFromState(
  raw: DeviceRelayState | null | undefined
): DeviceRelaySummary | null {
  if (!raw || typeof raw.enabled !== 'boolean') return null
  const listeners = Array.isArray(raw.listeners)
    ? raw.listeners.filter((item) => item?.listening === true).length
    : 0
  return {
    enabled: raw.enabled,
    active: raw.enabled && listeners > 0,
    listeners,
    firewallApplied: raw.firewall?.applied === true,
  }
}

/**
 * 详情页优先用设备端原始对象（比列表摘要新一次请求），没有就退回列表摘要。
 * 两者都没有 = 设备没上报。
 */
export function relaySummaryForDevice(
  deviceState: DeviceEgressDeviceState | null | undefined,
  fallback: DeviceRelaySummary | null | undefined
): DeviceRelaySummary | null {
  return relaySummaryFromState(deviceState?.relay) ?? fallback ?? null
}

/** 四态判定：null 永远是 unknown，绝不当成 disabled。 */
export function relayState(relay: DeviceRelaySummary | null | undefined): RelayViewState {
  if (!relay) return 'unknown'
  if (!relay.enabled) return 'disabled'
  return relay.active ? 'active' : 'idle'
}

/** 徽标文案：设备卡与详情共用同一套词。 */
export function relayLabel(relay: DeviceRelaySummary | null | undefined): string {
  switch (relayState(relay)) {
    case 'active':
      return '经设备中继'
    case 'idle':
      return '中继已启用 · 未监听'
    case 'disabled':
      return '中继未启用'
    default:
      return RELAY_UNKNOWN_LABEL
  }
}

/** 徽标配色：active 绿、idle/disabled 琥珀、unknown 灰。 */
export function relayTagClass(relay: DeviceRelaySummary | null | undefined): string {
  switch (relayState(relay)) {
    case 'active':
      return 'tag ok'
    case 'unknown':
      return 'tag muted'
    default:
      return 'tag warn'
  }
}

/**
 * 纯文字徽标配色（设备详情 info-box 里用）。
 * 那里 `.tag` 会被 `.info-box span` 的字号/颜色覆盖，所以改用 strong 的色调类。
 */
export function relayToneClass(relay: DeviceRelaySummary | null | undefined): string {
  switch (relayState(relay)) {
    case 'active':
      return 'relay-ok'
    case 'unknown':
      return 'relay-muted'
    default:
      return 'relay-warn'
  }
}

/** 悬停说明：把 listeners / uid 防火墙的真相说清楚，不用点进设备看。 */
export function relayTitle(relay: DeviceRelaySummary | null | undefined): string {
  switch (relayState(relay)) {
    case 'active':
      return `设备内中继正在转发：${relay?.listeners ?? 0} 个端口在监听，uid 防火墙${relay?.firewallApplied ? '已生效' : '未生效'}`
    case 'idle':
      return '[relay].enabled=true，但没有任何 listener 在 accept：带凭据代理暂时不可用'
    case 'disabled':
      return '[relay].enabled=false：设备端不启动本地中继，带凭据代理无法生效'
    default:
      return '设备离线、版本过低或本次读取失败：没有 relay 字段，不沿用旧值'
  }
}

/** 一行补充说明（卡片右侧 / 详情 info-box 用）。 */
export function relayDetail(relay: DeviceRelaySummary | null | undefined): string {
  switch (relayState(relay)) {
    case 'active':
      return `${relay?.listeners ?? 0} 个端口在监听 · uid 防火墙${relay?.firewallApplied ? '已生效' : '未生效'}`
    case 'idle':
      return '端口已分配但没有 listener 在 accept'
    case 'disabled':
      return '[relay].enabled=false'
    default:
      return '没有 relay 字段（离线 / 旧设备 / 读取失败）'
  }
}

/** 设备端分配的本地中继端点；没有端口时返回 null（不编 127.0.0.1:0）。 */
export function localRelayEndpoint(port: number | null | undefined): string | null {
  return typeof port === 'number' && Number.isInteger(port) && port > 0 ? `127.0.0.1:${port}` : null
}

/** 代理池三态文案：true=中继中、false=有端口未监听、null=未上报。 */
export function relayActiveLabel(relayActive: boolean | null | undefined): string {
  if (relayActive === true) return '中继中'
  if (relayActive === false) return '已分配端口 · 未监听'
  return RELAY_UNKNOWN_LABEL
}

/** 代理池三态配色（与设备卡同一套语义）。 */
export function relayActiveTagClass(relayActive: boolean | null | undefined): string {
  if (relayActive === true) return 'tag ok'
  if (relayActive === false) return 'tag warn'
  return 'tag muted'
}

/** 代理池行内补充说明。 */
export function relayActiveDetail(relayActive: boolean | null | undefined): string {
  if (relayActive === true) return '设备 listener 在 accept，凭据只在设备内使用'
  if (relayActive === false) return '端口已分配但没有 listener 在 accept'
  return '绑定设备没有上报 relay（离线 / 旧设备）'
}
