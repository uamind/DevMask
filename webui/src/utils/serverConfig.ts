// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

/**
 * server.toml 的纯文本解析/编辑 + 本机地址解析。
 *
 * 为什么用逐行正则而不是 TOML 解析器重排：server.toml 里有大量注释（默认模板
 * 49 行里一半是说明），并且可能有本版本还不认识的字段。整体重写会丢注释、打乱
 * 顺序，也会把未知字段写没。这里只改 `bind` / `allow_lan` 两个值，其余文本原样保留。
 *
 * 本文件不依赖任何浏览器/Node API，可直接用
 * `node --experimental-strip-types` 跑单测。
 */

export interface ServerSettings {
  /** 原始 bind 值（形如 `0.0.0.0:8686`）。 */
  bind: string
  /** bind 里的主机部分。 */
  host: string
  /** bind 里的端口；解析失败时为默认 8686。 */
  port: number
  /** `allow_lan` 是否打开。 */
  allowLan: boolean
  /** 文件里是否已有 `[[tokens]]`。 */
  hasToken: boolean
  /** `[[tokens]]` 数量。 */
  tokenCount: number
}

export interface ApplyServerSettingsOptions {
  allowLan: boolean
  port: number
}

export const DEFAULT_SERVER_PORT = 8686
export const LOOPBACK_HOST = '127.0.0.1'
export const ANY_HOST = '0.0.0.0'

const BIND_LINE_RE = /^(\s*bind\s*=\s*")([^"]*)("[^\n]*)$/m
const ALLOW_LAN_LINE_RE = /^(\s*allow_lan\s*=\s*)(true|false)([^\n]*)$/m
const TOKEN_TABLE_RE = /^\s*\[\[\s*tokens\s*\]\]\s*(?:#.*)?$/
const TABLE_HEADER_RE = /^\s*\[/

/** 端口是否合法（1-65535 的整数）。 */
export function isValidPort(value: number | string): boolean {
  const parsed = typeof value === 'number' ? value : Number(value)
  return Number.isInteger(parsed) && parsed >= 1 && parsed <= 65535
}

/** 归一化端口：非法值退回 8686（调用方应先用 isValidPort 提示用户）。 */
export function normalizePort(value: number | string): number {
  return isValidPort(value) ? Math.trunc(Number(value)) : DEFAULT_SERVER_PORT
}

function splitBind(bind: string): { host: string; port: number } {
  const trimmed = bind.trim()
  const separator = trimmed.lastIndexOf(':')
  if (separator <= 0) {
    return { host: LOOPBACK_HOST, port: DEFAULT_SERVER_PORT }
  }

  const host =
    trimmed
      .slice(0, separator)
      .replace(/^\[|\]$/g, '')
      .trim() || LOOPBACK_HOST
  const port = Number.parseInt(trimmed.slice(separator + 1).trim(), 10)
  return { host, port: isValidPort(port) ? port : DEFAULT_SERVER_PORT }
}

/** 解析 server.toml：只取 UI 需要的字段，缺什么用什么默认值。 */
export function parseServerSettings(text: string): ServerSettings {
  const bindMatch = BIND_LINE_RE.exec(text)
  const bind = bindMatch ? bindMatch[2].trim() : `${LOOPBACK_HOST}:${DEFAULT_SERVER_PORT}`
  const { host, port } = splitBind(bind)

  const allowMatch = ALLOW_LAN_LINE_RE.exec(text)
  const allowLan = allowMatch ? allowMatch[2] === 'true' : false

  const tokenCount = text.split('\n').filter((line) => TOKEN_TABLE_RE.test(line)).length

  return { bind, host, port, allowLan, hasToken: tokenCount > 0, tokenCount }
}

function appendLine(text: string, line: string): string {
  const base = text === '' || text.endsWith('\n') ? text : `${text}\n`
  return `${base}${line}\n`
}

/**
 * 只改 `bind` 与 `allow_lan`。
 *
 * - `allowLan=true` → `bind = "0.0.0.0:<port>"` + `allow_lan = true`；
 * - `allowLan=false` → `bind = "127.0.0.1:<port>"` + `allow_lan = false`；
 * - 缺少这两个键时追加到文件尾部（默认模板里两者都有，属于兜底）；
 * - 其余行（注释、未知字段、[[tokens]]）逐字保留。
 */
export function applyServerSettings(text: string, options: ApplyServerSettingsOptions): string {
  const port = normalizePort(options.port)
  const host = options.allowLan ? ANY_HOST : LOOPBACK_HOST
  const bindValue = `${host}:${port}`
  const allowValue = options.allowLan ? 'true' : 'false'

  let result = text

  if (BIND_LINE_RE.test(result)) {
    result = result.replace(
      BIND_LINE_RE,
      (_match, prefix: string, _value: string, suffix: string) => {
        return `${prefix}${bindValue}${suffix}`
      }
    )
  } else {
    result = appendLine(result, `bind = "${bindValue}"`)
  }

  if (ALLOW_LAN_LINE_RE.test(result)) {
    result = result.replace(
      ALLOW_LAN_LINE_RE,
      (_match, prefix: string, _value: string, suffix: string) => {
        return `${prefix}${allowValue}${suffix}`
      }
    )
  } else {
    result = appendLine(result, `allow_lan = ${allowValue}`)
  }

  return result
}

/**
 * 删掉所有 `[[tokens]]` 表（含其中的注释行），其余内容不动。
 *
 * 用于「重置访问 Token」：先清空旧 Token，再写入新的一条。
 */
export function stripTokenBlocks(text: string): string {
  const kept: string[] = []
  let insideTokenTable = false

  for (const line of text.split('\n')) {
    if (TABLE_HEADER_RE.test(line)) {
      insideTokenTable = TOKEN_TABLE_RE.test(line)
      if (insideTokenTable) {
        // 去掉紧邻的空行，避免删表后留下连续空行
        if (kept.length > 0 && kept[kept.length - 1].trim() === '') {
          kept.pop()
        }
        continue
      }
    }

    if (insideTokenTable) {
      continue
    }

    kept.push(line)
  }

  return kept.join('\n').replace(/\n{3,}/g, '\n\n')
}

/** 追加一条 UI 生成的 Token（id=ui、name=owner、全量 scopes），哈希统一带 sha256: 前缀。 */
export function appendTokenBlock(text: string, hash: string): string {
  const normalizedHash = hash.startsWith('sha256:') ? hash : `sha256:${hash}`
  const base = text.trimEnd()
  const block = [
    '',
    '[[tokens]]',
    'id = "ui"',
    'name = "owner"',
    `hash = "${normalizedHash}"`,
    'scopes = ["read", "write", "ops", "admin"]',
    '',
  ].join('\n')

  return base === '' ? block.trimStart() : `${base}\n${block}`
}

/** 局域网可访问地址（`http://<ip>:<port>`）。 */
export function formatLanUrls(addresses: string[], port: number): string[] {
  const normalized = normalizePort(port)
  return addresses.map((address) => `http://${address}:${normalized}`)
}

function isUsableLanIp(ip: string): boolean {
  if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(ip)) return false
  if (ip.startsWith('127.') || ip.startsWith('169.254.')) return false
  return true
}

/**
 * 解析 `ip -4 addr` 输出，取 wlan、eth、ap、br 接口上的 IPv4。
 *
 * 同时兼容多行与 `-o` 单行格式（单行里 iface 与 inet 在同一行）。
 */
export function parseIpAddrOutput(text: string): string[] {
  const addresses: string[] = []
  let iface = ''

  for (const line of text.split('\n')) {
    const header = /^\d+:\s+([^:@\s]+)/.exec(line)
    if (header) {
      iface = header[1]
    }

    const inet = /\binet\s+(\d{1,3}(?:\.\d{1,3}){3})/.exec(line)
    if (!inet) continue

    const ip = inet[1]
    if (!isUsableLanIp(ip)) continue
    if (iface && !/^(wlan|eth|ap|br|swlan|wifi)/i.test(iface)) continue

    addresses.push(ip)
  }

  return Array.from(new Set(addresses))
}

/** 解析 `getprop` 输出里的 `dhcp.*.ipaddress`（老设备上没有 `ip` 命令时兜底）。 */
export function parseDhcpProps(text: string): string[] {
  const addresses: string[] = []

  for (const line of text.split('\n')) {
    const match = /^\[([^\]]+)\]:\s*\[([^\]]*)\]/.exec(line.trim())
    if (!match) continue

    const [, key, value] = match
    if (!/^dhcp\.(wlan|eth)\d*\.ipaddress$/.test(key)) continue
    if (!isUsableLanIp(value.trim())) continue

    addresses.push(value.trim())
  }

  return Array.from(new Set(addresses))
}
