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

## 3. 平台侧状态

| 项 | 状态 | 说明 |
| --- | --- | --- |
| 仓库改名 `uamind/device_faker` → `uamind/DevMask` | ✅ 2026-11-26 | 旧地址 301 跳转；description/homepage 已更新 |
| 模板仓 `uamind/devmask-config` | ✅ 已建 | 911 个模板 + `dist/templates.toml` + bundle/validate 脚本，MIT |
| GitHub Pages | ✅ 已启用 | https://uamind.github.io/DevMask/ ，`BASE='/DevMask/'`（路径大小写敏感，已踩过一次坑） |
| Release v0.0.1 | ✅ 已发布 | 资产 `devmask-v0.0.1.zip`，与 `Update.json` 对齐 |
| 更新通道 | ✅ 可用 | `raw.githubusercontent.com/uamind/DevMask/main/Update.json` |
| CI（build / test / website） | ✅ 已跑通 | 见下方环境说明 |
| Gitee 镜像 | ✅ 2026-11-26 | [gitee.com/cutebabylab/devmask-config](https://gitee.com/cutebabylab/devmask-config)（推送账号 cutebabylab，公开仓）；GitHub Actions `mirror-gitee.yml` 在每次 push 后强制同步（已实测两端同 SHA）；WebUI gitee 源指向它，raw 实测 302→200 |
| 旧仓归档说明 | ⏳ | 需上游 `Seyud/device_faker` 侧操作，避免历史 issue 断链 |

### CI 环境说明

`Test (host crates)` 里 6 个 relay uid 用例在 GitHub runner 上失败（`app_scoped_listener_resolves_uids_and_requires_restart` 等），原因是 runner 上本地连接 uid 可解析，构造不出“未知 uid + 防火墙已生效”场景；与迁移和 WebUI 改动无关，真机/本地环境可过。后续可给这些用例加 `#[ignore]` 或注入 uid 解析失败点。

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