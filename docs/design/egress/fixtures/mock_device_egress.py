#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
"""t10 验收用的「带 egress 的设备端 Mock」——**历史验收装置，不再维护**。

> **状态（2026-12-01）**：egress 的五个端点已并入 **`manager/docs/mock_device.py`**
> （用法见 `manager/docs/RUN.md` §6.1）。**日常验证请直接用主线 mock**：
>
>     python3 manager/docs/mock_device.py 18686
>
> 本文件只保留为 t10 的**证据复现装置**（多一个 `--mode fail` 故障注入开关，
> 用来复现设备端 500 → job `device_error`），不随契约演进维护。

当时 manager/docs/mock_device.py 只实现 status/overview/revision/templates/apps，
没有 /api/v1/egress，因此 Manager 的 egress 下发在它上面只会得到
egress_unsupported(404)。本 Mock 按 docs/plans/egress-contract.md §1 实现
egress 子集，让「代理池 → 绑定 → 批量下发 → 探测」整条链路能真跑：

  GET  /api/v1/status              设备信息（enroll 用）
  GET  /api/v1/overview            device 对象（enroll 校验用）
  GET  /api/v1/egress              当前期望/生效态（含 revision）
  PUT  /api/v1/egress              合并写（default / proxies / apps / probe 策略）
  DELETE /api/v1/egress?confirm=true  清空（:0 语义等价）
  POST /api/v1/egress/probe        探测（返回固定 exitIp）
  GET  /api/v1/egress/proxies      脱敏代理列表

模式：
  ok    正常
  fail  GET /api/v1/egress 返回 500（模拟设备端故障 → Manager job 结果 failed/device_error）
离线设备不靠模式：直接把进程停掉即可（Manager 侧得到 device_offline）。

用法：python3 mock_device_egress.py --port 18686 --mode ok [--log /tmp/x.log]
"""

import argparse
import hashlib
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

EXIT_IP = "198.51.100.7"
DEVICE = {
    "manufacturer": "Google",
    "brand": "google",
    "model": "Pixel 9",
    "device": "tokay",
    "android": "17",
    "sdk": 37,
    "abi": "arm64-v8a",
}

lock = threading.Lock()
state = {
    "default": None,
    "proxies": [],
    "apps": [],
    "fail_open": True,
    "probe_url": "http://checkip.amazonaws.com/",
    "probe_timeout_seconds": 5,
    "revision": "",
    "mode": "ok",
    "put_count": 0,
    "last_probe": None,
}


def now():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def recompute_revision():
    blob = json.dumps(
        {
            "default": state["default"],
            "proxies": state["proxies"],
            "apps": state["apps"],
            "fail_open": state["fail_open"],
            "probe_url": state["probe_url"],
            "timeout": state["probe_timeout_seconds"],
        },
        sort_keys=True,
        ensure_ascii=False,
    ).encode()
    state["revision"] = hashlib.sha256(blob).hexdigest()


def redacted_proxies():
    out = []
    for item in state["proxies"]:
        out.append(
            {
                "name": item.get("name"),
                "type": item.get("type"),
                "host": item.get("host"),
                "port": item.get("port"),
                "hasCredentials": bool(item.get("username") or item.get("password")),
            }
        )
    return out


def upstream():
    if not state["default"]:
        return None
    for item in state["proxies"]:
        if item.get("name") == state["default"]:
            return {
                "name": item.get("name"),
                "type": item.get("type"),
                "host": item.get("host"),
                "port": item.get("port"),
                "hasCredentials": bool(item.get("username") or item.get("password")),
            }
    return None


def readback():
    up = upstream()
    if up:
        value = "%s:%s" % (up["host"], up["port"])
        dumpsys = "Global proxy: [%s] %s xl=" % (up["host"], up["port"])
    else:
        value = ":0"
        dumpsys = "Global proxy: none"
    return {"settingsValue": value, "dumpsysGlobalProxy": dumpsys, "matches": True, "at": now()}


def snapshot():
    up = upstream()
    return {
        "revision": state["revision"],
        "configRevision": hashlib.sha256(b"mock-config").hexdigest(),
        "configured": bool(state["default"] or state["proxies"] or state["apps"]),
        "mode": "proxy" if state["default"] else "direct",
        "effectiveMode": "proxy" if state["default"] else "direct",
        "default": state["default"],
        "failOpen": state["fail_open"],
        "probeUrl": state["probe_url"],
        "probeTimeoutSeconds": state["probe_timeout_seconds"],
        "exclusionList": [],
        "upstream": up,
        "proxies": redacted_proxies(),
        "apps": [
            {
                "package": item.get("package"),
                "egress": item.get("egress"),
                "effective": "direct" if item.get("egress") == "__DIRECT__" else "proxy",
                "proxy": None if item.get("egress") == "__DIRECT__" else item.get("egress"),
            }
            for item in state["apps"]
        ],
        "applied": True,
        "readback": readback(),
        "health": {
            "degraded": False,
            "degradedSince": None,
            "degradedReason": None,
            "lastProbe": state["last_probe"],
        },
        "egressFile": {"exists": True, "size": 240, "mtime": now()},
        "warnings": [],
    }


def apply_update(body):
    # 与契约一致：省略 = 不改；null = 清空；数组 = 整体替换
    if "default" in body:
        state["default"] = body["default"]
    if "failOpen" in body and body["failOpen"] is not None:
        state["fail_open"] = bool(body["failOpen"])
    if "probeUrl" in body:
        state["probe_url"] = body["probeUrl"] or "http://checkip.amazonaws.com/"
    if "probeTimeoutSeconds" in body and body["probeTimeoutSeconds"] is not None:
        state["probe_timeout_seconds"] = int(body["probeTimeoutSeconds"])
    if "proxies" in body and body["proxies"] is not None:
        state["proxies"] = body["proxies"]
    if "apps" in body and body["apps"] is not None:
        state["apps"] = body["apps"]
    state["put_count"] += 1
    recompute_revision()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("%s [%s] %s\n" % (now(), self.server.mode, fmt % args))

    def _send(self, status, payload):
        body = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def _error(self, status, code, message, details=None):
        self._send(
            status,
            {
                "error": {
                    "code": code,
                    "details": details,
                    "message": message,
                    "requestId": "req_mock_%d" % int(time.time() * 1000),
                    "retryable": False,
                }
            },
        )

    def _body(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length <= 0:
            return {}
        raw = self.rfile.read(length)
        try:
            return json.loads(raw)
        except ValueError:
            return {}

    def do_GET(self):
        path = self.path.split("?")[0]
        with lock:
            if path == "/api/v1/status":
                return self._send(
                    200,
                    {
                        "module": {"id": "device_faker", "version": "v1.10.0", "versionCode": 20261201},
                        "device": DEVICE,
                        "server": {
                            "version": "1.10.0",
                            "bind": "127.0.0.1:%d" % self.server.server_port,
                            "uptimeSec": 42,
                            "pid": 1,
                            "startedAt": now(),
                        },
                        "config": {"revision": state["revision"], "templates": 1, "apps": 1},
                        "capabilities": [
                            "config", "templates", "apps", "packages",
                            "device", "validate", "overview", "egress",
                        ],
                    },
                )
            if path == "/api/v1/overview":
                return self._send(
                    200,
                    {
                        "revision": state["revision"],
                        "device": DEVICE,
                        "templates": {"pixel_9": {"brand": "google", "model": "Pixel 9"}},
                        "apps": {"com.xingin.xhs": "pixel_9"},
                        "conflicts": [],
                    },
                )
            if path == "/api/v1/egress":
                if self.server.mode == "fail":
                    return self._error(500, "internal_error", "mock failure on GET /egress")
                return self._send(200, snapshot())
            if path == "/api/v1/egress/proxies":
                return self._send(
                    200,
                    {
                        "revision": state["revision"],
                        "configRevision": hashlib.sha256(b"mock-config").hexdigest(),
                        "total": len(state["proxies"]),
                        "egressFile": {"exists": True, "size": 240, "mtime": now()},
                        "items": [
                            dict(
                                item,
                                usedAsDefault=item.get("name") == state["default"],
                                apps=[
                                    a.get("package")
                                    for a in state["apps"]
                                    if a.get("egress") == item.get("name")
                                ],
                            )
                            for item in redacted_proxies()
                        ],
                        "warnings": [],
                    },
                )
            return self._error(404, "not_found", "mock: 未实现的路径 %s" % path)

    def do_PUT(self):
        path = self.path.split("?")[0]
        with lock:
            if path != "/api/v1/egress":
                return self._error(404, "not_found", "mock: 未实现的路径 %s" % path)
            if self.server.mode == "fail":
                return self._error(500, "internal_error", "mock failure on PUT /egress")
            previous = state["revision"]
            if_match = self.headers.get("If-Match")
            if self.headers.get("Content-Type", "").startswith("application/json") and not if_match:
                return self._error(
                    428,
                    "precondition_required",
                    "缺少 If-Match",
                    {"header": "If-Match"},
                )
            body = self._body()
            apply_update(body)
            self._send(
                200,
                {
                    "revision": state["revision"],
                    "previousRevision": previous,
                    "configRevision": hashlib.sha256(b"mock-config").hexdigest(),
                    "dryRun": False,
                    "changed": True,
                    "backupId": "mock.toml",
                    "mode": "proxy" if state["default"] else "direct",
                    "effectiveMode": "proxy" if state["default"] else "direct",
                    "default": state["default"],
                    "applied": True,
                    "upstream": upstream(),
                    "readback": readback(),
                    "health": {"degraded": False, "degradedSince": None, "degradedReason": None, "lastProbe": state["last_probe"]},
                    "warnings": [],
                },
            )

    def do_DELETE(self):
        path = self.path.split("?")[0]
        with lock:
            if path != "/api/v1/egress":
                return self._error(404, "not_found", "mock: 未实现的路径 %s" % path)
            previous = state["revision"]
            state.update({"default": None, "proxies": [], "apps": []})
            recompute_revision()
            self._send(
                200,
                {
                    "revision": state["revision"],
                    "previousRevision": previous,
                    "configRevision": hashlib.sha256(b"mock-config").hexdigest(),
                    "dryRun": False,
                    "changed": True,
                    "backupId": "mock.toml",
                    "mode": "direct",
                    "effectiveMode": "direct",
                    "default": None,
                    "applied": True,
                    "upstream": None,
                    "readback": readback(),
                    "health": {"degraded": False, "degradedSince": None, "degradedReason": None, "lastProbe": None},
                    "warnings": [],
                },
            )

    def do_POST(self):
        path = self.path.split("?")[0]
        with lock:
            if path != "/api/v1/egress/probe":
                return self._error(404, "not_found", "mock: 未实现的路径 %s" % path)
            body = self._body()
            scope = "app" if body.get("package") else "device"
            proxy_name = body.get("package") and None or state["default"]
            result = {
                "ok": True,
                "exitIp": EXIT_IP,
                "latencyMs": 12,
                "checkedAt": now(),
                "scope": scope,
                "via": "proxy" if proxy_name else "direct",
                "proxy": proxy_name,
                "url": body.get("url") or state["probe_url"],
                "upstreamStatus": 200,
                "approximate": scope == "app",
                "warnings": [],
            }
            state["last_probe"] = {
                "ok": True,
                "exitIp": EXIT_IP,
                "latencyMs": 12,
                "checkedAt": result["checkedAt"],
                "scope": scope,
                "via": result["via"],
                "proxy": proxy_name,
                "upstreamStatus": 200,
                "errorCode": None,
            }
            self._send(200, result)


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, addr, handler, mode):
        super().__init__(addr, handler)
        self.mode = mode


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18686)
    parser.add_argument("--mode", default="ok", choices=["ok", "fail"])
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()
    sys.stderr.write(
        "[mock] 历史装置：egress 已并入 manager/docs/mock_device.py（见 manager/docs/RUN.md §6.1）\n"
    )
    recompute_revision()
    server = Server((args.host, args.port), Handler, args.mode)
    sys.stderr.write(
        "%s [mock] listening %s:%d mode=%s\n" % (now(), args.host, args.port, args.mode)
    )
    sys.stderr.flush()
    server.serve_forever()


if __name__ == "__main__":
    main()
