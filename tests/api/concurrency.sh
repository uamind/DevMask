#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# 并发条件写测试：同一个 If-Match 并发两次 PUT /api/v1/templates/{name}，
# 断言恰好一个 200、一个 412（config_revision_mismatch），且胜者返回的 revision
# 与随后读取的 /config/revision 一致。
#
# 用法：
#   TOKEN=df_xxx bash tests/api/concurrency.sh
#   MOCK=1 bash tests/api/concurrency.sh
#   ROUNDS=3 TOKEN=df_xxx bash tests/api/concurrency.sh

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

ROUNDS="${ROUNDS:-1}"
NAME="df_concurrency_$$"
DF_CONC_NAME=$NAME

df_test_cleanup() {
  [ -n "${DF_CONC_NAME:-}" ] || return 0
  local auth rev
  auth=(-H 'X-DF-Test-Harness: 1' -H 'Accept: application/json')
  if [ -n "$TOKEN" ]; then
    auth+=(-H "Authorization: Bearer $TOKEN")
  fi
  set +e
  rev=$(curl -sS --max-time 3 "${auth[@]}" "$BASE/config/revision" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("revision",""))' 2>/dev/null || true)
  if [ -n "$rev" ]; then
    curl -sS --max-time 3 -X DELETE "${auth[@]}" -H "If-Match: $rev" "$BASE/templates/$NAME?confirm=true" >/dev/null 2>&1
  fi
  set -e
}

current_revision() {
  df_api GET "/config/revision" "${DF_AUTH[@]}"
  df_assert_status "200" "GET /config/revision"
  df_capture_revision "GET /config/revision"
}

json_field_of() {
  # $1=文件 $2=路径 [$3=默认]
  df_json_get "$1" "$2" "${3-}" 2>/dev/null || true
}

round=1
while [ "$round" -le "$ROUNDS" ]; do
  df_log "== 并发轮次 $round/${ROUNDS}（模板 ${NAME}）=="
  BASE_REV=$(current_revision)
  df_note "基线 revision：$BASE_REV"

  # 先建模板：让并发 PUT 是「更新」而非「创建」，契约上稳定得到 200 而不是 201。
  df_api PUT "/templates/$NAME" "${DF_AUTH[@]}" -H "If-Match: $BASE_REV" -H 'Content-Type: application/json' \
    --data-binary '{"packages":["com.example.concurrency"],"model":"Concurrency Seed","brand":"google"}'
  df_assert_status "200|201" "并发前置：创建模板 $NAME"
  SEED_REV=$(df_capture_revision "并发前置：创建模板")
  if [ "$SEED_REV" = "$BASE_REV" ]; then
    df_fail "并发前置：创建模板后 revision 未变化"
  fi
  BASE_REV=$SEED_REV
  df_note "并发基线 revision（模板已存在）：$BASE_REV"

  BODY_A="$DF_TMP/concurrent-a.json"
  BODY_B="$DF_TMP/concurrent-b.json"
  printf '{"packages":["com.example.concurrency"],"model":"Concurrency A %s","brand":"google"}' "$round" > "$BODY_A"
  printf '{"packages":["com.example.concurrency"],"model":"Concurrency B %s","brand":"google"}' "$round" > "$BODY_B"

  (
    df_api PUT "/templates/$NAME" "${DF_AUTH[@]}" -H "If-Match: $BASE_REV" -H 'Content-Type: application/json' --data-binary "@$BODY_A"
    printf '%s' "$DF_STATUS" > "$DF_TMP/status-a"
    cp "$DF_BODY" "$DF_TMP/body-a.json"
    cp "$DF_HEADERS" "$DF_TMP/headers-a.txt"
  ) &
  PID_A=$!

  (
    df_api PUT "/templates/$NAME" "${DF_AUTH[@]}" -H "If-Match: $BASE_REV" -H 'Content-Type: application/json' --data-binary "@$BODY_B"
    printf '%s' "$DF_STATUS" > "$DF_TMP/status-b"
    cp "$DF_BODY" "$DF_TMP/body-b.json"
    cp "$DF_HEADERS" "$DF_TMP/headers-b.txt"
  ) &
  PID_B=$!

  wait "$PID_A" || true
  wait "$PID_B" || true

  [ -f "$DF_TMP/status-a" ] || df_fail "并发请求 A 未产生状态码（子进程异常）"
  [ -f "$DF_TMP/status-b" ] || df_fail "并发请求 B 未产生状态码（子进程异常）"
  STATUS_A=$(cat "$DF_TMP/status-a")
  STATUS_B=$(cat "$DF_TMP/status-b")
  df_note "并发结果：A=$STATUS_A B=$STATUS_B"

  SORTED=$(printf '%s\n%s\n' "$STATUS_A" "$STATUS_B" | sort | tr '\n' ' ')
  if [ "$SORTED" != "200 412 " ]; then
    df_fail "同一 If-Match 并发两次 PUT：期望一个 200、一个 412，实际 A=$STATUS_A B=$STATUS_B"
  fi
  df_note "PASS 恰好一个 200、一个 412"

  if [ "$STATUS_A" = "200" ]; then
    WINNER_BODY="$DF_TMP/body-a.json"
    LOSER_BODY="$DF_TMP/body-b.json"
    WINNER_HEADERS="$DF_TMP/headers-a.txt"
    LOSER_HEADERS="$DF_TMP/headers-b.txt"
  else
    WINNER_BODY="$DF_TMP/body-b.json"
    LOSER_BODY="$DF_TMP/body-a.json"
    WINNER_HEADERS="$DF_TMP/headers-b.txt"
    LOSER_HEADERS="$DF_TMP/headers-a.txt"
  fi

  WINNER_REV=$(json_field_of "$WINNER_BODY" "revision")
  if [ -z "$WINNER_REV" ] || [ "$WINNER_REV" = "null" ]; then
    df_fail "胜者（200）响应缺少 revision"
  fi
  if [ "$WINNER_REV" = "$BASE_REV" ]; then
    df_fail "胜者 revision 未变化（仍是 ${BASE_REV}）"
  fi
  df_note "PASS 胜者 revision 已变更：$WINNER_REV"

  LOSER_CODE=$(json_field_of "$LOSER_BODY" "error.code")
  if [ "$LOSER_CODE" != "config_revision_mismatch" ]; then
    df_fail "败者（412）错误码期望 config_revision_mismatch，实际 [$LOSER_CODE]"
  fi
  df_note "PASS 败者错误码 = config_revision_mismatch"

  EXPECTED=$(json_field_of "$LOSER_BODY" "error.details.expected")
  ACTUAL=$(json_field_of "$LOSER_BODY" "error.details.actual")
  if [ -z "$EXPECTED" ] || [ -z "$ACTUAL" ]; then
    df_fail "败者错误信封缺少 details.expected / details.actual"
  fi
  if [ "$EXPECTED" != "$BASE_REV" ] || [ "$ACTUAL" != "$WINNER_REV" ]; then
    df_fail "败者 details 不一致：expected=[$EXPECTED] actual=[$ACTUAL]（应分别为 ${BASE_REV} 与 ${WINNER_REV}）"
  fi
  df_note "PASS 败者 details.expected/actual 正确"

  if [ "$ASSERT_HEADERS" = "1" ]; then
    if [ -z "$(df_header_value "$WINNER_HEADERS" "X-Request-Id" 2>/dev/null || true)" ]; then
      df_fail "胜者响应缺少 X-Request-Id"
    fi
    if [ -z "$(df_header_value "$LOSER_HEADERS" "X-Request-Id" 2>/dev/null || true)" ]; then
      df_fail "败者响应缺少 X-Request-Id"
    fi
    df_note "PASS 两个响应都带 X-Request-Id"
  fi

  CURRENT=$(current_revision)
  if [ "$CURRENT" != "$WINNER_REV" ]; then
    df_fail "胜者返回的 revision ($WINNER_REV) 与 /config/revision ($CURRENT) 不一致"
  fi
  df_note "PASS 胜者 revision 与当前配置一致"

  df_log "-- 控制组：过期 If-Match 必须 412 --"
  df_api PUT "/templates/$NAME" "${DF_AUTH[@]}" -H "If-Match: sha256:stale000000000000000000000000000000000000000000000000000000000000" -H 'Content-Type: application/json' --data-binary "@$BODY_A"
  df_assert_status "412" "过期 If-Match -> 412"
  df_assert_error_code "config_revision_mismatch" "过期 If-Match 错误码"

  df_log "-- 清理本轮模板 --"
  CURRENT=$(current_revision)
  df_api DELETE "/templates/$NAME?confirm=true" "${DF_AUTH[@]}" -H "If-Match: $CURRENT"
  df_assert_status "200|204" "DELETE /templates/$NAME"
  df_api GET "/templates/$NAME" "${DF_AUTH[@]}"
  df_assert_status "404" "已删除并发测试模板 -> 404"

  round=$((round + 1))
done

DF_CONC_NAME=""
df_log "CONCURRENCY PASS：$ROUNDS 轮均满足「一个 200、一个 412」"
