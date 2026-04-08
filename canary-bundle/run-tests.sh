#!/usr/bin/env bash
# Canary Fleet Test Runner — hits all endpoints and reports verdicts
# Usage: ./run-tests.sh [base_url]
#   base_url defaults to https://localhost:8090
set -euo pipefail

BASE_URL="${1:-https://localhost:8090}"
BASE="$BASE_URL/canary-fleet"
COOKIES=$(mktemp /tmp/canary-cookies.XXXXXX)
CSRF_TOKEN=""
PASS=0
FAIL=0
ERR=0

cleanup() { rm -f "$COOKIES"; }
trap cleanup EXIT

# ── Extract CSRF token from cookie jar ───────────────────────────

get_csrf_token() {
  CSRF_TOKEN=$(grep rivers_csrf "$COOKIES" 2>/dev/null | awk '{print $NF}') || true
}

# ── Test helper ──────────────────────────────────────────────────

test_ep() {
  local label="$1" method="$2" url="$3"
  shift 3
  local body="${1:-}"

  local resp
  if [ "$method" = "POST" ] || [ "$method" = "PUT" ] || [ "$method" = "DELETE" ]; then
    resp=$(curl -sk -m 8 -b "$COOKIES" -c "$COOKIES" \
      -X "$method" -H "Content-Type: application/json" \
      -H "X-CSRF-Token: ${CSRF_TOKEN}" \
      -d "${body:-{}}" "$url" 2>/dev/null) || true
  else
    resp=$(curl -sk -m 8 -b "$COOKIES" -c "$COOKIES" \
      -X "$method" "$url" 2>/dev/null) || true
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

# Warm up V8 engine — first request compiles script cache
curl -sk -m 5 "$BASE/handlers/canary/rt/ctx/trace-id" >/dev/null 2>&1
sleep 1

# ── AUTH Profile — login to get session + CSRF cookies ───────────

echo ""
echo "  ── AUTH Profile (guard login) ──"

# Guard login — special handling (returns {allow,session_claims}, not {test_id,passed})
GUARD_RESP=$(curl -sk -m 8 -b "$COOKIES" -c "$COOKIES" \
  -X POST -H "Content-Type: application/json" \
  -d '{"username":"canary","password":"canary-test"}' \
  "$BASE/guard/canary/auth/login" 2>/dev/null) || true

GUARD_ALLOW=$(echo "$GUARD_RESP" | python3 -c "import json,sys; d=json.load(sys.stdin); print('1' if d.get('allow') else '0')" 2>/dev/null) || true
GUARD_CODE=$(echo "$GUARD_RESP" | python3 -c "import json,sys; print(json.load(sys.stdin).get('code',''))" 2>/dev/null) || true

if [ "$GUARD_ALLOW" = "1" ]; then
  printf "  PASS %-40s\n" "AUTH-GUARD-LOGIN"
  PASS=$((PASS+1))
  # Extract CSRF token for subsequent write requests
  get_csrf_token
elif [ -n "$GUARD_CODE" ] && [ "$GUARD_CODE" != "" ]; then
  printf "  HTTP %-5s %-34s\n" "$GUARD_CODE" "guard-login"
  ERR=$((ERR+1))
else
  printf "  FAIL %-40s\n" "guard-login"
  FAIL=$((FAIL+1))
fi

# ── HANDLERS Profile (auth=none, no session needed) ──────────────

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
test_ep "error-sanitize"     GET  "$BASE/handlers/canary/rt/error/sanitize"
test_ep "eventbus-publish"   POST "$BASE/handlers/canary/rt/eventbus/publish" '{}'
test_ep "header-blocklist"   GET  "$BASE/handlers/canary/rt/header/blocklist"
test_ep "faker-determinism"  GET  "$BASE/handlers/canary/rt/faker/determinism"

# ── SQL Profile (auth=session, uses PG/MySQL/SQLite) ─────────────

echo ""
echo "  ── SQL Profile ──"
# PostgreSQL
test_ep "pg-param-order"     POST "$BASE/sql/canary/sql/pg/param-order" '{}'
test_ep "pg-insert"          POST "$BASE/sql/canary/sql/pg/insert" '{}'
test_ep "pg-select"          GET  "$BASE/sql/canary/sql/pg/select"
test_ep "pg-update"          PUT  "$BASE/sql/canary/sql/pg/update" '{}'
test_ep "pg-delete"          DELETE "$BASE/sql/canary/sql/pg/delete" '{}'
test_ep "pg-ddl-reject"      POST "$BASE/sql/canary/sql/pg/ddl-reject" '{}'
test_ep "pg-max-rows"        GET  "$BASE/sql/canary/sql/pg/max-rows"
# MySQL
test_ep "mysql-param-order"  POST "$BASE/sql/canary/sql/mysql/param-order" '{}'
test_ep "mysql-insert"       POST "$BASE/sql/canary/sql/mysql/insert" '{}'
test_ep "mysql-select"       GET  "$BASE/sql/canary/sql/mysql/select"
test_ep "mysql-update"       PUT  "$BASE/sql/canary/sql/mysql/update" '{}'
test_ep "mysql-delete"       DELETE "$BASE/sql/canary/sql/mysql/delete" '{}'
test_ep "mysql-ddl-reject"   POST "$BASE/sql/canary/sql/mysql/ddl-reject" '{}'
# SQLite
test_ep "sqlite-ddl-persist" GET  "$BASE/sql/canary/sql/sqlite/ddl-persist"
test_ep "sqlite-param-order" GET  "$BASE/sql/canary/sql/sqlite/param-order"
test_ep "sqlite-insert"      POST "$BASE/sql/canary/sql/sqlite/insert" '{}'
test_ep "sqlite-select"      GET  "$BASE/sql/canary/sql/sqlite/select"
test_ep "sqlite-prefix"      GET  "$BASE/sql/canary/sql/sqlite/prefix"
# Cache
test_ep "cache-l1-hit"       GET  "$BASE/sql/canary/sql/cache/l1-hit"
test_ep "cache-invalidate"   POST "$BASE/sql/canary/sql/cache/invalidate" '{}'
# Init + Negative
test_ep "init-ddl-success"   GET  "$BASE/sql/canary/sql/init/ddl-success"
test_ep "neg-ddl-rejected"   GET  "$BASE/sql/canary/sql/negative/ddl-rejected"
test_ep "neg-error-sanitized" GET "$BASE/sql/canary/sql/negative/error-sanitized"

# ── NoSQL Profile (auth=session, uses Mongo/ES/Couch/Cassandra/LDAP/Redis) ──

echo ""
echo "  ── NoSQL Profile ──"
# MongoDB
test_ep "mongo-ping"         GET  "$BASE/nosql/canary/nosql/mongo/ping"
test_ep "mongo-insert"       POST "$BASE/nosql/canary/nosql/mongo/insert" '{}'
test_ep "mongo-find"         GET  "$BASE/nosql/canary/nosql/mongo/find"
test_ep "mongo-admin-reject" POST "$BASE/nosql/canary/nosql/mongo/admin-reject" '{}'
# Elasticsearch
test_ep "es-ping"            GET  "$BASE/nosql/canary/nosql/es/ping"
test_ep "es-index"           POST "$BASE/nosql/canary/nosql/es/index" '{}'
test_ep "es-search"          GET  "$BASE/nosql/canary/nosql/es/search"
# CouchDB
test_ep "couch-ping"         GET  "$BASE/nosql/canary/nosql/couch/ping"
test_ep "couch-put"          POST "$BASE/nosql/canary/nosql/couch/put" '{}'
test_ep "couch-get"          GET  "$BASE/nosql/canary/nosql/couch/get"
# Cassandra
test_ep "cassandra-ping"     GET  "$BASE/nosql/canary/nosql/cassandra/ping"
test_ep "cassandra-insert"   POST "$BASE/nosql/canary/nosql/cassandra/insert" '{}'
test_ep "cassandra-select"   GET  "$BASE/nosql/canary/nosql/cassandra/select"
# LDAP
test_ep "ldap-ping"          GET  "$BASE/nosql/canary/nosql/ldap/ping"
test_ep "ldap-search"        GET  "$BASE/nosql/canary/nosql/ldap/search"
# Redis
test_ep "redis-ping"         GET  "$BASE/nosql/canary/nosql/redis/ping"
test_ep "redis-set"          POST "$BASE/nosql/canary/nosql/redis/set" '{}'
test_ep "redis-get"          GET  "$BASE/nosql/canary/nosql/redis/get"
test_ep "redis-admin-reject" POST "$BASE/nosql/canary/nosql/redis/admin-reject" '{}'

# ── STREAMS Profile ──────────────────────────────────────────────

echo ""
echo "  ── STREAMS Profile ──"
test_ep "poll-data"          GET  "$BASE/streams/canary/stream/poll/data"

# ── V8 Security (last — these are slow/destructive) ──────────────

echo ""
echo "  ── V8 Security (slow) ──"
# These tests deliberately trigger timeout/OOM — need longer curl timeout
ORIG_TIMEOUT=8
test_ep_v8sec() {
  local label="$1" test_id="$2" method="$3" url="$4"
  local resp http_status
  # Get both response body and HTTP status
  resp=$(curl -sk -m 15 -b "$COOKIES" -c "$COOKIES" -X "$method" -w '\n%{http_code}' "$url" 2>/dev/null) || true
  if [ -z "$resp" ]; then
    printf "  ? %-42s TIMEOUT\n" "$label"
    ERR=$((ERR+1)); return
  fi
  http_status=$(echo "$resp" | tail -1)
  local body
  body=$(echo "$resp" | sed '$d')

  # For V8 security tests, the expected outcome is:
  #   - Server responds (didn't crash) with 500 + timeout/OOM error
  #   - OR handler catches the error and returns {passed: true}
  local js_passed
  js_passed=$(echo "$body" | python3 -c "import json,sys; d=json.load(sys.stdin); print('1' if d.get('passed') else '0')" 2>/dev/null) || true

  if [ "$js_passed" = "1" ]; then
    printf "  PASS %-40s\n" "$test_id"; PASS=$((PASS+1))
  elif [ "$http_status" = "500" ]; then
    # 500 = server survived the attack and returned a graceful error (PASS)
    printf "  PASS %-40s (500 — server survived)\n" "$test_id"; PASS=$((PASS+1))
  else
    printf "  FAIL %-40s (HTTP %s)\n" "$test_id" "$http_status"; FAIL=$((FAIL+1))
  fi
}
test_ep_v8sec "v8-timeout" "RT-V8-TIMEOUT" GET  "$BASE/handlers/canary/rt/v8/timeout"
test_ep_v8sec "v8-heap"    "RT-V8-HEAP"    GET  "$BASE/handlers/canary/rt/v8/heap"

# ── Summary ──────────────────────────────────────────────────────

echo ""
echo "  ────────────────────────────────────────────────"
TOTAL=$((PASS+FAIL+ERR))
echo "  Pass: $PASS  Fail: $FAIL  Error/Timeout: $ERR  Total: $TOTAL"
echo ""
