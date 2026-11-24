// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

/**
 * 原生模式的 Token 存储与「需要登录」事件（契约 §3.5）。
 *
 * - 默认存 `sessionStorage`，用户勾选「记住」后才写 `localStorage`；
 * - 存储不可用（隐私模式 / 非浏览器环境）时退化为进程内变量，不抛错；
 * - 401 由 http 传输层调用 `requestToken()` 触发，UI 通过
 *   `setAuthPromptHandler()` 注册自己的输入框，二者解耦。
 */

const TOKEN_SESSION_KEY = 'device_faker_api_token'
const TOKEN_LOCAL_KEY = 'device_faker_api_token:remembered'
const REMEMBER_FLAG_KEY = 'device_faker_api_token:remember'

/** 用户提交的 Token。 */
export interface TokenSubmission {
  token: string
  remember: boolean
}

export type AuthPromptReason = 'unauthorized' | 'missing'

export type AuthPromptHandler = (reason: AuthPromptReason) => Promise<TokenSubmission | null>

/** 内存兜底（sessionStorage/localStorage 都不可用时使用）。 */
let memoryToken = ''
let memoryRemembered = false

function readSession(): Storage | null {
  try {
    return typeof sessionStorage === 'undefined' ? null : sessionStorage
  } catch {
    return null
  }
}

function readLocal(): Storage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

/** 当前 Token（localStorage 优先：勾选过「记住」的会话依旧可用）。 */
export function getToken(): string {
  const remembered = readLocal()?.getItem(TOKEN_LOCAL_KEY)
  if (remembered) return remembered
  if (memoryRemembered && memoryToken) return memoryToken
  return readSession()?.getItem(TOKEN_SESSION_KEY) || memoryToken
}

export function hasToken(): boolean {
  return getToken().length > 0
}

/** 当前 Token 是否来自 localStorage（勾选过「记住」）。 */
export function isTokenRemembered(): boolean {
  return Boolean(readLocal()?.getItem(TOKEN_LOCAL_KEY) || readLocal()?.getItem(REMEMBER_FLAG_KEY))
}

export function setToken(token: string, remember: boolean): void {
  const trimmed = token.trim()
  if (!trimmed) {
    clearToken()
    return
  }

  memoryToken = trimmed
  memoryRemembered = remember
  readSession()?.setItem(TOKEN_SESSION_KEY, trimmed)
  const local = readLocal()
  if (remember) {
    local?.setItem(TOKEN_LOCAL_KEY, trimmed)
    local?.setItem(REMEMBER_FLAG_KEY, '1')
  } else {
    local?.removeItem(TOKEN_LOCAL_KEY)
    local?.removeItem(REMEMBER_FLAG_KEY)
  }
}

export function clearToken(): void {
  memoryToken = ''
  memoryRemembered = false
  readSession()?.removeItem(TOKEN_SESSION_KEY)
  const local = readLocal()
  local?.removeItem(TOKEN_LOCAL_KEY)
  local?.removeItem(REMEMBER_FLAG_KEY)
}

// ---------------------------------------------------------------- 登录事件

let promptHandler: AuthPromptHandler | null = null
let pendingPrompt: Promise<TokenSubmission | null> | null = null
const authRequiredListeners = new Set<(reason: AuthPromptReason) => void>()

/** UI 注册自己的 Token 输入框（原生模式启动时由 stores/server.ts 注册）。 */
export function setAuthPromptHandler(handler: AuthPromptHandler | null): void {
  promptHandler = handler
}

export function hasAuthPromptHandler(): boolean {
  return promptHandler !== null
}

/** 订阅「需要重新登录」（例如 401 后 Token 已被清空，UI 需要更新状态）。 */
export function onAuthRequired(listener: (reason: AuthPromptReason) => void): () => void {
  authRequiredListeners.add(listener)
  return () => {
    authRequiredListeners.delete(listener)
  }
}

function notifyAuthRequired(reason: AuthPromptReason): void {
  for (const listener of authRequiredListeners) {
    try {
      listener(reason)
    } catch {
      // UI 回调异常不影响传输层
    }
  }
}

/**
 * 请求用户输入 Token（并发调用共享同一次对话框）。
 *
 * 没有注册 UI 处理器时退回 `window.prompt`：宁可难看，也不让首次请求静默失败。
 */
export function requestToken(
  reason: AuthPromptReason = 'unauthorized'
): Promise<TokenSubmission | null> {
  notifyAuthRequired(reason)

  if (!pendingPrompt) {
    pendingPrompt = (async () => {
      const handler = promptHandler
      if (handler) {
        try {
          return await handler(reason)
        } catch {
          return null
        }
      }

      if (typeof window === 'undefined' || typeof window.prompt !== 'function') {
        return null
      }

      const entered = window.prompt(
        reason === 'missing' ? '请输入 device_faker API Token' : 'Token 无效或已撤销，请重新输入'
      )
      return entered && entered.trim() ? { token: entered.trim(), remember: false } : null
    })().finally(() => {
      pendingPrompt = null
    })
  }

  return pendingPrompt
}
