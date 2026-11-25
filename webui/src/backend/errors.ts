// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

/**
 * /api/v1 错误信封与前端异常类型（契约见 docs/API.md §7）。
 *
 * 所有 HTTP 传输的失败都收敛成 `ApiError`（携带稳定错误码、details、
 * requestId、Retry-After），UI 只需按 `code`/`status` 分支，不再解析文本。
 */

/** 错误信封里的 error 对象（docs/API.md §7.1）。 */
export interface ApiErrorPayload {
  code: string
  message: string
  details?: unknown
  retryable?: boolean
  requestId?: string
}

/** 契约 §7.2 的错误码（本增量会出现的全集）。 */
export type ApiErrorCode =
  | 'unauthorized'
  | 'forbidden_scope'
  | 'not_found'
  | 'method_not_allowed'
  | 'confirmation_required'
  | 'conflict_duplicate_package'
  | 'config_revision_mismatch'
  | 'precondition_required'
  | 'validation_failed'
  | 'body_too_large'
  | 'unsupported_media_type'
  | 'rate_limited'
  | 'internal_error'
  | 'upstream_unavailable'
  // 出口代理（契约 §1.8）：命名带点，与其它码不同，是刻意的
  | 'egress.not_configured'
  | 'egress.probe_failed'

/** 契约 §6.7 的单条字段级错误。 */
export interface ValidationErrorItem {
  path: string
  code: string
  message: string
}

export interface ApiErrorOptions {
  /** `Retry-After` 头解析出的秒数（429/503）。 */
  retryAfterSec?: number
  /** 响应头 `X-Request-Id`（与信封里的 requestId 一致，优先信封）。 */
  requestId?: string
}

/** 服务端错误响应（4xx/5xx）对应的异常。 */
export class ApiError extends Error {
  readonly status: number
  readonly code: string
  readonly details: unknown
  readonly retryable: boolean
  readonly requestId?: string
  readonly retryAfterSec?: number

  constructor(status: number, payload: ApiErrorPayload, options: ApiErrorOptions = {}) {
    super(payload.message || `HTTP ${status}`)
    this.name = 'ApiError'
    this.status = status
    this.code = payload.code || 'internal_error'
    this.details = payload.details ?? null
    this.retryable = payload.retryable ?? status >= 500
    this.requestId = payload.requestId || options.requestId
    this.retryAfterSec = options.retryAfterSec
  }
}

/** 401：Token 缺失/无效，UI 需要弹出 Token 输入框。 */
export class AuthError extends ApiError {
  constructor(
    payload: ApiErrorPayload = { code: 'unauthorized', message: '缺少或无效 Token' },
    options: ApiErrorOptions = {}
  ) {
    super(401, payload, options)
    this.name = 'AuthError'
  }
}

/** 当前传输不支持该能力（原生模式下的深链、任意文件读写等）。 */
export class UnsupportedError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'UnsupportedError'
  }
}

export function isApiError(value: unknown): value is ApiError {
  return value instanceof ApiError
}

export function isAuthError(value: unknown): value is ApiError {
  return value instanceof AuthError || (value instanceof ApiError && value.status === 401)
}

export function isUnsupportedError(value: unknown): value is UnsupportedError {
  return value instanceof UnsupportedError
}

/** `details.errors`（ValidationError[]）→ 结构化列表；形状不符时返回 []。 */
export function listValidationErrors(error: ApiError): ValidationErrorItem[] {
  const details = error.details
  if (!details || typeof details !== 'object') {
    return []
  }

  const raw = (details as { errors?: unknown }).errors
  if (!Array.isArray(raw)) {
    return []
  }

  return raw.flatMap((item) => {
    if (!item || typeof item !== 'object') return []
    const record = item as Record<string, unknown>
    const path = typeof record.path === 'string' ? record.path : ''
    const code = typeof record.code === 'string' ? record.code : ''
    const message = typeof record.message === 'string' ? record.message : ''
    return message || path ? [{ path, code, message }] : []
  })
}

/** `details.required` / `details.granted`（403 forbidden_scope）。 */
export function readScopeDetails(error: ApiError): { required?: string[]; granted?: string[] } {
  const details = error.details
  if (!details || typeof details !== 'object') {
    return {}
  }

  const record = details as Record<string, unknown>
  const pick = (value: unknown) =>
    Array.isArray(value)
      ? value.filter((item): item is string => typeof item === 'string')
      : undefined

  return { required: pick(record.required), granted: pick(record.granted) }
}

/** `details.package` / `details.templates`（409 conflict_duplicate_package）。 */
export function readConflictDetails(error: ApiError): {
  packageName?: string
  templates: string[]
} {
  const details = error.details
  if (!details || typeof details !== 'object') {
    return { templates: [] }
  }

  const record = details as Record<string, unknown>
  const packageName = typeof record.package === 'string' ? record.package : undefined
  const templates = Array.isArray(record.templates)
    ? record.templates.filter((item): item is string => typeof item === 'string')
    : []

  return { packageName, templates }
}

export function parseRetryAfter(value: string | null): number | undefined {
  if (!value) return undefined
  const seconds = Number.parseInt(value.trim(), 10)
  return Number.isFinite(seconds) && seconds >= 0 ? seconds : undefined
}

function fallbackPayload(status: number, text: string): ApiErrorPayload {
  return {
    code: status === 401 ? 'unauthorized' : status >= 500 ? 'internal_error' : 'validation_failed',
    message: text.trim() || `HTTP ${status}`,
    retryable: status >= 500,
  }
}

/** 把非 2xx 响应解析成 ApiError（信封解析失败时按状态码兜底）。 */
export async function toApiError(response: Response): Promise<ApiError> {
  const requestId = response.headers.get('X-Request-Id') || undefined
  const retryAfterSec = parseRetryAfter(response.headers.get('Retry-After'))

  let text: string
  try {
    text = await response.text()
  } catch {
    text = ''
  }

  let payload: ApiErrorPayload = fallbackPayload(response.status, text)
  if (text) {
    try {
      const parsed = JSON.parse(text) as unknown
      const envelope =
        parsed && typeof parsed === 'object' ? (parsed as { error?: unknown }).error : null
      if (envelope && typeof envelope === 'object') {
        const record = envelope as Record<string, unknown>
        payload = {
          code: typeof record.code === 'string' && record.code ? record.code : payload.code,
          message:
            typeof record.message === 'string' && record.message ? record.message : payload.message,
          details: record.details ?? null,
          retryable: typeof record.retryable === 'boolean' ? record.retryable : payload.retryable,
          requestId: typeof record.requestId === 'string' ? record.requestId : undefined,
        }
      }
    } catch {
      // 非 JSON 响应（例如静态资源的 404），保留按状态码兜底的 payload
    }
  }

  const options: ApiErrorOptions = { requestId, retryAfterSec }
  return response.status === 401
    ? new AuthError(payload, options)
    : new ApiError(response.status, payload, options)
}
