// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker
//
// Manager Relay API 客户端（API v1）。契约：manager/docs/API.md。
// - baseUrl 默认同源 /relay/api/v1，可用 VITE_API_BASE 覆盖
// - Manager Token 存 localStorage.dfm_token；首个请求 401 时抛 AuthError，由 App.vue 弹 Token 对话框
// - 非 2xx 统一解析错误信封并抛 ApiError（带 status / code / requestId）
// - 只有 GET 的 retryable 错误会退避重试（250ms / 500ms / 1000ms），写请求不自动重放
// - 出口代理 egress 契约见 docs/plans/egress-contract.md §6；egress revision 只读
//   X-DF-Egress-Revision（或响应体 revision），绝不读 X-DF-Revision / ETag 覆盖 config revision

import type {
  AddDeviceRequest,
  ApplyAcceptedResponse,
  ApplyRequest,
  AppsMatrixResponse,
  AppsQuery,
  AuditQuery,
  AuditResponse,
  CreateEgressProxyRequest,
  DeviceDetail,
  DeviceEgressPutRequest,
  DeviceEgressPutResponse,
  DeviceEgressResponse,
  DeviceId,
  DeviceOverviewResponse,
  DeviceQuery,
  DeviceSummary,
  DevicesResponse,
  EgressApplyRequest,
  EgressApplyResponse,
  EgressProbeRequest,
  EgressProbeResponse,
  EgressProxy,
  EgressProxyDetail,
  EgressProxyId,
  EgressProxyListResponse,
  EgressProxyQuery,
  ErrorEnvelope,
  HealthResponse,
  Job,
  JobId,
  JobsQuery,
  JobsResponse,
  JsonObject,
  ManagerApi,
  ManagerOverviewResponse,
  PackageName,
  RotateTokenRequest,
  RotateTokenResponse,
  SyncAllResponse,
  SyncDeviceResponse,
  TemplatesResponse,
  UpdateDeviceRequest,
  UpdateEgressProxyRequest,
  UpdateEgressProxyResponse,
} from './types'

/** 默认 API 前缀（与 Relay 同源部署时无需配置）。 */
export const DEFAULT_API_BASE = '/relay/api/v1'

/** Manager Token 在 localStorage 中的键（API.md §2.5）。 */
export const TOKEN_STORAGE_KEY = 'dfm_token'

/** 内置默认 Token：仅用于初始化输入框，不写入页面 HTML（API.md §2.5）。 */
export const DEFAULT_TOKEN = 'admin123'

/** Relay 返回的 4xx / 5xx，字段与错误信封一一对应。 */
export class ApiError extends Error {
  readonly name = 'ApiError'
  readonly status: number
  readonly code: string
  readonly details: JsonObject | null
  readonly retryable: boolean
  readonly requestId: string | null

  constructor(init: {
    status: number
    code: string
    message: string
    details: JsonObject | null
    retryable: boolean
    requestId: string | null
  }) {
    super(init.message)
    this.status = init.status
    this.code = init.code
    this.details = init.details
    this.retryable = init.retryable
    this.requestId = init.requestId
  }
}

/** 401：Token 缺失 / 无效 / 已轮换，UI 需要弹 Token 对话框。 */
export class AuthError extends Error {
  readonly name = 'AuthError'
  readonly status = 401

  constructor(message = 'Manager Token 无效或已轮换，请重新输入 Token。') {
    super(message)
  }
}

/** 只有 VITE_MOCK=1 时才允许读取 src/mock 的示例数据。 */
export function isMockMode(): boolean {
  return import.meta.env.VITE_MOCK === '1'
}

/** 解析 API 基地址（去掉尾部斜杠）。 */
export function apiBaseUrl(): string {
  const raw = import.meta.env.VITE_API_BASE
  if (typeof raw === 'string' && raw.trim()) return raw.trim().replace(/\/+$/, '')
  return DEFAULT_API_BASE
}

/**
 * 读取本地 Manager Token；未设置时返回空字符串（不把内置默认值当成活跃凭据）。
 * DEFAULT_TOKEN 只用于 Token 对话框预填，空 Token 时请求不带 Authorization，由 401 触发输入。
 */
export function getStoredToken(): string {
  try {
    const value = localStorage.getItem(TOKEN_STORAGE_KEY)
    return value && value.trim() ? value : ''
  } catch {
    return ''
  }
}

/** 写入 / 轮换后的 Token。 */
export function storeToken(token: string): void {
  try {
    localStorage.setItem(TOKEN_STORAGE_KEY, token)
  } catch {
    // 隐私模式下写入失败：内存里仍会使用新 Token
  }
}

/** 清空本地 Token（回落到内置默认值）。 */
export function clearStoredToken(): void {
  try {
    localStorage.removeItem(TOKEN_STORAGE_KEY)
  } catch {
    // ignore
  }
}

/** 把任意异常转成一句可展示的中文说明；ApiError 带 requestId。 */
export function describeError(error: unknown, fallback = '请求失败'): string {
  if (error instanceof AuthError) return error.message
  if (error instanceof ApiError) {
    const requestId = error.requestId ? `（requestId ${error.requestId}）` : ''
    const code = error.code && !error.code.startsWith('http_') ? ` [${error.code}]` : ''
    return `${error.message}${code}${requestId}`
  }
  if (error instanceof Error && error.message) return error.message
  return fallback
}

/** 随机幂等键（POST /apply 可选，避免网络重试产生重复 job）。 */
export function newIdempotencyKey(): string {
  try {
    if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
      return crypto.randomUUID()
    }
  } catch {
    // ignore
  }
  return `dfm-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
}

/**
 * egress revision 的唯一响应头来源（docs/plans/egress-contract.md §1.2）。
 *
 * 硬约束：egress 端点**只**认 `X-DF-Egress-Revision`（或响应体里的 `revision`），
 * 绝不读 `X-DF-Revision` / `ETag`。那两个属于 config revision，一旦用 egress 的
 * 值覆盖 WebUI 侧缓存的 config revision，后续 PUT /config 的 If-Match 必然 412。
 */
export const EGRESS_REVISION_HEADER = 'X-DF-Egress-Revision'

/** 每台设备的 egress revision（来自 X-DF-Egress-Revision 或响应体），与 config revision 完全隔离。 */
const egressRevisions = new Map<string, string>()

/** 读某台设备最近一次 egress revision；未知返回 null。 */
export function egressRevisionFor(deviceId: string): string | null {
  return egressRevisions.get(deviceId) ?? null
}

/** 清空 egress revision 缓存（Token 轮换 / 手动重载时用）。 */
export function clearEgressRevisions(): void {
  egressRevisions.clear()
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms)
  })
}

type QueryValue = string | number | boolean | string[] | undefined | null

/** 按契约拼查询串；数组默认逗号分隔（与 status / action 参数一致）。 */
function buildQuery(params: Record<string, QueryValue>, repeatKeys: string[] = []): string {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === '') continue
    if (Array.isArray(value)) {
      if (!value.length) continue
      if (repeatKeys.includes(key)) {
        for (const item of value) search.append(key, item)
      } else {
        search.set(key, value.join(','))
      }
      continue
    }
    search.set(key, String(value))
  }
  const query = search.toString()
  return query ? `?${query}` : ''
}

function isErrorEnvelope(payload: unknown): payload is ErrorEnvelope {
  if (!payload || typeof payload !== 'object') return false
  const candidate = (payload as { error?: unknown }).error
  if (!candidate || typeof candidate !== 'object') return false
  const envelope = candidate as { code?: unknown; message?: unknown }
  return typeof envelope.code === 'string' && typeof envelope.message === 'string'
}

/** ManagerApi 的超集：额外提供透传代理与设备端应用清除。 */
export interface RelayClient extends ManagerApi {
  proxy<T = unknown>(
    id: DeviceId,
    path: string,
    init?: { method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'; body?: unknown }
  ): Promise<T>
  removeDeviceApp(id: DeviceId, packageName: PackageName): Promise<unknown>
}

export interface ManagerClientOptions {
  /** 默认 '/relay/api/v1'；独立部署时可用绝对地址。 */
  baseUrl?: string
  /** 每次请求重新读取 Token（支持轮换后立刻生效）。 */
  getToken?: () => string
  /** 注入 fetch（测试用）。 */
  fetchImpl?: typeof fetch
}

const RETRY_DELAYS = [250, 500, 1000]

interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  body?: unknown
  query?: string
  headers?: Record<string, string>
  idempotencyKey?: string
  /** GET 默认允许对 retryable 错误退避重试。 */
  retry?: boolean
  /**
   * 采集 egress revision 用的设备 id：成功响应里只读 `X-DF-Egress-Revision`，
   * 不碰 `X-DF-Revision` / `ETag`（见 EGRESS_REVISION_HEADER 的说明）。
   */
  egressRevisionKey?: string
}

/**
 * 创建 Relay 客户端。所有方法在非 2xx 时抛 ApiError，401 抛 AuthError。
 * 默认实例见文件末尾的 managerApi。
 */
export function createManagerClient(options: ManagerClientOptions = {}): RelayClient {
  const baseUrl = (options.baseUrl ?? apiBaseUrl()).replace(/\/+$/, '')
  const getToken = options.getToken ?? getStoredToken
  const fetchImpl: typeof fetch =
    options.fetchImpl ?? ((input, init) => fetch(input, init))

  async function readJson(response: Response): Promise<unknown> {
    try {
      const text = await response.text()
      if (!text) return undefined
      return JSON.parse(text) as unknown
    } catch {
      return undefined
    }
  }

  function toApiError(response: Response, payload: unknown): ApiError {
    const requestIdHeader = response.headers.get('X-Request-Id')
    if (isErrorEnvelope(payload)) {
      const envelope = payload.error
      return new ApiError({
        status: response.status,
        code: envelope.code,
        message: envelope.message,
        details: envelope.details ?? null,
        retryable: Boolean(envelope.retryable),
        requestId: envelope.requestId ?? requestIdHeader,
      })
    }
    return new ApiError({
      status: response.status,
      code: `http_${response.status}`,
      message:
        response.status >= 500
          ? `Relay 返回 HTTP ${response.status}（非 JSON 响应）。请确认 manager 已启动在 127.0.0.1:8765，且 Vite 代理 /relay 生效。`
          : `Relay 返回 HTTP ${response.status}（非 JSON 响应）。`,
      details: null,
      // 没有错误信封时无法判断服务端语义（常见于 Relay 未启动、代理返回 502），
      // 不做自动重试，避免启动失败时页面白等。
      retryable: false,
      requestId: requestIdHeader,
    })
  }

  async function send<T>(path: string, options2: RequestOptions = {}): Promise<T> {
    const method = options2.method ?? 'GET'
    const url = `${baseUrl}${path}${options2.query ?? ''}`
    const headers: Record<string, string> = { Accept: 'application/json' }
    const token = getToken()
    if (token) headers.Authorization = `Bearer ${token}`
    if (options2.body !== undefined) headers['Content-Type'] = 'application/json'
    if (options2.idempotencyKey) headers['Idempotency-Key'] = options2.idempotencyKey
    if (options2.headers) Object.assign(headers, options2.headers)

    const body = options2.body === undefined ? undefined : JSON.stringify(options2.body)
    const attempts = options2.retry === false || method !== 'GET' ? 1 : RETRY_DELAYS.length + 1
    let lastError: ApiError | null = null

    for (let attempt = 0; attempt < attempts; attempt += 1) {
      let response: Response
      try {
        response = await fetchImpl(url, { method, headers, body })
      } catch {
        throw new ApiError({
          status: 0,
          code: 'network_error',
          message: `无法连接 Relay（${baseUrl}）：请确认 manager 已启动在 127.0.0.1:8765。`,
          details: null,
          retryable: false,
          requestId: null,
        })
      }

      if (response.status === 401) throw new AuthError()
      if (response.status === 204) return undefined as T

      const payload = await readJson(response)
      if (!response.ok) {
        const apiError = toApiError(response, payload)
        if (apiError.retryable && method === 'GET' && attempt < attempts - 1) {
          lastError = apiError
          await sleep(RETRY_DELAYS[attempt] ?? 1000)
          continue
        }
        throw apiError
      }
      if (options2.egressRevisionKey) {
        // 只有这个头能更新 egress revision；config revision 缓存完全不参与。
        const revision = response.headers.get(EGRESS_REVISION_HEADER)
        if (revision) egressRevisions.set(options2.egressRevisionKey, revision)
      }
      return payload as T
    }

    throw (
      lastError ??
      new ApiError({
        status: 0,
        code: 'network_error',
        message: '请求 Relay 失败',
        details: null,
        retryable: false,
        requestId: null,
      })
    )
  }

  const encodeId = (id: string): string => encodeURIComponent(id)
  const encodePackage = (name: PackageName): string => encodeURIComponent(name)

  return {
    healthz: () => send<HealthResponse>('/healthz', { retry: false }),

    listDevices: (query: DeviceQuery = {}) =>
      send<DevicesResponse>('/devices', {
        query: buildQuery(
          {
            online: query.online,
            tag: query.tag,
            q: query.q,
            limit: query.limit,
            cursor: query.cursor,
          },
          ['tag']
        ),
      }),

    addDevice: (body: AddDeviceRequest) =>
      send<DeviceSummary>('/devices', { method: 'POST', body }),

    getDevice: (id: DeviceId) => send<DeviceDetail>(`/devices/${encodeId(id)}`),

    updateDevice: (id: DeviceId, body: UpdateDeviceRequest) =>
      send<DeviceSummary>(`/devices/${encodeId(id)}`, { method: 'PATCH', body }),

    removeDevice: async (id: DeviceId) => {
      await send<void>(`/devices/${encodeId(id)}`, { method: 'DELETE' })
    },

    syncDevice: (id: DeviceId) =>
      send<SyncDeviceResponse>(`/devices/${encodeId(id)}/sync`, {
        method: 'POST',
        body: {},
      }),

    syncAllDevices: () => send<SyncAllResponse>('/devices/sync', { method: 'POST', body: {} }),

    getDeviceOverview: (id: DeviceId, query?: { cached?: boolean }) =>
      send<DeviceOverviewResponse>(`/devices/${encodeId(id)}/overview`, {
        query: buildQuery({ cached: query?.cached ? '1' : undefined }),
      }),

    listTemplates: (query?: {
      package?: PackageName
      device?: DeviceId
      limit?: number
      cursor?: string
    }) =>
      send<TemplatesResponse>('/templates', {
        query: buildQuery({
          package: query?.package,
          device: query?.device,
          limit: query?.limit,
          cursor: query?.cursor,
        }),
      }),

    getOverview: () => send<ManagerOverviewResponse>('/overview'),

    getAppsMatrix: (query: AppsQuery = {}) =>
      send<AppsMatrixResponse>('/apps', {
        query: buildQuery({
          q: query.q,
          drift: query.drift === undefined ? undefined : query.drift ? 'true' : 'false',
          device: query.device,
          limit: query.limit,
          cursor: query.cursor,
        }),
      }),

    apply: (body: ApplyRequest, idempotencyKey?: string) =>
      send<ApplyAcceptedResponse>('/apply', {
        method: 'POST',
        body,
        idempotencyKey: idempotencyKey ?? newIdempotencyKey(),
      }),

    getJob: (id: JobId) => send<Job>(`/jobs/${encodeId(id)}`),

    listJobs: (query: JobsQuery = {}) =>
      send<JobsResponse>('/jobs', {
        query: buildQuery({
          status: query.status,
          kind: query.kind,
          limit: query.limit,
          cursor: query.cursor,
        }),
      }),

    listAudit: (query: AuditQuery = {}) =>
      send<AuditResponse>('/audit', {
        query: buildQuery({
          action: query.action,
          target: query.target,
          since: query.since,
          limit: query.limit,
          cursor: query.cursor,
        }),
      }),

    rotateToken: (body?: RotateTokenRequest) =>
      send<RotateTokenResponse>('/token/rotate', {
        method: 'POST',
        body: body ?? {},
      }),

    /** 设备端 /api/v1 原样透传（DELETE /apps/{pkg} 等未封装能力）。 */
    proxy: <T = unknown>(
      id: DeviceId,
      path: string,
      init?: { method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'; body?: unknown }
    ) =>
      send<T>(
        `/devices/${encodeId(id)}/proxy/api/v1/${path.replace(/^\/+/, '')}`,
        { method: init?.method ?? 'GET', body: init?.body }
      ),

    removeDeviceApp: (id: DeviceId, packageName: PackageName) =>
      send<unknown>(
        `/devices/${encodeId(id)}/proxy/api/v1/apps/${encodePackage(packageName)}`,
        { method: 'DELETE', query: buildQuery({ confirm: 'true', detach_templates: 'true' }) }
      ),

    // ---- 出口代理 egress（docs/plans/egress-contract.md §6）----
    // 探测（POST /egress-proxies/{id}/probe）对 http 与 socks5 都走真连：
    // http 预置 Proxy-Authorization；socks5 走 socks5h:// + RFC 1929，且不发任何认证头。
    listEgressProxies: (query: EgressProxyQuery = {}) =>
      send<EgressProxyListResponse>('/egress-proxies', {
        query: buildQuery(
          {
            q: query.q,
            enabled:
              query.enabled === undefined ? undefined : query.enabled ? 'true' : 'false',
            tag: query.tag,
            limit: query.limit,
            cursor: query.cursor,
          },
          ['tag']
        ),
      }),

    createEgressProxy: (body: CreateEgressProxyRequest) =>
      send<EgressProxy>('/egress-proxies', { method: 'POST', body }),

    getEgressProxy: (id: EgressProxyId) =>
      send<EgressProxyDetail>(`/egress-proxies/${encodeId(id)}`),

    updateEgressProxy: (id: EgressProxyId, body: UpdateEgressProxyRequest) =>
      send<UpdateEgressProxyResponse>(`/egress-proxies/${encodeId(id)}`, {
        method: 'PATCH',
        body,
      }),

    deleteEgressProxy: async (id: EgressProxyId) => {
      await send<void>(`/egress-proxies/${encodeId(id)}`, { method: 'DELETE' })
    },

    /** 真连探测出口 IP；失败统一 502 egress_probe_failed（details.reason 为六值枚举之一）。 */
    probeEgressProxy: (id: EgressProxyId, body?: EgressProbeRequest) =>
      send<EgressProbeResponse>(`/egress-proxies/${encodeId(id)}/probe`, {
        method: 'POST',
        body: body ?? {},
        retry: false,
      }),

    getDeviceEgress: (id: DeviceId) =>
      send<DeviceEgressResponse>(`/devices/${encodeId(id)}/egress`, {
        // egress revision 只从 X-DF-Egress-Revision 采集，与 config revision 无关。
        egressRevisionKey: id,
      }),

    putDeviceEgress: (id: DeviceId, body: DeviceEgressPutRequest) =>
      send<DeviceEgressPutResponse>(`/devices/${encodeId(id)}/egress`, {
        method: 'PUT',
        body,
        egressRevisionKey: id,
      }),

    applyEgress: (body: EgressApplyRequest, idempotencyKey?: string) =>
      send<EgressApplyResponse>('/devices/egress/apply', {
        method: 'POST',
        body,
        idempotencyKey: idempotencyKey ?? newIdempotencyKey(),
      }),
  }
}

/** 默认单例：Token 动态读取 localStorage.dfm_token。 */
export const managerApi = createManagerClient()
