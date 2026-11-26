<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div
    class="apps-page"
    :class="{ 'apps-page--background-loading': backgroundLoading }"
    :aria-busy="initialPageLoading || backgroundLoading ? 'true' : 'false'"
  >
    <template v-if="initialPageLoading">
      <AppsPageSkeleton />
    </template>

    <template v-else>
      <!-- 页头：标题 + 已配置/总数（等宽），下面一句话说明「未配置不受影响」 -->
      <header class="dm-page-head apps-head">
        <div class="apps-head__row">
          <h1 class="dm-page-title">{{ t('nav.apps') }}</h1>
          <span class="apps-head__count dm-mono">{{ configuredCount }} / {{ totalCount }}</span>
        </div>
        <p class="dm-page-sub">{{ t('apps.subtitle') }}</p>
      </header>

      <AppFilters
        v-model:search-query="searchInput"
        v-model:filter-type="filterType"
        :configured-count="configuredCount"
        :total-count="totalCount"
        :system-count="systemCount"
      />

      <AppList :apps="filteredApps" :empty-text="emptyText" :loading="false" @select="openConfig" />

      <!-- 分身与继承：只有确实存在「跟随主应用」的分身时才有意义 -->
      <section v-if="inheritedCount > 0" class="inherit">
        <h2 class="inherit__title">{{ t('apps.inherit.title') }}</h2>
        <p class="inherit__desc">{{ t('apps.inherit.desc', { count: inheritedCount }) }}</p>
      </section>
    </template>

    <AppConfigDialog
      v-if="configDialogVisible"
      v-model="configDialogVisible"
      :app="currentApp"
      @saved="handleConfigSaved"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import AppFilters from '../components/apps/AppFilters.vue'
import AppList from '../components/apps/AppList.vue'
import AppsPageSkeleton from '../components/apps/AppsPageSkeleton.vue'
import { useAppsStore } from '../stores/apps'
import { useConfigStore } from '../stores/config'
import { useSettingsStore } from '../stores/settings'
import { useModalHistory } from '../composables/useModalHistory'
import { useI18n } from '../utils/i18n'
import {
  installedLookupKeys,
  normalizePackageName,
  packageDisplayName,
  packageInstanceKey,
  parsePackageUser,
  runtimeConfigKeys,
} from '../utils/package'
import type { AppConfig, InstalledApp, Template } from '../types'

type FilterType = 'configured' | 'all' | 'system'
type AppListItem = InstalledApp & {
  configured: boolean
  /** 该实例本身没有自有配置，只是回退命中主应用的裸包名条目（分身常见） */
  inherited: boolean
  /** 右侧标签：该应用实际读到的机型；未配置为空串 */
  modelLabel: string
  /** 预计算的小写包名/应用名,避免每次键入搜索词都对全量列表做 toLowerCase */
  pkgLower: string
  nameLower: string
  /**
   * 预计算的排序 key:归一化包名 \0 userId(补零) \0 已安装优先。
   * 排序退化为单次字符串比较,不再在比较器里跑正则(normalizePackageName)。
   */
  sortKey: string
}

interface RowConfig {
  inherited: boolean
  modelLabel: string
}

function buildSortKey(packageName: string, installed: boolean): string {
  const { base, userId } = parsePackageUser(packageName)
  return `${base}\u0000${String(userId).padStart(8, '0')}\u0000${installed ? '0' : '1'}`
}

const AppConfigDialog = defineAsyncComponent(() => import('../components/apps/AppConfigDialog.vue'))

const configStore = useConfigStore()
const appsStore = useAppsStore()
const settingsStore = useSettingsStore()
const { t } = useI18n()

const searchInput = ref('')
const debouncedSearch = ref('')
let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null

// 搜索防抖:每个键入字符只更新原始输入,150ms 静默期后才触发全量过滤+排序
watch(searchInput, (value) => {
  if (searchDebounceTimer !== null) {
    clearTimeout(searchDebounceTimer)
  }
  searchDebounceTimer = setTimeout(() => {
    searchDebounceTimer = null
    debouncedSearch.value = value
  }, 150)
})

onUnmounted(() => {
  if (searchDebounceTimer !== null) {
    clearTimeout(searchDebounceTimer)
    searchDebounceTimer = null
  }
})

const filterType = ref<FilterType>('all')
const configDialogVisible = ref(false)
const currentApp = ref<InstalledApp | null>(null)

useModalHistory(configDialogVisible, () => {
  configDialogVisible.value = false
})
const showSystemApps = computed({
  get: () => settingsStore.showSystemApps,
  set: (value: boolean) => settingsStore.setShowSystemApps(value),
})

// 首次进入页面时仅等待用户应用列表，附加补全任务改为后台执行
const isInitializing = ref(!appsStore.hasLoadedUserApps)
const installedApps = computed(() => appsStore.installedApps)
const resolvedPackageInfo = computed(() => appsStore.resolvedPackageInfo)

const installedPackageState = computed(() => {
  const exactPackages = new Set<string>()

  for (const app of installedApps.value) {
    if (app.installed === false) continue
    exactPackages.add(app.packageName)
  }

  return {
    exactPackages,
  }
})

const configuredPackageState = computed(() => {
  const exactPackages = new Set<string>()
  const configuredAppsMap = new Map<string, InstalledApp>()

  for (const appConfig of configStore.getApps()) {
    exactPackages.add(appConfig.package)
    configuredAppsMap.set(appConfig.package, {
      packageName: appConfig.package,
      appName: appConfig.package,
      installed: false,
    })
  }

  for (const template of Object.values(configStore.getTemplates())) {
    if (!template.packages) continue
    for (const pkg of template.packages) {
      exactPackages.add(pkg)
      if (configuredAppsMap.has(pkg)) continue

      configuredAppsMap.set(pkg, {
        packageName: pkg,
        appName: pkg,
        installed: false,
      })
    }
  }

  return {
    packages: Array.from(exactPackages),
    exactPackages,
    configuredApps: Array.from(configuredAppsMap.values()),
  }
})

function isConfiguredPackage(packageName: string) {
  // 与 Rust 一致：配置项须精确命中 runtime 查找键（base@userId 或裸 base）。
  // 禁止把 @0/@998/@999 归一化后互相认成已配置。
  return runtimeConfigKeys(packageName).some((key) =>
    configuredPackageState.value.exactPackages.has(key)
  )
}

function isInstalledPackage(packageName: string) {
  return installedLookupKeys(packageName).some((key) =>
    installedPackageState.value.exactPackages.has(key)
  )
}

function getResolvedPackageInfo(packageName: string) {
  return resolvedPackageInfo.value[packageName]
}

function firstText(...values: (string | undefined | null)[]): string {
  for (const value of values) {
    if (typeof value === 'string' && value.trim()) return value.trim()
  }
  return ''
}

/**
 * 该实例实际命中的配置（行内展示用）：是否只是继承主应用 + 读到的机型。
 * 与运行时严格同源（resolvePackageMatches：键优先，命中即停），
 * 机型按「应用条目优先、模板兜底」取，最后回落到模板名。
 */
function resolveRowConfig(packageName: string): RowConfig {
  const matches = configStore.resolvePackageMatchesFor(packageName)
  if (!matches) {
    return { inherited: false, modelLabel: '' }
  }

  const app: AppConfig | undefined = matches.appMatch?.app
  const template: Template | undefined = matches.templateMatch?.template
  const inherited = (matches.appMatch ?? matches.templateMatch)?.inherited ?? false
  const modelLabel = firstText(
    app?.marketname,
    app?.model,
    app?.device,
    app?.product,
    template?.marketname,
    template?.model,
    template?.device,
    template?.product,
    matches.templateMatch?.templateName
  )

  return { inherited, modelLabel: modelLabel || t('apps.status.configured') }
}

function buildListEntry(app: InstalledApp, configured: boolean, installed: boolean): AppListItem {
  const appName = app.appName || app.packageName
  const rowConfig = resolveRowConfig(app.packageName)

  return {
    ...app,
    appName,
    configured,
    inherited: configured && rowConfig.inherited,
    modelLabel: configured ? rowConfig.modelLabel : '',
    pkgLower: app.packageName.toLowerCase(),
    nameLower: appName.toLowerCase(),
    sortKey: buildSortKey(app.packageName, installed),
  }
}

const allApps = computed<AppListItem[]>(() => {
  const result: AppListItem[] = []
  const packageIndex = new Map<string, number>()
  const normalizedIndex = new Map<string, number>()
  // 实例身份 → 行下标：user 0 的裸名与 pkg@0 是同一实例，不能再出一行
  const instanceIndex = new Map<string, number>()

  // 保留已安装应用的原始顺序
  for (const app of installedApps.value) {
    const normalized = normalizePackageName(app.packageName)
    if (packageIndex.has(app.packageName)) continue

    const entry = buildListEntry(app, isConfiguredPackage(app.packageName), app.installed ?? true)

    const idx = result.length
    result.push(entry)
    packageIndex.set(app.packageName, idx)
    if (!normalizedIndex.has(normalized)) {
      normalizedIndex.set(normalized, idx)
    }
    const instance = packageInstanceKey(app.packageName)
    if (!instanceIndex.has(instance)) {
      instanceIndex.set(instance, idx)
    }
  }

  // 合并配置项：不同实例（不同 userId）应显示为不同应用；
  // 但同一实例的等价写法（裸名 vs pkg@0）必须合并到已存在的那一行。
  for (const app of configuredPackageState.value.configuredApps) {
    if (packageIndex.has(app.packageName)) continue
    if (instanceIndex.has(packageInstanceKey(app.packageName))) continue

    // 查找具有相同归一化包名的已存在应用，复用其展示信息
    const normalized = normalizePackageName(app.packageName)
    const existingIdx = normalizedIndex.get(normalized)
    const existingApp = existingIdx !== undefined ? result[existingIdx] : undefined
    const resolvedInfo = getResolvedPackageInfo(app.packageName)
    const installed = isInstalledPackage(app.packageName)
    const displayName = packageDisplayName(app.packageName)
    const appName = resolvedInfo?.appName || existingApp?.appName || displayName
    const rowConfig = resolveRowConfig(displayName)

    const entry: AppListItem = {
      packageName: displayName,
      appName,
      icon: resolvedInfo?.icon || existingApp?.icon || '',
      versionName: resolvedInfo?.versionName || existingApp?.versionName || '',
      versionCode: resolvedInfo?.versionCode ?? existingApp?.versionCode ?? 0,
      installed,
      isSystem: existingApp?.isSystem ?? resolvedInfo?.isSystem ?? app.isSystem,
      configured: true,
      inherited: rowConfig.inherited,
      modelLabel: rowConfig.modelLabel,
      pkgLower: displayName.toLowerCase(),
      nameLower: appName.toLowerCase(),
      sortKey: buildSortKey(displayName, installed),
    }

    const idx = result.length
    result.push(entry)
    packageIndex.set(app.packageName, idx)
    packageIndex.set(displayName, idx)
    instanceIndex.set(packageInstanceKey(displayName), idx)
  }

  return result
})

/**
 * 列表可见集：系统应用默认隐形（设置里的开关 / 「系统应用」分组会打开），
 * 已配置的系统应用始终显示（配置过的必须能点进去改）。
 */
const visibleApps = computed(() =>
  allApps.value.filter((app) => showSystemApps.value || app.isSystem !== true || app.configured)
)

const initialPageLoading = computed(() => isInitializing.value)
const backgroundLoading = computed(() => appsStore.loading && !initialPageLoading.value)

const totalCount = computed(() => visibleApps.value.length)
const configuredCount = computed(() => visibleApps.value.filter((app) => app.configured).length)
/** 系统应用还没读过时不给数字，避免把「没加载」显示成 0。 */
const systemCount = computed(() =>
  showSystemApps.value ? visibleApps.value.filter((app) => app.isSystem === true).length : null
)
/** 「随主应用继承」的分身数量（含已配置但自身无自有条目者）。 */
const inheritedCount = computed(() => visibleApps.value.filter((app) => app.inherited).length)

const filteredApps = computed(() => {
  let apps = visibleApps.value

  const q = debouncedSearch.value.toLowerCase()
  if (q) {
    // 使用预计算的小写字段,键入时不再对全量列表做 toLowerCase
    apps = apps.filter((app) => app.pkgLower.includes(q) || app.nameLower.includes(q))
  }

  if (filterType.value === 'configured') {
    apps = apps.filter((app) => app.configured)
  } else if (filterType.value === 'system') {
    apps = apps.filter((app) => app.isSystem === true)
  }

  // 预计算 sortKey 后排序退化为纯字符串比较(归一化包名 → userId → 已安装优先)
  return apps.slice().sort((a, b) => (a.sortKey < b.sortKey ? -1 : a.sortKey > b.sortKey ? 1 : 0))
})

const emptyText = computed(() => {
  if (debouncedSearch.value) return t('apps.empty.search')
  if (filterType.value === 'configured') return t('apps.empty.configured')
  return t('apps.empty.all')
})

function openConfig(app: InstalledApp) {
  currentApp.value = app
  configDialogVisible.value = true
}

function handleConfigSaved() {
  // 预留钩子，未来可在保存后刷新列表或提示
}

async function loadApps(includeSystem: boolean) {
  await appsStore.ensureUserAppsLoaded()
  isInitializing.value = false

  void appsStore.resolvePackagesInfo(configuredPackageState.value.packages)
  if (includeSystem) {
    void appsStore.ensureSystemAppsLoaded()
  }
}

onMounted(async () => {
  await loadApps(showSystemApps.value)
})

watch(showSystemApps, (enabled, previous) => {
  if (enabled && enabled !== previous) {
    void appsStore.ensureSystemAppsLoaded()
  }
})

// 选中「系统应用」分组 = 需要系统应用列表：沿用同一个设置开关（读取一次后一直可见）
watch(filterType, (type) => {
  if (type === 'system' && !showSystemApps.value) {
    showSystemApps.value = true
  }
})

watch(
  () => configuredPackageState.value.packages,
  (packages) => {
    if (!packages.length) return
    void appsStore.resolvePackagesInfo(packages)
  },
  { deep: false }
)
</script>

<style scoped>
.apps-page {
  display: flex;
  flex-direction: column;
  box-sizing: border-box;
  overflow: hidden;
  padding-top: 4px;
}

.apps-head__row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
}

.apps-head__count {
  flex: none;
  font-size: 11px;
  color: var(--dim);
}

.inherit {
  margin-top: 12px;
  padding: 10px 15px 12px;
  border-radius: var(--radius-tile);
  background: var(--brand-soft);
}

.inherit__title {
  font-size: 11px;
  font-weight: 660;
  letter-spacing: 0.02em;
  color: var(--brand);
}

.inherit__desc {
  margin-top: 6px;
  font-size: 11.5px;
  line-height: 1.65;
  color: var(--muted);
}

html.dark .inherit__desc {
  color: var(--text);
  opacity: 0.72;
}
</style>
