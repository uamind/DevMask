// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

import { defineConfig } from 'vitepress'
import zh from './zh.ts'
import en from './en.ts'

export default defineConfig({
  locales: {
    root: {
      label: '简体中文',
      lang: 'zh-CN',
      description: '按应用伪造机型与系统属性，出口网络独立可控。',
      themeConfig: zh.themeConfig
    },
    en: {
      label: 'English',
      lang: 'en-US',
      description: 'Per-app device spoofing with an independent egress network.',
      themeConfig: en.themeConfig
    }
  }
})
