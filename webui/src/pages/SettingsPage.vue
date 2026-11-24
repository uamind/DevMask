<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  Device Faker: https://github.com/Seyud/device_faker
-->

<template>
  <div class="settings-page">
    <div class="settings-section glass-effect">
      <h2 class="section-title">{{ t('settings.display.title') }}</h2>

      <div class="setting-item setting-item-split">
        <div class="setting-info">
          <div class="setting-icon">
            <Moon :size="24" />
          </div>
          <div class="setting-text">
            <h3 class="setting-name">{{ t('settings.display.theme.label') }}</h3>
            <p class="setting-desc">{{ t('settings.display.theme.desc') }}</p>
          </div>
        </div>
        <BaseSelect
          v-model="currentTheme"
          class="setting-control"
          :options="themeOptions"
          :aria-label="t('settings.display.theme.label')"
          @change="onThemeChange"
        />
      </div>

      <div class="setting-item setting-item-split">
        <div class="setting-info">
          <div class="setting-icon">
            <Globe :size="24" />
          </div>
          <div class="setting-text">
            <h3 class="setting-name">{{ t('settings.display.language.label') }}</h3>
            <p class="setting-desc">{{ t('settings.display.language.desc') }}</p>
          </div>
        </div>
        <BaseSelect
          v-model="currentLanguage"
          class="setting-control"
          :options="languageOptions"
          :aria-label="t('settings.display.language.label')"
          @change="onLanguageChange"
        />
      </div>
    </div>

    <div class="settings-section glass-effect">
      <h2 class="section-title">{{ t('settings.tools.title') }}</h2>

      <div class="setting-item">
        <div class="setting-info">
          <div class="setting-icon">
            <FileUp :size="24" />
          </div>
          <div class="setting-text">
            <h3 class="setting-name">{{ t('settings.tools.convert.label') }}</h3>
            <p class="setting-desc">{{ t('settings.tools.convert.desc') }}</p>
          </div>
        </div>
        <el-button type="primary" :loading="converting" @click="startConversion">
          {{ t('settings.tools.convert.btn') }}
        </el-button>
      </div>
    </div>

    <div class="settings-section glass-effect">
      <h2 class="section-title">{{ t('settings.module.title') }}</h2>

      <div class="setting-item setting-item-horizontal">
        <div class="setting-info">
          <div class="setting-icon">
            <Bug :size="24" />
          </div>
          <div class="setting-text">
            <h3 class="setting-name">{{ t('settings.module.debug.label') }}</h3>
            <p class="setting-desc">{{ t('settings.module.debug.desc') }}</p>
          </div>
        </div>
        <el-switch v-model="debugMode" class="setting-control-switch" @change="onDebugChange" />
      </div>
    </div>

    <div v-if="serverAccess.supported" class="settings-section glass-effect">
      <h2 class="section-title">{{ t('settings.serverAccess.title') }}</h2>
      <p class="server-hint server-hint--lead">{{ t('settings.serverAccess.desc') }}</p>

      <div class="setting-item setting-item-horizontal">
        <div class="setting-info">
          <div class="setting-icon">
            <Wifi :size="24" />
          </div>
          <div class="setting-text">
            <h3 class="setting-name">{{ t('settings.serverAccess.allow_lan.label') }}</h3>
            <p class="setting-desc">{{ t('settings.serverAccess.allow_lan.desc') }}</p>
          </div>
        </div>
        <el-switch v-model="serverAccess.allowLan" class="setting-control-switch" />
      </div>

      <div class="setting-item setting-item-split">
        <div class="setting-info">
          <div class="setting-icon">
            <Network :size="24" />
          </div>
          <div class="setting-text">
            <h3 class="setting-name">{{ t('settings.serverAccess.port.label') }}</h3>
            <p class="setting-desc">{{ t('settings.serverAccess.port.desc') }}</p>
          </div>
        </div>
        <el-input-number
          v-model="serverAccess.port"
          class="setting-control"
          :min="1"
          :max="65535"
          :step="1"
          step-strictly
          controls-position="right"
        />
      </div>

      <div class="server-rows">
        <div class="server-row">
          <span class="server-label">{{ t('settings.serverAccess.bind') }}</span>
          <span class="server-value server-value--mono">{{ serverAccess.bindPreview }}</span>
        </div>
        <div class="server-row">
          <span class="server-label">{{ t('settings.serverAccess.status') }}</span>
          <span class="server-value">{{ serverAccess.statusLabel }}</span>
        </div>
        <div class="server-row">
          <span class="server-label">Token</span>
          <span class="server-value">
            {{
              serverAccess.hasToken
                ? t('settings.serverAccess.token_present')
                : t('settings.serverAccess.token_missing')
            }}
          </span>
        </div>
      </div>

      <div class="lan-block">
        <span class="server-label">{{ t('settings.serverAccess.lan') }}</span>
        <ul v-if="serverAccess.lanUrls.length > 0" class="lan-list">
          <li v-for="url in serverAccess.lanUrls" :key="url">
            <code>{{ url }}</code>
          </li>
        </ul>
        <p v-else class="server-hint">{{ t('settings.serverAccess.lan_empty') }}</p>
      </div>

      <p v-if="serverAccess.error" class="server-hint server-hint--error">
        {{ serverAccess.error }}
      </p>

      <div class="server-actions">
        <el-button type="primary" :loading="serverAccess.saving" @click="applyServerAccess">
          {{ t('settings.serverAccess.actions.apply') }}
        </el-button>
        <el-button :loading="serverAccess.saving" @click="resetAccessToken">
          {{ t('settings.serverAccess.actions.reset_token') }}
        </el-button>
        <el-button :loading="serverAccess.refreshing" @click="refreshServerAccess">
          {{ t('settings.serverAccess.actions.refresh') }}
        </el-button>
      </div>
    </div>

    <div class="settings-section glass-effect">
      <h2 class="section-title">{{ serverStore.messages.section.title }}</h2>

      <div v-if="serverStore.isReadOnly" class="server-banner">
        {{ serverStore.messages.section.readOnlyHint }}
      </div>

      <div class="server-rows">
        <div class="server-row">
          <span class="server-label">{{ serverStore.messages.section.runtimeLabel }}</span>
          <span class="server-value">{{ serverStore.runtimeLabel }}</span>
        </div>
        <div v-if="serverStore.isNative" class="server-row">
          <span class="server-label">{{ serverStore.messages.section.versionLabel }}</span>
          <span class="server-value">{{ serverStore.serverVersion || '--' }}</span>
        </div>
        <div v-if="serverStore.isNative" class="server-row">
          <span class="server-label">{{ serverStore.messages.section.bindLabel }}</span>
          <span class="server-value">{{ serverStore.bind || '--' }}</span>
        </div>
        <div v-if="serverStore.isNative" class="server-row">
          <span class="server-label">{{ serverStore.messages.section.revisionLabel }}</span>
          <span class="server-value server-value--mono">{{ shortRevision }}</span>
        </div>
        <div v-if="serverStore.isNative" class="server-row">
          <span class="server-label">{{ serverStore.messages.section.configMtimeLabel }}</span>
          <span class="server-value">{{ serverStore.configMtimeLabel }}</span>
        </div>
        <div v-if="serverStore.isNative" class="server-row">
          <span class="server-label">{{ serverStore.messages.section.tokenLabel }}</span>
          <span class="server-value">
            {{
              serverStore.tokenPresent
                ? serverStore.messages.section.tokenPresent
                : serverStore.messages.section.tokenMissing
            }}
          </span>
        </div>
        <div v-if="serverStore.isNative" class="server-row">
          <span class="server-label">{{ serverStore.messages.section.scopesLabel }}</span>
          <span class="server-value">{{ serverStore.scopeSummary }}</span>
        </div>
      </div>

      <p v-if="serverStore.isNative" class="server-hint">
        {{ serverStore.messages.section.degradedHint }}
      </p>
      <p v-if="serverStore.error" class="server-hint server-hint--error">{{ serverStore.error }}</p>

      <div v-if="serverStore.isNative || serverStore.isReadOnly" class="server-actions">
        <el-button :loading="serverStore.loading" @click="refreshServerStatus">
          {{ serverStore.messages.section.refresh }}
        </el-button>
        <template v-if="serverStore.isNative">
          <el-button type="primary" @click="openTokenDialog">
            {{ serverStore.messages.section.login }}
          </el-button>
          <el-button v-if="serverStore.tokenPresent" @click="logoutServer">
            {{ serverStore.messages.section.logout }}
          </el-button>
        </template>
      </div>
    </div>

    <!-- 转换结果对话框 -->
    <el-dialog
      v-model="convertDialogVisible"
      :title="t('settings.dialog.result.title')"
      width="90%"
      :close-on-click-modal="false"
      :append-to-body="true"
      class="template-dialog"
      modal-class="template-dialog-modal"
    >
      <el-form label-width="100px" label-position="top">
        <el-form-item :label="t('settings.dialog.result.template_name_label')">
          <el-input
            v-model="convertedTemplateName"
            :placeholder="t('settings.dialog.result.template_name_placeholder')"
          />
        </el-form-item>
        <el-form-item :label="t('settings.dialog.result.preview_label')">
          <el-input
            v-model="convertedContent"
            type="textarea"
            :rows="10"
            readonly
            class="code-font"
          />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="convertDialogVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" @click="saveConvertedTemplate">{{
          t('settings.dialog.result.btn_save')
        }}</el-button>
      </template>
    </el-dialog>

    <el-dialog
      v-model="serverAccess.tokenDialogVisible"
      :title="t('settings.serverAccess.token_dialog.title')"
      width="90%"
      :close-on-click-modal="false"
      :append-to-body="true"
      class="template-dialog"
      modal-class="template-dialog-modal"
      @closed="serverAccess.dismissTokenDialog()"
    >
      <p class="server-hint">{{ t('settings.serverAccess.token_dialog.desc') }}</p>
      <el-input :model-value="serverAccess.generatedToken" readonly class="code-font" />
      <template #footer>
        <el-button @click="serverAccess.dismissTokenDialog()">
          {{ t('settings.serverAccess.token_dialog.close') }}
        </el-button>
        <el-button type="primary" @click="copyAccessToken">
          {{ t('settings.serverAccess.token_dialog.copy') }}
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, watch, computed, onActivated, onMounted } from 'vue'
import { Moon, Globe, Bug, FileUp, Network, Wifi } from '@lucide/vue'
import { useConfigStore } from '../stores/config'
import { useSettingsStore } from '../stores/settings'
import { useI18n } from '../utils/i18n'
import { useServerStore } from '../stores/server'
import { useServerAccessStore } from '../stores/serverAccess'
import { copyTextToClipboard } from '../utils/templateTransfer'
import { toast } from 'kernelsu-alt'
import type { Template } from '../types'
import { parseFirstTemplateFromToml } from '../utils/templateTransfer'
import { readZipEntries } from '../utils/zipReader'
import { convertZipEntriesToTemplate } from '../utils/propsToTemplate'

const configStore = useConfigStore()
const settingsStore = useSettingsStore()
const serverStore = useServerStore()
const serverAccess = useServerAccessStore()
const { t } = useI18n()

/** revision 太长，界面只展示前 12 位（完整值仍可从服务端取）。 */
const shortRevision = computed(() => {
  const revision = serverStore.revision
  return revision ? `${revision.slice(0, 12)}…` : '--'
})

/** 刷新传输类型与 /status（readonly 时也能看到提示）。 */
async function refreshServerStatus() {
  await serverStore.ensureReady()
}

/** 手动输入 Token（登录成功后重新加载配置）。 */
async function openTokenDialog() {
  const result = await serverStore.promptLogin('missing')
  if (result) {
    await configStore.loadConfig()
  }
}

/** 保存外部访问开关/端口：写 server.toml → 重启服务 → 重新读取。 */
async function applyServerAccess() {
  await serverAccess.save()
}

/** 重置访问 Token（旧 Token 立即失效，明文只显示一次）。 */
async function resetAccessToken() {
  await serverAccess.resetToken()
}

/** 重新读取 server.toml、服务状态与本机地址。 */
async function refreshServerAccess() {
  await serverAccess.load()
}

/** 复制一次性展示的访问 Token。 */
async function copyAccessToken() {
  const copied = await copyTextToClipboard(serverAccess.generatedToken)
  if (copied) {
    toast(t('settings.serverAccess.messages.token_copied'))
  }
}

/** 退出登录：只清本地 Token 与服务状态，不主动发请求（否则会立刻再弹登录框）。 */
function logoutServer() {
  serverStore.logout()
}

const currentTheme = ref(settingsStore.theme)
const currentLanguage = ref(settingsStore.language)
const debugMode = ref(configStore.config.debug || false)

const convertDialogVisible = ref(false)
const converting = ref(false)
const convertedTemplate = ref<Template | null>(null)
const convertedTemplateName = ref('')
const convertedContent = ref('')

function onThemeChange(value: string | number) {
  settingsStore.setTheme(value as 'system' | 'light' | 'dark')
}

function onLanguageChange(value: string | number) {
  settingsStore.setLanguage(value as 'system' | 'zh' | 'en' | 'tr')
}

const themeOptions = computed(() => [
  { value: 'system', label: t('settings.display.theme.system') },
  { value: 'light', label: t('settings.display.theme.light') },
  { value: 'dark', label: t('settings.display.theme.dark') },
])

const languageOptions = computed(() => [
  { value: 'system', label: t('settings.display.language.system') },
  { value: 'zh', label: t('settings.display.language.zh') },
  { value: 'en', label: t('settings.display.language.en') },
  { value: 'tr', label: t('settings.display.language.tr') },
])

async function onDebugChange(value: boolean) {
  configStore.config.debug = value
  try {
    await configStore.saveConfig()
    toast(value ? t('settings.messages.debug_enabled') : t('settings.messages.debug_disabled'))
  } catch {
    toast(t('settings.messages.save_failed'))
  }
}

function pickZipFile(): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document.createElement('input')
    input.type = 'file'
    input.accept = '.zip'
    input.onchange = (event) => {
      const target = event.target as HTMLInputElement
      const file = target.files?.[0] || null
      resolve(file)
    }
    input.click()
  })
}

async function startConversion() {
  if (converting.value) return

  if (import.meta.env?.DEV) {
    const { mockConfig } = await import('../utils/mockData')
    const { templateData, defaultName } = parseFirstTemplateFromToml(mockConfig)
    convertedTemplate.value = templateData
    convertedTemplateName.value = defaultName
    convertedContent.value = mockConfig
    convertDialogVisible.value = true
    return
  }

  const file = await pickZipFile()
  if (!file) {
    return
  }

  converting.value = true

  try {
    // ZIP 在 WebView 内直接解析：不落盘、不 fork CLI、不依赖 manager 版本
    const entries = await readZipEntries(await file.arrayBuffer())
    const { name, template, toml } = convertZipEntriesToTemplate(entries)
    convertedTemplate.value = template
    convertedTemplateName.value = name
    convertedContent.value = toml
    convertDialogVisible.value = true
  } catch (err) {
    toast(
      `${t('settings.messages.convert_failed')}: ${err instanceof Error ? err.message : String(err)}`
    )
    console.error(err)
  } finally {
    converting.value = false
  }
}

async function saveConvertedTemplate() {
  if (!convertedTemplateName.value) {
    toast(t('settings.dialog.result.template_name_placeholder'))
    return
  }
  if (!convertedTemplate.value) return

  try {
    configStore.setTemplate(convertedTemplateName.value, convertedTemplate.value)
    await configStore.saveConfig()
    toast(t('settings.messages.template_saved'))
    convertDialogVisible.value = false
  } catch (err) {
    toast(
      `${t('settings.messages.save_failed')}: ${err instanceof Error ? err.message : String(err)}`
    )
  }
}

// 监听配置变化（只创建一次监听器）
watch(
  () => configStore.config.debug,
  (newDebug: boolean | undefined) => {
    const newValue = newDebug || false
    if (debugMode.value !== newValue) {
      debugMode.value = newValue
    }
  }
)

// 首次挂载时读一次服务状态（onActivated 只在 KeepAlive 场景触发，不能只靠它）
onMounted(() => {
  void serverStore.ensureReady()
  if (serverAccess.supported) {
    void serverAccess.load()
  }
})

// KeepAlive 激活时同步最新配置
onActivated(() => {
  currentTheme.value = settingsStore.theme
  currentLanguage.value = settingsStore.language
  debugMode.value = configStore.config.debug || false
  void serverStore.ensureReady()
})
</script>

<style scoped>
.settings-page {
  display: flex;
  flex-direction: column;
  gap: 1rem;
  width: 100%;
  max-width: 100%;
  box-sizing: border-box;
  /* 确保宽度稳定，不受滚动条影响 */
  overflow: hidden;
}

.settings-section {
  padding: 1.5rem;
  border-radius: 1rem;
  box-shadow: 0 2px 8px var(--shadow);
}

.section-title {
  font-size: 1.125rem;
  font-weight: 600;
  color: var(--text);
  margin-bottom: 1rem;
}

.setting-item {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  padding: 1rem 0;
  border-bottom: 1px solid var(--border);
}

.setting-item:last-child {
  border-bottom: none;
}

.setting-info {
  display: flex;
  align-items: center;
  gap: 1rem;
  flex: 1;
}

.setting-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  background: var(--background);
  border-radius: 0.5rem;
  color: var(--primary);
  flex-shrink: 0;
}

.setting-text {
  flex: 1;
  overflow: hidden;
}

.setting-name {
  font-size: 1rem;
  font-weight: 500;
  color: var(--text);
  margin: 0 0 0.25rem 0;
  line-height: 1.5;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.setting-desc {
  font-size: 0.875rem;
  color: var(--text-secondary);
  margin: 0;
  line-height: 1.5;
}

.setting-control {
  width: 100%;
}

.server-banner {
  padding: 0.75rem 1rem;
  margin-bottom: 0.75rem;
  border-radius: 0.75rem;
  border: 1px solid var(--border);
  background: var(--background);
  color: var(--text-secondary);
  font-size: 0.85rem;
  line-height: 1.5;
}

.server-rows {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.server-row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 1rem;
  font-size: 0.9rem;
}

.server-label {
  color: var(--text-secondary);
  flex-shrink: 0;
}

.server-value {
  color: var(--text);
  text-align: right;
  overflow-wrap: anywhere;
}

.server-value--mono {
  font-family: monospace;
}

.server-hint {
  margin: 0.75rem 0 0;
  font-size: 0.8rem;
  line-height: 1.5;
  color: var(--text-secondary);
}

.server-hint--error {
  color: #dc2626;
}

.server-hint--lead {
  margin: 0 0 0.75rem;
}

.lan-block {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  margin-top: 0.75rem;
}

.lan-list {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  margin: 0;
  padding-left: 1.1rem;
  font-size: 0.85rem;
}

.lan-list code {
  font-family: monospace;
  overflow-wrap: anywhere;
}

.server-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
  margin-top: 1rem;
}

.setting-item-horizontal {
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
}

.setting-item-split {
  flex-direction: row;
  align-items: flex-start;
  justify-content: space-between;
  gap: 1rem;
}

.setting-item-split .setting-info {
  min-width: 0;
}

.setting-item-split .setting-name {
  white-space: normal;
  text-overflow: clip;
}

.setting-item-split .setting-desc {
  overflow-wrap: anywhere;
}

.setting-item-split .setting-control {
  width: clamp(7rem, 30vw, 11rem);
  min-width: 7rem;
  flex-shrink: 1;
}

.setting-control-switch {
  flex-shrink: 0;
  margin-left: 1rem;
}

/* Code font for preview */
.code-font {
  font-family: monospace;
}

/* Dialog styles (copied from TemplatePage for consistency) */
.template-dialog :deep(.el-dialog) {
  margin-top: 5vh !important;
  margin-bottom: 80px !important;
  max-height: calc(100vh - 80px - 10vh) !important;
  display: flex;
  flex-direction: column;
  background: rgba(255, 255, 255, 0.15) !important;
  backdrop-filter: blur(40px) saturate(150%) brightness(1.1);
  border: 1px solid rgba(255, 255, 255, 0.4);
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.15);
}

@media (prefers-color-scheme: dark) {
  .template-dialog :deep(.el-dialog) {
    background: rgba(20, 20, 20, 0.6) !important;
    backdrop-filter: blur(40px) saturate(150%) brightness(0.9);
    border: 1px solid rgba(255, 255, 255, 0.15);
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  }
}

.template-dialog :deep(.el-dialog__body) {
  flex: 1;
  overflow-y: auto;
  padding: 1.5rem;
  background: transparent;
  /* 隐藏滚动条 */
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.template-dialog :deep(.el-dialog__body::-webkit-scrollbar) {
  display: none;
}

.template-dialog :deep(.el-dialog__header) {
  background: rgba(255, 255, 255, 0.1);
  backdrop-filter: blur(10px);
  border-bottom: 1px solid rgba(255, 255, 255, 0.2);
}

@media (prefers-color-scheme: dark) {
  .template-dialog :deep(.el-dialog__header) {
    background: rgba(0, 0, 0, 0.2);
    border-bottom: 1px solid rgba(255, 255, 255, 0.1);
  }
}

.template-dialog :deep(.el-dialog__footer) {
  padding: 1rem 1.5rem;
  border-top: 1px solid rgba(255, 255, 255, 0.2);
  background: rgba(255, 255, 255, 0.15);
  backdrop-filter: blur(20px);
  flex-shrink: 0;
}

@media (prefers-color-scheme: dark) {
  .template-dialog :deep(.el-dialog__footer) {
    border-top: 1px solid rgba(255, 255, 255, 0.1);
    background: rgba(0, 0, 0, 0.3);
  }
}

.template-dialog :deep(.el-overlay) {
  z-index: 2000 !important;
  backdrop-filter: blur(12px) saturate(120%) !important;
  background-color: rgba(0, 0, 0, 0.25) !important;
}

@media (prefers-color-scheme: dark) {
  .template-dialog :deep(.el-overlay) {
    backdrop-filter: blur(12px) saturate(120%) !important;
    background-color: rgba(0, 0, 0, 0.4) !important;
  }
}

.form-tip {
  font-size: 0.75rem;
  color: var(--text-secondary);
  margin-top: 0.25rem;
}
</style>
