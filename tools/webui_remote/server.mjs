#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask
//
// 外部 WebUI 桥接服务（开发工具，不属于模块运行时产物）。
//
// 作用：
//   1. 把 `module/webroot`（`cd webui && npm run build` 的产物）通过 HTTP 提供；
//   2. 在 index.html 注入 shim.js，把 WebUI 依赖的 KernelSU `ksu` 宿主接口
//      转发到本机 adb 连接的 Android 设备（exec / io / pm / moduleInfo / toast）；
//   3. 于是可以在电脑、甚至局域网其它设备的浏览器里直接使用模块 WebUI。
//
// 用法：
//   node tools/webui_remote/server.mjs [--port 8080] [--host 0.0.0.0]
//        [--serial <adb 序列号>] [--root <静态目录>] [--adb <adb 路径>]

import http from 'node:http'
import fs from 'node:fs'
import fsp from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import crypto from 'node:crypto'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const REPO_ROOT = path.resolve(HERE, '..', '..')

// ── 参数 ────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const opts = {
    host: '0.0.0.0',
    port: 8080,
    root: path.join(REPO_ROOT, 'module', 'webroot'),
    serial: process.env.ANDROID_SERIAL || '',
    adb: process.env.ADB || 'adb',
    timeout: 60000,
  }

  for (let i = 2; i < argv.length; i += 1) {
    const arg = argv[i]
    const [key, inlineValue] = arg.includes('=') ? arg.split(/=(.*)/s, 2) : [arg, undefined]
    const value = () => (inlineValue !== undefined ? inlineValue : argv[++i])

    switch (key) {
      case '--host':
        opts.host = value()
        break
      case '--port':
        opts.port = Number(value())
        break
      case '--root':
        opts.root = path.resolve(value())
        break
      case '--serial':
        opts.serial = value()
        break
      case '--adb':
        opts.adb = value()
        break
      case '--timeout':
        opts.timeout = Number(value()) * 1000
        break
      case '-h':
      case '--help':
        printUsage()
        process.exit(0)
        break
      default:
        console.error(`未知参数: ${arg}`)
        printUsage()
        process.exit(2)
    }
  }

  return opts
}

function printUsage() {
  console.log(`用法: node tools/webui_remote/server.mjs [选项]

  --host <地址>      监听地址，默认 0.0.0.0（局域网可访问）
  --port <端口>      监听端口，默认 8080
  --root <目录>      静态文件目录，默认 module/webroot
  --serial <序列号>  指定 adb 设备，默认取唯一在线设备
  --adb <路径>       adb 可执行文件，默认 adb
  --timeout <秒>     单条 shell 命令超时，默认 60`)
}

const opts = parseArgs(process.argv)

// ── adb 后端 ───────────────────────────────────────────────────────────────

let resolvedSerial = opts.serial
let deviceInfo = null

function runProcess(command, args, timeoutMs = opts.timeout) {
  return new Promise((resolve) => {
    let child
    try {
      child = spawn(command, args, { stdio: ['ignore', 'pipe', 'pipe'] })
    } catch (error) {
      resolve({ code: 127, stdout: Buffer.alloc(0), stderr: Buffer.from(String(error)), timedOut: false })
      return
    }

    const stdout = []
    const stderr = []
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      child.kill('SIGKILL')
    }, timeoutMs)

    child.stdout.on('data', (chunk) => stdout.push(chunk))
    child.stderr.on('data', (chunk) => stderr.push(chunk))
    child.on('error', (error) => stderr.push(Buffer.from(String(error))))
    child.on('close', (code) => {
      clearTimeout(timer)
      resolve({
        code: code ?? (timedOut ? 124 : 1),
        stdout: Buffer.concat(stdout),
        stderr: Buffer.concat(stderr),
        timedOut,
      })
    })
  })
}

const decode = (buffer) => buffer.toString('utf8').replace(/\r\n/g, '\n')

async function requireSerial() {
  if (resolvedSerial) return resolvedSerial

  const result = await runProcess(opts.adb, ['devices'], 8000)
  const line = decode(result.stdout)
    .split('\n')
    .map((entry) => entry.trim())
    .find((entry) => /^\S+\s+device\b/.test(entry))

  if (!line) {
    throw new Error('没有在线的 adb 设备：请先 `adb devices` 确认识别，或用 --serial 指定')
  }

  resolvedSerial = line.split(/\s+/)[0]
  return resolvedSerial
}

async function adb(args, timeoutMs = opts.timeout) {
  const serial = await requireSerial()
  return runProcess(opts.adb, ['-s', serial, ...args], timeoutMs)
}

async function shell(command, timeoutMs = opts.timeout) {
  const result = await adb(['shell', command], timeoutMs)
  return {
    code: result.code,
    stdout: decode(result.stdout),
    stderr: decode(result.stderr),
    timedOut: result.timedOut,
  }
}

async function execOut(command, timeoutMs = opts.timeout) {
  const result = await adb(['exec-out', command], timeoutMs)
  return { code: result.code, buffer: result.stdout, stderr: decode(result.stderr) }
}

const quote = (value) => `'${String(value).replace(/'/g, "'\''")}'`

async function detectDevice() {
  try {
    const serial = await requireSerial()
    const props = await shell('getprop ro.product.model; getprop ro.build.version.release', 8000)
    const [model, release] = props.stdout.split('\n')
    deviceInfo = { serial, model: model || '', android: release || '' }
  } catch {
    deviceInfo = null
  }
  return deviceInfo
}

// ── 文件 IO（ksu.io 后端）──────────────────────────────────────────────────

const readers = new Map()
const writers = new Map()

async function remoteReadFile(remotePath) {
  const result = await execOut(`cat ${quote(remotePath)}`)
  if (result.code !== 0) {
    throw new Error(result.stderr.trim() || `读取失败: ${remotePath}`)
  }
  return result.buffer
}

async function remoteWriteFile(remotePath, buffer, append = false) {
  const dir = path.posix.dirname(remotePath)
  if (dir && dir !== '.') {
    await shell(`mkdir -p ${quote(dir)}`)
  }

  const serial = await requireSerial()
  const command = `cat ${append ? '>>' : '>'} ${quote(remotePath)}`

  const code = await new Promise((resolve) => {
    const child = spawn(opts.adb, ['-s', serial, 'exec-in', command], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stderr = ''
    const timer = setTimeout(() => child.kill('SIGKILL'), opts.timeout)

    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString()
    })
    child.on('error', () => {
      clearTimeout(timer)
      resolve(127)
    })
    child.on('close', (exitCode) => {
      clearTimeout(timer)
      if (exitCode !== 0 && stderr.trim()) {
        console.error('[bridge] 写入 stderr:', stderr.trim())
      }
      resolve(exitCode ?? 1)
    })
    child.stdin.on('error', () => {})
    child.stdin.end(buffer)
  })

  if (code !== 0) {
    throw new Error(`写入失败: ${remotePath}`)
  }

  await shell(`chmod 644 ${quote(remotePath)}`)
}

// ── 包信息（pm / dumpsys）─────────────────────────────────────────────────

let packageDumpCache = { at: 0, map: null }
let packageListCache = new Map()
let userCache = { at: 0, ids: [0] }

async function userIds() {
  if (Date.now() - userCache.at < 60000) return userCache.ids

  const result = await shell('pm list users', 15000)
  const ids = []
  for (const match of result.stdout.matchAll(/UserInfo\{(\d+):/g)) {
    ids.push(Number(match[1]))
  }
  userCache = { at: Date.now(), ids: ids.length > 0 ? ids : [0] }
  return userCache.ids
}

async function packageDumpMap() {
  if (packageDumpCache.map && Date.now() - packageDumpCache.at < 120000) {
    return packageDumpCache.map
  }

  const result = await shell('dumpsys package', 120000)
  if (result.code !== 0) {
    throw new Error(result.stderr.trim() || 'dumpsys package 失败')
  }

  packageDumpCache = { at: Date.now(), map: parsePackageDump(result.stdout) }
  return packageDumpCache.map
}

function parsePackageDump(text) {
  const map = new Map()
  let current = null

  for (const line of text.split('\n')) {
    const header = /^  Package \[([^\]]+)\]/.exec(line)
    if (header) {
      current = { name: header[1], versionCode: 0, versionName: '', appId: null, system: false, codePath: '' }
      map.set(header[1], current)
      continue
    }
    if (!current) continue

    if (line.startsWith('    appId=')) {
      current.appId = Number.parseInt(line.slice('    appId='.length), 10)
    } else if (line.startsWith('    versionCode=')) {
      const value = Number.parseInt(line.slice('    versionCode='.length), 10)
      if (Number.isFinite(value)) current.versionCode = value
    } else if (line.startsWith('    versionName=')) {
      current.versionName = line.slice('    versionName='.length).trim()
    } else if (line.startsWith('    codePath=')) {
      current.codePath = line.slice('    codePath='.length).trim()
    } else if (line.startsWith('    pkgFlags=[') || line.startsWith('    flags=[')) {
      if (/\bSYSTEM\b/.test(line)) current.system = true
    }
  }

  for (const entry of map.values()) {
    if (!entry.system && /^\/(system|system_ext|product|vendor|odm|apex)\b/.test(entry.codePath)) {
      entry.system = true
    }
  }

  return map
}

async function listPackages(type) {
  const key = type || 'all'
  const cached = packageListCache.get(key)
  if (cached && Date.now() - cached.at < 60000) {
    return cached.list
  }

  const flag = type === 'system' ? '-s' : type === 'user' ? '-3' : ''
  const users = await userIds()
  const packages = new Set()

  for (const userId of users) {
    const command = `pm list packages ${flag} --user ${userId}`.replace(/\s+/g, ' ').trim()
    const result = await shell(command, 30000)
    for (const rawLine of result.stdout.split('\n')) {
      const line = rawLine.trim()
      if (!line.startsWith('package:')) continue
      const name = line.slice('package:'.length).trim()
      if (!name) continue
      packages.add(userId === 0 ? name : `${name}@${userId}`)
    }
  }

  const list = Array.from(packages).sort()
  packageListCache.set(key, { at: Date.now(), list })
  return list
}

function parseInstanceKey(packageName) {
  const at = packageName.lastIndexOf('@')
  if (at > 0) {
    const suffix = packageName.slice(at + 1)
    if (/^\d+$/.test(suffix)) {
      return { base: packageName.slice(0, at), userId: Number(suffix) }
    }
  }
  return { base: packageName, userId: 0 }
}

async function packageInfos(packages) {
  const map = await packageDumpMap()
  const result = []

  for (const packageName of packages) {
    const { base, userId } = parseInstanceKey(String(packageName))
    const entry = map.get(base)
    if (!entry) continue

    const appId = Number.isFinite(entry.appId) ? entry.appId : null
    result.push({
      packageName: base,
      versionName: entry.versionName,
      versionCode: entry.versionCode,
      appLabel: '',
      isSystem: entry.system,
      uid: appId === null ? null : appId + userId * 100000,
      userId,
    })
  }

  return result
}

// ── 模块信息 ──────────────────────────────────────────────────────────────

let moduleInfoCache = { at: 0, info: null }

function parseProp(text) {
  const map = {}
  for (const line of text.split('\n')) {
    const match = /^([^=#\s][^=]*)=(.*)$/.exec(line)
    if (match) map[match[1].trim()] = match[2].trim()
  }
  return map
}

async function moduleInfo() {
  if (moduleInfoCache.info && Date.now() - moduleInfoCache.at < 5000) {
    return moduleInfoCache.info
  }

  const found = await shell(
    "grep -l '^id=devmask' /data/adb/modules/*/module.prop /data/adb/modules_update/*/module.prop 2>/dev/null | head -1",
    15000,
  )
  const propPath = found.stdout.trim().split('\n')[0]
  if (!propPath) {
    moduleInfoCache = { at: Date.now(), info: null }
    return null
  }

  const dir = path.posix.dirname(propPath)
  const prop = parseProp((await remoteReadFile(propPath)).toString('utf8'))
  const disabled = await shell(`test -e ${quote(`${dir}/disable`)}`, 10000)

  const info = {
    moduleDir: dir,
    id: prop.id || 'devmask',
    name: prop.name || 'DevMask',
    version: prop.version || '',
    versionCode: Number.parseInt(prop.versionCode || '0', 10) || 0,
    author: prop.author || '',
    description: prop.description || '',
    enabled: disabled.code !== 0,
  }

  moduleInfoCache = { at: Date.now(), info }
  return info
}

// ── 桥接接口 ──────────────────────────────────────────────────────────────

async function handleBridge(route, body) {
  switch (route) {
    case '/exec': {
      let command = String(body.command ?? '')
      const options = body.options || {}
      if (options.env && typeof options.env === 'object') {
        const exports = Object.entries(options.env)
          .map(([key, value]) => `export ${key}=${quote(value)};`)
          .join(' ')
        command = `${exports} ${command}`
      }
      if (options.cwd) {
        command = `cd ${quote(options.cwd)} && { ${command}; }`
      }
      const result = await shell(command)
      return { errno: result.code, stdout: result.stdout, stderr: result.stderr }
    }

    case '/packages':
      return listPackages(body.type || 'all')

    case '/packages/info':
      return packageInfos(Array.isArray(body.packages) ? body.packages : [])

    case '/module-info':
      return (await moduleInfo()) || {}

    case '/state': {
      const info = deviceInfo || (await detectDevice())
      return { device: info, root: opts.root, webrootReady: fs.existsSync(path.join(opts.root, 'index.html')) }
    }

    case '/io/version':
      return { version: 1 }

    case '/io/stat': {
      const flag = body.kind === 'isFile' ? '-f' : body.kind === 'isDirectory' ? '-d' : '-e'
      const result = await shell(`test ${flag} ${quote(body.path)}`, 15000)
      return { ok: result.code === 0 }
    }

    case '/io/mkdirs': {
      await shell(`mkdir -p ${quote(body.path)}`, 15000)
      return { ok: true }
    }

    case '/io/delete': {
      await shell(`rm -f ${quote(body.path)}`, 15000)
      return { ok: true }
    }

    case '/io/rename': {
      const result = await shell(`mv -f ${quote(body.from)} ${quote(body.to)}`, 30000)
      return { ok: result.code === 0, error: result.stderr.trim() }
    }

    case '/io/open-read': {
      const buffer = await remoteReadFile(body.path)
      const id = crypto.randomUUID()
      readers.set(id, { buffer, read: false })
      return { id }
    }

    case '/io/read': {
      const reader = readers.get(body.id)
      if (!reader || reader.read) return { chunk: null }
      reader.read = true
      return { chunk: reader.buffer.toString('base64') }
    }

    case '/io/close': {
      readers.delete(body.id)
      return { ok: true }
    }

    case '/io/open-write': {
      const id = crypto.randomUUID()
      writers.set(id, { path: body.path, append: Boolean(body.append), chunks: [] })
      return { id }
    }

    case '/io/write': {
      const writer = writers.get(body.id)
      if (!writer) throw new Error('写入句柄不存在')
      writer.chunks.push(Buffer.from(String(body.data || ''), 'base64'))
      return { ok: true }
    }

    case '/io/flush':
      return { ok: true }

    case '/io/close-write': {
      const writer = writers.get(body.id)
      if (!writer) throw new Error('写入句柄不存在')
      writers.delete(body.id)
      const buffer = Buffer.concat(writer.chunks)
      await remoteWriteFile(writer.path, buffer, writer.append)
      return { ok: true }
    }

    default:
      throw new Error(`未知桥接接口: ${route}`)
  }
}

// ── 静态文件 ──────────────────────────────────────────────────────────────

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.map': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.gif': 'image/gif',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.otf': 'font/otf',
  '.wasm': 'application/wasm',
  '.txt': 'text/plain; charset=utf-8',
  '.toml': 'text/plain; charset=utf-8',
}

const SHIM_TAG = '<script src="/__bridge/shim.js"></script>'

function injectShim(html) {
  if (html.includes('/__bridge/shim.js')) return html
  const head = /<head[^>]*>/i.exec(html)
  if (head) {
    const end = head.index + head[0].length
    return `${html.slice(0, end)}\n  ${SHIM_TAG}${html.slice(end)}`
  }
  return `${SHIM_TAG}\n${html}`
}

async function serveStatic(req, res, pathname) {
  const root = path.resolve(opts.root)
  let filePath = path.resolve(root, `.${decodeURIComponent(pathname)}`)

  if (filePath !== root && !filePath.startsWith(root + path.sep)) {
    res.writeHead(403).end('Forbidden')
    return
  }

  let stat = null
  try {
    stat = await fsp.stat(filePath)
  } catch {
    stat = null
  }

  if (stat?.isDirectory()) {
    filePath = path.join(filePath, 'index.html')
    stat = await fsp.stat(filePath).catch(() => null)
  }

  if (!stat?.isFile()) {
    if (path.extname(pathname)) {
      res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' }).end('Not Found')
      return
    }
    filePath = path.join(root, 'index.html')
    stat = await fsp.stat(filePath).catch(() => null)
    if (!stat) {
      res
        .writeHead(500, { 'Content-Type': 'text/plain; charset=utf-8' })
        .end(`静态目录里没有 index.html：${root}\n请先运行 cd webui && npm run build 构建`)
      return
    }
  }

  const extension = path.extname(filePath).toLowerCase()
  const headers = { 'Content-Type': MIME[extension] || 'application/octet-stream' }

  if (extension === '.html') {
    headers['Cache-Control'] = 'no-store'
    const html = await fsp.readFile(filePath, 'utf8')
    const output = Buffer.from(injectShim(html), 'utf8')
    headers['Content-Length'] = String(output.length)
    res.writeHead(200, headers).end(output)
    return
  }

  headers['Cache-Control'] = pathname.startsWith('/assets/') ? 'public, max-age=31536000, immutable' : 'no-cache'
  headers['Content-Length'] = String(stat.size)
  res.writeHead(200, headers)
  fs.createReadStream(filePath).pipe(res)
}

async function readJsonBody(req) {
  const chunks = []
  let size = 0
  for await (const chunk of req) {
    size += chunk.length
    if (size > 64 * 1024 * 1024) throw new Error('请求体过大')
    chunks.push(chunk)
  }
  if (size === 0) return {}
  return JSON.parse(Buffer.concat(chunks).toString('utf8'))
}

function sendJson(res, status, payload) {
  const output = Buffer.from(JSON.stringify(payload), 'utf8')
  res.writeHead(status, {
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': String(output.length),
    'Cache-Control': 'no-store',
  })
  res.end(output)
}

// ── 服务 ──────────────────────────────────────────────────────────────────

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url || '/', 'http://localhost')

    if (url.pathname === '/__bridge/shim.js') {
      const shim = await fsp.readFile(path.join(HERE, 'shim.js'))
      res.writeHead(200, {
        'Content-Type': 'text/javascript; charset=utf-8',
        'Content-Length': String(shim.length),
        'Cache-Control': 'no-store',
      })
      res.end(shim)
      return
    }

    if (url.pathname.startsWith('/__bridge/icon/')) {
      res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' }).end('no icon')
      return
    }

    if (url.pathname.startsWith('/__bridge/')) {
      const route = url.pathname.slice('/__bridge'.length)
      const body = req.method === 'POST' ? await readJsonBody(req) : Object.fromEntries(url.searchParams)
      const payload = await handleBridge(route, body)
      sendJson(res, 200, payload)
      return
    }

    await serveStatic(req, res, url.pathname)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    console.error('[bridge]', message)
    if (!res.headersSent) {
      sendJson(res, 500, { error: message })
    } else {
      res.end()
    }
  }
})

function lanAddresses() {
  const addresses = []
  for (const entries of Object.values(os.networkInterfaces())) {
    for (const entry of entries || []) {
      if (entry.family === 'IPv4' && !entry.internal) addresses.push(entry.address)
    }
  }
  return addresses
}

server.on('error', (error) => {
  if (error && error.code === 'EADDRINUSE') {
    console.error(`端口 ${opts.port} 已被占用，换一个 --port 再试`)
  } else {
    console.error('服务启动失败:', error)
  }
  process.exit(1)
})

server.listen(opts.port, opts.host, async () => {
  const device = await detectDevice()
  const webrootReady = fs.existsSync(path.join(opts.root, 'index.html'))
  const shownHost = opts.host === '0.0.0.0' || opts.host === '::' ? '127.0.0.1' : opts.host

  console.log('DevMask WebUI 外部访问服务')
  console.log(`  静态目录: ${opts.root}${webrootReady ? '' : '（缺少 index.html，请先构建 webui）'}`)
  console.log(`  本机访问: http://${shownHost}:${opts.port}`)
  for (const address of lanAddresses()) {
    console.log(`  局域网:   http://${address}:${opts.port}`)
  }
  console.log(
    device
      ? `  设备:     ${device.serial}（${device.model || '未知机型'} / Android ${device.android || '?'}）`
      : `  设备:     未连接（页面可以打开，但读写配置/应用列表会失败）`,
  )
  console.log('  停止服务: Ctrl+C')

  // 预热包信息缓存，避免第一次打开应用页时同步等待 dumpsys
  if (device) {
    packageDumpMap().catch(() => {})
    listPackages('all').catch(() => {})
  }
})

process.on('SIGTERM', () => {
  server.close(() => process.exit(0))
})

process.on('SIGINT', () => {
  console.log('\n已停止')
  server.close(() => process.exit(0))
})
