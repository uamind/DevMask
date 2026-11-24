# 运行手册：Manager / Relay

面向「从干净 checkout 跑起来」的最小步骤、真实设备接入、端到端冒烟与排错。
接口与字段以 [API.md](API.md) 为准，内部结构见 [ARCH.md](ARCH.md)。

> 所有命令默认在仓库根目录执行（即 `device_faker/`）。

---

## 0. 5 分钟快速开始

    # 1) WebUI 构建（产物 manager/webui/dist，由 Manager 同进程托管）
    cd manager/webui
    npm install
    npm run build
    cd ../..

    # 2) 构建并启动 Manager（默认 127.0.0.1:8765）
    cargo run -p device_faker_manager

    # 3) 浏览器打开 http://127.0.0.1:8765
    #    Token 对话框输入 admin123（默认值，仅 loopback）

另开一个终端确认：

    curl -s http://127.0.0.1:8765/healthz | jq '{status, version, db, devices}'
    # {"status":"ok","version":"0.1.0","db":{"status":"ok","schemaVersion":1,"path":"manager.db"},...}

首次启动会创建数据目录与 `manager.key`；默认数据目录会打印在启动日志里
（macOS `~/Library/Application Support/device-faker-manager`，Linux
`$XDG_DATA_HOME/device-faker-manager` 或 `~/.local/share/device-faker-manager`，
Windows `%LOCALAPPDATA%\device-faker-manager`）。

---

## 1. 前置条件

| 工具 | 版本 | 用途 |
| --- | --- | --- |
| Rust | stable ≥ 1.85（edition 2024） | 构建 Manager |
| Node.js | ^20.19 或 ≥ 22.12（Vite 8 要求） | 构建 WebUI |
| npm | 随 Node | 安装 WebUI 依赖 |
| curl + jq | 任意近版本 | 冒烟与排错 |
| python3 | ≥ 3.9 | 可选：运行 Mock 设备脚本 |

端口：

| 端口 | 进程 | 说明 |
| --- | --- | --- |
| 8765 | Manager | `--bind` 可改；默认只监听 loopback |
| 5180 | Vite dev/preview | `npm run dev` / `npm run preview` |
| 8686 | Device Faker 设备端 | 可选；USB 用 `adb forward tcp:8686 tcp:8686` |
| 18686 | `mock_device.py` | 冒烟用；被占用时换端口 |

---

## 2. 构建

    # Manager（debug）
    cargo build -p device_faker_manager

    # Manager（release，单文件产物 target/release/device_faker_manager）
    cargo build -p device_faker_manager --release

    # WebUI：类型检查 + 构建（产物 manager/webui/dist）
    cd manager/webui
    npm install
    npx vue-tsc --noEmit
    npm run build
    cd ../..

- `npm run build` 只做 Vite 打包；类型检查用 `npx vue-tsc --noEmit`。
- 默认构建不包含 Mock 数据；只有 `VITE_MOCK=1 npm run build` 才会把 `src/mock` 打进产物。
- `dist` 缺失时 Manager 仍可启动，`/` 返回一页构建提示，API 不受影响。

---

## 3. 启动 Manager

### 3.1 常用命令

    # 默认：127.0.0.1:8765，数据目录按平台默认，Token=admin123
    cargo run -p device_faker_manager

    # 显式指定（推荐：数据目录放固定位置，便于备份）
    cargo run -p device_faker_manager -- \
      --bind 127.0.0.1:8765 \
      --data-dir ./tmp/manager-data \
      --static-dir manager/webui/dist \
      --token 'admin123'

    # 发布二进制
    ./target/release/device_faker_manager --data-dir /var/lib/device-faker-manager

    # 局域网 / 其它机器访问：必须换成至少 8 字符的非默认 Token
    DFM_TOKEN='S3cure-manager-token' \
      ./target/release/device_faker_manager --bind 0.0.0.0:8765

    # 调试日志
    DFM_LOG=debug cargo run -p device_faker_manager

    # 看帮助 / 版本
    cargo run -p device_faker_manager -- --help
    cargo run -p device_faker_manager -- --version

`Ctrl-C` 或 `SIGTERM` 优雅退出；SQLite 使用 WAL，异常退出后下次启动会自动恢复
（把遗留 `queued/running` job 标记为 `failed/interrupted`）。

### 3.2 参数

| 参数 | 环境变量 | 默认 | 说明 |
| --- | --- | --- | --- |
| `--bind <ADDR>` | `DFM_BIND` | `127.0.0.1:8765` | 监听地址 |
| `--data-dir <PATH>` | `DFM_DATA_DIR` | 平台默认 | `manager.db` / `manager.key` / `manager.token` 所在目录 |
| `--static-dir <PATH>` | `DFM_STATIC_DIR` | 自动查找 | WebUI `dist` 目录 |
| `--token <TOKEN>` | `DFM_TOKEN` | `admin123` | Manager Token |
| `--token-file <PATH>` | `DFM_TOKEN_FILE` | `<data-dir>/manager.token` | 轮换后写入的 Token 文件 |
| `--key-file <PATH>` | `DFM_KEY_FILE` | `<data-dir>/manager.key` | 设备 Token 加密密钥（32 B） |
| `--config <PATH>` | `DFM_CONFIG` | `<data-dir>/manager.toml` | 配置文件 |
| `--log-level <LEVEL>` | `DFM_LOG` / `RUST_LOG` | `info` | tracing 过滤规则 |

Token 优先级：`--token` > `DFM_TOKEN` > `manager.token` 文件 > `manager.toml` > 默认值。
并发：`DFM_APPLY_CONCURRENCY` / `DFM_SYNC_CONCURRENCY`（1–16，默认 4）。

静态目录自动查找顺序：显式 `--static-dir` / `DFM_STATIC_DIR` / 配置 `static_dir` →
`<可执行文件目录>/webui/dist` → `<cwd>/manager/webui/dist` → `<cwd>/webui/dist`。

### 3.3 配置文件 `manager.toml`

默认读取 `<data-dir>/manager.toml`，文件不存在不报错；未知键只 warn。

    bind = "127.0.0.1:8765"
    static_dir = "manager/webui/dist"
    token = "admin123"
    apply_concurrency = 4
    sync_concurrency = 4
    request_timeout_ms = 10000
    write_timeout_ms = 15000

    [manager]
    # 与顶层 token 等价；同时存在时 [manager].token 优先
    token = "admin123"

> `request_timeout_ms` / `write_timeout_ms` 当前不做上下限校验，建议保持默认；
> 不建议填 `0`（可能让设备请求立即超时）。

### 3.4 安全边界

- 监听非 loopback 地址时，Token 必须**不是** `admin123` 且长度 ≥ 8，否则拒绝启动。
- Manager Token 拥有全部权限；WebUI 把它存在 `localStorage.dfm_token`。
- 设备 Token 用 AES-256-GCM 加密存库，密钥是 `<data-dir>/manager.key`（0600）；
  API 响应、错误、审计、日志都不会出现设备 Token。
- Manager 本身不提供 TLS；跨机器访问请走内网 / `adb forward` / Tailscale 等隧道，
  或在前面加反向代理。

---

## 4. 打开 WebUI

1. 浏览器访问 `http://127.0.0.1:8765`。
2. 首次会弹出 Token 对话框；输入启动时的 Manager Token（默认 `admin123`）。
3. 顶部显示 Relay 在线状态；Token 错误时接口返回 401，页面会重新弹对话框。
4. 需要换 Token 时点「重新登录」清除本地 Token，或在浏览器控制台
   `localStorage.removeItem('dfm_token')`。

页面：总览 / 设备 / 应用矩阵 / 批量配置 / 接入 AI / 操作日志。
「接入 AI」页给出 Relay Base URL、Token、curl 示例与端点表。

### 4.1 开发模式（Vite dev server）

    cd manager/webui
    npm run dev
    # http://127.0.0.1:5180

Vite 已把 `/relay` 与 `/healthz` 代理到 `http://127.0.0.1:8765`，因此 WebUI 同源调用；
Manager 没启动时页面显示「Relay 请求失败」与启动提示（不是假数据）。

    # Manager 在别的地址时覆盖代理目标
    DFM_DEV_RELAY=http://127.0.0.1:9000 npm run dev

    # 只做 UI 预览、不连后端（写操作会明确报错）
    VITE_MOCK=1 npm run dev

---

## 5. 接入真实设备

设备端要求：Device Faker 模块 v1.9.0+，设备端 HTTP 服务已启动（默认
`127.0.0.1:8686`）。三种常见接法：

| 场景 | 做法 |
| --- | --- |
| USB（推荐调试） | `adb forward tcp:8686 tcp:8686`，Manager 里填 `http://127.0.0.1:8686` |
| 局域网 | 设备「外部访问」卡片开启局域网（`bind=0.0.0.0:8686` + `allow_lan=true` + Token），填 `http://<手机IP>:8686` |
| 隧道 | Tailscale / SSH 转发到设备 8686，填对应本地地址 |

设备 Token 在设备管理器 → 设置 → 外部访问 → 「访问 Token」设置；Manager 添加设备时
需要输入同一个 Token。**只读取 overview 不会改设备配置；执行批量下发会写设备配置**，
请确认目标设备与模板。

### 5.1 添加设备

WebUI：设备页 → 添加设备 → 地址 + Token → 测试连接 → 保存。

或 curl：

    BASE=http://127.0.0.1:8765/relay/api/v1
    TOKEN=admin123

    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"name":"Pixel 9 · 主力机","baseUrl":"http://127.0.0.1:8686","token":"<设备Token>","tags":["主力"]}' \
      "$BASE/devices" | jq '{id, online, model, serverVersion, latencyMs}'

添加时会实时探测设备端 `/api/v1/status` 与 `/api/v1/overview`；失败返回
`422 device_probe_failed`，`details.reason` 为 `connect` / `dns` / `timeout` /
`unauthorized` / `forbidden_scope` / `upstream_status` / `invalid_response`。

### 5.2 验证

    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices" | jq '.devices[] | {id, online, model}'
    curl -s -X POST -H "Authorization: Bearer $TOKEN" "$BASE/devices/<id>/sync" | jq '{online: .device.online, warnings}'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/apps" | jq '{totals, rows: [.apps[] | {package, configured, total, inconsistent}]}'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/devices/<id>/overview?cached=1" | jq '{online, stale, model: .overview.device.model}'

---

## 6. 端到端冒烟（Mock 设备，无需 Android）

仓库自带 [mock_device.py](mock_device.py)：实现 status / overview / revision /
templates / apps，并在首次 `PUT /apps/{pkg}` 返回 412，用来验证 Relay 的 revision
重取与重试。

    # 终端 A：Mock 设备（18686 被占用时换成 18687 等，并同步 $DEV）
    python3 manager/docs/mock_device.py 18686

    # 终端 B：Manager（新数据目录，避免污染真实数据）
    cargo run -p device_faker_manager -- --data-dir /tmp/dfm-smoke

    # 终端 C：冒烟脚本
    set -euo pipefail
    BASE=${BASE:-http://127.0.0.1:8765/relay/api/v1}
    TOKEN=${TOKEN:-admin123}
    DEV=${DEV:-http://127.0.0.1:18686}

    # 1) 探活（无鉴权）
    curl -s "$BASE/healthz" | jq '{status, version, devices}'

    # 2) 无 Token → 401
    curl -s -o /dev/null -w '%{http_code}\n' "$BASE/devices"          # 401

    # 3) 添加 Mock 设备（探测 status + overview）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d "{\"name\":\"Mock Pixel 9\",\"baseUrl\":\"$DEV\",\"token\":\"admin123\",\"tags\":[\"mock\"]}" \
      "$BASE/devices" | jq '{id, online, model, serverVersion}'

    # 4) 列表 / 矩阵 / 模板
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

    # 8) assignments 下发（逐设备逐模板）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"assignments":[{"device":"mock-pixel-9","app":"com.xingin.xhs","template":"pixel_9"}],"restart":false}' \
      "$BASE/apply" | jq '{jobId, total, kind}'

    # 9) 总览 + 审计
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/overview" | jq '{fleet, drift: [.drift[].package]}'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/audit?limit=10" | jq '[.entries[] | {action, status, target}]'

    # 10) 代理：读设备端单应用来源（GET /api/v1/apps/{pkg}）
    curl -s -H "Authorization: Bearer $TOKEN" \
      "$BASE/devices/mock-pixel-9/proxy/api/v1/apps/com.xingin.xhs" | jq '{source, resolved}'

    # 11) Token 轮换（之后所有请求改用新 Token；旧 Token 立即 401）
    NEW_TOKEN=$(curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{}' "$BASE/token/rotate" | jq -r .token)
    curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $TOKEN" "$BASE/devices"       # 401
    curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer $NEW_TOKEN" "$BASE/devices"   # 200

    # 12) 移除设备
    curl -s -o /dev/null -w '%{http_code}\n' -X DELETE -H "Authorization: Bearer $NEW_TOKEN" "$BASE/devices/mock-pixel-9"  # 204

预期：

| 步骤 | 期望 |
| --- | --- |
| 1 | `status=ok`，`devices.total` ≥ 0 |
| 2 | `401` |
| 3 | `online=true`，`model=Pixel 9`，id 为 `mock-pixel-9` |
| 4 | 矩阵含 `com.xingin.xhs`，模板含 `pixel_9` 且 `complete=true` |
| 6-7 | job 终态 `succeeded`，`results[0].attempts=2`（首次 412 后重试） |
| 8 | `202`，`kind=assignments` |
| 9 | `fleet.devices=1`；审计含 `device.add` / `device.sync` / `apply.bulk` / `apply.assignments` / `job.finished` |
| 10 | 返回 `source.type=template` 与 `resolved.model` |
| 11 | 旧 Token `401`，新 Token `200` |
| 12 | `204`，随后 `GET /devices` 为 0 |

> 冒烟会创建 `/tmp/dfm-smoke`，第 11 步还会轮换 Token；同一数据目录重跑前先
> `rm -rf /tmp/dfm-smoke`（或把 `TOKEN` 设成 `<data-dir>/manager.token` 的当前值），否则会 401。
> 独立运行 e2e 集成测试（进程内，无需端口）：

    cargo test -p device_faker_manager --test e2e_flow

---

## 7. Token 管理

- 默认 Token `admin123` 只允许 loopback；换绑定地址必须同时换 Token。
- `POST /token/rotate` 生成 `dfm_` + 32 位 base64url（或传 `{"token":"..."}` 自定义 16–128 字符），
  原子写入 `<data-dir>/manager.token`（0600），内存立即生效，旧 Token 当场失效。
- Token 来源为 `--token` / `DFM_TOKEN` 时返回 `409 invalid_state`
  （`details.reason="token_overridden"`）；改用 `manager.token` / `manager.toml` 或轮换接口。
- WebUI 轮换后需要立刻替换本地 Token（页面会提示）。
- 设备 Token 与 Manager Token 是两套：设备 Token 只在添加 / 修改设备时输入。

---

## 8. 数据目录、备份与重置

    <data-dir>/
    ├── manager.db          # SQLite 主库（运行时伴随 -wal / -shm）
    ├── manager.key         # 设备 Token 加密密钥（32 B，0600）
    ├── manager.token       # 轮换后写入的 Manager Token（0600）
    └── manager.toml        # 可选配置

- 备份：停服后复制 `manager.db` **和** `manager.key`；缺密钥时已存设备 Token 无法解密。
  运行中备份可用 `sqlite3 manager.db ".backup manager.db.bak"`（密钥仍需单独复制）。
- 重置：停止 Manager，删除整个数据目录（或只删 `manager.db`，但保留 `manager.key` 才能继续用旧设备 Token）。
- 自动清理：启动时删除 7 天前 / 超过 200 条的 job，并只保留最近 5000 条审计。
- 同一个数据目录只能有一个 Manager 进程，否则 SQLite 会报 `database is locked`。

---

## 9. 测试与验收

    # 全部测试（单元 + 集成 5 个文件）
    cargo test -p device_faker_manager

    # 只看端到端
    cargo test -p device_faker_manager --test e2e_flow

    # 静态检查
    cargo fmt --check
    cargo clippy -p device_faker_manager --all-targets -- -D warnings

    # WebUI
    cd manager/webui && npx vue-tsc --noEmit && npm run build

当前基线（2026-11-24）：53 个单元测试 + 52 个集成测试
（`manager_api` 16、`devices_flow` 13、`apply_flow` 11、`ops_endpoints` 8、`e2e_flow` 4）。
测试全部使用进程内 Router 与临时数据目录，不占用 8765 端口、不访问真实设备。

---

## 10. 常见错误

| 现象 | 原因 / 处理 |
| --- | --- |
| `监听 127.0.0.1:8765 失败` | 端口被占用；`--bind 127.0.0.1:8766` 或结束占用进程（`lsof -iTCP:8765 -sTCP:LISTEN`） |
| 启动即报「拒绝在非 loopback 地址使用默认或过短的 Manager Token」 | 用 `--token` / `DFM_TOKEN` 设置 ≥ 8 字符的非 `admin123` Token |
| WebUI 一直弹 Token 对话框 / 401 | Manager Token 不匹配；确认 WebUI 输入的是当前 Token（轮换后旧值立即失效） |
| `422 device_probe_failed` | 地址不可达（`connect`）、DNS、超时、设备 Token 错（`unauthorized`）、设备 4xx（`upstream_status`）或响应不是合法 overview（`invalid_response`）；先用 `curl -H "Authorization: Bearer <设备Token>" http://<设备>:8686/api/v1/status` 验证 |
| `409 device_exists` | 同一 `baseUrl` 或 `id` 已注册；先删除旧设备或换 id |
| `409 invalid_state / token_overridden` | Token 来自 CLI / 环境变量，不能被 rotate；改用 `manager.token` 或重启时去掉 `--token` |
| `422 validation_failed` | 看 `error.details.errors[].path`：常见是 `devices` 为空 / 重复 / 未注册，模板缺 `model`，`limit` 越界，cursor 非法 |
| `413 body_too_large` | 请求体 > 1 MiB（含代理）；拆分请求 |
| `404 not_found`（`resource="route"`） | API 路径拼错；注意前缀是 `/relay/api/v1`，根路径只有 `/healthz` |
| `405 method_not_allowed` | 方法不对（例如对 `/apply` 用 GET） |
| `404 device_not_found` | 设备已被删除或 id 拼错；重新 `GET /devices` |
| `502 device_offline` | 设备不可达；检查 adb forward / 局域网 / 端口；聚合视图仍返回缓存 |
| `504 upstream_timeout` | 设备响应超过读超时（默认 10 s）；检查设备负载 / 网络 |
| `502 device_error` | 代理目标返回 401/403（设备 Token 失配），或响应 > 8 MiB |
| 页面显示「WebUI dist 未就绪」 | 先 `cd manager/webui && npm run build`，或 `--static-dir` 指向正确 `dist` |
| Vite 页面提示 Relay 请求失败（502） | Manager 未启动或端口不是 8765；启动 Manager 或设置 `DFM_DEV_RELAY` |
| `database is locked` | 同一数据目录跑了两个 Manager；停掉一个 |
| 设备 Token 全部解不开 / 设备列表仍在但请求 500 | `manager.key` 丢失或被替换；恢复备份，或删除设备后重新添加 |

---

## 11. 停止与清理

    # Manager：Ctrl-C（或 kill -TERM <pid>）
    # Mock 设备：Ctrl-C
    # 冒烟数据
    rm -rf /tmp/dfm-smoke

服务托管（systemd / launchd）示例见 [ARCH.md §15.3](ARCH.md#153-服务托管示例)。

---

## 12. 许可

    SPDX-License-Identifier: GPL-3.0-or-later
    Copyright (c) 2025-2026 Seyud
    Device Faker: https://github.com/Seyud/device_faker
