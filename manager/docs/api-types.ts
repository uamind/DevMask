// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask
//
// Manager Relay API 类型草案（API v1 / manager 0.2.0；含 egress 增量）。
// 契约来源：manager/docs/API.md；字段命名规则见该文档 §2.2。
// 用法：复制为 manager/webui/src/api/types.ts（frontend-dev 作用域），不要改动字段名。
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
  | 'egress_proxy_exists'
  | 'egress_proxy_in_use'
  | 'egress_probe_failed'
  | 'internal_error'
  | 'rate_limited'

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
  | 'invalid_egress_name'
  | 'invalid_egress_proxy'
  | 'invalid_probe_url'
  /** v0.0.1 起 socks5 探测真连，本码不再返回（保留兼容）。 */
  | 'unsupported_proxy_type'

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

export interface DeviceEgressSummary {
  /** proxy / direct；设备级没有任何绑定时为 null。 */
  mode: 'proxy' | 'direct' | null
  proxyId: EgressProxyId | null
  proxyName: string | null
  exitIp: string | null
  checkedAt: Iso8601 | null
  ok: boolean | null
  errorCode: string | null
  /** v0.0.1：设备内认证中继状态；离线 / 旧设备 / 未绑定时为 null。 */
  relay?: DeviceRelaySummary | null
}

/** 设备内认证中继（relay）投影；来源设备 GET /egress 的 relay 对象。 */
export interface DeviceRelaySummary {
  /** [relay].enabled 的实际生效值。 */
  enabled: boolean
  /** 是否真的在经中继转发（enabled 且至少一个 listener 在 listening）。 */
  active: boolean
  /** 正在监听的中继端口数。 */
  listeners: number
  /** relay.firewall.applied：uid 防火墙规则是否已落地。 */
  firewallApplied: boolean
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
  /** 设备级出口摘要（旧版本设备为 null / 省略）。 */
  egress?: DeviceEgressSummary | null
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
// 出口代理（egress）：代理池 / 绑定 / 批量下发（API.md §4.19-§4.27、§5.11-§5.14）
//
// 硬约束：本块所有类型都**没有** username / password 字段；
// 凭据的唯一信号是 hasCredentials（服务端 AES-256-GCM 落库，永不回显）。
// ---------------------------------------------------------------------------

/** 代理 id：^egp_[0-9a-f]{32}$ */
export type EgressProxyId = string

/** 代理协议（DB 列名是 kind，线上字段叫 type）。 */
export type EgressProxyType = 'http' | 'socks5'

/** 绑定模式：proxy = 走代理，direct = 强制直连（__DIRECT__）。 */
export type EgressBindingMode = 'proxy' | 'direct'

/** 一次探测结果（lastCheck / recentChecks）。 */
export interface EgressCheck {
  source: 'device' | 'manager'
  deviceId: DeviceId | null
  app: PackageName | ''
  ok: boolean
  exitIp: string | null
  latencyMs: number | null
  errorCode: string | null
  checkedAt: Iso8601
}

/** 代理池条目（所有代理端点共用；无凭据字段）。 */
export interface EgressProxy {
  id: EgressProxyId
  /** ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$；下发时作为设备端 egress.toml 表名。 */
  name: string
  type: EgressProxyType
  host: string
  port: number
  hasCredentials: boolean
  region: string | null
  tags: string[]
  enabled: boolean
  boundDevices: number
  boundApps: number
  lastCheck: EgressCheck | null
  createdAt: Iso8601
  updatedAt: Iso8601
}

export interface EgressProxyListResponse {
  total: number
  nextCursor: string | null
  items: EgressProxy[]
}

export interface EgressProxyBinding {
  deviceId: DeviceId
  deviceName: string
  /** '' = 整机。 */
  app: PackageName | ''
  updatedAt: Iso8601
}

/** GET /egress-proxies/{id}：EgressProxy + 引用关系 + 最近探测。 */
export interface EgressProxyDetailResponse extends EgressProxy {
  bindings: EgressProxyBinding[]
  recentChecks: EgressCheck[]
}

/** POST /egress-proxies（用户名 / 密码只写不读）。 */
export interface CreateEgressProxyRequest {
  name: string
  type: EgressProxyType
  host: string
  port: number
  username?: string
  password?: string
  region?: string
  tags?: string[]
  enabled?: boolean
}

/** PATCH /egress-proxies/{id}：省略 = 不改；null = 清除（仅可空字段）。 */
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

export interface EgressProbeRequest {
  url?: string
}

export interface EgressProbeResponse {
  proxyId: EgressProxyId
  ok: true
  exitIp: string
  latencyMs: number
  checkedAt: Iso8601
  url: string
  upstreamStatus: number | null
}

/** GET /devices/{id}/egress 的绑定行。 */
export interface DeviceEgressBinding {
  app: PackageName | ''
  mode: EgressBindingMode
  proxyId: EgressProxyId | null
  proxyName: string | null
  updatedAt: Iso8601
}

export interface DeviceEgressResponse {
  deviceId: DeviceId
  bindings: DeviceEgressBinding[]
  /** 设备端 GET /api/v1/egress 的透传子集；离线 / 旧版本为 null。 */
  deviceState: JsonObject | null
  warnings: Warning[]
}

/** PUT /devices/{id}/egress 的单行：proxyId 允许 id / "__DIRECT__" / null（该行不保留）。 */
export interface DeviceEgressBindingInput {
  app: PackageName | ''
  proxyId: EgressProxyId | '__DIRECT__' | null
}

export interface DeviceEgressPutRequest {
  bindings: DeviceEgressBindingInput[]
}

export interface DeviceEgressPutResponse {
  deviceId: DeviceId
  changed: boolean
  bindings: DeviceEgressBinding[]
  /** 只落库、未下发时为 true。 */
  pendingApply: boolean
  warnings: Warning[]
}

export interface EgressAssignmentInput {
  deviceId: DeviceId
  app: PackageName | ''
  proxyId?: EgressProxyId | '__DIRECT__' | null
}

/** POST /devices/egress/apply：devices 与 assignments 二选一。 */
export interface EgressApplyRequest {
  devices?: 'all' | DeviceId[]
  assignments?: EgressAssignmentInput[]
  probe?: boolean
  dryRun?: boolean
}

export interface EgressApplyAcceptedResponse {
  jobId: JobId
  status: 'queued'
  kind: 'egress'
  total: number
  createdAt: Iso8601
  requestId: RequestId
}

export interface EgressApplyPlanItem {
  deviceId: DeviceId
  app: PackageName | ''
  mode: EgressBindingMode
  proxyName: string | null
}

export interface EgressApplyDryRunResponse {
  dryRun: true
  total: number
  plan: EgressApplyPlanItem[]
  warnings: Warning[]
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

  // egress（API.md §4.19-§4.27）
  listEgressProxies(query?: {
    q?: string
    enabled?: boolean
    tag?: string[]
    limit?: number
    cursor?: string
  }): Promise<EgressProxyListResponse>
  createEgressProxy(body: CreateEgressProxyRequest): Promise<EgressProxy>
  getEgressProxy(id: EgressProxyId): Promise<EgressProxyDetailResponse>
  updateEgressProxy(id: EgressProxyId, body: UpdateEgressProxyRequest): Promise<EgressProxy>
  deleteEgressProxy(id: EgressProxyId): Promise<void>
  probeEgressProxy(id: EgressProxyId, body?: EgressProbeRequest): Promise<EgressProbeResponse>
  getDeviceEgress(id: DeviceId): Promise<DeviceEgressResponse>
  putDeviceEgress(id: DeviceId, body: DeviceEgressPutRequest): Promise<DeviceEgressPutResponse>
  applyEgress(body: EgressApplyRequest, idempotencyKey?: string): Promise<EgressApplyAcceptedResponse | EgressApplyDryRunResponse>
}
