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

# 出口代理：卸载前必须把系统代理清成直连，否则设备会继续走一个已经不存在的代理。
# 清除只能写 :0（settings delete 无效），与 server/src/egress/system.rs 的 clear() 同一语义；
# 同时把排除列表写空串，避免下次安装继承上一轮的网段白名单。
settings put global http_proxy :0 >/dev/null 2>&1
settings put global global_http_proxy_exclusion_list "" >/dev/null 2>&1

rm -rf "$RUNTIME_DIR"
