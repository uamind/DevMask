<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <section class="dm-card net-card" data-testid="network-probe">
    <div class="net-card__head">
      <h2 class="dm-section__title">{{ t('settings.egress.probe.title') }}</h2>
      <span v-if="egress.probeResult && egress.probeApproximate" class="net-tag net-tag--default">
        {{ t('settings.egress.probe.approximate') }}
      </span>
    </div>

    <div class="net-inline net-probe__target">
      <span class="net-hint">
        {{ t('settings.egress.advanced.probe_target.label') }} · {{ targetLabel }}
      </span>
      <button type="button" class="dm-link" @click="emit('edit')">{{ t('common.edit') }}</button>
    </div>

    <div class="net-actions">
      <el-button
        type="primary"
        size="small"
        :loading="egress.probing"
        :disabled="!egress.probeSupported || egress.probing"
        data-testid="network-probe-run"
        @click="runProbe"
      >
        {{ t('settings.egress.actions.probe') }}
      </el-button>
      <span v-if="!egress.probeSupported" class="net-hint">
        {{ t('settings.egress.actions.probe_unavailable') }}
      </span>
    </div>

    <div v-if="egress.probeResult" class="net-rows" data-testid="network-probe-result">
      <NetworkRow
        :label="t('settings.egress.probe.exit_ip')"
        :value="egress.probeResult.exitIp || '--'"
        mono
      />
      <NetworkRow :label="t('settings.egress.probe.latency')" :value="latencyLabel" mono />
      <NetworkRow :label="t('settings.egress.probe.via')" :value="viaLabel" />
      <NetworkRow
        :label="t('settings.egress.probe.checked_at')"
        :value="egress.probeResult.checkedAt || '--'"
      />
      <NetworkRow
        :label="t('settings.egress.probe.upstream')"
        :value="String(egress.probeResult.upstreamStatus ?? '—')"
      />
      <NetworkRow :label="t('settings.egress.probe.scope')" :value="scopeLabel" />
    </div>
    <p v-else class="net-hint net-probe__empty">
      {{ t('settings.egress.status.never') }}
    </p>

    <p v-if="egress.probeApproximate" class="net-hint">
      {{ t('settings.egress.probe.approximate_hint') }}
    </p>
    <p v-for="(text, index) in probeWarnings" :key="index" class="net-hint net-hint--warn">
      {{ text }}
    </p>
  </section>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import NetworkRow from './NetworkRow.vue'
import { useEgressStore } from '../../stores/egress'
import { useI18n } from '../../utils/i18n'
import { relayEndpoint } from '../../backend'
import type { EgressProbeVia } from '../../backend'

/**
 * 探测结果卡。
 *
 * 「探测目标应用」在这里显示当前值，编辑入口指回出口定义弹窗的「高级」分组，
 * 避免同一个字段出现两份互相打架的控件。
 */
const egress = useEgressStore()
const { t } = useI18n()

const emit = defineEmits<{
  edit: []
}>()

const targetLabel = computed(
  () => egress.probeTarget || t('settings.egress.advanced.probe_target.device')
)

function probeViaLabel(via: EgressProbeVia): string {
  if (via === 'relay') return t('settings.egress.modes.relay')
  if (via === 'direct') return t('settings.egress.modes.direct')
  return t('settings.egress.modes.proxy')
}

const viaLabel = computed(() => {
  const result = egress.probeResult
  if (!result) return '--'
  const parts = [probeViaLabel(result.via)]
  if (result.via === 'relay' && result.relayPort !== null) {
    parts.push(relayEndpoint(result.relayPort))
  }
  if (result.proxy) parts.push(result.proxy)
  return parts.join(' · ')
})

const latencyLabel = computed(() => `${egress.probeResult?.latencyMs ?? 0} ms`)

const scopeLabel = computed(() =>
  egress.probeResult?.scope === 'app'
    ? t('settings.egress.probe.scope_app')
    : t('settings.egress.probe.scope_device')
)

const probeWarnings = computed(() =>
  (egress.probeResult?.warnings ?? []).map((warning) => egress.warningText(warning))
)

async function runProbe(): Promise<void> {
  await egress.probe()
}
</script>

<style scoped>
.net-probe__target {
  margin-bottom: 2px;
}

.net-probe__empty {
  margin-top: 10px;
}
</style>
