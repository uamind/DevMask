<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <el-dialog
    v-model="visible"
    :title="dialogTitle"
    width="90%"
    :close-on-click-modal="false"
    :append-to-body="true"
    :destroy-on-close="true"
    :z-index="2001"
    class="app-config-dialog"
    modal-class="app-config-modal"
  >
    <div class="dlg">
      <!-- 配置类型：自定义 / 模板（白卡，点卡片切分组，勾选框启用该类配置） -->
      <div class="dlg-modes" role="tablist">
        <div
          :class="['mode-card', { 'mode-card--active': activeTab === 'custom' }]"
          role="tab"
          tabindex="0"
          :aria-selected="activeTab === 'custom'"
          @click="activeTab = 'custom'"
          @keydown.enter="activeTab = 'custom'"
          @keydown.space.prevent="activeTab = 'custom'"
        >
          <el-checkbox
            :model-value="hasCustomConfig"
            @click.stop
            @change="(val: boolean) => handleConfigToggle('custom', val)"
          />
          <span class="mode-card__label">{{ t('apps.dialog.mode_custom') }}</span>
        </div>
        <div
          :class="['mode-card', { 'mode-card--active': activeTab === 'template' }]"
          role="tab"
          tabindex="0"
          :aria-selected="activeTab === 'template'"
          @click="activeTab = 'template'"
          @keydown.enter="activeTab = 'template'"
          @keydown.space.prevent="activeTab = 'template'"
        >
          <el-checkbox
            :model-value="hasTemplateConfig"
            @click.stop
            @change="(val: boolean) => handleConfigToggle('template', val)"
          />
          <span class="mode-card__label">{{ t('apps.dialog.mode_template') }}</span>
        </div>
      </div>

      <!-- 生效范围：明示命中的配置键作用到谁（仅主应用 / 主应用+分身回退 / 仅该分身） -->
      <p v-if="scopeText" class="dlg-scope">{{ scopeText }}</p>

      <!-- 继承提示：分身当前只是回退命中主应用的裸包名配置，本实例尚无自有条目 -->
      <p v-if="inheritedHintVisible" class="dlg-note">
        {{ t('apps.dialog.inherited_hint', { base: inheritedBase }) }}
      </p>

      <!-- 模板配置分组 -->
      <section v-if="activeTab === 'template'" class="dlg-card">
        <header class="dlg-card__head">
          <h3 class="dlg-card__title">{{ t('apps.dialog.mode_template') }}</h3>
        </header>
        <div class="dlg-card__body">
          <el-input
            v-model="templateSearch"
            :placeholder="t('apps.dialog.search_template_placeholder')"
            prefix-icon="Search"
            clearable
            class="template-search"
          />
          <el-select
            v-model="selectedTemplate"
            :placeholder="t('apps.dialog.select_template_placeholder')"
            :no-data-text="templateNoDataText"
            :no-match-text="templateNoMatchText"
            filterable
            style="width: 100%"
          >
            <el-option
              v-for="option in filteredTemplateOptions"
              :key="option.name"
              :label="option.label"
              :value="option.name"
            />
          </el-select>

          <el-button
            v-if="hasTemplateConfig"
            class="dlg-remove"
            type="danger"
            plain
            @click="removeTemplateConfig"
          >
            {{ t('apps.dialog.remove_template_config') }}
          </el-button>
        </div>
      </section>

      <!-- 自定义配置分组 -->
      <section v-if="activeTab === 'custom'" class="dlg-card">
        <header class="dlg-card__head">
          <h3 class="dlg-card__title">{{ t('apps.dialog.mode_custom') }}</h3>
        </header>
        <div class="dlg-card__body">
          <el-form :model="formData" label-position="top">
            <DevMaskFormFields />
          </el-form>

          <el-button
            v-if="hasCustomConfig"
            class="dlg-remove"
            type="danger"
            plain
            @click="removeCustomConfig"
          >
            {{ t('apps.dialog.remove_custom_config') }}
          </el-button>
        </div>
      </section>
    </div>

    <template #footer>
      <div class="dlg-foot">
        <el-button @click="visible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" @click="saveAppConfig">{{ t('common.confirm') }}</el-button>
      </div>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useConfigStore } from '../../stores/config'
import { useI18n } from '../../utils/i18n'
import { toast } from 'kernelsu-alt'
import type { InstalledApp, AppConfig } from '../../types'
import { provideDevMaskForm } from '../../composables/useDevMaskForm'
import DevMaskFormFields from '../shared/DevMaskFormFields.vue'
import {
  configKeyScope,
  configWriteKey,
  filterOutOwnConfigKeys,
  findOwnConfigKey,
  ownConfigKeys,
  parsePackageUser,
} from '../../utils/package'

interface TemplateOption {
  name: string
  label: string
  searchable: string
}

const props = defineProps<{
  modelValue: boolean
  app: InstalledApp | null
}>()

const emit = defineEmits<{ 'update:modelValue': [boolean]; saved: [] }>()

const configStore = useConfigStore()
const { t } = useI18n()

const templates = computed(() => configStore.getTemplates())
const visible = computed({
  get: () => props.modelValue,
  set: (val: boolean) => emit('update:modelValue', val),
})

const activeTab = ref<'template' | 'custom'>('custom')

const hasCustomConfig = ref(false)
const hasTemplateConfig = ref(false)

const originalCustomConfig = ref<AppConfig | null>(null)
const originalTemplateName = ref<string | null>(null)
/** 运行时命中的配置键（含可能继承主应用的裸 base 回退键），写回时会换算成本实例自有键 */
const resolvedConfigKey = ref<string | null>(null)
/** 当前配置是否只是「回退命中主应用的裸包名条目」（分身未单独配置） */
const customInherited = ref(false)
const templateInherited = ref(false)

const inheritedBase = computed(() => parsePackageUser(props.app?.packageName || '').base)
const inheritedHintVisible = computed(() => customInherited.value || templateInherited.value)

/**
 * 生效范围提示：把命中的配置键作用域写清楚。
 * 继承态不显示（此时由 inheritedHint 说明「继承自主应用」，避免两句话重复）。
 */
const scopeText = computed(() => {
  const key = resolvedConfigKey.value
  if (!key || inheritedHintVisible.value) {
    return ''
  }

  const scope = configKeyScope(key)
  if (scope === 'user0') {
    return t('apps.dialog.scope_user0')
  }
  if (scope === 'fallback') {
    return t('apps.dialog.scope_fallback')
  }

  return t('apps.dialog.scope_clone', { uid: String(parsePackageUser(key).userId) })
})

const selectedTemplate = ref('')
const templateSearch = ref('')

const { formData, resetForm, fillFromAppConfig, toAppConfig } = provideDevMaskForm()

const dialogTitle = computed(() =>
  t('apps.dialog.config_title', { name: props.app?.appName || '' })
)

const templateOptions = computed<TemplateOption[]>(() => {
  const allTemplates = templates.value || {}

  return Object.entries(allTemplates).map(([name, template]) => {
    const label = `${name} - ${template.brand || ''} ${template.model || ''}`
      .replace(/\s+/g, ' ')
      .trim()

    const searchable = [
      name,
      template.brand,
      template.model,
      template.marketname,
      template.build_id,
      template.device,
      template.product,
    ]
      .filter(Boolean)
      .map((part) => String(part).toLowerCase())
      .join(' ')

    return {
      name,
      label: label || name,
      searchable,
    }
  })
})

const filteredTemplateOptions = computed(() => {
  const keyword = templateSearch.value.trim().toLowerCase()
  if (!keyword) return templateOptions.value

  return templateOptions.value.filter((option) => option.searchable.includes(keyword))
})

const templateNoDataText = computed(() =>
  templateSearch.value.trim() ? t('apps.dialog.search_no_result') : t('apps.dialog.no_templates')
)
const templateNoMatchText = computed(() => t('apps.dialog.search_no_result'))

function handleConfigToggle(type: 'custom' | 'template', enabled: boolean) {
  if (type === 'custom') {
    hasCustomConfig.value = enabled
    if (enabled) {
      activeTab.value = 'custom'
    }
  } else {
    hasTemplateConfig.value = enabled
    if (enabled) {
      activeTab.value = 'template'
      if (!selectedTemplate.value && templateOptions.value.length > 0) {
        selectedTemplate.value = templateOptions.value[0].name
      }
    } else {
      selectedTemplate.value = ''
    }
  }
}

/**
 * 只移除本实例自有键。分身的裸 base 键是「主应用的键 + 分身回退键」，绝不能由分身编辑删除
 * （历史 bug：分身改模板把主应用的 packages 一并清空，主应用变「未配置」）。
 */
function removeOwnAppEntries(packageName: string): boolean {
  let removed = false
  for (const key of ownConfigKeys(packageName)) {
    if (!configStore.getApps().some((app) => app.package === key)) continue
    configStore.deleteApp(key)
    removed = true
  }
  return removed
}

function removeOwnTemplateEntries(packageName: string): boolean {
  let removed = false
  for (const { name, template } of configStore.templateEntries) {
    if (!template.packages?.length) continue
    const packages = filterOutOwnConfigKeys(template.packages, packageName)
    if (packages.length === template.packages.length) continue
    configStore.setTemplate(name, { ...template, packages })
    removed = true
  }
  return removed
}

function hasOwnAppEntry(packageName: string): boolean {
  const own = new Set(ownConfigKeys(packageName))
  return configStore.getApps().some((app) => own.has(app.package))
}

function hasOwnTemplateEntry(packageName: string): boolean {
  return configStore.templateEntries.some(
    ({ template }) => findOwnConfigKey(template.packages, packageName) !== null
  )
}

/** 配置只继承自主应用的裸包名条目：无法在本实例内移除，提示用户去主应用处理。 */
function notifyInherited() {
  toast(t('apps.messages.inherited_config', { base: inheritedBase.value }))
}

/**
 * 表单当前值与原配置是否等价（忽略空串/空对象/空数组，避免 fillFromAppConfig→toAppConfig
 * 往返把 undefined 写成 '' 造成假差异）。用于「没改就不落盘」：分身仅继承主应用配置时，
 * 打开对话框直接点确定不应静默把继承配置固化成本实例条目（否则此后不再跟随主应用改动）。
 */
function isFormEquivalentToAppConfig(original: AppConfig, packageName: string): boolean {
  const canonical = (config: AppConfig): string =>
    JSON.stringify(
      Object.entries(config)
        .filter(([key, value]) => {
          if (key === 'package') return false
          if (value === undefined || value === null || value === '') return false
          if (Array.isArray(value)) return value.length > 0
          if (typeof value === 'object') return Object.keys(value as object).length > 0
          return true
        })
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    )

  return canonical(toAppConfig(packageName)) === canonical(original)
}

function syncFromExistingConfig() {
  if (!props.app) return

  templateSearch.value = ''
  const packageName = props.app.packageName

  // 与 Rust 运行时严格同源（src/lib.rs + src/config.rs get_merged_config）：
  // 键优先（pkg@uid → 裸 pkg），命中第一个键即停；同一键内 [[apps]] 先于模板。
  // 不能用「先按模板书写顺序遍历」或「两类配置各自跨键扫描」：
  //  - 前者让先书写模板里的裸回退键遮蔽后书写模板的 @uid 精确键（分身显示主应用模板）；
  //  - 后者会把更靠后键上的另一类配置当成生效项（例：分身在 @uid 键被模板命中时，
  //    裸 base 上的 [[apps]] 条目对分身并不生效，不能显示成「继承的自定义配置」）。
  const matches = configStore.resolvePackageMatchesFor(packageName)
  const appMatch = matches?.appMatch ?? null
  const templateMatch = matches?.templateMatch ?? null

  resolvedConfigKey.value = matches?.key ?? null

  if (appMatch) {
    hasCustomConfig.value = true
    customInherited.value = appMatch.inherited
    originalCustomConfig.value = { ...appMatch.app }
    fillFromAppConfig(appMatch.app)
  } else {
    hasCustomConfig.value = false
    customInherited.value = false
    originalCustomConfig.value = null
    resetForm()
  }

  if (templateMatch) {
    hasTemplateConfig.value = true
    templateInherited.value = templateMatch.inherited
    originalTemplateName.value = templateMatch.templateName
    selectedTemplate.value = templateMatch.templateName
  } else {
    hasTemplateConfig.value = false
    templateInherited.value = false
    originalTemplateName.value = null
    selectedTemplate.value = ''
  }

  // 默认展示运行时真正生效的那一种：同一个键内 [[apps]] 先于模板
  activeTab.value = appMatch ? 'custom' : templateMatch ? 'template' : 'custom'
}

async function removeCustomConfig() {
  if (!props.app) return

  const packageName = props.app.packageName
  // 继承来的条目没有本实例自有键：删不掉，也不能删（会连主应用一起废掉）
  if (!hasOwnAppEntry(packageName)) {
    notifyInherited()
    return
  }

  removeOwnAppEntries(packageName)
  hasCustomConfig.value = false
  customInherited.value = false
  originalCustomConfig.value = null
  resetForm()

  try {
    await configStore.saveConfig()
    toast(t('apps.messages.custom_config_removed'))
  } catch {
    toast(t('common.failed'))
  }
}

async function removeTemplateConfig() {
  if (!props.app) return

  const packageName = props.app.packageName
  if (!hasOwnTemplateEntry(packageName)) {
    notifyInherited()
    return
  }

  removeOwnTemplateEntries(packageName)
  hasTemplateConfig.value = false
  templateInherited.value = false
  originalTemplateName.value = null
  selectedTemplate.value = ''

  try {
    await configStore.saveConfig()
    toast(t('apps.messages.template_config_removed'))
  } catch {
    toast(t('common.failed'))
  }
}

async function saveAppConfig() {
  if (!props.app) return

  const packageName = props.app.packageName
  const writeKey = configWriteKey(packageName, resolvedConfigKey.value)

  // 只是继承主应用的条目时，「取消勾选/移除」在本实例里没有可删的东西：不阻断保存其他改动，
  // 只在结束后提示原因（裸 base 键绝不能由分身删改）。
  const keepsInheritedTemplate =
    !hasTemplateConfig.value && templateInherited.value && !hasOwnTemplateEntry(packageName)
  const keepsInheritedCustom =
    !hasCustomConfig.value && customInherited.value && !hasOwnAppEntry(packageName)

  if (hasTemplateConfig.value) {
    if (!selectedTemplate.value) {
      toast(t('apps.messages.select_template'))
      return
    }

    const targetName = selectedTemplate.value
    if (!templates.value[targetName]) {
      toast(t('apps.messages.select_template'))
      return
    }

    // 只有「换了模板」或「新启用模板配置」才写盘：原模板未变时不重复写，
    // 尤其不能把继承自主应用的裸 base 键误固化成该分身的自有条目。
    const templateSelectionChanged =
      !originalTemplateName.value || originalTemplateName.value !== targetName

    if (originalTemplateName.value && templateSelectionChanged) {
      // 换模板：只把本实例自有键从旧模板剔除；主应用的裸 base 键与别的分身键原样保留
      const oldTemplate = templates.value[originalTemplateName.value]
      if (oldTemplate?.packages?.length) {
        const packages = filterOutOwnConfigKeys(oldTemplate.packages, packageName)
        if (packages.length !== oldTemplate.packages.length) {
          configStore.setTemplate(originalTemplateName.value, { ...oldTemplate, packages })
        }
      }
    }

    if (templateSelectionChanged) {
      const targetTemplate = templates.value[targetName]
      if (targetTemplate) {
        const packages = targetTemplate.packages ? [...targetTemplate.packages] : []
        // 复用本实例已有键；继承来的裸 base 键不算命中，分身必须写 pkg@uid
        const existingOwnKey = findOwnConfigKey(packages, packageName)
        const keyToWrite = existingOwnKey ?? writeKey
        if (!packages.includes(keyToWrite)) {
          packages.push(keyToWrite)
          configStore.setTemplate(targetName, { ...targetTemplate, packages })
        }
      }
    }
  } else {
    removeOwnTemplateEntries(packageName)
  }

  if (hasCustomConfig.value) {
    // 继承自主应用的条目：只有表单实际改动才落成该分身的自有条目（没改就不写盘）
    const inheritedAndUnchanged =
      customInherited.value &&
      originalCustomConfig.value !== null &&
      isFormEquivalentToAppConfig(originalCustomConfig.value, packageName)
    if (!inheritedAndUnchanged) {
      configStore.setApp(toAppConfig(writeKey))
    }
  } else {
    removeOwnAppEntries(packageName)
  }

  try {
    await configStore.saveConfig()
    if (keepsInheritedTemplate || keepsInheritedCustom) {
      // 其他改动已保存；被继承的那一份无法在本实例内移除，单独提示
      notifyInherited()
    } else {
      toast(t('apps.messages.saved'))
    }
    visible.value = false
    emit('saved')
  } catch {
    toast(t('common.failed'))
  }
}

watch(
  () => [props.app, visible.value],
  ([, dialogVisible]) => {
    if (dialogVisible) {
      syncFromExistingConfig()
    }
  },
  { immediate: true }
)

watch(selectedTemplate, (val) => {
  if (val) {
    hasTemplateConfig.value = true
  }
})
</script>

<style>
/* ───────────────────────────────────────────────────────────────────────────
 * 应用配置弹窗：白卡分组、圆角 18、主按钮 brand；不再使用毛玻璃。
 *
 * 不加 scoped：el-dialog 的骨架（header/body/footer）由 Element Plus 的子组件
 * 渲染，作用域选择器依赖 scope id，统一用 .app-config-dialog 前缀隔离更可靠。
 * ─────────────────────────────────────────────────────────────────────────── */

.el-dialog.app-config-dialog {
  display: flex;
  flex-direction: column;
  width: 90%;
  max-width: 420px;
  margin: 5vh auto 80px !important;
  max-height: calc(100vh - 80px - 5vh) !important;
  padding: 0 !important;
  background: var(--bg) !important;
  border: 1px solid var(--line) !important;
  border-radius: var(--radius-card) !important;
  box-shadow: var(--shadow-2) !important;
  overflow: hidden !important;
}

/* —— 头 —— */
.el-dialog.app-config-dialog .el-dialog__header {
  flex: none;
  margin: 0;
  padding: 15px 18px 12px;
  background: var(--surface);
  border-bottom: 1px solid var(--line);
}

.el-dialog.app-config-dialog .el-dialog__header.show-close {
  padding-right: 48px;
}

.el-dialog.app-config-dialog .el-dialog__title {
  font-size: 15px;
  font-weight: 660;
  letter-spacing: -0.01em;
  line-height: 1.35;
  color: var(--ink) !important;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.el-dialog.app-config-dialog .el-dialog__headerbtn {
  top: 2px;
  right: 2px;
  width: 42px;
  height: 42px;
}

.el-dialog.app-config-dialog .el-dialog__close {
  font-size: 15px;
  color: var(--muted) !important;
}

/* —— 内容：页面底色 + 白卡分组 —— */
.el-dialog.app-config-dialog .el-dialog__body {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  padding: 14px 16px 18px;
  background: var(--bg);
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.el-dialog.app-config-dialog .el-dialog__body::-webkit-scrollbar {
  display: none;
}

/* —— 底栏 —— */
.el-dialog.app-config-dialog .el-dialog__footer {
  flex: none;
  padding: 12px 16px calc(12px + var(--safe-area-inset-bottom, 0px));
  background: var(--surface);
  border-top: 1px solid var(--line);
}

.el-dialog.app-config-dialog .dlg-foot {
  display: flex;
  gap: 8px;
}

.el-dialog.app-config-dialog .dlg-foot .el-button {
  flex: 1 1 0;
  height: 40px;
  margin: 0 !important;
  border-color: var(--line);
  border-radius: var(--radius-btn) !important;
  font-family: inherit;
  font-size: 13.5px;
  font-weight: 560;
  color: var(--ink);
}

.el-dialog.app-config-dialog .dlg-foot .el-button--primary {
  background: var(--brand) !important;
  border-color: var(--brand) !important;
  color: #fff !important;
  font-weight: 620;
}

.el-dialog.app-config-dialog .dlg-foot .el-button--primary:hover {
  filter: brightness(0.96);
}

/* —— 内容块 —— */
.el-dialog.app-config-dialog .dlg {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.el-dialog.app-config-dialog .dlg-modes {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}

.el-dialog.app-config-dialog .mode-card {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  padding: 11px 12px;
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--radius-btn);
  box-shadow: var(--shadow-1);
  cursor: pointer;
  transition:
    border-color 0.16s ease,
    background-color 0.16s ease;
  -webkit-tap-highlight-color: transparent;
}

html.dark .el-dialog.app-config-dialog .mode-card {
  box-shadow: none;
}

.el-dialog.app-config-dialog .mode-card--active {
  background: var(--brand-soft);
  border-color: var(--brand);
}

.el-dialog.app-config-dialog .mode-card .el-checkbox {
  flex: none;
  height: auto;
  margin-right: 0;
}

.el-dialog.app-config-dialog .mode-card__label {
  min-width: 0;
  font-size: 12.5px;
  font-weight: 560;
  line-height: 1.35;
  color: var(--ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.el-dialog.app-config-dialog .mode-card--active .mode-card__label {
  color: var(--brand);
  font-weight: 640;
}

.el-dialog.app-config-dialog .dlg-scope {
  padding: 0 2px;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--muted);
}

.el-dialog.app-config-dialog .dlg-note {
  padding: 9px 12px;
  border-radius: 12px;
  background: var(--brand-soft);
  color: var(--brand);
  font-size: 11.5px;
  font-weight: 500;
  line-height: 1.6;
}

.el-dialog.app-config-dialog .dlg-card {
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--radius-card);
  box-shadow: var(--shadow-1);
  overflow: hidden;
}

html.dark .el-dialog.app-config-dialog .dlg-card {
  box-shadow: none;
}

.el-dialog.app-config-dialog .dlg-card__head {
  padding: 12px 14px 0;
}

.el-dialog.app-config-dialog .dlg-card__title {
  font-size: 12.5px;
  font-weight: 660;
  letter-spacing: -0.005em;
  color: var(--ink);
}

.el-dialog.app-config-dialog .dlg-card__body {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 10px 14px 14px;
}

.el-dialog.app-config-dialog .dlg-card__body .el-form-item {
  margin-bottom: 14px;
}

.el-dialog.app-config-dialog .dlg-card__body .el-form-item:last-child {
  margin-bottom: 0;
}

.el-dialog.app-config-dialog .dlg-card__body .el-form-item__label {
  margin-bottom: 4px;
  padding: 0;
  font-size: 11.5px;
  line-height: 1.5;
  /* 全局 style.css 用 !important 把 EP 表单标签锁成主文字色；弹窗内要弱化成说明文字 */
  color: var(--muted) !important;
}

.el-dialog.app-config-dialog .dlg-remove {
  align-self: flex-start;
  margin-top: 2px;
}

/* —— 遮罩：一层压暗，不做毛玻璃 —— */
.el-overlay.app-config-modal {
  background-color: rgba(8, 11, 18, 0.42) !important;
}

html.dark .el-overlay.app-config-modal {
  background-color: rgba(2, 4, 8, 0.66) !important;
}
</style>
