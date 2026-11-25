# t30 复验（round 2）：t29 对 t12 findings 的修复

- 复验对象：t29（manager 侧修复，源自 t12 的 17 条 findings）
- 复验口径：`docs/plans/egress-contract.md`（§6.12/§7.3/§7.4 按 captain 的 t32 同步口径，见下）+ `manager/docs/ARCH.md`
- 复验人：reviewer（t30）· 2026-11-24
- **结论：`pass`** —— t12 的 3 条 high 与 2 条 medium 全部 fixed；16/17 条 low fixed，L-10 只剩 GET 路径半边（low，carry-over）；
  另新增 1 条 low（探测**失败**路径丢弃 `egress_check_not_recorded`）。无 blocker/high。

> 口径说明（按 captain 在 t30 开工时对齐的两点）：
> 1) `manager/tests/common/mock_device.rs` 的改动是 B-1/B-2/B-3 测试成立的前提（也是 t12 L-8 的要求），按已声明处理，**不记缺陷**；
> 2) 下发语义改为「总是 PUT + 以设备 `changed` 判定 `egress_unchanged`」是有意选择，契约 §7.3/§6.12 的同步由 **t32** 承接；本报告只按新语义判断实现是否自洽，不把「与旧 §7.3 不符」列为缺陷。

---

## 1. 复核方式与命令

```sh
cargo test -p device_faker_manager
# 71 + 14 + 14 + 4 + 14(egress_flow) + 17 + 8 + 15 => 157 passed / 0 failed

# t12 的两份复现（真实 manager 二进制 + t10 fixture），数字已随修复翻转：
python3 docs/design/egress/evidence-t12/repro-manager-egress.py
#   A: dryRun 后 bindings 变化 0（修复前 1）；B: 轮换后 PUT 次数 1->2、code=None（修复前不增/egress_unchanged）

# 本轮新增的断言式端到端复验（真实 manager + mock 设备）：
python3 docs/design/egress/evidence-t30/repro-b1-b3-fixed.py
#   A/B/C/D 共 17 项断言全 PASS（dryRun 无副作用、轮换再发 PUT、解绑后设备归零、非法 Idempotency-Key 422）

# captain 指定核对项：显式 null 是否真的清掉设备端凭据（真实 device_faker_server）：
python3 docs/design/egress/evidence-t30/repro-null-clears-device-credential.py
#   password:null / username:null 后 egress.toml 里凭据行消失、代理定义保留、hasCredentials=false
```

---

## 2. 逐条 findings → 状态 + 证据

| id | 状态 | 证据（测试名 / 命令） |
| --- | --- | --- |
| **B-1** | ✅ fixed | `plan_egress_targets`（api/egress.rs:1686-1824，纯读）+ `persist_egress_bindings`（:1827-1852，仅真实下发调用）；dryRun 分支在落库之前返回（:1524-1533）；`dry_run_assignments_have_no_side_effects`（bindings=[]、无 egress.bind 审计、无 job）；t30 复现 A 全 PASS |
| **B-2** | ✅ fixed | 总是 PUT + `value["changed"]` 判定 `unchanged`（jobs.rs:1233-1274）；凭据字段总是显式（无密文发 `null`，jobs.rs:1602-1641）；`credential_rotation_and_clear_reach_device`（最后 PUT body 带新密码；清除后 `username/password` 为 null 且 mock `hasCredentials=false`）；t30 复现 B（PUT 1→2、code=None）；t30 复现「null 清设备凭据」（真机 device server） |
| **B-3** | ✅ fixed | 零 app 绑定时 `apps: null`（jobs.rs:1657-1672）；`unbinding_apps_clears_device_state_and_sends_null`（body.logs `apps` 为 null、default null、proxies []，mock 设备侧归零）；t30 复现 C 全 PASS；mock 的合并语义按 t12 L-8 要求改过（已声明，不记缺陷） |
| **M-1** | ✅ fixed | `valid_idempotency_key`（api/egress.rs:1893，1-128/无空白）+ 重放在校验之前（:1488-1510）+ UNIQUE 冲突回查重放（:1567-1575）；`invalid_idempotency_key_is_rejected_before_any_write`、`concurrent_same_idempotency_key_yields_one_job`；t30 复现 D（非法 key → 422） |
| **M-2** | ✅ fixed | `DesiredError::{Decrypt,Db}` → `decrypt_failed` / `device_error`（jobs.rs:1683-1724，details.op=decrypt）；设备 Token → `device_token_invalid`（jobs.rs:1104）；`credential_decrypt_failure_is_its_own_result_code`（decrypt_failed + op=decrypt + message 含 manager.key + 设备零请求）、`device_token_decrypt_failure_has_own_code` |
| **L-1** | ✅ fixed | `is_valid_proxy_host`：含 `:` 时只接受 IPv6 字面量（api/egress.rs:105-126） |
| **L-2** | ✅ fixed | probe 解密失败 `ApiError::internal("decrypt")`（api/egress.rs:1077）；job 侧 details.op=decrypt |
| **L-3** | ✅ fixed | 请求明文显式 `zeroize`（api/egress.rs:418/664/690/857，`zeroize_credentials`），解密侧仍 `Zeroizing`（:1059-1079）；`zeroizes_request_credentials_after_encryption` |
| **L-4** | ✅ fixed | `DbError::Migration{from,to}`（db.rs:219-220）+ `migration_error` 包装；`apply_schema_v2` 单事务（db.rs:1819-1852），FK OFF 在事务之前、结束后恢复 + `foreign_key_check` |
| **L-5** | ✅ fixed | `prune` 增加 egress_checks 每 `(device_id, app)` 保留 500 条（db.rs:1418-1430）+ `prune_keeps_latest_500_checks_per_device_and_app` |
| **L-6** | ✅ fixed（失败路径见 L-13） | `write_check` 返回 bool；probe 成功路径加 `egress_check_not_recorded`（api/egress.rs:971-973）；job 侧 `record_check` + `check_not_recorded_warning`（jobs.rs:1497-1531） |
| **L-7** | ✅ fixed | `check_proxy_id` 只对设备级行回落 default；app direct 行 → NULL（jobs.rs:1379-1384）；`probe_check_proxy_id_only_for_proxy_rows` |
| **L-8** | ✅ fixed（按 captain 口径不记缺陷） | mock 的 PUT 合并语义 + `egress_last_put_body()`/`egress_proxies()`/`egress_apps()` 访问器（manager/tests/common/mock_device.rs:651-842） |
| **L-9** | ✅ fixed | 预检 409 + FK 竞态再查一次并回 409（api/egress.rs:796/822） |
| **L-10** | ⚠️ **partial（job ✓ / GET ✗）** | job 路径 → `device_token_invalid`（jobs.rs:1104，有测试）；`GET /devices/{id}/egress` 的 Token 解密失败仍报 `device_offline`（api/egress.rs:1287-1293） |
| **L-11** | ✅ fixed（契约同步 t32） | `manager/docs/ARCH.md:833` §10.7 写明「failOpen/probeUrl/probeTimeoutSeconds/exclusionList 不在 Manager 请求体里出现」与取舍 |
| **L-12** | ✅ fixed | `ARCH.md:443` 指向 `db::tests::upgrades_v1_database_without_data_loss` |

### 2.1 复核 captain 点名的三个自洽性问题

| 问题 | 结论 | 证据 |
| --- | --- | --- |
| 凭据显式 `null` 是否**真的**清得掉 | ✅ | Manager 侧：body 里 `username/password` 为 `null`（测试 `credential_rotation_and_clear_reach_device` + mock `hasCredentials=false`）；设备侧用**真实 device server** 复验：`password:null`/`username:null` 后 `egress.toml` 的凭据行消失、定义与 host 保留、`GET /egress.proxies[0].hasCredentials=false` |
| `changed=false` 时 job 不落 `partial` | ✅ | jobs.rs:1472-1487 结果恒为 `status:"ok"` + `code:"egress_unchanged"`；`repeated_apply_is_idempotent_and_reports_unchanged` 断言 job `succeeded`、`skipped=0`、put step `skipped/egress_unchanged` |
| 离线 / 412 重试路径仍成立 | ✅ | 412：仍重取 revision 重试一次（jobs.rs:1276-1337），`not_configured_and_revision_conflict_paths`（attempts=2）；离线：`EgressFailure::from_error` 统一把 `Offline` 映射 `skipped/device_offline`（GET 与 PUT 共用），`apply_pushes_egress_and_records_per_device_results`（:416/:454）覆盖 |

---

## 3. 契约条款复核（按新语义 / t32 同步口径）

| 条款 | 结论 | 依据 |
| --- | --- | --- |
| §7.3 加密 AAD / v1 前缀 / fail closed | ✅ | `egress_aad`（crypto.rs:49-51）+ DDL CHECK（db.rs:132-133）+ 解密失败 fail closed（api/egress.rs:1059-1079）；本轮未改加密实现 |
| §7.3 明文 `Zeroizing` | ✅（增强） | 解密方向 `Zeroizing`；请求方向新增显式 zeroize（L-3） |
| §7.4 凭据生命周期 | ✅ | 创建/更新只写密文；下发时解密进请求体（每次 apply 都发，见 §6 观察）；轮换/清除立即生效（B-2）；日志/审计/响应仍只有 `hasCredentials` |
| §6.12 egress_unchanged 幂等短路 | ✅（语义已改为「设备 changed=false」） | jobs.rs:1233-1274、1472-1487；测试见上；契约文本由 t32 同步 |
| §6.12 结果码映射 | ✅ | `EgressFailure::from_error`（jobs.rs:1736-1785）与表一致；新增 `decrypt_failed` / `device_token_invalid`（t32 需登记） |
| §6.12 终态判定 | ✅（未改） | `finalize_job` 规则未变；unchanged 仍 ok |
| §6.6 删除 409 | ✅ | 预检 + FK 竞态兜底都回 `egress_proxy_in_use`（api/egress.rs:796/822）；`proxy_crud_is_secret_free_and_blocks_bound_delete` |
| §6.7 probe 错误码 | ✅ | socks5 → 422 `unsupported_proxy_type` + ok=0；不可达 → 502 `egress_probe_failed`；结果写库失败 → `egress_check_not_recorded`（成功路径） |
| §6.10 dryRun | ✅ | 不落库/不写审计/不建 job（B-1）；plan 来自「既有+本次」合并结果（`dry_run_assignments_have_no_side_effects` 断言 plan.mode=proxy 而 bindings=[]） |

---

## 4. Schema v2 迁移专项

- **顺序**：`apply_schema_v2`（db.rs:1819-1852）先 `PRAGMA foreign_keys=OFF; legacy_alter_table=ON`（事务**之前**），再开事务执行 `DDL_V2` + `rebuild_jobs_table` + 版本行，一次 commit；无论成败恢复 PRAGMA。
- **不丢 job_results**：`db::tests::upgrades_v1_database_without_data_loss` 仍通过（157 passed 里），断言 job_results 保留、`pragma_foreign_key_check=0`、`foreign_keys=1`、versions=[1,2]、重开幂等。
- **失败诊断**：`DbError::Migration{from,to}`（L-4）→ 启动错误带 `op=migrate, current=N, target=2`。

---

## 5. 七个维度逐条结论

| # | 维度 | 结论 | 依据 |
| --- | --- | --- | --- |
| 1 | 密码（AES-GCM/AAD/fail closed/响应·审计·错误·日志/WebUI） | **PASS** | AAD 绑 id+field；响应 DTO 无凭据字段；审计 detail 只有 id/name/changed；日志无请求体（本轮扫描 `tracing::` 无 body/password 命中）；WebUI 不预填、不写 localStorage（除 token）；新增 decrypt 错误只含 proxy id/字段名 |
| 2 | 迁移（v1→v2 幂等/事务/不丢数据/jobs CHECK） | **PASS** | 见 §4；v2 现在**单事务**（比 t12 更严） |
| 3 | 下发（revision/If-Match/412/逐设备结果码/离线/幂等） | **PASS**（新语义自洽） | 总是 PUT + 设备 changed；412 重试一次；离线 skipped；unchanged → ok+code；见 §2.1 |
| 4 | 删除语义（409/不误删设备与探测记录） | **PASS** | 预检 409 + FK 竞态兜底 409；`ON DELETE RESTRICT`/`SET NULL` 未变 |
| 5 | 探测（5s/不跟重定向/结构化/结果总是写） | **PASS** | 见 §3；唯一缺口：失败路径的写库失败未上报（L-13，low） |
| 6 | WebUI（不泄露密码/不重复提交/错误不误导） | **PASS** | 密码控件/不预填/不回显未变；t27 修了首登 token 保存与绑定弹窗空态；预演按钮不再有副作用（B-1 由服务端修，UI 无需改） |
| 7 | 测试覆盖 | **PASS** | 新增 8 条 egress_flow 用例覆盖本轮全部 high/medium + 多数 low（见 §2）；157 passed / 0 failed |

---

## 6. 新发现（不阻塞）

| id | severity | 问题 | 建议 |
| --- | --- | --- | --- |
| **L-13** | low（accepted） | `probe_proxy` 的**失败**分支丢弃 `write_check` 的返回值（api/egress.rs:1003-1018），DB 写失败时 502 错误里不会出现 `egress_check_not_recorded`；成功分支（:971-973）已处理。 | 失败分支把 warning 一并塞进 `details.warnings`（或至少日志区分）。 |
| — | observation | 总是 PUT 意味着**每次 apply 都把解密后的凭据放到设备端**（本机 HTTP + Bearer Token，与改动前同一通道；改动前仅在内容变化时才发）。 | 属新语义的固有代价，契约 §7.4 的「明文只在请求体里」仍成立；建议 t32 同步时在文档里点一句。 |

---

## 7. 结论与建议

**verdict: `pass`**（t12 的 3 high + 2 medium 全部 fixed，low 16/17 fixed；无 blocker/high）。

carry-over：
1. **L-10 的 GET 半边**：`GET /devices/{id}/egress` 的 Token 解密失败仍报 `device_offline`（api/egress.rs:1287-1293），建议改成 `device_token_invalid` 与 job 路径一致。
2. **L-13**：探测失败路径补 `egress_check_not_recorded`。
3. t32 同步契约时，请把 `decrypt_failed` / `device_token_invalid` / `egress_check_not_recorded` / 「总是 PUT + 设备 changed」一起写进 §6.12/§7.3/§1.8 与 `manager/docs/API.md`。

---

## 附录：本次新增的评审工件

| 文件 | 作用 |
| --- | --- |
| `docs/design/egress/t30-review.md` | 本文件 |
| `docs/design/egress/evidence-t30/repro-b1-b3-fixed.py` | B-1/B-2/B-3/M-1 的断言式端到端复验（17 项） |
| `docs/design/egress/evidence-t30/repro-b1-b3-fixed.out.txt` | 上者输出（全 PASS） |
| `docs/design/egress/evidence-t30/repro-null-clears-device-credential.py` | captain 指定项：显式 null 清设备凭据（真实 device server） |
| `docs/design/egress/evidence-t30/repro-null-clears-device-credential.out.txt` | 上者输出（全 PASS） |
