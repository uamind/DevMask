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

log() {
    printf '%s [uninstall.sh] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

# relay 的三条 iptables 链（与 server/src/relay/firewall.rs 的常量同名，改名要两边一起改）。
RELAY_CHAIN=DF_EGRESS_RLY
RELAY_ALLOW_CHAIN=DF_EGRESS_RLY_ALLOW
RELAY_DENY_CHAIN=DF_EGRESS_RLY_DENY

# 为什么要清 relay 防火墙链：
# 中继只放行 127.0.0.1:17800-17899 上的授权 uid，其余流量落到 DF_EGRESS_RLY_DENY 的
# REJECT。卸载/停服后若把规则留在设备上，这段端口会**永久不可用**（下一次装回来的中继
# 自己也会被自己的旧规则挡住），而且 OUTPUT 仍有 -j DF_EGRESS_RLY 跳转时，任何连这些
# 端口的新连接都会被内核直接拒绝——看不到监听进程，现象是「端口莫名 rejection」。
#
# 顺序固定：先摘 OUTPUT 跳转（此后新连接不进链）→ 再 -F 叶子链/入口链 → 最后 -X 删链。
# 反序会因「链仍被引用」失败。链/跳转本来不存在（退出码 1）是正常情况，只记日志，
# 绝不阻断卸载；其它非零退出码才记为失败。
run_iptables() {
    iptables -w 2 "$@" 2>/dev/null
    rc=$?
    if [ "$rc" -eq 2 ]; then
        # 老 busybox/iptables 不认 -w 2：去掉重试一次。
        iptables "$@" 2>/dev/null
        rc=$?
    fi
    return "$rc"
}

clear_relay_firewall() {
    if ! command -v iptables >/dev/null 2>&1; then
        log "iptables 不存在，跳过 relay 链清理"
        return 0
    fi
    run_iptables -D OUTPUT -j "$RELAY_CHAIN" || true
    failed=0
    for chain in "$RELAY_ALLOW_CHAIN" "$RELAY_DENY_CHAIN" "$RELAY_CHAIN"; do
        run_iptables -F "$chain"
        rc=$?
        if [ "$rc" -ne 0 ] && [ "$rc" -ne 1 ]; then
            failed=1
        fi
        run_iptables -X "$chain"
        rc=$?
        if [ "$rc" -ne 0 ] && [ "$rc" -ne 1 ]; then
            failed=1
        fi
    done
    if [ "$failed" -ne 0 ]; then
        log "relay 链清理有失败项（$RELAY_CHAIN / $RELAY_ALLOW_CHAIN / $RELAY_DENY_CHAIN 可能残留）"
    else
        log "已清理 relay 防火墙三条链（$RELAY_CHAIN 系列）"
    fi
    return 0
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

# relay：服务已停（上面 kill 过），现在清防火墙链，再删运行时目录。
# 顺序必须是「停服 -> 清链 -> 删目录」：先删目录会让日志无处可写，而清链要在
# 监听器已经关闭之后做（否则清完还有新连接被规则挡住/漏放）。
clear_relay_firewall

# 出口代理：卸载前必须把系统代理清成直连，否则设备会继续走一个已经不存在的代理。
# 清除只能写 :0（settings delete 无效），与 server/src/egress/system.rs 的 clear() 同一语义；
# 同时把排除列表写空串，避免下次安装继承上一轮的网段白名单。
settings put global http_proxy :0 >/dev/null 2>&1
settings put global global_http_proxy_exclusion_list "" >/dev/null 2>&1

rm -rf "$RUNTIME_DIR"
