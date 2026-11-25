// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker
//
// Manager Relay API 类型（API v1 / manager 0.1.0）。
// 契约来源：manager/docs/API.md 与 manager/docs/api-types.ts（已冻结，勿改字段名）。
// 本文件是 WebUI 侧的只读副本；契约变更请先改 docs/API.md 与 docs/api-types.ts 再同步。
//
// 命名约定：
// - Relay 元数据：lowerCamelCase（baseUrl / lastSyncedAt / httpStatus）
// - 设备配置内容（TemplateInput / ResolvedConfig / DeviceOverview）：snake_case，与设备端 config.toml 一致
// - 模板名：TemplateName；设备内部产品名在 TemplateInput.config.name 里，二者不同

/** RFC 3339 UTC 时间字符串，如 2026-11-24T08:12:31Z。 */
export type Iso8601 = string

/** 设备配置 revision（不透明字符串，可能带 sha256: 前缀）。 */
export type Revision = string

/** Java 包名，可带 @userId。 */
export type PackageName = string

/** 模板名：^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$ */
export type TemplateName = string

/** 设备 id：^[a-z0-9][a-z0-9_-]{0,63}$ */
export type DeviceId = string

/** job id：^job_[0-9a-f]{32}$ */
export type JobId = string

/** 请求 id：^req_[0-9a-f]{32}$ */
export type RequestId = string

/** JSON 值（用于 details / audit detail / custom_props 等自由结构）。 */
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }

/** JSON 对象。 */
export interface JsonObject {
  [key: string]: JsonValue
}

// ---------------------------------------------------------------------------
// 错误信封（API.md §2.6 / §2.7）
// ---------------------------------------------------------------------------

/** Manager 稳定错误码。 */
export type ManagerErrorCode =
  | 'unauthorized'
  | 'forbidden_scope'
  | 'not_found'
  | 'method_not_allowed'
  | 'device_not_found'
  | 'device_exists'
  | 'device_probe_failed'
  | 'validation_failed'
  | 'body_too_large'
  | 'unsupported_media_type'
  | 'invalid_state'
  | 'device_offline'
  | 'device_error'
  | 'upstream_timeout'
  | 'internal_error'
  | 'rate_limited'
  /** 代理名已被占用（409，契约 §6.3）。 */
  | 'egress_proxy_exists'
  /** 仍有绑定引用，拒绝删除（409，契约 §6.6）。 */
  | 'egress_proxy_in_use'
  /** Manager 直连探测失败（502，契约 §6.7）。 */
  | 'egress_probe_failed'

/** 请求体 / 查询参数校验错误码（details.errors[].code）。 */
export type ValidationErrorCode =
  | 'missing_required'
  | 'unknown_field'
  | 'invalid_type'
  | 'invalid_value'
  | 'invalid_package'
  | 'invalid_template_name'
  | 'invalid_device_id'
  | 'invalid_base_url'
  | 'invalid_token'
  | 'out_of_range'
  | 'too_many_items'
  | 'duplicate_assignment'
  /** 代理名不匹配 §2.5 正则或等于保留值 __DIRECT__。 */
  | 'invalid_egress_name'
  /** 代理 type / host / port / 凭据越界（契约 §6.3）。 */
  | 'invalid_egress_proxy'
  /** 探测 URL 非 http:// 或超长（契约 §6.7）。 */
  | 'invalid_probe_url'

/** 单个字段校验错误。 */
export interface ValidationError {
  /** 如 assignments[1].template / limit / template.config.model */
  path: string
  code: ValidationErrorCode
  message: string
}

/** validation_failed 的 details 形状。 */
export interface ValidationFailureDetails {
  errors: ValidationError[]
}

/** 业务错误对象（job 结果里的 error 同形，但没有 requestId）。 */
export interface ApiErrorPayload {
  code: ManagerErrorCode | string
  message: string
  details: JsonObject | null
  retryable: boolean
}

/** 所有 4xx / 5xx 的唯一响应体。 */
export interface ErrorEnvelope {
  error: ApiErrorPayload & { requestId: RequestId }
}

/** 前端 ApiError 建议形状（client.ts 抛出的错误）。 */
export interface ApiErrorShape {
  name: 'ApiError'
  status: number
  code: ManagerErrorCode | string
  message: string
  details: JsonObject | null
  retryable: boolean
  requestId: RequestId | null
}

/** 401 时抛出的专用错误，用于触发 Token 对话框。 */
export interface AuthErrorShape {
  name: 'AuthError'
  status: 401
  message: string
}

// ---------------------------------------------------------------------------
// 健康检查（API.md §4.1）
// ---------------------------------------------------------------------------

export type HealthStatus = 'ok' | 'degraded'

export interface HealthDbInfo {
  status: 'ok' | 'error'
  schemaVersion: number
  /** 相对数据目录的文件名，不是绝对路径。 */
  path: string
}

export interface HealthDeviceCounts {
  total: number
  online: number
  offline: number
}

export interface HealthJobCounts {
  queued: number
  running: number
}

export interface HealthResponse {
  status: HealthStatus
  version: string
  apiVersion: 'v1'
  uptimeSec: number
  startedAt: Iso8601
  db: HealthDbInfo
  devices: HealthDeviceCounts
  jobs: HealthJobCounts
}

/** 兼容别名。 */
export type HealthzResponse = HealthResponse

// ---------------------------------------------------------------------------
// 设备端 overview 透传（API.md §5.2）
// ---------------------------------------------------------------------------

/** 设备端模板对象（snake_case，不含模板名）。 */
export interface TemplateInput {
  packages?: PackageName[]
  manufacturer?: string
  marketname?: string
  brand?: string
  model?: string
  /** 设备内部产品名（ro.product.name），不是模板名。 */
  name?: string
  device?: string
  product?: string
  hardware?: string
  board?: string
  soc_model?: string
  fingerprint?: string
  build_id?: string
  display_id?: string
  incremental?: string
  security_patch?: string
  characteristics?: string
  android_version?: string
  /** 1..1000，或 "__DELETE__"。 */
  sdk_int?: number | '__DELETE__'
  dpi?: number
  custom_props?: Record<string, string>
  force_denylist_unmount?: boolean
  hide_maps?: string[]
  companion_resetprop?: boolean
}

/** 设备端 resolved（最终生效值，snake_case）。 */
export interface ResolvedConfig {
  manufacturer?: string
  marketname?: string
  brand?: string
  model?: string
  name?: string
  device?: string
  product?: string
  hardware?: string
  board?: string
  soc_model?: string
  fingerprint?: string
  build_id?: string
  display_id?: string
  incremental?: string
  security_patch?: string
  characteristics?: string
  android_version?: string
  sdk_int?: number | '__DELETE__'
  dpi?: number
  custom_props?: Record<string, string>
  force_denylist_unmount?: boolean
  hide_maps?: string[]
  companion_resetprop?: boolean
}

export interface DeviceModuleInfo {
  id: string
  version: string
  versionCode: number
}

export interface DeviceHardwareInfo {
  manufacturer: string
  brand: string
  model: string
  device: string
  android: string
  sdk: number
  abi: string
  fingerprint: string
}

export interface DeviceRootInfo {
  manager: string | null
  version: string | null
}

export interface DeviceServerInfo {
  version: string
  bind: string
  uptimeSec: number
  startedAt: Iso8601
  pid: number
}

export interface DeviceConfigMeta {
  revision: Revision
  mtime: Iso8601 | null
  size: number
  exists: boolean
  templates: number
  apps: number
  conflicts: number
}

export interface DeviceTemplateEntry {
  name: TemplateName
  position: number
  packages: PackageName[]
  config: TemplateInput
}

export type AppSourceType = 'inline' | 'template' | 'none'

export interface DeviceAppSource {
  type: AppSourceType
  template: TemplateName | null
  alsoInline?: boolean
}

export type Warning = {
  code: string
  message: string
  details: JsonObject | null
}

export interface DeviceAppEntry {
  package: PackageName
  installed: boolean | null
  source: DeviceAppSource
  resolved: ResolvedConfig
  warnings: Warning[]
}

/** 设备端 GET /api/v1/overview 的响应，Relay 原样缓存与透出。 */
export interface DeviceOverview {
  revision: Revision
  module: DeviceModuleInfo
  device: DeviceHardwareInfo
  root: DeviceRootInfo | null
  server: DeviceServerInfo
  config: DeviceConfigMeta
  templates: DeviceTemplateEntry[]
  apps: DeviceAppEntry[]
  warnings: Warning[]
}

// ---------------------------------------------------------------------------
// 设备（API.md §5.1 / §4.2-§4.9）
// ---------------------------------------------------------------------------

export interface DeviceLastError {
  code: string
  message: string
}

export interface DeviceSummary {
  id: DeviceId
  name: string
  baseUrl: string
  online: boolean
  tags: string[]
  model: string | null
  brand: string | null
  android: string | null
  sdk: number | null
  fingerprint: string | null
  root: string | null
  moduleVersion: string | null
  serverVersion: string | null
  lastSeen: Iso8601 | null
  lastSyncedAt: Iso8601 | null
  latencyMs: number | null
  createdAt: Iso8601
  lastError: DeviceLastError | null
  /**
   * 设备级出口摘要（契约 §6.8）：整机绑定 + 最近一条整机探测。
   * 没绑过代理时为 null；只有应用级绑定时 mode/proxyName 也为 null。
   */
  egress: DeviceEgressSummary | null
}

/** 设备详情里的应用摘要。 */
export interface DeviceAppSummary {
  package: PackageName
  installed: boolean | null
  source: DeviceAppSource
  model: string | null
  warnings: Warning[]
}

/** Relay 聚合出的模板（API.md §5.4）。 */
export interface TemplateSummary {
  name: TemplateName
  config: TemplateInput
  fieldsSet: string[]
  sources: DeviceId[]
  complete: boolean
  warnings: Warning[]
}

/** GET /devices/{id}：DeviceSummary + 缓存视图。 */
export interface DeviceDetail extends DeviceSummary {
  overview: DeviceOverview | null
  templates: TemplateSummary[]
  apps: DeviceAppSummary[]
  cachedAt: Iso8601 | null
  stale: boolean
  warnings: Warning[]
}

export interface DevicesResponse {
  devices: DeviceSummary[]
  total: number
  online: number
  offline: number
  nextCursor: string | null
}

export interface DeviceQuery {
  online?: boolean
  /** 多个 tag 为 AND 关系。 */
  tag?: string[]
  q?: string
  limit?: number
  cursor?: string
}

export interface AddDeviceRequest {
  name: string
  baseUrl: string
  token: string
  tags?: string[]
  id?: string
}

export interface UpdateDeviceRequest {
  name?: string
  tags?: string[]
  token?: string
  baseUrl?: string
}

export interface SyncDeviceResponse {
  device: DeviceSummary
  overview: DeviceOverview | null
  warnings: Warning[]
}

export type SyncStatus = 'ok' | 'offline' | 'error'

export interface SyncAllResult {
  deviceId: DeviceId
  status: SyncStatus
  online: boolean
  latencyMs: number | null
  code: string | null
  message: string | null
}

export interface SyncAllResponse {
  synced: number
  failed: number
  results: SyncAllResult[]
  warnings: Warning[]
}

export interface DeviceOverviewResponse {
  deviceId: DeviceId
  online: boolean
  cachedAt: Iso8601 | null
  stale: boolean
  overview: DeviceOverview
}

// ---------------------------------------------------------------------------
// 模板 / 矩阵 / 总览（API.md §4.10-§4.12）
// ---------------------------------------------------------------------------

export interface TemplatesResponse {
  generatedAt: Iso8601
  revision: Revision | null
  templates: TemplateSummary[]
  total: number
  nextCursor: string | null
  warnings: Warning[]
}

export interface MatrixDevice {
  id: DeviceId
  name: string
  online: boolean
  model: string | null
  android: string | null
  lastSyncedAt: Iso8601 | null
  stale: boolean
}

export interface MatrixCell {
  model: string | null
  template: TemplateName | null
  source: AppSourceType
  installed: boolean | null
  drift: boolean
  warnings: Warning[]
}

export interface MatrixApp {
  package: PackageName
  cells: Record<DeviceId, MatrixCell | null>
  configured: number
  total: number
  inconsistent: boolean
  models: string[]
  templates: TemplateName[]
  offlineDevices: DeviceId[]
  warnings: Warning[]
}

export interface AppsMatrixTotals {
  apps: number
  appsConfigured: number
  driftApps: number
  devices: number
}

export interface AppsMatrixResponse {
  generatedAt: Iso8601
  revision: Revision | null
  devices: MatrixDevice[]
  apps: MatrixApp[]
  totals: AppsMatrixTotals
  warnings: Warning[]
  nextCursor: string | null
}

export interface AppsQuery {
  q?: string
  drift?: boolean
  device?: DeviceId
  limit?: number
  cursor?: string
}

export interface FleetJobCounts {
  queued: number
  running: number
  last24h: number
  failedLast24h: number
}

export interface FleetCounts {
  devices: number
  online: number
  offline: number
  apps: number
  appsConfigured: number
  driftApps: number
  templates: number
  averageLatencyMs: number | null
  jobs: FleetJobCounts
}

export interface DriftRow {
  package: PackageName
  configured: number
  total: number
  models: string[]
  driftDevices: DeviceId[]
}

export interface ManagerOverviewResponse {
  generatedAt: Iso8601
  revision: Revision | null
  fleet: FleetCounts
  devices: DeviceSummary[]
  drift: DriftRow[]
  recentJobs: JobSummary[]
  warnings: Warning[]
}

// ---------------------------------------------------------------------------
// 下发（API.md §4.13 / §5.6）
// ---------------------------------------------------------------------------

/** 模板名字符串，或 { name, config }。 */
export type TemplateRef = TemplateName | { name: TemplateName; config: TemplateInput }

export interface ApplyBulkRequest {
  app: PackageName
  devices: 'all' | DeviceId[]
  template: TemplateRef
  /** 默认 false。 */
  restart?: boolean
}

export interface Assignment {
  device: DeviceId
  app: PackageName
  template: TemplateRef
}

export interface ApplyAssignmentsRequest {
  assignments: Assignment[]
  /** 默认 false。 */
  restart?: boolean
}

export type ApplyRequest = ApplyBulkRequest | ApplyAssignmentsRequest

export type JobKind = 'bulk' | 'assignments' | 'egress'

export type JobStatus = 'queued' | 'running' | 'succeeded' | 'partial' | 'failed'

export interface ApplyAcceptedResponse {
  jobId: JobId
  status: 'queued'
  kind: JobKind
  total: number
  queued: number
  createdAt: Iso8601
  /** 相对路径，如 /relay/api/v1/jobs/job_... */
  pollUrl: string
}

// ---------------------------------------------------------------------------
// Job 与逐设备结果（API.md §4.14 / §5.7）
// ---------------------------------------------------------------------------

export type JobResultStatus = 'ok' | 'skipped' | 'failed'

export type JobStepName =
  | 'revision'
  | 'template_get'
  | 'template_put'
  | 'bind'
  | 'restart'
  /** 出口代理：GET 现状 / PUT 下发 / 可选探测（契约 §6.12）。 */
  | 'egress_get'
  | 'egress_put'
  | 'egress_probe'

export type JobStepStatus = 'ok' | 'skipped' | 'failed'

export interface JobStep {
  step: JobStepName
  status: JobStepStatus
  httpStatus: number | null
  code: string | null
  durationMs: number
}

export interface RestartResult {
  status: JobStepStatus
  httpStatus: number | null
  code: string | null
  durationMs: number
}

export interface JobResult {
  deviceId: DeviceId
  deviceName: string
  app: PackageName
  status: JobResultStatus
  code: string | null
  httpStatus: number | null
  revision: Revision | null
  /** bind 尝试次数：1 或 2。 */
  attempts: number
  durationMs: number
  steps: JobStep[]
  restart: RestartResult | null
  warnings: Warning[]
  error: ApiErrorPayload | null
}

export interface JobSummary {
  id: JobId
  status: JobStatus
  kind: JobKind
  app: PackageName | null
  template: TemplateName | null
  restart: boolean
  total: number
  succeeded: number
  skipped: number
  failed: number
  createdAt: Iso8601
  startedAt: Iso8601 | null
  finishedAt: Iso8601 | null
  durationMs: number | null
}

/** GET /jobs/{id}：summary + 结果。 */
export interface Job extends JobSummary {
  requestId: RequestId
  idempotencyKey: string | null
  warnings: Warning[]
  results: JobResult[]
}

export interface JobsResponse {
  /** 规范字段（与 jobs 内容一致）。 */
  items: JobSummary[]
  /** 兼容旧客户端的别名。 */
  jobs: JobSummary[]
  total: number
  nextCursor: string | null
}

export interface JobsQuery {
  /** 逗号分隔由 client 拼接；类型上也接受数组。 */
  status?: JobStatus[] | string
  kind?: JobKind
  limit?: number
  cursor?: string
}

// ---------------------------------------------------------------------------
// 出口代理 egress（docs/plans/egress-contract.md §6）
// ---------------------------------------------------------------------------
//
// 命名口径（契约 §6.1）：查询参数 snake_case，请求体 / 响应体 camelCase。
// 凭据只有 `hasCredentials` 一个信号：本文件里没有任何 username / password 的读字段。

/** 代理 id：`^egp_[0-9a-f]{32}$`。 */
export type EgressProxyId = string

/** 代理协议；DB 列名是 kind，线上字段叫 type。 */
export type EgressProxyType = 'http' | 'socks5'

/** 绑定模式：proxy = 走代理；direct = 强制直连（应用级用 __DIRECT__ 表示）。 */
export type EgressMode = 'proxy' | 'direct'

/** 探测来源：device（设备端 probe） / manager（Manager 直连探测）。 */
export type EgressCheckSource = 'device' | 'manager'

/** 保留的「强制直连」代理名（不能用作代理定义名）。 */
export const EGRESS_DIRECT_PROXY = '__DIRECT__'

/** 一次出口探测结果（lastCheck / recentChecks 共用，契约 §6.2）。 */
export interface EgressCheck {
  source: EgressCheckSource
  /** manager 直连探测为 null。 */
  deviceId: DeviceId | null
  /** `""` = 整机（方案 A）。 */
  app: string
  ok: boolean
  exitIp: string | null
  latencyMs: number | null
  /**
   * 六值 reason 枚举（契约 §4.4）：connect / timeout / dns / tls / http_status / invalid_response。
   * http 与 socks5 探测现在都走真连，不再有 unsupported_proxy_type。
   */
  errorCode: string | null
  checkedAt: Iso8601
}

/** 代理池条目：所有 /egress-proxies 端点共用，永远不含凭据值。 */
export interface EgressProxy {
  id: EgressProxyId
  /** `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`，全局唯一，与设备端表名 1:1。 */
  name: string
  type: EgressProxyType
  /** ≤253，不含 scheme / 端口 / 路径。 */
  host: string
  port: number
  /** 唯一的凭据信号：已存密文时为 true。 */
  hasCredentials: boolean
  region: string | null
  tags: string[]
  /** false 时不允许新建绑定，已有绑定仍会下发（附 egress_proxy_disabled 警告）。 */
  enabled: boolean
  /** 引用它的去重设备数。 */
  boundDevices: number
  /** 引用行数（含 app="" 的整机行）。 */
  boundApps: number
  lastCheck: EgressCheck | null
  /**
   * 该定义在**绑定设备**上是否正**经设备内中继**生效（契约 §6.1）。
   * 任一绑定设备的中继 listener 在 accept → true；只分配了 localPort 但没在监听 → false；
   * 没有绑定 / 设备离线 / 旧设备没上报 relay → null（不猜）。
   */
  relayActive?: boolean | null
  createdAt: Iso8601
  updatedAt: Iso8601
}

/** GET /egress-proxies：按 name（ASCII）升序。 */
export interface EgressProxyListResponse {
  total: number
  nextCursor: string | null
  items: EgressProxy[]
}

/** GET /egress-proxies/{id} 的绑定行（app="" 表示整机）。 */
export interface EgressProxyBinding {
  deviceId: DeviceId
  deviceName: string
  app: string
  updatedAt: Iso8601
}

/** GET /egress-proxies/{id}：EgressProxy + bindings + recentChecks。 */
export interface EgressProxyDetail extends EgressProxy {
  bindings: EgressProxyBinding[]
  /** 最近 20 条，checked_at 降序。 */
  recentChecks: EgressCheck[]
}

/** GET /egress-proxies 查询参数（enabled / tag / q / limit / cursor）。 */
export interface EgressProxyQuery {
  /** name / host / region 子串，不区分大小写。 */
  q?: string
  enabled?: boolean
  /** 多个 tag 为 AND 关系。 */
  tag?: string[]
  limit?: number
  cursor?: string
}

/** POST /egress-proxies：username / password 只写不读。 */
export interface CreateEgressProxyRequest {
  name: string
  type: EgressProxyType
  host: string
  port: number
  /** ≤128，只写不读。 */
  username?: string
  /** ≤256，只写不读。 */
  password?: string
  region?: string
  tags?: string[]
  /** 默认 true。 */
  enabled?: boolean
}

/**
 * PATCH /egress-proxies/{id}：JSON Merge Patch + 凭据双态。
 * 字段**省略** = 不改；`null` = 清除（只有 region / tags / username / password 可置空）。
 */
export interface UpdateEgressProxyRequest {
  name?: string
  type?: EgressProxyType
  host?: string
  port?: number
  username?: string | null
  password?: string | null
  region?: string | null
  tags?: string[] | null
  enabled?: boolean
}

/** PATCH 响应 = 更新后的 EgressProxy（附可选 warnings，如 egress_rename_pending）。 */
export interface UpdateEgressProxyResponse extends EgressProxy {
  warnings?: Warning[]
}

/** POST /egress-proxies/{id}/probe 请求体（可省略）。 */
export interface EgressProbeRequest {
  /** 覆盖探测 URL；默认 http://checkip.amazonaws.com/，限 http://。 */
  url?: string
}

/** 探测成功响应；失败走 502 egress_probe_failed（不用 200 包裹）。 */
export interface EgressProbeResponse {
  proxyId: EgressProxyId
  ok: true
  exitIp: string
  latencyMs: number
  checkedAt: Iso8601
  url: string
  upstreamStatus: number | null
  /** 非致命提醒（如 egress_check_not_recorded：探测成功但结果没落库）。 */
  warnings?: Warning[]
}

/** 设备绑定行（GET / PUT /devices/{id}/egress 共用）。 */
export interface DeviceEgressBinding {
  /** `""` = 整机（方案 A），其余 = 应用级。 */
  app: string
  mode: EgressMode
  proxyId: EgressProxyId | null
  proxyName: string | null
  updatedAt: Iso8601
}

/**
 * 设备端 `relay` 对象的透传形状（契约 §6.1 字段表，GET /egress 原样转发）。
 * Manager 的摘要口径见 DeviceRelaySummary：listeners 只数 listening=true。
 */
export interface DeviceRelayState {
  /** `[relay].enabled` 的实际生效值（设备端降级后的结果）。 */
  enabled?: boolean
  /** 实际生效的 `base_port`：设备级端口等于它，应用级在 17800–17899 内哈希取端口。 */
  basePort?: number
  /** `[probe].fail_open` 的实际生效值（上游探测失败时是否改走直连）。 */
  failOpen?: boolean
  /** 本快照时刻（RFC 3339 UTC）。 */
  at?: Iso8601
  /** 并发上限。 */
  limits?: {
    maxConnections?: number
    maxConnectionsPerUid?: number
  }
  /** 每个「应该监听」的中继端口一条（设备级在前，其余按端口升序）。 */
  listeners?: DeviceRelayListener[]
  /** uid 防火墙状态（仅应用级监听需要）。 */
  firewall?: DeviceRelayFirewall | null
  /** 端口变化（仅写响应可能非空）。 */
  portChanges?: Array<{
    proxy?: string
    /** 变化前（null = 首次分配）。 */
    from?: number | null
    /** 变化后（null = 释放）。 */
    to?: number | null
    /** initial / collision / out_of_range / addr_in_use / role_change / credentials_change。 */
    reason?: string
  }>
  /** 需要重启才能拿到新注入值的应用（仅写响应可能非空；GET 恒 `[]`）。 */
  restartRequiredApps?: string[]
}

/** 设备端一个中继 listener 的透传形状。 */
export interface DeviceRelayListener {
  /** 代理定义名（定义表名）。 */
  proxy?: string
  /** 实际监听端口（等于 `localPort`）。 */
  port?: number
  /** `device` = 设备级端口；`app` = 应用级。 */
  scope?: 'device' | 'app' | string
  /** true = 正在 accept；false = 期望监听但失败（原因见 `lastError`）。 */
  listening?: boolean
  /** `device` 恒为 `all`；`app` 恒为 `restricted`。 */
  uidPolicy?: 'all' | 'restricted' | string
  /** `restricted` 时为解析出的 uid（升序，含 0）。 */
  allowedUids?: number[]
  /** 当前连接数（含握手中；进程重启清零）。 */
  activeConnections?: number
  /** 最近一次绑定 / 退出失败；code 属于 relay 警告码。 */
  lastError?: {
    code?: string
    /** 该码 `details.reason` 的值（如 listener_exited / persist_failed / uid_unresolved）。 */
    reason?: string
    at?: Iso8601
  } | null
}

/** 设备端 uid 防火墙状态（iptables 链 DF_EGRESS_RLY）。 */
export interface DeviceRelayFirewall {
  /** 期望规则是否已落地。 */
  applied?: boolean
  /** 三条链规则总条数。 */
  rules?: number
  allowRules?: number
  denyRules?: number
  /** 最近一次失败（如 `egress_relay_firewall_failed`）。 */
  lastError?: {
    code?: string
    reason?: string
    /** `apply` / `clear`。 */
    op?: string
    at?: Iso8601
  } | null
}

/** 设备端 `proxies[]` 的透传形状：`localPort` 只读，由设备端分配。 */
export interface DeviceEgressProxyState {
  name?: string
  /** 17800–17899；null / 缺省表示该定义在设备上没有分配中继端口。 */
  localPort?: number | null
  /** 本地 listener 是否在 accept（设备端事实）。 */
  relayActive?: boolean
}

/** 设备端 GET /egress 的透传子集（Relay 原样转发，允许出现新字段）。 */
export interface DeviceEgressDeviceState {
  fetchedAt?: Iso8601
  /** 设备端 egress revision（只来自响应体或 X-DF-Egress-Revision，见 client.ts）。 */
  revision?: Revision
  /** 设备端 config revision（与 egress revision 是两套，永不互相覆盖）。 */
  configRevision?: Revision
  configured?: boolean
  mode?: string
  effectiveMode?: string
  default?: string | null
  applied?: boolean
  readback?: {
    settingsValue?: string | null
    dumpsysGlobalProxy?: string | null
    matches?: boolean | null
    at?: Iso8601
  } | null
  health?: {
    degraded?: boolean
    degradedSince?: Iso8601 | null
    degradedReason?: string | null
    lastProbe?: {
      ok?: boolean
      exitIp?: string | null
      latencyMs?: number | null
      checkedAt?: Iso8601
      scope?: string
      via?: string
      proxy?: string | null
      upstreamStatus?: number | null
      errorCode?: string | null
    } | null
  } | null
  apps?: Array<{
    package?: PackageName
    egress?: string
    effective?: string
    proxy?: string | null
  }>
  /**
   * 设备内本地认证中继的运行时状态（契约 §6.1）。
   * 旧设备没有这个字段 —— 界面上要显示「设备未上报中继状态」，不要猜。
   */
  relay?: DeviceRelayState | null
  /** 逐代理定义的中继端口（只有分配了 localPort 的定义才出现）。 */
  proxies?: DeviceEgressProxyState[]
}

/** GET /devices/{id}/egress：绑定表 + 设备实时状态（离线时 deviceState=null）。 */
export interface DeviceEgressResponse {
  deviceId: DeviceId
  bindings: DeviceEgressBinding[]
  deviceState: DeviceEgressDeviceState | null
  warnings: Warning[]
}

/** PUT /devices/{id}/egress 的单行输入（proxyId=null 表示该行不保留）。 */
export interface DeviceEgressBindingInput {
  app: string
  proxyId: EgressProxyId | '__DIRECT__' | null
}

/** PUT /devices/{id}/egress：全量替换该设备的绑定表，只落库不下发。 */
export interface DeviceEgressPutRequest {
  bindings: DeviceEgressBindingInput[]
}

export interface DeviceEgressPutResponse {
  deviceId: DeviceId
  changed: boolean
  bindings: DeviceEgressBinding[]
  /** 恒为 true：真正的下发必须走 POST /devices/egress/apply。 */
  pendingApply: boolean
  warnings: Warning[]
}

/** POST /devices/egress/apply 形态 A：下发现有绑定。 */
export interface EgressApplyBulkRequest {
  devices: 'all' | DeviceId[]
  probe?: boolean
  dryRun?: boolean
}

/** POST /devices/egress/apply 形态 B：先落库再下发（批量绑定）。 */
export interface EgressAssignment {
  deviceId: DeviceId
  app: string
  proxyId: EgressProxyId | '__DIRECT__' | null
}

export interface EgressApplyAssignmentsRequest {
  assignments: EgressAssignment[]
  probe?: boolean
  dryRun?: boolean
}

/** 两种形态二选一（同时出现 → 422）。 */
export type EgressApplyRequest = EgressApplyBulkRequest | EgressApplyAssignmentsRequest

/** 202 响应（kind 恒为 egress）。 */
export interface EgressApplyAcceptedResponse {
  jobId: JobId
  status: 'queued'
  kind: 'egress'
  total: number
  createdAt: Iso8601
  requestId: RequestId
}

/** dryRun 时逐目标的计划行。 */
export interface EgressApplyPlanItem {
  deviceId: DeviceId
  app: string
  mode: EgressMode | string
  proxyName: string | null
}

/** dryRun=true 的 200 响应（不建 job、不落库）。 */
export interface EgressApplyDryRunResponse {
  dryRun: true
  total: number
  plan: EgressApplyPlanItem[]
  warnings: Warning[]
}

/** apply 的两种响应：正常 202 / dryRun 200。 */
export type EgressApplyResponse = EgressApplyAcceptedResponse | EgressApplyDryRunResponse

/**
 * 设备内本地认证中继（relay）的摘要，来源是设备 `GET /egress` 的 `relay` 对象。
 *
 * `listeners` 只数**正在 accept** 的 listener，因此 `active == enabled && listeners > 0`：
 * 绑定失败（listening=false）的端口不会让设备卡亮起「经设备中继」。
 */
export interface DeviceRelaySummary {
  /** `[relay].enabled` 的实际生效值（设备端降级后的结果）。 */
  enabled: boolean
  /** 是否真的在经设备内中继转发。 */
  active: boolean
  /** 正在监听的中继端口数量。 */
  listeners: number
  /** `relay.firewall.applied`：uid 防火墙规则是否已落地。 */
  firewallApplied: boolean
}

/** 设备级出口摘要（DeviceSummary.egress，契约 §6.12 收尾）。 */
export interface DeviceEgressSummary {
  /** proxy / direct；设备级没有任何绑定时为 null。 */
  mode: EgressMode | string | null
  proxyId: EgressProxyId | null
  proxyName: string | null
  /** 最近一次整机探测的出口 IP。 */
  exitIp: string | null
  checkedAt: Iso8601 | null
  ok: boolean | null
  errorCode: string | null
  /**
   * 设备内中继状态（字段只增不改）。
   * null / 缺省 = 设备没上报（离线、旧版本没有 relay 字段、或设备不可达）。
   */
  relay?: DeviceRelaySummary | null
}

// ---------------------------------------------------------------------------
// 审计（API.md §4.16 / §5.8）
// ---------------------------------------------------------------------------

export type AuditAction =
  | 'auth.failed'
  | 'device.add'
  | 'device.update'
  | 'device.remove'
  | 'device.sync'
  | 'device.sync_all'
  | 'apply.bulk'
  | 'apply.assignments'
  | 'job.finished'
  | 'token.rotate'
  /** 出口代理：创建 / 更新 / 删除 / 探测 / 绑定 / 批量下发（契约 §6.1）。 */
  | 'egress.proxy_create'
  | 'egress.proxy_update'
  | 'egress.proxy_delete'
  | 'egress.proxy_probe'
  | 'egress.bind'
  | 'egress.apply'

export type AuditStatus = 'ok' | 'failed'

export type AuditTargetType = 'device' | 'job' | 'template' | 'token' | 'egress_proxy'

export interface AuditTarget {
  type: AuditTargetType
  id: string | null
}

export interface AuditEntry {
  id: number
  ts: Iso8601
  actor: string | null
  action: AuditAction
  target: AuditTarget
  requestId: RequestId | null
  status: AuditStatus
  detail: JsonObject | null
}

export interface AuditResponse {
  entries: AuditEntry[]
  total: number
  nextCursor: string | null
}

export interface AuditQuery {
  action?: AuditAction[]
  /** `<type>:<id>` 或纯 id。 */
  target?: string
  since?: Iso8601
  limit?: number
  cursor?: string
}

// ---------------------------------------------------------------------------
// Token 轮换（API.md §4.17）
// ---------------------------------------------------------------------------

export interface RotateTokenRequest {
  /** 16-128 字符；省略则生成 dfm_ 前缀随机串。 */
  token?: string
}

export interface RotateTokenResponse {
  token: string
  rotatedAt: Iso8601
  tokenPersisted: boolean
  source: 'rotated'
}

// ---------------------------------------------------------------------------
// 客户端接口草案（frontend-dev 在 manager/webui/src/api/client.ts 实现）
// ---------------------------------------------------------------------------

/** 最小 fetch 形状，避免类型文件依赖 DOM lib。 */
export type FetchLike = (
  input: string,
  init?: { method?: string; headers?: Record<string, string>; body?: string }
) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>

export interface ManagerClientOptions {
  /** 默认 '/relay/api/v1'（同源）；开发或独立部署时可设绝对地址。 */
  baseUrl?: string
  token: string
  fetchImpl?: FetchLike
}

/**
 * 前端 API client 的方法签名（与 API.md §6 的映射表一致）。
 * 所有方法在非 2xx 时抛出 ApiErrorShape；401 抛出 AuthErrorShape。
 */
export interface ManagerApi {
  healthz(): Promise<HealthResponse>
  listDevices(query?: DeviceQuery): Promise<DevicesResponse>
  addDevice(body: AddDeviceRequest): Promise<DeviceSummary>
  getDevice(id: DeviceId): Promise<DeviceDetail>
  updateDevice(id: DeviceId, body: UpdateDeviceRequest): Promise<DeviceSummary>
  removeDevice(id: DeviceId): Promise<void>
  syncDevice(id: DeviceId): Promise<SyncDeviceResponse>
  syncAllDevices(): Promise<SyncAllResponse>
  getDeviceOverview(id: DeviceId, options?: { cached?: boolean }): Promise<DeviceOverviewResponse>
  listTemplates(query?: {
    package?: PackageName
    device?: DeviceId
    limit?: number
    cursor?: string
  }): Promise<TemplatesResponse>
  getOverview(): Promise<ManagerOverviewResponse>
  getAppsMatrix(query?: AppsQuery): Promise<AppsMatrixResponse>
  apply(body: ApplyRequest, idempotencyKey?: string): Promise<ApplyAcceptedResponse>
  getJob(id: JobId): Promise<Job>
  listJobs(query?: JobsQuery): Promise<JobsResponse>
  listAudit(query?: AuditQuery): Promise<AuditResponse>
  rotateToken(body?: RotateTokenRequest): Promise<RotateTokenResponse>
  // ---- 出口代理 egress（契约 §6.2 – §6.10）----
  listEgressProxies(query?: EgressProxyQuery): Promise<EgressProxyListResponse>
  createEgressProxy(body: CreateEgressProxyRequest): Promise<EgressProxy>
  getEgressProxy(id: EgressProxyId): Promise<EgressProxyDetail>
  updateEgressProxy(
    id: EgressProxyId,
    body: UpdateEgressProxyRequest
  ): Promise<UpdateEgressProxyResponse>
  deleteEgressProxy(id: EgressProxyId): Promise<void>
  probeEgressProxy(id: EgressProxyId, body?: EgressProbeRequest): Promise<EgressProbeResponse>
  getDeviceEgress(id: DeviceId): Promise<DeviceEgressResponse>
  putDeviceEgress(
    id: DeviceId,
    body: DeviceEgressPutRequest
  ): Promise<DeviceEgressPutResponse>
  applyEgress(
    body: EgressApplyRequest,
    idempotencyKey?: string
  ): Promise<EgressApplyResponse>
}
