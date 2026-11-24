# t14 独立复验报告（t9/t13 review findings）

- 日期：2026-11-24（Asia/Shanghai）
- 复验人：test-verifier
- 范围：t9（后端 R1-R9 + 关键低危）与 t13（WebUI R1/R15/R22）；**未修改业务代码**，只新增独立测试、扩展测试用 Mock 夹具并写本报告/日志。
- 代码：工作区 `d9aa881`（`manager/` 为未提交工作树；验证的是当前工作树）
- 方法：新增 [manager/tests/review_reverify.rs](../../../manager/tests/review_reverify.rs)（13 个独立用例，不复用 t9 的断言）+ 真实二进制/curl 探针 + `npm run check:template-payload` + 全量回归。**不是只跑旧测试。**

## 0. 结论总表

| 项 | 结论 | 独立证据 |
| --- | --- | --- |
| R1 模板 packages 回流与其它包绑定 | **PASS** | `r1_object_template_keeps_other_bindings_and_packages`：通过 Mock 内部 app→template 映射断言 4 个包绑定仍为 `shared`；GET template 含 packages；WebUI helper 断言 body 无 packages |
| R2 对象模板 attempts=1 / 强制 412 只重试一次 | **PASS** | `r2_object_template_binds_in_one_attempt`（attempts=1）、`r2_forced_412_retries_bind_exactly_once`（attempts=2）；二进制两次 apply 分别为 attempts=2、attempts=1 |
| R3 过期 / 并发幂等键不 500 | **PASS** | `r3_expired_idempotency_key_replays_same_job`、`r3_concurrent_idempotency_keys_share_one_job`；二进制过期 replay 202 同 job，3 并发 202 同一 job |
| R4 Cache-Control: no-store | **PASS** | `r4_every_api_response_is_no_store` 覆盖 200/202/401/405/413/rotate；二进制逐项确认 |
| R5 rotate 畸形 body 4xx 且不轮换 | **PASS** | `r5_malformed_rotate_bodies_never_rotate`；二进制 `[]`/malformed/unknown/text-plain 分别 422/422/422/415，旧 Token 仍 200，token 文件不存在 |
| R6 设备读接口 8 MiB 上限 | **PASS** | `r6_oversized_device_reads_are_rejected`（探测 422 invalid_response、代理 502 response_too_large）；二进制 9 MiB overview 探测 422 |
| R7 manager.token 不可读 fail closed | **PASS** | 二进制：`chmod 000` 后启动 exit=1，报 `读取 .../manager.token 失败：Permission denied` |
| R8 audit since +08:00 与 Z 等价 | **PASS** | `r8_audit_since_offset_equals_utc`；二进制 UTC 与 +08:00 查询 total/IDs 相同（total=2 < 全部 10） |
| R9 /templates?device= 只返回该设备 config | **PASS** | `r9_templates_device_filter_returns_that_device_config`：Pixel 9 过滤结果 config/sources 均为该设备，全部视图仍有 template_conflict |
| 低危：proxy 原始编码路径 | **PASS** | `low_proxy_preserves_encoded_question_mark_as_path`：原始 TCP 设备记录到 `/api/v1/raw%3Fencoded?real=1`，未变成 query |
| 低危：根 /healthz 405 JSON | **PASS** | `r4_every_api_response_is_no_store` + 二进制：405 `method_not_allowed`、no-store |
| 低危：模板 PUT 412 只重试一次 | **PASS** | `low_template_put_412_retries_once`：template_put 步骤 2 次（412→ok），job succeeded |
| 低危：bind 404 template 记 skipped | **PASS** | `low_bind_404_template_is_skipped`：job partial，result `skipped/template_not_found`，httpStatus=404 |

## 1. 新增独立测试

文件：[manager/tests/review_reverify.rs](../../../manager/tests/review_reverify.rs)，13 个 `#[tokio::test]`：

    r1_object_template_keeps_other_bindings_and_packages
    r2_object_template_binds_in_one_attempt
    r2_forced_412_retries_bind_exactly_once
    r3_expired_idempotency_key_replays_same_job
    r3_concurrent_idempotency_keys_share_one_job
    r4_every_api_response_is_no_store
    r5_malformed_rotate_bodies_never_rotate
    r6_oversized_device_reads_are_rejected
    r8_audit_since_offset_equals_utc
    r9_templates_device_filter_returns_that_device_config
    low_template_put_412_retries_once
    low_bind_404_template_is_skipped
    low_proxy_preserves_encoded_question_mark_as_path

配套测试夹具扩展（仅测试代码）：
- [manager/tests/common/mock_device.rs](../../../manager/tests/common/mock_device.rs)：`force_template_412_once`、`bind_template_404`、`set_oversized_overview`，以及 `template_config` / `app_template` 读取器，用于直接断言设备侧绑定关系。
- [manager/tests/common/mod.rs](../../../manager/tests/common/mod.rs)：`send_raw_body`，用于 rotate 的 `[]` / malformed / text-plain。
- 低危 proxy 用例内嵌一个原始 TCP 设备桩，记录真实请求行，避免只靠代码阅读判断 `%3F` 是否被改名。

## 2. R1：真实形状 + 其它包仍绑定

测试 `r1_object_template_keeps_other_bindings_and_packages`：

1. Mock 模板 `shared` 的 packages 初始为 `["com.xingin.xhs","com.other.app"]`，两个应用都绑定 `shared`。
2. 通过 Manager proxy 调设备 `GET /api/v1/templates/shared`，断言真实形状 `template.packages` 存在（不是把 packages 塞进 config）。
3. 对第三个包 `com.third.app` 下发对象模板（config 不带 packages）：不触发 `template_put`，bind 一次成功；断言 Mock 的 app→template 仍为：
   - `com.xingin.xhs → shared`
   - `com.other.app → shared`
   - `com.third.app → shared`
   且 template.packages 同时含三个包。
4. 用不同 config（dpi=440）对第四个包下发：触发 PUT，但 PUT 前合并设备原 packages；断言四个包仍都绑定 `shared`，packages 含四个包，并出现 `template_overwritten` 警告。

WebUI 侧：
- `npm run check:template-payload` 输出 `OK: bulk/assignments apply body has no packages; input config untouched`。
- 源码检查：`templateRefForApply` 只在 `stores/manager.ts` 的两条 apply 路径使用（第 557、584 行）；helper 先 `{...config}` 再 `delete next.packages`，不改入参。

**结论：PASS。**
已知限制：WebUI 侧采用纯函数 + 脚本断言，不重新抓浏览器网络包；t13 已有运行态探针记录。

## 3. R2：对象模板只 bind 一次 / 强制 412 只重试一次

测试 `r2_object_template_binds_in_one_attempt`：
- 新建对象模板 `r2_fresh` → 设备 GET 404 → PUT → **重新 GET revision** → bind；
- job `succeeded`，`result.attempts=1`，steps 只有 1 个 `bind`，且 revision 步骤至少 2 次（初始 + PUT 后）。
- steps：`revision → template_get(skipped 404) → template_put(ok 201) → revision(ok) → bind(ok)`。

测试 `r2_forced_412_retries_bind_exactly_once`：
- Mock 首次 bind 强制 412；结果 `attempts=2`，2 个 bind 步骤：第一个 412 `config_revision_mismatch`，第二个 ok。

真实二进制（Python Mock）：
- 第一次 apply 新模板：`attempts=2`（Python Mock 首次 PUT app 强制 412），steps 含 412 bind；
- 第二次 apply 另一个新模板：`attempts=1`，steps 只有一次 bind。
- 对应日志：[t14-probe.log](t14-probe.log) 的 R2 两段。

**结论：PASS。**
已知限制：强制 412 由测试桩注入；真实设备 412 场景未在物理设备上复现。

## 4. R3：过期 / 并发幂等键

测试 `r3_expired_idempotency_key_replays_same_job`：
- 用 `Idempotency-Key: r3-expired-key` 创建 job 并等待完成；
- 直接把 `jobs.created_at` 改为 `2020-01-01T00:00:00Z`（模拟远超旧 24h TTL）；
- 同键再次 POST：**202**、`Idempotent-Replay: true`、同一 jobId（无 500）。

测试 `r3_concurrent_idempotency_keys_share_one_job`：
- 同时发起 3 个同键请求（`tokio::join!`）；
- 三个响应均 202，jobId 完全相同（job UNIQUE 冲突路径回查重放），无 500。

真实二进制：
- 过期 replay：`first=job_... replayHTTP=202 replayJob=同 id replayHeader=idempotent-replay: true`；
- 并发：`codes=202,202,202`（日志中拼接为 202202202）、`distinct=1`。

**结论：PASS。**
已知限制：并发为 3 路功能验证，不是压测；未覆盖多进程/多实例并发（SQLite 单写连接）。

## 5. R4：no-store 覆盖

测试 `r4_every_api_response_is_no_store`：

| 响应 | 断言 |
| --- | --- |
| GET /devices 200 | `Cache-Control: no-store` |
| POST /apply 202 | `Cache-Control: no-store` |
| GET /devices 无 Token 401 | `Cache-Control: no-store` |
| POST /healthz 根路径 405 | JSON `method_not_allowed` + no-store |
| POST /relay/api/v1/healthz 405 | no-store |
| POST /apply 1 MiB+ 413 | `body_too_large` + no-store |
| POST /token/rotate 200 | 返回新 Token + no-store |

真实二进制逐项复核（[t14-probe.log](t14-probe.log)）：

    healthz 200: cache-control: no-store
    401: cache-control: no-store
    root /healthz POST 405: cache-control: no-store, code=method_not_allowed
    apply 202: cache-control: no-store
    413: cache-control: no-store, code=body_too_large
    rotate 200: cache-control: no-store, tokenPersisted=true

**结论：PASS。**
已知限制：只要求 API 响应；静态 `index.html` 仍使用 `no-cache`（设计如此，不属于 R4）。

## 6. R5：rotate 畸形 body 不轮换

测试 `r5_malformed_rotate_bodies_never_rotate` 与二进制探针：

| body | Content-Type | 状态 | error |
| --- | --- | --- | --- |
| `[]` | application/json | 422 | `validation_failed`（body 必须是对象） |
| `{not-json` | application/json | 422 | `validation_failed`（syntax_error） |
| `{"unknownField":true}` | application/json | 422 | `validation_failed`（unknown_field） |
| `{}` | text/plain | 415 | `unsupported_media_type` |

每个 case 均断言：
- 响应是 JSON 错误信封，`token` 字段不存在；
- `Cache-Control: no-store`；
- 旧 Token 仍可 `GET /devices` 200；
- `<data-dir>/manager.token` **不存在**（没有任何轮换落盘）。

随后合法 rotate `{"token":"r5-new-token-123456"}`：200，token 文件写入，旧 Token 401，新 Token 200。

二进制同结果（[t14-probe.log](t14-probe.log)）：`422/422/422/415`，`tokenFile=absent`，`oldToken=200`；合法 rotate 后旧 401/新 200。

**结论：PASS。**
已知限制：未覆盖 `Content-Type: application/json; charset=utf-8` 之外的厂商变体；仅验证四类代表性 body。

## 7. R6：设备读接口 8 MiB 上限

测试 `r6_oversized_device_reads_are_rejected`：
1. 探测（`POST /devices`）一个 overview 返回 9 MiB 的 Mock：`422 device_probe_failed`，`reason=invalid_response`，设备未落库。
2. 正常添加设备后把 Mock 切换为超大 overview，调用代理 `GET /devices/{id}/proxy/api/v1/overview`：`502 device_error`，`details.reason=response_too_large`。

真实二进制（[t14-r6.log](t14-r6.log)）：本地 9 MiB 设备桩，`HTTP 422 code=device_probe_failed reason=invalid_response`。
> [t14-probe.log](t14-probe.log) 同一场景里有一段 `BrokenPipeError`：Manager 在读取到超限 `Content-Length` 后关闭连接，设备桩继续写响应导致；属预期噪声，干净输出见 t14-r6.log。

**结论：PASS。**
已知限制：只覆盖 overview 读与代理响应；未逐一覆盖 status/templates/revision（同一 `DEVICE_RESPONSE_LIMIT` 常量约束）。

## 8. R7：manager.token 不可读 fail closed

二进制探针（[t14-probe.log](t14-probe.log)）：

    mkdir /tmp/dfm-t14-unreadable
    echo unreadable-token-123 > .../manager.token
    chmod 000 .../manager.token
    ./target/debug/device_faker_manager --data-dir ... --bind 127.0.0.1:18781
    exit=1
    error: 加载配置失败: 读取 .../manager.token 失败：Permission denied (os error 13)

即 token 文件存在但不可读时不会静默回退到 `admin123` 或 CLI/默认值，进程拒绝启动。
t9 同名单测也保留在全量测试中。

**结论：PASS。**
已知限制：验证于 macOS POSIX 权限；Windows ACL 未覆盖。

## 9. R8：audit since +08:00 与 Z 等价

测试 `r8_audit_since_offset_equals_utc`：
- 生成设备添加 + 401 两条审计；
- 取最旧审计 ts + 1 s 作为 cutoff，分别格式化为 `...Z` 与 `...+08:00`（后者 URL 编码 `+` 为 `%2B`）；
- 两次查询的 `total` 与 entry id 序列完全一致，且 total < 全部记录数（证明真的发生了过滤，不是都返回空/全量）；
- 非法 since → 422。

二进制（[t14-probe.log](t14-probe.log)）：

    oldest=2026-11-24T10:42:42Z cutoffUTC=2026-11-24T10:42:43Z cutoff+08=2026-11-24T18:42:43+08:00
    all=10 byUTC=2 by+08=2 idsEqual=True subset=True

**结论：PASS。**
已知限制：只验证 +08:00 与 UTC；其它偏移 / 夏令时边界未枚举。

## 10. R9：/templates?device= 只返回该设备 config

测试 `r9_templates_device_filter_returns_that_device_config`：
- 两台 Mock 都有同名模板 `shared`，但 config 分别为 Pixel 9 与 Xiaomi 15；
- `GET /templates?device=pixel-1`：只返回 1 个模板，`config.model=Pixel 9`、`config.brand=google`、`sources=["pixel-1"]`，无 `template_conflict`；
- 不带 device 的 `/templates`：同名模板出现 `template_conflict`，说明过滤路径不是简单复用全局并集。

**结论：PASS。**
已知限制：只验证两台设备同名模板；更多设备/字段差异未枚举。

## 11. 关键低危项

| 项 | 独立验证 |
| --- | --- |
| proxy 编码路径 | `low_proxy_preserves_encoded_question_mark_as_path` 使用原始 TCP 设备桩，记录到的请求行恰为 `/api/v1/raw%3Fencoded?real=1`；未出现 `/api/v1/raw?encoded`，证明 `%3F` 保留在 path 而非 query |
| 根 /healthz 405 JSON | 测试 + 二进制：405 `method_not_allowed`，`Cache-Control: no-store`，JSON 信封（非纯文本/HTML） |
| 模板 PUT 412 重试一次 | `low_template_put_412_retries_once`：2 个 template_put 步骤（首个 412，第二个 ok），job succeeded |
| bind 404 template skipped | `low_bind_404_template_is_skipped`：job partial，result skipped / template_not_found / httpStatus 404，`details.resource=template` |
| 数据目录权限（附带） | 二进制数据目录 0700；manager.db/-wal/-shm 与 key/token 均 0600（[t14-meta.log](t14-meta.log)） |

**结论：PASS。**

## 12. 全量检查

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --check` | exit 0 |
| `cargo clippy -p device_faker_manager --all-targets -- -D warnings` | exit 0 |
| `cargo test -p device_faker_manager` | exit 0：**56 unit + 70 integration = 126 tests**，0 failed |
| `cd manager/webui && npm run check:template-payload` | exit 0：body 无 packages，入参未改 |
| `npx vue-tsc --noEmit` | exit 0 |
| `npm run build` | exit 0，dist 构建成功 |

测试分布：`manager_api` 17、`devices_flow` 14、`apply_flow` 14、`e2e_flow` 4、`ops_endpoints` 8、`review_reverify` 13（t14 新增）。
完整输出：[t14-tests.log](t14-tests.log)、[t14-webui.log](t14-webui.log)、[t14-webui-evidence.log](t14-webui-evidence.log)。

## 13. 失败重试

- 新增测试首次编译失败：`tokio::join!` 中三个 future 借用了临时 `format!()` 字符串；改为绑定 `apply_uri` 后通过。属测试代码问题，不影响产品。
- clippy 报 `assert_eq!(bool, true)`；改为 `assert!(...)` 后通过。
- R6 二进制桩首次产生 `BrokenPipeError` 噪声：Manager 拒绝超限响应后主动关闭连接，桩继续写入导致；重新跑一份捕获异常的设备桩，输出在 [t14-r6.log](t14-r6.log)，结论不变。
- 无 R 项失败；无产品阻塞问题。

## 14. 已知限制汇总

1. R1 WebUI 是 helper/静态断言 + 源码 grep，未重新抓取浏览器请求体（t13 已有运行态探针）。
2. R2 412 与 R6 超限由测试桩注入；真实设备/真实大响应未在物理设备上复现。
3. R3 并发为 3 路功能验证，不是压力/多进程测试。
4. R4 只验证 API 动态响应；静态 HTML 仍 `no-cache` 属设计。
5. R5 覆盖四类代表性畸形 body，未枚举所有 Content-Type 变体。
6. R7 验证 macOS POSIX 权限；Windows ACL 未覆盖。
7. R8 只验证 +08:00 与 UTC 等价；其它偏移/夏令时未枚举。
8. R9 只验证两台设备同名模板冲突/过滤。
9. 低危 proxy 只验证 `%3F`；其它保留字符（`%2F`、中文编码等）未逐一验证。

## 15. 产物

- 本报告：[t14-reverify.md](t14-reverify.md)
- 测试：[t14-tests.log](t14-tests.log)
- WebUI：[t14-webui.log](t14-webui.log)、[t14-webui-evidence.log](t14-webui-evidence.log)
- 二进制探针：[t14-probe.log](t14-probe.log)、[t14-r6.log](t14-r6.log)
- 运行环境：[t14-manager.log](t14-manager.log)、[t14-mock.log](t14-mock.log)、[t14-meta.log](t14-meta.log)
- 测试源码：[manager/tests/review_reverify.rs](../../../manager/tests/review_reverify.rs)

## 16. 许可

    SPDX-License-Identifier: GPL-3.0-or-later
    Copyright (c) 2025-2026 Seyud
    Device Faker: https://github.com/Seyud/device_faker
