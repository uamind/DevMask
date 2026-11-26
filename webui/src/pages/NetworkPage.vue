<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div class="net-page">
    <header class="dm-page-head">
      <h1 class="dm-page-title">{{ t('network.title') }}</h1>
      <p class="dm-page-sub">{{ t('network.subtitle') }}</p>
    </header>

    <!-- 页头两块：出口状态（已连接 / 直连 / 未启用）+ 常驻「一键直连」 -->
    <NetworkStatusCard />

    <template v-if="egress.supported">
      <NetworkProxyList @edit="openEditor" />
      <NetworkProbeCard @edit="openAdvancedEditor" />
      <NetworkRelayCard />
    </template>

    <NetworkFormDialog v-model="editorOpen" />
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import NetworkStatusCard from '../components/network/NetworkStatusCard.vue'
import NetworkProxyList from '../components/network/NetworkProxyList.vue'
import NetworkProbeCard from '../components/network/NetworkProbeCard.vue'
import NetworkRelayCard from '../components/network/NetworkRelayCard.vue'
import NetworkFormDialog from '../components/network/NetworkFormDialog.vue'
import { useEgressStore } from '../stores/egress'
import { useI18n } from '../utils/i18n'
import '../components/network/network.css'

/**
 * 网络页（第 5 个 tab）：设备端出口的唯一配置入口。
 *
 * 结构按方向 B：状态卡 → 代理卡列表 → 探测结果卡 → 认证中继卡，
 * 编辑走弹窗；深色由 .dm-card / .net-* 的令牌切换成头发丝描边卡（方向 A）。
 * 读写全部经由 egress store（PUT/DELETE /egress、探测、重启中继），本页不新增 API。
 */
const egress = useEgressStore()
const { t } = useI18n()

const editorOpen = ref(false)

function openEditor(): void {
  editorOpen.value = true
}

/** 探测卡的「编辑」直接展开高级分组：探测目标就在那一组里。 */
function openAdvancedEditor(): void {
  egress.openAdvanced()
  editorOpen.value = true
}

onMounted(() => {
  void egress.load()
})
</script>
