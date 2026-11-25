#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
# Device Faker: https://github.com/Seyud/device_faker
"""Device Faker 设备端最小 Mock：供 Manager / Relay 冒烟验收。

用法：
    python3 manager/docs/mock_device.py 18686

实现 Manager 需要的 /api/v1 子集，仅用于本地联调，不写入任何真实设备：

- status / overview / config/revision / templates / apps：原有能力不变，并
  默认在首次 PUT /apps/{pkg} 返回 412，用于验证 Relay 的 revision 重取与重试；
- egress（v1.10.0 契约，见 docs/plans/egress-contract.md §1）：
    GET    /api/v1/egress            当前期望态 + 读回 + 探测结果（含 egress revision）
    PUT    /api/v1/egress            省略=不改 / null=清空 / 数组=整体替换；带 If-Match
    DELETE /api/v1/egress?confirm=true  恢复直连（清 default / proxies / apps 覆盖）
    POST   /api/v1/egress/probe      经“当前链路”探测（固定返回文档网段出口 IP）
    GET    /api/v1/egress/proxies    脱敏代理列表（usedAsDefault / 引用它的包名）

egress 相关的行为约定（与真实服务对齐，便于联调）：

- egress revision = sha256(configRevisionHex ‖ "\n" ‖ egress 文档字节)，
  egress 端点只回 X-DF-Egress-Revision，**不回** X-DF-Revision / ETag；
- 缺 If-Match → 428 precondition_required；不匹配 → 412 config_revision_mismatch
  （details 里带 resource="egress"）；
- 密码/用户名永不出现在任何响应里，唯一信号是 hasCredentials；
- 未配置任何出口时探测返回 409 egress.not_configured。

**不带 egress 的用法与改动前完全一致**：不调 /egress 时其余端点行为不变。
"""

import copy
import hashlib
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

TOKEN = "admin123"
FORCE_412_ONCE = True
#: 探测返回的“出口 IP”（RFC 5737 文档网段，永不指向真实地址）。
EXIT_IP = "198.51.100.7"
EGRESS_NAME_PATTERN = r"^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$"
DIRECT = "__DIRECT__"

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

#: egress 状态（对应 config.toml 的 [egress]/[[apps]].egress 与 egress.toml 的 [proxies]/[probe]）。
EGRESS = {
    "default": None,
    "proxies": [],
    "apps": [],
    "failOpen": True,
    "probeUrl": "http://checkip.amazonaws.com/",
    "probeTimeoutSeconds": 5,
    "exclusionList": [],
    "lastProbe": None,
}


def revision():
    blob = json.dumps(STATE, sort_keys=True, ensure_ascii=False).encode()
    return hashlib.sha256(blob).hexdigest()


def now():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def overview():
    rev = revision()
    return {
        "revision": rev,
        "module": {"id": "device_faker", "version": "v1.11.0", "versionCode": 20261202},
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
            "version": "1.11.0",
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


def egress_document():
    """egress 的“文件内容”摘要：契约里 revision 覆盖两个文件，这里用规范化 JSON 代替 egress.toml 字节。"""
    return json.dumps(
        {
            "default": EGRESS["default"],
            "proxies": EGRESS["proxies"],
            "apps": EGRESS["apps"],
            "failOpen": EGRESS["failOpen"],
            "probeUrl": EGRESS["probeUrl"],
            "probeTimeoutSeconds": EGRESS["probeTimeoutSeconds"],
            "exclusionList": EGRESS["exclusionList"],
        },
        sort_keys=True,
        ensure_ascii=False,
    ).encode()


def egress_revision():
    """sha256(configRevision 的小写十六进制 ‖ "\n" ‖ egress 字节)，见契约 §1.2。"""
    return hashlib.sha256(revision().encode() + b"\n" + egress_document()).hexdigest()


def has_credentials(item):
    return bool(item.get("username") or item.get("password"))


def redacted_proxies():
    return [
        {
            "name": item.get("name"),
            "type": item.get("type"),
            "host": item.get("host"),
            "port": item.get("port"),
            "hasCredentials": has_credentials(item),
        }
        for item in EGRESS["proxies"]
    ]


def find_proxy(name):
    for item in EGRESS["proxies"]:
        if item.get("name") == name:
            return item
    return None


def upstream():
    if not EGRESS["default"]:
        return None
    item = find_proxy(EGRESS["default"])
    if item is None:
        return None
    return {
        "name": item.get("name"),
        "type": item.get("type"),
        "host": item.get("host"),
        "port": item.get("port"),
        "hasCredentials": has_credentials(item),
    }


def readback():
    up = upstream()
    if up:
        value = "%s:%s" % (up["host"], up["port"])
        dumpsys = "Global proxy: [%s] port: %s" % (up["host"], up["port"])
    else:
        value = ":0"
        dumpsys = "Global proxy: none"
    return {
        "settingsValue": value,
        "dumpsysGlobalProxy": dumpsys,
        "matches": True,
        "at": now(),
    }


def egress_warnings():
    out = []
    default = EGRESS["default"]
    if default:
        item = find_proxy(default)
        if item is None:
            out.append(
                {
                    "code": "egress_proxy_not_found",
                    "message": "设备级默认代理 %s 未在 egress.toml 定义" % default,
                    "details": {"name": default, "scope": "device"},
                }
            )
        elif has_credentials(item):
            out.append(
                {
                    "code": "egress_credentials_ignored",
                    "message": "方案 A 不承载代理认证：%s 只按 host:port 生效" % default,
                    "details": {"name": default},
                }
            )
    for app in EGRESS["apps"]:
        value = app.get("egress")
        if value and value != DIRECT and find_proxy(value) is None:
            out.append(
                {
                    "code": "egress_proxy_not_found",
                    "message": "%s 引用的代理 %s 未定义" % (app.get("package"), value),
                    "details": {
                        "name": value,
                        "scope": "app",
                        "package": app.get("package"),
                    },
                }
            )
    return out


def egress_file_meta():
    exists = bool(EGRESS["default"] or EGRESS["proxies"] or EGRESS["apps"])
    return {
        "exists": exists,
        "size": len(egress_document()) if exists else 0,
        "mtime": now() if exists else None,
    }


def egress_state():
    """GET/PUT/DELETE 共用的状态块（契约 §6.8）。"""
    return {
        "configured": bool(EGRESS["default"] or EGRESS["proxies"] or EGRESS["apps"]),
        "mode": "proxy" if EGRESS["default"] else "direct",
        "effectiveMode": "proxy" if EGRESS["default"] else "direct",
        "default": EGRESS["default"],
        "failOpen": EGRESS["failOpen"],
        "probeUrl": EGRESS["probeUrl"],
        "probeTimeoutSeconds": EGRESS["probeTimeoutSeconds"],
        "exclusionList": EGRESS["exclusionList"],
        "upstream": upstream(),
        "proxies": redacted_proxies(),
        "apps": [
            {
                "package": app.get("package"),
                "egress": app.get("egress"),
                "effective": "direct" if app.get("egress") == DIRECT else "proxy",
                "proxy": None if app.get("egress") == DIRECT else app.get("egress"),
            }
            for app in EGRESS["apps"]
        ],
        "applied": True,
        "readback": readback(),
        "health": {
            "degraded": False,
            "degradedSince": None,
            "degradedReason": None,
            "lastProbe": EGRESS["lastProbe"],
        },
        "egressFile": egress_file_meta(),
        "warnings": egress_warnings(),
    }


def snapshot():
    doc = {"revision": egress_revision(), "configRevision": revision()}
    doc.update(egress_state())
    return doc


def proxies_payload():
    items = []
    for item in redacted_proxies():
        row = dict(item)
        row["usedAsDefault"] = item["name"] == EGRESS["default"]
        row["apps"] = [
            app.get("package")
            for app in EGRESS["apps"]
            if app.get("egress") == item["name"]
        ]
        items.append(row)
    return {
        "revision": egress_revision(),
        "configRevision": revision(),
        "total": len(items),
        "egressFile": egress_file_meta(),
        "items": items,
        "warnings": egress_warnings(),
    }


def _issue(path, code, message):
    return {"path": path, "code": code, "message": message}


def _name_issue(path, value, allow_direct=False):
    if not isinstance(value, str) or not value:
        return _issue(path, "invalid_egress_name", "代理名不能为空")
    if value == DIRECT and not allow_direct:
        return _issue(path, "invalid_egress_name", "__DIRECT__ 是保留值，不能当代理名")
    if len(value) > 64 or not _matches_name(value):
        return _issue(
            path,
            "invalid_egress_name",
            "代理名必须匹配 %s（1-64 字符）" % EGRESS_NAME_PATTERN,
        )
    return None


def _matches_name(value):
    import re

    return re.match(EGRESS_NAME_PATTERN, value) is not None


def validate_egress_update(body):
    """只做“能让联调跑起来”的校验，错误形状与真实服务一致（422 details.errors[]）。"""
    issues = []
    allowed = {
        "default",
        "failOpen",
        "probeUrl",
        "probeTimeoutSeconds",
        "exclusionList",
        "proxies",
        "apps",
    }
    for key in body:
        if key not in allowed:
            issues.append(_issue(key, "unknown_field", "未知字段 %s" % key))

    if "default" in body and body["default"] is not None:
        issue = _name_issue("default", body["default"])
        if issue:
            issues.append(issue)

    if "failOpen" in body and not isinstance(body["failOpen"], bool):
        issues.append(_issue("failOpen", "invalid_type", "failOpen 必须是布尔值"))

    if "probeUrl" in body and body["probeUrl"] is not None:
        url = body["probeUrl"]
        if not isinstance(url, str) or not url.startswith("http://") or len(url) > 512:
            issues.append(
                _issue("probeUrl", "invalid_probe_url", "探测 URL 必须以 http:// 开头且 ≤512 字符")
            )

    if "probeTimeoutSeconds" in body and body["probeTimeoutSeconds"] is not None:
        value = body["probeTimeoutSeconds"]
        if not isinstance(value, int) or not 1 <= value <= 30:
            issues.append(
                _issue("probeTimeoutSeconds", "out_of_range", "probeTimeoutSeconds 必须在 1..=30")
            )

    if "exclusionList" in body and body["exclusionList"] is not None:
        value = body["exclusionList"]
        if not isinstance(value, list) or len(value) > 32 or any(
            not isinstance(item, str) or len(item) > 64 for item in value
        ):
            issues.append(
                _issue("exclusionList", "invalid_value", "exclusionList 最多 32 项、每项 ≤64 字符")
            )

    proxies = body.get("proxies")
    if proxies is not None:
        if not isinstance(proxies, list):
            issues.append(_issue("proxies", "invalid_type", "proxies 必须是数组"))
        elif len(proxies) > 64:
            issues.append(_issue("proxies", "out_of_range", "代理定义最多 64 条"))
        else:
            seen = set()
            for index, item in enumerate(proxies):
                path = "proxies[%d]" % index
                if not isinstance(item, dict):
                    issues.append(_issue(path, "invalid_type", "每项必须是对象"))
                    continue
                issue = _name_issue(path + ".name", item.get("name"))
                if issue:
                    issues.append(issue)
                elif item.get("name") in seen:
                    issues.append(_issue(path + ".name", "invalid_value", "同一请求内代理名重复"))
                else:
                    seen.add(item.get("name"))
                if item.get("type") not in ("http", "socks5"):
                    issues.append(
                        _issue(path + ".type", "invalid_egress_proxy", "type 必须是 http 或 socks5")
                    )
                host = item.get("host")
                if (
                    not isinstance(host, str)
                    or not host
                    or len(host) > 253
                    or "://" in host
                    or "/" in host
                    or " " in host
                ):
                    issues.append(
                        _issue(path + ".host", "invalid_egress_proxy", "host 不合法（不含 scheme/路径）")
                    )
                port = item.get("port")
                if not isinstance(port, int) or not 1 <= port <= 65535:
                    issues.append(
                        _issue(path + ".port", "invalid_egress_proxy", "port 必须在 1..=65535")
                    )
                for field, limit in (("username", 128), ("password", 256)):
                    value = item.get(field)
                    if value is not None and (not isinstance(value, str) or len(value) > limit):
                        issues.append(
                            _issue(
                                path + "." + field,
                                "invalid_egress_proxy",
                                "%s 最多 %d 字符" % (field, limit),
                            )
                        )

    apps = body.get("apps")
    if apps is not None:
        if not isinstance(apps, list):
            issues.append(_issue("apps", "invalid_type", "apps 必须是数组"))
        else:
            seen = set()
            for index, item in enumerate(apps):
                path = "apps[%d]" % index
                if not isinstance(item, dict):
                    issues.append(_issue(path, "invalid_type", "每项必须是对象"))
                    continue
                package = item.get("package")
                if not isinstance(package, str) or not package:
                    issues.append(_issue(path + ".package", "missing_required", "缺少 package"))
                elif package in seen:
                    issues.append(_issue(path + ".package", "invalid_value", "同一请求内 package 重复"))
                else:
                    seen.add(package)
                if "egress" not in item:
                    issues.append(_issue(path + ".egress", "missing_required", "缺少 egress"))
                elif item["egress"] is not None:
                    issue = _name_issue(
                        path + ".egress", item["egress"], allow_direct=True
                    )
                    if issue:
                        issues.append(issue)

    return issues


def apply_egress_update(target, body):
    # 与契约一致：省略 = 不改；null = 清空；数组 = 整体替换。
    if "default" in body:
        target["default"] = body["default"]
    if "failOpen" in body and body["failOpen"] is not None:
        target["failOpen"] = bool(body["failOpen"])
    if "probeUrl" in body:
        target["probeUrl"] = body["probeUrl"] or "http://checkip.amazonaws.com/"
    if "probeTimeoutSeconds" in body and body["probeTimeoutSeconds"] is not None:
        target["probeTimeoutSeconds"] = int(body["probeTimeoutSeconds"])
    if "exclusionList" in body:
        target["exclusionList"] = body["exclusionList"] or []
    if "proxies" in body and body["proxies"] is not None:
        target["proxies"] = body["proxies"]
    if "apps" in body:
        target["apps"] = body["apps"] or []


def probe_payload(body):
    package = body.get("package")
    url = body.get("url") or EGRESS["probeUrl"]
    scope = "device"
    proxy_name = EGRESS["default"]
    if package:
        scope = "app"
        proxy_name = None
        for app in EGRESS["apps"]:
            if app.get("package") == package:
                proxy_name = app.get("egress")
                break
        if proxy_name == DIRECT:
            proxy_name = None
    configured = bool(EGRESS["default"] or EGRESS["proxies"])
    if package and not proxy_name:
        configured = False
    if not configured:
        return None, {
            "scope": scope,
            "package": package,
        }
    result = {
        "ok": True,
        "exitIp": EXIT_IP,
        "latencyMs": 12,
        "checkedAt": now(),
        "scope": scope,
        "via": "proxy" if proxy_name else "direct",
        "proxy": proxy_name,
        "url": url,
        "upstreamStatus": 200,
        "approximate": scope == "app",
        "warnings": [],
    }
    EGRESS["lastProbe"] = {
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
    return result, None


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, status, payload, egress=False):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("X-Request-Id", "req_mock")
        if egress:
            # 契约 §1.1：egress 端点只回 egress revision，绝不能回 X-DF-Revision / ETag，
            # 否则会污染客户端缓存的 config revision。
            self.send_header("X-DF-Egress-Revision", egress_revision())
        else:
            self.send_header("X-DF-Revision", revision())
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _error(self, status, code, message, details=None, egress=False):
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
            egress=egress,
        )

    def _authorized(self, egress=False):
        if self.headers.get("Authorization") != "Bearer " + TOKEN:
            self._error(401, "unauthorized", "bad token", egress=egress)
            return False
        return True

    def _body(self):
        length = int(self.headers.get("Content-Length", "0"))
        if length <= 0:
            return {}
        try:
            return json.loads(self.rfile.read(length) or b"{}")
        except ValueError:
            return {}

    def _path(self):
        return self.path.split("?")[0]

    def _query(self):
        _, _, tail = self.path.partition("?")
        out = {}
        for pair in tail.split("&"):
            if not pair:
                continue
            key, _, value = pair.partition("=")
            out[key] = value
        return out

    def _require_egress_precondition(self):
        """返回 True 表示已回错误、调用方应直接 return。"""
        header = self.headers.get("If-Match")
        if header is None or not header.strip():
            self._error(
                428,
                "precondition_required",
                "写 egress 必须带 If-Match",
                {"header": "If-Match"},
                egress=True,
            )
            return True
        expected = header.strip().strip('"')
        if expected.startswith("sha256:"):
            expected = expected[len("sha256:") :]
        current = egress_revision()
        if expected in ("*", current):
            return False
        self._error(
            412,
            "config_revision_mismatch",
            "egress 已被其他客户端修改",
            {"expected": current, "actual": expected, "resource": "egress"},
            egress=True,
        )
        return True

    # ── 原有端点（行为不变）─────────────────────────────────────────────

    def do_GET(self):
        if not self._authorized():
            return
        path = self._path()
        if path == "/api/v1/status":
            ov = overview()
            return self._send(
                200,
                {
                    "module": ov["module"],
                    "device": ov["device"],
                    "server": ov["server"],
                    "config": ov["config"],
                    "capabilities": [
                        "config",
                        "templates",
                        "apps",
                        "overview",
                        "egress",
                    ],
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
        if path == "/api/v1/egress":
            return self._send(200, snapshot(), egress=True)
        if path == "/api/v1/egress/proxies":
            return self._send(200, proxies_payload(), egress=True)
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
        path = self._path()
        if path == "/api/v1/egress":
            return self._put_egress()
        if not self._authorized():
            return
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
        path = self._path()
        if path == "/api/v1/egress/probe":
            return self._post_egress_probe()
        if not self._authorized():
            return
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

    def do_DELETE(self):
        path = self._path()
        if path != "/api/v1/egress":
            if not self._authorized():
                return
            return self._error(404, "not_found", "no route", {"resource": "route"})
        return self._delete_egress()

    # ── egress 端点 ─────────────────────────────────────────────────────

    def _put_egress(self):
        if not self._authorized(egress=True):
            return
        if self._require_egress_precondition():
            return
        body = self._body()
        if not isinstance(body, dict):
            return self._error(
                422,
                "validation_failed",
                "请求体必须是 JSON 对象",
                {"errors": [_issue("$", "invalid_type", "body 必须是对象")]},
                egress=True,
            )
        issues = validate_egress_update(body)
        if issues:
            return self._error(
                422, "validation_failed", "egress 请求不合法", {"errors": issues}, egress=True
            )

        dry_run = self._query().get("dry_run") in ("true", "1")
        previous_revision = egress_revision()
        if dry_run:
            candidate = copy.deepcopy(EGRESS)
            apply_egress_update(candidate, body)
            changed = candidate != EGRESS
            doc = snapshot()
            doc.update(
                {
                    "previousRevision": previous_revision,
                    "dryRun": True,
                    "changed": changed,
                    "backupId": None,
                }
            )
            return self._send(200, doc, egress=True)

        apply_egress_update(EGRESS, body)
        doc = snapshot()
        doc.update(
            {
                "previousRevision": previous_revision,
                "dryRun": False,
                "changed": egress_revision() != previous_revision,
                "backupId": "mock-egress.toml",
            }
        )
        return self._send(200, doc, egress=True)

    def _delete_egress(self):
        if not self._authorized(egress=True):
            return
        if self._query().get("confirm") not in ("true", "1"):
            return self._error(
                400,
                "confirmation_required",
                "恢复直连必须带 confirm=true",
                {"action": "egress_clear"},
                egress=True,
            )
        if self._require_egress_precondition():
            return
        previous_revision = egress_revision()
        had = bool(
            EGRESS["default"] or EGRESS["proxies"] or EGRESS["apps"] or EGRESS["exclusionList"]
        )
        EGRESS["default"] = None
        EGRESS["proxies"] = []
        EGRESS["apps"] = []
        EGRESS["exclusionList"] = []
        EGRESS["lastProbe"] = None
        doc = snapshot()
        doc.update(
            {
                "previousRevision": previous_revision,
                "dryRun": False,
                "changed": had,
                "backupId": "mock-egress.toml" if had else None,
            }
        )
        return self._send(200, doc, egress=True)

    def _post_egress_probe(self):
        if not self._authorized(egress=True):
            return
        body = self._body()
        if not isinstance(body, dict):
            body = {}
        url = body.get("url")
        if url is not None and (not isinstance(url, str) or not url.startswith("http://")):
            return self._error(
                422,
                "validation_failed",
                "探测 URL 必须以 http:// 开头",
                {"errors": [_issue("url", "invalid_probe_url", "只接受 http:// 探测地址")]},
                egress=True,
            )
        result, missing = probe_payload(body)
        if result is None:
            details = {"scope": missing["scope"]}
            if missing.get("package"):
                details["package"] = missing["package"]
            return self._error(
                409,
                "egress.not_configured",
                "没有可用的出口代理",
                details,
                egress=True,
            )
        return self._send(200, result, egress=True)


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18686
    server = HTTPServer(("127.0.0.1", port), Handler)
    print(
        "mock device on http://127.0.0.1:%d token=%s (egress: GET/PUT/DELETE /api/v1/egress, "
        "POST /api/v1/egress/probe, GET /api/v1/egress/proxies)" % (port, TOKEN),
        flush=True,
    )
    server.serve_forever()


if __name__ == "__main__":
    main()
