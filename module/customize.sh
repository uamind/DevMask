#!/system/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# DevMask: https://github.com/uamind/DevMask

# Language detection: Chinese if the system locale starts with "zh", otherwise English.
LOCALE_VALUE=""
for _locale_prop in persist.sys.locale ro.product.locale persist.sys.language ro.product.locale.language; do
    _locale_val=$(getprop "$_locale_prop" 2>/dev/null)
    if [ -n "$_locale_val" ]; then
        LOCALE_VALUE="$_locale_val"
        break
    fi
done

case "$LOCALE_VALUE" in
    zh*) LANG_CN=true ;;
    *)   LANG_CN=false ;;
esac

tr_print() {
    if $LANG_CN; then
        ui_print "$1"
    else
        ui_print "$2"
    fi
}

check_zygisk() {
    # libzygisk.so: ZygiskNext etc.; libzygisk64.so: YukiZygisk
    if find /data/adb/modules /data/adb/modules_update /data/adb/ksu/lib \( -name "libzygisk.so" -o -name "libzygisk64.so" \) 2>/dev/null | grep -q .; then
        return 0
    fi

    if $LANG_CN; then
        abort "! 未检测到 Zygisk 实现。请安装 ZygiskNext 模块（不支持 Magisk 自带 Zygisk）"
    else
        abort "! No Zygisk implementation found. Please install ZygiskNext (Magisk built-in Zygisk is not supported)"
    fi
}

check_zygisk

CONFIG_DIR="/data/adb/devmask/config"
CONFIG_FILE="$CONFIG_DIR/config.toml"

chooseport() {
    local timeout=10
    local count=0
    while [ $count -lt $timeout ]; do
        local key_event=$(timeout 1 getevent -lc 1 2>&1 | grep VOLUME | grep " DOWN")
        if [ -n "$key_event" ]; then
            echo "$key_event" | grep VOLUMEUP > /dev/null && return 0 || return 1
        fi
        count=$((count + 1))
    done
    return 1
}

tr_print "- 安装 DevMask 模块" "- Installing DevMask module"
mkdir -p "$CONFIG_DIR"
chmod 755 "$CONFIG_DIR"

SHOULD_COPY_CONFIG=true

if [ -f "$CONFIG_FILE" ]; then
    tr_print "- 检测到已有配置文件" "- Existing config file detected"
    tr_print "- 请选择配置文件处理方式：" "- Choose how to handle config:"
    tr_print "  [音量+] 使用模块默认配置（备份原有配置）" "  [Vol+] Use default config (backup existing)"
    tr_print "  [音量-] 使用现有配置" "  [Vol-] Keep existing config"
    tr_print "- 10秒内未选择将保留现有配置" "- Existing config will be kept if no choice within 10s"

    if chooseport; then
        tr_print "- 已选择：使用模块默认配置" "- Selected: use default config"
        BACKUP_FILE="$CONFIG_DIR/$(date +%Y%m%d_%H%M%S)_config.toml.bak"
        tr_print "- 备份旧配置到: $BACKUP_FILE" "- Backup saved to: $BACKUP_FILE"
        cp -f "$CONFIG_FILE" "$BACKUP_FILE"
        chmod 644 "$CONFIG_FILE" "$BACKUP_FILE"
    else
        tr_print "- 已选择：使用现有配置" "- Selected: keep existing config"
        SHOULD_COPY_CONFIG=false
    fi
fi

if [ "$SHOULD_COPY_CONFIG" = true ]; then
    cp -f "$MODPATH/config.toml" "$CONFIG_FILE"
    chmod 644 "$CONFIG_FILE"
fi


rm -f "$MODPATH/config.toml"

# 原生 Web 服务（bin/devmask_server）的运行时目录与默认设置。
RUNTIME_DIR="/data/adb/devmask"
SERVER_CONFIG_DIR="$RUNTIME_DIR/config"
SERVER_CONFIG_FILE="$SERVER_CONFIG_DIR/server.toml"
SERVER_LOG_DIR="$RUNTIME_DIR/logs"
SERVER_BACKUP_DIR="$SERVER_CONFIG_DIR/backups"

mkdir -p "$SERVER_CONFIG_DIR" "$SERVER_LOG_DIR" "$SERVER_BACKUP_DIR"
chmod 755 "$SERVER_CONFIG_DIR" "$SERVER_LOG_DIR"
chmod 700 "$SERVER_BACKUP_DIR"

if [ -f "$SERVER_CONFIG_FILE" ]; then
    tr_print "- 保留已有服务设置: $SERVER_CONFIG_FILE" "- Keeping existing server settings: $SERVER_CONFIG_FILE"
elif [ -f "$MODPATH/server.default.toml" ]; then
    cp -f "$MODPATH/server.default.toml" "$SERVER_CONFIG_FILE"
    tr_print "- 已写入默认服务设置: $SERVER_CONFIG_FILE" "- Wrote default server settings: $SERVER_CONFIG_FILE"
fi
if [ -f "$SERVER_CONFIG_FILE" ]; then
    chmod 600 "$SERVER_CONFIG_FILE"
fi

tr_print "- 原生 Web 服务: http://127.0.0.1:8686（重启设备后由 service.sh 拉起）" "- Native web service: http://127.0.0.1:8686 (started by service.sh after reboot)"
tr_print "- 首次启动的 Token 在 $SERVER_LOG_DIR/server.log 的横幅里" "- The first-run token is printed once in $SERVER_LOG_DIR/server.log"

tr_print "- 配置文件位置: $CONFIG_FILE" "- Config file location: $CONFIG_FILE"
tr_print "- 编辑配置文件添加需要伪装的应用" "- Edit config file to add apps for spoofing"
tr_print "- 修改配置后无需重启，仅需重启对应应用" "- No reboot needed, just restart the target app"

set_perm_recursive "$MODPATH" 0 0 0755 0644
set_perm_recursive "$MODPATH/zygisk" 0 0 0755 0644
# 上面一行把 $MODPATH 下所有文件都设成 0644，会抹掉服务二进制与启动脚本的执行位，
# 这里补回来：service.sh 由 init 执行，bin/devmask_server 由 service.sh 执行。
if [ -d "$MODPATH/bin" ]; then
    set_perm_recursive "$MODPATH/bin" 0 0 0755 0755
fi
for _script in service.sh customize.sh uninstall.sh; do
    if [ -f "$MODPATH/$_script" ]; then
        set_perm "$MODPATH/$_script" 0 0 0755
    fi
done
# TODO(remove-cli): devmask_cli 已不随模块打包（WebUI 转换已纯前端实现）。
# 若确认无人再手动使用，可连同 devmask_cli/ crate 一起删除。

tr_print "- 安装完成！" "- Installation complete!"
tr_print "- 重启设备后生效" "- Reboot device to take effect"
