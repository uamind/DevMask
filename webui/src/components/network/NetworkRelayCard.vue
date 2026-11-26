<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <section v-if="relay" class="dm-card net-card" data-testid="egress-relay">
    <div class="net-card__head">
      <h2 class="dm-section__title">{{ t('settings.egress.relay.title') }}</h2>
      <span
        :class="['net-tag', relay.enabled ? 'net-tag--relay' : 'net-tag--type']"
        data-testid="relay-enabled-badge"
      >
        {{
          relay.enabled
            ? t('settings.egress.relay.enabled_on')
            : t('settings.egress.relay.enabled_off')
        }}
      </span>
    </div>

    <p class="net-hint">{{ t('settings.egress.relay.desc') }}</p>

    <div class="net-rows">
      <NetworkRow
        :label="t('settings.egress.relay.base_port')"
        :value="String(relay.basePort)"
        mono
      />
      <NetworkRow
        :label="t('settings.egress.relay.fail_open')"
        :value="
          relay.failOpen
            ? t('settings.egress.relay.fail_open_on')
            : t('settings.egress.relay.fail_open_off')
        "
      />
      <NetworkRow :label="t('settings.egress.relay.limits')" :value="limitsLabel" mono />
      <NetworkRow :label="t('settings.egress.relay.firewall')" :value="firewallLabel" />
      <NetworkRow :label="t('settings.egress.relay.at')" :value="relay.at || '—'" mono />
    </div>

    <p
      v-if="relay.firewall.lastError"
      class="net-hint net-hint--error"
      data-testid="relay-firewall-error"
    >
      {{ t('settings.egress.relay.firewall_error') }}:
      <span class="net-code">{{ relay.firewall.lastError.code }}</span>
      · {{ relay.firewall.lastError.reason }}
      <template v-if="relay.firewall.lastError.at"> · {{ relay.firewall.lastError.at }}</template>
    </p>

    <div class="net-actions">
      <el-button
        size="small"
        :loading="egress.restarting"
        :disabled="!egress.relayRestartSupported || egress.restarting"
        data-testid="relay-restart"
        @click="restart"
      >
        {{ t('settings.egress.relay.restart') }}
      </el-button>
      <span v-if="!egress.relayRestartSupported" class="net-hint">
        {{ t('settings.egress.relay.restart_unavailable') }}
      </span>
    </div>

    <h3 class="net-subtitle">{{ t('settings.egress.relay.listeners') }}</h3>
    <p v-if="egress.relayListeners.length === 0" class="net-hint">
      {{ t('settings.egress.relay.listener_empty') }}
    </p>
    <div v-else class="net-listeners">
      <div
        v-for="listener in egress.relayListeners"
        :key="listener.proxy + ':' + listener.port"
        class="net-listener"
        data-testid="relay-listener"
      >
        <div class="net-listener__head">
          <span class="net-proxy__name">{{ listener.proxy }}</span>
          <span :class="['net-tag', listener.listening ? 'net-tag--relay' : 'net-tag--off']">
            {{
              listener.listening
                ? t('settings.egress.relay.listener_listening')
                : t('settings.egress.relay.listener_down')
            }}
          </span>
        </div>
        <div class="net-listener__meta">
          {{ relayEndpoint(listener.port) }} ·
          {{
            listener.scope === 'app'
              ? t('settings.egress.relay.scope_app')
              : t('settings.egress.relay.scope_device')
          }}
          · {{ t('settings.egress.relay.connections') }} {{ listener.activeConnections }} ·
          {{
            listener.uidPolicy === 'restricted'
              ? t('settings.egress.relay.uid_restricted', {
                  uids: listener.allowedUids.length ? listener.allowedUids.join(', ') : '—',
                })
              : t('settings.egress.relay.uid_all')
          }}
        </div>
        <p
          v-if="listener.lastError"
          class="net-hint net-hint--error"
          data-testid="relay-listener-error"
        >
          {{ t('settings.egress.relay.last_error') }}:
          <span class="net-code">{{ listener.lastError.code }}</span>
          · {{ listener.lastError.reason }}
        </p>
      </div>
    </div>

    <template v-if="egress.relayPortChanges.length > 0">
      <h3 class="net-subtitle">{{ t('settings.egress.relay.port_changes') }}</h3>
      <div class="net-rows net-rows--flush">
        <NetworkRow
          v-for="change in egress.relayPortChanges"
          :key="change.proxy + ':' + change.reason"
          :label="change.proxy"
          :value="`${change.from ?? '—'} → ${change.to ?? '—'} · ${change.reason}`"
          mono
        />
      </div>
    </template>

    <p
      v-if="egress.relayRestartRequired.length > 0"
      class="net-hint net-hint--warn"
      data-testid="relay-restart-required"
    >
      {{
        t('settings.egress.relay.restart_required', {
          apps: egress.relayRestartRequired.join(', '),
        })
      }}
    </p>
  </section>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import NetworkRow from './NetworkRow.vue'
import { useEgressStore } from '../../stores/egress'
import { useI18n } from '../../utils/i18n'
import { relayEndpoint } from '../../backend'

/**
 * 设备内中继（认证中继）卡片。
 *
 * 旧服务端 / 管理器模式没有 relay 字段时整块不渲染（store 里 relay 为 null），
 * 这里不做「假装有中继」的兜底：端口与监听必须来自服务端快照。
 */
const egress = useEgressStore()
const { t } = useI18n()

const relay = computed(() => egress.relay)

const limitsLabel = computed(() => {
  const current = relay.value
  if (!current) return '--'
  return `${current.limits.maxConnections} / ${current.limits.maxConnectionsPerUid}`
})

const firewallLabel = computed(() => {
  const current = relay.value
  if (!current) return '--'
  const state = current.firewall.applied
    ? t('settings.egress.relay.firewall_applied')
    : t('settings.egress.relay.firewall_missing')
  const rules = [
    `ALLOW ${current.firewall.allowRules}`,
    `DENY ${current.firewall.denyRules}`,
    `${t('settings.egress.relay.firewall_rules')} ${current.firewall.rules}`,
  ].join(' · ')
  return `${state} · ${rules}`
})

async function restart(): Promise<void> {
  await egress.restartRelay()
}
</script>
