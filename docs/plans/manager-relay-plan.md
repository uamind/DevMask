# 多设备管理端 / Relay 开发计划

> 状态：草案，待确认后开工
> 日期：2026-11-24
> 关联目录：manager/、manager/webui/、docs/design/manager-ui/
> 前置能力：设备端 /api/v1（v1.9.0）已具备 status、overview、templates、apps 等端点

## 1. 背景

DevMask 现在每台设备自带一个 devmask_server。AI 或脚本要管理 N 台设备时，需要保存 N 个地址和 N 个 Token，逐台查询 overview，逐台处理 revision、412、模板不存在和离线；也无法一眼看出同一个应用在不同设备上是否统一。

本项目新增一个管理端 / Relay，把多台设备聚合成一个控制面：

```text
AI / Agent
   |  Authorization: Bearer dfm_...
   v
DevMask Manager / Relay  (:8765)
   |-- 设备注册表（baseUrl + 设备 Token，加密存储）
   |-- 聚合层（/devices、/apps、/overview）
   |-- 下发编排（按设备 revision 逐台下发模板）
   |-- 审计 / 幂等 / 重试 / job 结果
        |  Authorization: Bearer <device token>
        v
   Pixel 9 (:8686)   Xiaomi 15 (:8686)   Redmi K80 (:8686) ...
```

UI 原型已经完成，见 manager/webui 与 docs/design/manager-ui/01-dashboard.jpg 至 05-relay.jpg。

## 2. 目标

### 2.1 必须实现

1. 设备管理：添加、列表、详情、删除、手动同步；添加时探测 GET /api/v1/status 与 GET /api/v1/overview。
2. Token 隔离：设备 Token 只存在 Relay 服务端，永不出现在 API 响应、日志或前端。
3. 聚合视图：设备列表、在线状态、最后同步、跨设备应用矩阵、配置漂移。
4. 批量下发：同一应用 + 全部/指定设备 + 同一模板；不同设备不同应用/模板的 assignments 形态。
5. 逐设备处理 revision、模板不存在、412、包未安装、离线，并返回 job 与逐设备结果。
6. AI Relay：单一 Base URL + Relay Token；可选单设备代理。
7. WebUI 接真实 API，保留现有页面视觉，补 loading/error/401 状态。
8. 测试、文档、端到端验收和单次 commit。

### 2.2 非目标

- 多租户、RBAC、公网 TLS、OAuth。
- 定时同步、WebSocket 实时进度、应用名/图标抓取。
- 移动端管理 UI 专项适配。
- 设备端模块本身的修改，除非发现阻塞 API 的 bug。

## 3. 架构

| 组件 | 默认地址 | 说明 |
| --- | --- | --- |
| Manager / Relay | 127.0.0.1:8765 | Rust axum，同时托管 WebUI 静态资源 |
| 设备端 server | <device>:8686 | 现有 devmask_server |
| WebUI dev | 127.0.0.1:5180 | Vite，/relay 代理到 8765 |

### 3.1 目录结构

```text
manager/
  Cargo.toml
  README.md
  docs/API.md
  docs/ARCH.md
  docs/RUN.md
  src/main.rs
  src/config.rs
  src/db.rs
  src/crypto.rs
  src/auth.rs
  src/error.rs
  src/device_client.rs
  src/jobs.rs
  src/static_files.rs
  src/api/{mod,devices,apps,apply,jobs}.rs
  tests/{manager_api,mock_device}.rs
  webui/src/api/client.ts
  webui/src/stores/manager.ts
  webui/vite.config.ts
```

### 3.2 依赖建议

axum 0.8、tokio 1、reqwest 0.12（json + rustls）、rusqlite 0.32（bundled）、serde、serde_json、uuid、aes-gcm、base64、sha2、tower-http（fs/trace/compression）、tracing、anyhow、thiserror。

## 4. Relay API

前缀 /relay/api/v1；鉴权 Authorization: Bearer <manager-token>；Manager Token 默认 admin123，可用 manager.toml、--token 或 DFM_TOKEN 覆盖。

错误信封与设备端一致：

```json
{
  "error": {
    "code": "device_offline",
    "message": "Pixel 9 连接超时",
    "details": {"deviceId": "pixel-9"},
    "retryable": true,
    "requestId": "req_..."
  }
}
```

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | /healthz | 无鉴权；版本、uptime、db 状态 |
| GET | /devices | 设备列表（不含 Token） |
| POST | /devices | 添加并探测设备 |
| GET | /devices/{id} | 设备详情 |
| DELETE | /devices/{id} | 从管理端移除 |
| POST | /devices/{id}/sync | 重新拉取 overview |
| GET | /devices/{id}/overview | 代理设备 overview |
| GET | /apps | 跨设备应用矩阵 |
| POST | /apply | 批量/逐项下发 |
| GET | /jobs/{id} | job 与逐设备结果 |
| ANY | /devices/{id}/proxy/api/v1/{*path} | 可选：单设备代理 |

### 4.1 添加设备

```json
POST /relay/api/v1/devices
{
  "name": "Pixel 9 - 主力机",
  "baseUrl": "http://192.168.30.2:8686",
  "token": "admin123",
  "tags": ["主力", "内网"]
}
```

成功 201 返回 id、name、baseUrl、online、model、android、fingerprint、moduleVersion、serverVersion、tags、lastSeen。探测失败返回 422 device_probe_failed，不写入设备表；重复 baseUrl 返回 409 device_exists。

### 4.2 应用矩阵

GET /relay/api/v1/apps 返回 devices、apps、warnings、revision。apps 每项包含 package、cells、configured、total、inconsistent；cells 以设备 id 为键，值为 model、template、source、warnings。

### 4.3 批量下发

Bulk：

```json
POST /relay/api/v1/apply
{
  "app": "com.xingin.xhs",
  "devices": "all",
  "template": {
    "name": "Pixel 9",
    "brand": "google",
    "model": "Pixel 9",
    "android": "17",
    "sdk_int": 37,
    "dpi": 420
  },
  "restart": true
}
```

Assignments：

```json
{
  "assignments": [
    {"device": "pixel-9", "app": "com.ss.android.ugc.aweme", "template": {"name": "Pixel 9", "model": "Pixel 9", "brand": "google", "android": "17", "sdk_int": 37, "dpi": 420}},
    {"device": "xiaomi-15", "app": "com.tencent.mm", "template": {"name": "Xiaomi 15", "model": "Xiaomi 15", "brand": "Xiaomi", "android": "15", "sdk_int": 35, "dpi": 440}}
  ],
  "restart": false
}
```

响应 202 返回 jobId、total、queued。逐设备流程：

1. GET /api/v1/config/revision。
2. GET /api/v1/templates/{name}；不存在则 PUT /templates/{name}（If-Match: *）。
3. PUT /api/v1/apps/{pkg}，body 为 template 绑定，detachFromTemplates=true。
4. 412 时重新取 revision，重试一次。
5. 404 包未安装记录 package_not_installed，继续。
6. 离线或超时记录 offline，继续。
7. restart=true 时调用 POST /apps/{pkg}/restart；ops 不足只记 warning。

### 4.4 Job

GET /relay/api/v1/jobs/{id} 返回 id、status（queued/running/succeeded/partial/failed）、total、succeeded、skipped、failed、results。results 每项包含 deviceId、status、httpStatus、error、durationMs。

## 5. 数据模型

SQLite 表：

```sql
CREATE TABLE devices (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  base_url TEXT NOT NULL UNIQUE,
  token_cipher TEXT NOT NULL,
  tags TEXT NOT NULL DEFAULT "[]",
  online INTEGER NOT NULL DEFAULT 0,
  model TEXT, brand TEXT, android TEXT, fingerprint TEXT,
  module_version TEXT, server_version TEXT, last_seen TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE jobs (
  id TEXT PRIMARY KEY, status TEXT NOT NULL, total INTEGER NOT NULL,
  succeeded INTEGER NOT NULL DEFAULT 0, skipped INTEGER NOT NULL DEFAULT 0,
  failed INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, finished_at TEXT
);
CREATE TABLE job_results (
  job_id TEXT NOT NULL, device_id TEXT NOT NULL, app TEXT NOT NULL,
  status TEXT NOT NULL, http_status INTEGER, error TEXT, duration_ms INTEGER,
  PRIMARY KEY (job_id, device_id, app)
);
CREATE TABLE audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, actor TEXT,
  action TEXT NOT NULL, target TEXT, request_id TEXT, detail TEXT
);
```

设备 Token：首次启动生成 32 字节 key，写入 <data-dir>/devmask-manager.key（0600）；Token 用 AES-256-GCM 加密，nonce 随机，密文与 nonce 一起 base64 存入 token_cipher；解密密文只在调用设备时发生，不写日志、不返回响应。

## 6. WebUI 接入

新增 manager/webui/src/api/client.ts：base 为 /relay/api/v1 或 VITE_API_BASE；Token 存 localStorage dm_token，默认 admin123；401 抛出 AuthError，由 App 弹出 Token 对话框；统一解析错误信封。

useManagerStore 改为 async：load（GET devices + apps）、addDevice（POST）、removeDevice（DELETE）、syncDevice（POST sync）、setAppConfig/removeAppConfig（apply assignments）、batchApply（POST apply 后轮询 job）。保留 VITE_MOCK=1 作为纯视觉预览。

补 loading skeleton、空状态、离线 badge、Token 失效弹窗、逐设备结果展开、错误 toast 带 requestId。

## 7. 里程碑与任务 DAG

| ID | 任务 | Owner | 依赖 | 交付 |
| --- | --- | --- | --- | --- |
| contract | 冻结 API / 数据模型 / 目录 | backend-dev | - | manager/docs/API.md、ARCH.md |
| backend-core | crate、SQLite、鉴权、静态托管 | backend-dev | contract | 可启动的 manager |
| backend-devices | 设备注册、探测、overview、apps 聚合 | backend-dev | backend-core | devices/apps API + 测试 |
| backend-apply | apply、revision 重试、jobs | backend-dev | backend-devices | apply/jobs API + 测试 |
| frontend-api | WebUI 接真实 API | frontend-dev | contract | 真实数据页面 + build |
| test-docs | 集成测试、Mock 设备、Run 文档 | test-verifier | backend-devices, backend-apply, frontend-api | 全绿测试 + 文档 |
| verify | 端到端验收与截图 | test-verifier | test-docs | 验收报告 |
| review | 代码审查 | reviewer | verify | findings + 结论 |
| repair | 修复 findings、准备单 commit | backend-dev | review | 可合并状态 |

写作用域：后端只改 manager/src、manager/Cargo.toml、manager/docs、manager/tests 和根 Cargo.toml；前端只改 manager/webui；测试文档只改 manager/tests、manager/docs、manager/README.md。

## 8. 验收清单

- [ ] cargo fmt --check 通过。
- [ ] cargo test -p devmask_manager 全绿。
- [ ] npx vue-tsc --noEmit 通过。
- [ ] npm run build 通过。
- [ ] GET /healthz 返回 200。
- [ ] 无 Token 访问 /relay/api/v1/devices 返回 401。
- [ ] 添加 Mock 设备成功，设备列表与 apps 矩阵出现该设备。
- [ ] apply bulk 返回 job，逐设备结果正确。
- [ ] apply assignments 支持不同设备不同模板。
- [ ] 412 场景只重试一次并成功。
- [ ] 离线设备跳过，job 标记 partial。
- [ ] 管理端静态托管 dist 页面可打开。
- [ ] 真实 Pixel 9（可选）只读 overview 能接入。
- [ ] Token 不出现在响应、日志或截图。
- [ ] 所有新增文件有 SPDX 和版权头。
- [ ] 一个 commit：feat(manager): add multi-device relay service and console。

## 9. 风险与待确认

| 风险 / 决策 | 影响 | 默认方案 |
| --- | --- | --- |
| 设备端模板名不一致 | apply 404 | Manager 先 PUT 模板再绑定，模板对象随 apply 传入 |
| 设备 Token 存储 | 安全 | AES-256-GCM + 0600 key 文件 |
| 离线设备 | 批量结果 | 跳过并记录，后续可加待下发队列 |
| UI 视觉是否保留 | 返工 | 先按当前原型，确认后只做小调 |
| 是否需要设备分组 | 批量范围 | 本增量只展示 tags，不做分组 API |
| 真实设备验收 | 网络 | Mock 为主，Pixel 9 只读可选 |

需要用户确认：

1. 页面视觉是否按当前原型定稿。
2. 批量是否只做应用 + 模板，还是同时要整机一键换机型。
3. 是否需要设备分组/标签批量。
4. 离线设备是跳过还是排队。
5. 是否接受 Manager Token 默认 admin123（内网便利）。

## 10. 许可

本增量沿用 GPL-3.0-or-later。所有新增 Rust、TypeScript、文档文件保留：

```text
SPDX-License-Identifier: GPL-3.0-or-later
Copyright (c) 2025-2026 Seyud
DevMask: https://github.com/uamind/DevMask
```