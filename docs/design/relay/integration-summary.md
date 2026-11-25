# 带认证中继（relay）集成收口：交付说明

- 日期：2026-11-25；分支 `main`（HEAD `3ddd031` + 未提交改动）；**未执行 git commit / git push**。
- 范围：契约 v1.2 + 设备端 relay（HTTP/SOCKS5、端口表、uid 防火墙）+ 模块注入 + Manager/WebUI + 文档 + 真机验收。
- 结论：**工程校验全绿、真机验收最终 PASS**；1 条发布步骤遗留（版本号 bump）与 2 条非阻塞观察见 §5/§6。

## 1. 改动清单（工作区快照）

`git status`：59 个文件修改、8 处新增；`git diff --stat` 汇总 `59 files changed, 5824 insertions(+), 441 deletions(-)`（不含新增未跟踪文件）。

| 区域 | 主要内容 |
| --- | --- |
| `server/src/relay/*`（新增 9 文件） | port（FNV-1a 分配/bind paused）、secret（zeroize）、basic、uid、http（CONNECT/absolute-URI）、socks5（RFC1929 + 远程 DNS）、firewall（三链 + 语义 -S 比较）、pipe、net（单 deadline）、dto/service/mod |
| `server/src/{api,egress,app,commands}` | `GET /egress.relay`、`GET /egress/proxies.localPort/relayActive`、`POST /egress/relay/restart`、5 个 warning 码、`via=relay` 探测、设备级 socks5 → 422/黑洞、PUT 期间不出现直连窗口（H1） |
| `src/*`（Zygisk 模块） | relay 视图注入（合法 → 127.0.0.1:local_port；非法 → fail-closed 127.0.0.1:9 + arm replay）、回退收窄、凭据载体移除 |
| `manager/src` + `manager/tests` | `DeviceSummary.egress.relay` 透传、`proxies[].relayActive`、socks5 探测真连（reqwest socks / RFC1929） |
| `manager/webui`、`webui` | Manager 中继徽标/详情区块/代理池三态；设备端「网络出口」卡片 relay 区块、restart 按钮、三语 i18n |
| `module/*` | `uninstall.sh` / `service.sh` 三链清理（幂等）；service.sh 陈旧 pid 校验（reboot 自启修复） |
| `docs/*` | 契约 v1.2、API/CONFIG/SERVER/CHANGELOG 中英、en/tr、openapi 30 操作、manager/docs 透传表 |
| `docs/design/relay/*` | 真机验收 t9/t20/t22 报告与证据、本说明 |

## 2. 校验命令与结果（全部 exit 0）

| # | 命令 | 结果 |
| --- | --- | --- |
| 1 | `cargo fmt --check` | EXIT=0（1s） |
| 2 | `cargo clippy --workspace --exclude device_faker --all-targets -- -D warnings` | EXIT=0（23s） |
| 3 | `cargo test --workspace --exclude device_faker` | EXIT=0（115s）；**706 passed / 0 failed**（含 server 184 + relay_api 21 + manager 166 等） |
| 4 | `cargo check -p device_faker --target aarch64-linux-android` | EXIT=0 |
| 5 | `cargo test -p device_faker_module_tests` | EXIT=0（60 passed） |
| 6 | `python webui/build_webui.py` | EXIT=0（14s，含 vue-tsc + eslint --max-warnings 0 + vite build） |
| 7 | `npm --prefix manager/webui run build` | EXIT=0 |
| 8 | `cd manager/webui && npx vue-tsc --noEmit` | EXIT=0 |
| 9 | contract 测试（含于 #3） | `contract_table_matches_openapi` / `every_documented_operation_is_routed` / `unimplemented_openapi_json_stays_404` 全 ok；documented == routed == 30 |

原始输出：`docs/design/relay/evidence-t12/commands.log`（每条含 `EXIT=` 与耗时）。

## 3. 真机验收轨迹（Pixel 9 / KernelSU）

| 轮次 | 结论 | 关键问题 |
| --- | --- | --- |
| [t9](t9-acceptance.md) | FAIL | 防火墙 ALLOW 重复 uid0 → state_mismatch；accept 期 uid_unresolved |
| [t20](t20-acceptance.md) | FAIL | -S 文本 vs 真机规范格式不一致；uid_unresolved 仍在 |
| [t22](t22-acceptance.md) | **PASS** | applied=true + listener accept；uid 两态（applied=true 放行 / firewall=false fail-closed）；应用级 SOCKS5 + 远程 DNS；#13 竞态 150 次 0 命中；#21 强制 apply；绑定面、凭据零泄漏、卸载清理、设备恢复 |

## 4. 已知限制（四条 + 补充）

1. **可检出性（P1–P11）**：中继端口在 `127.0.0.1:17800–17899`、链名 `DF_EGRESS_RLY`、注入目标从真实上游变为回环地址；同 uid 检测代码可枚举。
2. **设备级依赖 relay 存活**：listener 挂掉期间整机代理链路不可用（设备级 settings 指向回环）；服务有守护循环但存在窗口。
3. **root 出网绕过 uid 防火墙**：relay 以 root 连接上游，会绕过 AFWall+/NetGuard 等按 uid 的防火墙（P8）。
4. **fail-open 静默直连**：`[probe].fail_open=true`（默认）时，上游探测失败会清系统代理 → 静默直连（P9）；要「绝不直连」需设 `false`。`via=relay` 的失败**不**触发 fail-open。
5. 分层 uid 策略：accept 期归属失败时仅当 `firewall.applied=true` 放行（root-trust，只约束谁能连端口，不约束以谁 uid 出网）；`firewall=false` / `applied=false` 一律 fail-closed。
6. 设备级 `default` 只支持 http：socks5 → PUT `422 invalid_egress_default`；旧配置 `enabled=true` 时黑洞 `127.0.0.1:9` + `egress_relay_device_socks5_unsupported`，无 device listener。
7. 端口劫持窗口：释放→重绑之间可被本机进程抢占，检测靠下一次 bind 失败（`addr_in_use`）；应用级重分配、设备级不监听。
8. relay 只覆盖 Java 层：native 栈与 WebView 不受 fail-closed 保护。

## 5. 遗留项

- **发布步骤：版本号 bump**。当前 `server/Cargo.toml = 1.10.0`、根 `Cargo.toml = 1.10.0`、`module/module.prop = v1.10.0 (20261201)`、`manager/Cargo.toml = 0.2.0`（manager 独立版本线）；`docs/CHANGELOG.md` 顶部已是 `v1.10.0 → v1.11.0`。仓库历史（`chore(release)` 提交）显示 bump 是独立发布提交：请在最终发布提交里同步 bump server/root/module.prop（及 openapi 示例中的版本示例值），或明确接受「代码 1.10.0 + CHANGELOG 预告 1.11.0」的现状。
- **t37（device-server）清理项**：`BLACKHOLE` 注释与 `device_socks5_warning` 未用形参；属 cosmetics，未影响本轮校验，落地后建议只重跑 server 测试。
- 非阻塞观察 O1/O2（见 t22 报告 §6）：accept 期 uid 仍 `uid_unresolved`（策略已覆盖）；代理定义切换时曾出现一次 `applied=false`（-F+restart 恢复，干净重装不复现）。

## 6. 复现

`BT`bash
bash /tmp/t9-run/t12-commands.sh          # 串行跑 §2 的 1–7
cd manager/webui && npx vue-tsc --noEmit  # #8
`BT`

证据：`docs/design/relay/evidence-t12/`（commands.log / version-check.log / secret-scan-clean.log / stats.log）。
