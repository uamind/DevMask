#!/usr/bin/env python3
# t30 复验：t12 的 B-1 / B-2 / B-3 修复（真实 manager 二进制 + docs/design/egress/fixtures/mock_device_egress.py）。
#   A) dryRun + assignments 不落库：bindings 不变、无 egress.bind 审计、不建 job；
#   B) 凭据轮换后 apply 真的再发一次 PUT；
#   C) 解绑全部后 apply：设备侧 default/apps/proxies 归零。
#
# 用法：python3 docs/design/egress/evidence-t30/repro-b1-b3-fixed.py

import json, os, shutil, signal, subprocess, sys, time, urllib.error, urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
MGR_PORT, MOCK_PORT = 18768, 18698
DATA = "/tmp/dm-t30"
BASE = f"http://127.0.0.1:{MGR_PORT}/relay/api/v1"
TOKEN = "admin123"
MOCK_LOG = "/tmp/t30-mock.log"


def req(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(BASE + path, data=data, method=method)
    r.add_header("Authorization", "Bearer " + TOKEN)
    if data is not None:
        r.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(r, timeout=20) as resp:
            raw = resp.read().decode()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        try:
            return e.code, json.loads(raw)
        except ValueError:
            return e.code, raw


def puts():
    with open(MOCK_LOG, encoding="utf-8") as f:
        return f.read().count("PUT /api/v1/egress")


def wait_job(job_id):
    for _ in range(100):
        st, body = req("GET", f"/jobs/{job_id}")
        if st == 200 and body.get("status") in ("succeeded", "partial", "failed"):
            return body
        time.sleep(0.1)
    return body


def main():
    shutil.rmtree(DATA, ignore_errors=True)
    log = open(MOCK_LOG, "w+", encoding="utf-8")
    mgr_log = open("/tmp/t30-manager.log", "w+", encoding="utf-8")
    procs = []
    fails = []
    def check(name, ok, detail=""):
        print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}")
        if not ok:
            fails.append(name)
    try:
        procs.append(subprocess.Popen(
            [sys.executable, os.path.join(ROOT, "docs/design/egress/fixtures/mock_device_egress.py"),
             "--port", str(MOCK_PORT), "--mode", "ok"],
            stdout=subprocess.DEVNULL, stderr=log))
        procs.append(subprocess.Popen(
            [os.path.join(ROOT, "target/debug/devmask_manager"), "--bind", f"127.0.0.1:{MGR_PORT}",
             "--data-dir", DATA, "--token", TOKEN],
            cwd=ROOT, stdout=subprocess.DEVNULL, stderr=mgr_log))
        for _ in range(100):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{MGR_PORT}/healthz", timeout=2) as r:
                    if r.status == 200:
                        break
            except Exception:
                time.sleep(0.2)
        st, dev = req("POST", "/devices", {"name": "t30-mock", "baseUrl": f"http://127.0.0.1:{MOCK_PORT}", "token": "mocktoken"})
        did = dev["id"]
        check("设备注册", st == 201, f"status={st}")
        st, proxy = req("POST", "/egress-proxies", {"name": "t30-http", "type": "http", "host": "127.0.0.1", "port": 18080, "username": "alice", "password": "s3cret-1"})
        pid = proxy["id"]
        check("代理创建", st == 201, f"status={st}")

        print("\n=== A) dryRun + assignments 不落库 ===")
        _, before_audit = req("GET", "/audit?action=egress.bind&limit=200")
        _, before_jobs = req("GET", "/audit?action=egress.apply&limit=200")
        st, dry = req("POST", "/devices/egress/apply", {"assignments": [{"deviceId": did, "app": "", "proxyId": pid}], "dryRun": True})
        check("dryRun 200 + dryRun:true", st == 200 and dry.get("dryRun") is True, f"status={st}")
        _, eg = req("GET", f"/devices/{did}/egress")
        check("bindings 仍为空", eg["bindings"] == [], json.dumps(eg["bindings"], ensure_ascii=False))
        _, after_audit = req("GET", "/audit?action=egress.bind&limit=200")
        _, after_jobs = req("GET", "/audit?action=egress.apply&limit=200")
        check("无新增 egress.bind 审计", after_audit["total"] == before_audit["total"], f"{before_audit['total']} -> {after_audit['total']}")
        check("无新建 egress.apply", after_jobs["total"] == before_jobs["total"], f"{before_jobs['total']} -> {after_jobs['total']}")

        print("\n=== B) 轮换凭据后 apply 必须再发 PUT ===")
        req("PUT", f"/devices/{did}/egress", {"bindings": [{"app": "", "proxyId": pid}]})
        _, acc = req("POST", "/devices/egress/apply", {"devices": [did]})
        job1 = wait_job(acc["jobId"])
        puts1 = puts()
        check("首次下发成功", job1["status"] == "succeeded", json.dumps(job1.get("results"), ensure_ascii=False))
        check("首次下发有 PUT", puts1 >= 1, f"puts={puts1}")
        st, _ = req("PATCH", f"/egress-proxies/{pid}", {"password": "s3cret-2"})
        check("PATCH 轮换 200", st == 200, f"status={st}")
        _, acc2 = req("POST", "/devices/egress/apply", {"devices": [did]})
        job2 = wait_job(acc2["jobId"])
        puts2 = puts()
        check("轮换后 apply 成功", job2["status"] == "succeeded")
        check("轮换后再发了一次 PUT（旧实现会 egress_unchanged 跳过）", puts2 == puts1 + 1, f"{puts1} -> {puts2}")
        check("结果码不是 egress_unchanged", job2["results"][0].get("code") is None, json.dumps(job2["results"], ensure_ascii=False))

        print("\n=== C) 解绑全部后 apply：设备侧归零 ===")
        req("PUT", f"/devices/{did}/egress", {"bindings": []})
        _, acc3 = req("POST", "/devices/egress/apply", {"devices": [did]})
        job3 = wait_job(acc3["jobId"])
        check("解绑后 apply 成功", job3["status"] == "succeeded")
        _, eg2 = req("GET", f"/devices/{did}/egress")
        state = eg2.get("deviceState") or {}
        check("设备 default 归零", state.get("default") is None, f"default={state.get('default')}")
        check("设备 apps 归零", state.get("apps") == [], json.dumps(state.get("apps"), ensure_ascii=False))
        check("设备 proxies 归零", state.get("proxies") == [], json.dumps(state.get("proxies"), ensure_ascii=False))

        print("\n=== D) 非法 Idempotency-Key 在写之前被拒 ===")
        data = json.dumps({"devices": [did]}).encode()
        r = urllib.request.Request(BASE + "/devices/egress/apply", data=data, method="POST")
        r.add_header("Authorization", "Bearer " + TOKEN)
        r.add_header("Content-Type", "application/json")
        r.add_header("Idempotency-Key", "bad key with spaces")
        try:
            with urllib.request.urlopen(r, timeout=10) as resp:
                st = resp.status
        except urllib.error.HTTPError as e:
            st = e.code
        check("非法 key → 422", st == 422, f"status={st}")

        print()
        if fails:
            print(f"RESULT: FAIL ({len(fails)}): {fails}")
            return 1
        print("RESULT: PASS（B-1/B-2/B-3 与 M-1 端到端全部成立）")
        return 0
    finally:
        for p in procs:
            p.send_signal(signal.SIGTERM)
        for p in procs:
            try: p.wait(timeout=5)
            except subprocess.TimeoutExpired: p.kill()
        log.close(); mgr_log.close()


if __name__ == "__main__":
    sys.exit(main())
