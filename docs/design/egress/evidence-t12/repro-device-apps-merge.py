#!/usr/bin/env python3
# t12 评审复现（设备端语义旁证）：PUT /api/v1/egress 的 apps 数组不是「整体替换」。
#
# Manager 下发的期望态总是给 apps 数组（jobs.rs:1585-1593）。契约 §1.4 说「给了数组 = 整体替换」，
# 但 device server 的 apply_app_egress 只对列出的条目做 upsert/delete ——
# 于是「把某个应用级绑定从 Manager 里删掉」永远到不了设备。
#
# 用法：python3 docs/design/egress/evidence-t12/repro-device-apps-merge.py
# 依赖：target/debug/device_faker_server（host 上可跑；settings 命令缺失 → 写路径 503，但文件已落盘）。

import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
DATA = "/tmp/dfs-t12"
PORT = 8687
BASE = f"http://127.0.0.1:{PORT}/api/v1"
CONFIG = os.path.join(DATA, "config", "config.toml")


def req(method, path, body=None, if_match=None):
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(BASE + path, data=data, method=method)
    request.add_header("Authorization", "Bearer admin123")
    if data is not None:
        request.add_header("Content-Type", "application/json")
    if if_match is not None:
        request.add_header("If-Match", '"' + if_match + '"')
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            raw = response.read().decode()
            return response.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as error:
        raw = error.read().decode()
        try:
            return error.code, json.loads(raw)
        except ValueError:
            return error.code, raw


def read_config():
    try:
        with open(CONFIG, encoding="utf-8") as handle:
            return handle.read()
    except FileNotFoundError:
        return "<missing>"


def show_apps(label):
    text = read_config()
    apps = [line.strip() for line in text.splitlines() if line.strip().startswith("egress =")]
    print(f"{label}: config.toml apps 里的 egress 行 = {apps}")
    return apps


def main():
    shutil.rmtree(DATA, ignore_errors=True)
    os.makedirs(os.path.join(DATA, "config"), exist_ok=True)
    log = open("/tmp/t12-device.log", "w+", encoding="utf-8")
    proc = subprocess.Popen(
        [os.path.join(ROOT, "target/debug/device_faker_server"),
         "--config-path", CONFIG, "--bind", f"127.0.0.1:{PORT}",
         "--server-config", os.path.join(DATA, "config", "server.toml")],
        cwd=ROOT, stdout=subprocess.DEVNULL, stderr=log,
    )
    try:
        for _ in range(100):
            try:
                with urllib.request.urlopen(f"{BASE}/healthz", timeout=2) as r:
                    if r.status == 200:
                        break
            except Exception:
                time.sleep(0.2)
        status, egress = req("GET", "/egress")
        if status != 200:
            print(f"FATAL: GET /egress -> {status} {egress}")
            return 2
        revision = egress["revision"]
        print(f"0) 初始 revision={revision[:12]}… apps={egress['apps']}")

        print("\n1) 绑定一个应用级 __DIRECT__（模拟 Manager 下发一次 apps=[{com.example.a}]）")
        status, body = req(
            "PUT",
            "/egress",
            {"apps": [{"package": "com.example.a", "egress": "__DIRECT__"}]},
            if_match=revision,
        )
        print(f"   PUT apps=[com.example.a] -> HTTP {status}（host 无 settings 命令，503/200 都可，重点看文件）")
        show_apps("   写入后")

        status, egress = req("GET", "/egress")
        revision = egress["revision"]

        print("\n2) Manager 删掉这条绑定 → 下发 apps=[]（契约 §1.4：整体替换 = 清空）")
        status, body = req("PUT", "/egress", {"apps": []}, if_match=revision)
        print(f"   PUT apps=[] -> HTTP {status} changed={body.get('changed') if isinstance(body, dict) else body}")
        apps = show_apps("   写入后")
        status, egress = req("GET", "/egress")
        print(f"   GET /egress apps = {egress['apps']}")
        print(f"   >>> 结论: apps=[] 之后仍残留 {len(apps)} 行 → 契约「整体替换」未实现")

        print("\n3) 对照：显式 apps=null（device 端唯一能清空的写法）")
        revision = egress["revision"]
        status, body = req("PUT", "/egress", {"apps": None}, if_match=revision)
        print(f"   PUT apps=null -> HTTP {status}")
        apps = show_apps("   写入后")
        status, egress = req("GET", "/egress")
        print(f"   GET /egress apps = {egress['apps']}")
        return 0
    finally:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        log.close()


if __name__ == "__main__":
    sys.exit(main())
