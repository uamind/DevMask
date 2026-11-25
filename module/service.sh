#!/system/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# DevMask: https://github.com/uamind/DevMask
#
# devmask_server 开机启动脚本。
#
# Magisk / KernelSU 在 late_start service 阶段以 root 执行本脚本（每次开机一次）：
#   1. 等 sys.boot_completed（最多 10 分钟；超时也照常启动，只记一行日志）；
#   2. 单实例：守护循环已经在跑就直接退出，不重复拉起；
#   3. 拉起 bin/devmask_server，进程退出后 5 秒重新拉起；
#   4. 日志写 /data/adb/devmask/logs/server.log（0600，超过 1 MiB 轮转成 .1）；
#   5. pid 落 /data/adb/devmask/server.pid（server 进程）与
#      server.supervisor.pid（守护循环）。
#
# 手动用法（需要 root）：
#   sh /data/adb/modules/devmask/service.sh            # 等价 start
#   sh /data/adb/modules/devmask/service.sh start
#   sh /data/adb/modules/devmask/service.sh stop
#   sh /data/adb/modules/devmask/service.sh restart
#   sh /data/adb/modules/devmask/service.sh status

MODDIR=${0%/*}
[ -d "$MODDIR" ] || MODDIR=$(pwd)

RUNTIME_DIR=/data/adb/devmask
CONFIG_DIR=$RUNTIME_DIR/config
LOG_DIR=$RUNTIME_DIR/logs
BACKUP_DIR=$CONFIG_DIR/backups
LOG_FILE=$LOG_DIR/server.log
LOG_OLD_FILE=$LOG_DIR/server.log.1
PID_FILE=$RUNTIME_DIR/server.pid
SUPERVISOR_PID_FILE=$RUNTIME_DIR/server.supervisor.pid
CONFIG_FILE=$CONFIG_DIR/config.toml
SERVER_CONFIG=$CONFIG_DIR/server.toml
DEFAULT_SERVER_CONFIG=$MODDIR/server.default.toml
SERVER_BIN=$MODDIR/bin/devmask_server
STATIC_DIR=$MODDIR/webroot

# 崩溃重启间隔（秒）与单个日志文件上限（字节）。
RESTART_DELAY=5
LOG_MAX_BYTES=1048576
# 等待 boot_completed 的最多轮数（每轮 2 秒）。
BOOT_WAIT_TRIES=300

ACTION=${1:-start}

log() {
    printf '%s [service.sh] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" >> "$LOG_FILE"
}

# 运行时目录 + 日志文件权限。server.log 里可能出现首次启动的 Token 明文，
# 因此必须 0600（安装器 umask 通常是 022，不能依赖默认值）。
prepare_dirs() {
    mkdir -p "$CONFIG_DIR" "$LOG_DIR" "$BACKUP_DIR" 2>/dev/null
    chmod 755 "$CONFIG_DIR" "$LOG_DIR" 2>/dev/null
    chmod 700 "$BACKUP_DIR" 2>/dev/null
    [ -f "$LOG_FILE" ] || : > "$LOG_FILE" 2>/dev/null
    chmod 600 "$LOG_FILE" 2>/dev/null
}

# 日志轮转：只在两次启动之间做，避免把正在写的文件移走。
rotate_log() {
    [ -f "$LOG_FILE" ] || return 0
    size=$(wc -c < "$LOG_FILE" 2>/dev/null | tr -cd '0-9')
    [ -n "$size" ] || return 0
    if [ "$size" -ge "$LOG_MAX_BYTES" ]; then
        mv -f "$LOG_FILE" "$LOG_OLD_FILE" 2>/dev/null
        : > "$LOG_FILE" 2>/dev/null
        chmod 600 "$LOG_FILE" 2>/dev/null
        log "服务日志超过 ${LOG_MAX_BYTES} 字节（实际 ${size}），已轮转为 $(basename "$LOG_OLD_FILE")"
    fi
}

wait_for_boot() {
    tries=0
    while [ "$tries" -lt "$BOOT_WAIT_TRIES" ]; do
        if [ "$(getprop sys.boot_completed 2>/dev/null)" = "1" ]; then
            return 0
        fi
        sleep 2
        tries=$((tries + 1))
    done
    log "等待 sys.boot_completed 超时（$((BOOT_WAIT_TRIES * 2))s），继续启动服务"
    return 0
}

pid_alive() {
    [ -n "$1" ] && kill -0 "$1" 2>/dev/null
}

# 打印 pid 文件里的数字；文件缺失或内容非法时返回非 0。
read_pid() {
    [ -f "$1" ] || return 1
    pid=$(cat "$1" 2>/dev/null | tr -cd '0-9')
    [ -n "$pid" ] || return 1
    printf '%s' "$pid"
}

# /proc 根目录；用环境变量覆盖是为了能在主机上做冒烟（Android 上保持 /proc）。
PROC_ROOT=${PROC_ROOT:-/proc}

# 读进程命令行（NUL 分隔 → 去掉 NUL）；进程不存在或读不到时返回非 0。
proc_cmdline() {
    [ -n "$1" ] || return 1
    [ -r "$PROC_ROOT/$1/cmdline" ] || return 1
    tr -d '\000' < "$PROC_ROOT/$1/cmdline" 2>/dev/null
}

# pid 是否确实属于本模块。
#
# Android 重启后 pid 复用非常常见：pid 文件里的数字可能刚好被别的进程占用，
# 只看 kill -0 会把「陌生进程」当成「守护还在跑」，于是跳过启动（QA 在 t20 实测
# 必须手工 stop && start 才能拉起）。因此必须核对 /proc/<pid>/cmdline（或 comm）
# 里是否出现本模块的特征串。
pid_is_ours() {
    pid=$1
    pattern=$2
    pid_alive "$pid" || return 1
    cmdline=$(proc_cmdline "$pid")
    if [ -n "$cmdline" ]; then
        case "$cmdline" in
            *"$pattern"*) return 0 ;;
            *) return 1 ;;
        esac
    fi
    comm_file=$PROC_ROOT/$pid/comm
    if [ -r "$comm_file" ]; then
        comm=$(tr -d '\000' < "$comm_file" 2>/dev/null)
        case "$comm" in
            *"$pattern"*) return 0 ;;
            *) return 1 ;;
        esac
    fi
    # 连 /proc 都读不到（非 Android 环境）：按「是本进程」处理，避免误清理。
    [ -d "$PROC_ROOT" ] || return 0
    return 1
}

# 清理陈旧 pid 文件：进程已不存在，或该 pid 已被别的进程占用。
clean_stale_pid() {
    pid_file=$1
    pattern=$2
    label=$3
    stale_pid=$(read_pid "$pid_file") || { rm -f "$pid_file"; return 0; }
    if pid_is_ours "$stale_pid" "$pattern"; then
        return 0
    fi
    rm -f "$pid_file"
    log "stale $label pid=$stale_pid cleaned（重启后 pid 复用或进程已不存在）"
    return 0
}
# relay 的三条 iptables 链（与 server/src/relay/firewall.rs 的常量同名，改名要两边一起改）。
RELAY_CHAIN=DM_EGRESS_RLY
RELAY_ALLOW_CHAIN=DM_EGRESS_RLY_ALLOW
RELAY_DENY_CHAIN=DM_EGRESS_RLY_DENY

# 为什么要清 relay 防火墙链：
# 中继只放行 127.0.0.1:17800-17899 上的授权 uid，其余流量落到 DM_EGRESS_RLY_DENY 的
# REJECT。卸载/停服后若把规则留在设备上，这段端口会**永久不可用**（下一次装回来的中继
# 自己也会被自己的旧规则挡住），而且 OUTPUT 仍有 -j DM_EGRESS_RLY 跳转时，任何连这些
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

status_server() {
    supervisor_pid=$(read_pid "$SUPERVISOR_PID_FILE")
    if pid_alive "$supervisor_pid" && pid_is_ours "$supervisor_pid" "service.sh"; then
        echo "supervisor: running (pid=$supervisor_pid)"
    elif pid_alive "$supervisor_pid"; then
        echo "supervisor: stale pid $supervisor_pid（重启前的旧值，start 时会清理）"
    else
        echo "supervisor: stopped"
    fi

    server_pid=$(read_pid "$PID_FILE")
    if pid_alive "$server_pid" && pid_is_ours "$server_pid" "devmask_server"; then
        echo "server: running (pid=$server_pid)"
    elif pid_alive "$server_pid"; then
        echo "server: stale pid $server_pid（重启前的旧值，start 时会清理）"
    else
        echo "server: stopped"
    fi
}

# 先停守护循环（否则它会在 RESTART_DELAY 秒后把 server 重新拉起来），再停 server。
stop_server() {
    supervisor_pid=$(read_pid "$SUPERVISOR_PID_FILE")
    if pid_alive "$supervisor_pid" && pid_is_ours "$supervisor_pid" "service.sh"; then
        kill "$supervisor_pid" 2>/dev/null
        log "已向守护循环发送 TERM（pid=$supervisor_pid）"
    elif pid_alive "$supervisor_pid"; then
        # pid 被别的进程复用了：绝不能 kill，只清理文件。
        log "stale supervisor pid=$supervisor_pid cleaned（不属于本模块，不发送信号）"
    fi
    rm -f "$SUPERVISOR_PID_FILE"

    # 给守护循环里 trap 的时间窗口，先把子进程收掉。
    sleep 1

    server_pid=$(read_pid "$PID_FILE")
    if pid_alive "$server_pid" && pid_is_ours "$server_pid" "devmask_server"; then
        kill "$server_pid" 2>/dev/null
        sleep 1
        if pid_alive "$server_pid"; then
            kill -9 "$server_pid" 2>/dev/null
        fi
        log "已停止 devmask_server（pid=$server_pid）"
    elif pid_alive "$server_pid"; then
        log "stale server pid=$server_pid cleaned（不属于本模块，不发送信号）"
    fi
    rm -f "$PID_FILE"
}

start_server() {
    # 关闭继承的 stdout/stderr：adb shell / 管理器会在命令返回后等管道 EOF，
    # 后台守护进程若仍持有管道，调用方会挂到超时（实测 adb restart exit 124）。
    exec </dev/null >> "$LOG_FILE" 2>&1

    # 重启（reboot 或模块重新启用）后 pid 文件往往还是上一轮的值，pid 复用会让
    # kill -0 假阳性 → 必须先校验归属，陈旧就清理并照常启动（QA t20 实测的 bug）。
    clean_stale_pid "$SUPERVISOR_PID_FILE" "service.sh" "supervisor"
    clean_stale_pid "$PID_FILE" "devmask_server" "server"

    supervisor_pid=$(read_pid "$SUPERVISOR_PID_FILE")
    if pid_alive "$supervisor_pid"; then
        log "守护循环已在运行（pid=$supervisor_pid），跳过启动"
        return 0
    fi
    rm -f "$SUPERVISOR_PID_FILE" "$PID_FILE"

    (
        server_pid=0
        # 忽略 HUP：service.sh 退出时不能把常驻进程带走。
        trap '' HUP
        trap 'if [ "$server_pid" -gt 0 ]; then kill "$server_pid" 2>/dev/null; fi; log "守护循环收到停止信号，退出"; exit 0' TERM INT

        while true; do
            rotate_log
            "$SERVER_BIN" \
                --config-path "$CONFIG_FILE" \
                --static-dir "$STATIC_DIR" \
                --server-config "$SERVER_CONFIG" \
                >> "$LOG_FILE" 2>&1 &
            server_pid=$!
            printf '%s\n' "$server_pid" > "$PID_FILE"
            log "devmask_server 已启动（pid=$server_pid，bind 见 $SERVER_CONFIG）"

            wait "$server_pid"
            code=$?
            server_pid=0
            rm -f "$PID_FILE"
            log "devmask_server 退出（status=$code），${RESTART_DELAY} 秒后重启"
            sleep "$RESTART_DELAY"
        done
    ) >> "$LOG_FILE" 2>&1 &

    supervisor_pid=$!
    printf '%s\n' "$supervisor_pid" > "$SUPERVISOR_PID_FILE"
    log "守护循环已启动（pid=$supervisor_pid，bin=$SERVER_BIN）"
}

case "$ACTION" in
    stop)
        prepare_dirs
        stop_server
        # 服务已停（监听器随之关闭），再清 relay 防火墙链：
        # 禁用模块不会跑 uninstall.sh，但会跑本脚本的 stop，所以这里必须一起清，
        # 否则 17800-17899 会被残留的 REJECT 规则永久挡住。
        clear_relay_firewall
        log "stop: devmask_server 已停止，relay 防火墙链已清理"
        exit 0
        ;;
    status)
        status_server
        exit 0
        ;;
    start|restart)
        ;;
    *)
        echo "用法: sh service.sh [start|stop|restart|status]" >&2
        exit 1
        ;;
esac

prepare_dirs

if [ "$ACTION" = "restart" ]; then
    stop_server
    sleep 1
fi

# 模块被禁用（Magisk 在模块目录放 disable 文件）时不要拉起服务。
if [ -f "$MODDIR/disable" ]; then
    # 模块被禁用时不启动服务；顺手清一遍 relay 链（幂等）：正常开机 iptables 是空的，
    # 但如果有人在运行期放了 disable 文件再重启守护，这里能保证不留残余 REJECT。
    clear_relay_firewall
    log "模块已禁用（$MODDIR/disable），不启动服务"
    exit 0
fi

if [ ! -x "$SERVER_BIN" ]; then
    log "找不到可执行的 $SERVER_BIN，跳过启动（重新安装模块，或检查 bin/ 权限是否为 0755）"
    exit 0
fi

if [ ! -f "$SERVER_CONFIG" ] && [ -f "$DEFAULT_SERVER_CONFIG" ]; then
    cp -f "$DEFAULT_SERVER_CONFIG" "$SERVER_CONFIG"
    chmod 600 "$SERVER_CONFIG" 2>/dev/null
    log "已从 $DEFAULT_SERVER_CONFIG 复制默认服务设置到 $SERVER_CONFIG"
fi

[ -f "$CONFIG_FILE" ] || log "提示：$CONFIG_FILE 不存在，服务会按空配置运行（WebUI 可新建）"

wait_for_boot
start_server
