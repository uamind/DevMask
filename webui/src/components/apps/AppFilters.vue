<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div class="apps-filters">
    <!-- 搜索：白卡 + 放大镜，键入只更新输入值（防抖在页面里） -->
    <div class="search">
      <Search :size="15" :stroke-width="1.9" class="search__icon" aria-hidden="true" />
      <input
        v-model="searchModel"
        class="search__input"
        type="text"
        enterkeyhint="search"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="none"
        spellcheck="false"
        :placeholder="t('apps.search_placeholder')"
        :aria-label="t('apps.search_placeholder')"
      />
      <button
        v-if="searchQuery"
        type="button"
        class="search__clear"
        :aria-label="t('common.cancel')"
        @click="searchModel = ''"
      >
        <X :size="13" :stroke-width="2" aria-hidden="true" />
      </button>
    </div>

    <!-- 三段胶囊：已配置 / 全部 / 系统应用 -->
    <div class="seg" role="tablist">
      <button
        v-for="option in options"
        :key="option.id"
        type="button"
        role="tab"
        :class="['seg__item', { 'seg__item--active': filterType === option.id }]"
        :aria-selected="filterType === option.id"
        :aria-label="option.srLabel"
        @click="setFilter(option.id)"
      >
        <span class="seg__label">{{ option.label }}</span>
        <span v-if="option.count !== null" class="seg__count dm-mono">{{ option.count }}</span>
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Search, X } from '@lucide/vue'
import { useI18n } from '../../utils/i18n'

type FilterType = 'configured' | 'all' | 'system'

interface FilterOption {
  id: FilterType
  label: string
  /** null = 该分组还没加载（系统应用未读取时不显示数量，避免显示假的 0） */
  count: number | null
  /** 读屏用的一句话标签（含数量） */
  srLabel: string
}

const props = defineProps<{
  searchQuery: string
  filterType: FilterType
  configuredCount: number
  totalCount: number
  systemCount: number | null
}>()

const emit = defineEmits<{
  'update:searchQuery': [string]
  'update:filterType': [FilterType]
}>()

const { t } = useI18n()

const searchModel = computed({
  get: () => props.searchQuery,
  set: (val: string) => emit('update:searchQuery', val),
})

function withCount(label: string, count: number | null): Omit<FilterOption, 'id'> {
  return {
    label,
    count,
    // 视觉上标签与数量是两个 span（靠 gap 分隔），读屏需要一句话：已配置 3
    srLabel: count === null ? label : `${label} ${count}`,
  }
}

const options = computed<FilterOption[]>(() => [
  { id: 'configured', ...withCount(t('apps.tabs.configured'), props.configuredCount) },
  { id: 'all', ...withCount(t('apps.tabs.all'), props.totalCount) },
  { id: 'system', ...withCount(t('apps.tabs.system'), props.systemCount) },
])

const setFilter = (type: FilterType) => emit('update:filterType', type)
</script>

<style scoped>
.apps-filters {
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-bottom: 10px;
}

/* —— 搜索框 —— */
.search {
  display: flex;
  align-items: center;
  gap: 9px;
  height: 44px;
  padding: 0 13px;
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: 14px;
  box-shadow: var(--shadow-1);
}

html.dark .search {
  box-shadow: none;
}

.search:focus-within {
  border-color: var(--brand);
}

.search__icon {
  flex: none;
  color: var(--dim);
}

.search__input {
  flex: 1 1 auto;
  min-width: 0;
  height: 100%;
  padding: 0;
  border: 0;
  outline: none;
  background: transparent;
  color: var(--ink);
  font-family: inherit;
  font-size: 13px;
  line-height: 1.4;
  appearance: none;
}

.search__input::placeholder {
  color: var(--dim);
}

.search__clear {
  display: grid;
  place-items: center;
  flex: none;
  width: 22px;
  height: 22px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  background: var(--surface-3);
  color: var(--muted);
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

/* —— 三段胶囊 —— */
.seg {
  display: flex;
  gap: 3px;
  padding: 3px;
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--radius-pill);
  box-shadow: var(--shadow-1);
}

html.dark .seg {
  background: var(--surface-2);
  box-shadow: none;
}

.seg__item {
  display: inline-flex;
  flex: 1 1 0;
  align-items: center;
  justify-content: center;
  gap: 5px;
  min-width: 0;
  height: 32px;
  padding: 0 8px;
  border: 0;
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--muted);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 560;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color 0.16s ease,
    color 0.16s ease;
  -webkit-tap-highlight-color: transparent;
  user-select: none;
  -webkit-user-select: none;
}

.seg__item:active {
  opacity: 0.82;
}

.seg__item--active {
  background: var(--brand-soft);
  color: var(--brand);
  font-weight: 640;
}

.seg__label {
  overflow: hidden;
  text-overflow: ellipsis;
}

.seg__count {
  flex: none;
  font-size: 10.5px;
  font-weight: 500;
  color: var(--dim);
}

.seg__item--active .seg__count {
  color: var(--brand);
}
</style>
