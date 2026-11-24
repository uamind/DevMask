// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

import { stringify as stringifyToml } from 'smol-toml'
import { parsePackageUser } from '../utils/package'
import type { InstalledApp } from '../types'
import { clearToken, getToken, requestToken, setToken } from './auth'
import { ApiError, UnsupportedError, isApiError, toApiError } from './errors'
import {
  CONFIG_PATH,
  type Backend,
  type ConfigRevisionInfo,
  type ConfigWriteWarning,
  type DeepLinkOptions,
  type FetchOnlineTemplatesOptions,
  type GetAppsInfoOptions,
  type GetInstalledAppsOptions,
  type ModuleInfo,
  type PackageQueryType,
  type ServerRuntimeInfo,
  type ServerStatus,
  type ServerTokenPair,
  type WriteConfigOptions,
  type WriteConfigResult,
} from './types'

/**
 * 原生服务模式（/api/v1）的传输实现，能力映射见 docs/API.md §10.3。
 *
 * 与 ksu 实现的三点关键差异（都写在方法注释里）：
 * 1. 写配置必须带 `If-Match`：revision 由本模块缓存，读到/写到新值时自动更新；
 *    revision 为空（配置首次创建）时按契约 §2.5 发送 `If-Match: *`，否则发送
 *    `"<revision>"`；
 * 2. 没有任意文件与 shell 能力：`readFile` 只认 config.toml，深链直接抛
 *    `UnsupportedError` 交给 UI 降级；
 * 3. 401 会清空本地 Token 并请求用户重新输入（契约 §3.5），成功后再重试一次。
 */

const DEFAULT_API_BASE = '/api/v1'
const REQUEST_TIMEOUT_MS = 15000
const HEALTHZ_TIMEOUT_MS = 1500
const PAGE_LIMIT = 500
const MAX_PAGES = 20

/** 原生模式下请求的设备属性（§5.22 白名单内的子集，≤50 个）。 */
const HTTP_PROP_KEYS = [
  'ro.product.manufacturer',
  'ro.product.brand',
  'ro.product.model',
  'ro.product.marketname',
  'ro.product.name',
  'ro.product.device',
  'ro.product.product',
  'ro.product.board',
  'ro.product.build.fingerprint',
  'ro.product.vendor.manufacturer',
  'ro.product.vendor.brand',
  'ro.product.vendor.model',
  'ro.product.vendor.marketname',
  'ro.product.vendor.name',
  'ro.product.vendor.device',
  'ro.product.vendor.product',
  'ro.build.fingerprint',
  'ro.build.id',
  'ro.build.characteristics',
  'ro.build.version.release',
  'ro.build.version.sdk',
  'ro.build.version.security_patch',
  'ro.vendor.build.fingerprint',
  'ro.vendor.build.id',
  'ro.vendor.build.characteristics',
  'ro.vendor.build.version.release',
  'ro.vendor.build.version.sdk',
  'ro.vendor.build.security_patch',
  'ro.hardware',
  'ro.board.platform',
]

interface ApiPackageItem {
  package: string
  userIds?: number[]
  installed?: boolean
  isSystem?: boolean
  versionName?: string
  versionCode?: number
  label?: string | null
}

interface ApiPackagesResponse {
  total?: number
  nextCursor?: string | null
  source?: string
  warnings?: unknown[]
  items?: ApiPackageItem[]
}

interface ApiConfigTomlFallback {
  revision?: string
  exists?: boolean
  config?: Record<string, unknown>
}

interface InjectedServer {
  api?: unknown
  version?: unknown
  transport?: unknown
}

/** 读取服务端注入的 window.__DF_SERVER__（契约 §10.1）；不存在返回 null。 */
export function readInjectedServer(): ServerRuntimeInfo | null {
  const injected = (globalThis as { __DF_SERVER__?: InjectedServer }).__DF_SERVER__
  if (!injected || typeof injected !== 'object') {
    return null
  }

  const rawApi = typeof injected.api === 'string' && injected.api ? injected.api : DEFAULT_API_BASE
  return {
    api: rawApi.replace(/\/+$/, '') || DEFAULT_API_BASE,
    version: typeof injected.version === 'string' ? injected.version : '',
    transport: typeof injected.transport === 'string' ? injected.transport : 'http',
  }
}

/** 探测 `/api/v1/healthz`（契约 §5.1：无需认证）。 */
export async function probeHealthz(
  apiBase: string = DEFAULT_API_BASE,
  timeoutMs: number = HEALTHZ_TIMEOUT_MS
): Promise<boolean> {
  try {
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(), timeoutMs)
    try {
      const response = await fetch(`${apiBase.replace(/\/+$/, '')}/healthz`, {
        cache: 'no-store',
        signal: controller.signal,
      })
      return response.ok
    } finally {
      clearTimeout(timer)
    }
  } catch {
    return false
  }
}

export interface HttpBackendOptions {
  /** 注入的运行时信息；缺省时读 window.__DF_SERVER__。 */
  server?: ServerRuntimeInfo | null
}

function stripQuotes(value: string | null): string {
  if (!value) return ''
  let result = value.trim()
  if (result.startsWith('W/')) result = result.slice(2).trim()
  return result.replace(/^"|"$/g, '')
}

function normalizePath(path: string): string {
  return path.replace(/\/{2,}/g, '/').replace(/\/+$/, '')
}

function isConfigPath(path: string): boolean {
  return normalizePath(path) === normalizePath(CONFIG_PATH)
}

/** 原生模式的 `Backend` 实现。 */
export function createHttpBackend(options: HttpBackendOptions = {}): Backend {
  const serverRuntime = options.server ?? readInjectedServer()
  const apiBase = serverRuntime?.api || DEFAULT_API_BASE

  /** 最近一次看到的 revision（写操作的 If-Match 来源）。 */
  let lastRevision = ''
  /** 配置文件是否存在的最近一次观测结果。 */
  let lastExists = false

  function updateRevisionFromResponse(response: Response): void {
    const header = response.headers.get('X-DF-Revision') || response.headers.get('ETag')
    const revision = stripQuotes(header)
    if (revision) {
      lastRevision = revision
    }
  }

  async function send(
    path: string,
    init: RequestInit = {},
    allowAuthRetry = true
  ): Promise<Response> {
    const headers = new Headers(init.headers)
    const token = getToken()
    if (token) {
      headers.set('Authorization', `Bearer ${token}`)
    }
    if (!headers.has('Accept')) {
      headers.set('Accept', 'application/json')
    }

    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS)

    try {
      const response = await fetch(`${apiBase}${path}`, {
        ...init,
        headers,
        cache: 'no-store',
        signal: init.signal ?? controller.signal,
      })

      // 契约 §3.5：401 清空本地 Token，弹输入框后重试一次
      if (response.status === 401 && allowAuthRetry) {
        clearToken()
        const submission = await requestToken(token ? 'unauthorized' : 'missing')
        if (submission?.token) {
          setToken(submission.token, submission.remember)
          return await send(path, init, false)
        }
      }

      if (!response.ok) {
        throw await toApiError(response)
      }

      return response
    } finally {
      clearTimeout(timer)
    }
  }

  async function sendJson<T>(path: string, init: RequestInit = {}): Promise<T> {
    const response = await send(path, init)
    updateRevisionFromResponse(response)
    try {
      return (await response.json()) as T
    } catch {
      throw new ApiError(response.status, {
        code: 'internal_error',
        message: '服务端响应不是合法 JSON',
        retryable: true,
      })
    }
  }

  /** GET /config/revision（契约 §5.5）。 */
  async function getConfigRevision(): Promise<ConfigRevisionInfo> {
    const payload = await sendJson<{
      revision?: string
      exists?: boolean
      mtime?: string | null
      size?: number
    }>('/config/revision')

    const revision = typeof payload.revision === 'string' ? payload.revision : ''
    if (revision) {
      lastRevision = revision
    }
    lastExists = payload.exists !== false
    return {
      revision,
      exists: lastExists,
      // 配置不存在时服务端给 null：保持 null，交给 UI 走「配置缺失」分支，不要塞进日期格式化
      mtime: typeof payload.mtime === 'string' && payload.mtime ? payload.mtime : null,
      size: typeof payload.size === 'number' ? payload.size : 0,
    }
  }

  /** 写操作前确保手里有 revision（避免多一次请求）。 */
  async function ensureRevision(): Promise<string> {
    if (lastRevision) {
      return lastRevision
    }
    const info = await getConfigRevision()
    return info.revision
  }

  /**
   * 写请求的 `If-Match` 取值（契约 §2.5）。
   *
   * - revision 非空 → `"<revision>"`（服务端比对，不符回 412）；
   * - revision 为空（配置文件还不存在 / 服务端没给 revision）→ `*`，即「无条件写」，
   *   用于首次创建；绝不能发 `""`，那会被服务端当成不匹配的版本号拒掉。
   */
  function buildIfMatch(revision: string): string {
    return revision ? `"${revision}"` : '*'
  }

  /**
   * 兼容「`format=toml` 被服务端忽略」的实现（例如 tests/api/mock_server.py 返回
   * ConfigResponse JSON）：把 `config` 对象转回 TOML，让上层的 TOML 解析路径保持一致。
   */
  function configJsonToToml(raw: string): string {
    try {
      const parsed = JSON.parse(raw) as ApiConfigTomlFallback
      if (typeof parsed.revision === 'string' && parsed.revision) {
        lastRevision = parsed.revision
      }
      if (typeof parsed.exists === 'boolean') {
        lastExists = parsed.exists
      }
      const document = parsed.config ?? (parsed as Record<string, unknown>)
      return String(stringifyToml(document as never))
    } catch {
      return raw
    }
  }

  async function readConfigRaw(): Promise<string> {
    const response = await send('/config?format=toml', { headers: { Accept: 'text/plain' } })
    updateRevisionFromResponse(response)
    const contentType = (response.headers.get('Content-Type') || '').toLowerCase()
    const raw = await response.text()
    return contentType.includes('json') ? configJsonToToml(raw) : raw
  }

  function toWriteResult(
    payload: Partial<WriteConfigResult> | null,
    fallbackRevision: string
  ): WriteConfigResult {
    const revision =
      payload && typeof payload.revision === 'string' && payload.revision
        ? payload.revision
        : fallbackRevision
    if (revision) {
      lastRevision = revision
    }
    const warnings = Array.isArray(payload?.warnings)
      ? payload?.warnings.filter(
          (item): item is ConfigWriteWarning =>
            Boolean(item) && typeof item === 'object' && typeof item.code === 'string'
        )
      : []

    return {
      revision,
      previousRevision: payload?.previousRevision ?? null,
      warnings,
      diff: payload?.diff,
      dryRun: payload?.dryRun,
      backupId: payload?.backupId ?? null,
    }
  }

  /** 单次 PUT /config；返回解析后的 WriteResult 并回填 revision。 */
  async function sendWrite(
    content: string,
    query: string,
    ifMatch: string
  ): Promise<WriteConfigResult> {
    const response = await send(`/config${query}`, {
      method: 'PUT',
      headers: {
        'Content-Type': 'application/toml; charset=utf-8',
        'If-Match': ifMatch,
      },
      body: content,
    })
    updateRevisionFromResponse(response)

    let payload: Partial<WriteConfigResult> | null
    try {
      payload = (await response.json()) as Partial<WriteConfigResult>
    } catch {
      payload = null
    }
    const result = toWriteResult(payload, lastRevision)
    lastExists = true
    return result
  }

  async function writeConfigRaw(
    content: string,
    writeOptions: WriteConfigOptions = {}
  ): Promise<WriteConfigResult> {
    const params = new URLSearchParams()
    if (writeOptions.allowDuplicatePackage) params.set('allow_duplicate_package', 'true')
    if (writeOptions.diff) params.set('diff', 'true')
    const query = params.size > 0 ? `?${params.toString()}` : ''

    const revision = await ensureRevision()

    try {
      return await sendWrite(content, query, buildIfMatch(revision))
    } catch (error) {
      if (!isApiError(error)) {
        throw error
      }

      // 契约 §2.5：412 时刷新 revision（details.actual），由界面提示「重试」，不自动合并
      if (error.code === 'config_revision_mismatch') {
        const actual = (error.details as { actual?: unknown } | null)?.actual
        lastRevision = typeof actual === 'string' ? actual : ''
        throw error
      }

      // 428 precondition_required：服务端没拿到可用的 If-Match（例如缓存里的是空串）。
      // 重新读一次 revision 后按同一规则重试一次；仍失败就抛给界面提示，不吞掉。
      if (error.code === 'precondition_required') {
        lastRevision = ''
        const refreshed = await getConfigRevision()
          .then((info) => info.revision)
          .catch(() => '')
        return await sendWrite(content, query, buildIfMatch(refreshed))
      }

      throw error
    }
  }

  async function sendPackages(params: URLSearchParams): Promise<ApiPackageItem[]> {
    const items: ApiPackageItem[] = []
    let cursor: string | null = null

    for (let page = 0; page < MAX_PAGES; page += 1) {
      const query = new URLSearchParams(params)
      query.set('limit', String(PAGE_LIMIT))
      if (cursor) {
        query.set('cursor', cursor)
      }
      const payload = await sendJson<ApiPackagesResponse>(`/packages?${query.toString()}`)
      for (const item of payload.items ?? []) {
        if (item && typeof item.package === 'string' && item.package) {
          items.push(item)
        }
      }
      cursor = payload.nextCursor ?? null
      if (!cursor) {
        break
      }
    }

    return items
  }

  async function fetchAllPackages(): Promise<ApiPackageItem[]> {
    return await sendPackages(new URLSearchParams())
  }

  function buildPackageIndex(items: ApiPackageItem[]): Map<string, ApiPackageItem> {
    const index = new Map<string, ApiPackageItem>()
    for (const item of items) {
      if (!index.has(item.package)) {
        index.set(item.package, item)
      }
    }
    return index
  }

  /**
   * 逐实例展开包名：user 0 用裸包名，其他用户带 `@userId` 后缀
   * （与 ksu 实现、`utils/package.ts` 的实例身份约定一致）。
   */
  function expandInstances(item: ApiPackageItem, packageType: PackageQueryType): string[] {
    if (packageType === 'system' && item.isSystem !== true) return []
    if (packageType === 'user' && item.isSystem === true) return []

    const userIds = Array.isArray(item.userIds) && item.userIds.length > 0 ? item.userIds : [0]
    return userIds.map((userId) => (userId === 0 ? item.package : `${item.package}@${userId}`))
  }

  /**
   * 包条目 → InstalledApp。
   *
   * `label` 本增量恒为 null（应用名属 P4），因此 `appName` 退回包名；`icon` 留空，
   * 由 `composables/useAppIcons.ts` 在没有 ksu 图标协议时渲染首字母占位（§10.4）。
   */
  function toInstalledApp(
    packageName: string,
    index: Map<string, ApiPackageItem>,
    appOptions: GetAppsInfoOptions = {}
  ): InstalledApp {
    const { base } = parsePackageUser(packageName)
    const entry = index.get(base)

    return {
      packageName,
      appName: entry?.label || packageName,
      icon: '',
      versionName: entry?.versionName || '',
      versionCode: entry?.versionCode ?? 0,
      // 与 ksu 实现一致：拿不到逐用户条目时按包名条目继承元数据
      installed: appOptions.assumeInstalled || Boolean(entry),
      isSystem:
        entry?.isSystem ??
        (appOptions.fallbackType === 'system'
          ? true
          : appOptions.fallbackType === 'user'
            ? false
            : undefined),
    }
  }

  async function listPackages(packageType: PackageQueryType = 'user'): Promise<string[]> {
    const items = await fetchAllPackages()
    const packages: string[] = []
    for (const item of items) {
      packages.push(...expandInstances(item, packageType))
    }
    return Array.from(new Set(packages))
  }

  async function getInstalledApps(
    appOptions: GetInstalledAppsOptions = {}
  ): Promise<InstalledApp[]> {
    const packageType = appOptions.packageType || (appOptions.includeSystem ? 'all' : 'user')
    const items = await fetchAllPackages()
    const index = buildPackageIndex(items)

    const instances: string[] = []
    for (const item of items) {
      instances.push(...expandInstances(item, packageType))
    }

    return Array.from(new Set(instances)).map((packageName) =>
      toInstalledApp(packageName, index, {
        fallbackType: packageType === 'all' ? undefined : packageType,
        assumeInstalled: true,
      })
    )
  }

  async function getPackagesInfo(
    packageNames: string[],
    appOptions: GetAppsInfoOptions = {}
  ): Promise<InstalledApp[]> {
    const requested = Array.from(new Set(packageNames.map((name) => name.trim()).filter(Boolean)))
    if (requested.length === 0) {
      return []
    }

    // 服务端按裸包名聚合（一个包一条，userIds 列出安装它的用户）
    const baseNames = Array.from(new Set(requested.map((name) => parsePackageUser(name).base)))
    const params = new URLSearchParams({ names: baseNames.join(',') })
    const index = buildPackageIndex(await sendPackages(params))

    return requested.map((packageName) => toInstalledApp(packageName, index, appOptions))
  }

  async function getServerStatus(): Promise<ServerStatus | null> {
    return await sendJson<ServerStatus>('/status')
  }

  async function getModuleInfo(): Promise<ModuleInfo | null> {
    const status = await getServerStatus()
    const moduleInfo = status?.module
    if (!moduleInfo) {
      return null
    }
    return {
      id: moduleInfo.id,
      version: moduleInfo.version,
      versionCode: moduleInfo.versionCode,
    }
  }

  /**
   * 设备属性 → `getprop` 风格的 `[key]: [value]` 文本。
   *
   * 原生模式只能读白名单键（§5.22），因此模板转换会缺少部分分区变体；
   * 输出格式与 `getprop` 一致，`utils/propsToTemplate.ts` 无需改动。
   */
  async function getProps(): Promise<string> {
    const keys = HTTP_PROP_KEYS.join(',')
    const payload = await sendJson<{ props?: Record<string, string> }>(
      `/device/props?keys=${encodeURIComponent(keys)}`
    )
    const props = payload.props && typeof payload.props === 'object' ? payload.props : {}

    return Object.entries(props)
      .filter(([, value]) => typeof value === 'string')
      .map(([key, value]) => `[${key}]: [${value}]`)
      .join('\n')
  }

  async function fetchOnlineTemplates(
    url: string,
    fetchOptions: FetchOnlineTemplatesOptions = {}
  ): Promise<string> {
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS)
    const onAbort = () => controller.abort()
    fetchOptions.signal?.addEventListener('abort', onAbort, { once: true })

    try {
      const response = await fetch(url, {
        headers: { Accept: fetchOptions.accept || 'application/json' },
        cache: 'no-store',
        signal: controller.signal,
      })
      if (!response.ok) {
        throw new ApiError(response.status, {
          code: 'upstream_unavailable',
          message: `HTTP ${response.status}`,
          retryable: response.status === 429 || response.status >= 500,
        })
      }
      return await response.text()
    } finally {
      clearTimeout(timer)
      fetchOptions.signal?.removeEventListener('abort', onAbort)
    }
  }

  return {
    runtime: 'http',
    serverRuntime,
    readConfigRaw,
    writeConfigRaw,
    getConfigRevision,
    readFile: async (path: string) => {
      if (isConfigPath(path)) {
        return await readConfigRaw()
      }
      // 契约 §3.2：不提供任意文件读取
      throw new UnsupportedError(`原生模式不支持读取文件：${path}`)
    },
    writeFile: async (path: string, content: string) => {
      if (isConfigPath(path)) {
        await writeConfigRaw(content)
        return
      }
      throw new UnsupportedError(`原生模式不支持写入文件：${path}`)
    },
    fileExists: async (path: string) => {
      if (!isConfigPath(path)) {
        return false
      }
      const info = await getConfigRevision()
      return info.exists
    },
    // 目录由服务端维护（/data/adb/device_faker/config 由服务启动时创建）
    mkdir: async () => undefined,
    listPackages,
    getPackagesInfo,
    getInstalledApps,
    getModuleInfo,
    getProps,
    getServerStatus,
    // 原生模式没有 root shell，服务开关只能在管理器模式下操作（契约 §10.4 同一思路）
    readServerConfig: async () => {
      throw new UnsupportedError('原生模式不支持直接读取 server.toml')
    },
    writeServerConfig: async () => {
      throw new UnsupportedError('原生模式不支持直接写 server.toml')
    },
    restartServer: async () => {
      throw new UnsupportedError('原生模式不支持重启服务')
    },
    getServerControlStatus: async () => {
      throw new UnsupportedError('原生模式不支持读取服务脚本状态')
    },
    generateServerToken: async (): Promise<ServerTokenPair> => {
      throw new UnsupportedError('原生模式不支持生成访问 Token')
    },
    hashServerToken: async (_token: string): Promise<ServerTokenPair> => {
      throw new UnsupportedError('原生模式不支持修改访问 Token')
    },
    getLanAddresses: async (): Promise<string[]> => {
      throw new UnsupportedError('原生模式不支持枚举本机地址')
    },
    openDeepLink: async (_url: string, _options?: DeepLinkOptions) => {
      throw new UnsupportedError('原生模式不支持深链启动，请改用浏览器打开')
    },
    fetchOnlineTemplates,
  }
}
