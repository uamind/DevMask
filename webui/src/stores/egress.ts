// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import {
  DEFAULT_PROBE_TIMEOUT_SECONDS,
  DEFAULT_PROBE_URL,
  backend,
  buildProxyInputs,
  egressProxyPath,
  findProxySummary,
  isApiError,
  isUnsupportedError,
  listValidationErrors,
  validateEgressForm,
  type EgressFormError,
  type EgressFormValues,
  type EgressProbeResult,
  type EgressSnapshot,
  type EgressUpdateInput,
  type EgressWarning,
} from '../backend'
import { notify } from '../components/shared/notify'
import { useI18n } from '../utils/i18n'
import { useServerStore } from './server'

/** 已知警告码：只有这些会被翻译，未知码原样显示后端 message（避免 i18n 缺 key 警告）。 */
const KNOWN_WARNING_CODES = [
  'egress_proxy_not_found',
  'egress_credentials_ignored',
  'egress_degraded_to_direct',
  'egress_settings_mismatch',
  'egress_perm_fixed',
  'egress_probe_approximate',
  'egress_parse',
  'server_not_running',
  // 设备内中继（契约 §6.4）：四个码都由服务端 warnings 直接带出来。
  'egress_relay_bind_failed',
  'egress_relay_firewall_failed',
  'egress_relay_uid_unknown',
  'egress_relay_upstream_unreachable',
]

function blankForm(): EgressFormValues {
  return {
    enabled: false,
    name: '',
    type: 'http',
    host: '',
    port: 1080,
    username: '',
    password: '',
    clearCredentials: false,
    probeUrl: DEFAULT_PROBE_URL,
    probeTimeoutSeconds: DEFAULT_PROBE_TIMEOUT_SECONDS,
    failOpen: true,
  }
}

/**
 * 设置页「网络出口」卡片的状态与动作。
 *
 * 两种运行时的差异都收敛在 backend 里，这里只负责：表单 ↔ snapshot 的映射、
 * 错误码 → 文案、探测结果与「近似」标记、保存后重新读取。
 */
export const useEgressStore = defineStore('egress', () => {
  const { t } = useI18n()
  const serverStore = useServerStore()

  const snapshot = ref<EgressSnapshot | null>(null)
  const form = ref<EgressFormValues>(blankForm())
  /** 当前表单对应的已存定义名（用于识别「改名」）。 */
  const loadedName = ref<string | null>(null)

  const loading = ref(false)
  const saving = ref(false)
  const clearing = ref(false)
  const probing = ref(false)
  const restarting = ref(false)

  const error = ref('')
  const errorCode = ref('')
  const errorMessage = ref('')
  const errorDetail = ref('')
  const fieldErrors = ref<Record<string, string>>({})

  const probeResult = ref<EgressProbeResult | null>(null)
  /** 探测目标应用：'' = 设备级链路。 */
  const probeTarget = ref('')
  const advancedOpen = ref(false)

  const runtime = computed(() => serverStore.runtime)
  const supported = computed(() => serverStore.runtime !== 'readonly')
  const ksuMode = computed(() => serverStore.runtime === 'ksu')
  /** 只有原生服务能经代理探测出口 IP（管理器模式没有出口链路）。 */
  const probeSupported = computed(() => serverStore.runtime === 'http')

  const proxies = computed(() => snapshot.value?.proxies ?? [])
  const apps = computed(() => snapshot.value?.apps ?? [])
  const hasCredentials = computed(
    () => findProxySummary(proxies.value, loadedName.value)?.hasCredentials ?? false
  )
  const applied = computed(() => snapshot.value?.applied === true)
  const readbackAvailable = computed(() => snapshot.value?.readbackAvailable !== false)
  const revisionAvailable = computed(() => snapshot.value?.revisionAvailable !== false)
  const revisionLabel = computed(() => {
    const snapshotValue = snapshot.value
    if (!snapshotValue) return '--'
    if (!revisionAvailable.value) return t('settings.egress.status.revision_unavailable')
    return snapshotValue.revision ? `${snapshotValue.revision.slice(0, 12)}…` : '--'
  })
  const appliedLabel = computed(() => {
    if (!snapshot.value) return '--'
    if (!readbackAvailable.value) return t('settings.egress.status.applied_unknown')
    return applied.value
      ? t('settings.egress.status.applied_yes')
      : t('settings.egress.status.applied_no')
  })
  const modeLabel = computed(() =>
    snapshot.value?.mode === 'proxy'
      ? t('settings.egress.modes.proxy')
      : t('settings.egress.modes.direct')
  )
  const effectiveModeLabel = computed(() =>
    snapshot.value?.effectiveMode === 'proxy'
      ? t('settings.egress.modes.proxy')
      : t('settings.egress.modes.direct')
  )
  const lastProbe = computed(() => snapshot.value?.health.lastProbe ?? null)
  const degraded = computed(() => snapshot.value?.health.degraded === true)
  const warnings = computed(() => snapshot.value?.warnings ?? [])
  const probeApproximate = computed(() => probeResult.value?.approximate === true)

  // ---- 设备内中继（relay，契约 §6.1）----
  /** null = 服务端没有 relay 字段（旧版本 / 管理器模式）：界面整块隐藏，不报错。 */
  const relay = computed(() => snapshot.value?.relay ?? null)
  const relayListeners = computed(() => relay.value?.listeners ?? [])
  const relayPortChanges = computed(() => relay.value?.portChanges ?? [])
  const relayRestartRequired = computed(() => relay.value?.restartRequiredApps ?? [])
  /** 重启中继只在原生服务模式可用（POST /egress/relay/restart，ops 作用域）。 */
  const relayRestartSupported = computed(() => serverStore.runtime === 'http')
  /** 代理定义走哪条链路：relay / 直连上游 / 凭据未生效（界面标注用）。 */
  const proxyPath = egressProxyPath

  function warningText(warning: EgressWarning): string {
    if (KNOWN_WARNING_CODES.includes(warning.code)) {
      return t(`settings.egress.warnings.${warning.code}`)
    }
    return warning.message || warning.code
  }

  const warningTexts = computed(() => warnings.value.map((warning) => warningText(warning)))

  function clearError(): void {
    error.value = ''
    errorCode.value = ''
    errorMessage.value = ''
    errorDetail.value = ''
    fieldErrors.value = {}
  }

  function errorKeyForCode(code: string): string {
    switch (code) {
      case 'egress.not_configured':
        return 'settings.egress.errors.not_configured'
      case 'egress.probe_failed':
        return 'settings.egress.errors.probe_failed'
      case 'config_revision_mismatch':
        return 'settings.egress.errors.revision_mismatch'
      case 'precondition_required':
        return 'settings.egress.errors.precondition'
      case 'forbidden_scope':
        return 'settings.egress.errors.forbidden'
      case 'upstream_unavailable':
        return 'settings.egress.errors.settings_unavailable'
      case 'validation_failed':
        return 'settings.egress.errors.invalid_form'
      default:
        return ''
    }
  }

  function detailReason(failure: unknown): string {
    if (!isApiError(failure)) return ''
    const details = failure.details
    if (!details || typeof details !== 'object') return ''
    const reason = (details as { reason?: unknown }).reason
    return typeof reason === 'string' ? reason : ''
  }

  function applyFieldErrorsFrom(failure: unknown): void {
    if (!isApiError(failure) || failure.code !== 'validation_failed') return
    const items = listValidationErrors(failure)
    const next: Record<string, string> = {}
    for (const item of items) {
      const field = item.path.split('.').pop() ?? item.path
      const key = `settings.egress.errors.${item.code}`
      next[field] = item.code ? t(key) : item.message
    }
    fieldErrors.value = next
  }

  function setError(failure: unknown, fallbackKey: string): void {
    if (isUnsupportedError(failure)) {
      errorCode.value = 'unsupported'
      error.value = t(
        fallbackKey === 'settings.egress.errors.probe_failed'
          ? 'settings.egress.errors.probe_unsupported'
          : 'settings.egress.errors.unsupported'
      )
      errorMessage.value = failure.message
      return
    }
    if (isApiError(failure)) {
      const key = errorKeyForCode(failure.code)
      errorCode.value = failure.code
      error.value = t(key || fallbackKey)
      errorMessage.value = failure.message
      errorDetail.value = detailReason(failure)
      applyFieldErrorsFrom(failure)
      return
    }
    error.value = t(fallbackKey)
    errorMessage.value = failure instanceof Error ? failure.message : String(failure)
  }

  /** 把 snapshot 回填到表单（凭据永远留空：密码不回显，只显示是否已设置）。 */
  function applySnapshotToForm(): void {
    const current = snapshot.value
    if (!current) return
    const upstream = current.upstream
    const name = upstream?.name ?? current.default ?? ''
    form.value = {
      enabled: Boolean(current.default),
      name,
      type: upstream?.type ?? 'http',
      host: upstream?.host ?? '',
      port: upstream?.port ?? 1080,
      username: '',
      password: '',
      clearCredentials: false,
      probeUrl: current.probeUrl || DEFAULT_PROBE_URL,
      probeTimeoutSeconds: current.probeTimeoutSeconds || DEFAULT_PROBE_TIMEOUT_SECONDS,
      failOpen: current.failOpen !== false,
    }
    loadedName.value = upstream?.name ?? current.default ?? null
  }

  async function load(): Promise<void> {
    if (!supported.value) return
    loading.value = true
    clearError()
    try {
      snapshot.value = await backend.readEgress()
      applySnapshotToForm()
    } catch (failure) {
      snapshot.value = null
      setError(failure, 'settings.egress.errors.read_failed')
    } finally {
      loading.value = false
    }
  }

  function translateLocalErrors(errors: EgressFormError[]): Record<string, string> {
    const next: Record<string, string> = {}
    for (const item of errors) {
      next[item.field] = t(`settings.egress.errors.${item.code}`)
    }
    return next
  }

  function buildUpdate(): EgressUpdateInput {
    const values = form.value
    const name = values.name.trim()
    // 名字被清空（未启用时允许）= 删除该条定义；绝不能把空名字提交给服务端（422 missing_required）
    const nextProxies = buildProxyInputs(proxies.value, values, {
      previousName: loadedName.value,
    }).filter((proxy) => proxy.name.trim().length > 0)

    const update: EgressUpdateInput = {
      // 保留其它定义；改名时移除旧定义（卡片语义是同一条定义）
      proxies: nextProxies,
      default: values.enabled ? name : null,
      failOpen: values.failOpen,
      probeUrl: values.probeUrl.trim() ? values.probeUrl.trim() : null,
      probeTimeoutSeconds: values.probeTimeoutSeconds,
    }
    return update
  }

  async function save(): Promise<boolean> {
    clearError()
    const localErrors = validateEgressForm(form.value, { allowIpv6Host: false })
    if (localErrors.length > 0) {
      fieldErrors.value = translateLocalErrors(localErrors)
      error.value = t('settings.egress.errors.invalid_form')
      return false
    }

    saving.value = true
    try {
      await backend.writeEgress(buildUpdate())
      await load()
      notify(t('settings.egress.messages.saved'), 'success')
      if (warningTexts.value.length > 0) {
        notify(warningTexts.value[0], 'warning')
      }
      return true
    } catch (failure) {
      setError(failure, 'settings.egress.errors.save_failed')
      return false
    } finally {
      saving.value = false
    }
  }

  async function clear(): Promise<boolean> {
    clearError()
    clearing.value = true
    try {
      await backend.clearEgress()
      probeResult.value = null
      await load()
      notify(t('settings.egress.messages.cleared'), 'success')
      return true
    } catch (failure) {
      setError(failure, 'settings.egress.errors.clear_failed')
      return false
    } finally {
      clearing.value = false
    }
  }

  async function probe(): Promise<boolean> {
    clearError()
    if (!probeSupported.value) {
      errorCode.value = 'unsupported'
      error.value = t('settings.egress.errors.probe_unsupported')
      return false
    }
    probing.value = true
    try {
      probeResult.value = await backend.probeEgress({
        package: probeTarget.value || null,
      })
      notify(t('settings.egress.messages.probed'), 'success')
      return true
    } catch (failure) {
      probeResult.value = null
      setError(failure, 'settings.egress.errors.probe_failed')
      return false
    } finally {
      probing.value = false
    }
  }

  /**
   * 重启设备内中继：POST /egress/relay/restart（ops 作用域，无 If-Match / Idempotency-Key）。
   *
   * 失败必须把错误码展示出来（界面用 errorCode 渲染 [code]），不能只弹一句「失败」。
   */
  async function restartRelay(): Promise<boolean> {
    clearError()
    if (!relayRestartSupported.value) {
      errorCode.value = 'unsupported'
      error.value = t('settings.egress.errors.relay_restart_unsupported')
      return false
    }
    restarting.value = true
    try {
      const result = await backend.restartEgressRelay()
      const restartRequired = result.relay?.restartRequiredApps ?? []
      // 响应已带回重建后的 relay：先就地更新，避免额外一次往返期间界面闪回旧端口。
      if (result.relay && snapshot.value) {
        snapshot.value = {
          ...snapshot.value,
          relay: result.relay,
          warnings: result.warnings.length > 0 ? result.warnings : snapshot.value.warnings,
        }
      }
      await load()
      // load() 自己会吞掉读取错误并写进 error：读回失败时不要谎报「已重建」。
      if (!error.value) {
        notify(t('settings.egress.messages.relay_restarted'), 'success')
        if (warningTexts.value.length > 0) {
          notify(warningTexts.value[0], 'warning')
        }
        if (restartRequired.length > 0) {
          notify(
            t('settings.egress.relay.restart_required', { apps: restartRequired.join(', ') }),
            'warning'
          )
        }
      }
      return true
    } catch (failure) {
      setError(failure, 'settings.egress.errors.relay_restart_failed')
      return false
    } finally {
      restarting.value = false
    }
  }

  function openAdvanced(): void {
    advancedOpen.value = true
  }

  return {
    // state
    snapshot,
    form,
    loadedName,
    loading,
    saving,
    clearing,
    probing,
    restarting,
    error,
    errorCode,
    errorMessage,
    errorDetail,
    fieldErrors,
    probeResult,
    probeTarget,
    advancedOpen,
    // computed
    runtime,
    supported,
    ksuMode,
    probeSupported,
    proxies,
    apps,
    hasCredentials,
    applied,
    readbackAvailable,
    revisionAvailable,
    revisionLabel,
    appliedLabel,
    modeLabel,
    effectiveModeLabel,
    lastProbe,
    degraded,
    warnings,
    warningTexts,
    probeApproximate,
    // 设备内中继
    relay,
    relayListeners,
    relayPortChanges,
    relayRestartRequired,
    relayRestartSupported,
    proxyPath,
    // actions
    load,
    save,
    clear,
    probe,
    restartRelay,
    clearError,
    warningText,
    openAdvanced,
  }
})
