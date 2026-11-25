#!/usr/bin/env python3
# t24 复验（B-1 / H-1）：用真实 device_faker_server 证明
#   1) 仅改 probeUrl 的 dry_run 响应里没有既有密码子串，且**上下文行**里的凭据被替换为 <redacted>；
#   2) config.toml 的 diff 仍然正常输出（脱敏没有把 config 段吃掉）；
#   3) 轮换密码的 diff 里没有新旧密码（两条 password 行都脱敏）；
#   4) 应用级探测失败返回 502 + details.scope="app" + warning egress_probe_scope_app，且不动设备级 default；
#      （settings 在 host 上不可用，「settings get 不变」由 egress_api 的集成测试断言）
#
# 用法：python3 docs/design/egress/evidence-t24/repro-b1-h1.py
# 依赖：target/debug/device_faker_server（host 可跑；settings 缺失 → 写路径 503，但文件已落盘）。

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
DATA = "/tmp/dfs-t24"
PORT = 8688
BASE = f"http://127.0.0.1:{PORT}/api/v1"
CONFIG = os.path.join(DATA, "config", "config.toml")
REDACTED = 'password = "<redacted>"'


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
            return response.status, (json.loads(raw) if raw else None), raw
    except urllib.error.HTTPError as error:
        raw = error.read().decode()
        try:
            return error.code, json.loads(raw), raw
        except ValueError:
            return error.code, None, raw


def revision():
    status, body, _ = req("GET", "/egress")
    assert status == 200, body
    return body["revision"]


def proxies(password):
    # 顺序刻意让「带凭据的代理」成为 [probe] 之前的最后一张表：
    # unified diff 的 3 行上下文会包含 username/password 行（评审 B-1 场景 A）。
    return [
        {"name": "dead", "type": "socks5", "host": "127.0.0.1", "port": 9},
        {"name": "us_la_01", "type": "http", "host": "203.0.113.10", "port": 1080,
         "username": "alice", "password": password},
    ]


def main():
    shutil.rmtree(DATA, ignore_errors=True)
    os.makedirs(os.path.join(DATA, "config"), exist_ok=True)
    log = open("/tmp/t24-device.log", "w+", encoding="utf-8")
    proc = subprocess.Popen(
        [os.path.join(ROOT, "target/debug/device_faker_server"),
         "--config-path", CONFIG, "--bind", f"127.0.0.1:{PORT}",
         "--server-config", os.path.join(DATA, "config", "server.toml")],
        cwd=ROOT, stdout=subprocess.DEVNULL, stderr=log,
    )
    failures = []

    def check(name, ok, detail=""):
        print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}")
        if not ok:
            failures.append(name)

    try:
        for _ in range(100):
            try:
                with urllib.request.urlopen(f"{BASE}/healthz", timeout=2) as r:
                    if r.status == 200:
                        break
            except Exception:
                time.sleep(0.2)

        print("=== 准备：写入一条带凭据的代理 + 设备级 default + 一个指向死端口的应用代理 ===")
        rev = revision()
        status, _, _ = req("PUT", "/egress", {
            "default": "us_la_01",
            "proxies": proxies("s3cret"),
            "apps": [{"package": "com.example.inline", "egress": "dead"}],
        }, if_match=rev)
        check("设备级写入落盘（503 = settings 缺失，符合 host 预期）", status == 503, f"status={status}")
        text = open(CONFIG, encoding="utf-8").read()
        check("config.toml 有 [egress].default", 'default = "us_la_01"' in text)
        egress_text = open(os.path.join(DATA, "config", "egress.toml"), encoding="utf-8").read()
        check("egress.toml 有明文密码（前置条件）", 'password = "s3cret"' in egress_text)

        print("\n=== 1) B-1：只改 probeUrl 的 dry_run（评审给的复现方式，密码只出现在上下文行） ===")
        rev = revision()
        status, body, raw = req("PUT", "/egress?dry_run=true",
                                {"probeUrl": "http://checkip.example.com/"}, if_match=rev)
        diff = (body or {}).get("diff") or ""
        check("dry_run 返回 200", status == 200, f"status={status}")
        check("响应不含既有密码 s3cret", "s3cret" not in raw)
        check("响应不含用户名 alice", "alice" not in raw)
        check("上下文里的凭据行被替换为 <redacted>", REDACTED in diff)
        check("diff 仍保留 egress.toml 段与 probeUrl 变更",
              "--- a/egress.toml" in diff and '+url = "http://checkip.example.com/"' in diff)
        check("只改 probeUrl 时没有 config.toml 段", "--- a/config.toml" not in diff)

        print("\n=== 2) B-1 变体：dry_run 同时改 config（default）+ egress（probeUrl） ===")
        rev = revision()
        status, body, raw = req("PUT", "/egress?dry_run=true",
                                {"default": None, "probeUrl": "http://checkip.example.com/"}, if_match=rev)
        diff = (body or {}).get("diff") or ""
        check("dry_run 返回 200", status == 200, f"status={status}")
        check("config.toml 段仍在", "--- a/config.toml" in diff)
        check("config 段保留了 default 的删除行", '-default = "us_la_01"' in diff)
        check("egress 段仍在且脱敏", "--- a/egress.toml" in diff and REDACTED in diff)
        check("响应不含 s3cret / alice", "s3cret" not in raw and "alice" not in raw)

        print("\n=== 3) B-1 变体：轮换密码（dry_run + diff） ===")
        rev = revision()
        status, body, raw = req("PUT", "/egress?dry_run=true", {
            "proxies": proxies("n3wpass"),
            "apps": [{"package": "com.example.inline", "egress": "dead"}],
        }, if_match=rev)
        diff = (body or {}).get("diff") or ""
        check("dry_run 返回 200", status == 200, f"status={status}")
        check("响应不含旧密码 s3cret", "s3cret" not in raw)
        check("响应不含新密码 n3wpass", "n3wpass" not in raw)
        check("diff 有两条脱敏的 password 行（- 与 +）", diff.count(REDACTED) >= 2, f"count={diff.count(REDACTED)}")

        print("\n=== 4) H-1：应用级探测失败 → 502 + scope=app + warning，且不动设备级 default ===")
        status, body, _ = req("POST", "/egress/probe", {"package": "com.example.inline"})
        details = ((body or {}).get("error") or {}).get("details") or {}
        check("探测失败返回 502", status == 502, f"status={status}")
        check("details.scope == app", details.get("scope") == "app", f"scope={details.get('scope')}")
        check("details.warnings 带 egress_probe_scope_app",
              any(w.get("code") == "egress_probe_scope_app" for w in details.get("warnings") or []),
              json.dumps(details.get("warnings"), ensure_ascii=False))
        status, body, _ = req("GET", "/egress")
        check("设备级 default 未被应用级失败清掉", body.get("default") == "us_la_01", f"default={body.get('default')}")
        check("health.degraded == false（应用级失败不降级）",
              ((body or {}).get("health") or {}).get("degraded") is False)
        check("effectiveMode 仍为 proxy", body.get("effectiveMode") == "proxy", f"effectiveMode={body.get('effectiveMode')}")

        print()
        if failures:
            print(f"RESULT: FAIL ({len(failures)}): {failures}")
            return 1
        print("RESULT: PASS（B-1 与 H-1 的真实二进制复验全部通过）")
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
