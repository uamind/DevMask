#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# Device Faker: https://github.com/Seyud/device_faker
#
# t9 验收用：带认证上游 fixture 的启动/停止/取证包装（默认值与 t9-acceptance.md §2 一致）。
#   t9-fixture.sh start [evidence-dir]
#   t9-fixture.sh stop | status | log [n]
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
FIXTURE="$ROOT/docs/design/egress/fixtures/local_proxy.py"
RUN_DIR="${T9_RUN_DIR:-/tmp/t9-run}"
LOG_PATH="${T9_FIXTURE_LOG:-$RUN_DIR/fixture.log}"
PID_FILE="$RUN_DIR/fixture.pid"

HTTP_USER="${T9_HTTP_USER:-t9user}"
HTTP_PASS="${T9_HTTP_PASS:-t9pass-ro3X}"
SOCKS_USER="${T9_SOCKS_USER:-t9suser}"
SOCKS_PASS="${T9_SOCKS_PASS:-t9spass-ro3X}"

start() {
  local evidence="${1:-$RUN_DIR}"
  mkdir -p "$RUN_DIR" "$evidence"
  export FIXTURE_HTTP_PORTS="${T9_HTTP_PORTS:-18080,18082}"
  export FIXTURE_SOCKS_PORT="${T9_SOCKS_PORT:-18081}"
  export FIXTURE_LOG="$LOG_PATH"
  export FIXTURE_HTTP_AUTH=require FIXTURE_HTTP_USER="$HTTP_USER" FIXTURE_HTTP_PASS="$HTTP_PASS"
  export FIXTURE_SOCKS_MODE=require-userpass FIXTURE_SOCKS_USER="$SOCKS_USER" FIXTURE_SOCKS_PASS="$SOCKS_PASS"
  [ -f "$PID_FILE" ] && stop
  python3 "$FIXTURE" >"$RUN_DIR/fixture.out" 2>&1 &
  echo $! >"$PID_FILE"
  sleep 1
  if kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    {
      echo "# fixture 启动 $(date -u +%Y-%m-%dT%H:%M:%SZ)"
      echo "ports_http=$FIXTURE_HTTP_PORTS socks=$FIXTURE_SOCKS_PORT log=$LOG_PATH"
      echo "凭证摘要 http=$(printf '%s:%s' "$HTTP_USER" "$HTTP_PASS" | shasum -a 256 | cut -c1-12) socks=$(printf '%s:%s' "$SOCKS_USER" "$SOCKS_PASS" | shasum -a 256 | cut -c1-12)"
      echo "# 明文凭据只存在于调用方环境，不进任何 fixture 输出"
    } | tee "$evidence/fixture-start.txt"
  else
    echo "fixture 启动失败，见 $RUN_DIR/fixture.out" >&2
    tail -5 "$RUN_DIR/fixture.out" >&2
    return 1
  fi
}

stop() {
  if [ -f "$PID_FILE" ]; then
    kill "$(cat "$PID_FILE")" 2>/dev/null || true
    sleep 0.3
    kill -9 "$(cat "$PID_FILE")" 2>/dev/null || true
    rm -f "$PID_FILE"
  fi
  pkill -f 'local_proxy.py' 2>/dev/null || true
}

status() {
  if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    echo "running pid=$(cat "$PID_FILE") log=$LOG_PATH"
  else
    echo "not running"
    return 1
  fi
}

case "${1:-status}" in
  start) shift; start "${1:-$RUN_DIR}" ;;
  stop) stop ;;
  status) status ;;
  log) shift; tail -"${1:-40}" "$LOG_PATH" 2>/dev/null || echo "no log at $LOG_PATH" ;;
  *) echo "用法: $0 {start [evidence-dir]|stop|status|log [n]}" >&2; exit 2 ;;
esac
