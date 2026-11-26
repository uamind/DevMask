# v0.0.2 复验收尾报告 · R3 / F-5 / F-6 / R6（t3）

> 复验人：ui-verify（独立复验，只读页面代码，仅写 review/**）
> 日期：2026-11-26 12:40（实测窗口 12:20–12:40）
> 范围：`docs/plans/ui-redesign-followups.md` 的 v0.0.2 四项目标改动 + 是否带来新回退
> 方法：生产构建（`python webui/build_webui.py`）+ vite dev（390×844@2x，headless Chrome CDP 实测 computed style / Range 行盒 / 真实点击）；浅深双主题各跑一遍；R6 用 Pinia store 注入只含 `packages` 的模板
> 被测工作树：`webui/src/style.css`（t1）、`webui/src/pages/StatusPage.vue`（t2）、`webui/src/App.vue`（t5，locale→lang 同步）；三者均未 commit，`git diff | sha256` = `feb12795…e8842a`

## 0. 结论

| 项 | 内容 | 判定 | 关键数字 |
| --- | --- | --- | --- |
| 构建 | `python webui/build_webui.py` | ✅ **PASS** | EXIT 0，产物 **900.2 KB**（40 文件 / 976K 磁盘），13.9s |
| R3 / F-1 | 深色删除确认按钮 | ✅ **PASS** | 白字 on **#B91C1C = 6.47:1**（验收 ≥6）；hover 8.31，active 10.02 |
| F-5 | 浅色禁用主按钮 | ✅ **PASS** | **#5F6B7A on #F1F3F6 = 4.88:1**（改前 4.35）；深色 4.85 未变 |
| F-6 | tr 页面标题不折行 | ✅ **PASS（390/412 口径）** + ⚠️ 360dp 限制 | tr 5 页标题全单行，21px；390 下「Cihaz Şablonları」153.2px；**360dp 实测仍 2 行** |
| R6 | 概览模板卡超长 key 兜底 | ✅ **PASS** | 兜底卡 1 行 + ellipsis（86 vs 313px），`:title` 全名；对照改前 4 行/65px 内容被 2 行裁切 |
| 回归 | 删除确认 / 分区标题 / muted pill / 三列 / 未配置标签 | ✅ **PASS** | 逐项数值见 §6，与 recheck2 一致，无新回退 |
| console | 运行期报错 | ✅ **PASS** | 0 条；唯一网络失败是外部 `mui.kernelsu.org/internal/insets.css`（离线环境，改前同） |

**一句话**：四项都达到验收口径，构建全绿、无新回退；唯一要记的限制是 **360dp 视口下 tr 标题仍折两行**（390/412 单行），以及两条 info 级样式收敛建议。

---

## 1. 构建（可复现）

```
$ python webui/build_webui.py
12:31:18 ✅ 依赖一致性检查通过      (npm ci)
12:31:21 ✅ TypeScript 类型检查通过 (vue-tsc --noEmit)
12:31:24 ✅ 代码检查通过            (eslint --max-warnings 0)
12:31:27 ✅ 构建完成！              (vite build)
构建产物大小: 900.2 KB
EXIT:0        real 0m13.898s
```

- 产物：`module/webroot` **976K / 40 文件**；主 CSS `index-BN11n8mF.css` 55.5 KB。
- 产物内确认三处改动都在：`html[lang=tr] .dm-page-title{font-size:21px;line-height:1.3}`、`#5f6b7a`（浅色禁用）、`#b91c1c`（深色删除）；`index-*.js` 内存在 `documentElement.lang`（t5 的 locale→lang 同步）。
- 构建时间晚于三处源码 mtime（style.css 12:04 / StatusPage.vue 11:58 / App.vue 12:20），产物与截图对应同一份源码。日志：`/tmp/dm-t3-build2.log`。

---

## 2. R3 / F-1 深色删除确认按钮

选择器：`html.dark .el-message-box__btns .el-button--primary, html.dark .delete-confirm-box .el-button--primary`（`style.css:912-933`）

实测（删除确认框内 computed style，390×844@2x）：

| 主题 | 状态 | 文字 | 底色 | 对比度 | 改前 |
| --- | --- | --- | --- | --- | --- |
| 深色 | 默认 | `#FFFFFF` | `#B91C1C` rgb(185,28,28) | **6.47:1** | 2.77:1 |
| 深色 | hover | `#FFFFFF` | `#991B1B` rgb(153,27,27) | **8.31:1** | — |
| 深色 | active | `#FFFFFF` | `#7F1D1D` rgb(127,29,29) | **10.02:1** | — |
| 深色 | 取消（次要按钮） | `#C3D2E8` | `#141922` | **11.5:1** | 未变 |
| 浅色 | 默认 | `#FFFFFF` | `#DC2626` rgb(220,38,38) | **4.83:1** | 4.83（未变） |

- 达到任务书「深色 ≥6:1」；`--danger` / `--el-color-danger` 未被改动（错误文字仍是浅红，未误伤）。
- hover 由旧的 `filter: brightness(0.94)` 改为显式色值（`filter:none`），深色 hover 才能可测到 8.31。
- 浅色 hover/active 的 computed `background-color` 仍是 `#DC2626`（压暗靠 filter 合成，不改色值），视觉一致，不改对比度结论。
- 截图：[final2-delete-dark.png](final2-delete-dark.png)（深色，白字深红按钮）、[final2-delete-light.png](final2-delete-light.png)（浅色不变）。

---

## 3. F-5 浅色禁用主按钮

选择器：`:root:not(.dark) .el-button--primary.is-disabled`（`style.css:1058-1065`），实测对象＝模板弹窗里「添加」按钮（`packageInput` 为空时真禁用）。

| 主题 | 文字 | 底色 | 对比度 | 改前 |
| --- | --- | --- | --- | --- |
| 浅色 | `#5F6B7A` rgb(95,107,122) | `#F1F3F6`（--surface-3） | **4.88:1** | 4.35:1 |
| 深色 | `#8695AC` | `rgba(76,141,255,.13)` → 合成 `#1B283F` | **4.85:1** | 4.85（逐位未变） |

- 浅色越过 AA 4.5；`:root:not(.dark)` 作用域正确，深色禁用态零变化。
- 边框仍 `1px var(--line)` `#E9EBF0`，按钮形状/尺寸未变（118→62px 是本按钮宽度，非改动引起）。
- 截图：[final2-disabled-light.png](final2-disabled-light.png)（弹窗全屏）、[final2-disabled-crop-light.png](final2-disabled-crop-light.png)（按钮与输入框特写）、[final2-disabled-crop-dark.png](final2-disabled-crop-dark.png)（深色未变）。

---

## 4. F-6 土耳其语标题

选择器：`html[lang='tr'] .dm-page-title { font-size: 21px; line-height: 1.3 }`（`style.css:217-223`）

### 4.1 生效路径（由 t5 补上，本轮实测确认）

`webui/src/App.vue:167-177` 新增 `DOCUMENT_LANG = { zh:'zh-CN', en:'en', tr:'tr' }` + `watch(locale, …, { immediate:true })`。
因此**真实 tr 用户不需要手写注入**：locale=tr 加载后 `documentElement.lang` 自动为 `tr`，规则命中。

| 阶段 | html lang | 结果 |
| --- | --- | --- |
| locale=tr 直接加载（无干预） | `tr`（App.vue 同步） | 5 页标题 21px 单行 |
| 再手写 `documentElement.lang='tr'` | `tr` | 数值完全相同（幂等） |
| 切回 zh | `zh-CN` | 27px（浅）/ 24px（深），1 行 |
| 切到 en | `en` | 27px / 24px，「Device Templates」2 行——**与改前一致，非回归** |

> 说明：t1 交付时 `index.html` 硬编码 `lang="zh-CN"`、规则不可达（t1 的残留 ①），该缺口已由 t5 的 App.vue 同步关闭；本轮只做验证，未改任何代码。

### 4.2 五个页面标题（390×844@2x）

| 页面 | 文案 | 字号 | 行数 | 标题盒宽 | 可用宽 |
| --- | --- | --- | --- | --- | --- |
| 概览 | Genel Bakış | 21px | 1 | 350 | 350 |
| 模板 | **Cihaz Şablonları** | 21px | **1** | **153.2**（浅）/ **152.14**（深） | 161.6 |
| 应用 | Uygulamalar | 21px | 1 | 120.78 / 119.42 | — |
| 网络 | Ağ | 21px | 1 | 350 | 350 |
| 设置 | Ayarlar | 21px | 1 | 350 | 350 |

- 与任务书偏差：任务书写 24px，t1 按实测取 21px（24px 下该串需 173.1px > 161.6px 可用宽，仍折两行）。本轮复算一致：21px 下 153.2px，余量 ≈5%。
- 截图：[final2-tr-title-light.png](final2-tr-title-light.png)、[final2-tr-title-dark.png](final2-tr-title-dark.png)。

### 4.3 视口限制（实测，非计算值）

| 视口 | 「Cihaz Şablonları」 | 行数 |
| --- | --- | --- |
| **360×800** | 盒宽被压到 131.58px（自然宽 153.2），高 54.59 = 2×27.3 | **2 行** |
| 390×844（验收口径） | 153.2px | 1 行 |
| 412×915 | 153.2px | 1 行 |

- 360dp 是常见 Android 宽度，**该档位 tr 标题仍会折两行**（t1 交付说明里的计算值，本轮实测坐实）。不阻塞 390 口径验收，但建议后续单开：`html[lang='tr']` 再加一档窄屏（如 ≤374px 用 19px，或收紧胶囊/按钮占位），或允许两行并把 `line-height` 收到 1.15。

---

## 5. R6 概览模板卡兜底

改动：`StatusPage.vue:225-239`（`isFallback = label === name`）、卡片 class 绑定（`:103`）、`.tpl__name--fallback`（`:553-561`）。

注入方式：`pinia._s.get('config').config.templates['com_example_some_very_long_template_key_name'] = { packages: [...] }`（只含 `packages`，即 marketname/model/device/product 全空的兜底分支），测完即删除。

| 卡片 | 分支 | 标题渲染 | 行数 | 盒宽 / 内容宽 | 高度 | `:title` |
| --- | --- | --- | --- | --- | --- | --- |
| `com_example_…key_name` | 兜底（仅 key） | `com_exampl…` 单行省略 | **1** | 86 vs **313**（截断） | 16px（=lineHeight 16.2） | ✅ 全名 |
| `redmagic_9_pro` | 型号当标题 | `NX769J` | 1 | 86 vs 86 | 16px | ✅ |

- computed：`display:block; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; overflow-wrap:normal`（覆盖 `.tpl__name` 的 `-webkit-line-clamp:2`）。
- **改前对照**（同文本仅移除新 class，等价旧样式）：4 个行盒、`clientHeight 32`（2 行 clamp）/ `scrollHeight 65`、`overflow-wrap:anywhere` → 词中断行 + 裁切，与 recheck2 §6 记录的 R6 完全一致；现已消除。
- 三列布局未变：`grid-template-columns: 110px 110px 110px`，gap 10px（浅/深一致），1 套模板时只占首列。
- 截图：[final2-overview-fallback-light.png](final2-overview-fallback-light.png)、[final2-overview-fallback-dark.png](final2-overview-fallback-dark.png)。

---

## 6. 回归检查（浅 / 深）

| 检查点 | 浅色实测 | 深色实测 | 结论 |
| --- | --- | --- | --- |
| 删除确认主按钮 | `#DC2626` + 白 = 4.83:1 | `#B91C1C` + 白 = 6.47:1 | ✅ |
| 删除确认行为（点「删除」） | 卡片 1→0，弹窗关闭，显示空状态 | 同左 | ✅ |
| 删除确认行为（点「取消」） | 弹窗关闭，卡片仍在（列表仍渲染） | 同左 | ✅ |
| 设置页三组标题 | `#111827` / 13.5px / **16.26:1** ×3（外观·高级·关于） | `#8695AC` / 11px / **6.36:1** ×3 | ✅ 与 recheck.md 一致 |
| muted pill ① 模板页「1 套」 | `#FFF` + 1px `#E9EBF0` + `#6B7280` = **4.83:1** | `#1B2230` + 1px `rgba(255,255,255,.12)` + `#8695AC` = **5.24:1** | ✅ |
| muted pill ② 网络页「直连」 | 与①逐项相同（4.83:1） | 与①逐项相同（5.24:1） | ✅ 两处一致 |
| 概览模板卡三列 | `110px 110px 110px`, gap 10 | 同左 | ✅ |
| 应用页「未配置」标签 | `#6B7280` on 行白底 **4.83:1**（页面底 #F4F5F7 口径为 4.43，与 recheck2 同值）；1px dashed `#DDE1E8` | `#8695AC` on `#141922` = **5.80:1**；1px dashed `rgba(255,255,255,.12)` | ✅ |
| 应用页副标题文案 | 「为单个应用指定身份，未配置的应用不受影响。」 | 同左 | ✅ 未变 |
| 裸 i18n key / console 报错 | 0 / 0 | 0 / 0 | ✅ |
| 网络失败 | 仅外部 `https://mui.kernelsu.org/internal/insets.css`（ERR_CONNECTION_CLOSED，离线环境，改前同） | 同左 | ✅ 无新增 |

---

## 7. 截图（本轮新增 14 张，均 780×1688 = 390×844@2x，除标注外）

| 文件 | 覆盖项 |
| --- | --- |
| [final2-overview-light.png](final2-overview-light.png) / [final2-overview-dark.png](final2-overview-dark.png) | 概览三列、标题字号基线（27/24px）、muted「直连」 |
| [final2-templates-light.png](final2-templates-light.png) / [final2-templates-dark.png](final2-templates-dark.png) | 模板页「1 套」muted pill、标题 |
| [final2-delete-light.png](final2-delete-light.png) / [final2-delete-dark.png](final2-delete-dark.png) | R3 删除确认（浅 4.83 / 深 6.47） |
| [final2-disabled-light.png](final2-disabled-light.png) / [final2-disabled-dark.png](final2-disabled-dark.png) | F-5 模板弹窗（含禁用「添加」） |
| [final2-disabled-crop-light.png](final2-disabled-crop-light.png) / [final2-disabled-crop-dark.png](final2-disabled-crop-dark.png) | F-5 按钮特写（1198×421） |
| [final2-tr-title-light.png](final2-tr-title-light.png) / [final2-tr-title-dark.png](final2-tr-title-dark.png) | F-6 tr「Cihaz Şablonları」单行 |
| [final2-overview-fallback-light.png](final2-overview-fallback-light.png) / [final2-overview-fallback-dark.png](final2-overview-fallback-dark.png) | R6 兜底卡单行省略 + 正常卡 |

---

## 8. 残留与建议（info，均不阻塞 v0.0.2）

| # | 级别 | 问题 | 位置 | 建议 |
| --- | --- | --- | --- | --- |
| 1 | **medium（限制）** | 360dp 视口下 tr 模板页标题仍折两行（实测 131.58px 盒宽 vs 153.2px 自然宽，高 54.59px） | `style.css:217` | 加窄屏档（≤374px 19px）或允许两行 + `line-height:1.15`；或压缩胶囊/新建按钮占位 |
| 2 | info | `html.dark .el-message-box__btns .el-button--primary` 会把**所有** message box 主按钮（409 冲突确认、启用出口确认、导入确认等）染成深红；这是 HEAD 已有的「全局红主按钮」策略（`--danger`），非本轮引入 | `style.css:915` | 若要严格区分危险/普通确认，把选择器收敛到 `.delete-confirm-box` |
| 3 | info | 浅色 hover 仍是 `filter: brightness(0.94)`，与深色改后的显式色值不对称；computed bg 不变化，不影响对比度 | `style.css:907-910` | 可选：浅色也改成显式 `#B91C1C` 系 hover，两主题一致 |
| 4 | info | `review/` 目录现 11 MB（历史 68 张 + 本轮 14 张 ≈1.4 MB） | `docs/design/ui-redesign/review/` | F-7（t4）裁剪时请保留 `final2-*`、`final-*`、`recheck2-*`、`apps-inherit.png` |

---

## 9. 复现命令

```bash
# 1) 构建
python webui/build_webui.py                       # 期望 EXIT:0 / 900.2 KB

# 2) 独立 dev（与队友 5199 隔离，避免共用 node_modules/.vite 造成 504 Outdated Optimize Dep）
cd webui && npx vite --config /tmp/dm-t3-vite.config.ts      # port 5197, cacheDir /tmp/dm-t3-vite-cache

# 3) headless Chrome（独立 profile，勿与 9444 上的其他实例共用）
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --remote-debugging-port=9446 --user-data-dir=/tmp/dm-t3-chrome2 \
  --window-size=390,844 --hide-scrollbars

# 4) 四轮实测（full = 三项+回归+R6；tr = 标题）
node /tmp/dm-t3-verify.mjs full light zh docs/design/ui-redesign/review
node /tmp/dm-t3-verify.mjs full dark  zh docs/design/ui-redesign/review
node /tmp/dm-t3-verify.mjs tr   light tr docs/design/ui-redesign/review
node /tmp/dm-t3-verify.mjs tr   dark  tr docs/design/ui-redesign/review
node /tmp/dm-t3-width.mjs                          # 360/390/412 视口对比
node /tmp/dm-t3-behavior.mjs                       # 删除/取消真点一次 + 失败请求归因
```

> 边界：本轮只新增本文件 + 14 张 `final2-*.png`；未改任何页面代码、未 commit；dev server（5197）与 headless Chrome（9446/profile `/tmp/dm-t3-chrome2`）用完即停。
