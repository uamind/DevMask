<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div class="apps-list">
    <template v-if="loading">
      <div v-for="i in 3" :key="i" class="row-skeleton" aria-hidden="true">
        <span class="dm-skeleton row-skeleton__avatar"></span>
        <span class="row-skeleton__body">
          <span class="dm-skeleton row-skeleton__name"></span>
          <span class="dm-skeleton row-skeleton__pkg"></span>
        </span>
        <span class="dm-skeleton row-skeleton__tag"></span>
      </div>
    </template>

    <template v-else>
      <button
        v-for="app in apps"
        :key="app.packageName"
        type="button"
        class="app-row"
        :data-package="app.packageName"
        @click="emit('select', app)"
      >
        <!-- 图标容器同时是 useAppIcons 的观察目标：读取成功前先显示首字方块 -->
        <span
          class="app-icon-container app-row__avatar"
          :data-package="app.packageName"
          :style="{ background: avatarColor(app.packageName) }"
          aria-hidden="true"
        >
          <img
            v-if="hasIcon(app.packageName)"
            :src="appIcons[app.packageName]"
            class="app-row__icon"
            :class="{ 'app-row__icon--loaded': iconLoaded[app.packageName] }"
            alt=""
            loading="lazy"
            @load="onIconLoad(app.packageName)"
            @error="onIconError(app.packageName)"
          />
          <span v-else class="app-row__initial">{{ initialOf(app) }}</span>
        </span>

        <span class="app-row__body">
          <span class="app-row__name">{{ app.appName }}</span>
          <span class="app-row__package dm-mono">{{ app.packageName }}</span>
          <span v-if="metaOf(app)" class="app-row__meta">{{ metaOf(app) }}</span>
        </span>

        <!-- 右侧机型标签：已配置 = 实心，未配置 = 虚线灰 -->
        <span v-if="app.configured" class="app-row__tag" :title="app.modelLabel">
          {{ app.modelLabel }}
        </span>
        <span v-else class="app-row__tag app-row__tag--empty">
          {{ t('apps.status.unconfigured') }}
        </span>

        <ChevronRight :size="15" :stroke-width="1.9" class="app-row__chev" aria-hidden="true" />
      </button>

      <p v-if="apps.length === 0" class="dm-empty empty-state">{{ emptyText }}</p>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, watch } from 'vue'
import { ChevronRight } from '@lucide/vue'
import { useI18n } from '../../utils/i18n'
import { useAppIcons } from '../../composables/useAppIcons'
import type { InstalledApp } from '../../types'

type AppListItem = InstalledApp & {
  configured: boolean
  inherited: boolean
  /** 该应用读到的机型；未配置时为空串 */
  modelLabel: string
}

const props = defineProps<{
  apps: AppListItem[]
  emptyText: string
  loading: boolean
}>()

const emit = defineEmits<{ select: [AppListItem] }>()

const { t } = useI18n()
const { appIcons, iconLoaded, onIconLoad, onIconError, setupIconObserver, teardownIconObserver } =
  useAppIcons()

const loading = computed(() => props.loading)

/** 原生图标协议读到的 data URL / ksu URI；'fallback' 表示没有图标，用首字方块。 */
function hasIcon(packageName: string): boolean {
  const icon = appIcons.value[packageName]
  return typeof icon === 'string' && icon !== '' && icon !== 'fallback'
}

/** 首字：中文取第一个字，拉丁字母取首字母大写。 */
function initialOf(app: AppListItem): string {
  const name = (app.appName || app.packageName).trim()
  const char = Array.from(name)[0] ?? '?'
  return char.toUpperCase()
}

/** 首字方块底色：同一包名永远同色（只用于没有真实图标时的占位）。 */
const AVATAR_COLORS = [
  '#2C5BFF',
  '#0E9F6E',
  '#7A5AF8',
  '#E4570B',
  '#129A4E',
  '#DC3A55',
  '#0E1A38',
  '#0E7490',
  '#A16207',
  '#4C8DFF',
]

function avatarColor(packageName: string): string {
  let hash = 0
  for (let i = 0; i < packageName.length; i += 1) {
    hash = (hash * 31 + packageName.charCodeAt(i)) >>> 0
  }
  return AVATAR_COLORS[hash % AVATAR_COLORS.length]
}

/** 行内说明：分身继承主应用配置 / 该包当前未安装。 */
function metaOf(app: AppListItem): string {
  const parts: string[] = []
  if (app.configured && app.inherited) parts.push(t('apps.status.inherited'))
  if (app.installed === false) parts.push(t('apps.status.not_installed'))
  return parts.join(' · ')
}

watch(
  () => [props.apps, props.loading],
  () => {
    nextTick(() => setupIconObserver())
  }
)

onMounted(() => {
  setupIconObserver()
})

onUnmounted(() => {
  teardownIconObserver()
})
</script>

<style scoped>
.apps-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

/*
 * 列表行不用毛玻璃：backdrop-filter 是 WebView 滚动中最贵的绘制操作，
 * 几百行逐帧模糊必然掉帧。跳过视口外的布局/绘制（近似虚拟化）。
 */
.app-row {
  display: flex;
  align-items: center;
  gap: 11px;
  width: 100%;
  padding: 12px 13px;
  border: 1px solid var(--line);
  border-radius: var(--radius-tile);
  background: var(--surface);
  box-shadow: var(--shadow-1);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transform-origin: center;
  transition: transform 0.15s ease;
  contain: layout style paint;
  content-visibility: auto;
  contain-intrinsic-size: auto 64px;
  -webkit-tap-highlight-color: transparent;
  user-select: none;
  -webkit-user-select: none;
}

html.dark .app-row {
  box-shadow: none;
}

.app-row:active {
  transform: scale(0.985);
}

.app-row__avatar {
  position: relative;
  display: grid;
  place-items: center;
  flex: none;
  width: 38px;
  height: 38px;
  border-radius: 12px;
  overflow: hidden;
  color: #fff;
  font-size: 15px;
  font-weight: 600;
  line-height: 1;
}

.app-row__initial {
  pointer-events: none;
}

.app-row__icon {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  opacity: 0;
  transition: opacity 0.22s ease;
}

.app-row__icon--loaded {
  opacity: 1;
}

.app-row__body {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-width: 0;
}

.app-row__name {
  font-size: 14px;
  font-weight: 600;
  letter-spacing: -0.005em;
  line-height: 1.3;
  color: var(--ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.app-row__package {
  margin-top: 3px;
  font-size: 10px;
  line-height: 1.3;
  color: var(--dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.app-row__meta {
  margin-top: 3px;
  font-size: 10px;
  line-height: 1.3;
  color: var(--muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.app-row__tag {
  display: inline-flex;
  flex: none;
  align-items: center;
  max-width: 120px;
  margin-left: auto;
  padding: 4px 8px;
  border-radius: 7px;
  background: var(--brand-soft);
  color: var(--brand);
  font-size: 10.5px;
  font-weight: 560;
  line-height: 1.25;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.app-row__tag--empty {
  max-width: 82px;
  background: transparent;
  border: 1px dashed var(--line-strong);
  color: var(--muted);
  font-weight: 500;
}

.app-row__chev {
  flex: none;
  color: var(--dim);
}

.empty-state {
  margin-top: 2px;
}

/* —— loading（页面级骨架另有 AppsPageSkeleton，这里只做兜底） —— */
.row-skeleton {
  display: flex;
  align-items: center;
  gap: 11px;
  padding: 12px 13px;
  border: 1px solid var(--line);
  border-radius: var(--radius-tile);
  background: var(--surface);
  pointer-events: none;
}

.row-skeleton__avatar {
  width: 38px;
  height: 38px;
  flex: none;
  border-radius: 12px;
}

.row-skeleton__body {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.row-skeleton__name {
  width: 45%;
  height: 12px;
}

.row-skeleton__pkg {
  width: 62%;
  height: 9px;
}

.row-skeleton__tag {
  width: 62px;
  height: 20px;
  flex: none;
  border-radius: 7px;
}
</style>
