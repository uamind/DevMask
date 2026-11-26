# 独立验收报告 · DevMask WebUI 改版（B 浅色默认 / A 深色）

> 验收人：ui-verifier（独立于 t1–t4/t6 的实现者）
> 日期：2026-11-26
> 任务：t5 构建验证 + 五页浅深截图 + 偏差清单
> 结论：**构建全绿、五页可用、10 张复核截图齐全、0 运行期异常**；偏差以「深色可读性」与「与 mockup 的结构取舍」两类为主，未发现阻塞发布的回归。逐条见 §5。

---

## 1. 构建结果（置顶）

命令：`python webui/build_webui.py`（工作区根目录）

| 阶段 | 结果 | 证据 |
| --- | --- | --- |
| npm ci 依赖一致性 | ✅ 通过 | `added 228 packages in 5s`；仅 `fsevents` install-scripts 提示（无害） |
| TypeScript 类型检查 `vue-tsc --noEmit` | ✅ 通过 | `✅ TypeScript 类型检查通过` |
| 代码检查 `eslint . --max-warnings 0` | ✅ 通过 | `✅ 代码检查通过`（无 warning 溢出） |
| 生产构建 `vite build` | ✅ 通过 | `✓ 3657 modules transformed` / `✓ built in 819ms` / 产物 898.0 KB |
| 退出码 | **0** | `EXIT:0` |

- 首次运行：11:04:08 完成（EXIT 0）。
- **captain 在 11:04:36 改了设置页标题（新增 `settings.appearance.title` / `settings.advanced.title`）后复跑**：11:05:40 全绿，EXIT 0；本报告与全部截图均为该改动之后的状态（截图里设置页分区标题已是「外观 / 高级」）。
- 完整日志：`/tmp/dm-build-t5b.log`（复跑）、`/tmp/dm-build-t5.log`（首跑）。
- 产物目录：`module/webroot`（`index.html` 3.56 kB + assets）。

复跑命令与退出码记录：

```
$ python webui/build_webui.py ; echo EXIT:$?
EXIT:0
```

---

## 2. 验收环境与方法

| 项 | 值 |
| --- | --- |
| 渲染 | `vite dev`（Vite 8.3.4）`http://127.0.0.1:5199/` |
| 数据源 | **内置 dev mock**（`webui/src/utils/mockData.ts`，本轮未改）：1 套模板 `redmagic_9_pro`、5 个应用、`module.prop version=1.0.0` |
| 传输层 | DEV 下 `detectRuntimeSync()` 返回 `ksu`（`webui/src/backend/index.ts:48-50`），即管理器模式，不需要后端服务 |
| 浏览器 | Chrome HeadlessChrome/158.0.8127.37，CDP 直连 |
| 视口 | 390 × 844 CSS px，deviceScaleFactor 2 → 截图 780 × 1688 |
| 主题 | `localStorage.devmask_settings.theme` = light / dark（`html.dark` 生效） |
| 语言 | zh（另有 en / tr 缺键扫描，见 §6.5） |
| 截图脚本 | `/tmp/dm-t5-shots.mjs`、`/tmp/dm-t5-interact3.mjs`、`/tmp/dm-t5-drawer.mjs`、`/tmp/dm-t5-fixture-tpl.mjs`（全部在 /tmp，**未在仓库留下脚手架**） |

> 为什么不用 dist：`vite build` 产物走同一条渲染路径，但 dev 模式能拿到 dev mock 与 HMR 后的最新源码；本轮所有截图都来自 dev server，未使用 preview。

---

## 3. 五页浅 / 深截图（10 张，均 780×1688）

| 页面 | 浅色 | 深色 |
| --- | --- | --- |
| 概览 | [final-overview-light.png](final-overview-light.png) | [final-overview-dark.png](final-overview-dark.png) |
| 应用 | [final-apps-light.png](final-apps-light.png) | [final-apps-dark.png](final-apps-dark.png) |
| 模板 | [final-templates-light.png](final-templates-light.png) | [final-templates-dark.png](final-templates-dark.png) |
| 网络 | [final-network-light.png](final-network-light.png) | [final-network-dark.png](final-network-dark.png) |
| 设置 | [final-settings-light.png](final-settings-light.png) | [final-settings-dark.png](final-settings-dark.png) |

附加证据（同样只写 review/）：

| 文件 | 内容 |
| --- | --- |
| [final-dialog-export-light.png](final-dialog-export-light.png) / [-dark](final-dialog-export-dark.png) | 模板页「导入/导出」弹窗（`TemplateTransferDialog`） |
| [final-dialog-delete-light.png](final-dialog-delete-light.png) / [-dark](final-dialog-delete-dark.png) | 删除确认框（`ElMessageBox`，深色跟随令牌） |
| [final-dialog-app-light.png](final-dialog-app-light.png) / [-dark](final-dialog-app-dark.png) | 应用配置弹窗（白卡 + 18 圆角） |
| [final-dialog-network-light.png](final-dialog-network-light.png) / [-dark](final-dialog-network-dark.png) | 网络页编辑弹窗 |
| [final-library-light.png](final-library-light.png) / [-dark](final-library-dark.png) | 在线模板库（真实拉到 Gitee 数据：911/911，失败 0） |
| [final-library-drawer-light.png](final-library-drawer-light.png) / [-dark](final-library-drawer-dark.png) | 在线库预览抽屉（spec 表 + 作者胶囊 + 导入） |
| [final-templates-fixture-light.png](final-templates-fixture-light.png) / [-dark](final-templates-fixture-dark.png) | **浏览器内注入 5 套模板**后的模板页（非 dev mock） |
| [final-templates-conflict-light.png](final-templates-conflict-light.png) / [-dark](final-templates-conflict-dark.png) | 同上，展开「包名重复配置」告警 |
| [final-settings-about-light.png](final-settings-about-light.png) | 设置页滚到底：服务卡 + 关于卡（版本/作者/仓库/关注/译者/许可） |
| [final-network-bottom-light.png](final-network-bottom-light.png) | 网络页滚到底：探测结果 + 认证中继 |

---

## 4. 令牌核对（自动取 computed style）

| 令牌 | README §5 目标 | 实测浅色 | 实测深色 | 结果 |
| --- | --- | --- | --- | --- |
| `--bg` | #F4F5F7 / #0B0E13 | `rgb(244,245,247)` | `rgb(11,14,19)` | ✅ |
| `--surface` | #FFFFFF / #141922 | `rgb(255,255,255)` | `rgb(20,25,34)` | ✅ |
| `--ink` | #111827 / #E8EEF7 | `rgb(17,24,39)` | `rgb(232,238,247)` | ✅ |
| `--muted` | #6B7280 / #8695AC | `rgb(107,114,128)` | `rgb(134,149,172)` | ✅ |
| `--line` | #E9EBF0 / rgba(255,255,255,.075) | `rgb(233,235,240)` | `rgba(255,255,255,0.075)` | ✅ |
| `--brand` | #2C5BFF / #4C8DFF | `rgb(44,91,255)` | `rgb(76,141,255)` | ✅ |
| `--ok` | #0E9F6E / #34D399 | `rgb(14,159,110)` | `rgb(52,211,153)` | ✅ |
| 卡圆角 | 18 / 16 | 18px | 16px | ✅ |
| 卡阴影 | 轻阴影 / 无 | `rgba(17,24,39,0.04) 0 1px 2px` | `none` | ✅ |
| 页面标题 | — | 27px / 730 | 24px / 620 | ✅（A 收小字号符合方向） |
| 分区标题 | — | 13.5px / 660 / 中文 | 11px / ui-monospace / UPPERCASE / #5C6B82 | ✅ 形态符合 A；对比度见 F3 |

导航：浅深两套均为 **5 项** —— 概览 / 模板 / 应用 / 网络 / 设置（DOM 实测 navCount: 5）；浅色活跃项为 brand-soft 胶囊，深色为顶部 2px 下划线 + 无底（`App.vue:409-431`），与 A 参考一致。

---


## 5. 逐页偏差清单

> 参考图口径：`shots/dir-b.png`（B-1 概览 / B-2 应用）与 `shots/dir-a.png`（A-1 / A-2）**各只有两屏**。模板 / 网络 / 设置三页没有对应 mockup，按 `README.md` §5 令牌表、§6 页面级改动、§7 文案规范核对，已在每条注明依据。
> 严重度：**high** = 影响可用性/可读性；**medium** = 明显偏离设计或出现两套并存的样式；**low** = 打磨项；**info** = 记录性说明。

### 5.1 概览（home）

| # | 现象 | 位置 | 建议修法 | 严重度 |
| --- | --- | --- | --- | --- |
| F5 | 缺「服务状态」区块。README §6 明确要求概览新增「服务状态」，A-1 也有 Zygisk 注入 / COW 属性引擎 / 应用级出口 / API 令牌 四行 + 「查看日志」。当前只有 hero → 2×2 指标 → 常用模板 → 最近改动 | `webui/src/pages/StatusPage.vue:15-131` | 在 `.tiles` 之后插一个 `.dm-card` 服务卡：数据取 `serverStore.status`（runtime/version/bind）+ `egressStore.snapshot`（模式/连通）+ `configStore.debugMode`（日志），4 行「图标 + 名称 + 副标题 + 右侧状态色」；右侧「查看日志」复用设置页的复制入口 | **low**（叙事缺口，非功能缺失） |
| F6 | 「最近改动」没有「全部记录 →」入口，B-1 有 | `StatusPage.vue:117-120` | `.recent__head` 右侧加 `dm-link`；暂无可跳转目标时先按 `v-if="recentItems.length"` 控制显隐，避免假入口 | low |
| F7 | 「常用模板」网格 `minmax(140px,1fr)`：390px 宽只能 2 列，1 套时左对齐留白；B-1 是 3 张卡横排 | `StatusPage.vue:490-495` | 改 `minmax(104px,1fr)` 可容 3 列；或改横向 `overflow-x:auto` 的卡片条（更贴近 mockup 的「一排看三张」） | low |
| F10 | 深色 hero 仍有两处 radial-gradient 光斑（A 方向主张「无渐变」） | `StatusPage.vue:307-329`；token `--hero-glow-a/b` `style.css:129-130` | 深色下把 `--hero-glow-a/b` 设为 `transparent`（或 alpha 降到 0.06），保留 hairline 与 hero 底色即可 | low |
| F11 | 指标文案「上次写入」与 B-1 的「上次应用」不一致；深色下 hero 数字用浅蓝 `--hero-accent` 而 A-1 用白色 | `webui/src/utils/i18n.ts:60`（zh）/ `:802`（en）/ `:1564`（tr）；`style.css:128` | 文案二选一并统一三语；hero 数字深色改 `var(--hero-ink)` | info |

### 5.2 应用（apps）

| # | 现象 | 位置 | 建议修法 | 严重度 |
| --- | --- | --- | --- | --- |
| F8 | 副标题复用 `overview.subtitle_fallback`（「按应用伪造机型；未配置的应用读取的仍是真机信息。」），B-2 原文是「为单个应用指定身份，未配置的应用不受影响。」 | `webui/src/pages/AppsPage.vue:24` | i18n 增 `apps.subtitle` 三语（需写权限的同学补 key），页面改一行 | low |
| F9 | 缺 B-2 列表底部的常驻说明（「右侧标签即该应用读到的机型 / 点击应用可单独修改，或套用模板」）。现在只有 clone 计数 >0 时才出现的「分身与继承」卡 | `AppsPage.vue`（列表底部，现仅 `v-if` 说明卡）；对照 [apps-inherit.png](apps-inherit.png) 与 B-2 | 在列表后加一行 11px `--muted` 居中说明；分身卡保持条件渲染 | low |
| F12 | 三段筛选是**胶囊**（`seg__item--active` = brand-soft 底 + brand 字），B-2 是**下划线 tab** | `webui/src/components/apps/AppFilters.vue:36-50` | 若以 mockup 为准：活跃项改 `border-bottom:2px solid var(--brand)` + 透明底；若保留胶囊（触控更大更好点），请在 README/mockup 更新为胶囊，避免下轮又对不上 | low（设计取舍） |
| F12b | 「未配置」虚线标签文字过淡：浅色 #9AA3B2 on #FFF = **2.54:1**，深色 #5C6B82 on #141922 = **3.57:1** | `AppList.vue:59`（`app-row__tag--empty`） | 文字提到 `--muted`（浅 4.43 / 深 5.8），虚线边保留 `--line-strong` | low |
| — | 行卡 16px（深 14px）圆角、38px 首字方块、等宽包名、右侧机型标签、chevron 全部与 B-2 一致 | `AppList.vue:25-63` | — | ✅ |

### 5.3 模板（templates）

| # | 现象 | 位置 | 建议修法 | 严重度 |
| --- | --- | --- | --- | --- |
| F13 | 展开「包名重复配置」后，「忽略：xxx」行贴着卡片左内边距、并压到卡片底部（浅/深都可见），与上方 `com.mobilelegends.mi` 胶囊的左起点不一致 | `webui/src/pages/TemplatePage.vue:46-63`（`.conflict__list li` / `.conflict__ign`） | `.conflict__body` 内统一 `padding-left`；`li` 用 `display:flex; flex-wrap:wrap; gap:6px 10px; align-items:center`，「忽略」行改为独立第二行；`.conflict__body` 加 `padding-bottom:12px` | low |
| — | 卡片只留「机型名 + 型号代码 + 套用应用数」，0 应用时灰色；导出/删除保留次级图标；页头计数 pill、空状态、搜索无结果不给创建入口 —— 与 t3 声明一致 | `TemplateCard.vue:12-45`、`TemplateHeader.vue:9-52` | — | ✅ |
| — | 深色下冲突条 = surface + hairline + 琥珀左边条（不再整块琥珀底），实测符合 t3 说明 | `TemplatePage.vue:296` | — | ✅ |

> 说明：`final-templates-*-{light,dark}.png` 用的是 **dev mock 的 1 套模板**；5 套与冲突态是**浏览器内注入的夹具**（`final-templates-fixture-*` / `final-templates-conflict-*`），未写入仓库。t3 交付的 `templates-light/dark.png` 同样使用注入夹具。

### 5.4 网络（network）

| # | 现象 | 位置 | 建议修法 | 严重度 |
| --- | --- | --- | --- | --- |
| **F1** | **深色禁用主按钮几乎不可读**：管理器模式下 `probeSupported=false`，「探测出口 IP」为 `el-button--primary.is-disabled` → 底 `#A3C2FF` + 文字 `rgba(255,255,255,.5)` = **1.35:1**（浅色同项 `#96ADFF` + 白 = **2.16:1**） | 触发点 `webui/src/components/network/NetworkProbeCard.vue:24-33`；根因 `webui/src/style.css:588`（dark `--el-color-primary-light-5: #a3c2ff`）+ `style.css:544`（light `#96adff`），Element Plus 禁用态即用 light-5 底色 + 50% 白字 | 在 `style.css` 加禁用态覆盖（两主题都管），见下方代码块 | **high** |
| F2 | 深色 `.dm-pill--muted` 与页面底色几乎同色（详见 §6.1） | `style.css:283-286`、`:301-304` | 见 §6.1 方案 | medium |
| F14 | 深色 **Element Plus 主按钮**用白字（`#FFF on #4C8DFF` = **3.2:1**），而自定义主按钮已改深字（`.tpl-new` `#06101F on #4C8DFF` = **5.96:1**）—— 同一屏两套色彩语言 | `NetworkFormDialog.vue` footer `el-button type="primary"`；对照 `TemplateHeader.vue` `.tpl-new` | 二选一：① dark 下 `.el-button--primary{color:#06101f}` 与自定义按钮统一；② 自定义按钮回到白字以对齐 A mockup。推荐 ① | low |
| — | 页头常驻「一键直连」、4 张卡（当前状态/代理定义/探测结果/本机中继）、字段与 `settings.egress.*` 对齐、深色分区标题等宽大写 —— 与 t6 声明一致 | `NetworkPage.vue`、`components/network/*` | — | ✅ |

F1 建议补丁（放进 `webui/src/style.css` 的 Element Plus 覆盖段末尾）：

```css
/* 禁用态主按钮：浅色用中性底，深色用 brand-soft 底 + muted 字，避免 light-5 亮蓝配半透明白字 */
.el-button--primary.is-disabled,
.el-button--primary.is-disabled:hover,
.el-button--primary.is-disabled:focus {
  background: var(--surface-3) !important;
  border-color: var(--line) !important;
  color: var(--muted) !important;
}

html.dark .el-button--primary.is-disabled,
html.dark .el-button--primary.is-disabled:hover {
  background: rgba(76, 141, 255, 0.13) !important;
  border-color: rgba(76, 141, 255, 0.28) !important;
  color: var(--muted) !important;
}
```

### 5.5 设置（settings）

| # | 现象 | 位置 | 建议修法 | 严重度 |
| --- | --- | --- | --- | --- |
| F3 | 深色分区标题（外观 / 高级 / 关于 / 当前状态 / 代理定义 / 探测结果）`--dim #5C6B82` on `#0B0E13` = **3.57:1** @11px，低于 AA 4.5；A-1 的「服务状态」明显更亮 | `style.css:236-243`（`html.dark .dm-section__title`）；token `:108` | 改 `color: var(--muted)`（#8695AC → 6.36:1）或折中 #7D8CA3（5.66:1）；字号保持 11px 等宽大写 | medium |
| — | 三组标题已是「外观 / 高级 / 关于」（captain 11:04 的改动已生效，三语齐全：外观·高级 / Appearance·Advanced / Görünüm·Gelişmiş）；主题/语言选择器、网络出口入口卡、模块设置（调试开关 + 日志路径 + 复制）、工具、外部访问、服务卡、关于卡（版本/作者/仓库/关注/译者/许可 + SPDX 脚注）全部就位 | `SettingsPage.vue:17`、`:75`；`utils/i18n.ts:455-460 / 1202-1207 / 1971-1976` | — | ✅ |
| F15 | 关于卡「版本」显示 `1.0.0`（dev mock 的 module.prop 值），与本版文案口径 `v0.0.1` 不同屏可见（顶栏 pill 也是 1.0.0） | `SettingsPage.vue:509-511`；`usePageNavigation.ts:162-165` | 代码口径正确（见 §6.2），**不改代码**；建议在 README §7 补一句「顶栏/关于的版本取 module.prop，回落到 APP_VERSION」，并同步 mockup 示意 | info |

### 5.6 横向检查

| # | 现象 | 位置 | 建议修法 | 严重度 |
| --- | --- | --- | --- | --- |
| F16 | 浅色「运行中」pill `#0E9F6E on #E7F7F0` = **3.06:1**（11px）。与 B mockup 一致，但小字仍偏低 | `style.css:273-276` | 文字加深到 `#0B7D57`（≈4.6:1），底色不动 | info |
| F17 | 同一 `.dm-pill--muted` 两处底色不同：模板页计数 pill 用 `--surface`（深 #141922），网络页状态 pill 用 `--surface-3`（深 #171D27）；浅色同样白 / #F1F3F6 两种 | `TemplateHeader.vue:12` 的 `tpl-head__count` 覆盖 vs `style.css:283` | 明确一处语义：基类统一 `--surface-3`，模板页那张要么删覆盖、要么改名为 `.tpl-head__count` 专用样式并注释 | info |
| F18 | 唯一持续 console 报错来自 `index.html` 引的外部样式表 `https://mui.kernelsu.org/internal/insets.css`（`net::ERR_CONNECTION_CLOSED`），非本轮引入，设备端可用 | `webui/index.html:11` | 无需处理；若要在无网环境做干净回归，可给 dev 加本地兜底样式 | info |

---


## 6. 额外检查项（任务要求 4/5/6）

### 6.1 深色 `.dm-pill--muted` 与页面底色对比（实测）

先纠正任务描述里的一个前提：**设置页与应用页当前并没有使用 `.dm-pill--muted`**（实测可见数：overview 0 / apps 0 / templates 1 / network 1 / settings 0）。它在应用里的落点是：

| 位置 | 元素 | 深色实测：底 / 字 / 边 | 底 vs 页面底 | 字 vs 底 |
| --- | --- | --- | --- | --- |
| 网络页「当前状态」 | `.dm-pill--muted`（直连） | `#171D27` / `#8695AC` / 1px `rgba(255,255,255,.075)` | **1.14:1** | 5.56:1 |
| 模板页页头 | `.dm-pill--muted.tpl-head__count`（1 套） | `#141922` / `#8695AC` / 1px `rgba(255,255,255,.075)` | **1.10:1** | 5.80:1 |
| 顶栏运行态 | `.dm-pill--ok`（运行中） | `rgba(52,211,153,.1)` / `#34D399` / 1px `rgba(52,211,153,.22)` | 1.17:1 | 8.62:1 |
| 顶栏「检查中」态 | `.dm-pill--muted`（仅 configReady 前闪现） | 同上 muted | 1.14:1 | 5.80:1 |

**结论**：文字对比度没问题（5.5+），**问题在底色与页面底只差 1.10–1.14:1**，肉眼几乎只剩一圈 7.5% 白边。浅色下同一 pill 是白底（1.09:1）但落在 #F4F5F7 上仍清晰，所以这是**深色专属**问题；t3 在模板页做了局部强化（`TemplatePage.vue:296` 的 `.conflict` 走 surface+hairline），**但全局基类没跟上**，于是同一组件在页面间出现不同观感。

建议修法（一处改，三页受益，写进 `webui/src/style.css`）：

```css
/* 深色：muted 胶囊从「几乎同色」抬到可辨识的一层 */
html.dark .dm-pill--muted {
  background: var(--surface-2);          /* #1B2230 → 与 #0B0E13 = 1.21:1 */
  border: 1px solid var(--line-strong);  /* rgba(255,255,255,.12) */
  color: var(--muted);                   /* 5.80:1，不动 */
}

/* 若仍嫌 1.21 偏弱，可用「描边胶囊」更贴 A 的 hairline 语言 */
html.dark .dm-pill--muted.dm-pill--outline {
  background: transparent;
  border: 1px solid var(--line-strong);
  color: var(--muted);
}
```

参考实测：`--surface-2 #1B2230 → 1.21:1`、`#212A3A → 1.34:1`、透明底 + 12% 白边在 OLED 上最稳。

### 6.2 版本口径（不得回到硬编码）

- 常量只有一处：`webui/src/composables/usePageNavigation.ts:26` → `export const APP_VERSION = 'v0.0.1'`。
- 三处展示全部是「module.prop 优先、常量兜底」：
  - 顶栏：`usePageNavigation.ts:162-165`（`configStore.moduleMetaReady && configStore.moduleVersion ? moduleVersion : APP_VERSION`）
  - 概览指标：`StatusPage.vue:167-172`
  - 关于·版本：`SettingsPage.vue:509-511`
- 全仓扫描 `v0.0.1` / `1.0.0`（排除 `utils/mockData.ts`）只剩常量定义与两处注释 —— **没有任何页面写死版本号**。
- 截图里显示 `1.0.0`，来源是 dev mock 的 `mockModuleProp version=1.0.0`（`mockData.ts:34`），属夹具值；设备上显示 module.prop 的真实版本（仓库 `module/module.prop` 已是 0.0.1）。**结论：口径正确，不算回归**；要改只改 dev mock（t1 明确要求不动它）。

### 6.3 交互回归（导出 / 删除 / 在线库抽屉 / 删除确认 / 各弹窗）

浅色与深色各 13 个步骤全部通过；**0 个 JS 异常**，console 里只有外部样式表的 `ERR_CONNECTION_CLOSED`。

| 步骤 | 断言 | 浅色 | 深色 |
| --- | --- | --- | --- |
| 模板卡「导出」图标 | 点击后无异常（实现是复制到剪贴板 + toast，不是弹窗） | ✅ | ✅ |
| 「导入/导出」弹窗 | `.template-transfer-dialog` 可见、圆角/底色跟随令牌 | ✅ | ✅ |
| 关闭（ESC） | 弹窗消失 | ✅ | ✅ |
| 删除确认 | `.el-message-box` 可见，标题「确认删除」+ 模板名 | ✅ | ✅ |
| 取消删除 | 确认框关闭 | ✅ | ✅ |
| 在线模板库 | 打开、911/911 详情加载完成、失败 0 项 | ✅ | ✅ |
| 预览抽屉 | 点行 → `.sheet` 打开，标题 = 条目名，spec 表 + 作者胶囊 + 导入按钮 | ✅ | ✅ |
| 抽屉 ESC / scrim 关闭 | 抽屉消失，列表仍在 | ✅ | ✅ |
| 返回列表 | `.online-library-view` 消失 | ✅ | ✅ |
| 应用配置弹窗 | 打开/关闭 | ✅ | ✅ |
| 网络表单弹窗 | 打开/关闭 | ✅ | ✅ |

关于「在线库抽屉」：第一轮我等待不足（911 条详情要几秒），当时虚拟列表还没出第一行；单独复跑并轮询到 `.row` 出现后，抽屉的打开/关闭/返回全部通过，截图见 `final-library-drawer-{light,dark}.png`。抽屉里的 `fingerprint` 只在源数据带该字段时出现（Apple Watch 条目没有）。

### 6.4 console 报错明细

| 来源 | 文案 | 次数 | 归因 |
| --- | --- | --- | --- |
| `https://mui.kernelsu.org/internal/insets.css`（`webui/index.html:11`） | `net::ERR_CONNECTION_CLOSED` | 每次整页加载 2–3 条 | 外部 CDN 样式，沙箱无出网直连；设备端由 KernelSU 提供，非本轮改动 |
| 其余 | — | **0** | 无 Vue 警告、无未捕获异常、无 404（模板库真实拉到了 Gitee 的 templates.toml） |

### 6.5 i18n 缺键扫描（zh / en / tr × 5 页）

- 扫描方式：遍历可见文本节点，匹配 `^(settings|templates|apps|common|network|overview|about|shell|status)\\.[a-z0-9_.]+$`（排除包名 `com.*`、`egress.toml`）。
- 结果：**zh 0 / en 0 / tr 0**；en 的 5 页正文（Overview / Apps / Templates / Network / Settings + About）与 tr（Görünüm / Gelişmiş / Hakkında / SUNUCU）都完整；tr 长标签「Sistemi Takip Et」在选择器内不截断。
- captain 新加的两把 key 三语齐全：`settings.appearance.title`（外观 / Appearance / Görünüm）、`settings.advanced.title`（高级 / Advanced / Gelişmiş），行号 `utils/i18n.ts:455-460 / 1202-1207 / 1971-1976`。

### 6.6 证据核对

- `apps-inherit.png` **存在**（780×1608，10:51）：可见 clone 行「继承自主应用 + Pixel 9 Pro」与底部「分身与继承」卡（卡片被视口裁切，只露标题行）。这是 captain 收集的 t2 补证，非本次新拍。
- `templates-light/dark.png` 用的是**浏览器注入的 5 套夹具**（dev mock 只有 1 套）；本报告的 `final-templates-*.png` 用 dev mock 的 1 套，另有 `final-templates-fixture-*` / `final-templates-conflict-*` 作为 5 套 + 冲突态证据，均未写入仓库。
- 本轮只在 `docs/design/ui-redesign/review/**` 下新增图片与本报告，**未改任何页面代码、未改 i18n、未 commit**。

---

## 7. 建议修复顺序

1. **F1（high）** 深色/浅色禁用主按钮不可读 —— 一条 `.is-disabled` 覆盖即可，收益最大。
2. **F2（medium）** 深色 `.dm-pill--muted` 抬一层（`--surface-2` + `--line-strong`），一处改多页受益。
3. **F3（medium）** 深色分区标题从 `--dim` 换 `--muted`。
4. **F14 / F17 / F16（low）** 深色主按钮色彩语言统一、muted pill 底色语义统一、运行态 pill 文字加深。
5. **F5 / F6 / F9 / F8 / F12 / F13 / F7 / F10（low）** 概览补服务状态与「全部记录」、应用页补常驻说明与专属副标题、筛选形态定稿、冲突条展开排版、常用模板列数、深色 hero 去光斑。
6. **F11 / F15 / F18（info）** README 补版本口径说明、同步 mockup 文案、外部样式表沿用。

---

## 8. 附：复核用的可复现步骤

```bash
# 1) 构建
python webui/build_webui.py          # 期望 EXIT:0

# 2) 起 dev server
cd webui && npm run dev -- --port 5199 --strictPort

# 3) 起 headless Chrome（CDP 9444）
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --remote-debugging-port=9444 \
  --user-data-dir=/tmp/dm-t5-chrome --window-size=390,844 --hide-scrollbars

# 4) 抓图（脚本在 /tmp，未入库）
node /tmp/dm-t5-shots.mjs light docs/design/ui-redesign/review 9444
node /tmp/dm-t5-shots.mjs dark  docs/design/ui-redesign/review 9444

# 5) 交互回归（导出/删除/在线库/抽屉/弹窗）
node /tmp/dm-t5-interact3.mjs light docs/design/ui-redesign/review
node /tmp/dm-t5-interact3.mjs dark  docs/design/ui-redesign/review
node /tmp/dm-t5-drawer.mjs light docs/design/ui-redesign/review

# 6) 5 套模板 + 冲突态（浏览器内注入夹具）
node /tmp/dm-t5-fixture-tpl.mjs light docs/design/ui-redesign/review
node /tmp/dm-t5-fixture-tpl.mjs dark  docs/design/ui-redesign/review
```
