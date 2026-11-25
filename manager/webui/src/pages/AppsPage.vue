<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div>
    <div class="page-head">
      <div>
        <div class="eyebrow">MATRIX</div>
        <h1>应用矩阵</h1>
        <p>每一行是一个应用，每一列是一台设备。单元格显示该设备最终生效的机型。</p>
      </div>
      <div class="head-actions">
        <el-button type="primary" @click="router.push('/batch')">
          <el-icon style="margin-right: 4px"><MagicStick /></el-icon>
          批量统一
        </el-button>
      </div>
    </div>

    <div class="apps-summary">
      <span class="summary-pill">
        <el-icon><Grid /></el-icon>
        <strong>{{ manager.appCount }}</strong> 个应用
      </span>
      <span class="summary-pill">
        <el-icon><Monitor /></el-icon>
        <strong>{{ manager.deviceCount }}</strong> 台设备
      </span>
      <span :class="['summary-pill', manager.driftRows.length ? 'warn' : '']">
        <el-icon><Warning /></el-icon>
        <strong>{{ manager.driftRows.length }}</strong> 个配置不一致
      </span>
      <span class="summary-pill">
        <el-icon><Connection /></el-icon>
        <strong>{{ manager.configs.length }}</strong> 条配置
      </span>
    </div>

    <div class="toolbar">
      <el-input
        v-model="keyword"
        class="search"
        placeholder="搜索应用名或包名"
        clearable
      >
        <template #prefix><el-icon><Search /></el-icon></template>
      </el-input>
      <el-switch
        v-model="inconsistentOnly"
        active-text="只看不一致"
        style="--el-switch-on-color: #d97706"
      />
      <span class="spacer"></span>
      <span class="toolbar-count">
        显示 {{ filteredRows.length }} / {{ manager.matrix.length }} 个应用
      </span>
    </div>

    <div v-if="manager.loading && !manager.matrix.length" class="panel empty-state">
      <div class="empty-icon"><el-icon class="is-loading"><Loading /></el-icon></div>
      <span>正在从 GET /apps 加载应用矩阵…</span>
    </div>

    <template v-else>
      <div class="apps-table-wrap">
        <el-table
          :data="filteredRows"
          :row-key="rowKey"
          class="apps-table"
          :border="false"
          table-layout="fixed"
        >
          <el-table-column label="应用" width="250" fixed>
            <template #default="{ row }">
              <div class="app-cell">
                <div class="app-badge" :style="{ background: row.app.color }">
                  {{ row.app.short }}
                </div>
                <div class="app-cell-text">
                  <strong>{{ row.app.name }}</strong>
                  <span>{{ row.app.package }}</span>
                </div>
              </div>
            </template>
          </el-table-column>

          <el-table-column
            v-for="device in manager.devices"
            :key="device.id"
            min-width="172"
          >
            <template #header>
              <div class="device-col-head">
                <span
                  :class="['status-dot', device.online ? 'is-online' : 'is-offline']"
                  style="width: 6px; height: 6px; flex-basis: 6px"
                ></span>
                <span>{{ device.name.split(' · ')[0] }}</span>
              </div>
            </template>
            <template #default="{ row }">
              <div class="cell-click" @click="openCell(row.app, device.id)">
                <div
                  v-if="row.cells[device.id]"
                  :class="['model-cell', { 'has-warning': row.cells[device.id].warning }]"
                >
                  <span class="model-name">
                    {{ row.cells[device.id].model }}
                    <el-tooltip
                      v-if="row.cells[device.id].warning"
                      :content="row.cells[device.id].warning"
                      placement="top"
                    >
                      <el-icon style="color: #d97706; vertical-align: -2px">
                        <Warning />
                      </el-icon>
                    </el-tooltip>
                  </span>
                  <span class="model-source">
                    {{ row.cells[device.id].template ?? '内联配置' }} ·
                    {{ row.cells[device.id].source }}
                  </span>
                </div>
                <span v-else class="empty-cell">未配置</span>
              </div>
            </template>
          </el-table-column>

          <el-table-column label="" width="120" fixed="right" align="right">
            <template #default="{ row }">
              <el-button link type="primary" @click="goBatch(row.app.package)">
                统一配置
                <el-icon style="margin-left: 1px"><ArrowRight /></el-icon>
              </el-button>
            </template>
          </el-table-column>

          <template #empty>
            <div class="empty-state">
              <div class="empty-icon"><el-icon><Grid /></el-icon></div>
              <span>{{ manager.matrix.length ? '没有匹配的应用' : '还没有任何已配置应用' }}</span>
              <span v-if="!manager.matrix.length" class="hint">
                先在批量配置页选择或输入包名并下发，矩阵就会出现第一行。
              </span>
            </div>
          </template>
        </el-table>
      </div>

      <div v-if="manager.matrixNextCursor" style="display: flex; justify-content: center; margin-top: 12px">
        <el-button :loading="manager.loading" @click="manager.loadMoreMatrix()">
          加载更多应用
        </el-button>
      </div>
    </template>

    <div class="apps-legend">
      <span class="legend-item">
        <span class="legend-swatch"></span>
        正常配置
      </span>
      <span class="legend-item">
        <span class="legend-swatch warn"></span>
        有警告或未安装
      </span>
      <span class="legend-item">
        <span class="empty-cell">未配置</span>
        这台设备没有该应用的配置
      </span>
      <span style="margin-left: auto">
        点击单元格可单独下发；「统一配置」会跳到批量页
      </span>
    </div>

    <el-dialog
      v-model="cellDialogVisible"
      :title="cellApp ? `配置「${cellApp.name}」` : '配置应用'"
      width="480px"
      :close-on-click-modal="false"
    >
      <div v-if="cellApp && cellDevice" class="app-cell" style="margin-bottom: 16px">
        <div class="app-badge" :style="{ background: cellApp.color }">{{ cellApp.short }}</div>
        <div class="app-cell-text">
          <strong>{{ cellApp.name }}</strong>
          <span>{{ cellDevice.name }} · {{ cellDevice.baseUrl }}</span>
        </div>
        <span v-if="cellCurrent" class="model-chip">{{ cellCurrent.model }}</span>
        <span v-else class="empty-cell">未配置</span>
      </div>

      <el-form label-position="top">
        <el-form-item label="统一为模板">
          <el-select
            v-model="cellTemplateName"
            filterable
            style="width: 100%"
            :disabled="!manager.templates.length"
            placeholder="需要至少一台设备同步过模板"
          >
            <el-option
              v-for="template in manager.templates"
              :key="template.name"
              :label="`${template.name} · ${template.model} · Android ${template.android}`"
              :value="template.name"
            />
          </el-select>
        </el-form-item>
      </el-form>

      <div class="hint">
        只影响这一台设备的这个应用；批量统一请用「统一配置」或批量配置页。
        下发会以 <code>{name, config}</code> 模板对象提交，目标设备缺少该模板时自动创建。
      </div>

      <template #footer>
        <div style="display: flex; align-items: center">
          <el-button v-if="cellCurrent" type="danger" plain :loading="manager.savingCell" @click="clearCell">
            清除配置
          </el-button>
          <span style="flex: 1"></span>
          <el-button @click="cellDialogVisible = false">取消</el-button>
          <el-button
            type="primary"
            :loading="manager.savingCell"
            :disabled="!cellTemplateName"
            @click="saveCell"
          >
            保存并下发
          </el-button>
        </div>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import {
  ArrowRight,
  Connection,
  Grid,
  Loading,
  MagicStick,
  Monitor,
  Search,
  Warning,
} from '@element-plus/icons-vue'
import { describeError } from '../api/client'
import type { Job } from '../api/types'
import { useManagerStore } from '../stores/manager'
import type { AppDef } from '../types'

const route = useRoute()
const router = useRouter()
const manager = useManagerStore()

const keyword = ref(typeof route.query.q === 'string' ? route.query.q : '')
const inconsistentOnly = ref(false)
const cellDialogVisible = ref(false)
const cellApp = ref<AppDef | null>(null)
const cellDeviceId = ref('')
const cellTemplateName = ref('')

const cellDevice = computed(() => manager.deviceById(cellDeviceId.value))
const cellCurrent = computed(() =>
  cellApp.value ? manager.configFor(cellApp.value.package, cellDeviceId.value) : undefined
)

watch(
  () => route.query.q,
  (value) => {
    if (typeof value === 'string') keyword.value = value
  }
)

const filteredRows = computed(() => {
  const query = keyword.value.trim().toLowerCase()
  return manager.matrix.filter((row) => {
    if (inconsistentOnly.value && !row.inconsistent) return false
    if (!query) return true
    return (
      row.app.name.toLowerCase().includes(query) ||
      row.app.package.toLowerCase().includes(query)
    )
  })
})

function rowKey(row: { app: { package: string } }): string {
  return row.app.package
}

function openCell(app: AppDef, deviceId: string): void {
  cellApp.value = app
  cellDeviceId.value = deviceId
  const current = manager.configFor(app.package, deviceId)
  const currentName = current?.template ?? ''
  const matched = manager.templates.some((template) => template.name === currentName)
  cellTemplateName.value = matched ? currentName : manager.templates[0]?.name ?? ''
  cellDialogVisible.value = true
}

function describeJob(job: Job): string {
  const result = job.results[0]
  if (!result) return `job ${job.id} 已创建（${job.status}）`
  if (result.status === 'ok') return '配置已下发'
  const label = result.status === 'skipped' ? '已跳过' : '下发失败'
  const reason = result.error?.message ?? result.code ?? '未知原因'
  return `${label}：${reason}`
}

async function saveCell(): Promise<void> {
  if (!cellApp.value) return
  const template = manager.templates.find((item) => item.name === cellTemplateName.value)
  if (!template) {
    ElMessage.warning('请选择模板')
    return
  }
  try {
    const job = await manager.setAppConfig(cellApp.value.package, cellDeviceId.value, template)
    cellDialogVisible.value = false
    const result = job.results[0]
    if (result && result.status !== 'ok') ElMessage.warning(describeJob(job))
    else ElMessage.success(describeJob(job))
  } catch (cause) {
    ElMessage.error(describeError(cause, '配置失败'))
  }
}

async function clearCell(): Promise<void> {
  if (!cellApp.value) return
  try {
    await manager.removeAppConfig(cellApp.value.package, cellDeviceId.value)
    cellDialogVisible.value = false
    ElMessage.success('配置已清除')
  } catch (cause) {
    ElMessage.error(describeError(cause, '清除配置失败'))
  }
}

function goBatch(appPackage: string): void {
  router.push({ path: '/batch', query: { app: appPackage } })
}
</script>
