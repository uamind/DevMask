#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# Device Faker: https://github.com/Seyud/device_faker
"""Device Faker 设备端最小 Mock：供 Manager / Relay 冒烟验收。

用法：
    python3 manager/docs/mock_device.py 18686

实现 Manager 需要的 /api/v1 子集（status、overview、revision、templates、
apps）并默认在首次 PUT /apps/{pkg} 返回 412，用于验证 Relay 的 revision
重取与重试逻辑。仅用于本地联调，不写入任何真实设备。
"""

import hashlib
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

TOKEN = "admin123"
FORCE_412_ONCE = True
STATE = {
    "templates": {
        "pixel_9": {
            "packages": [],
            "brand": "google",
            "model": "Pixel 9",
            "android_version": "17",
            "sdk_int": 37,
            "dpi": 420,
        }
    },
    "apps": {"com.xingin.xhs": "pixel_9"},
    "forced": FORCE_412_ONCE,
}


def revision():
    blob = json.dumps(STATE, sort_keys=True, ensure_ascii=False).encode()
    return hashlib.sha256(blob).hexdigest()


def overview():
    rev = revision()
    return {
        "revision": rev,
        "module": {"id": "device_faker", "version": "v1.9.0", "versionCode": 20261125},
        "device": {
            "manufacturer": "Google",
            "brand": "google",
            "model": "Pixel 9",
            "device": "tokay",
            "android": "17",
            "sdk": 37,
            "abi": "arm64-v8a",
            "fingerprint": "google/tokay/tokay:17/BP2A.250605.031/13000000:user/release-keys",
        },
        "root": {"manager": "kernelsu", "version": "1.0.7"},
        "server": {
            "version": "1.9.0",
            "bind": "127.0.0.1:18686",
            "uptimeSec": 3600,
            "startedAt": "2026-11-24T08:00:00Z",
            "pid": 4321,
        },
        "config": {
            "revision": rev,
            "mtime": "2026-11-24T08:00:00Z",
            "size": 256,
            "exists": True,
            "templates": len(STATE["templates"]),
            "apps": len(STATE["apps"]),
            "conflicts": 0,
        },
        "templates": [
            {
                "name": name,
                "position": i,
                "packages": cfg.get("packages", []),
                "config": {k: v for k, v in cfg.items() if k != "packages"},
            }
            for i, (name, cfg) in enumerate(STATE["templates"].items())
        ],
        "apps": [
            {
                "package": pkg,
                "installed": True,
                "source": {"type": "template", "template": tpl, "alsoInline": False},
                "resolved": {"brand": "google", "model": "Pixel 9", "dpi": 420},
                "warnings": [],
            }
            for pkg, tpl in STATE["apps"].items()
        ],
        "warnings": [],
    }


def app_payload(pkg):
    tpl = STATE["apps"].get(pkg)
    if tpl is None:
        return None
    return {
        "package": pkg,
        "revision": revision(),
        "source": {"type": "template", "template": tpl, "alsoInline": False},
        "resolved": {"brand": "google", "model": "Pixel 9", "dpi": 420},
        "warnings": [],
    }


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("X-Request-Id", "req_mock")
        self.send_header("X-DF-Revision", revision())
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _error(self, status, code, message, details=None):
        self._send(
            status,
            {
                "error": {
                    "code": code,
                    "message": message,
                    "details": details,
                    "retryable": False,
                    "requestId": "req_mock",
                }
            },
        )

    def _authorized(self):
        if self.headers.get("Authorization") != "Bearer " + TOKEN:
            self._error(401, "unauthorized", "bad token")
            return False
        return True

    def _body(self):
        length = int(self.headers.get("Content-Length", "0"))
        return json.loads(self.rfile.read(length) or b"{}")

    def do_GET(self):
        if not self._authorized():
            return
        path = self.path.split("?")[0]
        if path == "/api/v1/status":
            ov = overview()
            return self._send(
                200,
                {
                    "module": ov["module"],
                    "device": ov["device"],
                    "server": ov["server"],
                    "config": ov["config"],
                    "capabilities": ["config", "templates", "apps", "overview"],
                },
            )
        if path == "/api/v1/overview":
            return self._send(200, overview())
        if path == "/api/v1/config/revision":
            return self._send(
                200,
                {
                    "revision": revision(),
                    "mtime": "2026-11-24T08:00:00Z",
                    "size": 256,
                    "exists": True,
                },
            )
        if path.startswith("/api/v1/templates/"):
            name = path.rsplit("/", 1)[-1]
            cfg = STATE["templates"].get(name)
            if cfg is None:
                return self._error(
                    404, "not_found", "template missing", {"resource": "template"}
                )
            return self._send(
                200,
                {"revision": revision(), "position": 0, "name": name, "template": cfg},
            )
        if path.startswith("/api/v1/apps/"):
            pkg = path.rsplit("/", 1)[-1]
            payload = app_payload(pkg)
            if payload is None:
                return self._error(
                    404,
                    "not_found",
                    "app config missing",
                    {"resource": "app", "name": pkg},
                )
            return self._send(200, payload)
        return self._error(404, "not_found", "no route", {"resource": "route"})

    def do_PUT(self):
        if not self._authorized():
            return
        path = self.path.split("?")[0]
        body = self._body()
        if path.startswith("/api/v1/templates/"):
            name = path.rsplit("/", 1)[-1]
            existed = name in STATE["templates"]
            STATE["templates"][name] = body
            return self._send(
                200 if existed else 201, {"revision": revision(), "warnings": []}
            )
        if path.startswith("/api/v1/apps/"):
            pkg = path.rsplit("/", 1)[-1]
            if pkg == "com.missing.app":
                return self._error(
                    404,
                    "not_found",
                    "package not installed",
                    {"resource": "package", "name": pkg},
                )
            if STATE["forced"]:
                STATE["forced"] = False
                return self._error(
                    412,
                    "config_revision_mismatch",
                    "revision mismatch",
                    {"expected": revision(), "actual": "0" * 64},
                )
            if_match = (self.headers.get("If-Match") or "").strip('"')
            if if_match != revision():
                return self._error(
                    412,
                    "config_revision_mismatch",
                    "revision mismatch",
                    {"expected": revision(), "actual": if_match},
                )
            template = body.get("template")
            if template not in STATE["templates"]:
                return self._error(
                    404, "not_found", "template missing", {"resource": "template"}
                )
            STATE["apps"][pkg] = template
            return self._send(
                200,
                {
                    "revision": revision(),
                    "previousRevision": None,
                    "dryRun": False,
                    "backupId": None,
                    "warnings": [],
                },
            )
        return self._error(404, "not_found", "no route", {"resource": "route"})

    def do_POST(self):
        if not self._authorized():
            return
        path = self.path.split("?")[0]
        if path.endswith("/restart"):
            pkg = path.split("/")[-2]
            return self._send(
                200,
                {
                    "package": pkg,
                    "forceStopped": True,
                    "launched": False,
                    "durationMs": 5,
                },
            )
        return self._error(404, "not_found", "no route", {"resource": "route"})


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18686
    server = HTTPServer(("127.0.0.1", port), Handler)
    print(f"mock device on http://127.0.0.1:{port} token={TOKEN}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
