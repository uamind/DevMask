// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

import { parse as parseToml, stringify as stringifyToml } from 'smol-toml'

/**
 * 出口代理（egress）：线上形状 + 纯函数。
 *
 * 契约：docs/plans/egress-contract.md §1（端点）、§2（文件与校验）、§4（探测）。
 *
 * 三条硬规则（写在这里，任何实现都不要绕开）：
 * 1. 凭据只有一个信号：响应里没有 username / password，只有 hasCredentials；密码输入框永远不回显已存值。
 * 2. egress revision 只认 X-DF-Egress-Revision 响应头或响应体的 revision。
 *    X-DF-Revision / ETag 是 config revision，绝不能拿来当 egress revision，
 *    否则一次 egress 写会把 config revision 缓存弄脏，下一次写配置必然 412。
 * 3. 设备级（方案 A）只承载 host:port，不支持认证；带凭据的定义只对应用级有效。
 */

/** egress.toml 的固定位置（与 config.toml 同目录，0600）。 */
export const EGRESS_PATH = '/data/adb/device_faker/config/egress.toml'

/** egress revision 的专用响应头（契约 §1.1）。 */
export const EGRESS_REVISION_HEADER = 'X-DF-Egress-Revision'

/** 强制直连标记（应用级用；设备级不允许）。 */
export const DIRECT_MARKER = '__DIRECT__'

/** 默认探测目标（契约 §4.3）。 */
export const DEFAULT_PROBE_URL = 'http://checkip.amazonaws.com/'

/** 默认 / 允许的探测超时（秒）。 */
export const DEFAULT_PROBE_TIMEOUT_SECONDS = 5
export const MIN_PROBE_TIMEOUT_SECONDS = 1
export const MAX_PROBE_TIMEOUT_SECONDS = 30

/** 代理名正则（契约 §2.5）；与模板名规则相同。 */
export const EGRESS_NAME_PATTERN = /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$/

/** 代理定义条数上限（契约 §2.6）。 */
export const MAX_PROXY_COUNT = 64
/** host / username / password / probe URL 长度上限。 */
export const MAX_HOST_LENGTH = 253
export const MAX_USERNAME_LENGTH = 128
export const MAX_PASSWORD_LENGTH = 256
export const MAX_PROBE_URL_LENGTH = 512

export type EgressProxyType = 'http' | 'socks5'
export type EgressMode = 'proxy' | 'direct'
export type EgressProbeScope = 'device' | 'app'
export type EgressProbeVia = 'proxy' | 'direct'

/** 脱敏代理摘要（GET /egress 的 proxies[] / upstream）。 */
export interface EgressProxySummary {
  name: string
  type: EgressProxyType
  host: string
  port: number
  /** 唯一的凭据信号；值永远不会出现。 */
  hasCredentials: boolean
}

/** 应用级覆盖（只含显式写了 egress 的 [[apps]]）。 */
export interface EgressAppView {
  package: string
  egress: string
  effective: EgressMode
  proxy: string | null
}

/** 方案 A 的读回值（settings + dumpsys 交叉验证）。 */
export interface EgressReadback {
  settingsValue: string | null
  dumpsysGlobalProxy: string | null
  matches: boolean
  at: string
}

/** health.lastProbe。 */
export interface EgressLastProbe {
  ok: boolean
  exitIp: string | null
  latencyMs: number
  checkedAt: string
  scope: EgressProbeScope
  via: EgressProbeVia
  proxy: string | null
  upstreamStatus: number | null
  errorCode: string | null
}

/** fail-open 状态（只影响链路，不影响探测结论）。 */
export interface EgressHealth {
  degraded: boolean
  degradedSince: string | null
  degradedReason: 'probe_failed' | 'settings_mismatch' | null
  lastProbe: EgressLastProbe | null
}

/** egress.toml 元数据（不给内容）。 */
export interface EgressFileInfo {
  exists: boolean
  size: number
  mtime: string | null
}

/** 警告（扁平：code / message / details）。 */
export interface EgressWarning {
  code: string
  message: string
  details?: unknown
}

/** GET /api/v1/egress 的响应（管理器模式由文件推导，字段尽量对齐）。 */
export interface EgressSnapshot {
  /** egress revision；管理器模式下无法合成，留空并由 revisionAvailable=false 标注。 */
  revision: string
  /** config.toml 自己的 revision（仅展示；管理器模式为 ''）。 */
  configRevision: string
  configured: boolean
  mode: EgressMode
  effectiveMode: EgressMode
  default: string | null
  failOpen: boolean
  probeUrl: string
  probeTimeoutSeconds: number
  exclusionList: string[]
  upstream: EgressProxySummary | null
  proxies: EgressProxySummary[]
  apps: EgressAppView[]
  /** 方案 A 读回是否与期望一致（管理器模式由 shell 读 settings 计算）。 */
  applied: boolean
  readback: EgressReadback
  health: EgressHealth
  egressFile: EgressFileInfo
  warnings: EgressWarning[]
  /** false = 该运行时拿不到合成 revision（管理器模式）。 */
  revisionAvailable: boolean
  /** false = 该运行时读不到 settings 读回值。 */
  readbackAvailable: boolean
}

/** POST /api/v1/egress/probe 请求体（全字段可选）。 */
export interface EgressProbeRequest {
  /** null / 省略 = 设备级链路；给包名 = 按该应用生效代理探测（approximate=true）。 */
  package?: string | null
  /** 临时覆盖探测 URL。 */
  url?: string | null
}

/** POST /api/v1/egress/probe 成功响应（失败走错误信封）。 */
export interface EgressProbeResult {
  ok: boolean
  exitIp: string
  latencyMs: number
  checkedAt: string
  scope: EgressProbeScope
  via: EgressProbeVia
  proxy: string | null
  url: string
  upstreamStatus: number | null
  /** 应用级探测恒 true：服务进程内的链路，不代表目标应用进程。 */
  approximate: boolean
  warnings: EgressWarning[]
}

/** 一条代理定义（PUT /egress 的请求侧）。 */
export interface EgressProxyInput {
  name: string
  type: EgressProxyType
  host: string
  port: number
  /** 省略 = 保留已存值；null = 清除；字符串 = 覆盖。 */
  username?: string | null
  password?: string | null
}

/** PUT /api/v1/egress 请求体（字段省略 = 不改）。 */
export interface EgressUpdateInput {
  default?: string | null
  failOpen?: boolean
  probeUrl?: string | null
  probeTimeoutSeconds?: number
  proxies?: EgressProxyInput[]
  apps?: Array<{ package: string; egress?: string | null }> | null
}

/** PUT / DELETE /egress 的响应（WriteResult 的超集）。 */
export interface EgressWriteResult {
  revision: string
  previousRevision: string | null
  configRevision: string
  dryRun: boolean
  changed: boolean
  backupId: string | null
  mode: EgressMode
  effectiveMode: EgressMode
  default: string | null
  applied: boolean
  upstream: EgressProxySummary | null
  readback: EgressReadback
  health: EgressHealth
  warnings: EgressWarning[]
  revisionAvailable: boolean
}

/** 表单值（设置页「网络出口」卡片）。 */
export interface EgressFormValues {
  enabled: boolean
  name: string
  type: EgressProxyType
  host: string
  port: number
  username: string
  password: string
  /** 勾选后把 egress.toml 里该定义的凭据清掉（双态语义里的 null）。 */
  clearCredentials: boolean
  probeUrl: string
  probeTimeoutSeconds: number
  failOpen: boolean
}

/** 表单校验失败项（message 留空时由界面按 code 翻译）。 */
export interface EgressFormError {
  field:
    'name' | 'type' | 'host' | 'port' | 'username' | 'password' | 'probeUrl' | 'probeTimeoutSeconds'
  code: 'invalid_egress_name' | 'invalid_egress_proxy' | 'invalid_probe_url' | 'out_of_range'
  message?: string
}

/** egress.toml 的解析结果（管理器模式读写用）。 */
export interface EgressFileModel {
  proxies: EgressProxyInput[]
  probe: { url: string; timeoutSeconds: number; failOpen: boolean }
  /** 解析失败时给出原因；调用方据此提示「保存会覆盖修复」。 */
  parseError: string | null
}

/** config.toml 里与 egress 有关的引用（[egress].default + [[apps]].egress）。 */
export interface EgressConfigRefs {
  default: string | null
  apps: Array<{ package: string; egress: string | null }>
  parseError: string | null
}

export function emptyEgressFile(): EgressFileModel {
  return {
    proxies: [],
    probe: {
      url: DEFAULT_PROBE_URL,
      timeoutSeconds: DEFAULT_PROBE_TIMEOUT_SECONDS,
      failOpen: true,
    },
    parseError: null,
  }
}

export function emptyConfigRefs(): EgressConfigRefs {
  return { default: null, apps: [], parseError: null }
}

/** 代理名是否合法（契约 §2.5；__DIRECT__ 是保留值）。 */
export function isValidEgressName(name: string): boolean {
  return name !== DIRECT_MARKER && EGRESS_NAME_PATTERN.test(name)
}

function isIpv6Literal(host: string): boolean {
  return host.includes(':') && /^[0-9A-Fa-f:.]+$/.test(host)
}

/**
 * 主机校验（与 server/src/egress/request.rs::validate_host 同源）。
 * 返回错误码；设备级场景另有一条 IPv6 限制。
 */
export function validateEgressHost(
  host: string,
  options: { deviceLevel: boolean }
): 'invalid_egress_proxy' | null {
  if (!host) return 'invalid_egress_proxy'
  if (host.length > MAX_HOST_LENGTH) return 'invalid_egress_proxy'
  if (host.includes('://') || host.includes('/') || host.includes('@')) {
    return 'invalid_egress_proxy'
  }
  if (host.includes(' ')) return 'invalid_egress_proxy'

  const ipv6 = isIpv6Literal(host)
  if (options.deviceLevel && ipv6) {
    // settings 里只写 host:port，IPv6 需要方括号，方案 A 不支持
    return 'invalid_egress_proxy'
  }
  if (ipv6) return null
  if (host.includes(':')) return 'invalid_egress_proxy'
  if (host.startsWith('.') || host.endsWith('.') || host.includes('..')) {
    return 'invalid_egress_proxy'
  }
  if (!/^[A-Za-z0-9.-]+$/.test(host)) return 'invalid_egress_proxy'
  return null
}

/** 探测 URL 校验（契约 §4.3：只允许 http://、≤512、无用户信息）。 */
export function isValidProbeUrl(url: string): boolean {
  const trimmed = url.trim()
  if (!trimmed || trimmed.length > MAX_PROBE_URL_LENGTH) return false
  if (!trimmed.startsWith('http://')) return false
  const rest = trimmed.slice('http://'.length)
  const authority = rest.split('/')[0] ?? ''
  if (!authority) return false
  if (authority.includes('@')) return false
  return true
}

/**
 * 表单校验（契约 §2.6 的前端副本）。
 *
 * 只做「不值得发一次网络请求」的本地检查；服务端仍会完整校验，冲突/重名等以服务端为准。
 */
export function validateEgressForm(
  values: EgressFormValues,
  options: { allowIpv6Host: boolean } = { allowIpv6Host: false }
): EgressFormError[] {
  const errors: EgressFormError[] = []
  const name = values.name.trim()

  if (!values.enabled && !name && !values.host.trim()) {
    // 未启用且没有任何定义：允许「什么都不配」，保存等价于恢复直连
    return errors
  }

  if (!isValidEgressName(name)) {
    errors.push({ field: 'name', code: 'invalid_egress_name' })
  }

  const hostError = validateEgressHost(values.host.trim(), {
    deviceLevel: values.enabled && !options.allowIpv6Host,
  })
  if (hostError) {
    errors.push({ field: 'host', code: hostError })
  }

  if (!Number.isInteger(values.port) || values.port < 1 || values.port > 65535) {
    errors.push({ field: 'port', code: 'invalid_egress_proxy' })
  }

  if (values.username.length > MAX_USERNAME_LENGTH) {
    errors.push({ field: 'username', code: 'invalid_egress_proxy' })
  }
  if (values.password.length > MAX_PASSWORD_LENGTH) {
    errors.push({ field: 'password', code: 'invalid_egress_proxy' })
  }

  if (values.probeUrl.trim() && !isValidProbeUrl(values.probeUrl)) {
    errors.push({ field: 'probeUrl', code: 'invalid_probe_url' })
  }

  const timeout = values.probeTimeoutSeconds
  if (
    !Number.isInteger(timeout) ||
    timeout < MIN_PROBE_TIMEOUT_SECONDS ||
    timeout > MAX_PROBE_TIMEOUT_SECONDS
  ) {
    errors.push({ field: 'probeTimeoutSeconds', code: 'out_of_range' })
  }

  return errors
}

/** 在脱敏列表里按名字找定义。 */
export function findProxySummary(
  proxies: readonly EgressProxySummary[],
  name: string | null | undefined
): EgressProxySummary | null {
  if (!name) return null
  return proxies.find((proxy) => proxy.name === name) ?? null
}

/** 设备级期望写入 settings 的值：host:port 或直连 :0（契约 §3.2）。 */
export function expectedSettingsValue(
  defaultName: string | null,
  proxies: readonly EgressProxySummary[]
): string {
  const proxy = findProxySummary(proxies, defaultName)
  return proxy ? `${proxy.host}:${proxy.port}` : ':0'
}

/**
 * 把表单合并进「PUT /egress 的 proxies 数组」。
 *
 * - 保留其它定义（只改/替换本条），避免把 Manager 下发的其它代理顺手删掉；
 * - 改名时移除旧定义（语义上这是同一张卡片，不是新增一条）；
 * - 凭据三态：勾了清除 → null；填了新值 → 字符串；否则省略（服务端保留已存值）。
 */
export function buildProxyInputs(
  current: readonly EgressProxySummary[],
  values: EgressFormValues,
  options: { previousName?: string | null } = {}
): EgressProxyInput[] {
  const name = values.name.trim()
  const previousName = options.previousName?.trim() || null

  const entry: EgressProxyInput = {
    name,
    type: values.type,
    host: values.host.trim(),
    port: values.port,
  }

  if (values.clearCredentials) {
    entry.username = null
    entry.password = null
  } else {
    const username = values.username.trim()
    const password = values.password
    if (username) entry.username = username
    if (password) entry.password = password
  }

  const result: EgressProxyInput[] = []
  let replaced = false

  for (const proxy of current) {
    if (proxy.name === name) {
      result.push(entry)
      replaced = true
      continue
    }
    if (previousName && proxy.name === previousName) {
      // 改名：旧定义直接消失（避免留下永不引用的孤儿定义）
      continue
    }
    result.push({
      name: proxy.name,
      type: proxy.type,
      host: proxy.host,
      port: proxy.port,
    })
  }

  if (!replaced) {
    result.push(entry)
  }

  return result
}

/** 设备级 default 跟着改名走：原来指向旧名字时改成新名字。 */
export function defaultAfterRename(
  currentDefault: string | null,
  previousName: string | null,
  nextName: string
): string | null {
  if (!currentDefault) return null
  if (previousName && currentDefault === previousName) return nextName
  return currentDefault
}

/** 解析 egress.toml 文本（管理器模式读路径）。 */
export function parseEgressFile(text: string): EgressFileModel {
  const model = emptyEgressFile()
  if (!text.trim()) return model

  let document: unknown
  try {
    document = parseToml(text)
  } catch (error) {
    model.parseError = error instanceof Error ? error.message : String(error)
    return model
  }

  if (!isRecord(document)) {
    model.parseError = 'egress.toml 顶层不是表'
    return model
  }

  if (typeof document.version === 'number' && document.version !== 1) {
    model.parseError = `不支持的 version: ${String(document.version)}`
    return model
  }

  const proxies = document.proxies
  if (isRecord(proxies)) {
    for (const [name, raw] of Object.entries(proxies)) {
      if (!isRecord(raw)) continue
      const type = raw.type === 'socks5' ? 'socks5' : raw.type === 'http' ? 'http' : null
      const host = typeof raw.host === 'string' ? raw.host : ''
      const port = typeof raw.port === 'number' ? raw.port : 0
      if (!type || !host || !port) {
        model.parseError = model.parseError ?? `代理定义不完整：${name}`
        continue
      }
      const entry: EgressProxyInput = { name, type, host, port }
      if (typeof raw.username === 'string' && raw.username) entry.username = raw.username
      if (typeof raw.password === 'string' && raw.password) entry.password = raw.password
      model.proxies.push(entry)
    }
  }

  const probe = document.probe
  if (isRecord(probe)) {
    if (typeof probe.url === 'string' && probe.url.trim()) {
      model.probe.url = probe.url.trim()
    }
    if (
      typeof probe.timeout_seconds === 'number' &&
      Number.isInteger(probe.timeout_seconds) &&
      probe.timeout_seconds >= MIN_PROBE_TIMEOUT_SECONDS &&
      probe.timeout_seconds <= MAX_PROBE_TIMEOUT_SECONDS
    ) {
      model.probe.timeoutSeconds = probe.timeout_seconds
    }
    if (typeof probe.fail_open === 'boolean') {
      model.probe.failOpen = probe.fail_open
    }
  }

  return model
}

/** 把模型渲染成 egress.toml 文本（管理器模式写路径；文件由模块托管，不保留注释）。 */
export function renderEgressFile(model: EgressFileModel): string {
  const proxies: Record<string, Record<string, unknown>> = {}
  for (const proxy of model.proxies) {
    const entry: Record<string, unknown> = {
      type: proxy.type,
      host: proxy.host,
      port: proxy.port,
    }
    const username = typeof proxy.username === 'string' ? proxy.username : ''
    const password = typeof proxy.password === 'string' ? proxy.password : ''
    if (username) entry.username = username
    if (password) entry.password = password
    proxies[proxy.name] = entry
  }

  const document: Record<string, unknown> = { version: 1 }
  if (Object.keys(proxies).length > 0) document.proxies = proxies
  document.probe = {
    url: model.probe.url,
    timeout_seconds: model.probe.timeoutSeconds,
    fail_open: model.probe.failOpen,
  }

  const header = [
    '# egress.toml —— 出口代理定义与探测策略（权限 0600，含密码，请勿分享）。',
    '# 由 device_faker 服务端 / WebUI 维护；手工编辑后重启服务生效。',
  ].join('\n')

  return `${header}\n${stringifyToml(document).trimEnd()}\n`
}

/** 读 config.toml 里与 egress 有关的引用（管理器模式）。 */
export function readConfigEgressRefs(text: string): EgressConfigRefs {
  const refs = emptyConfigRefs()
  if (!text.trim()) return refs

  let document: unknown
  try {
    document = parseToml(text)
  } catch (error) {
    refs.parseError = error instanceof Error ? error.message : String(error)
    return refs
  }
  if (!isRecord(document)) return refs

  const egress = document.egress
  if (isRecord(egress) && typeof egress.default === 'string' && egress.default.trim()) {
    refs.default = egress.default.trim()
  }

  const apps = document.apps
  if (Array.isArray(apps)) {
    for (const item of apps) {
      if (!isRecord(item) || typeof item.package !== 'string') continue
      const value = typeof item.egress === 'string' ? item.egress.trim() : ''
      if (!value) continue
      refs.apps.push({ package: item.package, egress: value })
    }
  }

  return refs
}

/**
 * 在 config.toml 文本里设置/清除 [egress].default（管理器模式写路径）。
 *
 * 逐行编辑而不是整体序列化：config.toml 是 0644、用户可手改的文件，
 * 保住注释与键顺序比一次漂亮的序列化重要得多。
 */
export function setConfigEgressDefault(text: string, name: string | null): string {
  const lines = text.split(/\r?\n/)
  const headerIndex = lines.findIndex((line) => isTableHeader(line, '[egress]'))

  if (headerIndex === -1) {
    if (!name) return text
    const trimmed = text.replace(/\s+$/, '')
    const prefix = trimmed ? `${trimmed}\n\n` : ''
    return `${prefix}[egress]\ndefault = ${quoteTomlString(name)}\n`
  }

  const endIndex = findSectionEnd(lines, headerIndex)
  const defaultOffset = lines
    .slice(headerIndex + 1, endIndex)
    .findIndex((line) => /^\s*default\s*=/.test(line))

  const next = [...lines]

  if (name) {
    const rendered = `default = ${quoteTomlString(name)}`
    if (defaultOffset === -1) {
      next.splice(headerIndex + 1, 0, rendered)
    } else {
      next[headerIndex + 1 + defaultOffset] = rendered
    }
    return next.join('\n')
  }

  if (defaultOffset !== -1) {
    next.splice(headerIndex + 1 + defaultOffset, 1)
  }

  // 表里没有有效行时连表头一起删（等价于 default = null）
  const remaining = next
    .slice(headerIndex + 1, findSectionEnd(next, headerIndex))
    .filter((line) => line.trim() && !line.trim().startsWith('#'))
  if (remaining.length === 0) {
    const end = findSectionEnd(next, headerIndex)
    next.splice(headerIndex, end - headerIndex)
    while (headerIndex > 0 && !next[headerIndex - 1].trim()) {
      next.splice(headerIndex - 1, 1)
    }
  }

  return next.join('\n')
}

/**
 * 清掉 config.toml 里所有出口引用：[egress] 整段 + [[apps]] 里的 egress 键。
 *
 * 与 DELETE /egress 的清除清单一致（契约 §1.5 第 2、3 项）；不动其它机型字段。
 */
export function clearConfigEgress(text: string): string {
  const withoutDefault = setConfigEgressDefault(text, null)
  const lines = withoutDefault.split(/\r?\n/)
  const result: string[] = []
  let insideApps = false

  for (const line of lines) {
    if (/^\s*\[\[/.test(line)) {
      insideApps = isTableHeader(line, '[[apps]]')
      result.push(line)
      continue
    }
    if (/^\s*\[/.test(line)) {
      insideApps = false
      result.push(line)
      continue
    }
    if (insideApps && /^\s*egress\s*=/.test(line)) {
      continue
    }
    result.push(line)
  }

  return result.join('\n')
}

function isTableHeader(line: string, header: string): boolean {
  return line.trim().replace(/\s+#.*$/, '') === header
}

/** 表头之后、下一个表头之前的行下标（不含）。 */
function findSectionEnd(lines: readonly string[], headerIndex: number): number {
  for (let index = headerIndex + 1; index < lines.length; index += 1) {
    if (/^\s*\[/.test(lines[index])) return index
  }
  return lines.length
}

function quoteTomlString(value: string): string {
  return JSON.stringify(value)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** 值可能是任意 JSON；统一窄化成基础类型，保证界面不因字段缺失而崩。 */
function asString(value: unknown): string | null {
  return typeof value === 'string' && value ? value : null
}

function asNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function asStringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string' && item.length > 0)
    : []
}

function asMode(value: unknown, fallback: EgressMode): EgressMode {
  return value === 'proxy' || value === 'direct' ? value : fallback
}

function asProxySummary(value: unknown): EgressProxySummary | null {
  if (!isRecord(value)) return null
  const name = asString(value.name)
  const host = asString(value.host)
  const port = asNumber(value.port)
  if (!name || !host || port === null) return null
  return {
    name,
    type: value.type === 'socks5' ? 'socks5' : 'http',
    host,
    port,
    hasCredentials: value.hasCredentials === true,
  }
}

function asAppView(value: unknown): EgressAppView | null {
  if (!isRecord(value)) return null
  const packageName = asString(value.package)
  if (!packageName) return null
  const egress = asString(value.egress) ?? ''
  return {
    package: packageName,
    egress,
    effective: asMode(value.effective, egress === DIRECT_MARKER ? 'direct' : 'proxy'),
    proxy: asString(value.proxy),
  }
}

function asWarnings(value: unknown): EgressWarning[] {
  if (!Array.isArray(value)) return []
  return value.flatMap((item) => {
    if (!isRecord(item)) return []
    const code = asString(item.code)
    if (!code) return []
    return [{ code, message: asString(item.message) ?? code, details: item.details }]
  })
}

function asReadback(value: unknown): EgressReadback {
  const record = isRecord(value) ? value : {}
  return {
    settingsValue: asString(record.settingsValue),
    dumpsysGlobalProxy: asString(record.dumpsysGlobalProxy),
    matches: record.matches === true,
    at: asString(record.at) ?? '',
  }
}

function asLastProbe(value: unknown): EgressLastProbe | null {
  if (!isRecord(value)) return null
  return {
    ok: value.ok === true,
    exitIp: asString(value.exitIp),
    latencyMs: asNumber(value.latencyMs) ?? 0,
    checkedAt: asString(value.checkedAt) ?? '',
    scope: value.scope === 'app' ? 'app' : 'device',
    via: value.via === 'direct' ? 'direct' : 'proxy',
    proxy: asString(value.proxy),
    upstreamStatus: asNumber(value.upstreamStatus),
    errorCode: asString(value.errorCode),
  }
}

function asHealth(value: unknown): EgressHealth {
  const record = isRecord(value) ? value : {}
  const reason = record.degradedReason
  return {
    degraded: record.degraded === true,
    degradedSince: asString(record.degradedSince),
    degradedReason: reason === 'probe_failed' || reason === 'settings_mismatch' ? reason : null,
    lastProbe: asLastProbe(record.lastProbe),
  }
}

function asFileInfo(value: unknown): EgressFileInfo {
  const record = isRecord(value) ? value : {}
  return {
    exists: record.exists === true,
    size: asNumber(record.size) ?? 0,
    mtime: asString(record.mtime),
  }
}

/** GET /egress 响应 → EgressSnapshot（字段缺失按契约默认值补齐）。 */
export function normalizeEgressSnapshot(payload: unknown): EgressSnapshot {
  const record = isRecord(payload) ? payload : {}
  const proxies = Array.isArray(record.proxies)
    ? record.proxies.flatMap((item) => {
        const proxy = asProxySummary(item)
        return proxy ? [proxy] : []
      })
    : []
  const apps = Array.isArray(record.apps)
    ? record.apps.flatMap((item) => {
        const app = asAppView(item)
        return app ? [app] : []
      })
    : []
  const readback = asReadback(record.readback)

  return {
    revision: asString(record.revision) ?? '',
    configRevision: asString(record.configRevision) ?? '',
    configured: record.configured === true,
    mode: asMode(record.mode, 'direct'),
    effectiveMode: asMode(record.effectiveMode, 'direct'),
    default: asString(record.default),
    failOpen: record.failOpen !== false,
    probeUrl: asString(record.probeUrl) ?? DEFAULT_PROBE_URL,
    probeTimeoutSeconds: asNumber(record.probeTimeoutSeconds) ?? DEFAULT_PROBE_TIMEOUT_SECONDS,
    exclusionList: asStringArray(record.exclusionList),
    upstream: asProxySummary(record.upstream),
    proxies,
    apps,
    applied: record.applied === true,
    readback,
    health: asHealth(record.health),
    egressFile: asFileInfo(record.egressFile),
    warnings: asWarnings(record.warnings),
    revisionAvailable: true,
    readbackAvailable: true,
  }
}

/** PUT / DELETE /egress 响应 → EgressWriteResult。 */
export function normalizeEgressWriteResult(payload: unknown): EgressWriteResult {
  const record = isRecord(payload) ? payload : {}
  return {
    revision: asString(record.revision) ?? '',
    previousRevision: asString(record.previousRevision),
    configRevision: asString(record.configRevision) ?? '',
    dryRun: record.dryRun === true,
    changed: record.changed === true,
    backupId: asString(record.backupId),
    mode: asMode(record.mode, 'direct'),
    effectiveMode: asMode(record.effectiveMode, 'direct'),
    default: asString(record.default),
    applied: record.applied === true,
    upstream: asProxySummary(record.upstream),
    readback: asReadback(record.readback),
    health: asHealth(record.health),
    warnings: asWarnings(record.warnings),
    revisionAvailable: true,
  }
}

/** POST /egress/probe 响应 → EgressProbeResult。 */
export function normalizeEgressProbeResult(payload: unknown): EgressProbeResult {
  const record = isRecord(payload) ? payload : {}
  return {
    ok: record.ok === true,
    exitIp: asString(record.exitIp) ?? '',
    latencyMs: asNumber(record.latencyMs) ?? 0,
    checkedAt: asString(record.checkedAt) ?? '',
    scope: record.scope === 'app' ? 'app' : 'device',
    via: record.via === 'direct' ? 'direct' : 'proxy',
    proxy: asString(record.proxy),
    url: asString(record.url) ?? '',
    upstreamStatus: asNumber(record.upstreamStatus),
    approximate: record.approximate === true,
    warnings: asWarnings(record.warnings),
  }
}
