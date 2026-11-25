// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask
//
// 浏览器侧的 KernelSU 宿主接口垫片：由 tools/webui_remote/server.mjs 注入到
// WebUI 的 index.html 里，在 Vue 应用加载前定义 window.ksu，把 exec / io / pm /
// moduleInfo 转发到桥接服务（桥接服务再用 adb 在设备上执行）。
//
// 只用于外部浏览器调试，不参与模块打包。

;(() => {
  'use strict'

  const BASE = '/__bridge'
  const ICON_PREFIX = 'ksu://icon/'

  // 同步请求：KernelSU 的 getPackagesInfo / pm.listPackages / moduleInfo 在宿主侧
  // 都是同步接口，垫片只能用同步 XHR 保持同样的调用语义。
  function request(pathname, payload) {
    const xhr = new XMLHttpRequest()
    xhr.open('POST', BASE + pathname, false)
    xhr.setRequestHeader('Content-Type', 'application/json')

    try {
      xhr.send(JSON.stringify(payload || {}))
    } catch (error) {
      throw new Error(pathname + ' 请求失败: ' + (error && error.message ? error.message : error))
    }

    if (xhr.status < 200 || xhr.status >= 300) {
      let message = xhr.responseText || 'HTTP ' + xhr.status
      try {
        const parsed = JSON.parse(message)
        if (parsed && parsed.error) message = parsed.error
      } catch {
        /* 非 JSON 错误响应 */
      }
      throw new Error(pathname + ': ' + message)
    }

    return xhr.responseText ? JSON.parse(xhr.responseText) : {}
  }

  function requestAsync(pathname, payload) {
    return fetch(BASE + pathname, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload || {}),
    }).then((response) => {
      return response.text().then((text) => {
        let data = {}
        try {
          data = text ? JSON.parse(text) : {}
        } catch {
          data = { error: text }
        }
        if (!response.ok) {
          throw new Error(data.error || 'HTTP ' + response.status)
        }
        return data
      })
    })
  }

  // ── toast ───────────────────────────────────────────────────────────────

  function showToast(message) {
    try {
      const host = document.getElementById('__df_bridge_toast') || document.createElement('div')
      host.id = '__df_bridge_toast'
      host.style.cssText =
        'position:fixed;left:50%;bottom:24px;transform:translateX(-50%);z-index:2147483647;' +
        'display:flex;flex-direction:column;gap:8px;align-items:center;pointer-events:none'
      if (!host.parentNode) {
        ;(document.body || document.documentElement).appendChild(host)
      }

      const item = document.createElement('div')
      item.textContent = String(message)
      item.style.cssText =
        'background:rgba(17,24,39,.92);color:#fff;padding:8px 14px;border-radius:8px;' +
        'font-size:13px;line-height:1.4;max-width:80vw;box-shadow:0 6px 20px rgba(0,0,0,.25)'
      host.appendChild(item)
      setTimeout(() => item.remove(), 2600)
    } catch {
      console.log(message)
    }
  }

  // ── exec ────────────────────────────────────────────────────────────────

  function exec(command, optionsJson, callbackName) {
    let options = {}
    try {
      options = optionsJson ? JSON.parse(optionsJson) : {}
    } catch {
      options = {}
    }

    const finish = (errno, stdout, stderr) => {
      const callback = window[callbackName]
      if (typeof callback === 'function') callback(errno, stdout, stderr)
    }

    requestAsync('/exec', { command: command, options: options })
      .then((result) => finish(result.errno, result.stdout, result.stderr))
      .catch((error) => finish(1, '', String(error && error.message ? error.message : error)))
  }

  // ── 包信息 ──────────────────────────────────────────────────────────────

  function getPackagesInfo(packagesJson) {
    let packages = []
    try {
      packages = JSON.parse(packagesJson)
    } catch {
      packages = []
    }
    return JSON.stringify(request('/packages/info', { packages: packages }))
  }

  function listPackages(type) {
    return JSON.stringify(request('/packages', { type: type || 'all' }))
  }

  function pm() {
    return {
      version: () => 1,
      listPackages: (type) => listPackages(type),
      getPackagesInfo: (json) => getPackagesInfo(json),
    }
  }

  function moduleInfo() {
    return JSON.stringify(request('/module-info', {}))
  }

  // ── 文件 IO ─────────────────────────────────────────────────────────────

  const io = {
    version: () => 1,
    File: (filePath) => ({
      exists: () => Boolean(request('/io/stat', { path: filePath, kind: 'exists' }).ok),
      isFile: () => Boolean(request('/io/stat', { path: filePath, kind: 'isFile' }).ok),
      isDirectory: () => Boolean(request('/io/stat', { path: filePath, kind: 'isDirectory' }).ok),
      mkdirs: () => {
        request('/io/mkdirs', { path: filePath })
      },
      delete: () => Boolean(request('/io/delete', { path: filePath }).ok),
      renameTo: (destination) => Boolean(request('/io/rename', { from: filePath, to: destination }).ok),
    }),
    FileInputStream: () => ({
      open: (filePath) => {
        const result = request('/io/open-read', { path: filePath })
        return result.id || null
      },
      read: (id) => request('/io/read', { id: id }).chunk,
      close: (id) => {
        request('/io/close', { id: id })
      },
    }),
    FileOutputStream: () => ({
      open: (filePath, append) => {
        const result = request('/io/open-write', { path: filePath, append: Boolean(append) })
        return result.id || null
      },
      write: (id, data) => Boolean(request('/io/write', { id: id, data: data }).ok),
      flush: () => true,
      close: (id) => {
        request('/io/close-write', { id: id })
      },
    }),
  }

  // ── 图标协议改写 ────────────────────────────────────────────────────────
  // 宿主用 ksu://icon/<包名> 由 PackageManager 出图；浏览器不认这个 scheme，
  // 改写成桥接服务的 HTTP 地址（当前实现返回 404，由 WebUI 回退到占位图标）。

  function rewriteIcon(node) {
    if (!node || node.nodeType !== 1) return
    const image = node.tagName === 'IMG' ? node : node.querySelector && node.querySelector('img[src^="ksu://icon/"]')
    if (!image) return
    const source = image.getAttribute('src') || ''
    if (!source.startsWith(ICON_PREFIX)) return
    const packageName = source.slice(ICON_PREFIX.length).split('?')[0]
    image.setAttribute('src', BASE + '/icon/' + encodeURIComponent(packageName))
  }

  function watchIcons() {
    const scan = (root) => {
      rewriteIcon(root)
      if (root.querySelectorAll) {
        root.querySelectorAll('img[src^="ksu://icon/"]').forEach(rewriteIcon)
      }
    }

    scan(document)

    const observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        if (mutation.type === 'attributes') {
          rewriteIcon(mutation.target)
        } else {
          mutation.addedNodes.forEach(scan)
        }
      }
    })

    observer.observe(document.documentElement, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['src'],
    })
  }

  // ── 暴露给 kernelsu-alt ─────────────────────────────────────────────────

  window.ksu = {
    exec: exec,
    getPackagesInfo: getPackagesInfo,
    listPackages: listPackages,
    pm: pm,
    io: io,
    moduleInfo: moduleInfo,
    toast: showToast,
    fullScreen: () => {},
    enableEdgeToEdge: () => {},
    enableInsets: () => {},
    exit: () => window.close(),
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', watchIcons, { once: true })
  } else {
    watchIcons()
  }

  document.documentElement.dataset.dfBridge = '1'
})()
