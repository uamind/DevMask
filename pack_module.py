#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# Device Faker: https://github.com/Seyud/device_faker

"""
打包 Magisk 模块脚本
将 module/ 目录打包为可安装的 ZIP 文件

与 `cargo xtask build` 的区别：xtask 会先编译 zygisk + server 再整体打包，
本脚本只做「把现有 module/ 装进 zip」，方便只改了 WebUI 或脚本时快速出包。

zip 内容约定（安装器按 zip 里的 unix mode 落盘）：
- module/ 下所有文件（.gitignore 之类的隐藏文件跳过）；
- bin/device_faker_server：从 target/aarch64-linux-android/{release,debug}/ 找，
  找到就以 0755 写进 zip，找不到只警告（配合 --no-server 可静默跳过）；
- *.sh 与 bin/ 下的文件一律 0755，其余 0644。
"""

import argparse
import datetime
import os
import stat
import sys
import zipfile

SERVER_BIN_NAME = "device_faker_server"
SERVER_TARGET_TRIPLE = "aarch64-linux-android"


def needs_lf_conversion(file_path):
    """检查文件是否需要转换为 LF 换行符"""
    with open(file_path, "rb") as f:
        content = f.read()
    return b"\r\n" in content


def convert_to_lf(file_path, rel_path):
    """将文件转换为 LF 换行符（仅当需要时）"""
    if not needs_lf_conversion(file_path):
        print(f"  跳过 (已是 LF): {rel_path}")
        return

    with open(file_path, "rb") as f:
        content = f.read()

    content = content.replace(b"\r\n", b"\n")

    with open(file_path, "wb") as f:
        f.write(content)

    print(f"  转换: {rel_path}")


def relative_arcname(module_dir, file_path):
    """zip 内的相对路径，统一用 / 分隔（Windows 的 \\ 会被装到错误目录）。"""
    return os.path.relpath(file_path, module_dir).replace(os.sep, "/")


def file_mode(arcname):
    """zip 条目的 unix 权限：脚本与 bin/ 下的二进制必须可执行。"""
    parts = arcname.split("/")
    if len(parts) > 1 and parts[-2] == "bin":
        return 0o755
    if parts[-1].endswith(".sh"):
        return 0o755
    return 0o644


def add_file(zipf, file_path, arcname, mode):
    """按指定 unix 权限写一个文件条目（ZipInfo.external_attr 的高 16 位是 st_mode）。"""
    modified = datetime.datetime.fromtimestamp(os.path.getmtime(file_path))
    info = zipfile.ZipInfo(arcname, date_time=modified.timetuple()[:6])
    info.compress_type = zipfile.ZIP_DEFLATED
    # create_system = 3(Unix)：只有标成 Unix 的条目，解压端才会按 external_attr 落权限
    info.create_system = 3
    info.external_attr = (stat.S_IFREG | mode) << 16
    with open(file_path, "rb") as f:
        zipf.writestr(info, f.read())


def find_server_binary(project_root):
    """找 `cargo xtask build` / `cargo ndk ... -p device_faker_server` 的产物，release 优先。"""
    for profile in ("release", "debug"):
        candidate = os.path.join(
            project_root, "target", SERVER_TARGET_TRIPLE, profile, SERVER_BIN_NAME
        )
        if os.path.isfile(candidate):
            return candidate
    return None


def create_magisk_module_zip(include_server=True, output_dir=None):
    """创建 Magisk 模块 ZIP 包"""
    project_root = os.path.dirname(os.path.abspath(__file__))
    module_dir = os.path.join(project_root, "module")
    output_dir = output_dir or os.path.join(project_root, "output")

    service_script = os.path.join(module_dir, "service.sh")
    if not os.path.isfile(service_script):
        print("错误: module/service.sh 不存在，模块无法开机启动", file=sys.stderr)
        return 1

    server_binary = find_server_binary(project_root) if include_server else None
    if include_server and server_binary is None:
        print(
            "警告: 未找到 bin/device_faker_server（target/"
            f"{SERVER_TARGET_TRIPLE}/{{release,debug}}/{SERVER_BIN_NAME}）。",
            file=sys.stderr,
        )
        print(
            "      这次打出的 zip 不含原生 Web 服务；先运行 "
            "`cargo xtask build -r` 或 `cargo ndk -t arm64-v8a --platform 29 "
            "build -p device_faker_server --release`。",
            file=sys.stderr,
        )

    os.makedirs(output_dir, exist_ok=True)

    # 转换所有 .sh 文件为 LF 换行符（CRLF 会让安装器的 ash 解析失败）
    print("正在处理 .sh 文件换行符...")
    for root, _dirs, files in os.walk(module_dir):
        for file in files:
            if file.endswith(".sh"):
                file_path = os.path.join(root, file)
                convert_to_lf(file_path, relative_arcname(module_dir, file_path))

    # 生成带时间戳的 ZIP 文件名
    timestamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
    zip_filename = f"device_faker_{timestamp}.zip"
    zip_path = os.path.join(output_dir, zip_filename)

    print("\n开始打包 Magisk 模块...")
    print(f"输出文件: {zip_filename}")

    entries = 0
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as zipf:
        for root, dirs, files in os.walk(module_dir):
            dirs[:] = sorted(d for d in dirs if not d.startswith("."))
            for file in sorted(files):
                # 跳过 .gitignore / .gitkeep 之类的隐藏文件
                if file.startswith("."):
                    continue
                file_path = os.path.join(root, file)
                arcname = relative_arcname(module_dir, file_path)
                add_file(zipf, file_path, arcname, file_mode(arcname))
                entries += 1
                print(f"  添加: {arcname} (0o{file_mode(arcname):o})")

        if server_binary is not None:
            arcname = f"bin/{SERVER_BIN_NAME}"
            add_file(zipf, server_binary, arcname, 0o755)
            entries += 1
            size_mb = os.path.getsize(server_binary) / (1024 * 1024)
            print(f"  添加: {arcname} (0o755, 来自 {os.path.relpath(server_binary, project_root)}, {size_mb:.2f} MB)")

    file_size = os.path.getsize(zip_path)
    size_mb = file_size / (1024 * 1024)

    print("\n✅ 打包完成！")
    print(f"📦 文件: {os.path.relpath(zip_path, project_root)}")
    print(f"📏 大小: {size_mb:.2f} MB")
    print(f"🗂  条目: {entries}")
    print("\n请将此 ZIP 文件通过 root 管理器安装")
    return 0


def main():
    parser = argparse.ArgumentParser(description="打包 Device Faker Magisk 模块")
    parser.add_argument(
        "--no-server",
        action="store_true",
        help="不打包 bin/device_faker_server（只出 WebUI + 脚本）",
    )
    parser.add_argument(
        "--output-dir",
        default=None,
        help="输出目录（默认 output/）",
    )
    args = parser.parse_args()
    return create_magisk_module_zip(
        include_server=not args.no_server, output_dir=args.output_dir
    )


if __name__ == "__main__":
    sys.exit(main())
