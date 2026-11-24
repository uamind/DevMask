<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  Device Faker: https://github.com/Seyud/device_faker
-->

<template>
  <div>
    <div class="page-head">
      <div>
        <div class="eyebrow">AUDIT</div>
        <h1>操作日志</h1>
        <p>来自 GET /audit：添加、删除、同步、下发与 Token 轮换，失败项附 requestId。</p>
      </div>
      <div class="head-actions">
        <el-button v-if="manager.mockMode" @click="resetData">重置预览数据</el-button>
        <el-button :loading="manager.loadingAudit" @click="refresh">
          <el-icon v-if="!manager.loadingAudit" style="margin-right: 4px"><Refresh /></el-icon>
          刷新日志
        </el-button>
      </div>
    </div>

    <div class="apps-summary">
      <span class="summary-pill">
        <el-icon><Document /></el-icon>
        <strong>{{ manager.logs.length }}</strong> 条记录
        <template v-if="manager.auditTotal > manager.logs.length">
          （共 {{ manager.auditTotal }} 条）
        </template>
      </span>
      <span class="summary-pill">
        <el-icon><CircleCheck /></el-icon>
        <strong>{{ successCount }}</strong> 成功
      </span>
      <span :class="['summary-pill', errorCount ? 'warn' : '']">
        <el-icon><Warning /></el-icon>
        <strong>{{ errorCount }}</strong> 失败
      </span>
    </div>

    <div class="toolbar">
      <el-input v-model="keyword" class="search" placeholder="搜索操作、详情或 requestId" clearable>
        <template #prefix><el-icon><Search /></el-icon></template>
      </el-input>
      <el-select v-model="typeFilter" style="width: 140px">
        <el-option label="全部类型" value="all" />
        <el-option label="添加设备" value="add" />
        <el-option label="移除设备" value="remove" />
        <el-option label="同步" value="sync" />
        <el-option label="批量配置" value="batch" />
        <el-option label="错误" value="error" />
      </el-select>
      <span class="spacer"></span>
      <span class="toolbar-count">显示 {{ filteredLogs.length }} 条</span>
    </div>

    <div v-if="manager.loadingAudit && !manager.logs.length" class="panel empty-state">
      <div class="empty-icon"><el-icon class="is-loading"><Loading /></el-icon></div>
      <span>正在加载审计日志…</span>
    </div>

    <div v-else class="logs-table">
      <el-table :data="filteredLogs" style="width: 100%">
        <el-table-column label="类型" width="120">
          <template #default="{ row }">
            <div style="display: flex; align-items: center; gap: 7px">
              <div :class="['activity-icon', row.status]" style="width: 25px; height: 25px; flex-basis: 25px">
                <el-icon><component :is="activityIcon(row.type)" /></el-icon>
              </div>
              <span :class="['tag', row.status === 'error' ? 'warn' : '']">
                {{ typeLabel(row.type) }}
              </span>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="操作" min-width="260">
          <template #default="{ row }">
            <div class="log-title">{{ row.title }}</div>
            <div class="log-detail">{{ row.detail }}</div>
            <div v-if="row.requestId" class="log-detail mono" style="font-size: 9.5px">
              requestId: {{ row.requestId }}
            </div>
          </template>
        </el-table-column>
        <el-table-column label="状态" width="100">
          <template #default="{ row }">
            <span
              :class="['model-chip', row.status === 'error' ? 'warn' : '']"
              style="border-color: transparent"
            >
              {{ row.status === 'success' ? '成功' : row.status === 'error' ? '失败' : '信息' }}
            </span>
          </template>
        </el-table-column>
        <el-table-column prop="time" label="时间" width="140" align="right">
          <template #default="{ row }">
            <span style="color: #9aa0b2; font-size: 10.5px">{{ row.time }}</span>
          </template>
        </el-table-column>

        <template #empty>
          <div class="empty-state">
            <div class="empty-icon"><el-icon><Document /></el-icon></div>
            <span>{{ manager.logs.length ? '没有匹配的记录' : '还没有审计记录' }}</span>
          </div>
        </template>
      </el-table>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import {
  CircleCheck,
  Delete,
  Document,
  Loading,
  MagicStick,
  Plus,
  Refresh,
  Search,
  Warning,
} from '@element-plus/icons-vue'
import { useManagerStore } from '../stores/manager'
import type { LogEntry } from '../types'

const manager = useManagerStore()
const keyword = ref('')
const typeFilter = ref<'all' | LogEntry['type']>('all')

const filteredLogs = computed(() => {
  const query = keyword.value.trim().toLowerCase()
  return manager.logs.filter((entry) => {
    if (typeFilter.value !== 'all' && entry.type !== typeFilter.value) return false
    if (!query) return true
    return `${entry.title} ${entry.detail} ${entry.requestId ?? ''}`
      .toLowerCase()
      .includes(query)
  })
})

const successCount = computed(
  () => manager.logs.filter((entry) => entry.status === 'success').length
)
const errorCount = computed(
  () => manager.logs.filter((entry) => entry.status === 'error').length
)

function typeLabel(type: LogEntry['type']): string {
  return {
    add: '添加',
    remove: '移除',
    sync: '同步',
    batch: '批量',
    error: '错误',
  }[type]
}

function activityIcon(type: LogEntry['type']) {
  if (type === 'error') return Warning
  if (type === 'add') return Plus
  if (type === 'remove') return Delete
  if (type === 'batch') return MagicStick
  return Refresh
}

async function refresh(): Promise<void> {
  await manager.loadAudit()
}

async function resetData(): Promise<void> {
  try {
    await ElMessageBox.confirm(
      '会重新加载 src/mock 的示例数据（仅 VITE_MOCK=1 预览模式可用）。',
      '重置预览数据',
      { confirmButtonText: '重置', cancelButtonText: '取消', type: 'warning' }
    )
  } catch {
    return
  }
  await manager.resetPreviewData()
  ElMessage.success('预览数据已恢复')
}

onMounted(() => {
  if (!manager.logs.length) void manager.loadAudit()
})
</script>
