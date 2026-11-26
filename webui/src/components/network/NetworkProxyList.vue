<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <section class="dm-card net-card" data-testid="network-proxies">
    <div class="net-card__head">
      <h2 class="dm-section__title">{{ t('settings.egress.proxy.title') }}</h2>
      <button type="button" class="dm-link" data-testid="network-edit" @click="emit('edit')">
        {{ t('common.edit') }}
      </button>
    </div>

    <div v-if="egress.proxies.length > 0" class="net-proxy-list">
      <article
        v-for="proxy in egress.proxies"
        :key="proxy.name"
        class="net-proxy"
        data-testid="network-proxy"
      >
        <div class="net-proxy__top">
          <span class="net-proxy__name">{{ proxy.name }}</span>
          <span v-if="isDefault(proxy)" class="net-tag net-tag--default">
            {{ t('settings.egress.proxy.default_badge') }}
          </span>
          <span class="net-tag net-tag--type">{{ proxy.type.toUpperCase() }}</span>
        </div>
        <span class="net-proxy__addr">{{ proxy.host }}:{{ proxy.port }}</span>
        <div class="net-proxy__foot">
          <span :class="['net-tag', pathTagClass(proxy)]">{{ pathLabel(proxy) }}</span>
          <span class="net-proxy__cred">
            {{
              proxy.hasCredentials
                ? t('settings.egress.status.credentials_set')
                : t('settings.egress.status.credentials_unset')
            }}
          </span>
        </div>
      </article>
    </div>

    <div v-else class="net-empty">
      <p class="dm-empty">{{ t('network.empty.title') }}<br />{{ t('network.empty.hint') }}</p>
      <button type="button" class="net-btn net-btn--primary" @click="emit('edit')">
        {{ t('common.add') }}
      </button>
    </div>
  </section>
</template>

<script setup lang="ts">
import { useEgressStore } from '../../stores/egress'
import { useI18n } from '../../utils/i18n'
import { relayEndpoint } from '../../backend'
import type { EgressProxySummary } from '../../backend'

/**
 * 代理定义卡列表。
 *
 * 只做展示：每条定义给出协议、地址、凭据状态与实际链路（经中继 / 直连上游 /
 * 凭据未生效）。编辑走右上角「编辑」→ 打开出口定义弹窗（与设置页原来的
 * 单表单语义完全一致：编辑的就是设备级默认出口）。
 */
const egress = useEgressStore()
const { t } = useI18n()

const emit = defineEmits<{
  edit: []
}>()

function isDefault(proxy: EgressProxySummary): boolean {
  return egress.snapshot?.default === proxy.name
}

function pathLabel(proxy: EgressProxySummary): string {
  const path = egress.proxyPath(proxy)
  if (path === 'relay') {
    return t('settings.egress.proxy.path_relay', { endpoint: relayEndpoint(proxy.localPort) })
  }
  if (path === 'credentials-inactive') {
    return t('settings.egress.proxy.path_credentials_off')
  }
  return t('settings.egress.proxy.path_upstream')
}

function pathTagClass(proxy: EgressProxySummary): string {
  const path = egress.proxyPath(proxy)
  if (path === 'relay') return 'net-tag--relay'
  if (path === 'credentials-inactive') return 'net-tag--off'
  return 'net-tag--upstream'
}
</script>
