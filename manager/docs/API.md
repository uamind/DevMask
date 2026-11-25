# Device Faker Manager Relay API 契约（/relay/api/v1）

> 状态：**已冻结（contract 里程碑）**
> 版本：Manager `0.2.0` / API `v1`（egress 增量）
> 日期：2026-11-24
> 上游契约：[设备端 /api/v1](../../docs/API.md)（device_faker_server 1.10.0；egress 端点自 1.10.0 起提供）
> 关联文档：[ARCH.md](ARCH.md)（实现架构与数据模型）、[api-types.ts](api-types.ts)（TypeScript 类型定义）、[../docs/plans/manager-relay-plan.md](../../docs/plans/manager-relay-plan.md)（计划书）

本文是 Manager / Relay 的唯一接口事实来源。`frontend-dev` 按本文 + `api-types.ts` 编码 WebUI，
`backend-dev` 按本文实现 `manager/src`，`test-verifier` 按本文与 [§9](#9-可执行验收curl) 写验收。
**文档与实现冲突时以本文为准**；要改契约，先改本文并在 §10 记一条变更。

约定用词：**必须** = 验收项；**应当** = 默认实现，偏离需在本文注明；**可以** = 可选。

---

## 1. 范围与版本

### 1.1 本增量实现

| 能力 | 说明 |
| --- | --- |
| 设备注册表 | 添加 / 详情 / 修改 / 移除 / 单台同步 / 全部同步；设备 Token 加密存储，永不出现在响应与日志 |
| 聚合视图 | `/overview`（总览）、`/apps`（应用 × 设备矩阵 + 漂移）、`/templates`（跨设备模板并集） |
| 批量下发 | `/apply` 的 bulk（一个应用 × 多设备 × 一个模板）与 assignments（逐设备逐应用逐模板）两种形态 |
| 任务与逐设备结果 | `/jobs/{id}`：queued/running/succeeded/partial/failed + 每台设备的 steps、重试次数、错误码 |
| Relay 鉴权 | 单一 Manager Token（Bearer）；CLI / 环境变量 / 文件 / 配置文件四级来源；`POST /token/rotate` 轮换 |
| 单设备代理 | `/devices/{id}/proxy/api/v1/{*path}`，把设备 `/api/v1` 原样透出，Token 由 Relay 注入 |
| 静态托管 | 同进程托管 WebUI `dist`，SPA fallback |
| 审计 | `/audit` 查询添加 / 修改 / 移除 / 同步 / 下发 / 轮换 / 鉴权失败 |
| 出口代理池（egress） | `/egress-proxies` CRUD（名称唯一、被绑定删除 409）、凭据加密落库、Manager 侧直连探测、最近探测结果 |
| 出口绑定与下发 | `/devices/{id}/egress` 绑定（整机 / 应用级）、`/devices/egress/apply` 批量下发（job `kind=egress`，含 `probe` 与 `dryRun`） |

### 1.2 明确不做（本增量）

- 多租户、RBAC、多 Token 与 scope 矩阵、OAuth、公网 TLS 终结。
- 定时同步、WebSocket 实时进度、应用名 / 图标抓取、应用商店搜索。
- 设备端模块与 `/api/v1` 的修改（除非发现阻塞 Relay 的设备端 bug）。
- 限流（不返回 429）、请求重放签名、审计导出。
- 设备分组维度批量（只保留 `tags` 展示与过滤）。
- 设备端全流量代理（iptables + tun2socks，含 UDP/QUIC）与 Manager 访问设备的出站代理：本增量不做。

### 1.3 版本与兼容

| 项 | 值 |
| --- | --- |
| API 前缀 | `/relay/api/v1`（下称 `BASE`） |
| `apiVersion` | `v1`（出现在 `/healthz`） |
| Manager 版本 | `0.2.0` |
| 设备端最低版本 | `1.9.0`（需要 `/status`、`/overview`、`/config/revision`、`/templates`、`/apps`、`/apps/{pkg}/restart`）；egress 端点需要 `1.10.0`，缺失时下发结果记 `egress_unsupported`（`skipped`） |
| 低版本设备 | 允许注册；缺少的字段为 `null`，聚合视图给 `unsupported_device_api` 警告；下发仍按 1.9.0 端点执行，失败按 §7.5 映射 |

兼容规则：

1. 新增响应字段 = 兼容变更，客户端必须忽略未知字段。
2. 删除字段、改字段类型 / 语义、改错误码含义 = 破坏性变更，必须升 `v2` 前缀。
3. 请求体是**严格模式**：出现未知字段 → `422 validation_failed`（`details.errors[].code = "unknown_field"`），
   与设备端一致，避免拼写错误静默生效。

---

## 2. 基础约定

### 2.1 监听、基地址与静态站点

| 项 | 默认值 | 覆盖方式 |
| --- | --- | --- |
| 监听 | `127.0.0.1:8765` | `--bind` / `DFM_BIND` / `manager.toml` 的 `bind` |
| API 基地址 | `http://127.0.0.1:8765/relay/api/v1` | — |
| 健康检查 | `GET /relay/api/v1/healthz`，别名 `GET /healthz`（同为无鉴权） | — |
| WebUI | 同进程 `/`，产物目录 `manager/webui/dist` | `--static-dir` / `DFM_STATIC_DIR` |
| 数据目录 | 见 [ARCH.md §4](ARCH.md#4-数据目录与配置文件) | `--data-dir` / `DFM_DATA_DIR` |
| WebUI 开发 | Vite `127.0.0.1:5180`，把 `/relay` 代理到 `8765` | `manager/webui/vite.config.ts` |

安全硬约束：

1. 默认只绑 loopback。`bind` 不是 loopback 且 Token 仍是默认 `admin123`（或长度 < 8）时，
   **服务拒绝启动**，日志写明原因与生成 Token 的方法。
2. `/relay/api/v1/*` 全部响应带 `Cache-Control: no-store`。
3. 不存在任意 shell 执行、任意文件读写端点；代理只能命中已注册设备的 `baseUrl`。

### 2.2 字段命名规则（必须严格遵守）

| 位置 | 规则 | 示例 |
| --- | --- | --- |
| 设备配置内容（`template.config` 内部、设备 overview 透传部分） | **snake_case**，与设备端 `config.toml` 同名同义 | `sdk_int`、`android_version`、`custom_props` |
| Relay 元数据（响应包装、请求控制字段） | **lowerCamelCase** | `baseUrl`、`lastSyncedAt`、`httpStatus`、`durationMs` |
| 查询参数 | **snake_case** 或单词小写 | `limit`、`cursor`、`online`、`drift`、`since` |
| 路径参数 | 原文，必要时百分号编码 | `com.xingin.xhs`、`pixel_9` |
| 请求 / 响应头 | 标准 HTTP 大小写 | `Authorization`、`If-Match`、`X-Request-Id` |

> 一句话：**设备里的配置长什么样就 snake_case 照抄；Relay 自己的字段一律 camelCase。**

### 2.3 请求头

| 头 | 必需 | 说明 |
| --- | --- | --- |
| `Authorization: Bearer <manager-token>` | 除 `/healthz` 外必需 | 见 §2.5 |
| `Content-Type: application/json` | 有请求体时必需 | 其他类型 → `415 unsupported_media_type` |
| `Accept: application/json` | 可选 | 默认 JSON |
| `Idempotency-Key` | 可选 | 仅 `POST /apply` 与 `POST /devices/egress/apply` 生效；≤128 字符，建议 `[A-Za-z0-9._:-]` |
| `X-Request-Id` | 可选 | 客户端关联 ID（≤128 字符）；服务端原样回填，并透传给设备 |

### 2.4 响应头

| 头 | 出现位置 | 说明 |
| --- | --- | --- |
| `X-Request-Id` | 所有响应 | 未提供时生成 `req_<32 位小写十六进制>` |
| `Cache-Control: no-store` | 所有 `/relay/api/v1/*` | 含错误响应 |
| `Idempotent-Replay: true` | 幂等键命中的 `POST /apply` | 响应体是首次的 job |
| `X-DF-Upstream-Request-Id` | 代理端点 | 设备端返回的 `X-Request-Id`，便于两段日志对齐 |
| `Retry-After` | 429（保留） | 本增量不返回 429 |
| `WWW-Authenticate: Bearer` | 401 | 标准质询 |

错误响应 `Content-Type` 恒为 `application/json; charset=utf-8`。

### 2.5 鉴权

- 唯一方式：`Authorization: Bearer <manager-token>`；不使用 Cookie，因此没有 CSRF 面。
- 缺失 / 格式错误 / 不匹配 → `401 unauthorized`（不区分原因，避免枚举），`details = null`。
- Token 比较使用常量时间比较；Token 明文只出现在：启动日志的一行提示（仅当使用默认值）、
  `POST /token/rotate` 的响应、`<data-dir>/manager.token`（0600）。不写审计、不写访问日志。
- Manager Token 来源与优先级（高 → 低）：

| 优先级 | 来源 | 说明 |
| --- | --- | --- |
| 1 | `--token <value>` | 进程参数，最高 |
| 2 | `DFM_TOKEN` | 环境变量 |
| 3 | `<data-dir>/manager.token` | `POST /token/rotate` 写入；重启后继续生效 |
| 4 | `manager.toml` 的 `[manager].token` | 配置文件 |
| 5 | 内置默认 `admin123` | 仅便于内网首次使用；建议首次启动后轮换 |

- `POST /token/rotate` 在来源为 1 或 2 时返回 `409 invalid_state`（`details.reason = "token_overridden"`），
  因为进程参数 / 环境变量无法由服务端安全改写。
- WebUI：Token 存 `localStorage` 的 `dfm_token`；首个请求 401 时弹出 Token 对话框，重新输入后重试。
  默认值 `admin123` 只用于初始化输入框，不写入页面 HTML。
- 401 响应不得包含 Token 片段、设备 Token、数据库路径。

### 2.6 错误信封

所有 4xx / 5xx 使用与设备端完全一致的唯一信封：

    {
      "error": {
        "code": "device_offline",
        "message": "Pixel 9 连接超时",
        "details": { "deviceId": "pixel-9", "reason": "timeout" },
        "retryable": true,
        "requestId": "req_3f1c9b7a2d4e5f60819a0b1c2d3e4f50"
      }
    }

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `code` | string | 是 | 稳定错误码，见 §2.7 |
| `message` | string | 是 | 人类可读短说明，可含中文 |
| `details` | object \| null | 是（可为 null） | 结构化原因，供 AI / UI 分支 |
| `retryable` | boolean | 是 | 原样重试是否可能成功；`true` 时客户端应退避重试（250 ms 起，最多 3 次） |
| `requestId` | string | 是 | 与 `X-Request-Id` 相同，用于查服务端日志 |

> **一个已知例外**：`POST /egress-proxies/{id}/probe` 的 `502 egress_probe_failed` 在信封之外**额外带顶层 `warnings`**（`[{code:"egress_check_not_recorded", …}]`；没有内容时是 `[]`）。客户端解析错误时应同时读顶层 `warnings`，别只读 `error` 而丢掉「探测结果没落库」这条信息。其余 4xx/5xx 仍是纯信封。

### 2.7 错误码表（Manager）

| code | HTTP | retryable | details 形状 | 含义 |
| --- | --- | --- | --- | --- |
| `unauthorized` | 401 | false | `null` | 缺少 / 格式错误 / 无效 Manager Token |
| `forbidden_scope` | 403 | false | `{required, granted}` | 保留：本增量单一 Token 拥有全部权限，正常不会返回 |
| `not_found` | 404 | false | `{resource, name}` | 资源不存在（含未知 API 路径，`resource = "route"`） |
| `method_not_allowed` | 405 | false | `{method, path}` | 路径存在但方法不支持 |
| `device_not_found` | 404 | false | `{deviceId}` | 设备未注册 / 已移除 |
| `device_exists` | 409 | false | `{field, value, deviceId}` | `baseUrl` 或 `id` 已被占用；`field` = `baseUrl` \| `id` |
| `device_probe_failed` | 422 | false | `{baseUrl, reason, upstream}` | 添加 / 修改设备时探测失败，未写入；见 §4.3 |
| `validation_failed` | 422 | false | `{errors: [ValidationError]}` | 请求体 / 查询参数语义不合法 |
| `body_too_large` | 413 | false | `{limitBytes}` | 请求体超过 1 MiB |
| `unsupported_media_type` | 415 | false | `{supported: ["application/json"]}` | `Content-Type` 不支持 |
| `invalid_state` | 409 | false | `{reason}` | 当前配置不允许该操作（如 Token 被 CLI / env 固定） |
| `device_offline` | 502 | true | `{deviceId, reason}` | 连接失败 / DNS / 超时；`reason` = `connect` \| `dns` \| `timeout` |
| `device_error` | 502 | true | `{deviceId, upstreamStatus, upstreamCode}` | 设备返回了非预期响应 |
| `upstream_timeout` | 504 | true | `{deviceId, timeoutMs}` | 设备端超过 Relay 设定时限 |
| `egress_proxy_exists` | 409 | false | `{field, value, id}` | 代理名已被占用（`field="name"`） |
| `egress_proxy_in_use` | 409 | false | `{proxyId, name, total, devices, apps}` | 仍有设备绑定引用该代理，拒绝删除 |
| `egress_probe_failed` | 502 | true | `{proxyId, url, reason, timeoutMs, upstreamStatus}` | Manager 侧直连探测失败（`reason` 同设备端枚举） |
| `internal_error` | 500 | true | `{op}` \| null | 未预期错误；用 `requestId` 查日志 |
| `rate_limited` | 429 | true | `{limit, windowSec}` | 保留：本增量不返回 |

`ValidationError`：

    { "path": "assignments[1].template", "code": "invalid_value", "message": "template 必须是模板名或 {name, config}" }

| `details.errors[].code` | 触发 |
| --- | --- |
| `missing_required` | 缺少必填字段 |
| `unknown_field` | 出现未知字段 |
| `invalid_type` | 类型不符 |
| `invalid_value` | 枚举 / 范围 / 组合非法（如同时给 `app` 与 `assignments`） |
| `invalid_package` | 包名不符合 `^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+(@[0-9]{1,3})?$` |
| `invalid_template_name` | 模板名不符合 `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$` |
| `invalid_device_id` | 设备 id 不符合 `^[a-z0-9][a-z0-9_-]{0,63}$` |
| `invalid_base_url` | 地址不是合法 `http(s)://host[:port][/prefix]` |
| `invalid_token` | 设备 Token 为空、含换行或超过 256 字符 |
| `out_of_range` | 数值 / 列表长度越界 |
| `too_many_items` | 超过批量上限（assignments ≤ 200，目标设备 ≤ 200，目标对 ≤ 200） |
| `duplicate_assignment` | 同一 `(device, app)` 出现两次 |
| `invalid_egress_name` | 代理名不符合 `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`，或等于保留值 `__DIRECT__` |
| `invalid_egress_proxy` | `type` 非枚举、`host` 含 scheme/端口或超长、`port` 越界、凭据超长、`enabled` 非布尔 |
| `invalid_probe_url` | 探测 URL 不以 `http://` 开头、超过 512 字符或含用户信息 |
| `unsupported_proxy_type` | `POST /egress-proxies/{id}/probe` 对 `socks5` 代理：Manager 侧探测只支持 HTTP 代理 |

### 2.8 Job 结果码与警告码

Job 逐设备结果的 `code`（即 `JobResult.code`，也是 `error.code`）取自：

| code | 归属 | 含义 |
| --- | --- | --- |
| `device_offline` | Manager | 设备不可达（connect / dns / timeout） |
| `upstream_timeout` | Manager | 单请求超时 |
| `device_error` | Manager | 设备返回非预期状态 / 无法解析 |
| `package_not_installed` | 设备 | 包未安装（设备 `404 not_found` + `details.resource="package"`） |
| `template_not_found` | Manager | 只给模板名，但目标设备没有该模板 |
| `config_revision_mismatch` | 设备 | 重试一次后仍 412 |
| `forbidden_scope` | 设备 | 设备 Token 缺 `write` / `ops` 作用域 |
| `validation_failed` | 设备 | 设备端 422（`details.errors` 原样保留） |
| `upstream_unavailable` | 设备 | 设备端 `pm` / `am` / `settings` 不可用（503） |
| `egress_unsupported` | Manager | 设备端没有 `/egress`（旧版本，404）：`skipped` |
| `egress_not_configured` | 设备 | 设备端没有任何可用出口（409 `egress.not_configured`）：`failed` |
| `egress_unchanged` | Manager | 幂等短路：**设备返回 `changed:false`**（每次都 PUT，Manager 不做本地比较）。**结果仍是 `ok`**，只是 `code` 非空（全部 `skipped` 会把 job 算成 `partial`） |
| `decrypt_failed` | Manager | 组装期望态时**代理凭据解密失败**（`manager.key` 变更 / 密文损坏）：`failed`，`details={deviceId, op:"decrypt", reason:"credential_decrypt_failed", message, detail}` |
| `device_token_invalid` | Manager | **设备 Token 解密失败**（`manager.key` 变更，需要重新登记设备 Token）：`failed`，`details={deviceId, op:"decrypt"}`；**不是** `device_offline` |

警告对象（`Warning`）：

    { "code": "package_not_installed", "message": "com.tencent.tmgp.pubgmhd 未安装", "details": { "package": "com.tencent.tmgp.pubgmhd" } }

Manager 产生的警告码：

| code | 出现位置 | details |
| --- | --- | --- |
| `device_offline` | 设备详情 / 矩阵 / 同步 | `{deviceId, reason}` |
| `device_cache_missing` | `/apps`、`/templates`、`/overview` | `{deviceId}` |
| `stale_cache` | `/apps`、`/templates`、`/overview`、设备详情 | `{deviceId, ageSec}` |
| `template_conflict` | `/templates` | `{name, devices, values}` |
| `template_overwritten` | job 结果 | `{deviceId, name}` |
| `restart_failed` | job 结果 | `{deviceId, package, code}` |
| `drift` | `/apps` | `{package, models}` |
| `no_devices` | `/apps`、`/overview`、`/templates` | `{}` |
| `unsupported_device_api` | 设备详情 / 矩阵 | `{deviceId, missing: ["overview"]}` |
| `egress_probe_failed` | job 结果 / `/devices/{id}/egress` | `{deviceId, app, reason}` |
| `egress_proxy_disabled` | job 结果 | `{proxyId, deviceId}` |
| `egress_rename_pending` | `PATCH /egress-proxies/{id}` | `{oldName, newName, devices}` |
| `egress_unsupported` | `/devices/{id}/egress` | `{deviceId, code}` |
| `egress_check_not_recorded` | job 结果 / `POST /egress-proxies/{id}/probe` 响应 | `{deviceId, app, scope, proxyId}`（job）；`{proxyId, source:"manager"}`（probe）——探测成功/失败后 `egress_checks` 写入失败 |

设备端警告（`package_not_installed`、`duplicate_package`、`empty_packages`、`inline_record_removed`、`config_missing`、`unknown_field_preserved`、`pm_unavailable`、`no_config`）在聚合时**原样透传**，不重命名。

设备端 **egress 警告**同样原样透传：出现在 `JobResult.warnings` 与 `/devices/{id}/egress` 的 `deviceState.warnings` 里，**details 保持设备端形状**（不补 `deviceId` / `app`）：

| code | details（设备端形状） | 触发 |
| --- | --- | --- |
| `egress_proxy_not_found` | `{name, scope:"device"\|"app", package?}` | 引用的代理名在设备端 `egress.toml` 未定义 |
| `egress_credentials_ignored` | `{name}` | 设备级指向带凭据代理（方案 A 只写 `host:port`） |
| `egress_auth_unsupported` | `{scope, proxy, field:"username"\|"password"}` | 生效代理带凭据但注入不支持认证（**同一代理只报一次**；写操作照常 2xx，不阻断） |
| `egress_inline_shadows_template` | `{package, template}` | 写应用级 `egress` 会建立内联记录、整记录覆盖模板 |
| `egress_app_not_installed` | `{package}` | 应用级覆盖的包未安装 |
| `egress_degraded_to_direct` | `{reason}` | fail-open 真正切到直连时才出现 |
| `egress_settings_mismatch` | `{expected, actual}` 或 `{reason:"settings_unavailable"}` | 读回不一致 / `settings` 命令不可用 |
| `egress_perm_fixed` | `{path, mode}` | `egress.toml` 权限被自愈为 0600 |
| `egress_probe_approximate` | `{package}` | 应用级探测在服务进程内完成 |
| `egress_probe_scope_app` | 出现在设备端探测 `502` 的 `details.warnings[]` | 应用级探测失败、设备级系统代理保持不变 |

> `dry_run` / `diff` 的响应里，代理凭据一律掩码成 `<redacted>`（设备端行为，见 [../../docs/API.md](../../docs/API.md) §5.26）；`settings` 失败只表现为设备端 `503 upstream_unavailable` + `details.component="settings"`，没有单独的错误码。

### 2.9 分页、过滤与排序

- `limit` 默认 50，最大 500；超出 → `422 validation_failed`（`path="limit"`）。
- `cursor` 为不透明字符串（Base64URL 的偏移量），取自上一页 `nextCursor`；`nextCursor = null` 表示没有下一页。
- 非空且无法解析的 `cursor` → `422 validation_failed`（`path="cursor"`）。
- 固定排序（实现不得依赖数据库默认顺序）：

| 列表 | 顺序 |
| --- | --- |
| `/devices` | `createdAt` 升序，同值按 `id` 升序 |
| `/templates` | `name` 升序（ASCII） |
| `/apps` | `package` 升序 |
| `/jobs` | `createdAt` 降序，同值按 `id` 升序 |
| `/audit` | `id` 降序 |
| `/egress-proxies` | `name` 升序（ASCII） |
| `GET /devices/{id}/egress` 的 `bindings` | `app` 升序（`""` 最前） |
| `GET /egress-proxies/{id}` 的 `bindings` | `deviceId` 升序，同值按 `app` 升序 |
| `job.results` | 请求里的 assignment 顺序（bulk 展开后按 `devices` 数组顺序；`all` 按设备 `createdAt` 升序） |

### 2.10 请求体上限、超时与并发

| 项 | 值 | 说明 |
| --- | --- | --- |
| 请求体上限 | 1 MiB（1048576 字节） | 超出 → `413 body_too_large` |
| 代理响应体上限 | 8 MiB | 超出 → `502 device_error`（`details.reason="response_too_large"`） |
| 连接超时（到设备） | 3 s | 固定 |
| 读请求超时 | 10 s | `GET /status`、`/overview`、`/templates`、`/config/revision` |
| 写请求超时 | 15 s | `PUT /templates`、`PUT /apps/{pkg}` |
| restart 超时 | 10 s | `POST /apps/{pkg}/restart` |
| 全量同步并发 | 4 | `DFM_SYNC_CONCURRENCY`，范围 1–16 |
| 下发并发 | 4 | `DFM_APPLY_CONCURRENCY`，范围 1–16；同一设备的下发串行 |
| 设备调用总时限 | 单个 assignment 60 s（目标） | 当前实现未额外施加硬上限：各请求按上表独立超时；超出按 `upstream_timeout` 记失败并继续其它设备（见 ARCH §17） |

### 2.11 幂等

- `POST /apply` 支持 `Idempotency-Key`，**不设 TTL**：同键请求直接返回首次创建的 job，
  并带 `Idempotent-Replay: true`；不比较请求体差异。旧键重放不会因过期而 500。
- 不同请求体复用同一键属于客户端错误，目前按「返回首次 job」处理（不新增错误码）。
- `GET` 天然幂等；`DELETE /devices/{id}` 第二次返回 `404 device_not_found`。
- 下发本身按「先取 revision 再写」执行，设备端 412 已由 Relay 重试一次（见 §7.2）。

### 2.12 设备侧字段透传规则

- `DeviceOverview`（设备 `/api/v1/overview` 的响应）被 Relay **原样缓存与透出**：
  Relay 不认识的新字段保留，不裁剪、不改名。
- 聚合层只读取其中稳定字段：`device.*`、`root.*`、`server.version`、`config.revision`、
  `templates[].name`、`templates[].config`、`apps[].package`、`apps[].source`、`apps[].resolved`、`apps[].warnings`。
- 设备端同名 JSON 键（如 `revision`）在 Relay 层级出现时，语义以所在对象为准（`DeviceOverview.revision` 是设备 revision，
  `/apps` 顶层 `revision` 是聚合 revision，见 §5.10）。

---

## 3. 端点总表

| # | 方法 | 路径（相对 `BASE`） | 鉴权 | 用途 | 幂等键 |
| --- | --- | --- | --- | --- | --- |
| 1 | GET | `/healthz` | 无 | 存活、版本、DB、设备计数 | — |
| 2 | GET | `/devices` | 是 | 设备列表 + 在线统计 | — |
| 3 | POST | `/devices` | 是 | 添加设备并探测 | — |
| 4 | GET | `/devices/{id}` | 是 | 设备详情（含缓存 overview / 模板 / 应用） | — |
| 5 | PATCH | `/devices/{id}` | 是 | 改名 / 改标签 / 换 Token / 换地址（重新探测） | — |
| 6 | DELETE | `/devices/{id}` | 是 | 从管理端移除（不动远端） | — |
| 7 | POST | `/devices/{id}/sync` | 是 | 重新拉取 status + overview | — |
| 8 | POST | `/devices/sync` | 是 | 同步全部设备 | — |
| 9 | GET | `/devices/{id}/overview` | 是 | 单台设备 overview（默认实时，可读缓存） | — |
| 10 | GET | `/templates` | 是 | 跨设备模板并集 | — |
| 11 | GET | `/overview` | 是 | 总览：设备 / 应用 / 模板 / 漂移 / 最近 job | — |
| 12 | GET | `/apps` | 是 | 应用 × 设备矩阵与漂移 | — |
| 13 | POST | `/apply` | 是 | 批量 / 逐项下发 | 支持 |
| 14 | GET | `/jobs/{id}` | 是 | job 与逐设备结果 | — |
| 15 | GET | `/jobs` | 是 | job 列表 | — |
| 16 | GET | `/audit` | 是 | 审计日志 | — |
| 17 | POST | `/token/rotate` | 是 | 轮换 Manager Token（响应返回一次新值） | — |
| 18 | ANY | `/devices/{id}/proxy/api/v1/{*path}` | 是 | 单设备 `/api/v1` 透传 | 透传 `Idempotency-Key` |
| 19 | GET | `/egress-proxies` | 是 | 代理池列表（脱敏 + 绑定数 + 最近探测） | — |
| 20 | POST | `/egress-proxies` | 是 | 新建代理（凭据只写不读） | — |
| 21 | GET | `/egress-proxies/{id}` | 是 | 代理详情（含 bindings / recentChecks） | — |
| 22 | PATCH | `/egress-proxies/{id}` | 是 | 修改（凭据双态：省略不改 / null 清除） | — |
| 23 | DELETE | `/egress-proxies/{id}` | 是 | 删除；被绑定 → 409 | — |
| 24 | POST | `/egress-proxies/{id}/probe` | 是 | Manager 侧直连探测（验证代理本身） | — |
| 25 | GET | `/devices/{id}/egress` | 是 | 该设备的绑定 + 设备侧实时状态 | — |
| 26 | PUT | `/devices/{id}/egress` | 是 | 全量替换绑定（只落库，`pendingApply:true`） | — |
| 27 | POST | `/devices/egress/apply` | 是 | 批量下发（202 + job `kind=egress`） | 支持 |

除 `/healthz` 外全部需要鉴权。路由不存在 → `404 not_found`（`details.resource="route"`，JSON，不走 SPA fallback）。

---

## 4. 端点明细

以下示例统一省略通用头；`$BASE` 与 `$TOKEN` 见 §9.2。

### 4.1 `GET /healthz`（无鉴权）

用途：进程探针、WebUI 顶部「Relay 在线」、AI 首次连通性检查。别名：`GET /healthz`（根路径）。

响应 `200`：

    {
      "status": "ok",
      "version": "0.2.0",
      "apiVersion": "v1",
      "uptimeSec": 3612,
      "startedAt": "2026-11-24T08:00:00Z",
      "db": { "status": "ok", "schemaVersion": 2, "path": "manager.db" },
      "devices": { "total": 2, "online": 1, "offline": 1 },
      "jobs": { "queued": 0, "running": 0 }
    }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `status` | `ok` \| `degraded` | DB 自检失败时为 `degraded`，HTTP 仍 200（探针不抖动） |
| `db.status` | `ok` \| `error` | 执行 `SELECT 1` 与迁移检查 |
| `db.path` | string | 相对数据目录的文件名，不泄露绝对路径 |
| `devices.*` | integer | 注册表统计；`online` 为最近一次探测 / 同步结果 |
| `jobs.*` | integer | `queued` + `running` 计数 |

不返回 Token、绝对路径、设备地址清单。

    curl -s "$BASE/healthz" | jq '{status, version, devices}'

错误：无（DB 故障降级为 `degraded`）。

### 4.2 `GET /devices`

查询参数：

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `online` | `true` \| `false` | 全部 | 按最近一次探测结果过滤 |
| `tag` | string | — | 精确匹配标签（可重复，多个 tag 为 AND） |
| `q` | string | — | 对 `name` / `model` / `baseUrl` 做不区分大小写子串匹配 |
| `limit` / `cursor` | — | 50 / — | 分页 |

响应 `200`：

    {
      "devices": [
        {
          "id": "pixel-9",
          "name": "Pixel 9 · 主力机",
          "baseUrl": "http://192.168.30.2:8686",
          "online": true,
          "tags": ["主力", "内网"],
          "model": "Pixel 9",
          "brand": "google",
          "android": "17",
          "sdk": 37,
          "fingerprint": "google/tokay/tokay:17/BP2A.250605.031/13000000:user/release-keys",
          "root": "kernelsu",
          "moduleVersion": "v1.10.0",
          "serverVersion": "1.10.0",
          "lastSeen": "2026-11-24T08:12:31Z",
          "lastSyncedAt": "2026-11-24T08:12:31Z",
          "latencyMs": 38,
          "createdAt": "2026-11-20T03:11:02Z",
          "lastError": null
        }
      ],
      "total": 1,
      "online": 1,
      "offline": 0,
      "nextCursor": null
    }

`total` 为**过滤后**总数；`online` / `offline` 为过滤后计数。`lastError` 形如
`{"code":"device_offline","message":"连接超时"}`，成功时 `null`。

字段表见 §5.1。排序见 §2.9。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices?online=true&tag=main" | jq '.devices[].id'

错误：401、422（查询参数非法）。

### 4.3 `POST /devices`

请求体：

    {
      "name": "Pixel 9 · 主力机",
      "baseUrl": "http://192.168.30.2:8686",
      "token": "admin123",
      "tags": ["主力", "内网"],
      "id": "pixel-9"
    }

| 字段 | 类型 | 必填 | 约束 |
| --- | --- | --- | --- |
| `name` | string | 是 | 1–64 字符，去首尾空白后非空 |
| `baseUrl` | string | 是 | 合法 `http(s)://host[:port][/prefix]`；见 §5.1 归一化规则 |
| `token` | string | 是 | 1–256 字符，不含 CR / LF；加密存储，永不回显 |
| `tags` | string[] | 否 | ≤ 8 项，每项 1–32 字符，去重保序 |
| `id` | string | 否 | 省略时由 `name` 生成；见 §5.1 |

处理顺序（必须按此顺序，保证失败不留脏数据）：

1. 校验请求体（`validation_failed`）。
2. `baseUrl` 归一化后查重 → `409 device_exists`（`field="baseUrl"`）。
3. `id` 查重 → `409 device_exists`（`field="id"`）。
4. 用给定 Token 探测：`GET {baseUrl}/api/v1/status`（10 s）与 `GET {baseUrl}/api/v1/overview`（10 s）。
   任一失败 → `422 device_probe_failed`，**不写入**。
5. 写入设备行 + overview 缓存 + 审计 `device.add`，返回 `201`。

响应 `201`：`DeviceSummary`（§5.1），与 `GET /devices` 的数组元素同形。

    201 Created
    X-Request-Id: req_...
    {"id":"pixel-9","name":"Pixel 9 · 主力机","baseUrl":"http://192.168.30.2:8686","online":true, …}

探测失败细节：

    {
      "error": {
        "code": "device_probe_failed",
        "message": "设备探测失败：连接超时",
        "details": {
          "baseUrl": "http://192.168.30.2:8686",
          "reason": "timeout",
          "upstream": null
        },
        "retryable": false,
        "requestId": "req_..."
      }
    }

`reason` 取值：`connect` | `dns` | `timeout` | `unauthorized`（设备 401）| `forbidden_scope`（设备 403）|
`upstream_status`（其他非 2xx）| `invalid_response`（2xx 但 JSON 不合法 / 缺 `device`）。`upstream` 形如
`{"status":401,"code":"unauthorized"}`，无上游响应时为 `null`。

    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"name":"Pixel 9 · 主力机","baseUrl":"http://192.168.30.2:8686","token":"admin123","tags":["主力"]}' \
      "$BASE/devices" | jq .

错误：401、409、413、415、422、500。

### 4.4 `GET /devices/{id}`

响应 `200`：`DeviceDetail` = `DeviceSummary` 的全部字段 + 下列字段：

    {
      "id": "pixel-9",
      "name": "Pixel 9 · 主力机",
      "baseUrl": "http://192.168.30.2:8686",
      "online": true,
      "tags": ["主力"],
      "model": "Pixel 9",
      "brand": "google",
      "android": "17",
      "sdk": 37,
      "fingerprint": "google/tokay/tokay:17/…",
      "root": "kernelsu",
      "moduleVersion": "v1.10.0",
      "serverVersion": "1.10.0",
      "lastSeen": "2026-11-24T08:12:31Z",
      "lastSyncedAt": "2026-11-24T08:12:31Z",
      "latencyMs": 38,
      "createdAt": "2026-11-20T03:11:02Z",
      "lastError": null,
      "overview": { "revision": "9f2c…", "device": { "…": "…" }, "apps": [], "…": "…" },
      "templates": [
        { "name": "pixel_9", "config": { "model": "Pixel 9", "brand": "google" }, "fieldsSet": ["brand","model"], "sources": ["pixel-9"], "complete": false, "warnings": [] }
      ],
      "apps": [
        { "package": "com.xingin.xhs", "installed": true, "source": { "type": "template", "template": "pixel_9", "alsoInline": false }, "model": "Pixel 9", "warnings": [] }
      ],
      "cachedAt": "2026-11-24T08:12:31Z",
      "stale": false,
      "warnings": []
    }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `overview` | `DeviceOverview` \| null | 最后一次成功拉取的设备 overview 原文；从未同步为 `null` |
| `templates` | `TemplateSummary[]` | 该设备上的模板，按设备端 `position` 升序 |
| `apps` | `DeviceAppSummary[]` | 该设备已配置应用（来自 overview.apps），按 `package` 升序 |
| `cachedAt` | string \| null | 缓存写入时间（RFC 3339 UTC） |
| `stale` | boolean | `cachedAt` 超过 300 s 或 `online=false` |
| `warnings` | `Warning[]` | 含 `stale_cache` / `device_offline` / `unsupported_device_api` |

`DeviceAppSummary`：`{package, installed: boolean|null, source: {type, template, alsoInline}, model: string|null, warnings: Warning[]}`。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices/pixel-9" | jq '{id, online, cachedAt, apps: [.apps[].package]}'

错误：401、404 `device_not_found`、500。

### 4.5 `PATCH /devices/{id}`

请求体（全部可选，至少一项）：

    { "name": "Pixel 9 · 备用机", "tags": ["备用"], "token": "new-token", "baseUrl": "http://192.168.30.9:8686" }

语义：

- `name` / `tags`：直接更新，不探测。
- `token` / `baseUrl`：用**合并后的新值**重新执行 §4.3 第 4 步探测；失败 → `422 device_probe_failed`，**整条更新回滚**。
- 新 `baseUrl` 与其它设备重复 → `409 device_exists`。
- 响应 `200`：更新后的 `DeviceSummary`。
- 审计 `device.update`，`detail` 只记录变更字段名，不含 Token 值。

    curl -s -X PATCH -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"tags":["备用","海外"]}' "$BASE/devices/pixel-9" | jq '{id, tags}'

错误：401、404、409、413、415、422、500。

### 4.6 `DELETE /devices/{id}`

- 语义：只删管理端记录（`devices` + `device_overviews` 级联；历史 `job_results` 保留用于追溯）。
  不调用设备端、不撤销设备 Token、不回滚已下发的配置。
- 响应：`204 No Content`（无响应体）。
- 审计 `device.remove`，`detail = {name, baseUrl}`。

    curl -s -o /dev/null -w '%{http_code}\n' -X DELETE -H "Authorization: Bearer $TOKEN" "$BASE/devices/pixel-9"

错误：401、404、500。

### 4.7 `POST /devices/{id}/sync`

- 语义：对单台设备实时拉取 `GET /api/v1/status` + `GET /api/v1/overview`，成功则更新
  `online`、`lastSeen`、`lastSyncedAt`、`latencyMs`、设备字段与 overview 缓存。
- 离线是**预期结果而非错误**：仍返回 `200`，`device.online=false`，`overview` 保留旧缓存，附 `device_offline` 警告。
  这样 WebUI 的「立即同步」不会弹错误对话框。

响应 `200`：

    {
      "device": { "id": "pixel-9", "online": true, "lastSyncedAt": "2026-11-24T08:30:02Z", "latencyMs": 24, "…": "…" },
      "overview": { "revision": "9f2c…", "…": "…" },
      "warnings": []
    }

离线示例：

    {
      "device": { "id": "redmi-k80", "online": false, "lastSyncedAt": "2026-11-24T06:00:00Z", "lastError": { "code": "device_offline", "message": "连接超时" }, "…": "…" },
      "overview": null,
      "warnings": [ { "code": "device_offline", "message": "Redmi K80 连接超时", "details": { "deviceId": "redmi-k80", "reason": "timeout" } } ]
    }

    curl -s -X POST -H "Authorization: Bearer $TOKEN" "$BASE/devices/pixel-9/sync" | jq '{online: .device.online, warnings}'

错误：401、404 `device_not_found`、500（离线不报错）。

### 4.8 `POST /devices/sync`（全部）

- 语义：对全部设备并发执行 §4.7（并发上限见 §2.10），单台失败不影响其它。
- 响应 `200`：

    {
      "synced": 2,
      "failed": 1,
      "results": [
        { "deviceId": "pixel-9", "status": "ok", "online": true, "latencyMs": 24, "code": null, "message": null },
        { "deviceId": "redmi-k80", "status": "offline", "online": false, "latencyMs": null, "code": "device_offline", "message": "连接超时" }
      ],
      "warnings": []
    }

`status`：`ok` | `offline` | `error`（`error` 为设备返回非预期响应或解析失败）。
审计 `device.sync_all`，`detail = {synced, failed}`。

    curl -s -X POST -H "Authorization: Bearer $TOKEN" "$BASE/devices/sync" | jq '{synced, failed}'

错误：401、500。

### 4.9 `GET /devices/{id}/overview`

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `cached` | `1` \| `true` | `false` | 只读缓存，不访问设备 |

- `cached=false`（默认）：实时请求设备 `/api/v1/overview`，成功后刷新缓存并返回。
- `cached=true`：直接返回缓存；缓存不存在 → `404 not_found`（`details.resource="device_overview"`）。

响应 `200`：

    {
      "deviceId": "pixel-9",
      "online": true,
      "cachedAt": "2026-11-24T08:30:02Z",
      "stale": false,
      "overview": { "revision": "9f2c…", "device": { "…": "…" }, "templates": [], "apps": [], "warnings": [] }
    }

`stale` 规则同 §4.4。实时请求失败 → `502 device_offline` / `504 upstream_timeout`（`cached=true` 永不因此失败）。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices/pixel-9/overview" | jq '.overview.config.revision'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices/pixel-9/overview?cached=1" | jq '.stale'

错误：401、404、502、504、500。

### 4.10 `GET /templates`

- 语义：把每台设备 overview 缓存里的 `templates[]` 按 `name` 合并成并集，**不访问设备**。
- 同名模板在不同设备上字段不一致时保留设备顺序中的第一个 `config`，并附 `template_conflict` 警告。

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `package` | string | — | 只返回 `config.packages` 含该包名的模板 |
| `device` | string | — | 只看该设备的模板 |
| `limit` / `cursor` | — | 50 / — | 分页 |

响应 `200`：

    {
      "generatedAt": "2026-11-24T08:31:00Z",
      "revision": "c0ffee…",
      "templates": [
        {
          "name": "pixel_9",
          "config": {
            "packages": ["com.xingin.xhs"],
            "brand": "google",
            "model": "Pixel 9",
            "android_version": "17",
            "sdk_int": 37,
            "dpi": 420
          },
          "fieldsSet": ["android_version", "brand", "dpi", "model", "sdk_int", "packages"],
          "sources": ["pixel-9", "xiaomi-15"],
          "complete": true,
          "warnings": []
        }
      ],
      "total": 1,
      "nextCursor": null,
      "warnings": []
    }

`complete = true` 当且仅当 `config` 同时含 `model`、`brand`、`android_version`、`sdk_int`、`dpi`。
`revision` 为聚合 revision（§5.10），无设备时为 `null`。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/templates?package=com.xingin.xhs" | jq '.templates[].name'

错误：401、422、500。

### 4.11 `GET /overview`

- 语义：总览页的一次性数据源，**只读缓存与数据库**，不访问设备。
- WebUI 首页可只调用本端点 + `/audit?limit=5`，不需要拉全量矩阵。

响应 `200`：

    {
      "generatedAt": "2026-11-24T08:31:00Z",
      "revision": "c0ffee…",
      "fleet": {
        "devices": 4,
        "online": 3,
        "offline": 1,
        "apps": 10,
        "appsConfigured": 38,
        "driftApps": 3,
        "templates": 7,
        "averageLatencyMs": 41,
        "jobs": { "queued": 0, "running": 0, "last24h": 5, "failedLast24h": 0 }
      },
      "devices": [ { "id": "pixel-9", "name": "Pixel 9 · 主力机", "online": true, "…": "…" } ],
      "drift": [
        {
          "package": "com.ss.android.ugc.aweme",
          "configured": 3,
          "total": 4,
          "models": ["Pixel 9", "Xiaomi 15"],
          "driftDevices": ["xiaomi-15"]
        }
      ],
      "recentJobs": [
        { "id": "job_…", "status": "partial", "kind": "bulk", "app": "com.xingin.xhs", "template": "pixel_9", "restart": true, "total": 4, "succeeded": 3, "skipped": 1, "failed": 0, "createdAt": "2026-11-24T08:22:10Z", "startedAt": "2026-11-24T08:22:10Z", "finishedAt": "2026-11-24T08:22:11Z", "durationMs": 812 }
      ],
      "warnings": []
    }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `fleet.devices/online/offline` | integer | 注册表统计 |
| `fleet.apps` | integer | 矩阵去重后的应用数 |
| `fleet.appsConfigured` | integer | 所有设备已配置的应用条目数（矩阵非空单元求和） |
| `fleet.driftApps` | integer | `inconsistent = true` 的应用数 |
| `fleet.templates` | integer | 模板并集数量 |
| `fleet.averageLatencyMs` | integer \| null | 在线设备 `latencyMs` 的均值（四舍五入）；无样本为 `null` |
| `fleet.jobs.last24h / failedLast24h` | integer | 滚动 24 小时窗口 |
| `devices` | `DeviceSummary[]` | 全部设备，排序同 `/devices` |
| `drift` | `DriftRow[]` | 最多 20 行；完整矩阵用 `/apps` |
| `recentJobs` | `JobSummary[]` | 最近 5 个 job |

`DriftRow = {package, configured, total, models: string[], driftDevices: string[]}`；
`driftDevices` 为与「首个非空机型」不同的设备 id。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/overview" | jq '{fleet, drift: [.drift[].package]}'

错误：401、500。

### 4.12 `GET /apps`

- 语义：应用 × 设备矩阵。行 = 已配置应用（各设备 overview 缓存 `apps[].package` 的并集）；
  列 = 全部设备；单元 = 该设备上该应用的最终生效机型。**不访问设备**。

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `q` | string | — | 对 `package` 做不区分大小写子串匹配 |
| `drift` | `true` | — | 只返回 `inconsistent=true` 的行 |
| `device` | string | — | 只返回该设备已配置的行（列仍为全部设备） |
| `limit` / `cursor` | — | 50 / — | 分页 |

响应 `200`：

    {
      "generatedAt": "2026-11-24T08:31:00Z",
      "revision": "c0ffee…",
      "devices": [
        { "id": "pixel-9", "name": "Pixel 9 · 主力机", "online": true, "model": "Pixel 9", "android": "17", "lastSyncedAt": "2026-11-24T08:12:31Z", "stale": false },
        { "id": "redmi-k80", "name": "Redmi K80 · 备用机", "online": false, "model": "Redmi K80 Pro", "android": "15", "lastSyncedAt": "2026-11-24T06:00:00Z", "stale": true }
      ],
      "apps": [
        {
          "package": "com.xingin.xhs",
          "cells": {
            "pixel-9": { "model": "Pixel 9", "template": "pixel_9", "source": "template", "installed": true, "drift": false, "warnings": [] },
            "redmi-k80": { "model": "Pixel 9", "template": "pixel_9", "source": "template", "installed": true, "drift": false, "warnings": [] }
          },
          "configured": 2,
          "total": 2,
          "inconsistent": false,
          "models": ["Pixel 9"],
          "templates": ["pixel_9"],
          "offlineDevices": ["redmi-k80"],
          "warnings": []
        },
        {
          "package": "com.ss.android.ugc.aweme",
          "cells": {
            "pixel-9": { "model": "Pixel 9", "template": "Pixel 9", "source": "template", "installed": true, "drift": false, "warnings": [] },
            "xiaomi-15": { "model": "Xiaomi 15", "template": "Xiaomi 15", "source": "template", "installed": true, "drift": true, "warnings": [] },
            "redmi-k80": null,
            "oneplus-13": { "model": "Pixel 9", "template": "Pixel 9", "source": "template", "installed": true, "drift": false, "warnings": [] }
          },
          "configured": 3,
          "total": 4,
          "inconsistent": true,
          "models": ["Pixel 9", "Xiaomi 15"],
          "templates": ["Pixel 9", "Xiaomi 15"],
          "offlineDevices": ["redmi-k80"],
          "warnings": [ { "code": "drift", "message": "com.ss.android.ugc.aweme 存在 2 种机型", "details": { "package": "com.ss.android.ugc.aweme", "models": ["Pixel 9", "Xiaomi 15"] } } ]
        }
      ],
      "totals": { "apps": 2, "appsConfigured": 5, "driftApps": 1, "devices": 4 },
      "warnings": [],
      "nextCursor": null
    }

计算规则（必须一致，UI 与测试依赖）：

1. `apps` 取所有设备缓存 overview 里 `apps[].package` 的并集，升序。
2. `total` = `devices.length`（含离线设备）。
3. `configured` = `cells` 中非 `null` 的个数。
4. `models` = 非空单元的 `model` 去重，按 `devices` 顺序首次出现排序。
5. `inconsistent = total > 0 && (configured < total || models.length > 1)`。
6. `cell.drift`：当 `inconsistent=false` 时恒 `false`；否则 `model !== models[0]`。
7. `cells[deviceId] = null` 表示该设备未配置该应用**或**没有该设备缓存（后者在 `devices[].stale=true` 与顶层 `warnings` 里给 `device_cache_missing`）。
8. `offlineDevices` 为 `online=false` 的设备 id。
9. 行级 `warnings` = 该应用所有单元的警告去重 + 漂移警告（`drift=true` 时）。
10. `nextCursor` 按行分页。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/apps?drift=true" | jq '.apps[] | {package, configured, total, models}'

错误：401、422、500。

### 4.13 `POST /apply`

请求体二选一：**bulk** 或 **assignments**。同时出现 `app` 与 `assignments` → `422 validation_failed`（`path="assignments"`）。

#### 4.13.1 bulk（一个应用 × 多设备 × 同一模板）

    {
      "app": "com.xingin.xhs",
      "devices": "all",
      "template": {
        "name": "pixel_9",
        "config": {
          "brand": "google",
          "model": "Pixel 9",
          "android_version": "17",
          "sdk_int": 37,
          "dpi": 420
        }
      },
      "restart": true
    }

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `app` | string | 是 | 包名；见 §2.7 `invalid_package` |
| `devices` | `all` \| string[] | 是 | `all` = 全部注册设备（含离线，离线得到 skipped 结果）；数组 ≤ 200，必须都是已注册 id |
| `template` | `TemplateRef` | 是 | 模板名字符串，或 `{name, config}` 对象；见 §5.6 |
| `restart` | boolean | 否，默认 `false` | 绑定成功后是否 `POST /apps/{pkg}/restart` |

归一化：`devices` 展开成 `assignments = [{device, app, template}]`，顺序即结果顺序。

#### 4.13.2 assignments（逐设备、逐应用、逐模板）

    {
      "assignments": [
        {
          "device": "pixel-9",
          "app": "com.ss.android.ugc.aweme",
          "template": "Pixel 9"
        },
        {
          "device": "xiaomi-15",
          "app": "com.tencent.mm",
          "template": {
            "name": "xiaomi_15",
            "config": {
              "brand": "Xiaomi",
              "model": "Xiaomi 15",
              "android_version": "15",
              "sdk_int": 35,
              "dpi": 440
            }
          }
        }
      ],
      "restart": false
    }

| 字段 | 类型 | 必填 | 约束 |
| --- | --- | --- | --- |
| `assignments` | `Assignment[]` | 是 | 1–200 项；`(device, app)` 不得重复（`duplicate_assignment`） |
| `assignments[].device` | string | 是 | 已注册设备 id |
| `assignments[].app` | string | 是 | 包名 |
| `assignments[].template` | `TemplateRef` | 是 | 同上 |
| `restart` | boolean | 否，默认 `false` | 作用于全部 assignment |

#### 4.13.3 响应 `202`

    {
      "jobId": "job_0193f0c2d4a87b6e9c1f5a3d2b7e8c40",
      "status": "queued",
      "kind": "bulk",
      "total": 2,
      "queued": 2,
      "createdAt": "2026-11-24T08:22:10Z",
      "pollUrl": "/relay/api/v1/jobs/job_0193f0c2d4a87b6e9c1f5a3d2b7e8c40"
    }

- `total` = 归一化后的 assignment 数；`queued` 初始等于 `total`（尚未完成的数量）。
- `kind`：`bulk` | `assignments`。
- 202 表示已入队，不代表下发成功；客户端轮询 `pollUrl`（建议 500 ms 起，指数退避，最长 60 s）。
- 带 `Idempotency-Key` 且命中时：同状态码，同响应体，附 `Idempotent-Replay: true`。

    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"app":"com.xingin.xhs","devices":"all","template":"pixel_9","restart":true}' \
      "$BASE/apply" | jq .

错误：401、409（`invalid_state`）、413、415、422、500。离线设备、包未安装、模板缺失**不**是请求错误，
它们出现在 job 结果里。

### 4.14 `GET /jobs/{id}`

响应 `200`：

    {
      "id": "job_0193f0c2d4a87b6e9c1f5a3d2b7e8c40",
      "status": "partial",
      "kind": "bulk",
      "app": "com.xingin.xhs",
      "template": "pixel_9",
      "restart": true,
      "total": 2,
      "succeeded": 1,
      "skipped": 1,
      "failed": 0,
      "createdAt": "2026-11-24T08:22:10Z",
      "startedAt": "2026-11-24T08:22:10Z",
      "finishedAt": "2026-11-24T08:22:11Z",
      "durationMs": 421,
      "requestId": "req_3f1c…",
      "idempotencyKey": null,
      "warnings": [],
      "results": [
        {
          "deviceId": "pixel-9",
          "deviceName": "Pixel 9 · 主力机",
          "app": "com.xingin.xhs",
          "status": "ok",
          "code": null,
          "httpStatus": 200,
          "revision": "1a7b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9",
          "attempts": 2,
          "durationMs": 210,
          "steps": [
            { "step": "revision", "status": "ok", "httpStatus": 200, "code": null, "durationMs": 11 },
            { "step": "template_put", "status": "ok", "httpStatus": 201, "code": null, "durationMs": 42 },
            { "step": "bind", "status": "ok", "httpStatus": 200, "code": null, "durationMs": 38 },
            { "step": "restart", "status": "ok", "httpStatus": 200, "code": null, "durationMs": 90 }
          ],
          "restart": { "status": "ok", "httpStatus": 200, "code": null, "durationMs": 90 },
          "warnings": [],
          "error": null
        },
        {
          "deviceId": "redmi-k80",
          "deviceName": "Redmi K80 · 备用机",
          "app": "com.xingin.xhs",
          "status": "skipped",
          "code": "device_offline",
          "httpStatus": null,
          "revision": null,
          "attempts": 1,
          "durationMs": 3001,
          "steps": [
            { "step": "revision", "status": "failed", "httpStatus": null, "code": "device_offline", "durationMs": 3000 }
          ],
          "restart": null,
          "warnings": [],
          "error": {
            "code": "device_offline",
            "message": "Redmi K80 连接超时",
            "details": { "deviceId": "redmi-k80", "reason": "timeout" },
            "retryable": true
          }
        }
      ]
    }

- 运行中：`results` 只含**已完成**的设备（按 assignment 顺序），`succeeded + skipped + failed` 为已完成数；
  `status="running"`。`queued` 阶段 `results` 为空。
- 结果字段表见 §5.7。
- 未知 id → `404 not_found`（`details = {resource: "job", name: "<id>"}`）。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/jobs/job_0193…" | jq '{status, results: [.results[] | {deviceId, status, code}]}'

错误：401、404、500。

### 4.15 `GET /jobs`

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `status` | string | — | 逗号分隔：`queued,running,succeeded,partial,failed`；非法值 → 422 |
| `kind` | string | — | `bulk` \| `assignments` |
| `app` | string | — | 精确匹配 job 的 bulk app（assignments job 为 null） |
| `device` | string | — | 只返回逐设备结果中包含该设备的 job |
| `limit` / `cursor` | — | 50 / — | 分页 |

响应 `200`：`{ "items": [JobSummary], "jobs": [JobSummary], "total": 5, "nextCursor": null }`
（`items` 为规范字段；`jobs` 是兼容旧前端的别名，二者内容一致；`JobSummary` 不含 `results`，见 §5.7）。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/jobs?status=partial,failed&limit=5" | jq '.items[] | {id, status}'

错误：401、422、500。

### 4.16 `GET /audit`

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `action` | string | — | 精确匹配，逗号分隔（如 `device.add,device.remove`） |
| `target` | string | — | `<type>:<id>`（如 `device:pixel-9`）或纯 id；与 `target_type`/`target_id` 匹配 |
| `since` | string | — | RFC 3339；返回 `ts >= since`；非法时间 → 422 |
| `limit` / `cursor` | — | 50 / — | 分页；最大 500 |

响应 `200`：

    {
      "entries": [
        {
          "id": 12,
          "ts": "2026-11-24T08:22:10Z",
          "actor": "manager-token",
          "action": "apply.bulk",
          "target": { "type": "device", "id": "all" },
          "requestId": "req_3f1c…",
          "status": "ok",
          "detail": { "jobId": "job_…", "total": 4 }
        }
      ],
      "total": 1,
      "nextCursor": null
    }

`action` 枚举：`auth.failed`、`device.add`、`device.update`、`device.remove`、`device.sync`、`device.sync_all`、
`apply.bulk`、`apply.assignments`、`job.finished`、`token.rotate`。
入队成功后写 `apply.bulk`（target `device:all` 或 `device:multiple`，detail 含 `jobId/total/app/template`）
或 `apply.assignments`（target `device:mixed`，detail 含 `jobId/total/assignments`）；审计失败只记 warn，不影响 202。
`status`：`ok` | `failed`。`detail` 是 JSON 对象或 `null`，**禁止**包含任何 Token、密文、完整 Authorization 头。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/audit?action=device.add&limit=5" | jq '.entries[] | {ts, action, status}'

错误：401、422、500。

### 4.17 `POST /token/rotate`

- 语义：生成或接收新的 Manager Token，写入 `<data-dir>/manager.token`（0600，原子替换），
  并立即热替换鉴权状态；旧 Token 当场失效。
- 请求体可省略；可带自定义 Token：

      { "token": "my-long-random-token" }

  `token` 约束：16–128 字符，不含空白与 CR/LF。省略时生成 `dfm_` + 32 位 base64url 随机串（192 bit 熵）。
- 响应 `200`（新 Token **只在此处出现一次**）：

      {
        "token": "dfm_7Kx9pQ2mN4vR8sT1wY6cB3hJ5aZ0dF4g",
        "rotatedAt": "2026-11-24T08:40:00Z",
        "tokenPersisted": true,
        "source": "rotated"
      }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `token` | string | 新 Token；客户端必须立即替换本地存储 |
| `tokenPersisted` | boolean | 是否成功写入 `manager.token`；写失败时 `false`（内存已生效，重启后失效） |
| `source` | `rotated` | 便于前端日志 |

- 请求体畸形（`[]`、非法 JSON、未知字段）或非 `application/json` → 统一 JSON 4xx（422/415），**不会**轮换 Token。
- 来源为 `--token` / `DFM_TOKEN` 时 → `409 invalid_state`（`details.reason="token_overridden"`）。
- WebUI 轮换后必须用新 Token 重新请求；页面提示「旧 Token 立即失效」。
- 审计 `token.rotate`，`detail` 不含 Token。

    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{}' "$BASE/token/rotate" | jq '{tokenPersisted, source}'

错误：401、409、413、415、422、500。

### 4.18 `ANY /devices/{id}/proxy/api/v1/{*path}`

- 语义：把请求原样转发到 `{baseUrl}/api/v1/{path}`，用于本契约未封装的设备端能力
  （例如 `DELETE /apps/{pkg}?confirm=true`、`GET /packages`、`GET /config`）。
- 路径参数 `path` 必须非空；`/devices/{id}/proxy/api/v1`（无尾段）→ `404 not_found`（`resource="route"`）。
- 转发内容：method、query string、请求体。请求头白名单：
  `Content-Type`、`Accept`、`If-Match`、`Idempotency-Key`、`X-Request-Id`、`Range`。
  其它头丢弃（含客户端的 `Authorization`、`Cookie`、`Host`、`Origin`）。
- Relay 注入 `Authorization: Bearer <解密后的设备 Token>` 与 `User-Agent: device-faker-manager/0.1.0`（UA 是 `device_client.rs` 里的独立常量，不随 crate 版本变化；`/healthz` 的 `version` 才是 0.2.0）。
- 响应：2xx 与 4xx（除 401/403）**原样透传**状态码与响应体；复制 `Content-Type`、`X-DF-Revision`、
  `ETag`、`Retry-After`；设备端 `X-Request-Id` 放入 `X-DF-Upstream-Request-Id`，Relay 自己回填 `X-Request-Id`。
- 设备 401/403 → Relay 返回 `502 device_error`（`details = {deviceId, upstreamStatus: 401|403, upstreamCode}`），
  避免浏览器把设备 Token 问题误判成 Manager 登录失效。
- 设备不可达 → `502 device_offline`；超时 → `504 upstream_timeout`；响应 > 8 MiB → `502 device_error`
  （`details.reason="response_too_large"`）。
- 路径含 `..` / `.` / 空段 / 反斜杠 → `404 not_found`（`details.resource="route"`），不会拼接出 `baseUrl` 之外的 URL。
- 设备返回的 4xx（如 400 `validation_failed`、404 `not_found`）原样透传状态码与 JSON；仅 401/403 重映射为 502。
- 代理**不**写审计（避免高频噪声）；不下发到未注册设备。

    # 解释某台设备上应用的生效来源
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices/pixel-9/proxy/api/v1/apps/com.xingin.xhs" | jq '{source, resolved}'

    # 清除某台设备上的应用配置（设备端需要 confirm=true）
    curl -s -X DELETE -H "Authorization: Bearer $TOKEN" \
      "$BASE/devices/pixel-9/proxy/api/v1/apps/com.xingin.xhs?confirm=true&detach_templates=true" | jq .

错误：401、404、413、502、504、500。

> 模板比较忽略 `packages`；对象形态 PUT 前若请求未显式给 `packages`，会合并目标设备现有值，避免解绑其它应用。
>
> 原型 `RelayPage.vue` 里的 `/devices/{id}/api/v1/overview` 是早期草案；冻结路径为
> `/devices/{id}/proxy/api/v1/overview`（或直接用 `/devices/{id}/overview`）。前端页面文案与端点表已按此更新。

---

### 4.19 `GET /egress-proxies`

查询参数：

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `q` | string | — | 对 `name` / `host` / `region` 做不区分大小写子串匹配 |
| `enabled` | `true`/`false` | 全部 | 按启用位过滤 |
| `tag` | string | — | 精确匹配标签；可重复，多个 tag 为 AND |
| `limit` / `cursor` | — | 50 / — | 分页（§2.9） |

响应 `200`：`{total, nextCursor, items: EgressProxy[]}`，按 `name` 升序（ASCII）。`EgressProxy` 字段见 §5.11，**永不含** `username` / `password`。

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress-proxies?enabled=true" | jq '.items[] | {name, hasCredentials, boundDevices}'

错误：401、422、500。

### 4.20 `POST /egress-proxies`

请求体（严格模式，未知字段 → 422）：

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `name` | string | 是 | `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`，全局唯一；下发时 1:1 作为设备端 `egress.toml` 的表名 |
| `type` | `"http"` \| `"socks5"` | 是 | 协议（DB 列名是 `kind`，线上叫 `type`） |
| `host` | string | 是 | ≤253；主机名或 IPv4，不含 scheme / 端口 / 路径 |
| `port` | integer | 是 | 1..=65535 |
| `username` | string | 否 | ≤128；**只写不读**。仅存储，本期应用流量不使用 |
| `password` | string | 否 | ≤256；**只写不读**，AES-256-GCM 落库（§7.3）。仅存储，本期应用流量不发送认证 |
| `region` | string | 否 | ≤64，自由文本（如 `US-LA`） |
| `tags` | string[] | 否 | ≤16 项，每项 ≤32 字符 |
| `enabled` | boolean | 否 | 默认 `true`。`false` 时**不允许新建绑定**，已有绑定仍会下发（避免误停导致断网） |

响应 `201`：`EgressProxy`。审计 `egress.proxy_create`（`detail={proxyId, name, changed}`，不含 host / 凭据）。

错误：401、409 `egress_proxy_exists`、413、415、422（`invalid_egress_name` / `invalid_egress_proxy` / `too_many_items`）、500。

> **认证限制（本期）**：`username` / `password` 可以录入并加密落库，但**设备端应用流量不会发送认证**（SOCKS5：属性可读而 libcore 不发送，实测代理侧 `methods=0002 user_ok=0 pass_ok=0` → `auth-failed`；HTTP Basic 未实现）。因此带凭据的代理请在代理服务端配 **IP 白名单**授权，或等后续「设备内本地认证中继」（P2，尚未实现）。Manager 侧 `POST /egress-proxies/{id}/probe` 只支持 HTTP 代理、不验证 SOCKS5 认证。

### 4.21 `GET /egress-proxies/{id}`

响应 `200`：`EgressProxy` 的全部字段 + `bindings[]`（引用它的 `{deviceId, deviceName, app, updatedAt}`，按 `deviceId, app` 升序）+ `recentChecks[]`（最近 20 条 `EgressCheck`，`checkedAt` 降序）。

错误：401、404 `not_found`（`details.resource="egress_proxy"`）、500。

### 4.22 `PATCH /egress-proxies/{id}`

Merge-Patch 风格，但凭据字段是**双态**：

| 字段 | 省略 | `null` | 有值 |
| --- | --- | --- | --- |
| `name` / `type` / `host` / `port` / `enabled` | 不改 | 422（不可置空） | 覆盖 |
| `region` / `tags` | 不改 | 清除（`null` / `[]`） | 覆盖 |
| `username` / `password` | **不改（已存密文保留）** | **清除** | 覆盖（重新加密，新 nonce） |

- 改名不会自动同步到设备：下次下发会按新名字写 `egress.toml`（整份 `proxies` 替换，旧表名自然消失），响应附 `egress_rename_pending` 警告。
- 改 `type`/`host`/`port`/凭据同理：只落库，需要 `POST /devices/egress/apply` 才下发。

响应 `200`：`EgressProxy` + `warnings[]`。审计 `egress.proxy_update`。

错误：401、404、409 `egress_proxy_exists`（改名撞车）、413、415、422、500。

### 4.23 `DELETE /egress-proxies/{id}`

- 仍被绑定 → `409 egress_proxy_in_use`，`details{proxyId, name, total, devices:[…], apps:[…]}`（`devices` 最多 20 项，`total` 给全量）。
- 成功 → `204 No Content`；`egress_checks.proxy_id` 按 `ON DELETE SET NULL` 保留历史行。审计 `egress.proxy_delete`。

错误：401、404、409、500。

### 4.24 `POST /egress-proxies/{id}/probe`

- 用途：**Manager 侧直连探测**，验证代理本身可用（与设备链路无关）。
- 请求体可省略：`{"url": "http://…"}`；默认 `http://checkip.amazonaws.com/`，只接受 `http://`、≤512 字符。
- 执行：用该代理的 `type/host/port/凭据`（解密后仅在内存）发一次 GET，5 s 超时；**成功与失败都写** `egress_checks(source="manager")`。
- 只支持 `http` 代理：`socks5` → `422`（`details.errors[].code="unsupported_proxy_type"`），并写一条 `ok=0, error_code="unsupported_proxy_type"`。

响应 `200`（成功）：`{proxyId, ok:true, exitIp, latencyMs, checkedAt, url, upstreamStatus, warnings}`——
`warnings` **恒在**（没东西时是 `[]`）；探测结果没写进 `egress_checks` 时带一条 `egress_check_not_recorded`（`{proxyId, source:"manager"}`），**探测本身仍按真实结果返回 200**。

失败：`502 egress_probe_failed`，`details{proxyId, url, reason, timeoutMs, upstreamStatus?}`；`reason` ∈ `connect`/`dns`/`timeout`/`tls`/`http_status`/`invalid_response`。

**失败响应的形状是「信封 + 顶层 `warnings`」**（不是纯信封；`warnings` 恒在，没有内容时 `[]`）：

    {
      "error": {
        "code": "egress_probe_failed",
        "message": "探测失败",
        "details": { "proxyId": "egp_3f1c…", "url": "http://checkip.example.com/", "reason": "timeout", "timeoutMs": 5000, "upstreamStatus": null },
        "retryable": true,
        "requestId": "req_3f1c…"
      },
      "warnings": [
        { "code": "egress_check_not_recorded", "message": "探测结果未能写入 egress_checks（数据库写入失败）",
          "details": { "proxyId": "egp_3f1c…", "source": "manager" } }
      ]
    }

`422 unsupported_proxy_type` 仍是**纯信封**（没有顶层 `warnings`）。

错误：401、404、422、502、500。审计 `egress.proxy_probe`（`detail={proxyId, name, changed:false}`）。

> **解密失败不是新码**：代理凭据无法解密时返回 `500 internal_error`（`details.op="decrypt"`，`retryable:true` 但重试无意义——只能重新录入凭据）。探测结果**写入失败**时响应体 `warnings` 带 `egress_check_not_recorded`（`{proxyId, source:"manager"}`），探测本身仍按真实结果返回。

### 4.25 `GET /devices/{id}/egress`

- 返回该设备的**绑定**与**设备侧实时状态**。设备在线时实时 `GET /api/v1/egress`（读超时 10 s）；仅当 HTTP 请求失败/超时才给 `device_offline`，旧版本（404）给 `egress_unsupported`。
- **设备 Token 无法解密**（`manager.key` 变更 / 密文损坏）时**不再归入 `device_offline`**：
  - `warnings` 里给 `{code:"device_token_invalid", message:"设备 Token 解密失败（manager.key 可能已变更，需要重新登记设备的 Token）：…", details:{deviceId, code:"device_token_invalid", op:"decrypt"}}`（与 job 结果码 `device_token_invalid` 同名同根因）；
  - `deviceState` 仍是 `null`（拿不到设备侧状态）；
  - **`bindings` 照常返回**——绑定是先在库里读出来的，不因 Token 解不开而丢；
  - 响应的 HTTP 状态仍是 `200`（这是可诊断的预期结果，不是 5xx）。

响应 `200`：

    {
      "deviceId": "pixel-9",
      "bindings": [
        { "app": "",             "mode": "proxy",  "proxyId": "egp_3f1c…", "proxyName": "office_http", "updatedAt": "2026-12-01T02:00:00Z" },
        { "app": "com.example.b", "mode": "direct", "proxyId": null,         "proxyName": null,          "updatedAt": "2026-12-01T02:00:00Z" }
      ],
      "deviceState": {
        "revision": "5b1f…", "configRevision": "9f2c…", "configured": true,
        "mode": "proxy", "effectiveMode": "proxy", "default": "office_http", "applied": true,
        "readback": { "settingsValue": "198.51.100.10:8080", "matches": true, "at": "2026-12-01T03:00:00Z" },
        "health": { "degraded": false, "lastProbe": { "ok": true, "exitIp": "198.51.100.77" } }
      },
      "warnings": []
    }

- `deviceState` 是设备端 `GET /egress` 的**透传子集**（原样转发，不裁剪 Relay 不认识的字段；遵循 §2.12）。
- `bindings[].app=""` 表示整机（方案 A）；`mode="direct"` 时 `proxyId`/`proxyName` 为 `null`。

错误：401、404 `device_not_found`、500（离线不报错）。

### 4.26 `PUT /devices/{id}/egress`

- **只落库**（全量替换该设备的绑定表），不下发；响应 `pendingApply:true`，真正的网络变更走 §4.27。

请求体：

    {
      "bindings": [
        { "app": "",              "proxyId": "egp_3f1c…" },
        { "app": "com.example.a", "proxyId": "egp_7a2b…" },
        { "app": "com.example.b", "proxyId": "__DIRECT__" },
        { "app": "com.example.c", "proxyId": null }
      ]
    }

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `bindings` | object[] | 是 | ≤200 项；同 `app` 唯一；`[]` = 清空 |
| `bindings[].app` | string | 是 | `""`（整机）或合法包名 |
| `bindings[].proxyId` | string \| `"__DIRECT__"` \| null | 是 | 代理 id / 强制直连 / `null` = 该行不保留 |

判定：

1. `app=""` + `"__DIRECT__"`/`null` → 不写库（整机直连等于没有绑定）。
2. 代理不存在 → 422 `invalid_value`；代理 `enabled=false` 且该行是**新增** → 422（已存在的行保留）。
3. 同一请求重放 → `changed:false`，`200`。
4. 绑定的代理若带凭据，本轮下发仍会成功，但**应用流量不会通过认证**（见 §4.20 末注）——上线前请先用探测与真机流量确认。

响应 `200`：`{deviceId, changed, bindings[], pendingApply, warnings[]}`。审计 `egress.bind`（`detail={deviceId, changed}`）。

错误：401、404 `device_not_found`、413、415、422、500。

### 4.27 `POST /devices/egress/apply`

- 把**当前绑定 + 代理定义**推送到设备，返回 `202` + job（`kind="egress"`）。两种形态二选一（同时出现 → 422）：

形态 A：bulk（下发现有绑定）

    { "devices": "all", "probe": true }

形态 B：assignments（先落库再下发，用于「批量绑定」）

    {
      "assignments": [
        { "deviceId": "pixel-9",   "app": "",              "proxyId": "egp_3f1c…" },
        { "deviceId": "redmi-k80", "app": "com.example.a", "proxyId": "__DIRECT__" }
      ],
      "probe": true
    }

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `devices` | `"all"` \| string[] | — | 形态 A 必填；`"all"` 按设备 `createdAt` 升序展开 |
| `assignments` | object[] | — | 形态 B 必填；≤200 项，每项 `{deviceId, app, proxyId}` |
| `probe` | boolean | `false` | 下发后是否 `POST /api/v1/egress/probe`；失败只记 `egress_probe_failed` 警告，不改结果 |
| `dryRun` | boolean | `false` | 只计算与校验、不落库、不下发；返回 `200` + `{dryRun, total, plan[], warnings}`（`plan[]` 每项 `{deviceId, app, mode, proxyName}`），不建 job |

响应 `202`：`{jobId, status:"queued", kind:"egress", total, createdAt, requestId}`。支持 `Idempotency-Key`（同键直接返回首次 job + `Idempotent-Replay: true`，不设 TTL）。审计 `egress.apply`（`detail={deviceId, changed}` 或 `{proxyId, name}`，不含凭据）。

错误：401、413、415、422（`missing_required` / `invalid_value` / `too_many_items` / `invalid_package`）、500。

---

## 5. 数据结构（完整字段表）

### 5.1 `DeviceSummary`

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `id` | string | 否 | `^[a-z0-9][a-z0-9_-]{0,63}$`。省略时由 `name` 生成：小写、`[^a-z0-9]+` → `-`、去首尾 `-`、截断 48；结果为空（如纯中文名）→ `device-<6 位 hex>` |
| `name` | string | 否 | 1–64 字符 |
| `baseUrl` | string | 否 | 归一化后的地址：小写 scheme/host、去默认端口、去尾部 `/`、禁止 userinfo / query / fragment；允许路径前缀，如 `http://10.0.0.5:8686/df` |
| `online` | boolean | 否 | 最近一次探测 / 同步 / 下发接触是否成功 |
| `tags` | string[] | 否 | 去重保序，≤ 8 项 |
| `model` | string | 是 | 来自设备 `device.model` |
| `brand` | string | 是 | 来自设备 `device.brand`（设备端为小写品牌名） |
| `android` | string | 是 | `device.android` |
| `sdk` | integer | 是 | `device.sdk` |
| `fingerprint` | string | 是 | `device.fingerprint` |
| `root` | string | 是 | `root.manager`（`kernelsu` / `magisk` / `apatch`）；无 root 块为 `null` |
| `moduleVersion` | string | 是 | `module.version`（如 `v1.10.0`） |
| `serverVersion` | string | 是 | `server.version`（如 `1.10.0`） |
| `lastSeen` | string | 是 | 最近一次成功接触（RFC 3339 UTC，秒精度） |
| `lastSyncedAt` | string | 是 | 最近一次成功拉取 overview 的时间 |
| `latencyMs` | integer | 是 | 最近一次探测 / 同步耗时（四舍五入，毫秒） |
| `createdAt` | string | 否 | 注册时间 |
| `lastError` | `{code, message}` | 是 | 最近一次失败；成功后清空 |
| `egress` | `DeviceEgressSummary` \| null | 是 | 设备级出口摘要（只增不改）：`{mode, proxyId, proxyName, exitIp, checkedAt, ok, errorCode}`；没有任何绑定时为 `null` |

`baseUrl` 在校验阶段拒绝：非 `http`/`https`、含 `@`（userinfo）、含 `?` / `#`、空 host、
以及 `file:` / `ftp:` 等 scheme。查询参数与片段一律不允许（设备端 API 不需要）。

### 5.2 `DeviceOverview`（设备端 `/api/v1/overview` 响应，原样透传）

顶层结构（与设备契约一致，管理器只读不改造）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `revision` | string | 设备配置 revision（64 位小写 hex，或带 `sha256:` 前缀） |
| `module` | `{id, version, versionCode}` | 模块信息 |
| `device` | `{manufacturer, brand, model, device, android, sdk, abi, fingerprint}` | 物理设备信息 |
| `root` | `{manager, version}` \| null | root 管理器 |
| `server` | `{version, bind, uptimeSec, startedAt, pid}` | 设备服务信息 |
| `config` | `{revision, mtime, size, exists, templates, apps, conflicts}` | 配置元信息 |
| `templates` | `{name, position, packages, config}[]` | `config` 是 `TemplateInput`（不含 `packages`） |
| `apps` | `{package, installed, source, resolved, warnings}[]` | `source = {type, template, alsoInline}`；`resolved` 见设备契约 §6.4 |
| `warnings` | `Warning[]` | 设备端警告 |

本增量 Relay 不修改该对象；设备端新增字段会随缓存保留。设备端契约见 `docs/API.md` §5.24。

### 5.3 `TemplateInput`（模板对象，snake_case）

`template.config` 与设备端 `PUT /api/v1/templates/{name}` 的请求体**完全一致**（`TemplateInput`），
**不含模板名**；模板名在 Relay 层由 `TemplateRef.name` 或 `TemplateSummary.name` 承担。

| 字段 | 类型 | 约束 | config.toml |
| --- | --- | --- | --- |
| `packages` | string[] | 每项包名（可带 `@userId`）；≤ 200；默认 `[]` | `packages` |
| `manufacturer` | string | ≤ 512 | `manufacturer` |
| `marketname` | string | ≤ 512 | `marketname` |
| `brand` | string | ≤ 512 | `brand` |
| `model` | string | ≤ 512 | `model` |
| `name` | string | ≤ 512；**设备内部产品名**（`ro.product.name`），不是模板名 | `name` |
| `device` | string | ≤ 512 | `device` |
| `product` | string | ≤ 512 | `product` |
| `hardware` | string | ≤ 512 | `hardware` |
| `board` | string | ≤ 512 | `board` |
| `soc_model` | string | ≤ 512 | `soc_model` |
| `fingerprint` | string | ≤ 512 | `fingerprint` |
| `build_id` | string | ≤ 512 | `build_id` |
| `display_id` | string | ≤ 512 | `display_id` |
| `incremental` | string | ≤ 512 | `incremental` |
| `security_patch` | string | `YYYY-MM-DD` | `security_patch` |
| `characteristics` | string | ≤ 512 | `characteristics` |
| `android_version` | string | `^[0-9]{1,2}(\.[0-9]{1,2}){0,2}$` | `android_version` |
| `sdk_int` | integer \| `__DELETE__` | 1–1000 | `sdk_int` |
| `dpi` | integer | 120–640 | `dpi` |
| `custom_props` | object | 键 `^(ro\|persist)\.`，值 ≤ 1024 字符，≤ 128 项 | `custom_props` |
| `force_denylist_unmount` | boolean | — | `force_denylist_unmount` |
| `hide_maps` | string[] | 每项 ≤ 256，≤ 64 项 | `hide_maps` |
| `companion_resetprop` | boolean | — | `companion_resetprop` |

标记值：`""` / `"__EMPTY__"` 写空串；`"__DELETE__"` 删除对应属性族（仅 `companion_resetprop=true` 时真正删除）。
未知字段 → `422 unknown_field`（设备端同样拒绝）。

### 5.4 `TemplateSummary`（Relay 聚合出的模板）

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `name` | string | 否 | 模板名（配置键），`^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$` |
| `config` | `TemplateInput` | 否 | 合并后的模板字段；同名冲突取设备顺序中的第一份 |
| `fieldsSet` | string[] | 否 | `config` 中显式出现的字段名（升序） |
| `sources` | string[] | 否 | 声明该模板的设备 id，按设备顺序 |
| `complete` | boolean | 否 | `model`、`brand`、`android_version`、`sdk_int`、`dpi` 均存在 |
| `warnings` | `Warning[]` | 否 | 冲突 / 缓存缺失 |

### 5.5 矩阵对象

`AppsMatrixResponse`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `generatedAt` | string | 生成时间（缓存快照时间） |
| `revision` | string \| null | 聚合 revision（§5.10） |
| `devices` | `MatrixDevice[]` | 列定义，顺序同 `/devices` |
| `apps` | `MatrixApp[]` | 行，按 `package` 升序 |
| `totals` | `{apps, appsConfigured, driftApps, devices}` | 汇总 |
| `warnings` | `Warning[]` | 全局警告（缓存缺失等） |
| `nextCursor` | string \| null | 行分页 |

`MatrixDevice`：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `id` | string | 否 | 设备 id |
| `name` | string | 否 | 名称 |
| `online` | boolean | 否 | 最近一次接触结果 |
| `model` | string | 是 | 设备机型 |
| `android` | string | 是 | Android 版本 |
| `lastSyncedAt` | string | 是 | 缓存时间 |
| `stale` | boolean | 否 | 缓存 > 300 s 或离线 |

`MatrixApp`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `package` | string | 包名 |
| `cells` | `Record<string, MatrixCell \| null>` | 键 = 设备 id；`null` = 未配置或无缓存 |
| `configured` | integer | 非空单元数 |
| `total` | integer | `devices.length` |
| `inconsistent` | boolean | §4.12 规则 5 |
| `models` | string[] | 去重机型，按设备顺序 |
| `templates` | string[] | 出现的模板名，按设备顺序去重 |
| `offlineDevices` | string[] | 离线设备 id |
| `warnings` | `Warning[]` | 行级警告 |

`MatrixCell`：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `model` | string | 是 | `resolved.model`（最终生效机型） |
| `template` | string | 是 | 生效模板名；内联配置为 `null` |
| `source` | `template` \| `inline` \| `none` | 否 | 生效来源 |
| `installed` | boolean | 是 | 设备端 `installed`；`pm` 不可用时 `null` |
| `drift` | boolean | 否 | §4.12 规则 6 |
| `warnings` | `Warning[]` | 否 | 设备端警告 |

### 5.6 `TemplateRef` 与 `Assignment`

    type TemplateRef =
      | string                                    // 已存在的模板名（目标设备必须有）
      | { name: string; config: TemplateInput };   // 名字 + 完整定义（不存在则先创建）

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `name` | string | 是（对象形态） | 模板名；`invalid_template_name` |
| `config` | `TemplateInput` | 是（对象形态） | 至少包含 `model`，否则 `422 validation_failed`（`path="template.config.model"`） |

对象形态中，`name` 是**模板名**；`config.name` 若出现是**设备内部产品名**，二者不冲突。
比较模板差异时忽略 `packages`；`config` 未显式给 `packages` 时，Relay 会把目标设备现有的 `packages` 合并回 PUT，避免解绑其它应用。

`Assignment`：

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `device` | string | 是 | 设备 id |
| `app` | string | 是 | 包名 |
| `template` | `TemplateRef` | 是 | 模板名或定义 |

### 5.7 `Job`、`JobResult`、`JobStep`、`JobSummary`

`Job`：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `id` | string | 否 | `^job_[0-9a-f]{32}$`，不透明 |
| `status` | `queued` \| `running` \| `succeeded` \| `partial` \| `failed` | 否 | 状态机见 §7.4 |
| `kind` | `bulk` \| `assignments` \| `egress` | 否 | 请求形态（`egress` = 出口下发，见 §7.7） |
| `app` | string | 是 | bulk 的应用；assignments 为 `null` |
| `template` | string | 是 | bulk 的模板名（对象形态取 `name`）；assignments 为 `null` |
| `restart` | boolean | 否 | 是否请求重启 |
| `total` | integer | 否 | assignment 总数（≥ 1） |
| `succeeded` | integer | 否 | `status="ok"` 的数量 |
| `skipped` | integer | 否 | `status="skipped"` 的数量 |
| `failed` | integer | 否 | `status="failed"` 的数量 |
| `createdAt` | string | 否 | 创建时间 |
| `startedAt` | string | 是 | 开始执行 |
| `finishedAt` | string | 是 | 结束时间 |
| `durationMs` | integer | 是 | `finishedAt - startedAt` |
| `requestId` | string | 否 | 触发请求的 requestId（与审计对齐） |
| `idempotencyKey` | string | 是 | 请求带的幂等键原值 |
| `warnings` | `Warning[]` | 否 | job 级警告 |
| `results` | `JobResult[]` | 否 | 逐设备结果；运行中只含已完成项 |

`JobResult`：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `deviceId` | string | 否 | 目标设备 |
| `deviceName` | string | 否 | 设备当前名称；设备已删除时回落为 `deviceId` |
| `app` | string | 否 | 目标包名 |
| `status` | `ok` \| `skipped` \| `failed` | 否 | 见 §7.3 |
| `code` | string | 是 | 失败 / 跳过时的稳定码（§2.8），成功为 `null`；egress 幂等短路（设备返回 `changed:false`）时结果为 `ok` 但 `code="egress_unchanged"` |
| `httpStatus` | integer | 是 | bind 步骤的 HTTP 状态；未发出请求为 `null` |
| `revision` | string | 是 | 绑定成功后的设备 revision |
| `attempts` | integer | 否 | bind 尝试次数（1 或 2） |
| `durationMs` | integer | 否 | 该设备总耗时 |
| `steps` | `JobStep[]` | 否 | 执行轨迹，见下 |
| `restart` | `RestartResult` | 是 | 未请求重启或 bind 未成功时为 `null` |
| `warnings` | `Warning[]` | 否 | 设备端警告 + `restart_failed` |
| `error` | `{code, message, details, retryable}` | 是 | 失败 / 跳过时的错误对象；成功为 `null` |

> 数据库里还有 `status='pending'` 的占位行（尚未执行 / 崩溃时未完成），**不属于 API 契约**：
> `results` 只返回 `ok` / `skipped` / `failed` 三种状态。运行中的进度用计数表达（见 §4.14）。

`JobStep`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `step` | `revision` \| `template_get` \| `template_put` \| `bind` \| `restart` \| `egress_get` \| `egress_put` \| `egress_probe` | 步骤 |
| `status` | `ok` \| `skipped` \| `failed` | 结果 |
| `httpStatus` | integer \| null | 设备返回状态 |
| `code` | string \| null | 失败 / 跳过码 |
| `durationMs` | integer | 耗时 |

`RestartResult`：`{status: "ok"|"skipped"|"failed", httpStatus: number|null, code: string|null, durationMs: number}`。

`JobSummary`：`{id, status, kind, app, template, restart, total, succeeded, skipped, failed, createdAt, startedAt, finishedAt, durationMs}`（无 `results` / `warnings`）。

### 5.8 `AuditEntry`

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `id` | integer | 否 | 自增主键，倒序分页 |
| `ts` | string | 否 | RFC 3339 UTC |
| `actor` | string | 是 | 预留；本增量恒为 `manager-token` |
| `action` | string | 否 | §4.16 枚举 |
| `target` | `{type, id}` | 否 | `type` = `device` \| `job` \| `template` \| `token`；`id` 可为 `null` |
| `requestId` | string | 是 | 触发请求 id（`auth.failed` 也记录） |
| `status` | `ok` \| `failed` | 否 | 操作结果 |
| `detail` | object \| null | 是 | 结构化附加信息；禁止 Token |

### 5.9 `Warning`

    { "code": "stale_cache", "message": "Redmi K80 的缓存已过期 2 小时", "details": { "deviceId": "redmi-k80", "ageSec": 7200 } }

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `code` | string | 是 | §2.8 表中的码 |
| `message` | string | 是 | 人类可读 |
| `details` | object \| null | 是 | 结构化上下文 |

### 5.10 聚合 revision

`/apps`、`/templates`、`/overview` 的顶层 `revision` 是**聚合 revision**，用于前端判断「缓存有没有变化」：

    revision = sha256( concat( sort(deviceId) 的 "deviceId:deviceRevision" + "\n" ) )

- 只计入**有 overview 缓存**的设备；`deviceRevision` 取 `DeviceOverview.config.revision`（或顶层 `revision`）。
- 无任何缓存 → `null`。
- 客户端必须把它当作不透明字符串，不要自行重算。它与设备端 revision 不同层级，不能拿去当 `If-Match`。

---

### 5.11 `EgressProxy`

列表项（§4.19）、创建/更新响应（§4.20 / §4.22）与详情（§4.21）共用。**没有** `username` / `password` 字段。

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `id` | string | 否 | `^egp_[0-9a-f]{32}$` |
| `name` | string | 否 | `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`，全局唯一；下发时作为设备端 `egress.toml` 表名 |
| `type` | `"http"` \| `"socks5"` | 否 | 协议（DB 列 `kind`） |
| `host` | string | 否 | ≤253，不含 scheme |
| `port` | integer | 否 | 1..65535 |
| `hasCredentials` | boolean | 否 | **唯一的凭据信号**：`username_cipher` 或 `password_cipher` 存在即 `true`。**只表示存了凭据，不表示认证可用**：本期设备端应用流量不发送认证，带凭据的代理需 IP 白名单授权（见 §4.20 末注） |
| `region` | string | 是 | ≤64 |
| `tags` | string[] | 否 | `[]` 表示无标签 |
| `enabled` | boolean | 否 | `false`：不允许新建绑定；已有绑定仍下发 |
| `boundDevices` | integer | 否 | 引用它的去重设备数 |
| `boundApps` | integer | 否 | 引用行数（含 `app=""` 的整机行） |
| `lastCheck` | `EgressCheck` | 是 | 最近一条探测（device / manager 混合取 `checkedAt` 最大者）；无记录为 `null` |
| `createdAt` / `updatedAt` | string | 否 | RFC 3339 UTC |

### 5.12 `EgressCheck`

`egress_checks` 表的一行（`lastCheck` / `recentChecks`）：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `source` | `"device"` \| `"manager"` | 否 | 设备侧探测（job 里 `probe=true`）还是 Manager 直连探测 |
| `deviceId` | string | 是 | Manager 直连探测为 `null` |
| `app` | string | 否 | `""` = 整机 |
| `ok` | boolean | 否 | 探测是否成功 |
| `exitIp` | string | 是 | 出口 IP |
| `latencyMs` | integer | 是 | 毫秒 |
| `errorCode` | string | 是 | 失败码（如 `egress.probe_failed` / `unsupported_proxy_type`） |
| `checkedAt` | string | 否 | RFC 3339 UTC |

### 5.13 `DeviceEgressBinding`

`GET/PUT /devices/{id}/egress` 的 `bindings[]`（来自 `device_egress` 表）：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `app` | string | 否 | `""` = 整机（方案 A），其余 = 应用级（方案 B） |
| `mode` | `"proxy"` \| `"direct"` | 否 | 与 `device_egress.mode` 一致 |
| `proxyId` | string | 是 | `mode="direct"` 时为 `null` |
| `proxyName` | string | 是 | 冗余出来的代理名，供 UI 直接展示 |
| `updatedAt` | string | 否 | 该行最后更新时间 |

### 5.14 `EgressApply`：dryRun 计划与 202 受理

`dryRun=true` 的响应（`200`，不建 job）：

    {
      "dryRun": true,
      "total": 2,
      "plan": [
        { "deviceId": "pixel-9",   "app": "",              "mode": "proxy",  "proxyName": "office_http" },
        { "deviceId": "redmi-k80", "app": "com.example.a", "mode": "direct", "proxyName": null }
      ],
      "warnings": []
    }

正式的 `202` 响应：

    { "jobId": "job_9c1e…", "status": "queued", "kind": "egress", "total": 3, "createdAt": "2026-12-01T03:00:00Z", "requestId": "req_3f1c…" }

job 本体与逐目标结果见 §5.7；`kind="egress"` 的流水线与结果码见 §7.7。

### 5.15 egress 三张表（schema v2）

DDL 与迁移步骤见 [ARCH.md §5.2.1](ARCH.md) / [§5.3](ARCH.md)；这里只给字段语义（实现与评审对照用）。

`egress_proxies`（代理池，凭据只存密文）：

| 列 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `id` | TEXT PK | 否 | `egp_<32 hex>` |
| `name` | TEXT UNIQUE | 否 | 代理名；下发时作为设备端 `egress.toml` 表名 |
| `kind` | TEXT | 否 | `http` / `socks5`（线上字段 `type`） |
| `host` | TEXT | 否 | 不含 scheme |
| `port` | INTEGER | 否 | 1..65535 |
| `username_cipher` / `password_cipher` | TEXT | 是 | `v1:` + base64(nonce ‖ ct ‖ tag)；AAD 绑 proxy id + 字段名（ARCH §6.2） |
| `region` | TEXT | 是 | ≤64 |
| `tags` | TEXT | 否 | JSON `string[]`，默认 `[]` |
| `enabled` | INTEGER | 否 | 0/1，默认 1 |
| `created_at` / `updated_at` | TEXT | 否 | RFC 3339 UTC |

索引：`idx_egress_proxies_name`、`idx_egress_proxies_enabled`。

`device_egress`（绑定，`app=''` 即整机）：

| 列 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `device_id` | TEXT | 否 | FK `devices(id)`，`ON DELETE CASCADE` |
| `app` | TEXT | 否 | `''` = 整机；与 `device_id` 组成主键 |
| `mode` | TEXT | 否 | `proxy` / `direct` |
| `proxy_id` | TEXT | 是 | FK `egress_proxies(id)`，`ON DELETE RESTRICT`（被绑定时删代理返回 409） |
| `updated_at` | TEXT | 否 | RFC 3339 UTC |

约束：`CHECK ((mode='proxy' AND proxy_id IS NOT NULL) OR (mode='direct' AND proxy_id IS NULL))`；索引 `idx_device_egress_proxy`、`idx_device_egress_device`。

`egress_checks`（探测结果，设备来源 + Manager 直连来源）：

| 列 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `id` | INTEGER PK | 否 | 自增 |
| `source` | TEXT | 否 | `device` / `manager` |
| `device_id` | TEXT | 是 | FK `devices(id)` CASCADE；Manager 直连探测为 `NULL` |
| `app` | TEXT | 否 | `''` = 整机 |
| `scope` | TEXT | 否 | `device` / `app` |
| `proxy_id` | TEXT | 是 | FK `egress_proxies(id)` `ON DELETE SET NULL`（删代理保留历史） |
| `exit_ip` | TEXT | 是 | 出口 IP |
| `latency_ms` | INTEGER | 是 | 毫秒 |
| `ok` | INTEGER | 否 | 0/1 |
| `error_code` | TEXT | 是 | 失败码 |
| `checked_at` | TEXT | 否 | RFC 3339 UTC |

索引：`idx_egress_checks_device`、`idx_egress_checks_proxy`、`idx_egress_checks_checked`。清理策略由实现决定（建议每 `(device_id, app)` 保留 500 条）；契约只保证「最近 1 条可读」（`lastCheck`）。

## 6. TypeScript 类型

完整类型见 **[api-types.ts](api-types.ts)**（`manager/webui/src/api/types.ts` 为同步副本）。
该文件包含：请求 / 响应 / 错误 / job / 矩阵 / 审计 / 客户端接口（`ManagerApi`）全部类型，
命名与本文表格一一对应，并已通过 `tsc --strict --noEmit`。

关键片段（与文件一致）：

    export type TemplateRef = string | { name: string; config: TemplateInput };

    export interface ApplyBulkRequest {
      app: string;
      devices: 'all' | string[];
      template: TemplateRef;
      restart?: boolean;
    }

    export interface JobResult {
      deviceId: string;
      deviceName: string;
      app: string;
      status: 'ok' | 'skipped' | 'failed';
      code: string | null;
      httpStatus: number | null;
      revision: string | null;
      attempts: number;
      durationMs: number;
      steps: JobStep[];
      restart: RestartResult | null;
      warnings: Warning[];
      error: ApiErrorPayload | null;
    }

前端接入约定：

1. `manager/webui/src/api/client.ts`：`baseUrl = import.meta.env.VITE_API_BASE ?? '/relay/api/v1'`；
   Token 取 `localStorage.dfm_token ?? 'admin123'`；统一加 `Authorization` 与 `Accept: application/json`。
2. 非 2xx 时解析错误信封，抛出带 `status` / `code` / `requestId` 的 `ApiError`；401 抛 `AuthError`，
   由 `App.vue` 弹 Token 对话框。
3. 开发环境 Vite 需要代理（`manager/webui/vite.config.ts`）：

       server: {
         port: 5180,
         proxy: {
           '/relay': { target: 'http://127.0.0.1:8765', changeOrigin: false },
           '/healthz': { target: 'http://127.0.0.1:8765', changeOrigin: false },
         },
       }

4. Store 方法 → 端点映射：

| Store 动作（原型 `stores/manager.ts`） | 真实端点 |
| --- | --- |
| `load()` | `GET /devices` + `GET /apps` + `GET /overview` |
| `addDevice(input)` | `POST /devices`（`endpoint` 字段改名为 `baseUrl`） |
| `removeDevice(id)` | `DELETE /devices/{id}` |
| `syncDevice(id)` | `POST /devices/{id}/sync` |
| `syncAll()` | `POST /devices/sync` |
| `setAppConfig(app, device, template)` | `POST /apply`，`assignments` 单条 |
| `batchApply(input)` | `POST /apply`（bulk 或 assignments）+ 轮询 `GET /jobs/{id}` |
| `removeAppConfig(app, device)` | `DELETE /devices/{id}/proxy/api/v1/apps/{pkg}?confirm=true&detach_templates=true` |
| `templates` | `GET /templates` |
| `logs` / `recentLogs` | `GET /audit` |
| `managerToken` / `regenerateToken()` | `localStorage.dfm_token` / `POST /token/rotate` |
| Relay 在线状态 | `GET /healthz` |

#### 原型字段 → v1 字段迁移对照

| 原型（`webui/src/types.ts`） | v1（本文 / api-types.ts） | 备注 |
| --- | --- | --- |
| `Device.endpoint` | `DeviceSummary.baseUrl` | 添加设备的表单字段名同步改 |
| `Device.latency` | `DeviceSummary.latencyMs` | 可空 |
| `Device.lastSeen` | `DeviceSummary.lastSeen` | RFC 3339，不再用「刚刚」 |
| `Device.root` / `moduleVersion` / `serverVersion` | 同名字段 | 可能为 `null` |
| `AppConfig.model` / `template` / `source` / `warning` | `MatrixCell.model` / `template` / `source` / `warnings[]` | `warnings` 是对象数组 |
| `AppDef.id/name/short/color` | 无对应 API 字段 | UI 展示层自行维护（`package` 为唯一键） |
| `TemplateDef.id/name/brand/model/android/dpi` | `TemplateSummary.name` + `config`（snake_case） | 读 `config.brand` / `config.android_version` / `config.dpi` |
| `MatrixRow.app` | `MatrixApp.package` | 应用元数据在 UI 侧 |
| `LogEntry` | `AuditEntry` | `time` → `ts`；`detail` 为对象 |

---

## 7. 下发语义（逐设备）

### 7.1 请求归一化

1. 校验 `apply` 请求（§4.13）：bulk → 展开 `devices`；assignments → 原样。
2. 生成 job 与 assignment 列表，写入 `jobs`；`status=queued`；`total = assignments.length`。
3. 返回 `202`，后台任务开始执行；`Idempotency-Key` 已存在则直接返回旧 job（不重复执行）。

### 7.2 单设备流水线

对每个 assignment 依次执行（同一设备内串行，跨设备并发 ≤ `DFM_APPLY_CONCURRENCY`）：

| 步 | 动作 | 设备端请求 | 头 | 失败处理 |
| --- | --- | --- | --- | --- |
| 1 | 取 revision | `GET /api/v1/config/revision` | `Authorization` | 连接 / DNS / 超时 → `skipped` `device_offline`；非 2xx → `failed` `device_error` |
| 2a | 模板对象：读取现状 | `GET /api/v1/templates/{name}` | `Authorization` | 200 且 `template` 与请求 `config` 相同 → 跳过 2b；200 但不同 → 执行 2b（覆盖）并记 `template_overwritten` 警告；404 → 执行 2b（新建）；401/403 → `failed` `forbidden_scope`；其它失败 → `failed` `device_error` |
| 2b | 模板对象：写入 | `PUT /api/v1/templates/{name}` | `If-Match: *` + JSON | 200/201 → 继续；412（`If-Match: *` 下不应出现，按设备端异常处理）→ 重试一次；仍失败 → `failed` `config_revision_mismatch`；其它 → 按 §7.5 映射 |
| 2c | 模板字符串：确认存在 | `GET /api/v1/templates/{name}` | `Authorization` | 404 → `skipped` `template_not_found`（不创建，因为没有定义）；其它失败 → 按 §7.5 映射 |
| 3 | 绑定 | `PUT /api/v1/apps/{pkg}`，body `{"template":"<name>","detachFromTemplates":true}` | `If-Match: "<revision>"` + JSON | 412 → 重取 revision，重试**一次**（`attempts=2`）；仍 412 → `failed` `config_revision_mismatch`；404 且 `details.resource="package"` → `skipped` `package_not_installed`；403 → `failed` `forbidden_scope`；422 → `failed` `validation_failed`（保留 `details.errors`） |
| 4 | 重启（仅 `restart=true` 且步骤 3 成功） | `POST /api/v1/apps/{pkg}/restart`，body `{"forceStop":true,"launch":false}` | `Idempotency-Key: <jobId>:<deviceId>:<pkg>` | 403 → 记 `restart_failed` 警告，结果仍 `ok`；404 → 警告；5xx / 超时 → 警告；**不**把结果改成 failed |
| 5 | 收尾 | — | — | 更新 `device.lastSeen` / `online`，写 `job_results`，累加计数；job 全部完成后写审计 `job.finished`，并对 `ok` 设备做一次 best-effort `GET /overview` 刷新缓存（失败只记警告，不改 job 结果） |

说明：

- 每次写操作前都重新取 revision（而不是复用缓存），因为设备可能被其它客户端改过。
- 412 只重试一次（对应计划书「412 只重试一次并成功」验收项）。
- 重试后仍失败，错误细节保留设备端 `details{expected, actual}`。
- 所有设备请求都带 `X-Request-Id: <manager requestId>`，设备审计可按 requestId 关联。
- 模板对象形态下，若目标设备已存在同名但字段不同的模板：**覆盖**为请求里的 `config`（`PUT` 整体替换语义），
  并在结果 `warnings` 里加 `template_overwritten`（`details = {name, deviceId}`）。

### 7.3 结果状态与计数

| 结果 status | 何时 | 计数 |
| --- | --- | --- |
| `ok` | bind 成功（restart 的失败只算警告） | `succeeded++` |
| `skipped` | 没有发出写请求：离线、包未安装、模板缺失（只给名字） | `skipped++` |
| `failed` | 发出写请求但失败：412 重试耗尽、403、422、设备 5xx、解析失败 | `failed++` |

### 7.4 Job 状态机

    queued ──► running ──► succeeded | partial | failed
       │          │
       └──────────┴──► failed（仅崩溃恢复时标记，error="interrupted"）

终态判定（全部结果写完后，必须与计数一致）：

| 条件 | status |
| --- | --- |
| `succeeded == total` | `succeeded` |
| `succeeded == 0 && failed > 0` | `failed` |
| 其它（含 `succeeded > 0 && (skipped > 0 \|\| failed > 0)`；以及全 skipped） | `partial` |

例：4 台设备 3 成功 1 离线 → `partial`；1 台设备离线 → `partial`；4 台全部 412 耗尽 → `failed`。

计数不变量（测试必须校验）：`succeeded + skipped + failed == total` 当且仅当 `status` 为终态。

### 7.5 设备错误 → Manager 结果映射

| 设备响应 | 映射 |
| --- | --- |
| 连接失败 / DNS / 超时 | `skipped`，`device_offline`（`details.reason`） |
| `401 unauthorized` | `failed`，`forbidden_scope`（`details = {required:"write", granted:[]}`） |
| `403 forbidden_scope` | `failed`，`forbidden_scope`（保留设备 `details`） |
| `404` + `details.resource="package"` | `skipped`，`package_not_installed` |
| `404` + `details.resource="template"` | `skipped`，`template_not_found` |
| `412 config_revision_mismatch` | 重试一次；仍失败 → `failed`，`config_revision_mismatch` |
| `422 validation_failed` | `failed`，`validation_failed`（`details.errors` 原样） |
| `428 precondition_required` / `400 confirmation_required` | `failed`，`device_error`（Relay 不应触发，视为实现 bug） |
| `503 upstream_unavailable` | `failed`，`upstream_unavailable`（`details.component`） |
| 其它 4xx / 5xx | `failed`，`device_error`（`details.upstreamStatus` / `upstreamCode`） |
| 响应不是 JSON / 缺字段 | `failed`，`device_error` |

### 7.6 与设备端 `/api/v1` 的映射总表

| Relay 能力 | 设备端调用 | 缓存 / 说明 |
| --- | --- | --- |
| 添加设备（探测） | `GET /status`、`GET /overview` | 两者成功才落库 |
| `POST /devices/{id}/sync` | `GET /status`、`GET /overview` | 更新设备行 + overview 缓存 |
| `POST /devices/sync` | 同上，逐台并发 | 单台失败不影响其它 |
| `GET /devices`、`GET /overview`、`GET /apps`、`GET /templates` | **不调用** | 全部来自 SQLite 缓存，保证 UI 秒开、离线可用 |
| `GET /devices/{id}/overview` | `GET /overview`（`cached=1` 时不调用） | 默认实时并刷新缓存 |
| `POST /apply` | `GET /config/revision`、`GET/PUT /templates/{name}`、`PUT /apps/{pkg}`、`POST /apps/{pkg}/restart` | §7.2 |
| `ANY /devices/{id}/proxy/api/v1/{*path}` | 同名路径透传 | 原样返回 |
| 设备 Token | 只用于发出请求 | 解密后仅存在于请求头，不落日志、不回显 |

---

### 7.7 egress 下发流水线（`kind="egress"`）

状态机与 §7.4 **完全同构**（不新增状态）：`queued → running → succeeded | partial | failed`；崩溃恢复记 `failed` + `error.code="interrupted"`。终态判定与计数不变量同 §7.4。

Job 字段：`kind="egress"`、`app=null`、`template=null`、`restart=false`；`total` = 展开后的 `(device, app)` 对数（整机行算 1 对）。

对每个 `(device, app)` 对（同一设备内串行，跨设备并发 ≤ `DFM_APPLY_CONCURRENCY`）：

| 步 | 动作 | 设备端请求 | 失败处理 |
| --- | --- | --- | --- |
| 1 | 取现状 | `GET /api/v1/egress` | 连接 / DNS / 超时 → `skipped device_offline`；404 → `skipped egress_unsupported`；403 → `failed forbidden_scope`；其它非 2xx → `failed device_error` |
| 2 | 组装期望态 | — | 只组装 **Manager 拥有的三样**：`default`、`apps`、`proxies`（含凭据）。`failOpen` / `probeUrl` / `probeTimeoutSeconds` / `exclusionList` **不进请求体**（属设备端 `egress.toml` / settings，Manager 不下发、不重置）。凭据字段总是显式（未设置即 `null`）；零应用级绑定时发 `apps: null` |
| 3 | 下发（**总是 PUT，不做本地比较**） | `PUT /api/v1/egress` + `If-Match: <步骤 1 的 egress revision>` | 412 → 重取 revision 重试**一次**（`attempts=2`）；仍 412 → `failed config_revision_mismatch`；403 → `forbidden_scope`；422 → `validation_failed`（保留 `details.errors`）；503 → `upstream_unavailable`。内容一致也会真写一次，**没有本地跳过** |
| 4 | 读 `changed` 落步骤 | — | 设备 `changed:true` → 步骤 `egress_put` 记 `ok`；`changed:false` → 步骤 `egress_put` 记 `status:"skipped"` + `code:"egress_unchanged"`，**结果行仍是 `ok`**、`code="egress_unchanged"`（全部 `skipped` 会把 job 算成 `partial`）。`JobResult.revision` 取设备返回的 `revision` |
| 5 | 探测（仅 `probe=true`，步骤 3 成功后；含 `changed:false`） | `POST /api/v1/egress/probe` | 任何失败 → 追加 `egress_probe_failed` 警告（`details{deviceId, app, reason}`），**不改结果状态**；成功则写 `egress_checks(source="device")` |
| 6 | 收尾 | — | 更新 `device.lastSeen` / `online`；写 `job_results`（`app=""` 表示整机）；累加计数；全部完成后写审计 `egress.apply` |

语义要点（与 [ARCH.md §10.7](ARCH.md) 同一口径）：

- **总是 PUT**：每次 apply 都会写一次设备（需要设备 Token 的 `write` 作用域），换来凭据轮换、设备侧漂移都能被下一次 apply 收敛；Manager 不存「已下发指纹」。
- **幂等判定权在设备**：`changed:false` 由设备端算（期望态 == 现状），Manager 只把它转写成 `egress_unchanged`（结果 `ok`）。
- **Manager 只下发三样**：`default` / `apps` / `proxies`；`failOpen`、`probeUrl`、`probeTimeoutSeconds`、`exclusionList` 保持设备现值，Manager 不比较也不重置。
- **Manager 侧探测只支持 HTTP 代理**（`socks5` → `422 unsupported_proxy_type`），只用于验证代理本身可用。
- **固有代价：凭据随请求下发**。「总是 PUT」意味着每次 apply 都会把**解密后的** `username`/`password` 放进 `PUT /api/v1/egress` 请求体，走与设备 Token 相同的通道（默认 loopback 明文 HTTP + Bearer；局域网无 TLS）。这是「总是 PUT」的代价而非缺陷：不先解密就推不动轮换后的凭据。明文只在请求内存里（`Zeroizing`），不落 Manager 日志、不进审计、不回响应；更强隔离用代理侧 IP 白名单或等 P2 本地认证中继。

结果码映射（`JobResult.code`）：

| 情形 | status | code |
| --- | --- | --- |
| 设备不可达 | `skipped` | `device_offline` |
| 单请求超时 | `skipped` | `upstream_timeout` |
| 设备没有 `/egress`（旧版本 404） | `skipped` | `egress_unsupported` |
| 设备端 409 `egress.not_configured` | `failed` | `egress_not_configured` |
| 401 / 403 | `failed` | `forbidden_scope` |
| 412 重试一次后仍失败 | `failed` | `config_revision_mismatch` |
| 422 | `failed` | `validation_failed` |
| 503 `upstream_unavailable` | `failed` | `upstream_unavailable` |
| 其它 4xx/5xx、响应非 JSON | `failed` | `device_error` |
| 设备返回 `changed:false`（幂等短路） | `ok` | `egress_unchanged` |

步骤名：`egress_get` / `egress_put` / `egress_probe`（§5.7 `JobStep.step` 枚举已扩充）。

## 8. 静态托管与 SPA

| 请求 | 行为 |
| --- | --- |
| `GET /` | 返回 `index.html`；无 `dist` 时返回 200 的构建提示页 |
| `GET /assets/*` | 静态文件；`Cache-Control: public, max-age=31536000, immutable`（Vite 带 hash） |
| `GET /favicon.ico` 等根级文件 | 命中则返回，未命中走 SPA fallback |
| `GET /devices`、`/apps` 等前端路由 | 返回 `index.html`（200），`Cache-Control: no-cache` |
| `GET /relay/api/v1/...` 未知路径 | JSON `404 not_found`，**不**返回 HTML |
| `GET /relay/api/v1`（无尾斜杠） | `404 not_found`（`resource="route"`） |
| 非 `GET`/`HEAD` 的非 API 路径 | `405 method_not_allowed` |

静态目录解析顺序：`--static-dir` → `DFM_STATIC_DIR` → `<exe 同目录>/webui/dist` → `<cwd>/manager/webui/dist` → `<cwd>/webui/dist` → 无（提示页）。
响应头附加：`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、
`Content-Security-Policy: default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'`。

---

## 9. 可执行验收（curl）

### 9.1 启动一个 Mock 设备（无 Android 依赖）

仓库内已提供可直接运行的 [mock_device.py](mock_device.py)（与下面脚本等价）。自 v1.10.0 起它也实现了 egress 的五个端点，可直接验证出口下发——步骤见 [RUN.md §6.1](RUN.md)：

    python3 manager/docs/mock_device.py 18686

脚本模拟设备端关键端点（status / overview / config/revision / templates / apps），
包含「首次 bind 返回 412、重试成功」场景（`FORCE_412_ONCE`）。端口被占用
（例如 `adb forward tcp:18686` 已监听）时换一个端口，并同步修改下文 `$DEV`。

    #!/usr/bin/env python3
    """Device Faker 设备端最小 Mock：供 Manager 冒烟验收。"""
    import hashlib
    import json
    import sys
    from http.server import BaseHTTPRequestHandler, HTTPServer

    TOKEN = "admin123"
    FORCE_412_ONCE = True
    STATE = {
        "templates": {
            "pixel_9": {
                "packages": [],
                "brand": "google",
                "model": "Pixel 9",
                "android_version": "17",
                "sdk_int": 37,
                "dpi": 420,
            }
        },
        "apps": {"com.xingin.xhs": "pixel_9"},
        "forced": FORCE_412_ONCE,
    }

    def revision():
        blob = json.dumps(STATE, sort_keys=True, ensure_ascii=False).encode()
        return hashlib.sha256(blob).hexdigest()

    def overview():
        rev = revision()
        return {
            "revision": rev,
            "module": {"id": "device_faker", "version": "v1.9.0", "versionCode": 20261125},
            "device": {
                "manufacturer": "Google", "brand": "google", "model": "Pixel 9",
                "device": "tokay", "android": "17", "sdk": 37, "abi": "arm64-v8a",
                "fingerprint": "google/tokay/tokay:17/BP2A.250605.031/13000000:user/release-keys",
            },
            "root": {"manager": "kernelsu", "version": "1.0.7"},
            "server": {"version": "1.9.0", "bind": "127.0.0.1:18686", "uptimeSec": 3600,
                       "startedAt": "2026-11-24T08:00:00Z", "pid": 4321},
            "config": {"revision": rev, "mtime": "2026-11-24T08:00:00Z", "size": 256,
                       "exists": True, "templates": len(STATE["templates"]),
                       "apps": len(STATE["apps"]), "conflicts": 0},
            "templates": [
                {"name": name, "position": i, "packages": cfg.get("packages", []),
                 "config": {k: v for k, v in cfg.items() if k != "packages"}}
                for i, (name, cfg) in enumerate(STATE["templates"].items())
            ],
            "apps": [
                {"package": pkg, "installed": True,
                 "source": {"type": "template", "template": tpl, "alsoInline": False},
                 "resolved": {"brand": "google", "model": "Pixel 9", "dpi": 420},
                 "warnings": []}
                for pkg, tpl in STATE["apps"].items()
            ],
            "warnings": [],
        }

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def _send(self, status, payload):
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("X-Request-Id", "req_mock")
            self.send_header("X-DF-Revision", revision())
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _error(self, status, code, message, details=None):
            self._send(status, {"error": {"code": code, "message": message, "details": details,
                                          "retryable": False, "requestId": "req_mock"}})

        def _authorized(self):
            if self.headers.get("Authorization") != "Bearer " + TOKEN:
                self._error(401, "unauthorized", "bad token")
                return False
            return True

        def _body(self):
            length = int(self.headers.get("Content-Length", "0"))
            return json.loads(self.rfile.read(length) or b"{}")

        def do_GET(self):
            if not self._authorized():
                return
            path = self.path.split("?")[0]
            if path == "/api/v1/status":
                ov = overview()
                return self._send(200, {"module": ov["module"], "device": ov["device"],
                                        "server": ov["server"], "config": ov["config"],
                                        "capabilities": ["config", "templates", "apps", "overview"]})
            if path == "/api/v1/overview":
                return self._send(200, overview())
            if path == "/api/v1/config/revision":
                return self._send(200, {"revision": revision(), "mtime": "2026-11-24T08:00:00Z",
                                        "size": 256, "exists": True})
            if path.startswith("/api/v1/templates/"):
                name = path.rsplit("/", 1)[-1]
                cfg = STATE["templates"].get(name)
                if cfg is None:
                    return self._error(404, "not_found", "template missing", {"resource": "template"})
                return self._send(200, {"revision": revision(), "position": 0, "name": name,
                                        "template": cfg})
            if path.startswith("/api/v1/apps/"):
                pkg = path.rsplit("/", 1)[-1]
                tpl = STATE["apps"].get(pkg)
                if tpl is None:
                    return self._error(404, "not_found", "app config missing",
                                       {"resource": "app", "name": pkg})
                return self._send(200, {
                    "package": pkg,
                    "revision": revision(),
                    "source": {"type": "template", "template": tpl, "alsoInline": False},
                    "resolved": {"brand": "google", "model": "Pixel 9", "dpi": 420},
                    "warnings": [],
                })
            return self._error(404, "not_found", "no route", {"resource": "route"})

        def do_PUT(self):
            if not self._authorized():
                return
            path = self.path.split("?")[0]
            body = self._body()
            if path.startswith("/api/v1/templates/"):
                name = path.rsplit("/", 1)[-1]
                existed = name in STATE["templates"]
                STATE["templates"][name] = body
                return self._send(200 if existed else 201, {"revision": revision(),
                                                            "warnings": []})
            if path.startswith("/api/v1/apps/"):
                pkg = path.rsplit("/", 1)[-1]
                if pkg == "com.missing.app":
                    return self._error(404, "not_found", "package not installed",
                                       {"resource": "package", "name": pkg})
                if STATE["forced"]:
                    STATE["forced"] = False
                    return self._error(412, "config_revision_mismatch", "revision mismatch",
                                       {"expected": revision(), "actual": "0" * 64})
                if_match = (self.headers.get("If-Match") or "").strip('"')
                if if_match != revision():
                    return self._error(412, "config_revision_mismatch", "revision mismatch",
                                       {"expected": revision(), "actual": if_match})
                template = body.get("template")
                if template not in STATE["templates"]:
                    return self._error(404, "not_found", "template missing", {"resource": "template"})
                STATE["apps"][pkg] = template
                return self._send(200, {"revision": revision(), "previousRevision": None,
                                        "dryRun": False, "backupId": None, "warnings": []})
            return self._error(404, "not_found", "no route", {"resource": "route"})

        def do_POST(self):
            if not self._authorized():
                return
            path = self.path.split("?")[0]
            if path.endswith("/restart"):
                pkg = path.split("/")[-2]
                return self._send(200, {"package": pkg, "forceStopped": True,
                                        "launched": False, "durationMs": 5})
            return self._error(404, "not_found", "no route", {"resource": "route"})

    if __name__ == "__main__":
        port = int(sys.argv[1]) if len(sys.argv) > 1 else 18686
        print(f"mock device on http://127.0.0.1:{port} token={TOKEN}", flush=True)
        HTTPServer(("127.0.0.1", port), Handler).serve_forever()

### 9.2 冒烟脚本（Manager 已启动，Mock 已启动）

    set -euo pipefail
    BASE=http://127.0.0.1:8765/relay/api/v1
    TOKEN=admin123                      # 与 Manager 启动时的 Token 一致
    DEV=http://127.0.0.1:18686          # mock_device.py 地址

    # 1) 探活（无鉴权）
    curl -s "$BASE/healthz" | jq '{status, version, devices}'

    # 2) 无 Token → 401
    curl -s -o /dev/null -w '%{http_code}\n' "$BASE/devices"          # 期望 401

    # 3) 添加 Mock 设备（探测 status + overview）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d "{\"name\":\"Mock Pixel 9\",\"baseUrl\":\"$DEV\",\"token\":\"admin123\",\"tags\":[\"mock\"]}" \
      "$BASE/devices" | jq '{id, online, model, serverVersion}'

    # 4) 设备列表 + 应用矩阵
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices" | jq '.devices[] | {id, online}'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/apps" | jq '{totals, packages: [.apps[].package]}'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/templates" | jq '[.templates[] | {name, complete}]'

    # 5) 单台同步
    curl -s -X POST -H "Authorization: Bearer $TOKEN" "$BASE/devices/mock-pixel-9/sync" \
      | jq '{online: .device.online, warnings}'

    # 6) bulk 下发（模板对象；Mock 首次 bind 返回 412，Relay 自动重试一次）
    JOB=$(curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"app":"com.xingin.xhs","devices":"all","template":{"name":"pixel_9","config":{"brand":"google","model":"Pixel 9","android_version":"17","sdk_int":37,"dpi":420}},"restart":true}' \
      "$BASE/apply" | jq -r .jobId)
    echo "job=$JOB"

    # 7) 轮询 job
    for _ in $(seq 1 20); do
      OUT=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/jobs/$JOB")
      STATUS=$(echo "$OUT" | jq -r .status)
      [ "$STATUS" = "queued" ] || [ "$STATUS" = "running" ] || break
      sleep 0.5
    done
    echo "$OUT" | jq '{status, total, succeeded, skipped, failed,
                      results: [.results[] | {deviceId, status, code, attempts, revision}]}'

    # 8) assignments 下发（不同设备不同模板）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"assignments":[{"device":"mock-pixel-9","app":"com.xingin.xhs","template":"pixel_9"}],"restart":false}' \
      "$BASE/apply" | jq '{jobId, total, kind}'

    # 9) 应用矩阵与漂移
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/apps" | jq '.apps[] | {package, configured, total, inconsistent, models}'

    # 10) 总览 + 审计
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/overview" | jq '{fleet, drift: [.drift[].package]}'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/audit?limit=5" | jq '[.entries[] | {action, status}]'

    # 11) 代理：读设备端单应用来源
    curl -s -H "Authorization: Bearer $TOKEN" \
      "$BASE/devices/mock-pixel-9/proxy/api/v1/apps/com.xingin.xhs" | jq '{source, resolved}'

    # 12) Token 轮换（之后所有请求改用新 Token）
    NEW_TOKEN=$(curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{}' "$BASE/token/rotate" | jq -r .token)
    curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $TOKEN" "$BASE/devices"   # 期望 401
    curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $NEW_TOKEN" "$BASE/devices" # 期望 200

    # 13) 移除设备
    curl -s -o /dev/null -w '%{http_code}\n' -X DELETE -H "Authorization: Bearer $NEW_TOKEN" "$BASE/devices/mock-pixel-9"

预期：

| 步骤 | 期望 |
| --- | --- |
| 1 | `status=ok`，`devices.total` ≥ 0 |
| 2 | `401` |
| 3 | `online=true`，`model=Pixel 9` |
| 4 | 矩阵含 `com.xingin.xhs`，模板含 `pixel_9` |
| 6-7 | job 终态 `succeeded`；`results[0].attempts=2`（首次 412 后重试） |
| 8 | `202`，`kind=assignments` |
| 12 | 旧 Token `401`，新 Token `200` |

### 9.3 egress 冒烟（代理池 → 绑定 → 批量下发 → 探测）

    # 1) 建池（用户名 / 密码只写不读）
    PID=$(curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"name":"office_http","type":"http","host":"198.51.100.10","port":8080,"username":"<USERNAME>","password":"<PASSWORD>","region":"SG","tags":["egress"]}' \
      "$BASE/egress-proxies" | jq -r .id)
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress-proxies/$PID" | jq '{name, hasCredentials, boundDevices, lastCheck}'

    # 2) Manager 侧直连探测（验证代理本身；socks5 会 422 unsupported_proxy_type）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"url":"http://checkip.example.com/"}' "$BASE/egress-proxies/$PID/probe" | jq .

    # 3) 绑定（只落库，pendingApply=true）
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"bindings":[{"app":"","proxyId":"'$PID'"},{"app":"com.example.b","proxyId":"__DIRECT__"}]}' \
      "$BASE/devices/pixel-9/egress" | jq '{changed, pendingApply, bindings}'

    # 4) 批量下发 → 轮询 job（kind=egress）
    JOB=$(curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"devices":"all","probe":true}' "$BASE/devices/egress/apply" | jq -r .jobId)
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/jobs/$JOB" \
      | jq '{status, kind, total, succeeded, skipped, failed, results:[.results[]|{deviceId, app, status, code, steps}]}'

    # 5) 回读设备侧出口
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices/pixel-9/egress" \
      | jq '.deviceState | {mode, effectiveMode, applied, readback, health}'

    # 6) 被绑定不能删；解绑后 204
    curl -s -o /dev/null -w '%{http_code}\n' -X DELETE -H "Authorization: Bearer $TOKEN" "$BASE/egress-proxies/$PID"   # 409
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{"bindings":[]}' \
      "$BASE/devices/pixel-9/egress" > /dev/null
    curl -s -o /dev/null -w '%{http_code}\n' -X DELETE -H "Authorization: Bearer $TOKEN" "$BASE/egress-proxies/$PID"   # 204

预期：步骤 1 `hasCredentials:true`（响应里没有用户名 / 密码）；步骤 2 成功返回 `exitIp`；步骤 4 用同一份内容重放时设备返回 `changed:false` → 结果 `status=ok` + `code=egress_unchanged`，job 终态仍 `succeeded`；步骤 6 首次 409（`egress_proxy_in_use`）、解绑后 204。

---

## 10. 变更记录

| 版本 | 日期 | 变更 |
| --- | --- | --- |
| v1.0 | 2026-11-24 | 冻结：18 个端点、错误信封与码表、模板对象、job 与逐设备结果、聚合 revision、TS 类型草案、可执行冒烟脚本 |
| v1.1 | 2026-11-24 | §9.1 Mock 设备补 `GET /api/v1/apps/{pkg}`（§9.2 第 11 步依赖）与 `If-Match` 空值防护；新增可直接运行的 [mock_device.py](mock_device.py)；§2.10 注明 assignment 总时限当前未硬性实施 |
| v1.2 | 2026-12-01 | egress 增量（Manager 0.2.0）：新增 9 个方法级端点（合计 27）、三张表与 schema v2、job `kind=egress` 与 `egress_unchanged` / `egress_unsupported` / `egress_not_configured` 结果码、3 个 `egress_*` 错误码与 6 个警告码；`Idempotency-Key` 扩展到 `/devices/egress/apply`；§5.7 枚举与 §7.7 流水线同步 |
| v1.3 | 2026-12-01 | 下发语义改为「**总是 PUT + 设备 `changed` 判定**」（§7.7 步骤 2/3/4、§2.8 `egress_unchanged`、§5.7 `JobResult.code` 说明与 §9.3 预期同步），并补代价与前提：每次 apply 一次设备写入、需要 `write` 作用域；与 [ARCH.md §10.7](ARCH.md) 同一口径 |
| v1.4 | 2026-12-01 | 登记 `decrypt_failed` / `device_token_invalid`（§2.8 结果码）与 `egress_check_not_recorded`（§2.8 警告）；§4.24 写明 probe 解密失败是 `500 internal_error(details.op=decrypt)`、探测结果落库失败带警告；§4.25 的 GET 警告列表补 `device_token_invalid`（与 job 同码）；§7.7 补「凭据随请求下发」的固有代价 |

---

## 11. 许可

    SPDX-License-Identifier: GPL-3.0-or-later
    Copyright (c) 2025-2026 Seyud
    Device Faker: https://github.com/Seyud/device_faker
