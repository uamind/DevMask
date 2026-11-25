#!/usr/bin/env python3
# t30 复验（captain 指定）：Manager 发 "password": null 是否真的清掉设备端已存凭据。
# 用真实 devmask_server 验证 merge_credential(Some(None)) → render 删掉 password 行。
#
# 用法：python3 docs/design/egress/evidence-t30/repro-null-clears-device-credential.py

import json, os, shutil, signal, subprocess, sys, time, urllib.error, urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
DATA = "/tmp/dfs-t30"
PORT = 8689
BASE = f"http://127.0.0.1:{PORT}/api/v1"
CONFIG = os.path.join(DATA, "config", "config.toml")
EGRESS = os.path.join(DATA, "config", "egress.toml")


def req(method, path, body=None, if_match=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(BASE + path, data=data, method=method)
    r.add_header("Authorization", "Bearer admin123")
    if data is not None:
        r.add_header("Content-Type", "application/json")
    if if_match is not None:
        r.add_header("If-Match", '"' + if_match + '"')
    try:
        with urllib.request.urlopen(r, timeout=15) as resp:
            raw = resp.read().decode()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        try:
            return e.code, json.loads(raw)
        except ValueError:
            return e.code, raw


def main():
    shutil.rmtree(DATA, ignore_errors=True)
    os.makedirs(os.path.join(DATA, "config"), exist_ok=True)
    log = open("/tmp/t30-device.log", "w+", encoding="utf-8")
    proc = subprocess.Popen(
        [os.path.join(ROOT, "target/debug/devmask_server"), "--config-path", CONFIG,
         "--bind", f"127.0.0.1:{PORT}", "--server-config", os.path.join(DATA, "config", "server.toml")],
        cwd=ROOT, stdout=subprocess.DEVNULL, stderr=log)
    fails = []
    def check(name, ok, detail=""):
        print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}")
        if not ok: fails.append(name)
    try:
        for _ in range(100):
            try:
                with urllib.request.urlopen(f"{BASE}/healthz", timeout=2) as r:
                    if r.status == 200: break
            except Exception: time.sleep(0.2)
        st, eg = req("GET", "/egress"); rev = eg["revision"]
        with_creds = {"default": None, "apps": None, "proxies": [
            {"name": "p1", "type": "http", "host": "203.0.113.10", "port": 1080,
             "username": "alice", "password": "s3cret"}]}
        st, _ = req("PUT", "/egress", with_creds, if_match=rev)
        text = open(EGRESS, encoding="utf-8").read()
        check("首次写入带凭据", 'password = "s3cret"' in text and 'username = "alice"' in text, f"status={st}")

        st, eg = req("GET", "/egress"); rev = eg["revision"]
        cleared = {"proxies": [
            {"name": "p1", "type": "http", "host": "203.0.113.10", "port": 1080,
             "username": None, "password": None}]}
        st, _ = req("PUT", "/egress", cleared, if_match=rev)
        text = open(EGRESS, encoding="utf-8").read()
        check("password:null 清掉已存密码", "password" not in text, text.replace(chr(10), " | "))
        check("username:null 清掉已存用户名", "username" not in text)
        check("代理定义仍在（只清凭据）", 'host = "203.0.113.10"' in text)

        st, eg = req("GET", "/egress")
        summary = {p["name"]: p.get("hasCredentials") for p in eg.get("proxies") or []}
        check("GET /egress proxies[0].hasCredentials == false", summary.get("p1") is False, json.dumps(summary))
        print()
        if fails:
            print(f"RESULT: FAIL ({len(fails)}): {fails}"); return 1
        print("RESULT: PASS（显式 null 真的能清掉设备端凭据，且不影响代理定义）"); return 0
    finally:
        proc.send_signal(signal.SIGTERM)
        try: proc.wait(timeout=5)
        except subprocess.TimeoutExpired: proc.kill()
        log.close()


if __name__ == "__main__":
    sys.exit(main())
