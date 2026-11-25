#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# 真机冒烟：adb forward tcp:8686 → curl healthz/status/config（可选取 /data/local/tmp 运行 server）。
#
# 前提：设备已连接、模块已安装、service.sh 已启动 devmask_server。
#
# 用法：
#   bash tests/device/smoke.sh
#   DEVICE=xxxx TOKEN=df_xxx bash tests/device/smoke.sh
#   STAGE_BIN=target/aarch64-linux-android/release/devmask_server bash tests/device/smoke.sh
#   STAGE_BIN=... STATIC_DIR=module/webroot bash tests/device/smoke.sh
#
# 环境变量：
#   ADB            默认 adb
#   DEVICE         adb -s 序列号（可选）
#   PORT           本机转发端口，默认 8686
#   REMOTE_PORT    设备端口，默认 = PORT
#   TOKEN          Bearer Token（服务启用鉴权时必填）
#   STAGE_BIN      可选：推送该二进制到 /data/local/tmp 并临时启动
#   STATIC_DIR     可选：随 STAGE_BIN 一起推送的 webroot 目录
#   REMOTE_DIR     默认 /data/local/tmp/devmask
#   LAUNCH_CMD     默认 "./devmask_server --bind 127.0.0.1:<PORT> --config ./config.toml"
#                  —— 必须与 server 实际 CLI 一致；不一致时显式覆盖本变量。
#   STATIC_CHECK   默认 1：额外检查静态首页 200
#   KEEP_FORWARD   默认 0：结束时移除 adb forward
#   STOP_STAGED    默认 1：结束时停止 STAGE_BIN 启动的临时服务

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TESTS_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ADB="${ADB:-adb}"
DEVICE="${DEVICE:-}"
PORT="${PORT:-8686}"
REMOTE_PORT="${REMOTE_PORT:-$PORT}"
BASE="http://127.0.0.1:${PORT}/api/v1"
TOKEN="${TOKEN:-}"
STAGE_BIN="${STAGE_BIN:-}"
STATIC_DIR="${STATIC_DIR:-}"
REMOTE_DIR="${REMOTE_DIR:-/data/local/tmp/devmask}"
LAUNCH_CMD="${LAUNCH_CMD:-}"
STATIC_CHECK="${STATIC_CHECK:-1}"
KEEP_FORWARD="${KEEP_FORWARD:-0}"
STOP_STAGED="${STOP_STAGED:-1}"

# shellcheck source=../api/common.sh
source "$TESTS_ROOT/api/common.sh"

FORWARD_ADDED=0
STAGED=0

adb_cmd() {
  if [ -n "$DEVICE" ]; then
    "$ADB" -s "$DEVICE" "$@"
  else
    "$ADB" "$@"
  fi
}

device_shell() {
  adb_cmd shell "$@"
}

df_test_cleanup() {
  set +e
  if [ "$STAGED" = "1" ] && [ "$STOP_STAGED" = "1" ]; then
    df_log "停止设备端临时服务"
    device_shell "sh $REMOTE_DIR/stop.sh" >/dev/null 2>&1
  fi
  if [ "$FORWARD_ADDED" = "1" ] && [ "$KEEP_FORWARD" != "1" ]; then
    df_log "移除 adb forward tcp:$PORT"
    adb_cmd forward --remove "tcp:$PORT" >/dev/null 2>&1
  fi
  set -e
}

df_init
df_require_cmd "$ADB"
if [ "$STATIC_CHECK" = "1" ]; then
  df_require_cmd curl
fi

# ---------------------------------------------------------------- 设备与转发

STATE=$(adb_cmd get-state 2>/dev/null || true)
if [ "$STATE" != "device" ]; then
  df_fail "adb get-state = [$STATE]，未发现可用设备（检查 USB 调试 / DEVICE 序列号）"
fi

MODEL=$(device_shell getprop ro.product.model 2>/dev/null | tr -d '\r' || true)
ANDROID=$(device_shell getprop ro.build.version.release 2>/dev/null | tr -d '\r' || true)
SU=$(device_shell "command -v su >/dev/null 2>&1 && echo yes || echo no" 2>/dev/null | tr -d '\r' || true)
df_log "设备：${MODEL:-unknown} / Android ${ANDROID:-unknown} / su=${SU:-unknown}"

df_log "adb forward tcp:$PORT -> tcp:$REMOTE_PORT"
adb_cmd forward "tcp:$PORT" "tcp:$REMOTE_PORT" >/dev/null || df_fail "adb forward 失败"
FORWARD_ADDED=1

# ---------------------------------------------------------------- 可选：设备端临时启动

if df_probe_health; then
  df_log "端口 $PORT 已有服务，复用现有进程"
elif [ -n "$STAGE_BIN" ]; then
  [ -f "$STAGE_BIN" ] || df_fail "STAGE_BIN 不存在：$STAGE_BIN"
  df_log "推送 server 到 $REMOTE_DIR 并临时启动"
  device_shell "mkdir -p $REMOTE_DIR" || df_fail "无法创建 $REMOTE_DIR"
  adb_cmd push "$STAGE_BIN" "$REMOTE_DIR/devmask_server" >/dev/null || df_fail "push server 失败"
  device_shell "chmod 755 $REMOTE_DIR/devmask_server"
  adb_cmd push "$TESTS_ROOT/fixtures/config/minimal.toml" "$REMOTE_DIR/config.toml" >/dev/null || df_fail "push config 失败"
  if [ -n "$STATIC_DIR" ]; then
    adb_cmd push "$STATIC_DIR" "$REMOTE_DIR/webroot" >/dev/null || df_fail "push STATIC_DIR 失败"
  fi
  if [ -z "$LAUNCH_CMD" ]; then
    LAUNCH_CMD="./devmask_server --bind 127.0.0.1:$PORT --config ./config.toml"
  fi
  {
    printf '#!/system/bin/sh\n'
    printf 'cd %s || exit 1\n' "$REMOTE_DIR"
    printf 'nohup %s > server.log 2>&1 &\n' "$LAUNCH_CMD"
    printf 'echo $! > server.pid\n'
  } > "$DF_TMP/start.sh"
  {
    printf '#!/system/bin/sh\n'
    printf 'cd %s || exit 1\n' "$REMOTE_DIR"
    printf 'if [ -f server.pid ]; then kill "$(cat server.pid)" 2>/dev/null; fi\n'
  } > "$DF_TMP/stop.sh"
  adb_cmd push "$DF_TMP/start.sh" "$REMOTE_DIR/start.sh" >/dev/null || df_fail "push start.sh 失败"
  adb_cmd push "$DF_TMP/stop.sh" "$REMOTE_DIR/stop.sh" >/dev/null || df_fail "push stop.sh 失败"
  device_shell "chmod 755 $REMOTE_DIR/start.sh $REMOTE_DIR/stop.sh"
  STAGED=1
  device_shell "sh $REMOTE_DIR/start.sh" || df_fail "设备端启动失败（查看 $REMOTE_DIR/server.log）"
  df_wait_health "等待设备端服务启动" 80
else
  df_fail "http://127.0.0.1:${PORT}/api/v1/healthz 不可达，且未提供 STAGE_BIN。
  请确认：
    1) 模块已安装且 service.sh 已启动 devmask_server；
    2) adb forward tcp:$PORT tcp:$REMOTE_PORT 已建立；
    3) 或设置 STAGE_BIN=<aarch64 devmask_server> 让本脚本临时启动。"
fi

# ---------------------------------------------------------------- API 检查

df_log "== GET /api/v1/healthz（无鉴权）=="
df_api_anon GET "/healthz"
df_assert_status "200" "GET /healthz"
df_assert_json "status" "ok" "healthz.status"

df_log "== GET /api/v1/status =="
df_api GET "/status" "${DF_AUTH[@]}"
df_assert_status "200" "GET /status"
df_assert_json "module.id" "devmask" "status.module.id"
STATUS_REV=$(df_json_get "$DF_BODY" "config.revision" "" 2>/dev/null || true)
if [ -z "$STATUS_REV" ] || [ "$STATUS_REV" = "null" ]; then
  df_fail "status.config.revision 缺失"
fi
STATUS_MODEL=$(df_json_get "$DF_BODY" "device.model" "" 2>/dev/null || true)
STATUS_BIND=$(df_json_get "$DF_BODY" "server.bind" "" 2>/dev/null || true)
df_note "设备模型=$STATUS_MODEL bind=$STATUS_BIND revision=$STATUS_REV"

df_log "== GET /api/v1/config =="
df_api GET "/config" "${DF_AUTH[@]}"
df_assert_status "200" "GET /config"
RV=$(df_capture_revision "GET /config")
if [ "$RV" != "$STATUS_REV" ]; then
  df_fail "status.config.revision ($STATUS_REV) 与 GET /config revision ($RV) 不一致"
fi
df_note "PASS status 与 config revision 一致"

df_log "== GET /api/v1/config/revision =="
df_api GET "/config/revision" "${DF_AUTH[@]}"
df_assert_status "200" "GET /config/revision"
df_assert_revision_matches "$DF_BODY" "revision" "$RV" "config/revision 与 config 一致"

if [ "$STATIC_CHECK" = "1" ]; then
  df_log "== GET /（静态首页）=="
  STATIC_URL="http://127.0.0.1:${PORT}/"
  STATIC_STATUS=$(curl -sS --max-time "$API_TIMEOUT" -o "$DF_TMP/index.html" -w '%{http_code}' "$STATIC_URL" || true)
  if [ "$STATIC_STATUS" != "200" ]; then
    df_fail "GET $STATIC_URL 期望 200，实际 $STATIC_STATUS"
  fi
  if ! grep -q "DevMask" "$DF_TMP/index.html"; then
    df_fail "静态首页未包含 'DevMask'，webroot 可能未部署"
  fi
  df_note "PASS 静态首页 200 且包含 DevMask"
fi

df_log "DEVICE SMOKE PASS：healthz → status → config → revision 全部通过（端口 ${PORT}）"
