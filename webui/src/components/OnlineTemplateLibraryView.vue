<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <section class="online-library-view">
    <header class="lib-head">
      <button
        type="button"
        class="lib-back"
        :aria-label="t('templates.online.back')"
        @click="emit('close')"
      >
        <ArrowLeft :size="17" />
      </button>

      <div class="lib-head__copy">
        <h2 class="lib-title">{{ t('templates.online.title') }}</h2>
        <p class="lib-sub">{{ t('templates.online.progress_label') }} · {{ progressCopy }}</p>
      </div>

      <button
        type="button"
        class="lib-refresh"
        :disabled="onlineTemplatesStore.isRefreshing"
        :aria-label="t('templates.online.refresh')"
        @click="handleRefresh"
      >
        <RefreshCw :size="16" :class="{ 'spin-icon': onlineTemplatesStore.isRefreshing }" />
      </button>
    </header>

    <div class="lib-meta">
      <span class="dm-pill dm-pill--muted">{{ sourceLabel }}</span>
      <span v-if="onlineTemplatesStore.isFallbackSource" class="dm-pill dm-pill--warn">
        {{ t('templates.online.status.fallback') }}
      </span>
      <span v-if="onlineTemplatesStore.showingCachedData" class="dm-pill dm-pill--muted">
        {{ t('templates.online.status.cached') }}
      </span>

      <button
        v-if="onlineTemplatesStore.failedDetailCount > 0"
        type="button"
        class="dm-link lib-retry"
        :disabled="onlineTemplatesStore.detailsStatus === 'loading'"
        @click="onlineTemplatesStore.retryFailedDetails()"
      >
        <RotateCw :size="12" />
        {{ t('templates.online.retry_failed') }}
      </button>
    </div>

    <div v-if="showProgressTrack" class="lib-track" aria-hidden="true">
      <i class="lib-track__value" :style="{ width: `${progressPercent}%` }"></i>
    </div>

    <div class="lib-search">
      <Search :size="15" class="lib-search__icon" aria-hidden="true" />
      <input
        v-model="searchInput"
        type="text"
        class="lib-search__input"
        enterkeyhint="search"
        :placeholder="t('templates.online.search_placeholder')"
        :aria-label="t('templates.online.search_placeholder')"
      />
      <button
        v-if="searchInput"
        type="button"
        class="lib-search__clear"
        :aria-label="t('common.cancel')"
        @click="searchInput = ''"
      >
        <X :size="12" />
      </button>
    </div>

    <div class="lib-chips" data-page-swipe-ignore>
      <button
        type="button"
        :class="['lib-chip', { 'is-active': onlineTemplatesStore.selectedCategory === 'all' }]"
        @click="onlineTemplatesStore.setSelectedCategory('all')"
      >
        {{ t('templates.categories.all') }}
      </button>
      <button
        v-for="(_, category) in onlineTemplatesStore.templateCategories"
        :key="category"
        type="button"
        :class="['lib-chip', { 'is-active': onlineTemplatesStore.selectedCategory === category }]"
        @click="onlineTemplatesStore.setSelectedCategory(category)"
      >
        {{ t(`templates.categories.${category}`) }}
      </button>
    </div>

    <div
      v-if="onlineTemplatesStore.availableBrands.length > 0"
      class="lib-chips lib-chips--brand"
      data-page-swipe-ignore
    >
      <button
        type="button"
        :class="['lib-chip', 'lib-chip--brand', { 'is-active': selectedBrand === null }]"
        @click="onlineTemplatesStore.setSelectedBrand(null)"
      >
        {{ t('templates.categories.all') }}
      </button>
      <button
        v-for="brand in onlineTemplatesStore.availableBrands"
        :key="brand"
        type="button"
        :class="['lib-chip', 'lib-chip--brand', { 'is-active': selectedBrand === brand }]"
        @click="onlineTemplatesStore.setSelectedBrand(brand)"
      >
        {{ brand }}
      </button>
    </div>

    <div v-if="onlineTemplatesStore.loadError" class="lib-warn">
      <TriangleAlert :size="15" class="lib-warn__icon" aria-hidden="true" />
      <p>{{ onlineTemplatesStore.loadError }}</p>
      <button
        type="button"
        class="lib-warn__close"
        :aria-label="t('common.confirm')"
        @click="onlineTemplatesStore.clearLoadError()"
      >
        <X :size="12" />
      </button>
    </div>

    <!-- 骨架 3 行：不做转圈 -->
    <div
      v-if="onlineTemplatesStore.indexStatus === 'loading' && !onlineTemplatesStore.hasAnyData"
      class="lib-state"
      aria-hidden="true"
    >
      <span v-for="index in 3" :key="index" class="dm-skeleton lib-skel"></span>
    </div>

    <div
      v-else-if="onlineTemplatesStore.indexStatus === 'error' && !onlineTemplatesStore.hasAnyData"
      class="dm-empty lib-empty"
    >
      <p>{{ t('templates.online.errors.no_templates') }}</p>
      <button type="button" class="lib-primary lib-empty__btn" @click="handleRefresh">
        <RefreshCw :size="13" />
        {{ t('templates.online.refresh') }}
      </button>
    </div>

    <div v-else-if="onlineTemplatesStore.filteredRecords.length === 0" class="dm-empty lib-empty">
      <p>{{ emptyCopy }}</p>
      <button
        v-if="hasActiveFilters"
        type="button"
        class="lib-ghost lib-empty__btn"
        @click="clearFilters"
      >
        <X :size="13" />
        {{ t('templates.online.clear_filters') }}
      </button>
    </div>

    <div v-else class="lib-list">
      <OnlineTemplateVirtualList
        ref="virtualListRef"
        :items="onlineTemplatesStore.filteredRecords"
        @preview="openPreview"
      />
    </div>

    <!-- 预览抽屉：列表只留一行摘要，完整字段与导入都在这里 -->
    <Teleport to="body">
      <Transition name="sheet">
        <div
          v-if="previewRecord"
          class="sheet"
          role="dialog"
          aria-modal="true"
          :aria-label="previewRecord.displayName"
        >
          <div class="sheet__scrim" @click="closePreview"></div>

          <section class="sheet__panel">
            <header class="sheet__head">
              <div class="sheet__head-copy">
                <p class="dm-eyebrow">
                  {{ t(`templates.categories.${previewRecord.category}`) }}
                </p>
                <h3 class="sheet__title">{{ previewRecord.displayName }}</h3>
                <p class="sheet__key dm-mono">{{ previewRecord.name }}</p>
              </div>
              <button
                type="button"
                class="sheet__close"
                :aria-label="t('templates.online.close')"
                @click="closePreview"
              >
                <X :size="16" />
              </button>
            </header>

            <div class="sheet__body">
              <template v-if="previewRecord.detailStatus === 'ready' && previewRecord.detail">
                <dl class="spec">
                  <div v-for="row in previewSpecRows" :key="row.label" class="spec__row">
                    <dt>{{ row.label }}</dt>
                    <dd :class="{ 'dm-mono': row.mono }">{{ row.value }}</dd>
                  </div>
                </dl>

                <div v-if="previewFingerprint" class="sheet__block">
                  <p class="sheet__label">{{ t('templates.fields.fingerprint') }}</p>
                  <p class="sheet__fingerprint dm-mono">{{ previewFingerprint }}</p>
                </div>

                <div v-if="previewMetaPills.length > 0" class="sheet__pills">
                  <span
                    v-for="pill in previewMetaPills"
                    :key="pill"
                    class="dm-pill dm-pill--brand"
                    >{{ pill }}</span
                  >
                </div>

                <div v-if="previewDescription" class="sheet__block">
                  <p class="sheet__label">{{ t('templates.labels.description') }}</p>
                  <p class="sheet__text">{{ previewDescription }}</p>
                </div>
              </template>

              <div v-else-if="previewRecord.detailStatus === 'error'" class="sheet__error">
                <TriangleAlert :size="16" />
                <p>{{ previewRecord.detailError || t('templates.online.errors.detail_failed') }}</p>
                <button type="button" class="lib-ghost" @click="retryPreview">
                  <RotateCw :size="13" />
                  {{ t('templates.online.retry_detail') }}
                </button>
              </div>

              <div v-else class="sheet__skeleton" aria-hidden="true">
                <span v-for="index in 3" :key="index" class="dm-skeleton sheet__skel"></span>
              </div>
            </div>

            <footer class="sheet__foot">
              <button type="button" class="lib-ghost sheet__cancel" @click="closePreview">
                {{ t('common.cancel') }}
              </button>
              <button
                type="button"
                class="lib-primary sheet__import"
                :disabled="!canImport"
                @click="handleImport"
              >
                <Download :size="14" />
                {{ isImporting ? t('templates.online.importing') : t('templates.online.import') }}
              </button>
            </footer>
          </section>
        </div>
      </Transition>
    </Teleport>
  </section>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { ArrowLeft, Download, RefreshCw, RotateCw, Search, TriangleAlert, X } from '@lucide/vue'
import OnlineTemplateVirtualList from './OnlineTemplateVirtualList.vue'
import { useOnlineTemplatesStore } from '../stores/onlineTemplates'
import { useModalHistory } from '../composables/useModalHistory'
import { useI18n } from '../utils/i18n'

type VirtualListExpose = {
  scrollToTop: () => void
}

const emit = defineEmits<{
  close: []
}>()

const onlineTemplatesStore = useOnlineTemplatesStore()
const { t } = useI18n()
const searchInput = ref(onlineTemplatesStore.keyword)
const virtualListRef = ref<VirtualListExpose | null>(null)
const previewId = ref<string | null>(null)

const previewRecord = computed(
  () => onlineTemplatesStore.filteredRecords.find((record) => record.id === previewId.value) ?? null
)

const isImporting = computed(() =>
  previewId.value ? onlineTemplatesStore.importingIds.includes(previewId.value) : false
)

const canImport = computed(
  () => previewRecord.value?.detailStatus === 'ready' && !isImporting.value
)

const sourceLabel = computed(() =>
  t('templates.online.status.source', {
    source: t(
      `templates.online.sources.${onlineTemplatesStore.activeSource || onlineTemplatesStore.preferredSource}`
    ),
  })
)

const selectedBrand = computed(() => onlineTemplatesStore.selectedBrand)

const hasActiveFilters = computed(() =>
  Boolean(
    searchInput.value.trim() ||
    selectedBrand.value ||
    onlineTemplatesStore.selectedCategory !== 'all'
  )
)

const progressPercent = computed(() => {
  if (onlineTemplatesStore.progress.total === 0) {
    return 0
  }

  return Math.round(
    (onlineTemplatesStore.progress.resolved / onlineTemplatesStore.progress.total) * 100
  )
})

const showProgressTrack = computed(
  () =>
    onlineTemplatesStore.progress.total > 0 &&
    !onlineTemplatesStore.allDetailsResolved &&
    onlineTemplatesStore.pendingDetailCount > 0
)

const progressCopy = computed(() => {
  if (onlineTemplatesStore.progress.total === 0) {
    return t('templates.online.progress_pending')
  }

  if (onlineTemplatesStore.allDetailsResolved) {
    return t('templates.online.progress_complete', {
      count: onlineTemplatesStore.progress.succeeded,
      total: onlineTemplatesStore.progress.total,
      failed: onlineTemplatesStore.progress.failed,
    })
  }

  return t('templates.online.progress_partial', {
    resolved: onlineTemplatesStore.progress.resolved,
    total: onlineTemplatesStore.progress.total,
    pending: onlineTemplatesStore.pendingDetailCount,
  })
})

const emptyCopy = computed(() =>
  hasActiveFilters.value
    ? t('templates.online.empty_filtered')
    : t('templates.online.empty_library')
)

const previewDetail = computed(() =>
  previewRecord.value?.detailStatus === 'ready' ? previewRecord.value.detail : undefined
)

const previewSpecRows = computed(() => {
  const template = previewDetail.value?.template
  if (!template) {
    return []
  }

  const rows: Array<{ label: string; value: string; mono?: boolean }> = []
  const push = (label: string, value: string | undefined, mono = false) => {
    const text = (value || '').trim()
    if (text) {
      rows.push({ label, value: text, mono })
    }
  }

  push(t('templates.online.model'), template.model, true)
  push(t('templates.online.market_name'), template.marketname)
  push(t('templates.online.brand'), template.brand)
  push(t('templates.fields.manufacturer'), template.manufacturer)
  push(t('templates.fields.device'), template.device, true)
  push(t('templates.fields.build_id'), template.build_id, true)
  push(t('templates.fields.android_version'), template.android_version)

  const packageCount = template.packages?.length ?? 0
  if (packageCount > 0) {
    push(t('templates.labels.packages'), String(packageCount))
  }

  return rows
})

const previewFingerprint = computed(() => previewDetail.value?.template.fingerprint?.trim() || '')

const previewDescription = computed(() => {
  const detail = previewDetail.value
  if (!detail) {
    return ''
  }

  return (detail.meta?.description || detail.template.description || '').trim()
})

const previewMetaPills = computed(() => {
  const detail = previewDetail.value
  if (!detail) {
    return []
  }

  const pills: string[] = []
  const author = (detail.meta?.author || detail.template.author || '').trim()
  const version = (detail.meta?.version || detail.template.version || '').trim()
  const versionCode = detail.meta?.version_code ?? detail.template.version_code

  if (author) {
    pills.push(`${t('templates.labels.author')} · ${author}`)
  }

  if (version) {
    pills.push(`${t('templates.labels.version')} ${version}`)
  }

  if (versionCode !== undefined && versionCode !== null) {
    pills.push(`${t('templates.labels.version_code')} ${versionCode}`)
  }

  return pills
})

function handleRefresh() {
  void onlineTemplatesStore.reloadCatalog()
}

function clearFilters() {
  searchInput.value = ''
  onlineTemplatesStore.setKeyword('')
  onlineTemplatesStore.setSelectedBrand(null)
  onlineTemplatesStore.setSelectedCategory('all')
}

function openPreview(id: string) {
  previewId.value = id
}

function closePreview() {
  previewId.value = null
}

function retryPreview() {
  if (previewId.value) {
    void onlineTemplatesStore.retryTemplateDetail(previewId.value)
  }
}

async function handleImport() {
  if (!previewId.value) {
    return
  }

  await onlineTemplatesStore.importTemplate(previewId.value)
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && previewId.value) {
    closePreview()
  }
}

useModalHistory(
  computed(() => previewId.value !== null),
  closePreview
)

watch(searchInput, (value) => {
  onlineTemplatesStore.setKeyword(value)
})

watch(
  () => [
    onlineTemplatesStore.keyword,
    onlineTemplatesStore.selectedCategory,
    onlineTemplatesStore.selectedBrand,
  ],
  () => {
    virtualListRef.value?.scrollToTop()
  }
)

onMounted(() => {
  window.addEventListener('keydown', handleKeydown)
  void onlineTemplatesStore.ensureCatalogLoaded()
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeydown)
})
</script>

<style scoped>
.online-library-view {
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: 100%;
  min-width: 0;
}

/* —— 页头 —— */
.lib-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 4px 0 2px;
}

.lib-back,
.lib-refresh {
  display: grid;
  place-items: center;
  flex: none;
  width: 34px;
  height: 34px;
  border: 1px solid var(--line);
  border-radius: var(--radius-btn);
  background: var(--surface);
  color: var(--ink);
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.lib-back:active,
.lib-refresh:active {
  background: var(--surface-2);
}

.lib-refresh:disabled {
  opacity: 0.5;
}

.lib-head__copy {
  flex: 1;
  min-width: 0;
}

.lib-title {
  font-size: 21px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1.25;
  color: var(--ink);
}

html.dark .lib-title {
  font-size: 19px;
  font-weight: 620;
  letter-spacing: -0.01em;
}

.lib-sub {
  margin-top: 3px;
  font-size: 10.5px;
  line-height: 1.4;
  color: var(--dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* —— 数据源 / 进度 —— */
.lib-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.lib-meta .dm-pill--muted {
  background: var(--surface);
  border: 1px solid var(--line);
  color: var(--muted);
}

.lib-retry {
  margin-left: auto;
  font-size: 11px;
}

.lib-retry:disabled {
  opacity: 0.5;
}

.lib-track {
  height: 3px;
  border-radius: 99px;
  background: var(--surface-3);
  overflow: hidden;
}

.lib-track__value {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: var(--brand);
  transition: width 0.25s ease;
}

/* —— 搜索 —— */
.lib-search {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 38px;
  padding: 0 11px;
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--radius-btn);
  box-shadow: var(--shadow-1);
}

html.dark .lib-search {
  box-shadow: none;
}

.lib-search__icon {
  flex: none;
  color: var(--dim);
}

.lib-search__input {
  flex: 1;
  min-width: 0;
  border: 0;
  background: transparent;
  color: var(--ink);
  font-family: inherit;
  font-size: 12.5px;
  outline: none;
}

.lib-search__input::placeholder {
  color: var(--dim);
}

.lib-search__clear {
  display: grid;
  place-items: center;
  flex: none;
  width: 20px;
  height: 20px;
  border: 0;
  border-radius: 50%;
  background: var(--surface-3);
  color: var(--muted);
  cursor: pointer;
}

/* —— 筛选胶囊 —— */
.lib-chips {
  display: flex;
  gap: 6px;
  overflow-x: auto;
  padding-bottom: 1px;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.lib-chips::-webkit-scrollbar {
  display: none;
}

.lib-chip {
  flex: none;
  height: 28px;
  padding: 0 11px;
  border: 1px solid var(--line);
  border-radius: var(--radius-pill);
  background: var(--surface);
  color: var(--muted);
  font-family: inherit;
  font-size: 11.5px;
  font-weight: 560;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.lib-chip.is-active {
  border-color: transparent;
  background: var(--brand-soft);
  color: var(--brand);
  font-weight: 620;
}

html.dark .lib-chip.is-active {
  border-color: rgba(76, 141, 255, 0.28);
}

.lib-chips--brand .lib-chip {
  font-size: 11px;
}

/* —— 告警 / 状态 —— */
.lib-warn {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 11px;
  border: 1px solid var(--line);
  border-left: 3px solid var(--warn);
  border-radius: var(--radius-tile);
  background: var(--warn-soft);
}

.lib-warn__icon {
  flex: none;
  color: var(--warn);
}

.lib-warn p {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  line-height: 1.5;
  color: var(--warn);
}

.lib-warn__close {
  display: grid;
  place-items: center;
  flex: none;
  width: 20px;
  height: 20px;
  border: 0;
  border-radius: 50%;
  background: transparent;
  color: var(--warn);
  cursor: pointer;
}

.lib-state {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.lib-skel {
  height: 64px;
  border-radius: var(--radius-tile);
}

.lib-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  padding: 24px 16px;
}

.lib-empty p {
  font-size: 12px;
  line-height: 1.6;
}

.lib-empty__btn {
  height: 32px;
  padding: 0 12px;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border-radius: var(--radius-btn);
  font-family: inherit;
  font-size: 11.5px;
  font-weight: 600;
  cursor: pointer;
}

.lib-list {
  height: clamp(18rem, calc(100dvh - 23rem), 40rem);
  min-height: 0;
}

/* —— 共享按钮 —— */
.lib-primary {
  border: 0;
  background: var(--brand);
  color: #ffffff;
}

html.dark .lib-primary {
  color: #06101f;
}

.lib-ghost {
  border: 1px solid var(--line-strong);
  background: var(--surface);
  color: var(--ink);
}

.lib-primary:disabled {
  opacity: 0.5;
}

/* —— 预览抽屉 —— */
.sheet {
  position: fixed;
  inset: 0;
  z-index: 2000;
  display: flex;
  align-items: flex-end;
  justify-content: center;
}

.sheet__scrim {
  position: absolute;
  inset: 0;
  background: rgba(8, 11, 18, 0.42);
}

html.dark .sheet__scrim {
  background: rgba(2, 4, 8, 0.66);
}

.sheet__panel {
  position: relative;
  display: flex;
  flex-direction: column;
  width: 100%;
  max-width: 560px;
  max-height: 82dvh;
  margin: 0 auto;
  background: var(--surface);
  border: 1px solid var(--line);
  border-bottom: 0;
  border-radius: var(--radius-card) var(--radius-card) 0 0;
  box-shadow: var(--shadow-2);
  overflow: hidden;
}

.sheet__head {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 14px 14px 10px;
  border-bottom: 1px solid var(--line);
}

.sheet__head-copy {
  flex: 1;
  min-width: 0;
}

.sheet__title {
  margin-top: 5px;
  font-size: 17px;
  font-weight: 700;
  line-height: 1.3;
  color: var(--ink);
  word-break: break-word;
}

.sheet__key {
  margin-top: 3px;
  font-size: 10.5px;
  color: var(--dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sheet__close {
  display: grid;
  place-items: center;
  flex: none;
  width: 30px;
  height: 30px;
  border: 0;
  border-radius: 50%;
  background: var(--surface-3);
  color: var(--muted);
  cursor: pointer;
}

.sheet__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px 14px;
  overscroll-behavior: contain;
}

.spec {
  display: flex;
  flex-direction: column;
  gap: 7px;
}

.spec__row {
  display: flex;
  align-items: baseline;
  gap: 10px;
}

.spec__row dt {
  flex: none;
  width: 84px;
  font-size: 10.5px;
  color: var(--dim);
}

.spec__row dd {
  flex: 1;
  min-width: 0;
  font-size: 11.5px;
  line-height: 1.45;
  color: var(--ink);
  word-break: break-word;
}

.spec__row dd.dm-mono {
  font-size: 11px;
}

.sheet__block {
  margin-top: 12px;
  padding-top: 10px;
  border-top: 1px solid var(--line);
}

.sheet__label {
  margin-bottom: 5px;
  font-size: 10.5px;
  color: var(--dim);
}

.sheet__fingerprint {
  font-size: 10px;
  line-height: 1.6;
  color: var(--muted);
  word-break: break-all;
}

.sheet__text {
  font-size: 11.5px;
  line-height: 1.65;
  color: var(--muted);
  white-space: pre-wrap;
  word-break: break-word;
}

.sheet__pills {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 12px;
}

.sheet__error {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 0;
  color: var(--warn);
}

.sheet__error p {
  font-size: 11.5px;
  line-height: 1.55;
}

.sheet__error .lib-ghost {
  height: 30px;
  padding: 0 11px;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border-radius: var(--radius-btn);
  font-family: inherit;
  font-size: 11.5px;
  font-weight: 600;
  cursor: pointer;
}

.sheet__skeleton {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 6px 0;
}

.sheet__skel {
  height: 12px;
}

.sheet__skel:nth-child(2) {
  width: 78%;
}

.sheet__skel:nth-child(3) {
  width: 54%;
}

.sheet__foot {
  display: flex;
  gap: 8px;
  padding: 10px 14px calc(10px + var(--safe-area-inset-bottom, 0px));
  border-top: 1px solid var(--line);
  background: var(--surface);
}

.sheet__cancel,
.sheet__import {
  height: 38px;
  border-radius: var(--radius-btn);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.sheet__cancel {
  flex: 0 0 auto;
  padding: 0 16px;
}

.sheet__import {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  flex: 1;
}

.sheet__import:disabled {
  opacity: 0.5;
  cursor: default;
}

.sheet-enter-active,
.sheet-leave-active {
  transition: opacity 0.2s ease;
}

.sheet-enter-active .sheet__panel,
.sheet-leave-active .sheet__panel {
  transition: transform 0.26s cubic-bezier(0.22, 1, 0.36, 1);
}

.sheet-enter-from,
.sheet-leave-to {
  opacity: 0;
}

.sheet-enter-from .sheet__panel,
.sheet-leave-to .sheet__panel {
  transform: translateY(12%);
}

.spin-icon {
  animation: lib-spin 0.9s linear infinite;
}

@keyframes lib-spin {
  from {
    transform: rotate(0deg);
  }

  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .sheet-enter-active,
  .sheet-leave-active,
  .sheet-enter-active .sheet__panel,
  .sheet-leave-active .sheet__panel {
    transition-duration: 1ms;
  }

  .sheet-enter-from .sheet__panel,
  .sheet-leave-to .sheet__panel {
    transform: none;
  }
}
</style>
