// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

import type { InstalledApp } from '../types'
import type { EgressProbeRequest, EgressUpdateInput } from './egress'
import { UnsupportedError } from './errors'
import { createHttpBackend, probeHealthz, readInjectedServer } from './http'
import { createKsuBackend } from './ksu'
import type {
  Backend,
  BackendRuntime,
  ConfigRevisionInfo,
  DeepLinkOptions,
  FetchOnlineTemplatesOptions,
  GetAppsInfoOptions,
  GetInstalledAppsOptions,
  ModuleInfo,
  PackageQueryType,
  ServerStatus,
  ServerTokenPair,
  WriteConfigOptions,
} from './types'

/**
 * 传输层入口：按运行时选择 ksu / http / readonly（契约 §10.2）。
 *
 * 1. `window.__DF_SERVER__` 存在 → 原生服务模式（不再探测 ksu）；
 * 2. 否则 `window.ksu` 存在 → 管理器模式（行为与以前完全一致）；
 * 3. 都没有 → 探测 `/api/v1/healthz`：可达走原生模式，不可达降级只读。
 *
 * 探测在第三种情况下必须异步，但 UI 需要同步拿到 `runtime` 决定降级方式，
 * 因此这里导出一个「懒解析」代理：方法首次调用时完成解析，`runtime` 在
 * 解析前给出探测初值（http），解析后更新为真实值。
 */

/** 同步可判定的传输类型（不做 healthz 探测）。 */
export function detectRuntimeSync(): BackendRuntime {
  if (readInjectedServer()) {
    return 'http'
  }

  if (typeof (globalThis as { ksu?: unknown }).ksu !== 'undefined') {
    return 'ksu'
  }

  // 开发模式（vite dev）没有管理器也没有服务端：继续用 ksu 传输的 mock 数据
  if (import.meta.env?.DEV) {
    return 'ksu'
  }

  // 两者都没有：可能是没注入脚本的反代部署，交给异步 healthz 探测确认
  return 'http'
}

function readOnlyError(action: string): UnsupportedError {
  return new UnsupportedError(`当前环境没有可用的设备接口，无法${action}`)
}

/** 只读降级实现：既没有管理器也没有原生服务时使用（契约 §10.2 第 3 条）。 */
export function createReadOnlyBackend(): Backend {
  return {
    runtime: 'readonly',
    serverRuntime: null,
    readConfigRaw: async () => {
      throw readOnlyError('读取配置')
    },
    writeConfigRaw: async (_content: string, _options?: WriteConfigOptions) => {
      throw readOnlyError('写入配置')
    },
    getConfigRevision: async (): Promise<ConfigRevisionInfo> => {
      throw readOnlyError('读取 revision')
    },
    readFile: async () => {
      throw readOnlyError('读取文件')
    },
    writeFile: async () => {
      throw readOnlyError('写入文件')
    },
    fileExists: async () => false,
    mkdir: async () => undefined,
    listPackages: async (_packageType?: PackageQueryType): Promise<string[]> => {
      throw readOnlyError('读取应用列表')
    },
    getPackagesInfo: async (
      _packageNames: string[],
      _options?: GetAppsInfoOptions
    ): Promise<InstalledApp[]> => {
      throw readOnlyError('读取应用信息')
    },
    getInstalledApps: async (_options?: GetInstalledAppsOptions): Promise<InstalledApp[]> => {
      throw readOnlyError('读取应用列表')
    },
    getModuleInfo: async (): Promise<ModuleInfo | null> => null,
    getProps: async () => {
      throw readOnlyError('读取设备属性')
    },
    getServerStatus: async (): Promise<ServerStatus | null> => null,
    readServerConfig: async () => {
      throw readOnlyError('读取服务设置')
    },
    writeServerConfig: async () => {
      throw readOnlyError('修改服务设置')
    },
    restartServer: async () => {
      throw readOnlyError('重启服务')
    },
    getServerControlStatus: async () => {
      throw readOnlyError('读取服务状态')
    },
    generateServerToken: async (): Promise<ServerTokenPair> => {
      throw readOnlyError('生成访问 Token')
    },
    hashServerToken: async (_token: string): Promise<ServerTokenPair> => {
      throw readOnlyError('修改访问 Token')
    },
    getLanAddresses: async (): Promise<string[]> => {
      throw readOnlyError('读取本机地址')
    },
    openDeepLink: async (_url: string, _options?: DeepLinkOptions) => {
      throw new UnsupportedError('当前环境不支持深链启动')
    },
    fetchOnlineTemplates: async (_url: string, _options?: FetchOnlineTemplatesOptions) => {
      throw readOnlyError('拉取在线模板')
    },
    readEgress: async () => {
      throw readOnlyError('读取出口代理')
    },
    writeEgress: async () => {
      throw readOnlyError('保存出口代理')
    },
    clearEgress: async () => {
      throw readOnlyError('恢复直连')
    },
    probeEgress: async () => {
      throw readOnlyError('探测出口 IP')
    },
    restartEgressRelay: async () => {
      throw readOnlyError('重启中继')
    },
  }
}

/** 按已知传输类型创建实现（不会做 healthz 探测）。 */
export function createBackend(runtime: BackendRuntime = detectRuntimeSync()): Backend {
  switch (runtime) {
    case 'ksu':
      return createKsuBackend()
    case 'http':
      return createHttpBackend()
    case 'readonly':
      return createReadOnlyBackend()
  }
}

/** 懒解析代理：首次调用时完成注入/探测，并把结果缓存下来。 */
function createLazyBackend(): Backend {
  const initialRuntime = detectRuntimeSync()
  let resolved: Backend | null = initialRuntime === 'ksu' ? createKsuBackend() : null
  let resolving: Promise<Backend> | null = null

  async function resolve(): Promise<Backend> {
    if (resolved) {
      return resolved
    }

    if (!resolving) {
      resolving = (async () => {
        const injected = readInjectedServer()
        if (injected) {
          return createHttpBackend({ server: injected })
        }
        if (await probeHealthz()) {
          return createHttpBackend()
        }
        return createReadOnlyBackend()
      })().then((instance) => {
        resolved = instance
        return instance
      })
    }

    return await resolving
  }

  return {
    get runtime(): BackendRuntime {
      return resolved?.runtime ?? initialRuntime
    },
    get serverRuntime() {
      return resolved?.serverRuntime ?? readInjectedServer()
    },
    readConfigRaw: async () => (await resolve()).readConfigRaw(),
    writeConfigRaw: async (content: string, options?: WriteConfigOptions) =>
      (await resolve()).writeConfigRaw(content, options),
    getConfigRevision: async () => (await resolve()).getConfigRevision(),
    readFile: async (path: string) => (await resolve()).readFile(path),
    writeFile: async (path: string, content: string) => (await resolve()).writeFile(path, content),
    fileExists: async (path: string) => (await resolve()).fileExists(path),
    mkdir: async (path: string) => (await resolve()).mkdir(path),
    listPackages: async (packageType?: PackageQueryType) =>
      (await resolve()).listPackages(packageType),
    getPackagesInfo: async (packageNames: string[], options?: GetAppsInfoOptions) =>
      (await resolve()).getPackagesInfo(packageNames, options),
    getInstalledApps: async (options?: GetInstalledAppsOptions) =>
      (await resolve()).getInstalledApps(options),
    getModuleInfo: async () => (await resolve()).getModuleInfo(),
    getProps: async () => (await resolve()).getProps(),
    getServerStatus: async () => (await resolve()).getServerStatus(),
    readServerConfig: async () => (await resolve()).readServerConfig(),
    writeServerConfig: async (text: string) => (await resolve()).writeServerConfig(text),
    restartServer: async () => (await resolve()).restartServer(),
    getServerControlStatus: async () => (await resolve()).getServerControlStatus(),
    generateServerToken: async () => (await resolve()).generateServerToken(),
    hashServerToken: async (token: string) => (await resolve()).hashServerToken(token),
    getLanAddresses: async () => (await resolve()).getLanAddresses(),
    openDeepLink: async (url: string, options?: DeepLinkOptions) =>
      (await resolve()).openDeepLink(url, options),
    fetchOnlineTemplates: async (url: string, options?: FetchOnlineTemplatesOptions) =>
      (await resolve()).fetchOnlineTemplates(url, options),
    readEgress: async () => (await resolve()).readEgress(),
    writeEgress: async (input: EgressUpdateInput) => (await resolve()).writeEgress(input),
    clearEgress: async () => (await resolve()).clearEgress(),
    probeEgress: async (request?: EgressProbeRequest) => (await resolve()).probeEgress(request),
    restartEgressRelay: async () => (await resolve()).restartEgressRelay(),
  }
}

/** 全局唯一的传输实例：stores 与组件统一从这里取设备能力。 */
export const backend: Backend = createLazyBackend()

/** 强制完成解析（供「设置 → 服务」显示真实传输类型）。 */
export async function resolveBackend(): Promise<Backend> {
  // 触发一次无副作用的解析：只读实现也会被正确解析出来
  try {
    await backend.getConfigRevision()
  } catch {
    // 读取失败不影响传输解析（readonly 实现就是在这里抛错的）
  }
  return backend
}

export {
  ApiError,
  AuthError,
  UnsupportedError,
  isApiError,
  isAuthError,
  isUnsupportedError,
  listValidationErrors,
  readConflictDetails,
  readScopeDetails,
} from './errors'
export {
  clearToken,
  getToken,
  hasToken,
  isTokenRemembered,
  onAuthRequired,
  requestToken,
  setAuthPromptHandler,
  setToken,
} from './auth'
export * from './types'
export * from './egress'
