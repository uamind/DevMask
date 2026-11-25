<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div>
    <div class="page-head">
      <div>
        <div class="eyebrow">AI GATEWAY</div>
        <h1>接入 AI</h1>
        <p>
          管理端本身就是一个 Relay。AI 只需要一个 Base URL 和一个 Token，
          不用知道每台设备的地址，也不用逐台保存凭据。
        </p>
      </div>
      <div class="head-actions">
        <span :class="['summary-pill', manager.relayOnline ? '' : 'warn']">
          <span
            :class="['status-dot', manager.relayOnline ? 'is-online' : 'is-offline']"
            style="width: 6px; height: 6px"
          ></span>
          {{ relayState }}
        </span>
        <el-button type="primary" @click="copy(relayBase, 'Base URL')">
          <el-icon style="margin-right: 4px"><CopyDocument /></el-icon>
          复制入口
        </el-button>
      </div>
    </div>

    <div class="relay-grid">
      <section class="relay-code-card">
        <div style="display: flex; align-items: center; gap: 10px">
          <div class="stat-icon" style="flex: 0 0 38px"><el-icon><Connection /></el-icon></div>
          <div>
            <h2>一个地址，控制所有设备</h2>
            <p style="margin: 3px 0 0">按设备、按应用或全量下发，失败自动带 requestId。</p>
          </div>
        </div>

        <div class="copy-row" style="margin-top: 16px">
          <span class="label">BASE URL</span>
          <code>{{ relayBase }}</code>
          <el-button link type="primary" @click="copy(relayBase, 'Base URL')">复制</el-button>
        </div>
        <div class="copy-row">
          <span class="label">TOKEN</span>
          <code>{{ manager.tokenVisible ? manager.token : maskedToken }}</code>
          <el-button link type="primary" @click="manager.tokenVisible = !manager.tokenVisible">
            <el-icon><component :is="manager.tokenVisible ? Hide : View" /></el-icon>
          </el-button>
          <el-button link type="primary" @click="copy(manager.token, 'Token')">复制</el-button>
        </div>

        <pre class="code-block"><span class="cmt"># 1. 看有哪些设备</span>
curl -s {{ relayBase }}/devices \
  -H <span class="str">"Authorization: Bearer $DFM_TOKEN"</span>

<span class="cmt"># 2. 给所有设备的小红书统一成 pixel_9 模板</span>
curl -s -X POST {{ relayBase }}/apply \
  -H <span class="str">"Authorization: Bearer $DFM_TOKEN"</span> \
  -H <span class="str">"Content-Type: application/json"</span> \
  -d <span class="str">'{"app":"com.xingin.xhs","devices":"all","template":{"name":"pixel_9","config":{"model":"Pixel 9"}}}'</span>

<span class="cmt"># 3. 只读某台设备缓存的 overview</span>
curl -s "{{ relayBase }}/devices/pixel-9/overview?cached=1" \
  -H <span class="str">"Authorization: Bearer $DFM_TOKEN"</span>

<span class="cmt"># 4. 透传设备端原 API（例：清除应用配置）</span>
curl -s -X DELETE "{{ relayBase }}/devices/pixel-9/proxy/api/v1/apps/com.xingin.xhs?confirm=true" \
  -H <span class="str">"Authorization: Bearer $DFM_TOKEN"</span></pre>

        <div style="display: flex; gap: 8px; margin-top: 14px">
          <el-button :loading="manager.rotatingToken" @click="regenerate">
            <el-icon v-if="!manager.rotatingToken" style="margin-right: 4px"><Refresh /></el-icon>
            重新生成 Token
          </el-button>
          <el-button type="primary" plain @click="copy(manager.token, 'Token')">
            复制 Token
          </el-button>
        </div>
        <div class="hint" style="margin-top: 9px">
          Token 轮换后旧值当场失效，所有 AI / 脚本需要同步更新。
        </div>
      </section>

      <section class="diagram">
        <h2>路由示意</h2>
        <p>AI 看到的是一台“超级设备”，Relay 负责拆到真实设备。</p>

        <div class="diagram-node" style="border-color: #dcd6ff; background: #faf9ff">
          <div class="stat-icon" style="width: 34px; height: 34px; flex-basis: 34px; font-size: 16px">
            <el-icon><MagicStick /></el-icon>
          </div>
          <div>
            <strong>AI / Agent</strong>
            <span>只保存一个 Relay Token</span>
          </div>
        </div>

        <div class="diagram-arrow">
          <el-icon><Bottom /></el-icon>
        </div>

        <div class="diagram-node" style="border-color: #c9efda; background: #f7fdf9">
          <div class="stat-icon green" style="width: 34px; height: 34px; flex-basis: 34px; font-size: 16px">
            <el-icon><Connection /></el-icon>
          </div>
          <div>
            <strong>DevMask Relay</strong>
            <span>127.0.0.1:8765 · 鉴权 / 路由 / 重试 / 审计</span>
          </div>
        </div>

        <div class="diagram-arrow">
          <el-icon><Bottom /></el-icon>
        </div>

        <div class="diagram-devices">
          <div v-for="device in manager.devices" :key="device.id" class="diagram-device">
            <span :class="['status-dot', device.online ? 'is-online' : 'is-offline']"
              style="width: 6px; height: 6px; flex-basis: 6px"></span>
            <span style="flex: 1">{{ device.name }}</span>
            <span style="color: #9aa0b2">{{ device.online ? device.model ?? '未识别' : '离线' }}</span>
          </div>
          <div v-if="!manager.devices.length" class="diagram-device">
            <span style="color: #9aa0b2">还没有注册设备</span>
          </div>
        </div>

        <div class="preview-note" style="margin: 12px 0 0">
          <el-icon style="margin-top: 1px"><Lock /></el-icon>
          <span>设备 Token 加密存在 Relay 服务端（AES-256-GCM），AI 永远不接触。</span>
        </div>
      </section>
    </div>

    <section class="endpoint-table">
      <div class="panel-head">
        <div>
          <h2>Relay API</h2>
          <p>管理端自己的 /relay/api/v1（API v1，与每台设备的 /api/v1 路径保持兼容）</p>
        </div>
        <span class="model-chip">v1</span>
      </div>
      <el-table :data="endpoints" style="width: 100%">
        <el-table-column label="方法" width="90">
          <template #default="{ row }">
            <span :class="['method-tag', row.method.toLowerCase()]">{{ row.method }}</span>
          </template>
        </el-table-column>
        <el-table-column prop="path" label="路径" width="360">
          <template #default="{ row }">
            <code style="font-size: 10.5px; color: #3c366b">{{ row.path }}</code>
          </template>
        </el-table-column>
        <el-table-column prop="description" label="说明" />
        <el-table-column label="作用域" width="110">
          <template #default="{ row }">
            <span class="tag">{{ row.scope }}</span>
          </template>
        </el-table-column>
      </el-table>
    </section>

    <div class="dashboard-grid lower" style="margin-top: 16px">
      <section class="panel">
        <div class="panel-head">
          <div>
            <h2>三种路由方式</h2>
            <p>同一个 /apply 接口，按 devices 字段决定范围</p>
          </div>
        </div>
        <div class="drift-list">
          <div class="drift-row">
            <div class="activity-icon"><el-icon><Grid /></el-icon></div>
            <div class="drift-main">
              <strong>全量：所有设备</strong>
              <span><code>{"devices":"all","app":"com.xingin.xhs","template":{"name":"pixel_9","config":{…}}}</code></span>
            </div>
            <span class="model-chip">all</span>
          </div>
          <div class="drift-row">
            <div class="activity-icon"><el-icon><Monitor /></el-icon></div>
            <div class="drift-main">
              <strong>指定设备：按 id 数组</strong>
              <span><code>{"devices":["pixel-9","xiaomi-15"],"app":…,"template":…}</code></span>
            </div>
            <span class="model-chip">ids</span>
          </div>
          <div class="drift-row">
            <div class="activity-icon"><el-icon><Connection /></el-icon></div>
            <div class="drift-main">
              <strong>单设备代理：保留原 API</strong>
              <span><code>ANY /devices/pixel-9/proxy/api/v1/{*path}</code></span>
            </div>
            <span class="model-chip">proxy</span>
          </div>
        </div>
      </section>

      <section class="panel">
        <div class="panel-head">
          <div>
            <h2>Token 与审计</h2>
            <p>Manager Token 支持轮换；设备 Token 只加密存服务端</p>
          </div>
        </div>
        <div class="relay-panel-body">
          <div class="info-grid">
            <div class="info-box">
              <span>审计记录</span>
              <strong>{{ manager.auditTotal }} 条</strong>
            </div>
            <div class="info-box">
              <span>失败记录</span>
              <strong>{{ failedLogs }} 条</strong>
            </div>
            <div class="info-box">
              <span>24h 下发</span>
              <strong>{{ jobsLast24h }} 次</strong>
            </div>
            <div class="info-box">
              <span>在线设备</span>
              <strong>{{ manager.onlineCount }} / {{ manager.deviceCount }}</strong>
            </div>
          </div>
          <div class="preview-note" style="margin: 13px 0 0">
            <el-icon style="margin-top: 1px"><InfoFilled /></el-icon>
            <span>真实后端会把每次 /apply 拆成逐设备调用，并记录 requestId、状态码与耗时。</span>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import {
  Bottom,
  Connection,
  CopyDocument,
  Grid,
  Hide,
  InfoFilled,
  Lock,
  MagicStick,
  Monitor,
  Refresh,
  View,
} from '@element-plus/icons-vue'
import { describeError } from '../api/client'
import { useManagerStore } from '../stores/manager'
import { copyText } from '../utils/clipboard'

const manager = useManagerStore()
const relayBase = 'http://127.0.0.1:8765/relay/api/v1'

const maskedToken = computed(() => {
  const token = manager.token
  if (!token) return '未设置'
  if (token.length <= 8) return '••••••••'
  return `${token.slice(0, 4)}••••${token.slice(-4)}`
})

const relayState = computed(() => {
  if (manager.relayOnline) return `Relay 在线 · v${manager.health?.version ?? '--'}`
  return 'Relay 未连接'
})

const failedLogs = computed(
  () => manager.logs.filter((entry) => entry.status === 'error').length
)
const jobsLast24h = computed(() => manager.overview?.fleet.jobs.last24h ?? 0)

const endpoints = [
  {
    method: 'GET',
    path: '/relay/api/v1/healthz',
    description: '存活、版本、DB 状态与设备计数（无需鉴权）',
    scope: 'public',
  },
  {
    method: 'GET',
    path: '/relay/api/v1/devices',
    description: '设备注册表：在线状态、最后同步、延迟',
    scope: 'read',
  },
  {
    method: 'POST',
    path: '/relay/api/v1/devices',
    description: '添加设备（baseUrl + token），先探测 /api/v1/status 与 /overview',
    scope: 'admin',
  },
  {
    method: 'PATCH',
    path: '/relay/api/v1/devices/{id}',
    description: '改名 / 改标签 / 换 Token / 换地址（重新探测）',
    scope: 'admin',
  },
  {
    method: 'DELETE',
    path: '/relay/api/v1/devices/{id}',
    description: '只从管理端移除，不动远端配置',
    scope: 'admin',
  },
  {
    method: 'POST',
    path: '/relay/api/v1/devices/{id}/sync',
    description: '实时拉取 status + overview，离线返回 200 + warning',
    scope: 'write',
  },
  {
    method: 'POST',
    path: '/relay/api/v1/devices/sync',
    description: '并发同步全部设备',
    scope: 'write',
  },
  {
    method: 'GET',
    path: '/relay/api/v1/devices/{id}/overview',
    description: '单设备 overview（默认实时，cached=1 只读缓存）',
    scope: 'read',
  },
  {
    method: 'GET',
    path: '/relay/api/v1/templates',
    description: '跨设备模板并集与冲突警告',
    scope: 'read',
  },
  {
    method: 'GET',
    path: '/relay/api/v1/apps',
    description: '应用 × 设备矩阵与配置漂移',
    scope: 'read',
  },
  {
    method: 'POST',
    path: '/relay/api/v1/apply',
    description: 'bulk 或 assignments 下发，返回 202 + jobId',
    scope: 'write',
  },
  {
    method: 'GET',
    path: '/relay/api/v1/jobs/{id}',
    description: 'job 状态与逐设备 steps / 错误码',
    scope: 'read',
  },
  {
    method: 'GET',
    path: '/relay/api/v1/audit',
    description: '审计日志（添加 / 同步 / 下发 / 轮换 / 鉴权失败）',
    scope: 'read',
  },
  {
    method: 'POST',
    path: '/relay/api/v1/token/rotate',
    description: '轮换 Manager Token，新值只在响应中出现一次',
    scope: 'admin',
  },
  {
    method: 'ANY',
    path: '/relay/api/v1/devices/{id}/proxy/api/v1/{*path}',
    description: '单设备 /api/v1 原样透传（Relay 注入设备 Token）',
    scope: 'proxy',
  },
]

async function copy(text: string, label: string): Promise<void> {
  const copied = await copyText(text)
  ElMessage[copied ? 'success' : 'warning'](copied ? `${label}已复制` : '复制失败')
}

async function regenerate(): Promise<void> {
  try {
    await ElMessageBox.confirm(
      '重新生成后旧 Token 立即失效，所有 AI / 脚本需要更新配置。继续？',
      '重新生成 Relay Token',
      { confirmButtonText: '生成', cancelButtonText: '取消', type: 'warning' }
    )
  } catch {
    return
  }
  try {
    const result = await manager.regenerateToken()
    if (result.tokenPersisted) {
      ElMessage.success('新 Token 已生成并写入 manager.token')
    } else {
      ElMessage.warning('新 Token 已生效，但未能写入 manager.token（重启后失效）')
    }
  } catch (cause) {
    ElMessage.error(describeError(cause, '轮换 Token 失败'))
  }
}
</script>
