#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# Device Faker: https://github.com/Seyud/device_faker
#
# t9 凭据零泄漏扫描：设备日志 / 审计 / API 响应 / fixture 日志。只读，不改变任何状态。
# 用法：t9-scan.sh [evidence-dir]；命中数 != 0 时退出码 1。
set -uo pipefail

TOKEN="${T9_TOKEN:-admin123}"
API="${T9_API:-http://127.0.0.1:18686/api/v1}"
MARK="t9pass-ro3X|t9spass-ro3X"
OUT="${1:-/tmp/t9-run}"
mkdir -p "$OUT"

fail=0
hit() { # name command...
  local name="$1"; shift
  local out count
  out="$("$@" 2>&1)"
  count="$(printf '%s\n' "$out" | awk -F: '{s+=$NF} END {print s+0}')"
  if [ "$count" -eq 0 ]; then
    echo "PASS $name count=0"
  else
    echo "FAIL $name count=$count :: $out"
    fail=1
  fi
}

hit device-server.log bash -c "adb shell su -c 'grep -Eci \"$MARK\" /data/adb/device_faker/logs/server.log 2>/dev/null' || true"
hit device-audit.jsonl bash -c "adb shell su -c 'grep -Eci \"$MARK\" /data/adb/device_faker/logs/api-audit.jsonl 2>/dev/null' || true"
hit api-egress bash -c "curl -s -H 'Authorization: Bearer $TOKEN' '$API/egress' | grep -Eci \"$MARK|password\" || true"
hit api-proxies bash -c "curl -s -H 'Authorization: Bearer $TOKEN' '$API/egress/proxies' | grep -Eci \"$MARK|password\" || true"
hit fixture-log bash -c "grep -Eci \"$MARK\" '${T9_FIXTURE_LOG:-/tmp/t9-run/fixture.log}' || true"

echo "----"
if [ "$fail" -eq 0 ]; then echo "RESULT: 凭据零泄漏 PASS（5/5 命中 0）"; else echo "RESULT: 凭据命中，FAIL"; fi
exit "$fail"
