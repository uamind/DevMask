# 项目更名与版本重开计划

> 状态：草案，待确认名称后执行
> 日期：2026-11-25
> 关联：仓库根、module/、core/、server/、manager/、webui/、docs/、.github/

## 1. 为什么要一次性做

改名不是全局替换字符串：它同时改**用户可见标识**（模块 id、安装路径、API 头、localStorage 键、数据库文件名、文档 URL），任何一处漏改都会留下"半旧半新"的状态，而且这些标识一旦发布就很难再动（模块 id 决定 `/data/adb/modules/<id>` 与更新通道）。

所以本轮把三件事绑定做：**换名 → 版本号重开 → README 重写**，一次提交，不留中间态。

## 1.5 决策记录（已定）

- 名称：**DevMask**（模块 id devmask；crate 前缀 devmask_；响应头 X-DM-*；运行路径 /data/adb/devmask）
- 兼容策略：**A 完全断开**（不读旧目录；老用户先卸载旧模块）
- 起始版本：**0.0.1**（设备端与 Manager 对齐），versionCode=20261125
- 历史：Device Faker 时期的 CHANGELOG 章节保留在文末，不删除

## 2. 影响面（已量化）

| 类别 | 数量 | 说明 |
| --- | --- | --- |
| 含旧名的文件 | ~450（改名前的统计：device_faker 424 / Device Faker 325 / DeviceFaker 46 / device-faker 41） | 现已全部替换为 DevMask 前缀 |
| 运行路径 `/data/adb/devmask` | 204 处 | 配置、日志、pid、备份、egress.toml、server.toml |
| `X-DM-*` 响应头 | 38 个文件 / 176 处 | `X-DM-Revision`、`X-DM-Egress-Revision`、`X-DM-Upstream-Request-Id`、`X-DM-Test-Harness` |
| crate 名 | 6 | 根 `devmask`、`devmask_server`、`devmask_core`、`devmask_manager`、`devmask_module_tests`、`xtask` |
| Manager 存储 | 3 | `devmask-manager.db`、`devmask-manager.key`、`/tmp/dm-*` 测试目录 |
| 前端存储键 | 2 | `dm_token`、mock 用 `dm_mock_device` |
| 文档与徽章 | 全站 | 含 en/tr、CHANGELOG、llms.txt、openapi |
| 仓库 URL | 1 | 现 remote `github.com/uamind/devmask`；代码内引用上游 `Seyud/devmask` |

## 3. 名称候选

| 名称 | 包/id | 说明 |
| --- | --- | --- |
| **DevMask**（推荐） | id=`devmask`、crate `devmask_*`、头 `X-DM-*`、路径 `/data/adb/devmask` | 短、可读、直指"设备伪装"；与现有安装标识无冲突 |
| Facade | `facade` | 语义优雅（给应用一层假外观）；需确认 crates.io 名未被占 |
| DeviceForge | `deviceforge` | 贴合"多设备农场"；偏长 |
| Mimic | `mimic` | 直观；已有同名项目（Android 应用篡改工具），撞名风险 |
| 保持不变，仅重排版本 | — | 若改名成本不划算，也可只做版本重开 + README |

> 一旦确定，模块 id 与路径按新名一次性切换；后续再改成本更高。

## 4. 命名映射（以 DevMask 为例）

| 旧 | 新 |
| --- | --- |
| module id `devmask` | `devmask` |
| 模块名 `DevMask` | `DevMask` |
| `/data/adb/devmask` | `/data/adb/devmask` |
| crate `devmask` / `_server` / `_core` / `_manager` / `_module_tests` | `devmask` / `devmask_server` / `devmask_core` / `devmask_manager` / `devmask_module_tests` |
| `X-DM-Revision` / `X-DM-Egress-Revision` | `X-DM-Revision` / `X-DM-Egress-Revision` |
| `dm_token` / `dm_mock_device` | `dm_token` / `dm_mock_device` |
| `devmask-manager.db` / `devmask-manager.key` | `devmask-manager.db` / `devmask-manager.key` |
| 环境变量 `DF_*`（若有） | `DM_*` |
| `https://github.com/uamind/DevMask` | 新仓库地址（待定） |

## 5. 兼容策略（需拍板）

- **A. 完全断开（推荐）**：新 id、新路径，不读旧目录。老用户升级前需卸载旧模块；旧配置可由用户手动拷到新路径。实现简单、状态干净。
- **B. 渐进迁移**：新路径优先，检测到 `/data/adb/devmask` 则复制配置并打日志。代价是长期保留双路径判断与测试矩阵。

建议 A——版本号重开本身就意味着"重新开始"，保留旧路径会让两套生命周期长期共存。

## 6. 版本方案

- 起始版本 **0.0.1**（新项目身份的第一个正式版本），`versionCode` 用日期式 `2026120X`，与现有规则一致。
- Manager 同步为 **0.0.1**（与设备端对齐，便于"同一代"认知）。
- CHANGELOG 新建文件头：`v0.x → v1.0.0（更名首发）`，历史章节整体归档到 `docs/CHANGELOG-archive.md`，不删。
- openapi `info.version` 与示例版本同步到 1.0.0。

## 7. README 重写要点

- 一句话定位：给每个应用一层"假设备 + 真出口"——机型/属性伪装 + 出口代理（含认证中继），由 Relay 统一管理多设备。
- 三个入口分层：设备端 WebUI（单机）、Manager（多设备/中转站）、AI（`/api/v1` + `/relay/api/v1`）。
- 快速开始：装模块 → 打开 WebUI → 建模板 → 绑应用 → （可选）配代理 → Manager 纳管。
- 能力矩阵表：机型伪装 / DPI / 属性族 / 应用级出口 / 认证中继 / 批量下发 / 审计。
- 已知限制（照实写）：带认证代理需设备内中继（v1.11 起）、设备级不支持 socks5、armed 进程常驻代码页、真机验证机型范围。
- 许可与二次分发：GPL-3.0-or-later + SPDX 头要求。
- en/tr 版本同步更新（至少 README 与 CONFIG 的关键段）。

## 8. 执行拆分（团队）

| # | 任务 | 负责 |
| --- | --- | --- |
| 1 | 命名与兼容决策冻结（id、路径、头、存储键、起始版本） | architect |
| 2 | Rust 侧改名：6 个 crate、路径常量、错误/日志前缀、模块脚本 | 后端 |
| 3 | API 头与前端存储键改名 + 契约文档同步 | 后端 + 前端 |
| 4 | Manager：包名、DB/key 文件名、测试夹具、mock | 后端 |
| 5 | 文档：README 重写（中英）、CONFIG/API/SERVER 全量替换、openapi、CHANGELOG 归档 | architect |
| 6 | 验证：全量测试 + 双侧构建 + 真机安装（新 id 路径）+ 卸载清理 | QA |
| 7 | 集成收口与单提交 | QA + captain |

## 9. 风险

| 风险 | 缓解 |
| --- | --- |
| 漏改导致半新半旧（最常见） | 收尾用 grep 白名单扫描：旧名 0 命中（除历史 CHANGELOG 与 archive） |
| 模块 id 变化导致覆盖安装失效 | 文档写明必须先卸载旧模块；`customize.sh` 检测旧目录并提示 |
| 测试夹具硬编码旧版本/旧路径 | 全仓 grep `/data/adb/devmask`、`X-DF`、`dfm_` 作为验收断言 |
| 更名后 CI 缓存与产物路径 | 同步 workflow 与 artifact 名 |
