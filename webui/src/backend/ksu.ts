// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

import { exec, listPackages as listPackagesLegacy, getPackagesInfo, pm, io } from 'kernelsu-alt'
import { normalizePackageName, parsePackageUser } from '../utils/package'
import type { InstalledApp } from '../types'
import { parseDhcpProps, parseIpAddrOutput } from '../utils/serverConfig'
import {
  EGRESS_PATH,
  DEFAULT_PROBE_URL,
  DIRECT_MARKER,
  clearConfigEgress,
  expectedSettingsValue,
  parseEgressFile,
  readConfigEgressRefs,
  renderEgressFile,
  setConfigEgressDefault,
  type EgressAppView,
  type EgressFileInfo,
  type EgressFileModel,
  type EgressProbeRequest,
  type EgressProbeResult,
  type EgressProxyInput,
  type EgressProxySummary,
  type EgressSnapshot,
  type EgressUpdateInput,
  type EgressWarning,
  type EgressWriteResult,
} from './egress'
import { UnsupportedError } from './errors'
import {
  CONFIG_DIR,
  CONFIG_PATH,
  MODULE_DIR_FALLBACK,
  SERVER_CONFIG_PATH,
  type Backend,
  type DeepLinkOptions,
  type FetchOnlineTemplatesOptions,
  type GetAppsInfoOptions,
  type GetInstalledAppsOptions,
  type ModuleInfo,
  type PackageQueryType,
  type ServerTokenPair,
} from './types'

/**
 * 管理器模式（`window.ksu`）的传输实现。
 *
 * 本文件是原 `utils/ksu.ts` 的原样迁移（该文件已删除）：组件与 stores
 * 不再直接引用它，统一从 `backend/index.ts` 取 `Backend` 实例。除迁移外只补了接口
 * 要求的适配方法（`readConfigRaw` / `writeConfigRaw` / `getProps` / `openDeepLink` /
 * `fetchOnlineTemplates`），实现直接复用下面的既有函数。
 */

/**
 * 旧版兼容清单（TODO：确认最低支持 manager 版本后整体删除）
 *
 * 判断条件统一是 `kernelsu-alt` 在模块加载时求值出的 `pm.support` / `io.support`：
 *
 * 1. 文件 IO（`io.support`）：
 *    - `readFile` → `cat`
 *    - `writeFile` → `writeFileViaShell`（heredoc + `mv`）
 *    - `fileExists` → `test -f`
 *    - `mkdir` → `mkdir -p`
 * 2. 包枚举（`pm.support`）：`getUserIds` / `listPackagesForUser` /
 *    `getPackageListViaLegacyApi`（`pm list users` + 每用户一次 `pm list packages --user`）
 *    与 `fetchPackageInfos` 里旧 `getPackagesInfo` 分支。
 *
 * 全仓 grep `TODO(legacy-compat)` 可列出所有待删除分支。
 *
 * 与之相对，下面这些 exec 用法**不是**兼容分支、不要当成待删除项：
 * - 深链启动（`am start -d ...`，FollowDialog / TranslatorsDialog）
 * - 在线模板库的 curl/wget/busybox 兜底抓取（shell 无 CORS 限制）
 * - 「读当前设备配置」的 `getprop`（ZIP/属性→模板的转换已在
 *   `utils/propsToTemplate.ts` 里纯前端完成，不再调用 device_faker_cli）
 */

/**
 * KernelSU `getPackagesInfo` / `pm.getPackagesInfo` 返回的条目。
 *
 * - `pm`（KernelSU 3.3.1+）按 uid 返回：同一包名在不同用户下各一条，带 `userId`；
 * - 旧 `getPackagesInfo` 每个包名一条，没有 `userId`（可退化为 `uid / 100000`）；
 * - 解析失败时返回 `{ packageName, error }`。
 */
interface KernelSUPackageInfo {
  packageName: string
  versionName?: string
  versionCode?: number
  appLabel?: string
  isSystem?: boolean | null
  uid?: number | null
  userId?: number | null
  error?: string
}

const getPackagesInfoBatch = getPackagesInfo as unknown as (
  pkg: string | string[]
) => Promise<KernelSUPackageInfo | KernelSUPackageInfo[]>

const getPackagesInfoViaPm = pm.getPackagesInfo as unknown as (
  pkg: string[]
) => Promise<KernelSUPackageInfo[]>

/** 归一化后的条目：包名去掉 `@userId` 后缀，并补齐所属用户 ID。 */
type PackageInfoEntry = KernelSUPackageInfo & { base: string; userId: number }

/** 包信息索引：按「实例键 base@userId」与「包名 base」两套视图查表。 */
interface PackageInfoIndex {
  /** `base@userId` → 该用户下的条目（同键保留最小 uid） */
  perInstance: Map<string, PackageInfoEntry>
  /** `base` → 跨用户的最小 uid 条目，供分身元数据继承 */
  perBase: Map<string, PackageInfoEntry>
}

// 执行命令
export async function execCommand(command: string): Promise<string> {
  // 开发模式下的模拟数据
  if (import.meta.env?.DEV) {
    return new Promise((resolve) => {
      setTimeout(() => resolve(''), 100)
    })
  }

  // 使用 kernelsu-alt 的 exec
  const result = await exec(command)
  if (result.errno === 0) {
    return result.stdout || ''
  } else {
    throw new Error(result.stderr || `Command failed with error code ${result.errno}`)
  }
}

// 读取文件
export async function readFile(path: string): Promise<string> {
  // 开发模式返回模拟数据
  if (import.meta.env?.DEV) {
    const { mockConfig, mockModuleProp } = await import('../utils/mockData')
    if (path.includes('config.toml')) {
      return mockConfig
    }
    if (path.includes('module.prop')) {
      return mockModuleProp
    }
    return ''
  }

  // TODO(legacy-compat): io 接口不可用时回退 `cat`，兼容期结束后删除该分支
  const content = io.support ? await readFileViaIoWithFallback(path) : await readFileViaShell(path)
  return content.trim()
}

/** 旧版兼容路径：整读文件走 `cat`。 */
async function readFileViaShell(path: string): Promise<string> {
  return await execCommand(`cat ${shellQuotePath(path)}`)
}

/**
 * 经 io 读文件，**io 抛错时回落 `cat`**。
 *
 * `io.support` 只判断 `typeof ksu.io === "function"`，并不保证宿主真的实现了
 * `FileInputStream`（各 manager 版本差异很大：方法缺失、`open()` 直接抛错、
 * 只支持部分路径）。此前这里没有兜底，宿主 io 不可用时整个读取就失败——调用方
 * 只能静默降级（版本号一直停在 `0.0.0`、config.toml 回落到 localStorage 备份），
 * 看起来像「WebUI 只坏了一个字段」。
 */
async function readFileViaIoWithFallback(path: string): Promise<string> {
  try {
    return await readFileViaIo(path)
  } catch {
    return await readFileViaShell(path)
  }
}

/** 经 io 接口整读一个文件（base64 分块读出后按 UTF-8 解码）。 */
async function readFileViaIo(path: string): Promise<string> {
  const reader = io.FileInputStream()
  const id = reader.open(path)
  if (!id) {
    throw new Error(`Failed to open ${path}`)
  }

  try {
    // 每个 read() 返回的是「该块字节自己的 base64」，各自带 `=` 填充，
    // 因此必须**逐块解码再拼字节**，不能把 base64 串直接相连（会拼出非法 base64）。
    // 逐块 atob 得到的是字节级二进制字符串，跨块的多字节 UTF-8 字符不会被打断，
    // 最后一次性按 UTF-8 解码即可。
    let binary = ''
    for (;;) {
      const chunk = reader.read(id)
      if (!chunk) break
      binary += atob(chunk)
    }
    return decodeBinaryUtf8(binary)
  } finally {
    reader.close(id)
  }
}

function decodeBinaryUtf8(binary: string): string {
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i)
  }
  return new TextDecoder().decode(bytes)
}

function encodeBase64Utf8(text: string): string {
  const bytes = new TextEncoder().encode(text)
  let binary = ''
  const CHUNK = 0x8000
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK))
  }
  return btoa(binary)
}

// 写入文件
export async function writeFile(path: string, content: string): Promise<void> {
  // TODO(legacy-compat): io 接口不可用时回退 heredoc 脚本，兼容期结束后删除该分支
  if (!io.support) {
    await writeFileViaShell(path, content)
    return
  }

  try {
    await writeFileViaIo(path, content)
  } catch {
    // io 存在但不可用（宿主未实现 / 抛错）时回落 heredoc 写入
    await writeFileViaShell(path, content)
  }
}

/**
 * 经 io 接口写入文件：先写临时文件再 `renameTo`，语义对齐旧实现的
 * 「heredoc 到 tmp + `mv -f`」（不会出现半截文件）。
 */
async function writeFileViaIo(path: string, content: string): Promise<void> {
  const dir = path.slice(0, path.lastIndexOf('/'))
  if (dir) {
    io.File(dir).mkdirs()
  }

  const tempPath = `${path}.tmp.${Date.now()}`
  const writer = io.FileOutputStream()
  const id = writer.open(tempPath, false)
  if (!id) {
    throw new Error(`Failed to open ${tempPath} for writing`)
  }

  try {
    if (!writer.write(id, encodeBase64Utf8(content))) {
      throw new Error(`Failed to write ${tempPath}`)
    }
    writer.flush(id)
  } catch (err) {
    writer.close(id)
    io.File(tempPath).delete()
    throw err
  }

  writer.close(id)

  if (!io.File(tempPath).renameTo(path)) {
    io.File(tempPath).delete()
    throw new Error(`Failed to move ${tempPath} to ${path}`)
  }
}

function escapeShellPath(path: string): string {
  return path.replace(/'/g, "'\\''")
}

/** 单引号包裹 shell 参数（仅旧版兼容分支拼接命令时使用）。 */
function shellQuotePath(value: string): string {
  return `'${escapeShellPath(value)}'`
}

function hasHeredocDelimiter(content: string, delimiter: string): boolean {
  const normalized = content.replace(/\r\n/g, '\n')
  if (normalized.includes(`\n${delimiter}\n`)) return true
  if (normalized.startsWith(`${delimiter}\n`)) return true
  if (normalized.endsWith(`\n${delimiter}`)) return true
  return false
}

function pickHeredocDelimiter(content: string): string {
  const base = 'EOF_DEVICE_FAKER'
  let delimiter = base
  let attempt = 0
  while (hasHeredocDelimiter(content, delimiter)) {
    attempt += 1
    delimiter = `${base}_${attempt}_${Math.random().toString(36).slice(2, 8)}`
  }
  return delimiter
}

/** @deprecated 旧版兼容路径（TODO(legacy-compat)：io 接口普及时删除） */
async function writeFileViaShell(path: string, content: string): Promise<void> {
  const delimiter = pickHeredocDelimiter(content)
  const escapedPath = escapeShellPath(path)
  const tempPath = escapeShellPath(`${path}.tmp.${Date.now()}`)
  const script = [
    `cat << '${delimiter}' > '${tempPath}'`,
    content,
    delimiter,
    `sync '${tempPath}' || true`,
    `mv -f '${tempPath}' '${escapedPath}'`,
  ].join('\n')

  try {
    await execCommand(script)
  } catch (err) {
    await execCommand(`rm -f '${tempPath}'`).catch(() => {})
    throw err
  }
}

/**
 * @deprecated 旧版兼容路径（TODO(legacy-compat)：`pm` 接口普及时删除）。
 *
 * 回退路径（user 0）：使用 kernelsu-alt 的 legacy listPackages API 获取已安装应用列表。
 */
async function getPackageListViaLegacyApi(type: PackageQueryType): Promise<string[]> {
  try {
    if (type === 'all') {
      const [userPkgs, systemPkgs] = await Promise.all([
        listPackagesLegacy('user').catch(() => []),
        listPackagesLegacy('system').catch(() => []),
      ])
      return [...userPkgs, ...systemPkgs]
    }

    return await listPackagesLegacy(type)
  } catch {
    return []
  }
}

/**
 * 把接口原始条目归一化成实例条目；解析失败（带 error）的条目返回 null。
 */
function packageInfoEntryFromRaw(raw: KernelSUPackageInfo): PackageInfoEntry | null {
  if (!raw || typeof raw.packageName !== 'string' || raw.error) {
    return null
  }

  const base = normalizePackageName(raw.packageName)
  const userId =
    typeof raw.userId === 'number'
      ? raw.userId
      : typeof raw.uid === 'number'
        ? Math.floor(raw.uid / 100000)
        : 0

  return { ...raw, base, userId }
}

/**
 * 建立查表索引。
 *
 * `pm` 接口对同一包名会返回多条（每个 uid 一条，uid 升序），因此同键保留首条
 * （最小 uid），避免高分身的条目覆盖主用户条目。
 */
function buildPackageInfoIndex(entries: KernelSUPackageInfo[]): PackageInfoIndex {
  const perInstance = new Map<string, PackageInfoEntry>()
  const perBase = new Map<string, PackageInfoEntry>()

  for (const raw of entries) {
    const entry = packageInfoEntryFromRaw(raw)
    if (!entry) continue

    const instanceKey = `${entry.base}@${entry.userId}`
    if (!perInstance.has(instanceKey)) {
      perInstance.set(instanceKey, entry)
    }
    if (!perBase.has(entry.base)) {
      perBase.set(entry.base, entry)
    }
  }

  return { perInstance, perBase }
}

/**
 * 查询包信息。
 *
 * 优先走 `pm` 接口（每个 uid 一条、带 userId，可区分分身）；环境不支持时回退旧
 * `getPackagesInfo`（仅每个包名一条，分身信息只能按 base 继承）。
 * 注意两者都只认裸包名——带 `@userId` 后缀的实例键查不到任何条目。
 */
async function fetchPackageInfos(packageNames: string[]): Promise<KernelSUPackageInfo[]> {
  if (packageNames.length === 0) {
    return []
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  if (typeof (globalThis as any).ksu?.getPackagesInfo === 'undefined') {
    return []
  }

  try {
    const raw = pm.support
      ? await getPackagesInfoViaPm(packageNames)
      : // TODO(legacy-compat): 旧 getPackagesInfo（每包一条、无 userId），兼容期结束后删除
        await getPackagesInfoBatch(packageNames)
    return Array.isArray(raw) ? raw : [raw]
  } catch {
    return []
  }
}

/**
 * 把实例键（`base` 或 `base@userId`）连同已查到的元数据装成 InstalledApp。
 */
function toInstalledApp(
  packageName: string,
  index: PackageInfoIndex,
  options: GetAppsInfoOptions = {}
): InstalledApp {
  const { fallbackType, assumeInstalled = false } = options
  const { base, userId } = parsePackageUser(packageName)

  // 优先精确命中该用户实例；分身自身没有条目时回退到该包跨用户条目（元数据继承）
  const displayInfo = index.perInstance.get(`${base}@${userId}`) ?? index.perBase.get(base)

  return {
    packageName,
    appName: displayInfo?.appLabel || packageName,
    icon: '',
    versionName: displayInfo?.versionName || '',
    versionCode: displayInfo?.versionCode ?? 0,
    installed: assumeInstalled || Boolean(displayInfo),
    isSystem:
      displayInfo?.isSystem ??
      (fallbackType === 'system' ? true : fallbackType === 'user' ? false : undefined),
  }
}

// 模块级缓存：设备上的全部用户 ID
let cachedUserIds: number[] | null = null

/**
 * 枚举设备上所有用户 ID（经 pm list users），结果缓存。
 * 失败或无用户时安全降级返回 [0]。
 *
 * TODO(legacy-compat): 仅旧路径使用；`pm` 接口的 `userId` 字段已包含该信息，可删除。
 */
async function getUserIds(): Promise<number[]> {
  if (cachedUserIds !== null) {
    return cachedUserIds
  }

  try {
    const output = await execCommand('pm list users')
    const ids: number[] = []
    const regex = /UserInfo\{(\d+):/g
    let match: RegExpExecArray | null
    while ((match = regex.exec(output)) !== null) {
      ids.push(Number(match[1]))
    }
    cachedUserIds = ids.length > 0 ? ids : [0]
  } catch {
    cachedUserIds = [0]
  }

  return cachedUserIds
}

/**
 * TODO(legacy-compat): 每用户一次 shell fork 的旧实现，`pm` 接口普及时删除。
 *
 * 使用 pm list packages --user <id> 枚举指定用户的应用列表。
 * @param type 应用类型
 * @param userId 用户 ID
 * @returns 包名数组；非零用户的包名追加 @userId 后缀
 */
async function listPackagesForUser(type: PackageQueryType, userId: number): Promise<string[]> {
  const typeFlag = type === 'system' ? '-s' : type === 'user' ? '-3' : ''
  const commandParts = ['pm', 'list', 'packages']
  if (typeFlag) commandParts.push(typeFlag)
  commandParts.push('--user', String(userId))

  try {
    const output = await execCommand(commandParts.join(' '))
    const packages: string[] = []
    for (const line of output.split('\n')) {
      const trimmed = line.trim()
      if (!trimmed.startsWith('package:')) continue
      const name = trimmed.slice('package:'.length)
      if (!name) continue
      // user 0 保持原样，非零用户追加 @userId 后缀
      packages.push(userId === 0 ? name : `${name}@${userId}`)
    }
    return packages
  } catch {
    return []
  }
}

/**
 * 回退路径：枚举已安装应用（按用户展开成实例键）。
 *
 * TODO(legacy-compat): 整条 shell 回退链（getUserIds + listPackagesForUser +
 * getPackageListViaLegacyApi）待 `pm` 接口普及时删除。
 *
 * user 0 走 KernelSU 原生 listPackages，非零用户用 `pm list packages --user <id>`
 * 逐个 fork 枚举。仅当 `pm` 接口不可用（旧 manager / APatch）时才会走到这里。
 */
async function getPackageList(type: PackageQueryType): Promise<string[]> {
  const userIds = await getUserIds()

  // user 0 走 KernelSU 原生 listPackages API
  let packageList = await getPackageListViaLegacyApi(type)

  // 非零用户：用 pm list packages --user <id> 枚举
  const nonZeroUserIds = userIds.filter((id) => id !== 0)
  if (nonZeroUserIds.length > 0) {
    const results = await Promise.all(
      nonZeroUserIds.map((userId) => listPackagesForUser(type, userId))
    )
    packageList = [...packageList, ...results.flat()]
  }

  return Array.from(new Set(packageList))
}

/**
 * 枚举已安装包名（实例键：user 0 是裸包名，其他用户带 `@userId` 后缀）。
 *
 * `pm` 接口优先，环境不支持时回退旧的 shell 链；两条路都不可用时返回空数组
 * （与 `getInstalledApps` 的降级语义一致）。
 */
export async function listPackages(packageType: PackageQueryType = 'user'): Promise<string[]> {
  // 开发模式返回模拟数据（过滤规则与 getInstalledApps 保持一致）
  if (import.meta.env?.DEV) {
    const { mockInstalledApps } = await import('../utils/mockData')
    return mockInstalledApps
      .filter((app) => {
        if (packageType === 'system') return app.isSystem === true
        if (packageType === 'user') return app.isSystem !== true
        return true
      })
      .map((app) => app.packageName)
  }

  try {
    if (pm.support) {
      const names = await pm.listPackages(packageType)
      if (names.length > 0) {
        return names
      }
    }
  } catch {
    // pm 接口存在但不可用，落到下面的旧路径
  }

  return await getPackageList(packageType)
}

export async function getAppsInfo(packageNames: string[], options: GetAppsInfoOptions = {}) {
  const uniquePackages = Array.from(new Set(packageNames.map((pkg) => pkg.trim()).filter(Boolean)))

  if (uniquePackages.length === 0) {
    return []
  }

  // pm / 旧 getPackagesInfo 都只认裸包名：传 base 名一次拿到该包在所有用户下的条目。
  const basePackages = Array.from(new Set(uniquePackages.map((pkg) => parsePackageUser(pkg).base)))
  const index = buildPackageInfoIndex(await fetchPackageInfos(basePackages))

  return uniquePackages.map((packageName) => toInstalledApp(packageName, index, options))
}

/**
 * `pm` 接口路径：一次 listPackages + 一次 getPackagesInfo 就能同时拿到
 * 「哪些用户装了哪些包」和逐 uid 的元数据，全程不需要 fork root shell。
 *
 * 返回 null 表示需要回退旧路径（接口不可用、返回空列表或逐 uid 条目全缺）。
 */
async function loadInstalledAppsViaPm(type: PackageQueryType): Promise<InstalledApp[] | null> {
  try {
    const names = await pm.listPackages(type)
    if (names.length === 0) {
      return null
    }

    const index = buildPackageInfoIndex(await fetchPackageInfos(names))
    if (index.perInstance.size === 0) {
      return null
    }

    // 实例键：user 0 用裸包名（与 package.ts 的实例身份约定一致），分身带 @userId 后缀
    const instances = new Set<string>()
    for (const entry of index.perInstance.values()) {
      instances.add(entry.userId === 0 ? entry.base : `${entry.base}@${entry.userId}`)
    }
    // 拿不到逐 uid 条目的包（manager 过滤掉的 special 条目等）用裸包名兜底，避免漏行
    for (const name of names) {
      const base = normalizePackageName(name)
      if (!index.perBase.has(base)) {
        instances.add(base)
      }
    }

    return Array.from(instances).map((pkg) =>
      toInstalledApp(pkg, index, { fallbackType: type, assumeInstalled: true })
    )
  } catch {
    return null
  }
}

// 获取已安装应用列表
export async function getInstalledApps(options: GetInstalledAppsOptions = {}) {
  // 开发模式返回模拟数据
  if (import.meta.env?.DEV) {
    const { mockInstalledApps } = await import('../utils/mockData')
    const packageType = options.packageType || (options.includeSystem ? 'all' : 'user')
    return mockInstalledApps.filter((app) => {
      if (packageType === 'system') return app.isSystem === true
      if (packageType === 'user') return app.isSystem !== true
      return true
    })
  }

  try {
    const packageType = options.packageType || (options.includeSystem ? 'all' : 'user')

    // 新 manager（KernelSU 3.3.1+）：pm 接口一次拿全，含分身实例与各自的元数据
    if (pm.support) {
      const viaPm = await loadInstalledAppsViaPm(packageType)
      if (viaPm && viaPm.length > 0) {
        return viaPm
      }
    }

    const packageList = await getPackageList(packageType)

    // 如果仍然没有获取到应用列表，返回空列表
    if (packageList.length === 0) {
      return []
    }

    return await getAppsInfo(packageList, { fallbackType: packageType, assumeInstalled: true })
  } catch {
    return []
  }
}

/**
 * @deprecated 用 `backend/types` 里的 `ModuleInfo`；保留旧名，避免调用方跟着改。
 */
export type KsuModuleInfo = ModuleInfo

/** `ksu` 桥接里本文件需要的那部分（`kernelsu-alt` 未封装 `moduleInfo`）。 */
interface KsuBridgeWithModuleInfo {
  moduleInfo?: () => string | KsuModuleInfo
}

/**
 * 取当前模块元数据。
 *
 * 走宿主的 `ksu.moduleInfo()`（KernelSU `WebViewInterface.kt::moduleInfo`）：
 * 元数据由 manager 从模块目录解析，**不读文件、不走 shell**，因此
 * `ksu.io` / `ksu.exec` 不可用、或模块目录名与 module.prop 的 `id` 不一致时
 * 依然能拿到版本号。
 *
 * 宿主未实现（旧版 manager / 非 KernelSU 系宿主）时返回 `null`，调用方自行回退。
 * 这里是同步实现，`Backend` 接口由 `createKsuBackend()` 包成 Promise。
 */
export function getModuleInfo(): KsuModuleInfo | null {
  const bridge = (globalThis as { ksu?: KsuBridgeWithModuleInfo }).ksu
  if (!bridge || typeof bridge.moduleInfo !== 'function') {
    return null
  }

  try {
    const raw = bridge.moduleInfo()
    const parsed = typeof raw === 'string' ? JSON.parse(raw) : raw
    return parsed && typeof parsed === 'object' ? (parsed as KsuModuleInfo) : null
  } catch {
    return null
  }
}

export async function fileExists(path: string): Promise<boolean> {
  if (io.support) {
    try {
      // isFile() 与旧的 `test -f` 同义：只认普通文件（目录返回 false）
      return io.File(path).isFile()
    } catch {
      // 宿主 io 不可用，落到下面的 `test -f`
    }
  }

  // TODO(legacy-compat): io 接口不可用时回退 `test -f`，兼容期结束后删除该分支
  try {
    await execCommand(`test -f ${shellQuotePath(path)}`)
    return true
  } catch {
    return false
  }
}

// 创建目录
export async function mkdir(path: string): Promise<void> {
  if (io.support) {
    try {
      io.File(path).mkdirs()
      return
    } catch {
      // 宿主 io 不可用，落到下面的 `mkdir -p`
    }
  }

  // TODO(legacy-compat): io 接口不可用时回退 `mkdir -p`，兼容期结束后删除该分支
  await execCommand(`mkdir -p ${shellQuotePath(path)}`)
}

/**
 * 读当前设备属性全集（`getprop` 输出，`[key]: [value]` 行）。
 *
 * 属性→模板的转换在 `utils/propsToTemplate.ts` 里纯前端完成，这里只取原始文本。
 */
export async function getProps(): Promise<string> {
  return await execCommand('getprop')
}

/**
 * 用系统意图打开链接 / scheme。
 *
 * 失败（宿主没有 exec 能力 / am 报错）时抛错，调用方按原有语义回落到 `window.open`。
 */
export async function openDeepLink(url: string, options: DeepLinkOptions = {}): Promise<void> {
  const { useViewAction = true } = options
  const flags = useViewAction
    ? '-a android.intent.action.VIEW -c android.intent.category.BROWSABLE '
    : ''
  await execCommand(`am start ${flags}-d ${shellQuotePath(url)} >/dev/null 2>&1`)
}

/** 在线模板抓取的 shell 超时（秒），与原先 15s 的请求超时保持一致。 */
const SHELL_FETCH_TIMEOUT_SECONDS = 15
const DEFAULT_FETCH_ACCEPT = 'application/json'

/**
 * 管理器模式抓取在线模板原始文本。
 *
 * WebView 的 fetch 受 CORS 限制（gitee raw 不带 `Access-Control-Allow-Origin`），
 * shell 没有这条限制，所以管理器模式保留 shell 兜底；原生服务模式由服务端代理取代。
 *
 * TODO(dual-transport): `utils/onlineTemplates.ts` 的 shell 兜底切换到本方法后，
 * 删除那里的本地实现。
 */
export async function fetchOnlineTemplates(
  url: string,
  options: FetchOnlineTemplatesOptions = {}
): Promise<string> {
  if (import.meta.env?.DEV) {
    throw new Error('Shell HTTP fallback is not available in development mode.')
  }

  const accept = options.accept || DEFAULT_FETCH_ACCEPT
  const escapedUrl = escapeShellPath(url)
  const escapedAccept = escapeShellPath(accept)
  const timeout = SHELL_FETCH_TIMEOUT_SECONDS

  // 不写死任何 root 方案的目录，运行时按「PATH → 通用 busybox 形态」自己找工具：
  //   · WebUI 的 exec 通常直接以 root 跑 `sh`（真机实测 id = uid=0，context u:r:magisk:s0），
  //     没有 su 参与，因此 PATH 就是 app 继承的标准 Android PATH —— 里面基本没有 curl/wget/busybox；
  //   · KernelSU 的 ksud su 会注入 /data/adb/ksu/bin，Magisk/WeaveMask 则不注入，
  //     所以只按 PATH 找不够，再补一层 `*/busybox`、`*/bin/busybox` 的通用 glob（不点名具体方案）。
  // 顺带：shell 没有 CORS 限制，所以 gitee raw（WebView fetch 拿不到，响应缺
  // Access-Control-Allow-Origin）也能借这条兜底直接抓。
  const script = [
    `CURL=$(command -v curl 2>/dev/null)`,
    `WGET=$(command -v wget 2>/dev/null)`,
    `BB=$(command -v busybox 2>/dev/null)`,
    `[ -z "$BB" ] && BB=$(ls /data/adb/*/busybox /data/adb/*/bin/busybox 2>/dev/null | head -n 1)`,
    // 不要用 exec 替换 shell：真机实测 libsu 抓不到 exec 之后的输出（返回空串），
    // 用 if/elif 顺序执行才能把 stdout 正常带回来。
    `if [ -n "$CURL" ]; then "$CURL" -fsSL --connect-timeout ${timeout} --max-time ${timeout} -H 'Accept: ${escapedAccept}' '${escapedUrl}'`,
    `elif [ -n "$WGET" ]; then "$WGET" -q -O - --timeout=${timeout} --header='Accept: ${escapedAccept}' '${escapedUrl}'`,
    `elif [ -n "$BB" ]; then "$BB" wget -q -O - -T ${timeout} --header='Accept: ${escapedAccept}' '${escapedUrl}'`,
    `else echo 'No usable HTTP tool (curl/wget/busybox) found' >&2; exit 127; fi`,
  ].join('; ')

  return await execCommand(script)
}

/**
 * 管理器模式的 `Backend` 实现。
 *
 * 除 `readConfigRaw` / `writeConfigRaw` 固定映射到 config.toml 外，其余方法与
 * 上面的同名函数一一对应——迁移只换了调用方式，没有改行为。
 */
// ---------------------------------------------------------------- 原生服务开关

/** 模块目录：优先宿主直供的 moduleDir，其次标准安装路径。 */
function resolveModuleDir(): string {
  const moduleDir = (getModuleInfo()?.moduleDir || '').trim()
  return moduleDir || MODULE_DIR_FALLBACK
}

/**
 * 读 server.toml；缺失（或为空）时回退模块目录里的 server.default.toml。
 *
 * 回退的意义：新装设备上 server.toml 还没被 service.sh 生成，UI 仍要能展示默认
 * bind 并让用户直接开启外部访问（保存时会写到正式路径）。
 */
export async function readServerConfig(): Promise<string | null> {
  try {
    const current = await readFile(SERVER_CONFIG_PATH)
    if (current.trim()) {
      return current
    }
  } catch {
    // 文件不存在 / 无权限：走默认模板
  }

  try {
    const fallback = await readFile(`${resolveModuleDir()}/server.default.toml`)
    return fallback.trim() ? fallback : null
  } catch {
    return null
  }
}

/** 写 server.toml 并把权限收紧到 0600（文件里存着 Token 哈希）。 */
export async function writeServerConfig(text: string): Promise<void> {
  await writeFile(SERVER_CONFIG_PATH, text)
  await execCommand(`chmod 600 ${shellQuotePath(SERVER_CONFIG_PATH)}`)
}

/** 调 service.sh 执行一个动作（start / stop / restart / status）。 */
async function runServiceScript(action: string): Promise<string> {
  const script = `${resolveModuleDir()}/service.sh`
  // status 需要读输出；start/stop/restart 的守护进程会继承 stdout，
  // 不关掉的话 adb/管理器会一直等管道 EOF（实测 restart 挂到超时）。
  const redirect = action === 'status' ? '' : ' >/dev/null 2>&1'
  return await execCommand(`sh ${shellQuotePath(script)} ${action}${redirect}`)
}

/** 重启原生服务（service.sh restart），返回脚本 stdout。 */
export async function restartServer(): Promise<string> {
  return await runServiceScript('restart')
}

/** 原生服务状态（service.sh status）的原始输出。 */
export async function getServerControlStatus(): Promise<string> {
  return await runServiceScript('status')
}

/**
 * 生成访问 Token（明文只返回一次，不落盘）。
 *
 * 与 server.default.toml 注释里的命令保持一致：32 字节随机数 → base64url（`-_`、
 * 去掉 `=`）→ `df_` 前缀；哈希用 sha256sum 取 64 位小写十六进制，返回时带
 * `sha256:` 前缀，可直接写进 [[tokens]]。
 */
export async function generateServerToken(): Promise<ServerTokenPair> {
  const script = [
    'set -e',
    'RAW=$(head -c 32 /dev/urandom | base64 | tr -d "\\n" | tr "+/" "-_")',
    'RAW=$(printf "%s" "$RAW" | tr -d "=")',
    'TOKEN="df_$RAW"',
    'HASH=$(printf "%s" "$TOKEN" | sha256sum | cut -d" " -f1)',
    'printf "%s\\n%s\\n" "$TOKEN" "$HASH"',
  ].join('; ')

  const output = (await execCommand(script)).trim()
  const [token, hash] = output.split('\n').map((line) => line.trim())

  if (!token?.startsWith('df_') || !/^[0-9a-f]{64}$/.test(hash || '')) {
    throw new Error(`Token 生成失败：${output || '无输出'}`)
  }

  return { token, hash: `sha256:${hash}` }
}

/**
 * 计算用户自定义 Token 的 sha256（明文不落盘）。
 *
 * 单引号包裹后交给 shell，避免 `$`、反引号、空格等被解释；换行会让下面的
 * 两行输出协议失效，因此直接拒绝。
 */
export async function hashServerToken(token: string): Promise<ServerTokenPair> {
  if (!token) {
    throw new Error('Token 不能为空')
  }
  if (token.includes('\n') || token.includes('\r')) {
    throw new Error('Token 不能包含换行')
  }

  const quoted = shellQuotePath(token)
  const script = [
    'set -e',
    `HASH=$(printf '%s' ${quoted} | sha256sum | cut -d" " -f1)`,
    `printf '%s\\n%s\\n' ${quoted} "$HASH"`,
  ].join('; ')

  const output = (await execCommand(script)).trim()
  const [echoed, hash] = output.split('\n').map((line) => line.trim())
  if (echoed !== token || !/^[0-9a-f]{64}$/.test(hash || '')) {
    throw new Error(`Token 哈希计算失败：${output || '无输出'}`)
  }

  return { token, hash: `sha256:${hash}` }
}

/** 本机可用于局域网访问的 IPv4（`ip` 优先，`getprop dhcp.*` 兜底）。 */
export async function getLanAddresses(): Promise<string[]> {
  let addresses: string[]

  try {
    addresses = parseIpAddrOutput(await execCommand('ip -4 addr show 2>/dev/null || true'))
  } catch {
    addresses = []
  }

  if (addresses.length === 0) {
    try {
      addresses = parseDhcpProps(await execCommand('getprop 2>/dev/null || true'))
    } catch {
      addresses = []
    }
  }

  return addresses
}

// ---------------------------------------------------------------------------
// 出口代理（egress）：管理器模式的读写（契约 §1 / §2 / §3）
// ---------------------------------------------------------------------------
//
// 管理器模式没有 /api/v1，因此这里直接读写两个文件：
// - egress.toml：代理定义 + [probe]（0600，含密码，只在本进程内存里出现）；
// - config.toml：[egress].default（只放引用，不放凭据）。
// 写完后按 service.sh restart 拉起原生服务，让它按文件状态应用方案 A；
// 同时本地执行一次 settings put 让保存立即生效（幂等，服务启动后会再校正一次）。

const EGRESS_FILE_MODE = '600'
const CONFIG_FILE_MODE = '644'

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms)
  })
}

async function chmodPath(path: string, mode: string): Promise<void> {
  try {
    await execCommand(`chmod ${mode} ${shellQuotePath(path)}`)
  } catch {
    // 权限收紧失败不阻塞主流程：服务端读 egress.toml 时会自愈成 0600（契约 §2.2）
  }
}

async function readTextOrEmpty(path: string): Promise<string> {
  try {
    return await readFile(path)
  } catch {
    return ''
  }
}

interface SystemProxyReadback {
  value: string | null
  dumpsys: string | null
  available: boolean
}

/** 用 shell 读方案 A 的真实读回值（settings get + dumpsys 交叉验证）。 */
async function readSystemProxy(): Promise<SystemProxyReadback> {
  let value: string | null
  let dumpsys: string | null
  let available = false

  try {
    const output = (await execCommand('settings get global http_proxy 2>/dev/null || true')).trim()
    value = !output || output === 'null' ? null : output
    available = true
  } catch {
    value = null
  }

  try {
    const output = await execCommand(
      "dumpsys connectivity 2>/dev/null | grep -m1 -i 'global proxy' || true"
    )
    const line =
      output
        .split('\n')
        .map((item) => item.trim())
        .find((item) => item.length > 0) ?? ''
    dumpsys = line || null
  } catch {
    dumpsys = null
  }

  return { value, dumpsys, available }
}

/** settings 的直连表示：null / 空串 / "null" / ":0" 都算直连。 */
function normalizeSettingsValue(value: string | null): string {
  if (!value) return ':0'
  const trimmed = value.trim()
  return !trimmed || trimmed === 'null' ? ':0' : trimmed
}

async function readEgressFileInfo(text: string): Promise<EgressFileInfo> {
  let exists: boolean
  try {
    exists = await fileExists(EGRESS_PATH)
  } catch {
    // 宿主文件接口不可用时按「文件内容是否非空」推断，界面不因一次 stat 失败整块报错
    exists = text.length > 0
  }
  let mtime: string | null = null
  if (exists) {
    try {
      const output = (
        await execCommand(`stat -c %Y ${shellQuotePath(EGRESS_PATH)} 2>/dev/null || true`)
      ).trim()
      const seconds = Number.parseInt(output, 10)
      if (Number.isFinite(seconds) && seconds > 0) {
        mtime = new Date(seconds * 1000).toISOString()
      }
    } catch {
      mtime = null
    }
  }
  return { exists, size: text.length, mtime }
}

function toProxySummaries(proxies: readonly EgressProxyInput[]): EgressProxySummary[] {
  return proxies.map((proxy) => ({
    name: proxy.name,
    type: proxy.type,
    host: proxy.host,
    port: proxy.port,
    hasCredentials: Boolean(proxy.username || proxy.password),
  }))
}

/** 代理定义 → 应用级视图（与 GET /egress 的 apps[] 同义）。 */
function toAppViews(
  apps: ReadonlyArray<{ package: string; egress: string | null }>,
  proxies: readonly EgressProxySummary[]
): EgressAppView[] {
  return apps.flatMap((item) => {
    const name = item.egress
    if (!name) return []
    if (name === DIRECT_MARKER) {
      return [{ package: item.package, egress: name, effective: 'direct' as const, proxy: null }]
    }
    const proxy = proxies.find((candidate) => candidate.name === name)
    return [
      {
        package: item.package,
        egress: name,
        effective: proxy ? ('proxy' as const) : ('direct' as const),
        proxy: proxy ? proxy.name : null,
      },
    ]
  })
}

/** 管理器模式的 GET /egress 等价物：两个文件 + settings 读回。 */
async function readEgress(): Promise<EgressSnapshot> {
  const egressText = await readTextOrEmpty(EGRESS_PATH)
  const file = parseEgressFile(egressText)
  const configText = await readTextOrEmpty(CONFIG_PATH)
  const refs = readConfigEgressRefs(configText)
  const system = await readSystemProxy()
  const egressFile = await readEgressFileInfo(egressText)

  const proxies = toProxySummaries(file.proxies)
  const defaultName = refs.default
  const upstream = proxies.find((proxy) => proxy.name === defaultName) ?? null
  const expected = expectedSettingsValue(defaultName, proxies)
  const actual = normalizeSettingsValue(system.value)
  const applied = system.available ? actual === expected : false
  const apps = toAppViews(refs.apps, proxies)
  const mode = defaultName ? ('proxy' as const) : ('direct' as const)

  const warnings: EgressWarning[] = []
  if (file.parseError) {
    warnings.push({ code: 'egress_parse', message: file.parseError })
  }
  if (refs.parseError) {
    warnings.push({ code: 'egress_parse', message: `config.toml 解析失败：${refs.parseError}` })
  }
  if (defaultName && !upstream) {
    warnings.push({
      code: 'egress_proxy_not_found',
      message: `设备级默认代理没有对应定义：${defaultName}`,
    })
  }
  if (upstream?.hasCredentials) {
    warnings.push({
      code: 'egress_credentials_ignored',
      message: '设备级代理带凭据：方案 A 只写 host:port，认证不生效',
    })
  }
  if (system.available && !applied) {
    warnings.push({
      code: 'egress_settings_mismatch',
      message: `settings 读回与期望不一致（期望 ${expected}，实际 ${actual}）`,
    })
  }

  return {
    revision: '',
    configRevision: '',
    configured: proxies.length > 0 || Boolean(defaultName) || apps.length > 0,
    mode,
    effectiveMode: mode,
    default: defaultName,
    failOpen: file.probe.failOpen,
    probeUrl: file.probe.url,
    probeTimeoutSeconds: file.probe.timeoutSeconds,
    exclusionList: [],
    upstream,
    proxies,
    apps,
    applied,
    readback: {
      settingsValue: system.value,
      dumpsysGlobalProxy: system.dumpsys,
      matches: applied,
      at: new Date().toISOString(),
    },
    health: { degraded: false, degradedSince: null, degradedReason: null, lastProbe: null },
    egressFile,
    warnings,
    revisionAvailable: false,
    readbackAvailable: system.available,
  }
}

/** 入参里省略的凭据沿用同名的已存值（对应 PUT 语义里的「省略 = 不改」）。 */
function mergeStoredCredentials(
  current: readonly EgressProxyInput[],
  incoming: readonly EgressProxyInput[]
): EgressProxyInput[] {
  return incoming.map((proxy) => {
    const previous = current.find((item) => item.name === proxy.name)
    const entry: EgressProxyInput = {
      name: proxy.name,
      type: proxy.type,
      host: proxy.host,
      port: proxy.port,
    }
    const username = proxy.username === undefined ? previous?.username : proxy.username
    const password = proxy.password === undefined ? previous?.password : proxy.password
    if (typeof username === 'string' && username) entry.username = username
    if (typeof password === 'string' && password) entry.password = password
    return entry
  })
}

async function applySystemProxy(value: string): Promise<void> {
  await execCommand(`settings put global http_proxy ${shellQuotePath(value)}`)
}

/** service.sh restart 后轮询状态，确认服务真的回来了。 */
async function restartServerAndWait(timeoutMs = 8000): Promise<boolean> {
  await restartServer()
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const status = await getServerControlStatus()
      if (/server:\s*running/i.test(status)) return true
    } catch {
      // 服务刚拉起时 status 可能短暂失败，继续轮询
    }
    await sleep(500)
  }
  return false
}

async function writeEgress(input: EgressUpdateInput): Promise<EgressWriteResult> {
  if (input.apps !== undefined) {
    throw new UnsupportedError('管理器模式不支持写入应用级出口覆盖，请在原生服务模式下操作')
  }

  const file = parseEgressFile(await readTextOrEmpty(EGRESS_PATH))
  const model: EgressFileModel = {
    proxies: input.proxies ? mergeStoredCredentials(file.proxies, input.proxies) : file.proxies,
    probe: {
      url: input.probeUrl === undefined ? file.probe.url : (input.probeUrl ?? DEFAULT_PROBE_URL),
      timeoutSeconds: input.probeTimeoutSeconds ?? file.probe.timeoutSeconds,
      failOpen: input.failOpen ?? file.probe.failOpen,
    },
    parseError: null,
  }

  await mkdir(CONFIG_DIR)
  await writeFile(EGRESS_PATH, renderEgressFile(model))
  await chmodPath(EGRESS_PATH, EGRESS_FILE_MODE)

  const configText = await readTextOrEmpty(CONFIG_PATH)
  const nextConfig =
    input.default === undefined ? configText : setConfigEgressDefault(configText, input.default)
  if (nextConfig !== configText) {
    await writeFile(CONFIG_PATH, nextConfig)
    await chmodPath(CONFIG_PATH, CONFIG_FILE_MODE)
  }

  // 立即应用一次方案 A：管理器模式下服务不一定在跑，保存就该当场生效。
  // 服务启动后会按 egress.toml + config.toml 再校正一次（幂等）。
  if (input.default !== undefined) {
    const target = input.default
      ? model.proxies.find((item) => item.name === input.default)
      : undefined
    const value = target ? `${target.host}:${target.port}` : ':0'
    try {
      await applySystemProxy(value)
    } catch {
      // 写 settings 失败不算致命：readback.applied 会显示 false，界面提示
    }
  }

  const running = await restartServerAndWait()
  const after = await readEgress()
  const warnings = [...after.warnings]
  if (!running) {
    warnings.push({
      code: 'server_not_running',
      message: '服务未在超时内恢复运行：出口配置已写入，但可能尚未被服务应用',
    })
  }

  return {
    revision: '',
    previousRevision: null,
    configRevision: '',
    dryRun: false,
    changed: true,
    backupId: null,
    mode: after.mode,
    effectiveMode: after.effectiveMode,
    default: after.default,
    applied: after.applied,
    upstream: after.upstream,
    readback: after.readback,
    health: after.health,
    warnings,
    revisionAvailable: false,
  }
}

/** 一键恢复直连：清定义 + 清 config 引用 + settings 置 :0 + 重启服务。 */
async function clearEgress(): Promise<EgressWriteResult> {
  const file = parseEgressFile(await readTextOrEmpty(EGRESS_PATH))
  const model: EgressFileModel = {
    proxies: [],
    // [probe] 是策略不是出口，保留（契约 §1.5 第 4 项）
    probe: { ...file.probe },
    parseError: null,
  }

  await mkdir(CONFIG_DIR)
  await writeFile(EGRESS_PATH, renderEgressFile(model))
  await chmodPath(EGRESS_PATH, EGRESS_FILE_MODE)

  const configText = await readTextOrEmpty(CONFIG_PATH)
  const nextConfig = clearConfigEgress(configText)
  if (nextConfig !== configText) {
    await writeFile(CONFIG_PATH, nextConfig)
    await chmodPath(CONFIG_PATH, CONFIG_FILE_MODE)
  }

  // 立即切直连（必须用 :0，settings delete 无效，契约 §3.2）
  try {
    await applySystemProxy(':0')
  } catch {
    // readback 会暴露不一致
  }

  const running = await restartServerAndWait()
  const after = await readEgress()
  const warnings = [...after.warnings]
  if (!running) {
    warnings.push({
      code: 'server_not_running',
      message: '服务未在超时内恢复运行：出口配置已清空，但可能尚未被服务应用',
    })
  }

  return {
    revision: '',
    previousRevision: null,
    configRevision: '',
    dryRun: false,
    changed: true,
    backupId: null,
    mode: after.mode,
    effectiveMode: after.effectiveMode,
    default: after.default,
    applied: after.applied,
    upstream: after.upstream,
    readback: after.readback,
    health: after.health,
    warnings,
    revisionAvailable: false,
  }
}

/** 管理器模式下探测必须由原生服务执行：这里明确失败，绝不假装成功。 */
async function probeEgress(_request?: EgressProbeRequest): Promise<EgressProbeResult> {
  throw new UnsupportedError(
    '管理器模式下无法探测出口 IP：探测由原生服务执行，请改用服务地址访问 WebUI'
  )
}

export function createKsuBackend(): Backend {
  return {
    runtime: 'ksu',
    // 管理器模式没有服务端，也就没有服务版本/bind/注入信息
    serverRuntime: null,
    readConfigRaw: () => readFile(CONFIG_PATH),
    // 直接写文件：没有 revision 可以比对，写后也不回填（保持旧行为）
    writeConfigRaw: async (content) => {
      await writeFile(CONFIG_PATH, content)
      return { revision: '', previousRevision: null, warnings: [] }
    },
    getConfigRevision: async () => ({ revision: '', exists: await fileExists(CONFIG_PATH) }),
    getServerStatus: async () => null,
    readFile,
    writeFile,
    fileExists,
    mkdir,
    listPackages,
    getPackagesInfo: getAppsInfo,
    getInstalledApps,
    getModuleInfo: async () => getModuleInfo(),
    getProps,
    readServerConfig,
    writeServerConfig,
    restartServer,
    getServerControlStatus,
    generateServerToken,
    hashServerToken,
    getLanAddresses,
    openDeepLink,
    fetchOnlineTemplates,
    readEgress,
    writeEgress,
    clearEgress,
    probeEgress,
  }
}

export default {
  execCommand,
  readFile,
  writeFile,
  getInstalledApps,
  fileExists,
  mkdir,
}
