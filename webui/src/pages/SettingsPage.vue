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

    <div class="settings-section glass-effect">
      <h2 class="section-title">{{ t('settings.egress.title') }}</h2>
      <p class="server-hint server-hint--lead">{{ t('settings.egress.desc') }}</p>
      <div class="server-banner">{{ t('settings.egress.runtime.' + egress.runtime) }}</div>

      <template v-if="egress.supported">
        <div class="setting-item setting-item-horizontal">
          <div class="setting-info">
            <div class="setting-icon">
              <Route :size="24" />
            </div>
            <div class="setting-text">
              <h3 class="setting-name">{{ t('settings.egress.enable.label') }}</h3>
              <p class="setting-desc">{{ t('settings.egress.enable.desc') }}</p>
            </div>
          </div>
          <el-switch v-model="egress.form.enabled" class="setting-control-switch" />
        </div>

        <el-form label-position="top" class="egress-form">
          <div class="egress-grid">
            <el-form-item :label="t('settings.egress.fields.name.label')">
              <el-input
                v-model="egress.form.name"
                :placeholder="t('settings.egress.fields.name.placeholder')"
              />
              <p class="form-tip">{{ t('settings.egress.fields.name.desc') }}</p>
              <p v-if="egress.fieldErrors.name" class="form-tip form-tip--error">
                {{ egress.fieldErrors.name }}
              </p>
            </el-form-item>
            <el-form-item :label="t('settings.egress.fields.type.label')">
              <BaseSelect
                v-model="egress.form.type"
                :options="egressTypeOptions"
                :aria-label="t('settings.egress.fields.type.label')"
              />
            </el-form-item>
            <el-form-item :label="t('settings.egress.fields.host.label')">
              <el-input
                v-model="egress.form.host"
                :placeholder="t('settings.egress.fields.host.placeholder')"
              />
              <p v-if="egress.fieldErrors.host" class="form-tip form-tip--error">
                {{ egress.fieldErrors.host }}
              </p>
            </el-form-item>
            <el-form-item :label="t('settings.egress.fields.port.label')">
              <el-input-number
                v-model="egress.form.port"
                :min="1"
                :max="65535"
                :step="1"
                step-strictly
                controls-position="right"
              />
              <p v-if="egress.fieldErrors.port" class="form-tip form-tip--error">
                {{ egress.fieldErrors.port }}
              </p>
            </el-form-item>
            <el-form-item :label="t('settings.egress.fields.username.label')">
              <el-input
                v-model="egress.form.username"
                autocomplete="off"
                :placeholder="t('settings.egress.fields.username.placeholder')"
              />
              <p v-if="egress.fieldErrors.username" class="form-tip form-tip--error">
                {{ egress.fieldErrors.username }}
              </p>
            </el-form-item>
            <el-form-item :label="t('settings.egress.fields.password.label')">
              <el-input
                v-model="egress.form.password"
                type="password"
                show-password
                autocomplete="new-password"
                :placeholder="passwordPlaceholder"
              />
              <p class="form-tip">{{ credentialsLabel }}</p>
              <p v-if="egress.fieldErrors.password" class="form-tip form-tip--error">
                {{ egress.fieldErrors.password }}
              </p>
            </el-form-item>
          </div>

          <div class="egress-inline">
            <el-checkbox v-model="egress.form.clearCredentials" :disabled="!egress.hasCredentials">
              {{ t('settings.egress.fields.clear_credentials.label') }}
            </el-checkbox>
            <span class="form-tip">{{ t('settings.egress.fields.clear_credentials.desc') }}</span>
          </div>

          <div class="egress-inline">
            <el-button link type="primary" @click="toggleAdvanced">
              {{ t('settings.egress.advanced.toggle') }}
            </el-button>
          </div>

          <div v-if="egress.advancedOpen" class="egress-grid">
            <el-form-item :label="t('settings.egress.advanced.probe_url.label')">
              <el-input
                v-model="egress.form.probeUrl"
                placeholder="http://checkip.amazonaws.com/"
              />
              <p class="form-tip">{{ t('settings.egress.advanced.probe_url.desc') }}</p>
              <p v-if="egress.fieldErrors.probeUrl" class="form-tip form-tip--error">
                {{ egress.fieldErrors.probeUrl }}
              </p>
            </el-form-item>
            <el-form-item :label="t('settings.egress.advanced.timeout.label')">
              <el-input-number
                v-model="egress.form.probeTimeoutSeconds"
                :min="1"
                :max="30"
                :step="1"
                step-strictly
                controls-position="right"
              />
              <p v-if="egress.fieldErrors.probeTimeoutSeconds" class="form-tip form-tip--error">
                {{ egress.fieldErrors.probeTimeoutSeconds }}
              </p>
            </el-form-item>
            <el-form-item :label="t('settings.egress.advanced.fail_open.label')">
              <el-switch v-model="egress.form.failOpen" />
              <p class="form-tip">{{ t('settings.egress.advanced.fail_open.desc') }}</p>
            </el-form-item>
            <el-form-item :label="t('settings.egress.advanced.probe_target.label')">
              <BaseSelect
                v-model="egress.probeTarget"
                :options="probeTargetOptions"
                :aria-label="t('settings.egress.advanced.probe_target.label')"
              />
              <p class="form-tip">{{ t('settings.egress.advanced.probe_target.desc') }}</p>
            </el-form-item>
          </div>
        </el-form>

        <h3 class="setting-name egress-status-title">{{ t('settings.egress.status.title') }}</h3>
        <div class="server-rows">
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.mode') }}</span>
            <span class="server-value">{{ egress.modeLabel }}</span>
          </div>
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.effective_mode') }}</span>
            <span class="server-value">{{ egress.effectiveModeLabel }}</span>
          </div>
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.applied') }}</span>
            <span class="server-value">{{ egress.appliedLabel }}</span>
          </div>
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.readback') }}</span>
            <span class="server-value server-value--mono">{{ readbackLabel }}</span>
          </div>
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.revision') }}</span>
            <span class="server-value server-value--mono">{{ egress.revisionLabel }}</span>
          </div>
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.credentials') }}</span>
            <span class="server-value">{{ credentialsLabel }}</span>
          </div>
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.egress_file') }}</span>
            <span class="server-value">{{ egressFileLabel }}</span>
          </div>
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.degraded') }}</span>
            <span class="server-value">
              {{
                egress.degraded
                  ? t('settings.egress.status.degraded_yes')
                  : t('settings.egress.status.degraded_no')
              }}
            </span>
          </div>
          <div class="server-row">
            <span class="server-label">{{ t('settings.egress.status.last_probe') }}</span>
            <span class="server-value">{{ lastProbeLabel }}</span>
          </div>
        </div>

        <p
          v-for="(text, index) in egress.warningTexts"
          :key="index"
          class="server-hint server-hint--warn"
        >
          {{ text }}
        </p>

        <p v-if="egress.error" class="server-hint server-hint--error">
          {{ egress.error }}
          <span v-if="egress.errorCode" class="egress-code">[{{ egress.errorCode }}]</span>
          <span v-if="egress.errorMessage"> · {{ egress.errorMessage }}</span>
          <span v-if="egress.errorDetail"> · {{ egress.errorDetail }}</span>
        </p>

        <div v-if="egress.probeResult" class="egress-probe">
          <div class="egress-probe-head">
            <h3 class="setting-name">{{ t('settings.egress.probe.title') }}</h3>
            <span v-if="egress.probeApproximate" class="egress-badge">
              {{ t('settings.egress.probe.approximate') }}
            </span>
          </div>
          <div class="server-rows">
            <div class="server-row">
              <span class="server-label">{{ t('settings.egress.probe.exit_ip') }}</span>
              <span class="server-value server-value--mono">{{ egress.probeResult.exitIp }}</span>
            </div>
            <div class="server-row">
              <span class="server-label">{{ t('settings.egress.probe.latency') }}</span>
              <span class="server-value">{{ egress.probeResult.latencyMs }} ms</span>
            </div>
            <div class="server-row">
              <span class="server-label">{{ t('settings.egress.probe.via') }}</span>
              <span class="server-value">
                {{
                  egress.probeResult.via === 'proxy'
                    ? t('settings.egress.modes.proxy')
                    : t('settings.egress.modes.direct')
                }}
                <template v-if="egress.probeResult.proxy">
                  · {{ egress.probeResult.proxy }}</template
                >
              </span>
            </div>
            <div class="server-row">
              <span class="server-label">{{ t('settings.egress.probe.checked_at') }}</span>
              <span class="server-value">{{ egress.probeResult.checkedAt }}</span>
            </div>
            <div class="server-row">
              <span class="server-label">{{ t('settings.egress.probe.upstream') }}</span>
              <span class="server-value">{{ egress.probeResult.upstreamStatus ?? '—' }}</span>
            </div>
            <div class="server-row">
              <span class="server-label">{{ t('settings.egress.probe.scope') }}</span>
              <span class="server-value">
                {{
                  egress.probeResult.scope === 'app'
                    ? t('settings.egress.probe.scope_app')
                    : t('settings.egress.probe.scope_device')
                }}
              </span>
            </div>
          </div>
          <p v-if="egress.probeApproximate" class="form-tip">
            {{ t('settings.egress.probe.approximate_hint') }}
          </p>
        </div>

        <div class="server-actions">
          <el-button type="primary" :loading="egress.saving" @click="saveEgress">
            {{ t('settings.egress.actions.save') }}
          </el-button>
          <el-button
            :loading="egress.probing"
            :disabled="!egress.probeSupported"
            @click="probeEgress"
          >
            {{ t('settings.egress.actions.probe') }}
          </el-button>
          <el-button :loading="egress.clearing" @click="clearEgress">
            {{ t('settings.egress.actions.clear') }}
          </el-button>
          <el-button :loading="egress.loading" @click="loadEgress">
            {{ t('settings.egress.actions.refresh') }}
          </el-button>
        </div>
        <p v-if="!egress.probeSupported" class="form-tip">
          {{ t('settings.egress.actions.probe_unavailable') }} ·
          {{ t('settings.egress.errors.probe_unsupported') }}
        </p>
      </template>
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

      <div class="setting-item setting-item-split">
        <div class="setting-info">
          <div class="setting-icon">
            <KeyRound :size="24" />
          </div>
          <div class="setting-text">
            <h3 class="setting-name">{{ t('settings.serverAccess.token_input.label') }}</h3>
            <p class="setting-desc">{{ t('settings.serverAccess.token_input.desc') }}</p>
          </div>
        </div>
        <el-input
          v-model="serverAccess.tokenDraft"
          class="setting-control"
          type="password"
          show-password
          autocomplete="new-password"
          :placeholder="t('settings.serverAccess.token_input.placeholder')"
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
import { Moon, Globe, Bug, FileUp, KeyRound, Network, Route, Wifi } from '@lucide/vue'
import { useConfigStore } from '../stores/config'
import { useEgressStore } from '../stores/egress'
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
const egress = useEgressStore()
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

// ---- 出口代理（egress）----

const egressTypeOptions = computed(() => [
  { value: 'http', label: t('settings.egress.fields.type.http') },
  { value: 'socks5', label: t('settings.egress.fields.type.socks5') },
])

/** 探测目标：设备级 + 已配置的应用级覆盖。 */
const probeTargetOptions = computed(() => [
  { value: '', label: t('settings.egress.advanced.probe_target.device') },
  ...egress.apps.map((app) => ({ value: app.package, label: app.package })),
])

/** 密码输入框占位：只说「是否已设置」，永不回显。 */
const passwordPlaceholder = computed(() =>
  egress.hasCredentials
    ? t('settings.egress.fields.password.placeholder_set')
    : t('settings.egress.fields.password.placeholder_unset')
)

const credentialsLabel = computed(() =>
  egress.hasCredentials
    ? t('settings.egress.status.credentials_set')
    : t('settings.egress.status.credentials_unset')
)

const readbackLabel = computed(() => {
  if (!egress.snapshot) return '--'
  if (!egress.readbackAvailable) return t('settings.egress.status.applied_unknown')
  const value = egress.snapshot.readback.settingsValue
  return value && value.trim() ? value : t('settings.egress.status.readback_none')
})

const egressFileLabel = computed(() => {
  const info = egress.snapshot?.egressFile
  if (!info) return '--'
  if (!info.exists) return t('settings.egress.status.egress_file_missing')
  return `${info.size} B${info.mtime ? ` · ${info.mtime}` : ''}`
})

const lastProbeLabel = computed(() => {
  const probe = egress.lastProbe
  if (!probe) return t('settings.egress.status.never')
  const result = probe.ok
    ? (probe.exitIp ?? '--')
    : (probe.errorCode ?? t('settings.egress.errors.probe_failed'))
  return `${result} · ${probe.latencyMs ?? 0} ms · ${probe.checkedAt}`
})

function toggleAdvanced() {
  egress.advancedOpen = !egress.advancedOpen
}

async function saveEgress() {
  await egress.save()
}

async function probeEgress() {
  await egress.probe()
}

async function clearEgress() {
  await egress.clear()
}

async function loadEgress() {
  await egress.load()
}

// 首次挂载时读一次服务状态（onActivated 只在 KeepAlive 场景触发，不能只靠它）
onMounted(() => {
  void serverStore.ensureReady()
  if (serverAccess.supported) {
    void serverAccess.load()
  }
  void egress.load()
})

// KeepAlive 激活时同步最新配置
onActivated(() => {
  currentTheme.value = settingsStore.theme
  currentLanguage.value = settingsStore.language
  debugMode.value = configStore.config.debug || false
  void serverStore.ensureReady()
  void egress.load()
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

.egress-form {
  margin-top: 0.75rem;
}

.egress-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(11rem, 1fr));
  gap: 0.25rem 1rem;
}

.egress-inline {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.5rem;
  margin-bottom: 0.75rem;
  font-size: 0.8rem;
  color: var(--text-secondary);
}

.egress-status-title {
  margin: 1rem 0 0.5rem;
}

.egress-probe {
  margin-top: 1rem;
  padding: 0.75rem 1rem;
  border: 1px solid var(--border);
  border-radius: 0.75rem;
  background: var(--background);
}

.egress-probe-head {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  margin-bottom: 0.5rem;
}

.egress-badge {
  padding: 0.05rem 0.4rem;
  border-radius: 0.4rem;
  background: rgba(234, 179, 8, 0.16);
  color: #b45309;
  font-size: 0.7rem;
  font-weight: 600;
}

.egress-code {
  font-family: monospace;
  font-size: 0.75rem;
  opacity: 0.85;
}

.server-hint--warn {
  color: #b45309;
}

.form-tip--error {
  color: #dc2626;
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
