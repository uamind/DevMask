<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div class="settings-page">
    <header class="dm-page-head">
      <h1 class="dm-page-title">{{ t('nav.settings') }}</h1>
      <p class="dm-page-sub">{{ t('about.description') }}</p>
    </header>

    <!-- 外观：主题 + 语言（主题默认跟随系统） -->
    <section class="dm-section">
      <div class="dm-section__head">
        <h2 class="dm-section__title">{{ t('settings.appearance.title') }}</h2>
      </div>

      <div class="dm-card set-card">
        <div class="set-row">
          <span class="set-row__icon" aria-hidden="true">
            <Moon :size="17" :stroke-width="1.8" />
          </span>
          <div class="set-row__text">
            <h3 class="set-row__name">{{ t('settings.display.theme.label') }}</h3>
            <p class="set-row__desc">{{ t('settings.display.theme.desc') }}</p>
          </div>
          <BaseSelect
            v-model="currentTheme"
            class="set-row__select"
            :options="themeOptions"
            :aria-label="t('settings.display.theme.label')"
            @change="onThemeChange"
          />
        </div>

        <div class="set-row">
          <span class="set-row__icon" aria-hidden="true">
            <Globe :size="17" :stroke-width="1.8" />
          </span>
          <div class="set-row__text">
            <h3 class="set-row__name">{{ t('settings.display.language.label') }}</h3>
            <p class="set-row__desc">{{ t('settings.display.language.desc') }}</p>
          </div>
          <BaseSelect
            v-model="currentLanguage"
            class="set-row__select"
            :options="languageOptions"
            :aria-label="t('settings.display.language.label')"
            @change="onLanguageChange"
          />
        </div>
      </div>
    </section>

    <!-- 网络出口已独立成「网络」页：设置里只留这一行入口，不重复任何出口表单 -->
    <button type="button" class="entry dm-card" @click="goNetwork">
      <span class="entry__icon" aria-hidden="true">
        <Route :size="17" :stroke-width="1.8" />
      </span>
      <span class="entry__text">
        <span class="entry__name">{{ t('settings.egress.title') }}</span>
        <span class="entry__desc">{{ t('network.subtitle') }}</span>
      </span>
      <span class="entry__go">
        {{ t('nav.network') }}
        <ArrowRight :size="13" aria-hidden="true" />
      </span>
    </button>

    <!-- 高级：模块调试 / 工具 / 外部访问与 API 令牌 / 服务状态 -->
    <section class="dm-section">
      <div class="dm-section__head">
        <h2 class="dm-section__title">{{ t('settings.advanced.title') }}</h2>
      </div>

      <div class="set-stack">
        <div class="dm-card set-card">
          <div class="set-card__head">{{ t('settings.module.title') }}</div>

          <div class="set-row">
            <span class="set-row__icon" aria-hidden="true">
              <Bug :size="17" :stroke-width="1.8" />
            </span>
            <div class="set-row__text">
              <h3 class="set-row__name">{{ t('settings.module.debug.label') }}</h3>
              <p class="set-row__desc">{{ t('settings.module.debug.desc') }}</p>
            </div>
            <el-switch v-model="debugMode" class="set-switch" @change="onDebugChange" />
          </div>

          <!-- 日志入口：调试日志落在模块运行目录，路径可直接复制 -->
          <div class="set-log">
            <span class="set-log__icon" aria-hidden="true">
              <Terminal :size="14" :stroke-width="1.8" />
            </span>
            <code class="set-log__path dm-mono">{{ LOG_PATH }}</code>
            <button type="button" class="dm-link" @click="copyLogPath">
              {{ t('settings.serverAccess.token_dialog.copy') }}
            </button>
          </div>
        </div>

        <div class="dm-card set-card">
          <div class="set-card__head">{{ t('settings.tools.title') }}</div>

          <div class="set-row">
            <span class="set-row__icon" aria-hidden="true">
              <FileUp :size="17" :stroke-width="1.8" />
            </span>
            <div class="set-row__text">
              <h3 class="set-row__name">{{ t('settings.tools.convert.label') }}</h3>
              <p class="set-row__desc">{{ t('settings.tools.convert.desc') }}</p>
            </div>
            <el-button type="primary" :loading="converting" @click="startConversion">
              {{ t('settings.tools.convert.btn') }}
            </el-button>
          </div>
        </div>

        <div v-if="serverAccess.supported" class="dm-card set-card">
          <div class="set-card__head">{{ t('settings.serverAccess.title') }}</div>
          <p class="set-hint set-hint--lead">{{ t('settings.serverAccess.desc') }}</p>

          <div class="set-row">
            <span class="set-row__icon" aria-hidden="true">
              <Wifi :size="17" :stroke-width="1.8" />
            </span>
            <div class="set-row__text">
              <h3 class="set-row__name">{{ t('settings.serverAccess.allow_lan.label') }}</h3>
              <p class="set-row__desc">{{ t('settings.serverAccess.allow_lan.desc') }}</p>
            </div>
            <el-switch v-model="serverAccess.allowLan" class="set-switch" />
          </div>

          <div class="set-row set-row--split">
            <span class="set-row__icon" aria-hidden="true">
              <Network :size="17" :stroke-width="1.8" />
            </span>
            <div class="set-row__text">
              <h3 class="set-row__name">{{ t('settings.serverAccess.port.label') }}</h3>
              <p class="set-row__desc">{{ t('settings.serverAccess.port.desc') }}</p>
            </div>
            <el-input-number
              v-model="serverAccess.port"
              class="set-control"
              :min="1"
              :max="65535"
              :step="1"
              step-strictly
              controls-position="right"
            />
          </div>

          <div class="set-row set-row--split">
            <span class="set-row__icon" aria-hidden="true">
              <KeyRound :size="17" :stroke-width="1.8" />
            </span>
            <div class="set-row__text">
              <h3 class="set-row__name">{{ t('settings.serverAccess.token_input.label') }}</h3>
              <p class="set-row__desc">{{ t('settings.serverAccess.token_input.desc') }}</p>
            </div>
            <el-input
              v-model="serverAccess.tokenDraft"
              class="set-control"
              type="password"
              show-password
              autocomplete="new-password"
              :placeholder="t('settings.serverAccess.token_input.placeholder')"
            />
          </div>

          <div class="set-rows">
            <div class="set-kv">
              <span class="set-kv__k">{{ t('settings.serverAccess.bind') }}</span>
              <span class="set-kv__v set-kv__v--mono">{{ serverAccess.bindPreview }}</span>
            </div>
            <div class="set-kv">
              <span class="set-kv__k">{{ t('settings.serverAccess.status') }}</span>
              <span class="set-kv__v">{{ serverAccess.statusLabel }}</span>
            </div>
            <div class="set-kv">
              <span class="set-kv__k">Token</span>
              <span class="set-kv__v">
                {{
                  serverAccess.hasToken
                    ? t('settings.serverAccess.token_present')
                    : t('settings.serverAccess.token_missing')
                }}
              </span>
            </div>
          </div>

          <div class="set-lan">
            <span class="set-kv__k">{{ t('settings.serverAccess.lan') }}</span>
            <ul v-if="serverAccess.lanUrls.length > 0" class="set-lan__list">
              <li v-for="url in serverAccess.lanUrls" :key="url">
                <code>{{ url }}</code>
              </li>
            </ul>
            <p v-else class="set-hint">{{ t('settings.serverAccess.lan_empty') }}</p>
          </div>

          <p v-if="serverAccess.error" class="set-hint set-hint--error">
            {{ serverAccess.error }}
          </p>

          <div class="set-actions">
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

        <div class="dm-card set-card">
          <div class="set-card__head">{{ serverStore.messages.section.title }}</div>

          <div v-if="serverStore.isReadOnly" class="set-banner">
            {{ serverStore.messages.section.readOnlyHint }}
          </div>

          <div class="set-rows set-rows--flush">
            <div class="set-kv">
              <span class="set-kv__k">{{ serverStore.messages.section.runtimeLabel }}</span>
              <span class="set-kv__v">{{ serverStore.runtimeLabel }}</span>
            </div>
            <div v-if="serverStore.isNative" class="set-kv">
              <span class="set-kv__k">{{ serverStore.messages.section.versionLabel }}</span>
              <span class="set-kv__v">{{ serverStore.serverVersion || '--' }}</span>
            </div>
            <div v-if="serverStore.isNative" class="set-kv">
              <span class="set-kv__k">{{ serverStore.messages.section.bindLabel }}</span>
              <span class="set-kv__v">{{ serverStore.bind || '--' }}</span>
            </div>
            <div v-if="serverStore.isNative" class="set-kv">
              <span class="set-kv__k">{{ serverStore.messages.section.revisionLabel }}</span>
              <span class="set-kv__v set-kv__v--mono">{{ shortRevision }}</span>
            </div>
            <div v-if="serverStore.isNative" class="set-kv">
              <span class="set-kv__k">{{ serverStore.messages.section.configMtimeLabel }}</span>
              <span class="set-kv__v">{{ serverStore.configMtimeLabel }}</span>
            </div>
            <div v-if="serverStore.isNative" class="set-kv">
              <span class="set-kv__k">{{ serverStore.messages.section.tokenLabel }}</span>
              <span class="set-kv__v">
                {{
                  serverStore.tokenPresent
                    ? serverStore.messages.section.tokenPresent
                    : serverStore.messages.section.tokenMissing
                }}
              </span>
            </div>
            <div v-if="serverStore.isNative" class="set-kv">
              <span class="set-kv__k">{{ serverStore.messages.section.scopesLabel }}</span>
              <span class="set-kv__v">{{ serverStore.scopeSummary }}</span>
            </div>
          </div>

          <p v-if="serverStore.isNative" class="set-hint">
            {{ serverStore.messages.section.degradedHint }}
          </p>
          <p v-if="serverStore.error" class="set-hint set-hint--error">
            {{ serverStore.error }}
          </p>

          <div v-if="serverStore.isNative || serverStore.isReadOnly" class="set-actions">
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
      </div>
    </section>

    <!-- 关于：版本 / 作者 / 仓库 / 关注模块 / 译者 / 许可 -->
    <section class="dm-section">
      <div class="dm-section__head">
        <h2 class="dm-section__title">{{ t('about.title') }}</h2>
      </div>

      <div class="dm-card set-card about-card">
        <div class="about-brand">
          <span class="about-brand__mark" aria-hidden="true">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none">
              <path
                d="M4.5 8.2c0-1 .8-1.8 1.8-1.8h11.4c1 0 1.8.8 1.8 1.8v2.1c-1.3.5-2.2 1.7-2.2 3.1 0 1.4.9 2.6 2.2 3.1v2.1c0 1-.8 1.8-1.8 1.8H6.3c-1 0-1.8-.8-1.8-1.8v-2.1c1.3-.5 2.2-1.7 2.2-3.1 0-1.4-.9-2.6-2.2-3.1V8.2z"
                fill="#fff"
              />
              <circle cx="9.2" cy="10.6" r="1.15" fill="var(--brand)" />
              <circle cx="14.8" cy="10.6" r="1.15" fill="var(--brand)" />
            </svg>
          </span>
          <span class="about-brand__body">
            <span class="about-brand__name">DevMask</span>
            <span class="about-brand__desc">{{ t('about.description') }}</span>
          </span>
        </div>

        <div class="set-row">
          <span class="set-row__icon" aria-hidden="true">
            <Package :size="17" :stroke-width="1.8" />
          </span>
          <div class="set-row__text">
            <h3 class="set-row__name">{{ t('about.version') }}</h3>
          </div>
          <span class="set-row__value dm-mono">{{ moduleVersionLabel }}</span>
        </div>

        <div class="set-row">
          <span class="set-row__icon" aria-hidden="true">
            <UserRound :size="17" :stroke-width="1.8" />
          </span>
          <div class="set-row__text">
            <h3 class="set-row__name">{{ t('about.author') }}</h3>
          </div>
          <span class="set-row__value">{{ authorLabel }}</span>
        </div>

        <button type="button" class="set-row set-row--link" @click="openExternal(REPOSITORY_URL)">
          <span class="set-row__icon" aria-hidden="true">
            <ExternalLink :size="17" :stroke-width="1.8" />
          </span>
          <span class="set-row__text">
            <span class="set-row__name">{{ t('about.repository') }}</span>
            <span class="set-row__desc dm-mono">uamind/DevMask</span>
          </span>
          <ChevronRight :size="15" class="set-row__chev" aria-hidden="true" />
        </button>

        <button type="button" class="set-row set-row--link" @click="followDialogVisible = true">
          <span class="set-row__icon" aria-hidden="true">
            <Heart :size="17" :stroke-width="1.8" />
          </span>
          <span class="set-row__text">
            <span class="set-row__name">{{ t('about.follow') }}</span>
            <span class="set-row__desc">{{ t('status.follow.channels') }}</span>
          </span>
          <ChevronRight :size="15" class="set-row__chev" aria-hidden="true" />
        </button>

        <button
          type="button"
          class="set-row set-row--link"
          @click="translatorsDialogVisible = true"
        >
          <span class="set-row__icon" aria-hidden="true">
            <Languages :size="17" :stroke-width="1.8" />
          </span>
          <span class="set-row__text">
            <span class="set-row__name">{{ t('about.translators') }}</span>
            <span class="set-row__desc">{{ t('status.translators.action') }}</span>
          </span>
          <ChevronRight :size="15" class="set-row__chev" aria-hidden="true" />
        </button>

        <button type="button" class="set-row set-row--link" @click="openExternal(LICENSE_URL)">
          <span class="set-row__icon" aria-hidden="true">
            <Scale :size="17" :stroke-width="1.8" />
          </span>
          <span class="set-row__text">
            <span class="set-row__name">{{ t('about.license') }}</span>
            <span class="set-row__desc set-row__desc--license dm-mono">
              SPDX-License-Identifier: {{ t('about.license_value') }}
            </span>
          </span>
          <ChevronRight :size="15" class="set-row__chev" aria-hidden="true" />
        </button>

        <div class="about-foot">
          <span class="dm-mono">{{ t('about.license_value') }}</span>
          <span class="about-foot__sep" aria-hidden="true">·</span>
          <span class="dm-mono">© 2025-2026 Seyud</span>
        </div>
      </div>
    </section>

    <!-- 转换结果对话框 -->
    <el-dialog
      v-model="convertDialogVisible"
      :title="t('settings.dialog.result.title')"
      width="90%"
      :close-on-click-modal="false"
      :append-to-body="true"
      class="template-dialog"
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
        <el-button type="primary" @click="saveConvertedTemplate">
          {{ t('settings.dialog.result.btn_save') }}
        </el-button>
      </template>
    </el-dialog>

    <el-dialog
      v-model="serverAccess.tokenDialogVisible"
      :title="t('settings.serverAccess.token_dialog.title')"
      width="90%"
      :close-on-click-modal="false"
      :append-to-body="true"
      class="template-dialog"
      @closed="serverAccess.dismissTokenDialog()"
    >
      <p class="set-hint">{{ t('settings.serverAccess.token_dialog.desc') }}</p>
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

    <!-- 关注模块 / 译者：直接复用 status 分组的既有弹窗 -->
    <FollowDialog v-model="followDialogVisible" />
    <TranslatorsDialog v-model="translatorsDialogVisible" />
  </div>
</template>

<script setup lang="ts">
import { ref, watch, computed, onActivated, onMounted } from 'vue'
import {
  ArrowRight,
  Bug,
  ChevronRight,
  ExternalLink,
  FileUp,
  Globe,
  Heart,
  KeyRound,
  Languages,
  Moon,
  Network,
  Package,
  Route,
  Scale,
  Terminal,
  UserRound,
  Wifi,
} from '@lucide/vue'
import FollowDialog from '../components/status/FollowDialog.vue'
import TranslatorsDialog from '../components/status/TranslatorsDialog.vue'
import { useConfigStore } from '../stores/config'
import { useSettingsStore } from '../stores/settings'
import { useI18n } from '../utils/i18n'
import { APP_VERSION, navigateToPage } from '../composables/usePageNavigation'
import { useServerStore } from '../stores/server'
import { useServerAccessStore } from '../stores/serverAccess'
import { backend } from '../backend'
import { copyTextToClipboard, parseFirstTemplateFromToml } from '../utils/templateTransfer'
import { toast } from 'kernelsu-alt'
import type { Template } from '../types'
import { readZipEntries } from '../utils/zipReader'
import { convertZipEntriesToTemplate } from '../utils/propsToTemplate'

const configStore = useConfigStore()
const settingsStore = useSettingsStore()
const serverStore = useServerStore()
const serverAccess = useServerAccessStore()
const { t } = useI18n()

/** 关于分组：仓库、许可与日志入口的固定地址（后端契约不提供，属文档级常量）。 */
const REPOSITORY_URL = 'https://github.com/uamind/DevMask'
const LICENSE_URL = `${REPOSITORY_URL}/blob/main/LICENSE`
const LOG_PATH = '/data/adb/devmask/logs/devmask.log'

/** 出口配置整体搬到「网络」页：设置页只留一行入口。 */
function goNetwork(): void {
  navigateToPage('network')
}

/**
 * 模块版本：module.prop 读到什么就显示什么（与概览同一口径），
 * 只有 moduleMeta 还没就绪（首帧 / 读取失败）时才回落到本版 WebUI 的 v0.0.1。
 */
const moduleVersionLabel = computed(() =>
  configStore.moduleMetaReady && configStore.moduleVersion ? configStore.moduleVersion : APP_VERSION
)

const authorLabel = computed(() => configStore.moduleAuthor || 'Seyud')

/** 关于分组的弹窗（复用 components/status 的既有实现）。 */
const followDialogVisible = ref(false)
const translatorsDialogVisible = ref(false)

/** 深链优先走 backend（Android WebView），失败再回落到浏览器新标签。 */
async function openExternal(url: string) {
  try {
    await backend.openDeepLink(url)
  } catch {
    window.open(url, '_blank', 'noopener,noreferrer')
  }
}

/** 日志入口：路径直接复制，方便在模块运行目录里定位 devmask.log。 */
async function copyLogPath() {
  const copied = await copyTextToClipboard(LOG_PATH)
  if (copied) {
    toast(t('common.success'))
  }
}

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
  width: 100%;
  max-width: 100%;
  box-sizing: border-box;
  overflow: hidden;
  padding-top: 4px;
}

/* 三个分组之间的呼吸：外观 / 网络出口入口 / 高级 / 关于 */
.dm-section {
  margin-top: 16px;
}

/* —— 卡片与行基元（浅色 = B 白卡；深色 = A 的 hairline） —— */
.set-card {
  padding: 2px 15px 6px;
}

.set-card__head {
  padding: 12px 0 8px;
  font-size: 12.5px;
  font-weight: 660;
  color: var(--ink);
}

html.dark .set-card__head {
  font-family: var(--font-mono);
  font-size: 10.5px;
  font-weight: 400;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--dim);
}

.set-stack {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.set-row {
  display: flex;
  align-items: center;
  gap: 11px;
  padding: 12px 0;
}

/* 相邻行之间只用一条 hairline，不叠阴影 */
.set-row + .set-row,
.set-hint--lead + .set-row,
.set-rows + .set-row {
  border-top: 1px solid var(--line);
}

.set-row__icon {
  display: grid;
  place-items: center;
  flex: none;
  width: 32px;
  height: 32px;
  border-radius: 10px;
  background: var(--brand-soft);
  color: var(--brand);
}

.set-row__text {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-width: 0;
}

.set-row__name {
  font-size: 12.5px;
  font-weight: 640;
  line-height: 1.4;
  color: var(--ink);
}

.set-row__desc {
  margin-top: 2px;
  font-size: 11px;
  line-height: 1.55;
  color: var(--muted);
  overflow-wrap: anywhere;
}

/* SPDX 标识整行可读：等宽下略小一号，避免在 GPL-3.0-or-later 中间断行 */
.set-row__desc--license {
  font-size: 10px;
  letter-spacing: -0.01em;
}

.set-row__value {
  flex: none;
  max-width: 52%;
  font-size: 11.5px;
  line-height: 1.5;
  color: var(--ink);
  text-align: right;
  overflow-wrap: anywhere;
}

.set-row__select {
  flex: none;
  width: 148px;
}

/* 三语里土耳其语的主题名最长（Sistemi Takip Et），小一号字号保证不截断 */
.set-row__select :deep(.base-select__trigger) {
  font-size: 12px;
}

/* 端口 / Token：说明占满第一行，控件换行后与文字左对齐，避免输入框被挤到 6 个字符宽 */
.set-row--split {
  flex-wrap: wrap;
  row-gap: 8px;
}

.set-row--split .set-control {
  flex: none;
  width: calc(100% - 43px);
  margin-left: 43px;
}

.set-control {
  width: 100%;
}

.set-switch {
  flex: none;
}

/* 关于分组的跳转行：整行可点，右侧一个 chevron */
.set-row--link {
  width: 100%;
  border: 0;
  background: transparent;
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.set-row--link:active {
  opacity: 0.72;
}

.set-row__chev {
  flex: none;
  color: var(--dim);
}

/* —— 网络出口入口（一行，配置都在「网络」页） —— */
.entry {
  display: flex;
  align-items: center;
  gap: 11px;
  width: 100%;
  margin-top: 12px;
  padding: 13px 15px;
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.entry:active {
  opacity: 0.86;
}

.entry__icon {
  display: grid;
  place-items: center;
  flex: none;
  width: 32px;
  height: 32px;
  border-radius: 10px;
  background: var(--brand-soft);
  color: var(--brand);
}

.entry__text {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.entry__name {
  font-size: 13px;
  font-weight: 640;
  color: var(--ink);
}

.entry__desc {
  font-size: 10.8px;
  line-height: 1.5;
  color: var(--muted);
  overflow-wrap: anywhere;
}

.entry__go {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex: none;
  font-size: 11.5px;
  font-weight: 600;
  color: var(--brand);
}

/* —— 日志入口 —— */
.set-log {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 0 8px;
  border-top: 1px solid var(--line);
}

.set-log__icon {
  display: inline-flex;
  flex: none;
  color: var(--dim);
}

.set-log__path {
  flex: 1;
  min-width: 0;
  font-size: 10.5px;
  color: var(--muted);
  overflow-x: auto;
  white-space: nowrap;
}

/* —— 键值行 / 提示 / 局域网地址 / 操作区 —— */
.set-rows {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 12px 0;
}

.set-row + .set-rows {
  border-top: 1px solid var(--line);
}

.set-rows--flush {
  padding-top: 2px;
}

.set-kv {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  font-size: 11.5px;
  line-height: 1.55;
}

.set-kv__k {
  flex: none;
  color: var(--muted);
}

.set-kv__v {
  color: var(--ink);
  text-align: right;
  overflow-wrap: anywhere;
}

.set-kv__v--mono {
  font-family: var(--font-mono);
  font-variant-numeric: tabular-nums;
  font-size: 11px;
}

.set-hint {
  margin: 8px 0 0;
  font-size: 11.5px;
  line-height: 1.65;
  color: var(--muted);
}

.set-hint--lead {
  margin: 0 0 10px;
}

.set-hint--error {
  color: var(--danger);
}

.set-lan {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px 0 4px;
  font-size: 11.5px;
}

.set-lan__list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding-left: 1.1rem;
  font-size: 11px;
}

.set-lan__list code {
  font-family: var(--font-mono);
  overflow-wrap: anywhere;
}

.set-banner {
  padding: 10px 12px;
  margin: 2px 0 8px;
  border: 1px solid var(--line);
  border-radius: var(--radius-btn);
  background: var(--surface-2);
  color: var(--muted);
  font-size: 11.5px;
  line-height: 1.6;
}

.set-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 12px 0 4px;
  border-top: 1px solid var(--line);
}

/* —— 关于 —— */
.about-brand {
  display: flex;
  align-items: center;
  gap: 11px;
  padding: 14px 0 12px;
}

.about-brand__mark {
  display: grid;
  place-items: center;
  flex: none;
  width: 36px;
  height: 36px;
  border-radius: 11px;
  background: var(--brand);
  box-shadow: 0 6px 14px rgba(44, 91, 255, 0.26);
}

html.dark .about-brand__mark {
  box-shadow: none;
}

.about-brand__body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.about-brand__name {
  font-size: 14.5px;
  font-weight: 720;
  letter-spacing: -0.01em;
  color: var(--ink);
}

.about-brand__desc {
  font-size: 11px;
  color: var(--muted);
}

.about-foot {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  margin-top: 2px;
  padding: 12px 0 6px;
  border-top: 1px solid var(--line);
  font-size: 10px;
  color: var(--dim);
}

.about-foot__sep {
  opacity: 0.55;
}

/* —— 转换结果 / Token 弹窗 —— */
.code-font {
  font-family: var(--font-mono);
}

.template-dialog :deep(.el-dialog__body) {
  max-height: 62vh;
  overflow-y: auto;
}

.template-dialog :deep(.el-form-item:last-child) {
  margin-bottom: 0;
}
</style>
