<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (c) 2025-2026 Seyud
  DevMask: https://github.com/uamind/DevMask
-->

<template>
  <div>
    <div class="page-head">
      <div>
        <div class="eyebrow">EGRESS</div>
        <h1>代理池</h1>
        <p>
          设备级 HTTP 代理（方案 A：settings put global http_proxy）与应用级 SOCKS5 注入（方案 B）共用同一份代理定义。
          密码只写不读：这里的每一行都只有「是否已设置凭据」。带凭据的代理由设备内本地认证中继
          （127.0.0.1:localPort，HTTP Basic / SOCKS5 RFC 1929）持有凭据，应用只连中继。
        </p>
      </div>
      <div class="head-actions">
        <el-button :loading="manager.egressLoading" @click="reload">
          <el-icon v-if="!manager.egressLoading" style="margin-right: 4px"><Refresh /></el-icon>
          刷新
        </el-button>
        <el-button type="primary" @click="openCreate">
          <el-icon style="margin-right: 4px"><Plus /></el-icon>
          新建代理
        </el-button>
      </div>
    </div>

    <div class="toolbar">
      <el-input
        v-model="keyword"
        class="search"
        placeholder="搜索名称、主机或地区"
        clearable
        @keyup.enter="reload"
        @clear="reload"
      >
        <template #prefix><el-icon><Search /></el-icon></template>
      </el-input>
      <el-radio-group v-model="enabledFilter" size="small" @change="reload">
        <el-radio-button value="all">全部</el-radio-button>
        <el-radio-button value="enabled">启用</el-radio-button>
        <el-radio-button value="disabled">停用</el-radio-button>
      </el-radio-group>
      <span class="spacer"></span>
      <span class="toolbar-count">
        {{ manager.egressProxies.length }} / {{ manager.egressProxyTotal }} 个代理 ·
        {{ manager.boundProxyCount }} 个已绑定 · {{ socksCount }} 个 SOCKS5 ·
        {{ relayActiveCount }} 个经设备中继
      </span>
    </div>

    <div v-if="manager.egressLoading && !manager.egressProxies.length" class="panel empty-state">
      <div class="empty-icon"><el-icon class="is-loading"><Loading /></el-icon></div>
      <span>正在从 GET /egress-proxies 加载代理池…</span>
    </div>

    <section v-else class="egress-table">
      <div class="panel-head">
        <div>
          <h2>代理定义</h2>
          <p>
            GET /egress-proxies · 按 name 升序 · 绑定数与最近探测来自
            <code>device_egress</code> / <code>egress_checks</code>
          </p>
        </div>
        <span class="model-chip">{{ manager.egressProxyTotal }} 条</span>
      </div>

      <el-table :data="manager.egressProxies" empty-text="还没有代理定义，先新建一个">
        <el-table-column label="名称" min-width="190">
          <template #default="{ row }">
            <div class="log-title">{{ row.name }}</div>
            <div class="egress-tags">
              <span v-if="row.hasCredentials" class="tag accent">已设凭据</span>
              <span v-else class="tag">无凭据</span>
              <span
                v-if="row.relayActive === true"
                class="tag ok"
                title="绑定设备上的本地中继 listener 正在 accept"
                data-testid="proxy-relay-tag"
              >
                经设备中继
              </span>
              <span
                v-else-if="row.hasCredentials"
                class="tag warn"
                title="带凭据代理必须由设备内中继承载：中继没起来时该代理解析不到凭据"
              >
                需设备中继
              </span>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="类型" width="92">
          <template #default="{ row }">
            <el-tag size="small" :type="row.type === 'http' ? 'success' : 'warning'">
              {{ row.type }}
            </el-tag>
          </template>
        </el-table-column>

        <el-table-column label="地址" min-width="180">
          <template #default="{ row }">
            <span class="egress-addr">{{ row.host }}:{{ row.port }}</span>
          </template>
        </el-table-column>

        <el-table-column label="地区" width="96">
          <template #default="{ row }">
            <span class="text-muted">{{ row.region ?? '—' }}</span>
          </template>
        </el-table-column>

        <el-table-column label="标签" min-width="130">
          <template #default="{ row }">
            <div v-if="row.tags.length" class="tag-list">
              <span v-for="tag in row.tags" :key="tag" class="tag">{{ tag }}</span>
            </div>
            <span v-else class="text-muted">—</span>
          </template>
        </el-table-column>

        <el-table-column label="绑定" width="118">
          <template #default="{ row }">
            <div class="log-title" style="font-size: 11.5px">{{ row.boundDevices }} 台设备</div>
            <div class="log-detail">{{ row.boundApps }} 条绑定</div>
          </template>
        </el-table-column>

        <el-table-column label="最近探测" min-width="196">
          <template #default="{ row }">
            <template v-if="row.lastCheck">
              <div :class="['probe-ip', row.lastCheck.ok ? '' : 'is-bad']">
                {{ row.lastCheck.ok ? row.lastCheck.exitIp : (row.lastCheck.errorCode ?? '失败') }}
              </div>
              <div class="probe-meta">
                {{ row.lastCheck.latencyMs !== null ? row.lastCheck.latencyMs + ' ms' : '--' }} ·
                {{ row.lastCheck.source === 'device' ? '设备' : 'Manager' }} ·
                {{ relativeTime(row.lastCheck.checkedAt) }}
              </div>
            </template>
            <span v-else class="text-muted">未探测</span>
          </template>
        </el-table-column>

        <el-table-column label="设备中继" min-width="150">
          <template #default="{ row }">
            <span
              :class="relayActiveTagClass(row.relayActive)"
              :title="proxyRelayDetail(row as EgressProxy)"
              data-testid="proxy-relay-state"
            >
              {{ proxyRelayLabel(row as EgressProxy) }}
            </span>
            <div class="probe-meta">{{ proxyRelayDetail(row as EgressProxy) }}</div>
          </template>
        </el-table-column>

        <el-table-column label="启用" width="78">
          <template #default="{ row }">
            <el-switch
              :model-value="row.enabled"
              size="small"
              @change="(value: string | number | boolean) => toggleEnabled(row as EgressProxy, Boolean(value))"
            />
          </template>
        </el-table-column>

        <el-table-column label="操作" width="248" align="right">
          <template #default="{ row }">
            <div class="egress-actions">
              <el-tooltip :content="probeHint(row as EgressProxy)" placement="top">
                <el-button
                  link
                  type="primary"
                  :loading="manager.isProbingProxy(row.id)"
                  data-testid="proxy-probe"
                  @click="probe(row as EgressProxy)"
                >
                  探测
                </el-button>
              </el-tooltip>
              <el-button link type="primary" @click="openBind(row as EgressProxy)">绑定设备</el-button>
              <el-button link type="primary" @click="openEdit(row as EgressProxy)">编辑</el-button>
              <el-button link type="danger" @click="remove(row as EgressProxy)">删除</el-button>
            </div>
          </template>
        </el-table-column>
      </el-table>

      <div v-if="manager.egressProxyTotal > manager.egressProxies.length" class="hint" style="padding: 0 20px 14px">
        只显示前 {{ manager.egressProxies.length }} 个（共 {{ manager.egressProxyTotal }} 个）。用搜索缩小范围。
      </div>
    </section>

    <!-- 新建 / 编辑 -->
    <el-dialog
      v-model="formVisible"
      :title="editingId ? '编辑代理' : '新建代理'"
      width="560px"
      :close-on-click-modal="false"
      @closed="resetForm"
    >
      <p class="hint" style="margin-top: -4px; margin-bottom: 16px">
        代理名匹配 <code>^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$</code>，与设备端
        <code>egress.toml</code> 的表名 1:1。用户名与密码是<strong>只写</strong>字段：保存后任何接口都不回显。
      </p>

      <el-form label-position="top">
        <div class="egress-form-grid">
          <el-form-item label="名称 (name)">
            <el-input v-model="form.name" placeholder="us_la_01" />
          </el-form-item>
          <el-form-item label="类型 (type)">
            <el-select v-model="form.type" style="width: 100%">
              <el-option label="http（设备级 + 应用级）" value="http" />
              <el-option label="socks5（应用级；探测经 socks5h 真连）" value="socks5" />
            </el-select>
          </el-form-item>
          <el-form-item label="主机 (host)">
            <el-input v-model="form.host" placeholder="203.0.113.10（不含 http:// 与端口）" />
          </el-form-item>
          <el-form-item label="端口 (port)">
            <el-input-number v-model="form.port" :min="1" :max="65535" :controls="false" style="width: 100%" />
          </el-form-item>
          <el-form-item label="用户名（可选，只写不读）">
            <el-input v-model="form.username" autocomplete="off" placeholder="留空表示不修改" />
          </el-form-item>
          <el-form-item label="密码（可选，只写不读）">
            <el-input
              v-model="form.password"
              type="password"
              show-password
              autocomplete="new-password"
              placeholder="留空表示不修改"
            />
          </el-form-item>
          <el-form-item label="地区（可选）">
            <el-input v-model="form.region" placeholder="US-LA" />
          </el-form-item>
          <el-form-item label="标签（可选）">
            <el-select
              v-model="form.tags"
              multiple
              filterable
              allow-create
              default-first-option
              placeholder="us / mobile"
              style="width: 100%"
            />
          </el-form-item>
        </div>

        <div class="egress-inline-row">
          <el-switch v-model="form.enabled" />
          <span>启用（停用后不允许新建绑定，已有绑定仍会下发并附 <code>egress_proxy_disabled</code>）</span>
        </div>

        <div v-if="editingId" class="egress-inline-row">
          <el-switch v-model="form.clearCredentials" />
          <span>
            清除已保存的凭据（当前状态：{{ editingHasCredentials ? '已设置' : '未设置' }}；密码永不在界面上回显）
          </span>
        </div>

        <div v-if="formHasCredentials" class="inline-alert is-muted" data-testid="credential-relay-hint">
          <el-icon><InfoFilled /></el-icon>
          <span>
            带凭据代理需设备内中继，保存后重启目标应用生效：设备在本机
            <code>127.0.0.1:17800–17899</code> 起一个认证中继 listener，只有中继拿得到用户名 / 密码，
            应用配置里不会出现凭据。
          </span>
        </div>
      </el-form>

      <div v-if="formError" class="inline-alert is-error">
        <el-icon><WarningFilled /></el-icon>
        <span>{{ formError }}</span>
      </div>

      <template #footer>
        <div style="display: flex; align-items: center">
          <span class="hint" style="margin: 0">
            {{ editingId ? 'PATCH /egress-proxies/{id}' : 'POST /egress-proxies' }}
          </span>
          <span style="flex: 1"></span>
          <el-button @click="formVisible = false">取消</el-button>
          <el-button type="primary" :loading="egressSaving" @click="submitForm">
            {{ editingId ? '保存' : '创建' }}
          </el-button>
        </div>
      </template>
    </el-dialog>

    <!-- 批量绑定 + 下发 -->
    <el-dialog
      v-model="bindVisible"
      :title="bindProxy ? `把「${bindProxy.name}」下发给设备` : '批量绑定'"
      width="720px"
      :close-on-click-modal="false"
      @closed="resetBind"
    >
      <p class="hint" style="margin-top: -4px; margin-bottom: 14px">
        POST /devices/egress/apply（assignments 形态）：先落库再下发，返回 job kind=egress。
        应用留空 = 整机（方案 A，settings put global http_proxy）；填包名 = 应用级注入（方案 B，socks5 走这里）。
      </p>

      <div class="egress-form-grid">
        <el-form-item label="应用（留空 = 整机）">
          <el-select
            v-model="bindApp"
            filterable
            allow-create
            clearable
            default-first-option
            placeholder="留空 = 整机；或输入包名 com.example.app"
            style="width: 100%"
          >
            <el-option
              v-for="app in appOptions"
              :key="app.package"
              :label="`${app.name} · ${app.package}`"
              :value="app.package"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="下发后探测出口 IP">
          <div class="egress-inline-row" style="margin-top: 6px">
            <el-switch v-model="bindProbe" />
            <span class="text-muted" style="font-size: 11px">probe=true，失败只记 warning，不改结果</span>
          </div>
        </el-form-item>
      </div>

      <div class="egress-inline-row">
        <el-checkbox v-model="bindOnlyUnbound">只看未配代理的设备</el-checkbox>
        <el-button link type="primary" :disabled="!visibleBindTargets.length" @click="selectAllDevices">
          全选
        </el-button>
        <el-button link type="primary" :disabled="!bindDeviceIds.length" @click="bindDeviceIds = []">
          清空
        </el-button>
        <span style="flex: 1"></span>
        <span class="text-muted" style="font-size: 11px">
          已选 {{ bindDeviceIds.length }} 台 · 可绑定 {{ visibleBindTargets.length }} 台 ·
          已配代理 {{ boundDeviceCount }} 台
        </span>
      </div>

      <div class="target-list" style="max-height: 240px; overflow: auto">
        <label
          v-for="device in visibleBindTargets"
          :key="device.id"
          :class="['target-option', { 'is-selected': bindDeviceIds.includes(device.id) }]"
        >
          <el-checkbox
            :model-value="bindDeviceIds.includes(device.id)"
            @change="(value: unknown) => toggleBindDevice(device.id, Boolean(value))"
          />
          <span :class="['status-dot', device.online ? 'is-online' : 'is-offline']" style="width: 6px; height: 6px; flex-basis: 6px"></span>
          <div class="target-main">
            <strong>{{ device.name }}</strong>
            <span>
              {{ device.online ? device.model ?? '未识别' : '离线，下发会被跳过' }} ·
              {{ deviceEgressLabel(device.id) }}
            </span>
          </div>
        </label>

        <div v-if="bindEmptyReason" class="bind-empty" data-testid="bind-empty-state">
          <el-icon class="bind-empty-icon"><InfoFilled /></el-icon>
          <strong>{{ bindEmptyTitle }}</strong>
          <p>{{ bindEmptyHint }}</p>
          <div class="bind-empty-actions">
            <el-button
              v-if="bindEmptyReason === 'no-devices'"
              size="small"
              type="primary"
              plain
              @click="openDevicePage"
            >
              去添加设备
            </el-button>
            <el-button
              v-else-if="bindOnlyUnbound"
              size="small"
              type="primary"
              plain
              data-testid="bind-show-all"
              @click="showAllBindTargets"
            >
              显示全部设备
            </el-button>
            <el-button
              v-if="bindOnlyUnbound && bindEmptyReason !== 'no-devices'"
              size="small"
              link
              @click="showAllBindTargets"
            >
              清除筛选
            </el-button>
          </div>
        </div>
      </div>

      <p v-if="!bindDeviceIds.length" class="hint" style="margin: 8px 0 0">
        下发需要至少选择 1 台设备；没有候选时可先清除筛选或到「设备」页添加设备。
      </p>

      <div v-if="bindDetail?.bindings?.length" class="inline-alert is-muted" style="margin-top: 12px">
        <el-icon><InfoFilled /></el-icon>
        <span>
          当前已绑定 {{ bindDetail.bindings.length }} 条：
          {{ bindDetail.bindings.map((item) => `${item.deviceName} · ${item.app || '整机'}`).join('；') }}
        </span>
      </div>

      <div v-if="bindError" class="inline-alert is-error" style="margin-top: 12px">
        <el-icon><WarningFilled /></el-icon>
        <span>{{ bindError }}</span>
      </div>

      <template #footer>
        <div style="display: flex; align-items: center">
          <el-button :loading="egressSaving" :disabled="!bindDeviceIds.length" @click="runBind(true)">
            预演（dryRun）
          </el-button>
          <span style="flex: 1"></span>
          <el-button @click="bindVisible = false">关闭</el-button>
          <el-button
            type="primary"
            :loading="egressSaving"
            :disabled="!bindDeviceIds.length"
            data-testid="bind-submit"
            @click="runBind(false)"
          >
            下发
          </el-button>
        </div>
      </template>
    </el-dialog>

    <!-- 预演计划 -->
    <section v-if="bindPlan" class="endpoint-table" style="margin-top: 16px">
      <div class="panel-head">
        <div>
          <h2>预演计划</h2>
          <p>dryRun=true：只计算与校验，不落库、不下发，共 {{ bindPlan.length }} 项</p>
        </div>
        <span class="model-chip">dryRun</span>
      </div>
      <el-table :data="bindPlan" style="width: 100%">
        <el-table-column label="设备" min-width="150" prop="deviceId" />
        <el-table-column label="应用" min-width="190">
          <template #default="{ row }">{{ row.app || '整机（方案 A）' }}</template>
        </el-table-column>
        <el-table-column label="模式" width="100" prop="mode" />
        <el-table-column label="代理" min-width="140">
          <template #default="{ row }">{{ row.proxyName ?? '—' }}</template>
        </el-table-column>
      </el-table>
    </section>

    <!-- 下发结果 -->
    <section v-if="bindJob" class="endpoint-table" style="margin-top: 16px">
      <div class="panel-head">
        <div>
          <h2>下发结果</h2>
          <p>
            job <code>{{ bindJob.id }}</code> · {{ jobStatusLabel(bindJob.status) }} ·
            成功 {{ bindJob.succeeded }} / 跳过 {{ bindJob.skipped }} / 失败 {{ bindJob.failed }} ·
            共 {{ bindJob.total }} 项
          </p>
        </div>
        <span :class="['model-chip', bindJob.status === 'failed' || bindJob.status === 'partial' ? 'warn' : '']">
          {{ bindJob.kind }}
        </span>
      </div>
      <el-table :data="bindJob.results" style="width: 100%">
        <el-table-column label="设备" min-width="160">
          <template #default="{ row }">
            <div class="log-title">{{ row.deviceName }}</div>
            <div class="log-detail">{{ row.deviceId }}</div>
          </template>
        </el-table-column>
        <el-table-column label="应用" min-width="180">
          <template #default="{ row }">
            <code style="font-size: 10.5px; color: #3c366b">{{ appLabel(row.app) }}</code>
          </template>
        </el-table-column>
        <el-table-column label="结果" width="96">
          <template #default="{ row }">
            <el-tag size="small" :type="resultTagType(row.status)">
              {{ resultLabel(row.status) }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column label="结果码 / 说明" min-width="230">
          <template #default="{ row }">
            <span v-if="row.error" style="font-size: 11px; color: #b91c1c">
              {{ row.error.code }} · {{ row.error.message }}
            </span>
            <span v-else-if="row.code" style="font-size: 11px; color: #b45309">
              {{ codeLabel(row.code) }}
            </span>
            <span v-else style="font-size: 11px; color: #9aa0b2">—</span>
          </template>
        </el-table-column>
        <el-table-column label="尝试" width="76" align="right">
          <template #default="{ row }">
            <span style="font-size: 11px">{{ row.attempts }} 次</span>
          </template>
        </el-table-column>
        <el-table-column label="耗时" width="92" align="right">
          <template #default="{ row }">
            <span style="font-size: 11px; color: #9aa0b2">{{ row.durationMs }} ms</span>
          </template>
        </el-table-column>
      </el-table>
      <div class="hint" style="padding: 0 20px 14px">
        <code>egress_unchanged</code> 表示设备上的出口状态已与期望一致（幂等重放），该行结果仍是 ok，不会把 job 变成 partial。
      </div>
    </section>

    <div class="hint" style="margin-top: 18px">
      <el-icon style="vertical-align: -2px"><Connection /></el-icon>
      契约：<code>docs/plans/egress-contract.md</code> §6。egress revision 只从
      <code>X-DM-Egress-Revision</code> 或响应体的 <code>revision</code> 读取，绝不用 <code>X-DM-Revision</code> / <code>ETag</code>
      覆盖 config revision 缓存。
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import {
  Connection,
  InfoFilled,
  Loading,
  Plus,
  Refresh,
  Search,
  WarningFilled,
} from '@element-plus/icons-vue'
import { ApiError, describeError } from '../api/client'
import type {
  CreateEgressProxyRequest,
  DeviceSummary,
  EgressApplyPlanItem,
  EgressProxy,
  EgressProxyDetail,
  EgressProxyType,
  Job,
  JobResultStatus,
  UpdateEgressProxyRequest,
} from '../api/types'
import { useRouter } from 'vue-router'
import { PACKAGE_PATTERN, appDisplayName } from '../appCatalog'
import { useManagerStore } from '../stores/manager'
import { relativeTime } from '../utils/format'
import {
  relayActiveDetail,
  relayActiveLabel,
  relayActiveTagClass,
  relayLabel,
} from '../utils/relay'

const manager = useManagerStore()
const router = useRouter()

/** 代理名正则（契约 §2.5）；__DIRECT__ 是保留值，不能当代理定义名。 */
const PROXY_NAME_PATTERN = /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$/

const keyword = ref('')
const enabledFilter = ref<'all' | 'enabled' | 'disabled'>('all')

const formVisible = ref(false)
const editingId = ref('')
const editingHasCredentials = ref(false)
const formError = ref('')
interface EgressProxyFormState {
  name: string
  type: EgressProxyType
  host: string
  port: number
  username: string
  password: string
  region: string
  tags: string[]
  enabled: boolean
  clearCredentials: boolean
}

/** 编辑表单：凭据字段只写不读，永远不从服务端预填。 */
function blankForm(): EgressProxyFormState {
  return {
    name: '',
    type: 'http',
    host: '',
    port: 1080,
    username: '',
    password: '',
    region: '',
    tags: [],
    enabled: true,
    clearCredentials: false,
  }
}

const form = ref<EgressProxyFormState>(blankForm())

const bindVisible = ref(false)
const bindProxy = ref<EgressProxy | null>(null)
const bindDetail = ref<EgressProxyDetail | null>(null)
const bindDeviceIds = ref<string[]>([])
const bindApp = ref('')
const bindProbe = ref(true)
const bindOnlyUnbound = ref(true)
const bindError = ref('')
const bindPlan = ref<EgressApplyPlanItem[] | null>(null)
const bindJob = ref<Job | null>(null)

const socksCount = computed(
  () => manager.egressProxies.filter((proxy) => proxy.type === 'socks5').length
)

/** 当前列表里正**经设备内中继**生效的代理数（relayActive=true）。 */
const relayActiveCount = computed(
  () => manager.egressProxies.filter((proxy) => proxy.relayActive === true).length
)

/**
 * 这张表单保存后是不是「带凭据」的代理：
 * - 勾了「清除已保存的凭据」→ 不是；
 * - 用户名 / 密码任一填了 → 是（新建或覆盖）；
 * - 编辑已有凭据的代理但没动凭据 → 仍是。
 * 只有带凭据的代理才需要设备内中继，所以提示跟着这个判断走。
 */
const formHasCredentials = computed(() => {
  if (form.value.clearCredentials) return false
  if (form.value.username.trim() || form.value.password) return true
  return Boolean(editingId.value && editingHasCredentials.value)
})

// 模板里在「组件 slot 套组件 slot」（el-dialog > el-form-item > …）时直接读 store 属性，
// vue-tsc 会把它当 ref 再取一次 .value；这里给两个热点做本地投影，模板只用本地名。
const egressSaving = computed(() => manager.egressSaving)
const appOptions = computed(() => manager.appOptions)

const visibleBindTargets = computed<DeviceSummary[]>(() =>
  manager.devices.filter((device) => !bindOnlyUnbound.value || !manager.hasDeviceEgress(device))
)

/** 已配设备级代理的设备数（空态文案要说清「为什么一台都选不到」）。 */
const boundDeviceCount = computed(
  () => manager.devices.filter((device) => manager.hasDeviceEgress(device)).length
)

/**
 * 候选列表为空时的原因：
 * - no-devices：整个管理端还没有设备；
 * - all-bound：开了「只看未配代理」，但所有设备都已绑定；
 * - no-match：其它情况（理论上只剩「显示全部」也为空），保底文案。
 */
const bindEmptyReason = computed<'no-devices' | 'all-bound' | 'no-match' | null>(() => {
  if (visibleBindTargets.value.length > 0) return null
  if (manager.devices.length === 0) return 'no-devices'
  if (bindOnlyUnbound.value) return 'all-bound'
  return 'no-match'
})

const bindEmptyTitle = computed(() => {
  switch (bindEmptyReason.value) {
    case 'no-devices':
      return '还没有可绑定的设备'
    case 'all-bound':
      return '全部设备都已配置设备级代理'
    default:
      return '没有可绑定的设备'
  }
})

const bindEmptyHint = computed(() => {
  switch (bindEmptyReason.value) {
    case 'no-devices':
      return '先到「设备」页添加设备并完成首次同步，再回来下发代理。'
    case 'all-bound':
      return `当前 ${manager.deviceCount} 台设备都已有设备级代理（共 ${boundDeviceCount.value} 台）。点「显示全部设备」可以把它们也列出来，重新绑定会覆盖原有代理。`
    default:
      return '当前筛选条件下没有设备，试试显示全部设备。'
  }
})

// 候选列表变化（绑定状态刷新 / 切换筛选）后，清掉已不在列表里的勾选，
// 避免出现「可绑定 0 台、已选 3 台」这种自相矛盾的状态。
watch(visibleBindTargets, (targets) => {
  const allowed = new Set(targets.map((device) => device.id))
  const next = bindDeviceIds.value.filter((id) => allowed.has(id))
  if (next.length !== bindDeviceIds.value.length) {
    bindDeviceIds.value = next
  }
})

function showAllBindTargets(): void {
  bindOnlyUnbound.value = false
}

function openDevicePage(): void {
  void router.push({ path: '/devices', query: { add: '1' } })
}

function egressQuery(): { q?: string; enabled?: boolean } {
  const query: { q?: string; enabled?: boolean } = {}
  const q = keyword.value.trim()
  if (q) query.q = q
  if (enabledFilter.value === 'enabled') query.enabled = true
  if (enabledFilter.value === 'disabled') query.enabled = false
  return query
}

function reload(): void {
  void manager.loadEgressProxies(egressQuery())
}

function resetForm(): void {
  form.value.name = ''
  form.value.type = 'http'
  form.value.host = ''
  form.value.port = 1080
  form.value.username = ''
  form.value.password = ''
  form.value.region = ''
  form.value.tags = []
  form.value.enabled = true
  form.value.clearCredentials = false
  formError.value = ''
  editingId.value = ''
  editingHasCredentials.value = false
}

function openCreate(): void {
  resetForm()
  formVisible.value = true
}

function openEdit(row: EgressProxy): void {
  resetForm()
  editingId.value = row.id
  editingHasCredentials.value = row.hasCredentials
  form.value.name = row.name
  form.value.type = row.type
  form.value.host = row.host
  form.value.port = row.port
  form.value.region = row.region ?? ''
  form.value.tags = [...row.tags]
  form.value.enabled = row.enabled
  // username / password 永远不预填：服务端也不返回它们。
  formVisible.value = true
}

function validateForm(): string | null {
  const name = form.value.name.trim()
  if (!PROXY_NAME_PATTERN.test(name)) {
    return '代理名需匹配 ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$，且不能是保留值 __DIRECT__'
  }
  if (!form.value.host.trim()) return '主机不能为空（不要带 http:// 与端口）'
  if (!Number.isInteger(form.value.port) || form.value.port < 1 || form.value.port > 65535) {
    return '端口需在 1-65535 之间'
  }
  if (form.value.username.length > 128) return '用户名最多 128 字符'
  if (form.value.password.length > 256) return '密码最多 256 字符'
  if (form.value.region.trim().length > 64) return '地区最多 64 字符'
  return null
}

async function submitForm(): Promise<void> {
  formError.value = ''
  const invalid = validateForm()
  if (invalid) {
    formError.value = invalid
    return
  }
  try {
    if (editingId.value) {
      const patch: UpdateEgressProxyRequest = {
        name: form.value.name.trim(),
        type: form.value.type,
        host: form.value.host.trim(),
        port: form.value.port,
        region: form.value.region.trim() ? form.value.region.trim() : null,
        tags: form.value.tags,
        enabled: form.value.enabled,
      }
      if (form.value.clearCredentials) {
        // 双态：null = 清除；省略 = 保留已存密文。
        patch.username = null
        patch.password = null
      } else {
        if (form.value.username.trim()) patch.username = form.value.username.trim()
        if (form.value.password) patch.password = form.value.password
      }
      await manager.updateProxy(editingId.value, patch)
      ElMessage.success('代理已更新（凭据不会回显）')
    } else {
      const body: CreateEgressProxyRequest = {
        name: form.value.name.trim(),
        type: form.value.type,
        host: form.value.host.trim(),
        port: form.value.port,
        tags: form.value.tags,
        enabled: form.value.enabled,
      }
      if (form.value.region.trim()) body.region = form.value.region.trim()
      if (form.value.username.trim()) body.username = form.value.username.trim()
      if (form.value.password) body.password = form.value.password
      await manager.createProxy(body)
      ElMessage.success('代理已创建')
    }
    formVisible.value = false
  } catch (cause) {
    formError.value = describeError(cause, '保存代理失败')
  }
}

async function toggleEnabled(row: EgressProxy, enabled: boolean): Promise<void> {
  try {
    await manager.updateProxy(row.id, { enabled })
    ElMessage.success(enabled ? '代理已启用' : '代理已停用（已有绑定仍会下发）')
  } catch (cause) {
    await manager.reloadEgressProxiesQuiet()
    ElMessage.error(describeError(cause, '切换启用状态失败'))
  }
}

/** 六值 reason 的中文说明（设备端与 Manager 探测共用同一套 reason，契约 §4.4）。 */
const PROBE_REASON_LABELS: Record<string, string> = {
  connect: '连不上代理',
  timeout: '超时',
  dns: 'DNS 解析失败',
  tls: 'TLS 握手失败',
  http_status: '上游返回非 2xx',
  invalid_response: '响应不是 IP 字面量',
}

function probeReasonLabel(reason: string | null): string {
  if (!reason) return 'unknown'
  const label = PROBE_REASON_LABELS[reason]
  return label ? `${label}（${reason}）` : reason
}

/** 探测链路说明（按钮悬停）：http 与 socks5 的认证方式不同，socks5 现在也是真连。 */
function probeHint(row: EgressProxy): string {
  if (row.type === 'socks5') {
    return 'socks5：经 socks5h 真连（DNS 交给代理解析），凭据按 RFC 1929 协商；隧道内不发 Proxy-Authorization'
  }
  return 'http：经代理发 GET，凭据以 Proxy-Authorization 预置（不走 407 往返）'
}

async function probe(row: EgressProxy): Promise<void> {
  try {
    const result = await manager.probeProxy(row.id)
    const upstream = result.upstreamStatus ? ` · 上游 ${result.upstreamStatus}` : ''
    ElMessage.success(
      `${row.type} 出口 IP ${result.exitIp} · ${result.latencyMs} ms（${relativeTime(result.checkedAt)}${upstream}）`
    )
    // 探测成功但结果没落库这类提醒不能静默吞掉。
    const warning = result.warnings?.[0]?.message
    if (warning) ElMessage.warning(warning)
  } catch (cause) {
    if (cause instanceof ApiError && cause.code === 'egress_probe_failed') {
      const details = cause.details ?? null
      const reason = typeof details?.reason === 'string' ? details.reason : null
      ElMessage.error(`探测失败（${probeReasonLabel(reason)}）：${cause.message}`)
      return
    }
    ElMessage.error(describeError(cause, '探测失败'))
  }
}

async function remove(row: EgressProxy): Promise<void> {
  try {
    await ElMessageBox.confirm(`确定删除代理「${row.name}」？`, '删除代理', {
      confirmButtonText: '删除',
      cancelButtonText: '取消',
      type: 'warning',
    })
  } catch {
    return
  }
  try {
    await manager.deleteProxy(row.id)
    ElMessage.success('代理已删除')
  } catch (cause) {
    if (cause instanceof ApiError && cause.code === 'egress_proxy_in_use') {
      const details = cause.details ?? null
      const devices = Array.isArray(details?.devices) ? (details?.devices as string[]) : []
      const total = typeof details?.total === 'number' ? details.total : devices.length
      await ElMessageBox.alert(
        `仍有 ${total} 条绑定引用该代理：${devices.join('、') || '—'}。请先解绑（删除设备出口绑定或改绑其它代理）后再删除。`,
        '先解绑再删除',
        { confirmButtonText: '知道了' }
      )
      return
    }
    ElMessage.error(describeError(cause, '删除代理失败'))
  }
}

function resetBind(): void {
  bindProxy.value = null
  bindDetail.value = null
  bindDeviceIds.value = []
  bindApp.value = ''
  bindProbe.value = true
  bindOnlyUnbound.value = true
  bindError.value = ''
  bindPlan.value = null
  bindJob.value = null
}

async function openBind(row: EgressProxy): Promise<void> {
  resetBind()
  bindProxy.value = row
  bindDeviceIds.value = manager.devicesWithoutEgress.map((device) => device.id)
  bindVisible.value = true
  try {
    bindDetail.value = await manager.loadEgressProxyDetail(row.id)
  } catch {
    bindDetail.value = null
  }
}

function selectAllDevices(): void {
  bindDeviceIds.value = visibleBindTargets.value.map((device) => device.id)
}

function toggleBindDevice(id: string, checked: boolean): void {
  if (checked) {
    if (!bindDeviceIds.value.includes(id)) bindDeviceIds.value = [...bindDeviceIds.value, id]
  } else {
    bindDeviceIds.value = bindDeviceIds.value.filter((item) => item !== id)
  }
}

/**
 * 「设备中继」列的文案：relayActive 三态直接映射；
 * 但一条没有绑定的代理谈不上「设备未上报」—— 它压根没有设备可上报。
 */
function proxyRelayLabel(row: EgressProxy): string {
  const unbound = row.boundDevices === 0 && row.boundApps === 0
  if (unbound && (row.relayActive === undefined || row.relayActive === null)) return '未绑定设备'
  return relayActiveLabel(row.relayActive)
}

function proxyRelayDetail(row: EgressProxy): string {
  const unbound = row.boundDevices === 0 && row.boundApps === 0
  if (unbound && (row.relayActive === undefined || row.relayActive === null)) {
    return '绑定到设备后才会分配本地中继端口'
  }
  return relayActiveDetail(row.relayActive)
}

function deviceEgressLabel(deviceId: string): string {
  const summary = manager.deviceById(deviceId)?.egress
  if (!summary) return '未配代理'
  // 绑定候选里要能看出这台设备的中继是否在跑：带凭据代理只有中继起来才生效。
  const relay = summary.relay ? ` · ${relayLabel(summary.relay)}` : ''
  if (summary.mode === 'proxy' && summary.proxyName) {
    const exit = summary.exitIp ? ` · 出口 ${summary.exitIp}` : ''
    return `${summary.proxyName}${exit}${relay}`
  }
  if (summary.mode === 'direct') return `整机直连${relay}`
  return '未配代理'
}

async function runBind(dryRun: boolean): Promise<void> {
  bindError.value = ''
  const row = bindProxy.value
  if (!row) return
  if (!bindDeviceIds.value.length) {
    bindError.value = '请至少选择一台设备'
    return
  }
  const app = bindApp.value.trim()
  if (app && !PACKAGE_PATTERN.test(app)) {
    bindError.value = '应用包名不合法（形如 com.example.app）；整机请留空'
    return
  }
  try {
    const result = await manager.applyEgress({
      assignments: bindDeviceIds.value.map((deviceId) => ({ deviceId, app, proxyId: row.id })),
      probe: bindProbe.value,
      dryRun,
    })
    if (result.plan) {
      bindPlan.value = result.plan
      bindJob.value = null
      return
    }
    bindPlan.value = null
    const job = result.job
    bindJob.value = job
    if (!job) return
    if (job.status === 'succeeded') {
      ElMessage.success(`已下发 ${job.succeeded} 项`)
    } else if (job.status === 'failed') {
      ElMessage.error(`下发失败 ${job.failed} 项，详见逐设备结果`)
    } else {
      ElMessage.warning(`成功 ${job.succeeded} · 跳过 ${job.skipped} · 失败 ${job.failed}`)
    }
  } catch (cause) {
    bindError.value = describeError(cause, '批量绑定失败')
  }
}

function jobStatusLabel(status: Job['status']): string {
  return (
    {
      queued: '排队中',
      running: '执行中',
      succeeded: '全部成功',
      partial: '部分成功',
      failed: '全部失败',
    }[status] ?? status
  )
}

function resultLabel(status: JobResultStatus): string {
  return { ok: '成功', skipped: '跳过', failed: '失败' }[status] ?? status
}

function resultTagType(status: JobResultStatus): 'success' | 'info' | 'danger' {
  if (status === 'ok') return 'success'
  if (status === 'skipped') return 'info'
  return 'danger'
}

/** 结果码的中文说明（契约 §6.12 结果码映射）。 */
function codeLabel(code: string | null): string {
  if (!code) return '—'
  const label =
    {
      egress_unchanged: '已是最新，未重复下发',
      device_offline: '设备离线，已跳过',
      upstream_timeout: '请求超时，已跳过',
      egress_unsupported: '设备端没有 /egress（旧版本）',
      forbidden_scope: '设备拒绝（403）',
      config_revision_mismatch: 'egress revision 冲突（412）',
      validation_failed: '设备端校验失败（422）',
      upstream_unavailable: '设备 settings 不可用（503）',
      egress_not_configured: '设备上尚未配置出口代理',
      device_error: '设备返回异常',
      egress_proxy_disabled: '绑定了已停用的代理',
      egress_proxy_not_found: '代理定义已不存在',
    }[code] ?? code
  return code === label ? code : `${label}（${code}）`
}

function appLabel(app: string): string {
  return app ? `${appDisplayName(app)} · ${app}` : '整机（方案 A）'
}

onMounted(reload)
</script>
