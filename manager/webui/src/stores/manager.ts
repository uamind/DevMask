// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask
//
// Manager store：所有数据来自 Relay /relay/api/v1（API.md §6 映射表）。
// - load(): GET /devices + /apps + /templates + /overview + /audit?limit=20
// - addDevice / removeDevice / syncDevice / syncAll：真实设备注册表动作
// - setAppConfig：POST /apply assignments（单条）；removeAppConfig：透传 DELETE /apps/{pkg}
// - batchApply：POST /apply bulk + 轮询 GET /jobs/{id}
// - regenerateToken：POST /token/rotate（旧 Token 当场失效）
// 只有 VITE_MOCK=1 时 load() 才读取 src/mock 的纯视觉预览数据，且写操作会明确报错。

import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import {
  ApiError,
  AuthError,
  createManagerClient,
  describeError,
  getStoredToken,
  storeToken,
} from '../api/client'
import type {
  ApplyAcceptedResponse,
  ApplyAssignmentsRequest,
  ApplyBulkRequest,
  AppsMatrixResponse,
  AuditEntry,
  CreateEgressProxyRequest,
  DeviceDetail,
  DeviceEgressResponse,
  DeviceSummary,
  EgressApplyPlanItem,
  EgressApplyRequest,
  EgressProbeResponse,
  EgressProxy,
  EgressProxyDetail,
  EgressProxyQuery,
  HealthResponse,
  Job,
  JsonObject,
  ManagerOverviewResponse,
  RotateTokenResponse,
  TemplateSummary,
  UpdateEgressProxyRequest,
  UpdateEgressProxyResponse,
  Warning,
} from '../api/types'
import { appDefFor, knownAppPackages } from '../appCatalog'
import type {
  AppConfig,
  AppDef,
  BatchApplyInput,
  Device,
  LogEntry,
  MatrixRow,
  NewDeviceInput,
  TemplateDef,
} from '../types'
import { relativeTime } from '../utils/format'
import { templateRefForApply } from '../utils/templatePayload'

const JOB_TERMINAL = new Set(['succeeded', 'partial', 'failed'])
const JOB_POLL_TIMEOUT_MS = 60_000
const PAGE_LIMIT = 500

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms)
  })
}

function warningText(warnings: Warning[] | null | undefined): string | undefined {
  if (!warnings || !warnings.length) return undefined
  return warnings.map((warning) => warning.message || warning.code).join('；')
}

function toTemplateDef(summary: TemplateSummary): TemplateDef {
  const config = summary.config ?? {}
  return {
    id: summary.name,
    name: summary.name,
    model: typeof config.model === 'string' && config.model ? config.model : summary.name,
    brand: typeof config.brand === 'string' && config.brand ? config.brand : '--',
    android: typeof config.android_version === 'string' ? config.android_version : '--',
    dpi: typeof config.dpi === 'number' ? config.dpi : null,
    config,
    sources: summary.sources ?? [],
    complete: Boolean(summary.complete),
  }
}

function compactDetail(detail: JsonObject): string {
  const parts = Object.entries(detail).map(([key, value]) => {
    if (value === null || value === undefined) return `${key}=null`
    if (typeof value === 'object') return `${key}=${JSON.stringify(value)}`
    return `${key}=${String(value)}`
  })
  const text = parts.join(' · ')
  return text.length > 160 ? `${text.slice(0, 160)}…` : text
}

const AUDIT_META: Record<string, { type: LogEntry['type']; label: string }> = {
  'auth.failed': { type: 'error', label: '鉴权失败' },
  'device.add': { type: 'add', label: '添加设备' },
  'device.update': { type: 'sync', label: '更新设备' },
  'device.remove': { type: 'remove', label: '移除设备' },
  'device.sync': { type: 'sync', label: '同步设备' },
  'device.sync_all': { type: 'sync', label: '同步全部设备' },
  'apply.bulk': { type: 'batch', label: '批量下发' },
  'apply.assignments': { type: 'batch', label: '逐项下发' },
  'job.finished': { type: 'batch', label: '下发完成' },
  'token.rotate': { type: 'sync', label: '轮换 Manager Token' },
}

function auditToLog(entry: AuditEntry): LogEntry {
  const meta = AUDIT_META[entry.action] ?? { type: 'sync' as const, label: entry.action }
  const target = entry.target?.id ? ` · ${entry.target.type} ${entry.target.id}` : ''
  return {
    id: `audit-${entry.id}`,
    type: entry.status === 'failed' ? 'error' : meta.type,
    title: `${meta.label}${target}`,
    detail: entry.detail ? compactDetail(entry.detail) : entry.target?.type ?? '—',
    time: relativeTime(entry.ts),
    status: entry.status === 'failed' ? 'error' : meta.type === 'remove' ? 'info' : 'success',
    requestId: entry.requestId,
  }
}

export const useManagerStore = defineStore('manager', () => {
  const client = createManagerClient()
  const mockMode = import.meta.env.VITE_MOCK === '1'

  // ---------------------------------------------------------------- state
  const devices = ref<DeviceSummary[]>([])
  // 出口代理（egress）：代理池列表与每台设备的绑定/实时状态缓存。
  const egressProxies = ref<EgressProxy[]>([])
  const egressProxyTotal = ref(0)
  const egressLoading = ref(false)
  const egressSaving = ref(false)
  const egressProbingIds = ref<string[]>([])
  const deviceEgressById = ref<Record<string, DeviceEgressResponse>>({})
  /** 代理池当前筛选条件（增删改后复用，避免列表与工具栏筛选不一致）。 */
  const egressQueryState = ref<EgressProxyQuery>({})
  const templateSummaries = ref<TemplateSummary[]>([])
  const matrixData = ref<AppsMatrixResponse | null>(null)
  const matrixNextCursor = ref<string | null>(null)
  const overview = ref<ManagerOverviewResponse | null>(null)
  const health = ref<HealthResponse | null>(null)
  const auditEntries = ref<AuditEntry[]>([])
  const auditTotal = ref(0)

  const loading = ref(false)
  const loadingAudit = ref(false)
  const syncingAll = ref(false)
  const syncingIds = ref<string[]>([])
  const applying = ref(false)
  const savingCell = ref(false)
  const rotatingToken = ref(false)

  const error = ref<string | null>(null)
  const errorRequestId = ref<string | null>(null)
  const relayError = ref<string | null>(null)
  const authRequired = ref(false)

  const token = ref(getStoredToken())
  const tokenVisible = ref(false)

  // ---------------------------------------------------------------- derived
  const onlineCount = computed(() => devices.value.filter((device) => device.online).length)
  const offlineCount = computed(() => devices.value.length - onlineCount.value)
  const deviceCount = computed(() => devices.value.length)
  const relayOnline = computed(() => Boolean(health.value) && !relayError.value)

  const templates = computed<TemplateDef[]>(() => templateSummaries.value.map(toTemplateDef))

  const matrix = computed<MatrixRow[]>(() => {
    const rows = matrixData.value?.apps ?? []
    return rows.map((row) => {
      const cells: Record<string, AppConfig | undefined> = {}
      for (const [deviceId, cell] of Object.entries(row.cells ?? {})) {
        if (!cell) {
          cells[deviceId] = undefined
          continue
        }
        cells[deviceId] = {
          appId: row.package,
          deviceId,
          model: cell.model ?? '未识别',
          template: cell.template,
          source: cell.source === 'inline' ? 'inline' : 'template',
          warning: warningText(cell.warnings),
        }
      }
      return {
        app: appDefFor(row.package),
        cells,
        configured: row.configured,
        total: row.total,
        inconsistent: row.inconsistent,
        models: row.models,
        offlineDevices: row.offlineDevices,
        warnings: row.warnings ?? [],
      }
    })
  })

  /** 已配置应用（矩阵行），保持 API 的 package 升序。 */
  const apps = computed<AppDef[]>(() => {
    const seen = new Set<string>()
    const result: AppDef[] = []
    for (const row of matrix.value) {
      if (seen.has(row.app.package)) continue
      seen.add(row.app.package)
      result.push(row.app)
    }
    return result
  })

  /** 批量页可选项：矩阵里的应用 + UI 目录里的常见应用。 */
  const appOptions = computed<AppDef[]>(() => {
    const byPackage = new Map<string, AppDef>()
    for (const app of apps.value) byPackage.set(app.package, app)
    for (const packageName of knownAppPackages()) {
      if (!byPackage.has(packageName)) byPackage.set(packageName, appDefFor(packageName))
    }
    return Array.from(byPackage.values())
  })

  const appCount = computed(() => apps.value.length)

  /** 所有非空矩阵单元格的扁平列表（Dashboard / Apps 页的「配置条数」）。 */
  const configs = computed<AppConfig[]>(() => {
    const list: AppConfig[] = []
    for (const row of matrix.value) {
      for (const cell of Object.values(row.cells)) {
        if (cell) list.push(cell)
      }
    }
    return list
  })

  const driftRows = computed(() => matrix.value.filter((row) => row.inconsistent))

  const logs = computed<LogEntry[]>(() => auditEntries.value.map(auditToLog))
  const recentLogs = computed(() => logs.value.slice(0, 5))

  const averageLatencyMs = computed<number | null>(() => {
    const samples = devices.value
      .map((device) => device.latencyMs)
      .filter((value): value is number => typeof value === 'number')
    if (!samples.length) return null
    return Math.round(samples.reduce((sum, value) => sum + value, 0) / samples.length)
  })

  /** 代理池里有绑定（整机或应用级）的代理数量。 */
  const boundProxyCount = computed(
    () =>
      egressProxies.value.filter((proxy) => proxy.boundDevices > 0 || proxy.boundApps > 0).length
  )

  /** 设备级没有代理绑定的设备（DevicesPage「未配代理」筛选 + 批量绑定候选）。 */
  const devicesWithoutEgress = computed(() => devices.value.filter((device) => !hasDeviceEgress(device)))

  /** 设备级代理摘要（DeviceSummary.egress）是否指向一个代理。 */
  function hasDeviceEgress(device: DeviceSummary): boolean {
    return device.egress?.mode === 'proxy' && Boolean(device.egress?.proxyName)
  }

  // ---------------------------------------------------------------- helpers
  function deviceById(id: string): DeviceSummary | undefined {
    return devices.value.find((device) => device.id === id)
  }

  function appById(id: string): AppDef | undefined {
    return (
      apps.value.find((app) => app.package === id || app.id === id) ??
      appOptions.value.find((app) => app.package === id || app.id === id)
    )
  }

  function matrixFor(packageName: string): MatrixRow | undefined {
    return matrix.value.find(
      (row) => row.app.package === packageName || row.app.id === packageName
    )
  }

  function configFor(appId: string, deviceId: string): AppConfig | undefined {
    return matrixFor(appId)?.cells[deviceId]
  }

  function appCountForDevice(deviceId: string): number {
    return matrix.value.filter((row) => row.cells[deviceId]).length
  }

  function isSyncing(id: string): boolean {
    return syncingIds.value.includes(id)
  }

  function egressProxyById(id: string): EgressProxy | undefined {
    return egressProxies.value.find((proxy) => proxy.id === id)
  }

  /** 某台设备最近一次 GET /devices/{id}/egress 的结果（未拉取过为 null）。 */
  function deviceEgressState(deviceId: string): DeviceEgressResponse | null {
    return deviceEgressById.value[deviceId] ?? null
  }

  function isProbingProxy(id: string): boolean {
    return egressProbingIds.value.includes(id)
  }

  function clearError(): void {
    error.value = null
    errorRequestId.value = null
  }

  function reportError(cause: unknown, fallback: string): string {
    if (cause instanceof AuthError) authRequired.value = true
    const message = describeError(cause, fallback)
    error.value = message
    errorRequestId.value = cause instanceof ApiError ? cause.requestId : null
    return message
  }

  function previewOnly(): ApiError {
    return new ApiError({
      status: 0,
      code: 'mock_mode',
      message:
        'VITE_MOCK=1 为纯视觉预览：只展示示例数据。请启动 Relay（127.0.0.1:8765）后使用真实 API。',
      details: null,
      retryable: false,
      requestId: null,
    })
  }

  function replaceDevice(updated: DeviceSummary): void {
    const index = devices.value.findIndex((device) => device.id === updated.id)
    if (index === -1) devices.value = [...devices.value, updated]
    else devices.value = devices.value.map((device) => (device.id === updated.id ? updated : device))
  }

  // ---------------------------------------------------------------- loaders
  async function loadMock(): Promise<void> {
    loading.value = true
    clearError()
    try {
      const mock = await import('../mock/data')
      devices.value = mock.mockDevices.map((device) => ({ ...device, tags: [...device.tags] }))
      egressProxies.value = mock.mockEgressProxies.map((proxy) => ({ ...proxy, tags: [...proxy.tags] }))
      egressProxyTotal.value = mock.mockEgressProxies.length
      deviceEgressById.value = mock.mockDeviceEgress
      templateSummaries.value = mock.mockTemplates.map((template) => ({ ...template }))
      matrixData.value = mock.mockMatrix
      matrixNextCursor.value = null
      overview.value = mock.mockOverview
      auditEntries.value = mock.mockAudit
      auditTotal.value = mock.mockAudit.length
      health.value = mock.mockHealth
      relayError.value = null
    } finally {
      loading.value = false
    }
  }

  async function load(): Promise<void> {
    if (import.meta.env.VITE_MOCK === '1') {
      await loadMock()
      return
    }
    loading.value = true
    clearError()
    authRequired.value = false
    try {
      const [devicesResult, matrixResult, templatesResult, overviewResult, auditResult] =
        await Promise.allSettled([
          client.listDevices({ limit: PAGE_LIMIT }),
          client.getAppsMatrix({ limit: PAGE_LIMIT }),
          client.listTemplates({ limit: PAGE_LIMIT }),
          client.getOverview(),
          client.listAudit({ limit: 20 }),
        ])

      if (devicesResult.status === 'fulfilled') devices.value = devicesResult.value.devices
      if (matrixResult.status === 'fulfilled') {
        matrixData.value = matrixResult.value
        matrixNextCursor.value = matrixResult.value.nextCursor
      }
      if (templatesResult.status === 'fulfilled') {
        templateSummaries.value = templatesResult.value.templates
      }
      if (overviewResult.status === 'fulfilled') overview.value = overviewResult.value
      if (auditResult.status === 'fulfilled') {
        auditEntries.value = auditResult.value.entries
        auditTotal.value = auditResult.value.total
      }

      const failed = [devicesResult, matrixResult, templatesResult, overviewResult, auditResult].find(
        (result) => result.status === 'rejected'
      )
      if (failed && failed.status === 'rejected') {
        reportError(failed.reason, '加载 Relay 数据失败')
      }
    } finally {
      loading.value = false
    }
  }

  async function refreshHealth(): Promise<boolean> {
    if (mockMode) {
      relayError.value = null
      return Boolean(health.value)
    }
    try {
      health.value = await client.healthz()
      relayError.value = null
      return true
    } catch (cause) {
      health.value = null
      relayError.value = describeError(cause, 'Relay 未连接')
      return false
    }
  }

  async function loadAudit(limit = 100): Promise<void> {
    if (import.meta.env.VITE_MOCK === '1') {
      loadingAudit.value = true
      try {
        const mock = await import('../mock/data')
        auditEntries.value = mock.mockAudit
        auditTotal.value = mock.mockAudit.length
      } finally {
        loadingAudit.value = false
      }
      return
    }
    loadingAudit.value = true
    try {
      const response = await client.listAudit({ limit })
      auditEntries.value = response.entries
      auditTotal.value = response.total
    } catch (cause) {
      reportError(cause, '加载审计日志失败')
    } finally {
      loadingAudit.value = false
    }
  }

  async function reloadAuditQuiet(): Promise<void> {
    if (mockMode) return
    try {
      const response = await client.listAudit({ limit: 20 })
      auditEntries.value = response.entries
      auditTotal.value = response.total
    } catch {
      // 审计刷新失败不影响主流程
    }
  }

  async function refreshAggregates(): Promise<void> {
    if (mockMode) return
    const [matrixResult, templatesResult, overviewResult] = await Promise.allSettled([
      client.getAppsMatrix({ limit: PAGE_LIMIT }),
      client.listTemplates({ limit: PAGE_LIMIT }),
      client.getOverview(),
    ])
    if (matrixResult.status === 'fulfilled') {
      matrixData.value = matrixResult.value
      matrixNextCursor.value = matrixResult.value.nextCursor
    }
    if (templatesResult.status === 'fulfilled') {
      templateSummaries.value = templatesResult.value.templates
    }
    if (overviewResult.status === 'fulfilled') overview.value = overviewResult.value
  }

  async function loadMoreMatrix(): Promise<void> {
    if (mockMode || !matrixNextCursor.value) return
    loading.value = true
    try {
      const response = await client.getAppsMatrix({ limit: PAGE_LIMIT, cursor: matrixNextCursor.value })
      if (matrixData.value) {
        matrixData.value = {
          ...matrixData.value,
          apps: [...matrixData.value.apps, ...response.apps],
          nextCursor: response.nextCursor,
        }
      }
      matrixNextCursor.value = response.nextCursor
    } catch (cause) {
      reportError(cause, '加载更多应用失败')
    } finally {
      loading.value = false
    }
  }

  // ---------------------------------------------------------------- devices
  async function addDevice(input: NewDeviceInput): Promise<DeviceSummary> {
    if (mockMode) throw previewOnly()
    try {
      const created = await client.addDevice({
        name: input.name.trim(),
        baseUrl: input.baseUrl.trim(),
        token: input.token,
        tags: input.tags.map((tag) => tag.trim()).filter(Boolean),
      })
      replaceDevice(created)
      await refreshAggregates()
      await reloadAuditQuiet()
      return created
    } catch (cause) {
      throw reportErrorThrown(cause, '添加设备失败')
    }
  }

  function reportErrorThrown(cause: unknown, fallback: string): unknown {
    reportError(cause, fallback)
    return cause
  }

  async function removeDevice(id: string): Promise<void> {
    if (mockMode) throw previewOnly()
    try {
      await client.removeDevice(id)
      devices.value = devices.value.filter((device) => device.id !== id)
      await refreshAggregates()
      await reloadAuditQuiet()
    } catch (cause) {
      throw reportErrorThrown(cause, '移除设备失败')
    }
  }

  async function syncDevice(id: string): Promise<{
    online: boolean
    warnings: Warning[]
    latencyMs: number | null
  }> {
    if (mockMode) throw previewOnly()
    syncingIds.value = [...syncingIds.value, id]
    try {
      const response = await client.syncDevice(id)
      replaceDevice(response.device)
      await refreshAggregates()
      await reloadAuditQuiet()
      return {
        online: response.device.online,
        warnings: response.warnings,
        latencyMs: response.device.latencyMs,
      }
    } catch (cause) {
      throw reportErrorThrown(cause, '同步设备失败')
    } finally {
      syncingIds.value = syncingIds.value.filter((item) => item !== id)
    }
  }

  async function syncAll(): Promise<{ synced: number; failed: number }> {
    if (mockMode) throw previewOnly()
    syncingAll.value = true
    try {
      const response = await client.syncAllDevices()
      const devicesResult = await client.listDevices({ limit: PAGE_LIMIT })
      devices.value = devicesResult.devices
      await refreshAggregates()
      await reloadAuditQuiet()
      return { synced: response.synced, failed: response.failed }
    } catch (cause) {
      throw reportErrorThrown(cause, '同步全部设备失败')
    } finally {
      syncingAll.value = false
    }
  }

  function getDeviceDetail(id: string): Promise<DeviceDetail> {
    if (mockMode) return Promise.reject(previewOnly())
    return client.getDevice(id)
  }

  // ---------------------------------------------------------------- egress
  // 契约：docs/plans/egress-contract.md §6。所有写操作都不带 config revision；
  // egress revision 由 client.ts 从 X-DM-Egress-Revision 采集，互不覆盖。

  async function loadEgressProxies(query?: EgressProxyQuery): Promise<void> {
    if (query) egressQueryState.value = query
    const active = egressQueryState.value
    if (mockMode) {
      egressLoading.value = true
      try {
        const mock = await import('../mock/data')
        const keyword = (active.q ?? '').trim().toLowerCase()
        const items = mock.mockEgressProxies.filter((proxy) => {
          if (active.enabled !== undefined && proxy.enabled !== active.enabled) return false
          if (!keyword) return true
          return `${proxy.name} ${proxy.host} ${proxy.region ?? ''}`
            .toLowerCase()
            .includes(keyword)
        })
        egressProxies.value = items.map((proxy) => ({ ...proxy, tags: [...proxy.tags] }))
        egressProxyTotal.value = items.length
      } finally {
        egressLoading.value = false
      }
      return
    }
    egressLoading.value = true
    try {
      const response = await client.listEgressProxies({ limit: PAGE_LIMIT, ...active })
      egressProxies.value = response.items
      egressProxyTotal.value = response.total
    } catch (cause) {
      reportError(cause, '加载代理池失败')
    } finally {
      egressLoading.value = false
    }
  }

  /** 静默刷新代理池（探测失败后同步 lastCheck 用）。 */
  async function reloadEgressProxiesQuiet(): Promise<void> {
    if (mockMode) return
    try {
      const response = await client.listEgressProxies({
        limit: PAGE_LIMIT,
        ...egressQueryState.value,
      })
      egressProxies.value = response.items
      egressProxyTotal.value = response.total
    } catch {
      // 刷新失败不影响主流程
    }
  }

  async function loadEgressProxyDetail(id: string): Promise<EgressProxyDetail> {
    if (mockMode) {
      const mock = await import('../mock/data')
      const detail = mock.mockEgressProxyDetails[id]
      if (!detail) throw previewOnly()
      return detail
    }
    return client.getEgressProxy(id)
  }

  async function createProxy(input: CreateEgressProxyRequest): Promise<EgressProxy> {
    if (mockMode) throw previewOnly()
    egressSaving.value = true
    try {
      const created = await client.createEgressProxy(input)
      await loadEgressProxies()
      await reloadAuditQuiet()
      return created
    } catch (cause) {
      throw reportErrorThrown(cause, '创建代理失败')
    } finally {
      egressSaving.value = false
    }
  }

  async function updateProxy(
    id: string,
    patch: UpdateEgressProxyRequest
  ): Promise<UpdateEgressProxyResponse> {
    if (mockMode) throw previewOnly()
    egressSaving.value = true
    try {
      const updated = await client.updateEgressProxy(id, patch)
      const { warnings: _warnings, ...proxy } = updated
      egressProxies.value = egressProxies.value.map((item) =>
        item.id === proxy.id ? proxy : item
      )
      await reloadAuditQuiet()
      return updated
    } catch (cause) {
      throw reportErrorThrown(cause, '更新代理失败')
    } finally {
      egressSaving.value = false
    }
  }

  async function deleteProxy(id: string): Promise<void> {
    if (mockMode) throw previewOnly()
    egressSaving.value = true
    try {
      await client.deleteEgressProxy(id)
      egressProxies.value = egressProxies.value.filter((proxy) => proxy.id !== id)
      egressProxyTotal.value = Math.max(0, egressProxyTotal.value - 1)
      // 绑定摘要挂在设备列表里，删除后重新拉一次（不含代理的设备不受影响）。
      const devicesResult = await client.listDevices({ limit: PAGE_LIMIT })
      devices.value = devicesResult.devices
      await reloadAuditQuiet()
    } catch (cause) {
      throw reportErrorThrown(cause, '删除代理失败')
    } finally {
      egressSaving.value = false
    }
  }

  async function probeProxy(id: string, url?: string): Promise<EgressProbeResponse> {
    if (mockMode) throw previewOnly()
    egressProbingIds.value = [...egressProbingIds.value, id]
    try {
      const result = await client.probeEgressProxy(id, url ? { url } : {})
      // 就地更新 lastCheck（后端同时写了一条 manager 来源的 egress_checks）。
      egressProxies.value = egressProxies.value.map((proxy) =>
        proxy.id === id
          ? {
              ...proxy,
              lastCheck: {
                source: 'manager' as const,
                deviceId: null,
                app: '',
                ok: true,
                exitIp: result.exitIp,
                latencyMs: result.latencyMs,
                errorCode: null,
                checkedAt: result.checkedAt,
              },
            }
          : proxy
      )
      await reloadAuditQuiet()
      return result
    } catch (cause) {
      // 失败也写了一条 ok=0 的 egress_checks：拉一次让「最近探测」显示原因。
      await reloadEgressProxiesQuiet()
      throw cause
    } finally {
      egressProbingIds.value = egressProbingIds.value.filter((item) => item !== id)
    }
  }

  async function loadDeviceEgress(deviceId: string): Promise<DeviceEgressResponse> {
    if (mockMode) {
      const mock = await import('../mock/data')
      const cached = mock.mockDeviceEgress[deviceId] ?? {
        deviceId,
        bindings: [],
        deviceState: null,
        warnings: [],
      }
      deviceEgressById.value = { ...deviceEgressById.value, [deviceId]: cached }
      return cached
    }
    const response = await client.getDeviceEgress(deviceId)
    deviceEgressById.value = { ...deviceEgressById.value, [deviceId]: response }
    return response
  }

  async function applyEgress(
    body: EgressApplyRequest
  ): Promise<{ job: Job | null; plan: EgressApplyPlanItem[] | null }> {
    if (mockMode) throw previewOnly()
    egressSaving.value = true
    try {
      const accepted = await client.applyEgress(body)
      if ('plan' in accepted) return { job: null, plan: accepted.plan }
      const job = await pollJob(accepted.jobId)
      const devicesResult = await client.listDevices({ limit: PAGE_LIMIT })
      devices.value = devicesResult.devices
      await refreshAggregates()
      await reloadEgressProxiesQuiet()
      await reloadAuditQuiet()
      return { job, plan: null }
    } catch (cause) {
      throw reportErrorThrown(cause, '下发出口代理失败')
    } finally {
      egressSaving.value = false
    }
  }

  // ---------------------------------------------------------------- apply
  async function pollJob(jobId: string): Promise<Job> {
    const startedAt = Date.now()
    let delayMs = 500
    for (;;) {
      const job = await client.getJob(jobId)
      if (JOB_TERMINAL.has(job.status)) return job
      if (Date.now() - startedAt > JOB_POLL_TIMEOUT_MS) return job
      await sleep(delayMs)
      delayMs = Math.min(Math.round(delayMs * 1.6), 2500)
    }
  }

  async function applyAndPoll(body: ApplyBulkRequest | ApplyAssignmentsRequest): Promise<Job> {
    const accepted: ApplyAcceptedResponse = await client.apply(body)
    return pollJob(accepted.jobId)
  }

  async function batchApply(input: BatchApplyInput): Promise<Job> {
    if (mockMode) throw previewOnly()
    applying.value = true
    try {
      const job = await applyAndPoll({
        app: input.app,
        devices: input.devices,
        // packages 只是 /templates 的展示性快照，不能回流到设备（t8 R1）
        template: templateRefForApply(input.template.name, input.template.config),
        restart: input.restart,
      } satisfies ApplyBulkRequest)
      await refreshAggregates()
      await reloadAuditQuiet()
      return job
    } catch (cause) {
      throw reportErrorThrown(cause, '下发失败')
    } finally {
      applying.value = false
    }
  }

  async function setAppConfig(
    packageName: string,
    deviceId: string,
    template: TemplateDef
  ): Promise<Job> {
    if (mockMode) throw previewOnly()
    savingCell.value = true
    try {
      const job = await applyAndPoll({
        assignments: [
          {
            device: deviceId,
            app: packageName,
            // 与 bulk 同一 helper：剔除展示性 packages，避免跨设备包绑定泄漏
            template: templateRefForApply(template.name, template.config),
          },
        ],
        restart: false,
      } satisfies ApplyAssignmentsRequest)
      await refreshAggregates()
      await reloadAuditQuiet()
      return job
    } catch (cause) {
      throw reportErrorThrown(cause, '配置应用失败')
    } finally {
      savingCell.value = false
    }
  }

  async function removeAppConfig(packageName: string, deviceId: string): Promise<void> {
    if (mockMode) throw previewOnly()
    savingCell.value = true
    try {
      await client.removeDeviceApp(deviceId, packageName)
      await refreshAggregates()
      await reloadAuditQuiet()
    } catch (cause) {
      throw reportErrorThrown(cause, '清除应用配置失败')
    } finally {
      savingCell.value = false
    }
  }

  // ---------------------------------------------------------------- token
  async function regenerateToken(customToken?: string): Promise<RotateTokenResponse> {
    if (mockMode) throw previewOnly()
    rotatingToken.value = true
    try {
      const body = customToken && customToken.trim() ? { token: customToken.trim() } : {}
      const response = await client.rotateToken(body)
      storeToken(response.token)
      token.value = response.token
      tokenVisible.value = true
      await load()
      return response
    } catch (cause) {
      throw reportErrorThrown(cause, '轮换 Manager Token 失败')
    } finally {
      rotatingToken.value = false
    }
  }

  async function applyToken(value: string): Promise<void> {
    const next = value.trim()
    if (!next) return
    storeToken(next)
    token.value = next
    authRequired.value = false
    clearError()
    await load()
    await refreshHealth()
  }

  async function resetPreviewData(): Promise<void> {
    if (import.meta.env.VITE_MOCK !== '1') return
    await loadMock()
  }

  return {
    // state
    devices,
    templates,
    matrix,
    apps,
    appOptions,
    appCount,
    configs,
    driftRows,
    logs,
    recentLogs,
    overview,
    health,
    auditTotal,
    loading,
    loadingAudit,
    syncingAll,
    syncingIds,
    applying,
    savingCell,
    rotatingToken,
    error,
    errorRequestId,
    relayError,
    relayOnline,
    authRequired,
    token,
    tokenVisible,
    mockMode,
    matrixNextCursor,
    // egress state
    egressProxies,
    egressProxyTotal,
    egressLoading,
    egressSaving,
    egressProbingIds,
    deviceEgressById,
    // derived
    onlineCount,
    offlineCount,
    deviceCount,
    averageLatencyMs,
    boundProxyCount,
    devicesWithoutEgress,
    // getters
    deviceById,
    appById,
    matrixFor,
    configFor,
    appCountForDevice,
    isSyncing,
    hasDeviceEgress,
    egressProxyById,
    deviceEgressState,
    isProbingProxy,
    // actions
    load,
    refreshHealth,
    loadAudit,
    loadMoreMatrix,
    addDevice,
    removeDevice,
    syncDevice,
    syncAll,
    getDeviceDetail,
    // egress actions
    loadEgressProxies,
    reloadEgressProxiesQuiet,
    loadEgressProxyDetail,
    createProxy,
    updateProxy,
    deleteProxy,
    probeProxy,
    loadDeviceEgress,
    applyEgress,
    batchApply,
    setAppConfig,
    removeAppConfig,
    regenerateToken,
    applyToken,
    resetPreviewData,
    clearError,
  }
})

export type ManagerStore = ReturnType<typeof useManagerStore>
export type { Device }
