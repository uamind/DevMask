# t24 复验（round 2）：t23 对 t11 findings 的修复

- 复验对象：t23（设备端 egress 修复，源自 t11 的 15 条 findings）
- 复验口径：`docs/plans/egress-contract.md`（冻结 v1）+ `manager/docs/ARCH.md` §5.3
- 复验人：reviewer（t24）· 2026-11-24
- **结论：`pass`** —— 本轮 gate（B-1 / H-1 / M-1 / M-2 / M-3 / M-4 / M-6）全部满足；
  M-5 与 L-4（Debug 遮蔽 username）已转 t26（in_progress）、L-6（`egress_auth_unsupported` 文档登记）已转 t25 且**已完成**；
  遗留 1 条 medium（M-3 修复带来的锁内子进程无超时风险，M-7）与 3 条 low（L-5 未修、L-8/L-9 新增），
  均为 carry-over，不阻塞本轮 gate。

---

## 1. 复核方式与命令

```sh
# 1) 全量测试（本次实测 252 passed；t23 报告写 248，差额是本轮新增的 apps 数组 3 条 + 1 条）
cargo test -p devmask_server
#    lib 120 + egress_api 42 + 其余 90 => 252 passed / 0 failed
cargo test -p devmask_module_tests          # 30 passed（模块侧 replay 自检未破坏既有用例）

# 2) 评审给的复现方式（真实 devmask_server，未改 server/src）
python3 docs/design/egress/evidence-t24/repro-b1-h1.py
#    场景 1：只改 probeUrl 的 dry_run；场景 2：同时改 config+egress；场景 3：轮换密码；场景 4：应用级探测失败
#    => 24 项断言全 PASS / 0 FAIL（见 evidence-t24/repro-b1-h1.out.txt）

# 3) 复用 t11 的两份探针复现（直接 #[path] 引用真实 system.rs）
rustc --edition 2021 -o /tmp/t24-dumpsys docs/design/egress/evidence-t11/repro-dumpsys-parse.rs && /tmp/t24-dumpsys
rustc --edition 2021 -o /tmp/t24-failopen docs/design/egress/evidence-t11/repro-failopen-cleared.rs && /tmp/t24-failopen
```

`repro-dumpsys` 的输出已随修复变化：真机原文 `Global proxy: [127.0.0.1] 18080 xl=` → `Proxy { 127.0.0.1, 18080 }`，
且 `matches(Expected::Direct) = false`（修复前是 `Unknown` / `true`）。

---

## 2. 逐条 findings → 状态 + 证据

| id | 状态 | 证据（测试名 / 命令） |
| --- | --- | --- |
| **B-1** | ✅ fixed | `dry_run_removal_diff_does_not_echo_stored_credentials`（只改 probeUrl + 删定义两种 diff）、`diff_rotation_does_not_echo_old_or_new_password`、`redact_diff_line` 单测、`combined_diff_redacts_egress_but_keeps_config_changes`；真实 server 复现场景 1/2/3 全 PASS（上下文行、删除行、轮换行都 `<redacted>`，且 config.toml 段与 probeUrl 变更仍在） |
| **H-1** | ✅ fixed | `app_scope_probe_failure_does_not_degrade_device_proxy`：502 + `details.scope="app"` + `warnings[0].code=egress_probe_scope_app`，且 `settings put` 调用数 **0**、`health.degraded=false`、`effectiveMode=proxy`；真实 server 复现场景 4 同样 PASS |
| **M-1** | ✅ fixed | `degrade_to_direct` 以 `ApplyOutcome.matches` 判定 cleared（state.rs:169-205）；`health.cleared` 驱动 `effectiveMode`（api/egress.rs:332-346）；`stubborn_settings_report_actual_mode_and_need_real_clear`（报 proxy + degradedReason=settings_unavailable + 无 egress_degraded_to_direct）、`probe_success_does_not_recover_when_reapply_fails`（apply 不生效不 recover） |
| **M-2** | ✅ fixed（缺 1 个端到端用例，见 L-9） | `parse_dumpsys_state` 同时接受 `[h] port: N`、`[h] N`、尾部属性、IPv6（system.rs:505-530 单测）；`find_global_proxy_line` 只认行首 `Global proxy:`；评审复现：真机原文 → `Proxy{127.0.0.1,18080}` 且 `matches(Direct)=false` |
| **M-3** | ✅ fixed（带来 M-7 的新风险） | `reconcile_once` 按写路径锁序取 config 共享 → egress 共享快照（state.rs:204-252）；`reconcile_waits_for_the_write_lock`（持有写锁时 reconcile 阻塞 200ms 不完成）；PUT/DELETE 的 settings 应用移入同一临界区（api/egress.rs:809-837、991-1004） |
| **M-4** | ✅ fixed | `SettingsError::Failed{code, stderr}` 截断到 MAX_STDERR（system.rs:107-140，`settings_failure_keeps_truncated_stderr` 断言长度 + 不含 password）；`settings_api_error` 每次失败恰好一行 warn 且带 stderr（`settings_failure_maps_to_503_and_one_warn_line`、`unavailable_settings_logs_without_stderr`）；`settings_failure_returns_503_and_keeps_files`、`settings_unavailable_returns_503_but_writes_files`（GET 给 `egress_settings_mismatch` + `details.reason=settings_unavailable`） |
| **M-6** | ✅ fixed | settings 从 `changed` 条件里拆出（api/egress.rs:809-835）；`replay_reapplies_settings_when_readback_drifted`：文件没变、`changed:false`、revision 不变，但 settings 被补回期望值 |
| **M-5** | ↔ transferred → **t26** | t23 已落地 `install_replay_selfcheck` / `replay_selfcheck_at_exit`（hooks.rs:737-772，日志 `replay hook never fired package=… symbols=…`，`docs/CONFIG.md`/`SERVER.md` 写明诊断行）；剩余属 t26，不在本轮判缺陷。**（t26 已落地并改文案：现为 `replay hook not fired within 2000ms … module retained …`，触发点=窗口到点定时器+退出兜底，当前口径见 [../../CONFIG.md](../../CONFIG.md)）** |
| **L-1** | ✅ fixed | `fail_closed_does_not_clear_settings`（PUT 读回不一致 → `settings put global http_proxy :0` 次数 0、effectiveMode=proxy）、`fail_closed_probe_failure_does_not_touch_settings`（探测失败 → settings put 0 次、degraded=true 但 reason=probe_failed） |
| **L-2** | ✅ fixed | `undefined_app_proxy_falls_back_to_device_mode`：设备级有 default → `effective=proxy, proxy=us_la_01` + `egress_proxy_not_found`；无 default → `effective=direct, proxy=null` |
| **L-3** | ✅ fixed | `file::backup_id` 改为剥离 `<file>.` 前缀（file.rs:717-722）；`backup_id_matches_contract_shape`（35 字符 `<stamp>-<rev8>`、不含 `toml`）；`config_backup_id` 复用同一口径；docs/SERVER.md 的排查 glob 已同步 |
| **L-4** | ↔ transferred → **t26（in_progress）** | t26 的 subject 明确含「Debug 遮蔽 username」；当前代码 `src/egress.rs:213` 仍是 `.field("username", &self.username)`、测试只断言 password（:1156-1163），因此 t23 报告「已满足」为时过早，但工作已排进 t26，不在本轮判缺陷 |
| **L-5** | ❌ **not fixed → accepted (low)** | `api/egress.rs:69` 与 `:739` 仍是 `format!("配置解析失败: {error:#}")`；`Config::from_toml` 仍是 `Ok(toml::from_str(content)?)`。触发条件 = 用户把凭据粘进 config.toml **且**该行语法坏；且 `GET /config` 本就可读 config.toml，增量暴露有限。**t23 报告称「已满足」不准确** |
| **L-6** | ↔ transferred → **t25（已完成）** | 契约 §1.8:452-453 登记 `egress_auth_unsupported` / `egress_probe_scope_app`；`docs/API.md:1092/1101/1108`、`docs/CONFIG.md:356` 同步 |
| **L-7** | ✅ fixed | `heal_mode` 自愈时打 `tracing::warn!(path, mode, "egress.toml 权限过宽，已自愈为 0600")`（file.rs:697-701） |

### 2.1 新增 findings（本轮修复带来 / 未覆盖）

| id | severity | 问题 | 证据 / 建议 |
| --- | --- | --- | --- |
| **M-7** | medium（accepted，follow-up） | M-3 把 settings 应用移进**写临界区**后，PUT/DELETE 在持有 config 排他 flock 期间要跑 3–6 次 `settings`/`dumpsys` 子进程；而 `SystemRunner`（commands.rs:63-79）用 `Command::output()` **没有超时**。一次卡住的 `settings`/`dumpsys` 会把整条配置 API（读写、其它写端点、reconcile）一起挂住——修复前只影响那一个请求。 | 真机 `dumpsys connectivity` 通常 0.1–0.5s/次，正常路径会增加 0.3–2s 的锁持有时间；建议给 `SystemRunner` 加 `wait_timeout`（如 10s）或按 t11 的备选口径「锁内只做文件事务，settings 在独立 mutex 下应用 + 应用后重算 revision 有界重试」 |
| **L-8** | low（accepted） | `degrade_to_direct` 的降级 warn 新增 `actual` 字段（settings 值 = `host:port`），与契约 §3.5「降级日志字段 reason / effectiveMode，**不含 host/port**」冲突。仅地址、非凭据。 | state.rs:190-199；建议删掉 `actual` 或只记 `actual_present=true` |
| **L-9** | low（accepted） | M-2 缺「settings=:0 但 dumpsys 显示有代理 → 非直连」的端到端用例：现有 `SettingsSim` 的 dumpsys 由同一份 state 推导，无法表达这种分歧；t23 报告称补了该用例，实际只有解析层单测。 | 建议给 sim 增加 `set_dumpsys_only_proxy(host, port)` 能力并补一条用例；解析层行为已由评审复现验证 |

---

## 3. 修复未引入新泄漏面的逐面复核

| 面 | 结论 | 依据 |
| --- | --- | --- |
| 响应 | ✅ | `diff` 全路径经 `combined_diff → redact_diff_credentials`（api/egress.rs:569-615），上下文行/删除行/轮换行三种形态都有测试；真实 server 复现场景 1/2/3 断言响应体不含 `s3cret`/`alice`/`n3wpass` |
| 响应（其它字段） | ✅ | 新增的 502 `details.scope` / `details.warnings[]` 只有 scope/reason/proxy 名；`health` 未新增字段（`cleared` 只在进程内，`HealthView` 不序列化它） |
| 日志 | ⚠️ 一处低风险见 L-8 | `settings_api_error` 只记 `component/code/stderr`（stderr 截断 ≤MAX_STDERR，参数是 `host:port`，无凭据；单测断言不含 password）；`heal_mode` 只记 path/mode；app-scope 失败只记 reason；degrade 日志新增 `actual`（host:port）→ L-8 |
| 审计 | ✅ | 审计 action/detail 未改动（`api/egress.rs:412-419`、`:774-782`、`:1229-1241`、`:1473-1486`）；`write_semantics::audit_log_records_writes_without_secrets` 仍绿 |
| 错误 details | ✅（L-5 为既有面，未新增） | egress.toml 解析失败仍是 `details{op:"egress_parse", message}`，message 来自 `short_toml_error`（只回 key 路径/行号，不含原文）；probe 502 details 新增 scope/warnings 但无凭据；config.toml 解析错误的 `{error:#}` 属既有 L-5 面 |
| diff | ✅ | 见 B-1；`combined_diff_redacts_egress_but_keeps_config_changes` 明确断言 config.toml 段未被脱敏吃掉 |

补充核对：`tracing_never_formats_credentials`（egress_api.rs:1353）仍扫描 `server/src/egress/**` 的所有日志行禁止出现 `password`，252 passed 覆盖了它。

---

## 4. 契约条款复核（修复后仍成立）

| 条款 | 结论 | 依据 |
| --- | --- | --- |
| §1.8 警告码表 | ✅ 已登记 10 条 | 契约 §1.8:452-453 新增 `egress_auth_unsupported`、`egress_probe_scope_app`；docs/API.md:1092/1101/1108 同步（t25） |
| §1.6 fail-open 粒度 | ✅ | `scope:"app"` 失败只记 lastProbe + `details.scope` + `egress_probe_scope_app`；只有 device 才 `degrade_to_direct`（契约 §1.6:370） |
| §3.5 恢复语义 | ✅ | 探测成功先 `apply` 再 `readback` 校验，`matches` 才 `recover()`（`probe_success_does_not_recover_when_reapply_fails`）；`effectiveMode` 由 `degraded && cleared` 推导 |
| §3.5 降级日志字段 | ⚠️ L-8 | reason/cleared/actual 中 `actual` 是 host:port，契约要求不含 host/port |
| §2.2 权限自愈 | ✅ | 自愈 chmod + 日志 warn（L-7 修复）+ 响应 warning `egress_perm_fixed`，三处齐了 |
| §1.4 apps 语义 | ✅（顺带） | 设备端 `apps` 数组已实现「整体替换」（`clear_app_egress_except`，api/egress.rs:512-534 + :719-727），`apps_empty_array_clears_all_overrides` / `apps_array_replaces_unlisted_overrides` / `apps_null_and_omitted_follow_contract` 三条测试 —— 这是 t12 的 B-3 在设备端的那一半，**不属于 t11 findings**，供 t29 参考（Manager 侧仍需兜底或对齐） |

---

## 5. 结论与建议

**verdict: `pass`**（本轮 gate 全部满足；无 blocker / high）。

放行后的 carry-over（建议由 captain 决定归属，均不阻塞本轮）：

1. **M-7（medium）**：`SystemRunner` 加超时，或把 settings 应用移出 config flock（独立 egress mutex + 应用后 revision 有界重试）。
2. **L-5（low）**：config 解析错误改用 `toml::de::Error::message()`（与模块侧口径一致）。
3. **L-8（low）**：降级日志去掉 `actual`（或只记是否非空）。
4. **L-9（low）**：`SettingsSim` 增加 dumpsys-only 分歧能力 + 一条端到端用例。

L-4 已在 t26 的范围内（模块侧收口），无需另外建任务。

另外提醒：t23 的报告里 L-4（username 遮蔽）与 L-5（解析错误口径）**声称已修但当前代码仍未改**；L-4 已由 t26 承接，L-5 仍待安排，本轮已在 §2 里按实际代码状态标注；
其余「248 passed」与实测「252 passed」的差额是本次统计时刻的测试增量（apps 数组 3 条 + 1 条），不影响结论。

---

## 附录：本次新增的评审工件

| 文件 | 作用 |
| --- | --- |
| `docs/design/egress/t24-review.md` | 本文件 |
| `docs/design/egress/evidence-t24/repro-b1-h1.py` | B-1 / H-1 的真实二进制复验（24 项断言） |
| `docs/design/egress/evidence-t24/repro-b1-h1.out.txt` | 上者输出（全 PASS） |

复现脚本临时数据目录 `/tmp/dfs-t24`，收尾自动 SIGTERM 退出；仓库内状态未修改（只写 `docs/design/egress/`）。
