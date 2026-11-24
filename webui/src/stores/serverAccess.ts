// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { backend, isUnsupportedError } from '../backend'
import { notify } from '../components/shared/notify'
import { useI18n } from '../utils/i18n'
import {
  appendTokenBlock,
  applyServerSettings,
  formatLanUrls,
  isValidPort,
  normalizePort,
  parseServerSettings,
  stripTokenBlocks,
  type ServerSettings,
} from '../utils/serverConfig'

/**
 * 管理器模式的「外部访问」控制（开/关、端口、访问 Token、服务状态）。
 *
 * 为什么只在管理器模式可用：这些操作要么需要 root 写 /data/adb（server.toml），
 * 要么需要执行 service.sh；原生模式的 HTTP 接口按契约不提供这类能力，只读模式更
 * 没有设备接口。两种模式下本 store 的 `supported` 为 false，界面不渲染卡片。
 */
export const useServerAccessStore = defineStore('serverAccess', () => {
  const { t } = useI18n()

  const supported = computed(() => backend.runtime === 'ksu')
  const loading = ref(false)
  const saving = ref(false)
  const refreshing = ref(false)
  const error = ref<string | null>(null)

  /** 最近一次读到的 server.toml 原文（写回时的基底，保留注释与未知字段）。 */
  const rawConfig = ref<string | null>(null)
  const settings = ref<ServerSettings | null>(null)
  const statusText = ref('')
  const status = ref<'running' | 'stopped' | 'unknown'>('unknown')
  const lanAddresses = ref<string[]>([])

  const allowLan = ref(false)
  const port = ref(8686)

  const tokenDialogVisible = ref(false)
  const generatedToken = ref('')

  const hasToken = computed(() => settings.value?.hasToken ?? false)
  const lanUrls = computed(() => formatLanUrls(lanAddresses.value, port.value))
  const bindPreview = computed(
    () => `${allowLan.value ? '0.0.0.0' : '127.0.0.1'}:${normalizePort(port.value)}`
  )
  const statusLabel = computed(() => {
    if (status.value === 'running') return t('settings.serverAccess.status_running')
    if (status.value === 'stopped') return t('settings.serverAccess.status_stopped')
    return t('settings.serverAccess.status_unknown')
  })

  function describe(failure: unknown): string {
    if (isUnsupportedError(failure)) {
      return t('settings.serverAccess.errors.unsupported')
    }
    const message = failure instanceof Error ? failure.message : String(failure)
    if (/No such file|not found|can't execute/i.test(message)) {
      return t('settings.serverAccess.errors.binary_missing')
    }
    return message || t('settings.serverAccess.errors.apply_failed')
  }

  function updateStatus(text: string): void {
    statusText.value = text.trim()
    if (/server:\s*running/i.test(statusText.value)) {
      status.value = 'running'
    } else if (/server:\s*stopped/i.test(statusText.value)) {
      status.value = 'stopped'
    } else {
      status.value = 'unknown'
    }
  }

  async function refreshStatus(): Promise<void> {
    if (!supported.value) return
    refreshing.value = true
    try {
      updateStatus(await backend.getServerControlStatus())
    } catch (failure) {
      statusText.value = ''
      status.value = 'unknown'
      error.value = describe(failure)
    } finally {
      refreshing.value = false
    }
  }

  /**
   * service.sh restart 返回后 supervisor/server 可能还没写 pid；
   * 轮询 status 最多 6 秒，避免界面短暂显示 Stopped。
   */
  async function waitForServerRunning(timeoutMs = 6000): Promise<void> {
    const deadline = Date.now() + timeoutMs
    while (Date.now() < deadline) {
      try {
        updateStatus(await backend.getServerControlStatus())
        if (status.value === 'running') return
      } catch {
        // 服务刚重启时 status 可能短暂失败，继续轮询
      }
      await new Promise((resolve) => setTimeout(resolve, 500))
    }
  }

  async function refreshLanAddresses(): Promise<void> {
    if (!supported.value) return
    try {
      lanAddresses.value = await backend.getLanAddresses()
    } catch {
      // 地址枚举失败不影响开关本身；界面上展示「未检测到局域网地址」
      lanAddresses.value = []
    }
  }

  /** 读 server.toml + 服务状态 + 本机地址（页面挂载/保存后调用）。 */
  async function load(): Promise<void> {
    if (!supported.value) return
    loading.value = true
    error.value = null
    try {
      const text = await backend.readServerConfig()
      rawConfig.value = text
      settings.value = text ? parseServerSettings(text) : null
      allowLan.value = settings.value?.allowLan ?? false
      port.value = settings.value?.port ?? 8686
      await Promise.all([refreshStatus(), refreshLanAddresses()])
    } catch (failure) {
      error.value = describe(failure)
    } finally {
      loading.value = false
    }
  }

  /** 当前 server.toml 文本；没有（新装设备）时给一份最小可用内容。 */
  async function readBaseConfig(): Promise<string> {
    const current = rawConfig.value ?? (await backend.readServerConfig())
    return current ?? `bind = "127.0.0.1:8686"\nallow_lan = false\n`
  }

  /**
   * 保存开关/端口：改写 bind 与 allow_lan → 写盘（0600）→ 重启服务 → 重新读取。
   *
   * 开启外部访问但没有 Token 时自动生成一条，并把明文交给弹窗显示一次。
   */
  async function save(): Promise<boolean> {
    if (!supported.value) return false
    if (!isValidPort(port.value)) {
      error.value = t('settings.serverAccess.errors.invalid_port')
      return false
    }

    saving.value = true
    error.value = null
    try {
      const base = await readBaseConfig()
      let nextText = applyServerSettings(base, {
        allowLan: allowLan.value,
        port: normalizePort(port.value),
      })

      if (allowLan.value && !parseServerSettings(nextText).hasToken) {
        const pair = await backend.generateServerToken()
        nextText = appendTokenBlock(stripTokenBlocks(nextText), pair.hash)
        generatedToken.value = pair.token
        tokenDialogVisible.value = true
      }

      await backend.writeServerConfig(nextText)
      await backend.restartServer()
      await waitForServerRunning()
      await load()

      if (status.value === 'stopped') {
        error.value = t('settings.serverAccess.errors.server_not_running')
        void notify(error.value, 'warning')
        return false
      }

      void notify(t('settings.serverAccess.messages.applied'), 'success')
      return true
    } catch (failure) {
      error.value = describe(failure)
      void notify(`${t('settings.serverAccess.errors.apply_failed')}: ${error.value}`, 'error')
      return false
    } finally {
      saving.value = false
    }
  }

  /** 重置访问 Token：清空旧 [[tokens]]，写入新哈希，重启服务后只显示一次明文。 */
  async function resetToken(): Promise<boolean> {
    if (!supported.value) return false

    saving.value = true
    error.value = null
    try {
      const base = await readBaseConfig()
      const pair = await backend.generateServerToken()
      const nextText = appendTokenBlock(stripTokenBlocks(base), pair.hash)

      await backend.writeServerConfig(nextText)
      await backend.restartServer()
      await waitForServerRunning()
      await load()

      generatedToken.value = pair.token
      tokenDialogVisible.value = true
      void notify(t('settings.serverAccess.messages.token_reset'), 'success')
      return true
    } catch (failure) {
      error.value = describe(failure)
      void notify(`${t('settings.serverAccess.errors.token_failed')}: ${error.value}`, 'error')
      return false
    } finally {
      saving.value = false
    }
  }

  function openTokenDialog(): void {
    if (generatedToken.value) {
      tokenDialogVisible.value = true
    }
  }

  function dismissTokenDialog(): void {
    tokenDialogVisible.value = false
    generatedToken.value = ''
  }

  return {
    supported,
    loading,
    saving,
    refreshing,
    error,
    rawConfig,
    settings,
    statusText,
    status,
    statusLabel,
    lanAddresses,
    allowLan,
    port,
    tokenDialogVisible,
    generatedToken,
    hasToken,
    lanUrls,
    bindPreview,
    load,
    save,
    resetToken,
    refreshStatus,
    refreshLanAddresses,
    openTokenDialog,
    dismissTokenDialog,
  }
})
