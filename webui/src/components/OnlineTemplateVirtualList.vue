<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div ref="viewportRef" class="virtual-list" @scroll="handleScroll">
    <div class="virtual-spacer" :style="{ height: `${totalHeight}px` }">
      <article
        v-for="entry in visibleEntries"
        :key="entry.item.id"
        class="tpl-row"
        :style="{ transform: `translateY(${entry.offset}px)` }"
      >
        <!-- 行只给「名字 + 机型/分类 + 状态」，细节与导入都进预览抽屉 -->
        <button type="button" class="row" @click="emit('preview', entry.item.id)">
          <span class="row__body">
            <span class="row__name" :title="entry.item.displayName">
              {{ entry.item.displayName }}
            </span>
            <span class="row__sub" :title="describe(entry.item)">{{ describe(entry.item) }}</span>
          </span>

          <span
            v-if="entry.item.detailStatus !== 'ready'"
            :class="['dm-pill', statusTone(entry.item.detailStatus)]"
          >
            {{ statusLabel(entry.item.detailStatus) }}
          </span>
          <ChevronRight v-else :size="15" class="row__chev" aria-hidden="true" />
        </button>
      </article>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ChevronRight } from '@lucide/vue'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import type { OnlineTemplateLoadState, OnlineTemplateRecord } from '../types'
import { useI18n } from '../utils/i18n'

/** 固定行高：所有行（就绪 / 加载中 / 失败）同高，虚拟列表才能用纯算术定位。 */
const ROW_HEIGHT = 64
const ROW_GAP = 8
const ROW_STEP = ROW_HEIGHT + ROW_GAP
const DEFAULT_OVERSCAN = 4

const props = withDefaults(
  defineProps<{
    items: OnlineTemplateRecord[]
    overscan?: number
  }>(),
  { overscan: DEFAULT_OVERSCAN }
)

const emit = defineEmits<{ preview: [string] }>()

const { t } = useI18n()
const viewportRef = ref<HTMLElement | null>(null)
const viewportHeight = ref(0)
const scrollTop = ref(0)
let cleanupResizeObserver: (() => void) | null = null

const totalHeight = computed(() =>
  props.items.length > 0 ? props.items.length * ROW_STEP - ROW_GAP : 0
)

const startIndex = computed(() =>
  Math.max(0, Math.floor(scrollTop.value / ROW_STEP) - props.overscan)
)

const endIndex = computed(() => {
  const visible = Math.ceil(viewportHeight.value / ROW_STEP) + props.overscan * 2 + 1
  return Math.min(props.items.length, startIndex.value + visible)
})

const visibleEntries = computed(() =>
  props.items.slice(startIndex.value, endIndex.value).map((item, index) => ({
    item,
    offset: (startIndex.value + index) * ROW_STEP,
  }))
)

/** 次行文案：详情就绪时用型号代码，否则退回品牌 + 分类。 */
function describe(item: OnlineTemplateRecord) {
  const model = item.detail?.template.model?.trim() || ''
  const category = t(`templates.categories.${item.category}`)
  return [model, item.brand || '', category].filter(Boolean).join(' · ')
}

function statusLabel(status: OnlineTemplateLoadState) {
  switch (status) {
    case 'ready':
      return t('templates.online.status.ready')
    case 'error':
      return t('templates.online.status.failed')
    case 'loading':
      return t('templates.online.status.loading')
    default:
      return t('templates.online.status.pending')
  }
}

function statusTone(status: OnlineTemplateLoadState) {
  return status === 'error' ? 'dm-pill--warn' : 'dm-pill--muted'
}

function handleScroll(event: Event) {
  const target = event.target as HTMLElement
  scrollTop.value = target.scrollTop
}

function syncViewportHeight() {
  viewportHeight.value = viewportRef.value?.clientHeight || 0
}

function scrollToTop() {
  if (!viewportRef.value) {
    return
  }

  viewportRef.value.scrollTop = 0
  scrollTop.value = 0
}

watch(totalHeight, () => {
  const maxScrollTop = Math.max(0, totalHeight.value - viewportHeight.value)
  if (scrollTop.value > maxScrollTop && viewportRef.value) {
    viewportRef.value.scrollTop = maxScrollTop
    scrollTop.value = maxScrollTop
  }
})

onMounted(() => {
  syncViewportHeight()

  if (viewportRef.value && typeof window.ResizeObserver !== 'undefined') {
    const observer = new window.ResizeObserver(() => {
      syncViewportHeight()
    })
    observer.observe(viewportRef.value)
    cleanupResizeObserver = () => observer.disconnect()
  }
})

onUnmounted(() => {
  cleanupResizeObserver?.()
  cleanupResizeObserver = null
})

defineExpose({
  scrollToTop,
})
</script>

<style scoped>
.virtual-list {
  position: relative;
  height: 100%;
  overflow-y: auto;
  overflow-x: hidden;
  overscroll-behavior: contain;
  -webkit-overflow-scrolling: touch;
  padding-right: 2px;
  scrollbar-width: thin;
}

.virtual-spacer {
  position: relative;
  width: 100%;
}

.tpl-row {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: 64px;
  will-change: transform;
}

.row {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  height: 100%;
  padding: 0 11px 0 13px;
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--radius-tile);
  box-shadow: var(--shadow-1);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

html.dark .row {
  box-shadow: none;
}

.row:active {
  background: var(--surface-2);
}

.row__body {
  display: flex;
  flex-direction: column;
  gap: 3px;
  flex: 1;
  min-width: 0;
}

.row__name {
  font-size: 13px;
  font-weight: 600;
  line-height: 1.3;
  color: var(--ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row__sub {
  font-size: 11px;
  line-height: 1.3;
  color: var(--muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row__chev {
  flex: none;
  color: var(--dim);
}
</style>
