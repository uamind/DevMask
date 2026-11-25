<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div>
    <div class="page-head">
      <div>
        <div class="eyebrow">ROLLOUT</div>
        <h1>批量配置</h1>
        <p>
          选一个应用，再选目标设备，把同一个模板下发给所有设备。
          这就是「AI 只调一次，统一所有设备机型」的入口。
        </p>
      </div>
      <div class="head-actions">
        <el-button @click="router.push('/apps')">
          <el-icon style="margin-right: 4px"><Grid /></el-icon>
          应用矩阵
        </el-button>
      </div>
    </div>

    <div class="batch-grid">
      <div class="batch-form">
        <section class="batch-card">
          <div class="batch-card-head">
            <span class="step-number">1</span>
            <div>
              <strong>选择应用</strong>
              <span>同一个包名，在所有目标设备上统一成同一机型</span>
            </div>
          </div>
          <el-select
            v-model="selectedPackage"
            filterable
            allow-create
            default-first-option
            :reserve-keyword="false"
            placeholder="选择应用，或直接输入包名"
            style="width: 100%"
          >
            <el-option
              v-for="app in manager.appOptions"
              :key="app.package"
              :label="`${app.name}  ·  ${app.package}`"
              :value="app.package"
            >
              <div style="display: flex; align-items: center; gap: 9px">
                <span
                  class="app-badge"
                  :style="{ background: app.color, width: '24px', height: '24px', flexBasis: '24px', fontSize: '10px' }"
                >
                  {{ app.short }}
                </span>
                <span style="font-size: 12px">{{ app.name }}</span>
                <span style="margin-left: auto; color: #9aa0b2; font-size: 10px">
                  {{ app.package }}
                </span>
              </div>
            </el-option>
          </el-select>

          <div v-if="selectedApp" class="apps-summary" style="margin: 12px 0 0">
            <span class="summary-pill">
              <strong>{{ configuredCount }}</strong> / {{ manager.deviceCount }} 台已配置
            </span>
            <span
              v-for="model in currentModels"
              :key="model"
              :class="['model-chip', currentModels.length > 1 ? 'warn' : '']"
            >
              {{ model }}
            </span>
          </div>
        </section>

        <section class="batch-card">
          <div class="batch-card-head">
            <span class="step-number">2</span>
            <div>
              <strong>目标设备</strong>
              <span>全部设备，或手动勾选一部分</span>
            </div>
          </div>

          <el-radio-group v-model="targetMode" style="margin-bottom: 13px">
            <el-radio-button value="all">全部设备</el-radio-button>
            <el-radio-button value="selected">指定设备</el-radio-button>
          </el-radio-group>

          <div v-if="targetMode === 'selected'" class="target-list">
            <label
              v-for="device in manager.devices"
              :key="device.id"
              :class="[
                'target-option',
                { 'is-selected': selectedDeviceIds.includes(device.id), 'is-disabled': !device.online },
              ]"
            >
              <el-checkbox
                :model-value="selectedDeviceIds.includes(device.id)"
                :disabled="!device.online"
                @change="toggleDevice(device.id, $event)"
              />
              <span
                :class="['status-dot', device.online ? 'is-online' : 'is-offline']"
                style="width: 6px; height: 6px; flex-basis: 6px"
              ></span>
              <div class="target-main">
                <strong>{{ device.name }}</strong>
                <span>
                  {{ device.online ? device.model ?? '未识别' : '离线' }} ·
                  {{ manager.appCountForDevice(device.id) }} 个应用
                </span>
              </div>
            </label>
            <div v-if="!manager.devices.length" class="empty-state" style="padding: 20px">
              <span>还没有注册设备</span>
            </div>
          </div>

          <div v-else class="target-list">
            <div
              v-for="device in manager.devices"
              :key="device.id"
              :class="['target-option', 'is-selected', { 'is-disabled': !device.online }]"
            >
              <span
                :class="['status-dot', device.online ? 'is-online' : 'is-offline']"
                style="width: 6px; height: 6px; flex-basis: 6px"
              ></span>
              <div class="target-main">
                <strong>{{ device.name }}</strong>
                <span>{{ device.model ?? '未识别' }} · {{ device.online ? (device.latencyMs ?? '--') + ' ms' : '离线' }}</span>
              </div>
              <el-tag v-if="!device.online" size="small" type="info">离线</el-tag>
            </div>
            <div v-if="!manager.devices.length" class="empty-state" style="padding: 20px">
              <span>还没有注册设备</span>
            </div>
          </div>

          <div style="display: flex; align-items: center; gap: 10px; margin-top: 13px">
            <el-switch v-model="onlineOnly" />
            <span style="font-size: 11px; color: #858a9d">只下发给在线设备</span>
            <span style="flex: 1"></span>
            <span style="font-size: 10.5px; color: #858a9d">
              {{ targetDevices.length }} 台目标 · {{ offlineCount }} 台离线
            </span>
          </div>
        </section>

        <section class="batch-card">
          <div class="batch-card-head">
            <span class="step-number">3</span>
            <div>
              <strong>统一为</strong>
              <span>选择一个模板；模板里的指纹、DPI、SDK 会一起下发</span>
            </div>
          </div>
          <el-select
            v-model="templateName"
            filterable
            style="width: 100%"
            :disabled="!manager.templates.length"
            placeholder="需要至少一台设备同步过模板"
          >
            <el-option
              v-for="template in manager.templates"
              :key="template.name"
              :label="`${template.name}  ·  ${template.model}  ·  Android ${template.android}`"
              :value="template.name"
            >
              <div style="display: flex; align-items: center; gap: 8px">
                <span style="font-size: 12px">{{ template.name }}</span>
                <span style="margin-left: auto; color: #9aa0b2; font-size: 10px">
                  {{ template.brand }} · {{ template.dpi ?? '--' }} dpi
                </span>
              </div>
            </el-option>
          </el-select>

          <div v-if="selectedTemplate" class="target-list" style="margin-top: 12px; grid-template-columns: repeat(4, 1fr)">
            <div class="info-box">
              <span>MODEL</span>
              <strong>{{ selectedTemplate.model }}</strong>
            </div>
            <div class="info-box">
              <span>BRAND</span>
              <strong>{{ selectedTemplate.brand }}</strong>
            </div>
            <div class="info-box">
              <span>ANDROID</span>
              <strong>{{ selectedTemplate.android }}</strong>
            </div>
            <div class="info-box">
              <span>DPI</span>
              <strong>{{ selectedTemplate.dpi ?? '--' }}</strong>
            </div>
          </div>

          <div style="display: flex; align-items: center; gap: 10px; margin-top: 13px">
            <el-switch v-model="restartAfterApply" />
            <span style="font-size: 11px; color: #858a9d">
              下发后重启目标应用（force-stop）
            </span>
          </div>
        </section>
      </div>

      <aside class="batch-preview">
        <div class="batch-preview-head">
          <div class="eyebrow">PREVIEW</div>
          <h2>变更预览</h2>
          <p>确认后再下发；离线设备会被跳过</p>
        </div>
        <div class="preview-body">
          <div class="preview-scope">
            <div class="scope-chip">
              <span>应用</span>
              <strong>{{ selectedApp?.name ?? '未选择' }}</strong>
            </div>
            <div class="scope-chip">
              <span>设备</span>
              <strong>{{ effectiveTargets.length }} 台</strong>
            </div>
            <div class="scope-chip">
              <span>模板</span>
              <strong>{{ selectedTemplate?.model ?? '未选择' }}</strong>
            </div>
          </div>

          <div v-if="targetDevices.length" class="preview-list">
            <div
              v-for="device in targetDevices"
              :key="device.id"
              :class="['preview-row', { 'is-offline': !device.online && onlineOnly }]"
            >
              <div class="preview-device">
                <strong>{{ device.name }}</strong>
                <span>{{ device.online ? device.model ?? '未识别' : '离线，将跳过' }}</span>
              </div>
              <div class="preview-models">
                <span style="color: #9aa0b2">{{ currentModel(device.id) }}</span>
                <el-icon class="preview-arrow"><ArrowRight /></el-icon>
                <span class="preview-new">{{ selectedTemplate?.model ?? '—' }}</span>
              </div>
            </div>
          </div>
          <div v-else class="preview-empty">选择一个应用和目标设备后显示变更</div>

          <div class="preview-foot">
            <div class="preview-note">
              <el-icon style="margin-top: 1px"><InfoFilled /></el-icon>
              <span>
                通过 Relay 逐台调用 <code>PUT /api/v1/apps/{pkg}</code>，使用每台设备各自的
                revision 自动重试一次 412；结果按设备逐条返回。
              </span>
            </div>
            <div v-if="jobError" class="inline-alert is-error" style="margin-bottom: 9px">
              <el-icon><WarningFilled /></el-icon>
              <span>{{ jobError }}</span>
            </div>
            <el-button
              type="primary"
              style="width: 100%"
              :disabled="!canApply"
              :loading="manager.applying"
              @click="apply"
            >
              <el-icon v-if="!manager.applying" style="margin-right: 4px"><MagicStick /></el-icon>
              执行配置
            </el-button>
            <div v-if="lastJob" class="hint" style="margin-top: 9px; color: #15803d">
              已创建 job · 成功 {{ lastJob.succeeded }} / 跳过 {{ lastJob.skipped }} / 失败 {{ lastJob.failed }}
            </div>
          </div>
        </div>
      </aside>
    </div>

    <section v-if="lastJob" class="endpoint-table" style="margin-top: 16px">
      <div class="panel-head">
        <div>
          <h2>下发结果</h2>
          <p>
            job <code>{{ lastJob.id }}</code> · {{ jobStatusLabel(lastJob.status) }} ·
            共 {{ lastJob.total }} 项 · {{ lastJob.durationMs ?? '--' }} ms
          </p>
        </div>
        <span :class="['model-chip', lastJob.status === 'failed' || lastJob.status === 'partial' ? 'warn' : '']">
          {{ lastJob.kind }}
        </span>
      </div>
      <el-table :data="lastJob.results" style="width: 100%">
        <el-table-column label="设备" min-width="170">
          <template #default="{ row }">
            <div class="log-title">{{ row.deviceName }}</div>
            <div class="log-detail">{{ row.deviceId }}</div>
          </template>
        </el-table-column>
        <el-table-column label="应用" min-width="190">
          <template #default="{ row }">
            <code style="font-size: 10.5px; color: #3c366b">{{ row.app }}</code>
          </template>
        </el-table-column>
        <el-table-column label="结果" width="110">
          <template #default="{ row }">
            <el-tag size="small" :type="resultTagType(row.status)">
              {{ resultLabel(row.status) }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column label="错误码 / 说明" min-width="220">
          <template #default="{ row }">
            <span v-if="row.error" style="font-size: 11px; color: #b91c1c">
              {{ row.error.code }} · {{ row.error.message }}
            </span>
            <span v-else-if="row.code" style="font-size: 11px; color: #b45309">{{ row.code }}</span>
            <span v-else style="font-size: 11px; color: #9aa0b2">—</span>
          </template>
        </el-table-column>
        <el-table-column label="尝试" width="80" align="right">
          <template #default="{ row }">
            <span style="font-size: 11px">{{ row.attempts }} 次</span>
          </template>
        </el-table-column>
        <el-table-column label="耗时" width="95" align="right">
          <template #default="{ row }">
            <span style="font-size: 11px; color: #9aa0b2">{{ row.durationMs }} ms</span>
          </template>
        </el-table-column>
      </el-table>
      <div
        v-if="lastJob.status === 'queued' || lastJob.status === 'running'"
        class="hint"
        style="padding: 0 20px 14px"
      >
        仍在执行（轮询已超过 60 秒），稍后到操作日志或重新执行查看最终结果。
      </div>
    </section>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import {
  ArrowRight,
  Grid,
  InfoFilled,
  MagicStick,
  WarningFilled,
} from '@element-plus/icons-vue'
import { describeError } from '../api/client'
import type { Job, JobResultStatus } from '../api/types'
import { PACKAGE_PATTERN, appDefFor } from '../appCatalog'
import { useManagerStore } from '../stores/manager'
import type { ApplyTargets } from '../types'

const route = useRoute()
const router = useRouter()
const manager = useManagerStore()

const queryApp = typeof route.query.app === 'string' ? route.query.app : ''
const selectedPackage = ref(queryApp || (manager.appOptions[0]?.package ?? ''))
const targetMode = ref<'all' | 'selected'>('all')
const selectedDeviceIds = ref<string[]>([])
const onlineOnly = ref(true)
const templateName = ref(manager.templates[0]?.name ?? '')
const restartAfterApply = ref(true)
const lastJob = ref<Job | null>(null)
const jobError = ref('')

watch(
  () => route.query.app,
  (value) => {
    if (typeof value === 'string' && value) selectedPackage.value = value
  }
)

watch(
  () => manager.deviceCount,
  () => {
    if (!selectedDeviceIds.value.length) {
      selectedDeviceIds.value = manager.devices
        .filter((device) => device.online)
        .map((device) => device.id)
    }
  },
  { immediate: true }
)

watch(
  () => manager.templates.length,
  () => {
    if (!templateName.value) templateName.value = manager.templates[0]?.name ?? ''
  },
  { immediate: true }
)

watch(
  () => manager.appOptions.length,
  () => {
    if (!selectedPackage.value) selectedPackage.value = manager.appOptions[0]?.package ?? ''
  },
  { immediate: true }
)

const selectedApp = computed(() => {
  const name = selectedPackage.value.trim()
  if (!name) return undefined
  return manager.appById(name) ?? appDefFor(name)
})

const selectedTemplate = computed(() =>
  manager.templates.find((template) => template.name === templateName.value)
)

const matrixRow = computed(() => manager.matrixFor(selectedPackage.value.trim()))
const configuredCount = computed(() => matrixRow.value?.configured ?? 0)
const currentModels = computed(() => matrixRow.value?.models ?? [])

const targetDevices = computed(() => {
  if (targetMode.value === 'all') return manager.devices
  return manager.devices.filter((device) => selectedDeviceIds.value.includes(device.id))
})

const effectiveTargets = computed(() => {
  if (onlineOnly.value) return targetDevices.value.filter((device) => device.online)
  return targetDevices.value
})

const offlineCount = computed(
  () => targetDevices.value.filter((device) => !device.online).length
)

const applyTargets = computed<ApplyTargets>(() => {
  if (targetMode.value === 'all' && !onlineOnly.value) return 'all'
  return effectiveTargets.value.map((device) => device.id)
})

const canApply = computed(() => {
  const packageName = selectedPackage.value.trim()
  return (
    PACKAGE_PATTERN.test(packageName) &&
    Boolean(selectedTemplate.value) &&
    effectiveTargets.value.length > 0
  )
})

function currentModel(deviceId: string): string {
  if (!selectedPackage.value) return '未配置'
  return manager.configFor(selectedPackage.value.trim(), deviceId)?.model ?? '未配置'
}

function toggleDevice(id: string, checked: unknown): void {
  if (checked) {
    if (!selectedDeviceIds.value.includes(id)) selectedDeviceIds.value.push(id)
  } else {
    selectedDeviceIds.value = selectedDeviceIds.value.filter((item) => item !== id)
  }
}

function jobStatusLabel(status: Job['status']): string {
  return (
    {
      queued: '排队中',
      running: '执行中',
      succeeded: '全部成功',
      partial: '部分成功',
      failed: '全部失败',
    }[status] ?? status
  )
}

function resultLabel(status: JobResultStatus): string {
  return { ok: '成功', skipped: '跳过', failed: '失败' }[status] ?? status
}

function resultTagType(status: JobResultStatus): 'success' | 'info' | 'danger' {
  if (status === 'ok') return 'success'
  if (status === 'skipped') return 'info'
  return 'danger'
}

async function apply(): Promise<void> {
  jobError.value = ''
  const packageName = selectedPackage.value.trim()
  if (!PACKAGE_PATTERN.test(packageName)) {
    ElMessage.warning('包名不合法：需要形如 com.example.app')
    return
  }
  const template = selectedTemplate.value
  if (!template) {
    ElMessage.warning('请选择模板（需要至少一台设备同步过模板）')
    return
  }
  if (!effectiveTargets.value.length) {
    ElMessage.warning('没有可下发的目标设备')
    return
  }
  try {
    const job = await manager.batchApply({
      app: packageName,
      devices: applyTargets.value,
      template,
      restart: restartAfterApply.value,
    })
    lastJob.value = job
    if (job.status === 'succeeded') {
      ElMessage.success(`已下发 ${job.succeeded} 台设备`)
    } else if (job.status === 'failed') {
      ElMessage.error(`下发失败 ${job.failed} 台，详见逐设备结果`)
    } else {
      ElMessage.warning(
        `已下发：成功 ${job.succeeded}，跳过 ${job.skipped}，失败 ${job.failed}`
      )
    }
  } catch (cause) {
    jobError.value = describeError(cause, '下发失败')
    ElMessage.error(jobError.value)
  }
}
</script>
