# 第一增量集成验收报告（devmask_server / WebUI 原生模式）

- 日期：2026-11-24
- 范围：`docs/dev/server-api-plan.md` 第一增量（P0/P1/P2 核心），不含 MCP 与 P3/P4
- 契约：[docs/API.md](../API.md)（draft-07 schema 修订后）、[docs/openapi.yaml](../openapi.yaml)
- 执行环境：
  - macOS 26（arm64），Rust `rustc 1.99.0`，`cargo-ndk 4.1.2`，NDK r30（`/opt/homebrew/share/android-ndk`）
  - Node `v26.13.1` / npm `11.20.0`
  - 真机：Pixel 9 / Android 17 / KernelSU（`su` 可用，context `u:r:ksu:s0`）
- 结论：**第一增量核心链路通过**；限流（429）与鉴权失败封禁为已知缺口（P4），真机矩阵只覆盖 KernelSU，24h 稳定性未执行。

---

## 1. 结果总览

| # | 验收项 | 结果 | 关键证据 |
| --- | --- | --- | --- |
| 1 | core 单测 | PASS | `cargo test -p devmask_core` exit 0，**51 passed / 0 failed** |
| 2 | server 单测 | PASS | `cargo test -p devmask_server` exit 0，**166 passed / 0 failed** |
| 3 | xtask 单测 | PASS | `cargo test -p xtask` exit 0，**3 passed / 0 failed** |
| 4 | WebUI 构建 | PASS | `python3 webui/build_webui.py` exit 0（npm ci + type-check + lint + vite build） |
| 4b | 测试基线自检 | PASS | `bash tests/check.sh` exit 0，12 项检查 0 失败（bash -n / py_compile / fixture / shim / mock e2e） |
| 5 | API 冒烟（真实 server） | PASS | host `tests/api/smoke.sh` exit 0；真机 exit 0 |
| 6 | If-Match 并发 | PASS | host + 真机 `tests/api/concurrency.sh` exit 0，均 `200 + 412` |
| 7 | 全量打包（xtask） | PASS | `cargo run -p xtask -- build -r` exit 0；zip 74 条目，权限正确 |
| 8 | 对照打包（pack_module） | PASS | `python3 pack_module.py` exit 0；66 条目，权限正确 |
| 9 | 真机 server | PASS | Pixel 9 上 healthz/status/config/packages、模板 CRUD、备份恢复全部通过 |
| 10 | service.sh | PASS | start/status/stop、5 秒拉起、停止后端口释放均通过 |
| 11 | WebUI 原生模式 | PASS（含说明） | 401 登录、首存、次存、412、428、422、mtime=null 均实测；409 见 §8 说明 |
| 12 | 管理器模式回归 | PASS | Vite dev mock：module 1.0.0、模板 `redmagic_9_pro`、configReady=true |
| 13 | 审计日志 | PASS | `api-audit.jsonl` 10 行，含 tokenId/tokenName/revisionBefore/After/durationMs |
| 14 | 限流 429 / 鉴权封禁 | KNOWN GAP | 40 次快速 GET 全 200；server 无 rate-limit 中间件（owner：server-dev，P4） |

---

## 2. 代码与单测

命令与退出码：

    cargo test -p devmask_core     # exit 0
    cargo test -p devmask_server   # exit 0
    cargo test -p xtask                 # exit 0

汇总（各测试二进制相加）：

| 包 | passed | failed |
| --- | ---: | ---: |
| devmask_core | 51 | 0 |
| devmask_server | 166 | 0 |
| xtask | 3 | 0 |

server 侧覆盖了 23 个 operation 的契约测试（t4 输出），本报告不重复其内部用例清单。

---

## 3. WebUI 构建

    python3 webui/build_webui.py     # exit 0

关键输出：

    npm ci 依赖一致性校验 完成
    TypeScript 类型检查 完成
    代码检查 完成
    构建生产版本 完成
    构建完成！输出目录: webui/../module/webroot

产物：`module/webroot/index.html` + `assets/*`（约 816 KB）。

---

## 4. API 冒烟（真实 host server）

启动方式：`target/debug/devmask_server` + 临时 `server.toml`（单 Token）+ `tests/fixtures/config/minimal.toml`。

    BASE=http://127.0.0.1:18880/api/v1 TOKEN=<token> bash tests/api/smoke.sh       # exit 0
    BASE=http://127.0.0.1:18880/api/v1 TOKEN=<token> bash tests/api/concurrency.sh # exit 0

覆盖：healthz / status / config / config.revision / templates CRUD / apps（绑定模板 + 内联 + explain=`GET /apps/{pkg}`）/ packages / 404 错误信封；所有写响应校验 `revision`、`previousRevision`、`X-DM-Revision` 与带引号 `ETag`。

> 本轮修正测试基线 bug：smoke 的测试包名原为 `com.devicefaker.smoke.<pid>`，包名段以数字开头不是合法 Java 标识符，真实 server 返回 422 `invalid_package`。已改为 `com.devicefaker.smoke.p<pid>`，同步收紧 `tests/api/mock_server.py` 的包名正则（每段以字母/下划线/$ 开头，userId ≤ 3 位）。改动仅在 `tests/**`。

---

## 5. 并发与一致性

host 与真机各跑一遍：

    BASE=... TOKEN=... bash tests/api/concurrency.sh   # exit 0

结果：

    并发结果：A=200 B=412
    PASS 恰好一个 200、一个 412
    PASS 败者错误码 = config_revision_mismatch
    PASS 败者 details.expected/actual 正确
    CONCURRENCY PASS

同一 If-Match 并发两次 PUT 未产生半截文件；真机测试后按备份恢复，`sha256` 与 mtime 均回到基线（见 §7）。

---

## 6. 打包与权限

### 6.1 xtask 全量构建

首次执行失败（环境缺少 NDK 变量）：

    error: Could not find any NDK.
    note: Set the environment ANDROID_NDK_HOME ...

设置 `ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk` 后重跑：

    cargo run -p xtask -- build -r    # exit 0

产物：`output/devmask-(release).zip`，**74 entries**（含目录条目），2.80 MB。

已核对的条目与权限：

| 条目 | mode | 结果 |
| --- | --- | --- |
| `bin/devmask_server` | `-rwxr-xr-x` (0755) | PASS，3,682,512 bytes |
| `service.sh` | 0755 | PASS |
| `customize.sh` | 0755 | PASS |
| `uninstall.sh` | 0755 | PASS |
| `server.default.toml` | 0644 | PASS |
| `webroot/index.html` | 存在（0644） | PASS |

### 6.2 pack_module.py 对照

    python3 pack_module.py    # exit 0

产物：`output/devmask_20261124_122311.zip`，**66 entries**（不含目录条目），2.48 MB；权限与 xtask 一致：

    -rwxr-xr-x customize.sh
    -rw-r--r-- server.default.toml
    -rwxr-xr-x service.sh
    -rwxr-xr-x uninstall.sh
    -rw-r--r-- webroot/index.html
    -rwxr-xr-x bin/devmask_server

两个 zip 的差异仅为目录条目（xtask 写入、pack_module 不写），文件集合一致。

---

## 7. 真机验收（Pixel 9 / Android 17 / KernelSU）

### 7.1 部署与启动

- `adb push target/aarch64-linux-android/release/devmask_server /data/local/tmp/df-t9/bin/`，`chmod 755`；
- `module/webroot`（56 文件）与 `module.prop` 推到 `/data/local/tmp/df-t9/`；
- 临时 `server.toml`：bind `127.0.0.1:18879`、config 指向**真实** `/data/adb/devmask/config/config.toml`、backup/audit 指向 `/data/local/tmp/df-t9`；
- 以 `su -c` 启动，`adb forward tcp:18878 tcp:18879`。

服务首次启动输出（stdout 重定向到 server.log）：

    [devmask-server] initial token = df_i8Ge...

说明：loopback + 空 token 表时自动生成 Token、只写 sha256、明文仅打印一次，与文档一致。

只读检查：

    healthz=200
    status: module=v1.7.0 device=Pixel 9/37 server.bind=127.0.0.1:18879
    config: revision=f9f80df8... size=9361 templates=26 apps=15 exists=true

### 7.2 写操作与恢复

    BASE=http://127.0.0.1:18878/api/v1 TOKEN=... bash tests/api/smoke.sh       # exit 0
    BASE=http://127.0.0.1:18878/api/v1 TOKEN=... bash tests/api/concurrency.sh # exit 0
    PORT=18878 REMOTE_PORT=18879 TOKEN=... bash tests/device/smoke.sh           # exit 0

- 配置 mtime 变化：`1793793738 → 1795494229`；
- 每次写入在临时 backup_dir 生成备份（9 份），未污染真实备份目录；
- 测试后从 `config.toml.bak` 恢复：sha256 `f9f80df8af73...`、size 9361、mtime 1793793738，与基线一致；
- 结束后设备上无 `devmask_server` 进程、无 18878/18879/18882 监听。

### 7.3 service.sh

把 service.sh 复制到 `/data/local/tmp/df-t9/module/`，用 sed 把 `RUNTIME_DIR` 改到 `/data/local/tmp/df-t9/runtime`（不改仓库文件），配 `server.toml` bind 18882：

| 步骤 | 观察 |
| --- | --- |
| `service.sh start` | supervisor pid 24819；server pid 24824；日志出现 Token banner |
| `kill -9 24824` + 等 8s | pid 变为 **25502**；healthz 200 → supervisor 5 秒拉起 PASS |
| `service.sh status` | `supervisor: running` / `server: running` |
| `service.sh stop` | `supervisor: stopped` / `server: stopped`；healthz 返回 `000`（端口释放） |

日志写入 `runtime/logs/server.log`，大小轮转与 0600 权限由脚本准备步骤保证；`--server-config` 生效（bind 来自 runtime/config/server.toml）。

> 复现提示：直接 `adb shell su -c "sh ... start"` 时，host 侧 adb 可能等待守护循环的后台 stdio 而挂住；本轮改用 `timeout 20 adb shell ...` 包裹后正常返回。这是 adb/su 的调用方式问题，不是 service.sh 缺陷。

---

## 8. WebUI 原生模式

真机/主机同源页面（server static_dir 指向当前 `module/webroot`）。

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| 401 → 登录框 | PASS | 首次打开弹出 “Sign in to devmask”，输入 Token 后 `sessionStorage.devmask_api_token` 写入 |
| 登录后加载 | PASS | Pinia store：moduleVersion=v1.7.0，templates=[pixel_9]，apps=1；网络记录 `/status`、`/config?format=toml`、`/packages?limit=500` 均 200 |
| 保存（次存） | PASS | saveConfig() 后 revision `b4f73d43… → 88c4d454…`，无错误 |
| 412 | PASS | 外部 PUT 改 revision 后再保存：错误 “Config was modified by another client; revision refreshed, please retry（request ID: req_ooZ_…）”；再次保存成功 |
| 428 一次性重试 | PASS | 拦截 fetch 删除首个 PUT 的 If-Match，server 返回 428；客户端自动重读 revision 并重发 PUT，最终 0 error |
| 422 | PASS | 构造两条同包 `[[apps]]`：错误 “Config validation failed: apps.com.example.dupapp: …（request ID: req_KlU…）” |
| 首存 `If-Match: *` | PASS | 缺失配置 server 上登录后自动创建 config.toml（42 bytes）；客户端代码 `buildIfMatch` 在 revision 为空时发送 `*`（t12 已逐请求验证），server 端 `If-Match: *` 创建成功 |
| mtime=null | PASS | 缺失配置 server：`{"mtime":null,"size":0,"exists":false,"revision":"e3b0c442…"}`，与 UI「配置缺失」分支一致（t6/t11） |
| 409 | 说明 | `PUT /config` 在真实 server 上**允许**重复包名（整份配置语义），WebUI 的 409 确认分支因此无法经保存路径触发；单独调用 `PUT /templates/{name}` 返回 409 `conflict_duplicate_package`（details.templates=[pixel_9,dup_t9]），错误信封正确 |
| 429 | KNOWN GAP | 40 次快速 GET /status：`status_200=40, status_429=0`；server 未实现限流（见 §1、§11） |

说明：共享浏览器窗口的页面 `visibilityState=hidden`，基于 rAF 的翻页与骨架动画不推进，无法用坐标完成整页点击。因此 UI 用例通过直接调用 Pinia `config` store 的 `bootstrap/saveConfig` 驱动（与页面按钮共用同一套实现），并辅以截图/登录框 a11y 证据；这是验证手段限制，不是产品缺陷。

### 管理器模式回归

    python3 webui/build_webui.py dev   # Vite v8.3.4，http://localhost:3000

在 dev 页面读取 Pinia：`moduleVersion=1.0.0`、模板 `redmagic_9_pro`、configReady=true、无 Token 登录框；`window.__DF_SERVER__` 不存在，走 DEV→ksu mock 分支。管理器模式数据路径未回归。

---

## 9. 契约与安全抽查

| 检查 | 结果 |
| --- | --- |
| 无 Token → 401 | PASS（真实 server） |
| 只读 Token 读 → 200 | PASS |
| 只读 Token 写 → 403 | PASS：`forbidden_scope {"granted":["read"],"required":"write"}` |
| 缺 If-Match → 428 | PASS（WebUI 428 用例） |
| 过期 If-Match → 412 | PASS，details.expected/actual 正确 |
| 重复包名模板写 → 409 | PASS（`conflict_duplicate_package`） |
| 语义错误 → 422 | PASS（重复 `[[apps]]`、非法包名） |
| 缺配置 → mtime=null / size=0 / exists=false | PASS |
| 注释保留 | PASS：host smoke 后 fixture 的注释行仍在（`# DevMask 最小可用配置 fixture`） |
| 审计 | PASS：`/tmp/df-t9-ui/audit.jsonl` 10 行，字段含 revisionBefore/After、tokenId、durationMs；`remote:null` 为当前实现 |
| 静态托管 | PASS：真机 `GET /` 200 且包含 `DevMask`；host 同样 |

---

## 10. 计划书第 16 节对照（仅本增量）

| 条目 | 结论 | 说明 |
| --- | --- | --- |
| 1. 无管理器浏览器管理模板/应用/设置 | PASS（链路） | 原生模式加载/保存/错误处理通过；翻页视觉交互受 hidden 浏览器限制（见 §8） |
| 2. adb forward 后浏览器/curl 可用 | PASS | 真机 18878→18879，curl + 静态页 + WebUI 均通 |
| 3. MCP 工具闭环 | N/A | 下一增量（P3） |
| 4. 并发一个 200 一个 412 | PASS | host + 真机 |
| 5. 注释/未知字段保留 | PASS | toml_edit 写入 + fixture 注释校验；未知字段由 core 单测覆盖 |
| 6. 管理器模式回归 | PASS | dev mock：module 1.0.0 / 模板 redmagic_9_pro / configReady |
| 7. 断电重启自动恢复 | PARTIAL | service.sh start/stop/restart 与 5 秒拉起已验证；真机重启未执行（避免影响设备） |
| 8. 审计包含 revision 与 actor | PASS | tokenId/tokenName/revisionBefore/After；remote 暂为 null |
| 9. OpenAPI 契约 + 错误信封 | PASS | t4 23 op 契约测试；本轮实测 401/403/404/409/412/422/428 信封 |
| 10. 只读 Token 无法写；无 Token 无法读 | PASS | 401/403 实测 |
| 11. 24h 稳定性 | N/A | 未执行，列入下一增量 |
| 12. 文档齐全 | PASS | API.md / SERVER.md 已对齐；llms.txt 早前已更新，报告补齐 |

---

## 11. 已知差距与建议

| 缺口 | 影响 | 建议 / Owner |
| --- | --- | --- |
| 429 限流未实现；鉴权失败 10 次/分钟封禁未实现；请求超时未强制 | 高频写入/暴力猜 Token 无服务端保护；客户端 429 分支为防御性代码 | server-dev，P4 加中间件；已在 API.md §1.2/§2.9 标注 |
| WebUI 409 确认分支无法经 `PUT /config` 触发（整份配置允许重复包名） | 409 UX 在当前 server 上不可达；模板/Packages 端点仍会 409 | api-architect + webui-dev 下一增量决定：整份配置是否也做重复包名校验，或删除该 UI 分支 |
| service.sh 不支持 `RUNTIME_DIR` 环境变量覆盖 | 冒烟需复制脚本并 sed（本轮做法）；真机安装不受影响 | infra-dev，可加一行 `RUNTIME_DIR=${RUNTIME_DIR:-/data/adb/devmask}`（下一增量） |
| 构建强依赖 `ANDROID_NDK_HOME` | 未设置时 xtask 直接失败（本轮已复现） | infra-dev：文档已补；可在 xtask 里探测常见路径，失败信息指向 SERVER.md |
| Magisk 矩阵 / Android 12/14 / 断电重启 / 24h 稳定性未跑 | 兼容性结论仅 KernelSU + Android 17 | qa/infra，下一增量按测试矩阵补 |
| 审计 `remote` 恒为 null | 审计少来源 IP | server-dev，接入 `ConnectInfo` 后补 |

---

## 12. 本轮改动（仅验收范围）

- `tests/api/smoke.sh`：测试包名改为合法 Java 标识符（`com.devicefaker.smoke.p<pid>`）。
- `tests/api/mock_server.py`：包名正则与 core 对齐（每段以字母/下划线/$ 开头，userId 0–999）。
- `docs/API.md`：
  - §1.2、§2.9 标注限流/鉴权封禁/请求超时为已知缺口（本增量不返回 429）；
  - §3.6 审计字段补 tokenName/remote/userAgent，remote 当前为 null；
  - §5.2 status 示例与字段表补 `server.pid`；
  - §8.1 包名规则 userId 范围改为 0–999。
- `docs/SERVER.md`：构建前必须设置 `ANDROID_NDK_HOME`；排障表补 NDK/429/审计/requestId。
- 新增本报告。
- 未改业务代码，未 commit。

---

## 13. 复现命令速查

    # 单测
    cargo test -p devmask_core && cargo test -p devmask_server && cargo test -p xtask

    # WebUI
    python3 webui/build_webui.py

    # 打包
    export ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk
    cargo run -p xtask -- build -r
    python3 pack_module.py

    # 真实 server 冒烟（host）
    BASE=http://127.0.0.1:18880/api/v1 TOKEN=<token> bash tests/api/smoke.sh
    BASE=http://127.0.0.1:18880/api/v1 TOKEN=<token> bash tests/api/concurrency.sh

    # 真机（server 已在 18879 运行）
    adb forward tcp:18878 tcp:18879
    BASE=http://127.0.0.1:18878/api/v1 TOKEN=<token> bash tests/api/smoke.sh
    PORT=18878 REMOTE_PORT=18879 TOKEN=<token> bash tests/device/smoke.sh

    # 恢复真机配置（本轮）
    adb shell 'su -c "cp -p /data/local/tmp/df-t9/config.toml.bak /data/adb/devmask/config/config.toml"'

---

## 14. 下一增量建议

1. 限流中间件（读/写/鉴权失败），补 429 与审计；请求超时保护。
2. 明确整份配置重复包名策略，统一 WebUI 409 分支。
3. service.sh 支持 `RUNTIME_DIR` 覆盖，方便自动冒烟。
4. 执行 Magisk / Android 12/14 / 断电重启 / 24h 稳定性矩阵。
5. 下一增量 P3：MCP、plan/apply、OpenAPI 契约测试自动化。
