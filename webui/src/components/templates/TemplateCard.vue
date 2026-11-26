<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <!--
    模板卡只保留三件事：机型名（模板名）、型号代码、套用应用数。
    其余字段（fingerprint / build_id / 包名列表…）留在编辑页，避免列表变成表单快照。
  -->
  <article class="tpl-card">
    <button type="button" class="tpl-card__main" @click="emit('edit', name, template)">
      <span class="tpl-card__glyph" aria-hidden="true">
        <Smartphone :size="16" :stroke-width="1.7" />
      </span>

      <span class="tpl-card__body">
        <span class="tpl-card__name" :title="name">{{ name }}</span>
        <span class="tpl-card__meta">
          <span class="tpl-card__code dm-mono" :title="codeLabel">{{ codeLabel }}</span>
          <span class="tpl-card__dot" aria-hidden="true">·</span>
          <span :class="['tpl-card__count', { 'is-zero': packageCount === 0 }]">{{
            countLabel
          }}</span>
        </span>
      </span>
    </button>

    <span class="tpl-card__acts">
      <button
        type="button"
        class="tpl-icon"
        :title="t('templates.actions.export')"
        :aria-label="t('templates.actions.export')"
        @click="emit('export', name, template)"
      >
        <Copy :size="14" :stroke-width="1.8" />
      </button>
      <button
        type="button"
        class="tpl-icon tpl-icon--danger"
        :title="t('common.delete')"
        :aria-label="t('common.delete')"
        @click="emit('delete', name)"
      >
        <Trash :size="14" :stroke-width="1.8" />
      </button>
    </span>
  </article>
</template>

<script setup lang="ts">
import { Copy, Smartphone, Trash } from '@lucide/vue'
import { computed, toRefs } from 'vue'
import { useI18n } from '../../utils/i18n'
import type { Template } from '../../types'

const props = defineProps<{ name: string; template: Template }>()
const { name, template } = toRefs(props)
const emit = defineEmits<{
  export: [string, Template]
  edit: [string, Template]
  delete: [string]
}>()

const { t } = useI18n()

/** 型号代码：优先 model，其次 device / product 代号。 */
const codeLabel = computed(() => {
  const code = template.value.model || template.value.device || template.value.product || ''
  return code.trim() || t('overview.templates.model_unknown')
})

const packageCount = computed(() => template.value.packages?.length ?? 0)

const countLabel = computed(() => t('overview.templates.apps', { count: packageCount.value }))
</script>

<style scoped>
.tpl-card {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 11px 10px 11px 12px;
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--radius-tile);
  box-shadow: var(--shadow-1);
  min-width: 0;
}

html.dark .tpl-card {
  box-shadow: none;
}

.tpl-card__main {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: 1;
  min-width: 0;
  padding: 0;
  border: 0;
  background: transparent;
  font-family: inherit;
  color: inherit;
  text-align: left;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
  user-select: none;
  -webkit-user-select: none;
}

.tpl-card:active {
  background: var(--surface-2);
}

.tpl-card__main:focus-visible {
  outline: 2px solid var(--brand);
  outline-offset: 4px;
  border-radius: 8px;
}

.tpl-card__glyph {
  display: grid;
  place-items: center;
  flex: none;
  width: 30px;
  height: 30px;
  border-radius: 9px;
  background: var(--brand-soft);
  color: var(--brand);
}

.tpl-card__body {
  display: flex;
  flex-direction: column;
  gap: 3px;
  flex: 1;
  min-width: 0;
}

.tpl-card__name {
  font-size: 13px;
  font-weight: 620;
  line-height: 1.3;
  color: var(--ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tpl-card__meta {
  display: flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  font-size: 10.5px;
  line-height: 1.3;
  color: var(--muted);
}

.tpl-card__code {
  font-size: 10.5px;
  color: var(--muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tpl-card__dot {
  flex: none;
  color: var(--dim);
}

.tpl-card__count {
  flex: none;
  color: var(--brand);
}

.tpl-card__count.is-zero {
  color: var(--dim);
}

.tpl-card__acts {
  display: flex;
  align-items: center;
  gap: 1px;
  flex: none;
}

.tpl-icon {
  display: grid;
  place-items: center;
  width: 27px;
  height: 27px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--dim);
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.tpl-icon:active {
  background: var(--surface-3);
  color: var(--ink);
}

.tpl-icon--danger:active {
  color: var(--danger);
}
</style>
