<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  Device Faker: https://github.com/Seyud/device_faker
-->

<template>
  <div class="app-shell">
    <aside class="sidebar">
      <div class="brand">
        <div class="brand-mark">
          <el-icon :size="21"><Cpu /></el-icon>
        </div>
        <div class="brand-text">
          <strong>Device Faker</strong>
          <span>Relay Console</span>
        </div>
      </div>

      <div class="workspace-chip">
        <span class="dot"></span>
        <span>工作空间</span>
        <strong>Default</strong>
      </div>

      <nav class="side-nav">
        <div class="nav-label">管理</div>
        <router-link
          v-for="item in navItems"
          :key="item.path"
          :to="item.path"
          class="nav-item"
          active-class="is-active"
        >
          <el-icon><component :is="item.icon" /></el-icon>
          <span>{{ item.label }}</span>
          <span
            v-if="item.key === 'apps' && manager.driftRows.length"
            class="nav-badge urgent"
          >
            {{ manager.driftRows.length }}
          </span>
          <span
            v-else-if="item.key === 'devices'"
            class="nav-badge"
          >
            {{ manager.onlineCount }}/{{ manager.deviceCount }}
          </span>
          <span
            v-else-if="item.key === 'proxies' && manager.egressProxyTotal"
            class="nav-badge"
          >
            {{ manager.egressProxyTotal }}
          </span>
        </router-link>
      </nav>

      <div class="sidebar-footer">
        <div :class="['relay-status', { 'is-offline': !manager.relayOnline }]">
          <span class="pulse"></span>
          <span>
            <strong>{{ manager.relayOnline ? 'Relay 在线' : 'Relay 未连接' }}</strong> ·
            {{ manager.health ? 'v' + manager.health.version : ':8765' }}
          </span>
        </div>
        <button type="button" class="token-link" @click="openTokenDialog">
          <el-icon><Key /></el-icon>
          {{ manager.token ? 'Manager Token 已设置' : '设置 Manager Token' }}
        </button>
        <div class="sidebar-version">
          v0.1.0 · {{ manager.mockMode ? 'MOCK 预览' : 'manager API v1' }}
        </div>
      </div>
    </aside>

    <div class="main">
      <header class="topbar">
        <div class="topbar-title">
          <h2>{{ pageTitle }}</h2>
          <p>{{ pageSubtitle }}</p>
        </div>

        <div class="topbar-actions">
          <el-input
            v-model="search"
            class="global-search"
            placeholder="搜索设备 / 应用"
            clearable
            @keyup.enter="openSearch"
          >
            <template #prefix>
              <el-icon><Search /></el-icon>
            </template>
          </el-input>
          <el-tooltip content="同步所有设备（POST /devices/sync）" placement="bottom">
            <el-button circle :loading="manager.syncingAll" @click="refreshAll">
              <el-icon v-if="!manager.syncingAll"><Refresh /></el-icon>
            </el-button>
          </el-tooltip>
          <el-tooltip content="Manager Token" placement="bottom">
            <el-button circle @click="openTokenDialog">
              <el-icon><Key /></el-icon>
            </el-button>
          </el-tooltip>
          <el-tooltip content="操作日志" placement="bottom">
            <el-button circle @click="router.push('/logs')">
              <el-icon><Bell /></el-icon>
            </el-button>
          </el-tooltip>
          <el-button type="primary" @click="goAddDevice">
            <el-icon style="margin-right: 4px"><Plus /></el-icon>
            添加设备
          </el-button>
          <div class="avatar">YG</div>
        </div>
      </header>

      <main class="page">
        <div v-if="manager.error" class="api-banner">
          <el-icon class="api-banner-icon"><WarningFilled /></el-icon>
          <div class="api-banner-body">
            <strong>Relay 请求失败</strong>
            <span>{{ manager.error }}</span>
            <span v-if="manager.errorRequestId" class="mono api-banner-rid">
              requestId: {{ manager.errorRequestId }}
            </span>
          </div>
          <el-button size="small" :loading="manager.loading" @click="retryLoad">重新加载</el-button>
          <el-button size="small" text @click="manager.clearError()">关闭</el-button>
        </div>

        <div v-if="manager.mockMode" class="api-banner is-mock">
          <el-icon class="api-banner-icon"><InfoFilled /></el-icon>
          <div class="api-banner-body">
            <strong>MOCK 预览模式</strong>
            <span>VITE_MOCK=1，页面只展示 src/mock 的示例数据；写操作会被拒绝。</span>
          </div>
        </div>

        <router-view />
      </main>
    </div>

    <el-dialog
      v-model="tokenDialogVisible"
      title="Manager Token"
      width="520px"
      :close-on-click-modal="false"
      data-testid="manager-token-dialog"
    >
      <p class="hint" style="margin-top: 0">
        Relay 用 <code>Authorization: Bearer &lt;manager-token&gt;</code> 鉴权。Token 来自
        <code>--token</code> / <code>DFM_TOKEN</code> / <code>&lt;data-dir&gt;/manager.token</code>，
        首次启动默认 <code>admin123</code>。保存后写入本浏览器 <code>localStorage.dfm_token</code>，
        界面不会回显明文。
      </p>
      <el-input
        ref="tokenInputRef"
        v-model="tokenInput"
        type="password"
        show-password
        autocomplete="off"
        placeholder="dfm_... 或 admin123"
        data-testid="manager-token-input"
        @keyup.enter="submitToken"
      />
      <p v-if="manager.authRequired" class="hint" style="margin-bottom: 0; color: #b45309">
        当前 Token 无效或已被轮换，请输入新 Token 后重试。
      </p>
      <template #footer>
        <div style="display: flex; align-items: center">
          <el-button link type="primary" @click="router.push('/relay')">去轮换 Token</el-button>
          <span style="flex: 1"></span>
          <el-button @click="tokenDialogVisible = false">取消</el-button>
          <el-button
            type="primary"
            :loading="tokenSubmitting"
            data-testid="manager-token-submit"
            @click="submitToken"
          >
            保存并重试
          </el-button>
        </div>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import {
  Bell,
  Connection,
  Cpu,
  Document,
  Grid,
  InfoFilled,
  Key,
  MagicStick,
  Monitor,
  Odometer,
  Plus,
  Refresh,
  Search,
  Share,
  WarningFilled,
} from '@element-plus/icons-vue'
import { DEFAULT_TOKEN, describeError, getStoredToken } from './api/client'
import { useManagerStore } from './stores/manager'

const route = useRoute()
const router = useRouter()
const manager = useManagerStore()
const search = ref('')

const tokenDialogVisible = ref(false)
const tokenInput = ref(DEFAULT_TOKEN)
const tokenSubmitting = ref(false)
/**
 * Token 输入框（受控组件）的实例。
 *
 * 保存时**以 DOM 里的当前值为准**：自动化/第三方脚本可能只改了 input.value 而没有派发
 * input 事件，只读 v-model 会拿到旧值（填了真 Token 却存下 admin123）。见 readTokenInputValue。
 */
const tokenInputRef = ref<unknown>(null)

const navItems = [
  { key: 'dashboard', path: '/', label: '总览', icon: Odometer },
  { key: 'devices', path: '/devices', label: '设备', icon: Monitor },
  { key: 'proxies', path: '/proxies', label: '代理池', icon: Share },
  { key: 'apps', path: '/apps', label: '应用矩阵', icon: Grid },
  { key: 'batch', path: '/batch', label: '批量配置', icon: MagicStick },
  { key: 'relay', path: '/relay', label: '接入 AI', icon: Connection },
  { key: 'logs', path: '/logs', label: '操作日志', icon: Document },
]

const pageTitle = computed(() => (route.meta.title as string) || '设备总览')
const pageSubtitle = computed(
  () => (route.meta.subtitle as string) || '统一管理多台 Device Faker 设备'
)

watch(
  () => manager.authRequired,
  (required) => {
    if (required) openTokenDialog()
  }
)

onMounted(async () => {
  await manager.load()
  await manager.refreshHealth()
  // 代理池数量给侧边栏角标用；静默失败（旧版 manager 没有 /egress-proxies 也不会弹横幅）
  await manager.reloadEgressProxiesQuiet()
  if (manager.authRequired) openTokenDialog()
})

function openSearch(): void {
  const query = search.value.trim()
  router.push({ path: '/apps', query: query ? { q: query } : {} })
}

function goAddDevice(): void {
  router.push({ path: '/devices', query: { add: '1' } })
}

/**
 * 读取用户在对话框里实际输入的 Token。
 *
 * 优先读真实 DOM：受控组件（el-input）只有在 input 事件触发时才更新 v-model，
 * 自动化脚本/书签直接写 .value 会绕过这一步，只读 v-model 就会把「屏幕上的新 Token」
 * 丢成旧值。保存动作必须以界面上的当前内容为准。
 */
function readTokenInputValue(): string {
  const instance = tokenInputRef.value as { input?: HTMLInputElement | null } | null
  const fromInstance = instance?.input
  if (fromInstance instanceof HTMLInputElement) return fromInstance.value
  if (typeof document !== 'undefined') {
    const target = document.querySelector('[data-testid="manager-token-input"]')
    if (target instanceof HTMLInputElement) return target.value
    const nested = target?.querySelector('input')
    if (nested instanceof HTMLInputElement) return nested.value
  }
  return tokenInput.value
}

function openTokenDialog(): void {
  // 不回显已保存的 Token：只有从未设置过时才预填内置默认值（admin123 不是秘密）
  tokenInput.value = manager.token ? '' : DEFAULT_TOKEN
  tokenDialogVisible.value = true
}

async function submitToken(): Promise<void> {
  const value = readTokenInputValue().trim()
  if (!value) {
    // 没有新输入但手里已有 Token：按「保持当前凭据重试」处理，避免用户卡在对话框
    if (manager.token) {
      tokenSubmitting.value = true
      try {
        await retryLoad()
        if (manager.authRequired) {
          ElMessage.error('当前 Token 仍然无效：请输入新 Token')
          return
        }
        tokenDialogVisible.value = false
        ElMessage.success('已使用当前 Token 重新加载')
      } finally {
        tokenSubmitting.value = false
      }
      return
    }
    ElMessage.warning('请输入 Manager Token')
    return
  }

  tokenSubmitting.value = true
  try {
    await manager.applyToken(value)
    // 落盘断言：隐私模式 / 配额不足时 setItem 会失败，必须显式报错而不是假装成功
    if (getStoredToken() !== value) {
      ElMessage.error('Token 未能写入本地存储（localStorage 不可用？），请检查浏览器隐私设置')
      return
    }
    if (manager.authRequired) {
      ElMessage.error('Token 仍然无效：请检查服务端 --token / DFM_TOKEN / manager.token')
      return
    }
    tokenInput.value = '' // 不在 DOM 里留下明文
    tokenDialogVisible.value = false
    ElMessage.success('Token 已保存，已使用新凭据重新加载')
    await manager.refreshHealth()
  } finally {
    tokenSubmitting.value = false
  }
}

async function retryLoad(): Promise<void> {
  await manager.load()
  await manager.refreshHealth()
}

async function refreshAll(): Promise<void> {
  try {
    const result = await manager.syncAll()
    ElMessage.success(
      result.failed > 0
        ? `已同步 ${result.synced} 台，${result.failed} 台失败（离线不影响其它设备）`
        : `已同步 ${result.synced} 台设备`
    )
  } catch (cause) {
    ElMessage.error(describeError(cause, '同步失败'))
  }
}
</script>
