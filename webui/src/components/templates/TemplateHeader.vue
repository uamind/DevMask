<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <header class="tpl-head">
    <div class="tpl-head__top">
      <div class="tpl-head__copy">
        <h1 class="dm-page-title">{{ t('templates.title') }}</h1>
        <span class="dm-pill dm-pill--muted tpl-head__count">
          {{ t('overview.metrics.templates_value', { count }) }}
        </span>
      </div>

      <button type="button" class="tpl-new" @click="emit('open-create')">
        <Plus :size="15" :stroke-width="2.2" aria-hidden="true" />
        {{ t('templates.actions.new') }}
      </button>
    </div>

    <div class="tpl-search">
      <Search :size="15" class="tpl-search__icon" aria-hidden="true" />
      <input
        v-model="searchQuery"
        type="text"
        class="tpl-search__input"
        enterkeyhint="search"
        :placeholder="t('templates.search.placeholder')"
        :aria-label="t('templates.search.placeholder')"
      />
      <button
        v-if="searchQuery"
        type="button"
        class="tpl-search__clear"
        :aria-label="t('common.cancel')"
        @click="clearSearch"
      >
        <X :size="12" />
      </button>
    </div>

    <div class="tpl-acts">
      <button type="button" class="tpl-act" @click="emit('open-online')">
        <Download :size="14" aria-hidden="true" />
        {{ t('templates.actions.online') }}
      </button>
      <button type="button" class="tpl-act" @click="emit('open-transfer')">
        <ArrowLeftRight :size="14" aria-hidden="true" />
        {{ t('templates.actions.transfer') }}
      </button>
    </div>
  </header>
</template>

<script setup lang="ts">
import { ArrowLeftRight, Download, Plus, Search, X } from '@lucide/vue'
import { ref, watch } from 'vue'
import { useI18n } from '../../utils/i18n'

defineProps<{ count: number }>()

const emit = defineEmits<{
  'open-online': []
  'open-create': []
  'open-transfer': []
  search: [string]
}>()

const { t } = useI18n()
const searchQuery = ref('')

function clearSearch() {
  searchQuery.value = ''
  emit('search', '')
}

watch(searchQuery, (value) => {
  emit('search', value)
})
</script>

<style scoped>
/* 页头：标题 + 计数 + 主操作；搜索与次操作各占一行，避免 390px 下互相挤压 */
.tpl-head {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 4px 0 14px;
}

.tpl-head__top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.tpl-head__copy {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.tpl-head__count {
  flex: none;
  /* 底色/边框交给 .dm-pill--muted 基类，避免与网络页同一胶囊两种观感 */
  color: var(--muted);
}

.tpl-new {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  flex: none;
  height: 34px;
  padding: 0 13px;
  border: 0;
  border-radius: var(--radius-btn);
  background: var(--brand);
  color: #ffffff;
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 640;
  white-space: nowrap;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

html.dark .tpl-new {
  color: #06101f;
}

.tpl-new:active {
  opacity: 0.88;
}

.tpl-search {
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

html.dark .tpl-search {
  box-shadow: none;
}

.tpl-search__icon {
  flex: none;
  color: var(--dim);
}

.tpl-search__input {
  flex: 1;
  min-width: 0;
  border: 0;
  background: transparent;
  color: var(--ink);
  font-family: inherit;
  font-size: 12.5px;
  outline: none;
}

.tpl-search__input::placeholder {
  color: var(--dim);
}

.tpl-search__clear {
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
  -webkit-tap-highlight-color: transparent;
}

.tpl-acts {
  display: flex;
  gap: 8px;
}

.tpl-act {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  flex: 1 1 0;
  height: 34px;
  border: 1px solid var(--line);
  border-radius: var(--radius-btn);
  background: var(--surface);
  color: var(--ink);
  font-family: inherit;
  font-size: 12px;
  font-weight: 560;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.tpl-act:active {
  background: var(--surface-2);
}

.tpl-act :deep(svg) {
  color: var(--muted);
}
</style>
