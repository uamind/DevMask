# DevMask WebUI 改版 · 后续开发计划

- 版本口径：v0.0.1（本轮）/ 目标 v0.0.2 与 v0.1.0
- 日期：2026-11-26
- 前置：`docs/design/ui-redesign/README.md`（决策与令牌）、`review/report.md`（验收）、`review/recheck2.md`（末轮复验）
- 总原则：动 UI 不动 API 契约；每项都要 `python webui/build_webui.py` 全绿 + 浅深双主题截图

## 1. 背景

B（浅色）/ A（深色）双主题改版已交付：5 个导航项、`NetworkPage` 独立、设置页三分组、i18n 三语补齐。独立验收（t5/t8/t9）结论为「不阻塞发布」，剩 2 条视觉遗留 + 1 个待补区块 + Manager 控制台未动。

## 2. 任务清单

| ID | 任务 | 优先级 | 估时 | 依赖 |
| --- | --- | --- | --- | --- |
| F-1 | 深色删除确认按钮对比度 | P0 | 0.5h | — |
| F-2 | 概览模板卡超长 key 兜底 | P1 | 1h | — |
| F-3 | 概览「服务状态」块（真实信号） | P1 | 4–6h | /status 字段确认 |
| F-4 | Manager 控制台改版 | P2 | 3–5d | 单独评审 |
| F-5 | 浅色禁用按钮 4.35 → ≥4.5 | P3 | 0.5h | — |
| F-6 | 土耳其语标题换行 | P3 | 0.5h | — |
| F-7 | review 截图裁剪 + WebUI 文档同步 | P2 | 2h | — |

### F-1 深色删除确认按钮对比度（R3）

- 现状：`html.dark` 下删除确认按钮白字 on `#F87171` = 2.77:1（`style.css` 的 `.el-message-box__btns .el-button--primary` 复用红色）
- 做法：只覆盖删除按钮底色，`html.dark .el-message-box__btns .el-button--primary { background:#B91C1C !important; border-color:#B91C1C !important; color:#fff !important }`，hover/active 依次 `#991B1B` / `#7F1D1D`
- 禁止：改 `--el-color-danger`（错误文字会一起变暗）
- 文件：`webui/src/style.css`
- 验收：深色删除按钮 ≥6:1；浅色不变（`#DC2626` + 白 4.83）；删除/取消各点一次

### F-2 概览模板卡超长 key 兜底（R6）

- 现状：模板既无 `marketname` 也无 `model/device/product` 时，标题退回 TOML key，110px 卡片内会词中断行并被 2 行 clamp 裁切
- 做法：`StatusPage.topTemplates` 增加 `isFallback` 标记；兜底时标题 `white-space:nowrap; text-overflow:ellipsis` 单行，`:title` 已给全名；其余情况保持 2 行
- 文件：`webui/src/pages/StatusPage.vue`
- 验收：注入一个只有 `packages` 的模板，标题单行省略、hover 可见全名；正常模板三列不变

### F-3 概览「服务状态」块（只显示真实信号）

- 现状：README §6 要求概览有服务状态，但 Zygisk / COW 属性引擎 / 中继没有可查询 API
- 可用信号：`GET /api/v1/status` 的 `module.version/versionCode`、`server.version/uptime/bind`、`config.exists/revision/mtime/conflicts`、`capabilities`；egress store 的 `effectiveMode/relay`
- 做法：
  1. 块内四行：模块（v0.0.1 · versionCode）、服务（版本 · uptime · bind）、配置（revision 短哈希 · mtime · 冲突数）、出口（已连接/直连/未启用）
  2. 不用假数据，不显示 Zygisk/COW；若行缺字段就整行隐藏，不显示 `--`
  3. 位置：概览 hero 之后、指标块之前；深色用 A 的 hairline 行式排版
- 扩展（可选，另立任务）：若确实要「Zygisk 注入 / COW 键数」，需 server 新增字段（如 `/status.runtime { zygisk:bool, cowProps:int }`），并同步 `docs/API.md` + `openapi.yaml` + Manager 透传；属于契约变更，单独立项
- 文件：`webui/src/pages/StatusPage.vue`、`webui/src/utils/i18n.ts`（`overview.status.*` 三语）
- 验收：无服务模式（只读/管理器）时整块隐藏；`build_webui.py` 全绿；浅深截图各 1 张

### F-4 Manager 控制台改版

- 现状：Manager 仍是旧视觉（桌面宽屏），与设备端 B/A 双主题不一致
- 范围：设备列表、代理池、作业/批量下发三页 + 全局导航
- 做法：复用设备端语义 token（B 浅色 + A 深色），宽屏栅格（侧栏 240px + 内容区），不套手机端卡片密度
- 依赖：先出一版设计稿（可参照 `shots/dir-c.png` 的终端风或 B 的宽屏变体），你确认后再排期
- 验收：桌面 1440 / 1024 两档截图；现有 API 调用不变；`manager/` 构建通过

### F-5 浅色禁用按钮 4.35:1

- 现状：浅色禁用主按钮 `--surface-3` 底 + `--muted` 字 = 4.35:1，略低于 AA 4.5
- 做法：`--muted` 在该选择器内降到 `#5F6B7A`（≈4.8:1），或判定禁用态豁免并记录
- 文件：`webui/src/style.css`

### F-6 土耳其语标题换行

- 现状：tr 模板页标题两行（"Şablonlar" 等长词），mockup 未覆盖 tr
- 做法：`.dm-page-title` 在 `html[lang='tr']` 下 `font-size:0.92em` 或允许两行并加大行高
- 文件：`webui/src/style.css`

### F-7 截图裁剪与文档同步

- `docs/design/ui-redesign/review/` 现有 68 张 / 11 MB，保留 `final-*`、`recheck2-*`、`apps-inherit.png` 与三份 md，其余中间轮次归档到 `docs/design/ui-redesign/archive/`（或删除）
- 更新 `website/docs/guide/webui.md`（中英）描述 5 个导航项与网络页；补 2–3 张终版截图
- 验收：仓库体积下降；文档与界面一致

执行记录（2026-11-26，t4）：

- 裁剪已完成：`review/` 从 83 个文件 / 11 MB 降到 57 个 / 6.5 MB（删 26 张中间轮次截图，全部在 git 历史里可回溯）。保留 `final-*`（30）、`final2-*`（14）、`recheck2-*`（8）、`apps-inherit.png` 与四份 md；`recheck.md` / `recheck2.md` 里指向已删图的链接改成文件名并在文首加了归档说明，无死链。
- 文档已同步：中英 `guide/webui.md` 改写为 5 个导航项（概览 / 模板 / 应用 / 网络 / 设置）、网络页四块（出口状态 + 一键直连 / 代理定义 / 探测结果 / 本机中继）、设置页三组（外观 / 高级 / 关于）；`guide/index.md` 的页面清单补上「网络」，并把出口配置从设置改到网络。
- 未做：网站文档仍未内嵌截图（站点现在零图片，加图会引入二进制且离线无法验证 VitePress 构建），如需配图另开任务。

## 3. 里程碑

| 里程碑 | 内容 | 出口标准 |
| --- | --- | --- |
| v0.0.2（本周） | F-1、F-2、F-5、F-6、F-7 | build 全绿；深色删除 6:1；截图裁剪完成 |
| v0.1.0 | F-3 服务状态块（+ 可选 /status 扩展）、F-4 Manager 设计稿 | 契约文档同步；双端视觉一致 |
| 后续 | F-4 实施、Zygisk/COW 运行态字段 | 独立评审后排期 |

## 4. 统一验收口径

1. `python webui/build_webui.py`（npm ci + vue-tsc + eslint --max-warnings 0 + vite build）
2. 浅色 / 深色各截图一次（概览 + 变更页）
3. 对比度用实测值记录（不写"看起来可以"）
4. 不改 `backend/**`、store 语义、API 契约；契约变更单独立项
5. 保留 SPDX 头与版权行；不引入新的全局渐变/毛玻璃

## 5. 不做清单

- 不用 `--el-color-danger` 全局调暗来修删除按钮
- 不为 dev mock 的 `version=1.0.0` 改产品版本口径（顶栏/关于始终 `moduleVersion || APP_VERSION`）
- 不在没有 API 的情况下显示 Zygisk / COW / 中继健康状态
- 不在本轮把 Manager 控制台塞进设备端 WebUI
