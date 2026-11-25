# t11 评审：设备端出口代理（凭据 / fail-open / 幂等 / SELinux / 注入安全）

- 评审对象：t3（`server/src/egress/*` + `server/src/api/egress.rs` + `server/src/commands.rs` + `module/uninstall.sh`）与 t4/t14/t17（`src/egress.rs` + `src/hooks.rs` + `src/lib.rs` + `src/spec.rs`）的合并结果
- 验收材料：`docs/design/egress/t9-acceptance.md`、`t15-acceptance.md`、`t18-acceptance.md` 及 `evidence*/`
- 评审口径：`docs/plans/egress-contract.md`（下称「契约」，冻结 v1）
- 设备（引用验收材料，本次评审未连真机）：Pixel 9 / `4A190DLAQ002QR` / Android 17 (SDK 37) / KernelSU
- 评审人：reviewer（t11）· 2026-11-24
- **结论：`needs_revision` —— 1 blocker + 1 high + 6 medium + 7 low**

---

## 0. 结论摘要

设备端控制面（存储、原子写、读时自愈、revision、If-Match、探测、清除单实现）整体质量高，
契约的多数硬约束有可失败的单测支撑（server 230 passed / module 30 passed）。

但**凭据边界有一个 blocker**：`PUT /egress?dry_run=true` 或 `?diff=true` 会把 `egress.toml` 的
unified diff 原样返回，里面就有 `password = ...`——**不需要知道密码的 write 客户端，只要把
`probeUrl` 改成一个新值再 preview 一次，就能读回已存密码**（§1 的复现用的是真实
`server/src/diff.rs`）。这直接违反契约 §7.1 第 1/3 条与 §2.2「不进任何 API 明文」。

另有一条 high：**应用级（`package`）探测失败会触发设备级 fail-open**（`settings put global
http_proxy :0`），即「一个应用的代理挂了 → 整机所有应用切回真实 IP」。契约 §6.12 的 job 步骤 5
就是 `POST /api/v1/egress/probe`，而该端点支持 `package`——只要 Manager job 或任何脚本对某个应用
探测一次，设备级出口就会掉到直连，不需要人工确认。

此外 fail-open 的「清直连是否真的成功」判定用的是 `CommandOutcome::Ok`，与读回解耦：用真实
`system.rs` 驱动一个「put 返回 0 但不生效」的 runner，`clear()` 返回 `Ok` 且 `matches=false`，
而服务仍会把 `health.degraded` 置真、`effectiveMode` 报 `direct`（§M-1）。

---

## 1. 复核方式与已跑命令

本次评审没有修改 `server/`、`src/`、`core/`、`manager/`（task 的 out-of-scope），只在
`docs/design/egress/` 下写评审文档与三份最小复现程序（`evidence-t11/`）。

```sh
# 1) 服务端全量测试（含 egress_api 26 条）
cargo test -p devmask_server
#    lib 114 + apps 9 + config 21 + contract 3 + egress_api 26 + overview 3
#    + packages_device 9 + skeleton 15 + static_site 8 + templates 13 + write_semantics 9
#    => 230 passed / 0 failed

# 2) 模块侧宿主单测（t17 交付的降级分支就在这里）
cargo test -p devmask_module_tests
#    => 30 passed / 0 failed

# 3) 三份复现（#[path] 直接引用被评审的真实源码，不复制业务逻辑）
rustc --edition 2021 -o /tmp/t11-repro    docs/design/egress/evidence-t11/repro-diff-credential.rs   && /tmp/t11-repro
rustc --edition 2021 -o /tmp/t11-dumpsys  docs/design/egress/evidence-t11/repro-dumpsys-parse.rs     && /tmp/t11-dumpsys
rustc --edition 2021 -o /tmp/t11-failopen docs/design/egress/evidence-t11/repro-failopen-cleared.rs  && /tmp/t11-failopen

# 4) 源码级断言（人工复核用；仓库里已有同类断言测试）
grep -rn 'settings delete' --include=*.rs --include=*.sh server module | grep -v docs/   # egress 代码里 0 处
grep -rn 'password' server/src/egress/dto.rs                                            # 0 处
```

复现输出留在 `evidence-t11/*.out.txt`，下文逐条引用。

---

## 2. 七个维度逐条结论

| # | 维度 | 结论 | 依据 |
| --- | --- | --- | --- |
| 1 | 凭据（0600 / hasCredentials / 日志·审计·错误 / 备份） | **FAIL**（B-1） | 0600 创建·自愈·备份、DTO 无凭据字段、审计无请求体均 PASS；但 `dry_run/diff` 响应带出密码 |
| 2 | fail-open（不可达仍直连 / fail-closed 阻断 / 默认值） | **FAIL**（H-1、M-1） | 三时点判定与 `:0` 动作齐备、默认 `true` 正确；应用级探测越权清设备级、清直连成功与否未验证、fail-closed 零测试（L-1） |
| 3 | 幂等与并发（重复 PUT / If-Match / flock / reconcile） | **PARTIAL**（M-3、M-6） | 重复 PUT `changed:false` 且 0 次 settings 写、412 语义正确、flock+原子写覆盖 egress.toml；reconcile 两文件非一致快照、settings 在锁外写、幂等重放不修 settings 漂移 |
| 4 | 清除路径（严格 `:0` / 无 delete / 卸载） | **PASS**（L-7 附带） | 唯一 `:0` 实现（源码级测试 `clearing_the_system_proxy_has_a_single_implementation`）、四条路径共用 `SystemProxy::clear`、`uninstall.sh` 停服后 `:0`+清空排除列表、脏状态先写 `:0` |
| 5 | SELinux / 权限上报 | **PARTIAL**（M-4） | 非零退出 → `503 upstream_unavailable{component:settings}` ✓、写前读回交叉验证 ✓；但请求路径没有 §3.6 要求的日志一行、stderr 被丢弃、0 测试 |
| 6 | 注入安全（失败不阻断 / 零回归 / 日志不泄密） | **PASS**（M-5、L-4 附带） | `proxy=None` 首分支 return（0 次 JNI）、逐项收错 + 最外层 catch-unwind、未配置进程零 hook/零线程/零映射（t18 CASE D：module log delta 0、`[egress]` 0 行、maps 按名 0 命中）；armed-never-fired 不可观测见 M-5 |
| 7 | 测试覆盖（可失败的测试 / 假绿） | **PARTIAL**（L-1、M-2、M-4） | 260 条全绿，且多数约束有负向用例；但 6 条硬规则 0 覆盖：fail-closed、settings 失败、`dry_run/diff` 凭据、app-scope fail-open、clear 失败时的 `effectiveMode`、dumpsys 真机格式 |

---

## 3. 契约条款逐条核对

### 3.1 §1.9 幂等矩阵 —— 符合

| 端点 | 契约要求 | 实现 / 证据 |
| --- | --- | --- |
| `GET /egress`、`GET /egress/proxies` | 只读 | 只读（`egress/store.rs` 共享 flock），无写路径 |
| `PUT /egress` | 条件幂等：首次 `changed:true`；新 revision 重放同内容 `changed:false`；旧 revision `412` | `api/egress.rs:661-663`（文本比较）+ `:666` 不写盘；`same_value_put_does_not_write_settings_twice`（`egress_api.rs:553-606`）断言 0 次 `settings put` 且 revision 不变；412 见 `:581-591` 与 `if_match_is_required...`（`:461-523`） |
| `PUT /egress?dry_run=true` | 恒 200、不改状态 | `:666` 与 `:703-704` 双重 `!dry_run` 守卫；文件/settings/备份都不动。**但该路径的响应 diff 带密码（B-1），且 0 测试** |
| `DELETE /egress` | 条件幂等 + `confirm` | `:774` `params.confirm(\"egress_clear\")`；`delete_clears_system_proxy_with_colon_zero`（`:304`）；已是直连且无变更时跳过 settings（`:884-894`） |
| `POST /egress/probe` | **非幂等**、不走幂等缓存 | `probe_egress` 不在 write 组；`put_replays_through_idempotency_cache_but_probe_never_does`（`:1242`） |

结论：矩阵本身**符合**。唯一的语义缺口是 M-6（文件未变时完全跳过 settings 读回，幂等重放修不了 settings 漂移），这不违反 §1.9 的字面（文件确实没写），但会让「重复 PUT」在设备侧失去自愈能力。

### 3.2 §3.5 fail-open —— **不符合（H-1、M-1）**

| 契约点 | 实现 | 判定 |
| --- | --- | --- |
| 默认 `fail_open = true` | `ProbeSettings::default`（`file.rs:116-124`）+ `[probe].fail_open` 解析（`:589-593`） | ✅ |
| 判定只有三个时点 | ① `probe_egress` `:1054`；② PUT 读回不一致 `:720-728`；③ `reconcile_once` `state.rs:242-252` | ✅ |
| 动作 = 清直连 + 保留配置 + health 字段 | `state.rs:154-165` + `state.rs:77-84`（since 只在首次写） | ⚠️ 动作未校验是否真的清成功（M-1） |
| `effectiveMode` = 计入 fail-open 后的**实际**模式 | `api/egress.rs:325-329` 只按 `health.degraded && fail_open` 推导 | ❌ 清直连失败时仍报 `direct`（M-1） |
| `reason ∈ {http_status, invalid_response}` 不触发 | `probe.rs:243-248` + 测试 `:963-997` | ✅ |
| fail-closed 判定照做但**不动 settings** | `:720-728`、`:1054-1063` | ✅（代码）／0 测试（L-1） |
| 恢复：成功探测 → 重写期望 → 读回校验 → `degraded:false` | `api/egress.rs:996-1001`：`let _ = apply(...)` 后**无条件** `recover()` | ❌ 未做读回校验（M-1） |
| 判定粒度为「代理不可用」 | 任意 `scope` 的探测失败都清**设备级**代理 | ❌ 应用级探测越权（H-1） |

### 3.3 §2.2 权限与原子写 —— 符合（两处轻微偏差见 L-3 / L-7）

| 契约要求 | 实现 / 证据 |
| --- | --- |
| 文件 `0600`、目录 `0700` | `EGRESS_FILE_MODE`（`mod.rs:72`）；真机 `-rw------- 1 root root`（`evidence/t1-put-http-proxy.log:117`）、`drwx------`（`evidence/t6-uninstall.log:26`） |
| `O_CREAT\|O_EXCL` 一步 0600 | `core::revision::atomic_write`（`revision.rs:214-245`）：`create_new(true).mode(LOCK_FILE_MODE)` + 显式 `set_mode(temp, 0600)` 后再 rename；无「先 0644 后 chmod」窗口 |
| 落盘 `write→fsync→rename→fsync(dir)` | 同上（`:230-237`） |
| 读时自愈 + warning | `file.rs:343` `heal_mode` → `api/egress.rs:84-93` 给 `egress_perm_fixed`；零宽模式测试 `egress_toml_is_0600_self_healing_with_0600_backups`（`:609`） |
| 备份 0600 | `create_backup` 复制后 `set_mode(BACKUP_FILE_MODE)`；真机 `-rw-------`（`evidence/t9-final-state.log:29-30`）；测试同上 |
| 导出隔离 | 仅 egress 端点读 `egress.toml`；`GET /config?format=toml` 只读 config.toml（无凭据键，`egress_section_issues` 把凭据键升级为 422） |
| 卸载 `:0` | `module/uninstall.sh:43-46`（停服 → `:0` + 清空排除列表 → `rm -rf`） |

偏差：① 备份文件实际叫 `egress.toml.<ts>-<rev8>.bak`，契约写的是 `egress-<ts>-<rev8>.toml`（L-3）；② 请求路径的自愈只给响应 warning，没有契约要求的「日志 warn」（L-7，只有 reconcile 打日志）。

### 3.4 §1.2 egress revision —— 符合

- 公式：`egress_revision(config_revision, bytes)`（`mod.rs:81-87`）= sha256(configRevisionHex ‖ `\n` ‖ bytes)；`egress_api.rs:221-228` 用真实文件字节独立重算并断言相等，另断言换行/前缀任一变化都会改变 revision。
- 只发 `X-DM-Egress-Revision`：`api/mod.rs:95-100`；测试断言 `x-dm-revision` 与 `etag` 都不存在（`egress_api.rs:212-214`）。
- `If-Match` 容错：`store.rs:103-132`（引号、`sha256:` 前缀、`*`）；缺失 → 428（`:775-776`），不匹配 → 412 + `details{expected, actual, resource:\"egress\"}`（`error.rs:315-330`）。
- 损坏文件修复通道：`load_situation` fail-closed 500 + `details.op=egress_parse`（`api/egress.rs:74-81`），`If-Match: *` 以空文档为基线重写（`:593-598`）；`broken_egress_toml_fails_closed_and_is_repairable`（`:1203`）。

---

## 4. t14 修复的反检测代价评估（是否可接受 / 是否已在文档声明）

**结论：可接受，且已在四处文档声明；但有一个「付了代价没拿到收益」的缺口（M-5）。**

现状（代码）：

- 只有**显式**写了 `[[apps]].egress` 的进程才会走到 `arm_egress_replay`：`resolve_for_package` 对
  「键不存在 / 空串 / 未定义代理名」一律返回 `None`（`src/egress.rs:293-321`），`apply_egress_proxy`
  首分支 `let proxy = proxy?;` 直接返回（`src/hooks.rs:291`）。
- `EGRESS_REPLAY_ACTIVE` 只在 arm 进入 `Proceed` 后置位（`src/hooks.rs:658`），三条降级路径统一由
  `replay_unavailable` 按 `ArmOutcome::keeps_module_mapped()` 复位（`:731-740`），因此 arm 失败的进程
  仍会 `dlclose`（`src/lib.rs:374-380`、`:124-127`）。
- 代价：armed 进程在**整个进程生命周期**内不能 `dlclose`（重放线程 + PLT hook 的指令都在 .so 里），
  且 `setpriority` / `__system_property_get` 两个 PLT 跳转常驻。

声明核对（四处一致，无「只在设计文档里」的情况）：

| 位置 | 内容 |
| --- | --- |
| `docs/CONFIG.md:346` | 显式配置 egress 的进程保留模块代码页；**按名字扫 `/proc/<pid>/maps` 看不到**（Zygisk 匿名映射，实测 3170 行命中 0）；真判断要比 `r-xp` 匿名段；未配置进程照旧立即卸载 |
| `docs/SERVER.md:332` | 同上（故障排查表） |
| `docs/en/CONFIG.md:329` | 英文同口径 |
| `docs/CHANGELOG.md:20` ① | 同上（发布说明） |

评估意见：

1. 用「匿名可执行段」而不是文件名作为可检出判据是正确结论；把它写进用户文档是对的。
2. `__system_property_get` 是启动期最热的符号之一，装 hook 本身是额外可探测面；建议在文档里补一句
   「armed 进程多两个 PLT 跳转（开销在纳秒级，但可被测量）」，与现有「可检出性」段落并列，避免读者
   以为代价只有内存。
3. 未配置应用的零回归有真机证据（t18 `evidence-t18/t18-d-zero-injection.log`）：module log 行数 delta=0、
   该包 `[egress]` 0 行、maps 按名 0 命中——满足本任务「CASE C/D 零 hook、零线程、零 .so 保留」的要求。
4. 缺口：armed 之后两个 hook **都没被调用**时，进程既没有重放、也没有日志，却永久保留模块映射与
   `REPLAY_PENDING` 里的 arming（见 M-5）。这与「只有拿到了收益才付代价」的设计意图相反。

---

## 5. 重放机制鲁棒性

| 检查点 | 结论 | 依据 |
| --- | --- | --- |
| 0–2000ms 窗口、不早退 | ✅ | `replay_offsets_ms()`（`src/egress.rs:608-620`）：0..=160 每 2ms、165..=300 每 5ms、340 起 ×5/4+10、末点 2000，共 118 点；`replay_schedule_is_sorted_dense_early_and_covers_window`（`:1334`）；真机 `probes=118 elapsed=2000ms`（`t18-acceptance.md:78`） |
| 被覆盖后「整套重写」而非只补一个键 | ✅ | `run_replay_loop`（`:670-699`）在任一偏差出现时遍历全部 expectations；`replay_loop_restores_direct_after_framework_writes_device_proxy`（`:1414`） |
| 第一次修复后继续观察（防二次覆盖） | ✅ | 循环不 break；`replay_loop_keeps_watching_after_the_first_repair`（`:1448`） |
| 重复 spawn | ✅ 不会 | `REPLAY_SPAWNED` 先查后置位 + `REPLAY_PENDING.take()`（`src/hooks.rs:838-849`），只有一个线程能拿到 arming；真机每进程只有一行 `replay start` |
| 线程泄漏 | ✅ 线程 ≤2s 后退出（detached，JoinHandle 丢弃）；`REPLAY_PENDING` 在正常路径被取空 |
| arm 期降级（镜像缺失 / commit 失败 / 排障开关） | ✅ 都有 `replay hook unavailable reason=...` + 宿主单测（`src/egress.rs:1493`、`:1514`、`:1535`；t18 `strings` 证据 `t18-acceptance.md:164-167`） |
| **runtime 未命中（armed 但符号没被调用）** | ❌ 无任何记录、代价白付 | 见 M-5 |
| attach 失败 | ✅ 退避 10 次（约 100ms）后放弃并记 `replay failed step=attach`（debug） | `src/hooks.rs:892-926` |
| 凭据载体在日志里 | ✅ 只输出 `set:true/false` | `Divergence::describe`（`src/egress.rs:553-563`）+ `secret_divergence_never_prints_the_value`（`:1321`） |

---

## 6. `__DIRECT__` / socks5 × 设备级 http 的交叉场景

**结论：互不污染，清空与设置都在同一份重放集合里。**

| 场景 | 计划（`injection_plan`，`src/egress.rs:389-478`） | 重放（`replay_expectations`，`:506-538`） | 真机 |
| --- | --- | --- | --- |
| 应用级 http | Set http/https 四项 + **Clear** socks 两项 | 4 个 must-equal + 2 个 must-absent | CASE A：`http.proxyPort=18082`，fixture `listener=18082` |
| 应用级 socks5 | Set socks 两项 + **Clear** http/https 四项（+ 凭据载体） | 2 个 must-equal + 4 个 must-absent（凭据两项标 secret） | CASE B：`http/https=<null>`、`socksProxyHost=127.0.0.1:18081`、`listener=18081` |
| `__DIRECT__` | **零 Set** + Clear 六项 + direct selector | 6 个 must-absent | CASE C：六项全 null、selector DIRECT、`app.fixture` 命中 0 |

要点：因为 `Clear` 也会变成 `must-absent` 期望，框架按设备级 `ProxyInfo` 写回来的 `http.proxyHost`
会在下一次 tick 被清掉——这正是 F2（`__DIRECT__` 拉不回直连）的修复点，`replay_loop_restores_direct_after_framework_writes_device_proxy`
覆盖了这条时序。

一处建议（非缺陷）：`java.net.socks.username/password` 只在 socks5 计划里写入，`__DIRECT__`/http 计划
不清除它们。跨进程没有影响（新进程干净），但同一进程内把配置从 socks5 换成 direct 需要重启应用——
这与既有「改配置需重启应用」一致，建议在 `docs/CONFIG.md` 的「切换 `[[apps]].egress` 类型需重启应用」
处点一句，避免被当成 bug 报告。

---

## 7. Findings

> 每条给「文件:行 / 问题 / 必须的修复」。severity 与最终 verdict 一致：存在 blocker/high → `needs_revision`。

### 7.1 blocker

#### B-1 `PUT /egress?dry_run=true` / `?diff=true` 的 egress.toml diff 原样返回明文密码

- **文件:行**
  - `server/src/api/egress.rs:508-521`（`combined_diff`：对 egress.toml 也做 unified_diff 并拼接）
  - `server/src/api/egress.rs:733-742`（`if want_diff || dry_run` → 把上面这段放进响应 `diff`）
  - `server/src/diff.rs:55-75`（上下文行与 `-`/`+` 行**逐字**输出，不做任何脱敏）
  - `server/src/egress/file.rs:626-633`（`render` 把 `password = \"...\"` 写进 egress.toml 文本）
- **问题**：`diff` 字段是 egress.toml 的原文 diff，因此响应里会出现 `password = \"s3cret\"`。
  两条真实路径：
  1. **不需要知道密码的读回**：已存 `alice/s3cret` 的代理保持不动，只发 `PUT /egress?dry_run=true`
     把 `probeUrl` 改个值——diff 的 3 行上下文就会带出最后一条代理的 `password`（`repro` 场景 A）；
  2. **轮换密码**：`-password = \"s3cret\"` / `+password = \"n3wpass\"` 同时出现（场景 B）。
  违反契约 §7.1 第 1/3 条（响应里没有 password、错误/响应不得回显请求体）与 §2.2 表格
  「不进任何 API 明文」；也让「任何带 write 作用域的 Token 都能读回已存凭据」，凭据边界失效。
- **必须的修复**：
  1. 对 egress 段做**行级脱敏**后再返回：把 `^[ +\\-](username|password)\\s*=` 的值替换为 `<redacted>`
     （保留键名与行结构，diff 仍可读）；或更彻底：`diff` 只返回 config.toml 段，egress 段改为字段级
     摘要（`+proxies[us_la_01] hasCredentials=true`）。
  2. 加两条**可失败**的测试（当前 `diff` 在整个 `egress_api.rs` 里 0 次出现）：
     - 已存密码 + 只改 `probeUrl` + `dry_run=true` → 响应体 `!contains(\"s3cret\")`；
     - `diff=true` 轮换密码 → 响应体不含旧值也不含新值。
  3. 顺手在 `docs/API.md` §5.26 的 `diff` 参数说明里写清「egress.toml 段已脱敏」。
- **证据**：`docs/design/egress/evidence-t11/repro-diff-credential.rs` + `.out.txt`（直接 `#[path]` 引用
  真实 `server/src/diff.rs` 运行；输入文本形态由 `file.rs` 自身测试 `render_round_trips_and_preserves_unknown_keys`
  断言过）。输出：

  ```
  === 场景 A: 只改 probeUrl（dry_run 预览） ===
  @@ -8,6 +8,6 @@
   password = \"s3cret\"            <-- 已存密码，调用方从未提交过它
   [probe]
  -url = \"http://checkip.amazonaws.com/\"
  +url = \"http://checkip.example.com/\"
  -> 泄漏 s3cret: true
  ```

### 7.2 high

#### H-1 应用级（`package`）探测失败会清掉**设备级**系统代理

- **文件:行**：`server/src/api/egress.rs:1053-1064`（`failure.triggers_fail_open()` 之后不看 `scope`）；
  `scope` 在 `:382` 计算、`:1034` 只用于 `approximate` 标注。
- **问题**：`POST /egress/probe {package: \"com.example.a\"}` 用的可能是**只有该应用**使用的代理定义；
  一旦 `connect/dns/timeout/tls` 失败，代码调用 `degrade_to_direct` → `settings put global http_proxy :0`，
  把**整机**设备级出口切成直连，并把 `health.degraded` 置真（`GET /egress` 的 `effectiveMode` 变成
  `direct`）。对「出口 IP 一致性」场景这是最坏的失败方式：一个应用的代理挂了 → 其它所有应用的
  真实 IP 暴露；而该应用的注入是进程内的，清设备级代理对它本身毫无帮助。
  契约 §3.5 只定义了「代理不可用 → 清设备级」，没有区分探测粒度；但 §5.2/§5.4 明确「应用级只影响
  目标进程、同机其它应用零影响」，本实现让应用级探测产生了跨应用副作用。可达性：契约 §6.12 的
  job 步骤 5 就是调用 `POST /api/v1/egress/probe`（其 warning details 为 `{deviceId, app, reason}`，
  即应用级探测是被支持的调用形态），任何一次应用级探测都会触发这条路径。
- **必须的修复**：`degrade_to_direct` 只允许在 `scope == \"device\"`（且 channel 来自 `[egress].default`）
  时调用；`scope == \"app\"` 的失败只写 `health.lastProbe` + 响应 warning（例如复用
  `egress_degraded_to_direct` 但 `details.scope=\"app\"`，或新增 `egress_app_probe_failed`）。
  加一条测试：设备级 default + 应用级 socks5（不可达端口），`probe {package}` 返回 502 后
  `settings put global http_proxy` 的调用次数必须为 0、`health.degraded` 必须为 false。
  若产品决定保留现行为，则必须先在契约 §3.5 里把粒度写清并在 `CONFIG.md` 明确告知——但我不建议，
  因为「一个应用的代理故障 → 整机回真实 IP」与 fail-open 的初衷相反。

### 7.3 medium

#### M-1 fail-open 的「已清直连」判定与读回脱钩；恢复路径也不校验读回

- **文件:行**：`server/src/egress/state.rs:154-165`（`let cleared = system.clear().is_ok();`）、
  `server/src/egress/system.rs:200-246`（`apply`/`clear` 即使读回不一致也返回 `Ok`）、
  `server/src/api/egress.rs:325-329`（`effectiveMode` 只看 `health.degraded && fail_open`）、
  `server/src/api/egress.rs:996-1001`（探测成功后 `let _ = apply(...)`，随后**无条件** `recover()`）。
- **问题**：`SystemProxy::clear()` 的返回值只表示命令退出码为 0；`ApplyOutcome.matches` 才是「真的清了」。
  `degrade_to_direct` 用 `is_ok()`，于是「put 返回 0 但没生效」（SELinux 静默拒绝、VPN/其它组件立刻
  写回、settings 服务异常）时仍然：日志 `cleared=true`、`health.degraded=true`、
  `effectiveMode=\"direct\"`——而设备实际还在走那个已经不可用的代理。恢复路径同理：`apply` 失败被
  `let _ =` 丢掉，`degraded` 却被清空，`effectiveMode` 回到 `proxy`。
- **必须的修复**：`degrade_to_direct` 以 `outcome.matches` 作为成功判据：
  - `matches==true` → 置 `degraded`、`effectiveMode=\"direct\"`；
  - `matches==false` → `degradedReason=\"settings_unavailable\"`（或保留原 reason + 一个
    `applied:false` 的真实值），`effectiveMode` 必须报**实际**模式（读回仍是 `h:p` 就是 `proxy`），并在
    日志里给出 `cleared=false`；
  - 恢复路径：`apply` 之后必须 `readback` 校验，`matches==true` 才 `recover()`。
  加两条测试：`stubborn_with_proxy` 驱动 PUT/probe 失败 → `effectiveMode` 仍为 `proxy`、`applied:false`；
  探测成功但 `apply` 不生效 → `health.degraded` 仍为 true。
- **证据**：`evidence-t11/repro-failopen-cleared.rs` + `.out.txt`（真实 `system.rs` + 「put 返回 0 但不生效」
  的 runner）输出：

  ```
  clear() 的返回值: Ok(ApplyOutcome { write_count: 1, matches: false })
  degrade_to_direct 只看 is_ok() -> cleared = true
  而读回仍然是: Some(\"127.0.0.1:18080\")
  ```

#### M-2 dumpsys 交叉验证在真机上恒为 `Unknown`，「dumpsys 说仍有代理」判不出来

- **文件:行**：`server/src/egress/system.rs:296-321`（`parse_dumpsys_state` 只接受 `] port: N`）、
  `server/src/egress/system.rs:257-278`（`matches` 依赖它）；测试用的是契约里的假格式
  `server/tests/egress_api.rs:170-178`（`Global proxy: [{host}] port: {port}`）。
- **问题**：Pixel 9 / Android 17 的真机输出是 `Global proxy: [127.0.0.1] 18080 xl=`
  （`evidence/t1-put-http-proxy.log:75`、`evidence/t4-reconcile.log:27`），即 `ProxyInfo.toString()` 形态，
  **没有 `port:` 前缀**。`tail.trim().strip_prefix(\"port:\")` 必然失败 → 恒 `Unknown`。真值表第 4 行
  「dumpsys 说有代理但主值是 null」因此永远不会被判为不一致（`matches(Expected::Direct)` 里
  `!matches!(Unknown, Proxy)` 为真）；只有 `global_http_proxy_host` 残留这一条兜底。
  单测覆盖的是一个真机不会出现的格式 —— 典型「假绿」。
- **必须的修复**：同时接受 `[h] <port>` 与 `[h] port: <port>`（允许尾部属性，如 `xl=`）；
  `find_global_proxy_line` 收紧为 `Global proxy:` 前缀匹配，避免命中 exclude/PAC 相关行；
  用真机原文（`Global proxy: [127.0.0.1] 18080 xl=`）加一条测试，断言解析出 `Proxy{127.0.0.1,18080}`，
  并加一条「settings `:0` + dumpsys 有代理 → `matches(Direct)==false`」的用例。
- **证据**：`evidence-t11/repro-dumpsys-parse.rs` + `.out.txt`（`#[path]` 引用真实 `system.rs`）：

  ```
  Pixel 9 / Android 17 真机（evidence/t1-put-http-proxy.log:75）
    input  = Global proxy: [127.0.0.1] 18080 xl=
    parsed = Unknown
  matches(Expected::Direct) = true   // 真代理仍在生效，却判为「已直连」
  ```

#### M-3 启动 reconcile 读两个文件不是一致快照；settings 写在锁外（并发 PUT 可把 settings 写成旧值）

- **文件:行**：`server/src/egress/state.rs:170-226`（先 `store::read_snapshot` 读 config、再
  `service.store().read()` 读 egress，两次各自加锁、中间无共同快照）；
  `server/src/api/egress.rs:703-704`（`drop(egress_lock); drop(config_lock);`）→ `:714-717` 才 apply；
  DELETE 同理 `:881-893`。
- **问题**：两个方向都能踩：
  1. reconcile 读到旧 config（`default=A`）后，PUT 写入新 egress+config（`default=B`）并 apply，
     随后 reconcile 用「旧 default + 新 egress」算出期望 A 并写到 settings → 设备最终停在 A，
     而 config 是 B；下次启动前没有任何东西会修正（自愈只在启动跑一次）；
  2. 两个并发 PUT 都在释放锁之后才 apply settings，后者先写、前者后写 → settings 停在先释放者的值，
     与最终文件不一致。
- **必须的修复**：reconcile 用与写路径**相同的锁序**取一致快照（config 共享 → egress 共享，或干脆复用
  写路径的「config 排他 → egress 排他」）；PUT/DELETE 把 settings 应用放在锁内（settings 调用是毫秒级，
  可接受），或应用后重算 revision，发现变化则再应用一次（有界重试）。加一条确定性测试：在 reconcile
  读 egress 的瞬间注入一次 PUT（可用 fixture runner 的回调），断言最终 settings 与最终 config 一致。

#### M-4 settings 失败没有 §3.6 要求的日志行；stderr 被丢弃；`settings` 不可用时 GET 无任何 warning

- **文件:行**：`server/src/egress/system.rs:148-154`（`CommandOutcome::Failed { code, .. }` 丢掉 `stderr`）、
  `server/src/api/egress.rs:714-717` 与 `:890-893`（映射 503，但不打日志）、
  `server/src/api/egress.rs:298`（`readback().unwrap_or_else(|_| empty_readback())` 静默降级）、
  `server/src/api/egress.rs:306-315`（只在 `readback.available` 时给 `egress_settings_mismatch`）。
- **问题**：契约 §3.6 对「SELinux 拒绝（avc: denied）」的要求是「非零退出 → 503 + **日志一行**；
  `applied=false`」。现在：503 有了，日志没有；`stderr`（唯一能区分「SELinux 拒绝 / settings 不存在 /
  服务异常」的信息）被丢在 `CommandOutcome` 里；GET 在 settings 不可用时只报 `applied:false`，
  连一条 warning 都没有，客户端无法区分「漂移」和「settings 坏了」。
- **必须的修复**：`SettingsError::Failed { code, stderr_excerpt }` 保留**截断**的 stderr（≤256 字符，
  settings 的参数里没有凭据，安全），并在 PUT/DELETE/probe 的失败分支 `tracing::warn!(component=\"settings\",
  code, stderr)`（每次请求一行，不做全局去重）；`readback` 失败时 GET 加 warning
  （复用 `egress_settings_mismatch` + `details.reason=\"settings_unavailable\"` 或按契约走 warning 表）。
  加两条测试：`FixtureRunner::with_failure(\"settings put global http_proxy\", 1)` → 503 且 capture 到一行 warn；
  空 runner（`Unavailable`）→ 503 且文件已写入。

#### M-5 armed 但 hook 从未命中：无日志、无收尾，模块映射与 arming 永久保留

- **文件:行**：`src/hooks.rs:619-725`（arm 只处理「开关 / 镜像缺失 / commit 失败」三种失败）、
  `src/hooks.rs:838-882`（重放线程**只在 hook 命中时**才 spawn）、`src/hooks.rs:593-595`（`REPLAY_PENDING`/
  `REPLAY_SPAWNED`）、`docs/CHANGELOG.md:20` ③（「`setpriority` hook 未命中时退化为一次性注入（`replay hook unavailable`）」）。
- **问题**：`plt_hook_commit()` 成功但两个符号都没被调用（平台改了调用点、或该进程根本没走到
  `SpecializeCommon` 的 setpriority）时：没有 `replay hook unavailable`、没有 `replay start`、
  没有 `replay done`；`EGRESS_REPLAY_ACTIVE` 保持 true → 模块**永久**不 `dlclose`，
  `REPLAY_PENDING` 里的 arming（含 JavaVM 与凭据期望）常驻，而应用级出口静默失效。
  debug 模式下唯一能看到的是 `replay hook armed`，看起来像成功。
  与 CHANGELOG ③ 的措辞（「未命中 → 记一行」）不一致；`docs/CONFIG.md:348` 只承诺了
  「镜像缺失 / commit 失败」两种情况记日志，所以文档本身没错，但**行为缺口**真实存在。
- **必须的修复**：在 `post_app_specialize`（`src/lib.rs:118-128`，此时 hook 应已命中）做一次自检：
  `if EGRESS_REPLAY_ACTIVE && !REPLAY_SPAWNED { warn!(\"[egress] replay hook never fired package=…\"); }`
  并按 `keeps_module_mapped()==false` 复位 `EGRESS_REPLAY_ACTIVE`、清空 `REPLAY_PENDING`（这样
  「没拿到收益」的进程照旧 `dlclose`）。若实测该时刻早于 `setpriority`，则把自检放到第一个可安全
  执行的后续回调，并在 `CONFIG.md` 写明「armed 但未 fire」的诊断行。加一条宿主单测（纯逻辑已具备
  `plan_replay_arm`/`finish_replay_arm` 的形态，可扩一个 `ArmOutcome::NeverFired`）。

#### M-6 文件未变化时 PUT 完全跳过 settings 读回 → 幂等重放修不了 settings 漂移

- **文件:行**：`server/src/api/egress.rs:706`（`if changed && !dry_run { … apply … }`）；
  对应测试 `server/tests/egress_api.rs:553-606`（断言重放时 `settings put` 次数为 0）。
- **问题**：契约 §3.4 第 1 条要求「写前先读：读回 == 期望 → 不写 settings」，而不是「文件没变就
  连读都不读」。现在的实现让「重复 PUT」失去自愈能力：如果 settings 在两次 PUT 之间被重置
  （用户手工 `:0`、其它工具、VPN 切换、system_server 重启），Manager 的 apply job 重放同一份
  body 会得到 `changed:false` + `applied:false`，但 settings 不会被修回；而 `applied:false` 在
  `changed:false` 的响应里很容易被当成「无需处理」。
- **必须的修复**：把 settings 的「读回 + 必要时补写」从 `changed` 条件里拆出来——文件不写时仍执行
  `apply(expected, None)`（`write_count` 为 0 时不写 settings），响应保持 `changed:false`、revision 不变；
  加测试：PUT → `sim.set_proxy(Some(\":0\"))` → 同 body 重放 → settings 回到 `h:p`、`changed:false`、
  `applied:true`。

### 7.4 low

#### L-1 fail-closed（`failOpen=false`）整条路径零测试

- **文件:行**：`server/src/api/egress.rs:720-728`、`:1054-1063`；测试目录里只有请求体解析用到了
  `failOpen:false`（`server/tests/egress_api.rs:1534`，且是别的用例的输入）。
- **问题**：这是「宁断网也不换 IP」的安全选项，一旦回归成 fail-open 就是真实 IP 泄漏，但目前没有任何
  可失败测试锁住它。
- **必须的修复**：补两个用例——① `failOpen:false` + PUT 后读回不一致（`stubborn_with_proxy`）→
  `settings put global http_proxy` 调用次数为 0、`health.degraded=true`、`effectiveMode` 保持 `proxy`；
  ② `failOpen:false` + 探测 `connect` 失败 → 502、settings 不动、`degraded=true`。

#### L-2 `apps[].effective` 对「引用了未定义代理名」报 `proxy` 且 `proxy:null`

- **文件:行**：`server/src/api/egress.rs:241-247`（`exists = model.proxy(value).is_some();` 但
  `effective` 恒为 `\"proxy\"`）。
- **问题**：契约 §2.4 对未定义名字的语义是「视为未配置」（运行期继承设备级 / 直连），模块侧也确实是
  「不注入」（`src/egress.rs:306-321`）。视图层却报 `effective:\"proxy\", proxy:null`，前端/Manager
  只能靠 `egress_proxy_not_found` 警告自行推断。该警告码与 `effective` 字段目前**都没有测试**。
- **必须的修复**：名字未定义时按「无覆盖」计算视图（`effective` 跟随设备级 `mode`，`proxy` 给设备级
  默认名或 null），并补 `egress_proxy_not_found` + `effective` 的断言。

#### L-3 `backupId` 与备份文件命名偏离契约 §2.2

- **文件:行**：`server/src/egress/file.rs:704-709`（`backup_id_from`）与 `server/src/api/egress.rs:523-528`
  （`backup_id_of`，两份重复实现）都用**第一个** `.` 切分。
- **问题**：文件实际名是 `egress.toml.<ts>-<rev8>.bak`（`docs/design/egress/evidence/t9-final-state.log:29-30`），
  切出来的 `backupId` 是 `toml.<ts>-<rev8>`；真机响应已经能看到这个形态
  （`docs/design/egress/t9-acceptance.md:80`：`\"backupId\": \"toml.20261124T131637…\"`），而契约 §1.4/§2.2
  要求是 `<时间>-<revision 前 8 位>`、文件名为 `egress-<ts>-<rev8>.toml`。除了字段形态，实际后果是
  「按 `backups/egress-*.toml` 排查是否残留凭据」的脚本会**一个也找不到**（文件是 `.bak`）。
- **必须的修复**：用 `rsplit_once` / 先剥 `.bak` 再取 `<file>.` 之后的部分，让 `backupId` 符合契约；
  决定并写清备份命名（改成契约形态，或保留 `.bak` 但在契约与 `docs/SERVER.md` 的排查命令里给出正确
  glob），并加一条断言 `backupId` 正则的测试。

#### L-4 模块 `EgressProxy` 的 `Debug` 没有遮蔽 username

- **文件:行**：`src/egress.rs:207-218`（`password` 打码，`username` 原样）；对照服务端
  `server/src/egress/file.rs:91-103`（两者都打码）。
- **问题**：契约 §7.1 第 5 条明确「不记录代理密码/**用户名**」。当前没有任何日志路径打印 `EgressProxy`
  的 `Debug`（只有测试 `src/egress.rs:1160` 用），所以是潜在风险而非现行泄漏；但服务端已经做了，
  模块侧不一致容易被后人误用。
- **必须的修复**：`username` 也 `.map(|_| \"<redacted>\")`，并把 `debug_output_redacts_password`
  扩成同时断言 `username` 不出现。

#### L-5 配置解析错误用 `{error:#}`，可能回显 config.toml 出错行原文

- **文件:行**：`server/src/api/egress.rs:68-70`（GET/PUT/DELETE 的 `load_situation`）、`:606-608`、`:634-636`。
- **问题**：`anyhow` 里的 `toml::de::Error` 的 `Display` 会带上出错行的原文（模块侧注释已经写明这一点，
  并因此改用 `message()`：`src/lib.rs:447-455`）。如果用户把凭据粘进 config.toml 且该行语法坏了
  （例如未闭合字符串），`internal_error.message` 会把这一行带出来。契约 §7.1 第 3 条要求错误响应
  不回显请求体。
- **必须的修复**：与模块侧统一——只取 `err.downcast_ref::<toml::de::Error>().map(|e| e.message())`，
  拿不到时给通用文案；加一条「config.toml 含坏行凭据 → 响应不含该串」的测试。

#### L-6 新增警告码 `egress_auth_unsupported` 没进契约 §1.8 / `docs/API.md`

- **文件:行**：`server/src/api/egress.rs:182`、`:194-229`（`auth_warning`/`auth_warnings`，在 GET/PUT/DELETE
  与 probe 响应里都会出现）；`docs/API.md:1092` 的 warnings 清单没有它，契约 §1.8 的 8 条警告码表也没有。
- **问题**：契约把警告码表冻结为 8 条，实现里增加到 9 条而没有任何契约/文档更新——客户端（尤其 Manager）
  只能当作未知码透传；`docs/CONFIG.md:345` 讲了这个限制，但**码本身**没登记。
- **必须的修复**：在契约 §1.8 与 `docs/API.md:1092` 补一行（code / details 形状 / 触发），或改用一个
  已登记的码（语义不同，不建议）；至少补一条断言「码出现在响应里」的测试（现有测试已断言，
  缺的是契约/文档同步）。

#### L-7 权限自愈在请求路径只有响应 warning，没有 §2.2 要求的日志 warn

- **文件:行**：`server/src/egress/file.rs:343`（`heal_mode`）、`server/src/api/egress.rs:84-93`（只 push warning）、
  `server/src/egress/state.rs:208-213`（只有 reconcile 打日志）。
- **问题**：契约 §2.2 的读时自愈要求「chmod 0600 **+ 日志 warn** + 响应 warning `egress_perm_fixed`」；
  现在 GET/PUT/DELETE 触发的自愈只有响应里的一条 warning，设备日志里没有任何痕迹（`logs/*.log`
  排查时看不到「权限被人改过」）。
- **必须的修复**：在 `read_locked` 的自愈分支加 `tracing::warn!(path, mode, \"egress.toml 权限过宽，已自愈为 0600\")`
  （与 reconcile 那条同文案即可，重复无妨）；或把 reconcile 的日志上移到 `heal_mode` 并去掉重复。

---

## 8. PASS 项的核对证据（无发现的部分）

| 核对项 | 命令 / 测试 | 结果 |
| --- | --- | --- |
| DTO 无凭据字段 | `grep -n password server/src/egress/dto.rs`；`ProxySummary` 固定 5 键（`dto.rs:16-44`） | 0 命中；GET/PUT 响应无 `password`/`username` 字段 |
| 响应不带凭据 | `put_applies_system_proxy_and_get_reports_applied`（`egress_api.rs:243-249`：`!contains(\"s3cret\")`、`!contains(\"alice\")`、`!contains(\"password\")`） | PASS（**但不覆盖 `diff`/`dry_run`，见 B-1**） |
| `/egress/proxies` 脱敏 | `proxies_endpoint_is_masked_and_shows_references`（`:684`） | PASS |
| 校验错误不带值 | `validation_errors_never_echo_credentials`（`:1172`，长密码 → 只报「超过 256 字符」且不落盘） | PASS |
| 日志无凭据 | `tracing_never_formats_credentials`（`:1349`，源码级）+ `Debug` 手写脱敏（`file.rs:91-103`、`probe.rs:157-179`） | PASS |
| 审计无请求体 | `audit.rs:63-88`（字段固定，无 body）+ `write_semantics::audit_log_records_writes_without_secrets` + `egress_api.rs:297-300` | PASS |
| egress.toml 0600 / 自愈 / 备份 0600 | `egress_toml_is_0600_self_healing_with_0600_backups`（`:609`）+ 真机 `evidence/t1-put-http-proxy.log:117`、`evidence/t9-final-state.log:29` | PASS |
| config.toml 保持 0644、无凭据键 | `:289-294` + `egress_section_credential_keys_are_upgraded_to_422_unknown_field`（`:1473`） | PASS |
| 清除只用 `:0`、无 `settings delete` | `clearing_the_system_proxy_has_a_single_implementation`（`:1126`：源码扫描断言 + `:0` 唯一持有者必须是 `egress/system.rs`）+ `delete_clears_system_proxy_with_colon_zero`（`:304`） | PASS |
| 卸载清 `:0` | `evidence/t6-uninstall.log:3`（`Global proxy: none`）+ `module/uninstall.sh:41-46` | PASS（未做失败回读，见 §9 观察） |
| 真值表四行（settings 侧） | `truth_table_rows_are_evaluated_from_readback`（`:408`）+ 脏状态先写 `:0`（`system.rs:208-213`） | PASS（dumpsys 侧见 M-2） |
| If-Match：428 / 412 / `*` / 引号 / `sha256:` | `if_match_is_required_and_compared_against_egress_revision`（`:461`）+ `write_semantics::if_match_accepts_quoted_unquoted_prefixed_and_star` | PASS |
| 幂等：重复 PUT 不重复写、revision 不变 | `same_value_put_does_not_write_settings_twice`（`:554`） | PASS（settings 漂移见 M-6） |
| probe 非幂等 | `put_replays_through_idempotency_cache_but_probe_never_does`（`:1242`） | PASS |
| 探测：成功 / `connect` 触发 fail-open / `http_status` 不触发 | `probe_reports_exit_ip_and_requires_ops_scope`（`:779`）、`probe_failure_is_502_and_connect_failure_triggers_fail_open`（`:907`） | PASS |
| 启动自愈：解析失败不写 settings、不一致重写、一致不写 | `startup_reconcile_skips_on_parse_error_and_fixes_mismatch`（`:1042`） | PASS（并发一致性见 M-3） |
| 未配置应用零注入 / 未定义名字不注入 | 宿主单测 `only_explicit_app_level_egress_is_injected`（`src/egress.rs:961`）、`no_egress_configuration_means_no_injection`（`:1014`）、`undefined_proxy_name_is_not_injected`（`:1029`）+ 真机 `evidence-t18/t18-d-zero-injection.log` | PASS |
| 重放时间表 / 不早退 / secret 不打印 | `replay_schedule_is_sorted_dense_early_and_covers_window`（`:1334`）、`replay_loop_repairs_http_overwrite_at_95ms`、`secret_divergence_never_prints_the_value`（`:1321`） | PASS |
| arm 降级三路径 | `arm_plan_honours_the_debug_guardrail`（`:1468`）、`missing_libandroid_runtime_degrades_with_the_expected_log`（`:1493`）、`commit_failure_degrades_with_the_expected_log`（`:1514`）、`debug_switch_outcome_is_logged_and_does_not_keep_the_module`（`:1535`） | PASS |

---

## 9. 结论与下一步

**verdict: `needs_revision`**（存在 blocker B-1 与 high H-1，按评审约定不允许 pass）。

放行前必须完成：

1. **B-1**：egress 段 diff 脱敏 + 两条负向测试（这条不修，凭据边界这条硬约束就是破的）。
2. **H-1**：应用级探测失败不得改动设备级 settings（或先改契约并显式文档化，但我不建议保留现行为）。
3. **M-1 / M-2 / M-4**：都属于契约硬条款（§3.5 的 `effectiveMode` 语义、§3.3 的读回交叉验证、§3.6 的失败面日志），
   建议一并修；其中 M-2 只需改解析 + 一条真机格式测试，成本很低。
4. **M-3 / M-5 / M-6**：并发一致性与可观测性，可在同一轮修（M-6 会让 Manager 的 apply 真正具备自愈能力）。
5. **L-1**：fail-closed 的两条测试必须补——这是唯一会在回归时**泄漏真实 IP** 的开关。
6. L-2..L-7 可随修，其中 L-3（backupId）与 L-6（未登记警告码）会让文档/排查脚本对不上，建议同轮收口。

复验时建议直接引用以下新测试（名字即契约）：

- `dry_run_diff_never_echoes_stored_credentials`
- `app_scope_probe_failure_does_not_clear_device_proxy`
- `fail_open_reports_the_real_effective_mode_when_clear_is_ineffective`
- `dumpsys_global_proxy_line_from_android17_is_parsed`
- `settings_failure_is_503_with_a_log_line`
- `idempotent_put_repairs_settings_drift`
- `fail_closed_never_touches_settings`

其它观察（不阻塞、不需要专门修）：

- `module/uninstall.sh` 的 `settings put ... >/dev/null 2>&1` 不回读：卸载场景下失败无从观测；若希望更稳，
  可回读一行 `settings get global http_proxy` 并打印（模块管理器的卸载输出可见）。
- `GET /egress` 每次都会跑 2 次 `settings get` + 1 次 `dumpsys connectivity`（契约要求 readback，属已知成本）；
  WebUI 轮询该端点时建议在 UI 侧做节流，本次不改服务端。
- 应用级代理**没有** fail-open（该应用直接断网）：与契约 §5 一致，但 `CONFIG.md` 的「已知限制」没写；
  建议补一句，避免用户误以为 `fail_open` 对应用级也生效。

---

## 附录 A：本次新增的评审工件

| 文件 | 作用 |
| --- | --- |
| `docs/design/egress/t11-review.md` | 本文件 |
| `docs/design/egress/evidence-t11/repro-diff-credential.rs` | B-1 复现：`#[path]` 引用真实 `server/src/diff.rs` |
| `docs/design/egress/evidence-t11/repro-diff-credential.out.txt` | 上者输出（场景 A 只改 probeUrl 就带出 `s3cret`） |
| `docs/design/egress/evidence-t11/repro-dumpsys-parse.rs` | M-2 复现：`#[path]` 引用真实 `server/src/egress/system.rs` |
| `docs/design/egress/evidence-t11/repro-dumpsys-parse.out.txt` | 上者输出（真机格式 → `Unknown`；`matches(Direct)==true`） |
| `docs/design/egress/evidence-t11/repro-failopen-cleared.rs` | M-1 复现：真实 `system.rs` + 「put 返回 0 但不生效」runner |
| `docs/design/egress/evidence-t11/repro-failopen-cleared.out.txt` | 上者输出（`clear()` 返回 `Ok` 但 `matches:false`） |

复现程序只读引用 `server/` 源码，不参与构建、不进 Cargo workspace（`rustc` 直接编译单文件）。

## 附录 B：本次未覆盖（out of scope）

- Manager（`manager/`）与两个 WebUI 的下发/展示链路：只核对了「Manager job 会自动发起应用级探测」这一条
  契约依据（§6.12），用于判定 H-1 的可达性；Manager 侧凭据加密（§7.3）未复核。
- `core/` 的 `EgressSection` / `AppConfig.egress` 模型本身：只经由 server 的行为间接核对（422 码、继承链）。
- 真机：本次评审没有操作设备，所有真机事实引自 t9/t15/t18 的 evidence 文件（逐条标注了路径与行号）。
