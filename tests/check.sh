#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# tests/ 自检：bash 语法、fixture 可解析、PATH shim 可用、mock 端到端。
#
# 用法：
#   bash tests/check.sh            # 全量（含 mock e2e）
#   bash tests/check.sh --no-e2e   # 只做静态检查

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TESTS_ROOT=$SCRIPT_DIR
REPO_ROOT="$(cd "$TESTS_ROOT/.." && pwd)"

WITH_E2E=1
for arg in "$@"; do
  case "$arg" in
    --no-e2e) WITH_E2E=0 ;;
    --with-e2e) WITH_E2E=1 ;;
    -h | --help)
      sed -n '2,9p' "$0"
      exit 0
      ;;
    *)
      printf '未知参数：%s\n' "$arg" >&2
      exit 2
      ;;
  esac
done

CHECKS=0
FAIL=0

pass() {
  CHECKS=$((CHECKS + 1))
  printf '  ok   %s\n' "$*"
}

fail() {
  CHECKS=$((CHECKS + 1))
  FAIL=$((FAIL + 1))
  printf '  FAIL %s\n' "$*"
}

section() {
  printf '\n== %s ==\n' "$*"
}

# ---------------------------------------------------------------- bash -n

section "bash -n"
while IFS= read -r script; do
  if bash -n "$script" 2>/dev/null; then
    pass "bash -n ${script#$REPO_ROOT/}"
  else
    fail "bash -n $script"
  fi
done < <(find "$TESTS_ROOT" -type f -name '*.sh' | sort)

# ---------------------------------------------------------------- python 语法

section "python 语法"
if python3 -m py_compile "$TESTS_ROOT/api/mock_server.py" 2>/dev/null; then
  pass "py_compile tests/api/mock_server.py"
else
  fail "py_compile tests/api/mock_server.py"
fi
rm -rf "$TESTS_ROOT/api/__pycache__"

# ---------------------------------------------------------------- fixture 结构

section "fixture 结构"
if python3 - "$TESTS_ROOT/fixtures" <<'PY'
import json
import os
import sys

root = sys.argv[1]
errors = []


def check(condition, message):
    if not condition:
        errors.append(message)


def path(*parts):
    return os.path.join(root, *parts)


def read(*parts):
    with open(path(*parts), encoding="utf-8") as fh:
        return fh.read()


try:
    import tomllib
except ImportError:  # pragma: no cover - Python < 3.11
    tomllib = None

if tomllib is not None:
    parsed = {}
    for name in ("minimal.toml", "commented.toml", "conflict.toml", "invalid.toml"):
        try:
            parsed[name] = tomllib.loads(read("config", name))
        except Exception as exc:
            errors.append(f"config/{name} TOML 解析失败: {exc}")

    minimal = parsed.get("minimal.toml", {})
    check(minimal.get("debug") is False, "minimal.toml debug 应为 false")
    check(minimal.get("default_force_denylist_unmount") is False, "minimal.toml 全局开关缺失")
    template = minimal.get("templates", {}).get("pixel_9", {})
    check(template.get("model") == "Pixel 9", "minimal.toml templates.pixel_9.model 应为 Pixel 9")
    check(template.get("sdk_int") == 35, "minimal.toml sdk_int 应为 35")
    check(template.get("dpi") == 420, "minimal.toml dpi 应为 420")
    check(template.get("packages") == ["com.example.app"], "minimal.toml packages 不符")
    check(len(minimal.get("apps", [])) == 1, "minimal.toml 应有 1 个 [[apps]]")
    check(minimal.get("apps", [{}])[0].get("package") == "com.example.inline", "minimal.toml apps[0].package 不符")

    commented = parsed.get("commented.toml", {})
    samsung = commented.get("templates", {}).get("samsung_s24_ultra", {})
    check(samsung.get("future_field") == "keep-me", "commented.toml 未知字段 future_field 丢失")
    check("experimental" in commented, "commented.toml 未知表 experimental 丢失")
    check(samsung.get("custom_props", {}).get("ro.product.vendor.model") == "SM-S9280", "commented.toml custom_props 不符")

    conflict = parsed.get("conflict.toml", {}).get("templates", {})
    check(
        "com.example.conflict" in conflict.get("alpha", {}).get("packages", [])
        and "com.example.conflict" in conflict.get("beta", {}).get("packages", []),
        "conflict.toml 未构造出重复包名",
    )

    invalid = parsed.get("invalid.toml", {}).get("templates", {}).get("broken", {})
    check(invalid.get("dpi") == 9999, "invalid.toml dpi 应为 9999")

    try:
        server = tomllib.loads(read("server", "server.default.toml"))
    except Exception as exc:
        errors.append(f"server/server.default.toml TOML 解析失败: {exc}")
        server = {}
    check(server.get("bind") == "127.0.0.1:8686", "server.default.toml bind 不符")
    tokens = server.get("tokens", [])
    check(len(tokens) == 2, "server.default.toml 应有 2 个 [[tokens]]")
    if tokens:
        check(str(tokens[0].get("hash", "")).startswith("sha256:"), "owner token hash 前缀不符")
        check("admin" in tokens[0].get("scopes", []), "owner token 缺少 admin scope")
        check(tokens[1].get("scopes") == ["read"], "readonly token scopes 应为 [read]")
else:
    print("  (skip) Python 无 tomllib，跳过 TOML 解析断言")

module_prop = read("module", "module.prop")
check("id=device_faker" in module_prop, "module.prop 缺少 id=device_faker")
check("version=v1.11.0" in module_prop, "module.prop 缺少 version=v1.11.0")
check("versionCode=20261202" in module_prop, "module.prop 缺少 versionCode")

pm_lines = read("android", "pm-list-packages.txt").strip().splitlines()
check(len(pm_lines) >= 5, "pm-list-packages.txt 行数过少")
check(all(line.startswith("package:") for line in pm_lines), "pm-list-packages.txt 存在非 package: 行")
check(
    "package:com.example.app uid:10123" in read("android", "pm-list-packages-u.txt"),
    "pm-list-packages-u.txt 缺少 com.example.app uid",
)
dumpsys = read("android", "dumpsys-package-com.example.app.txt")
check("Package [com.example.app]" in dumpsys, "dumpsys fixture 缺少 Package 段")
check("versionName=1.4.2" in dumpsys, "dumpsys fixture 缺少 versionName")
check("installed=true" in dumpsys, "dumpsys fixture 缺少 installed=true")
check("[ro.product.model]: [Pixel 9]" in read("android", "getprop.txt"), "getprop fixture 缺少 ro.product.model")

template_json = json.loads(read("api", "template.json"))
check(template_json.get("packages") == ["com.example.app"], "api/template.json packages 不符")
check(template_json.get("dpi") == 420, "api/template.json dpi 不符")
check(json.loads(read("api", "app.json")).get("template") == "pixel_9", "api/app.json template 不符")

for shim in ("pm", "dumpsys", "getprop"):
    executable = os.access(path("bin", shim), os.X_OK)
    check(executable, f"bin/{shim} 不可执行")

if errors:
    for message in errors:
        print("  - " + message)
    sys.exit(1)
PY
then
  pass "fixture 结构"
else
  fail "fixture 结构（见上方明细）"
fi

# ---------------------------------------------------------------- PATH shim

section "PATH shim"
SHIM_BIN="$TESTS_ROOT/fixtures/bin"
PM_COUNT=$(PATH="$SHIM_BIN:$PATH" pm list packages -U 2>/dev/null | grep -c '^package:' || true)
if [ "${PM_COUNT:-0}" -ge 5 ]; then
  pass "pm list packages -U（$PM_COUNT 行）"
else
  fail "pm shim 输出异常（$PM_COUNT 行）"
fi
if PATH="$SHIM_BIN:$PATH" dumpsys package com.example.app 2>/dev/null | grep -q 'versionName=1.4.2'; then
  pass "dumpsys package com.example.app"
else
  fail "dumpsys shim 输出异常"
fi
if [ "$(PATH="$SHIM_BIN:$PATH" getprop ro.product.model 2>/dev/null)" = "Pixel 9" ]; then
  pass "getprop ro.product.model"
else
  fail "getprop shim 输出异常"
fi

# ---------------------------------------------------------------- mock e2e

if [ "$WITH_E2E" = "1" ]; then
  section "mock e2e（tests/api + mock_server.py）"
  E2E_PORT=$(python3 - <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)
  if ( unset BASE TOKEN; MOCK=1 MOCK_PORT="$E2E_PORT" bash "$TESTS_ROOT/api/smoke.sh" ); then
    pass "api/smoke.sh"
  else
    fail "api/smoke.sh"
  fi
  if ( unset BASE TOKEN; MOCK=1 MOCK_PORT="$E2E_PORT" ROUNDS=1 bash "$TESTS_ROOT/api/concurrency.sh" ); then
    pass "api/concurrency.sh"
  else
    fail "api/concurrency.sh"
  fi
fi

# ---------------------------------------------------------------- 汇总

printf '\n== 汇总 ==\n%d 项检查，%d 项失败\n' "$CHECKS" "$FAIL"
if [ "$FAIL" -ne 0 ]; then
  exit 1
fi
printf 'CHECK PASS\n'
