#!/usr/bin/env python3
# t12 评审复现：Manager 侧两条「下发语义」缺陷。
#
#   A) POST /devices/egress/apply 的 dryRun=true + assignments 形态会**真的落库**（契约 §6.10：dryRun 不落库）。
#   B) 代理凭据轮换后，apply 比对（egress_unchanged）只比 name/type/host/port，
#      而设备端 GET /egress 本来就只回 hasCredentials —— 新密码永远推不下去。
#
# 用法（仓库根目录）：python3 docs/design/egress/evidence-t12/repro-manager-egress.py
# 依赖：target/debug/devmask_manager 已构建；docs/design/egress/fixtures/mock_device_egress.py。

import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

MGR_PORT = 18766
MOCK_PORT = 18696
DATA = "/tmp/dm-t12"
BASE = f"http://127.0.0.1:{MGR_PORT}/relay/api/v1"
TOKEN = "admin123"
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))


def req(method, path, body=None, token=TOKEN):
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(BASE + path, data=data, method=method)
    request.add_header("Authorization", "Bearer " + token)
    if data is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            raw = response.read().decode()
            return response.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as error:
        raw = error.read().decode()
        try:
            return error.code, json.loads(raw)
        except ValueError:
            return error.code, raw


def wait_ready():
    for _ in range(100):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{MGR_PORT}/healthz", timeout=2) as r:
                if r.status == 200:
                    return True
        except Exception:
            pass
        time.sleep(0.2)
    return False


def wait_job(job_id):
    for _ in range(100):
        status, body = req("GET", f"/jobs/{job_id}")
        if status == 200 and body.get("status") in ("succeeded", "partial", "failed"):
            return body
        time.sleep(0.1)
    return body


def main():
    shutil.rmtree(DATA, ignore_errors=True)
    mock_log = open("/tmp/t12-mock.log", "w+", encoding="utf-8")
    manager_log = open("/tmp/t12-manager.log", "w+", encoding="utf-8")
    procs = []
    try:
        mock = subprocess.Popen(
            [sys.executable, os.path.join(ROOT, "docs/design/egress/fixtures/mock_device_egress.py"),
             "--port", str(MOCK_PORT), "--mode", "ok"],
            stdout=subprocess.DEVNULL, stderr=mock_log,
        )
        procs.append(mock)
        manager = subprocess.Popen(
            [os.path.join(ROOT, "target/debug/devmask_manager"),
             "--bind", f"127.0.0.1:{MGR_PORT}", "--data-dir", DATA, "--token", TOKEN],
            cwd=ROOT, stdout=subprocess.DEVNULL, stderr=manager_log,
        )
        procs.append(manager)
        if not wait_ready():
            print("FATAL: manager 未就绪"); return 2

        print("=== 0. 准备：注册 mock 设备 + 建一条带凭据的代理 ===")
        status, device = req("POST", "/devices", {
            "name": "t12-mock",
            "baseUrl": f"http://127.0.0.1:{MOCK_PORT}",
            "token": "mocktoken",
        })
        device_id = device["id"]
        print(f"device: HTTP {status} id={device_id}")
        status, proxy = req("POST", "/egress-proxies", {
            "name": "t12-http",
            "type": "http",
            "host": "127.0.0.1",
            "port": 18080,
            "username": "alice",
            "password": "s3cret-1",
        })
        proxy_id = proxy["id"]
        print(f"proxy : HTTP {status} id={proxy_id} hasCredentials={proxy['hasCredentials']}")

        print()
        print("=== A. dryRun + assignments 是否真的「不落库」 ===")
        _, before = req("GET", f"/devices/{device_id}/egress")
        print("dryRun 前 bindings =", before["bindings"])
        status, planned = req("POST", "/devices/egress/apply", {
            "assignments": [{"deviceId": device_id, "app": "", "proxyId": proxy_id}],
            "dryRun": True,
        })
        print(f"dryRun 响应: HTTP {status} dryRun={planned.get('dryRun')} total={planned.get('total')}")
        print("plan =", json.dumps(planned.get("plan"), ensure_ascii=False))
        _, after = req("GET", f"/devices/{device_id}/egress")
        print("dryRun 后 bindings =", after["bindings"])
        leaked = len(after["bindings"]) - len(before["bindings"])
        print(f">>> A 结论: dryRun 之后绑定行数变化 = {leaked}（契约 §6.10 要求 0；非 0 即 dryRun 落库）")

        print()
        print("=== B. 凭据轮换后 apply 是否把新密码推到设备 ===")
        # 先做一次真实下发（此时绑定已在 A 步骤里被 dryRun 落库，这里再 PUT 一次保证状态明确）
        status, _ = req("PUT", f"/devices/{device_id}/egress", {
            "bindings": [{"app": "", "proxyId": proxy_id}],
        })
        print(f"PUT bindings: HTTP {status}")
        status, accepted = req("POST", "/devices/egress/apply", {"devices": [device_id]})
        job1 = wait_job(accepted["jobId"])
        put_after_first = mock_log.flush() or open("/tmp/t12-mock.log", encoding="utf-8").read().count("PUT /api/v1/egress")
        print(f"首次 apply: HTTP {status} status={job1['status']} results={[(r['app'], r['status'], r['code']) for r in job1['results']]}")
        print(f"设备侧 PUT /egress 次数（首次下发后）= {put_after_first}")

        print("-- 轮换密码：PATCH /egress-proxies/{id} password=s3cret-2 --")
        status, patched = req("PATCH", f"/egress-proxies/{proxy_id}", {"password": "s3cret-2"})
        print(f"PATCH: HTTP {status} hasCredentials={patched['hasCredentials']} updatedAt={patched['updatedAt']}")
        status, accepted2 = req("POST", "/devices/egress/apply", {"devices": [device_id]})
        job2 = wait_job(accepted2["jobId"])
        time.sleep(0.3)
        puts_total = open("/tmp/t12-mock.log", encoding="utf-8").read().count("PUT /api/v1/egress")
        print(f"轮换后 apply: HTTP {status} status={job2['status']} results={[(r['app'], r['status'], r['code']) for r in job2['results']]}")
        print(f"设备侧 PUT /egress 次数（轮换后）= {puts_total}（与首次相同 = 没有下发新密码）")
        print(f">>> B 结论: 轮换后 code={job2['results'][0]['code']!r}、PUT 次数增量={puts_total - put_after_first}")

        print()
        print("=== C. 旁证：设备侧最后一次收到的定义（mock 内存态，GET /egress/proxies 脱敏）===")
        status, proxies = req("GET", f"/devices/{device_id}/egress")
        state = proxies.get("deviceState") or {}
        print("deviceState.proxies =", json.dumps(state.get("proxies"), ensure_ascii=False))
        print("deviceState.default =", state.get("default"))
        return 0
    finally:
        for proc in procs:
            proc.send_signal(signal.SIGTERM)
        for proc in procs:
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        mock_log.close()
        manager_log.close()


if __name__ == "__main__":
    sys.exit(main())
