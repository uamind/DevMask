# DevMask 独立化与仓库迁移

- 日期：2026-11-26
- 目标：DevMask 以独立项目身份维护（不是 Device Faker 的 fork 镜像），主仓库、模板源、发布通道全部收敛到自有账号，同时完整保留 GPL 义务。

## 1. 定位与许可口径

- **独立项目**：代码、WebUI、网络出口、Manager 均由 DevMask 团队维护；上游只作为早期来源与致谢对象。
- **不否认衍生关系**：早期版本基于 [Device Faker](https://github.com/Seyud/device_faker)（GPL-3.0-or-later，作者 Seyud）。`NOTICES`、源文件 `SPDX` 头、`Copyright (c) 2025-2026 Seyud` 一律保留，衍生作品继续以 GPL-3.0-or-later 发布并提供完整源码。
- 对外文案统一为「独立维护的项目，早期版本基于 Device Faker，感谢原作者与社区」，不写「非衍生」「原创」等与事实冲突的表述。

## 2. 仓库内已完成（本次提交）

| 项 | 位置 |
| --- | --- |
| README 中英土：独立性声明 + 官方源表 + 致谢改写 | `docs/README.md`、`docs/en/README.md`、`docs/tr/README.md` |
| 机型模板源改为自有仓优先、上游兜底 | `webui/src/utils/onlineTemplates.ts`（`SOURCE_CONFIGS` / `UPSTREAM_CONFIGS`） |
| 文档站与 FAQ 模板仓指向 | `website/docs/**` |
| 站点域名 | `website/docs/.vitepress/config.ts` → `https://uamind.github.io/DevMask/` |
| 子模块地址 | `.gitmodules` → `https://github.com/uamind/devmask-config.git` |
| 更新通道 | `module/module.prop` 的 `updateJson` → `uamind/DevMask/raw/main/Update.json` |
| 本地远端 | `origin` → `https://github.com/uamind/DevMask.git` |

## 3. 平台侧待办（需要 GitHub / Gitee 操作）

1. **仓库改名/转移**：`uamind/device_faker` → `uamind/DevMask`（GitHub 会对旧地址 301 跳转）。
2. **创建模板仓 `uamind/devmask-config`**：结构与上游一致，见第 4 节。
3. **Gitee 镜像**：建 `uamind/devmask-config`（或保留同名 mirror 仓），与 GitHub 同步；WebUI 的 gitee 源已指向该地址。
4. **发布资产**：Release 附件命名为 `devmask-v<version>.zip`，与 `Update.json` 的 `zipUrl` 对齐；如后端打包脚本输出 `devmask-(release).zip`，在 CI 里补一步重命名。
5. **GitHub Pages**：仓库 Settings → Pages 指向 `website/docs/.vitepress/dist` 的构建产物；确认 `BASE='/devmask/'` 与新仓库名一致。
6. **Secrets**：`BOT_TOKEN` / `CHAT_ID` / `MESSAGE_THREAD_ID` 在新仓库重新配置（CI 通知用）。
7. **旧仓归档**：`Seyud/device_faker` 侧加归档说明与跳转链接；我们不要删除旧仓，避免历史 issue/PR 与 GPL 溯源断链。

## 4. 模板仓 `uamind/devmask-config` 结构

```
templates/<name>.toml        # 单个机型模板（[templates."<name>"] 单表）
dist/templates.toml          # 聚合文件，由 .github/scripts/bundle_templates.py 生成
```

- 聚合文件是 WebUI「一次拉全量」的入口，必须随模板变更重新生成并提交。
- 每个模板至少包含 `model`（或 `marketname`）与 `fingerprint`；只写 `packages` 的模板会被概览卡当成兜底样式。
- 模板仓单独用 GPL-3.0-or-later 或 CC0；在 README 里写清来源与免责声明。

## 5. 验收清单

```bash
# 1) 主仓可克隆、CI 全绿
git clone https://github.com/uamind/DevMask.git
# 2) 模板聚合文件可直取
curl -fsSL https://raw.githubusercontent.com/uamind/devmask-config/main/dist/templates.toml | head
# 3) 上游兜底仍在（自有仓 404 时）
curl -fsSL https://raw.githubusercontent.com/Seyud/device_faker_config/main/dist/templates.toml | head -3
# 4) 更新通道
curl -fsSL https://raw.githubusercontent.com/uamind/DevMask/main/Update.json
```

WebUI 侧：在线模板库能列出模板、能打开详情抽屉、断开自有仓（临时改 `SOURCE_CONFIGS` 或用代理拦截）时回落到上游且不报错。

## 6. 回滚

- 远端：`git remote set-url origin https://github.com/uamind/device_faker.git`
- 模板源：把 `SOURCE_CONFIGS` 的 owner/repo 改回 `Seyud/device_faker_config` 与 `Seyud/device_faker_config_mirror`，或直接删除 `UPSTREAM_CONFIGS` 兜底分支。
- 文档与站点：`SITE` 改回旧域名，模板仓链接回退。

## 7. 沟通口径（公告 / Release note 用）

> DevMask 现已独立维护：主仓库迁至 github.com/uamind/DevMask，机型模板库为 uamind/devmask-config。
> 项目早期版本基于 Device Faker，感谢 Seyud 与社区；GPL 版权与出处完整保留。
> 旧地址会跳转到新仓库，已安装用户无需重装，更新通道会自动指向新版本。
