// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

import type { InstalledApp } from '../types'

/**
 * WebUI 传输层（backend）的公共接口。
 *
 * 同一份界面代码要在两种环境下运行：
 * - 管理器模式（现有）：实现落在 `backend/ksu.ts`，经 `window.ksu` 直读文件 / 跑命令；
 * - 原生服务模式（下一增量）：实现落在 `backend/http.ts`，经 `/api/v1` 读写。
 *
 * stores 与组件只认本文件的接口与 `backend/index.ts` 导出的实例，不直接依赖
 * `window.ksu` 或别的 manager 专有 API。
 */

/** 配置文件的固定位置（管理器模式直写；原生模式由服务端托管）。 */
export const CONFIG_PATH = '/data/adb/device_faker/config/config.toml'

/** 配置目录（首次启动时按需创建）。 */
export const CONFIG_DIR = '/data/adb/device_faker/config'

/** 原生服务设置（bind / allow_lan / tokens 哈希）；管理器模式可直读直写。 */
export const SERVER_CONFIG_PATH = '/data/adb/device_faker/config/server.toml'

/** 模块目录兜底（`ksu.moduleInfo().moduleDir` 不可用时使用）。 */
export const MODULE_DIR_FALLBACK = '/data/adb/modules/device_faker'

/** 包查询类型：user = 第三方应用，system = 系统应用，all = 两者。 */
export type PackageQueryType = 'user' | 'system' | 'all'

/**
 * 传输类型：管理器 WebView（ksu）/ 原生 HTTP 服务（http）/ 两者都不可用（readonly）。
 *
 * `readonly` 只会出现在「既没有 window.__DF_SERVER__ 也没有 window.ksu、且 /api/v1/healthz
 * 探测失败」的场合（例如把构建产物当本地文件打开）：读写一律失败，UI 显示只读提示。
 */
export type BackendRuntime = 'ksu' | 'http' | 'readonly'

export interface GetInstalledAppsOptions {
  includeSystem?: boolean
  packageType?: PackageQueryType
}

export interface GetAppsInfoOptions {
  fallbackType?: PackageQueryType
  assumeInstalled?: boolean
}

/**
 * 模块元数据。
 *
 * 管理器模式由宿主的 `ksu.moduleInfo()` 直供（KernelSU `WebViewInterface.kt::moduleInfo`），
 * 原生服务模式由 `/api/v1/status` 提供。`moduleDir` 是当前模块的**真实目录**
 * （`/data/adb/modules/<目录名>`，目录名与 module.prop 里的 `id` 不保证相同），
 * 其余是 manager 已解析好的 module.prop 元数据。
 */
export interface ModuleInfo {
  moduleDir?: string
  id?: string
  name?: string
  version?: string
  versionCode?: number
  author?: string
  description?: string
  enabled?: boolean
}

/** 深链（`am start`）启动选项。 */
export interface DeepLinkOptions {
  /**
   * 是否附带 `-a android.intent.action.VIEW -c android.intent.category.BROWSABLE`
   * （默认 true）。部分自定义 scheme 只按 `am start -d <uri>` 触发，需显式关闭。
   */
  useViewAction?: boolean
}

/** 在线模板抓取选项。 */
export interface FetchOnlineTemplatesOptions {
  /** 请求的 Accept 头，默认 application/json。 */
  accept?: string
  /** 中止信号（浏览器 fetch 实现会转发给 AbortController）。 */
  signal?: AbortSignal
}

/** 写配置的附加参数（契约 §2.6）。 */
export interface WriteConfigOptions {
  /** 允许「同一包名出现在多个模板」，默认 false（服务端 409）。 */
  allowDuplicatePackage?: boolean
  /** 要求服务端在响应里附 unified diff。 */
  diff?: boolean
}

/** 写配置返回的警告（契约 §6.6）。 */
export interface ConfigWriteWarning {
  code: string
  message: string
  details?: unknown
}

/** 写配置结果（契约 §6.5 的子集，WebUI 只用到这些字段）。 */
export interface WriteConfigResult {
  /** 写入后的 revision（sha256: 前缀可有可无，客户端当作不透明串）。 */
  revision: string
  /** 写入前的 revision；首次创建为 null。 */
  previousRevision?: string | null
  warnings: ConfigWriteWarning[]
  diff?: string
  dryRun?: boolean
  backupId?: string | null
}

/**
 * 配置 revision 信息（契约 §5.5）。
 *
 * 配置文件不存在时：`exists:false`、`mtime:null`、`size:0`，revision 为空字节的 SHA-256。
 */
export interface ConfigRevisionInfo {
  revision: string
  exists: boolean
  mtime?: string | null
  size?: number
}

/** 新生成的访问 Token（明文只返回一次，落盘只写 hash）。 */
export interface ServerTokenPair {
  /** 明文 Token（`df_` + 43 字符 base64url）。 */
  token: string
  /** 落盘用的哈希，形如 `sha256:<64 位小写十六进制>`。 */
  hash: string
}

/** 服务端注入的运行时信息（window.__DF_SERVER__，契约 §10.1）。 */
export interface ServerRuntimeInfo {
  /** API 前缀，例如 /api/v1。 */
  api: string
  /** 注入的版本号（当前为 1）。 */
  version: string
  /** 传输标识（当前为 http）。 */
  transport: string
}

/** GET /api/v1/status 的响应（契约 §5.2）。 */
export interface ServerStatus {
  module?: { id?: string; version?: string; versionCode?: number }
  device?: {
    manufacturer?: string
    brand?: string
    model?: string
    device?: string
    android?: string
    sdk?: number
    abi?: string
  }
  server?: { version?: string; bind?: string; uptimeSec?: number; startedAt?: string }
  config?: {
    revision?: string
    /** 配置不存在时为 null（t11 契约补充）。 */
    mtime?: string | null
    size?: number
    exists?: boolean
    templates?: number
    apps?: number
    conflicts?: number
  }
  capabilities?: string[]
}

/**
 * 传输层接口：UI 需要的全部设备能力。
 *
 * 约定（两种实现必须一致）：
 * - 读文件返回**去掉首尾空白**的文本（与历史 `cat` + `trim` 行为一致），
 *   让 `parseToml` / `parseModulePropValue` 的既有判断继续成立；
 * - 失败一律抛 `Error`，由调用方按原有语义降级，实现里不要吞异常；
 * - 写配置要么整体生效要么抛错（临时文件 + rename 语义），不能留下半截文件。
 */
export interface Backend {
  /** 当前生效的传输类型。 */
  readonly runtime: BackendRuntime

  /** 读 config.toml 原文（原生模式为 GET /config?format=toml）。 */
  readConfigRaw(): Promise<string>
  /** 整体替换 config.toml；原生模式必须带 If-Match（内部自动补）。 */
  writeConfigRaw(content: string, options?: WriteConfigOptions): Promise<WriteConfigResult>
  /** 配置 revision 与存在性（原生模式 GET /config/revision；管理器模式 revision 恒为空）。 */
  getConfigRevision(): Promise<ConfigRevisionInfo>

  /** 读任意可访问文件（module.prop 等）。 */
  readFile(path: string): Promise<string>
  /** 写任意可访问文件。 */
  writeFile(path: string, content: string): Promise<void>
  /** 文件是否存在（只认普通文件，目录返回 false）。 */
  fileExists(path: string): Promise<boolean>
  /** 递归创建目录。 */
  mkdir(path: string): Promise<void>

  /** 枚举已安装包名；分身实例带 `@userId` 后缀。 */
  listPackages(packageType?: PackageQueryType): Promise<string[]>
  /** 批量查询包元数据（解析失败的条目会被丢弃）。 */
  getPackagesInfo(packageNames: string[], options?: GetAppsInfoOptions): Promise<InstalledApp[]>
  /** 枚举已安装应用（含旧版 manager 的回退链）。 */
  getInstalledApps(options?: GetInstalledAppsOptions): Promise<InstalledApp[]>

  /** 模块元数据；宿主不支持时返回 null，调用方回退读 module.prop。 */
  getModuleInfo(): Promise<ModuleInfo | null>
  /** 读当前设备属性全集（`getprop` 输出）。 */
  getProps(): Promise<string>

  /** 原生模式的服务状态（GET /status）；管理器模式返回 null。 */
  getServerStatus(): Promise<ServerStatus | null>
  /** 注入的服务运行时信息（window.__DF_SERVER__）；管理器模式为 null。 */
  readonly serverRuntime: ServerRuntimeInfo | null

  // ---- 原生服务开关（仅管理器模式；原生/只读模式抛 UnsupportedError）----
  /** 读 server.toml；缺失时回退模块目录的 server.default.toml，都没有返回 null。 */
  readServerConfig(): Promise<string | null>
  /** 写 server.toml（内部收紧到 0600）。 */
  writeServerConfig(text: string): Promise<void>
  /** 重启原生服务（`service.sh restart`），返回脚本 stdout。 */
  restartServer(): Promise<string>
  /** 原生服务状态（`service.sh status`）的原始输出。 */
  getServerControlStatus(): Promise<string>
  /** 生成一个只返回、不落盘的访问 Token 与其 sha256。 */
  generateServerToken(): Promise<ServerTokenPair>
  /** 可用于局域网访问的本机 IPv4 列表。 */
  getLanAddresses(): Promise<string[]>

  /** 用系统意图打开链接 / scheme。 */
  openDeepLink(url: string, options?: DeepLinkOptions): Promise<void>
  /** 抓取在线模板原始文本（管理器走 shell，原生模式走服务端代理）。 */
  fetchOnlineTemplates(url: string, options?: FetchOnlineTemplatesOptions): Promise<string>
}
