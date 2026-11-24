# Device Faker 原生 Web 服务与 AI API 计划书

- 文档版本：v0.1（草案）
- 日期：2026-11-24
- 适用代码：device_faker v1.7.0（当前工作区）
- 关联文档：docs/README.md、docs/CONFIG.md、docs/API.md（待新增）
- 状态：待评审

---

## 1. 摘要

目标是把现在「只能由 KernelSU / Magisk 管理器 WebView 打开、能力全部依赖 window.ksu」的 WebUI，升级为模块自带的 HTTP 服务，并在此之上提供一套面向 AI 与自动化的稳定 API。

三条主线：

1. **原生 Web**：模块内置常驻进程 device_faker_server，直接托管 module/webroot，手机浏览器或局域网浏览器打开即可完整管理，不依赖管理器 WebView。
2. **AI API**：同一进程暴露版本化 REST API（/api/v1）、OpenAPI 3.1 描述、JSON Schema、llms.txt，并提供 MCP（Model Context Protocol）端点，AI 客户端可以直接把它当工具集使用。
3. **双形态兼容**：现有管理器模式（ksu 桥接）继续可用；WebUI 增加 HTTP 传输层，按运行环境自动切换，不维护两份前端。

预期结果：

- 手机本机打开 http://127.0.0.1:8686 即可管理全部配置；
- 电脑通过 adb forward 或局域网访问同一页面；
- AI 通过 MCP 或 REST 完成「查状态 → 建模板 → 绑定包名 → 预演 diff → 应用 → 重启应用 → 验证」的闭环；
- 所有写操作可审计、可回滚、带版本号防并发覆盖。

工期预估：约 15 到 18 人日（P0 到 P4 合计约 14.5 人日，另计约 20% 缓冲；单人全职约 3 周，含测试与文档）。

---

## 2. 背景与现状

### 2.1 当前链路

- **Zygisk 模块**：Rust cdylib（src/lib.rs），在 zygote 中注入，按包名伪装 Build 字段、COW 属性，并通过 companion 进程做 resetprop 与密度管理。
- **配置**：唯一事实来源是 /data/adb/device_faker/config/config.toml，TOML 格式，模板（templates）+ 应用内联配置（apps）+ 全局开关。
- **配置生效**：companion 把配置预渲染成 per-package TOML 放在内存，config.toml 的 mtime 变化时重渲染（src/spec.rs）；目标应用下一次启动即拿到新 spec。
- **WebUI**：webui/ 是 Vue 3 + Vite 单页应用，构建产物输出到 module/webroot，由管理器以本地页面加载。
- **能力来源**：WebUI 不直接读文件、不直接跑命令，全部通过管理器注入的 window.ksu（kernelsu-alt 封装）：exec、io、pm、getPackagesInfo、moduleInfo、toast。
- **服务端**：没有。仓库中不存在任何 HTTP 服务、端口监听或 API。
- **日志**：/data/adb/device_faker/logs/device_faker.log。

### 2.2 痛点

1. WebUI 只能在管理器里用，桌面浏览器、平板、无管理器环境都用不了。
2. AI 与自动化工具没有稳定的机器接口，只能模拟点击或直接改 TOML 文件，缺少校验、并发保护与可观测性。
3. 现有社区做法（包括本仓 tools/webui_remote 的 host 侧桥接）依赖电脑 + adb，属于开发辅助，不适合作为模块的正式能力。
4. 配置写入没有版本号、没有锁、没有审计，多端同时编辑会静默覆盖。

### 2.3 约束

- 模块运行在 Android 设备上，服务必须是原生二进制（不能依赖 Node/Python 运行时）。
- 启动入口只有模块脚本：service.sh（Magisk / KernelSU 在 late_start service 阶段以 root 执行）。
- 需要同时兼容管理器模式，不能破坏现有 ksu 路径。
- 配置写入必须与 companion 的 mtime 重渲染机制配合，不能引入第二套生效链路。
- 模块当前只构建 arm64-v8a，服务保持同一 ABI 范围。

### 2.4 已完成的可行性验证（本机 Pixel 9 / Android 17 / KernelSU 域）

- 在 ksu 域内绑定 TCP 端口成功（toybox nc 监听 9999，adb forward 可访问）。
- 从 /data/adb/modules/device_faker 执行二进制成功（上下文 u:object_r:system_file:s0）。
- 配置目录可写，写入后文件上下文为 u:object_r:system_file:s0，权限可控。
- 结论：service.sh 启动原生服务、监听端口、读写 /data/adb 在目标环境可行；Magisk 环境需在测试矩阵中复验。

---

## 3. 目标与非目标

### 3.1 目标

- G1：模块自带 Web 服务，直接托管 WebUI，管理器不再是必需条件。
- G2：提供 /api/v1 REST API，覆盖配置、模板、应用、设备信息、日志、备份。
- G3：提供 OpenAPI 3.1、JSON Schema、llms.txt，让 AI 无需人工文档即可发现与调用。
- G4：提供 MCP 端点与 stdio 桥接，兼容主流 AI 客户端。
- G5：所有写操作具备版本号（ETag / revision）、原子替换、备份、审计日志。
- G6：管理器模式零回归，WebUI 单一构建产物同时支持两种传输。

### 3.2 非目标

- 不提供任意 shell 执行 API；只提供语义化、白名单化的动作。
- 不在本期实现账号体系、多租户、远程公网托管。
- 不改变 Zygisk 注入逻辑与伪装算法，只增加控制面。
- 不把 WebUI 重写为服务端渲染；仍为静态 SPA。

---

## 4. 总体架构

    ┌───────────────────────────── Android 设备 ─────────────────────────────┐
    │                                                                        │
    │  service.sh (root)                                                     │
    │      └─ device_faker_server  (Rust / axum)                             │
    │            ├─ 静态资源：/data/adb/modules/device_faker/webroot         │
    │            ├─ REST /api/v1/*                                           │
    │            ├─ MCP   /mcp                                               │
    │            ├─ 认证 / 审计 / 限流 / 备份                                 │
    │            └─ 配置仓库（toml_edit + flock + revision）                  │
    │                     │                                                  │
    │                     ▼                                                  │
    │            /data/adb/device_faker/config/config.toml                   │
    │                     │ mtime 变化                                        │
    │                     ▼                                                  │
    │  zygote / companion：重渲染 per-package spec → 目标应用启动时生效        │
    │                                                                        │
    │  管理器 WebView ── window.ksu ── 旧链路（保留）                         │
    └────────────────────────────────────────────────────────────────────────┘
                ▲                                   ▲
                │ http://127.0.0.1:8686             │ Bearer Token
       手机浏览器 / 桌面浏览器                  AI 客户端（MCP / REST）
                ▲
                │ adb forward tcp:8686 tcp:8686（推荐，不暴露局域网）
                │ 或局域网 http://<手机IP>:8686（显式开启）

### 4.1 组件划分

| 组件 | 位置 | 职责 |
| --- | --- | --- |
| device_faker_server | 新增 crate server/ | HTTP、静态托管、REST、MCP、认证、审计 |
| device_faker_core | 新增 crate core/ | 配置模型、校验、合并、TOML 读写、revision |
| zygisk 模块 | src/ | 复用 core，行为不变 |
| WebUI | webui/ | 新增传输抽象，支持 ksu 与 HTTP |
| service.sh | module/ | 开机启动、崩溃拉起、日志轮转 |
| MCP stdio 桥 | tools/mcp_stdio_bridge.mjs | 给只支持 stdio 的 AI 客户端用 |
| 契约测试 | tests/api/ | OpenAPI 校验、并发、鉴权、兼容 |

### 4.2 两种运行形态

| 形态 | 入口 | 能力来源 | 适用 |
| --- | --- | --- | --- |
| 管理器模式（保留） | 管理器 WebView 加载 webroot | window.ksu | 现有用户，零改动 |
| 原生服务模式（新增） | 浏览器访问设备 8686 端口 | /api/v1 | 桌面、平板、AI、自动化 |

WebUI 启动时探测：存在 window.__DF_SERVER__ 或 /api/v1/healthz 可达 → HTTP 传输；存在 window.ksu → ksu 传输；都没有 → 只读提示。

### 4.3 配置写入与生效链路

1. 客户端调用写接口，携带 If-Match（当前 revision）。
2. 服务端加 flock 排他锁，读取当前文件，计算 revision，比对。
3. 用 toml_edit 做定点修改，保留注释与未知字段，生成新文本。
4. 校验：语法 + 语义（包名、dpi、sdk、重复模板等）。
5. 备份当前文件，写临时文件 → fsync → rename → fsync 目录 → 设权限 644。
6. 返回新 revision 与 diff。
7. companion 下次 GetSpec 时发现 mtime 变化，重渲染内存 spec。
8. 目标应用重启后生效（服务端提供 restart 接口，可显式触发）。

### 4.4 访问路径

| 场景 | 方式 | 说明 |
| --- | --- | --- |
| 手机本机浏览器 | http://127.0.0.1:8686 | 默认绑定，最安全 |
| 电脑调试 / AI | adb forward tcp:8686 tcp:8686 | 推荐，不暴露网络 |
| 局域网设备 | http://手机IP:8686 | 必须在 server.toml 显式改 bind=0.0.0.0 并启用 token |
| 公网 | 不建议直连 | 需要时用 Tailscale / 反代 + TLS |

---

## 5. API 设计

### 5.1 基本原则

- 前缀 /api/v1，版本只在大版本不兼容时递增；新增字段不升版本。
- 请求与响应均为 JSON（配置原文接口可选 TOML）。
- 所有资源带 revision（当前配置文件 sha256），写操作支持 If-Match。
- 错误统一信封，稳定错误码，AI 可据此分支处理。
- 所有列表接口支持 limit / cursor；默认 limit=50，最大 500。
- 变更类接口支持 dry_run=true 或独立 plan 接口，返回 diff 与 warnings。
- 幂等：POST 支持 Idempotency-Key，24 小时内重复键返回首次结果。
- 响应带 X-Request-Id、X-DF-Revision，便于追踪与条件写。

### 5.2 认证与权限

- 认证：Authorization: Bearer <token>；MCP 同样。
- Token 存于 /data/adb/device_faker/config/server.toml 的 tokens 表，落盘只存 sha256，明文仅在创建时返回一次。
- 作用域：
  - read：读取状态、配置、模板、应用、日志。
  - write：增删改模板、应用、配置。
  - ops：应用/重启、备份恢复、批量应用。
  - admin：token 管理、服务端设置、导入导出。
- 浏览器登录：首次访问 API 返回 401，WebUI 弹 Token 输入框，存 sessionStorage；也可勾选记住到 localStorage。
- 不做 Cookie 会话（避免 CSRF 面），前端统一 Bearer 头。
- 默认绑定 127.0.0.1；改为 0.0.0.0 时若没有配置 token，服务拒绝启动并在日志给出生成命令。

### 5.3 端点总表

系统：

| 方法 | 路径 | 作用 | 作用域 |
| --- | --- | --- | --- |
| GET | /api/v1/healthz | 存活探针 | 无 |
| GET | /api/v1/status | 模块/设备/服务/配置状态 | read |
| GET | /api/v1/capabilities | 能力与限制发现 | read |
| GET | /api/v1/openapi.json | OpenAPI 3.1 | 无 |
| GET | /api/v1/schema/config | 配置 JSON Schema | read |
| GET | /api/v1/guide | llms.txt 风格使用指南 | 无 |
| GET | /api/v1/events | SSE：配置变更、日志、审计 | read |

配置：

| 方法 | 路径 | 作用 | 作用域 |
| --- | --- | --- | --- |
| GET | /api/v1/config | 读取配置（json 或 toml） | read |
| PUT | /api/v1/config | 整体替换（If-Match 必填） | write |
| POST | /api/v1/config/validate | 校验文本，返回 errors/warnings | read |
| POST | /api/v1/config/reload | 触发 revision 重算与缓存失效 | ops |
| GET | /api/v1/config/revision | 仅取 revision 与 mtime | read |
| GET | /api/v1/backups | 备份列表 | read |
| POST | /api/v1/backups | 立即备份 | ops |
| POST | /api/v1/backups/{id}/restore | 恢复备份（需 confirm） | ops |
| DELETE | /api/v1/backups/{id} | 删除备份 | ops |

模板：

| 方法 | 路径 | 作用 | 作用域 |
| --- | --- | --- | --- |
| GET | /api/v1/templates | 列表（含包名数量、摘要） | read |
| GET | /api/v1/templates/{name} | 详情 | read |
| PUT | /api/v1/templates/{name} | 新建或整体更新 | write |
| PATCH | /api/v1/templates/{name} | 字段级合并 | write |
| DELETE | /api/v1/templates/{name} | 删除（需 confirm） | write |
| POST | /api/v1/templates/{name}/clone | 复制为新模板 | write |
| POST | /api/v1/templates/{name}/packages | 批量绑定包名 | write |
| DELETE | /api/v1/templates/{name}/packages/{pkg} | 解绑单个包名 | write |
| POST | /api/v1/templates/import | 导入 TOML/JSON | write |
| GET | /api/v1/templates/{name}/export | 导出 TOML/JSON | read |

应用与设备：

| 方法 | 路径 | 作用 | 作用域 |
| --- | --- | --- | --- |
| GET | /api/v1/apps | 应用列表（已配置 + 可选已安装） | read |
| GET | /api/v1/apps/{pkg} | 单个应用解析结果与来源 | read |
| PUT | /api/v1/apps/{pkg} | 设置内联配置或绑定模板 | write |
| DELETE | /api/v1/apps/{pkg} | 移除应用配置 | write |
| GET | /api/v1/apps/{pkg}/explain | 解释将伪装哪些字段与属性 | read |
| POST | /api/v1/apps/{pkg}/restart | force-stop，可选重新拉起 | ops |
| GET | /api/v1/packages | 已安装包元数据（多用户） | read |
| GET | /api/v1/packages/{pkg}/label | 应用名（P4） | read |
| GET | /api/v1/packages/{pkg}/icon | 图标 PNG（P4） | read |
| GET | /api/v1/device | 设备与模块信息 | read |
| GET | /api/v1/device/props | getprop 白名单读取 | read |
| GET | /api/v1/device/users | 用户列表 | read |

AI 与运维：

| 方法 | 路径 | 作用 | 作用域 |
| --- | --- | --- | --- |
| POST | /api/v1/plan | 计算变更计划与 diff，不落盘 | read |
| POST | /api/v1/apply | 执行计划（planId 或 operations） | write |
| POST | /api/v1/simulate | 预演模板解析出的 Build/属性 | read |
| POST | /api/v1/batch | 最多 20 个子请求，逐项结果 | 取各项 |
| GET | /api/v1/conflicts | 同一包名多模板冲突 | read |
| GET | /api/v1/logs | 模块日志尾部 | read |
| GET | /api/v1/audit | 审计日志（JSONL） | admin |
| GET | /api/v1/tokens | Token 列表（不含明文） | admin |
| POST | /api/v1/tokens | 新建 Token | admin |
| DELETE | /api/v1/tokens/{id} | 撤销 Token | admin |

### 5.4 关键端点细节

**GET /api/v1/status**

    200 OK
    {
      "module": {"id": "device_faker", "version": "v1.7.0", "versionCode": 20261109},
      "device": {"model": "Pixel 9", "android": "17", "sdk": 37, "abi": "arm64-v8a"},
      "server": {"version": "1.0.0", "uptimeSec": 3600, "bind": "127.0.0.1:8686"},
      "config": {
        "revision": "sha256:9f2c...",
        "mtime": "2026-11-24T03:00:00Z",
        "size": 9361,
        "templates": 26,
        "apps": 27,
        "conflicts": 0
      },
      "capabilities": ["templates", "apps", "plan", "mcp", "backups"]
    }

**PUT /api/v1/templates/{name}**

    If-Match: "sha256:9f2c..."
    Content-Type: application/json
    {
      "packages": ["com.example.app"],
      "brand": "samsung",
      "manufacturer": "samsung",
      "model": "SM-S9280",
      "device": "e3q",
      "android_version": "15",
      "sdk_int": 35,
      "dpi": 420,
      "custom_props": {"ro.product.vendor.model": "SM-S9280"}
    }

    200 OK
    {
      "revision": "sha256:1a7b...",
      "template": {"name": "samsung_s24_ultra", "...": "..."},
      "warnings": []
    }

**POST /api/v1/plan**

    {
      "baseRevision": "sha256:9f2c...",
      "operations": [
        {"op": "template.upsert", "name": "pixel_9", "value": {"model": "Pixel 9", "brand": "google"}},
        {"op": "template.bind", "name": "pixel_9", "packages": ["com.example.app"]},
        {"op": "app.set", "package": "com.example.two", "template": "pixel_9"},
        {"op": "app.delete", "package": "com.example.old"}
      ]
    }

    200 OK
    {
      "planId": "plan_01JC...",
      "baseRevision": "sha256:9f2c...",
      "expiresAt": "2026-11-24T03:10:00Z",
      "diff": "--- a/config.toml\n+++ b/config.toml\n@@ ...",
      "operations": [{"op": "template.upsert", "status": "ok"}],
      "warnings": ["com.example.two 尚未安装，配置仍会保留"]
    }

**POST /api/v1/apply**

    {"planId": "plan_01JC...", "confirm": true}

    200 OK
    {"revision": "sha256:77aa...", "applied": 4, "backupId": "20261124T031002Z-9f2c"}

**GET /api/v1/apps/{pkg}/explain**

    200 OK
    {
      "package": "com.example.app",
      "installed": true,
      "source": {"type": "template", "name": "pixel_9", "alsoInline": false},
      "resolved": {
        "brand": "google",
        "model": "Pixel 9",
        "fingerprint": "google/tokay/tokay:15/...",
        "dpi": 420
      },
      "properties": [
        {"key": "ro.product.model", "value": "Pixel 9", "mode": "cow"},
        {"key": "ro.build.fingerprint", "value": "google/...", "mode": "companion"}
      ],
      "warnings": []
    }

### 5.5 错误信封与错误码

    {
      "error": {
        "code": "config_revision_mismatch",
        "message": "配置已被其他客户端修改",
        "details": {"expected": "sha256:9f2c...", "actual": "sha256:1a7b..."},
        "retryable": false,
        "requestId": "req_01JC..."
      }
    }

| 错误码 | HTTP | 含义 |
| --- | --- | --- |
| unauthorized | 401 | 缺少或无效 Token |
| forbidden_scope | 403 | Token 作用域不足 |
| not_found | 404 | 资源不存在 |
| validation_failed | 422 | 配置语义校验失败，details 含字段错误 |
| config_revision_mismatch | 412 | If-Match 不匹配 |
| plan_expired | 410 | 计划过期 |
| conflict_duplicate_package | 409 | 同一包名被多个模板声明且需显式确认 |
| rate_limited | 429 | 触发限流，带 Retry-After |
| body_too_large | 413 | 请求体超限 |
| internal_error | 500 | 未预期错误，含 requestId |

### 5.6 并发与一致性

- 读：共享锁 + 读文件；revision 为文件字节 sha256。
- 写：flock 排他 + 临时文件 + rename + fsync 目录，保证不出现半截文件。
- 条件写：If-Match 不匹配返回 412，不自动合并；客户端需重新读取。
- 管理器 WebUI 仍可直接写文件（旧路径），可能绕过锁；API 侧通过 revision 检测并在响应中提示外部变更。P2 起，原生模式 WebUI 一律走 API，只有管理器模式保留直写。
- 备份：每次成功写入前自动备份，保留最近 10 份；支持手动备份与恢复。
- 幂等键：POST 写操作支持 Idempotency-Key，避免 AI 重试造成重复写入。

### 5.7 审计

- 路径：/data/adb/device_faker/logs/api-audit.jsonl。
- 字段：ts、requestId、tokenId、tokenName、remote、method、path、status、revisionBefore、revisionAfter、durationMs、userAgent。
- 敏感值（Token、完整配置内容）不落审计。
- 日志轮转：单文件 5MB，保留 3 份。

### 5.8 契约与文档产物

- /api/v1/openapi.json：由 utoipa 从代码生成，构建期产出 docs/openapi.json。
- /api/v1/schema/config：schemars 从 core 的 Config 结构生成。
- /llms.txt 与 /api/v1/guide：面向 AI 的自然语言说明，包含认证、常用流程、错误处理、限制。
- 文档站新增 docs/API.md（人读）与 docs/MCP.md（AI 客户端配置）。

---

## 6. AI 集成设计

### 6.1 双通道

- **MCP**：给支持 Model Context Protocol 的客户端（Claude Desktop、Cursor、IDE Agent）直接挂载工具。
- **REST + OpenAPI**：给通用 Agent 框架、脚本、函数调用使用；OpenAPI 可直接转成函数 schema。
- 两者共享同一套内部服务层，行为一致，避免两套逻辑。

### 6.2 MCP 工具清单

| 工具名 | 作用 | 只读 | 破坏性 |
| --- | --- | --- | --- |
| df_get_status | 模块、设备、配置 revision | 是 | 否 |
| df_list_templates | 模板列表 | 是 | 否 |
| df_get_template | 模板详情 | 是 | 否 |
| df_upsert_template | 新建/更新模板 | 否 | 否 |
| df_delete_template | 删除模板 | 否 | 是 |
| df_list_apps | 应用列表与配置状态 | 是 | 否 |
| df_get_app | 单应用解析结果 | 是 | 否 |
| df_set_app | 绑定模板或写内联配置 | 否 | 否 |
| df_remove_app | 移除应用配置 | 否 | 是 |
| df_explain_app | 解释伪装字段与属性来源 | 是 | 否 |
| df_simulate | 预演模板解析结果 | 是 | 否 |
| df_plan | 生成变更计划与 diff | 是 | 否 |
| df_apply | 执行计划 | 否 | 是 |
| df_restart_app | 重启目标应用 | 否 | 是 |
| df_validate_config | 校验配置文本 | 是 | 否 |
| df_list_conflicts | 冲突检查 | 是 | 否 |
| df_get_device | 设备与支持属性 | 是 | 否 |
| df_read_logs | 模块日志尾部 | 是 | 否 |
| df_export_config | 导出 TOML/JSON | 是 | 否 |
| df_import_config | 导入配置 | 否 | 是 |
| df_get_schema | 配置 JSON Schema | 是 | 否 |

MCP 资源：

- device_faker://config（当前配置）
- device_faker://templates/{name}
- device_faker://schema/config
- device_faker://guide

MCP 提示模板：

- create_spoof_profile：按机型、Android 版本、目标包名生成模板并绑定。
- diagnose_missing_spoof：读取 explain 与日志，定位为什么没有生效。

### 6.3 传输

- HTTP：POST /mcp，JSON-RPC 2.0；响应 JSON；支持 Mcp-Session-Id；GET /mcp 提供 SSE 通知（可选）。
- stdio：tools/mcp_stdio_bridge.mjs 负责把 stdio 转发到 http://127.0.0.1:8686/mcp，带 Token；供只支持 stdio 的客户端使用。
- 协议版本：固定 2025-06-18，服务端做版本协商。

### 6.4 典型 AI 工作流

1. df_get_status → 记录 revision。
2. df_list_templates → 判断是否已有可复用模板。
3. df_upsert_template 或 df_simulate → 预览解析出的 Build 字段。
4. df_plan → 得到 diff 与 warnings。
5. 人工确认或策略允许后 df_apply。
6. df_restart_app（force-stop，可选拉起）。
7. df_explain_app 与 df_read_logs 验证。
8. 失败时 df_list_conflicts / df_validate_config 定位。

### 6.5 AI 友好性要求

- 所有字段有 JSON Schema 描述与枚举，避免自由文本猜测。
- 错误码稳定，details 给出字段级原因与修复建议。
- plan 与 apply 分离，默认不自动执行破坏性操作。
- 输出中带 revision，便于多轮会话判断状态是否变化。
- 提供只读模式 Token，方便把查询能力交给不受信任的 Agent。
- 支持 XML/JSON 双格式说明（guide 中给出），减少模型解析负担。

---

## 7. WebUI 原生支持

### 7.1 传输抽象

新增 webui/src/utils/backend.ts，定义统一接口：

| 方法 | ksu 实现 | HTTP 实现 |
| --- | --- | --- |
| getRuntime | window.ksu 存在 | __DF_SERVER__ 存在 |
| readConfigRaw | io/cat | GET /api/v1/config?format=toml |
| writeConfigRaw | io/heredoc | PUT /api/v1/config |
| listPackages | pm.listPackages | GET /api/v1/packages |
| getPackagesInfo | pm.getPackagesInfo | GET /api/v1/packages?names= |
| getModuleInfo | ksu.moduleInfo | GET /api/v1/status |
| getProps | exec getprop | GET /api/v1/device/props |
| openDeepLink | exec am start | POST /api/v1/intents/open |
| fetchOnlineTemplates | exec curl | 浏览器 fetch 或 GET /api/v1/online/fetch |

现有 utils/ksu.ts 拆为 backend/ksu.ts 与 backend/http.ts，stores 只依赖接口。

### 7.2 原生模式识别与登录

- 服务端在 index.html 注入：window.__DF_SERVER__ = { api: "/api/v1", version: "1" }。
- 前端启动时若有该对象，走 HTTP；首次 API 调用 401 时弹出 Token 输入框。
- Token 存 sessionStorage（默认）或 localStorage（勾选记住）。
- 设置页展示：服务版本、绑定地址、revision、当前 Token 作用域、退出登录。

### 7.3 同源部署

- 静态资源由同一进程托管，无需 CORS。
- index.html 不缓存，assets 长缓存（文件名带哈希）。
- 服务端注入的运行时配置不包含 Token，避免泄露。

### 7.4 兼容矩阵

| 能力 | 管理器模式 | 原生服务模式 |
| --- | --- | --- |
| 模板/应用读写 | 直写文件 | API + revision |
| 应用列表 | pm | API packages |
| getprop | exec | API props |
| 在线模板 | exec curl | fetch 或代理 |
| 深链 | exec am start | API intents |
| 模块升级 | 管理器 | 管理器或 API（P4） |
| 无管理器环境 | 不可用 | 完整可用 |

---

## 8. 设备端服务实现

### 8.1 技术选型

- HTTP：axum 0.8 + tokio（net、rt-multi-thread、signal）。
- 中间件：tower-http（trace、compression、limit、fs）。
- 配置编辑：toml_edit（保留注释与键顺序）。
- 序列化/校验：serde、serde_json、schemars。
- OpenAPI：utoipa（derive），构建期导出 JSON。
- 哈希/锁：sha2、fs2（flock）。
- 日志：tracing + tracing-subscriber，输出 JSON 或文本，重定向到文件。
- 不引入数据库；状态全部来自文件与进程内缓存。

### 8.2 进程与启动

module/service.sh（新增）：

    #!/system/bin/sh
    MODDIR=$(dirname "$0")
    RUNTIME=/data/adb/device_faker
    mkdir -p "$RUNTIME/logs"
    until [ "$(getprop sys.boot_completed)" = "1" ]; do sleep 2; done
    [ -f "$RUNTIME/config/server.toml" ] || cp "$MODDIR/server.default.toml" "$RUNTIME/config/server.toml"
    while true; do
      "$MODDIR/bin/device_faker_server" --config "$RUNTIME/config/server.toml" >> "$RUNTIME/logs/server.log" 2>&1
      echo "server exited ($?), restarting in 5s" >> "$RUNTIME/logs/server.log"
      sleep 5
    done &

要点：

- 后台常驻，崩溃 5 秒后拉起。
- 单实例：启动时写 /data/adb/device_faker/server.pid，检测旧 pid 存活则退出。
- 退出：uninstall.sh 读取 pid 并 kill；也可由服务自身处理 SIGTERM。
- 日志轮转：服务内按大小切分，或由 service.sh 简单 mv。

### 8.3 文件与权限

| 路径 | 用途 | 权限 |
| --- | --- | --- |
| /data/adb/modules/device_faker/bin/device_faker_server | 二进制 | 755 |
| /data/adb/modules/device_faker/webroot/ | 静态资源 | 644/755 |
| /data/adb/device_faker/config/config.toml | 配置 | 644 |
| /data/adb/device_faker/config/server.toml | 服务设置与 token 哈希 | 600 |
| /data/adb/device_faker/config/backups/ | 备份 | 700 |
| /data/adb/device_faker/logs/server.log | 服务日志 | 600 |
| /data/adb/device_faker/logs/api-audit.jsonl | 审计 | 600 |

- 已验证：模块目录二进制上下文 system_file 可执行；配置目录写入后上下文正确。
- Magisk 环境需验证 service.sh 的域是否能绑定端口；若受限，回退方案为 Unix socket + adb forward，或在 post-fs-data 阶段启动。
- 端口默认 8686，server.toml 可改；启动时检测占用并记录明确日志。

### 8.4 静态资源与 SPA

- 根路径返回 module/webroot/index.html，注入运行时配置脚本。
- 其余路径按文件返回；未命中且无扩展名时回退 index.html。
- 防目录穿越：规范化路径后必须位于 webroot 内。
- MIME 表覆盖 js/css/json/svg/png/woff2/wasm。
- 可选 gzip/br：tower-http compression。

### 8.5 资源目标

| 指标 | 目标 |
| --- | --- |
| 二进制大小（arm64, strip, lto） | < 8 MB |
| 空闲内存 RSS | < 20 MB |
| 空闲 CPU | < 0.5% |
| 冷启动到可服务 | < 1 s |
| 配置读取（缓存命中） | < 5 ms |
| 包列表（缓存命中） | < 10 ms |

### 8.6 构建与打包

- 新增 workspace 成员：core、server。
- xtask build 增加：
  - cargo ndk -t arm64-v8a -p 29 build -p device_faker_server --release
  - 复制产物到临时目录 bin/device_faker_server
  - 复制 module/service.sh、server.default.toml
  - 保持现有 zygisk 构建与 webroot 复制流程
- 兼容现有 pack_module.py（遍历 module 目录，自动包含 bin 与 service.sh）。
- CI：GitHub Actions 增加 server 构建与 cargo test；发布产物包含源码（GPL 要求）。

---

## 9. 共享配置内核重构

### 9.1 目标

把配置模型从 Zygisk cdylib 中抽出，成为可被 server、CLI、测试复用的库，同时不改变现有行为。

### 9.2 拆分

- core/src/config.rs：Config、DeviceTemplate、AppConfig、NumberSetting、合并与冲突检测。
- core/src/edit.rs：toml_edit 文档级修改（upsert template、bind package、set app、delete）。
- core/src/revision.rs：sha256、原子写、备份、flock。
- core/src/schema.rs：schemars 生成 JSON Schema。
- src/（zygisk）改为依赖 core；companion 的 spec 渲染逻辑保留或下沉到 core。

### 9.3 序列化

- 现有 Config 仅 Deserialize；新增 Serialize 或单独的 API DTO，避免把内部字段直接暴露。
- API JSON 字段与 TOML 保持一致（snake_case），降低 AI 与用户的理解成本。
- 保留 IndexMap 的模板顺序，确保「同名包多模板」命中规则不变。

### 9.4 校验规则

- 包名格式：Java 包名正则；支持 pkg@userId。
- dpi：120 到 640。
- sdk_int：合法整数或 __DELETE__。
- android_version：数字字符串。
- custom_props：键必须匹配 ro.* 或 persist.*，值长度上限。
- 冲突：同一包名出现在多个模板的 packages 中 → warning 或 error（按配置）。
- 未知字段：保留并 warning，不丢弃。

### 9.5 注释保留

- 服务端写配置使用 toml_edit 定点修改，保留注释、空行与键顺序。
- 管理器模式 WebUI 仍用 smol-toml 整体重写（注释会丢）；原生模式统一走 API 后不再丢注释。
- 文档中明确两种模式的差异。

---

## 10. 安全设计

### 10.1 威胁模型

| 威胁 | 影响 | 对策 |
| --- | --- | --- |
| 同机恶意 App 访问 127.0.0.1 | 读取/篡改配置 | 强制 Token；默认只读 Token 可选 |
| 局域网被扫描 | 未授权访问 | 默认只绑 loopback；LAN 需显式开启 + Token |
| Token 泄露 | 全量控制 | 作用域、可撤销、审计、只读 Token |
| 恶意配置注入 | 影响目标应用行为 | 字段白名单、长度限制、语义校验 |
| 路径穿越 | 读写任意文件 | 静态资源与文件接口均做根目录约束 |
| 请求体过大/慢速攻击 | 资源耗尽 | body 上限 2MB、超时、限流 |
| 日志泄露敏感信息 | 信息外泄 | 审计不记录 Token 与完整配置 |

### 10.2 明确不做的事

- 不提供任意 shell 执行接口。
- 不提供任意文件读写接口；文件访问限定在 /data/adb/device_faker 与模块目录。
- 不允许通过 API 修改服务自身二进制或开机脚本。

### 10.3 网络与 TLS

- 默认 127.0.0.1:8686。
- 局域网访问建议配合 Tailscale / WireGuard；如需直接暴露，P4 提供可选 TLS（rustls + 自签证书 + 指纹输出）。
- 支持 Unix socket（可选）：/data/adb/device_faker/server.sock，便于 adb forward 之外的本地集成。

### 10.4 审计与告警

- 每次写操作记录 actor、revision 变化、diff 哈希。
- 连续鉴权失败 10 次/分钟 → 临时封禁来源 IP 5 分钟，并写审计。

---

## 11. 兼容性

| 维度 | 范围 | 说明 |
| --- | --- | --- |
| Android | 10 到 17 | 与模块现状一致；服务使用普通 socket 与文件 API |
| Root 方案 | KernelSU、Magisk | service.sh 均支持；SELinux 域需分别验证 |
| ABI | arm64-v8a | 与现有 zygisk 产物一致；x86_64 模拟器可选 |
| WebView | 管理器旧版 | WebUI 保持 ES2022 兼容与 polyfill |
| 管理器 | KernelSU / APatch / Magisk | ksu 传输保持原逻辑 |
| 端口 | 默认 8686，可配置 | 冲突时启动失败并写明确日志 |
| 旧版模块 | 无服务端 | 前端探测失败时回退只读提示 |

---

## 12. 实施计划

### P0 契约与参考实现（1.5 人日）

- 产出 docs/API.md 初稿、openapi.yaml 骨架、错误码表。
- 把现有 tools/webui_remote 的桥接服务改造为 /api/v1 的 Node 参考实现（只读 + 配置读写 + 包列表）。
- 用 curl 与脚本跑通「读状态 → 读模板 → 写模板 → 读回」。
- 交付：contract 文档、参考实现、冒烟脚本。
- 价值：AI 侧可以立刻按契约开发，不阻塞 Rust 服务。

### P1 core crate 与只读 API（4 人日）

- 抽出 device_faker_core，zygisk 改为依赖 core，行为不变。
- 新建 server crate：静态托管、认证、status、config 读取、templates/apps/packages 只读、schema、openapi。
- service.sh 与构建打包打通，设备上可访问。
- 交付：可在手机浏览器打开完整只读 WebUI；curl 可读全部资源。
- 验收：真机 Android 17 + 一台 Android 12 设备通过。

### P2 写 API 与 WebUI 原生模式（3 人日）

- toml_edit 写路径、flock、revision、If-Match、备份、审计。
- 模板/应用 CRUD、validate、restart、props、users。
- WebUI 传输抽象与登录；原生模式功能对齐管理器模式。
- 交付：无管理器环境下完整管理。
- 验收：并发写测试、注释保留测试、管理器模式回归。

### P3 AI 层（3 人日）

- plan/apply/simulate/explain/conflicts/batch。
- MCP HTTP 端点与 stdio 桥；tools/resources/prompts。
- llms.txt、guide、OpenAPI 完善；AI e2e 脚本。
- 交付：Claude Desktop / Cursor 可挂载；脚本化 Agent 完成建模板到生效。
- 验收：MCP Inspector 全工具通过；AI e2e 用例通过。

### P4 加固与发布（3 人日）

- 应用名与图标（见 12.1）、限流、审计轮转、TLS 可选、Unix socket。
- 性能优化、压力测试、文档站更新、发布流程与 GPL 源码包。
- 交付：v1.8.0 发布候选。

### 12.1 应用名与图标方案（P4）

- 现状：adb/CLI 无 PackageManager 标签接口；管理器通过 binder 提供。
- 方案 A：服务端解析 APK 的 AndroidManifest 与 resources.arsc，取 label 与 icon（纯 Rust，需引入 arsc 解析）。
- 方案 B：随模块带一个极小 dex，用 app_process 调 PackageManager 生成 label/icon 缓存（实现重，但准确）。
- 方案 C：只显示包名与首字母占位图，文档说明限制。
- 推荐：P4 先做方案 C 的占位优化，方案 A 作为后续独立迭代；不阻塞主链路。

---

## 13. 测试计划

### 13.1 单元测试

- core：TOML 往返、注释保留、模板顺序、冲突检测、校验规则、revision 计算。
- edit：upsert/bind/delete 的幂等性与边界（空配置、未知字段、同名模板）。
- auth：Token 哈希、作用域判定、限流。

### 13.2 契约测试

- 用 schemathesis 或 Dredd 对 /openapi.json 跑全端点。
- 断言：所有 4xx 使用统一错误信封；所有写操作返回 revision。
- 生成 TypeScript 客户端类型，校验与 WebUI 使用一致。

### 13.3 设备测试

- 矩阵：Android 17（Pixel 9）、Android 14、Android 12；KernelSU 与 Magisk 各一台。
- 脚本：tests/device/api_smoke.sh，通过 adb forward 执行。
- 用例：
  1. 服务随开机启动，pid 与端口正确。
  2. 静态页 200，assets 命中缓存头。
  3. 无 Token 401，错误 Token 401，只读 Token 写 403。
  4. 读配置 revision 与文件 sha256 一致。
  5. 写模板后 mtime 变化，重启目标应用后 getprop 显示伪装值。
  6. 并发写同 revision：一个 200，一个 412。
  7. 断电/重启后配置与备份完整。
  8. 卸载模块后进程退出、端口释放。

### 13.4 安全测试

- 路径穿越：../、编码、符号链接。
- 超大 body、超长字段、慢速请求。
- 伪造 Host、CORS 预检、方法覆盖。
- Token 暴力尝试触发限流与审计。

### 13.5 AI e2e

- 用 MCP Inspector + 脚本：status → template → plan → apply → restart → explain。
- 用只读 Token 验证 Agent 不能越权。
- 断网、服务重启、计划过期等异常路径。

### 13.6 性能

- wrk/hey 对只读接口压测：目标 200 QPS（localhost）。
- 包列表 500 个包、配置 1MB 场景下的响应时间。
- 内存与 fd 泄漏检查（长时间运行 24h）。

---

## 14. 风险与对策

| 风险 | 概率 | 影响 | 对策 |
| --- | --- | --- | --- |
| Magisk 域无法绑定端口 | 中 | 高 | P0 真机验证；回退 Unix socket + adb forward；或改为 post-fs-data 启动 |
| 与管理器 WebUI 并发写冲突 | 高 | 中 | revision + 412；原生模式统一走 API；文档提示 |
| 二进制体积/依赖膨胀 | 中 | 中 | 精简 feature、strip、lto；目标 < 8MB |
| Android 厂商杀后台 | 低 | 中 | service.sh 守护循环；前台服务不适用（无 APK） |
| AI 误操作破坏配置 | 中 | 高 | plan/apply 分离、备份、只读 Token、confirm |
| 配置注释丢失 | 中 | 低 | toml_edit；管理器模式文档说明 |
| 应用名/图标不可得 | 高 | 低 | P4 方案 A/B；先占位 |
| MCP 协议演进 | 中 | 中 | 版本协商；REST 作为稳定底座 |
| 安全暴露 | 中 | 高 | 默认 loopback + Token + 无 exec + 限流 |
| 构建链复杂度 | 中 | 中 | xtask 单入口；CI 缓存；文档化 |

---

## 15. 交付物清单

代码：

- core/（配置内核、编辑、校验、schema）
- server/（HTTP、REST、MCP、认证、审计、静态托管）
- src/ 改造为依赖 core
- webui/src/utils/backend.ts 与 backend/ksu.ts、backend/http.ts
- module/service.sh、module/server.default.toml、module/bin/device_faker_server（构建产物）
- tools/mcp_stdio_bridge.mjs
- tests/api/、tests/device/

文档：

- docs/API.md（人读）
- docs/MCP.md（客户端配置）
- docs/SERVER.md（部署、端口、Token、排障）
- docs/openapi.json（构建产物）
- docs/dev/server-api-plan.md（本文）
- 文档站对应页面与变更日志

发布：

- 模块 zip 包含 server 二进制与 service.sh
- GitHub Release 附源码包（GPL-3.0 要求）
- 升级说明：旧配置无需迁移；server.toml 首次启动自动生成

---

## 16. 验收标准

1. 未安装任何管理器的情况下，手机浏览器打开 http://127.0.0.1:8686 可完成模板、应用、设置的完整管理。
2. 电脑执行 adb forward 后，浏览器与 curl 均可访问；携带 Token 的 API 返回 200。
3. MCP 客户端能列出全部工具，并完成一次「建模板 → 绑定包名 → plan → apply → 重启应用」。
4. 并发写入：两个客户端持同一 revision 写，一个成功一个 412，文件无损坏。
5. 写配置后注释与未知字段保留（原生模式）。
6. 管理器模式回归：原有 WebUI 功能、样式、交互无变化。
7. 断电重启后服务自动恢复，端口与 pid 正确。
8. 审计日志包含每次写操作的 revision 变化与 actor。
9. OpenAPI 通过契约测试；错误信封一致。
10. 只读 Token 无法执行任何写操作；无 Token 无法读取配置。
11. 24 小时稳定性测试无内存/fd 泄漏。
12. 文档齐全，AI 仅凭 llms.txt + OpenAPI 即可完成典型任务。

---

## 17. 待决策项（附推荐）

| 决策 | 选项 | 推荐 |
| --- | --- | --- |
| 默认监听 | 127.0.0.1:8686 / 0.0.0.0:8686 | 127.0.0.1，LAN 显式开启 |
| 默认认证 | 无 / 强制 Token | 强制 Token，首次启动生成 |
| 任意 exec | 提供 / 不提供 | 不提供，改为语义化端点 |
| WebUI 传输 | 只 HTTP / 双传输 | 双传输，管理器模式保留 |
| MCP 传输 | HTTP / stdio / 两者 | HTTP + stdio 桥 |
| 配置写实现 | 整体重写 / toml_edit | toml_edit |
| 应用名图标 | 本期 / 延后 | 延后到 P4 |
| TLS | 本期 / 延后 | 延后，优先 Tailscale/反代 |

---

## 18. 附录

### A. server.default.toml

    bind = "127.0.0.1:8686"
    static_dir = "/data/adb/modules/device_faker/webroot"
    config_path = "/data/adb/device_faker/config/config.toml"
    backup_dir = "/data/adb/device_faker/config/backups"
    audit_log = "/data/adb/device_faker/logs/api-audit.jsonl"
    allow_lan = false
    body_limit_bytes = 2097152
    read_rate_per_min = 120
    write_rate_per_min = 30

    [[tokens]]
    id = "default"
    name = "owner"
    hash = "sha256:..."
    scopes = ["read", "write", "ops", "admin"]

### B. 常用 curl

    TOKEN=df_xxx
    BASE=http://127.0.0.1:8686/api/v1

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/status"
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/templates"
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"baseRevision":"sha256:...","operations":[{"op":"template.upsert","name":"pixel_9","value":{"model":"Pixel 9"}}]}' \
      "$BASE/plan"

### C. MCP 客户端配置（stdio 桥）

    {
      "mcpServers": {
        "device-faker": {
          "command": "node",
          "args": ["/path/to/device_faker/tools/mcp_stdio_bridge.mjs"],
          "env": {
            "DF_URL": "http://127.0.0.1:8686/mcp",
            "DF_TOKEN": "df_xxx"
          }
        }
      }
    }

### D. 配置字段映射

| TOML 字段 | JSON 字段 | 类型 | 说明 |
| --- | --- | --- | --- |
| templates.<name>.packages | packages | string[] | 绑定包名，支持 pkg@userId |
| templates.<name>.brand | brand | string | Build.BRAND |
| templates.<name>.model | model | string | Build.MODEL |
| templates.<name>.fingerprint | fingerprint | string | Build.FINGERPRINT |
| templates.<name>.sdk_int | sdk_int | int 或 "__DELETE__" | Build.VERSION.SDK_INT |
| templates.<name>.dpi | dpi | int | 120 到 640 |
| templates.<name>.custom_props | custom_props | object | 自定义属性映射 |
| apps[].package | package | string | 应用内联配置 |
| default_force_denylist_unmount | default_force_denylist_unmount | bool | 全局开关 |
| debug | debug | bool | 调试日志 |

---

## 19. 结论

该方案不改变模块现有伪装链路，只在控制面增加一个原生服务与一层稳定契约：

- 对用户：WebUI 摆脱管理器，手机或电脑浏览器都能用。
- 对 AI：REST + OpenAPI + MCP 三件套，可发现、可预演、可审计、可回滚。
- 对维护者：core 统一配置模型，schema 自动生成，前端双传输不分裂。

建议按 P0 → P1 → P2 → P3 → P4 推进，P1 结束即可交付「浏览器可用」，P3 结束即可交付「AI 可用」。
