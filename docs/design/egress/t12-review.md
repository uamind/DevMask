# t12 评审：Manager 端出口代理（加密 / 迁移 / 下发映射 / WebUI）

- 评审对象：t5（`manager/src/{crypto,db,api/egress,api/egress_dto,jobs}.rs`）与 t6（`manager/webui/src/{pages/ProxiesPage.vue,stores/manager.ts,api/client.ts}`），含管理端验收 t10
- 验收材料：`docs/design/egress/t10-acceptance.md` + `evidence-t10/`（本次评审复核了其中的关键结论）
- 评审口径：`docs/plans/egress-contract.md`（下称「契约」，冻结 v1）与 `manager/docs/ARCH.md` §5.3
- 评审人：reviewer（t12）· 2026-11-24
- **结论：`needs_revision` —— 0 blocker + 3 high + 2 medium + 12 low**

> **后续状态（2026-12-01 补注）**：本文的 findings 已由 t23 / t29 修复，其中与下发语义相关的
> **B-2（凭据轮换推不到设备）与 L-11（比较字段不含 failOpen 等）** 的最终解法是**放弃本地比较**——
> Manager 现在**每次 apply 都 PUT**，由设备返回的 `changed` 决定是否记 `egress_unchanged`；
> 契约 §6.12 已随之改写（t32），Manager 只拥有 `default` / `apps` / `proxies`，
> `failOpen` / `probeUrl` / `probeTimeoutSeconds` / `exclusionList` 归设备。
> 因此下文「比较面」「比较字段」等描述是**当时的实现快照**，不再代表现状。

---

## 0. 结论摘要

加密与迁移这两块**做得很稳**：AAD 同时绑定 `proxy id` 与字段名（`crypto.rs:49-51`）、随机 nonce、
`v1:` 前缀白名单、解密失败 fail closed；v2 迁移的 `PRAGMA foreign_keys=OFF` 在事务**之前**执行，
jobs 重建在事务内完成，结束后 `foreign_key_check` + 恢复 `foreign_keys=ON`，并有**真实 v1 库**的
迁移测试断言 devices/jobs/job_results/audit 全部保留（`db.rs:2139-2269`）。

但「下发」语义有三处 high，全部用**真实 Manager 二进制 + 契约 mock 设备**复现（见 §1、`evidence-t12/`）：

1. **B-1 `dryRun=true` + `assignments` 会真的落库**（契约 §6.10 明写「dryRun 只计算与校验、不落库、不下发」）。
   WebUI 的「预演（dryRun）」按钮走的就是这条路径——点一次预览，绑定已经写进 `device_egress`。
   复现：dryRun 前 `bindings=[]`，dryRun 后 `bindings=[{app:"", mode:"proxy", …}]`。
2. **B-2 凭据轮换/清除永远推不到设备**：`egress_unchanged` 只比 `name/type/host/port`，而设备端 `GET /egress`
   本来就只回 `hasCredentials`——密码改了、清了，apply 仍判「已是最新」，设备继续用旧密码。
   复现：PATCH password 后 apply → `code=egress_unchanged`，设备侧 `PUT /egress` 次数不增加（停在 1）。
3. **B-3 应用级绑定删不掉**：Manager 的期望态**总是**给 `apps` 数组（`jobs.rs:1585-1593`），而设备端把数组
   当「逐条 upsert」，`apps: []`/缺少某包都不会删除旧 `[[apps]].egress`——契约 §1.4 要求「给了数组 = 整体替换」。
   复现（真实 device server）：`apps:[]` 之后 `config.toml` 仍残留 `egress = "__DIRECT__"`，`GET /egress` 仍列出该应用；
   只有 `apps:null` 才清得掉。

t10 的 8 条验收结论本身**与实现一致**（409、结果码、probe、无密码明文、142 passed 都复核通过），
但这三条缺陷恰好落在验收没覆盖的组合上（dryRun×assignments、轮换×apply、apps 移除×设备合并语义），
且 t10 用的 mock 把 `apps` 整体替换了（`mock_device_egress.py` 的 `state["apps"] = body["apps"]`），
与真实设备语义不一致，因此测不出来（见 L-8）。

---

## 1. 复核方式与已跑命令

本次评审没有修改 `manager/src/`、`manager/webui/`、`server/`、`core/`（task 的 out-of-scope），
只在 `docs/design/egress/` 下写评审文档与两份可执行复现脚本（`evidence-t12/`）。

```sh
# 1) Manager 全量测试（与 t10 一致：142 passed）
cargo test -p devmask_manager
#    lib 64 + apply_flow 14 + devices_flow 14 + e2e 4 + egress_flow 6
#    + manager_api 17 + ops_endpoints 8 + review_reverify 15 => 142 passed / 0 failed

# 2) 复现 A/B（真实 manager 二进制 + docs/design/egress/fixtures/mock_device_egress.py）
cargo build -p devmask_manager
python3 docs/design/egress/evidence-t12/repro-manager-egress.py
#    A: dryRun 前 bindings=[] -> dryRun 后 bindings 多 1 行
#    B: PATCH password -> apply 结果 egress_unchanged、设备侧 PUT /egress 次数不增加

# 3) 复现 C（真实 device server 的 apps 合并语义旁证）
cargo build -p devmask_server
python3 docs/design/egress/evidence-t12/repro-device-apps-merge.py
#    apps:[] 后 config.toml 仍保留 egress = "__DIRECT__"；apps:null 才清空

# 4) 凭据边界（用复现跑出的真实数据目录）
grep -c s3cret /tmp/t12-manager.log /tmp/dm-t12/devmask-manager.db   # 全 0（收尾 checkpoint 后 -wal/-shm 已回收）
ls -l /tmp/dm-t12/devmask-manager.key                                                            # -rw------- 32 字节
```

复现输出留在 `evidence-t12/repro-manager-egress.out.txt` 与 `repro-device-apps-merge.out.txt`。

---

## 2. 七个维度逐条结论

| # | 维度 | 结论 | 依据 |
| --- | --- | --- | --- |
| 1 | 密码（AAD / fail closed / 响应·审计·错误·日志 / WebUI） | **PASS** | AAD 绑 id+字段（`crypto.rs:49-51`）、nonce 随机、`v1:` 白名单、解密失败 fail closed（`api/egress.rs:966-986`）；响应 DTO 无凭据字段（`egress_dto.rs:55-101`）；审计 detail 只有 id/name/changed（`api/egress.rs:412-419` 等）；复现目录日志/DB 扫描 s3cret = 0；WebUI 密码框 `type=password` + `autocomplete=new-password` + 永不回填 |
| 2 | 迁移（v1→v2 幂等 / 事务 / 老库不丢数据 / jobs CHECK） | **PASS**（两处低风险，L-4） | `db.rs:1781-1814`：`foreign_keys=OFF` 在 `transaction()` **之前**；重建配方在事务内；结束后 `PRAGMA foreign_keys=ON` + `foreign_key_check`；`upgrades_v1_database_without_data_loss`（`db.rs:2139-2269`）断言 job_results 未丢、FK 干净、versions=[1,2]、重开幂等、kind=egress 可插入 |
| 3 | 下发（revision / If-Match / 412 / 逐设备结果码 / 离线 / 幂等） | **FAIL**（B-1、B-2、B-3、M-1） | 412 重试一次（`jobs.rs:1246-1359`，测试 `egress_flow.rs:788-820` 断言 attempts=2）、逐设备结果码与 §6.12 一致（`jobs.rs:1660-1710`）、离线 skipped/device_offline（`:454`）、egress_unchanged 不进 partial（`:607-616`）都 PASS；但 dryRun 落库、凭据轮换不下发、apps 删不掉、Idempotency-Key 未按 bulk 路径处理 |
| 4 | 删除语义（被绑定 409 / 不误删设备与探测记录） | **PASS** | `api/egress.rs:738-785` 预检 → 409 `egress_proxy_in_use`（devices/apps ≤20 + total，`error.rs:256-275`）；`device_egress.proxy_id ON DELETE RESTRICT`、`egress_checks.proxy_id ON DELETE SET NULL`（`db.rs:138-166`）——删除代理不会删设备/绑定/探测行，历史 check 只置空 proxy_id |
| 5 | 探测（5s 超时 / 不跟重定向 / 结构化失败 / 结果总是写） | **PASS**（两处低风险，L-5/L-6） | `probe_through_proxy`（`api/egress.rs:989-1043`）：connect+total 5s、`Policy::none()`、4 KiB body、`classify_probe_error` 六类 reason；成功/失败都 `insert_egress_check`；socks5 → 422 `unsupported_proxy_type` 且落 ok=0（测试 `egress_flow.rs:620-755`）；`proxy-authorization` 只用于这一次请求 |
| 6 | WebUI（不泄露密码 / 批量不重复提交 / 错误不误导） | **PASS**（B-1 的用户触点除外） | 列表/详情只有 `hasCredentials`；编辑弹窗永不回填（`ProxiesPage.vue:686`）、提交后 `resetForm` 清空；绑定/下发按钮都绑 `:loading="egressSaving"`（Element Plus loading 即禁用，`:387`/`:394`）；`describeError` 只渲染 message+code+requestId（`api/client.ts:149-157`），不回显 details |
| 7 | 测试覆盖（迁移 / 加密 / 409 / 离线 / 健壮性） | **PARTIAL**（3 条 high 全部 0 覆盖） | 迁移、加密 round-trip、409、离线、412、egress_unchanged 都有可失败测试；但 dryRun×assignments、凭据轮换×apply、apps 移除×设备合并、Idempotency-Key 并发都没有用例（L-8） |

---

## 3. 契约条款逐条核对

### 3.1 §7.3 加密（AES-256-GCM / AAD / 密钥 / fail closed）—— 符合

| 契约点 | 实现 / 证据 |
| --- | --- |
| 复用 `devmask-manager.key` 32 字节，不引第二把密钥 | `DeviceKey::load_or_create`（`crypto.rs:97-146`）；文件 0600（复现目录 `-rw------- 32`） |
| `v1:` + BASE64(nonce‖ct‖tag) | `encrypt_with_aad`（`crypto.rs:204-221`）；解密强制 `VERSION_PREFIX`（`:223-227`，未知前缀 `v2:` → 失败） |
| nonce 随机 12 字节、绝不重用 | `Aes256Gcm::generate_nonce(&mut OsRng)`（`:206`） |
| AAD = `egress_proxy:{id}:{field}` | `egress_aad`（`:49-51`）；`encrypt_egress`/`decrypt_egress`（`:184-202`）注入同一 AAD |
| 列只接受 `v1:` 前缀（DDL CHECK） | `db.rs:132-133`（两条 CHECK） |
| 明文 `Zeroizing` | 解密返回 `Zeroizing<String>`（`crypto.rs:244`）；`decrypt_credential`（`api/egress.rs:966-986`）只在 probe 的手工认证头里用到 |
| 解密失败 `internal_error(op=decrypt)`，不降级为无凭据 | probe 路径：`api/egress.rs:982-985`（**`op` 实际是 `egress.decrypt`**，见 L-2）；apply 路径：见 M-2 |
| 密钥丢失 = 只能重填 | 密钥长度不对 → `InvalidLength` → 启动失败（`crypto.rs:105-117`、测试 `:315-329`） |

补充：**请求侧的明文没有零化**（`CreateEgressProxyRequest.username/password: String`，`egress_dto.rs:152-155`），
契约 §7.4「明文只在这一跳的内存里，`Zeroizing` 清零」目前只对**解密出来的**明文成立。属低风险内存卫生问题（L-3 附近）。

### 3.2 §6.11 schema v2 迁移 —— 符合（专项核实，见 §4）

### 3.3 §6.6 删除 —— 符合

- 预检绑定 → `409 egress_proxy_in_use`，`details{proxyId, name, total, devices, apps}`，devices/apps 各截断 20（`api/egress.rs:748-766`、`error.rs:256-275`）。
- 成功 → `204`（`:784`）；审计 `egress.proxy_delete` `{proxyId, name}`（`:774-782`）。
- 级联：`device_egress.proxy_id ON DELETE RESTRICT`（引用存在时 DB 层也会拒绝，与 409 同向）；`egress_checks.proxy_id ON DELETE SET NULL`（历史行保留，只断开关联）。
- 唯一残留：预检与删除之间若恰好新建绑定，`delete_egress_proxy` 的 FK 冲突会走 `internal("egress.delete")` → **500** 而不是 409（竞态窗口极小，见 L-9）。

### 3.4 §6.7 探测 —— 符合（契约明确的 socks5 降级已实现）

- 5s 超时（`PROBE_TIMEOUT_MS`，connect+total 都设）、`redirect(Policy::none())`、body ≤4 KiB、失败结构化（`reason` 六枚举）。
- 成功/失败都写 `egress_checks(source="manager")`；socks5 → 422 `unsupported_proxy_type` + `ok=0,error_code=unsupported_proxy_type` 入库（`api/egress.rs:841-863`）。
- 失败响应用 `ApiError::egress_probe_failed` → `details{proxyId,url,reason,timeoutMs,upstreamStatus}`（`error.rs:279-300`），与 §6.7 一致。
- 凭据只在这一跳：`proxy.basic_auth` + 手工 `proxy-authorization`（`api/egress.rs:1001-1023`），日志只打 `host`/`reason`（`:917-923`）。

### 3.5 §6.9 绑定 / §6.10 下发 —— **部分符合（B-1/B-3）**

| 契约点 | 实现 | 判定 |
| --- | --- | --- |
| 绑定全量替换、`[]` 清空、`changed:false` 幂等 | `replace_device_egress` 事务内比较后替换（`db.rs:1633-1679`）；测试 `egress_flow.rs:283-378` | ✅ |
| `""` + `__DIRECT__`/null → 不写库 | `prepare_bindings`（`api/egress.rs:1312-1324`） | ✅ |
| 不存在代理 → 422 | `:1325-1333` | ✅ |
| disabled 代理：新增 422、已有保留 | `:1334-1346` | ✅ |
| `pendingApply:true` | `:1250` | ✅ |
| §6.10 dryRun「不落库、不下发」 | `resolve_egress_targets` 在 dryRun 分支**之前**执行，assignments 形态会 `replace_device_egress`（`:1382`、`:1563-1607`） | ❌ B-1 |
| §6.10 形态 B「先落库再下发」 | 同上（非 dryRun 时正确） | ✅ |
| §6.10 `Idempotency-Key` 同键返回首次 job | `:1425-1439` 先查后插；**无 UNIQUE 冲突回查**（bulk 路径有，`api/apply.rs:114-119`）、未校验 key 形态 | ⚠️ M-1 |
| §6.12 比较字段 default/apps/proxies/**failOpen/probeUrl/exclusionList** | `egress_unchanged` 只比 default/apps/proxies（`jobs.rs:1604-1612`） | ⚠️ L-11（Manager 不管理后三者，但契约要求纳入比较）——**已解决**：t29 改为总是 PUT，契约 §6.12 由 t32 改写 |

### 3.6 §6.12 job 状态机与结果码 —— 符合

- 终态：`succeeded==total → succeeded`；`succeeded==0 && failed>0 → failed`；其余 `partial`；不变量 `succeeded+skipped+failed==total` 未满则停在 `running`（`db.rs:1086-1118`）。
- 结果码映射：`409 + egress.* → egress_not_configured`、`404 → skipped egress_unsupported`、`401/403 → failed forbidden_scope`、`412 → config_revision_mismatch`（仅重试一次后）、`422 → validation_failed`、`503 upstream_unavailable → upstream_unavailable`、其余 `device_error`（`jobs.rs:1660-1710`）——与 §6.12 表一致。
- `egress_unchanged` 是 `ok` + code，不把 job 变 partial（`jobs.rs:1474-1477`，测试 `egress_flow.rs:607-616`）。
- 探测失败只追加 warning，不改结果（`jobs.rs:1417-1434`）；`egress_proxy_disabled` 在期望态组装时给 warning（`:1542-1548`）。
- 崩溃恢复：启动时 `UPDATE jobs SET status='failed' WHERE status IN ('queued','running')`（`db.rs:1359-1375`）。
- 并发：`job_sem(2)` + `apply_sem(DFM_APPLY_CONCURRENCY)` + 每设备 `device_lock`（`state.rs:93-94`、`jobs.rs:1002-1010`）——满足「同设备串行、跨设备有界并发」。

---

## 4. Schema v2 迁移专项（验收要求的「顺序 + 不丢 job_results」）

**顺序核实**（逐行对 `db.rs:1781-1814`）：

1. 先 `execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;")`（`:1796`）——在 `transaction()` **之前**，
   避开了「PRAGMA 在事务内无效」这个经典坑；
2. `connection.transaction()` → `execute_batch(JOBS_REBUILD_SQL)` → `commit()`（`:1797-1801`），重建配方含
   `CREATE jobs_v2` → `INSERT … SELECT`（显式列名，v1 18 列全列，`db.rs:192-199` 与 v1 DDL `:61-79` 逐列一致）→
   `DROP TABLE jobs` → `RENAME` → 重建两个索引（`:172-203`）；
3. 提交后恢复 `legacy_alter_table=OFF` + `foreign_keys=ON`（`:1802`），再检查 `rebuild?` 的结果；
4. `SELECT COUNT(*) FROM pragma_foreign_key_check` 必须为 0，否则 `ForeignKeyViolation` 让启动失败（`:1805-1811`）；
5. 版本行 `record_migration(2)` 在 v2 DDL + 重建**都成功之后**才写（`:596-601`），且迁移前先执行 v1 全量 DDL
   （`CREATE TABLE IF NOT EXISTS`，对 v1/v2 库都是空操作）。

**测试证据**（`db.rs:2139-2269` `upgrades_v1_database_without_data_loss`）：

- 构造**真实 v1 库**：`execute_batch(DDL)` + `schema_migrations(1)` + 1 台设备 + 1 个 bulk job + 1 条 `job_results` + 1 条 audit，
  并断言迁移前 `PRAGMA user_version = 0`；
- 打开（触发迁移）后断言：`schemaVersion=2`、设备行保留、`get_job('job_v1')` 仍是 bulk/succeeded、
  **`job_results` 仍有 1 行（`:2191-2193`，正是 FK 忘了关时会静默丢的那张表）**、audit 保留、
  `pragma_foreign_key_check = 0`（`:2205-2215`）、`PRAGMA foreign_keys = 1`（`:2217-2224`）、`versions = [1, 2]`、
  `kind=egress` 立即可插入（`:2237-2251`）；
- 关闭后重开：设备/job 仍在、`COUNT(schema_migrations) = 2`（`:2253-2267`，幂等）。

结论：**迁移顺序与「不丢 job_results」都有可失败的测试证据**，符合契约 §6.11 与 ARCH §5.3 的冻结配方。
仅两处低风险：迁移失败的错误没有 `op="migrate"`/版本号（ARCH §5.3 要求），以及 v2 三张表的 DDL 不在事务内（L-4）。

---

## 5. 下发映射专项（revision / If-Match / 412 / 结果码 / 幂等）

| 检查点 | 实现 / 证据 | 判定 |
| --- | --- | --- |
| 取现状 `GET /egress`（10s） | `device_client.get_egress` + `request_timeout`（默认 10000ms） | ✅ |
| 期望态 = 该设备全部绑定合并成一次 PUT | `build_desired_egress`（`jobs.rs:1501-1601`） | ✅ |
| `If-Match` = 现状 revision（带引号）；缺失则 `*` | `jobs.rs:1242-1245` | ✅ |
| 412 → 重取 revision 重试一次；仍 412 → failed | `:1268-1329`；测试断言 attempts=2 且出现 httpStatus=412 步骤（`egress_flow.rs:788-820`） | ✅ |
| 逐设备结果一行、(device, app) 一行 | `rows.iter().map(...)`（`jobs.rs:1468-1487`） | ✅ |
| 离线/超时 → skipped | `EgressFailure::from_error`（`:1663-1676`） | ✅ |
| 幂等短路（已是最新 → ok+egress_unchanged） | `:1227-1239`、`:1474-1477` | ✅（但判据漏凭据，B-2） |
| 应用级增删改要能到达设备 | `apps` 数组只 upsert 列出项（B-3） | ❌ |
| 凭据新增/轮换/清除要能到达设备 | 判据只比 name/type/host/port（B-2） | ❌ |
| dryRun 不落库 | assignments 分支先写库（B-1） | ❌ |
| Manager 作为绑定真实来源（移除绑定不残留） | `default` 显式 null ✅；`apps` 数组不成立 ❌（B-3） | ❌ |

**为什么会这样（根因链）**：

- B-2：设备端 `GET /egress` 的代理摘要只有 5 个键（服务端 `dto.rs:16-44`，`hasCredentials` 是唯一凭据信号），
  所以 Manager 用「GET 现状 vs 期望」做比较时，**凭据变化在比较面上不存在**；`egress_proxy_set` 又只投影
  `(name, type, host, port)`（`jobs.rs:1630-1649`），两层叠加后密码变化必然被判「一致」。
- B-3：Manager 假设「给出 apps 数组就是整体替换」（`jobs.rs:1585` 的注释也这么写），但设备端
  `app_updates` → `Entries` → `apply_app_egress` 只对列出的包做 set/remove（`server/src/egress/request.rs:505-529`、
  `server/src/egress/api/egress.rs:614-622`、`server/src/egress/mod.rs:116-153`），`apps: []` 是 0 次改动。
  契约 §1.4 明确「给了数组 = 整体替换该数组」，所以**设备端是实现偏差**、Manager 的期望本身是对的；
  但 Manager 侧没有任何兜底（例如零 app 绑定时发 `apps: null`），缺陷因此可观测。
- B-1：`resolve_egress_targets` 同时承担「校验 + 落库」两个职责（`api/egress.rs:1511-1608`），dryRun 只想跳过
  「建 job + 下发」，却把它一起执行了；WebUI 的预演按钮（`ProxiesPage.vue:387` → `runBind(true)` → `:874-878`）
  正好走 assignments 形态。

---

## 6. WebUI 专项（t6）

| 检查点 | 结论 | 依据 |
| --- | --- | --- |
| 密码不回显 | ✅ | 列表/详情只渲染 `hasCredentials`；编辑弹窗注释「username / password 永远不预填」（`ProxiesPage.vue:686`），提交后 `resetForm` 清空（`:654-668`） |
| 密码控件 | ✅ | `type="password"` + `show-password` + `autocomplete="new-password"`（`:202-205`） |
| 不写 localStorage / URL | ✅ | 仅 Token 用 `localStorage.dm_token`（`api/client.ts:63/123/133`）；`grep localStorage` 无其它命中 |
| 错误提示不误导 | ✅（除 B-1 的交互） | `describeError` 只渲染 message+code+requestId（`api/client.ts:149-157`）；409 删除弹窗展示 devices+total（`ProxiesPage.vue:798-807`）；probe 422 明确提示 socks5 不支持（`:771-779`） |
| 批量绑定防重复提交 | ✅ | 两个按钮都绑 `:loading="egressSaving"`（`:387`/`:394`），Element Plus loading 态按钮不可点击；store 在 `applyEgress` 第一行置位并在 `finally` 复位（`stores/manager.ts:748-767`） |
| 预演是否「只读」 | ❌ | 「预演（dryRun）」会真的落库（B-1），且界面没有任何「本次预览已写入绑定」的提示——用户以为只是看看 |
| 结果码文案 | ✅ | `codeLabel`（`:922-941`）覆盖 §6.12 全部结果码，含 `egress_unchanged`/`device_offline`/`egress_proxy_disabled` |
| 设备卡/抽屉 | ✅ | 只显示 proxyName / exitIp / mode / applied / readback.matches / degraded（`DevicesPage.vue:110-122`、`:320-390`），无凭据 |

---

## 7. Findings

> 每条给「文件:行 / 问题 / 必须的修复」。存在 high → verdict=`needs_revision`。

### 7.1 high

#### B-1 `dryRun=true` + `assignments` 会真的落库（WebUI 的「预演」按钮即触发）

- **文件:行**：`manager/src/api/egress.rs:1382`（`resolve_egress_targets` 在 dryRun 分支之前调用）、
  `:1414-1423`（dryRun 分支）、`:1563-1607`（assignments → `replace_device_egress` + `egress.bind` 审计）。
  用户触点：`manager/webui/src/pages/ProxiesPage.vue:387`（「预演（dryRun）」按钮）→ `:874-878`（`applyEgress({assignments, dryRun:true})`）。
- **问题**：契约 §6.10 明确 `dryRun` = 「只计算与校验、**不落库**、不下发」。当前实现先落库再判断 dryRun，
  于是「预演」会写 `device_egress`（还可能写 `egress.bind` 审计），界面完全不提示。复现（`evidence-t12/repro-manager-egress.out.txt` §A）：
  dryRun 前 `bindings=[]` → dryRun 后 `bindings=[{app:"", mode:"proxy", proxyId:…}]`。
  后果：用户在预览时静默改变了设备的绑定关系；随后的「下发」只是把这个已被写下的绑定再推一遍，
  「先看后改」的交互语义失效；多用户并发时还会出现「谁的预览最后点，绑定就是谁的」。
- **必须的修复**：把 `resolve_egress_targets` 拆成「纯校验/归一化」（返回 grouping + prepared rows）与「落库」两步，
  dryRun 只走前者并用 prepared rows 组计划；或在 `if dry_run` 分支里改用不写库的 `plan_assignments()`。
  加测试：`POST /devices/egress/apply {assignments…, dryRun:true}` → `GET /devices/{id}/egress` 的 bindings 不变、
  `SELECT COUNT(*) FROM audit WHERE action='egress.bind'` 不增加、不建 job。

#### B-2 凭据轮换 / 清除永远推不到设备（apply 报 `egress_unchanged`）

> **已解决（t29）**：改为「总是 PUT + 凭据字段显式下发（未设置即 null）」，由设备 `changed` 判定幂等短路。

- **文件:行**：`manager/src/jobs.rs:1227`（`if egress_unchanged(&current, &desired.body)` 短路）、
  `:1604-1612`（只比 default/apps/proxies）、`:1630-1649`（`egress_proxy_set` 只投影 name/type/host/port）、
  `:1556-1569`（期望态里其实带了明文 `username`/`password`）。
- **问题**：设备端 `GET /egress` 的代理摘要**本来就不回凭据**（`server/src/egress/dto.rs:16-44`，只有 `hasCredentials`），
  所以「GET 现状 vs 期望」这个比较面对密码变化是**盲的**。PATCH 改密码后 apply 判「一致」→ 不 PUT →
  设备继续用旧密码（或继续带着已被清除的凭据），job 仍报 `ok/egress_unchanged`。
  复现（`repro-manager-egress.out.txt` §B）：PATCH `password=s3cret-2` 200 → apply 结果 `code=egress_unchanged`、
  设备侧 `PUT /api/v1/egress` 次数保持 1（不增加）。这不是「少一次优化」，而是**撤销的凭据在设备上没有撤销**。
- **必须的修复**（二选一，建议 A）：
  A. **总是 PUT**：设备端 `PUT /egress` 在文本一致时本身就是幂等的（`changed:false`、不写盘、不备份额、不改 settings），
     因此直接用「PUT 响应里的 `changed`」判定 `egress_unchanged`：`changed:false` → code=`egress_unchanged`，
     `changed:true` → 正常 ok。这样凭据、`failOpen` 等所有不可见字段都自然覆盖。
  B. 在 Manager 侧维护「已下发指纹」：给 `device_egress` 增加 `applied_proxy_updated_at`，
     任何绑定代理的 `egress_proxies.updated_at` 晚于它就强制 PUT。
  加测试：轮换密码 → apply → mock 记录最后一次 PUT body，断言 `password` 是新值；清除凭据 → apply → body 里没有凭据。

#### B-3 应用级绑定删不掉（`apps` 数组在设备端不是「整体替换」）

- **文件:行**：`manager/src/jobs.rs:1585-1593`（期望态**总是**给 `apps` 数组，含空数组）；
  设备端根因：`server/src/egress/request.rs:505-529`（`Some(Some(apps)) → AppUpdates::Entries`）、
  `server/src/egress/api/egress.rs:614-622`、`server/src/egress/mod.rs:116-153`（`apply_app_egress` 只处理列出的条目）。
- **问题**：契约 §1.4 冻结「给了数组 = 整体替换该数组」，但设备端把 `apps` 当「逐条 upsert」：
  `apps: []` 是 0 次改动，数组里缺少的包也不会被删除。于是 Manager 里删掉一个应用级绑定后，
  设备上的 `[[apps]].egress` 永远留着；如果这个代理名之后又被重新创建/绑定，残留行会**突然生效**，
  表现为「明明解绑了却还在走代理」。
  复现（`repro-device-apps-merge.out.txt`，真实 device server）：先 `apps=[{com.example.a, __DIRECT__}]`，
  再 `apps=[]` → `config.toml` 仍是 `egress = "__DIRECT__"`、`GET /egress` 仍列出该应用；只有 `apps:null` 才清空。
- **必须的修复**：
  1. **设备端**（契约正确的一侧）：`apps` 数组 = 先删除**所有** `[[apps]].egress` 再写 entries（等价于 `null` + 逐条），
     或在 `merge_egress` 里把「数组」实现成 replace-all；补一条「PUT apps=[] 清空」的设备端测试。
  2. **Manager 端兜底**：当该设备零个 app 绑定时发 `apps: null`（而不是 `[]`），至少覆盖最常见的「全部解绑」；
     并在契约/文档写明 `apps:null` 与 `apps:[]` 的差别，直到设备端修好。
  3. 加 Manager 测试：绑定 app → apply → 解绑 → apply → 断言设备状态里该应用消失（mock 必须模拟真实合并语义，见 L-8）。

### 7.2 medium

#### M-1 egress apply 的 `Idempotency-Key` 处理与 bulk 路径不一致（并发同键 → 500）

- **文件:行**：`manager/src/api/egress.rs:1425-1439`（先 `find_job_by_idempotency` 再继续）、`:1467-1471`（直接 `insert_job_with_assignments`，
  失败即 `internal("egress.apply.insert")`）；对照 bulk 路径 `manager/src/api/apply.rs:114-119`（UNIQUE 冲突回查并重放）与 `:470-476`（`valid_idempotency_key`）。
- **问题**：① 两个并发请求带同一 `Idempotency-Key` 时，第二个会在 `insert` 撞 `jobs.idempotency_key UNIQUE` → **500**，
  而契约 §6.10 要求「同键直接返回首次 job」；② egress 路径没有 `valid_idempotency_key` 校验，任意长度/含控制字符的键都会入库，
  与 bulk 路径行为不一致。
- **必须的修复**：照抄 bulk 路径：先 `valid_idempotency_key`（1..=128、无空白/控制字符，非法 → 422），
  `insert_job_with_assignments` 失败后回查同键并 `replay_response(existing)`，回查不到才 500。加并发同键测试。

#### M-2 代理凭据解密失败被伪装成设备故障（`device_error`）

- **文件:行**：`manager/src/jobs.rs:1191-1214`（`build_desired_egress` 任何错误 → `device_error` + message「组装期望出口状态失败」）、
  `:1556-1569`（`decrypt_egress` 的 `KeyError` 被 `map_err(|error| error.to_string())` 压成 String）、
  `manager/src/api/egress.rs:982-985`（probe 路径的 `internal("egress.decrypt")`）。
- **问题**：密钥被替换/密文列被搬运时（契约 §7.3 明确「解密失败不得降级、不得伪装」），apply 的 job 结果
  却是 `failed/device_error`——把「Manager 自己的密钥问题」报成「设备返回异常」，运维会去查设备。
  真实原因只出现在 Manager 日志（`tracing::error!`）里，job 结果与 UI 都看不到。
- **必须的修复**：给 `build_desired_egress` 的错误分类（`Decrypt` vs `Db`），`Decrypt` 映射为独立的 job 结果码
  （例如 `internal_error` + `details.op="decrypt"`，或新增并在契约 §6.12 登记 `decrypt_failed`），message 写「代理凭据解密失败（密钥不匹配或密文损坏）」；
  加测试：把 `devmask-manager.key` 换掉后 apply → 结果码是解密失败而不是 `device_error`。

### 7.3 low

#### L-1 `is_valid_proxy_host` 没拒绝 `:`（与自己的错误文案矛盾，且设备端必然 422）

- **文件:行**：`manager/src/api/egress.rs:105-114`（检查了 `://`、`/`、`@`、`?`、`#`、空白，**没有** `:`），
  错误文案却写「不含 scheme / 端口 / 路径 / 用户信息」（`:339`、`:532`）。
- **问题**：`host="1.2.3.4:8080"` 能创建成功；apply 时设备端 `validate_host` 拒绝（`server/src/egress/request.rs:384-386`）→
  job `validation_failed`；Manager 自己的 probe 会拼出 `http://1.2.3.4:8080:1080` 而失败。用户要到下发阶段才发现。
- **必须的修复**：与设备端同规则——非 IPv6 字面量时拒绝 `:`（IPv6 用 `[::1]` 或允许并显式说明）；补一条拒绝用例。

#### L-2 `details.op` 用 `egress.decrypt`，契约 §7.3 写的是 `decrypt`

- **文件:行**：`manager/src/api/egress.rs:982-985`（`ApiError::internal("egress.decrypt")` → `details{op:"egress.decrypt"}`），契约 §7.3 表格为 `op="decrypt"`。
- **问题**：客户端/排障脚本按契约里的 `decrypt` 匹配会漏；与设备端 `details.op="egress_parse"` 的风格也不统一。
- **必须的修复**：统一为 `decrypt`（或把契约与文档改成 `egress.decrypt`，二选一并写进契约）。

#### L-3 请求侧明文没有零化（`Zeroizing` 只覆盖解密方向）

- **文件:行**：`manager/src/api/egress_dto.rs:152-155`（`CreateEgressProxyRequest.username/password: Option<String>`）、`:177-179`（PATCH 同理）；
  对照契约 §7.4「明文只在这一跳的内存里，`Zeroizing` 清零」与 `crypto.rs:244`（解密侧已用 `Zeroizing`）。
- **问题**：创建/更新请求的明文密码随 `String` 一起 drop，不清零；与解密侧的标准不一致（低风险内存卫生）。
- **必须的修复**：请求 DTO 的凭据字段用 `zeroize::Zeroizing<String>`（serde 可用 `#[serde(with=…)]` 或自定义 `Deserialize`），
  或在 `encrypt_credential` 之后显式 `zeroize()`；加一条「请求结构 Drop 后内存被清零」的单测（可用 `Zeroizing` 的 `as_str` 断言间接覆盖）。

#### L-4 迁移失败没有 `op="migrate"` 与版本号；v2 三张表的 DDL 不在事务内

- **文件:行**：`manager/src/db.rs:582-608`（`migrate` 的 `execute_batch(DDL)`/`DDL_V2` 不在事务内）、`:596-601`；
  `manager/src/lib.rs:78-79`（`Db::open` 失败只加「打开数据库失败」context）；`manager/docs/ARCH.md:427`（要求 `op="migrate"` 与版本号）。
- **问题**：迁移失败时运维看到的只有「打开数据库失败：…：SQLite 错误：…」，分不清是迁移还是打开/权限问题；
  另外 v2 的三张表 DDL 是分条 autocommit 执行的（进程中断只留下部分表，靠 `IF NOT EXISTS` 重试自愈，不会丢数据，但不符合 ARCH 的「逐版本在事务内」）。
- **必须的修复**：`Db::open` 的错误里带 `op="migrate"` 与 `current/target` 版本；把 `DDL_V2` 放进同一个事务（`transaction.execute_batch(DDL_V2)`）。

#### L-5 `egress_checks` 无清理（无限增长）

- **文件:行**：`manager/src/db.rs:1682-1705`（只 INSERT）、`:1377-1398`（`prune()` 只清 jobs/audit）；契约 §6.11:1273 建议每 `(device_id, app)` 保留 500 条。
- **问题**：每次 apply+probe、每次手工探测都追加一行；长期运行后表无限增长，`latest_egress_checks` 的窗口查询也会变慢。
- **必须的修复**：在 `prune()` 里按 `(device_id, app)` 保留最近 500 条（或按时间 30 天），并加一条单测。

#### L-6 探测结果写入失败被吞（「结果总是写入」无法保证）

- **文件:行**：`manager/src/api/egress.rs:960-964`（`write_check` 只 `tracing::warn!`）、`manager/src/jobs.rs:1413-1415`、`:1460-1462`。
- **问题**：契约 §6.7 说结果「**总是**写入 `egress_checks`」；DB 写失败时响应仍是 200/202，调用方与 UI 都以为写进去了。
- **必须的修复**：失败时在响应（或 job warnings）里加一条 `egress_check_not_recorded`，或让 probe 返回 503（写结果属于该端点的契约副作用）。

#### L-7 `direct` 应用的探测结果被归到设备默认代理

- **文件:行**：`manager/src/jobs.rs:1398-1403` 与 `:1444-1449`（`row_proxy.get(&row.app).cloned().flatten().or_else(|| default_proxy_id.clone())`）。
- **问题**：应用级 `__DIRECT__` 的 `row_proxy[app]` 是 `None`，`or_else` 会把设备级默认代理 id 填进去——
  `egress_checks` 里这条 direct 探测被算到默认代理头上，代理详情页的 `recentChecks` 会混入并不经该代理的记录。
- **必须的修复**：只在 app 行的 mode=proxy 时回填；direct 行 `proxy_id` 留 NULL（`scope="app"` 已足够表达）。

#### L-8 测试缺口：3 条 high 全部 0 覆盖，且 t10 的 mock 与真实设备语义不一致

- **文件:行**：`manager/tests/egress_flow.rs:556`（dryRun 只测了 `devices` 形态）；全文件无「轮换后 apply」「解绑后 apply」用例；
  `docs/design/egress/fixtures/mock_device_egress.py` 的 `apply_update` 里 `state["apps"] = body["apps"]`（**整体替换**，与真实设备相反）。
- **问题**：dryRun×assignments、凭据轮换×apply、apps 移除×设备合并三条路径都没有可失败测试；mock 的 apps 语义比真实设备「更正确」，
  正好掩盖了 B-3（t10 的 8 条验收因此全绿）。
- **必须的修复**：补四条测试（见 §9 的建议测试名）；把 mock 的 `apps` 改成真实合并语义（只 upsert 列出项、`null` 才清空），
  并让 mock 记录最后一次 PUT body 供断言（或提供 `GET /api/v1/egress/proxies` 之外的测试专用端点）。

#### L-9 删除代理与新建绑定的竞态 → 500 而非 409

- **文件:行**：`manager/src/api/egress.rs:768-772`（预检通过后 `delete_egress_proxy`）；`manager/src/db.rs:1537-1548`（FK 冲突原样冒泡）。
- **问题**：预检与 DELETE 之间若有新绑定写入，`ON DELETE RESTRICT` 触发约束错误 → `internal("egress.delete")` → 500。契约 §6.6 的语义是 409。
- **必须的修复**：捕获 `rusqlite::Error::SqliteFailure` 的 FK 约束错误（`ErrorCode::ConstraintViolation`）并映射为 409 `egress_proxy_in_use`（重新查一次绑定拿 details）。

#### L-10 设备 Token 解密失败被报成 `device_offline` 警告

- **文件:行**：`manager/src/api/egress.rs:1176-1195`（`Err(error) => warnings.push(Warning::new("device_offline", error.to_string(), …))`）。
- **问题**：`GET /devices/{id}/egress` 在**密钥问题**下告诉用户「设备离线」，与网络不可达无法区分；`deviceState` 为 null 时用户会去重启设备。
- **必须的修复**：用独立码（如 `device_token_invalid` / `internal_error`）并在 message 里写「设备 Token 无法解密（devmask-manager.key 变更？）」。

#### L-11 `egress_unchanged` 未比较 `failOpen` / `probeUrl` / `probeTimeoutSeconds` / `exclusionList`

> **已解决（t29 + t32）**：Manager 不再做本地比较（每次 PUT），并明确「这几个字段归设备、Manager 不下发不比较」；契约 §6.12 已改写。

- **文件:行**：`manager/src/jobs.rs:1604-1612`；契约 §6.12 步骤 3 的比较字段列表包含它们（**该步骤在 t29 已删除**：改为总是 PUT，契约由 t32 同步）。
- **问题**：设备端这些字段被手工改过时，Manager 判「已是最新」，永远不会拉回。
- **必须的修复**：要么把这些字段纳入比较（读设备 GET 的 `failOpen`/`probeUrl` 等与 Manager 的期望——但 Manager 目前不管理它们，
  需要先决定「Manager 是否拥有这些字段」），要么在契约/文档里明确「Manager 只管 default/apps/proxies，其余字段归设备」。

#### L-12 `ARCH.md` 把迁移测试的位置写错

- **文件:行**：`manager/docs/ARCH.md:440`（「测试 `egress_flow.rs` 覆盖了 v1 库升级不丢 devices/jobs/job_results/audit」）；
  实际测试是 `manager/src/db.rs:2139` 的 `upgrades_v1_database_without_data_loss`。
- **问题**：文档指向错误的文件，排查时找不到证据。
- **必须的修复**：改成 `db.rs::upgrades_v1_database_without_data_loss`（并顺手把 `manager/docs/RUN.md` 里的 egress 冒烟说明补上 Manager 侧 mock 的正确路径，若还没补）。

---

## 8. PASS 项的核对证据

| 核对项 | 命令 / 测试 | 结果 |
| --- | --- | --- |
| 加密 round-trip / 错误 device id / 篡改密文 / 未知前缀 / 密钥长度 | `crypto.rs:253-329`（5 个单测） | PASS |
| AAD 绑字段（username/password 互换解不开） | 实现 `crypto.rs:49-51` 正确；**没有对应测试**（L-3 附近的补充项） | 实现 PASS / 覆盖缺失 |
| v1→v2 迁移不丢数据 + 幂等 + FK 干净 | `db.rs::upgrades_v1_database_without_data_loss` | PASS |
| 代理池 CRUD 脱敏 + 名字冲突 409 + 被绑定删除 409 | `egress_flow.rs:116-283` | PASS |
| 绑定 PUT/GET 往返 + deviceState | `egress_flow.rs:283-378` | PASS |
| apply 逐设备结果（ok/skipped/failed + egress_unchanged + device_offline） | `egress_flow.rs:378-618` + t10 `apply-1..4` | PASS |
| 412 重试一次（attempts=2） | `egress_flow.rs:788-820` | PASS |
| probe 成功/失败/结构化 + socks5 422 + `egress_checks` | `egress_flow.rs:620-755` | PASS |
| 无密码明文（响应/审计/日志/DB） | 复现目录扫描（§1 第 4 条）+ t10 `t10-6-security.log`（含 WAL/key 扫描） | PASS（0 命中） |
| devmask-manager.key 0600 32 字节 | 复现目录 `ls -l` | PASS |
| 审计 detail 无凭据 | `api/egress.rs:412-419`、`:704-716`、`:1229-1241`、`:1473-1486`、`:774-782`、`:896-948` | PASS |
| WebUI 密码不回显 / 不回填 / 不写本地存储 | `ProxiesPage.vue:202-205`、`:686`、`api/client.ts:63-142` | PASS |
| 批量绑定防重复提交 | `:387`/`:394` `:loading` + `stores/manager.ts:748-767` | PASS |

---

## 9. 结论与下一步

**verdict: `needs_revision`**（3 条 high，按评审约定不能 pass）。

放行前必须完成：

1. **B-1**：dryRun 不落库（含 WebUI 预演路径），加「dryRun 后 bindings/审计/job 都不变」的测试。
2. **B-2**：凭据（以及所有不可见字段）必须能推到设备——建议改成「总是 PUT，用设备返回的 `changed` 收敛 egress_unchanged」，
   并加轮换/清除两条测试。
3. **B-3**：设备端把 `apps` 数组实现成契约要求的「整体替换」（或 Manager 兜底发 `apps: null` + 文档写明），
   并加「解绑后 apply 真的从设备消失」的测试；mock 同步改成真实合并语义。
4. **M-1 / M-2**：建议同轮修（都只涉及一个分支 + 一条测试）。
5. L-1..L-12 可随修；其中 L-5（探测结果无限增长）、L-8（mock 保真）与 L-9（竞态 500）建议优先。

复验建议直接引用以下新测试名：

- `dry_run_with_assignments_does_not_write_bindings`
- `credential_rotation_is_pushed_by_apply`
- `clearing_credentials_is_pushed_by_apply`
- `removing_an_app_binding_propagates_to_the_device`
- `apply_idempotency_key_concurrent_requests_replay_one_job`
- `decrypt_failure_is_not_reported_as_device_error`

—— 本次评审未修改 `manager/src`、`manager/webui`、`server`、`core`；所有复现脚本只启动真实二进制并使用
`docs/design/egress/fixtures/` 下既有装置（Manager 数据目录 `/tmp/dm-t12`、device server 数据目录 `/tmp/dfs-t12`，收尾已退出进程）。

---

## 附录：本次新增的评审工件

| 文件 | 作用 |
| --- | --- |
| `docs/design/egress/t12-review.md` | 本文件 |
| `docs/design/egress/evidence-t12/repro-manager-egress.py` | B-1/B-2 复现：真实 manager 二进制 + mock 设备 + mock 代理定义 |
| `docs/design/egress/evidence-t12/repro-manager-egress.out.txt` | 上者输出（dryRun 落库、轮换后 egress_unchanged 且无 PUT） |
| `docs/design/egress/evidence-t12/repro-device-apps-merge.py` | B-3 旁证：真实 device server 的 `apps` 合并语义 |
| `docs/design/egress/evidence-t12/repro-device-apps-merge.out.txt` | 上者输出（`apps:[]` 不清空、`apps:null` 清空） |

复现脚本自带进程清理（`SIGTERM` + `wait`），不写入任何仓库内状态；设备 Token、代理密码均为本地临时值。
