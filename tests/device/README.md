# 真机测试（tests/device）

通过 `adb forward` 访问设备上的 device_faker_server，验证「服务已随开机启动」
与只读接口的连通性。可选地把一份 server 二进制推到 `/data/local/tmp` 临时运行，
用于 server 尚未打进模块时的早期联调。

## 前提

- 设备已连接（`adb devices` 显示 `device`）；
- 模块已安装，`service.sh` 已启动 `device_faker_server`；
- 或提供 `STAGE_BIN` 让脚本临时推送并启动；
- 主机有 `curl`、`python3`。

## 用法

```bash
# 默认：复用设备上已在运行的服务
bash tests/device/smoke.sh

# 指定设备与 Token
DEVICE=4A190DLAQ002QR TOKEN=df_xxx bash tests/device/smoke.sh

# 设备端临时启动（server 尚未入模块时）
STAGE_BIN=target/aarch64-linux-android/release/device_faker_server bash tests/device/smoke.sh

# 同时推送 webroot，检查静态首页
STAGE_BIN=... STATIC_DIR=module/webroot bash tests/device/smoke.sh
```

## 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `ADB` | `adb` | adb 可执行文件 |
| `DEVICE` | 空 | `adb -s` 序列号，多设备时必填 |
| `PORT` | `8686` | 主机转发端口 |
| `REMOTE_PORT` | `= PORT` | 设备端口 |
| `TOKEN` | 空 | Bearer Token（服务启用鉴权时必填） |
| `STAGE_BIN` | 空 | 可选：推送并临时启动的 server 二进制 |
| `STATIC_DIR` | 空 | 可选：随 `STAGE_BIN` 推送的 webroot 目录 |
| `REMOTE_DIR` | `/data/local/tmp/device_faker` | 设备端临时目录 |
| `LAUNCH_CMD` | 见脚本注释 | 设备端启动命令，必须与 server 实际 CLI 一致 |
| `STATIC_CHECK` | `1` | 是否检查 `GET /` 静态首页 |
| `KEEP_FORWARD` | `0` | `1` = 结束时保留 adb forward |
| `STOP_STAGED` | `1` | `1` = 结束时停止 STAGE_BIN 启动的服务 |

## 覆盖范围

1. `adb get-state` 必须是 `device`，打印机型/Android 版本/su 可用性；
2. 建立 `adb forward tcp:<PORT> tcp:<REMOTE_PORT>`；
3. 若端口已有服务则复用；否则用 `STAGE_BIN` 推送、`chmod 755`、后台启动并等待 healthz；
4. `GET /api/v1/healthz`（无鉴权）200；
5. `GET /api/v1/status` 200，`module.id=device_faker`，打印 `device.model` / `server.bind` / revision；
6. `GET /api/v1/config` 与 `/config/revision`，revision 三者一致；
7. `STATIC_CHECK=1` 时 `GET /` 200 且包含 `Device Faker`；
8. 退出时按 `KEEP_FORWARD` / `STOP_STAGED` 清理转发与临时进程。

### 期望输出（服务已运行）

    [api-test] BASE=http://127.0.0.1:8686/api/v1  TOKEN=<空：匿名/本机未启用鉴权>
    [api-test] 设备：Pixel 9 / Android 17 / su=yes
    [api-test] adb forward tcp:8686 -> tcp:8686
    [api-test] 端口 8686 已有服务，复用现有进程
    [api-test] == GET /api/v1/healthz（无鉴权）==
    [api-test]   PASS GET /healthz -> HTTP 200
    ...
    [api-test] DEVICE SMOKE PASS：healthz → status → config → revision 全部通过（端口 8686）

### 服务未运行时

脚本会以退出码 1 结束，并打印三种恢复方式（启动模块服务 / 手动 forward / 使用 STAGE_BIN）。
如果只想验证脚本本身，用 `MOCK=1 bash tests/api/smoke.sh`（mock 不经过 adb）。

## 注意

- `STAGE_BIN` 的默认 `LAUNCH_CMD` 是占位约定（`--bind / --config`）；
  server CLI 定稿后请以实际参数覆盖，或同步修改 tests/device/smoke.sh。
- 临时启动使用 `nohup ... &`，进程随设备重启消失，不影响模块自带的 service.sh。
- 真机验收矩阵（Android 17 / 14 / 12，KernelSU / Magisk）见计划书 13.3；
  本脚本只覆盖单机冒烟，不代替矩阵验收。
