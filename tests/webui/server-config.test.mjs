// SPDX-License-Identifier: GPL-3.0-or-later
// server.toml 纯函数验收（t13 外部访问设置）。运行：
//   node --experimental-strip-types tests/webui/server-config.test.mjs

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import {
  appendTokenBlock,
  applyServerSettings,
  formatLanUrls,
  isValidPort,
  normalizePort,
  parseDhcpProps,
  parseIpAddrOutput,
  parseServerSettings,
  stripTokenBlocks,
} from '../../webui/src/utils/serverConfig.ts'

let passed = 0
function test(name, fn) {
  fn()
  passed += 1
  console.log('PASS ' + name)
}

const defaultTemplate = readFileSync(new URL('../../module/server.default.toml', import.meta.url), 'utf8')

test('空文本使用 loopback:8686 默认值', () => {
  const parsed = parseServerSettings('')
  assert.equal(parsed.bind, '127.0.0.1:8686')
  assert.equal(parsed.host, '127.0.0.1')
  assert.equal(parsed.port, 8686)
  assert.equal(parsed.allowLan, false)
  assert.equal(parsed.hasToken, false)
})

test('解析默认模板', () => {
  const parsed = parseServerSettings(defaultTemplate)
  assert.equal(parsed.bind, '127.0.0.1:8686')
  assert.equal(parsed.allowLan, false)
  assert.equal(parsed.tokenCount, 0)
})

test('开启外部访问保留注释/未知字段/Token 块', () => {
  const source = '# keep me\nunknown_field = \'x\'\n\nbind = "127.0.0.1:8686"\nallow_lan = false\n\n[[tokens]]\nid = "a"\nhash = "sha256:aa"\n'
  const out = applyServerSettings(source, { allowLan: true, port: 18901 })
  assert.match(out, /bind = "0\.0\.0\.0:18901"/)
  assert.match(out, /allow_lan = true/)
  assert.match(out, /# keep me/)
  assert.match(out, /unknown_field = 'x'/)
  assert.match(out, /\[\[tokens\]\]/)
  assert.match(out, /hash = "sha256:aa"/)
  assert.equal(parseServerSettings(out).allowLan, true)
  assert.equal(parseServerSettings(out).port, 18901)
})

test('关闭外部访问改回 loopback 且保留 Token', () => {
  const source = 'bind = "0.0.0.0:9000"\nallow_lan = true\n\n[[tokens]]\nid = "ui"\nhash = "sha256:bb"\n'
  const out = applyServerSettings(source, { allowLan: false, port: 9123 })
  assert.match(out, /bind = "127\.0\.0\.1:9123"/)
  assert.match(out, /allow_lan = false/)
  assert.equal(parseServerSettings(out).tokenCount, 1)
})

test('缺少 bind / allow_lan 时追加默认键', () => {
  const out = applyServerSettings('debug = 1\n', { allowLan: true, port: 9001 })
  assert.match(out, /bind = "0\.0\.0\.0:9001"/)
  assert.match(out, /allow_lan = true/)
  assert.match(out, /debug = 1/)
})

test('端口边界与归一化', () => {
  assert.equal(isValidPort(1), true)
  assert.equal(isValidPort(65535), true)
  assert.equal(isValidPort('8686'), true)
  assert.equal(isValidPort(0), false)
  assert.equal(isValidPort(65536), false)
  assert.equal(isValidPort('abc'), false)
  assert.equal(isValidPort(1.5), false)
  assert.equal(normalizePort(0), 8686)
  assert.equal(normalizePort(65536), 8686)
})

test('stripTokenBlocks 清空所有 Token 并保留其它表', () => {
  const source = '# head\nbind = "127.0.0.1:8686"\n\n[[tokens]]\nid = "a"\n\n[[tokens]]\nid = "b"\n\n[other]\nkeep = 1\n'
  const out = stripTokenBlocks(source)
  assert.doesNotMatch(out, /\[\[tokens\]\]/)
  assert.match(out, /# head/)
  assert.match(out, /\[other\]/)
  assert.match(out, /keep = 1/)
  assert.equal(parseServerSettings(out).tokenCount, 0)
})

test('appendTokenBlock 补齐 sha256 前缀且只追加一条', () => {
  const source = 'bind = "0.0.0.0:9000"\nallow_lan = true\n'
  const hash = 'abcdef0123456789'
  const out = appendTokenBlock(source, hash)
  assert.match(out, /\[\[tokens\]\]/)
  assert.match(out, new RegExp('hash = "sha256:' + hash + '"'))
  assert.equal(parseServerSettings(out).tokenCount, 1)
  assert.equal(parseServerSettings(out).hasToken, true)
})

test('格式局域网地址与接口解析', () => {
  const ipOutput = [
    '1: lo: <LOOPBACK,UP>',
    '    inet 127.0.0.1/8 scope host lo',
    '2: wlan0: <BROADCAST,UP>',
    '    inet 192.168.1.50/24 brd 192.168.1.255 scope global wlan0',
    '3: rmnet_data0: <UP>',
    '    inet 10.0.0.2/24 scope global rmnet_data0',
    '4: ap0: <UP>',
    '    inet 192.168.43.1/24 scope global ap0',
  ].join('\n')
  assert.deepEqual(parseIpAddrOutput(ipOutput), ['192.168.1.50', '192.168.43.1'])
  assert.deepEqual(parseIpAddrOutput('2: wlan0    inet 192.168.1.51/24 brd 192.168.1.255 scope global wlan0'), ['192.168.1.51'])
  assert.deepEqual(parseDhcpProps('[dhcp.wlan0.ipaddress]: [192.168.1.52]\n[dhcp.eth0.ipaddress]: [169.254.1.1]\n'), ['192.168.1.52'])
  assert.deepEqual(formatLanUrls(['192.168.1.50'], 9000), ['http://192.168.1.50:9000'])
})

console.log('SERVERCONFIG PASS: ' + passed + ' groups')
