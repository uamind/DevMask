// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import Components from 'unplugin-vue-components/vite'
import { ElementPlusResolver } from 'unplugin-vue-components/resolvers'
import { resolve } from 'path'

// Relay 默认监听 127.0.0.1:8765（manager/docs/API.md §2.1）。
// 开发 / 预览服务器把 /relay 与 /healthz 代理过去，WebUI 因此可以同源调用，无需 CORS。
const RELAY_TARGET = process.env.DFM_DEV_RELAY ?? 'http://127.0.0.1:8765'

const relayProxy = {
  '/relay': { target: RELAY_TARGET, changeOrigin: false },
  '/healthz': { target: RELAY_TARGET, changeOrigin: false },
}

export default defineConfig({
  plugins: [
    vue(),
    Components({
      resolvers: [ElementPlusResolver({ importStyle: false })],
      dts: 'src/components.d.ts',
    }),
  ],
  resolve: {
    alias: {
      '@': resolve(import.meta.dirname, 'src'),
    },
  },
  server: {
    port: 5180,
    host: '0.0.0.0',
    strictPort: true,
    proxy: relayProxy,
  },
  preview: {
    port: 5180,
    proxy: relayProxy,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    assetsDir: 'assets',
  },
})
