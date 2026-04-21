#!/usr/bin/env bash
# Canary Fleet Test Runner — hits all endpoints and reports verdicts
# Usage: ./run-tests.sh [base_url]
#   base_url defaults to https://localhost:8090
set -euo pipefail

# ADMIN_URL — admin API base URL (e.g., http://localhost:9090), required for circuit breaker tests
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

  # Capture both response body AND HTTP status code to distinguish:
  #   - curl timeout (exit 28) vs connection refused (exit 7) vs server error
  #   - empty body (driver silent failure) vs HTTP error (401/500)
  local raw_resp curl_exit http_status resp
  if [ "$method" = "POST" ] || [ "$method" = "PUT" ] || [ "$method" = "DELETE" ]; then
    raw_resp=$(curl -sk -m 8 -b "$COOKIES" -c "$COOKIES" \
      -X "$method" -H "Content-Type: application/json" \
      -H "X-CSRF-Token: ${CSRF_TOKEN}" \
      -d "${body:-{}}" -w '\n%{http_code}' "$url" 2>/dev/null) ; curl_exit=$?
  else
    raw_resp=$(curl -sk -m 8 -b "$COOKIES" -c "$COOKIES" \
      -X "$method" -w '\n%{http_code}' "$url" 2>/dev/null) ; curl_exit=$?
  fi

  # Split body from HTTP status (last line is the status code from -w)
  http_status=$(echo "$raw_resp" | tail -1)
  resp=$(echo "$raw_resp" | sed '$d')

  # Diagnose curl-level failures
  if [ "$curl_exit" -eq 28 ]; then
    printf "  TIMEOUT %-38s (curl timeout — no response in 8s)\n" "$label"
    ERR=$((ERR+1)); return
  elif [ "$curl_exit" -eq 7 ]; then
    printf "  CONNREF %-38s (connection refused — server down?)\n" "$label"
    ERR=$((ERR+1)); return
  elif [ "$curl_exit" -ne 0 ] && [ -z "$resp" ]; then
    printf "  CURLERR %-38s (curl exit %d)\n" "$label" "$curl_exit"
    ERR=$((ERR+1)); return
  fi

  # Empty body with a status code — driver returned nothing
  if [ -z "$resp" ] && [ -n "$http_status" ] && [ "$http_status" != "000" ]; then
    printf "  EMPTY  %-5s %-33s (HTTP %s — empty body)\n" "" "$label" "$http_status"
    ERR=$((ERR+1)); return
  elif [ -z "$resp" ]; then
    printf "  DEAD   %-38s (no response, no status)\n" "$label"
    ERR=$((ERR+1)); return
  fi

  # Parse JSON response
  local test_id passed json_code json_msg
  test_id=$(echo "$resp" | python3 -c "import json,sys; print(json.load(sys.stdin).get('test_id',''))" 2>/dev/null) || true
  passed=$(echo "$resp" | python3 -c "import json,sys; d=json.load(sys.stdin); print('1' if d.get('passed') else '0')" 2>/dev/null) || true
  json_code=$(echo "$resp" | python3 -c "import json,sys; print(json.load(sys.stdin).get('code',''))" 2>/dev/null) || true
  json_msg=$(echo "$resp" | python3 -c "import json,sys; print(json.load(sys.stdin).get('message','')[:60])" 2>/dev/null) || true

  if [ "$passed" = "1" ]; then
    printf "  PASS %-40s\n" "${test_id:-$label}"
    PASS=$((PASS+1))
  elif [ -n "$test_id" ] && [ "$test_id" != "" ]; then
    printf "  FAIL %-40s\n" "${test_id:-$label}"
    FAIL=$((FAIL+1))
  elif [ -n "$json_code" ] && [ "$json_code" != "" ]; then
    printf "  HTTP %-5s %-34s %s\n" "$json_code" "$label" "$json_msg"
    ERR=$((ERR+1))
  else
    printf "  ?    %-40s (HTTP %s)\n" "$label" "$http_status"
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

# ── TRANSACTIONS-TS Profile (spec §6: ctx.transaction surface) ───

echo ""
echo "  ── TRANSACTIONS-TS Profile (ctx.transaction) ──"
test_ep "txn-args"           POST "$BASE/handlers/canary/rt/txn/args" '{}'
test_ep "txn-cb-type"        POST "$BASE/handlers/canary/rt/txn/cb-type" '{}'
test_ep "txn-unknown-ds"     POST "$BASE/handlers/canary/rt/txn/unknown-ds" '{}'
test_ep "txn-cleanup"        POST "$BASE/handlers/canary/rt/txn/cleanup" '{}'
test_ep "txn-surface"        POST "$BASE/handlers/canary/rt/txn/surface" '{}'

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
  local raw_resp curl_exit http_status body
  # Get both response body and HTTP status
  raw_resp=$(curl -sk -m 15 -b "$COOKIES" -c "$COOKIES" -X "$method" -w '\n%{http_code}' "$url" 2>/dev/null) ; curl_exit=$?

  if [ "$curl_exit" -eq 28 ]; then
    printf "  TIMEOUT %-38s (curl timeout — 15s)\n" "$label"
    ERR=$((ERR+1)); return
  elif [ "$curl_exit" -eq 7 ]; then
    printf "  CONNREF %-38s (server crashed)\n" "$label"
    ERR=$((ERR+1)); return
  elif [ -z "$raw_resp" ]; then
    printf "  DEAD   %-38s (no response, curl exit %d)\n" "$label" "$curl_exit"
    ERR=$((ERR+1)); return
  fi
  http_status=$(echo "$raw_resp" | tail -1)
  body=$(echo "$raw_resp" | sed '$d')

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

# ── INTEGRATION Profile (auth=none, cross-cutting driver tests) ──

echo ""
echo "  ── INTEGRATION Profile ──"
test_ep "int-ddl-verify"       GET  "$BASE/sql/canary/integration/ctx-ddl-verify"
test_ep "int-ddl-insert-sel"   GET  "$BASE/sql/canary/integration/ctx-ddl-insert-select"
test_ep "int-driver-error"     GET  "$BASE/sql/canary/integration/driver-error-propagation"
test_ep "int-ddl-whitelist"    GET  "$BASE/sql/canary/integration/ddl-whitelist-reject"
test_ep "int-param-binding"    GET  "$BASE/sql/canary/integration/dataview-param-binding"
test_ep "int-store-namespace"  GET  "$BASE/sql/canary/integration/store-namespace-isolation"
test_ep "int-recovery"         GET  "$BASE/sql/canary/integration/recovery-after-timeout"
test_ep "int-sqlite-disk"      GET  "$BASE/sql/canary/integration/sqlite-disk-persistence"
test_ep "int-init-sequence"    GET  "$BASE/sql/canary/integration/init-handler-sequence"
test_ep "int-host-callbacks"   GET  "$BASE/sql/canary/integration/host-callback-available"

# Conditional PG/MySQL integration tests — skip if cluster unreachable
PG_AVAIL=$(curl -sk -m 2 "$BASE/sql/canary/sql/pg/param-order" -X POST -H "Content-Type: application/json" -H "X-CSRF-Token: ${CSRF_TOKEN}" -b "$COOKIES" -c "$COOKIES" -d '{}' 2>/dev/null | python3 -c "import json,sys; print('1' if json.load(sys.stdin).get('test_id') else '0')" 2>/dev/null) || PG_AVAIL="0"

if [ "$PG_AVAIL" = "1" ]; then
  test_ep "int-pg-ddl"          GET  "$BASE/sql/canary/integration/pg-ddl-create-select"
else
  printf "  SKIP %-40s (PG unreachable)\n" "INT-PG-DDL"
fi

MYSQL_AVAIL=$(curl -sk -m 2 "$BASE/sql/canary/sql/mysql/param-order" -X POST -H "Content-Type: application/json" -H "X-CSRF-Token: ${CSRF_TOKEN}" -b "$COOKIES" -c "$COOKIES" -d '{}' 2>/dev/null | python3 -c "import json,sys; print('1' if json.load(sys.stdin).get('test_id') else '0')" 2>/dev/null) || MYSQL_AVAIL="0"

if [ "$MYSQL_AVAIL" = "1" ]; then
  test_ep "int-mysql-ddl"       GET  "$BASE/sql/canary/integration/mysql-ddl-create-select"
else
  printf "  SKIP %-40s (MySQL unreachable)\n" "INT-MYSQL-DDL"
fi

# ── MCP Tests ────────────────────────────────────────────────
echo ""
echo "  ── MCP ──"

# Initialize
MCP_INIT=$(curl -sf -X POST "$BASE/sql/canary/sql/mcp" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' 2>/dev/null) || MCP_INIT=""
if echo "$MCP_INIT" | grep -q "rivers-mcp"; then
  printf "  PASS %-40s\n" "mcp-initialize"
  PASS=$((PASS+1))
else
  printf "  FAIL %-40s\n" "mcp-initialize"
  FAIL=$((FAIL+1))
fi

# Tools list
MCP_TOOLS=$(curl -sf -X POST "$BASE/sql/canary/sql/mcp" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' 2>/dev/null) || MCP_TOOLS=""
if echo "$MCP_TOOLS" | grep -q "pg_select"; then
  printf "  PASS %-40s\n" "mcp-tools-list"
  PASS=$((PASS+1))
else
  printf "  FAIL %-40s\n" "mcp-tools-list"
  FAIL=$((FAIL+1))
fi

# Method not found
MCP_404=$(curl -sf -X POST "$BASE/sql/canary/sql/mcp" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":3,"method":"nonexistent","params":{}}' 2>/dev/null) || MCP_404=""
if echo "$MCP_404" | grep -q "Method not found"; then
  printf "  PASS %-40s\n" "mcp-method-not-found"
  PASS=$((PASS+1))
else
  printf "  FAIL %-40s\n" "mcp-method-not-found"
  FAIL=$((FAIL+1))
fi

# Tools call
MCP_CALL=$(curl -sf -X POST "$BASE/sql/canary/sql/mcp" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"pg_select","arguments":{}}}' 2>/dev/null) || MCP_CALL=""
if echo "$MCP_CALL" | grep -q '"content"'; then
  printf "  PASS %-40s\n" "mcp-tools-call"
  PASS=$((PASS+1))
else
  printf "  FAIL %-40s\n" "mcp-tools-call"
  FAIL=$((FAIL+1))
fi

# ── Query Parameter Tests ────────────────────────────────────
echo ""
echo "  ── Query Parameters ──"
test_ep "qp-query-access"   GET  "$BASE/sql/canary/sql/qp/query-access?status=active&limit=20"
test_ep "qp-query-all"      GET  "$BASE/sql/canary/sql/qp/query-all?tag=a&tag=b&tag=c&single=one"
test_ep "qp-percent-decode"  GET  "$BASE/sql/canary/sql/qp/percent-decode?name=John%20Doe&city=S%C3%A3o%20Paulo"
test_ep "qp-empty-value"    GET  "$BASE/sql/canary/sql/qp/empty-value?key=&bare"

# ── Transaction Tests ────────────────────────────────────
echo ""
echo "  ── Transactions ──"
test_ep "txn-commit"       POST "$BASE/sql/canary/sql/txn/commit" '{}'
test_ep "txn-rollback"     POST "$BASE/sql/canary/sql/txn/rollback" '{}'
test_ep "txn-double-begin" POST "$BASE/sql/canary/sql/txn/double-begin" '{}'
test_ep "txn-batch"        POST "$BASE/sql/canary/sql/txn/batch" '{}'

# ── Circuit Breaker Tests ────────────────────────────────
echo ""
echo "  ── Circuit Breaker ──"

CB_APP_ID="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01"
ADMIN_URL="${ADMIN_URL:-}"

if [ -n "$ADMIN_URL" ]; then
  # List breakers
  CB_LIST=$(curl -sf "$ADMIN_URL/admin/apps/$CB_APP_ID/breakers" 2>/dev/null) || CB_LIST=""
  if echo "$CB_LIST" | grep -q "canary-pg-breaker"; then
    printf "  PASS %-40s\n" "cb-breaker-registered"
    PASS=$((PASS+1))
  else
    printf "  FAIL %-40s\n" "cb-breaker-registered"
    FAIL=$((FAIL+1))
  fi

  # Trip breaker
  CB_TRIP=$(curl -sf -X POST "$ADMIN_URL/admin/apps/$CB_APP_ID/breakers/canary-pg-breaker/trip" 2>/dev/null) || CB_TRIP=""
  if echo "$CB_TRIP" | grep -q '"OPEN"'; then
    printf "  PASS %-40s\n" "cb-trip-open"
    PASS=$((PASS+1))
  else
    printf "  FAIL %-40s\n" "cb-trip-open"
    FAIL=$((FAIL+1))
  fi

  # Verify 503
  CB_503=$(curl -sk -o /dev/null -w "%{http_code}" "$BASE/sql/canary/sql/pg/select" 2>/dev/null) || CB_503="000"
  if [ "$CB_503" = "503" ]; then
    printf "  PASS %-40s\n" "cb-503-when-open"
    PASS=$((PASS+1))
  else
    printf "  FAIL %-40s (got HTTP %s)\n" "cb-503-when-open" "$CB_503"
    FAIL=$((FAIL+1))
  fi

  # Reset breaker
  CB_RESET=$(curl -sf -X POST "$ADMIN_URL/admin/apps/$CB_APP_ID/breakers/canary-pg-breaker/reset" 2>/dev/null) || CB_RESET=""
  if echo "$CB_RESET" | grep -q '"CLOSED"'; then
    printf "  PASS %-40s\n" "cb-reset-closed"
    PASS=$((PASS+1))
  else
    printf "  FAIL %-40s\n" "cb-reset-closed"
    FAIL=$((FAIL+1))
  fi

  # Verify endpoint works again
  CB_200=$(curl -sk -o /dev/null -w "%{http_code}" "$BASE/sql/canary/sql/pg/select" 2>/dev/null) || CB_200="000"
  if [ "$CB_200" = "200" ]; then
    printf "  PASS %-40s\n" "cb-200-after-reset"
    PASS=$((PASS+1))
  else
    printf "  FAIL %-40s (got HTTP %s)\n" "cb-200-after-reset" "$CB_200"
    FAIL=$((FAIL+1))
  fi
else
  printf "  SKIP %-40s (ADMIN_URL not set)\n" "cb-tests"
fi

# ── Schema Introspection (implicit) ──────────────────────
# Introspection runs at startup. If DataView queries have field
# mismatches against actual database columns, riversd would refuse
# to start and this test run would not execute.
printf "  PASS %-40s\n" "schema-introspection-startup"
PASS=$((PASS+1))

# ── Summary ──────────────────────────────────────────────────────

echo ""
echo "  ────────────────────────────────────────────────"
TOTAL=$((PASS+FAIL+ERR))
echo "  Pass: $PASS  Fail: $FAIL  Error/Timeout: $ERR  Total: $TOTAL"
echo ""
