# 出口代理（Egress）契约与数据模型冻结

> 文档版本：**egress contract v1**（第一增量冻结）
> 状态：**冻结**。本文件是设备端 `devmask_server`、Zygisk 注入、Manager、双侧 WebUI 与验收脚本的唯一口径。
> 日期：2026-11-24
> 上游依据：[proxy-support-plan.md](proxy-support-plan.md)（需求与实测记录）、[../API.md](../API.md) §2.2 / §2.5 / §7 / §9、[../../manager/docs/API.md](../../manager/docs/API.md) §2.2 / §2.6 / §2.7 / §2.8 / §5.7 / §7、[../../manager/docs/ARCH.md](../../manager/docs/ARCH.md) §5 / §6
> 实现落点：`server/src/`（新增 `egress` 模块）、`core/src/`（配置模型）、`manager/src/`、`webui/src/`、`manager/webui/src/`
> 许可：GPL-3.0-or-later，`Copyright (c) 2025-2026 Seyud`

本文件与实现冲突时**按本文件判缺陷**。与 [proxy-support-plan.md](proxy-support-plan.md) 的差异集中在 **§8 与需求文档的差异**，其余逐条对齐。

---

## 阅读约定

### 术语（本项目内固定含义）

| 术语 | 含义 | 不指 |
| --- | --- | --- |
| **egress / 出口** | 设备上应用流量离开设备时走的网络出口 | 不是 Manager 访问设备用的出站代理 |
| **方案 A / 设备级** | 写 Android `settings global http_proxy`，整机 HTTP(S) 出口 | 不含 SOCKS5、不承载认证 |
| **方案 B / 应用级** | Zygisk 在目标进程内设置 Java 属性 + `ProxySelector`，按应用指定出口 | 不覆盖 native 栈 |
| **代理定义（proxy definition）** | 一条 `{name, type, host, port, username?, password?}` | 不是 Manager 的 `devices.baseUrl` |
| **代理名（egress name）** | 代理定义的键，`config.toml` 用它引用 | 不是包名、不是模板名 |
| **绑定（binding）** | Manager 侧 `device_egress` 里「设备 / 设备+应用 → 代理定义」的关系 | 不是设备端配置 |
| **下发（apply）** | Manager 把绑定与代理定义推送到设备（`PUT /api/v1/egress`） | 不是 `PUT /apps/{pkg}` |
| **探测（probe）** | 经当前生效链路请求 probe URL，拿出口 IP | 不是健康检查端点 |
| **fail-open** | 代理不可用时放行直连（默认开） | 不是「忽略错误继续返回成功」 |
| **`__DIRECT__`** | 应用级强制直连的保留标记 | 不是代理名，也不能写进设备级 `default` |

### 命名规则（沿用两侧既有三分法，只补一条 egress 规则）

| 位置 | 规则 | egress 示例 |
| --- | --- | --- |
| 线上 JSON 字段（设备端 `/api/v1` 与 Manager `/relay/api/v1`） | **lowerCamelCase** | `probeUrl`、`failOpen`、`exitIp`、`latencyMs`、`checkedAt`、`hasCredentials`、`configRevision`、`effectiveMode` |
| `egress.toml` / `config.toml` 的键 | **snake_case**，与文件同名同义 | `timeout_seconds`、`fail_open`、`default`、`egress` |
| 查询参数 | **snake_case** | `confirm`、`dry_run`、`diff`、`limit`、`cursor`、`probe` |
| 路径参数 | 原文，必要时百分号编码 | `egp_3f1c…`、`pixel-9`、`com.example.app` |
| 请求 / 响应头 | 标准 HTTP 大小写 | `If-Match`、`X-DM-Egress-Revision`、`X-Request-Id` |

**新增规则（egress 专属，只此一条）**：设备端 egress 领域的**错误码**用点分命名空间 `egress.*`（只有 `egress.not_configured`、`egress.probe_failed` 两个），其余错误码保持既有扁平码；**警告码**一律扁平（`egress_*`），与既有 8 个警告码同风格。两处不同写法是**刻意**的：错误码表要向需求文档对齐，警告码表要与既有表同构。

### 端点总览

设备端 `/api/v1`（本增量 **+5**，合计 29）：

| # | 方法 | 路径 | scope | 章节 |
| --- | --- | --- | --- | --- |
| 25 | GET | `/api/v1/egress` | `read` | §1.3 |
| 26 | PUT | `/api/v1/egress` | `write` | §1.4 |
| 27 | DELETE | `/api/v1/egress` | `write` | §1.5 |
| 28 | POST | `/api/v1/egress/probe` | `ops` | §1.6 |
| 29 | GET | `/api/v1/egress/proxies` | `read` | §1.7 |

Manager `/relay/api/v1`（本增量 **+9 个方法级操作 / +5 个路径**，合计 27）：

| # | 方法 | 路径 | 章节 |
| --- | --- | --- | --- |
| 19 | GET | `/egress-proxies` | §6.2 |
| 20 | POST | `/egress-proxies` | §6.3 |
| 21 | GET | `/egress-proxies/{id}` | §6.4 |
| 22 | PATCH | `/egress-proxies/{id}` | §6.5 |
| 23 | DELETE | `/egress-proxies/{id}` | §6.6 |
| 24 | POST | `/egress-proxies/{id}/probe` | §6.7 |
| 25 | GET | `/devices/{id}/egress` | §6.8 |
| 26 | PUT | `/devices/{id}/egress` | §6.9 |
| 27 | POST | `/devices/egress/apply` | §6.10 |

### 下游任务取用映射

| 任务 | 必读章节 |
| --- | --- |
| t2 `core` 配置模型 | §2.3、§2.4、§2.5、§2.6、§2.7 |
| t3 设备端 server | §1 全部、§2.1、§2.2、§3、§4、§7.2 |
| t4 Zygisk 注入 | §5 全部、§2.4、§1.4（`apps[]` 语义） |
| t5 Manager | §6 全部、§7.3、§7.4 |
| t6 / t7 双侧 WebUI | §1.3 响应字段、§1.8 错误码、§6.2/§6.4 响应字段、§7.1 |
| t8 文档收口 | 附录 B |
| t9 / t10 验收 | 附录 A |
| t11 / t12 评审 | §1.9、§3.5、§6.12、§7 |

---

## 1. 设备端 API：`/api/v1/egress*`

### 1.1 通用约定

- 认证与控制面约定沿用 [docs/API.md](../API.md) §3：`Authorization: Bearer <token>`；作用域按总览表；不足 → `403 forbidden_scope`。
- 所有响应带 `Cache-Control: no-store`、`X-Request-Id`。
- 4xx/5xx 一律使用统一错误信封（[docs/API.md](../API.md) §7.1），不新增信封字段。
- **响应头规则（重要，防跨端点污染）**：egress 端点**不设置** `X-DM-Revision`，也**不设置** `ETag`。egress revision 只出现在响应体的 `revision` 字段，并以附加头 `X-DM-Egress-Revision: <64 位小写十六进制>` 出现。
  - 理由：既有 WebUI 传输层会在**任何**响应上读取 `X-DM-Revision` / `ETag` 并覆盖本地缓存的 config revision（`webui/src/api/*` 的统一拦截器）。若 egress 端点复用这两个头，一次 `GET /egress` 就会把 config revision 覆盖成 egress revision，下一次 `PUT /config` 必然 412。属于必须避免的集成事故。
- 写端点（PUT/DELETE）的 `If-Match` 比较对象是 **egress revision**（§1.2），不是 config revision。

### 1.2 egress revision 的定义

egress 状态跨两个文件（`config.toml` 的 `[egress]` + `[[apps]].egress`，`egress.toml` 的代理定义与 probe 段），因此 revision 是两者的**合成值**：

    egressRevision = SHA-256( configRevision 的 64 位小写十六进制字符串的 UTF-8 字节
                              ‖ "\n"
                              ‖ egress.toml 的原始字节（文件不存在时为空字节） )
                     → 64 位小写十六进制

- 任一文件变化（含只改 `egress.toml` 的密码）都会改变 egress revision。
- 客户端把 `revision` 当作**不透明字符串**：原样保存、原样回传，不自行重算。
- 响应体的 `configRevision` 是 `config.toml` 自己的 revision（即 [docs/API.md](../API.md) §2.5 定义的全局 revision），仅用于排障与前端显示，**不能**用于 egress 端点的 `If-Match`。
- `If-Match` 的容错与 [docs/API.md](../API.md) §2.5 完全一致：接受带引号（`ETag` 风格）、不带引号、带 `sha256:` 前缀；`*` 表示无条件写。
- 缺失 `If-Match` → `428 precondition_required`（`details{header:"If-Match"}`）。
- 不匹配 → `412 config_revision_mismatch`，`details{expected, actual, resource:"egress"}`。**复用既有 412 码**，使 Manager 既有的「412 重试一次」逻辑无需分支；`resource` 字段是纯增量，客户端安全忽略。
- 修复通道：当 `GET /egress` 因 `egress.toml` 解析失败返回 500 时，客户端用 `If-Match: *` 重写即可修复（`*` 永远合法）。

### 1.3 `GET /api/v1/egress`

- 作用域：`read`。无 `If-Match`。
- 用途：一次拿到「当前模式 + 生效值 + 上游（脱敏）+ 应用级覆盖 + 最近探测」。

响应 `200`：

    {
      "revision": "5b1f0c9e8d7a6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c",
      "configRevision": "9f2c5d1e8a7b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5",
      "configured": true,
      "mode": "proxy",
      "effectiveMode": "proxy",
      "default": "us_la_01",
      "failOpen": true,
      "probeUrl": "http://checkip.amazonaws.com/",
      "probeTimeoutSeconds": 5,
      "exclusionList": ["10.0.0.0/8", "192.168.0.0/16"],
      "upstream": {
        "name": "us_la_01",
        "type": "http",
        "host": "203.0.113.10",
        "port": 1080,
        "hasCredentials": false
      },
      "proxies": [
        { "name": "us_la_01", "type": "http",   "host": "203.0.113.10", "port": 1080, "hasCredentials": false },
        { "name": "jp_tokyo_02", "type": "socks5", "host": "198.51.100.7", "port": 1080, "hasCredentials": true }
      ],
      "apps": [
        { "package": "com.example.a", "egress": "jp_tokyo_02", "effective": "proxy",  "proxy": "jp_tokyo_02" },
        { "package": "com.example.b", "egress": "__DIRECT__",  "effective": "direct", "proxy": null }
      ],
      "applied": true,
      "readback": {
        "settingsValue": "203.0.113.10:1080",
        "dumpsysGlobalProxy": "Global proxy: [203.0.113.10] port: 1080",
        "matches": true,
        "at": "2026-11-24T12:00:00Z"
      },
      "health": {
        "degraded": false,
        "degradedSince": null,
        "degradedReason": null,
        "lastProbe": {
          "ok": true,
          "exitIp": "203.0.113.7",
          "latencyMs": 412,
          "checkedAt": "2026-11-24T11:59:12Z",
          "scope": "device",
          "via": "proxy",
          "proxy": "us_la_01",
          "upstreamStatus": 200,
          "errorCode": null
        }
      },
      "egressFile": { "exists": true, "size": 412, "mtime": "2026-11-24T11:58:03Z" },
      "warnings": []
    }

字段表（含 TOML 对照）：

| 字段 | 类型 | 可空 | 来源 / TOML 键 | 说明 |
| --- | --- | --- | --- | --- |
| `revision` | string | 否 | 合成值（§1.2） | egress revision，写端点的 `If-Match` |
| `configRevision` | string | 否 | `config.toml` 字节的 sha256 | 全局 revision，仅排障/展示 |
| `configured` | boolean | 否 | 派生 | 至少存在一条代理定义，或 `[egress].default` 非空，或任一 `[[apps]].egress` 非空 |
| `mode` | `"proxy"` \| `"direct"` | 否 | `[egress].default` | 设备级**期望**模式：`default` 非空即 `proxy` |
| `effectiveMode` | `"proxy"` \| `"direct"` | 否 | 派生（§3.5） | 计入 fail-open 后的**实际**模式；`health.degraded && failOpen` 时为 `direct` |
| `default` | string | 是 | `[egress].default` | 设备级代理名；`null` 表示未配置 |
| `failOpen` | boolean | 否 | `[probe].fail_open` | 默认 `true` |
| `probeUrl` | string | 否 | `[probe].url` | 探测目标，默认 `http://checkip.amazonaws.com/` |
| `probeTimeoutSeconds` | integer | 否 | `[probe].timeout_seconds` | 默认 `5`，范围 1..=30 |
| `exclusionList` | string[] | 否 | **无 TOML**（落 `settings global global_http_proxy_exclusion_list`） | 逗号连接的网段白名单；≤32 项，每项 ≤64 字符；空数组表示不写该键 |
| `upstream` | object\|null | 是 | 由 `default` 解析出的 `[proxies.<name>]` | **脱敏**代理摘要；`default` 为空或定义缺失时为 `null` |
| `upstream.name` | string | 否 | `[proxies.<name>]` 表名 | 代理名 |
| `upstream.type` | `"http"` \| `"socks5"` | 否 | `type` | 协议 |
| `upstream.host` | string | 否 | `host` | 主机名或 IP，不含 scheme |
| `upstream.port` | integer | 否 | `port` | 1..65535 |
| `upstream.hasCredentials` | boolean | 否 | `username` / `password` 是否存在 | **唯一的凭据信号**，永不带值 |
| `proxies` | object[] | 否 | `[proxies.*]` | 全部定义（脱敏），按文件书写顺序 |
| `proxies[].name` | string | 否 | `[proxies.<name>]` | 同上 |
| `proxies[].type` | `"http"` \| `"socks5"` | 否 | `type` | 同上 |
| `proxies[].host` | string | 否 | `host` | 同上 |
| `proxies[].port` | integer | 否 | `port` | 同上 |
| `proxies[].hasCredentials` | boolean | 否 | `username` / `password` | 同上 |
| `apps` | object[] | 否 | `[[apps]]`（只列**显式**写了 `egress` 的记录） | 按 `[[apps]]` 数组顺序 |
| `apps[].package` | string | 否 | `[[apps]].package` | 包名（可带 `@userId`） |
| `apps[].egress` | string | 否 | `[[apps]].egress` | 代理名或 `"__DIRECT__"` |
| `apps[].effective` | `"proxy"` \| `"direct"` | 否 | 派生 | `__DIRECT__` → `direct`；代理名 → `proxy` |
| `apps[].proxy` | string\|null | 是 | 派生 | 生效代理名；直连或定义缺失时为 `null` |
| `applied` | boolean | 否 | 派生（§3.3） | **只描述方案 A**：读回值与期望值是否一致 |
| `readback` | object | 否 | 运行时 | 设备真实读回值（§3.3），`null` 字段表示命令输出为空/不可用 |
| `readback.settingsValue` | string\|null | 是 | `settings get global http_proxy` | 原文（`null` / `:0` / `host:port`） |
| `readback.dumpsysGlobalProxy` | string\|null | 是 | `dumpsys connectivity` | 命中行原文，用于交叉验证 |
| `readback.matches` | boolean | 否 | 派生 | 读回值是否等于期望值 |
| `readback.at` | string | 否 | 运行时 | 本次读回时间（RFC 3339 UTC） |
| `health` | object | 否 | 运行时 + 最近探测 | 见 §3.5 |
| `health.degraded` | boolean | 否 | 派生 | 已降级为直连或已判定链路不可用 |
| `health.degradedSince` | string\|null | 是 | 运行时 | 进入降级的时间 |
| `health.degradedReason` | `"probe_failed"` \| `"settings_mismatch"` \| `"settings_unavailable"` \| null | 是 | 运行时 | 降级原因枚举（`settings_unavailable` = 降级动作执行了但清直连没落地，此时 `effectiveMode` 报实际模式） |
| `health.lastProbe` | object\|null | 是 | 最近一次探测（仅内存，见 §4.6） | 结构见 §1.6 |
| `egressFile` | object | 否 | `egress.toml` 元数据 | `{exists, size, mtime}` |
| `warnings` | object[] | 否 | 运行时 | 见 §1.8 警告码 |

错误：`401`、`403`、`500 internal_error`（`details.op="egress_parse"` —— `egress.toml` 无法解析；此时用 `If-Match: *` 的 `PUT /egress` 修复）。

### 1.4 `PUT /api/v1/egress`

- 作用域：`write`。**`If-Match` 必填**（egress revision，§1.2）。
- 用途：写 `egress.toml` 的 `[proxies.*]` / `[probe]`，写 `config.toml` 的 `[egress].default` 与 `[[apps]].egress`，然后应用方案 A（§3）。
- 语义总则：**省略 = 不改；`null` = 清空；给了数组 = 整体替换该数组**（与 `PUT /config` 的整写风格一致；`apps: []` 与 `apps: null` 同样清空全部覆盖）。

请求体（全部字段可选）：

| 字段 | 类型 | 省略 | `null` | 写入位置 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `default` | string \| null | 不改 | 清除设备级默认 | `config.toml` `[egress].default` | 代理名；必须符合 §2.5 字符集；**不接受** `"__DIRECT__"`（422 `invalid_egress_default`） |
| `failOpen` | boolean | 不改 | —（不接受 null） | `egress.toml` `[probe].fail_open` | 默认 `true` |
| `probeUrl` | string \| null | 不改 | 恢复默认值 `http://checkip.amazonaws.com/` | `[probe].url` | 必须以 `http://` 开头（§4.3），≤512 字符 |
| `probeTimeoutSeconds` | integer | 不改 | — | `[probe].timeout_seconds` | 1..=30，默认 5 |
| `exclusionList` | string[] \| null | 不改 | 清除该键 | Android settings（不落 TOML） | ≤32 项，每项 ≤64 字符 |
| `proxies` | object[] | 不改（**已存的代理定义与密码原样保留**） | —（不接受 null） | `egress.toml` `[proxies.*]` | **整体替换**；`[]` 清空全部定义；≤64 条 |
| `proxies[].name` | string | 必填 | — | 表名 | §2.5 字符集；同请求内唯一 |
| `proxies[].type` | `"http"` \| `"socks5"` | 必填 | — | `type` | 枚举 |
| `proxies[].host` | string | 必填 | — | `host` | 主机名或 IP，≤253 字符，不含 scheme/端口/路径 |
| `proxies[].port` | integer | 必填 | — | `port` | 1..=65535 |
| `proxies[].username` | string \| null | 不改**该条已存用户名** | 清除 | `username` | ≤128 字符；空串等价 `null` |
| `proxies[].password` | string \| null | 不改**该条已存密码** | 清除 | `password` | ≤256 字符；空串等价 `null` |
| `apps` | object[] \| null | 不改任何应用覆盖 | 清除**所有** `[[apps]].egress`（`[]` 与 `null` 等价） | `config.toml` `[[apps]]` | **整体替换**：列出的应用写入/清除该键，**未列出的应用会删掉 `egress` 键**；只影响 `egress` 字段，不整记录覆盖 |
| `apps[].package` | string | 必填 | — | `[[apps]].package` | 包名规则同 [docs/API.md](../API.md) §8.1；同请求内唯一 |
| `apps[].egress` | string \| null | **必填**（缺 → 422 `missing_required`） | 清除该应用的覆盖 | `[[apps]].egress` | 代理名或 `"__DIRECT__"` |

查询参数：

| 参数 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `dry_run` | `true`/`false` | `false` | 完整校验 + 计算 diff，但不落盘、不应用 settings、不备份；响应 `dryRun:true`，`revision`/`applied` 保持当前值。diff 里的 `username` / `password` 值掩码为 `<redacted>` |
| `diff` | `true`/`false` | `false` | 成功写入时附 unified diff（两个文件合并展示，头部标注 `a/config.toml` / `a/egress.toml`）；上限 64 KiB，超出 `diffTruncated:true`。**`username` / `password` 行的值在返回前脱敏为 `<redacted>`**（键名保留，不会出现旧值或新密码） |
| `confirm` | `true`/`false` | 不需要 | 本端点不需要（只有 DELETE 需要） |

响应 `200`（`EgressWriteResult` = `WriteResult` 的超集，既有客户端可原地解析）：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `revision` | string | 否 | 写入后的 egress revision（`dryRun` 时为当前值） |
| `previousRevision` | string | 是 | 写入前 egress revision；文件均不存在时 `null` |
| `configRevision` | string | 否 | 写入后的 config revision |
| `dryRun` | boolean | 否 | 是否演练 |
| `changed` | boolean | 否 | 是否真的落盘（`false` = 命中幂等短路，见下） |
| `backupId` | string \| null | 是 | 自动备份 ID；`dryRun` 或 `changed:false` 时为 `null` |
| `mode` / `effectiveMode` / `default` / `applied` / `upstream` / `readback` / `health` | 同 §1.3 | — | 写入并应用后的真实状态 |
| `diff` / `diffTruncated` | string / boolean | 是 | 仅 `diff=true` 或 `dry_run=true` |
| `warnings` | object[] | 否 | §1.8 |

**幂等（冻结，勿自行解释）**：

1. 归一化后「期望状态」与「当前状态」完全一致 → **不写任何文件、不应用 settings、不备份、不动 mtime**，返回 `200` + `changed:false` + 原 `revision`。因此客户端可以带着**同一个** `If-Match` 重复提交同内容，永远得到 `200`（这是「重复 PUT 不重复写」的实现口径）。
2. 期望状态与当前不同 → 落盘并推进 revision。此后拿**旧** `If-Match` 再提交同一份内容 → `412`（符合 [docs/API.md](../API.md) §2.8：天然幂等的写端点第二次必然 412，客户端重新 GET 即可）。
3. `dry_run` 不改变任何状态，可无限次重复。

**写入顺序与原子性（冻结）**：

1. 先取 `config.toml` 锁，再取 `egress.toml` 锁（**固定顺序，禁止反向**，避免与其它写端点交叉死锁）。
2. 在临界区内完成：读两文件 → 校验 `If-Match` → 合并 → 全量语义校验 → 备份 → 原子写。
3. **先写 `egress.toml`，再写 `config.toml`**。中断只会留下「未被引用的代理定义」（无害，`GET /egress` 可见、`configured` 可能为真但不生效）；反向顺序会留下「引用了不存在的代理」的坏状态。
4. 两份文件都是「临时文件 → fsync → rename → fsync 目录」；`egress.toml` 权限见 §2.2。
5. 应用方案 A（写 `settings`）在**两个文件都落盘之后**执行；应用失败不回滚文件（配置是期望态，设备是实际态，两者差异由 `readback`/`applied` 暴露），保证「配置文件可修复、可重试」。

**不自动探测**：`PUT /egress` 不会替调用方探测出口（探测是独立 `ops` 动作，避免写路径产生额外网络副作用）。需要「下发 + 校验」的调用方（Manager job）显式调 `POST /egress/probe`。

错误：`400`（无）、`401`、`403`、`412`、`413 body_too_large`、`415 unsupported_media_type`、`422 validation_failed`、`428 precondition_required`、`500`、`503 upstream_unavailable`（`details.component="settings"`，文件已写入但方案 A 应用失败）。

### 1.5 `DELETE /api/v1/egress`

- 作用域：`write`。**`If-Match` 必填**；**`confirm=true` 必填**（缺失 → `400 confirmation_required`，`details{action:"egress_clear"}`）。
- 用途：一键恢复直连（U5「回滚」）。
- 清除清单（全部执行，缺一不可）：
  1. 删除 `[proxies.*]` 全部定义；
  2. 删除 `[egress].default`（表本身可保留为空表或删除，二者等价）；
  3. 清除**所有** `[[apps]]` 记录的 `egress` 字段（不动其它机型字段）；
  4. `[probe]` 段保留（`fail_open` / `url` / `timeout_seconds` 是策略而非出口，保留可减少下一次配置成本）；
  5. 执行 `settings put global http_proxy :0`（**必须用 `:0`，见 §3.2**）；
  6. 清除 `global_http_proxy_exclusion_list`（写空串）；
  7. 清空内存中的 `health`（`degraded:false`、`lastProbe:null`）。
- 幂等：已是「无定义 + 无 default + 无应用覆盖 + settings 已直连」→ `changed:false`，不写文件、不执行 settings，`200`。
- 响应 `200`：`EgressWriteResult`（同 §1.4，额外 `changed` 说明是否真的清了东西）。

请求 / 响应字段表：

| 项 | 位置 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- | --- |
| `If-Match` | 请求头 | string | **是** | egress revision（§1.2）；`*` 表示无条件写 |
| `confirm` | 查询参数 | `true` | **是** | 缺失 → `400 confirmation_required`（`details{action:"egress_clear"}`） |
| `revision` | 响应体 | string | — | 清除后的 egress revision |
| `previousRevision` | 响应体 | string 或 null | — | 清除前的 egress revision |
| `configRevision` | 响应体 | string | — | 清除后的 config revision |
| `dryRun` | 响应体 | boolean | — | 恒 `false`（本端点不支持 `dry_run`） |
| `changed` | 响应体 | boolean | — | 是否真的清掉了东西 |
| `backupId` | 响应体 | string 或 null | — | 有落盘时给备份 ID；`changed:false` 时为 `null` |
| `mode` / `effectiveMode` / `default` / `applied` / `upstream` / `readback` / `health` | 响应体 | 同 §1.3 | — | 清除后的真实状态：必然 `mode:"direct"`、`default:null`、`upstream:null`、`applied:true` |
| `warnings` | 响应体 | object[] | — | §1.8 |

- 错误：`400`、`401`、`403`、`412`、`428`、`500`、`503 upstream_unavailable`（`settings` 不可用）。

### 1.6 `POST /api/v1/egress/probe`

- 作用域：`ops`。无 `If-Match`；不支持 `Idempotency-Key`（探测每次都要真发请求，且可能触发 fail-open，非幂等）。
- 用途：经**当前生效链路**请求 probe URL，返回出口 IP。

请求体（可省略，全字段可选）：

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `package` | string \| null | `null` | `null` = 按设备级链路（方案 A）；给定包名 = 按该应用的**生效代理定义**发请求（用于验证 U1 的配置是否指对） |
| `url` | string \| null | `[probe].url` | 临时覆盖探测 URL；规则同 §4.3 |

响应 `200`：

    {
      "ok": true,
      "exitIp": "203.0.113.7",
      "latencyMs": 412,
      "checkedAt": "2026-11-24T12:00:00Z",
      "scope": "device",
      "via": "proxy",
      "proxy": "us_la_01",
      "url": "http://checkip.amazonaws.com/",
      "upstreamStatus": 200,
      "approximate": false,
      "warnings": []
    }

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `ok` | boolean | 否 | 成功时恒 `true`（失败走错误响应，见下） |
| `exitIp` | string | 否 | 出口 IP 字面量（IPv4 或 IPv6） |
| `latencyMs` | integer | 否 | 从发请求到收完响应体的毫秒数 |
| `checkedAt` | string | 否 | 探测时刻（RFC 3339 UTC） |
| `scope` | `"device"` \| `"app"` | 否 | 请求的 `package` 省略时为 `device` |
| `via` | `"proxy"` \| `"direct"` | 否 | 本次实际走的链路；`direct` 表示无代理（设备级未配置或该应用为直连） |
| `proxy` | string \| null | 是 | 实际使用的代理名；`via:"direct"` 时为 `null` |
| `url` | string | 否 | 实际请求的 URL |
| `upstreamStatus` | integer \| null | 是 | probe URL 返回的 HTTP 状态码；连接失败前未拿到响应时为 `null` |
| `approximate` | boolean | 否 | `scope:"app"` 时恒 `true`：探测在**服务进程内**用该应用的代理定义发请求，不代表目标应用进程内的真实链路（注入发生在 Zygisk 进程内，无 IPC 回传）。此时`warnings` 必带 `egress_probe_approximate` |
| `warnings` | object[] | 否 | §1.8 |

失败（**不在 200 里塞 `ok:false`**，统一走错误信封）：

| 情形 | HTTP | code | details |
| --- | --- | --- | --- |
| 未配置任何可用出口（设备级无 `default` 且无代理定义；或指定 `package` 但该应用既无覆盖也无设备级默认） | `409` | `egress.not_configured` | `{scope:"device"\|"app", package?}` |
| 探测失败（连接/DNS/超时/TLS/HTTP 非 2xx/响应不是 IP） | `502` | `egress.probe_failed` | `{url, reason, timeoutMs, upstreamStatus?, scope?, warnings?}`（`scope:"app"` 时附 `warnings:[{code:"egress_probe_scope_app"}]`） |
| `settings` 不可用导致无法构造设备级链路 | `503` | `upstream_unavailable` | `{component:"settings"}` |
| `package` 不是合法包名 / `url` 非法 | `422` | `validation_failed` | `{errors:[…]}` |

`reason` 枚举（`egress.probe_failed` 的 `details.reason`，**只增不改**）：`connect` | `dns` | `timeout` | `tls` | `http_status` | `invalid_response`。

**应用级失败不降级**：`scope:"app"` 的探测失败只写 `health.lastProbe`（`details.scope="app"`、`details.warnings` 里给 `egress_probe_scope_app`），**不会**修改设备级 `settings` 代理；只有 `scope:"device"` 的失败才按 §3.5 触发 fail-open。

**fail-open 与探测的关系（冻结）**：探测本身**不回落**。失败就是失败，`via` 不会因为 `failOpen=true` 而变成 `direct`。fail-open 只影响**流量链路**（§3.5），且触发条件见 §3.5。

### 1.7 `GET /api/v1/egress/proxies`

- 作用域：`read`。无 `If-Match`。
- 用途：给 WebUI 的代理下拉框、给「删定义前看看谁在引用」提供**脱敏**定义列表。

响应 `200`：

    {
      "revision": "5b1f0c9e…",
      "configRevision": "9f2c5d1e…",
      "total": 2,
      "egressFile": { "exists": true, "size": 412, "mtime": "2026-11-24T11:58:03Z" },
      "items": [
        {
          "name": "us_la_01",
          "type": "http",
          "host": "203.0.113.10",
          "port": 1080,
          "hasCredentials": false,
          "usedAsDefault": true,
          "apps": []
        },
        {
          "name": "jp_tokyo_02",
          "type": "socks5",
          "host": "198.51.100.7",
          "port": 1080,
          "hasCredentials": true,
          "usedAsDefault": false,
          "apps": ["com.example.a"]
        }
      ],
      "warnings": []
    }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `items[].name` / `type` / `host` / `port` / `hasCredentials` | — | 与 §1.3 `proxies[]` 同义，**不含凭据值** |
| `items[].usedAsDefault` | boolean | 是否为 `[egress].default` 指向的定义 |
| `items[].apps` | string[] | 引用该定义的包名（`[[apps]].egress` 命中项，按数组顺序） |
| `total` | integer | `items.length`（不分页；定义数上限 64） |

错误：`401`、`403`、`500`（`egress_parse`）。

### 1.8 错误码与警告码

**新增错误码**（两张表用词不同是刻意的，见「阅读约定」）：

| code | HTTP | retryable | details 形状 | 含义 |
| --- | --- | --- | --- | --- |
| `egress.not_configured` | 409 | false | `{scope, package?}` | 没有任何可用出口，无法按当前链路探测 |
| `egress.probe_failed` | 502 | true | `{url, reason, timeoutMs, upstreamStatus?}` | 探测请求失败或响应不可解析 |

**复用既有错误码**（不新增、不改语义）：`unauthorized`、`forbidden_scope`、`precondition_required`、`config_revision_mismatch`（`details` 追加 `resource:"egress"`）、`confirmation_required`、`validation_failed`、`body_too_large`、`unsupported_media_type`、`upstream_unavailable`（`details.component="settings"`）、`internal_error`。

> `upstream_unavailable` 只用于 `settings` 命令失败/不可用：响应 `503` + `details.component="settings"`，服务端另记一行 `settings 命令执行失败`（`code` 为退出码、`stderr` 截断 ≤256 字符、不含凭据）；二进制缺失（`Unavailable`）走同一 503。**没有**名为 `settings_api_error` 的对公错误码，那只是实现里的函数名。

**新增 ValidationError code**（`422` 的 `details.errors[].code`）：

| code | 触发 | `path` 示例 |
| --- | --- | --- |
| `invalid_egress_name` | 代理名不符合 §2.5 字符集/长度，或等于保留值 `__DIRECT__` | `proxies[0].name`、`apps[1].egress`、`default` |
| `invalid_egress_proxy` | `type` 非枚举、`host` 含 scheme/端口/路径或超长、`port` 越界、`username`/`password` 超长 | `proxies[0].host` |
| `invalid_egress_default` | `default` 写成 `"__DIRECT__"`（设备级请清空该键） | `default` |
| `invalid_probe_url` | 不以 `http://` 开头、超 512 字符、含用户信息（`user@`） | `probeUrl` |

**新增警告码**（扁平，出现在 `GET/PUT/DELETE /egress` 的 `warnings`）：

| code | details | 触发 |
| --- | --- | --- |
| `egress_proxy_not_found` | `{name, scope:"device"\|"app", package?}` | 引用的代理名在 `egress.toml` 未定义（**允许写入**，跨文件解耦；运行期按 §3.5 处理） |
| `egress_credentials_ignored` | `{name}` | 设备级指向的代理带凭据：方案 A 不承载认证，只写 `host:port`（§3.7） |
| `egress_inline_shadows_template` | `{package, template}` | 写应用级 `egress` 会为该包建立内联记录，整记录覆盖模板指纹（提示，不阻断） |
| `egress_app_not_installed` | `{package}` | 应用级覆盖的包未安装（不阻断） |
| `egress_degraded_to_direct` | `{reason}` | 本次请求触发了 fail-open 切直连（§3.5） |
| `egress_settings_mismatch` | `{expected, actual}` | 写 `settings` 后读回与期望不一致（`applied:false`） |
| `egress_perm_fixed` | `{path, mode}` | `egress.toml` 权限被自愈为 0600（§2.2） |
| `egress_probe_approximate` | `{package}` | 应用级探测在服务进程内完成（§1.6） |
| `egress_auth_unsupported` | `{scope:"device"\|"app", proxy, field:"username"\|"password"}` | 生效代理带凭据但注入不支持认证（libcore 不读 `java.net.socks.*`、HTTP 认证未实现）；**同一代理只报一次**（device 优先、否则按 `[[apps]]` 顺序），`field` 优先 username；写操作照常 2xx，**不阻断** |
| `egress_probe_scope_app` | `{scope:"app", package?}` | 出现在 `502 egress.probe_failed` 的 `details.warnings[]`：说明本次失败是**应用级**探测、设备级系统代理保持不变（§1.6） |


> 本表只覆盖**设备端**。Manager 侧的码不在本节：结果码（`decrypt_failed` / `device_token_invalid` / `egress_unchanged` /
> `egress_unsupported` / `egress_not_configured`）、警告码（含 `egress_check_not_recorded`）与错误码见 **§6.12**。

### 1.9 幂等矩阵（评审与测试的对照表）

| 端点 | 幂等？ | 重复调用（同 `If-Match`、同请求体） | 说明 |
| --- | --- | --- | --- |
| `GET /egress` | 是 | 恒 `200`，同内容 | 只读；但 `readback`/`health` 随时间变化 |
| `GET /egress/proxies` | 是 | 恒 `200`，同内容 | 只读 |
| `PUT /egress` | 条件幂等 | 首次 `200 changed:true`；用**新** revision 重放同内容 → `200 changed:false`；用**旧** revision → `412` | §1.4 幂等三条 |
| `PUT /egress`（`dry_run=true`） | 是 | 恒 `200`，不改状态 | 可无限重放 |
| `DELETE /egress` | 条件幂等 | 首次 `200 changed:true`；已清空后再删（新 revision）→ `200 changed:false`；旧 revision → `412` | 需 `confirm=true` |
| `POST /egress/probe` | **否** | 每次都真发请求；可能触发 fail-open | 不支持 `Idempotency-Key` |

---

## 2. 配置文件：`egress.toml` 与 `config.toml [egress]`

### 2.1 `egress.toml` 完整结构

路径：`/data/adb/devmask/config/egress.toml`（与 `config.toml` 同目录；目录权限 0700 root）。

    # egress.toml —— 代理定义与探测策略。权限 0600，进不了 API 明文、进不了导出。
    # 本文件由 devmask_server 维护；手工编辑后重启服务生效。
    version = 1

    [proxies.us_la_01]
    type     = "http"           # http | socks5
    host     = "203.0.113.10"
    port     = 1080
    username = "user"           # 可选；有值即 hasCredentials=true
    password = "<PASSWORD>"     # 可选；永不出现在任何响应 / 日志 / 审计 / WebUI

    [proxies.jp_tokyo_02]
    type     = "socks5"
    host     = "198.51.100.7"
    port     = 1080

    [probe]
    url             = "http://checkip.amazonaws.com/"
    timeout_seconds = 5
    fail_open       = true

| 键 | 类型 | 必填 | 默认 | 约束 |
| --- | --- | --- | --- | --- |
| `version` | integer | 否 | `1` | 仅接受 `1`；其它值 → 500 `internal_error` `details.op="egress_parse"` |
| `[proxies]` | table | 否 | 空 | ≤64 张子表；表名即代理名（§2.5）；顺序即书写顺序（响应保序） |
| `proxies.<name>.type` | string | **是** | — | `"http"` \| `"socks5"` |
| `proxies.<name>.host` | string | **是** | — | ≤253 字符；主机名或 IP 字面量；**不许**含 `://`、`/`、`@`、`:` |
| `proxies.<name>.port` | integer | **是** | — | 1..=65535 |
| `proxies.<name>.username` | string | 否 | 无 | ≤128 字符；`""` 视为未设置 |
| `proxies.<name>.password` | string | 否 | 无 | ≤256 字符；`""` 视为未设置 |
| `[probe].url` | string | 否 | `http://checkip.amazonaws.com/` | 必须以 `http://` 开头（§4.3）；≤512 字符；不含 `user:pass@` |
| `[probe].timeout_seconds` | integer | 否 | `5` | 1..=30 |
| `[probe].fail_open` | boolean | 否 | `true` | §3.5 |

未知键（例如 `[proxies.x] proxy_host = …`）：**忽略但保留**（与 [docs/API.md](../API.md) §6.1 的 `unknown_field_preserved` 精神一致），并在 `GET /egress` 的 `warnings` 里给 `unknown_field_preserved`（复用既有码，`details{fields:[]}`）。语法错误或类型错误 → 500（§1.3 末）。

### 2.2 权限与原子写（硬约束）

| 项 | 值 | 说明 |
| --- | --- | --- |
| 目录 | `/data/adb/devmask/config`：`0700 root` | 已存在；本增量不改 |
| 文件 | `egress.toml`：**`0600 root`** | 与 `config.toml` 的 0644 **刻意不同**（§7.2） |
| 创建 | 先以 `0600` 创建临时文件（`O_CREAT\|O_EXCL, 0o600`），再写入 | 避免「先 0644 后 chmod」的窗口 |
| 落盘 | `write → fsync(file) → rename → fsync(dir)` | 与 `config.toml` 写路径同构；永不出现半截文件 |
| 读时自愈 | 若 `mode & 0o077 != 0` → `chmod 0600` + 日志 warn + 响应 warning `egress_perm_fixed` | 参照 `devmask-manager.key` 的自愈做法 |
| 备份 | `/data/adb/devmask/config/backups/egress.toml.<YYYYMMDDTHHMMSS.nnnnnnnnnZ>-<revision 前 8 位>.bak`（例 `egress.toml.20261201T030000.987654321Z-5b1f2c3d.bak`），权限 `0600`，保留最近 10 份；响应里的 `backupId` 就是中间那段 `<stamp>-<rev8>` | 备份里也有密码，因此同样 0600 |
| 导出 | **不进** `GET /config?format=toml`、**不进** WebUI 导出、**不进**任何 API 明文 | §7.2 |
| 卸载 | `uninstall.sh` 在停服后执行 `settings put global http_proxy :0`，再 `rm -rf /data/adb/devmask` | 见 §3.2 |
| 模块禁用 | 只在服务运行时才能清 settings；因此**禁用前必须先在 WebUI/API 执行 `DELETE /egress`**（文档写入 CONFIG.md，t8） | 已知限制 |

### 2.3 `config.toml` 的 `[egress]` 段

`config.toml` 仍是 0644、可分享、可导出，因此**只放引用，不放凭据**。

    [egress]
    default = "us_la_01"        # 设备级默认（方案 A）；省略或空串 = 直连

| 键 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `[egress]` | table | 否 | 只能有 `default` 一个键 |
| `[egress].default` | string | 否 | 代理名，§2.5 字符集；**不接受** `"__DIRECT__"`（请删除该键） |

硬约束：

1. `[egress]` 里出现 `host` / `port` / `username` / `password` / `type` 等任何其它键 → **`422 unknown_field`**（守住「0644 文件里不许有凭据」这条线）。
2. `default` 指向的代理名在 `egress.toml` 未定义 → **允许写入**（跨文件解耦，配置可以先于定义存在），运行期按 §3.5 处理，并在 `GET /egress` 的 `warnings` 给 `egress_proxy_not_found`（`scope:"device"`）。
3. 删除 `[egress]` 整段等价于 `default = null`（直连），不视为错误。

### 2.4 `AppConfig.egress` 语义（应用级，方案 B）

```toml
[[apps]]
package = "com.example.a"
egress  = "jp_tokyo_02"     # 应用级覆盖
# …其余机型字段照旧

[[apps]]
package = "com.example.b"
egress  = "__DIRECT__"      # 强制直连，不受设备级影响
```

| `[[apps]].egress` 取值 | 含义 | 生效链路 |
| --- | --- | --- |
| 键**不存在**（`None`） | 未配置 | **继承** `[egress].default`；设备级也没配 → 直连 |
| `"__DIRECT__"` | 强制直连 | 直连，**并显式覆盖设备级系统代理**（§5.2） |
| 合法代理名 | 用该代理 | `egress.toml` 有定义 → 该代理；无定义 → 视为「未配置」，按 fail-open/fail-closed 处理（§3.5） |
| 非法名（字符集/长度不符） | 非法 | 写入即 `422` `invalid_egress_name` |
| 空串 `""` | 等价于「未配置」 | 与 [docs/CONFIG.md](../CONFIG.md) 的 `"__EMPTY__"` 语义无关，请不要用它表达删除；要删除就删键 |

继承链（唯一口径）：`[[apps]].egress` → `[egress].default` → 直连。**没有第三层**（模板不参与，见 §8 第 9 条）。

`DeviceTemplate` **不支持** `egress` 字段：模板里出现 `egress` → `422 unknown_field`（理由：模板被多个包共享，语义上不是「每应用出口」；且 `TemplateInput` 是机型字段的封闭集合）。

### 2.5 代理名合法字符集与长度上限

代理名同时是 TOML 表名、API 字段值和 Manager 的唯一键，因此**一套规则三处复用**：

    ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$

- 长度 1..=64 字符（正则已含）。
- 首字符必须是字母、数字或下划线；其后允许 `.` 与 `-`。
- **区分大小写**（`US_LA_01` 与 `us_la_01` 是两条定义）。
- **保留值**：`__DIRECT__`（全大写）不得作为代理名 → `422 invalid_egress_name`。
- 与模板名规则（[docs/API.md](../API.md) §8.1）完全相同，便于实现复用同一个校验函数。
- Manager 的 `egress_proxies.name` 用**同一条正则**：因为下发时设备端会用它当表名（1:1，不做改名）。

### 2.6 校验清单（实现与测试共用）

| 项 | 规则 | 违反时 |
| --- | --- | --- |
| 代理名 | §2.5 | `422 invalid_egress_name` |
| 代理条数 | ≤64 | `422 out_of_range`（`path="proxies"`） |
| `type` | `http` / `socks5` | `422 invalid_egress_proxy` |
| `host` | ≤253，无 scheme/路径/端口/用户信息 | `422 invalid_egress_proxy` |
| `port` | 1..=65535 | `422 invalid_egress_proxy` |
| `username` / `password` | ≤128 / ≤256 | `422 invalid_egress_proxy` |
| `probeUrl` | `http://` 前缀、≤512、无用户信息 | `422 invalid_probe_url` |
| `probeTimeoutSeconds` | 1..=30 | `422 out_of_range` |
| `exclusionList` | ≤32 项，每项 ≤64，形如 `a.b.c.d/len` 或 `host` | `422 invalid_value` |
| `default` | §2.5 且 ≠ `__DIRECT__` | `422 invalid_egress_name` / `invalid_egress_default` |
| `apps[].package` | 既有包名规则 | `422 invalid_package` |
| `apps[]` 重复 | 同请求内 package 唯一 | `422 invalid_value` |
| 未知字段 | 请求体出现未定义字段 | `422 unknown_field` |

### 2.7 与 JSON Schema / 增量编辑的关系

- `GET /schema/config` 必须新增两个属性：`egress`（object，`additionalProperties: false`，仅 `default`）与 `AppConfig.egress`（`{"type":["string","null"]}` + `pattern`）。draft-07 方言不变。
- 结构化写端点必须用 `toml_edit` 定点修改（保留注释、键顺序、未知表）：`[egress]` 表的插入/删除、`[[apps]]` 记录的 `egress` 键插入/删除，**不得**重排其它键。
- `PUT /config`（TOML 原文整替换）路径同样接受 `[egress]` 与 `apps[].egress`，但被替换掉的部分不会保留注释（既有行为）。

---

## 3. 设备级应用语义（方案 A）

### 3.1 写入与生效

| 项 | 值 |
| --- | --- |
| 目标值 | `<host>:<port>`（取 `[egress].default` 指向的定义；host 原样，不做 DNS 解析） |
| 写入命令 | `settings put global http_proxy <host>:<port>` |
| 可选项 | `settings put global global_http_proxy_exclusion_list "<a,b,c>"`（`exclusionList` 非空时） |
| 生效范围 | 尊重系统代理的应用（OkHttp / HttpURLConnection / WebView / 多数 SDK 默认链路）；**仅 HTTP/HTTPS CONNECT，不支持 SOCKS5** |
| 生效时机 | 新建立的连接；已建立的连接需应用重连（§3.7） |
| 执行身份 | 服务进程 uid 0 / 域 `u:r:ksu:s0`，与 `devmask_server` 同域同 uid，可直接 exec `settings`（Pixel 9 已实测） |
| 命令通道 | 必须走既有 `server/src/commands.rs` 的命令抽象（新增 `settings` / `dumpsys` 两个程序的语义化封装），**不新增任意 exec 端点** |

### 3.2 清除必须用 `:0`（`settings delete` 无效）

这是本增量最容易踩的坑，冻结为硬规则：

| 动作 | 命令 | 结果（Pixel 9 实测） |
| --- | --- | --- |
| 设置 | `settings put global http_proxy 203.0.113.10:1080` | `dumpsys connectivity` 立即显示 `Global proxy: [203.0.113.10] port: 1080` |
| 「删除」 | `settings delete global http_proxy` | **不清除**：`global_http_proxy_host` / `global_http_proxy_port` 仍在，代理继续生效 |
| 正确清除 | `settings put global http_proxy :0` | `dumpsys connectivity` 显示 `Global proxy: none` |

实测依据（设备与结论来自 [proxy-support-plan.md](proxy-support-plan.md) §5 的真机记录）：

| 项 | 记录 |
| --- | --- |
| 设备 | Pixel 9，序列号 `4A190DLAQ002QR` |
| 系统 | Android 17 / SDK 37，KernelSU |
| root | `su -c id` → `uid=0`，域 `u:r:ksu:s0`（与服务同域同 uid） |
| `settings put global http_proxy host:port` | ✅ 立即生效，`Global proxy: [host] port` |
| `settings delete global http_proxy` | ⚠️ **不清除**，代理继续生效 |
| `settings put global http_proxy :0` | ✅ 恢复 `Global proxy: none` |
| 相关键 | `http_proxy`、`global_http_proxy_host`、`global_http_proxy_port`、`global_http_proxy_exclusion_list`、`global_proxy_pac_url` |

复现命令（t9 必须重跑并贴输出）：

    adb shell su -c 'settings put global http_proxy 203.0.113.10:1080'
    adb shell su -c 'settings get global http_proxy'                                  # 203.0.113.10:1080
    adb shell su -c 'dumpsys connectivity | grep -i "global proxy"'                   # Global proxy: [203.0.113.10] port: 1080
    adb shell su -c 'settings delete global http_proxy'
    adb shell su -c 'settings get global global_http_proxy_host'                      # 仍有值 → 这就是坑
    adb shell su -c 'settings put global http_proxy :0'
    adb shell su -c 'dumpsys connectivity | grep -i "global proxy"'                   # Global proxy: none

实现侧硬约束：

1. 「清除」**只能**走 `settings put global http_proxy :0`；代码里**禁止**出现 `settings delete global http_proxy`。
2. 清除时同时把 `global_http_proxy_exclusion_list` 写成空串（否则白名单残留，下一次设置会意外继承）。
3. `DELETE /egress`、fail-open 切直连、卸载脚本、启动自愈四条路径都必须走同一个「清直连」函数（单一实现，可被单测覆盖）。

### 3.3 读回校验

写后有且只有一种判定方式：**读回 + 对比**（FR-1「防写了没生效」）。

| 读回 | 命令 | 说明 |
| --- | --- | --- |
| 主 | `settings get global http_proxy` | 输出 `host:port` / `:0` / `null` |
| 交叉验证 | `dumpsys connectivity` 中 `Global proxy:` 行 | `Global proxy: none` / `Global proxy: [host] port: N` |
| 诊断 | `settings get global global_http_proxy_host` / `…_port` | 出现「`http_proxy` 是 null 但这两个有值」即为脏状态 |

判定真值表（冻结）：

| `settings get global http_proxy` | `dumpsys` 行 | 期望=直连时 | 期望=`h:p` 时 |
| --- | --- | --- | --- |
| `null` | `Global proxy: none` | `applied=true` | `applied=false` → 补写 |
| `:0` | `Global proxy: none` | `applied=true`（`:0` 与 `null` **等价**） | `applied=false` → 补写 |
| `h:p` | `Global proxy: [h] port: p` | `applied=false` → 清直连 | `applied=true` |
| `null`（但 `global_http_proxy_host` 非空） | `Global proxy: none` | 脏状态 → 先写 `:0` 再写期望 | 脏状态 → 直接写期望 |

- `applied` 严格按上表计算，不猜测。
- 写后读回不一致 → 响应 `applied:false` + warning `egress_settings_mismatch`(`{expected, actual}`) + `health.degradedReason="settings_mismatch"`。
- `readback.settingsValue` 保留**原文**（含 `null` 字符串），便于排障；`readback.matches` 是布尔结论。

### 3.4 幂等与启动自愈

1. **写前先读**：当前读回值已等于期望值 → 不执行 `settings put`（FR-1「值相同不重复写」），仅刷新 `readback.at`。
2. **启动自愈**（FR-9）：服务启动完成、开始监听后，异步执行一次「按配置重算期望值 → 读回 → 不一致就重写」。
   - 自愈不阻塞监听；失败只写日志 warn，不影响 `GET /api/v1/healthz`。
   - 自愈**不探测**（探测只能由显式请求触发，§4.1）。
3. 自愈时的配置来源：`config.toml` + `egress.toml` 当前内容；解析失败 → 只记日志，不写 settings（保守：宁可不动）。
4. 卸载：`uninstall.sh` 在停止服务**之前/之后**执行 `settings put global http_proxy :0`（root 可用），再删运行时目录（§2.2）。

### 3.5 fail-open 判定与观测

**默认 `fail_open = true`**（决策 #5）。判定只在**三个时点**发生，**没有后台常驻健康检查**（不引入定时器/额外流量/额外被检测面）：

| 时点 | 触发条件 | 动作 |
| --- | --- | --- |
| ① 显式探测失败 | `POST /egress/probe` 返回 `egress.probe_failed` 且 `reason ∈ {connect, dns, timeout, tls}` | 见下「fail-open 动作」 |
| ② 写入后读回不一致 | `PUT/DELETE /egress` 的 `readback.matches=false` | 同上（`degradedReason="settings_mismatch"`） |
| ③ 启动自愈读回不一致 | 自愈时 `matches=false` | 同上 |

`reason ∈ {http_status, invalid_response}` **不触发** fail-open：那说明代理链路是通的、只是目标站点或响应形态有问题，切直连无益。

**fail-open 动作**（`failOpen=true`）：执行「清直连」（`settings put global http_proxy :0`），保留全部配置（`egress.toml`、`[egress].default`、`[[apps]].egress` 都不动），并置：

    health.degraded       = true
    health.degradedSince  = <首次降级时刻，不随后续刷新重置>
    health.degradedReason = "probe_failed" | "settings_mismatch" | "settings_unavailable"
    mode                  = "proxy"      （期望模式不变）
    effectiveMode         = "direct"     （实际模式）
    applied               = false

响应里追加 warning `egress_degraded_to_direct`(`{reason}`)。

**恢复**：任一次成功探测（`ok:true`）→ 重写期望值 → 读回校验 → `degraded:false`、`degradedSince:null`、`effectiveMode:"proxy"`、`applied:true`，并记一条 info 日志 `egress recovered`。

**fail-closed**（`failOpen=false`）：判定照做，但**不动 settings**；`applied` 报真实值，`health.degraded=true` 仅表示「探测失败，链路可能不通」。这是给「宁断网也不换 IP」的场景。

**观测方式**（三条，缺一不可）：

1. `GET /egress` 的 `health.{degraded, degradedSince, degradedReason, lastProbe}` + `readback`（唯一权威状态）。
2. 服务日志：降级 `warn` 一行（`egress degraded`，字段 `reason` / `effectiveMode`，**不含 host/port/凭据**）；恢复 `info` 一行。
3. 人工/QA：`adb shell su -c 'dumpsys connectivity | grep -i "global proxy"'` 应为 `Global proxy: none`。

**协议状态不持久化**：`health` 与 `lastProbe` 只存在内存，服务重启后清空（`degraded:false`）。设备端没有数据库，落盘文件只增加暴露面；长期观测由 Manager 的 `egress_checks` 承担（§6.11）。

### 3.6 失败面

| 情形 | 表现 | 错误码 / 处理 |
| --- | --- | --- |
| `settings` 二进制缺失 | `CommandOutcome::Unavailable` | `503 upstream_unavailable` `{component:"settings"}`；文件仍写入 |
| SELinux 拒绝（`avc: denied`） | 命令非零退出 | 同上 + 日志一行；`applied=false` |
| `settings get` 输出不可解析 | 判定为 `matches=false` | warning `egress_settings_mismatch` |
| host 含 IPv6 字面量 | `settings` 需要 `[v6]:port` 形态 | 设备级**拒绝** IPv6 字面量：`422 invalid_egress_proxy`（`details.errors[].message` 说明「设备级仅支持 IPv4/域名」）；应用级仍可用 IPv6 |
| host 是域名 | 原样写入 `settings`（Android 侧解析） | 允许 |

### 3.7 已知限制（方案 A，写进文档但不阻塞验收）

1. **只覆盖 HTTP/HTTPS CONNECT**：SOCKS5 必须走方案 B。
2. **不承载代理认证**：`settings global http_proxy` 只写 `host:port`。指向带凭据的代理时，仍写 `host:port` 并给 warning `egress_credentials_ignored`（带认证需求请用应用级）。
3. **UDP/QUIC 不受约束**（QUIC 可能绕过代理直连）；P2 才考虑 `iptables` DROP udp/443。
4. **DNS**：HTTP CONNECT 由代理解析域名（不泄漏）；应用「先本地解析再连 IP」的场景会绕过。
5. **已建立的连接不切链路**，需应用重连。
6. **VPN 类应用会覆盖 settings 代理**。
7. **`http.proxyHost` 对应用可见**，属可检测项（伪装场景本就要它生效）。

---

## 4. 出口 IP 探测契约

### 4.1 触发点

| 触发点 | 谁发起 | scope | 结果去向 |
| --- | --- | --- | --- |
| `POST /api/v1/egress/probe` | WebUI / Manager job / 用户 curl | 设备 | 设备内存 `health.lastProbe` + 响应；Manager job 另写 `egress_checks(source="device")` |
| `POST /relay/api/v1/egress-proxies/{id}/probe` | Manager WebUI / 用户 curl | Manager 主机 | `egress_checks(source="manager")`（验证代理本身，与设备无关） |

设备端**没有**后台探测（不引入定时器），探测永远是显式动作。

### 4.2 请求 / 响应契约

请求与成功响应字段表见 §1.6。此处固化语义：

| 项 | 冻结值 |
| --- | --- |
| probe URL 默认值 | `http://checkip.amazonaws.com/`（明文 HTTP，避免引入 TLS 栈，符合 FR-3） |
| probe URL 可配置 | 是：`[probe].url`（持久）+ 请求体 `url`（单次覆盖） |
| 协议限制 | **必须 `http://`**：拒绝 `https://`（避免新增 TLS 依赖与证书校验语义）→ `422 invalid_probe_url` |
| 超时 | **默认 5 s**；`[probe].timeout_seconds` / 请求体不改超时（只有 `[probe].timeout_seconds` 可配）；硬上限 30 s |
| 重定向 | 不跟随（`30x` 视为 `http_status` 失败） |
| 请求头 | 仅 `Host`、`User-Agent: devmask/<version>`、`Accept: text/plain, application/json`；**不带**任何凭据头 |
| 响应体上限 | 4 KiB（防大响应拖死探测） |
| 响应字段 | `exitIp` / `latencyMs` / `checkedAt`（+ 本节 §1.6 的辅助字段） |

### 4.3 URL 与响应体校验

URL：必须以 `http://` 开头；≤512 字符；不得含 `user:pass@`；`host` 非空；端口 1..=65535（缺省 80）。

响应体解析（按序尝试，命中即返回 `exitIp`）：

1. `trim()` 后是 IPv4 或 IPv6 字面量 → 采用。
2. `trim()` 后是 JSON 对象且含字符串字段 `ip` / `origin` / `query`，其值可解析为 IP 字面量 → 采用该字段。
3. 其它（HTML、空体、多个 IP 用逗号并列、非 IP 文本）→ `502 egress.probe_failed` `reason="invalid_response"`。

`latencyMs`：从建立连接到读完响应体（≥0，整数毫秒）。`checkedAt`：请求发出时刻（RFC 3339 UTC，秒级）。

### 4.4 失败码

| code | HTTP | retryable | details | 何时 |
| --- | --- | --- | --- | --- |
| `egress.not_configured` | 409 | false | `{scope, package?}` | 设备级无 `default` 且无任何代理定义；或 `scope:"app"` 的包既无覆盖也无设备级默认 |
| `egress.probe_failed` | 502 | true | `{url, reason, timeoutMs, upstreamStatus?}` | 见下 reason 枚举 |

`reason` 与触发条件（`fail-open` 只认前四个，§3.5）：

| reason | 触发 |
| --- | --- |
| `connect` | TCP 连接失败/被拒 |
| `dns` | 域名解析失败 |
| `timeout` | 超过 `timeout_seconds`（含读响应体阶段） |
| `tls` | 误配 https 目标或 TLS 握手失败（正常不会出现，因为 URL 限 http） |
| `http_status` | 收到非 2xx 响应 |
| `invalid_response` | 2xx 但响应体不满足 §4.3 的解析规则 / 超过 4 KiB |

### 4.5 与 fail-open 的关系（重申，防误读）

- 探测**不回落**：失败就返回 502，`via` 不会变成 `direct`。
- 只有在 `reason ∈ {connect, dns, timeout, tls}` 且 `failOpen=true` 时，探测**副作用**才会把流量切到直连（§3.5），并在响应头之外的 `health` 里体现。验收时必须把「探测返回 502」与「随后流量变直连」当成两个独立的可观测事实。

### 4.6 结果留存

| 侧 | 留存 | 生命周期 |
| --- | --- | --- |
| 设备 | 内存 `health.lastProbe`（只有 1 条） | 进程重启清空 |
| Manager | `egress_checks` 表（历史） | 见 §6.11；契约只保证「最近 1 条可读」 |

---

## 5. 应用级注入清单与降级（方案 B）

### 5.1 注入清单（冻结）

注入发生在 Zygisk 的 `app_specialize` 阶段（目标进程内，早于应用代码执行）。生效时机为**进程重启**（与既有配置语义一致）。

| # | 载体 | 值 | 适用 `type` | 失败后果 |
| --- | --- | --- | --- | --- |
| 1 | 系统属性 `http.proxyHost` | `host` | `http` | 跳过该项，继续后续项 |
| 2 | 系统属性 `http.proxyPort` | `port`（十进制字符串） | `http` | 同上 |
| 3 | 系统属性 `https.proxyHost` | `host` | `http` | 同上 |
| 4 | 系统属性 `https.proxyPort` | `port` | `http` | 同上 |
| 5 | 系统属性 `socksProxyHost` / `socksProxyPort` | `host` / `port` | `socks5` | 同上 |
| 6 | `java.net.ProxySelector.setDefault(ProxySelector.of(InetSocketAddress.createUnresolved(host, port)))` | 与 1–5 同源的地址 | `http` 与 `socks5`（`socks5` 用 `Proxy.Type.SOCKS` 的 selector） | 反射失败 → 只保留属性注入，日志 warn |
| 7 | 凭据载体（对应 FR-6 / 决策 #3）：`java.net.socks.username` / `java.net.socks.password`；HTTP Basic 用 `java.net.Authenticator.setDefault`（`getRequestorType()==PROXY` 时返回该代理的凭据） | `username` / `password` | 仅当该定义 `hasCredentials` | 任一步失败 → 只记日志；应用仍按无认证链路走 |
| 7-注 | **Android 17 真机实测不成立，已降级为文档限制**：属性写入正确（进程内可读），但 libcore 的 SOCKS 客户端不发送 `java.net.socks.username/password`（fixture：`methods=0002 user_ok=0 pass_ok=0` → `auth-failed`）；HTTP Basic 未实现（仅注入 host/port，记 `[egress] unsupported-auth`）。因此**本期不支持带认证的代理**，需 IP 白名单或等 P2「设备内本地认证中继」；契约其余条目不变。见 [../CONFIG.md](../CONFIG.md#已知限制) | — | — | 文档限制（t15 O1） |

说明：

- 1–6 是**必做清单**；7 是「有凭据才做」的条件项（不做会让带认证的代理 100% 不可用，属 FR-6 的落地面）。
- 只操作 **Java 层**：系统属性 + `ProxySelector`。**不改** native（cronet / 自研 C++ 栈 / QUIC），不写 iptables。
- 注入的属性值来自 companion 下发的**已解析**结果（`package → {type, host, port, username?, password?} | direct`），注入侧不读 `egress.toml`（Zygisk 侧不碰 0600 文件，减少暴露面）。

### 5.2 `__DIRECT__` 必须显式直连

关键点：设备级（方案 A）写的是**系统级** `http_proxy`，Android 的默认 `ProxySelector` 会把它回报给应用。因此应用级的「强制直连」**不能只靠「不注入」**——那等于继承系统代理，`__DIRECT__` 会失效。

冻结规则：`__DIRECT__` 的处理是**主动**的：

1. 注入 `ProxySelector.setDefault(<direct selector>)`（返回只含 `Proxy.NO_PROXY` 的列表）。
2. **不**设置 `http.proxyHost` / `https.proxyHost` / `socksProxyHost` 任何一项（若父进程环境里已有同名属性，显式 `clearProperty` 再置空）。
3. 结果：该应用在系统代理开启时依然直连。

同理，「`[[apps]].egress` 未配置」的应用**不做任何注入**（保持继承），不能顺手清属性——否则会破坏「设备级对该应用生效」的预期。

### 5.3 协议与属性互斥（防错配）

| `type` | 设置 | **不设置** | 理由 |
| --- | --- | --- | --- |
| `http` | `http.proxyHost/Port`、`https.proxyHost/Port`、`ProxySelector`（HTTP 代理） | `socksProxyHost/Port` | 若把 `socksProxyHost` 指向 HTTP 代理端口，SOCKS 握手会打到 HTTP 端口上，表现为「全都连不上」 |
| `socks5` | `socksProxyHost/Port`、`ProxySelector`（`Proxy.Type.SOCKS`）、`java.net.socks.username/password`（有凭据时） | `http.proxyHost/Port`、`https.proxyHost/Port` | 反向同理：把 HTTP 属性指向 SOCKS 端口会让 HTTP 客户端发 `CONNECT` 到 SOCKS 端口 |

### 5.4 注入时机与可见性

1. 时机：`app_specialize`（与既有 `android.os.Build` 字段覆写同一阶段），在 `Application` 构造之前。
   - **实现补充（v1.10.0 真机修复后）**：框架会在绑定应用时按设备级 `ProxyInfo` 重写这些属性，因此实现增加了「后置重放」——注入后按 0–2000ms 的时间表读回并重写，且只对显式写法注入。机制、代价与日志见 [../CONFIG.md](../CONFIG.md#应用级注入如何生效后置重放)；本节其余契约（属性清单、互斥规则、失败不阻断启动）不变。
2. 生效：进程启动即生效；改配置后需**重启应用**（与既有「重启后生效」一致；Manager job 可带 `restart`）。
3. 可见性：注入后的系统属性对应用**可见**（`System.getProperty("http.proxyHost")`），属可检测项——与方案 A 的「`http.proxyHost` 可见」同一性质，不额外隐藏。
4. 作用域：只影响**目标进程**；同机其它应用零影响（方案 B 的核心价值）。

### 5.5 降级规则（硬约束）

**注入失败不得阻断应用启动。** 具体口径：

| 情形 | 行为 |
| --- | --- |
| 代理定义缺失 / 名字非法（配置层已过滤，正常不会到这里） | 视为未配置：不注入 |
| 反射 `ProxySelector.setDefault` 失败（隐藏 API / 类加载失败） | 保留已成功的属性注入；日志 `warn` 一行；应用继续启动 |
| 属性写入抛异常 | 跳过该项，继续其余项；汇总一行 `warn` |
| 凭据载体注入失败 | 忽略凭据，按无认证继续；`warn` |
| 任何未预期异常 | 最外层 `catch-all`：记日志后 `return`，**绝不**抛出到 `app_specialize` 之外 |

日志格式（不含凭据）：

    [egress] inject failed package=com.example.a step=proxy_selector error=<简短原因>
    [egress] injected package=com.example.a type=socks5 host=198.51.100.7 port=1080 creds=1

观测：注入结果只在**日志**里（native 侧无回传通道）。配置层状态由 `GET /api/v1/egress` 的 `apps[].effective` 表达；`GET /apps/{pkg}` 的 `resolved` **不**增加 egress 字段（`resolved` 是机型字段的封闭集合，见 [docs/API.md](../API.md) §6.4）。

### 5.6 覆盖边界

| 通道 | 方案 A（设备级） | 方案 B（应用级） |
| --- | --- | --- |
| OkHttp / HttpURLConnection | ✅（系统代理） | ✅ |
| WebView | ✅ | ❌（WebView 走系统代理，不看进程内属性） |
| native（cronet / C++ / QUIC） | 部分（遵循系统代理的栈） | ❌ |
| UDP | ❌ | ❌ |

---

## 6. Manager 端 API 与数据模型

### 6.1 通用约定

- 基地址 `/relay/api/v1`；鉴权、错误信封、`Cache-Control: no-store`、`X-Request-Id` 全部沿用 [manager/docs/API.md](../../manager/docs/API.md) §2.3–§2.6。
- 字段命名：Relay 自己的字段 **camelCase**；查询参数 snake_case。
- 精确口径：**查询参数统一 snake_case**（`limit`、`cursor`、`enabled`、`q`、`tag`），**请求体与响应体字段 camelCase**（`dryRun`、`proxyId`、`deviceId`）。与设备端 `/api/v1` 一致；egress 端点的 `dryRun` 只出现在**请求体**里（不设 `dry_run` 查询参数）。
- 分页：`limit` 默认 50 / 最大 500；`cursor` 不透明；`nextCursor:null` 结束（沿用 §2.9）。
- 固定排序（新增一行，插进 §2.9 的表）：`/egress-proxies` 按 `name` 升序（ASCII）；`/devices/{id}/egress` 的 `bindings` 按 `app` 升序（`""` 最前）。
- 审计 action 新增：`egress.proxy_create`、`egress.proxy_update`、`egress.proxy_delete`、`egress.proxy_probe`、`egress.bind`、`egress.apply`。`detail` 只放 `{proxyId, name, deviceId?, app?, changed}`，**绝不**放 host 凭据/密码。`target.type` 取值：`egress_proxy`（新）与既有 `device` / `job`。
- ID 形态：`^egp_[0-9a-f]{32}$`（与 `job_*` / `req_*` 风格一致）。
- **信封的已知例外**：`POST /egress-proxies/{id}/probe` 的 `502 egress_probe_failed` 在信封之外**额外带顶层 `warnings`**（探针结果没写进 `egress_checks` 时给 `egress_check_not_recorded`，否则 `[]`）。客户端读错误时必须同时读顶层 `warnings`；其余 4xx/5xx 仍是纯信封（§6.7）。

### 6.2 `GET /egress-proxies`

查询参数：`q`（name/host/region 子串，不区分大小写）、`enabled`(`true`/`false`)、`tag`（可重复，AND）、`limit`、`cursor`。

响应 `200`：

    {
      "total": 2,
      "nextCursor": null,
      "items": [
        {
          "id": "egp_3f1c9b7a2d4e5f60819a0b1c2d3e4f50",
          "name": "us_la_01",
          "type": "http",
          "host": "203.0.113.10",
          "port": 1080,
          "hasCredentials": true,
          "region": "US-LA",
          "tags": ["us", "mobile"],
          "enabled": true,
          "boundDevices": 3,
          "boundApps": 4,
          "lastCheck": {
            "ok": true,
            "source": "device",
            "exitIp": "203.0.113.7",
            "latencyMs": 412,
            "errorCode": null,
            "checkedAt": "2026-11-24T12:00:00Z",
            "deviceId": "pixel-9",
            "app": ""
          },
          "createdAt": "2026-11-24T08:00:00Z",
          "updatedAt": "2026-11-24T09:30:00Z"
        }
      ]
    }

`EgressProxy` 字段表（**所有端点共用**）：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `id` | string | 否 | `^egp_[0-9a-f]{32}$` |
| `name` | string | 否 | §2.5 正则；全局唯一（与设备端表名 1:1） |
| `type` | `"http"` \| `"socks5"` | 否 | DB 列名是 `kind`（§6.11），线上叫 `type` |
| `host` | string | 否 | ≤253，不含 scheme/路径 |
| `port` | integer | 否 | 1..65535 |
| `hasCredentials` | boolean | 否 | `username_cipher` 或 `password_cipher` 存在即 `true`。**唯一的凭据信号**——响应里没有 `username` 字段 |
| `region` | string | 是 | ≤64；自由文本（如 `US-LA`），用于「地区与机型不匹配」告警 |
| `tags` | string[] | 否 | ≤16 项，每项 ≤32 字符；`[]` 表示无标签 |
| `enabled` | boolean | 否 | `false`：**不允许新建绑定**；已有绑定仍下发但附 warning（避免误停导致整机断网） |
| `boundDevices` | integer | 否 | `device_egress` 中引用它的去重设备数 |
| `boundApps` | integer | 否 | 引用行数（含 `app=""` 的整机行） |
| `lastCheck` | object\|null | 是 | 最近一条 `egress_checks`（device/manager 来源混合取 `checked_at` 最大者）；无记录为 `null` |
| `createdAt` / `updatedAt` | string | 否 | RFC 3339 UTC |

`lastCheck` 子对象：`{ok, source:"device"|"manager", exitIp|null, latencyMs|null, errorCode|null, checkedAt, deviceId|null, app}`。

### 6.3 `POST /egress-proxies`

请求体：

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `name` | string | 是 | §2.5；唯一 |
| `type` | `"http"` \| `"socks5"` | 是 | — |
| `host` | string | 是 | ≤253 |
| `port` | integer | 是 | 1..65535 |
| `username` | string | 否 | ≤128；**只写不读**（响应里没有这个字段） |
| `password` | string | 否 | ≤256；**只写不读** |
| `region` | string | 否 | ≤64 |
| `tags` | string[] | 否 | ≤16 项 |
| `enabled` | boolean | 否 | 默认 `true` |

响应 `201`：`EgressProxy`。

错误：`401`、`409 egress_proxy_exists`（`details{field:"name", value, id}`）、`413`、`415`、`422 validation_failed`（`ValidationError.code` 新增 `invalid_egress_name` / `invalid_egress_proxy` / `out_of_range` / `duplicate_name`）、`500`。

### 6.4 `GET /egress-proxies/{id}`

响应 `200`：`EgressProxy` + 两个附加对象（详情才有）：

    {
      "…": "EgressProxy 的全部字段",
      "bindings": [
        { "deviceId": "pixel-9", "deviceName": "Pixel 9", "app": "", "updatedAt": "2026-11-24T09:00:00Z" },
        { "deviceId": "pixel-9", "deviceName": "Pixel 9", "app": "com.example.a", "updatedAt": "2026-11-24T09:00:00Z" }
      ],
      "recentChecks": [
        { "source": "device", "deviceId": "pixel-9", "app": "", "ok": true, "exitIp": "203.0.113.7", "latencyMs": 412, "errorCode": null, "checkedAt": "2026-11-24T12:00:00Z" }
      ]
    }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `bindings` | object[] | 引用该定义的绑定（`app=""` 表示整机）；按 `deviceId, app` 升序 |
| `recentChecks` | object[] | 最近 20 条 `egress_checks`（`checked_at` 降序） |

错误：`401`、`404 not_found`（`details{resource:"egress_proxy", name:`…`}`）、`500`。

### 6.5 `PATCH /egress-proxies/{id}`

JSON Merge Patch 风格（沿用 [docs/API.md](../API.md) §5.11 的口径），但凭据字段用**双态**：

| 字段 | 省略 | `null` | 有值 |
| --- | --- | --- | --- |
| `name` / `type` / `host` / `port` / `enabled` | 不改 | 422（这些字段不可置空） | 覆盖 |
| `region` / `tags` | 不改 | 清除（`NULL` / `[]`） | 覆盖 |
| `username` / `password` | **不改（已存密文原样保留）** | **清除** | 覆盖（重新加密） |

响应 `200`：`EgressProxy`（更新后）。

边界：

1. 改名（`name`）→ 已绑定设备**不会**自动改名；下次 `apply` 时设备端会按新名字写 `egress.toml` 并把旧表名当作未引用定义清理（同一次 `PUT /egress` 是整体替换 `proxies`，旧名自然消失）。响应附 warning `egress_rename_pending`(`{oldName, newName, devices}`)。
2. 改 `type`/`host`/`port` 与凭据同理：只落库，不自动下发。
3. `enabled=false` 不影响已有绑定。

错误：`401`、`404`、`409 egress_proxy_exists`（改名撞车）、`413`、`415`、`422`、`500`。

### 6.6 `DELETE /egress-proxies/{id}`

- 被绑定则拒绝：`device_egress` 有引用 → `409 egress_proxy_in_use`，`details{proxyId, name, total, devices:[…], apps:[…]}`（`devices` ≤20 项，超出截断并在 `total` 里给全量）。
- 成功 → `204 No Content`（与 `DELETE /devices/{id}` 一致）。`egress_checks` 里 `proxy_id` 按 `ON DELETE SET NULL` 保留历史行的其它字段。
- 审计 `egress.proxy_delete`，`detail={proxyId, name}`（不含 host/凭据）。

字段表：

| 项 | 位置 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- | --- |
| `id` | 路径参数 | string | 是 | `^egp_[0-9a-f]{32}$` |
| （请求体） | — | — | — | DELETE 不接受请求体；带体将被忽略 |
| （响应体） | — | — | — | `204` 无响应体；失败情况走统一错误信封 |

错误：`401`、`404`、`409`、`500`。

### 6.7 `POST /egress-proxies/{id}/probe`

- 用途：**Manager 侧直连探测**，验证代理本身可用（与设备链路无关，FR-3）。
- 请求体可省略（全字段可选）：

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `url` | string | 否 | 覆盖探测 URL；默认 `manager.toml` 的 `[egress].probe_url`，再缺省 `http://checkip.amazonaws.com/`；限 `http://`、≤512 字符 |

> 该 URL 是 **Manager 主机**的探测目标（与设备端 `[probe].url` 各自独立），因为「验证代理本身可用」不需要设备参与。
- 执行：用该代理的 `type/host/port/凭据`（解密后仅在内存）发一次 GET，5 s 超时；结果**总是**写入 `egress_checks(source="manager")`（成功与失败都写）。

响应 `200`（与设备端一致：失败不用 200 包裹）：

    {
      "proxyId": "egp_3f1c…",
      "ok": true,
      "exitIp": "203.0.113.7",
      "latencyMs": 380,
      "checkedAt": "2026-11-24T12:00:00Z",
      "url": "http://checkip.amazonaws.com/",
      "upstreamStatus": 200,
      "warnings": []
    }

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `proxyId` | string | 否 | 被探测的代理 id |
| `ok` | boolean | 否 | 成功恒 `true`（失败走 `502`，不用 200 包裹） |
| `exitIp` | string | 否 | 出口 IP 字面量 |
| `latencyMs` | integer | 否 | 毫秒 |
| `checkedAt` | string | 否 | RFC 3339 UTC |
| `url` | string | 否 | 实际请求的 URL |
| `upstreamStatus` | integer 或 null | 是 | probe URL 的响应状态码；未拿到响应时为 `null` |
| `warnings` | Warning[] | 否 | **恒在**（没有内容时 `[]`）；探测结果没落库时带 `egress_check_not_recorded`（`{proxyId, source:"manager"}`），探测本身仍返回 `200` |

失败：`502 egress_probe_failed`，`details{proxyId, url, reason, timeoutMs, upstreamStatus?}`，`reason` 枚举同 §4.4（`connect`/`dns`/`timeout`/`tls`/`http_status`/`invalid_response`）。此时响应体之外还会写一条 `ok=0` 的 `egress_checks`。

**失败响应 = 错误信封 + 顶层 `warnings`**（不是纯信封；`warnings` 恒在）：

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

`422 unsupported_proxy_type`（`socks5` 代理）仍是**纯信封**，不带顶层 `warnings`。

错误：`401`、`404`、`422 invalid_probe_url/unsupported_proxy_type`（`socks5` 需要 SOCKS5 客户端；若实现只支持 HTTP CONNECT，则 `socks5` 探测返回 `422 unsupported_proxy_type` 并把 `ok=0,error_code="unsupported_proxy_type"` 入库）、`502`、`500`。

### 6.8 `GET /devices/{id}/egress`

- 用途：看该设备「绑了什么」+「设备上实际是什么」。
- 实拉设备：设备在线时实时 `GET /api/v1/egress`（读超时 10 s）；离线/失败不报错，`deviceState:null` + warning `device_offline`（沿用 §4.7「离线是预期结果」的口径）；旧版本（404）给 `egress_unsupported`。
- **设备 Token 无法解密**时（`devmask-manager.key` 变更 / 密文损坏）**不归入 `device_offline`**：`warnings` 给 `device_token_invalid`（`details={deviceId, code:"device_token_invalid", op:"decrypt"}`，与 job 结果码同名同根因），`deviceState` 仍为 `null`，但 **`bindings` 照常返回**（绑定先查库、再解密 Token），HTTP 状态仍是 `200`。

响应 `200`：

    {
      "deviceId": "pixel-9",
      "bindings": [
        { "app": "", "mode": "proxy", "proxyId": "egp_3f1c…", "proxyName": "us_la_01", "updatedAt": "2026-11-24T09:00:00Z" },
        { "app": "com.example.b", "mode": "direct", "proxyId": null, "proxyName": null, "updatedAt": "2026-11-24T09:00:00Z" }
      ],
      "deviceState": {
        "fetchedAt": "2026-11-24T12:00:00Z",
        "revision": "5b1f0c9e…",
        "configRevision": "9f2c5d1e…",
        "configured": true,
        "mode": "proxy",
        "effectiveMode": "proxy",
        "default": "us_la_01",
        "applied": true,
        "readback": { "settingsValue": "203.0.113.10:1080", "dumpsysGlobalProxy": "Global proxy: [203.0.113.10] port: 1080", "matches": true, "at": "2026-11-24T12:00:00Z" },
        "health": { "degraded": false, "degradedSince": null, "degradedReason": null, "lastProbe": { "ok": true, "exitIp": "203.0.113.7", "latencyMs": 412, "checkedAt": "2026-11-24T11:59:12Z", "scope": "device", "via": "proxy", "proxy": "us_la_01", "upstreamStatus": 200, "errorCode": null } },
        "apps": [ { "package": "com.example.b", "egress": "__DIRECT__", "effective": "direct", "proxy": null } ]
      },
      "warnings": []
    }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `bindings[].app` | string | `""` = 整机（方案 A），其余 = 应用级 |
| `bindings[].mode` | `"proxy"` \| `"direct"` | 与 `device_egress.mode` 一致；`direct` 时 `proxyId/proxyName` 为 `null` |
| `deviceState` | object\|null | **设备端 `GET /egress` 的透传子集**（原样转发，不裁剪新字段；Relay 不认识的新字段保留，遵循 §2.12） |
| `deviceState.fetchedAt` | string | 本次实时拉取时刻 |

错误：`401`、`404 device_not_found`、`500`（离线不报错）。

### 6.9 `PUT /devices/{id}/egress`

- 用途：**只落库**（绑定关系），不下发。响应显式给 `pendingApply:true`，实际下发必须调 §6.10（单一职责：绑定可反复编辑，下发才发网络请求、才进 job/审计）。
- 请求体（**全量替换**该设备的绑定表）：

      {
        "bindings": [
          { "app": "",             "proxyId": "egp_3f1c…" },
          { "app": "com.example.a", "proxyId": "egp_7a2b…" },
          { "app": "com.example.b", "proxyId": "__DIRECT__" },
          { "app": "com.example.c", "proxyId": null }
        ]
      }

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `bindings` | object[] | 是 | ≤200 项；同 `app` 唯一；`[]` = 清空所有绑定 |
| `bindings[].app` | string | 是 | `""` 或合法包名 |
| `bindings[].proxyId` | string \| `"__DIRECT__"` \| null | 是 | 代理 id / 强制直连 / `null` = 该行不保留（等价于从表里删除） |

语义与校验：

1. `""`（整机）+ `"__DIRECT__"`/`null` → 该行**不写库**（整机直连 = 无绑定）；响应 `bindings` 里不出现。
2. `proxyId` 指向不存在的代理 → `422 validation_failed`（`path="bindings[1].proxyId"`，`code="invalid_value"`）。
3. `proxyId` 指向 `enabled=false` 的代理且该行是**新增** → `422`（`code="invalid_value"`，message「代理已停用，不能新建绑定」）。已存在的行保留。
4. 未知 `app`（空字符串以外不以合法包名开头）→ `422 invalid_package`。
5. 幂等：同一请求重放 → 同 `changed:false`，`200`。

响应 `200`：

    {
      "deviceId": "pixel-9",
      "changed": true,
      "bindings": [ …与 GET 同构… ],
      "pendingApply": true,
      "warnings": []
    }

错误：`401`、`404 device_not_found`、`413`、`415`、`422`、`500`。

### 6.10 `POST /devices/egress/apply`

- 用途：把**当前绑定 + 代理定义**推送到设备，返回 `202` + `Job(kind="egress")`。
- 两种请求形态，**二选一**（同时出现 → `422 invalid_value`），与 `POST /apply` 的 bulk/assignments 双形态对齐（§4.13）：

形态 A：bulk（下发现有绑定）

    { "devices": "all" | ["pixel-9", "redmi-k80"], "probe": false, "dryRun": false }

形态 B：assignments（先落库再下发，用于「批量绑定」）

    {
      "assignments": [
        { "deviceId": "pixel-9", "app": "", "proxyId": "egp_3f1c…" },
        { "deviceId": "redmi-k80", "app": "com.example.a", "proxyId": "__DIRECT__" }
      ],
      "probe": true,
      "dryRun": false
    }

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `devices` | `"all"` \| string[] | — | 形态 A 必填；`"all"` 展开为全部注册设备（按 `createdAt` 升序） |
| `assignments` | object[] | — | 形态 B 必填；≤200 项；每项 `{deviceId, app, proxyId}`，`proxyId` 允许代理 id / `"__DIRECT__"` / `null` |
| `probe` | boolean | `false` | 下发成功后是否 `POST /api/v1/egress/probe`（失败只记 warning，不改结果） |
| `dryRun` | boolean | `false` | 只计算与校验、不落库、不下发；响应 `200` + `dryRun:true` + 逐目标 `plan`（不建 job） |

响应 `202`（形态 A/B）：

    {
      "jobId": "job_9c1e…",
      "status": "queued",
      "kind": "egress",
      "total": 3,
      "createdAt": "2026-11-24T12:00:00Z",
      "requestId": "req_3f1c…"
    }

幂等：支持 `Idempotency-Key`（沿用 §2.11：同键直接返回首次 job + `Idempotent-Replay: true`，不设 TTL）。

错误：`401`、`413`、`415`、`422`（`missing_required` / `invalid_value` / `too_many_items` / `invalid_package`）、`500`。

### 6.11 SQLite v2：三张表 DDL 与迁移

**schema version 2**（在 `schema_migrations` 追加一行 `version=2`；迁移在监听端口之前串行执行，沿用 [manager/docs/ARCH.md](../../manager/docs/ARCH.md) §5.3）。

    -- v2-1 代理池
    CREATE TABLE IF NOT EXISTS egress_proxies (
      id               TEXT PRIMARY KEY,                      -- egp_<32 hex>
      name             TEXT NOT NULL UNIQUE,                   -- ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$
      kind             TEXT NOT NULL CHECK (kind IN ('http','socks5')),  -- 线上字段叫 type
      host             TEXT NOT NULL,                          -- ≤253，不含 scheme
      port             INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
      username_cipher  TEXT,                                   -- NULL = 未设；"v1:"+b64(nonce||ct||tag)
      password_cipher  TEXT,                                   -- NULL = 未设
      region           TEXT,                                   -- ≤64，可 NULL
      tags             TEXT NOT NULL DEFAULT '[]',              -- JSON string[]
      enabled          INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
      created_at       TEXT NOT NULL,                           -- RFC 3339 UTC
      updated_at       TEXT NOT NULL,
      CHECK (username_cipher IS NULL OR username_cipher LIKE 'v1:%'),
      CHECK (password_cipher IS NULL OR password_cipher LIKE 'v1:%')
    );
    CREATE INDEX IF NOT EXISTS idx_egress_proxies_name    ON egress_proxies(name);
    CREATE INDEX IF NOT EXISTS idx_egress_proxies_enabled ON egress_proxies(enabled);

    -- v2-2 绑定（app='' 即整机）
    CREATE TABLE IF NOT EXISTS device_egress (
      device_id  TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
      app        TEXT NOT NULL DEFAULT '',                      -- '' = 整机（方案 A）
      mode       TEXT NOT NULL CHECK (mode IN ('proxy','direct')),
      proxy_id   TEXT REFERENCES egress_proxies(id) ON DELETE RESTRICT,
      updated_at TEXT NOT NULL,
      PRIMARY KEY (device_id, app),
      CHECK ((mode = 'proxy'  AND proxy_id IS NOT NULL)
          OR (mode = 'direct' AND proxy_id IS NULL))
    );
    CREATE INDEX IF NOT EXISTS idx_device_egress_proxy  ON device_egress(proxy_id);
    CREATE INDEX IF NOT EXISTS idx_device_egress_device ON device_egress(device_id);

    -- v2-3 探测结果（设备来源 + Manager 直连来源）
    CREATE TABLE IF NOT EXISTS egress_checks (
      id         INTEGER PRIMARY KEY AUTOINCREMENT,
      source     TEXT NOT NULL CHECK (source IN ('device','manager')),
      device_id  TEXT REFERENCES devices(id) ON DELETE CASCADE,  -- manager 直连探测为 NULL
      app        TEXT NOT NULL DEFAULT '',                        -- '' = 整机
      scope      TEXT NOT NULL DEFAULT 'device' CHECK (scope IN ('device','app')),
      proxy_id   TEXT REFERENCES egress_proxies(id) ON DELETE SET NULL,
      exit_ip    TEXT,
      latency_ms INTEGER,
      ok         INTEGER NOT NULL CHECK (ok IN (0,1)),
      error_code TEXT,
      checked_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_egress_checks_device  ON egress_checks(device_id, checked_at DESC);
    CREATE INDEX IF NOT EXISTS idx_egress_checks_proxy   ON egress_checks(proxy_id, checked_at DESC);
    CREATE INDEX IF NOT EXISTS idx_egress_checks_checked ON egress_checks(checked_at DESC);

**`jobs.kind` 的 CHECK 必须扩容（SQLite 不能 ALTER CHECK，只能重建表）** —— 这是本增量唯一的破坏性迁移，冻结配方：

    1) PRAGMA foreign_keys = OFF;              -- 连接级；必须先关，否则 step 4 会级联删 job_results
    2) BEGIN;
    3) CREATE TABLE jobs_v2 ( …与 v1 完全相同的列… ,
         kind TEXT NOT NULL CHECK (kind IN ('bulk','assignments','egress')) );
    4) INSERT INTO jobs_v2 SELECT <按列名显式列出全部 v1 列> FROM jobs;
    5) DROP TABLE jobs;
    6) ALTER TABLE jobs_v2 RENAME TO jobs;
    7) CREATE INDEX IF NOT EXISTS idx_jobs_created ON jobs(created_at DESC);
       CREATE INDEX IF NOT EXISTS idx_jobs_status  ON jobs(status);
    8) COMMIT;
    9) PRAGMA foreign_keys = ON;
    10) PRAGMA foreign_key_check;              -- 必须返回空集，否则迁移失败 → 启动失败

要点：

- 步骤 1 关 FK 是**硬要求**：`job_results.job_id REFERENCES jobs(id) ON DELETE CASCADE`，FK 开启时 `DROP TABLE jobs` 会静默删掉历史结果。
- 迁移失败 → 启动失败并给出 `op="migrate"`（沿用 ARCH §5.3），不接受请求。
- `job_results` **不改结构**：egress job 的结果行用 `app=''`（整机）或包名，`revision` 列存**设备端 egress revision**，`attempts/revision/steps/error` 复用。
- `egress_checks` 清理：实现可自行清理（建议每 `(device_id, app)` 保留 500 条、`checked_at` 最旧优先）；契约只保证「最近 1 条可读」（`lastCheck`）。

### 6.12 job `kind="egress"`：状态机与结果码映射

**状态机**（与 §7.4 完全同构，不新增状态）：

    queued ──► running ──► succeeded | partial | failed
       │          │
       └──────────┴──► failed（仅崩溃恢复，error.code="interrupted"）

终态判定（与计数一致，§7.4）：`succeeded==total` → `succeeded`；`succeeded==0 && failed>0` → `failed`；其余 → `partial`。不变量：终态时 `succeeded+skipped+failed==total`。

**Job 字段映射**：`kind="egress"`、`app=null`、`template=null`、`restart=false`（egress 下发**不**重启应用：方案 A 立即生效，方案 B 需应用重启时由调用方另行发起，避免一次下发改动所有应用）。`total` = 展开后的 `(device, app)` 对数（整机行算 1 对）。

**逐目标流水线**（同一设备内串行；跨设备并发 ≤ `DFM_APPLY_CONCURRENCY`）：

| 步 | 动作 | 设备端请求 | 失败处理 |
| --- | --- | --- | --- |
| 0 | 落库（仅形态 B） | — | 校验失败 → 整个请求 422，不建 job |
| 1 | 取现状 | `GET /api/v1/egress` | 连接/DNS/超时 → `skipped` `device_offline`；404 → `skipped` `egress_unsupported`；403 → `failed` `forbidden_scope`；其它非 2xx → `failed` `device_error` |
| 2 | 组装期望态 | —（本地组装，不发请求） | 只组装 **Manager 拥有的三样**：`default`、`apps`、`proxies`（含凭据）。`failOpen` / `probeUrl` / `probeTimeoutSeconds` / `exclusionList` **不出现在请求体里**（属设备端 `egress.toml` / Android settings，Manager 不下发、不重置）。凭据字段**总是显式给出**（未设置即 `username:null` / `password:null`；省略在设备端等于「保留旧凭据」）。零应用级绑定时发 `apps: null` |
| 3 | 下发（**总是 PUT，Manager 不做本地比较**） | `PUT /api/v1/egress`，`If-Match: <步骤 1 的 egress revision>` | 412 → 重取 revision 重试**一次**（`attempts=2`）；仍 412 → `failed` `config_revision_mismatch`；403 → `forbidden_scope`；422 → `validation_failed`（保留 `details.errors`）；503 `upstream_unavailable` → `upstream_unavailable`。**没有「本地判定没变就跳过」的捷径**：即使内容一致也会真的写一次，是否变化交给设备回答 |
| 4 | 读 `changed` 并落步骤 | — | 设备 `changed:true` → 步骤 `egress_put` 记 `ok`；`changed:false` → 步骤 `egress_put` 记 `status:"skipped"`、`code:"egress_unchanged"`，**结果行仍是 `ok`**、`code="egress_unchanged"`（全部 `skipped` 会把 job 算成 `partial`，所以结果状态不能沿用 `skipped`）。`JobResult.revision` 取设备返回的 `revision` |
| 5 | 探测（仅 `probe=true` 且步骤 3 成功——含 `changed:false` 的情形） | `POST /api/v1/egress/probe` | 任何失败 → 追加 warning `egress_probe_failed`（`details{deviceId, app, reason}`），**不改结果状态**（与 `restart` 的处理一致）；成功则写 `egress_checks(source="device")` |
| 6 | 收尾 | — | 更新 `device.lastSeen`/`online`；写 `job_results`；累加计数；全部完成后写审计 `egress.apply` 并 best-effort 刷新该设备 overview 缓存（不动 job 结果） |

**代价与前提（冻结，评审必须接受）**：

- 每次 `apply` 都会产生**一次设备写入**（`PUT /api/v1/egress`），因此设备 Token 必须带 `write` 作用域；只读 Token 会得到 `403 forbidden_scope`。
- 换来的是：代理凭据轮换能真正下发、设备侧被手改/漂移能被下一次 apply 收敛、Manager 不需要维护「已下发指纹」列（少一份状态就少一处不一致）。
- 幂等语义仍在，只是判定权在设备：`changed:false` 由设备端算（期望态 == 现状），Manager 只负责如实转写成 `egress_unchanged`。
- Manager 侧探测（`POST /egress-proxies/{id}/probe`）仍**只支持 HTTP 代理**（`socks5` → `422 unsupported_proxy_type`），与本条无关。

**结果码映射**（`JobResult.code`）：

| 情形 | status | code | 来源 |
| --- | --- | --- | --- |
| 设备不可达（connect/dns/timeout） | `skipped` | `device_offline` | Manager（既有） |
| 单请求超时 | `skipped` | `upstream_timeout` | Manager（既有） |
| 设备没有 `/egress`（旧版本，404 + `details.resource="path"`） | `skipped` | `egress_unsupported` | Manager（**新增**） |
| 401 / 403 | `failed` | `forbidden_scope` | 设备（既有） |
| 412 重试一次后仍失败 | `failed` | `config_revision_mismatch` | 设备（既有） |
| 422 | `failed` | `validation_failed` | 设备（既有，`details.errors` 原样） |
| 503 `upstream_unavailable`（`settings`） | `failed` | `upstream_unavailable` | 设备（既有） |
| 其它 4xx/5xx、响应非 JSON | `failed` | `device_error` | Manager（既有） |
| 组装期望态时代理凭据解密失败（`devmask-manager.key` 变更 / 密文损坏） | `failed` | `decrypt_failed` | Manager（`details={deviceId, op:"decrypt", reason:"credential_decrypt_failed", message, detail}`） |
| 设备 Token 解密失败（`devmask-manager.key` 变更，需重新登记设备 Token） | `failed` | `device_token_invalid` | Manager（job 结果 `details={deviceId, op:"decrypt"}`；`GET /devices/{id}/egress` 的警告同为该码、`details` 额外带 `code`；**不是** `device_offline`） |
| 设备返回 `changed:false`（内容未变，幂等短路） | `ok` | `egress_unchanged` | 设备（响应体 `changed`） |
| 探测失败（可选步骤） | 结果不变 | — | warning `egress_probe_failed` |

**新增 JobStep.step 值**（扩展 [manager/docs/API.md](../../manager/docs/API.md) §5.7 的枚举，只增不改）：`egress_get`、`egress_put`、`egress_probe`。

**新增警告码**（Manager 侧，`Warning.code`）：

| code | details | 出现位置 |
| --- | --- | --- |
| `egress_probe_failed` | `{deviceId, app, reason}` | job 结果 / 设备出口详情 |
| `egress_credentials_ignored` | `{deviceId, app, name}` | job 结果（设备端警告原样透传） |
| `egress_proxy_not_found` | `{deviceId, app, name}` | job 结果（设备端警告原样透传） |
| `egress_rename_pending` | `{oldName, newName, devices}` | `PATCH /egress-proxies/{id}` |
| `egress_proxy_disabled` | `{proxyId, deviceId}` | job 结果（绑定了已停用代理） |
| `egress_check_not_recorded` | `{deviceId, app, scope, proxyId}`（job）/ `{proxyId, source:"manager"}`（probe 响应） | 探测成功或失败后 `egress_checks` **写入失败**（DB 错误）：job 结果追加该警告；`POST /egress-proxies/{id}/probe` 在响应体 `warnings` 里给同码 |

**新增错误码**（Manager 侧，扁平）：

| code | HTTP | retryable | details | 含义 |
| --- | --- | --- | --- | --- |
| `egress_proxy_exists` | 409 | false | `{field:"name", value, id}` | 代理名已被占用 |
| `egress_proxy_in_use` | 409 | false | `{proxyId, name, total, devices, apps}` | 仍有绑定引用，拒绝删除 |
| `egress_probe_failed` | 502 | true | `{proxyId, url, reason, timeoutMs, upstreamStatus?}` | Manager 直连探测失败 |

> `POST /egress-proxies/{id}/probe` 在**解密代理凭据**失败时不是新码：返回既有 `500 internal_error`，`details.op="decrypt"`（`retryable:true`，但重试无意义——`devmask-manager.key` 变更只能重新录入凭据）。这条路径与 job 的 `decrypt_failed` 是同一根因、不同表达。

### 6.13 与既有 Relay 契约的衔接

| 点 | 口径 |
| --- | --- |
| 端点总表 | 追加 9 行（§6 总览表），`#` 编号续 19–27 |
| `Job.kind` 枚举 | `bulk` \| `assignments` \| `egress`（DB CHECK 同步扩容，§6.11） |
| `JobStep.step` 枚举 | 追加 `egress_get` / `egress_put` / `egress_probe` |
| `AuditEntry.action` | 追加 6 个 `egress.*`（§6.1） |
| `AuditEntry.target.type` | 追加 `egress_proxy` |
| `GET /healthz` | `db.schemaVersion` 由 `1` → `2`（字段不变） |
| `DeviceOverview` 透传 | 不变；egress 状态走独立端点，不进 overview 缓存（避免每次同步都打设备） |

---

## 7. 凭据边界（三条硬约束）

### 7.1 约束一：密码不出现在响应 / 日志 / 审计 / WebUI

**响应**（设备端与 Manager 同一口径）：

1. 任何 DTO **没有** `password` 字段，也**没有** `username` 字段；唯一信号是 `hasCredentials: boolean`。
2. 代理摘要对象固定 5 个键：`{name, type, host, port, hasCredentials}`（§1.3 `upstream`/`proxies[]`、§6.2 `EgressProxy` 的对应子集）。
3. 错误响应不得回显请求体；`validation_failed` 的 `message` 只描述规则，不带值（例如「password 超过 256 字符」，而不是把密码截断打印）。
4. `Debug` 实现必须手写脱敏（对含凭据的结构体，`Debug` 输出 `password: <redacted>`），禁止 `#[derive(Debug)]` 直接暴露。

**日志**：

5. 不记录请求体、不记录 `Authorization`、不记录代理密码/用户名；只记录 `{proxyName, deviceId, app, result}`。
6. 探测日志只记录 `url` 的 host 部分与 `reason`。
7. 日志里的 `egress.toml` 路径可以出现，文件内容不可以。

**审计**：

8. 设备端 `api-audit.jsonl` 沿用既有字段（`method/path/status/revision…`），**不新增请求体快照**；`PUT /egress` 的审计行只有路径与状态码。
9. Manager `audit.detail` 只允许 `{proxyId, name, deviceId, app, changed}`（§6.1）；`ARCH.md` §5.4「`audit.detail` 禁止 Token / 密文」的既有禁令扩写为「…禁止 Token / 密文 / 代理凭据」。

**WebUI**：

10. 输入框 `type="password"`、`autocomplete="new-password"`，**永不回填**已存密码（用 `hasCredentials` 显示「已设置」+「重置」按钮）。
11. 提交后立即清空 DOM 里的值；不写 `localStorage` / `sessionStorage`；不进 URL / hash。
12. 列表、卡片、Toast、错误提示一律不出现凭据；「复制」类交互只允许复制 `host:port`。
13. 设备端 WebUI 的「网络出口」卡片与 Manager「代理池」页同守此规则。

### 7.2 约束二：设备端 `egress.toml` 0600

| 项 | 冻结值 |
| --- | --- |
| 文件权限 | `0600`（root only）；目录 `0700` |
| 创建方式 | `O_CREAT\|O_EXCL` 一步 0600（禁止「先 0644 后 chmod」） |
| 写路径 | tmp → fsync → rename → fsync(dir)（§2.2） |
| 读时自愈 | `mode & 0o077 != 0` → `chmod 0600` + warn + warning `egress_perm_fixed` |
| 备份 | `backups/egress-*.toml` 也是 `0600` |
| 导出 | 不进 `GET /config?format=toml`、不进 WebUI 导出、不进 `GET /egress`（只给脱敏摘要） |
| 与 `config.toml` 的分工 | `config.toml` 保持 `0644`（可分享/可导出），只放**代理名引用**（§2.3/§2.4）；出现凭据键 → `422 unknown_field` |
| Zygisk 侧 | 不读 `egress.toml`；端口/凭据由 companion 解析后下发（§5.1） |

验收断言（t9 执行）：

    adb shell su -c 'ls -l /data/adb/devmask/config/egress.toml'      # -rw------- 1 root root
    adb shell su -c 'ls -ld /data/adb/devmask/config'                 # drwx------ 1 root root
    adb shell su -c 'ls -l /data/adb/devmask/config/config.toml'      # -rw-r--r--（保持 0644）
    adb shell su -c 'grep -c password /data/adb/devmask/config/config.toml'   # 0

### 7.3 约束三：Manager 侧 AES-256-GCM（复用 DeviceKey，AAD 绑 proxy id）

复用 [manager/docs/ARCH.md](../../manager/docs/ARCH.md) §6 的 `devmask-manager.key`（32 字节）与 `crypto.rs`，**不引入第二把密钥**：

| 项 | 冻结值 |
| --- | --- |
| 算法 | AES-256-GCM |
| 密钥 | `<data-dir>/devmask-manager.key` 的 32 字节（与设备 Token 加密同一把） |
| 密文格式 | `"v1:" + BASE64_STANDARD( nonce[12] ‖ ciphertext ‖ tag[16] )`（与 `token_cipher` 完全同构） |
| nonce | 每次加密随机 12 字节（`OsRng`），**绝不重用** |
| AAD | `"egress_proxy:" ‖ proxy_id ‖ ":" ‖ field`，`field ∈ {"username","password"}`（UTF-8 字节）——在「绑定 proxy id」基础上再绑字段名，防止两列密文互换后仍能解密 |
| 列 | `egress_proxies.username_cipher` / `password_cipher`，`TEXT`，可 `NULL`（=未设置） |
| 明文生命周期 | 解密后放 `Zeroizing<String>`，构造请求头/认证对象后立即释放；不跨 await 长存 |
| 密钥丢失 | 与设备 Token 同一后果：代理凭据不可解密 → 只能重填凭据（不提供找回） |
| 解密失败 | **不**降级为「无凭据」静默重试（避免把认证失败伪装成网络故障）。对外分三处表达：job 组装期望态 → 结果码 `decrypt_failed`（`details={deviceId, op:"decrypt", reason:"credential_decrypt_failed", message, detail}`）；`POST /egress-proxies/{id}/probe` → `500 internal_error`（`details.op="decrypt"`）；**设备 Token** 解密失败 → `device_token_invalid`（`details={deviceId, op:"decrypt"}`，GET 与 job 两条路径同码）。详见 §6.12 |
| 迁移 | 密文列只接受 `v1:` 前缀（DDL 有 CHECK）；未知前缀 → 解密失败同上 |

**固有代价：凭据随请求下发（冻结，评审必须接受）**

「总是 PUT」（§6.12）意味着每次 `POST /devices/egress/apply` 都会把**解密后的** `username` / `password`
放进 `PUT /api/v1/egress` 的请求体，走与设备 Token 完全相同的通道（默认 loopback 明文 HTTP + `Authorization: Bearer`；
局域网模式无 TLS，见 [docs/SERVER.md](../SERVER.md) §9）。

- 这是「总是 PUT」的**代价，不是缺陷**：不先解密就无法把轮换后的凭据送到设备，也就无法修复「凭据轮换推不到设备」。
- 边界不变：明文只存在于这次请求的内存里（`Zeroizing<String>`），不落 Manager 日志、不进审计、不回任何响应；
  设备端把它写进 `egress.toml`（0600）。
- 已就位的缓解：默认只绑 loopback / 走 `adb forward`；`bind` 非 loopback 时强制 Token；
  `X-Request-Id` 可把两段日志对齐（两段都不含凭据）。
- 需要更强隔离时用代理侧 **IP 白名单**替代账号密码，或等 P2「设备内本地认证中继」（尚未实现）。

### 7.4 凭据生命周期

| 阶段 | 动作 | 凭据可见性 |
| --- | --- | --- |
| 创建 / 更新（`POST`/`PATCH /egress-proxies`） | 请求体带明文 → 立即加密 → 只把密文写库 | 明文只在这一跳的内存里，`Zeroizing` 清零 |
| 下发（`POST /devices/egress/apply`） | 从库读密文 → 解密 → 组装 `PUT /api/v1/egress` 请求体 → 请求结束释放 | 明文只在 TLS/明文 HTTP 的请求体里（与设备 Token 同理）；**不落 Manager 日志** |
| 设备落盘 | 设备端写 `egress.toml` 0600 | 只在设备端文件里 |
| 设备响应 | 只回 `hasCredentials` | 永不回值 |
| 删除 | `DELETE /egress-proxies/{id}` 删除行（被绑定则 409）；`DELETE /egress` 清空设备端定义 | — |
| 轮换 | 重新 `PATCH` 凭据即重新加密（新 nonce）；旧密文被覆盖 | — |

### 7.5 可执行断言清单（t9 / t10 / t11 / t12 共用）

    # 1) 设备端响应无凭据（把 pass 换成你配置的真值做负向断言）
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress"          | jq -e 'tostring|test("password")|not'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress/proxies"  | jq -e '[.items[]|has("password") or has("username")]|any|not'
    # 2) Manager 响应无凭据
    curl -s -H "Authorization: Bearer $MTOKEN" "$MBASE/egress-proxies" | jq -e '[.items[]|has("password") or has("username")]|any|not'
    # 3) 源码级：DTO 里没有凭据字段（白名单外出现即为缺陷）
    grep -rn "password" server/src/dto.rs manager/src/api/dto.rs | grep -v "///" # 期望无命中
    # 4) 日志/审计无凭据（跑完一轮增删改后）
    adb shell su -c 'grep -ci password /data/adb/devmask/logs/*.log'      # 期望 0
    adb shell su -c 'grep -ci password /data/adb/devmask/logs/api-audit.jsonl'  # 期望 0
    grep -ci password <data-dir>/manager.log 2>/dev/null                        # 期望 0
    # 5) WebUI 只以 type=password 出现
    grep -rn 'password' manager/webui/src webui/src | grep -v 'type="password"' | grep -v 'hasCredentials'

---

## 8. 与需求文档的差异

以下条目是落地时必须补足、或计划书未规定的细节。**没有一条改变 [proxy-support-plan.md](proxy-support-plan.md) 的结论**；冲突列全部为「否」。

| # | 计划书原文 / 位置 | 本契约 | 理由 | 冲突 |
| --- | --- | --- | --- | --- |
| 1 | §9「PUT /egress 支持 If-Match/412」，未定义 egress 的 revision 域 | `egressRevision` = `sha256(configRevisionHex ‖ "\n" ‖ egressTomlBytes)`（§1.2） | 一次写跨两个文件，必须有一个能同时覆盖两者的前置条件；否则只改密码的并发写会静默覆盖 | 否（补足） |
| 2 | 未规定 egress 端点能否复用 `X-DM-Revision`/`ETag` | **禁止**复用；改用 `X-DM-Egress-Revision`（§1.1） | 既有 WebUI 拦截器会在任何响应上缓存 `X-DM-Revision` 作为 config revision，复用会直接导致后续 `PUT /config` 412 | 否（补足，防集成事故） |
| 3 | §8「egress_checks \| id, device_id, app, proxy_id, exit_ip, latency_ms, ok, error_code, checked_at」 | 追加 `source`（`device`/`manager`）、`scope`，且 `device_id` 可空（§6.11） | FR-3 要求 Manager 独立探测，两条来源必须可区分；Manager 直连探测没有设备 | 否（补足） |
| 4 | §8 egress.toml 示例无版本键 | 追加 `version = 1`（缺省视为 1，见 §2.1） | 唯一能安全演进文件结构的办法；缺省兼容 | 否（补足） |
| 5 | FR-3「默认明文 HTTP 服务」，未给默认值 | 默认 `http://checkip.amazonaws.com/`；只接受 `http://`（§4.2/§4.3） | 需要一个可用的零配置默认值；限 http 是 FR-3 的原意（不引 TLS 栈） | 否（补足） |
| 6 | FR-4「DELETE /egress 恢复直连」，未规定确认参数 | 要求 `confirm=true`（缺失 → `400 confirmation_required`，§1.5） | 会删除全部凭据定义，与既有 `DELETE /templates`、`DELETE /apps` 的破坏性操作对齐 | 否（补足） |
| 7 | 决策 #3「认证：支持用户名/密码（Basic）」，未区分层级 | 设备级（方案 A）**不承载认证**，带凭据的代理只走应用级；设备级给 warning `egress_credentials_ignored`（§3.7、§8 本条） | `settings global http_proxy` 只接受 `host:port`，没有认证通道；写死这条能防止「配了密码却以为生效」 | 否（限制补充） |
| 8 | 数据模型只给 `apps[].egress`，未提模板 | `DeviceTemplate` **不支持** `egress`；应用级 egress 只能写在 `[[apps]]`（§2.4），并且会为模板绑定型应用建立内联记录、整记录覆盖模板指纹（写入时给 warning `egress_inline_shadows_template`） | 模板被多个包共享，不是「每应用出口」；内联覆盖是既有配置语义（[docs/API.md](../API.md) §6.3），必须提示 | 否（限制补充） |
| 9 | 未规定 fail-open 的判定时点与动作 | 判定只在「显式探测失败 / 写入后读回不一致 / 启动自愈不一致」三个时点；动作是 `settings put :0`；恢复需一次成功探测（§3.5） | 不做后台常驻探测（不加定时器、不加流量、不加被检测面），同时保证「配错不变砖」 | 否（补足） |
| 10 | FR-3 只写「经当前链路探测」 | 支持 `package` 参数按应用生效代理探测，但在服务进程内完成，响应标 `approximate:true` + warning `egress_probe_approximate`（§1.6） | Zygisk 进程无 IPC 回传，应用级探测在语义上只能是近似；必须显式标注，避免误当真实链路 | 否（补足） |
| 11 | 未规定 Manager 探测失败的表达 | 失败返回 `502`（设备端 `egress.probe_failed` / Manager `egress_probe_failed`），且**仍写** `egress_checks(ok=0)`（§6.7） | 两侧口径统一；诊断数据不因失败而丢失 | 否（补足） |
| 12 | 未规定 `jobs.kind` 扩容方式 | `jobs` 表重建配方（关闭 FK → 建新表 → 搬行 → 改名 → 重建索引 → `foreign_key_check`），见 §6.11 | SQLite 无法 `ALTER CHECK`；FK 开启时 `DROP` 会级联删历史结果（真实事故） | 否（实现细节冻结） |
| 13 | 未规定幂等短路口径 | 设备端：「期望态 == 现状 → 不写盘、不备份、`changed:false`、revision 不变」（§1.4、§1.9）。Manager 端：**不做本地比较**，每次 apply 都 PUT，由设备返回的 `changed` 决定步骤 `egress_put` 记 `ok` 还是 `skipped/egress_unchanged`，结果行统一 `ok` + `code="egress_unchanged"`（§6.12） | 落实 P0 验收第 5 条「重复 PUT 不重复写」，同时让凭据轮换与设备侧漂移能被下一次 apply 收敛；避免幂等重放把 job 变成 `partial` | 否（补足，t29 起） |
| 14 | 错误码表未涉及 | 设备端新增 2 个点分错误码 + 8 个扁平警告码；Manager 新增 3 个扁平错误码 + 5 个警告码 + 1 个结果码（§1.8、§6.12） | 需求文档只给了 `egress.probe_failed`；其余码必须一次冻结，否则 6 个实现任务各造一套 | 否（补足） |

---

## 附录 A 落地映射与验收命令

### A.1 实现落点（非契约，仅供拆分任务）

| 任务 | 建议落点 | 负责章节 |
| --- | --- | --- |
| t2 | `core/src/config.rs`（`Config.egress`、`AppConfig.egress`）、`core/src/validate.rs`、`core/src/edit.rs`、`core/src/schema.rs` | §2.3–§2.7 |
| t3 | `server/src/egress/`（`mod.rs` / `store.rs` / `system.rs` / `probe.rs` / `dto.rs`）、`server/src/api/egress.rs`、`server/src/commands.rs`（扩 `settings`/`dumpsys`）、`module/uninstall.sh` | §1、§2.1–§2.2、§3、§4、§7.2 |
| t4 | Zygisk 侧注入（`src/` 下新模块） | §5 |
| t5 | `manager/src/db.rs`（v2 迁移）、`manager/src/api/egress.rs`、`manager/src/jobs.rs`（egress kind）、`manager/src/crypto.rs`（AAD 变体） | §6、§7.3 |
| t6 / t7 | `manager/webui/src/`、`webui/src/` | §1.3、§6.2/§6.4、§7.1 |

> 注意重名：`server/src/settings.rs` 已存在，是 **`server.toml` 的解析模块**，与 Android `settings` 命令无关。Android 侧命令封装请放在 `server/src/egress/system.rs` 或 `commands.rs`，不要复用 `settings.rs` 之名。

### A.2 设备端冒烟（curl，t9 用）

    BASE=http://127.0.0.1:8686/api/v1
    TOKEN=admin123

    # 1) 读现状（拿 egress revision）
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress"       | jq '{revision,configRevision,configured,mode,effectiveMode,default,applied,readback,health}'

    # 2) 写入：整体替换 proxies + 设备级 default（期望 applied=true）
    REV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$REV\"" \
      -H 'Content-Type: application/json' \
      -d '{"default":"us_la_01","failOpen":true,"probeUrl":"http://checkip.amazonaws.com/","proxies":[{"name":"us_la_01","type":"http","host":"203.0.113.10","port":1080}]}' \
      "$BASE/egress" | jq '{changed,applied,readback,revision,warnings}'

    # 3) 设备侧读回（人工断言）
    adb shell su -c 'settings get global http_proxy'                 # 203.0.113.10:1080
    adb shell su -c 'dumpsys connectivity | grep -i "global proxy"'   # Global proxy: [203.0.113.10] port: 1080

    # 4) 幂等：用**新** revision 重放同一份内容 → changed=false、revision 不变
    REV2=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$REV2\"" \
      -H 'Content-Type: application/json' \
      -d '{"default":"us_la_01","failOpen":true,"probeUrl":"http://checkip.amazonaws.com/","proxies":[{"name":"us_la_01","type":"http","host":"203.0.113.10","port":1080}]}' \
      "$BASE/egress" | jq '{changed,revision}'                        # changed=false

    # 5) 412：拿步骤 2 的旧 revision 再写 → 412
    curl -s -o /dev/null -w '%{http_code}\n' -X PUT -H "Authorization: Bearer $TOKEN" \
      -H "If-Match: \"$REV\"" -H 'Content-Type: application/json' -d '{"default":null}' "$BASE/egress"

    # 6) 探测（设备级）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{}' "$BASE/egress/probe" | jq .
    # 未配置时：409 egress.not_configured；代理挂了：502 egress.probe_failed

    # 7) 应用级覆盖 + 强制直连
    REV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$REV\"" \
      -H 'Content-Type: application/json' \
      -d '{"apps":[{"package":"com.example.b","egress":"__DIRECT__"}]}' "$BASE/egress" | jq '.apps,.warnings'

    # 8) 清除（必须 confirm=true），并确认 Global proxy: none
    REV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)
    curl -s -X DELETE -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$REV\"" \
      "$BASE/egress?confirm=true" | jq '{changed,mode,applied,readback}'
    adb shell su -c 'dumpsys connectivity | grep -i "global proxy"'   # Global proxy: none
    adb shell su -c 'settings get global http_proxy'                  # :0（或 null）

### A.3 Manager 冒烟（curl，t10 用）

    MBASE=http://127.0.0.1:8765/relay/api/v1
    MTOKEN=admin123

    # 1) 建池（密码只写不读）
    PID=$(curl -s -X POST -H "Authorization: Bearer $MTOKEN" -H 'Content-Type: application/json' \
      -d '{"name":"us_la_01","type":"http","host":"203.0.113.10","port":1080,"username":"u","password":"p","region":"US-LA","tags":["us"]}' \
      "$MBASE/egress-proxies" | jq -r .id)
    curl -s -H "Authorization: Bearer $MTOKEN" "$MBASE/egress-proxies/$PID" | jq '{name,hasCredentials,bindings,lastCheck}'

    # 2) Manager 直连探测
    curl -s -X POST -H "Authorization: Bearer $MTOKEN" "$MBASE/egress-proxies/$PID/probe" | jq .

    # 3) 绑定（只落库）+ 批量下发（返回 job，kind=egress）
    curl -s -X PUT -H "Authorization: Bearer $MTOKEN" -H 'Content-Type: application/json' \
      -d '{"bindings":[{"app":"","proxyId":"'$PID'"},{"app":"com.example.b","proxyId":"__DIRECT__"}]}' \
      "$MBASE/devices/pixel-9/egress" | jq '{changed,pendingApply,bindings}'
    JOB=$(curl -s -X POST -H "Authorization: Bearer $MTOKEN" -H 'Content-Type: application/json' \
      -d '{"devices":"all","probe":true}' "$MBASE/devices/egress/apply" | jq -r .jobId)
    curl -s -H "Authorization: Bearer $MTOKEN" "$MBASE/jobs/$JOB" \
      | jq '{status,kind,total,succeeded,skipped,failed,results:[.results[]|{deviceId,app,status,code,steps}]}'

    # 4) 被绑定则删除拒绝
    curl -s -o /dev/null -w '%{http_code}\n' -X DELETE -H "Authorization: Bearer $MTOKEN" "$MBASE/egress-proxies/$PID"   # 409

    # 5) 幂等重放：同内容再 apply，结果应为 ok + step code=egress_unchanged，job=succeeded
    # 6) 解绑后删除 → 204
    curl -s -X PUT -H "Authorization: Bearer $MTOKEN" -H 'Content-Type: application/json' \
      -d '{"bindings":[]}' "$MBASE/devices/pixel-9/egress" >/dev/null
    curl -s -o /dev/null -w '%{http_code}\n' -X DELETE -H "Authorization: Bearer $MTOKEN" "$MBASE/egress-proxies/$PID"   # 204

### A.4 三条硬约束的验收（§7.5 汇总）

见 §7.5：响应无凭据、文件 0600、密文格式与 AAD、日志/审计 0 命中。

---

## 附录 B 版本收口输入（供 t8，非本契约强制）

| 项 | 当前 | 本增量后 | 说明 |
| --- | --- | --- | --- |
| 设备端 `/api/v1` 端点数 | 24 | **29** | +`GET/PUT/DELETE /egress`、`POST /egress/probe`、`GET /egress/proxies`；`CONTRACT_OPERATIONS` 常量与契约测试同步 +5，`CAPABILITIES` 追加 `egress` |
| 设备端 `devmask_server` 版本 | 1.9.0 | **1.10.0** | 新增端点属可选增量，`/api/v1` 前缀不变 |
| Manager `/relay/api/v1` 端点数 | 18 | **27** | +9 个方法级操作 |
| Manager 版本 | 0.1.0 | **0.2.0** | `/healthz` 的 `db.schemaVersion` → `2` |
| SQLite schema | v1 | **v2** | 3 新表 + `jobs` 重建（§6.11） |
| 需同步的文档 | — | `docs/API.md`（§4 总表 +5、§5 新增 5 小节、§7.2 错误码 +2、§8.2 校验码 +4、§13 变更记录）、`docs/CONFIG.md`（`[egress]`、`[[apps]].egress`、`egress.toml` 与 0600、卸载前清代理）、`docs/SERVER.md`、`manager/docs/API.md`（§3 总表 +9、§5.7 枚举、§2.7/§2.8 新码、§5.11 DDL 附注）、`manager/docs/ARCH.md`（§5.2 增 v2、§6.2 AAD 变体）、`docs/en/*`、`docs/tr/*`、`CHANGELOG.md` | 三语同改 |

## 附录 C 变更记录

| 版本 | 日期 | 变更 |
| --- | --- | --- |
| egress contract v1 | 2026-11-24 | 首版冻结：设备端 5 端点 + Manager 9 操作、`egress.toml` 0600、方案 A `:0` 清除语义、探测契约、应用级注入清单、SQLite v2 三表、job kind=egress、凭据三条硬约束 |
| v1.1 | 2026-12-01 | 同步 t29 的下发语义：Manager **总是 PUT**、由设备 `changed` 判定（§6.12 流水线与 §8 #13 改写，补「每次 apply 一次设备写入 / 需要 write scope」的代价与前提）；补 §1.8 `egress_auth_unsupported` / `egress_probe_scope_app`、`settings_unavailable`、备份命名与 diff 脱敏（t20/t23 行为） |
| v1.2 | 2026-12-01 | 补登记 Manager 侧 `decrypt_failed` / `device_token_invalid` / `egress_check_not_recorded` 三个码（§6.12 结果码/警告表、§7.3 解密失败表述）；写明「每次 apply 把解密后的凭据放进设备请求体」的固有代价（§7.3）；§1.8 加 Manager 侧码位指引 |
