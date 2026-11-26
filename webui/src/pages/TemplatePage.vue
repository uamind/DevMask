<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div class="template-page">
    <Transition :name="viewTransitionName" @after-enter="handleViewAfterEnter">
      <OnlineTemplateLibraryView
        v-if="onlineTemplatesStore.libraryOpen"
        key="online-library"
        @close="closeOnlineLibrary"
      />

      <div v-else key="template-list" class="template-home-view">
        <TemplateHeader
          :count="allTemplates.length"
          @open-online="showOnlineLibrary"
          @open-create="showCreateDialog"
          @open-transfer="showTransferDialog"
          @search="handleSearch"
        />

        <!--
          包名重复配置：多个模板声明同一包名时只有排在前面的生效。
          默认收起成一行摘要，点开才展开逐条明细——告警要留，但不抢占整页注意力。
        -->
        <section v-if="conflictingPackages.length > 0" class="conflict">
          <button
            type="button"
            class="conflict__head"
            :aria-expanded="conflictExpanded"
            @click="conflictExpanded = !conflictExpanded"
          >
            <TriangleAlert :size="14" class="conflict__icon" aria-hidden="true" />
            <span class="conflict__title">{{ t('templates.conflict.banner_title') }}</span>
            <span class="conflict__count dm-mono">{{ conflictingPackages.length }}</span>
            <ChevronDown
              :size="14"
              :class="['conflict__chev', { 'is-open': conflictExpanded }]"
              aria-hidden="true"
            />
          </button>

          <div v-if="conflictExpanded" class="conflict__body">
            <p class="conflict__desc">{{ t('templates.conflict.banner_desc') }}</p>
            <ul class="conflict__list">
              <li v-for="conflict in conflictingPackages" :key="conflict.packageName">
                <span class="conflict__pkg dm-mono">{{ conflict.packageName }}</span>
                <span class="conflict__eff">
                  {{ t('templates.conflict.effective', { name: conflict.templates[0] }) }}
                </span>
                <span class="conflict__ign">
                  {{
                    t('templates.conflict.ignored', {
                      names: conflict.templates.slice(1).join(', '),
                    })
                  }}
                </span>
              </li>
            </ul>
          </div>
        </section>

        <TemplateList
          :entries="filteredTemplates"
          :is-searching="searchQuery.length > 0"
          @export="handleExport"
          @edit="handleEdit"
          @delete="deleteTemplateConfirm"
          @create="showCreateDialog"
          @online="showOnlineLibrary"
        />

        <TemplateDialog
          v-if="dialogVisible"
          v-model="dialogVisible"
          :is-editing="isEditing"
          :locale="locale"
          :template-name="editingTemplateName"
          :template-data="editingTemplate"
          @saved="handleTemplateSaved"
        />

        <TemplateTransferDialog v-if="transferDialogVisible" v-model="transferDialogVisible" />
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { computed, defineAsyncComponent, onActivated, ref } from 'vue'
import { ChevronDown, TriangleAlert } from '@lucide/vue'
import { storeToRefs } from 'pinia'
import TemplateHeader from '../components/templates/TemplateHeader.vue'
import TemplateList from '../components/templates/TemplateList.vue'
import OnlineTemplateLibraryView from '../components/OnlineTemplateLibraryView.vue'
import { useConfigStore } from '../stores/config'
import { useOnlineTemplatesStore } from '../stores/onlineTemplates'
import { useModalHistory } from '../composables/useModalHistory'
import { useI18n } from '../utils/i18n'
import { useLazyMessageBox } from '../utils/elementPlus'
import { copyTextToClipboard, stringifyTemplatesToToml } from '../utils/templateTransfer'
import { findConflictingPackages } from '../utils/config'
import { toast } from 'kernelsu-alt'
import type { Template } from '../types'

const TemplateDialog = defineAsyncComponent(
  () => import('../components/templates/TemplateDialog.vue')
)
const TemplateTransferDialog = defineAsyncComponent(
  () => import('../components/templates/TemplateTransferDialog.vue')
)

const configStore = useConfigStore()
const onlineTemplatesStore = useOnlineTemplatesStore()
const { libraryOpen } = storeToRefs(onlineTemplatesStore)
const { t, locale } = useI18n()
const getMessageBox = useLazyMessageBox()

const searchQuery = ref('')
const conflictExpanded = ref(false)
const viewTransitionName = ref<'template-library-forward' | 'template-library-back'>(
  'template-library-forward'
)

const allTemplates = computed(() => configStore.templateEntries)

/**
 * 被多个模板同时声明的包名。运行时只取第一个模板，其余静默忽略
 * —— 这是「配了三星却生效 iQOO」的根因，必须在模板页显式告警。
 */
const conflictingPackages = computed(() => findConflictingPackages(allTemplates.value))

const filteredTemplates = computed(() => {
  if (!searchQuery.value.trim()) {
    return allTemplates.value
  }

  const tokens = searchQuery.value.toLowerCase().trim().split(/\s+/).filter(Boolean)

  return allTemplates.value.filter(({ name, template }) => {
    const searchFields = [
      name,
      template.brand || '',
      template.model || '',
      template.build_id || '',
      template.device || '',
      template.manufacturer || '',
      template.product || '',
      template.board || '',
      template.soc_model || '',
      template.description || '',
    ]

    // 与在线模板库一致：多关键词按「全部命中」匹配，描述里的多段信息也能搜到
    const haystack = searchFields.join(' ').toLowerCase()
    return tokens.every((token) => haystack.includes(token))
  })
})

function handleSearch(query: string) {
  searchQuery.value = query
}

const dialogVisible = ref(false)
const transferDialogVisible = ref(false)
const isEditing = ref(false)
const editingTemplateName = ref<string | null>(null)
const editingTemplate = ref<Template | null>(null)

function showOnlineLibrary() {
  viewTransitionName.value = 'template-library-forward'
  onlineTemplatesStore.openLibrary()
  void onlineTemplatesStore.ensureCatalogLoaded()
}

function closeOnlineLibrary() {
  viewTransitionName.value = 'template-library-back'
  onlineTemplatesStore.closeLibrary()
}

function handleViewAfterEnter() {
  window.dispatchEvent(new Event('resize'))
}

useModalHistory(libraryOpen, closeOnlineLibrary)
useModalHistory(dialogVisible, () => {
  dialogVisible.value = false
})
useModalHistory(transferDialogVisible, () => {
  transferDialogVisible.value = false
})

function showCreateDialog() {
  isEditing.value = false
  editingTemplateName.value = null
  editingTemplate.value = null
  dialogVisible.value = true
}

function showTransferDialog() {
  transferDialogVisible.value = true
}

function handleEdit(name: string, template: Template) {
  isEditing.value = true
  editingTemplateName.value = name
  editingTemplate.value = template
  dialogVisible.value = true
}

async function handleExport(name: string, template: Template) {
  try {
    const content = stringifyTemplatesToToml({ [name]: template })
    const copied = await copyTextToClipboard(content)
    toast(
      copied
        ? t('templates.messages.export_copy_success')
        : t('templates.messages.export_copy_failed')
    )
  } catch (error) {
    console.error('Export template failed:', error)
    toast(t('templates.messages.export_copy_failed'))
  }
}

async function deleteTemplateConfirm(name: string) {
  try {
    const messageBox = await getMessageBox()
    await messageBox.confirm(
      t('templates.dialog.delete_confirm', { name }),
      t('templates.dialog.delete_title'),
      {
        confirmButtonText: t('common.delete'),
        cancelButtonText: t('common.cancel'),
        type: 'warning',
        appendTo: 'body',
        customClass: 'delete-confirm-box',
        modalClass: 'delete-confirm-modal',
      }
    )

    configStore.deleteTemplate(name)
    await configStore.saveConfig()
    toast(t('templates.messages.deleted'))
  } catch (e) {
    if (e === 'cancel') return
    console.error('Delete template failed:', e)
    const errorMessage = e instanceof Error ? e.message : String(e)
    toast(`${t('common.failed')}: ${errorMessage}`)
  }
}

function handleTemplateSaved() {
  // 保存后无需额外动作，保留扩展点
}

onActivated(() => {
  // KeepAlive 激活时触发一次尺寸计算，确保列表布局正确
  window.dispatchEvent(new Event('resize'))
})
</script>

<style scoped>
.template-page {
  --template-view-slide-distance: 1.25rem;
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 1rem;
  width: 100%;
  max-width: 100%;
  box-sizing: border-box;
  overflow: hidden;
  padding-top: 4px;
}

.template-home-view {
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: 100%;
}

/* —— 包名重复配置告警：一行摘要，展开后才看明细 —— */
.conflict {
  border: 1px solid var(--line);
  border-left: 3px solid var(--warn);
  border-radius: var(--radius-tile);
  background: var(--warn-soft);
  overflow: hidden;
}

html.dark .conflict {
  background: var(--surface);
  border-color: var(--line);
  border-left-color: var(--warn);
}

.conflict__head {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  padding: 9px 11px;
  border: 0;
  background: transparent;
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.conflict__icon {
  flex: none;
  color: var(--warn);
}

.conflict__title {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  font-weight: 600;
  color: var(--warn);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conflict__count {
  flex: none;
  font-size: 11px;
  color: var(--warn);
  opacity: 0.75;
}

.conflict__chev {
  flex: none;
  color: var(--warn);
  opacity: 0.75;
  transition: transform 0.18s ease;
}

.conflict__chev.is-open {
  transform: rotate(180deg);
}

.conflict__body {
  padding: 0 11px 10px;
}

.conflict__desc {
  margin: 0 0 7px;
  font-size: 11px;
  line-height: 1.55;
  color: var(--warn);
  opacity: 0.82;
}

.conflict__list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  list-style: none;
}

.conflict__list li {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  font-size: 10.5px;
  line-height: 1.45;
}

.conflict__pkg {
  padding: 1px 5px;
  border-radius: 5px;
  background: var(--surface);
  border: 1px solid var(--line);
  font-size: 10px;
  color: var(--ink);
}

.conflict__eff {
  color: var(--ok);
  font-weight: 600;
}

.conflict__ign {
  color: var(--muted);
  text-decoration: line-through;
  text-decoration-color: var(--dim);
}

/* —— 在线模板库 / 列表之间的横向转场 —— */
.template-library-forward-enter-active,
.template-library-forward-leave-active,
.template-library-back-enter-active,
.template-library-back-leave-active {
  transition:
    opacity 220ms ease,
    transform 260ms cubic-bezier(0.22, 1, 0.36, 1);
  will-change: opacity, transform;
}

.template-library-forward-leave-active,
.template-library-back-leave-active {
  position: absolute;
  inset: 0;
  z-index: 1;
  width: 100%;
}

.template-library-forward-enter-active,
.template-library-back-enter-active {
  position: relative;
  z-index: 2;
}

.template-library-forward-enter-from {
  opacity: 0;
  transform: translate3d(var(--template-view-slide-distance), 0, 0);
}

.template-library-forward-leave-to {
  opacity: 0;
  transform: translate3d(calc(var(--template-view-slide-distance) * -1), 0, 0);
}

.template-library-back-enter-from {
  opacity: 0;
  transform: translate3d(calc(var(--template-view-slide-distance) * -1), 0, 0);
}

.template-library-back-leave-to {
  opacity: 0;
  transform: translate3d(var(--template-view-slide-distance), 0, 0);
}

@media (prefers-reduced-motion: reduce) {
  .template-library-forward-enter-active,
  .template-library-forward-leave-active,
  .template-library-back-enter-active,
  .template-library-back-leave-active {
    transition-duration: 1ms;
  }

  .template-library-forward-enter-from,
  .template-library-forward-leave-to,
  .template-library-back-enter-from,
  .template-library-back-leave-to {
    transform: none;
  }
}
</style>
