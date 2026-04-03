#!/usr/bin/env bash
# Canary Fleet Test Runner — hits all endpoints and reports verdicts
set -euo pipefail

BASE="${1:-http://localhost:8090}/canary-fleet"
PASS=0
FAIL=0
ERR=0

test_ep() {
  local label="$1" method="$2" url="$3"
  shift 3
  local body="${1:-}"

  local resp
  if [ "$method" = "POST" ] || [ "$method" = "PUT" ] || [ "$method" = "DELETE" ]; then
    resp=$(curl -s -m 8 -X "$method" -H "Content-Type: application/json" -d "${body:-{}}" "$url" 2>/dev/null) || true
  else
    resp=$(curl -s -m 8 -X "$method" "$url" 2>/dev/null) || true
  fi

  if [ -z "$resp" ]; then
    printf "  ? %-42s TIMEOUT\n" "$label"
    ERR=$((ERR+1))
    return
  fi

  local test_id passed http_code
  test_id=$(echo "$resp" | python3 -c "import json,sys; print(json.load(sys.stdin).get('test_id',''))" 2>/dev/null) || true
  passed=$(echo "$resp" | python3 -c "import json,sys; d=json.load(sys.stdin); print('1' if d.get('passed') else '0')" 2>/dev/null) || true
  http_code=$(echo "$resp" | python3 -c "import json,sys; print(json.load(sys.stdin).get('code',''))" 2>/dev/null) || true

  if [ "$passed" = "1" ]; then
    printf "  PASS %-40s\n" "${test_id:-$label}"
    PASS=$((PASS+1))
  elif [ -n "$test_id" ] && [ "$test_id" != "" ]; then
    printf "  FAIL %-40s\n" "${test_id:-$label}"
    FAIL=$((FAIL+1))
  elif [ -n "$http_code" ] && [ "$http_code" != "" ]; then
    printf "  HTTP %-5s %-34s\n" "$http_code" "$label"
    ERR=$((ERR+1))
  else
    printf "  ?    %-40s\n" "$label"
    ERR=$((ERR+1))
  fi
}

echo ""
echo "  CANARY FLEET — Test Results"
echo "  $(date)"
echo "  Base: $BASE"
echo "  ────────────────────────────────────────────────"

echo ""
echo "  ── HANDLERS Profile (RUNTIME) ──"
test_ep "ctx-request"        POST "$BASE/handlers/canary/rt/ctx/request" '{}'
test_ep "ctx-resdata"        GET  "$BASE/handlers/canary/rt/ctx/resdata"
test_ep "ctx-data"           GET  "$BASE/handlers/canary/rt/ctx/data"
test_ep "ctx-dataview"       GET  "$BASE/handlers/canary/rt/ctx/dataview"
test_ep "ctx-dataview-params" POST "$BASE/handlers/canary/rt/ctx/dataview-params" '{}'
test_ep "ctx-pseudo-dv"      GET  "$BASE/handlers/canary/rt/ctx/pseudo-dv"
test_ep "ctx-store-set"      POST "$BASE/handlers/canary/rt/ctx/store" '{}'
test_ep "ctx-store-get"      GET  "$BASE/handlers/canary/rt/ctx/store"
test_ep "ctx-store-ns"       GET  "$BASE/handlers/canary/rt/ctx/store-ns"
test_ep "ctx-trace-id"       GET  "$BASE/handlers/canary/rt/ctx/trace-id"
test_ep "ctx-node-id"        GET  "$BASE/handlers/canary/rt/ctx/node-id"
test_ep "ctx-app-id"         GET  "$BASE/handlers/canary/rt/ctx/app-id"
test_ep "ctx-env"            GET  "$BASE/handlers/canary/rt/ctx/env"
test_ep "ctx-session"        GET  "$BASE/handlers/canary/rt/ctx/session"
test_ep "crypto-random"      GET  "$BASE/handlers/canary/rt/rivers/crypto-random"
test_ep "crypto-hash"        GET  "$BASE/handlers/canary/rt/rivers/crypto-hash"
test_ep "crypto-timing"      GET  "$BASE/handlers/canary/rt/rivers/crypto-timing"
test_ep "crypto-hmac"        GET  "$BASE/handlers/canary/rt/rivers/crypto-hmac"
test_ep "rivers-log"         GET  "$BASE/handlers/canary/rt/rivers/log"
test_ep "v8-codegen"         GET  "$BASE/handlers/canary/rt/v8/codegen"
test_ep "v8-console"         GET  "$BASE/handlers/canary/rt/v8/console"
test_ep "v8-timeout"         GET  "$BASE/handlers/canary/rt/v8/timeout"
test_ep "v8-heap"            GET  "$BASE/handlers/canary/rt/v8/heap"
test_ep "error-sanitize"     GET  "$BASE/handlers/canary/rt/error/sanitize"
test_ep "eventbus-publish"   POST "$BASE/handlers/canary/rt/eventbus/publish" '{}'
test_ep "header-blocklist"   GET  "$BASE/handlers/canary/rt/header/blocklist"
test_ep "faker-determinism"  GET  "$BASE/handlers/canary/rt/faker/determinism"

echo ""
echo "  ── AUTH Profile (guard login) ──"
test_ep "guard-login"        POST "$BASE/guard/canary/auth/login" '{"username":"canary","password":"canary-test"}'

echo ""
echo "  ── STREAMS Profile ──"
test_ep "poll-data"          GET  "$BASE/streams/canary/stream/poll/data"

echo ""
echo "  ────────────────────────────────────────────────"
echo "  Pass: $PASS  Fail: $FAIL  Error/Timeout: $ERR  Total: $((PASS+FAIL+ERR))"
echo ""
