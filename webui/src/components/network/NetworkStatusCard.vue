<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <section class="dm-card net-card net-status" data-testid="network-status">
    <div class="net-status__top">
      <div class="net-status__id">
        <h2 class="dm-section__title">{{ t('settings.egress.status.title') }}</h2>
        <div class="net-status__state">
          <span :class="['dm-pill', statePillClass]" data-testid="network-state">
            <i class="net-dot" aria-hidden="true"></i>
            {{ stateLabel }}
          </span>
          <span v-if="degraded" class="dm-pill dm-pill--warn">
            {{ t('settings.egress.status.degraded_yes') }}
          </span>
          <span v-if="mismatch" class="dm-pill dm-pill--muted">
            {{ t('settings.egress.status.applied_no') }}
          </span>
        </div>
        <p class="net-status__line" data-testid="network-upstream">{{ summaryLine }}</p>
      </div>

      <!-- 常驻「一键直连」：任何状态下都在页头，二次确认后清空系统代理并回落直连 -->
      <button
        type="button"
        class="net-btn net-btn--primary net-btn--direct"
        :disabled="directDisabled"
        :aria-busy="egress.clearing"
        data-testid="network-direct"
        @click="confirmDirect"
      >
        <Zap :size="14" :stroke-width="1.9" aria-hidden="true" />
        {{ t('network.direct') }}
      </button>
    </div>

    <div class="net-status__meta">
      <span>
        {{ t('settings.egress.status.mode') }} · <b>{{ egress.modeLabel }}</b>
      </span>
      <span>
        {{ t('settings.egress.status.effective_mode') }} · <b>{{ egress.effectiveModeLabel }}</b>
      </span>
      <button
        type="button"
        class="dm-link"
        :disabled="egress.loading"
        data-testid="network-refresh"
        @click="refresh"
      >
        {{ egress.loading ? t('common.loading') : t('settings.egress.actions.refresh') }}
      </button>
    </div>

    <div class="net-rows">
      <NetworkRow :label="t('settings.egress.status.applied')" :value="egress.appliedLabel" />
      <NetworkRow :label="t('settings.egress.status.readback')" :value="readbackLabel" mono />
      <NetworkRow
        :label="t('settings.egress.status.revision')"
        :value="egress.revisionLabel"
        mono
      />
      <NetworkRow :label="t('settings.egress.status.egress_file')" :value="egressFileLabel" />
      <NetworkRow :label="t('settings.egress.status.credentials')" :value="credentialsLabel" />
      <NetworkRow :label="t('settings.egress.status.last_probe')" :value="lastProbeLabel" />
    </div>

    <p v-if="!egress.supported" class="net-hint">
      {{ t('settings.egress.runtime.' + egress.runtime) }}
    </p>

    <p v-if="egress.error" class="net-hint net-hint--error" data-testid="network-error">
      {{ egress.error }}
      <span v-if="egress.errorCode" class="net-code">[{{ egress.errorCode }}]</span>
      <span v-if="egress.errorMessage"> · {{ egress.errorMessage }}</span>
      <span v-if="egress.errorDetail"> · {{ egress.errorDetail }}</span>
    </p>

    <p
      v-for="(text, index) in egress.warningTexts"
      :key="index"
      class="net-hint net-hint--warn"
      data-testid="network-warning"
    >
      {{ text }}
    </p>
  </section>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Zap } from '@lucide/vue'
import NetworkRow from './NetworkRow.vue'
import { useEgressStore } from '../../stores/egress'
import { useI18n } from '../../utils/i18n'
import { useLazyMessageBox } from '../../utils/elementPlus'
import { notify } from '../shared/notify'

/**
 * 网络页页头：出口状态 + 常驻「一键直连」。
 *
 * 三态口径（状态词来自 network.state，禁用态用 common.disabled）：
 * - 已连接：有设备级出口（snapshot.default）或期望模式就是代理；
 * - 直连：已保存过定义，但当前没有启用设备级出口；
 * - 禁用：没读过快照 / 当前运行时不支持出口配置。
 * 数据全部来自 egress store，本组件只读它、不写 API。
 */
const egress = useEgressStore()
const { t } = useI18n()

const degraded = computed(() => egress.degraded)
const mismatch = computed(
  () => Boolean(egress.snapshot) && egress.readbackAvailable && !egress.applied
)

const stateKind = computed<'connected' | 'direct' | 'disabled' | 'unknown'>(() => {
  // 运行时不提供出口接口（只读部署）：出口功能未启用
  if (!egress.supported) return 'disabled'
  const snapshot = egress.snapshot
  if (!snapshot) return 'unknown'
  if (snapshot.default || snapshot.mode === 'proxy') return 'connected'
  // 其余都是直连：包括「刚点了一键直连、定义已被清空」这种最常见的回落状态
  return 'direct'
})

const stateLabel = computed(() => {
  switch (stateKind.value) {
    case 'connected':
      return t('network.state.connected')
    case 'direct':
      return t('network.state.direct')
    case 'disabled':
      return t('network.state.disabled')
    default:
      return t('network.state.unknown')
  }
})

const statePillClass = computed(() =>
  stateKind.value === 'connected' ? 'dm-pill--ok' : 'dm-pill--muted'
)

/** 页头第二行：当前上游出口（没有就说明还没配置）。 */
const summaryLine = computed(() => {
  const snapshot = egress.snapshot
  if (!snapshot) return egress.loading ? t('common.loading') : '--'
  const upstream = snapshot.upstream
  if (upstream) return `${upstream.type.toUpperCase()} · ${upstream.host}:${upstream.port}`
  return t('network.empty.title')
})

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

const directDisabled = computed(() => egress.clearing || egress.loading || !egress.supported)

async function refresh(): Promise<void> {
  await egress.load()
}

/**
 * 一键直连：二次确认 → DELETE /egress（清空系统代理 + 清出口定义）→ 回落直连。
 * 成功提示由 store.clear() 内的 notify 给出（settings.egress.messages.cleared），
 * 这里只补失败提示，避免一次动作弹两条成功 toast。
 */
async function confirmDirect(): Promise<void> {
  if (directDisabled.value) return

  try {
    const messageBox = await useLazyMessageBox()()
    await messageBox.confirm(t('settings.egress.enable.desc'), t('network.direct'), {
      confirmButtonText: t('common.confirm'),
      cancelButtonText: t('common.cancel'),
      type: 'warning',
    })
  } catch {
    // 用户取消：什么都不做
    return
  }

  const ok = await egress.clear()
  if (ok) return
  await notify(egress.error || t('settings.egress.errors.clear_failed'), 'error')
}
</script>

<style scoped>
.net-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  flex: none;
}

.dm-pill--ok .net-dot {
  box-shadow: 0 0 0 3px rgba(14, 159, 110, 0.14);
}

html.dark .dm-pill--ok .net-dot {
  box-shadow: 0 0 0 3px rgba(52, 211, 153, 0.16);
}
</style>
