#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask
//
// 运行时断言（t8 review R1）：apply 的 bulk / assignments 请求体都不得出现 packages。
// 直接以 Node 的类型剥离运行 TypeScript helper：npm run check:template-payload
// Node >= 23.6 默认支持；较早版本可加 --experimental-strip-types。

import assert from 'node:assert/strict'
import { templateConfigForApply, templateRefForApply } from '../src/utils/templatePayload.ts'

const displayConfig = {
  packages: ['com.xingin.xhs', 'com.tencent.mm@2'],
  brand: 'google',
  model: 'Pixel 9',
  android_version: '17',
  sdk_int: 37,
  dpi: 420,
}

const payload = templateConfigForApply(displayConfig)
assert.ok(!('packages' in payload), 'apply config 不应包含 packages')
assert.deepEqual(payload, {
  brand: 'google',
  model: 'Pixel 9',
  android_version: '17',
  sdk_int: 37,
  dpi: 420,
})
assert.ok('packages' in displayConfig, '不得修改入参（展示层仍需要 packages）')

const ref = templateRefForApply('pixel_9', displayConfig)
assert.equal(ref.name, 'pixel_9')
assert.ok(!('packages' in ref.config), 'TemplateRef.config 不应包含 packages')
assert.deepEqual(templateConfigForApply({ model: 'X' }), { model: 'X' })
assert.deepEqual(templateConfigForApply({}), {})

// 与 store 里两条 apply 路径一致的请求体形状
const bulkBody = {
  app: 'com.xingin.xhs',
  devices: 'all',
  template: templateRefForApply('pixel_9', displayConfig),
  restart: true,
}
const assignmentsBody = {
  assignments: [
    {
      device: 'xiaomi-15',
      app: 'com.tencent.mm',
      template: templateRefForApply('pixel_9', displayConfig),
    },
  ],
  restart: false,
}
for (const [label, body] of [
  ['bulk', bulkBody],
  ['assignments', assignmentsBody],
]) {
  const json = JSON.stringify(body)
  assert.ok(!json.includes('packages'), `${label} apply body 不得包含 packages: ${json}`)
  assert.ok(!json.includes('com.xingin.xhs') || label === 'bulk', `${label} apply body 不得携带展示性 packages 内容`)
}

console.log('OK: bulk/assignments apply body has no packages; input config untouched')
