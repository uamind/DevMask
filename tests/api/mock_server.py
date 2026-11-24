#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Device Faker /api/v1 契约 mock server（测试用，不是产品实现）。

按 docs/API.md 第一增量实现最小可校验的契约，供 tests/api/smoke.sh 与
tests/api/concurrency.sh 在没有 host server 时自证脚本与断言：

  GET    /api/v1/healthz
  GET    /api/v1/status
  GET    /api/v1/config | /config/revision
  PUT    /api/v1/config                          （If-Match）
  GET    /api/v1/templates                       （items + total + nextCursor）
  GET    /api/v1/templates/{name}
  PUT    /api/v1/templates/{name}                （If-Match）
  PATCH  /api/v1/templates/{name}                （If-Match）
  DELETE /api/v1/templates/{name}?confirm=true   （If-Match）
  POST   /api/v1/templates/{name}/packages       （If-Match）
  DELETE /api/v1/templates/{name}/packages/{pkg} （If-Match）
  GET    /api/v1/apps | /apps/{pkg}
  PUT    /api/v1/apps/{pkg}                      （If-Match）
  DELETE /api/v1/apps/{pkg}?confirm=true         （If-Match）
  GET    /api/v1/packages

并发语义：写操作在进程内同一把锁里完成「读 revision → 比对 If-Match → 写」，
因此同一 If-Match 的并发 PUT 必然一个 200、一个 412。真实 server 用 flock。

用法：
  python3 tests/api/mock_server.py --port 18686 --token df_test_token_0123456789
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import threading
import time
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlsplit

PACKAGE_RE = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*(\.[A-Za-z_$][A-Za-z0-9_$]*)+(@[0-9]{1,3})?$")
NAME_RE = re.compile(r"^[A-Za-z0-9_.\-]+$")
MAX_BODY = 2 * 1024 * 1024
START_TIME = time.time()

TEMPLATE_FIELDS = {
    "packages",
    "manufacturer",
    "brand",
    "marketname",
    "model",
    "name",
    "device",
    "product",
    "hardware",
    "board",
    "soc_model",
    "fingerprint",
    "build_id",
    "display_id",
    "incremental",
    "security_patch",
    "characteristics",
    "android_version",
    "sdk_int",
    "dpi",
    "custom_props",
    "force_denylist_unmount",
    "hide_maps",
    "companion_resetprop",
}
APP_FIELDS = TEMPLATE_FIELDS | {"package"}
APP_FIELDS.discard("packages")


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


APPROX_STARTED_AT = utc_now()


def warning(code: str, message: str, details: dict | None = None) -> dict:
    return {"code": code, "message": message, "details": details or {}}


class State:
    """极简配置仓库；revision = 落盘文件字节的 sha256（含 seq，保证每次写都变化）。"""

    def __init__(self, config_path: str) -> None:
        self.path = config_path
        self.lock = threading.Lock()
        self.seq = 0
        self.flags = {"debug": False, "default_force_denylist_unmount": False, "default_hide_maps": []}
        self.templates: dict[str, dict] = {
            "pixel_9": {
                "packages": ["com.example.app"],
                "manufacturer": "Google",
                "brand": "google",
                "model": "Pixel 9",
                "device": "tokay",
                "android_version": "15",
                "sdk_int": 35,
                "dpi": 420,
            }
        }
        self.apps: dict[str, dict] = {
            "com.example.inline": {"package": "com.example.inline", "model": "SM-S9280", "brand": "samsung"}
        }
        self._flush()

    # ------------------------------------------------------------ 持久化

    def _config_document(self) -> dict:
        return {
            "debug": self.flags["debug"],
            "default_force_denylist_unmount": self.flags["default_force_denylist_unmount"],
            "default_hide_maps": list(self.flags.get("default_hide_maps") or []),
            "templates": self.templates,
            "apps": list(self.apps.values()),
        }

    def _flush(self) -> None:
        data = json.dumps({"seq": self.seq, "config": self._config_document()}, ensure_ascii=False, sort_keys=True)
        with open(self.path, "wb") as fh:
            fh.write(data.encode("utf-8"))
            fh.flush()
            os.fsync(fh.fileno())

    def commit(self) -> None:
        self.seq += 1
        self._flush()

    def revision(self) -> str:
        with open(self.path, "rb") as fh:
            return "sha256:" + hashlib.sha256(fh.read()).hexdigest()

    def size(self) -> int:
        return os.path.getsize(self.path)

    def exists(self) -> bool:
        return os.path.exists(self.path)

    def mtime(self) -> str:
        return datetime.fromtimestamp(os.path.getmtime(self.path), tz=timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

    # ------------------------------------------------------------ 查询

    def position_of(self, name: str) -> int:
        return list(self.templates).index(name)

    def template_detail(self, name: str) -> dict:
        return {
            "revision": self.revision(),
            "position": self.position_of(name),
            "name": name,
            "template": dict(self.templates[name]),
        }

    def fields_set(self, data: dict) -> list[str]:
        return sorted(key for key in data if key not in ("packages", "package"))

    def bound_template(self, package: str) -> str | None:
        for name, template in self.templates.items():
            if package in template.get("packages", []):
                return name
        return None

    def app_detail(self, package: str) -> dict:
        inline = self.apps.get(package)
        bound = self.bound_template(package)
        if inline is not None:
            source = {"type": "inline", "template": bound, "alsoInline": bound is not None}
            merged = dict(self.templates.get(bound, {})) if bound else {}
            merged.update({key: value for key, value in inline.items() if key not in ("package", "packages")})
        elif bound is not None:
            source = {"type": "template", "template": bound, "alsoInline": False}
            merged = dict(self.templates[bound])
        else:
            return {
                "package": package,
                "revision": self.revision(),
                "installed": package in installed_packages(),
                "source": {"type": "none", "template": None, "alsoInline": False},
                "resolved": {},
                "warnings": [warning("no_config", f"{package} 没有任何配置", {"package": package})],
            }
        merged.pop("packages", None)
        return {
            "package": package,
            "revision": self.revision(),
            "installed": package in installed_packages(),
            "source": source,
            "resolved": merged,
            "warnings": [],
        }

    def conflicts(self) -> int:
        seen: dict[str, str] = {}
        conflicts = 0
        for name, template in self.templates.items():
            for package in template.get("packages", []):
                if package in seen and seen[package] != name:
                    conflicts += 1
                else:
                    seen[package] = name
        return conflicts


STATE: State


def installed_packages() -> set[str]:
    return {
        "com.example.app",
        "com.example.inline",
        "com.example.installed",
        "com.android.settings",
        "com.android.systemui",
        "com.example.disabled",
    }


def validate_template(payload: dict, fields: set[str]) -> list[dict]:
    errors: list[dict] = []
    for key in payload:
        if key not in fields:
            errors.append({"path": key, "code": "unknown_field", "message": f"未知字段: {key}"})
    packages = payload.get("packages", [])
    if packages is None:
        packages = []
    if not isinstance(packages, list):
        errors.append({"path": "packages", "code": "invalid_type", "message": "必须是字符串数组"})
    else:
        for package in packages:
            if not isinstance(package, str) or not PACKAGE_RE.match(package):
                errors.append({"path": "packages", "code": "invalid_package", "message": f"非法包名: {package!r}"})
    dpi = payload.get("dpi")
    if dpi is not None and (not isinstance(dpi, int) or isinstance(dpi, bool) or not (120 <= dpi <= 640)):
        errors.append({"path": "dpi", "code": "out_of_range", "message": "dpi 必须为 120-640 的整数"})
    sdk = payload.get("sdk_int")
    if sdk is not None and sdk != "__DELETE__":
        if not isinstance(sdk, int) or isinstance(sdk, bool) or sdk <= 0:
            errors.append({"path": "sdk_int", "code": "invalid_value", "message": "sdk_int 必须为正整数或 __DELETE__"})
    custom = payload.get("custom_props")
    if custom is not None and not isinstance(custom, dict):
        errors.append({"path": "custom_props", "code": "invalid_type", "message": "custom_props 必须是对象"})
    return errors


class Handler(BaseHTTPRequestHandler):
    server_version = "device-faker-mock/0.2"
    protocol_version = "HTTP/1.1"

    # ------------------------------------------------------------ 基础

    def log_message(self, fmt: str, *args) -> None:
        if getattr(self.server, "verbose", False):
            sys.stderr.write("[mock] " + (fmt % args) + "\n")

    def _request_id(self) -> str:
        return "req_" + uuid.uuid4().hex[:12]

    def _send_json(self, status: int, payload: object, request_id: str | None = None) -> None:
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        revision = STATE.revision()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("X-Request-Id", request_id or self._request_id())
        self.send_header("X-DF-Revision", revision)
        self.send_header("ETag", '"' + revision + '"')
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _error(self, status: int, code: str, message: str, details: dict | None = None) -> None:
        self._send_json(
            status,
            {
                "error": {
                    "code": code,
                    "message": message,
                    "details": details,
                    "retryable": False,
                    "requestId": self._request_id(),
                }
            },
        )

    def _read_json(self) -> dict | None:
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            length = 0
        if length > MAX_BODY:
            self._error(413, "body_too_large", "请求体超过上限", {"limitBytes": MAX_BODY})
            return None
        if length <= 0:
            return {}
        raw = self.rfile.read(length)
        content_type = (self.headers.get("Content-Type") or "").lower()
        try:
            if "toml" in content_type or "text/plain" in content_type:
                try:
                    import tomllib
                except ImportError:  # pragma: no cover
                    self._error(422, "validation_failed", "mock 需要 Python >= 3.11 解析 TOML")
                    return None
                return tomllib.loads(raw.decode("utf-8"))
            return json.loads(raw.decode("utf-8") or "{}")
        except Exception as exc:
            self._error(422, "validation_failed", "请求体解析失败", {"errors": [{"path": "", "code": "invalid_json", "message": str(exc)}]})
            return None

    # ------------------------------------------------------------ 鉴权 / 条件写

    def _authorized(self) -> bool:
        token = getattr(self.server, "token", "")
        if not token:
            return True
        if self.headers.get("Authorization", "") == "Bearer " + token:
            return True
        self._error(401, "unauthorized", "缺少或无效 Token", None)
        return False

    def _if_match(self) -> bool:
        """通过返回 False；失败时已发送 428/412 并返回 True。"""
        header = self.headers.get("If-Match")
        if header is None:
            self._error(428, "precondition_required", "写操作必须携带 If-Match", {"header": "If-Match"})
            return True
        value = header.strip()
        if value == "*":
            return False
        if value.startswith("W/"):
            value = value[2:].strip()
        value = value.strip('"')
        current = STATE.revision()
        if value != current:
            self._error(
                412,
                "config_revision_mismatch",
                "配置已被其他客户端修改",
                {"expected": value, "actual": current},
            )
            return True
        return False

    def _write_result(self, previous_revision: str, warnings: list[dict] | None = None) -> dict:
        return {
            "revision": STATE.revision(),
            "previousRevision": previous_revision,
            "dryRun": False,
            "backupId": "mock-" + STATE.revision().split(":")[-1][:12],
            "diffTruncated": False,
            "warnings": warnings or [],
        }

    # ------------------------------------------------------------ 路由

    def _route(self) -> tuple[list[str], dict[str, list[str]]]:
        parts = urlsplit(self.path)
        prefix = getattr(self.server, "prefix", "/api/v1").rstrip("/")
        path = parts.path
        if path.startswith(prefix):
            path = path[len(prefix):]
        segments = [unquote(seg) for seg in path.split("/") if seg]
        return segments, parse_qs(parts.query)

    @staticmethod
    def _confirm(query: dict[str, list[str]]) -> bool:
        return (query.get("confirm") or [""])[0] in ("true", "1")

    def _dispatch(self, method: str) -> None:
        segments, query = self._route()

        if method == "GET" and segments in (["healthz"], []):
            self._send_json(
                200,
                {"status": "ok", "version": "0.0.0-mock", "uptimeSec": int(time.time() - START_TIME)},
            )
            return

        if not self._authorized():
            return

        if method == "GET" and segments == ["status"]:
            self._send_json(200, self._status_doc())
            return

        if method == "GET" and segments == ["config"]:
            self._send_json(200, self._config_response())
            return
        if method == "GET" and segments == ["config", "revision"]:
            self._send_json(
                200,
                {
                    "revision": STATE.revision(),
                    "mtime": STATE.mtime(),
                    "size": STATE.size(),
                    "exists": STATE.exists(),
                },
            )
            return
        if method == "PUT" and segments == ["config"]:
            payload = self._read_json()
            if payload is None:
                return
            if "config" in payload and isinstance(payload["config"], dict):
                payload = payload["config"]
            errors = validate_template(payload, TEMPLATE_FIELDS | {"templates", "apps", "debug", "default_force_denylist_unmount", "default_hide_maps"})
            if errors:
                self._error(422, "validation_failed", "配置校验失败", {"errors": errors})
                return
            with STATE.lock:
                if self._if_match():
                    return
                previous = STATE.revision()
                templates = payload.get("templates") or {}
                if isinstance(templates, dict):
                    STATE.templates = {str(name): dict(value) for name, value in templates.items()}
                apps = payload.get("apps") or []
                STATE.apps = {str(item.get("package")): dict(item) for item in apps if isinstance(item, dict) and item.get("package")}
                STATE.flags["debug"] = bool(payload.get("debug", False))
                STATE.flags["default_force_denylist_unmount"] = bool(payload.get("default_force_denylist_unmount", False))
                STATE.flags["default_hide_maps"] = list(payload.get("default_hide_maps") or [])
                STATE.commit()
            self._send_json(200, self._write_result(previous))
            return

        if method == "GET" and segments == ["templates"]:
            items = []
            for position, name in enumerate(STATE.templates):
                template = STATE.templates[name]
                items.append(
                    {
                        "name": name,
                        "position": position,
                        "packages": list(template.get("packages", [])),
                        "packageCount": len(template.get("packages", [])),
                        "fieldsSet": STATE.fields_set(template),
                    }
                )
            self._send_json(
                200,
                {"revision": STATE.revision(), "total": len(items), "nextCursor": None, "items": items},
            )
            return

        if method == "GET" and len(segments) == 2 and segments[0] == "templates":
            name = segments[1]
            if name not in STATE.templates:
                self._error(404, "not_found", f"模板不存在: {name}", {"resource": "template", "name": name})
                return
            self._send_json(200, STATE.template_detail(name))
            return

        if method == "POST" and len(segments) == 3 and segments[0] == "templates" and segments[2] == "packages":
            payload = self._read_json() or {}
            name = segments[1]
            packages = payload.get("packages") or []
            mode = payload.get("mode") or "append"
            errors = validate_template({"packages": packages}, {"packages", "mode"})
            if errors or mode not in ("append", "replace"):
                if not errors:
                    errors = [{"path": "mode", "code": "invalid_value", "message": "mode 必须是 append 或 replace"}]
                self._error(422, "validation_failed", "包名校验失败", {"errors": errors})
                return
            with STATE.lock:
                if self._if_match():
                    return
                if name not in STATE.templates:
                    self._error(404, "not_found", f"模板不存在: {name}", {"resource": "template", "name": name})
                    return
                previous = STATE.revision()
                if mode == "replace":
                    STATE.templates[name]["packages"] = list(packages)
                else:
                    current = STATE.templates[name].setdefault("packages", [])
                    for package in packages:
                        if package not in current:
                            current.append(package)
                STATE.commit()
            self._send_json(200, self._write_result(previous))
            return

        if method == "DELETE" and len(segments) == 4 and segments[0] == "templates" and segments[2] == "packages":
            name, package = segments[1], segments[3]
            with STATE.lock:
                if self._if_match():
                    return
                if name not in STATE.templates:
                    self._error(404, "not_found", f"模板不存在: {name}", {"resource": "template", "name": name})
                    return
                if package not in STATE.templates[name].get("packages", []):
                    self._error(
                        404,
                        "not_found",
                        f"模板 {name} 未绑定包名: {package}",
                        {"resource": "template_package", "name": package},
                    )
                    return
                previous = STATE.revision()
                STATE.templates[name]["packages"].remove(package)
                STATE.commit()
            self._send_json(200, self._write_result(previous))
            return

        if method in ("PUT", "PATCH") and len(segments) == 2 and segments[0] == "templates":
            payload = self._read_json()
            if payload is None:
                return
            name = segments[1]
            if not NAME_RE.match(name):
                self._error(422, "validation_failed", "模板名非法", {"errors": [{"path": "name", "code": "invalid_template_name", "message": "模板名只允许字母数字与 _ . -"}]})
                return
            errors = validate_template(payload, TEMPLATE_FIELDS)
            if errors:
                self._error(422, "validation_failed", "模板校验失败", {"errors": errors})
                return
            with STATE.lock:
                if self._if_match():
                    return
                previous = STATE.revision()
                existed = name in STATE.templates
                if method == "PUT":
                    STATE.templates[name] = dict(payload)
                    warnings = []
                    if not payload.get("packages"):
                        warnings.append(warning("empty_packages", f"模板 {name} 没有绑定任何包名", {"template": name}))
                else:
                    if not existed:
                        self._error(404, "not_found", f"模板不存在: {name}", {"resource": "template", "name": name})
                        return
                    for key, value in payload.items():
                        if value is None:
                            STATE.templates[name].pop(key, None)
                        else:
                            STATE.templates[name][key] = value
                    warnings = []
                STATE.commit()
            self._send_json(200 if method == "PATCH" or existed else 201, self._write_result(previous, warnings))
            return

        if method == "DELETE" and len(segments) == 2 and segments[0] == "templates":
            name = segments[1]
            if not self._confirm(query):
                self._error(400, "confirmation_required", "删除模板需要 confirm=true", {"action": "delete_template"})
                return
            with STATE.lock:
                if self._if_match():
                    return
                if name not in STATE.templates:
                    self._error(404, "not_found", f"模板不存在: {name}", {"resource": "template", "name": name})
                    return
                previous = STATE.revision()
                del STATE.templates[name]
                STATE.commit()
            self._send_json(200, self._write_result(previous))
            return

        if method == "GET" and segments == ["apps"]:
            items = []
            for position, package in enumerate(STATE.apps):
                inline = STATE.apps[package]
                items.append(
                    {
                        "package": package,
                        "position": position,
                        "source": {"type": "inline", "template": STATE.bound_template(package)},
                        "fieldsSet": STATE.fields_set(inline),
                    }
                )
            self._send_json(
                200,
                {"revision": STATE.revision(), "total": len(items), "nextCursor": None, "items": items},
            )
            return

        if method == "GET" and len(segments) == 2 and segments[0] == "apps":
            package = segments[1]
            if not PACKAGE_RE.match(package):
                self._error(422, "validation_failed", "非法包名", {"errors": [{"path": "package", "code": "invalid_package", "message": package}]})
                return
            self._send_json(200, STATE.app_detail(package))
            return

        if method == "PUT" and len(segments) == 2 and segments[0] == "apps":
            payload = self._read_json()
            if payload is None:
                return
            package = segments[1]
            if not PACKAGE_RE.match(package):
                self._error(422, "validation_failed", "非法包名", {"errors": [{"path": "package", "code": "invalid_package", "message": package}]})
                return
            has_template = "template" in payload
            has_config = "config" in payload
            if has_template == has_config:
                self._error(422, "validation_failed", "template 与 config 必须恰好提供一个", {"errors": [{"path": "template|config", "code": "invalid_value", "message": "二选一"}]})
                return
            if has_template:
                template_name = payload.get("template")
                if not isinstance(template_name, str):
                    self._error(422, "validation_failed", "template 必须是字符串", {"errors": [{"path": "template", "code": "invalid_type", "message": "string"}]})
                    return
                detach = bool(payload.get("detachFromTemplates", True))
                with STATE.lock:
                    if self._if_match():
                        return
                    if template_name not in STATE.templates:
                        self._error(404, "not_found", f"模板不存在: {template_name}", {"resource": "template", "name": template_name})
                        return
                    previous = STATE.revision()
                    warnings = []
                    if detach:
                        for name, template in STATE.templates.items():
                            if name != template_name:
                                packages = template.get("packages", [])
                                if package in packages:
                                    packages.remove(package)
                    packages = STATE.templates[template_name].setdefault("packages", [])
                    if package not in packages:
                        packages.append(package)
                    if package in STATE.apps:
                        del STATE.apps[package]
                        warnings.append(warning("inline_record_removed", f"{package} 的内联记录已删除", {"package": package}))
                    if package not in installed_packages():
                        warnings.append(warning("package_not_installed", f"{package} 尚未安装", {"package": package}))
                    STATE.commit()
                self._send_json(200, self._write_result(previous, warnings))
                return
            config = payload.get("config")
            if not isinstance(config, dict):
                self._error(422, "validation_failed", "config 必须是对象", {"errors": [{"path": "config", "code": "invalid_type", "message": "object"}]})
                return
            errors = validate_template(config, APP_FIELDS | {"package"})
            declared = config.get("package")
            if declared is not None and declared != package:
                errors.append({"path": "config.package", "code": "invalid_value", "message": f"与路径包名不一致: {package}"})
            if errors:
                self._error(422, "validation_failed", "应用配置校验失败", {"errors": errors})
                return
            detach = bool(payload.get("detachFromTemplates", False))
            with STATE.lock:
                if self._if_match():
                    return
                previous = STATE.revision()
                created = package not in STATE.apps
                record = dict(config)
                record["package"] = package
                record.pop("packages", None)
                STATE.apps[package] = record
                if detach:
                    for template in STATE.templates.values():
                        packages = template.get("packages", [])
                        if package in packages:
                            packages.remove(package)
                warnings = []
                if package not in installed_packages():
                    warnings.append(warning("package_not_installed", f"{package} 尚未安装", {"package": package}))
                STATE.commit()
            self._send_json(201 if created else 200, self._write_result(previous, warnings))
            return

        if method == "DELETE" and len(segments) == 2 and segments[0] == "apps":
            package = segments[1]
            if not self._confirm(query):
                self._error(400, "confirmation_required", "删除应用配置需要 confirm=true", {"action": "delete_app"})
                return
            detach = (query.get("detach_templates") or ["false"])[0] in ("true", "1")
            with STATE.lock:
                if self._if_match():
                    return
                previous = STATE.revision()
                removed = False
                if package in STATE.apps:
                    del STATE.apps[package]
                    removed = True
                if detach:
                    for template in STATE.templates.values():
                        packages = template.get("packages", [])
                        if package in packages:
                            packages.remove(package)
                            removed = True
                if not removed:
                    self._error(404, "not_found", f"{package} 没有可删除的配置", {"resource": "app_config", "name": package})
                    return
                STATE.commit()
            self._send_json(200, self._write_result(previous))
            return

        if method == "GET" and segments == ["packages"]:
            items = []
            for package in sorted(installed_packages()):
                items.append(
                    {
                        "package": package,
                        "userIds": [0],
                        "installed": True,
                        "isSystem": package.startswith(("com.android.", "android")),
                        "versionName": "1.4.2" if package == "com.example.app" else "1.0",
                        "versionCode": 42 if package == "com.example.app" else 1,
                        "label": None,
                    }
                )
            self._send_json(
                200,
                {"total": len(items), "nextCursor": None, "source": "pm", "warnings": [], "items": items},
            )
            return

        self._error(404, "not_found", f"未知端点: {method} {self.path}", {"resource": "endpoint", "name": self.path})

    # ------------------------------------------------------------ 文档

    def _config_response(self) -> dict:
        return {
            "revision": STATE.revision(),
            "mtime": STATE.mtime(),
            "size": STATE.size(),
            "exists": STATE.exists(),
            "warnings": [],
            "config": STATE._config_document(),
        }

    def _status_doc(self) -> dict:
        return {
            "module": {"id": "device_faker", "version": "v1.7.0", "versionCode": 20261109},
            "device": {
                "manufacturer": "Google",
                "brand": "google",
                "model": "Mock Phone",
                "device": "mock",
                "android": "17",
                "sdk": 37,
                "abi": "arm64-v8a",
            },
            "server": {
                "version": "0.0.0-mock",
                "bind": "127.0.0.1:%d" % self.server.server_address[1],
                "uptimeSec": int(time.time() - START_TIME),
                "startedAt": APPROX_STARTED_AT,
            },
            "config": {
                "revision": STATE.revision(),
                "mtime": STATE.mtime(),
                "size": STATE.size(),
                "exists": STATE.exists(),
                "templates": len(STATE.templates),
                "apps": len(STATE.apps),
                "conflicts": STATE.conflicts(),
            },
            "capabilities": ["config", "templates", "apps", "packages", "device", "validate"],
        }

    def do_GET(self) -> None:
        self._dispatch("GET")

    def do_PUT(self) -> None:
        self._dispatch("PUT")

    def do_POST(self) -> None:
        self._dispatch("POST")

    def do_PATCH(self) -> None:
        self._dispatch("PATCH")

    def do_DELETE(self) -> None:
        self._dispatch("DELETE")


class Server(ThreadingHTTPServer):
    daemon_threads = True
    token = ""
    prefix = "/api/v1"
    verbose = False


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Device Faker API contract mock")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=18686)
    parser.add_argument("--token", default="")
    parser.add_argument("--config", default="")
    parser.add_argument("--base", default="http://127.0.0.1:18686/api/v1")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args(argv)

    prefix = urlsplit(args.base).path.rstrip("/") or "/api/v1"
    config_path = args.config or os.path.join(os.getcwd(), "mock-config.json")
    os.makedirs(os.path.dirname(os.path.abspath(config_path)) or ".", exist_ok=True)

    global STATE
    STATE = State(config_path)

    server = Server((args.host, args.port), Handler)
    server.token = args.token
    server.prefix = prefix
    server.verbose = args.verbose

    sys.stderr.write(
        "mock listening on http://%s:%d%s (token=%s)\n"
        % (args.host, args.port, prefix, "set" if args.token else "none")
    )
    sys.stderr.flush()
    try:
        server.serve_forever(poll_interval=0.1)
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
