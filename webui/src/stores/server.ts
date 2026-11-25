// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import {
  backend,
  clearToken,
  getToken,
  hasToken,
  isTokenRemembered,
  onAuthRequired,
  resolveBackend,
  setAuthPromptHandler,
  setToken,
  type BackendRuntime,
  type ServerRuntimeInfo,
  type ServerStatus,
} from '../backend'
import { dismissTokenPrompt, promptForToken } from '../components/server/tokenPrompt'
import { useI18n } from '../utils/i18n'
import { describeApiError, pickServerMessages } from './serverMessages'

/** 登录成功后广播：配置 store 据此重新加载（它不能反向依赖本 store，避免循环导入）。 */
const TOKEN_CHANGED_EVENT = 'devmask:token-changed'

/**
 * 原生模式的服务状态与登录态（契约 §3.5 / §10.5）。
 *
 * `scopes` 无法从 `/status` 直接读到（契约未返回），因此按「已验证过的能力」推断：
 * 成功读状态 → `read`，成功写配置 → `write`；403 时展示 `details.required`。
 */
export const useServerStore = defineStore('server', () => {
  const { locale } = useI18n()

  const runtime = ref<BackendRuntime>(backend.runtime)
  const runtimeInfo = ref<ServerRuntimeInfo | null>(backend.serverRuntime)
  const status = ref<ServerStatus | null>(null)
  const loading = ref(false)
  const error = ref<string | null>(null)
  const tokenPresent = ref(hasToken())
  const tokenRemembered = ref(isTokenRemembered())
  const readVerified = ref(false)
  const writeVerified = ref(false)
  const revision = ref('')

  const messages = computed(() => pickServerMessages(locale.value))
  const isNative = computed(() => runtime.value === 'http')
  const isReadOnly = computed(() => runtime.value === 'readonly')
  const runtimeLabel = computed(() =>
    isReadOnly.value
      ? messages.value.section.runtimeReadonly
      : isNative.value
        ? messages.value.section.runtimeHttp
        : messages.value.section.runtimeKsu
  )
  const serverVersion = computed(() => status.value?.server?.version || '')
  const bind = computed(() => status.value?.server?.bind || '')
  const capabilities = computed(() => status.value?.capabilities ?? [])
  /** 配置文件是否存在（t11：不存在时 exists=false、mtime=null、size=0）。 */
  const configExists = computed(() => status.value?.config?.exists !== false)
  /**
   * 「最后修改」展示文本。
   *
   * 契约补充（t11）：配置缺失时 mtime 为 null，绝不能把它塞进日期格式化；
   * exists=false 走「配置缺失」分支，exists=true 且 mtime 非法时退回原文。
   */
  const configMtimeLabel = computed(() => {
    if (!configExists.value) {
      return messages.value.section.configMissing
    }
    const raw = status.value?.config?.mtime
    if (typeof raw !== 'string' || !raw) {
      return '—'
    }
    const timestamp = Date.parse(raw)
    return Number.isNaN(timestamp) ? raw : new Date(timestamp).toLocaleString(locale.value)
  })
  const scopes = computed(() => {
    const list: string[] = []
    if (readVerified.value) list.push('read')
    if (writeVerified.value) list.push('write')
    return list
  })
  const scopeSummary = computed(() => {
    if (!isNative.value || scopes.value.length === 0) {
      return messages.value.section.scopeUnknown
    }
    return `${scopes.value.join(', ')} (${messages.value.section.scopeVerified})`
  })

  async function refreshRuntime(): Promise<void> {
    ensureServerRuntimeWired()
    const resolved = await resolveBackend()
    runtime.value = resolved.runtime
    runtimeInfo.value = resolved.serverRuntime
  }

  async function refreshStatus(): Promise<void> {
    ensureServerRuntimeWired()
    if (!isNative.value) {
      status.value = null
      return
    }

    loading.value = true
    try {
      const next = await backend.getServerStatus()
      status.value = next
      const nextRevision = next?.config?.revision
      if (typeof nextRevision === 'string' && nextRevision) {
        revision.value = nextRevision
      }
      readVerified.value = true
      tokenPresent.value = hasToken()
      tokenRemembered.value = isTokenRemembered()
      error.value = null
    } catch (e) {
      error.value = describeApiError(e, messages.value)
    } finally {
      loading.value = false
    }
  }

  /** 通知其它 store 登录态变化（logout 不触发重载，避免退出后立刻又弹登录框）。 */
  function dispatchTokenChanged(reason: 'login' | 'logout'): void {
    if (typeof window === 'undefined') return
    window.dispatchEvent(new CustomEvent(TOKEN_CHANGED_EVENT, { detail: reason }))
  }

  /** 页面挂载时调用：解析传输类型 + 读一次服务状态。 */
  async function ensureReady(): Promise<void> {
    await refreshRuntime()
    await refreshStatus()
  }

  async function login(token: string, remember: boolean): Promise<void> {
    setToken(token, remember)
    tokenPresent.value = true
    tokenRemembered.value = remember
    await refreshStatus()
    dispatchTokenChanged('login')
  }

  /** 弹出 Token 输入框（401 时由传输层经 `setAuthPromptHandler` 触发）。 */
  async function promptLogin(
    reason: 'unauthorized' | 'missing' = 'unauthorized'
  ): Promise<{ token: string; remember: boolean } | null> {
    const result = await promptForToken({
      locale: locale.value,
      reason,
      initialToken: getToken(),
      remember: isTokenRemembered(),
    })
    if (!result) {
      return null
    }
    await login(result.token, result.remember)
    return result
  }

  function logout(): void {
    clearToken()
    dismissTokenPrompt()
    dispatchTokenChanged('logout')
    tokenPresent.value = false
    tokenRemembered.value = false
    readVerified.value = false
    writeVerified.value = false
    status.value = null
    revision.value = ''
    error.value = null
  }

  function markLoggedOut(): void {
    tokenPresent.value = false
    readVerified.value = false
  }

  function markWriteVerified(): void {
    writeVerified.value = true
  }

  function setRevision(next: string): void {
    if (next) {
      revision.value = next
    }
  }

  return {
    runtime,
    runtimeInfo,
    status,
    loading,
    error,
    tokenPresent,
    tokenRemembered,
    revision,
    messages,
    isNative,
    isReadOnly,
    runtimeLabel,
    serverVersion,
    bind,
    capabilities,
    configExists,
    configMtimeLabel,
    scopes,
    scopeSummary,
    refreshRuntime,
    refreshStatus,
    ensureReady,
    login,
    promptLogin,
    logout,
    markLoggedOut,
    markWriteVerified,
    setRevision,
    dispatchTokenChanged,
  }
})

let wired = false

/**
 * 注册 Token 输入框 + 401 事件监听（幂等）。
 *
 * 由 `stores/config.ts` 在 bootstrap/保存前调用：配置 bootstrap 是页面启动后的
 * 第一个网络请求，必须在它之前完成注册，首次 401 才能弹出输入框。
 */
export function ensureServerRuntimeWired(): void {
  if (wired) {
    return
  }
  wired = true

  if (backend.runtime !== 'http') {
    return
  }

  setAuthPromptHandler(async (reason) => {
    const store = useServerStore()
    return await store.promptLogin(reason)
  })

  onAuthRequired(() => {
    try {
      useServerStore().markLoggedOut()
    } catch {
      // pinia 尚未激活（极早期请求）时忽略：Token 已被 auth.ts 清空，下次请求会再弹
    }
  })
}
