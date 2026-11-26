<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div class="tpl-list">
    <TemplateCard
      v-for="entry in entries"
      :key="entry.name"
      :name="entry.name"
      :template="entry.template"
      @export="emit('export', entry.name, entry.template)"
      @edit="emit('edit', entry.name, entry.template)"
      @delete="emit('delete', entry.name)"
    />

    <!-- 空状态：一句解释 + 一个主操作（搜索无结果时不给创建入口，避免误导） -->
    <div v-if="entries.length === 0" class="dm-empty tpl-empty">
      <p class="tpl-empty__title">{{ emptyTitle }}</p>
      <template v-if="!isSearching">
        <p class="tpl-empty__hint">{{ t('templates.empty.hint') }}</p>
        <div class="tpl-empty__acts">
          <button type="button" class="tpl-empty__primary" @click="emit('create')">
            {{ t('templates.actions.new') }}
          </button>
          <button type="button" class="tpl-empty__ghost" @click="emit('online')">
            {{ t('templates.actions.online') }}
          </button>
        </div>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import TemplateCard from './TemplateCard.vue'
import { useI18n } from '../../utils/i18n'
import type { Template } from '../../types'

interface TemplateListEntry {
  name: string
  template: Template
}

const props = withDefaults(defineProps<{ entries: TemplateListEntry[]; isSearching?: boolean }>(), {
  isSearching: false,
})

const emit = defineEmits<{
  export: [string, Template]
  edit: [string, Template]
  delete: [string]
  create: []
  online: []
}>()

const { t } = useI18n()

const emptyTitle = computed(() =>
  props.isSearching ? t('templates.search.no_results') : t('templates.empty.title')
)
</script>

<style scoped>
.tpl-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  box-sizing: border-box;
}

.tpl-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 26px 16px 20px;
}

.tpl-empty__title {
  font-size: 13px;
  font-weight: 600;
  color: var(--ink);
}

.tpl-empty__hint {
  font-size: 11.5px;
  line-height: 1.6;
  color: var(--dim);
}

.tpl-empty__acts {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}

.tpl-empty__primary,
.tpl-empty__ghost {
  height: 34px;
  padding: 0 14px;
  border-radius: var(--radius-btn);
  font-family: inherit;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.tpl-empty__primary {
  border: 0;
  background: var(--brand);
  color: #ffffff;
}

html.dark .tpl-empty__primary {
  color: #06101f;
}

.tpl-empty__ghost {
  border: 1px solid var(--line-strong);
  background: var(--surface);
  color: var(--ink);
}

.tpl-empty__primary:active,
.tpl-empty__ghost:active {
  opacity: 0.86;
}
</style>
