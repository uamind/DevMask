# 复验报告 · F1 / F2+F17 / F3 等修复（t8）

> 复验人：ui-verifier（与修复者独立）
> 日期：2026-11-26 11:36
> 范围：t5 报告里的 F1 / F2 / F3 / F12b / F8 / F6 / F10 / F14 / F16 共 9 项
> 方法：vite dev（dev mock）+ headless Chrome CDP，390×844@2x，浅/深两主题**实测 computed style 与对比度**，不只看代码

## 0. 结论

| 项 | 内容 | 判定 |
| --- | --- | --- |
| F1 | 禁用主按钮（探测出口 IP）+ 深色主按钮深字 | ✅ **PASS**（深 1.35 → **5.46**，浅 2.16 → **4.35**） |
| F1b | 删除确认框红色主按钮未被误伤 | ✅ **PASS**（仍白字红底；深色红底白字 2.77:1 是既有问题，见 R3） |
| F2 | 深色 muted 胶囊 ≥1.2:1 | ✅ **PASS**（两处均 **1.21:1**，字 5.24:1） |
| F17 | 两处 muted 胶囊底色一致 | ✅ **PASS**（均为 surface-2 + 12% 白边） |
| F2-light | 浅色 muted 胶囊 | ⚠️ **PARTIAL / 新回退 R1**（统一成 1.02:1 且无边，模板页从"白底+边"退化） |
| F3 | 深色分区标题对比度 | ✅ **PASS**（3.57 → **6.36**，设置 3 组 + 网络 3 组全测） |
| F12b | 未配置标签字色 | ✅ **PASS**（浅 2.54 → **4.43**，深 3.57 → **6.36**） |
| F8 | apps.subtitle 三语 | ✅ **PASS**（zh/en/tr 实测文案正确、无裸 key） |
| F6 | 常用模板 3 列 | ✅ **PASS**（3 列 × 110px）／ ⚠️ 副作用见 **R2** |
| F10 | 深色 hero 去光斑 | ✅ **PASS**（深色 ::before/::after 已是全透明渐变） |
| F14 | 深色主按钮深字 | ✅ **PASS**（#06101F on #4C8DFF = **5.96:1**） |
| F16 | 浅色「运行中」胶囊深字 | ❌ **FAIL**（仍 #0E9F6E on #E7F7F0 = **3.06:1**，未改） |
| 构建 | python webui/build_webui.py | ✅ **PASS**（EXIT 0，11:29:19–11:29:24） |

**一句话**：深色这批改到位了（F1/F2/F3/F14 全部达标，F10 生效），但 **F16 没改到**，且 **F2 的统一把浅色胶囊一起压平了（R1）**、**F6 把模板卡压到 110px 导致机型名断词（R2）**。建议再补一轮小改（见 §6），不影响已有深色收益。

---

## 1. F1 禁用主按钮 + 删除确认（任务 1）

改动位置：`webui/src/style.css:1008-1021`（两主题禁用态覆盖）、`:1024-1026`（深色主按钮深字）。

实测（网络页「探测结果」卡里的 disabled primary，即「探测出口 IP」）：

| 主题 | 背景 | 文字 | 边框 | 文字/背景对比 | 改前 |
| --- | --- | --- | --- | --- | --- |
| 深色 | `rgba(76,141,255,.13)` | `#8695AC` | 1px `rgba(76,141,255,.28)` | **5.46:1** | 1.35:1（#A3C2FF + 50% 白字） |
| 浅色 | `#F1F3F6`（--surface-3） | `#6B7280` | 1px `#E9EBF0` | **4.35:1** | 2.16:1（#96ADFF + 白字） |

- 深色达到 AA（≥4.5），浅色 4.35 略低于 4.5 —— 但**禁用态不受 WCAG 对比度约束**，且从"几乎看不见"变成"一眼能读出是禁用"，判 PASS。
- 截图：[recheck-network-dark.png](recheck-network-dark.png)（禁用胶囊 = 淡蓝底 + 灰字，不再是亮蓝底白字）。

**删除确认框红色主按钮（是否被误伤）**：

| 主题 | 删除按钮背景 | 文字 | 对比 | 结论 |
| --- | --- | --- | --- | --- |
| 浅色 | `#DC2626`（--danger） | `#FFFFFF` | **4.83:1** | ✅ 未被 F1 覆盖规则吃掉 |
| 深色 | `#F87171`（--danger 深色值） | `#FFFFFF` | **2.77:1** | ✅ 仍是白字红底（新规则带 `!important` 的白字优先），**未被误伤** |

- 验证要点：新增的 `html.dark .el-button--primary:not(.is-disabled){color:#06101f}` 没有带 `!important`，而 `.el-message-box__btns .el-button--primary`（style.css:880-885）的 `color:#fff !important` 优先级更高，所以确认框没有被染成深字。实测也如此。
- 截图：[recheck-delete-confirm-dark.png](recheck-delete-confirm-dark.png)、[recheck-delete-confirm-light.png](recheck-delete-confirm-light.png)。
- 深色红底白字只有 2.77:1，属**既有问题**（--danger 深色值一直是 #F87171），不是这批引入；见 R3。

---

## 2. F2 / F17 深色 muted 胶囊（任务 2）

改动位置：`webui/src/style.css:301-304`（`background: var(--surface-2)` + `border: 1px solid var(--line-strong)`）；`webui/src/components/templates/TemplateHeader.vue:107-111`（`.tpl-head__count` 只留 `color`，删掉 background/border 覆盖）。

深色实测（两处逐一取 computed style）：

| 位置 | 背景 | 边框 | 文字/背景 | 底 vs 页面底 |
| --- | --- | --- | --- | --- |
| 网络页「当前状态」的「直连」 | `#1B2230` | 1px `rgba(255,255,255,.12)` | 5.24:1 | **1.21:1** |
| 模板页页头「1 套」 | `#1B2230` | 1px `rgba(255,255,255,.12)` | 5.24:1 | **1.21:1** |

- **底色完全一致** ✅；**≥1.2:1** ✅（1.21，改进前分别是 1.14 / 1.10）。
- 补充口径：胶囊实际坐在**卡片底** `#141922` 上时是 **1.11:1**（改进前 1.04），形状主要靠 12% 白边立起来 —— 数值达标，但视觉上是"细描边胶囊"，可接受。
- 截图：[recheck-network-dark.png](recheck-network-dark.png)（直连胶囊有可见描边）、[recheck-delete-confirm-dark.png](recheck-delete-confirm-dark.png)（背景里的「1 套」胶囊同样有描边）。

### ⚠️ R1（新回退，medium）：浅色胶囊被一起"压平"

`TemplateHeader.vue` 删掉覆盖后，浅色下两处胶囊都用基类 `.dm-pill--muted`（`style.css:283-286`），而基类**浅色没有边框**、底色是 `--surface-3 #F1F3F6`：

| 位置 | 改前（浅色） | 改后（浅色） | 与页面底 #F4F5F7 |
| --- | --- | --- | --- |
| 模板页「1 套」 | 白底 #FFF + 1px #E9EBF0 边（1.09:1，清晰） | `#F1F3F6` + **0px 边** | **1.02:1**，胶囊形状消失，只剩文字 |
| 网络页「直连」 | #F1F3F6 无边框（本来就弱） | 同上 | 1.02:1 |

- 截图证据：[recheck-templates-light.png](recheck-templates-light.png)（「1 adet / 1 套」只剩文字，没有胶囊轮廓）。
- 建议（一处改，两处同时修好）：

```css
/* 浅色 muted 胶囊：补一层白底 + hairline，与深色的「surface-2 + line-strong」对称 */
:root:not(.dark) .dm-pill--muted {
  background: var(--surface);
  border: 1px solid var(--line);
}
```

  或保留基类为「无底色 + hairline」：`background: transparent; border: 1px solid var(--line)`（浅色下 1.11:1 描边，深色已有 12% 边）。二者都能让浅色恢复「看得出是胶囊」。

---

## 3. F3 深色分区标题（任务 3）

改动位置：`webui/src/style.css:236-242` → `color: var(--muted)`（原 `--dim`）。

| 页面 | 标题 | 实测颜色 | 对比度（vs #0B0E13） | 改前 |
| --- | --- | --- | --- | --- |
| 设置 | 外观 / 高级 / 关于 | `#8695AC` | **6.36:1** ×3 | 3.57:1 |
| 网络 | 当前状态 / 代理定义 / 探测结果 | `#8695AC` | **6.36:1** ×3 | 3.57:1 |

- 字号/字族仍是 11px `ui-monospace` + UPPERCASE + 0.1em（A 方向形态不变）✅
- 截图：[recheck-settings-dark.png](recheck-settings-dark.png)、[recheck-network-dark.png](recheck-network-dark.png)（分区标题明显比改版前亮一档）。

---

## 4. 其余逐项（任务 4）

### F12b 未配置标签（`AppList.vue:289-295`）

| 主题 | 颜色 | 对比 | 改前 | 判定 |
| --- | --- | --- | --- | --- |
| 浅色 | `#6B7280`（--muted） | **4.43:1** | 2.54:1 | ✅ PASS |
| 深色 | `#8695AC`（--muted） | **6.36:1** | 3.57:1 | ✅ PASS |

边框仍是 1px dashed `--line-strong`，「未配置」与「已配置」的实心/虚线对比保留。

### F8 apps.subtitle 三语（`AppsPage.vue:24`；`i18n.ts:413 / 1159 / 1928`）

实测三语（切语言后读页面副标题）：

| 语言 | 渲染文案 | 裸 key |
| --- | --- | --- |
| zh | 为单个应用指定身份，未配置的应用不受影响。 | 无 |
| en | Give a single app its own identity; unconfigured apps stay untouched. | 无 |
| tr | Tek bir uygulamaya kimlik atayın; yapılandırılmamış uygulamalar etkilenmez. | 无 |

✅ PASS（不再复用 `overview.subtitle_fallback`）。

### F6 常用模板 3 列（`StatusPage.vue:493`）

- 实测 `grid-template-columns: 110px 110px 110px`（3 列，390px 视口），1 套模板不再独占半行 ✅
- 但见 **R2**。

### F10 深色 hero 去光斑（`style.css:129-130`）

| 主题 | `::before` 背景 | `::after` 背景 |
| --- | --- | --- |
| 深色 | `radial-gradient(circle, rgba(0,0,0,0), rgba(0,0,0,0) 65%)` | 同（全透明） |
| 浅色 | `rgba(76,120,255,.55) → transparent` | `rgba(126,86,255,.4) → transparent` |

✅ PASS（深色 hero 变为纯 `#151B26` + hairline，浅色保留品牌光斑）。截图：[recheck-overview-dark.png](recheck-overview-dark.png)。

### F14 深色主按钮深字（`style.css:1024-1026`）

- 设置页「开始转换」实测：`color rgb(6,16,31)` on `bg rgb(76,141,255)` = **5.96:1**（改前白字 3.2:1）✅ PASS
- 与自定义 `.tpl-new` / `.hero__cta` 的深字口径统一；浅色仍白字 5.19:1（各自都达标）。

### ❌ F16 浅色「运行中」胶囊（未改）

- 实测仍是 `#0E9F6E` on `#E7F7F0` = **3.06:1**（11px），与 t5 报告数字完全一致；`style.css:273-276` 未变，全仓也没有 `#0B7D57`/`ok-ink` 之类的新色值。
- 深色那侧本来就好（`#34D399` on `rgba(52,211,153,.1)` = 8.62:1），不需要改。
- 建议：浅色把 `.dm-pill--ok` 的文字改成 `#0B7D57`（on `#E7F7F0` = **4.64:1**），底色不动：

```css
:root:not(.dark) .dm-pill--ok { color: #0b7d57; }
```

---

## 5. 构建复跑（任务 5）

```
$ python webui/build_webui.py ; echo EXIT:$?
... npm ci ✅ / ✅ TypeScript 类型检查通过 / ✅ 代码检查通过 / ✓ built in ...
EXIT:0
```

- 全绿，时间戳 11:29:19 → 11:29:24（与你 11:28 那次一致，我这次是独立复跑）。
- 日志：`/tmp/dm-build-t8.log`。

---

## 6. 新增发现（本轮修复引入 / 未覆盖）

| # | 严重度 | 问题 | 位置 | 建议 |
| --- | --- | --- | --- | --- |
| **R1** | medium | 浅色 muted 胶囊被统一成 `#F1F3F6` + 无边框，与页面底 **1.02:1**，模板页「N 套」从"白底+边"退化成纯文字 | `style.css:283-286` + `TemplateHeader.vue:107-111` | 见 §2 补丁：浅色补 `background: var(--surface); border: 1px solid var(--line)` |
| **R2** | medium | 常用模板卡被压到 **110px**，模板名断词换行（截图里 `redmagic_9_p / ro`） | `StatusPage.vue:493` + `.tpl__name` 的 `overflow-wrap: anywhere` | ① 卡片标题改用 `template.marketname || model `（B-1 mockup 本来展示的是「红魔 9 Pro」这类市场名，不是 TOML key）；或 ② 按数量自适应：`repeat(3, minmax(0,1fr))` 只在 ≥3 套时启用，否则回 `minmax(140px,1fr)` |
| **R3** | info（既有） | 深色删除确认按钮 白字 on `#F87171` = **2.77:1**，低于 AA；非本轮引入 | `style.css:880-885` + `--danger` 深色值 `:118` | 深色 danger 改 `#B91C1C`（白字 6.47:1）或保留但把删除按钮文字加粗/加大；若要连 F1 的语义一起，可只对 message-box 的 danger 加深 |
| **R4** | info | 浅色禁用按钮 4.35:1 略低于 AA 4.5（禁用态可豁免） | `style.css:1008-1015` | 可把文字从 `--muted` 提到 `#5B6472`（≈4.9）或不动 |
| **R5** | info | tr 模板页标题「Cihaz Şablonları」在 390px 下换行两行，pill 与主按钮被挤到首行右侧；非本轮引入 | `TemplateHeader.vue` 布局 | mockup 未覆盖 tr；可给标题 `font-size: clamp()` 或允许 pill 换行 |

---

## 7. 截图（本轮新增，全部 780×1688）

深色关键 4 张：

| 文件 | 覆盖项 |
| --- | --- |
| [recheck-network-dark.png](recheck-network-dark.png) | F1 禁用胶囊、F2 直连胶囊、F3 网络分区标题、F14 深字主按钮 |
| [recheck-settings-dark.png](recheck-settings-dark.png) | F3 三组标题、F14「开始转换」深字 |
| [recheck-overview-dark.png](recheck-overview-dark.png) | F10 去光斑、F6 三列、R2 名称断词 |
| [recheck-delete-confirm-dark.png](recheck-delete-confirm-dark.png) | F1b 红色主按钮未被误伤、F2 背景里的「1 套」胶囊 |

佐证（浅色 / 状态对照）：

| 文件 | 用途 |
| --- | --- |
| [recheck-templates-light.png](recheck-templates-light.png) | R1：浅色「N 套」胶囊无轮廓 |
| [recheck-network-light.png](recheck-network-light.png) | F1 浅色禁用态、F2 浅色胶囊 |
| [recheck-overview-light.png](recheck-overview-light.png) | F6 三列、R2 名称断词（浅色同） |
| [recheck-delete-confirm-light.png](recheck-delete-confirm-light.png) | F1b 浅色红底白字 4.83:1 |
| [recheck-apps-light.png](recheck-apps-light.png) / [-dark](recheck-apps-dark.png) | F8 新副标题、F12b 未配置标签 |

---

## 8. 复现命令

```bash
python webui/build_webui.py            # 期望 EXIT:0
cd webui && npm run dev -- --port 5199 --strictPort
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --remote-debugging-port=9444 \
  --user-data-dir=/tmp/dm-t8-chrome --window-size=390,844 --hide-scrollbars

node /tmp/dm-t8-recheck.mjs light docs/design/ui-redesign/review 9444
node /tmp/dm-t8-recheck.mjs dark  docs/design/ui-redesign/review 9444
node /tmp/dm-t8-extra.mjs light zh  docs/design/ui-redesign/review   # 模板页胶囊 + 三语副标题
```

> 说明：本轮只写 `docs/design/ui-redesign/review/**`（recheck.md + 9 张 png），未改页面代码、未 commit；dev server 与 headless Chrome 用完即停。console 侧新增一条 `/favicon.ico 404`（项目本就没有 favicon，dev 环境无关项）；外部 `mui.kernelsu.org/insets.css` 的 `ERR_CONNECTION_CLOSED` 与上轮相同。
