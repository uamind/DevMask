#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Device Faker API 测试公共库（tests/api/*.sh 共用）。
#
# 兼容 bash 3.2（macOS 自带）与 bash 5.x；不依赖 jq，JSON 解析统一走 python3。
# 通过 source 使用：
#   source "$(dirname "$0")/common.sh"
#   df_init
#   df_ensure_server
#   df_api GET /status "${DF_AUTH[@]}"
#   df_assert_status "200" "status"
#   DF_REV=$(df_capture_revision "status")
#
# 由 df_api 写入的全局变量：
#   DF_STATUS       最近一次 HTTP 状态码（字符串）
#   DF_BODY         最近一次响应体文件
#   DF_HEADERS      最近一次响应头文件
#   DF_LAST_METHOD  最近一次请求方法
#   DF_LAST_PATH    最近一次请求路径

set -euo pipefail

COMMON_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TESTS_ROOT="$(cd "$COMMON_DIR/.." && pwd)"
REPO_ROOT="$(cd "$TESTS_ROOT/.." && pwd)"

: "${BASE:=http://127.0.0.1:8686/api/v1}"
: "${TOKEN:=}"
: "${API_TIMEOUT:=20}"
: "${MOCK:=0}"
: "${MOCK_PORT:=18686}"
: "${ASSERT_HEADERS:=1}"
: "${VERBOSE:=0}"
: "${KEEP_TMP:=0}"

DF_FIXTURE_TOKEN="df_test_token_0123456789"
DF_STATUS=""
DF_BODY=""
DF_HEADERS=""
DF_LAST_METHOD=""
DF_LAST_PATH=""
DF_SERVER_PID=""
DF_TMP=""
DF_AUTH=()

# ---------------------------------------------------------------- 输出

df_log() {
  printf '[api-test] %s\n' "$*" >&2
}

df_note() {
  printf '[api-test]   %s\n' "$*" >&2
}

df_warn() {
  printf '[api-test][warn] %s\n' "$*" >&2
}

df_json_pretty() {
  # $1 = 文件；解析失败时原样输出（截断到 4000 字符）
  python3 - "$1" <<'PY' 2>/dev/null || head -c 4000 "$1"
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        data = json.load(fh)
except Exception:
    sys.exit(2)
text = json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True)
print(text[:4000])
PY
}

df_fail() {
  printf '\n[api-test][FAIL] %s\n' "$*" >&2
  if [ -n "${DF_LAST_PATH:-}" ]; then
    printf '  request : %s %s%s\n' "$DF_LAST_METHOD" "$BASE" "$DF_LAST_PATH" >&2
    printf '  status  : %s\n' "${DF_STATUS:-<未发出请求>}" >&2
  fi
  if [ -n "${DF_HEADERS:-}" ] && [ -f "$DF_HEADERS" ]; then
    printf '  headers :\n' >&2
    sed 's/^/    /' "$DF_HEADERS" 2>/dev/null | head -n 20 >&2 || true
  fi
  if [ -n "${DF_BODY:-}" ] && [ -f "$DF_BODY" ]; then
    printf '  body    :\n' >&2
    df_json_pretty "$DF_BODY" | sed 's/^/    /' >&2 || true
    printf '\n' >&2
  fi
  if [ -n "${DF_SERVER_PID:-}" ] && [ -f "${DF_TMP:-}/server.log" ]; then
    printf '  server.log (tail):\n' >&2
    tail -n 40 "$DF_TMP/server.log" | sed 's/^/    /' >&2 || true
  fi
  exit 1
}

# ---------------------------------------------------------------- 初始化

df_require_cmd() {
  local cmd=$1
  command -v "$cmd" >/dev/null 2>&1 || {
    printf '[api-test][FAIL] 缺少依赖命令：%s\n' "$cmd" >&2
    exit 1
  }
}

df_init() {
  df_require_cmd curl
  df_require_cmd python3
  DF_TMP="$(mktemp -d "${TMPDIR:-/tmp}/df-api-test.XXXXXX")"
  trap df_cleanup EXIT
  DF_AUTH=(-H 'X-DF-Test-Harness: 1')
  if [ -n "$TOKEN" ]; then
    DF_AUTH+=(-H "Authorization: Bearer $TOKEN")
    df_log "BASE=$BASE  TOKEN=${TOKEN:0:4}…(${#TOKEN} chars)"
  else
    df_log "BASE=$BASE  TOKEN=<空：匿名/本机未启用鉴权>"
  fi
  if [ "$MOCK" = "1" ]; then
    df_log "MOCK=1：未连接真实服务时将启动 tests/api/mock_server.py"
  fi
}

df_cleanup() {
  local code=$?
  trap - EXIT
  # 脚本可定义 df_test_cleanup 做尽力而为的资源清理（删除测试模板/应用）。
  # 它在服务仍存活时执行；任何失败都忽略，避免掩盖原始退出码。
  if declare -F df_test_cleanup >/dev/null 2>&1; then
    df_test_cleanup || true
  fi
  if [ -n "${DF_SERVER_PID:-}" ] && kill -0 "$DF_SERVER_PID" 2>/dev/null; then
    kill "$DF_SERVER_PID" 2>/dev/null || true
    wait "$DF_SERVER_PID" 2>/dev/null || true
  fi
  if [ -n "${DF_TMP:-}" ] && [ -d "$DF_TMP" ]; then
    if [ "${KEEP_TMP:-0}" = "1" ]; then
      df_log "临时目录保留：$DF_TMP"
    else
      rm -rf "$DF_TMP"
    fi
  fi
  exit "$code"
}

# ---------------------------------------------------------------- 请求

_df_request() {
  # 内部：method path auth(1|0) [curl args...]
  local method=$1 path=$2 use_auth=$3
  shift 3

  # 每次请求独立文件：并发脚本在子 shell 里调用 df_api 也不会互相覆盖。
  local body headers
  body=$(mktemp "$DF_TMP/body.XXXXXX")
  headers=$(mktemp "$DF_TMP/headers.XXXXXX")
  local args=()
  if [ "$use_auth" = "1" ]; then
    args=("${DF_AUTH[@]}")
  else
    args=(-H 'X-DF-Test-Harness: 1')
  fi
  args+=("$@")

  DF_LAST_METHOD=$method
  DF_LAST_PATH=$path
  DF_BODY=$body
  DF_HEADERS=$headers

  local code rc
  set +e
  code=$(curl -sS --max-time "$API_TIMEOUT" \
    -o "$body" -D "$headers" -w '%{http_code}' \
    -X "$method" "${args[@]}" "$BASE$path")
  rc=$?
  set -e

  DF_STATUS=$code
  if [ "$rc" -ne 0 ]; then
    df_fail "curl 失败（exit=${rc}）：$method $BASE$path"
  fi
  if [ "$VERBOSE" = "1" ]; then
    df_log "$method $path -> $DF_STATUS"
  fi
}

df_api() {
  # 带认证（TOKEN 非空时）请求；额外参数原样传给 curl。
  _df_request "$1" "$2" 1 "${@:3}"
}

df_api_anon() {
  # 不发送 Authorization 的请求（用于 healthz / 401 断言）。
  _df_request "$1" "$2" 0 "${@:3}"
}

# ---------------------------------------------------------------- JSON 取值

df_json_get() {
  # $1=文件 $2=点路径（支持数组下标，空/root 表示根） [$3=默认值]
  local file=$1 path=$2
  local out rc
  set +e
  out=$(python3 - "$file" "$path" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        value = json.load(fh)
except Exception:
    sys.exit(2)

path = sys.argv[2]
if path not in ("", ".", "root", "$"):
    for part in path.split("."):
        if part == "":
            continue
        try:
            if isinstance(value, list):
                value = value[int(part)]
            elif isinstance(value, dict):
                value = value[part]
            else:
                sys.exit(3)
        except (KeyError, IndexError, ValueError, TypeError):
            sys.exit(3)

if isinstance(value, bool):
    print("true" if value else "false")
elif value is None:
    print("null")
elif isinstance(value, (dict, list)):
    print(json.dumps(value, ensure_ascii=False, sort_keys=True))
else:
    print(value)
PY
)
  rc=$?
  set -e
  if [ "$rc" -ne 0 ]; then
    if [ "$#" -ge 3 ]; then
      printf '%s' "$3"
      return 0
    fi
    return "$rc"
  fi
  printf '%s' "$out"
}

df_json_count() {
  # $1=文件 $2=逗号分隔的候选路径；返回第一个命中的数组/对象长度。
  local file=$1 candidates=$2
  local out rc
  set +e
  out=$(python3 - "$file" "$candidates" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        root = json.load(fh)
except Exception:
    sys.exit(2)

for candidate in sys.argv[2].split(","):
    value = root
    ok = True
    for part in candidate.split("."):
        if part in ("", ".", "root", "$"):
            continue
        try:
            if isinstance(value, list):
                value = value[int(part)]
            elif isinstance(value, dict):
                value = value[part]
            else:
                ok = False
                break
        except (KeyError, IndexError, ValueError, TypeError):
            ok = False
            break
    if ok and isinstance(value, (list, dict)):
        print(len(value))
        sys.exit(0)
sys.exit(4)
PY
)
  rc=$?
  set -e
  [ "$rc" -eq 0 ] || return 1
  printf '%s' "$out"
}

df_capture_revision() {
  # 从当前 DF_BODY 读取 revision；缺失/为空即失败。
  local label=${1:-响应}
  local rev
  rev=$(df_json_get "$DF_BODY" "revision" 2>/dev/null || true)
  if [ -z "$rev" ] || [ "$rev" = "null" ]; then
    rev=$(df_json_get "$DF_BODY" "data.revision" 2>/dev/null || true)
  fi
  if [ -z "$rev" ] || [ "$rev" = "null" ]; then
    rev=$(df_json_get "$DF_BODY" "meta.revision" 2>/dev/null || true)
  fi
  if [ -z "$rev" ] || [ "$rev" = "null" ]; then
    df_fail "$label: 响应缺少 revision 字段"
  fi
  printf '%s' "$rev"
}

# ---------------------------------------------------------------- 断言

df_status_is() {
  # $1=期望状态，允许 "200" 或 "200|201"
  local expected=${1//|/ }
  local status
  for status in $expected; do
    if [ "$DF_STATUS" = "$status" ]; then
      return 0
    fi
  done
  return 1
}

df_assert_status() {
  local expected=$1 label=$2
  if df_status_is "$expected"; then
    df_note "PASS $label -> HTTP $DF_STATUS"
  else
    df_fail "$label: 期望 HTTP ${expected}，实际 $DF_STATUS"
  fi
}

df_assert_json() {
  # $1=路径 $2=期望值 $3=标签
  local path=$1 expected=$2 label=$3
  local actual
  actual=$(df_json_get "$DF_BODY" "$path" "" 2>/dev/null || true)
  if [ "$actual" = "$expected" ]; then
    df_note "PASS $label ($path=$actual)"
  else
    df_fail "$label: $path 期望 [$expected]，实际 [$actual]"
  fi
}

df_assert_json_nonempty() {
  local path=$1 label=$2
  local actual
  actual=$(df_json_get "$DF_BODY" "$path" "" 2>/dev/null || true)
  if [ -n "$actual" ] && [ "$actual" != "null" ] && [ "$actual" != "{}" ] && [ "$actual" != "[]" ]; then
    df_note "PASS $label ($path 非空)"
  else
    df_fail "$label: 字段 $path 为空"
  fi
}

df_assert_error_code() {
  local code=$1 label=$2
  df_assert_json "error.code" "$code" "$label"
}

df_header_value() {
  # $1=响应头文件 $2=头名；输出去掉 CR 的值
  python3 - "$1" "$2" <<'PY'
import sys

name = sys.argv[2].lower()
try:
    with open(sys.argv[1], encoding="utf-8", errors="replace") as fh:
        for line in fh:
            if ":" not in line:
                continue
            key, value = line.split(":", 1)
            if key.strip().lower() == name:
                print(value.strip())
                sys.exit(0)
except OSError:
    pass
sys.exit(1)
PY
}

df_assert_header() {
  # $1=头名 $2=期望值（为空只断言存在） $3=标签
  local name=$1 expected=${2-} label=${3-}
  local value
  value=$(df_header_value "$DF_HEADERS" "$name" 2>/dev/null || true)
  if [ -z "$value" ]; then
    df_fail "$label: 响应缺少头 $name"
  fi
  if [ "$#" -ge 3 ] && [ -n "$expected" ] && [ "$value" != "$expected" ]; then
    df_fail "$label: 头 $name 期望 [$expected]，实际 [$value]"
  fi
  df_note "PASS $label ($name: $value)"
}

df_assert_revision_matches() {
  # $1=文件 $2=路径 $3=期望 revision $4=标签
  local file=$1 path=$2 expected=$3 label=$4
  local actual
  actual=$(df_json_get "$file" "$path" "" 2>/dev/null || true)
  if [ "$actual" = "$expected" ]; then
    df_note "PASS $label ($path == $expected)"
  else
    df_fail "$label: $path 期望 [$expected]，实际 [$actual]"
  fi
}

# ---------------------------------------------------------------- 服务探测/启动

df_base_port() {
  python3 - "$BASE" <<'PY'
import sys
import urllib.parse

parts = urllib.parse.urlsplit(sys.argv[1])
default = 443 if parts.scheme == "https" else 80
print(parts.port or default)
PY
}

df_probe_health() {
  local code
  set +e
  code=$(curl -sS --max-time 3 -o /dev/null -w '%{http_code}' "$BASE/healthz" 2>/dev/null)
  set -e
  [ "$code" = "200" ]
}

df_wait_health() {
  local label=$1
  local tries=${2:-120}
  local i=0
  while [ "$i" -lt "$tries" ]; do
    if [ -n "${DF_SERVER_PID:-}" ] && ! kill -0 "$DF_SERVER_PID" 2>/dev/null; then
      if [ -f "$DF_TMP/server.log" ]; then
        printf '[api-test][FAIL] %s: 服务进程已退出，日志：\n' "$label" >&2
        tail -n 40 "$DF_TMP/server.log" | sed 's/^/    /' >&2 || true
      fi
      df_fail "$label: 服务进程已退出"
    fi
    if df_probe_health; then
      df_log "$label: 服务就绪（${BASE}）"
      return 0
    fi
    i=$((i + 1))
    sleep 0.25
  done
  if [ -f "$DF_TMP/server.log" ]; then
    printf '[api-test][FAIL] %s: 等待健康检查超时，日志：\n' "$label" >&2
    tail -n 40 "$DF_TMP/server.log" | sed 's/^/    /' >&2 || true
  fi
  df_fail "$label: 等待 $BASE/healthz 超时（$((tries / 4))s）"
}

df_ensure_server() {
  if df_probe_health; then
    df_log "使用已运行的服务：$BASE"
    return 0
  fi

  local log="$DF_TMP/server.log"

  if [ -n "${SERVER_CMD:-}" ]; then
    df_log "按 SERVER_CMD 启动服务：$SERVER_CMD"
    ( cd "$DF_TMP" && eval "$SERVER_CMD" ) >"$log" 2>&1 &
    DF_SERVER_PID=$!
  elif [ -n "${SERVER_BIN:-}" ]; then
    [ -x "$SERVER_BIN" ] || df_fail "SERVER_BIN 不可执行：$SERVER_BIN"
    df_log "启动 SERVER_BIN：$SERVER_BIN ${SERVER_ARGS:-}"
    # shellcheck disable=SC2086  # SERVER_ARGS 故意做单词拆分
    ( cd "$DF_TMP" && "$SERVER_BIN" ${SERVER_ARGS:-} ) >"$log" 2>&1 &
    DF_SERVER_PID=$!
  elif [ "$MOCK" = "1" ]; then
    if [ -z "$TOKEN" ]; then
      TOKEN="$DF_FIXTURE_TOKEN"
      DF_AUTH+=(-H "Authorization: Bearer $TOKEN")
      df_warn "MOCK 模式未提供 TOKEN，使用 fixture token：$TOKEN"
    fi
    local port
    port=$(df_base_port)
    df_log "启动契约 mock server：python3 tests/api/mock_server.py --port $port"
    python3 "$COMMON_DIR/mock_server.py" \
      --port "$port" --token "$TOKEN" --config "$DF_TMP/config.toml" \
      --base "$BASE" >"$log" 2>&1 &
    DF_SERVER_PID=$!
  else
    df_fail "无法连接 $BASE/healthz。
  可选做法：
    1) 已运行服务：BASE=http://127.0.0.1:8686/api/v1 TOKEN=<token> bash $0
    2) 本机二进制：SERVER_BIN=target/debug/device_faker_server [SERVER_ARGS='--config ...'] ...
    3) 契约自测：MOCK=1 bash $0
  详见 tests/api/README.md"
  fi

  df_wait_health "等待服务启动"
}
