<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div class="status-page">
    <header class="dm-page-head">
      <h1 class="dm-page-title">{{ t('overview.title') }}</h1>
      <p class="dm-page-sub">{{ deviceLine }}</p>
    </header>

    <!-- hero：一个应用，一个身份 -->
    <section class="hero">
      <div class="hero__phones" aria-hidden="true"><i></i><i></i><i></i></div>
      <div class="hero__eyebrow">{{ t('overview.hero.eyebrow') }}</div>
      <h2 class="hero__title">
        <span v-if="configReady" class="hero__count dm-mono">{{ appsCountDisplay }}</span>
        <span v-else class="hero__count-skeleton dm-skeleton" aria-hidden="true"></span>
        <span class="hero__word">{{ t('overview.hero.suffix') }}</span>
        <br />
        <span class="hero__word">{{ t('overview.hero.line2') }}</span>
      </h2>
      <p class="hero__sub">{{ heroSub }}</p>
      <div class="hero__foot">
        <button type="button" class="hero__cta" @click="go('apps')">
          {{ t('overview.hero.manage') }}
          <svg width="13" height="13" viewBox="0 0 24 24" class="hero__arrow" aria-hidden="true">
            <path d="M5 12h13M13 6l6 6-6 6" />
          </svg>
        </button>
        <button type="button" class="hero__ghost" @click="go('templates')">
          {{ t('overview.hero.new_template') }}
        </button>
      </div>
    </section>

    <!-- 2×2 指标 -->
    <div class="tiles">
      <article class="dm-tile">
        <span class="dm-tile__icon" aria-hidden="true">
          <FileText :size="16" :stroke-width="1.7" />
        </span>
        <span class="dm-tile__body">
          <span class="dm-tile__key">{{ t('overview.metrics.templates_label') }}</span>
          <span v-if="configReady" class="dm-tile__value">
            {{ t('overview.metrics.templates_value', { count: configStore.templateCount }) }}
          </span>
          <span v-else class="dm-skeleton tile-skeleton" aria-hidden="true"></span>
        </span>
      </article>

      <article class="dm-tile">
        <span :class="['dm-tile__icon', egressToneClass]" aria-hidden="true">
          <Globe :size="16" :stroke-width="1.7" />
        </span>
        <span class="dm-tile__body">
          <span class="dm-tile__key">{{ t('overview.metrics.egress_label') }}</span>
          <span class="dm-tile__value">{{ egressLabel }}</span>
        </span>
      </article>

      <article class="dm-tile">
        <span class="dm-tile__icon dm-tile__icon--violet" aria-hidden="true">
          <ShieldCheck :size="16" :stroke-width="1.7" />
        </span>
        <span class="dm-tile__body">
          <span class="dm-tile__key">{{ t('status.items.module_version') }}</span>
          <span class="dm-tile__value dm-tile__value--mono">{{ moduleVersionLabel }}</span>
        </span>
      </article>

      <article class="dm-tile">
        <span class="dm-tile__icon" aria-hidden="true">
          <Clock :size="16" :stroke-width="1.7" />
        </span>
        <span class="dm-tile__body">
          <span class="dm-tile__key">{{ t('overview.metrics.last_saved_label') }}</span>
          <span class="dm-tile__value">{{ lastSavedLabel }}</span>
        </span>
      </article>
    </div>

    <!-- 常用模板 -->
    <section class="dm-section">
      <div class="dm-section__head">
        <h3 class="dm-section__title">{{ t('overview.templates.title') }}</h3>
        <button type="button" class="dm-link" @click="go('templates')">
          {{ t('overview.templates.all', { count: configStore.templateCount }) }}
        </button>
      </div>

      <div v-if="topTemplates.length > 0" class="tpl-row">
        <button
          v-for="item in topTemplates"
          :key="item.name"
          type="button"
          class="tpl"
          :title="item.name"
          @click="go('templates')"
        >
          <span class="tpl__name">{{ item.label }}</span>
          <span v-if="item.model" class="tpl__model dm-mono">{{ item.model }}</span>
          <span class="tpl__count">{{ t('overview.templates.apps', { count: item.count }) }}</span>
        </button>
      </div>
      <div v-else-if="!configReady" class="tpl-row" aria-hidden="true">
        <span class="tpl tpl--skeleton"><span class="dm-skeleton tpl__line" /></span>
        <span class="tpl tpl--skeleton"><span class="dm-skeleton tpl__line" /></span>
      </div>
      <p v-else class="dm-empty">
        {{ t('overview.templates.empty') }}<br />{{ t('overview.templates.empty_hint') }}
      </p>
    </section>

    <!-- 最近改动 -->
    <section class="recent">
      <div class="recent__head">
        <span class="recent__title">{{ t('overview.recent.title') }}</span>
      </div>
      <ul v-if="recentItems.length > 0" class="recent__list">
        <li v-for="item in recentItems" :key="item.id" class="act">
          <i :class="['act__dot', item.tone === 'ok' ? 'act__dot--ok' : '']" aria-hidden="true"></i>
          <span class="act__text">{{ item.text }}</span>
          <span v-if="item.time" class="act__time dm-mono">{{ item.time }}</span>
        </li>
      </ul>
      <p v-else class="recent__empty">
        {{ t('overview.recent.empty') }}<br />{{ t('overview.recent.empty_hint') }}
      </p>
    </section>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { Clock, FileText, Globe, ShieldCheck } from '@lucide/vue'
import { useConfigStore } from '../stores/config'
import { useEgressStore } from '../stores/egress'
import { useServerStore } from '../stores/server'
import { useI18n } from '../utils/i18n'
import { APP_VERSION, navigateToPage } from '../composables/usePageNavigation'
import type { PageId } from '../stores/navigation'

type EgressState = 'connected' | 'direct' | 'unknown'

interface RecentItem {
  id: string
  text: string
  time: string
  tone: 'brand' | 'ok'
}

const configStore = useConfigStore()
const egressStore = useEgressStore()
const serverStore = useServerStore()
const { t } = useI18n()

const configReady = computed(() => configStore.configReady)

const appsCountDisplay = computed(() => String(configStore.deviceFakerCount))

/**
 * 模块版本：module.prop 读到什么就显示什么（设备口径）。
 * 只有 moduleMeta 还没就绪（首帧 / 读取失败）时才回落到本版 WebUI 的 v0.0.1。
 */
const moduleVersionLabel = computed(() => {
  if (configStore.moduleMetaReady && configStore.moduleVersion) {
    return configStore.moduleVersion
  }
  return APP_VERSION
})

/** 概览只读出口状态：真正读写出口的是「网络」页。 */
const egressState = computed<EgressState>(() => {
  if (!egressStore.supported) return 'unknown'
  const snapshot = egressStore.snapshot
  if (!snapshot) return 'unknown'
  const proxied = snapshot.effectiveMode === 'proxy' || Boolean(snapshot.default)
  return proxied ? 'connected' : 'direct'
})

const egressLabel = computed(() => {
  if (egressState.value === 'connected') return t('network.state.connected')
  if (egressState.value === 'direct') return t('network.state.direct')
  return t('network.state.unknown')
})

const egressToneClass = computed(() => {
  if (egressState.value === 'connected') return 'dm-tile__icon--ok'
  return ''
})

const heroSub = computed(() =>
  t('overview.hero.sub', {
    templates: configStore.templateCount,
    egress: egressLabel.value,
  })
)

/** 设备信息只有原生服务模式能读到；管理器 / 开发环境下用一句话说明当前语义。 */
const deviceLine = computed(() => {
  const device = serverStore.status?.device
  if (device?.model) {
    const name = [device.manufacturer || device.brand, device.model].filter(Boolean).join(' ')
    return t('overview.subtitle', {
      device: name,
      android: device.android || '--',
    })
  }
  return t('overview.subtitle_fallback')
})

const lastSavedLabel = computed(() => {
  const mtime = serverStore.status?.config?.mtime
  if (!mtime) return t('overview.metrics.unknown')
  return formatRelative(mtime)
})

const topTemplates = computed(() =>
  [...configStore.templateEntries]
    .map(({ name, template }) => {
      const model = (template.model || template.device || template.product || '').trim()
      const market = (template.marketname || '').trim()
      return {
        name,
        // 标题优先市场名，其次型号，最后退回 TOML key——避免 key 在窄卡片里被断词
        label: market || model || name,
        // 市场名已经表达清楚时不再重复型号；只有市场名 + 型号同时存在才显示副标题
        model: market ? model : '',
        count: template.packages?.length ?? 0,
      }
    })
    .sort((a, b) => b.count - a.count)
    .slice(0, 3)
)

const recentItems = computed<RecentItem[]>(() => {
  const items: RecentItem[] = []
  const mtime = serverStore.status?.config?.mtime
  if (mtime) {
    items.push({
      id: 'config',
      text: t('overview.recent.config_saved'),
      time: formatRelative(mtime),
      tone: 'brand',
    })
  }
  if (egressState.value === 'connected') {
    items.push({
      id: 'egress',
      text: t('overview.recent.egress_connected'),
      time: '',
      tone: 'ok',
    })
  }
  return items
})

function formatRelative(value: string): string {
  const time = new Date(value).getTime()
  if (!Number.isFinite(time)) return t('overview.metrics.unknown')

  const diffMs = Date.now() - time
  const minutes = Math.floor(diffMs / 60000)
  if (minutes < 1) return t('overview.time.just_now')
  if (minutes < 60) return t('overview.time.minutes', { n: minutes })

  const hours = Math.floor(minutes / 60)
  if (hours < 24) return t('overview.time.hours', { n: hours })

  const days = Math.floor(hours / 24)
  if (days < 30) return t('overview.time.days', { n: days })

  return new Date(time).toLocaleDateString()
}

function go(pageId: PageId) {
  navigateToPage(pageId)
}

onMounted(() => {
  // 概览要回答「出口通不通」：只在还没读过时拉一次，不干扰网络页的编辑态
  if (egressStore.supported && !egressStore.snapshot && !egressStore.loading) {
    void egressStore.load()
  }
})
</script>

<style scoped>
.status-page {
  display: flex;
  flex-direction: column;
  width: 100%;
  max-width: 100%;
  box-sizing: border-box;
  overflow: hidden;
  padding-top: 4px;
}

/* —— hero —— */
.hero {
  position: relative;
  overflow: hidden;
  background: var(--hero-bg);
  border: 1px solid var(--hero-line);
  border-radius: 22px;
  padding: 16px 20px 14px;
  color: var(--hero-ink);
  box-shadow: var(--shadow-hero);
}

.hero::before {
  content: '';
  position: absolute;
  top: -90px;
  right: -70px;
  width: 220px;
  height: 220px;
  border-radius: 50%;
  background: radial-gradient(circle at center, var(--hero-glow-a), transparent 65%);
  pointer-events: none;
}

.hero::after {
  content: '';
  position: absolute;
  right: -30px;
  bottom: -90px;
  width: 150px;
  height: 150px;
  border-radius: 50%;
  background: radial-gradient(circle at center, var(--hero-glow-b), transparent 68%);
  pointer-events: none;
}

html.dark .hero {
  border-radius: var(--radius-card);
}

.hero__phones {
  position: absolute;
  top: 16px;
  right: 16px;
  display: flex;
  opacity: 0.9;
}

.hero__phones i {
  display: block;
  width: 26px;
  height: 52px;
  margin-left: -6px;
  border: 1.4px solid var(--hero-line);
  border-radius: 7px;
  position: relative;
}

.hero__phones i::before {
  content: '';
  position: absolute;
  top: 5px;
  left: 50%;
  width: 7px;
  height: 1.4px;
  border-radius: 2px;
  background: var(--hero-muted);
  transform: translateX(-50%);
}

.hero__eyebrow {
  position: relative;
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--hero-muted);
}

.hero__title {
  position: relative;
  margin-top: 12px;
  font-size: 28px;
  line-height: 1.2;
  font-weight: 700;
  letter-spacing: -0.02em;
  color: var(--hero-ink);
}

.hero__count {
  color: var(--hero-accent);
  font-size: 28px;
  font-weight: 700;
}

.hero__count-skeleton {
  display: inline-block;
  width: 34px;
  height: 24px;
  vertical-align: -2px;
  background: rgba(255, 255, 255, 0.18);
  animation: none;
}

.hero__word {
  font-size: 28px;
}

/* 数字与「个应用」之间留一点空隙，同时避免把第二行一起缩进 */
.hero__count + .hero__word,
.hero__count-skeleton + .hero__word {
  margin-left: 6px;
}

.hero__sub {
  position: relative;
  margin-top: 7px;
  font-size: 12.5px;
  line-height: 1.55;
  color: var(--hero-muted);
}

.hero__foot {
  position: relative;
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 15px;
}

.hero__cta {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 36px;
  padding: 0 14px;
  border: 0;
  border-radius: var(--radius-pill);
  background: var(--hero-ink);
  color: var(--hero-bg);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 640;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.hero__cta:active {
  opacity: 0.9;
}

/* 深色用 A 的强调色实心按钮，浅色仍是 B 的白色胶囊 */
html.dark .hero__cta {
  background: var(--brand);
  color: #06101f;
}

.hero__arrow {
  stroke: currentColor;
  fill: none;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.hero__ghost {
  padding: 0;
  border: 0;
  background: none;
  color: var(--hero-muted);
  font-family: inherit;
  font-size: 12.5px;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

/* —— 2×2 指标 —— */
.tiles {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px;
  margin-top: 14px;
}

html.dark .dm-tile {
  box-shadow: none;
}

.tile-skeleton {
  width: 54px;
  height: 14px;
  margin-top: 4px;
}

/* —— 常用模板 —— */
.tpl-row {
  display: grid;
  /* 自适应列宽：1 张时只占一格，不撑出空洞；多张时自动换行 */
  grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
  gap: 10px;
}

.tpl {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  padding: 9px 11px;
  border: 1px solid var(--line);
  border-radius: var(--radius-tile);
  background: var(--surface);
  box-shadow: var(--shadow-1);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

html.dark .tpl {
  box-shadow: none;
  background: var(--surface);
}

.tpl--skeleton {
  height: 72px;
  justify-content: center;
  cursor: default;
}

.tpl__line {
  width: 70%;
  height: 12px;
}

.tpl__name {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  overflow-wrap: anywhere;
  font-size: 12px;
  font-weight: 620;
  line-height: 1.35;
  color: var(--ink);
}

.tpl__model {
  font-size: 9.5px;
  color: var(--dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tpl__count {
  margin-top: 4px;
  font-size: 10.5px;
  color: var(--muted);
}

/* —— 最近改动 —— */
.recent {
  margin-top: 14px;
  border: 1px solid var(--line);
  border-radius: var(--radius-card);
  background: var(--surface);
  box-shadow: var(--shadow-1);
  overflow: hidden;
}

html.dark .recent {
  box-shadow: none;
}

.recent__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 11px 15px 8px;
}

.recent__title {
  font-size: 12.5px;
  font-weight: 640;
  color: var(--ink);
}

html.dark .recent__title {
  font-family: var(--font-mono);
  font-size: 11px;
  font-weight: 400;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--dim);
}

.recent__list {
  list-style: none;
}

.act {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 15px;
  border-top: 1px solid var(--line);
}

.act__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--brand);
  flex: none;
}

.act__dot--ok {
  background: var(--ok);
}

.act__text {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

html.dark .act__text {
  color: var(--muted);
}

.act__time {
  flex: none;
  font-size: 10.5px;
  color: var(--dim);
}

.recent__empty {
  padding: 12px 15px 14px;
  border-top: 1px solid var(--line);
  font-size: 11.5px;
  line-height: 1.65;
  color: var(--dim);
}
</style>
