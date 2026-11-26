<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <el-dialog
    v-model="visible"
    :title="isEditing ? t('templates.dialog.edit_title') : t('templates.dialog.new_title')"
    width="90%"
    :close-on-click-modal="false"
    :append-to-body="true"
    :destroy-on-close="true"
    :z-index="2001"
    class="template-dialog"
    modal-class="template-dialog-modal"
  >
    <el-form :model="formData" label-width="120px" label-position="top">
      <el-form-item :label="t('templates.fields.name')">
        <el-input v-model="templateNameInput" :placeholder="t('templates.placeholders.name')" />
      </el-form-item>

      <DevMaskFormFields>
        <template #packages>
          <el-form-item :label="t('templates.fields.packages')">
            <div class="package-manager">
              <div :class="['package-input-wrapper', { 'stacked-layout': locale === 'en' }]">
                <el-autocomplete
                  v-model="packageInput"
                  :fetch-suggestions="searchPackages"
                  :placeholder="t('templates.placeholders.packages')"
                  clearable
                  class="package-autocomplete"
                  @keyup.enter="addPackage"
                >
                  <template #default="{ item }">
                    <div class="package-suggestion">
                      <div class="app-info">
                        <div class="app-name">{{ item.appName }}</div>
                        <div class="package-name">{{ item.packageName }}</div>
                      </div>
                    </div>
                  </template>
                </el-autocomplete>
                <el-button type="primary" :disabled="!packageInput" @click="addPackage">
                  {{ t('templates.actions.add') }}
                </el-button>
              </div>

              <div v-if="formData.packages.length > 0" class="package-list">
                <div v-for="(pkg, index) in formData.packages" :key="pkg" class="package-item">
                  <div class="package-info">
                    <span class="package-name-text">{{ pkg }}</span>
                    <span v-if="getAppName(pkg)" class="app-name-text">{{ getAppName(pkg) }}</span>
                  </div>
                  <el-button type="danger" size="small" circle @click="removePackage(index)">
                    ×
                  </el-button>
                </div>
              </div>
              <div v-else class="package-list-empty">{{ t('templates.empty.packages') }}</div>
            </div>
          </el-form-item>
        </template>
      </DevMaskFormFields>
    </el-form>

    <template #footer>
      <el-button @click="visible = false">{{ t('common.cancel') }}</el-button>
      <el-button type="primary" @click="saveTemplate">{{ t('common.save') }}</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useConfigStore } from '../../stores/config'
import { useAppsStore } from '../../stores/apps'
import { useI18n } from '../../utils/i18n'
import { useLazyMessageBox } from '../../utils/elementPlus'
import { toast } from 'kernelsu-alt'
import type { Template } from '../../types'
import { provideDevMaskForm } from '../../composables/useDevMaskForm'
import DevMaskFormFields from '../shared/DevMaskFormFields.vue'

const props = defineProps<{
  modelValue: boolean
  isEditing: boolean
  locale: string
  templateName?: string | null
  templateData?: Template | null
}>()

const emit = defineEmits<{ 'update:modelValue': [boolean]; saved: [string] }>()

const { t } = useI18n()
const configStore = useConfigStore()
const appsStore = useAppsStore()
const getMessageBox = useLazyMessageBox()

const installedApps = computed(() => appsStore.installedApps)
const visible = computed({
  get: () => props.modelValue,
  set: (val: boolean) => emit('update:modelValue', val),
})

const packageInput = ref('')
const templateNameInput = ref('')

const { formData, resetForm, fillFromTemplate, toTemplate } = provideDevMaskForm()

function searchPackages(
  queryString: string,
  cb: (suggestions: Array<{ value: string; appName: string; packageName: string }>) => void
) {
  const suggestions = installedApps.value
    .filter((app) => {
      const query = queryString.toLowerCase()
      return (
        app.packageName.toLowerCase().includes(query) || app.appName.toLowerCase().includes(query)
      )
    })
    .map((app) => ({
      value: app.packageName,
      appName: app.appName,
      packageName: app.packageName,
    }))
  cb(suggestions)
}

async function addPackage() {
  const pkgName = packageInput.value.trim()
  if (!pkgName) return

  if (formData.value.packages.includes(pkgName)) {
    toast(t('templates.messages.pkg_exists'))
    return
  }

  formData.value.packages.push(pkgName)
  packageInput.value = ''
}

function removePackage(index: number) {
  formData.value.packages.splice(index, 1)
}

function getAppName(packageName: string): string {
  const app = installedApps.value.find((a) => a.packageName === packageName)
  return app ? app.appName : ''
}

async function confirmRenameOverwrite(oldName: string, newName: string) {
  const messageBox = await getMessageBox()
  await messageBox.confirm(
    t('templates.dialog.rename_overwrite_confirm', { oldName, newName }),
    t('templates.dialog.rename_overwrite_title'),
    {
      confirmButtonText: t('common.confirm'),
      cancelButtonText: t('common.cancel'),
      type: 'warning',
      appendTo: 'body',
    }
  )
}

async function saveTemplate() {
  const name = templateNameInput.value.trim()

  if (!name) {
    toast(t('templates.messages.name_required'))
    return
  }

  templateNameInput.value = name

  const originalTemplateName = props.templateName?.trim() || ''
  const isRenaming =
    props.isEditing && originalTemplateName.length > 0 && originalTemplateName !== name
  let overwriteRenameTarget = false

  const template = toTemplate(props.templateData || undefined)

  try {
    if (isRenaming && Object.prototype.hasOwnProperty.call(configStore.getTemplates(), name)) {
      await confirmRenameOverwrite(originalTemplateName, name)
      overwriteRenameTarget = true
    }

    if (isRenaming) {
      configStore.renameTemplate(originalTemplateName, name, template, {
        overwrite: overwriteRenameTarget,
      })
    } else {
      configStore.setTemplate(name, template, { replace: true })
    }

    await configStore.saveConfig()
    toast(t('templates.messages.saved'))
    emit('saved', name)
    visible.value = false
  } catch (e) {
    if (e === 'cancel') {
      return
    }

    console.error('Save template failed:', e)
    const errorMessage = e instanceof Error ? e.message : String(e)
    toast(`${t('common.failed')}: ${errorMessage}`)
  }
}

watch(
  () => [props.modelValue, props.isEditing, props.templateName, props.templateData],
  ([dialogVisible, editing]) => {
    if (!dialogVisible) return

    packageInput.value = ''
    if (editing) {
      fillFromTemplate(props.templateData!)
      templateNameInput.value = props.templateName || ''
    } else {
      resetForm()
      templateNameInput.value = ''
    }
  },
  { immediate: true }
)
</script>

<style scoped>
.package-manager {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.package-input-wrapper {
  display: flex;
  gap: 8px;
  align-items: center;
}

.package-input-wrapper.stacked-layout {
  flex-direction: column;
}

.package-autocomplete {
  width: 100%;
}

.package-input-wrapper :deep(.el-input__wrapper) {
  height: 32px;
}

.package-input-wrapper .el-button {
  height: 32px;
  padding: 0 16px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.package-input-wrapper.stacked-layout .el-button {
  width: 100%;
}

.package-suggestion {
  padding: 0.25rem 0;
}

.app-info {
  display: flex;
  flex-direction: column;
  gap: 0.125rem;
}

.app-name {
  font-size: 0.875rem;
  color: var(--ink);
}

.package-name {
  font-family: var(--font-mono);
  font-size: 0.75rem;
  color: var(--muted);
}

.package-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px;
  background: var(--surface-2);
  border: 1px solid var(--line);
  border-radius: var(--radius-btn);
  max-height: 300px;
  overflow-y: auto;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.package-list::-webkit-scrollbar {
  display: none;
}

.package-list-empty {
  padding: 18px 12px;
  text-align: center;
  color: var(--muted);
  font-size: 11.5px;
  line-height: 1.55;
  background: var(--surface-2);
  border: 1px dashed var(--line-strong);
  border-radius: var(--radius-btn);
}

.package-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 10px;
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: 10px;
}

.package-info {
  display: flex;
  flex-direction: column;
  gap: 0.125rem;
  flex: 1;
  min-width: 0;
}

.package-name-text {
  font-family: var(--font-mono);
  font-size: 11.5px;
  color: var(--ink);
  word-break: break-all;
}

.app-name-text {
  font-size: 10.5px;
  color: var(--muted);
}
</style>

<style>
/*
 * 模板编辑弹窗的外壳：与模板页/导入导出弹窗同一套 B/A 令牌。
 * 下拉、输入、折叠面板的细节样式由 style.css 统一处理，这里只管外壳。
 */
.el-dialog.template-dialog {
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--radius-card);
  box-shadow: var(--shadow-2);
  box-sizing: border-box;
  margin-top: 6vh !important;
  margin-bottom: 80px !important;
  max-height: calc(100vh - 80px - 6vh) !important;
  display: flex;
  flex-direction: column;
}

html.dark .el-dialog.template-dialog {
  box-shadow: none;
}

.el-dialog.template-dialog .el-dialog__header {
  padding: 14px 16px 10px;
  border-bottom: 1px solid var(--line);
  background: var(--surface);
}

.el-dialog.template-dialog .el-dialog__title {
  font-size: 14px;
  font-weight: 640;
  color: var(--ink);
}

.el-dialog.template-dialog .el-dialog__headerbtn {
  top: 8px;
  right: 8px;
  width: 32px;
  height: 32px;
}

.el-dialog.template-dialog .el-dialog__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 14px 16px 16px;
  background: var(--surface);
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.el-dialog.template-dialog .el-dialog__body::-webkit-scrollbar {
  display: none;
}

.el-dialog.template-dialog .el-dialog__footer {
  padding: 10px 16px 14px;
  border-top: 1px solid var(--line);
  background: var(--surface);
  flex-shrink: 0;
}
</style>
