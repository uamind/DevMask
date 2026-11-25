# Device Faker HTTP API（/api/v1）契约

> 文档版本：v1.2（第一增量 + egress 增量冻结）
> 适用：device_faker v1.10.0 起自带的 `device_faker_server`
> 机器可读契约：[openapi.yaml](openapi.yaml) · 面向 AI 摘要：[llms.txt](llms.txt)
> 设计依据：[docs/dev/server-api-plan.md](dev/server-api-plan.md) 第 5 节（API 设计）与第 10 节（安全设计）、[docs/CONFIG.md](CONFIG.md)

本文件是**契约**：server 实现与 WebUI 传输层都以本文件的路径、字段名、错误码为准。任何实现与本文件不一致的地方按缺陷处理。

---

## 1. 范围

### 1.1 本增量（第一增量）实现的端点

- 系统：`GET /api/v1/healthz`、`GET /api/v1/status`、`GET /api/v1/overview`、`GET /api/v1/schema/config`
- 配置：读取（JSON / TOML）、revision、整体替换、文本校验
- 模板：列表、详情、整体替换、字段合并、删除、批量绑定包名、解绑单个包名
- 应用：列表、详情（含解析结果）、内联写入或模板绑定、删除、重启
- 包与设备：已安装包元数据、设备信息、getprop 白名单读取、用户列表
- 出口代理（egress，v1.10.0 新增）：`GET/PUT/DELETE /api/v1/egress`、`POST /api/v1/egress/probe`、`GET /api/v1/egress/proxies`

合计 **29 个端点**，总表见 §4。

### 1.2 明确不在本增量

| 能力 | 计划版本 | 本增量行为 |
| --- | --- | --- |
| MCP（`/mcp`）与 stdio 桥 | P3 | 路径不存在，404 |
| `POST /plan`、`/apply`、`/simulate`、`/batch`、`/conflicts` | P3 | 不存在；用写端点的 `dry_run` 替代预演 |
| 备份管理 `/backups*`（列表 / 手动备份 / 恢复） | P4 | 不存在；写入时的自动备份已生效，见 §9 |
| 审计查询 `GET /audit`、日志 `GET /logs` | P4 | 不存在；审计仍在后台写 JSONL |
| Token 管理 `/tokens*` | 下一增量 | 不存在；Token 通过管理器 WebUI 或 `server.toml` 配置，见 §3.4 |
| 应用名 / 图标 `/packages/{pkg}/label`、`/packages/{pkg}/icon` | P4 | 不存在；包列表的 `label` 恒为 `null` |
| TLS、Unix socket | P4 | 仅明文 HTTP + 可选 loopback 绑定 |
| 限流（读/写配额）与鉴权失败封禁 | P4 | `server.toml` 的 `read_rate_per_min` / `write_rate_per_min` 只解析、**不执行**；本增量不会返回 429 |
| `GET /capabilities`、`GET /guide`、`GET /openapi.json`、`GET /events`（SSE） | 下一增量 | 不存在；能力清单放在 `GET /status` 的 `capabilities` 字段 |
| 深链 `POST /intents/open`、在线模板代理 | 未排期 | 不存在；原生模式下对应 WebUI 功能降级，见 §10.4 |

> 未实现的 `/api/v1/*` 路径必须返回 **404 + 统一错误信封**（`code=not_found`），不要返回 501，也不要回退到 `index.html`。客户端据此判断能力缺失。

### 1.3 版本与兼容

- 前缀固定 `/api/v1`；仅在不兼容变更时升到 `/api/v2`。
- 新增**可选**字段、新增端点不升版本；客户端必须忽略响应里的未知字段。
- 枚举值只增不改；字段不重命名、不改类型。

---

## 2. 基础约定

### 2.1 基地址与监听

| 项 | 默认值 | 说明 |
| --- | --- | --- |
| 基地址 | `http://127.0.0.1:8686/api/v1` | 设备本机浏览器 / adb forward 后使用 |
| 监听 | `127.0.0.1:8686` | `server.toml` 的 `bind` 可改；默认只绑 loopback |
| 局域网 | 关闭 | `allow_lan = true` 且 `bind = "0.0.0.0:8686"` 才监听局域网 |
| 推荐调试 | `adb forward tcp:8686 tcp:8686` | 不暴露到网络 |
| 静态站点 | 同进程 `/` | 非 `/api/v1` 路径为 WebUI 静态资源，见 §11 |

安全约束（硬性）：

1. `bind` 不是 loopback 时，`server.toml` 必须至少有一个 Token，否则服务**拒绝启动**并在 `logs/server.log` 写明原因。
2. 任何 `/api/v1/*` 响应都带 `Cache-Control: no-store`。
3. 不存在任意 shell 执行、任意文件读写端点。

### 2.2 字段命名规则（实现必须严格遵守）

| 位置 | 规则 | 示例 |
| --- | --- | --- |
| 配置内容字段（`Config` / `Template` / `AppConfig` 内部的键） | **snake_case**，与 `config.toml` 完全同名同义 | `sdk_int`、`custom_props`、`default_force_denylist_unmount`、`companion_resetprop` |
| API 元数据（响应包装、请求控制字段） | **lowerCamelCase** | `revision`、`nextCursor`、`dryRun`、`previousRevision`、`backupId`、`forceStop`、`detachFromTemplates`、`uptimeSec` |
| 查询参数 | **snake_case** | `dry_run`、`diff`、`confirm`、`detach_templates`、`allow_duplicate_package`、`format`、`limit`、`cursor` |
| 路径参数 | 原文，必要时百分号编码 | `com.example.app%40999`（即 `com.example.app@999`） |
| 请求 / 响应头 | 标准 HTTP 大小写 | `If-Match`、`X-DF-Revision`、`X-Request-Id` |

> 一句话：**配置长什么样就 snake_case 照抄；围绕配置的元数据用 camelCase；URL 查询参数用 snake_case。**

### 2.3 请求头

| 头 | 必需 | 说明 |
| --- | --- | --- |
| `Authorization: Bearer <token>` | 除 `/healthz` 外必需 | 见 §3 |
| `Content-Type` | 有请求体时必需 | `application/json`；`PUT /config` 另接受 `application/toml` 与 `text/plain` |
| `If-Match` | 所有**修改配置**的端点（PUT / PATCH / DELETE）必需 | 当前配置 revision，见 §2.5；`POST /apps/{pkg}/restart` 与 `POST /config/validate` 不需要 |
| `Idempotency-Key` | 可选 | 目前仅 `POST /apps/{pkg}/restart` 生效；≤128 字符 |
| `X-Request-Id` | 可选 | 客户端关联 ID（≤128 字符，建议 `[A-Za-z0-9._-]`）；服务端原样回填 |
| `Accept` | 可选 | `application/json`（默认）；`GET /config?format=toml` 返回 `text/plain; charset=utf-8` |

### 2.4 响应头

| 头 | 出现位置 | 说明 |
| --- | --- | --- |
| `X-Request-Id` | 所有响应 | 客户端未提供时服务端生成 `req_<ulid>` |
| `X-DF-Revision` | 配置 / 模板 / 应用相关的读写响应 | 不带引号的 revision，如 `9f2c…` |
| `X-DF-Egress-Revision` | egress 端点（`/egress`、`/egress/probe`、`/egress/proxies`） | **egress revision**（不是 config revision），见 §2.5。egress 端点**不**回 `X-DF-Revision` / `ETag` |
| `ETag` | `GET /config`、`GET /config/revision`、`GET /templates/{name}`、`GET /apps/{pkg}` | **带引号**的强校验值（RFC 9110），可直接作为 `If-Match` 回传 |
| `Retry-After` | 429 | 秒数 |
| `Idempotent-Replay: true` | 命中幂等键的重放响应 | 响应体为首次结果 |

示例：

    HTTP/1.1 200 OK
    Content-Type: application/json; charset=utf-8
    Cache-Control: no-store
    X-Request-Id: req_01JC7Z6Q3M8V2S9K4T1B0A
    X-DF-Revision: 9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5
    ETag: "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5"
    {"revision":"9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5", …}

### 2.5 revision 与 If-Match

- **revision 是配置原始字节的 SHA-256**，规范形态为 64 位小写十六进制（如 `9f2c5d1e…`）。实现可以在前面加 `sha256:` 前缀，但同一实例的所有响应必须一致。
- **客户端必须把 revision 当作不透明字符串**：原样保存、原样回传，不要自己重算、裁剪或拼接前缀。
- revision 是**整份配置**的版本号；模板、应用、`/config/revision` 上的 revision 都是它，不存在「单模板 revision」。
- 配置文件不存在时：`GET /config` 仍返回 200，`exists:false` + 空配置文档，revision 为空内容字节的 SHA-256：
  `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`。
- 引号容错：`If-Match` 接受带引号（即 `ETag` 原值）或不带引号的形式；比较时忽略可选的 `sha256:` 前缀差异，其余逐字符一致。
- `If-Match: *` 表示无条件写（用于首次创建 / 客户端已知会覆盖）。
- 缺失 `If-Match` → `428 precondition_required`。
- 不匹配 → `412 config_revision_mismatch`，`details{expected, actual}`，**服务端不做自动合并**。客户端应重新 `GET` 后重放自己的变更。
- 任何成功写入（包括只改空白或注释）都会改变 revision；写响应里的 `revision` 是新值。
- 管理器模式的旧 WebUI 可能直接写文件绕过锁与版本号；原生模式的 API 会在响应里以 `config_revision_mismatch` 暴露这种外部变更。

egress 端点（§5.25–§5.29）使用**另一套 revision**，叫 **egress revision**，因为它同时覆盖两个文件：

    egressRevision = SHA-256( configRevision 的 64 位小写十六进制字符串的 UTF-8 字节 ‖ "\n" ‖ egress.toml 原始字节 )
                     → 64 位小写十六进制（egress.toml 不存在时按空字节参与）

- 任一文件变化（含只改 `egress.toml` 里的代理密码）都会改变 egress revision。
- `PUT` / `DELETE /egress` 的 `If-Match` 比较对象是 **egress revision**；`configRevision`（响应体里的另一个字段）只用于排障，**不能**拿来做 egress 端点的 `If-Match`。
- 缺失 `If-Match` → `428 precondition_required`；不匹配 → `412 config_revision_mismatch` 且 `details` 追加 `resource:"egress"`。
- egress 端点**不**返回 `X-DF-Revision` / `ETag`（否则会污染 WebUI 缓存的 config revision），改用 `X-DF-Egress-Revision` 头。

### 2.6 dry_run、diff、confirm、allow_duplicate_package

| 参数 | 适用端点 | 语义 |
| --- | --- | --- |
| `dry_run=true` | 所有写端点 | 完整执行校验与 diff 计算，但**不落盘、不备份**。响应 `dryRun:true`，`revision` 仍为当前值，`backupId` 为 `null`。 |
| `diff=true` | 所有写端点 | 成功写入时在响应里附 unified diff（默认 3 行上下文）。diff 上限 64 KiB，超出截断并置 `diffTruncated:true`。`dry_run` 时总是返回 diff。 |
| `confirm=true` | `DELETE /templates/{name}`、`DELETE /apps/{pkg}` | 必填。缺失 → `400 confirmation_required`。 |
| `allow_duplicate_package=true` | `PUT/PATCH /templates/{name}`、`POST /templates/{name}/packages` | 默认 `false`。会写出「同一包名出现在多个模板」时拒绝并返回 `409 conflict_duplicate_package`；置 `true` 才允许并附 `duplicate_package` 警告。 |

### 2.7 分页

- `limit` 默认 50，最大 500（超出 → 422 `validation_failed`，`path="limit"`）。
- `cursor` 为不透明字符串，取自上一页的 `nextCursor`；`nextCursor` 为 `null` 表示没有下一页。
- 非空且无法解析的 `cursor` → 422 `validation_failed`（`path="cursor"`）。
- 列表顺序 = 配置文件书写顺序（`templates` 的对象键顺序、`apps` 的数组顺序），**永不重排**。列表项带 0 基 `position`。

### 2.8 幂等

- `POST /apps/{pkg}/restart` 接受 `Idempotency-Key`，保留 24 小时：同键重放返回首次状态码与响应体，并带 `Idempotent-Replay: true`。
- `PUT` / `PATCH` / `DELETE` 天然幂等：同一 `If-Match` 的第二次相同请求会得到 412（说明第一次已生效），客户端按 412 处理即可。
- `POST /config/validate` 是纯函数，无副作用，不需要幂等键。

### 2.9 限流与请求体上限

| 项 | 默认 | 本增量行为 |
| --- | --- | --- |
| 读请求 | 120 次/分钟/Token | **未执行**（known gap，P4）：`read_rate_per_min` 只解析、不计数，不会返回 429 |
| 写请求 | 30 次/分钟/Token | **未执行**（known gap，P4）：同上；WebUI 的 429 处理是防御性代码 |
| 鉴权失败 | 10 次/分钟/来源 IP | **未实现**（known gap，P4）；401/403 正常返回，但不封禁、不计数 |
| 请求体 | 2 MiB（2097152 字节） | 已实现：超出 → `413 body_too_large` |
| 单请求时限 | 15 s | **未强制**（known gap，P4）；`pm`/`am` 等待由各端点自行处理 |

`429 rate_limited` 错误码保留给 P4 的限流中间件；本增量任何请求都不会返回 429。客户端可继续按 §10.5 处理 429（服务端升级后即生效）。

---

## 3. 认证与作用域

### 3.1 认证方式

- 唯一方式：`Authorization: Bearer <token>`。Token 可由用户自定义（默认 `admin123`），也支持 `df_` + 43 字符 base64url。
- 落盘只存 `sha256(token)` 到 `/data/adb/device_faker/config/server.toml`（权限 600），明文只在生成时输出一次。
- 不使用 Cookie 会话，因此没有 CSRF 面；前端统一加 `Authorization` 头。
- 缺失、格式错误、未知、已撤销 → `401 unauthorized`（不区分原因，避免枚举）。
- `GET /api/v1/healthz` 不需要认证；其余端点全部需要。

### 3.2 作用域

| 作用域 | 允许 | 端点示例 |
| --- | --- | --- |
| `read` | 只读：状态、配置、模板、应用、包、设备 | `GET /status`、`GET /templates`、`GET /packages` |
| `write` | 修改配置、模板、应用 | `PUT /config`、`PATCH /templates/{name}`、`PUT /apps/{pkg}` |
| `ops` | 运行时操作：重启应用、出口探测 | `POST /apps/{pkg}/restart`、`POST /egress/probe` |
| `admin` | 服务端管理：Token、设置、导入导出 | 本增量无端点，仅保留 |

判定规则：

1. 作用域**不隐含**：`write` 不包含 `read`，`ops` 不包含 `read`；需要「先读后写」的客户端应申请 `["read","write"]`。
2. `admin` 视为其余三个的超集。
3. 不足 → `403 forbidden_scope`，`details{required, granted}`。
4. 只读 Token 无法执行任何写操作（验收项之一）。

### 3.3 端点 → 作用域

见 §4 总表的「作用域」列；每个端点明细里也重复标注，二者必须一致。

### 3.4 Token 供给

- 首次启动：若 `server.toml` 没有任何 Token 且 `bind` 是 loopback，写入名为 `owner` 的默认 Token **`admin123`**（`["read","write","ops","admin"]`），只把哈希写入 `server.toml`，明文同时写一行 `logs/server.log` 横幅 `[device-faker-server] initial token = admin123`。
- 修改 Token：管理器 WebUI → 设置 → 外部访问 → 「访问 Token」输入框，填任意值后保存；WebUI 用 `sha256sum` 算哈希写入 `server.toml`（0600），重启服务后生效。留空表示不修改。
- `server.toml` 结构（计划书附录 A）：

      bind = "127.0.0.1:8686"
      allow_lan = false
      body_limit_bytes = 2097152
      read_rate_per_min = 120
      write_rate_per_min = 30

      [[tokens]]
      id = "default"
      name = "owner"
      hash = "…"
      scopes = ["read", "write", "ops", "admin"]

- `bind` 非 loopback 且无 Token → **拒绝启动**，日志写明生成命令。
- 本增量不提供 Token 的 HTTP 管理；轮换 = 管理器 WebUI 里改 Token，或直接改 `server.toml` 的 `[[tokens]]` 后重启服务。

### 3.5 浏览器登录（原生模式）

1. 前端在 `window.__DF_SERVER__` 存在时走 HTTP。
2. 首个 API 请求返回 401 → 弹 Token 输入框。
3. Token 默认存 `sessionStorage`，用户勾选「记住」后存 `localStorage`。
4. 设置页展示：服务版本、`bind`、当前 revision、当前 Token 的作用域（由 `GET /status` + 本地记录推断）、退出登录（清空存储）。

### 3.6 审计

每次写操作（含 412/422 等失败尝试）在 `/data/adb/device_faker/logs/api-audit.jsonl` 追加一行（`ts、requestId、tokenId、tokenName、remote、method、path、status、revisionBefore、revisionAfter、durationMs、userAgent`）。`remote` 当前恒为 `null`（未接入 ConnectInfo）。本增量**不提供查询端点**；审计不含 Token 明文与完整配置内容。审计文件 `0600`，写失败只记 warn，不影响请求结果。

---

## 4. 端点总表

| # | 方法 | 路径 | 作用 | 作用域 | 条件请求 |
| --- | --- | --- | --- | --- | --- |
| 1 | GET | `/api/v1/healthz` | 存活探针 | 无 | — |
| 2 | GET | `/api/v1/status` | 模块 / 设备 / 服务 / 配置状态 | read | — |
| 3 | GET | `/api/v1/schema/config` | 配置 JSON Schema | read | — |
| 4 | GET | `/api/v1/config` | 读取配置（JSON 文档或 TOML 原文） | read | — |
| 5 | GET | `/api/v1/config/revision` | 仅取 revision / mtime / size | read | — |
| 6 | PUT | `/api/v1/config` | 整体替换配置 | write | If-Match 必填 |
| 7 | POST | `/api/v1/config/validate` | 校验任意配置文本 | read | — |
| 8 | GET | `/api/v1/templates` | 模板列表（保序） | read | — |
| 9 | GET | `/api/v1/templates/{name}` | 模板详情 | read | — |
| 10 | PUT | `/api/v1/templates/{name}` | 新建或整体替换模板 | write | If-Match 必填 |
| 11 | PATCH | `/api/v1/templates/{name}` | 字段级合并模板 | write | If-Match 必填 |
| 12 | DELETE | `/api/v1/templates/{name}` | 删除模板 | write | If-Match + `confirm=true` |
| 13 | POST | `/api/v1/templates/{name}/packages` | 批量绑定包名 | write | If-Match 必填 |
| 14 | DELETE | `/api/v1/templates/{name}/packages/{pkg}` | 解绑单个包名 | write | If-Match 必填 |
| 15 | GET | `/api/v1/apps` | 已配置应用列表（保序） | read | — |
| 16 | GET | `/api/v1/apps/{pkg}` | 单应用来源与解析结果 | read | — |
| 17 | PUT | `/api/v1/apps/{pkg}` | 写内联配置或绑定模板 | write | If-Match 必填 |
| 18 | DELETE | `/api/v1/apps/{pkg}` | 移除应用配置 | write | If-Match + `confirm=true` |
| 19 | POST | `/api/v1/apps/{pkg}/restart` | force-stop / 重新拉起 | ops | — |
| 20 | GET | `/api/v1/packages` | 已安装包元数据（多用户） | read | — |
| 21 | GET | `/api/v1/device` | 设备与模块信息 | read | — |
| 22 | GET | `/api/v1/device/props` | getprop 白名单读取 | read | — |
| 23 | GET | `/api/v1/device/users` | 用户列表 | read | — |
| 24 | GET | `/api/v1/overview` | 设备、模板与每个应用配置汇总 | read | — |
| 25 | GET | `/api/v1/egress` | 出口代理状态（模式 / 生效值 / 脱敏上游 / 应用覆盖 / 最近探测） | read | — |
| 26 | PUT | `/api/v1/egress` | 写代理定义与设备级 / 应用级出口，并应用方案 A | write | If-Match 必填（egress revision） |
| 27 | DELETE | `/api/v1/egress` | 恢复直连（清空定义与系统代理） | write | If-Match + `confirm=true` |
| 28 | POST | `/api/v1/egress/probe` | 经当前链路探测出口 IP | ops | — |
| 29 | GET | `/api/v1/egress/proxies` | 代理定义列表（脱敏 + 引用关系） | read | — |

---

## 5. 端点明细

约定：所有示例省略 `X-Request-Id` 等通用头；`…` 表示省略的字段。错误响应均为统一信封（§7）。

### 5.1 GET /api/v1/healthz

- 作用域：无。认证：不需要。
- 用途：WebUI 探测原生服务、进程探针。
- 请求：无参数，无请求体。

响应 `200`：

    {
      "status": "ok",
      "version": "1.10.0",
      "uptimeSec": 3600
    }

错误：`500 internal_error`（进程内部自检失败时）。

### 5.2 GET /api/v1/status

- 作用域：`read`。
- 用途：一次拿到模块、设备、服务、配置四块状态；AI 的入口调用。

响应 `200`：

    {
      "module": {
        "id": "device_faker",
        "version": "v1.10.0",
        "versionCode": 20261201
      },
      "device": {
        "manufacturer": "Google",
        "brand": "google",
        "model": "Pixel 9",
        "device": "tokay",
        "android": "17",
        "sdk": 37,
        "abi": "arm64-v8a"
      },
      "server": {
        "version": "1.10.0",
        "bind": "127.0.0.1:8686",
        "uptimeSec": 3600,
        "startedAt": "2026-11-24T03:00:00Z",
        "pid": 4321
      },
      "config": {
        "revision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
        "mtime": "2026-11-24T03:00:00Z",
        "size": 9361,
        "exists": true,
        "templates": 26,
        "apps": 27,
        "conflicts": 0
      },
      "capabilities": ["config", "templates", "apps", "packages", "device", "validate", "overview", "egress"]
    }

字段说明：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `module.version` | string | 模块 `module.prop` 的版本 |
| `module.versionCode` | integer | 模块版本号 |
| `server.pid` | integer | 服务进程 pid（便于排障与 service.sh 对齐） |
| `device.abi` | string | 运行服务的 ABI，恒为 `arm64-v8a` |
| `config.mtime` | string 或 null | 配置文件 mtime（UTC RFC 3339）；文件不存在（`exists: false`）或元数据不可读时为 `null` |
| `config.conflicts` | integer | 同一包名被多个模板声明的数量（去重后的包数） |
| `capabilities` | string[] | 本服务已实现的能力标签；缺失标签代表该能力不可用 |

配置文件不存在时（首次启动 / host 测试）：`config.mtime` 为 `null`、`config.size` 为 `0`、`config.exists` 为 `false`、`config.revision` 为空内容字节的 SHA-256。

错误：401、403、500。

### 5.3 GET /api/v1/schema/config

- 作用域：`read`。
- 返回 **draft-07** JSON Schema（`$schema: http://json-schema.org/draft-07/schema#`），由 `device_faker_core::schema::config_schema()` 用 schemars 0.8 依据 `Config` 结构生成，服务端原样透传，不做二次加工。
- 这是**单份 Config 的 schema**：`apps` / `debug` / `default_force_denylist_unmount` / `default_hide_maps` / `templates` 直接是顶层 `properties`；被引用的类型放在 `definitions`（`AppConfig`、`DeviceTemplate`、`NumberSetting`），`$ref` 形如 `#/definitions/DeviceTemplate`。
- 方言就是 draft-07：没有 `$id`、没有 `additionalProperties: false`，引用只出现在 `definitions` 下；客户端不要按更新版本的方言解析，也不要假设 `prefixItems` 之类的新关键字可用。

响应 `200`（节选，字段名、方言与 core 实际输出一致）：

    {
      "$schema": "http://json-schema.org/draft-07/schema#",
      "title": "DeviceFakerConfig",
      "type": "object",
      "properties": {
        "apps": {
          "default": [],
          "description": "应用配置",
          "items": { "$ref": "#/definitions/AppConfig" },
          "type": "array"
        },
        "debug": {
          "default": false,
          "description": "是否启用调试日志（默认关闭以提高隐蔽性）",
          "type": "boolean"
        },
        "default_force_denylist_unmount": {
          "default": false,
          "description": "是否默认启用 FORCE_DENYLIST_UNMOUNT（避免模块挂载痕迹）",
          "type": "boolean"
        },
        "default_hide_maps": {
          "default": [],
          "description": "要从 /proc/self/maps 中清除的属性映射模式列表（默认不启用）",
          "items": { "type": "string" },
          "type": "array"
        },
        "templates": {
          "additionalProperties": { "$ref": "#/definitions/DeviceTemplate" },
          "default": {},
          "type": "object"
        }
      },
      "definitions": {
        "NumberSetting": {
          "anyOf": [
            {
              "description": "整数伪装值（sdk_int）",
              "maximum": 1000.0,
              "minimum": 1.0,
              "type": "integer"
            },
            {
              "const": "__DELETE__",
              "description": "删除 ro.build.version.sdk 整族",
              "type": "string"
            }
          ],
          "description": "整数伪装值，或字符串 __DELETE__",
          "title": "NumberSetting"
        },
        "DeviceTemplate": {
          "description": "机型模板",
          "properties": { "packages": { "default": [], "items": { "type": "string" }, "type": "array" } },
          "type": "object"
        },
        "AppConfig": {
          "required": ["package"],
          "properties": { "package": { "type": "string" } },
          "type": "object"
        }
      }
    }

- `DeviceTemplate` / `AppConfig` 的 `sdk_int` 字段写作 `anyOf: [{$ref: "#/definitions/NumberSetting"}, {type: "null"}]`，因此合法取值是 **1..=1000 的整数**或字符串 `"__DELETE__"`。
- 客户端不要写死必填字段：除 `apps[].package` 外全部可选；缺省值由各字段的 `default` 表达。
- 本增量只提供这一份 schema（`/schema/config`），模板与应用的 schema 通过 `definitions` 复用，没有 `/schema/template` 或 `/schema/app` 端点。

### 5.4 GET /api/v1/config

- 作用域：`read`。
- 查询参数：

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `format` | `json` 或 `toml` | `json` | `json` 返回 `ConfigResponse`（含 revision 包装）；`toml` 直接返回配置文件的 `text/plain` 原文 |

```
GET /api/v1/config?format=json
Authorization: Bearer $TOKEN
```

响应 `200 application/json`：

    {
      "revision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
      "mtime": "2026-11-24T03:00:00Z",
      "size": 9361,
      "exists": true,
      "warnings": [],
      "config": {
        "debug": false,
        "default_force_denylist_unmount": false,
        "default_hide_maps": [],
        "templates": {
          "pixel_9": {
            "packages": ["com.example.app"],
            "manufacturer": "Google",
            "brand": "google",
            "model": "Pixel 9",
            "device": "tokay",
            "android_version": "15",
            "sdk_int": 35,
            "dpi": 420,
            "custom_props": { "ro.product.vendor.model": "Pixel 9" }
          }
        },
        "apps": [
          { "package": "com.example.two", "manufacturer": "Xiaomi", "model": "2509FPN0BC", "dpi": 420 }
        ]
      }
    }

响应 `200 text/plain; charset=utf-8`（`format=toml`）：响应体为文件原文（保留注释与顺序），头里带 `X-DF-Revision` 与 `ETag`。

配置文件不存在时：`200`，`exists:false`，`mtime:null`，`size:0`，`revision` 为空内容字节的 SHA-256，`config` 为空文档（`templates:{}, apps:[]`，布尔字段为 `false`），`warnings:[{"code":"config_missing"}]`。

错误：401、403、415（不支持的 `format`）、422（`format` 非法）、500。

### 5.5 GET /api/v1/config/revision

- 作用域：`read`。
- 轻量端点，供写前取 `If-Match`。

响应 `200`：

    {
      "revision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
      "mtime": "2026-11-24T03:00:00Z",
      "size": 9361,
      "exists": true
    }

配置文件不存在时（`exists:false`）：

    {
      "revision": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "mtime": null,
      "size": 0,
      "exists": false
    }

- `mtime` 在 `exists:false` 时为 `null`；`size` 为 `0`；`revision` 为空内容字节的 SHA-256。

错误：401、403、500。

### 5.6 PUT /api/v1/config

- 作用域：`write`。`If-Match` 必填。
- 用途：整体替换配置。原生模式 WebUI 的「原始配置编辑」用它。
- 请求体（二选一）：

1. `application/json`：`ConfigDocument`（§6.1），字段 snake_case。
2. `application/toml` 或 `text/plain`：TOML 原文。服务端**不做注释/未知字段保留**（整体替换），但会保留文件权限与原子写语义。

```
PUT /api/v1/config?diff=true
Authorization: Bearer $TOKEN
If-Match: "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5"
Content-Type: application/json

{
  "debug": false,
  "default_force_denylist_unmount": false,
  "default_hide_maps": [],
  "templates": {
    "pixel_9": {
      "packages": ["com.example.app"],
      "brand": "google",
      "model": "Pixel 9",
      "android_version": "15",
      "sdk_int": 35
    }
  },
  "apps": [
    { "package": "com.example.two", "manufacturer": "Xiaomi", "model": "2509FPN0BC" }
  ]
}
```

> **未知字段会被拒绝**（422 `unknown_field`）。`apps[]` 记录的正确写法见 §6.3。

响应 `200`（`WriteResult`，§6.5）：

    {
      "revision": "1a7b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9",
      "previousRevision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
      "dryRun": false,
      "backupId": "20261124T031002.123456789Z-9f2c5d1e",
      "diff": "--- a/config.toml\n+++ b/config.toml\n@@ -1,3 +1,4 @@\n debug = false\n+default_hide_maps = []\n",
      "diffTruncated": false,
      "warnings": []
    }

错误：401、403、412、413、415、422、428、500。

### 5.7 POST /api/v1/config/validate

- 作用域：`read`（不落盘）。无需 `If-Match`。
- 用途：校验任意 TOML / JSON 文本，返回字段级错误。

请求：

    {
      "format": "toml",
      "content": "debug = false\n\n[templates.pixel_9]\npackages = [\"com.example.app\"]\ndpi = 900\n"
    }

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `format` | `"toml"` 或 `"json"` | 是 | `json` 时 `content` 可以是对象，也可为 JSON 字符串 |
| `content` | string 或 object | 是 | 待校验内容；上限 2 MiB |

响应 `200`（**校验失败也是 200**，用 `valid` 区分）：

    {
      "valid": false,
      "revision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
      "errors": [
        {
          "path": "templates.pixel_9.dpi",
          "code": "out_of_range",
          "message": "dpi 必须在 120..=640"
        }
      ],
      "warnings": []
    }

- `revision` 是「校验时的当前配置 revision」，便于客户端判断校验后是否又被人改过。
- 错误码见 §8.2。

错误：401、403、413、415、422（`format` 非法）、500。

### 5.8 GET /api/v1/templates

- 作用域：`read`。
- 查询参数：`limit`、`cursor`、`package`（只列出 packages 里含该包名的模板）。

响应 `200`：

    {
      "revision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
      "total": 2,
      "nextCursor": null,
      "items": [
        {
          "name": "pixel_9",
          "position": 0,
          "packages": ["com.example.app"],
          "packageCount": 1,
          "fieldsSet": ["android_version", "brand", "dpi", "manufacturer", "model", "sdk_int"]
        },
        {
          "name": "samsung_s24_ultra",
          "position": 1,
          "packages": ["com.other.app@999"],
          "packageCount": 1,
          "fieldsSet": ["brand", "dpi", "model"]
        }
      ]
    }

- `items` 顺序 = 配置文件 `[templates]` 的书写顺序，分页也不打乱。
- `packageCount` 等于 `packages` 长度（含 `@userId` 写法）。

错误：401、403、422、500。

### 5.9 GET /api/v1/templates/{name}

- 作用域：`read`。
- 路径参数 `name`：模板名，需百分号编码。

响应 `200`（`X-DF-Revision` + `ETag` 同配置 revision）：

    {
      "revision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
      "position": 0,
      "name": "pixel_9",
      "template": {
        "packages": ["com.example.app"],
        "manufacturer": "Google",
        "brand": "google",
        "model": "Pixel 9",
        "device": "tokay",
        "android_version": "15",
        "sdk_int": 35,
        "dpi": 420,
        "custom_props": { "ro.product.vendor.model": "Pixel 9" }
      }
    }

- 顶层 `name` 是模板名（config.toml 的配置键）；`template` 对象内部**不含**模板名。
- `template.name` 若出现，是**设备内部产品名**（映射 `ro.product.name`），与模板名无关。

错误：401、403、404（`not_found`，`details.resource="template"`）、500。

### 5.10 PUT /api/v1/templates/{name}

- 作用域：`write`。`If-Match` 必填。
- 语义：**整体替换**。请求体里未出现的字段视为「删除该字段」；`packages` 未出现等价于 `[]`（会解绑全部包名，返回 `empty_packages` 警告）。
- 已存在的模板保持原 `position`；新模板追加到末尾。
- 请求体字段：`TemplateInput`（§6.2），不含 `name`（名字取自路径）。

请求：

    {
      "packages": ["com.example.app"],
      "manufacturer": "Samsung",
      "brand": "samsung",
      "model": "SM-S9280",
      "device": "e3q",
      "android_version": "15",
      "sdk_int": 35,
      "dpi": 420,
      "custom_props": { "ro.product.vendor.model": "SM-S9280" }
    }

响应 `200`（已存在）或 `201`（新建）：

    {
      "revision": "77aabbcc…",
      "previousRevision": "9f2c5d1e…",
      "dryRun": false,
      "backupId": "20261124T031002.123456789Z-9f2c5d1e",
      "diffTruncated": false,
      "warnings": []
    }

错误：401、403、409、412、413、422（模板名非法时为 `validation_failed` + `details.errors[0].code="invalid_template_name"`）、428、500。

### 5.11 PATCH /api/v1/templates/{name}

- 作用域：`write`。`If-Match` 必填。
- 语义：JSON Merge Patch 风格。
  - 顶层键**缺席** = 不改；
  - 键值为 `null` = 删除该字段（回到未设置）；
  - `packages`、`custom_props`、`hide_maps` 为整体替换（不做数组合并）；
  - 未知键 → 422 `unknown_field`；
  - 不存在的模板 → 404（PATCH 不创建）。
- `sdk_int` 传 `"__DELETE__"` 表示「删除 SDK 整族属性」；字符串字段的 `"__DELETE__"` 同理。两者都按原样写入 TOML。

请求：

    {
      "dpi": 440,
      "model": "SM-S9280",
      "custom_props": null,
      "force_denylist_unmount": true
    }

响应 `200`：`WriteResult`。

### 5.12 DELETE /api/v1/templates/{name}

- 作用域：`write`。`If-Match` 必填，`confirm=true` 必填。
- 副作用：删除 `[templates.<name>]`；引用该模板的 `[[apps]]` 内联记录**不受影响**（内联本来就整记录覆盖模板）。
- 缺失 `confirm=true` → 400 `confirmation_required`（`details.action="delete_template"`）。

响应 `200`：`WriteResult`。

错误：400、401、403、404、412、428、500。

### 5.13 POST /api/v1/templates/{name}/packages

- 作用域：`write`。`If-Match` 必填。
- 语义：批量绑定包名。
- 请求：

      {
        "packages": ["com.example.app", "com.example.app@999"],
        "mode": "append"
      }

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `packages` | string[] | — | 1..200 项，每项满足包名规则（可带 `@userId`） |
| `mode` | `"append"` 或 `"replace"` | `"append"` | `append`：保留已有顺序并在末尾追加（重复项去重，保留首次出现位置）；`replace`：整体替换 |

- 重复包名（该包已存在于**其它**模板）默认 409；`allow_duplicate_package=true` 时写入并回 `duplicate_package` 警告。
- 响应 `200`：`WriteResult`（需要模板最新状态时再 `GET /templates/{name}`）。

错误：401、403、404、409、412、413、422、428、500。

### 5.14 DELETE /api/v1/templates/{name}/packages/{pkg}

- 作用域：`write`。`If-Match` 必填。
- 路径参数 `pkg`：包名，`@` 需编码为 `%40`（如 `com.example.app%40999`）。
- 语义：从该模板的 `packages` 中删除精确匹配项（不删除 `@999` 这种带用户后缀的不同项）。
- 不存在该包名 → 404 `not_found`（`details.resource="template_package"`）。

响应 `200`：`WriteResult`。

### 5.15 GET /api/v1/apps

- 作用域：`read`。
- 查询参数：`limit`、`cursor`、`source`（`inline` 或 `template`，可选过滤）。
- 返回**已配置**的应用（`[[apps]]` 的内联记录），按数组顺序；模板绑定产生的应用不在此列（用 `GET /templates?package=` 查）。

响应 `200`：

    {
      "revision": "9f2c5d1e…",
      "total": 1,
      "nextCursor": null,
      "items": [
        {
          "package": "com.example.two",
          "position": 0,
          "source": { "type": "inline", "template": null },
          "fieldsSet": ["dpi", "manufacturer", "model"]
        }
      ]
    }

错误：401、403、422、500。

### 5.16 GET /api/v1/apps/{pkg}

- 作用域：`read`。
- 用途：解释单个应用的最终来源与生效字段（「为什么没生效」的入口）。

响应 `200`：

    {
      "package": "com.example.app",
      "revision": "9f2c5d1e…",
      "installed": true,
      "source": { "type": "template", "template": "pixel_9", "alsoInline": false },
      "resolved": {
        "manufacturer": "Google",
        "brand": "google",
        "model": "Pixel 9",
        "device": "tokay",
        "android_version": "15",
        "sdk_int": 35,
        "dpi": 420,
        "custom_props": { "ro.product.vendor.model": "Pixel 9" },
        "force_denylist_unmount": false,
        "hide_maps": [],
        "companion_resetprop": false
      },
      "warnings": []
    }

- `source.type`：`"inline"` | `"template"` | `"none"`（未配置）。
- `source.template`：命中的模板名；内联时 `null`。
- `source.alsoInline`：内联记录存在但同时命中模板（内联优先，模板不生效）时的提示。
- `installed`：能否从 PackageManager 查到；查不到时 `false`；`pm` 不可用时 `null`。
- `resolved`：**最终生效值**。未设置的字符串字段直接省略；布尔/数组字段给出继承后的有效值。`packages` 不会出现在这里。
- 未配置的包名也返回 `200`，`source.type="none"`、`resolved:{}, warnings:[{code:"no_config"}]`。

错误：401、403、422（包名非法）、500。

### 5.17 PUT /api/v1/apps/{pkg}

- 作用域：`write`。`If-Match` 必填。
- 两种模式，`template` 与 `config` **必须恰好提供一个**（否则 422 `invalid_value`，`path="template|config"`）。

模式 A：绑定模板

    {
      "template": "pixel_9",
      "detachFromTemplates": true
    }

1. 模板不存在 → 404（`details.resource="template"`）。
2. 把 `pkg` 加入该模板 `packages` 末尾（已存在则原地不动）。
3. `detachFromTemplates` 默认 `true`：把 `pkg` 从**其它**模板的 `packages` 中移除，避免重复包名冲突。
4. 若存在该包的内联 `[[apps]]` 记录，**删除它**（内联优先，留着会让绑定无效），并回 `inline_record_removed` 警告。

模式 B：写内联配置

    {
      "config": {
        "package": "com.example.two",
        "manufacturer": "Xiaomi",
        "brand": "Redmi",
        "model": "2509FPN0BC",
        "android_version": "15",
        "sdk_int": 35,
        "dpi": 420,
        "force_denylist_unmount": true
      }
    }

1. `config.package` 可省略；出现时必须与路径 `pkg` 一致，否则 422。
2. 语义为**整记录替换**（与该包原有的内联记录合并方式一致），不是字段合并；要字段级修改请先 `GET /apps/{pkg}` 再整体写回。
3. `detachFromTemplates` 默认 `false`：保留模板绑定（内联仍然优先），不产生额外写入。

响应 `200` 或 `201`（新建内联记录）：`WriteResult`（`warnings` 可能含 `duplicate_package`、`inline_record_removed`、`package_not_installed`）。

错误：401、403、404、409、412、413、422、428、500。

### 5.18 DELETE /api/v1/apps/{pkg}

- 作用域：`write`。`If-Match` 必填，`confirm=true` 必填。
- 查询参数 `detach_templates`（默认 `false`）：为 `true` 时同时把该包名从所有模板的 `packages` 中移除。
- 什么都没删到 → 404 `not_found`（`details.resource="app_config"`）。

响应 `200`：`WriteResult`。

错误：400、401、403、404、412、428、500。

### 5.19 POST /api/v1/apps/{pkg}/restart

- 作用域：`ops`。不要求 `If-Match`（读配置不涉及写入）。
- 语义：`am force-stop`；可选 `am start` 重新拉起。
- 请求体可省略，字段全为可选：

      {
        "forceStop": true,
        "launch": false,
        "user": 0
      }

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `forceStop` | boolean | `true` | 执行 `am force-stop --user <user> <pkg>` |
| `launch` | boolean | `false` | force-stop 后再 `am start`（monkey 或 launcher intent） |
| `user` | integer | `0` | Android 用户 id；`pkg` 里的 `@uid` 优先 |

响应 `200`：

    {
      "package": "com.example.app",
      "forceStopped": true,
      "launched": false,
      "durationMs": 118
    }

- 未安装 → 404 `not_found`（`details.resource="package"`）。
- `am` 执行失败/超时 → 503 `upstream_unavailable`（`details.component="am"`）。
- 带 `Idempotency-Key` 时，24h 内同键重放直接返回首次结果。

错误：401、403、404、500、503；429 为 P4 限流预留，本增量不会返回。

### 5.20 GET /api/v1/packages

- 作用域：`read`。
- 查询参数：

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `names` | 逗号分隔包名 | 全部 | 只查这些包（≤500 个）；用于 `pm getPackagesInfo` 的等价能力 |
| `user` | integer | 全部用户 | 只返回该用户下安装的包 |
| `limit` / `cursor` | — | 50 / — | 分页 |

响应 `200`：

    {
      "total": 1,
      "nextCursor": null,
      "source": "pm",
      "warnings": [],
      "items": [
        {
          "package": "com.android.chrome",
          "userIds": [0],
          "installed": true,
          "isSystem": true,
          "versionName": "131.0.6778.200",
          "versionCode": 677820000,
          "label": null
        }
      ]
    }

- `source`：`"pm"` 表示取自 PackageManager；`"unavailable"` 表示系统查询失败（此时 `items:[]` 且 `warnings` 含 `pm_unavailable`）。
- `label` 本增量恒为 `null`（应用名属 P4）。
- `userIds`：该包已安装在哪些用户下。

错误：401、403、422、500。

### 5.21 GET /api/v1/device

- 作用域：`read`。

响应 `200`：

    {
      "module": { "id": "device_faker", "version": "v1.10.0", "versionCode": 20261201 },
      "device": {
        "manufacturer": "Google",
        "brand": "google",
        "model": "Pixel 9",
        "device": "tokay",
        "android": "17",
        "sdk": 37,
        "abi": "arm64-v8a",
        "fingerprint": "google/tokay/tokay:17/BP2A.250605.031/13000000:user/release-keys"
      },
      "server": { "version": "1.10.0", "bind": "127.0.0.1:8686", "uptimeSec": 3600, "pid": 4321 },
      "root": { "manager": "kernelsu", "version": "1.0.7" }
    }

- `root.manager`：`"kernelsu"` | `"magisk"` | `"apatch"` | `null`（无法识别）。

错误：401、403、500。

### 5.22 GET /api/v1/device/props

- 作用域：`read`。
- 查询参数 `keys`：逗号分隔，1..50 个，每个 ≤128 字符。
- 白名单前缀（其余一律 422）：`ro.product.`、`ro.build.`、`ro.soc.`、`ro.hardware`、`ro.board.`、`ro.vendor.`。

响应 `200`：

    {
      "props": {
        "ro.product.model": "Pixel 9",
        "ro.build.fingerprint": "google/tokay/tokay:17/…"
      },
      "missing": ["ro.vendor.does_not_exist"],
      "source": "getprop"
    }

- `props`：读到的键值。
- `missing`：请求了但设备上不存在的键。
- `source`：`"getprop"`（正常）或 `"unavailable"`（不可用；此时返回 503，见下）。

错误：401、403、422（键不在白名单 / 数量超限）、500、503（`getprop` 不可用）。

### 5.23 GET /api/v1/device/users

- 作用域：`read`。

响应 `200`：

    {
      "users": [
        { "id": 0, "name": "Owner", "running": true, "isCurrent": true },
        { "id": 999, "name": "Work", "running": true, "isCurrent": false }
      ]
    }

错误：401、403、500。

### 5.24 GET /api/v1/overview

- 作用域：`read`。
- 用途：一次拿全「设备状态 + 物理 fingerprint + 每个已配置应用的有效指纹」，AI 不必逐包调用。
- `apps` 只包含**已配置**的包（内联记录与所有模板 `packages` 的并集），按包名排序；未配置的已安装包不出现。

响应 `200`：

    {
      "revision": "9f2c5d1e…",
      "module": { "id": "device_faker", "version": "v1.10.0", "versionCode": 20261201 },
      "device": {
        "manufacturer": "Google", "brand": "google", "model": "Pixel 9",
        "device": "tokay", "android": "17", "sdk": 37, "abi": "arm64-v8a",
        "fingerprint": "google/tokay/tokay:17/BP2A.250605.031/13000000:user/release-keys"
      },
      "root": { "manager": "kernelsu", "version": "1.0.7" },
      "server": { "version": "1.10.0", "bind": "127.0.0.1:8686", "uptimeSec": 3600, "startedAt": "2026-11-24T03:00:00Z", "pid": 4321 },
      "config": {
        "revision": "9f2c5d1e…", "mtime": "2026-11-24T03:00:00Z",
        "size": 9361, "exists": true, "templates": 2, "apps": 1, "conflicts": 1
      },
      "templates": [
        {
          "name": "pixel_9", "position": 0,
          "packages": ["com.example.app"],
          "config": { "brand": "google", "model": "Pixel 9", "dpi": 420 }
        }
      ],
      "apps": [
        {
          "package": "com.example.app",
          "installed": true,
          "source": { "type": "template", "template": "pixel_9", "alsoInline": false },
          "resolved": { "brand": "google", "model": "Pixel 9", "fingerprint": "google/tokay/tokay:17/…" },
          "warnings": []
        }
      ],
      "warnings": []
    }

- `resolved` 与 `GET /apps/{pkg}` 的 `resolved` 相同：最终生效值（含 `fingerprint`、`dpi`、`sdk_int` 等）。
- `templates[].config` 是模板原始字段（不含 `packages`）。
- 响应带 `X-DF-Revision`，可用于下一次写操作的 `If-Match`。

错误：401、403、500。

### 5.25 GET /api/v1/egress

- 作用域：`read`。无 `If-Match`；响应头带 `X-DF-Egress-Revision`（不带引号），**不带** `X-DF-Revision` / `ETag`。
- 用途：一次拿到「当前出口模式 + 设备上真实读回的生效值 + 脱敏上游 + 各应用覆盖 + 最近探测」。不需要自己拼 `egress.toml`。

响应 `200`（状态块字段见 §6.8）：

    {
      "revision": "5b1f0c9e8d7a6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c",
      "configRevision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
      "configured": true,
      "mode": "proxy",
      "effectiveMode": "proxy",
      "default": "office_http",
      "failOpen": true,
      "probeUrl": "http://checkip.example.com/",
      "probeTimeoutSeconds": 5,
      "exclusionList": ["10.0.0.0/8"],
      "upstream": {
        "name": "office_http", "type": "http",
        "host": "198.51.100.10", "port": 8080, "hasCredentials": false
      },
      "proxies": [
        { "name": "office_http", "type": "http",   "host": "198.51.100.10", "port": 8080, "hasCredentials": false },
        { "name": "lab_socks",   "type": "socks5", "host": "198.51.100.20", "port": 1080, "hasCredentials": true }
      ],
      "apps": [
        { "package": "com.example.a", "egress": "lab_socks",  "effective": "proxy",  "proxy": "lab_socks" },
        { "package": "com.example.b", "egress": "__DIRECT__", "effective": "direct", "proxy": null }
      ],
      "applied": true,
      "readback": {
        "settingsValue": "198.51.100.10:8080",
        "dumpsysGlobalProxy": "Global proxy: [198.51.100.10] port: 8080",
        "matches": true,
        "at": "2026-12-01T03:00:00Z"
      },
      "health": {
        "degraded": false, "degradedSince": null, "degradedReason": null,
        "lastProbe": {
          "ok": true, "exitIp": "198.51.100.77", "latencyMs": 412,
          "checkedAt": "2026-12-01T02:59:12Z", "scope": "device", "via": "proxy",
          "proxy": "office_http", "upstreamStatus": 200, "errorCode": null
        }
      },
      "egressFile": { "exists": true, "size": 412, "mtime": "2026-12-01T02:58:03Z" },
      "warnings": []
    }

- `applied` **只描述方案 A**（设备级 HTTP 代理）：读回值与期望值是否一致；应用级注入（方案 B）无法从服务进程读回，只体现在 `apps[].effective`。
- `warnings` 可能包含：`egress_proxy_not_found`、`egress_credentials_ignored`、`egress_auth_unsupported`（生效代理带凭据：凭据**不会生效**，`details={scope:"device"|"app", proxy, field:"username"|"password"}`，同一代理只报一次）、`egress_inline_shadows_template`、`egress_app_not_installed`、`egress_degraded_to_direct`（仅在**清直连真的落地**时出现）、`egress_settings_mismatch`（读回不一致，或 `details.reason="settings_unavailable"` 表示 `settings` 命令不可用）、`egress_perm_fixed`、`egress_probe_approximate`、`unknown_field_preserved`（egress.toml 里有未知键，已原样保留）。
- `health.degradedReason` 取值：`probe_failed`、`settings_mismatch`、`settings_unavailable`（降级动作执行了但**清直连没落地**：此时 `effectiveMode` 报实际模式，不会谎报 `direct`）。

egress 警告码一览（设备端 `warnings[]`；与 §6.6 的通用码同形状）：

| code | details | 触发 |
| --- | --- | --- |
| `egress_proxy_not_found` | `{name, scope:"device"\|"app", package?}` | 引用的代理名在 `egress.toml` 未定义（允许写入，运行期按直连处理） |
| `egress_credentials_ignored` | `{name}` | 设备级指向带凭据代理：方案 A 只写 `host:port` |
| `egress_auth_unsupported` | `{scope:"device"\|"app", proxy, field:"username"\|"password"}` | 生效代理带凭据但注入不支持认证；**同一代理只报一次**，写操作照常 2xx、不阻断 |
| `egress_inline_shadows_template` | `{package, template}` | 写应用级 `egress` 会为该包建立内联记录、整记录覆盖模板 |
| `egress_app_not_installed` | `{package}` | 应用级覆盖的包未安装（不阻断） |
| `egress_degraded_to_direct` | `{reason}` | 本次请求触发了 fail-open 且**清直连真的落地** |
| `egress_settings_mismatch` | `{expected, actual}` 或 `{reason:"settings_unavailable"}` | 写后读回不一致 / `settings` 命令不可用 |
| `egress_perm_fixed` | `{path, mode}` | `egress.toml` 权限被自愈为 0600 |
| `egress_probe_approximate` | `{package}` | 应用级探测在服务进程内完成（结果只代表该代理定义） |
| `egress_probe_scope_app` | 出现在 `502 egress.probe_failed` 的 `details.warnings[]` | 应用级探测失败、设备级系统代理保持不变 |
- **带认证的代理本期不可用**：`username` / `password` 会被接收并安全存储（唯一信号 `hasCredentials`），但**应用流量不会发送认证**——SOCKS5 的 `java.net.socks.username/password` 属性进程内可读但 libcore 不发送（Android 17 实测代理侧 `methods=0002 user_ok=0 pass_ok=0` → `auth-failed`），HTTP Basic 未实现（只注入 host/port）。带凭据的代理请用 **IP 白名单**授权，或等后续「设备内本地认证中继」（P2）。注意本接口看不出这一点：`hasCredentials:true` 只表示**存了凭据**，不表示能出网；设备的探测走服务自己的客户端（SOCKS5 下可带认证），所以「探测成功」也不等于「应用能出网」。**从 v1.10.0 起这一点会显式提示**：生效代理（设备级或应用级）带 `username`/`password` 时，`GET/PUT/DELETE /egress` 与 `POST /egress/probe` 的 `warnings` 都会带 `egress_auth_unsupported`（`details={scope, proxy, field}`，同一代理只报一次）。详见 [CONFIG.md「已知限制」](CONFIG.md#已知限制)。
- **注入侧状态不在本响应里**：应用级出口的「后置重放」属于 Zygisk 进程内行为（`replay hook unavailable`、`proxy_selector degraded`、`replay done ... state=match|mismatch`），服务进程读不到，**不产生任何 warning / 字段**，只落 logcat 与模块日志（详见 [CONFIG.md](CONFIG.md#应用级注入如何生效后置重放)）。换言之：`apps[].effective` 只表示**配置层的生效目标**，不代表目标进程内的注入结果。
- `egress.toml` 解析失败 → `500 internal_error`，`details.op="egress_parse"`（fail closed，不按空配置继续）。此时用 `If-Match: *` 的 `PUT /egress` 重写即可修复。

错误：401、403、500。

### 5.26 PUT /api/v1/egress

- 作用域：`write`。**`If-Match` 必填**（egress revision，见 §2.5）。
- 用途：写代理定义（`egress.toml`）与设备级 / 应用级出口（`config.toml` 的 `[egress].default` 与 `[[apps]].egress`），随后把设备级出口应用到 Android 的 `settings global http_proxy`。
- 语义总则：**省略 = 不改；`null` = 清空；给了数组 = 整体替换该数组**。

请求体（全部字段可选；未知字段 → 422）：

| 字段 | 类型 | 省略 | `null` | 说明 |
| --- | --- | --- | --- | --- |
| `default` | string | 不改 | 清除设备级默认（等价直连） | 设备级代理名，须符合 §8.1 的 egress 名规则；**不接受** `"__DIRECT__"` |
| `failOpen` | boolean | 不改 | —（不接受 null） | 代理不可用时是否放行直连（默认 `true`） |
| `probeUrl` | string | 不改 | 恢复默认值 | 探测 URL，必须以 `http://` 开头，≤512 字符 |
| `probeTimeoutSeconds` | integer | 不改 | — | 1..=30，默认 5 |
| `exclusionList` | string[] | 不改 | 清除该键 | ≤32 项，每项 ≤64 字符（写 `global_http_proxy_exclusion_list`） |
| `proxies` | object[] | 不改（**已存定义与密码原样保留**） | —（不接受 null） | **整体替换**；`[]` 清空全部定义；≤64 条 |
| `proxies[].name` | string | 必填 | — | 代理名；同一请求内唯一 |
| `proxies[].type` | `"http"` \| `"socks5"` | 必填 | — | 协议 |
| `proxies[].host` | string | 必填 | — | 主机名或 IPv4；≤253 字符，不含 scheme / 端口 / 路径 |
| `proxies[].port` | integer | 必填 | — | 1..=65535 |
| `proxies[].username` | string | 不改**该条已存用户名** | 清除 | ≤128 字符；只写不读。**本期应用流量不使用它**（见 §5.25） |
| `proxies[].password` | string | 不改**该条已存密码** | 清除 | ≤256 字符；只写不读。**本期应用流量不发送认证**（见 §5.25） |
| `apps` | object[] | 不改任何应用覆盖 | 清除**所有** `[[apps]].egress` | **整体替换**：列出的应用写入/清除该键，**未列出的应用会删掉 `egress` 键**（其它机型字段不动）；`[]` 与 `null` 等价于清空全部 |
| `apps[].package` | string | 必填 | — | 包名（可带 `@userId`） |
| `apps[].egress` | string | **必填**（缺 → 422 `missing_required`） | 清除该应用的覆盖 | 代理名或 `"__DIRECT__"` |

查询参数：

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `dry_run` | `true`/`false` | `false` | 完整校验并计算 diff，但不落盘、不改 `settings`、不备份；响应 `dryRun:true`，`revision` / `applied` 保持当前值 |
| `diff` | `true`/`false` | `false` | 成功写入时附 unified diff（两份文件合并展示）；上限 64 KiB，超出 `diffTruncated:true` |

响应 `200`（`WriteResult` 的超集，见 §6.5 与 §6.8）：

    {
      "revision": "8c1d…",
      "previousRevision": "5b1f…",
      "configRevision": "77aa…",
      "dryRun": false,
      "changed": true,
      "backupId": "20261201T030000.987654321Z-5b1f2c3d",
      "diffTruncated": false,
      "mode": "proxy",
      "effectiveMode": "proxy",
      "applied": true,
      "warnings": []
    }

- **幂等**：期望状态与当前状态一致时不写任何文件、不备份、mtime 不变，返回 `changed:false` 与原 `revision`；因此带同一 `If-Match` 重复提交同内容恒为 `200`。真正写入后再拿**旧** `If-Match` 重放会得到 `412`（客户端重新 `GET` 即可）。
  - **但 `settings` 的读回/补写与「文件是否变化」无关**：文件没变（`changed:false`）时仍会读回系统代理，发现被外部改回直连就补写一次，响应保持 `changed:false`、`revision` 不变（只有读回本身不产生写入时才 `write_count=0`）。
- 写入顺序固定：先把 `egress.toml`、`config.toml` 落盘，再在**同一把排他锁内**应用 `settings`（避免与并发写交叉）；应用失败不回滚文件（差异由 `applied` / `readback` 暴露，可重试）。
- **diff 脱敏**：`diff=true` 与 `dry_run=true` 返回的 unified diff 会在返回前对 `username` / `password` 行做行级脱敏（值替换为 `<redacted>`，键名保留），**不会出现**旧值或新密码。
- **fail-open 粒度**：只有**设备级**（`scope=device`，链路来自 `[egress].default`）的失败才会清直连降级；`POST /egress/probe` 的应用级失败（`scope=app`）只记 `health.lastProbe` 并在错误 `details.warnings` 里说明，**不动**设备级系统代理。
- `backupId` 形态：`<YYYYMMDDTHHMMSS.nnnnnnnnnZ>-<revision 前 8 位>`（例 `20261201T030000.987654321Z-5b1f2c3d`）；备份文件名 `backups/egress.toml.<stamp>-<rev8>.bak`（`config.toml` 同规则，见 [SERVER.md](SERVER.md)）。
- 本端点**不自动探测**；需要「下发 + 校验」的调用方另外调 `POST /egress/probe`。
- 应用级 `egress` 会给该包建立内联记录（整记录覆盖模板指纹），此时返回 `egress_inline_shadows_template` 警告。

错误：401、403、412、413、415、422、428、500、503。

> `503 upstream_unavailable` 只表示 `settings` 命令失败或不可用（`details.component="settings"`）：文件已写入但应用/读回失败，服务端另记一行 `settings 命令执行失败`（`code` 为退出码、`stderr` 截断 ≤256 字符、不含凭据）。**不存在**名为 `settings_api_error` 的对公错误码——那是实现里的函数名。

### 5.27 DELETE /api/v1/egress

- 作用域：`write`。**`If-Match` 必填**；**`confirm=true` 必填**（缺失 → `400 confirmation_required`，`details.action="egress_clear"`）。
- 用途：一键恢复直连。
- 清除内容：`[proxies.*]` 全部定义、`[egress].default`、**所有** `[[apps]].egress`、`global_http_proxy_exclusion_list`，并执行 `settings put global http_proxy :0`（**必须用 `:0`**：`settings delete` 在 Android 17 上清不掉，实测见 [SERVER.md](SERVER.md)）；`[probe]` 段（`url` / `timeout_seconds` / `fail_open`）保留。
- 已经是直连且无定义 → `changed:false`，不写盘、不执行 `settings`。
- 响应 `200`：与 §5.26 同结构（`mode:"direct"`、`default:null`、`upstream:null`、`applied:true` 为正常结果）。

错误：400、401、403、412、428、500、503。

### 5.28 POST /api/v1/egress/probe

- 作用域：`ops`。无 `If-Match`；**不支持** `Idempotency-Key`（每次都真发请求，且可能触发 fail-open）。
- 用途：经**当前生效链路**请求 probe URL，返回出口 IP。

请求体可省略（全字段可选）：

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `package` | string | `null` | `null` = 按设备级链路；给定包名 = 用该应用的生效代理定义发请求 |
| `url` | string | `[probe].url` | 单次覆盖探测 URL；规则同 §5.26 |

响应 `200`（字段见 §6.10）：

    {
      "ok": true,
      "exitIp": "198.51.100.77",
      "latencyMs": 412,
      "checkedAt": "2026-12-01T03:00:00Z",
      "scope": "device",
      "via": "proxy",
      "proxy": "office_http",
      "url": "http://checkip.example.com/",
      "upstreamStatus": 200,
      "approximate": false,
      "warnings": []
    }

失败（统一走错误信封，不用 200 包 `ok:false`）：

| 情形 | HTTP | code | details |
| --- | --- | --- | --- |
| 没有任何可用出口 | 409 | `egress.not_configured` | `{scope, package?}` |
| 探测失败 | 502 | `egress.probe_failed` | `{url, reason, timeoutMs, upstreamStatus?, scope, warnings[]}`（`scope` = `device`/`app`；`warnings` 见下） |
| `settings` 不可用 | 503 | `upstream_unavailable` | `{component:"settings"}` |
| `package` / `url` 非法 | 422 | `validation_failed` | `{errors:[…]}` |

- 探测本身**不回落**：失败就是 502，`via` 不会因为开了 fail-open 变成 `direct`。fail-open 只影响流量链路，且**只对设备级链路生效**：当 `scope="device"`、`reason` ∈ `connect`/`dns`/`timeout`/`tls` 且 `fail_open=true` 时，服务把系统代理切成直连并在 `GET /egress` 的 `health` 里标记降级；`details.warnings` 会带一条 `egress_degraded_to_direct`（`details.cleared` 表示清直连是否真的落地）。
- **应用级失败不降级**：`scope="app"` 的失败只记 `health.lastProbe`，`details.scope="app"`，并在 `details.warnings` 里给一条 `egress_probe_scope_app`（说明「设备级系统代理保持不变」）；不会写任何 `settings`。
- `scope:"app"` 时 `approximate` 恒为 `true` 且附 `egress_probe_approximate` 警告：探测在**服务进程**里用该应用的代理定义发请求，不代表目标应用进程内的真实链路（注入发生在 Zygisk 侧，无回传通道）。

错误：401、403、409、422、502、503、500。

### 5.29 GET /api/v1/egress/proxies

- 作用域：`read`。无 `If-Match`。
- 用途：给 WebUI 下拉框与「删定义前看谁在引用」提供**脱敏**定义列表。

响应 `200`：

    {
      "revision": "5b1f…",
      "configRevision": "9f2c…",
      "total": 2,
      "egressFile": { "exists": true, "size": 412, "mtime": "2026-12-01T02:58:03Z" },
      "items": [
        { "name": "office_http", "type": "http",   "host": "198.51.100.10", "port": 8080,
          "hasCredentials": false, "usedAsDefault": true,  "apps": [] },
        { "name": "lab_socks",   "type": "socks5", "host": "198.51.100.20", "port": 1080,
          "hasCredentials": true,  "usedAsDefault": false, "apps": ["com.example.a"] }
      ],
      "warnings": []
    }

- `items[].apps` 是引用该定义的包名（按 `[[apps]]` 顺序）；`usedAsDefault` 表示它是否是 `[egress].default`。
- 列表**不分页**（定义上限 64 条）；**永不含** `username` / `password`，唯一凭据信号是 `hasCredentials`。

错误：401、403、500。

---

## 6. 数据结构

### 6.1 ConfigDocument

与 `config.toml` 一一对应，字段全部 snake_case。

    {
      "debug": false,
      "default_force_denylist_unmount": false,
      "default_hide_maps": [],
      "egress": { "default": "office_http" },
      "templates": {
        "<name>": { "packages": [], "brand": "google", "model": "Pixel 9", "sdk_int": 35, "dpi": 420 }
      },
      "apps": [
        { "package": "com.example.app", "brand": "Xiaomi", "sdk_int": 35 }
      ]
    }

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `debug` | boolean | `false` | 调试日志 |
| `default_force_denylist_unmount` | boolean | `false` | 全局 FORCE_DENYLIST_UNMOUNT 默认值 |
| `default_hide_maps` | string[] | `[]` | 默认 maps 清理模式 |
| `egress` | object | 省略 | 设备级出口：只允许一个键 `default`（代理名，v1.10.0 新增）。出现 `host` / `port` / `username` / `password` 等其它键 → 422 `unknown_field`（凭据只存在于 `egress.toml`） |
| `templates` | object | `{}` | 键为模板名；**顺序即书写顺序**，服务端保留 |
| `apps` | array | `[]` | 内联应用配置，顺序保留 |

- 未设置的字段可省略；`null` 等价于省略（响应里不会出现 `null` 的模板字段）。
- 未知字段：请求中出现 → 422；文件中已存在 → 保留并回 `unknown_field_preserved` 警告。
- JSON 对象键顺序在规范里不保证，需要顺序时用 `GET /templates` 的数组 + `position`。

### 6.2 模板对象（TemplateInput）

`GET /templates/{name}` 响应里的 `template`、以及 config.toml 中 `[templates.<name>]` 的值，都是下面这个对象：**不含模板名**。

    {
      "packages": ["com.example.app", "com.example.app@999"],
      "manufacturer": "Google",
      "brand": "google",
      "model": "Pixel 9",
      "name": "tokay",
      "device": "tokay",
      "product": "tokay",
      "hardware": "tensor",
      "board": "g4",
      "soc_model": "Tensor G4",
      "fingerprint": "google/tokay/tokay:17/…",
      "build_id": "BP2A.250605.031",
      "display_id": "BP2A.250605.031",
      "incremental": "13000000",
      "security_patch": "2025-06-05",
      "characteristics": "phone",
      "android_version": "17",
      "sdk_int": 37,
      "dpi": 420,
      "custom_props": { "ro.product.vendor.model": "Pixel 9" },
      "force_denylist_unmount": false,
      "hide_maps": [],
      "companion_resetprop": false
    }

> 模板名由三处提供：`GET /templates` 列表项的 `name`、`GET /templates/{name}` 顶层的 `name`、ConfigDocument 里 `templates` 的对象键。模板对象内部的 `name` 字段是**设备内部产品名**（映射 `ro.product.name`），两者不共用同一个 JSON 字段。API 请求里的未知字段一律 422。

| 字段 | 类型 | 约束 | 对应 config.toml |
| --- | --- | --- | --- |
| `packages` | string[] | 每项符合包名规则，可带 `@userId`；≤200 项 | `templates.<name>.packages` |
| `manufacturer` | string | ≤512 | `manufacturer` |
| `brand` | string | ≤512 | `brand` |
| `marketname` | string | ≤512 | `marketname` |
| `model` | string | ≤512 | `model` |
| `name` | string | ≤512（模板内部字段，不是模板名） | `name` |
| `device` | string | ≤512 | `device` |
| `product` | string | ≤512 | `product` |
| `hardware` | string | ≤512 | `hardware` |
| `board` | string | ≤512 | `board` |
| `soc_model` | string | ≤512 | `soc_model` |
| `fingerprint` | string | ≤512 | `fingerprint` |
| `build_id` | string | ≤512 | `build_id` |
| `display_id` | string | ≤512 | `display_id` |
| `incremental` | string | ≤512 | `incremental` |
| `security_patch` | string | `YYYY-MM-DD` | `security_patch` |
| `characteristics` | string | ≤512 | `characteristics` |
| `android_version` | string | 数字（可带点）≤16 字符 | `android_version` |
| `sdk_int` | integer 或 `"__DELETE__"` | 1..=1000 | `sdk_int` |
| `dpi` | integer | 120..=640 | `dpi` |
| `custom_props` | object | 键 `^(ro\|persist)\.`，值 ≤1024 字符，≤128 项 | `custom_props` |
| `force_denylist_unmount` | boolean | — | `force_denylist_unmount` |
| `hide_maps` | string[] | 每项 ≤256，≤64 项 | `hide_maps` |
| `companion_resetprop` | boolean | — | `companion_resetprop` |

标记值（与 `docs/CONFIG.md` 一致）：`""` / `"__EMPTY__"` 写空串；`"__DELETE__"` 删除对应属性族（仅在 `companion_resetprop=true` 时真正删除）。

### 6.3 AppConfig

与 `[[apps]]` 一一对应，字段集合同 `Template` 去掉 `packages`，加上必填 `package`。

    {
      "package": "com.example.two",
      "manufacturer": "Xiaomi",
      "brand": "Redmi",
      "model": "2509FPN0BC",
      "name": "popsicle",
      "device": "Xiaomi 15 Pro",
      "product": "popsicle",
      "android_version": "15",
      "sdk_int": 35,
      "dpi": 420,
      "custom_props": { "ro.debug.mode": "__DELETE__" },
      "force_denylist_unmount": true,
      "hide_maps": [],
      "companion_resetprop": false
    }

- `package` 必填（`PUT /apps/{pkg}` 的 `config` 里可省略，路径值胜出）。
- 内联记录是**整记录**语义：未设置字段**不**回落模板，只回落全局默认（`docs/CONFIG.md` 优先级）。
- 同一 `package` 出现多次 → 422 `duplicate_app_record`。
- `egress`（v1.10.0 新增）：应用级出口，取值是 `egress.toml` 里的代理名或 `"__DIRECT__"`（强制直连，会主动覆盖设备级系统代理）；未设置时继承 `[egress].default`，两者都没有则直连。非法名 → 422 `invalid_egress_name`；引用了尚未定义的代理名**允许写入**（运行期给 `egress_proxy_not_found` 警告）。模板（`[templates.*]`）**不支持** `egress`。

### 6.4 resolved（`GET /apps/{pkg}` 的解析结果）

- 字符串字段：只出现在配置里显式设置过时；未设置则**省略**。
- 布尔字段（`force_denylist_unmount`、`companion_resetprop`）与数组字段（`hide_maps`）：给出继承后的有效值。
- 不包含 `packages` 与 `package`。
- 也不包含 `egress`：出口链路不是机型字段，它在 `GET /apps/{pkg}` 里不可见，要看 `GET /egress` 的 `apps[].effective`（§5.25）。

### 6.5 WriteResult

    {
      "revision": "新值",
      "previousRevision": "旧值",
      "dryRun": false,
      "backupId": "20261124T031002.123456789Z-9f2c5d1e",
      "diff": "--- a/config.toml\n+++ b/config.toml\n@@ …",
      "diffTruncated": false,
      "warnings": [
        {
          "code": "duplicate_package",
          "message": "com.example.app 同时被 pixel_9 与 samsung_s24_ultra 声明",
          "details": { "package": "com.example.app", "templates": ["pixel_9", "samsung_s24_ultra"] }
        }
      ]
    }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `revision` | string | 写入后的配置 revision（`dryRun:true` 时仍为当前值） |
| `previousRevision` | string | 写入前 revision；首次创建时为 null |
| `dryRun` | boolean | 是否演练 |
| `backupId` | string 或 null | 自动备份 ID；`dryRun` 时为 `null` |
| `diff` | string | 仅在 `diff=true` 或 `dry_run=true` 时出现 |
| `diffTruncated` | boolean | diff 超过 64 KiB 被截断 |
| `warnings` | Warning[] | 见 §6.6 |

### 6.6 Warning

    { "code": "empty_packages", "message": "模板 pixel_9 没有绑定任何包名", "details": { "template": "pixel_9" } }

| code | 触发 | details |
| --- | --- | --- |
| `duplicate_package` | 同一包名出现在多个模板 | `{package, templates}` |
| `empty_packages` | 模板 packages 为空 | `{template}` |
| `package_not_installed` | 绑定的包名未安装 | `{package}` |
| `inline_record_removed` | 绑定模板时删掉了同包名内联记录 | `{package}` |
| `config_missing` | 配置文件不存在 | `{}` |
| `unknown_field_preserved` | 文件里存在未知字段，已原样保留 | `{fields:[]}` |
| `pm_unavailable` | PackageManager 不可用 | `{}` |
| `no_config` | `GET /apps/{pkg}` 查不到任何配置 | `{package}` |

> egress 相关的警告码见 §5.25。**带认证的代理失败不会产生 warning**：服务进程看不到应用侧的认证结果，症状表现为应用自己的网络错误（见 §5.25 的认证说明与 [CONFIG.md「已知限制」](CONFIG.md#已知限制)）。

### 6.7 ValidationError

    { "path": "apps[0].package", "code": "invalid_package", "message": "不是合法 Java 包名" }

### 6.8 egress 状态块（EgressConfig，GET/PUT/DELETE `/egress` 共用）

`GET /egress` 的顶层字段（除 `revision` / `configRevision`）与 `PUT` / `DELETE` 的响应体是同一块，字段名全部 lowerCamelCase，来源全部是 **TOML 键或运行时读回值**：

| 字段 | 类型 | 可空 | 来源 / TOML 键 | 说明 |
| --- | --- | --- | --- | --- |
| `configured` | boolean | 否 | 派生 | 至少有一条代理定义，或 `[egress].default` 非空，或任一 `[[apps]].egress` 非空 |
| `mode` | `"proxy"` \| `"direct"` | 否 | `[egress].default` | 设备级**期望**模式：`default` 非空即 `proxy` |
| `effectiveMode` | `"proxy"` \| `"direct"` | 否 | 派生 | 计入 fail-open 后的**实际**模式；`health.degraded && failOpen` 时为 `direct` |
| `default` | string | 是 | `[egress].default` | 设备级代理名；`null` 表示未配置 |
| `failOpen` | boolean | 否 | `[probe].fail_open` | 默认 `true` |
| `probeUrl` | string | 否 | `[probe].url` | 探测目标，默认 `http://checkip.amazonaws.com/` |
| `probeTimeoutSeconds` | integer | 否 | `[probe].timeout_seconds` | 1..=30，默认 5 |
| `exclusionList` | string[] | 否 | **无 TOML**（落 `settings global global_http_proxy_exclusion_list`） | 逗号连接的网段清单；空数组表示该键为空 |
| `upstream` | object\|null | 是 | 由 `default` 解析出的定义 | 脱敏摘要（§6.9）；`default` 为空或定义缺失时为 `null` |
| `proxies` | object[] | 否 | `[proxies.*]` | 全部定义（脱敏，§6.9），按文件书写顺序 |
| `apps` | object[] | 否 | `[[apps]]`（**只列显式写了 `egress` 的记录**） | `{package, egress, effective, proxy}`；`effective` ∈ `proxy`/`direct`，`proxy` 是生效代理名或 `null` |
| `applied` | boolean | 否 | 派生 | **只描述方案 A**：读回值与期望值是否一致 |
| `readback` | object | 否 | 运行时 | `{settingsValue, dumpsysGlobalProxy, matches, at}`；`settingsValue` 原文可能是 `null` / `:0` / `host:port` |
| `health` | object | 否 | 运行时 + 最近探测 | `{degraded, degradedSince, degradedReason, lastProbe}`；`degradedReason` ∈ `probe_failed`/`settings_mismatch`/`null` |
| `health`（范围） | — | — | — | **只描述方案 A**（设备级 `settings global http_proxy`）：探测失败或读回不一致才置 `degraded`。应用级注入的重放/selector 降级与此无关，也不会让它变 `degraded` |
| `egressFile` | object | 否 | `egress.toml` 元数据 | `{exists, size, mtime}`，**不含内容** |
| `warnings` | Warning[] | 否 | 运行时 | §6.6 的警告码 + §5.25 列出的 egress 警告 |

`health.lastProbe` 的结构与 §5.28 的探测响应同源：`{ok, exitIp, latencyMs, checkedAt, scope, via, proxy, upstreamStatus, errorCode}`，只保留**最近一次**（内存态，服务重启后为 `null`）。

### 6.9 EgressProxySummary（脱敏代理摘要）

`upstream`、`proxies[]`、`GET /egress/proxies` 的 `items[]` 都基于这一组字段，**固定 5 个键**：

    { "name": "office_http", "type": "http", "host": "198.51.100.10", "port": 8080, "hasCredentials": false }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `name` | string | 代理名（`^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`） |
| `type` | `"http"` \| `"socks5"` | 协议（TOML 键也是 `type`） |
| `host` | string | 主机名或 IPv4，不含 scheme |
| `port` | integer | 1..=65535 |
| `hasCredentials` | boolean | **唯一的凭据信号**：只要 `username` 或 `password` 有值即为 `true`；任何响应都不回凭据值。**它只表示存了凭据，不表示认证可用**——本期应用流量不发送认证（§5.25） |

`GET /egress/proxies` 的 `items[]` 在此基础上追加两个引用字段：`usedAsDefault`（boolean）与 `apps`（string[]）。

### 6.10 ProbeResult（出口探测结果）

`POST /egress/probe` 的响应体；`health.lastProbe` 复用同一组字段（去掉 `url` / `approximate` / `warnings`）：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `ok` | boolean | 否 | 成功时恒 `true`（失败走错误信封） |
| `exitIp` | string | 否 | 出口 IP 字面量（IPv4 或 IPv6） |
| `latencyMs` | integer | 否 | 从发请求到读完响应体的毫秒数 |
| `checkedAt` | string | 否 | 探测时刻（RFC 3339 UTC） |
| `scope` | `"device"` \| `"app"` | 否 | 请求里省略 `package` 时为 `device` |
| `via` | `"proxy"` \| `"direct"` | 否 | 本次实际走的链路 |
| `proxy` | string | 是 | 实际使用的代理名；`via:"direct"` 时为 `null` |
| `url` | string | 否 | 实际请求的 URL |
| `upstreamStatus` | integer | 是 | probe URL 返回的 HTTP 状态码；没拿到响应时为 `null` |
| `approximate` | boolean | 否 | `scope:"app"` 时恒 `true`（探测在服务进程内完成，见 §5.28） |
| `warnings` | Warning[] | 否 | 例如 `egress_probe_approximate` |

---

## 7. 错误信封与错误码

### 7.1 信封（所有 4xx/5xx 唯一格式）

    {
      "error": {
        "code": "config_revision_mismatch",
        "message": "配置已被其他客户端修改",
        "details": {
          "expected": "9f2c5d1e…",
          "actual": "1a7b2c3d…"
        },
        "retryable": false,
        "requestId": "req_01JC7Z6Q3M8V2S9K4T1B0A"
      }
    }

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `code` | string | 是 | 稳定错误码，见 §7.2 |
| `message` | string | 是 | 人类可读的短说明（可含中文） |
| `details` | object 或 null | 是（可为 null） | 结构化原因，供 AI 分支处理 |
| `retryable` | boolean | 是 | 同一请求原样重试是否可能成功 |
| `requestId` | string | 是 | 与 `X-Request-Id` 相同，用于查日志 |

### 7.2 错误码表

| code | HTTP | retryable | details 形状 | 含义 |
| --- | --- | --- | --- | --- |
| `unauthorized` | 401 | false | `null` | 缺少 / 格式错误 / 无效 Token |
| `forbidden_scope` | 403 | false | `{required, granted}` | Token 作用域不足 |
| `not_found` | 404 | false | `{resource, name}` | 资源不存在（含未知 `/api/v1` 路径） |
| `method_not_allowed` | 405 | false | `{method, path}` | 路径存在但方法不支持 |
| `confirmation_required` | 400 | false | `{action}` | 破坏性操作缺 `confirm=true` |
| `conflict_duplicate_package` | 409 | false | `{package, templates}` | 会写出重复包名，需要 `allow_duplicate_package=true` |
| `config_revision_mismatch` | 412 | false | `{expected, actual}` | `If-Match` 与当前 revision 不符 |
| `precondition_required` | 428 | false | `{header:"If-Match"}` | 写请求缺 `If-Match` |
| `validation_failed` | 422 | false | `{errors:[ValidationError]}` | 请求体 / 配置语义不合法 |
| `body_too_large` | 413 | false | `{limitBytes}` | 请求体超过 2 MiB |
| `unsupported_media_type` | 415 | false | `{supported:[…]}` | `Content-Type` 不支持 |
| `rate_limited` | 429 | true | `{limit, windowSec}` | P4 限流预留；本增量不会返回（见 §2.9） |
| `internal_error` | 500 | true | `null` | 未预期错误，用 `requestId` 查日志 |
| `upstream_unavailable` | 503 | true | `{component}` | `pm` / `am` / `getprop` / `settings` 调用失败或超时 |
| `egress.not_configured` | 409 | false | `{scope, package?}` | 没有任何可用出口，无法按当前链路探测（v1.10.0 新增） |
| `egress.probe_failed` | 502 | true | `{url, reason, timeoutMs, upstreamStatus?}` | 出口探测失败；`reason` ∈ `connect`/`dns`/`timeout`/`tls`/`http_status`/`invalid_response`（v1.10.0 新增） |

补充规则：

- 4xx 一律按此信封；错误响应 `Content-Type` 为 `application/json`。
- `retryable:true` 只表示「原样重试可能成功」，不代表业务成功；客户端应做指数退避（建议 250 ms 起、最多 3 次、尊重 `Retry-After`）。
- 412 不 retryable：必须重新读取 revision 并重建变更。
- 422 的 `details.errors` 至少一项，`path` 用 `templates.<name>.dpi`、`apps[0].package` 这样的点/方括号路径。

---

## 8. 校验规则

### 8.1 字段与格式

| 项 | 规则 |
| --- | --- |
| 模板名 | `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$` |
| 包名 | `^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+(@[0-9]{1,3})?$`（每段以字母开头；userId 0–999） |
| `dpi` | 120..=640 |
| `sdk_int` | 1..=1000，或字符串 `"__DELETE__"` |
| `android_version` | `^[0-9]{1,2}(\.[0-9]{1,2}){0,2}$` |
| `security_patch` | `^[0-9]{4}-[0-9]{2}-[0-9]{2}$` |
| `custom_props` 键 | `^(ro\|persist)\.[A-Za-z0-9_.-]{1,120}$` |
| `custom_props` 值 | ≤1024 字符；`"__EMPTY__"`、`"__DELETE__"` 为保留标记 |
| `hide_maps` / `default_hide_maps` | 每项 ≤256 字符，≤64 项 |
| 普通字符串字段 | ≤512 字符 |
| 单个模板 `packages` | ≤200 项，去重后保留首次出现顺序 |
| `apps` 记录 | ≤500 条，`package` 唯一 |
| egress 代理名 | `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`（1..=64 字符）；保留值 `__DIRECT__` 不能当代理名；≤64 条定义 |
| egress 目标 | `host` ≤253 字符且不含 scheme / 端口 / 路径；`port` 1..=65535；`username` ≤128 / `password` ≤256 |
| 请求体 | ≤2 MiB |

### 8.2 校验错误码（`ValidationError.code`）

| code | 含义 |
| --- | --- |
| `syntax_error` | TOML/JSON 语法错误（`path` 给行列或 JSON Pointer） |
| `invalid_type` | 类型不符 |
| `invalid_package` | 包名不合法 |
| `invalid_template_name` | 模板名不合法 |
| `out_of_range` | 数值越界（dpi、sdk_int） |
| `invalid_value` | 枚举 / 标记值 / 日期格式不合法 |
| `unknown_field` | API 请求里出现未知字段 |
| `missing_required` | 缺少必填字段（`apps[].package`） |
| `duplicate_app_record` | 同一包名出现多条内联记录 |
| `invalid_egress_name` | 代理名不合法（`^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$`，或用了保留值 `__DIRECT__`） |
| `invalid_egress_default` | `[egress].default` 写成了 `__DIRECT__`（设备级要直连请删掉该键） |

### 8.3 冲突判定

- 同一包名出现在 ≥2 个模板的 `packages` 中 = 冲突。
- 写操作默认拒绝（409）；`allow_duplicate_package=true` 时写入并返回 `duplicate_package` 警告。
- `GET /status` 的 `config.conflicts` 给出当前冲突包数；读取不改文件。

---

## 9. 并发、写入与生效

写路径（所有写端点共用）：

1. 取 `config.toml` 的排他 `flock`。
2. 读原始字节，算 revision，与 `If-Match` 比对；不符 → 412，**不做自动合并**。
3. 用 `toml_edit` 做定点修改：保留注释、空行、键顺序、未知字段与未知表。
4. 全量语义校验（§8）；失败 → 422，文件不动。
5. 自动备份到 `/data/adb/device_faker/config/backups/`（文件名 `<UTC 时间>-<revision 前 4 位>`，保留最近 10 份；本增量无备份管理端点）。
6. 写临时文件 → `fsync` → `rename` → `fsync` 目录 → `chmod 644`。
7. 释放锁，返回新 revision（+ 可选 diff + warnings）。
8. companion 在下一次 `GetSpec` 时发现 mtime 变化，重渲染 per-package spec。
9. **目标应用重启后生效**；需要立即验证时用 `POST /apps/{pkg}/restart`。

保证：

- 永不出现半截文件（rename 原子替换）。
- 并发两个客户端持同一 revision 写：一个 200，另一个 412，文件不损坏。
- 模板顺序（书写顺序）与 `[[apps]]` 顺序在定点修改后保持不变。
- 注释与未知字段在 `PUT /config` 的 **TOML 原文整替换**路径下会按你提交的内容重写（这是预期行为）；用结构化端点（templates/apps）时注释不丢。
- 管理器模式 WebUI 仍可直写文件，可能绕过锁；API 侧通过 revision 发现并在下次写入时报 412。

---

## 10. WebUI 契约

### 10.1 运行时注入（服务端职责）

```
<!doctype html>
<html>
  <head>
    <script>window.__DF_SERVER__ = { api: "/api/v1", version: "1", transport: "http" };</script>
    <script type="module" src="/assets/index-<hash>.js"></script>
  </head>
  <body><div id="app"></div></body>
</html>
```

- 注入发生在 `webroot/index.html` 返回前，**不包含 Token**。
- `index.html` 与 `/api/v1/*` 响应 `Cache-Control: no-store`；`/assets/*`（文件名带哈希）可长缓存。
- 服务端不得把 Token、路径以外的敏感配置写进注入脚本。

### 10.2 传输探测（前端职责）

1. `window.__DF_SERVER__` 存在 → HTTP 传输（不再探测 ksu）。
2. 否则 `window.ksu` 存在 → ksu 传输（管理器模式，行为零改动）。
3. 都没有 → 只读提示页，禁用写操作。

### 10.3 能力映射（原生模式）

| WebUI 能力 | ksu 实现 | HTTP 实现 |
| --- | --- | --- |
| 模块信息 | `ksu.moduleInfo` | `GET /api/v1/status` |
| 读原始配置 | `ksu.io` cat | `GET /api/v1/config?format=toml` |
| 写原始配置 | `ksu.io` heredoc | `PUT /api/v1/config`（`text/plain`，带 `If-Match`） |
| 模板 / 应用 CRUD | 直写 TOML | `/api/v1/templates*`、`/api/v1/apps*` |
| 已安装包列表 | `ksu.pm.listPackages` / `getPackagesInfo` | `GET /api/v1/packages`（`?names=` 等价） |
| getprop | `ksu.exec("getprop")` | `GET /api/v1/device/props`（需在白名单内） |
| 重启应用 | `ksu.exec("am force-stop")` | `POST /api/v1/apps/{pkg}/restart` |
| 配置 revision | 无 | `GET /api/v1/config/revision` |

### 10.4 原生模式下不支持的 UI

| 功能 | 原生模式处理 |
| --- | --- |
| 应用名 / 图标 | 显示包名与首字母占位，说明「应用名/图标将在后续版本提供」 |
| 深链打开 | 隐藏入口或提示仅管理器模式可用 |
| 在线模板拉取 | 由浏览器直接 `fetch`（同源/外部）或提示不可用；不新增 API |
| 导入导出 | 用 `GET /config?format=toml` + 浏览器下载；导入走 `PUT /config` |

### 10.5 错误处理（前端）

| 状态 | UI 行为 |
| --- | --- |
| 401 | 弹 Token 输入框，清空当前 Token |
| 403 | 提示「当前 Token 作用域不足」，展示 `details.required` |
| 412 | 提示「配置已被其他客户端修改」，自动刷新 revision 并让用户重试 |
| 409 | 展示冲突包名与模板，提供「仍然写入」按钮（`allow_duplicate_package=true`） |
| 422 | 把 `details.errors` 映射到表单字段 |
| 429 | 按 `Retry-After` 倒计时，禁用提交按钮 |
| 5xx | 显示 `requestId` 便于查日志 |

---

## 11. 静态资源（非 API 路径）

| 路径 | 行为 |
| --- | --- |
| `/` | 返回 `webroot/index.html` + 运行时注入 |
| `/assets/<file>` | 静态文件，带哈希文件名，长缓存 |
| `/<无扩展名路径>` | SPA 回退到 `index.html` |
| `/<缺失的带扩展名路径>` | 404 纯文本，不进 SPA 回退 |
| `/api/v1/*` | 只走 API；未匹配 → 404 错误信封；**禁止**回退 index.html |

- 目录穿越防护：规范化后的绝对路径必须仍在 `webroot` 内，否则 404。
- MIME 覆盖 `js / css / json / svg / png / webp / woff2 / wasm / ico / txt`。
- 编码：`gzip` / `br`（`tower-http compression`）；`index.html` 不压缩、每次重算 ETag。

---

## 12. 冒烟验收（curl）

    TOKEN=admin123          # 默认值；在管理器 WebUI 改过就换成你的
    BASE=http://127.0.0.1:8686/api/v1

    # 1) 探活（无认证）
    curl -s "$BASE/healthz" | jq .

    # 2) 状态 + revision
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/status" | jq '.config.revision'
    # 2b) 一次看设备指纹 + 每个已配置应用的最终值
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/overview" | jq '{device, apps}'

    # 3) 读模板（保序）
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/templates" | jq '.items[].name'

    # 4) 校验一段 TOML（不落盘）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"format":"toml","content":"[templates.demo]\npackages=[\"com.example.app\"]\ndpi=420\n"}' \
      "$BASE/config/validate" | jq .

    # 5) 建模板（If-Match 用上一步拿到的 revision）
    REV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/config/revision" | jq -r .revision)
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$REV\"" \
      -H "Content-Type: application/json" -d '{"packages":["com.example.app"],"brand":"google","model":"Pixel 9","dpi":420}' \
      "$BASE/templates/demo" | jq .

    # 6) 绑定包名并重启目标应用
    REV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/config/revision" | jq -r .revision)
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$REV\"" -H "Content-Type: application/json" \
      -d '{"packages":["com.example.app@0"],"mode":"append"}' "$BASE/templates/demo/packages" | jq .
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"forceStop":true,"launch":false}' "$BASE/apps/com.example.app/restart" | jq .

    # 7) 解释应用生效来源
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/apps/com.example.app" | jq '{source, resolved}'

预期：步骤 4 返回 `valid:true`；步骤 5 首次 201、重复同内容 200；步骤 7 的 `source.type="template"`。

出口代理（egress）冒烟：

    # 8) 读当前出口状态（拿 egress revision；注意响应头是 X-DF-Egress-Revision）
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" \
      | jq '{revision, configured, mode, effectiveMode, default, applied, readback, health}'

    # 9) 写代理定义 + 设备级出口（If-Match 用 egress revision）
    EREV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$EREV\"" \
      -H "Content-Type: application/json" \
      -d '{"default":"office_http","failOpen":true,"probeUrl":"http://checkip.example.com/","proxies":[{"name":"office_http","type":"http","host":"198.51.100.10","port":8080}]}' \
      "$BASE/egress" | jq '{changed, applied, readback, warnings}'

    # 10) 设备侧读回（需要 root）
    adb shell su -c 'settings get global http_proxy'                  # 期望 198.51.100.10:8080
    adb shell su -c 'dumpsys connectivity | grep -i "global proxy"'   # 期望 Global proxy: [198.51.100.10] port: 8080

    # 11) 探测出口 IP
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{}' "$BASE/egress/probe" | jq '{exitIp, latencyMs, checkedAt, via}'

    # 12) 应用级覆盖：只给单个应用指定 SOCKS5
    EREV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$EREV\"" \
      -H "Content-Type: application/json" \
      -d '{"proxies":[{"name":"lab_socks","type":"socks5","host":"198.51.100.20","port":1080,"username":"<USERNAME>","password":"<PASSWORD>"}],"apps":[{"package":"com.example.a","egress":"lab_socks"},{"package":"com.example.b","egress":"__DIRECT__"}]}' \
      "$BASE/egress" | jq '.apps, .warnings'

    # 13) 回滚直连（必须 confirm=true）
    EREV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)
    curl -s -X DELETE -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$EREV\"" \
      "$BASE/egress?confirm=true" | jq '{changed, mode, applied}'
    adb shell su -c 'dumpsys connectivity | grep -i "global proxy"'   # 期望 Global proxy: none

预期（egress）：步骤 9 的 `changed:true`、`applied:true`；步骤 11 `exitIp` 与代理出口一致；步骤 12 只影响 `com.example.a`（`com.example.b` 直连）；步骤 13 后 `mode:"direct"` 且设备侧显示 `Global proxy: none`。同一份内容重复 PUT（用新 revision）应返回 `changed:false`。

---

## 13. 变更记录

| 版本 | 日期 | 变更 |
| --- | --- | --- |
| v1 | 2026-11-24 | 第一增量契约冻结：23 个端点、错误码表、revision/If-Match 语义、WebUI 注入与降级规则 |
| v1.1 | 2026-11-25 | 新增 `GET /overview`（合计 24 个端点）；首次启动 Token 默认 `admin123`，可在管理器 WebUI 修改 |
| v1.2 | 2026-12-01 | egress 增量（v1.10.0）：新增 `/egress`、`/egress/probe`、`/egress/proxies` 共 5 个端点（合计 29）；新增 `X-DF-Egress-Revision`、egress revision 语义、`egress.not_configured` / `egress.probe_failed` 与两个校验码；`upstream_unavailable` 的 `component` 增加 `settings` |
