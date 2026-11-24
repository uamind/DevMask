#!/system/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# Device Faker: https://github.com/Seyud/device_faker
#
# device_faker_server 开机启动脚本。
#
# Magisk / KernelSU 在 late_start service 阶段以 root 执行本脚本（每次开机一次）：
#   1. 等 sys.boot_completed（最多 10 分钟；超时也照常启动，只记一行日志）；
#   2. 单实例：守护循环已经在跑就直接退出，不重复拉起；
#   3. 拉起 bin/device_faker_server，进程退出后 5 秒重新拉起；
#   4. 日志写 /data/adb/device_faker/logs/server.log（0600，超过 1 MiB 轮转成 .1）；
#   5. pid 落 /data/adb/device_faker/server.pid（server 进程）与
#      server.supervisor.pid（守护循环）。
#
# 手动用法（需要 root）：
#   sh /data/adb/modules/device_faker/service.sh            # 等价 start
#   sh /data/adb/modules/device_faker/service.sh start
#   sh /data/adb/modules/device_faker/service.sh stop
#   sh /data/adb/modules/device_faker/service.sh restart
#   sh /data/adb/modules/device_faker/service.sh status

MODDIR=${0%/*}
[ -d "$MODDIR" ] || MODDIR=$(pwd)

RUNTIME_DIR=/data/adb/device_faker
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
SERVER_BIN=$MODDIR/bin/device_faker_server
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

status_server() {
    supervisor_pid=$(read_pid "$SUPERVISOR_PID_FILE")
    if pid_alive "$supervisor_pid"; then
        echo "supervisor: running (pid=$supervisor_pid)"
    else
        echo "supervisor: stopped"
    fi

    server_pid=$(read_pid "$PID_FILE")
    if pid_alive "$server_pid"; then
        echo "server: running (pid=$server_pid)"
    else
        echo "server: stopped"
    fi
}

# 先停守护循环（否则它会在 RESTART_DELAY 秒后把 server 重新拉起来），再停 server。
stop_server() {
    supervisor_pid=$(read_pid "$SUPERVISOR_PID_FILE")
    if pid_alive "$supervisor_pid"; then
        kill "$supervisor_pid" 2>/dev/null
        log "已向守护循环发送 TERM（pid=$supervisor_pid）"
    fi
    rm -f "$SUPERVISOR_PID_FILE"

    # 给守护循环里 trap 的时间窗口，先把子进程收掉。
    sleep 1

    server_pid=$(read_pid "$PID_FILE")
    if pid_alive "$server_pid"; then
        kill "$server_pid" 2>/dev/null
        sleep 1
        if pid_alive "$server_pid"; then
            kill -9 "$server_pid" 2>/dev/null
        fi
        log "已停止 device_faker_server（pid=$server_pid）"
    fi
    rm -f "$PID_FILE"
}

start_server() {
    # 关闭继承的 stdout/stderr：adb shell / 管理器会在命令返回后等管道 EOF，
    # 后台守护进程若仍持有管道，调用方会挂到超时（实测 adb restart exit 124）。
    exec </dev/null >> "$LOG_FILE" 2>&1

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
            log "device_faker_server 已启动（pid=$server_pid，bind 见 $SERVER_CONFIG）"

            wait "$server_pid"
            code=$?
            server_pid=0
            rm -f "$PID_FILE"
            log "device_faker_server 退出（status=$code），${RESTART_DELAY} 秒后重启"
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
        log "stop: device_faker_server 已停止"
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
