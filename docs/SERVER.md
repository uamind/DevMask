# Device Faker 原生 Web 服务（device_faker_server）

> 状态：**草稿 / 第一增量**。本文覆盖「二进制怎么构建、怎么装进模块、开机怎么启动、端口与 Token 怎么用、出问题怎么查」。
> 接口契约（端点、错误码、鉴权语义）以 [API.md](API.md) 为准，本文不重复。

## 1. 它是什么

`device_faker_server` 是随模块发布的原生 HTTP 服务（Rust + axum，arm64 单个静态可执行文件），负责两件事：

1. **托管 WebUI**：直接读 `module/webroot`，手机浏览器打开 `http://127.0.0.1:8686` 就能管理，不再依赖管理器 WebView；
2. **提供 `/api/v1`**：给脚本、AI 与自动化用的版本化 REST 接口（配置 / 模板 / 应用读写 + 状态查询）。

从 v1.10.0 起它还负责**出口代理（egress）**：把整机的 HTTP 代理写进 Android 全局设置、管理 `egress.toml` 里的代理定义与密码，并提供出口 IP 探测（见 §10）。

它与 Zygisk 共用同一条生效链路：写配置仍然落到 `/data/adb/device_faker/config/config.toml`，companion 靠 mtime 变化重新渲染，**没有第二套配置来源**。

本增量（P0/P1/P2 核心）不做：MCP、plan/apply、备份管理界面、审计查询、Token 管理界面、图标与应用名伪装、TLS、`/intents`。未实现的路径按契约返回 404 错误信封。

## 2. 文件与权限

| 路径 | 用途 | 权限 |
| --- | --- | --- |
| `/data/adb/modules/device_faker/bin/device_faker_server` | 服务二进制 | `0755` |
| `/data/adb/modules/device_faker/webroot/` | WebUI 静态资源 | `0755` / `0644` |
| `/data/adb/modules/device_faker/service.sh` | 开机启动脚本 | `0755` |
| `/data/adb/modules/device_faker/server.default.toml` | 默认服务设置（安装时复制一份） | `0644` |
| `/data/adb/device_faker/config/config.toml` | 唯一事实来源（Zygisk 也读它） | `0644` |
| `/data/adb/device_faker/config/server.toml` | 服务设置 + **Token 的 sha256** | `0600` |
| `/data/adb/device_faker/config/backups/` | 写配置前的备份（保留 10 份） | `0700` |
| `/data/adb/device_faker/config/egress.toml` | 代理定义与探测策略（**含密码**，不进导出、不进任何 API 明文） | `0600` |
| `/data/adb/device_faker/config/backups/egress.toml.<stamp>-<rev8>.bak` | egress 写入前的备份（保留 10 份，同样含密码；`stamp` = `YYYYMMDDTHHMMSS.nnnnnnnnnZ`） | `0600` |
| `/data/adb/device_faker/logs/server.log` | 服务日志（含首次启动的 Token 明文横幅） | `0600` |
| `/data/adb/device_faker/logs/server.log.1` | 轮转后的上一份日志 | `0600` |
| `/data/adb/device_faker/server.pid` | server 进程 pid | `0644` |
| `/data/adb/device_faker/server.supervisor.pid` | 守护循环 pid | `0644` |

日志里会出现 Token 明文（仅首次生成时一行），所以 `server.log` 固定 `0600` —— 不要把它贴到公开 issue 里。

## 3. 首次启动与 Token

首次启动（`bind` 是 loopback 且 `server.toml` 里一个 Token 都没有）时，服务会：

1. 写入默认 Token `admin123`（`id = default`、`name = owner`、`scopes = [read, write, ops, admin]`）；
2. 只把 **sha256** 写进 `server.toml`（原子写，`0600`）；
3. 明文往 **stdout 打印一次**，`service.sh` 把 stdout 重定向进 `logs/server.log`：

```sh
# 手机（root）上取出 Token
su -c "grep 'initial token' /data/adb/device_faker/logs/server.log"
# [device-faker-server] initial token = admin123
```

拿到后用法：

```sh
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8686/api/v1/status
```

注意事项：

- 明文**只出现这一次**（重启不会重新生成，也不会再打印）；丢了就在 WebUI 里重设，或删掉 `server.toml` 里对应的 `[[tokens]]` 条目再重启服务（会写回 `admin123`）；
- 覆盖安装不会覆盖已有的 `server.toml`：旧 Token 继续有效；要换成默认值，在 WebUI 的「访问 Token」填 `admin123` 保存，或点 Reset；
- `bind` 不是 loopback 时**不会**自动生成，且没有 Token 会直接拒绝启动（避免管理面无认证暴露到网络）；
- 作用域不隐含：`write` 不含 `read`，`admin` 是四个作用域的并集，按需给。

想自己指定 Token（任选一种）：

- 管理器 WebUI → 设置 → 外部访问 → 「访问 Token」填值后保存（自动算 sha256、写盘、重启）；
- 或用下面的命令自己算哈希后写 `server.toml`：

```sh
TOKEN=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')
echo "$TOKEN"                                   # 保存好，只在这里出现
printf '%s' "$TOKEN" | sha256sum                # 把 64 位小写十六进制填进 server.toml
```

```toml
[[tokens]]
id = "laptop"
name = "macbook"
hash = "sha256:把 64 位十六进制粘在这里"
scopes = ["read", "write"]
```

## 4. 服务设置：server.toml

安装时 `customize.sh` 把 `module/server.default.toml` 复制到 `/data/adb/device_faker/config/server.toml`（**已存在则不覆盖**）。默认值：

```toml
bind = "127.0.0.1:8686"
static_dir = "/data/adb/modules/device_faker/webroot"
config_path = "/data/adb/device_faker/config/config.toml"
backup_dir = "/data/adb/device_faker/config/backups"
audit_log = "/data/adb/device_faker/logs/api-audit.jsonl"
allow_lan = false
body_limit_bytes = 2097152
read_rate_per_min = 120
write_rate_per_min = 30
```

| 键 | 说明 |
| --- | --- |
| `bind` | `ip:port`。后缀非 loopback 时必须同时 `allow_lan = true` 且至少一个 Token，否则拒绝启动 |
| `static_dir` | WebUI 目录；缺失时服务照常起，只是页面 404（`/api/v1` 不受影响） |
| `config_path` | config.toml 路径；`server.toml` 缺省与它同目录 |
| `backup_dir` / `audit_log` | 本增量只透传，写入逻辑留给下一增量 |
| `allow_lan` | 是否允许监听非 loopback 地址 |
| `body_limit_bytes` | 请求体上限（默认 2 MiB） |
| `read_rate_per_min` / `write_rate_per_min` | 限流配额（本增量只解析与透传） |
| `[[tokens]]` | `id` / `name` / `hash`（sha256，可带 `sha256:` 前缀）/ `scopes` |

优先级：**命令行 > server.toml > 设备默认值**。改完重启服务生效。

## 5. 开机启动：service.sh

`service.sh` 由 Magisk / KernelSU 在 `late_start service` 阶段以 root 执行：

1. 建运行时目录、把 `server.log` 拉到 `0600`；
2. 等 `sys.boot_completed=1`（最多 300 × 2s = 10 分钟，超时也照常启动，只记一行日志）；
3. 模块被禁用（存在 `$MODDIR/disable`）或二进制不可执行时，记一行日志后退出，不空转；
4. 起一个守护循环：拉起 `bin/device_faker_server`，进程退出后 **5 秒**重新拉起；
5. 日志超过 **1 MiB** 时在两次启动之间轮转成 `server.log.1`。

只允许一个实例：守护循环 pid 写在 `server.supervisor.pid`，已在运行就直接退出。

手动控制（需要 root）：

```sh
MOD=/data/adb/modules/device_faker
su -c "sh $MOD/service.sh status"     # 看 supervisor / server 是否在跑
su -c "sh $MOD/service.sh restart"    # 改完 server.toml 后重启
su -c "sh $MOD/service.sh stop"       # 停止（含守护循环）
su -c "sh $MOD/service.sh"            # 等价 start
```

卸载时 `uninstall.sh` 会先停守护循环、再停 server，最后删 `/data/adb/device_faker`；顺序不能反，否则守护循环会在 5 秒后把进程重新拉起来。

## 6. 访问方式

| 场景 | 做法 |
| --- | --- |
| 手机浏览器 | 打开 `http://127.0.0.1:8686` |
| 电脑（USB） | `adb forward tcp:8686 tcp:8686`，然后开 `http://127.0.0.1:8686` |
| 局域网 | `server.toml` 改 `bind = "0.0.0.0:8686"` + `allow_lan = true` + 配好 Token，重启服务，访问 `http://<手机IP>:8686` |
| 脚本 / AI | `GET /api/v1/status` 摸状态，其余见 [API.md](API.md) |

WebUI 会自动选择传输层：服务端注入的 `window.__DF_SERVER__` 存在时走 HTTP（原生模式），否则回退管理器模式（`window.ksu`），两者都没有就只读。

### 6.1 管理器模式：外部访问卡片（推荐）

管理器（KernelSU / Magisk）打开 WebUI → **Settings → External access**。该卡片只在
`window.ksu` 管理器模式出现；原生 HTTP 模式与只读模式不显示（它们没有 root 写
`server.toml` / 调 `service.sh` 的能力）。

| 操作 | 行为 |
| --- | --- |
| 打开开关 + 端口 → Save | 把 `bind` 改为 `0.0.0.0:<port>`、`allow_lan = true`；缺 Token 时写入默认 `admin123` 的哈希；执行 `service.sh restart`；明文只在弹窗显示一次 |
| 关闭开关 → Save | 改为 `bind = "127.0.0.1:<port>"`、`allow_lan = false`；保留已有 Token |
| 访问 Token 输入框 + Save | 非空时清空所有 `[[tokens]]`，写入该 Token 的 sha256（`id="default"`、全量作用域），重启服务 |
| Reset token | 清空所有 `[[tokens]]`，写回默认 `admin123`，重启服务 |
| Refresh | 读取 `service.sh status` 与 `ip -4 addr` 的 wlan/eth/ap 地址，列出 `http://<IP>:<port>` |

写盘保留 `server.toml` 的注释与未知字段，权限固定 `0600`。卡片上的「服务状态」
来自 `service.sh status`（supervisor/server 两行）；二进制缺失时提示重新安装模块。

**安全提醒**：打开后同一局域网内任何人都能访问页面与 API，必须依赖 Token；
本增量没有 TLS，**不要**把端口映射到公网。公共 Wi-Fi / 访客网络通常有 AP 隔离，
即使配置正确也连不上，属于网络侧限制，不是模块故障。更安全的远程方式是
`adb forward` 或 Tailscale 之类的加密隧道。

### 6.2 手工配置与验证（无管理器 UI 时）

```sh
# 开：0.0.0.0:8686 + allow_lan + Token（哈希用 sha256sum 生成）
su -c 'sed -i "s/^bind = .*/bind = \"0.0.0.0:8686\"/; s/^allow_lan = .*/allow_lan = true/" /data/adb/device_faker/config/server.toml'
su -c 'sh /data/adb/modules/device_faker/service.sh restart'
su -c 'sh /data/adb/modules/device_faker/service.sh status'

# Mac 与手机同一局域网，直接访问手机 IP
curl -H "Authorization: Bearer $TOKEN" http://192.168.1.50:8686/api/v1/status
curl -H "Authorization: Bearer $TOKEN" http://192.168.1.50:8686/api/v1/config

# 关：恢复 loopback（Token 保留）
su -c 'sed -i "s/^bind = .*/bind = \"127.0.0.1:8686\"/; s/^allow_lan = .*/allow_lan = false/" /data/adb/device_faker/config/server.toml'
su -c 'sh /data/adb/modules/device_faker/service.sh restart'
```

手机 IP 用 `ip -4 addr show wlan0` 查看；端口被占用时先 `ss -ltn | grep <port>`。
修改后必须 restart，server 只在启动时读取 `server.toml`。

## 7. 构建与打包

### 7.1 只构建服务二进制

```sh
export ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk   # NDK r30
# cargo-ndk 4.x：短参数是 -P（3.x 是 -p），长参数 --platform 两代通用
cargo ndk -t arm64-v8a --platform 29 build -p device_faker_server --release
ls -lh target/aarch64-linux-android/release/device_faker_server   # ≈ 2.8 MB
```

> 体积目标 < 8 MB：靠根 `Cargo.toml` 的 `lto = "fat"`、`strip = true`、`codegen-units = 1`、`panic = "abort"`。

### 7.2 构建整个模块（推荐）

```sh
export ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk   # 必需；未设置时 cargo ndk 报 Could not find any NDK
cargo run --package xtask -- build -r   # CI（.github/workflows/build.yml）用的就是这个形式
# 本地 .cargo/config.toml 配了别名时，也可以简写成 cargo xtask build -r
```

`xtask build` 依次做：zygisk（`libzygisk.so`）→ `device_faker_server`（`--platform 29`，默认 toolchain）→ WebUI（`npm install` + `npm run build`；要跑完整检查用 `python3 webui/build_webui.py`）→ 把 `module/` 复制到 `output/.temp/release` → 放 `zygisk/arm64-v8a.so` 与 `bin/device_faker_server`（`0755`）→ 打 zip（`*.sh` 与 `bin/*` 条目写成 `0755`）→ `output/device_faker-(release).zip`。

### 7.3 只重新打包现有 module/

```sh
python3 pack_module.py              # 自动带 target/.../release|debug 下的 server 二进制
python3 pack_module.py --no-server  # 只出 WebUI + 脚本（不带服务）
```

`pack_module.py` 会把 `module/**` 与 `bin/device_faker_server` 一起写进 zip，并按 `0755` / `0644` 写 unix 权限；找不到二进制时只警告。

## 8. 排障

| 现象 | 排查 |
| --- | --- |
| `curl` 连不上 | `su -c "sh /data/adb/modules/device_faker/service.sh status"`；再看 `logs/server.log` 最后 50 行 |
| 日志里 `绑定 ... 失败` | 端口被占用。`su -c "cat /proc/net/tcp \| grep :21EE"`（`8686 = 0x21EE`）或改 `server.toml` 的 `bind` |
| 日志里 `allow_lan` 报错 | 把 `bind` 改成非 loopback 时必须 `allow_lan = true` 且配 Token |
| `/healthz` 200 但页面 404 | `static_dir` 不对，或 `module/webroot` 没打进模块（`python3 webui/build_webui.py` 后重新打包） |
| 401 | 没带 `Authorization: Bearer <token>`，或 Token 不对（`server.toml` 只存 sha256，没法反查） |
| 403 | Token 作用域不足；`write` 不含 `read`，看响应 `details.required` 与 `details.granted` |
| 服务反复重启 | `logs/server.log` 里每次退出都有 `status=...`；看启动失败原因（配置解析、端口、Token 格式） |
| Token 明文丢了 | 删 `server.toml` 里的 `[[tokens]]` 块 → `service.sh restart` → 重新 grep `initial token` |
| 二进制无法执行 | `ls -l .../bin/device_faker_server` 必须是 `0755`；不对就 `chmod 755` 或重装模块 |
| 外部访问卡片看不到 | 只在管理器（ksu）模式显示；原生 HTTP / 只读模式不渲染。确认是从管理器 WebView 打开 |
| 开关保存后 LAN 连不上 | 检查 `grep -E "^(bind\|allow_lan)" server.toml`、`ss -ltn` 是否监听 `0.0.0.0`、路由器是否开 AP 隔离/防火墙；手机和电脑是否同一网段 |
| 改端口不生效 | 需要 Save 后 `service.sh restart`（卡片会自动重启）；检查旧端口是否仍被其它进程占用 |
| Token 弹窗忘了保存 | 明文只显示一次；用 Reset token 重新生成，然后在浏览器重新登录 |
| 构建报 `Could not find any NDK` | 先 `export ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk`（或用 Android Studio 的 NDK 路径）再跑 xtask |
| 看不到 429 | 本增量**未实现限流**（known gap，P4）；`read_rate_per_min` / `write_rate_per_min` 只解析不执行 |
| 审计在哪里 | `server.toml` 的 `audit_log`（默认 `/data/adb/device_faker/logs/api-audit.jsonl`），每次写尝试一行 JSONL；`remote` 暂为 null |
| 需要 requestId 对日志 | 响应头 `X-Request-Id` 与错误信封里的 `requestId` 相同，可在 `server.log`/`api-audit.jsonl` 里 grep |

Magisk 与 KernelSU 的 SELinux 域不同，个别机型可能出现「能启动但绑不上端口」。第一增量已在 KernelSU 域验证；Magisk 环境请复验，若受限可作为已知问题记录（回退方案：Unix socket + `adb forward`，见计划书 8.3）。

## 9. 安全说明

- 默认只绑 loopback；要暴露到局域网必须**显式**三件事：改 `bind`、开 `allow_lan`、配 Token，缺一个都拒绝启动；
- 管理器「外部访问」卡片只改这三件事并重启服务；局域网内无 TLS，仅建议在可信网络使用，或改用 `adb forward` / 加密隧道；
- `server.toml` 只存 sha256（`0600`），比较用常量时间；Token 明文只在首次生成时出现一次；
- 写入配置走 `toml_edit` 定点修改（保留注释与键顺序）+ flock + 原子写 + 备份，并支持 `If-Match` 防并发覆盖（详见 [API.md](API.md) 第 2、3 节）；
- 静态托管做了目录穿越防护（百分号解码后拒 `..`、反斜杠、NUL，`canonicalize` 后必须仍在 webroot 内）；
- `/api/v1/*` 一律 `Cache-Control: no-store`。

## 10. 出口代理与设备网络（egress）

服务是设备侧唯一会改「网络出口」的组件。两种粒度、两套机制：

| 粒度 | 服务做什么 | 作用面 |
| --- | --- | --- |
| 设备级（方案 A） | 写 Android 全局代理：`settings put global http_proxy <host>:<port>`（可选 `global_http_proxy_exclusion_list`） | 整机尊重系统代理的应用；**仅 HTTP/HTTPS，不承载认证** |
| 应用级（方案 B） | 通过 companion 把解析好的代理下发给 Zygisk，在目标进程内注入 `http/https/socksProxy` 与 `ProxySelector` | 只有目标应用；SOCKS5 可用，但**带认证的代理本期不可用**（见 §10.7）；**不动系统设置** |

### 10.1 三条必须记住的命令规则

| 目的 | 命令 | 备注 |
| --- | --- | --- |
| 设置 | `settings put global http_proxy 198.51.100.10:8080` | 立即生效，`dumpsys connectivity` 显示 `Global proxy: [host] port: N` |
| 读回校验 | `settings get global http_proxy` | 返回 `host:port` / `:0` / `null`；服务再用 `dumpsys connectivity` 的 `Global proxy:` 行交叉验证 |
| **清除** | `settings put global http_proxy :0` | **只能用这个**：`settings delete global http_proxy` 在 Android 17 上**清不掉**（`global_http_proxy_host/port` 残留、代理继续生效），Pixel 9 / Android 17 实测 |

实测环境：Pixel 9（`4A190DLAQ002QR`）、Android 17 / SDK 37、KernelSU；服务进程 uid 0、域 `u:r:ksu:s0`，与 `settings` 所需权限一致。

服务内部只有**一份**清除实现（`:0` + 排除列表写空串），`DELETE /api/v1/egress`、fail-open 降级、启动自愈、卸载脚本都复用它——手工操作时也请照做。

### 10.2 生效、自愈与降级

- **生效**：设备级立即生效（新连接即走代理，已建立的连接要应用重连）；应用级在目标应用**重启后**生效。
- **启动自愈**：服务开始监听后异步按当前配置重算期望值，读回不一致就重写一次（失败只记 warn，不影响 `/healthz`）。
- **fail-open**（`egress.toml` 的 `[probe].fail_open`，默认 `true`）：显式探测失败（`connect`/`dns`/`timeout`/`tls`）、写入后读回不一致、启动自愈读回不一致——三者任一发生时把系统代理切成直连并标记 `health.degraded`；配置本身不变，一次成功探测即恢复。`fail_open = false` 时判定照做但不动系统代理。
- **幂等**：读回值已等于期望值时不再执行 `settings put`；`PUT /api/v1/egress` 在期望状态与现状一致时也不落盘（`changed:false`）。

### 10.3 探测

设备上没有 `curl` / `wget`（只有 `nc`、`ping`），所以出口 IP 探测是**服务自己发请求**：

- `POST /api/v1/egress/probe`，URL 取 `egress.toml` 的 `[probe].url`（默认 `http://checkip.amazonaws.com/`，只接受 `http://`），超时 5 s（1–30 可配）。
- 直连 / HTTP 代理（absolute-form）/ SOCKS5（含用户名密码认证）三条路径都由服务实现；响应体上限 4 KiB、不跟随重定向。
- **探测链路 ≠ 应用链路**：服务自己的客户端在 SOCKS5 下可以发用户名密码认证，但应用进程里的 libcore（经方案 B 注入）不会发送凭据（§10.7）。所以带凭据的代理可能「探测成功、应用仍出不去」，别拿探测结果反推应用可用性。
- 失败返回 `502 egress.probe_failed`（`reason` = `connect`/`dns`/`timeout`/`tls`/`http_status`/`invalid_response`）；未配置出口返回 `409 egress.not_configured`。
- 探测**不回落**：失败就是失败；fail-open 只影响流量链路。

### 10.4 凭据与文件权限

- 代理定义与密码写在 `/data/adb/device_faker/config/egress.toml`，权限 **0600**（目录 0700，备份 0600）；`config.toml` 保持 0644，只放**代理名引用**（`[egress].default` / `[[apps]].egress`）。
- 写文件走 `O_CREAT|O_EXCL` 一步 0600 + 临时文件 → `fsync` → `rename` → `fsync(dir)`，不会出现半截文件或「先 0644 后 chmod」的窗口。
- 权限被改宽时读取会自愈回 0600（日志一行 `egress.toml 权限过宽，已自愈为 0600`，`path`/`mode` 字段）并给 `egress_perm_fixed` 警告。
- 备份与 `backupId`：写入前备份到 `backups/egress.toml.<stamp>-<rev8>.bak`（0600），响应里的 `backupId` 就是 `<stamp>-<rev8>`（例如 `20261201T030000.987654321Z-5b1f2c3d`）。排查历史版本：
  ```sh
  adb shell su -c 'ls -lt /data/adb/device_faker/config/backups/egress.toml.*.bak'
  adb shell su -c 'ls -lt /data/adb/device_faker/config/backups/config.toml.*.bak'
  ```
- 应用级注入的「后置重放」如果**一次都没命中** hook（`setpriority`/`__system_property_get` 都没触发），会记一行（logcat 标签 `DeviceFaker`，模块日志同文）：
  `[egress] replay hook not fired within 2000ms (no replay started); module retained (code pages stay mapped for this process) package=… waited=2000ms`。
  - **两个触发时机**：① 重放窗口（`REPLAY_WINDOW_MS`，默认 2000ms）到点的 `alarm` 定时器——到点仍未 spawn 就打，并**立刻**还原 `SIGALRM` 处置；② 进程在窗口内退出时由 `atexit` 兜底补一行（`waited=` 是实际等待毫秒数，定时器那条是窗口值）。两个时机共用「只报一次」的开关。
  - **正常路径不打印**：hook 命中（通常 ~26ms）会 `alarm(0)` 撤销定时器、还原信号处置并压住退出兜底，所以日志与未加诊断前完全一致。
  - **打印后模块代码页仍然保留**：`armed` 的进程本来就跳过 `dlclose`，而且这里不能卸载——重放线程与 hook 的代码都在 `.so` 里，卸载即崩（行文案的 `module retained (code pages stay mapped for this process)` 就是这件事；按名字扫 `/proc/<pid>/maps` 仍看不到，见 [CONFIG.md](CONFIG.md) 的「已知限制」）。
  - **SIGKILL**（AMS 杀进程的主要方式）下两个时机都不执行，只能靠「logcat 里没有 `replay start`」判断。
  - 它与 `replay hook armed`（武装成功）、`replay hook unavailable`（没挂上）、`replay hook fired`（命中了）一起构成诊断四行。
- 任何 API 响应 / 日志 / 审计都**不含**密码与用户名，唯一信号是 `hasCredentials`；`egress.toml` 不进 `GET /config?format=toml` 导出。

### 10.5 卸载与禁用清理

`uninstall.sh` 的顺序是：停守护循环 → 停 server → **清系统代理（`settings put global http_proxy :0`）** → 删 `/data/adb/device_faker`。前两步顺序不能反（否则守护循环 5 秒后会把进程拉回来）。

⚠️ **手动「禁用模块」（管理器里关掉开关）不会执行 `uninstall.sh`**：服务不启动，就没有进程替你清系统代理，设备会继续走那条可能已经失效的代理。禁用前请先执行一次 `DELETE /api/v1/egress`（WebUI 设置页「网络出口」开关等价），或者在禁用后用 root 手工清：

```sh
su -c 'settings put global http_proxy :0'
su -c 'settings get global http_proxy'                              # :0 或 null
su -c 'dumpsys connectivity | grep -i "global proxy"'                # Global proxy: none
```

### 10.6 排障速查

| 现象 | 排查 |
| --- | --- |
| 设了代理但应用还是直连 | `settings get global http_proxy` 看是否写入；`dumpsys connectivity \| grep -i "global proxy"` 看框架是否认；应用是否走的 native 栈（设备级不覆盖） |
| 清除后代理仍生效 | 用了 `settings delete`。改 `settings put global http_proxy :0` |
| `GET /api/v1/egress` 返回 500 `egress_parse` | `egress.toml` 损坏；用 `PUT /api/v1/egress` + `If-Match: *` 重写修复 |
| `applied:false` | 读回值与期望不一致（`readback` 里有原文）；`settings` 是否可用（503，`details.component="settings"`）、SELinux 是否放行 |
| 探测 502 `egress.probe_failed` | 看 `details.reason`：`dns`/`connect` 多为代理不可达；`http_status` 是探测站点返回非 2xx；`invalid_response` 是响应不是 IP 字面量 |
| 探测 409 `egress.not_configured` | 设备级没有 `default` 且没有定义；先 `PUT /api/v1/egress` |
| 应用级配了但没效果 | 应用需**重启**；native 栈（cronet/QUIC）不覆盖；`__DIRECT__` 会主动覆盖系统代理，检查配置是否写反 |
| `health.degraded=true` | fail-open 已把系统代理切直连；修好代理后跑一次 `POST /api/v1/egress/probe` 即恢复 |

### 10.7 应用级注入的已知代价与限制（模块侧）

服务只负责设备级（方案 A）。应用级（方案 B）由 Zygisk 在目标进程内完成，因此下面这些代价与限制不在服务日志里，而在 **logcat / 模块日志**里：

| 代价 | 表现 | 观测 |
| --- | --- | --- |
| 显式配置 `egress` 的进程**保留模块代码页** | 进程内常驻模块代码（重放线程 + PLT hook 都在 `.so` 里，不能提前 `dlclose`），但**按名字扫 `/proc/<pid>/maps` 看不到**：Zygisk 用无名/匿名映射，实测 armed 进程 3170 行里 `libzygisk` 命中 0；只有 **companion 进程**的 maps 能看到 `…/zygisk/arm64-v8a.so`。真判断要比**匿名可执行段**（对比未配置进程的 `r-xp` 匿名段） | 未配置应用级出口的进程不受影响，照旧立即卸载 |
| **带用户名/密码的代理本期不可用** | 应用流量不发认证：SOCKS5 的 `java.net.socks.username/password` 进程内可读，但 libcore 不发送（代理侧 `methods=0002 user_ok=0 pass_ok=0`、最终 `auth-failed`）；HTTP Basic 未实现（只注入 host/port） | `[egress] unsupported-auth package=… type=http`（HTTP）；SOCKS5 看代理自身的 `auth-failed` 日志。可用 **IP 白名单**授权，或等 P2「设备内本地认证中继」（尚未实现） |
| **运行中修改设备级代理不重放** | 应用冷启动后的重放窗口只有 0–2000ms；跑起来之后再改 `[egress] default`，该进程不会跟着变 | 重启应用（或 `POST /api/v1/apps/{pkg}/restart`）后生效 |
| **`setpriority` hook 未命中时退化为一次性注入** | 注入值写进去了，但可能被框架的 `updateHttpProxy` 覆盖：应用级 HTTP 出口不生效、`__DIRECT__` 拉不回直连 | `adb logcat -s DeviceFaker` 里的 `[egress] replay hook unavailable reason=...` |

机制与重放时间表见 [CONFIG.md「应用级注入如何生效」](CONFIG.md#应用级注入如何生效后置重放)。

## 11. 许可

GPL-3.0-or-later，`Copyright (c) 2025-2026 Seyud`。分发模块 zip（含 `bin/device_faker_server`）时必须同时提供对应完整源码，详见 [README.md](README.md) 的「许可与二次分发」。
