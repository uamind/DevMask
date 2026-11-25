#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# DevMask /api/v1 冒烟测试（契约见 docs/API.md）：
# healthz → status → config → templates CRUD → apps（绑定 + 内联 + explain）→ packages → 清理。
#
# 用法（详见 tests/api/README.md）：
#   TOKEN=df_xxx bash tests/api/smoke.sh
#   MOCK=1 bash tests/api/smoke.sh                  # 无服务时用契约 mock 自测
#   SERVER_BIN=target/debug/devmask_server TOKEN=df_xxx bash tests/api/smoke.sh
#
# 断言：所有请求的状态码、写响应 WriteResult.revision / previousRevision、
#       X-Request-Id / X-DM-Revision / ETag 响应头、404 统一错误信封。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BASE_WAS_SET="${BASE+x}"
MOCK="${MOCK:-0}"
MOCK_PORT="${MOCK_PORT:-18686}"

if [ "$MOCK" = "1" ] && [ -z "$BASE_WAS_SET" ]; then
  BASE="http://127.0.0.1:$MOCK_PORT/api/v1"
fi

# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

df_init
df_ensure_server

TPL="df_smoke_$$"
PKG="com.devmask.smoke.p$$"
DF_SMOKE_TPL=$TPL
DF_SMOKE_PKG=$PKG

# 失败/中断时尽力清理本脚本创建的模板与应用（服务需仍存活）。
df_test_cleanup() {
  [ -n "${DF_SMOKE_TPL:-}" ] || return 0
  local auth rev
  auth=(-H 'X-DM-Test-Harness: 1' -H 'Accept: application/json')
  if [ -n "$TOKEN" ]; then
    auth+=(-H "Authorization: Bearer $TOKEN")
  fi
  _cleanup_revision() {
    curl -sS --max-time 3 "${auth[@]}" "$BASE/config/revision" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("revision",""))' 2>/dev/null || true
  }
  set +e
  rev=$(_cleanup_revision)
  if [ -n "$rev" ]; then
    curl -sS --max-time 3 -X DELETE "${auth[@]}" -H "If-Match: $rev" "$BASE/apps/$PKG?confirm=true&detach_templates=true" >/dev/null 2>&1
    rev=$(_cleanup_revision)
    if [ -n "$rev" ]; then
      curl -sS --max-time 3 -X DELETE "${auth[@]}" -H "If-Match: $rev" "$BASE/templates/$TPL?confirm=true" >/dev/null 2>&1
    fi
  fi
  set -e
}

capture_write_revision() {
  # WriteResult.revision；若端点返回 204（契约不允许）则回读 /config/revision 兜底。
  local label=$1 rev
  rev=$(df_json_get "$DF_BODY" "revision" 2>/dev/null || true)
  if [ -z "$rev" ] || [ "$rev" = "null" ]; then
    df_api GET "/config/revision" "${DF_AUTH[@]}"
    df_assert_status "200" "${label}（回读 revision）"
    rev=$(df_capture_revision "${label}（回读 revision）")
  fi
  printf '%s' "$rev"
}

assert_revision_changed() {
  local old=$1 new=$2 label=$3
  if [ -z "$new" ] || [ "$new" = "null" ]; then
    df_fail "$label: 写操作未返回 revision"
  fi
  if [ "$new" = "$old" ]; then
    df_fail "$label: revision 未变化（仍是 ${old}）"
  fi
  df_note "PASS $label (revision ${old} -> ${new})"
}

assert_previous_revision() {
  # WriteResult.previousRevision 必须等于写入前的 revision（docs/API.md §6.5）。
  local expected=$1 label=$2 actual
  actual=$(df_json_get "$DF_BODY" "previousRevision" "" 2>/dev/null || true)
  if [ "$actual" != "$expected" ]; then
    df_fail "$label: previousRevision 期望 [$expected]，实际 [$actual]"
  fi
  df_note "PASS $label (previousRevision=$actual)"
}

# ---------------------------------------------------------------- 1. healthz

df_log "== 1/8 healthz（无鉴权）=="
df_api_anon GET "/healthz"
df_assert_status "200" "GET /healthz"
df_assert_json "status" "ok" "healthz.status"

# ---------------------------------------------------------------- 2. status

df_log "== 2/8 status =="
df_api GET "/status" "${DF_AUTH[@]}"
df_assert_status "200" "GET /status"
df_assert_json "module.id" "devmask" "status.module.id"
STATUS_REV=$(df_json_get "$DF_BODY" "config.revision" "" 2>/dev/null || true)
if [ -z "$STATUS_REV" ] || [ "$STATUS_REV" = "null" ]; then
  df_fail "status.config.revision 缺失或为空"
fi
df_note "PASS status.config.revision = $STATUS_REV"
if [ "$ASSERT_HEADERS" = "1" ]; then
  df_assert_header "X-Request-Id" "" "status: X-Request-Id"
  df_assert_header "X-DM-Revision" "$STATUS_REV" "status: X-DM-Revision"
fi

# ---------------------------------------------------------------- 3. config

df_log "== 3/8 config =="
df_api GET "/config" "${DF_AUTH[@]}"
df_assert_status "200" "GET /config"
RV=$(df_capture_revision "GET /config")
CONFIG_BODY=$(df_json_get "$DF_BODY" "config" "" 2>/dev/null || true)
if [ -z "$CONFIG_BODY" ]; then
  df_fail "GET /config: 响应缺少 config 对象"
fi
df_note "PASS GET /config 返回 config 对象"
if [ "$ASSERT_HEADERS" = "1" ]; then
  df_assert_header "X-DM-Revision" "$RV" "config: X-DM-Revision"
  df_assert_header "ETag" "\"$RV\"" "config: ETag"
fi
if [ "$STATUS_REV" != "$RV" ]; then
  df_fail "status.config.revision ($STATUS_REV) 与 GET /config revision ($RV) 不一致"
fi
df_note "PASS status 与 config revision 一致：$RV"

df_api GET "/config/revision" "${DF_AUTH[@]}"
df_assert_status "200" "GET /config/revision"
df_assert_revision_matches "$DF_BODY" "revision" "$RV" "config/revision 与 GET /config 一致"
if [ "$ASSERT_HEADERS" = "1" ]; then
  df_assert_header "ETag" "\"$RV\"" "config/revision: ETag"
fi

# ---------------------------------------------------------------- 4. templates 列表

df_log "== 4/8 templates 列表 =="
df_api GET "/templates" "${DF_AUTH[@]}"
df_assert_status "200" "GET /templates"
TEMPLATE_COUNT=$(df_json_count "$DF_BODY" "items,templates,data.templates" || true)
if [ -z "$TEMPLATE_COUNT" ]; then
  df_fail "GET /templates: 无法识别列表形状（items/templates/data.templates）"
fi
df_assert_json_nonempty "total" "templates.total"
df_note "PASS 首页模板数量 = $TEMPLATE_COUNT"

# ---------------------------------------------------------------- 5. templates CRUD

df_log "== 5/8 templates CRUD =="
TEMPLATE_PAYLOAD="$DF_TMP/template-create.json"
python3 - "$TESTS_ROOT/fixtures/api/template.json" "$TEMPLATE_PAYLOAD" <<'PY'
import json
import sys

source, target = sys.argv[1:3]
with open(source, encoding="utf-8") as fh:
    data = json.load(fh)
# 先建空包名模板，随后由 PUT /apps/{pkg} 绑定，才能观察到 revision 变化。
data["packages"] = []
with open(target, "w", encoding="utf-8") as fh:
    json.dump(data, fh, ensure_ascii=False)
    fh.write("\n")
PY

df_api PUT "/templates/$TPL" "${DF_AUTH[@]}" -H "If-Match: $RV" -H 'Content-Type: application/json' --data-binary "@$TEMPLATE_PAYLOAD"
df_assert_status "200|201" "PUT /templates/${TPL}（创建）"
NEW_RV=$(capture_write_revision "PUT /templates/$TPL")
assert_revision_changed "$RV" "$NEW_RV" "创建模板"
RV=$NEW_RV

df_api GET "/templates/$TPL" "${DF_AUTH[@]}"
df_assert_status "200" "GET /templates/$TPL"
df_assert_json "name" "$TPL" "模板详情顶层 name"
df_assert_json "template.model" "Pixel 9" "模板详情 template.model"
if [ "$ASSERT_HEADERS" = "1" ]; then
  df_assert_header "ETag" "\"$RV\"" "模板详情: ETag"
fi

df_api PATCH "/templates/$TPL" "${DF_AUTH[@]}" -H "If-Match: $RV" -H 'Content-Type: application/json' --data-binary '{"model":"Pixel 9 Pro","dpi":430}'
df_assert_status "200" "PATCH /templates/$TPL"
NEW_RV=$(capture_write_revision "PATCH /templates/$TPL")
assert_previous_revision "$RV" "PATCH /templates/$TPL"
assert_revision_changed "$RV" "$NEW_RV" "PATCH 模板"
RV=$NEW_RV

df_api GET "/templates/$TPL" "${DF_AUTH[@]}"
df_assert_status "200" "GET /templates/${TPL}（PATCH 后）"
df_assert_json "template.model" "Pixel 9 Pro" "PATCH 后 template.model"



# ---------------------------------------------------------------- 6. apps：绑定模板 + 内联 + explain

df_log "== 6/8 apps（绑定 / 内联 / explain）=="
df_api GET "/apps" "${DF_AUTH[@]}"
df_assert_status "200" "GET /apps"
APPS_COUNT=$(df_json_count "$DF_BODY" "items,apps,data.apps" || true)
if [ -z "$APPS_COUNT" ]; then
  df_fail "GET /apps: 无法识别列表形状（items/apps/data.apps）"
fi
df_note "PASS 内联应用数量 = $APPS_COUNT"

df_api PUT "/apps/$PKG" "${DF_AUTH[@]}" -H "If-Match: $RV" -H 'Content-Type: application/json' --data-binary "$(printf '{"template":"%s"}' "$TPL")"
df_assert_status "200|201" "PUT /apps/${PKG}（绑定模板）"
NEW_RV=$(capture_write_revision "PUT /apps/${PKG}（绑定模板）")
assert_previous_revision "$RV" "PUT /apps/${PKG}（绑定模板）"
assert_revision_changed "$RV" "$NEW_RV" "绑定应用"
RV=$NEW_RV

df_api GET "/apps/$PKG" "${DF_AUTH[@]}"
df_assert_status "200" "GET /apps/${PKG}（explain）"
df_assert_json "package" "$PKG" "应用详情 package"
df_assert_json "source.type" "template" "绑定后 source.type"
df_assert_json "source.template" "$TPL" "绑定后 source.template"
df_assert_json "resolved.model" "Pixel 9 Pro" "绑定后 resolved.model"
if [ "$ASSERT_HEADERS" = "1" ]; then
  df_assert_header "ETag" "\"$RV\"" "应用详情: ETag"
fi

df_api PUT "/apps/$PKG" "${DF_AUTH[@]}" -H "If-Match: $RV" -H 'Content-Type: application/json' \
  --data-binary "$(printf '{"config":{"package":"%s","manufacturer":"Google","brand":"google","model":"Inline Model 9","android_version":"15","sdk_int":35,"dpi":400}}' "$PKG")"
df_assert_status "200|201" "PUT /apps/${PKG}（内联配置）"
NEW_RV=$(capture_write_revision "PUT /apps/${PKG}（内联配置）")
assert_previous_revision "$RV" "PUT /apps/${PKG}（内联配置）"
assert_revision_changed "$RV" "$NEW_RV" "写入内联配置"
RV=$NEW_RV

df_api GET "/apps/$PKG" "${DF_AUTH[@]}"
df_assert_status "200" "GET /apps/${PKG}（内联后）"
df_assert_json "source.type" "inline" "内联后 source.type"
df_assert_json "resolved.model" "Inline Model 9" "内联后 resolved.model"
df_assert_json "source.alsoInline" "true" "内联覆盖模板时 source.alsoInline"

# ---------------------------------------------------------------- 7. packages

df_log "== 7/8 packages =="
df_api GET "/packages" "${DF_AUTH[@]}"
df_assert_status "200" "GET /packages"
PACKAGES_COUNT=$(df_json_count "$DF_BODY" "items,packages,data.packages" || true)
if [ -z "$PACKAGES_COUNT" ]; then
  df_fail "GET /packages: 无法识别列表形状（items/packages/data.packages）"
fi
df_assert_json_nonempty "source" "packages.source"
df_note "PASS 已安装包数量 = $PACKAGES_COUNT"

# ---------------------------------------------------------------- 8. 清理 + 最终一致性

df_log "== 8/8 清理与最终 revision =="
df_api DELETE "/apps/$PKG?confirm=true&detach_templates=true" "${DF_AUTH[@]}" -H "If-Match: $RV"
df_assert_status "200" "DELETE /apps/$PKG?confirm=true&detach_templates=true"
NEW_RV=$(capture_write_revision "DELETE /apps/$PKG")
assert_previous_revision "$RV" "DELETE /apps/$PKG"
assert_revision_changed "$RV" "$NEW_RV" "删除应用"
RV=$NEW_RV

df_api GET "/apps/$PKG" "${DF_AUTH[@]}"
df_assert_status "200" "GET 已删除应用（source=none）"
df_assert_json "source.type" "none" "已删除应用 source.type"

df_api DELETE "/templates/$TPL?confirm=true" "${DF_AUTH[@]}" -H "If-Match: $RV"
df_assert_status "200" "DELETE /templates/$TPL?confirm=true"
NEW_RV=$(capture_write_revision "DELETE /templates/$TPL")
assert_previous_revision "$RV" "DELETE /templates/$TPL"
assert_revision_changed "$RV" "$NEW_RV" "删除模板"
RV=$NEW_RV

df_api GET "/templates/$TPL" "${DF_AUTH[@]}"
df_assert_status "404" "GET 已删除模板 -> 404"
df_assert_error_code "not_found" "已删除模板错误码"

df_api GET "/config/revision" "${DF_AUTH[@]}"
df_assert_status "200" "GET /config/revision（收尾）"
df_assert_revision_matches "$DF_BODY" "revision" "$RV" "收尾 revision 与最后一次写一致"

DF_SMOKE_TPL=""
df_log "SMOKE PASS：status → config → templates CRUD → apps → packages → explain 全部通过"
df_log "最终 revision: $RV"
