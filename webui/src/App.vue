<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div :class="['app-shell', { dark: isDark }]">
    <!-- App Shell 顶栏：品牌 + 版本 pill + 运行状态。不参与滚动。 -->
    <header class="shell-bar">
      <div class="shell-brand">
        <span class="shell-brand__mark" aria-hidden="true">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none">
            <path
              d="M4.5 8.2c0-1 .8-1.8 1.8-1.8h11.4c1 0 1.8.8 1.8 1.8v2.1c-1.3.5-2.2 1.7-2.2 3.1 0 1.4.9 2.6 2.2 3.1v2.1c0 1-.8 1.8-1.8 1.8H6.3c-1 0-1.8-.8-1.8-1.8v-2.1c1.3-.5 2.2-1.7 2.2-3.1 0-1.4-.9-2.6-2.2-3.1V8.2z"
              fill="#fff"
            />
            <circle cx="9.2" cy="10.6" r="1.15" fill="var(--brand)" />
            <circle cx="14.8" cy="10.6" r="1.15" fill="var(--brand)" />
          </svg>
        </span>
        <span class="shell-brand__name">DevMask</span>
        <span class="shell-brand__version dm-mono">{{ nav.versionDisplay.value }}</span>
      </div>

      <span :class="['dm-pill', runtimePill.class]" role="status">
        <i class="shell-live__dot" aria-hidden="true"></i>
        {{ runtimePill.label }}
      </span>
    </header>

    <main class="main-content">
      <div
        ref="pageStageRef"
        :class="['page-stage', { 'page-stage--dragging': swipe.isSwipeDragging.value }]"
        @click.capture="swipe.handleClickCapture"
        @pointercancel="swipe.handlePointerCancel"
        @pointerdown="swipe.handlePointerDown"
        @pointermove="swipe.handlePointerMove"
        @pointerup="swipe.handlePointerEnd"
      >
        <div class="page-track">
          <section
            v-for="page in nav.pages.value"
            :key="page.id"
            :aria-hidden="nav.activePage.value !== page.id"
            class="page-panel"
          >
            <div
              class="page-scroll"
              @touchstart="overscroll.handleTouchStart"
              @touchmove="overscroll.handleTouchMove"
              @touchend="overscroll.handleTouchEnd"
              @touchcancel="overscroll.handleTouchEnd"
            >
              <div class="page-scroll-content">
                <component
                  :is="page.component"
                  v-if="nav.shouldRenderPage(page.id)"
                  class="page-view"
                />
                <AsyncPagePlaceholder v-else />
              </div>
            </div>
          </section>
        </div>
      </div>
    </main>

    <nav class="bottom-nav">
      <button
        v-for="page in nav.pages.value"
        :key="page.id"
        type="button"
        :class="['nav-item', { 'nav-item--active': nav.activePage.value === page.id }]"
        :aria-current="nav.activePage.value === page.id ? 'page' : undefined"
        @pointerdown="primePage(page.id)"
        @click.stop="nav.handlePageChange(page.id)"
      >
        <span class="nav-item__glyph">
          <i class="nav-item__bar" aria-hidden="true"></i>
          <component :is="page.icon" :size="22" :stroke-width="1.7" />
        </span>
        <span class="nav-label">{{ page.label }}</span>
      </button>
    </nav>
  </div>
</template>

<script setup lang="ts">
import { computed, defineComponent, h, onMounted, onUnmounted, ref, watch } from 'vue'
import { useConfigStore } from './stores/config'
import { useSettingsStore } from './stores/settings'
import { applyThemeToDocument, resolveIsDarkTheme, systemPrefersDark } from './utils/theme'
import { usePageNavigation, PAGE_INDEX_BY_ID } from './composables/usePageNavigation'
import { usePageSwipe } from './composables/usePageSwipe'
import { useOverscrollBounce } from './composables/useOverscrollBounce'
import { usePageHistory } from './composables/usePageHistory'
import { useI18n } from './utils/i18n'
import type { PageId } from './stores/navigation'

type ResizeObserverInstance = InstanceType<typeof window.ResizeObserver>

const AsyncPagePlaceholder = defineComponent({
  name: 'AsyncPagePlaceholder',
  setup() {
    return () =>
      h('div', { class: 'page-placeholder' }, [
        h('div', { class: 'page-placeholder__line page-placeholder__line--title' }),
        h('div', { class: 'page-placeholder__line' }),
        h('div', { class: 'page-placeholder__line page-placeholder__line--short' }),
      ])
  },
})

let pageStageResizeObserver: ResizeObserverInstance | null = null
let mediaQuery: ReturnType<typeof window.matchMedia> | null = null
let mediaQueryListener: ((event: { matches: boolean }) => void) | null = null

const pageStageRef = ref<HTMLElement | null>(null)
const settingsStore = useSettingsStore()
const configStore = useConfigStore()
const { t } = useI18n()

const systemPrefersDarkRef = ref(systemPrefersDark())

// 默认跟随系统（settings store 默认 'system'），用户显式选择后以设置为准
const isDark = computed(() => resolveIsDarkTheme(settingsStore.theme, systemPrefersDarkRef.value))

// 顶栏状态：配置读到 = 运行中；未读到 = 检查中；读失败 = 未运行
const runtimePill = computed(() => {
  if (!configStore.configReady) {
    return { class: 'dm-pill--muted', label: t('shell.checking') }
  }
  if (configStore.error) {
    return { class: 'dm-pill--warn', label: t('shell.stopped') }
  }
  return { class: 'dm-pill--ok', label: t('shell.running') }
})

// Composables — break circular dependency
const history = usePageHistory()
const nav = usePageNavigation({
  isHandlingPopstate: history.isHandlingPopstate,
  animateToPage: (pageId) => swipe.settleTo(PAGE_INDEX_BY_ID[pageId]),
})
history.bindSetActivePage(nav.setActivePage)

const swipe = usePageSwipe({
  activePage: nav.activePage,
  pageStageWidth: nav.pageStageWidth,
  pageStageRef,
  primeNeighborPages: nav.primeNeighborPages,
  setActivePageSilent: nav.setActivePageSilent,
})

const overscroll = useOverscrollBounce()

function primePage(pageId: PageId) {
  if (pageId === 'home') return
  nav.primeNeighborPages(PAGE_INDEX_BY_ID[pageId])
}

// Theme
watch(isDark, (v) => applyThemeToDocument(v), { immediate: true })

// Lifecycle
function syncWidth() {
  nav.syncPageStageWidth(pageStageRef.value)
  swipe.snapToCurrentPage()
}

onMounted(() => {
  history.setupHistory()
  nav.scheduleConfigBootstrap()
  nav.schedulePageWarmup()
  nav.scheduleAppDataWarmup()
  syncWidth()

  window.addEventListener('resize', syncWidth, { passive: true })

  if (typeof window.ResizeObserver === 'function' && pageStageRef.value) {
    pageStageResizeObserver = new window.ResizeObserver(syncWidth)
    pageStageResizeObserver.observe(pageStageRef.value)
  }

  mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  systemPrefersDarkRef.value = mediaQuery.matches
  mediaQueryListener = (event) => {
    systemPrefersDarkRef.value = event.matches
  }
  mediaQuery.addEventListener('change', mediaQueryListener)
})

onUnmounted(() => {
  history.cleanup()
  swipe.cleanup()
  overscroll.cleanup()
  nav.cleanup()

  window.removeEventListener('resize', syncWidth)
  pageStageResizeObserver?.disconnect()
  pageStageResizeObserver = null

  if (mediaQuery && mediaQueryListener) {
    mediaQuery.removeEventListener('change', mediaQueryListener)
  }
})
</script>

<style scoped>
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100vh;
  height: 100dvh;
  min-height: 0;
  background: var(--bg);
  color: var(--ink);
  padding: 0 var(--safe-area-inset-right) var(--safe-area-inset-bottom) var(--safe-area-inset-left);
}

/* —— 顶栏 —— */
.shell-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex: none;
  gap: 10px;
  padding: calc(var(--safe-area-inset-top) + 10px) var(--page-x) 10px;
  background: var(--bg);
}

.shell-brand {
  display: flex;
  align-items: center;
  gap: 9px;
  min-width: 0;
}

.shell-brand__mark {
  display: grid;
  place-items: center;
  flex: none;
  width: 29px;
  height: 29px;
  border-radius: 9px;
  background: var(--brand);
  box-shadow: 0 6px 14px rgba(44, 91, 255, 0.28);
}

html.dark .shell-brand__mark {
  box-shadow: none;
}

.shell-brand__name {
  font-size: 15.5px;
  font-weight: 700;
  letter-spacing: -0.01em;
  color: var(--ink);
}

.shell-brand__version {
  padding: 2px 6px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: var(--surface);
  font-size: 10px;
  color: var(--muted);
  white-space: nowrap;
}

.shell-live__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  flex: none;
}

.dm-pill--ok .shell-live__dot {
  box-shadow: 0 0 0 3px rgba(14, 159, 110, 0.14);
}

html.dark .dm-pill--ok .shell-live__dot {
  box-shadow: 0 0 0 3px rgba(52, 211, 153, 0.16);
}

/* —— 页面轨道 —— */
.main-content {
  flex: 1 1 auto;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

.page-stage {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  touch-action: pan-y;
}

.page-stage--dragging {
  user-select: none;
  -webkit-user-select: none;
  cursor: grabbing;
}

.page-track {
  display: flex;
  height: 100%;
  will-change: transform;
  backface-visibility: hidden;
  -webkit-backface-visibility: hidden;
}

.page-panel {
  flex: 0 0 100%;
  box-sizing: border-box;
  min-width: 0;
  min-height: 0;
  height: 100%;
  padding: 0 var(--page-x);
}

.page-scroll {
  height: 100%;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding-bottom: calc(var(--nav-h) + var(--safe-area-inset-bottom) + 24px);
  -webkit-overflow-scrolling: touch;
  overscroll-behavior-y: none;
  overflow-anchor: none;
  touch-action: pan-y;
}

.page-scroll-content {
  min-height: 100%;
}

.page-view {
  min-height: 100%;
  width: 100%;
}

/* —— 底部导航：浅色胶囊 / 深色下划线 —— */
.bottom-nav {
  position: fixed;
  left: 0;
  right: 0;
  bottom: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: space-around;
  gap: 2px;
  height: calc(var(--nav-h) + var(--safe-area-inset-bottom));
  padding: 6px 8px calc(6px + var(--safe-area-inset-bottom));
  background: var(--surface);
  border-top: 1px solid var(--line);
  pointer-events: auto;
}

html.dark .bottom-nav {
  background: var(--bg);
}

.nav-item {
  position: relative;
  display: flex;
  flex: 1 1 0;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 3px;
  max-width: 78px;
  height: 48px;
  padding: 4px 2px;
  border: 0;
  border-radius: var(--radius-btn);
  background: transparent;
  color: var(--dim);
  font-family: inherit;
  cursor: pointer;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  user-select: none;
  -webkit-user-select: none;
  transition:
    color 0.18s ease,
    background-color 0.18s ease;
}

.nav-item__glyph {
  position: relative;
  display: grid;
  place-items: center;
  height: 24px;
}

/* 下划线只在深色主题出现 */
.nav-item__bar {
  display: none;
}

.nav-item--active {
  color: var(--brand);
  background: var(--brand-soft);
  font-weight: 600;
}

html.dark .nav-item--active {
  color: #cfe0ff;
  background: transparent;
  font-weight: 500;
}

html.dark .nav-item--active .nav-item__bar {
  display: block;
  position: absolute;
  top: -8px;
  left: 50%;
  width: 22px;
  height: 2px;
  border-radius: 2px;
  background: var(--brand);
  transform: translateX(-50%);
}

.nav-label {
  font-size: 10px;
  line-height: 1.2;
  letter-spacing: 0.01em;
  white-space: nowrap;
}
</style>
