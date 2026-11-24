<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  Device Faker: https://github.com/Seyud/device_faker
-->

<template>
  <div>
    <div class="page-head">
      <div>
        <div class="eyebrow">CONTROL PLANE</div>
        <h1>设备中转站</h1>
        <p>
          把多台 Device Faker 设备收进一个控制面。AI 只认一个 Relay 地址，
          就能按设备、按应用或全量下发统一的伪装机型。
        </p>
      </div>
      <div class="head-actions">
        <el-button @click="router.push('/batch')">
          <el-icon style="margin-right: 4px"><MagicStick /></el-icon>
          批量统一机型
        </el-button>
        <el-button type="primary" @click="router.push('/devices?add=1')">
          <el-icon style="margin-right: 4px"><Plus /></el-icon>
          添加设备
        </el-button>
      </div>
    </div>

    <div v-if="manager.loading && !manager.deviceCount" class="panel empty-state">
      <div class="empty-icon"><el-icon class="is-loading"><Loading /></el-icon></div>
      <span>正在加载 Relay 数据…</span>
    </div>

    <template v-else>
      <div class="stat-grid">
        <div class="stat-card">
          <div class="stat-icon green"><el-icon><Monitor /></el-icon></div>
          <div class="stat-meta">
            <span>设备在线</span>
            <strong>{{ manager.onlineCount }}<small>/ {{ manager.deviceCount }} 台</small></strong>
            <span class="stat-trend up">
              <el-icon><CircleCheck /></el-icon>
              {{ manager.offlineCount }} 台离线待处理
            </span>
          </div>
        </div>
        <div class="stat-card">
          <div class="stat-icon"><el-icon><Grid /></el-icon></div>
          <div class="stat-meta">
            <span>纳管应用</span>
            <strong>{{ manager.appCount }}<small>个</small></strong>
            <span class="stat-trend">共 {{ manager.configs.length }} 条设备配置</span>
          </div>
        </div>
        <div class="stat-card">
          <div class="stat-icon blue"><el-icon><MagicStick /></el-icon></div>
          <div class="stat-meta">
            <span>可用模板</span>
            <strong>{{ manager.templates.length }}<small>个</small></strong>
            <span class="stat-trend">跨设备模板并集（/templates）</span>
          </div>
        </div>
        <div class="stat-card">
          <div class="stat-icon amber"><el-icon><Warning /></el-icon></div>
          <div class="stat-meta">
            <span>配置漂移</span>
            <strong>{{ manager.driftRows.length }}<small>个应用</small></strong>
            <span class="stat-trend warn">
              <el-icon><Warning /></el-icon>
              机型不一致或缺少配置
            </span>
          </div>
        </div>
      </div>

      <div class="dashboard-grid">
        <section class="panel">
          <div class="panel-head">
            <div>
              <h2>设备</h2>
              <p>点击设备查看指纹、应用配置和连接信息</p>
            </div>
            <router-link to="/devices" class="panel-link">
              全部设备
              <el-icon><ArrowRight /></el-icon>
            </router-link>
          </div>
          <div v-if="manager.deviceCount" class="device-list">
            <div
              v-for="device in manager.devices"
              :key="device.id"
              class="device-row"
              @click="openDevice(device.id)"
            >
              <span :class="['status-dot', device.online ? 'is-online' : 'is-offline']"></span>
              <div class="device-avatar"><el-icon><Iphone /></el-icon></div>
              <div class="device-main">
                <strong>{{ device.name }}</strong>
                <span>
                  {{ device.model ?? '未识别' }} · Android {{ device.android ?? '--' }} ·
                  {{ device.baseUrl }}
                </span>
              </div>
              <div class="device-metrics">
                <span>{{ manager.appCountForDevice(device.id) }} 应用</span>
                <span>{{ device.online ? relativeTime(device.lastSeen) : '离线 · ' + relativeTime(device.lastSeen) }}</span>
              </div>
              <el-icon class="row-arrow"><ArrowRight /></el-icon>
            </div>
          </div>
          <div v-else class="empty-state">
            <div class="empty-icon"><el-icon><Monitor /></el-icon></div>
            <span>还没有注册设备，添加一台即可开始</span>
            <el-button type="primary" plain @click="router.push('/devices?add=1')">添加设备</el-button>
          </div>
        </section>

        <section class="panel">
          <div class="panel-head">
            <div>
              <h2>Relay 入口</h2>
              <p>AI 只需要拿到这个地址和 Token</p>
            </div>
            <span class="model-chip">统一入口</span>
          </div>
          <div class="relay-panel-body">
            <div class="relay-card">
              <div class="relay-label">API BASE</div>
              <code class="relay-endpoint">{{ relayBase }}</code>
              <div class="relay-token">
                <span>TOKEN</span>
                <code>{{ maskedToken }}</code>
              </div>
              <div class="relay-actions">
                <el-button @click="copy(relayBase, '地址')">
                  <el-icon style="margin-right: 3px"><CopyDocument /></el-icon>
                  复制地址
                </el-button>
                <el-button type="primary" plain @click="router.push('/relay')">
                  接入文档
                </el-button>
              </div>
            </div>
            <div class="side-metrics">
              <div class="side-metric">
                <span>设备配置总数</span>
                <strong>{{ manager.configs.length }}</strong>
              </div>
              <div class="side-metric">
                <span>在线设备</span>
                <strong>{{ manager.onlineCount }} / {{ manager.deviceCount }}</strong>
              </div>
              <div class="side-metric">
                <span>24h 下发 / 失败</span>
                <strong>{{ jobsLast24h }} / {{ jobsFailedLast24h }}</strong>
              </div>
              <div class="side-metric">
                <span>平均延迟</span>
                <strong>{{ averageLatency }} ms</strong>
              </div>
            </div>
          </div>
        </section>
      </div>

      <div class="dashboard-grid lower">
        <section class="panel">
          <div class="panel-head">
            <div>
              <h2>配置漂移</h2>
              <p>同一个应用在不同设备上的机型不一致，或部分设备未配置</p>
            </div>
            <router-link to="/apps" class="panel-link">
              应用矩阵
              <el-icon><ArrowRight /></el-icon>
            </router-link>
          </div>
          <div v-if="manager.driftRows.length" class="drift-list">
            <div v-for="row in manager.driftRows.slice(0, 3)" :key="row.app.id" class="drift-row">
              <div class="app-badge" :style="{ background: row.app.color }">
                {{ row.app.short }}
              </div>
              <div class="drift-main">
                <strong>{{ row.app.name }}</strong>
                <span>
                  {{ row.configured }}/{{ row.total }} 台已配置 ·
                  {{ row.models.length }} 种机型
                </span>
              </div>
              <span v-for="model in row.models.slice(0, 2)" :key="model" class="model-chip warn">
                {{ model }}
              </span>
              <el-button link type="primary" @click="goBatch(row.app.package)">
                去统一
              </el-button>
            </div>
          </div>
          <div v-else class="empty-state">
            <div class="empty-icon"><el-icon><CircleCheck /></el-icon></div>
            <span>所有已配置应用机型一致</span>
          </div>
        </section>

        <section class="panel">
          <div class="panel-head">
            <div>
              <h2>最近操作</h2>
              <p>来自 /audit 的添加、同步与批量下发记录</p>
            </div>
            <router-link to="/logs" class="panel-link">
              全部
              <el-icon><ArrowRight /></el-icon>
            </router-link>
          </div>
          <div v-if="manager.recentLogs.length" class="activity-list">
            <div v-for="entry in manager.recentLogs" :key="entry.id" class="activity-item">
              <div :class="['activity-icon', entry.status]">
                <el-icon><component :is="activityIcon(entry.type)" /></el-icon>
              </div>
              <div class="activity-main">
                <strong>{{ entry.title }}</strong>
                <span>{{ entry.detail }}</span>
              </div>
              <div class="activity-time">{{ entry.time }}</div>
            </div>
          </div>
          <div v-else class="empty-state">
            <div class="empty-icon"><el-icon><Document /></el-icon></div>
            <span>还没有审计记录</span>
          </div>
        </section>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import {
  ArrowRight,
  CircleCheck,
  CopyDocument,
  Delete,
  Document,
  Grid,
  Iphone,
  Loading,
  MagicStick,
  Monitor,
  Plus,
  Refresh,
  Warning,
} from '@element-plus/icons-vue'
import { useManagerStore } from '../stores/manager'
import { copyText } from '../utils/clipboard'
import { relativeTime } from '../utils/format'
import type { LogEntry } from '../types'

const router = useRouter()
const manager = useManagerStore()
const relayBase = 'http://127.0.0.1:8765/relay/api/v1'

const maskedToken = computed(() => {
  const token = manager.token
  if (!token) return '未设置'
  if (token.length <= 8) return '••••••••'
  return `${token.slice(0, 4)}••••${token.slice(-4)}`
})

const averageLatency = computed(() => {
  const value = manager.averageLatencyMs
  return value === null ? '--' : String(value)
})

const jobsLast24h = computed(() => manager.overview?.fleet.jobs.last24h ?? 0)
const jobsFailedLast24h = computed(() => manager.overview?.fleet.jobs.failedLast24h ?? 0)

function activityIcon(type: LogEntry['type']) {
  if (type === 'error') return Warning
  if (type === 'add') return Plus
  if (type === 'remove') return Delete
  if (type === 'batch') return MagicStick
  return Refresh
}

function openDevice(id: string): void {
  router.push({ path: '/devices', query: { device: id } })
}

function goBatch(appPackage: string): void {
  router.push({ path: '/batch', query: { app: appPackage } })
}

async function copy(text: string, label: string): Promise<void> {
  const copied = await copyText(text)
  ElMessage[copied ? 'success' : 'warning'](copied ? `${label}已复制` : '复制失败，请手动选择')
}
</script>
