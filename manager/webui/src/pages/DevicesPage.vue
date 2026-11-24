<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  Device Faker: https://github.com/Seyud/device_faker
-->

<template>
  <div>
    <div class="page-head">
      <div>
        <div class="eyebrow">FLEET</div>
        <h1>设备</h1>
        <p>每台设备暴露自己的 /api/v1，Relay 负责保存地址与 Token，并统一代理请求。</p>
      </div>
      <div class="head-actions">
        <el-button :loading="manager.syncingAll" @click="syncAll">
          <el-icon v-if="!manager.syncingAll" style="margin-right: 4px"><Refresh /></el-icon>
          同步全部
        </el-button>
        <el-button type="primary" @click="openAdd">
          <el-icon style="margin-right: 4px"><Plus /></el-icon>
          添加设备
        </el-button>
      </div>
    </div>

    <div class="toolbar">
      <el-input
        v-model="keyword"
        class="search"
        placeholder="搜索名称、机型或地址"
        clearable
      >
        <template #prefix><el-icon><Search /></el-icon></template>
      </el-input>
      <el-radio-group v-model="statusFilter" size="small">
        <el-radio-button value="all">全部</el-radio-button>
        <el-radio-button value="online">在线</el-radio-button>
        <el-radio-button value="offline">离线</el-radio-button>
      </el-radio-group>
      <span class="spacer"></span>
      <span class="toolbar-count">
        {{ filteredDevices.length }} 台设备 · {{ manager.onlineCount }} 台在线
      </span>
    </div>

    <div v-if="manager.loading && !manager.deviceCount" class="panel empty-state">
      <div class="empty-icon"><el-icon class="is-loading"><Loading /></el-icon></div>
      <span>正在从 GET /devices 加载设备…</span>
    </div>

    <div v-else-if="filteredDevices.length" class="device-grid">
      <article
        v-for="device in filteredDevices"
        :key="device.id"
        :class="['device-card', { 'is-offline': !device.online }]"
        @click="openDevice(device.id)"
      >
        <div class="device-card-head">
          <span :class="['status-dot', device.online ? 'is-online' : 'is-offline']"></span>
          <strong>{{ device.name }}</strong>
          <div class="tag-list">
            <span
              v-for="tag in device.tags.slice(0, 2)"
              :key="tag"
              :class="['tag', tag === '主力' ? 'accent' : '']"
            >
              {{ tag }}
            </span>
          </div>
          <el-dropdown
            class="card-menu"
            trigger="click"
            @command="(command: string) => handleCommand(command, device.id)"
            @click.stop
          >
            <el-button circle text @click.stop>
              <el-icon><MoreFilled /></el-icon>
            </el-button>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item command="sync" :disabled="manager.isSyncing(device.id)">
                  {{ manager.isSyncing(device.id) ? '同步中…' : '立即同步' }}
                </el-dropdown-item>
                <el-dropdown-item command="open">打开设备 WebUI</el-dropdown-item>
                <el-dropdown-item command="delete" divided>移除设备</el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>
        </div>

        <div class="device-card-model">
          <div class="phone-tile"><el-icon><Iphone /></el-icon></div>
          <div class="device-card-model-main">
            <strong>{{ device.model ?? '未识别' }} · Android {{ device.android ?? '--' }}</strong>
            <span>
              {{ device.brand ?? 'unknown' }} · {{ device.root ?? '无 root' }} ·
              模块 {{ device.moduleVersion ?? '--' }}
            </span>
          </div>
          <span :class="['model-chip', device.online ? '' : 'warn']">
            {{ device.online ? (device.latencyMs ?? '--') + ' ms' : '离线' }}
          </span>
        </div>

        <div class="fingerprint-line">{{ device.fingerprint ?? '尚无指纹（未同步）' }}</div>

        <div class="mini-stats">
          <div class="mini-stat">
            <span>应用配置</span>
            <strong>{{ manager.appCountForDevice(device.id) }}</strong>
          </div>
          <div class="mini-stat">
            <span>最后同步</span>
            <strong style="font-size: 11px">{{ relativeTime(device.lastSyncedAt ?? device.lastSeen) }}</strong>
          </div>
          <div class="mini-stat">
            <span>服务版本</span>
            <strong style="font-size: 11px">v{{ device.serverVersion ?? '--' }}</strong>
          </div>
        </div>

        <div class="device-card-foot">
          <span class="endpoint-text">{{ device.baseUrl }}</span>
          <el-button link type="primary" @click.stop="openDevice(device.id)">
            查看配置
            <el-icon style="margin-left: 2px"><ArrowRight /></el-icon>
          </el-button>
        </div>
      </article>
    </div>

    <div v-else class="empty-state panel">
      <div class="empty-icon"><el-icon><Monitor /></el-icon></div>
      <span>{{ manager.deviceCount ? '没有匹配的设备' : '还没有注册设备' }}</span>
      <el-button v-if="!manager.deviceCount" type="primary" plain @click="openAdd">
        添加第一台设备
      </el-button>
    </div>

    <!-- 添加设备 -->
    <el-dialog
      v-model="addVisible"
      title="添加设备"
      width="520px"
      :close-on-click-modal="false"
      @closed="handleAddClosed"
    >
      <p class="hint" style="margin-top: -4px; margin-bottom: 18px">
        设备需要先开启 External Access（或通过 adb forward 暴露）。Relay 会用下面的 Token
        探测 <code>GET /api/v1/status</code> 与 <code>GET /api/v1/overview</code>，
        探测失败不会写入设备表。
      </p>

      <el-form label-position="top">
        <el-form-item label="设备名称">
          <el-input v-model="formName" placeholder="例如：Pixel 9 · 主力机" />
        </el-form-item>
        <el-form-item label="设备地址 (baseUrl)">
          <el-input v-model="formBaseUrl" placeholder="http://192.168.30.2:8686" />
        </el-form-item>
        <el-form-item label="Access Token">
          <el-input
            v-model="formToken"
            type="password"
            show-password
            placeholder="默认 admin123"
          />
        </el-form-item>
        <el-form-item label="标签（可选）">
          <el-select
            v-model="formTags"
            multiple
            filterable
            allow-create
            default-first-option
            placeholder="输入后回车创建，例如：主力 / 测试 / 海外"
            style="width: 100%"
          />
        </el-form-item>
      </el-form>

      <div v-if="addError" class="inline-alert is-error">
        <el-icon><WarningFilled /></el-icon>
        <span>{{ addError }}</span>
      </div>
      <div v-else-if="tested" :class="['inline-alert', testOk ? 'is-ok' : 'is-warn']">
        <el-icon><component :is="testOk ? CircleCheck : WarningFilled" /></el-icon>
        <span>{{ testResult }}</span>
      </div>

      <template #footer>
        <div style="display: flex; align-items: center">
          <el-button :loading="checking" @click="checkRelay">检查 Relay</el-button>
          <span style="flex: 1"></span>
          <el-button @click="addVisible = false">取消</el-button>
          <el-button type="primary" :loading="adding" @click="submitAdd">添加设备</el-button>
        </div>
      </template>
    </el-dialog>

    <!-- 设备详情 -->
    <el-drawer
      v-model="drawerVisible"
      size="620px"
      :with-header="false"
      class="device-drawer"
      @closed="handleDrawerClosed"
    >
      <div v-if="selectedDevice" style="padding: 20px">
        <div class="drawer-hero">
          <div class="drawer-hero-top">
            <span
              :class="['status-dot', selectedDevice.online ? 'is-online' : 'is-offline']"
            ></span>
            <span style="font-size: 11px">
              {{ selectedDevice.online ? '在线' : '离线' }} ·
              {{ relativeTime(selectedDevice.lastSeen) }}
            </span>
          </div>
          <h2>{{ selectedDevice.name }}</h2>
          <p>
            {{ selectedDevice.model ?? '未识别' }} · Android {{ selectedDevice.android ?? '--' }} ·
            {{ selectedDevice.baseUrl }}
          </p>
          <div class="drawer-actions">
            <el-button size="small" :loading="selectedSyncing" @click="syncSelected">
              <el-icon v-if="!selectedSyncing" style="margin-right: 3px"><Refresh /></el-icon>
              同步配置
            </el-button>
            <el-button size="small" plain @click="openWebUI(selectedDevice.baseUrl)">
              <el-icon style="margin-right: 3px"><Link /></el-icon>
              打开 WebUI
            </el-button>
          </div>
        </div>

        <div v-if="detailError" class="inline-alert is-error" style="margin-bottom: 14px">
          <el-icon><WarningFilled /></el-icon>
          <span>{{ detailError }}</span>
          <el-button link type="primary" @click="reloadDetail">重试</el-button>
        </div>

        <el-tabs v-model="detailTab">
          <el-tab-pane label="概览" name="overview">
            <div class="drawer-section">
              <h3>设备指纹</h3>
              <div class="code-panel">{{ selectedDevice.fingerprint ?? '尚无指纹（未同步）' }}</div>
            </div>

            <div class="drawer-section">
              <h3>运行信息</h3>
              <div class="info-grid">
                <div class="info-box">
                  <span>ROOT 管理器</span>
                  <strong>{{ selectedDevice.root ?? '未检测' }}</strong>
                </div>
                <div class="info-box">
                  <span>模块版本</span>
                  <strong>{{ selectedDevice.moduleVersion ?? '--' }}</strong>
                </div>
                <div class="info-box">
                  <span>服务版本</span>
                  <strong>v{{ selectedDevice.serverVersion ?? '--' }}</strong>
                </div>
                <div class="info-box">
                  <span>网络延迟</span>
                  <strong>{{ selectedDevice.latencyMs !== null ? selectedDevice.latencyMs + ' ms' : '--' }}</strong>
                </div>
              </div>
            </div>

            <div class="drawer-section">
              <h3>配置概览</h3>
              <div class="info-grid">
                <div class="info-box">
                  <span>应用配置</span>
                  <strong>{{ selectedApps.length }} 个</strong>
                </div>
                <div class="info-box">
                  <span>配置漂移</span>
                  <strong>{{ selectedDriftCount }} 个</strong>
                </div>
                <div class="info-box">
                  <span>缓存时间</span>
                  <strong>{{ detail?.cachedAt ? relativeTime(detail.cachedAt) : '--' }}</strong>
                </div>
                <div class="info-box">
                  <span>缓存状态</span>
                  <strong>{{ detail ? (detail.stale ? '已过期' : '最新') : '—' }}</strong>
                </div>
              </div>
            </div>

            <div v-if="selectedTemplates.length" class="drawer-section">
              <h3>设备上的模板</h3>
              <div class="target-list" style="grid-template-columns: 1fr 1fr">
                <div v-for="template in selectedTemplates" :key="template.name" class="info-box">
                  <span>{{ template.name }}</span>
                  <strong>{{ template.config.model ?? '—' }}</strong>
                </div>
              </div>
            </div>

            <div v-if="selectedWarnings.length" class="drawer-section">
              <h3>警告</h3>
              <div v-for="warning in selectedWarnings" :key="warning.code + warning.message" class="inline-alert is-warn" style="margin-bottom: 6px">
                <el-icon><Warning /></el-icon>
                <span>{{ warning.message }}</span>
              </div>
            </div>
          </el-tab-pane>

          <el-tab-pane label="应用配置" name="apps">
            <div v-if="detailLoading" class="inline-alert is-muted">
              <el-icon class="is-loading"><Loading /></el-icon>
              <span>正在加载 GET /devices/{{ selectedDevice.id }}…</span>
            </div>
            <div v-else-if="selectedApps.length" class="drawer-apps">
              <div v-for="item in selectedApps" :key="item.app.package" class="drawer-app-row">
                <div class="app-badge" :style="{ background: item.app.color }">
                  {{ item.app.short }}
                </div>
                <div class="drawer-app-main">
                  <strong>{{ item.app.name }}</strong>
                  <span>{{ item.cell.source.template ?? '内联配置' }} · {{ item.cell.source.type }}</span>
                </div>
                <span class="model-chip">{{ item.cell.model ?? '未识别' }}</span>
                <el-tooltip v-if="item.warning" :content="item.warning">
                  <el-icon style="color: #d97706"><Warning /></el-icon>
                </el-tooltip>
              </div>
            </div>
            <div v-else class="empty-state">
              <div class="empty-icon"><el-icon><Grid /></el-icon></div>
              <span>这台设备还没有应用配置</span>
            </div>
          </el-tab-pane>

          <el-tab-pane label="连接信息" name="connection">
            <div class="drawer-section">
              <h3>Endpoint</h3>
              <div class="copy-row">
                <span class="label">BASE URL</span>
                <code>{{ selectedDevice.baseUrl }}</code>
                <el-button link type="primary" @click="copy(selectedDevice.baseUrl, '地址')">
                  复制
                </el-button>
              </div>
              <div class="copy-row">
                <span class="label">TOKEN</span>
                <code>••••••••••••••••（加密保存在 Relay 服务端）</code>
              </div>
            </div>

            <div class="drawer-section">
              <h3>Relay 调用</h3>
              <div class="code-panel">
                # 缓存的 overview（不访问设备）<br />
                curl -H "Authorization: Bearer $DFM_TOKEN" \<br />
                &nbsp;&nbsp;{{ relayBase }}/devices/{{ selectedDevice.id }}/overview?cached=1<br /><br />
                # 原样透传设备端 /api/v1<br />
                curl -H "Authorization: Bearer $DFM_TOKEN" \<br />
                &nbsp;&nbsp;{{ relayBase }}/devices/{{ selectedDevice.id }}/proxy/api/v1/apps/&lt;pkg&gt;
              </div>
            </div>

            <el-button type="danger" plain style="width: 100%" @click="removeSelected">
              <el-icon style="margin-right: 4px"><Delete /></el-icon>
              从管理端移除设备
            </el-button>
          </el-tab-pane>
        </el-tabs>
      </div>

      <div v-else class="empty-state">
        <div class="empty-icon"><el-icon class="is-loading"><Loading /></el-icon></div>
        <span>正在加载设备…</span>
      </div>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import {
  ArrowRight,
  CircleCheck,
  Delete,
  Grid,
  Iphone,
  Link,
  Loading,
  Monitor,
  MoreFilled,
  Plus,
  Refresh,
  Search,
  Warning,
  WarningFilled,
} from '@element-plus/icons-vue'
import { describeError } from '../api/client'
import type { DeviceAppSummary, DeviceDetail, TemplateSummary, Warning as ApiWarning } from '../api/types'
import { appDefFor } from '../appCatalog'
import { useManagerStore } from '../stores/manager'
import { copyText } from '../utils/clipboard'
import { relativeTime } from '../utils/format'
import type { AppDef } from '../types'

const route = useRoute()
const router = useRouter()
const manager = useManagerStore()

const keyword = ref('')
const statusFilter = ref<'all' | 'online' | 'offline'>('all')
const addVisible = ref(false)
const adding = ref(false)
const addError = ref('')
const checking = ref(false)
const tested = ref(false)
const testOk = ref(false)
const testResult = ref('')
const drawerVisible = ref(false)
const selectedId = ref('')
const detailTab = ref('overview')
const detail = ref<DeviceDetail | null>(null)
const detailLoading = ref(false)
const detailError = ref('')
const relayBase = 'http://127.0.0.1:8765/relay/api/v1'

const formName = ref('')
const formBaseUrl = ref('')
const formToken = ref('admin123')
const formTags = ref<string[]>([])

const filteredDevices = computed(() => {
  const query = keyword.value.trim().toLowerCase()
  return manager.devices.filter((device) => {
    if (statusFilter.value === 'online' && !device.online) return false
    if (statusFilter.value === 'offline' && device.online) return false
    if (!query) return true
    return [
      device.name,
      device.model ?? '',
      device.baseUrl,
      device.tags.join(' '),
    ]
      .join(' ')
      .toLowerCase()
      .includes(query)
  })
})

const selectedSummary = computed(() => manager.deviceById(selectedId.value))
const selectedDevice = computed(() => {
  if (detail.value && detail.value.id === selectedId.value) return detail.value
  return selectedSummary.value
})

const selectedApps = computed<Array<{ app: AppDef; cell: DeviceAppSummary; warning?: string }>>(
  () =>
    (detail.value?.apps ?? []).map((cell) => ({
      app: appDefFor(cell.package),
      cell,
      warning: cell.warnings?.length
        ? cell.warnings.map((warning) => warning.message).join('；')
        : undefined,
    }))
)

const selectedTemplates = computed<TemplateSummary[]>(() => detail.value?.templates ?? [])

const selectedWarnings = computed<ApiWarning[]>(() => {
  const list = [...(detail.value?.warnings ?? [])]
  const lastError = selectedDevice.value?.lastError
  if (lastError && !list.some((warning) => warning.code === lastError.code)) {
    list.push({ code: lastError.code, message: lastError.message, details: null })
  }
  return list
})

const selectedSyncing = computed(() =>
  selectedId.value ? manager.isSyncing(selectedId.value) : false
)

const selectedDriftCount = computed(() => {
  if (!selectedDevice.value) return 0
  return manager.driftRows.filter((row) => row.cells[selectedDevice.value?.id ?? '']).length
})

function openAdd(): void {
  addVisible.value = true
}

function handleAddClosed(): void {
  resetForm()
  if (route.query.add) {
    const query = { ...route.query }
    delete query.add
    router.replace({ path: '/devices', query })
  }
}

function handleDrawerClosed(): void {
  selectedId.value = ''
  detail.value = null
  detailError.value = ''
}

function resetForm(): void {
  formName.value = ''
  formBaseUrl.value = ''
  formToken.value = 'admin123'
  formTags.value = []
  tested.value = false
  testResult.value = ''
  addError.value = ''
}

async function checkRelay(): Promise<void> {
  checking.value = true
  try {
    const ok = await manager.refreshHealth()
    tested.value = true
    testOk.value = ok
    if (ok && manager.health) {
      testResult.value = `Relay 在线 · v${manager.health.version} · 设备 ${manager.health.devices.total} 台（在线 ${manager.health.devices.online}）`
    } else {
      testResult.value = `Relay 未连接：${manager.relayError ?? '请先启动 manager（127.0.0.1:8765）'}`
    }
  } finally {
    checking.value = false
  }
}

async function submitAdd(): Promise<void> {
  addError.value = ''
  const name = formName.value.trim()
  const baseUrl = formBaseUrl.value.trim()
  if (!name) {
    ElMessage.warning('请填写设备名称')
    return
  }
  if (!/^https?:\/\//i.test(baseUrl)) {
    ElMessage.warning('设备地址需要以 http:// 或 https:// 开头')
    return
  }
  if (!formToken.value.trim()) {
    ElMessage.warning('请填写设备 Access Token')
    return
  }
  adding.value = true
  try {
    const device = await manager.addDevice({
      name,
      baseUrl,
      token: formToken.value.trim(),
      tags: formTags.value,
    })
    addVisible.value = false
    ElMessage.success('设备已添加，探测通过')
    openDevice(device.id)
  } catch (cause) {
    addError.value = describeError(cause, '添加设备失败')
  } finally {
    adding.value = false
  }
}

async function loadDetail(id: string): Promise<void> {
  detailLoading.value = true
  detailError.value = ''
  try {
    detail.value = await manager.getDeviceDetail(id)
  } catch (cause) {
    detail.value = null
    detailError.value = describeError(cause, '加载设备详情失败')
  } finally {
    detailLoading.value = false
  }
}

function reloadDetail(): void {
  if (selectedId.value) void loadDetail(selectedId.value)
}

function openDevice(id: string): void {
  selectedId.value = id
  detailTab.value = 'overview'
  detail.value = null
  drawerVisible.value = true
  void loadDetail(id)
}

function openWebUI(baseUrl: string): void {
  window.open(baseUrl, '_blank', 'noopener')
}

async function syncSelected(): Promise<void> {
  if (!selectedDevice.value) return
  const id = selectedDevice.value.id
  try {
    const result = await manager.syncDevice(id)
    await loadDetail(id)
    if (result.online) {
      ElMessage.success('配置已同步')
    } else {
      ElMessage.warning(result.warnings[0]?.message ?? '设备离线，保留上次缓存')
    }
  } catch (cause) {
    ElMessage.error(describeError(cause, '同步失败'))
  }
}

async function syncAll(): Promise<void> {
  try {
    const result = await manager.syncAll()
    ElMessage.success(
      result.failed > 0
        ? `已同步 ${result.synced} 台，${result.failed} 台失败`
        : `已同步 ${result.synced} 台设备`
    )
  } catch (cause) {
    ElMessage.error(describeError(cause, '同步失败'))
  }
}

async function handleCommand(command: string, id: string): Promise<void> {
  if (command === 'sync') {
    try {
      const result = await manager.syncDevice(id)
      if (result.online) ElMessage.success('配置已同步')
      else ElMessage.warning(result.warnings[0]?.message ?? '设备离线，保留上次缓存')
    } catch (cause) {
      ElMessage.error(describeError(cause, '同步失败'))
    }
    return
  }
  if (command === 'open') {
    const device = manager.deviceById(id)
    if (device) openWebUI(device.baseUrl)
    return
  }
  if (command === 'delete') {
    await removeDevice(id)
  }
}

async function removeDevice(id: string): Promise<void> {
  const device = manager.deviceById(id)
  if (!device) return
  try {
    await ElMessageBox.confirm(
      `确定从管理端移除「${device.name}」？远端配置不会被删除。`,
      '移除设备',
      { confirmButtonText: '移除', cancelButtonText: '取消', type: 'warning' }
    )
  } catch {
    return
  }
  try {
    await manager.removeDevice(id)
    if (selectedId.value === id) {
      drawerVisible.value = false
    }
    ElMessage.success('设备已移除')
  } catch (cause) {
    ElMessage.error(describeError(cause, '移除设备失败'))
  }
}

async function removeSelected(): Promise<void> {
  if (!selectedDevice.value) return
  await removeDevice(selectedDevice.value.id)
}

async function copy(text: string, label: string): Promise<void> {
  const copied = await copyText(text)
  ElMessage[copied ? 'success' : 'warning'](copied ? `${label}已复制` : '复制失败')
}

function applyQuery(): void {
  if (route.query.add === '1') {
    addVisible.value = true
  }
  const id = route.query.device
  if (typeof id === 'string' && manager.deviceById(id) && !drawerVisible.value) {
    openDevice(id)
  }
}

onMounted(applyQuery)
watch([() => route.query, () => manager.deviceCount], applyQuery, { deep: true })
watch(drawerVisible, (visible) => {
  if (!visible) detail.value = null
})
</script>
