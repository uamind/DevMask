# 末轮复验报告 · F16 / R1 / R2（t9）

> 复验人：ui-verifier
> 日期：2026-11-26 11:44
> 范围：仅 recheck.md 的三项 —— F16、R1、R2，外加「是否引入新回退」
> 方法：vite dev（dev mock）+ headless Chrome CDP，390×844@2x，浅/深两主题实测 computed style；关键行为用夹具注入补验
> 归档说明（F-7，v0.0.2）：上一轮的 `recheck-*.png` 已在截图裁剪中删除，R1 一节里用于前后对比的 `recheck-templates-light.png` 因此改为文件名。取回方式：`git log -- docs/design/ui-redesign/review/recheck-templates-light.png` 找提交，再 `git show <commit>:<path>`。

## 0. 结论

| 项 | 内容 | 判定 |
| --- | --- | --- |
| F16 | 浅色「运行中」胶囊深字 | ✅ **PASS** — `#0B7D57` on `#E7F7F0` = **4.64:1**（改前 3.06），深色侧未被波及（仍 8.62:1） |
| R1 | 浅色 muted 胶囊轮廓 | ✅ **PASS** — 浅色恢复 `#FFF` + 1px `#E9EBF0`（底 vs 页面底 1.09、字 4.83）；模板页与网络页**完全一致**，与深色（surface-2 + 12% 边）形成对称 |
| R2 | 概览模板卡标题 | ✅ **PASS** — `label = marketname || model || name`，副标题仅在「市场名 + 型号」同时存在时出现；390 宽下 3 列（110px×3）成立，**无断词** |
| 构建 | python webui/build_webui.py | ✅ **PASS** — EXIT 0（11:38:39 → 11:38:46） |
| 回归 | 删除确认 / 分区标题 / 未配置标签 / i18n / console | ✅ **PASS** — 与 recheck.md 数字逐项一致，无新回退 |

**一句话**：三项都修好了，且没有带回任何回退。只剩一条 info 级残留建议（R6：纯 TOML key 兜底的模板卡仍会断词并被裁切），不影响本轮验收。

---

## 1. F16 浅色「运行中」胶囊（任务 1）

改动位置：`webui/src/style.css:293-296`

```css
:root:not(.dark) .dm-pill--ok {
  /* 11px 小字：加深到 4.64:1，底色不动 */
  color: #0b7d57;
}
```

实测（顶栏状态胶囊）：

| 主题 | 文字 | 底色 | 对比度 | 改前 |
| --- | --- | --- | --- | --- |
| 浅色 | `rgb(11,125,87)`（#0B7D57） | `rgb(231,247,240)`（#E7F7F0） | **4.64:1** | 3.06:1 |
| 深色 | `rgb(52,211,153)` | `rgba(52,211,153,.1)` | 8.62:1（未变） | 8.62:1 |

- 与预期值完全一致（≈4.6），越过 AA 4.5 ✅
- `:root:not(.dark)` 作用域正确：深色下 `html.dark` 存在，规则不匹配，深色胶囊零变化（实测颜色/底/边一字未变）。
- 截图：[recheck2-overview-light.png](recheck2-overview-light.png)（右上「运行中」明显变深）、[recheck2-overview-dark.png](recheck2-overview-dark.png)（深色无变化）。

---

## 2. R1 浅色 muted 胶囊轮廓（任务 2）

改动位置：`webui/src/style.css:306-310`

```css
:root:not(.dark) .dm-pill--muted {
  /* 浅色：白底 + hairline，与深色 surface-2 + 12% 边对称 */
  background: var(--surface);
  border: 1px solid var(--line);
}
```

实测（两处逐一取 computed style）：

| 主题 | 位置 | 背景 | 边框 | 字 vs 底 | 底 vs 页面底 |
| --- | --- | --- | --- | --- | --- |
| 浅色 | 模板页「1 套」 | `#FFFFFF` | 1px `#E9EBF0` | 4.83:1 | **1.09:1** |
| 浅色 | 网络页「直连」 | `#FFFFFF` | 1px `#E9EBF0` | 4.83:1 | **1.09:1** |
| 深色 | 模板页「1 套」 | `#1B2230` | 1px `rgba(255,255,255,.12)` | 5.24:1 | **1.21:1** |
| 深色 | 网络页「直连」 | `#1B2230` | 1px `rgba(255,255,255,.12)` | 5.24:1 | **1.21:1** |

- **轮廓回来了**：浅色从 1.02 / 0px 边变成 1.09 / 1px hairline，截图上「1 套」是清晰的白胶囊（对比 `recheck-templates-light.png` 里上一轮只剩文字的状态）。
- **深浅对称**：浅 = 白底 + #E9EBF0 边；深 = surface-2 + 12% 白边；两主题都是「一层底 + 发丝边」的同一种语言，模板页与网络页四处完全一致（数值逐项相等）✅
- 深色未被浅色规则污染（`:root:not(.dark)` 生效），仍维持 recheck.md 的 1.21:1。
- 截图：[recheck2-templates-light.png](recheck2-templates-light.png)、[recheck2-templates-dark.png](recheck2-templates-dark.png)、[recheck2-network-light.png](recheck2-network-light.png)、[recheck2-network-dark.png](recheck2-network-dark.png)。

---

## 3. R2 概览模板卡标题（任务 3）

改动位置：`webui/src/pages/StatusPage.vue:220-236`（topTemplates）、`:94-104`（卡片模板）

```ts
label: market || model || name,      // 标题优先市场名，其次型号，最后退回 TOML key
model: market ? model : '',          // 只有市场名 + 型号同时存在才给副标题
```

### 3.1 dev mock（只有型号、没有市场名）

| 卡片 | 标题 | 副标题 | 行数 / 断词 | 宽度 |
| --- | --- | --- | --- | --- |
| redmagic_9_pro | `NX769J` | 无（正确：市场名缺失时不重复型号） | 1 行，**无断词** | 110px |

- 改前是 `redmagic_9_p / ro`（断词 + 2 行）；改后 1 行完整显示 ✅

### 3.2 夹具（验证三条分支）

注入 3 套模板（Pixel 9 Pro 有市场名+型号、Shield TV Pro 只有市场名、key 兜底同轮存在于库中）：

| 模板 | 标题 | 副标题 | 断词 |
| --- | --- | --- | --- |
| pixel_9_pro（market + model） | `Pixel 9 Pro` | `caiman`（等宽小字） | 无 |
| redmagic_9_pro（model only） | `NX769J` | 无 | 无 |
| shield_tv（market only） | `NVIDIA Shield TV Pro` | 无（型号缺失不补空行） | 无（在空格处换行成 2 行） |

- 用 Range 逐字符取行位置判定：三张卡的换行**全部落在词边界**，没有 `…_p / ro` 这种词中断行 ✅
- 截图：[recheck2-overview-fixture-light.png](recheck2-overview-fixture-light.png)

### 3.3 三列布局

| 主题 | `grid-template-columns` | 列数 | 卡宽 |
| --- | --- | --- | --- |
| 浅色 | `110px 110px 110px` | 3 | 110px |
| 深色 | `110px 110px 110px` | 3 | 110px |

390px 视口下 3 列成立，1 套模板时只占首列（不再半行留白）✅

---

## 4. 回归检查（是否引入新回退）

| 检查点 | recheck.md 数值 | 本轮实测 | 结论 |
| --- | --- | --- | --- |
| 删除确认主按钮（浅） | `#DC2626` + 白 | `rgb(220,38,38)` + `rgb(255,255,255)` | ✅ 未变 |
| 删除确认主按钮（深） | `#F87171` + 白 | `rgb(248,113,113)` + 白 | ✅ 未变 |
| 深色分区标题 | `#8695AC` = 6.36:1 | `rgb(134,149,172)`（设置 3 组全测） | ✅ 未变 |
| 浅色分区标题 | `#111827` | `rgb(17,24,39)` | ✅ 未变 |
| 未配置标签（浅） | 4.43:1 | 4.43:1（`#6B7280` + 1px dashed） | ✅ 未变 |
| 应用页副标题 | apps.subtitle 三语 | 仍为「为单个应用指定身份，未配置的应用不受影响。」 | ✅ 未变 |
| 裸 i18n key | 0 | 0（`rawKeyVisible: false`） | ✅ 无新增 |
| console | favicon 404 + 外部 insets.css | 同（无新报错、无异常） | ✅ |

---

## 5. 构建复跑（任务 4）

```
$ python webui/build_webui.py ; echo EXIT:$?
2026-11-26 11:38:39  ✅ 依赖一致性检查通过   (npm ci)
2026-11-26 11:38:42  ✅ TypeScript 类型检查通过
2026-11-26 11:38:45  ✅ 代码检查通过          (eslint --max-warnings 0)
2026-11-26 11:38:46  ✅ 构建完成！             (vite build)
EXIT:0
```

- 覆盖性核对：三处修复的 mtime 是 `style.css 11:37:29` / `StatusPage.vue 11:37:29`，都早于本次构建（11:38:39）；构建后 `webui/src` 无任何文件再被改动，因此构建产物与截图对应同一份源码。
- 日志：`/tmp/dm-build-t9.log`。

---

## 6. 残留建议（info，不阻塞）

| # | 严重度 | 问题 | 位置 | 建议 |
| --- | --- | --- | --- | --- |
| R6 | info | 兜底分支（模板既无 `marketname` 也无 `model/device/product`）时，标题退回 TOML key，长 key 仍会**词中断行并被 2 行 clamp 裁切**：实测 `com_example_some_very_long_template_key_name` 出现 3 处 mid-word 换行，可见高度 32px vs 内容 65px | `StatusPage.vue:228` + `.tpl__name`（line-clamp 2 + overflow-wrap:anywhere） | 给概览卡片加 `:title="item.name"`（hover/长按可看全名）；或对兜底分支改用 `font-size: 11px` + 单行 ellipsis，避免"半词 + 裁切"。正常模板都有 model，命中率极低 |

- 截图：[recheck2-overview-keyonly-light.png](recheck2-overview-keyonly-light.png)

---

## 7. 截图（本轮新增，均 780×1688）

| 文件 | 覆盖项 |
| --- | --- |
| [recheck2-overview-light.png](recheck2-overview-light.png) | F16 浅色深字 + R2 单行标题（NX769J）+ 3 列 |
| [recheck2-overview-dark.png](recheck2-overview-dark.png) | F16 深色未变 + R2 深色 3 列 |
| [recheck2-templates-light.png](recheck2-templates-light.png) | R1 浅色「1 套」白底 + hairline 轮廓 |
| [recheck2-templates-dark.png](recheck2-templates-dark.png) | R1 深色对称（surface-2 + 12% 边） |
| [recheck2-network-light.png](recheck2-network-light.png) | R1 网络页同一胶囊（与模板页一致） |
| [recheck2-network-dark.png](recheck2-network-dark.png) | R1 深色同款 |
| [recheck2-overview-fixture-light.png](recheck2-overview-fixture-light.png) | R2 三分支：Pixel 9 Pro+caiman / NX769J / NVIDIA Shield TV Pro |
| [recheck2-overview-keyonly-light.png](recheck2-overview-keyonly-light.png) | R6 残留：纯 key 兜底断词 + 裁切 |

---

## 8. 复现命令

```bash
python webui/build_webui.py            # 期望 EXIT:0
cd webui && npm run dev -- --port 5199 --strictPort
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --remote-debugging-port=9444 \
  --user-data-dir=/tmp/dm-t9-chrome --window-size=390,844 --hide-scrollbars

node /tmp/dm-t9-recheck2.mjs light docs/design/ui-redesign/review 9444
node /tmp/dm-t9-recheck2.mjs dark  docs/design/ui-redesign/review 9444
node /tmp/dm-t9-fixture.mjs light  docs/design/ui-redesign/review      # R2 三分支
node /tmp/dm-t9-keyonly.mjs       docs/design/ui-redesign/review      # R6 残留
```

> 边界：本轮只新增 `docs/design/ui-redesign/review/recheck2.md` + 8 张 `recheck2-*.png`；未改页面代码、未 commit；vite dev 与 headless Chrome 用完即停。
