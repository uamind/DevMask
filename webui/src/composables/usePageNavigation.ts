// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// DevMask: https://github.com/uamind/DevMask

import { computed, defineAsyncComponent, defineComponent, h, ref } from 'vue'
import { FileText, House, LayoutGrid, Settings, Wifi } from '@lucide/vue'
import AppsPageSkeleton from '../components/apps/AppsPageSkeleton.vue'
import NetworkPage from '../pages/NetworkPage.vue'
import StatusPage from '../pages/StatusPage.vue'
import { useAppsStore } from '../stores/apps'
import { useConfigStore } from '../stores/config'
import { useNavigationStore } from '../stores/navigation'
import type { PageId } from '../stores/navigation'
import { useI18n } from '../utils/i18n'

type AppsPageComponent = (typeof import('../pages/AppsPage.vue'))['default']
type TemplatePageComponent = (typeof import('../pages/TemplatePage.vue'))['default']
type SettingsPageComponent = (typeof import('../pages/SettingsPage.vue'))['default']

/**
 * 界面版本（docs/design/ui-redesign §7：任何界面一律 v0.0.1）。
 *
 * 顶部 pill 展示的是这一版 WebUI 的版本口径，与 module.prop / package.json 同源；
 * 模块真实版本（可能被用户自定义）在「设置 · 关于」里展示。
 */
export const APP_VERSION = 'v0.0.1'

/** 底部导航顺序：概览 / 模板 / 应用 / 网络 / 设置。 */
export const PAGE_ORDER: PageId[] = ['home', 'templates', 'apps', 'network', 'settings']
export const PAGE_INDEX_BY_ID: Record<PageId, number> = {
  home: 0,
  templates: 1,
  apps: 2,
  network: 3,
  settings: 4,
}

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

/* ---------------------------------------------------------------------------
 * 页面切换桥：页面内的 CTA（例如概览 hero 的「管理应用」）需要切 tab，
 * 但动画与 history 只在 App.vue 的实例里，因此暴露一个模块级回调。
 * ------------------------------------------------------------------------- */
let pageChangeHandler: ((pageId: PageId) => void) | null = null

export function setPageChangeHandler(handler: ((pageId: PageId) => void) | null) {
  pageChangeHandler = handler
}

/** 从页面内部请求切换底部导航（带转场与 history）。 */
export function navigateToPage(pageId: PageId) {
  pageChangeHandler?.(pageId)
}

let appsPageLoader: Promise<AppsPageComponent> | null = null
let templatePageLoader: Promise<TemplatePageComponent> | null = null
let settingsPageLoader: Promise<SettingsPageComponent> | null = null
let idleWarmupTimer: number | null = null
let idleWarmupId: number | null = null
let appDataWarmupTimer: number | null = null
let appDataWarmupId: number | null = null

function preloadAppsPage() {
  if (!appsPageLoader) {
    appsPageLoader = import('../pages/AppsPage.vue')
      .then((m) => m.default)
      .catch((e) => {
        appsPageLoader = null
        throw e
      })
  }
  return appsPageLoader
}

function preloadTemplatePage() {
  if (!templatePageLoader) {
    templatePageLoader = import('../pages/TemplatePage.vue')
      .then((m) => m.default)
      .catch((e) => {
        templatePageLoader = null
        throw e
      })
  }
  return templatePageLoader
}

function preloadSettingsPage() {
  if (!settingsPageLoader) {
    settingsPageLoader = import('../pages/SettingsPage.vue')
      .then((m) => m.default)
      .catch((e) => {
        settingsPageLoader = null
        throw e
      })
  }
  return settingsPageLoader
}

export function usePageNavigation(opts?: {
  isHandlingPopstate?: () => boolean
  animateToPage?: (pageId: PageId) => void
}) {
  const isPopstateActive = opts?.isHandlingPopstate ?? (() => false)
  const animateToPage = opts?.animateToPage
  const configStore = useConfigStore()
  const appsStore = useAppsStore()
  const navigationStore = useNavigationStore()
  const { t } = useI18n()

  const activePage = ref<PageId>('home')
  const renderedPageIds = ref<PageId[]>(['home'])
  const pageStageWidth = ref(window.innerWidth)

  const AsyncAppsPage = defineAsyncComponent({
    loader: preloadAppsPage,
    suspensible: false,
    loadingComponent: AppsPageSkeleton,
    delay: 0,
  })
  const AsyncTemplatePage = defineAsyncComponent<TemplatePageComponent>({
    loader: preloadTemplatePage,
    suspensible: false,
    loadingComponent: AsyncPagePlaceholder,
    delay: 0,
  })
  const AsyncSettingsPage = defineAsyncComponent<SettingsPageComponent>({
    loader: preloadSettingsPage,
    suspensible: false,
    loadingComponent: AsyncPagePlaceholder,
    delay: 0,
  })

  const pages = computed(() => [
    { id: 'home' as const, label: t('nav.home'), icon: House, component: StatusPage },
    {
      id: 'templates' as const,
      label: t('nav.templates'),
      icon: FileText,
      component: AsyncTemplatePage,
    },
    { id: 'apps' as const, label: t('nav.apps'), icon: LayoutGrid, component: AsyncAppsPage },
    { id: 'network' as const, label: t('nav.network'), icon: Wifi, component: NetworkPage },
    {
      id: 'settings' as const,
      label: t('nav.settings'),
      icon: Settings,
      component: AsyncSettingsPage,
    },
  ])
  const activePageIndex = computed(() => PAGE_INDEX_BY_ID[activePage.value])
  // 顶栏 pill 显示模块真实版本；只有 moduleMeta 未就绪时才回落到本版 WebUI 版本
  const versionDisplay = computed(() =>
    configStore.moduleMetaReady && configStore.moduleVersion
      ? configStore.moduleVersion
      : APP_VERSION
  )

  function markPageAsRendered(pageId: PageId) {
    if (renderedPageIds.value.includes(pageId)) return
    renderedPageIds.value = [...renderedPageIds.value, pageId]
  }

  function shouldRenderPage(pageId: PageId) {
    return renderedPageIds.value.includes(pageId) || activePage.value === pageId
  }

  function syncPageStageWidth(stageRef: HTMLElement | null) {
    const measuredWidth = stageRef?.clientWidth ?? window.innerWidth
    if (measuredWidth > 0) {
      pageStageWidth.value = measuredWidth
    }
  }

  function warmPage(pageId: PageId, options: { includeAppData?: boolean } = {}) {
    if (pageId === 'apps') {
      void preloadAppsPage().catch(() => {})
      if (options.includeAppData) {
        void appsStore.ensureUserAppsLoaded()
      }
      return
    }
    if (pageId === 'templates') {
      void preloadTemplatePage().catch(() => {})
      return
    }
    if (pageId === 'settings') {
      void preloadSettingsPage().catch(() => {})
    }
    // 'home' / 'network' 是同步组件，无需预热
  }

  function primePage(pageId: PageId) {
    if (pageId === 'home' || pageId === 'network') return
    markPageAsRendered(pageId)
    warmPage(pageId, { includeAppData: pageId === 'apps' })
  }

  function primeNeighborPages(index: number) {
    const prev = PAGE_ORDER[index - 1]
    const next = PAGE_ORDER[index + 1]
    if (prev) {
      markPageAsRendered(prev)
      primePage(prev)
    }
    if (next) {
      markPageAsRendered(next)
      primePage(next)
    }
  }

  function setActivePage(pageId: PageId, options: { skipHistory?: boolean } = {}) {
    if (activePage.value === pageId) return

    markPageAsRendered(pageId)
    activePage.value = pageId
    primePage(pageId)
    navigationStore.setCurrentPage(pageId)
    if (!options.skipHistory && !isPopstateActive()) {
      navigationStore.pushPageToStack(pageId)
    }

    // Animate track to the new page (via swipe composable's CSS transition)
    if (animateToPage) {
      animateToPage(pageId)
    }
  }

  function setActivePageSilent(pageId: PageId) {
    markPageAsRendered(pageId)
    activePage.value = pageId
    primePage(pageId)
    navigationStore.setCurrentPage(pageId)
    navigationStore.pushPageToStack(pageId)
  }

  function handlePageChange(pageId: PageId) {
    setActivePage(pageId)
  }

  // 页面内的 CTA 走同一条切换路径（转场 + history 都保留）
  setPageChangeHandler(handlePageChange)

  function scheduleConfigBootstrap() {
    requestAnimationFrame(() => {
      window.setTimeout(() => {
        void configStore.bootstrap()
      }, 0)
    })
  }

  function schedulePageWarmup() {
    const runWarmup = () => {
      idleWarmupId = null
      idleWarmupTimer = null
      warmPage('templates')
      warmPage('settings')
      warmPage('apps')
    }
    if (typeof window.requestIdleCallback === 'function') {
      idleWarmupId = window.requestIdleCallback(runWarmup, { timeout: 1500 })
      return
    }
    idleWarmupTimer = window.setTimeout(runWarmup, 800)
  }

  function scheduleAppDataWarmup() {
    const runWarmup = () => {
      appDataWarmupId = null
      appDataWarmupTimer = null
      void appsStore.ensureUserAppsLoaded()
    }
    if (typeof window.requestIdleCallback === 'function') {
      appDataWarmupId = window.requestIdleCallback(runWarmup, { timeout: 2500 })
      return
    }
    appDataWarmupTimer = window.setTimeout(runWarmup, 1800)
  }

  function cleanup() {
    if (idleWarmupTimer !== null) window.clearTimeout(idleWarmupTimer)
    if (idleWarmupId !== null && typeof window.cancelIdleCallback === 'function') {
      window.cancelIdleCallback(idleWarmupId)
    }
    if (appDataWarmupTimer !== null) window.clearTimeout(appDataWarmupTimer)
    if (appDataWarmupId !== null && typeof window.cancelIdleCallback === 'function') {
      window.cancelIdleCallback(appDataWarmupId)
    }
    setPageChangeHandler(null)
  }

  return {
    activePage,
    activePageIndex,
    pages,
    pageStageWidth,
    renderedPageIds,
    versionDisplay,
    shouldRenderPage,
    setActivePage,
    setActivePageSilent,
    handlePageChange,
    primeNeighborPages,
    syncPageStageWidth,
    scheduleConfigBootstrap,
    schedulePageWarmup,
    scheduleAppDataWarmup,
    cleanup,
  }
}
