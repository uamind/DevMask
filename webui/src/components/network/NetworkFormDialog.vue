<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <el-dialog
    :model-value="modelValue"
    :title="t('settings.egress.proxy.title')"
    width="92%"
    top="6vh"
    class="net-dialog"
    modal-class="net-dialog-modal"
    :close-on-click-modal="false"
    :append-to-body="true"
    @update:model-value="onVisibleChange"
  >
    <p class="net-hint">{{ t('settings.egress.desc') }}</p>

    <div class="net-form">
      <!-- 出口开关（设备级） -->
      <div class="net-toggle">
        <div class="net-toggle__text">
          <p class="net-field__label">{{ t('settings.egress.enable.label') }}</p>
          <p class="net-field__tip">{{ t('settings.egress.enable.desc') }}</p>
        </div>
        <el-switch
          v-model="egress.form.enabled"
          :aria-label="t('settings.egress.enable.label')"
          data-testid="egress-enabled"
        />
      </div>

      <!-- 代理定义：名字 / 协议 / 主机 / 端口 / 凭据 -->
      <div class="net-field">
        <p class="net-field__label">{{ t('settings.egress.fields.name.label') }}</p>
        <el-input
          v-model="egress.form.name"
          :aria-label="t('settings.egress.fields.name.label')"
          :placeholder="t('settings.egress.fields.name.placeholder')"
        />
        <p class="net-field__tip">{{ t('settings.egress.fields.name.desc') }}</p>
        <p v-if="egress.fieldErrors.name" class="net-field__tip net-field__tip--error">
          {{ egress.fieldErrors.name }}
        </p>
      </div>

      <div class="net-field">
        <p class="net-field__label">{{ t('settings.egress.fields.type.label') }}</p>
        <BaseSelect
          v-model="egress.form.type"
          :options="egressTypeOptions"
          :aria-label="t('settings.egress.fields.type.label')"
        />
      </div>

      <div class="net-field">
        <p class="net-field__label">{{ t('settings.egress.fields.host.label') }}</p>
        <el-input
          v-model="egress.form.host"
          :aria-label="t('settings.egress.fields.host.label')"
          :placeholder="t('settings.egress.fields.host.placeholder')"
        />
        <p v-if="egress.fieldErrors.host" class="net-field__tip net-field__tip--error">
          {{ egress.fieldErrors.host }}
        </p>
      </div>

      <div class="net-field">
        <p class="net-field__label">{{ t('settings.egress.fields.port.label') }}</p>
        <el-input-number
          v-model="egress.form.port"
          :aria-label="t('settings.egress.fields.port.label')"
          :min="1"
          :max="65535"
          :step="1"
          step-strictly
          controls-position="right"
        />
        <p v-if="egress.fieldErrors.port" class="net-field__tip net-field__tip--error">
          {{ egress.fieldErrors.port }}
        </p>
      </div>

      <div class="net-field">
        <p class="net-field__label">{{ t('settings.egress.fields.username.label') }}</p>
        <el-input
          v-model="egress.form.username"
          :aria-label="t('settings.egress.fields.username.label')"
          autocomplete="off"
          :placeholder="t('settings.egress.fields.username.placeholder')"
        />
        <p v-if="egress.fieldErrors.username" class="net-field__tip net-field__tip--error">
          {{ egress.fieldErrors.username }}
        </p>
      </div>

      <div class="net-field">
        <p class="net-field__label">{{ t('settings.egress.fields.password.label') }}</p>
        <el-input
          v-model="egress.form.password"
          :aria-label="t('settings.egress.fields.password.label')"
          type="password"
          show-password
          autocomplete="new-password"
          :placeholder="passwordPlaceholder"
        />
        <p class="net-field__tip">{{ credentialsLabel }}</p>
        <p v-if="egress.fieldErrors.password" class="net-field__tip net-field__tip--error">
          {{ egress.fieldErrors.password }}
        </p>
      </div>

      <!-- 清凭据 -->
      <div class="net-inline">
        <el-checkbox
          v-model="egress.form.clearCredentials"
          :disabled="!egress.hasCredentials"
          data-testid="egress-clear-credentials"
        >
          {{ t('settings.egress.fields.clear_credentials.label') }}
        </el-checkbox>
        <span class="net-field__tip">{{ t('settings.egress.fields.clear_credentials.desc') }}</span>
      </div>

      <!-- 高级：探测地址 / 超时 / fail-open / 探测目标 -->
      <div class="net-advanced-toggle">
        <button type="button" class="dm-link" data-testid="egress-advanced" @click="toggleAdvanced">
          {{ t('settings.egress.advanced.toggle') }}
          <ChevronDown :size="13" :class="{ 'is-open': egress.advancedOpen }" aria-hidden="true" />
        </button>
      </div>

      <template v-if="egress.advancedOpen">
        <div class="net-field">
          <p class="net-field__label">{{ t('settings.egress.advanced.probe_url.label') }}</p>
          <el-input
            v-model="egress.form.probeUrl"
            :aria-label="t('settings.egress.advanced.probe_url.label')"
            placeholder="http://checkip.amazonaws.com/"
          />
          <p class="net-field__tip">{{ t('settings.egress.advanced.probe_url.desc') }}</p>
          <p v-if="egress.fieldErrors.probeUrl" class="net-field__tip net-field__tip--error">
            {{ egress.fieldErrors.probeUrl }}
          </p>
        </div>

        <div class="net-field">
          <p class="net-field__label">{{ t('settings.egress.advanced.timeout.label') }}</p>
          <el-input-number
            v-model="egress.form.probeTimeoutSeconds"
            :aria-label="t('settings.egress.advanced.timeout.label')"
            :min="1"
            :max="30"
            :step="1"
            step-strictly
            controls-position="right"
          />
          <p
            v-if="egress.fieldErrors.probeTimeoutSeconds"
            class="net-field__tip net-field__tip--error"
          >
            {{ egress.fieldErrors.probeTimeoutSeconds }}
          </p>
        </div>

        <div class="net-field">
          <p class="net-field__label">{{ t('settings.egress.advanced.fail_open.label') }}</p>
          <el-switch
            v-model="egress.form.failOpen"
            :aria-label="t('settings.egress.advanced.fail_open.label')"
          />
          <p class="net-field__tip">{{ t('settings.egress.advanced.fail_open.desc') }}</p>
        </div>

        <div class="net-field">
          <p class="net-field__label">{{ t('settings.egress.advanced.probe_target.label') }}</p>
          <BaseSelect
            v-model="egress.probeTarget"
            :options="probeTargetOptions"
            :aria-label="t('settings.egress.advanced.probe_target.label')"
          />
          <p class="net-field__tip">{{ t('settings.egress.advanced.probe_target.desc') }}</p>
        </div>
      </template>

      <p v-if="egress.error" class="net-hint net-hint--error" data-testid="egress-form-error">
        {{ egress.error }}
        <span v-if="egress.errorCode" class="net-code">[{{ egress.errorCode }}]</span>
      </p>
    </div>

    <template #footer>
      <div class="net-dialog__footer">
        <el-button @click="close">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="egress.saving" data-testid="egress-save" @click="save">
          {{ t('settings.egress.actions.save') }}
        </el-button>
      </div>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { ChevronDown } from '@lucide/vue'
import BaseSelect from '../shared/BaseSelect.vue'
import { useEgressStore } from '../../stores/egress'
import { useI18n } from '../../utils/i18n'
import { useModalHistory } from '../../composables/useModalHistory'

/**
 * 出口定义编辑弹窗。
 *
 * 与设置页原来的「网络出口」表单一一对应：开关、代理定义（名字/协议/主机/端口/
 * 用户名/密码/清凭据）、高级（探测地址/超时/fail-open/探测目标）。
 * 校验与保存全部走 egress store（本地校验 + PUT /egress），本组件不改写任何调用。
 */
const props = defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()

const egress = useEgressStore()
const { t } = useI18n()

const visible = computed<boolean>({
  get: () => props.modelValue,
  set: (value: boolean) => emit('update:modelValue', value),
})

function close(): void {
  visible.value = false
}

// 系统返回键先关这个弹窗（与模板/应用弹窗同一套 history 语义）
useModalHistory(visible, close)

function onVisibleChange(value: boolean): void {
  visible.value = value
}

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

function toggleAdvanced(): void {
  egress.advancedOpen = !egress.advancedOpen
}

async function save(): Promise<void> {
  const ok = await egress.save()
  if (ok) close()
}
</script>
