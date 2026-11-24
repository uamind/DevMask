#!/system/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# Device Faker: https://github.com/Seyud/device_faker
#
# 卸载：先停掉 device_faker_server（含守护循环），再删运行时目录，
# 否则守护循环会在 5 秒后把已经消失的二进制重新拉起（并不断刷日志）。

RUNTIME_DIR=/data/adb/device_faker
PID_FILE=$RUNTIME_DIR/server.pid
SUPERVISOR_PID_FILE=$RUNTIME_DIR/server.supervisor.pid

pid_alive() {
    [ -n "$1" ] && kill -0 "$1" 2>/dev/null
}

read_pid() {
    [ -f "$1" ] || return 1
    pid=$(cat "$1" 2>/dev/null | tr -cd '0-9')
    [ -n "$pid" ] || return 1
    printf '%s' "$pid"
}

# 先停守护循环（它会负责收掉 server 子进程），再直接收一遍 server。
supervisor_pid=$(read_pid "$SUPERVISOR_PID_FILE")
if pid_alive "$supervisor_pid"; then
    kill "$supervisor_pid" 2>/dev/null
fi
rm -f "$SUPERVISOR_PID_FILE"

sleep 1

server_pid=$(read_pid "$PID_FILE")
if pid_alive "$server_pid"; then
    kill "$server_pid" 2>/dev/null
    sleep 1
    pid_alive "$server_pid" && kill -9 "$server_pid" 2>/dev/null
fi

rm -rf "$RUNTIME_DIR"
