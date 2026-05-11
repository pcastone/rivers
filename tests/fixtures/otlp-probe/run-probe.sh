#!/usr/bin/env bash
# OTLP end-to-end smoke probe (CB-OTLP Track O5.6).
#
# Targets the NEW `view_type = "OTLP"` dispatcher (PR #117) — this is the
# inverse of CB's `cb-rivers-otlp-feature-request/run-probe.sh`, which
# targeted their REST-workaround bundle. Their probe expected JSON to
# pass and protobuf/gzip to fail. Ours expects all three (and more) to
# pass once Tracks O2+O3+O5 are merged.
#
# Scenarios:
#   1. JSON (uncompressed)            → expect 200 {"ingested": 1, ...}
#   2. JSON + Content-Encoding: gzip  → expect 200 {"ingested": 1, ...}
#   3. JSON + Content-Encoding: deflate → expect 200 {"ingested": 1, ...}
#   4. protobuf (empty body)          → expect 200 {"ingested": 0, ...}
#   5. partialSuccess (handler-side reject) → expect 200 {"partialSuccess":{...}}
#   6. Oversized body (3 MB > 2 MB cap)    → expect 413
#   7. Unsupported Content-Encoding (br)   → expect 415
#   8. Unknown signal path (/otlp/v1/wat)  → expect 404
#   9. Missing handler (/otlp/v1/logs on metrics-only — not in this bundle)
#                                          → covered by integration tests
#
# Exit codes:
#   0  All scenarios passed
#   1  At least one scenario failed
#   2  Environment problem (couldn't boot riversd, port collision, …)
#
# Usage:
#   tests/fixtures/otlp-probe/run-probe.sh
#
# Honors env vars:
#   RIVERSD       — path to the riversd binary (default: target/debug/riversd)
#   RIVERPACKAGE  — path to riverpackage     (default: target/debug/riverpackage)
#   PORT          — listen port              (default: 8197)
#   HOST          — listen host              (default: 127.0.0.1)
#   KEEP_TMP      — set to keep the temp work dir for inspection

set -euo pipefail
cd "$(dirname "$0")"
PROBE_DIR="$(pwd)"

WORKSPACE_ROOT="$(cd "$PROBE_DIR/../../.." && pwd)"
RIVERSD="${RIVERSD:-$WORKSPACE_ROOT/target/debug/riversd}"
RIVERPACKAGE="${RIVERPACKAGE:-$WORKSPACE_ROOT/target/debug/riverpackage}"
PORT="${PORT:-8197}"
HOST="${HOST:-127.0.0.1}"
# Routes are namespaced by bundle/entry-point at runtime:
#   /<bundle_name>/<entry_point>/<view_path>
# Bundle = otlp-smoke; entry_point = otlp (from app manifest); view path = /otlp.
# This is a known v1 wart for OTLP — real OTel clients post to a fixed
# `/v1/<signal>` URL, so production deployments will need a `route_prefix`
# override or a reverse-proxy rewrite. Documented in tests/fixtures/otlp-probe/README.md.
BASE="https://$HOST:$PORT/otlp-smoke/otlp/otlp"

# Working files in a per-run tmp dir so concurrent runs don't collide.
WORK="$(mktemp -d -t otlp-probe.XXXXXX)"
BUNDLE_COPY="$WORK/bundle"
CFG="$WORK/riversd.toml"
LOG="$WORK/riversd.log"
RESP="$WORK/response"

cleanup() {
    if [[ -n "${RIVERS_PID:-}" ]]; then
        kill "$RIVERS_PID" 2>/dev/null || true
        wait "$RIVERS_PID" 2>/dev/null || true
    fi
    if [[ -z "${KEEP_TMP:-}" ]]; then
        rm -rf "$WORK"
    else
        echo "WORK preserved at: $WORK"
    fi
}
trap cleanup EXIT INT TERM

# ── Prechecks ────────────────────────────────────────────────────
if [[ ! -x "$RIVERSD" ]]; then
    echo "ERROR: riversd not built or not executable at $RIVERSD"
    echo "       Build with: cargo build -p riversd"
    exit 2
fi
if [[ ! -x "$RIVERPACKAGE" ]]; then
    echo "ERROR: riverpackage not built or not executable at $RIVERPACKAGE"
    echo "       Build with: cargo build -p riverpackage"
    exit 2
fi

# Copy the bundle so the probe's data/ doesn't pollute the repo.
cp -R "$PROBE_DIR/bundle" "$BUNDLE_COPY"
mkdir -p "$BUNDLE_COPY/otlp/data"

echo "▶ validating bundle (Layer 1-3)"
"$RIVERPACKAGE" validate "$BUNDLE_COPY" 2>&1 | tail -5
echo ""

cat > "$CFG" <<EOF
bundle_path = "$BUNDLE_COPY/"
[base]
host      = "$HOST"
port      = $PORT
log_level = "info"

[base.tls]
# No cert/key paths so riversd auto-generates a self-signed cert.
# redirect=false skips the port-80 HTTP-to-HTTPS redirect listener.
redirect = false

[storage_engine]
backend = "memory"
EOF

echo "▶ booting riversd ($RIVERSD --config $CFG, log: $LOG)"
"$RIVERSD" --config "$CFG" > "$LOG" 2>&1 &
RIVERS_PID=$!

# Wait up to 15s for the HTTPS listener to come up (riversd auto-gens a
# self-signed cert when no cert paths are configured — curl -k accepts).
READY=0
for _ in $(seq 1 30); do
    if curl -ks "https://$HOST:$PORT/health" >/dev/null 2>&1; then
        READY=1
        break
    fi
    sleep 0.5
done
if (( READY == 0 )); then
    echo "ERROR: riversd did not become ready within 15s. Log:"
    tail -40 "$LOG"
    exit 2
fi

# ── Test payloads ────────────────────────────────────────────────
OTLP_METRICS=$(cat <<'JSON'
{
  "resourceMetrics": [{
    "resource": {"attributes": []},
    "scopeMetrics": [{
      "scope": {"name": "probe"},
      "metrics": [{
        "name": "probe.counter",
        "sum": {
          "dataPoints": [{"asInt": "1", "timeUnixNano": "1778500000000000000"}],
          "aggregationTemporality": 2,
          "isMonotonic": true
        }
      }]
    }]
  }]
}
JSON
)

OTLP_LOGS=$(cat <<'JSON'
{
  "resourceLogs": [{
    "resource": {"attributes": []},
    "scopeLogs": [{
      "scope": {"name": "probe"},
      "logRecords": [{"severityNumber": 9, "body": {"stringValue": "hello"}}]
    }]
  }]
}
JSON
)

OTLP_TRACES=$(cat <<'JSON'
{
  "resourceSpans": [{
    "resource": {"attributes": []},
    "scopeSpans": [{
      "scope": {"name": "probe"},
      "spans": [{
        "traceId": "00000000000000000000000000000001",
        "spanId":  "0000000000000001",
        "name":    "probe.span",
        "startTimeUnixNano": "1778500000000000000",
        "endTimeUnixNano":   "1778500000000001000"
      }]
    }]
  }]
}
JSON
)

# Reject payload — handler returns rejected=1 + errorMessage when it sees this.
PROBE_REJECT='{"_probe_reject": true, "resourceMetrics": []}'

# ── Test runner ──────────────────────────────────────────────────
TOTAL=0
PASSED=0
FAILED_NAMES=""

run_test() {
    local name="$1"; shift
    local expected_status="$1"; shift
    local body_check="$1"; shift
    # remaining args are curl args. -H "Expect:" suppresses HTTP/1.1
    # 100-continue probing so `%{http_code}` reports the final status,
    # not the intermediate 100 (curl 7.x defaults to Expect: 100-continue
    # for POSTs over a threshold).
    TOTAL=$((TOTAL + 1))
    local http
    http=$(curl -ks -o "$RESP" -w "%{http_code}" -H "Expect:" "$@" || true)
    local body
    body="$(cat "$RESP" 2>/dev/null || true)"
    local ok=1
    if [[ "$http" != "$expected_status" ]]; then
        ok=0
    fi
    if [[ -n "$body_check" ]] && ! echo "$body" | grep -q "$body_check"; then
        ok=0
    fi
    if (( ok == 1 )); then
        PASSED=$((PASSED + 1))
        printf "  [PASS] %-46s HTTP %s  body=%s\n" "$name" "$http" "$(echo "$body" | head -c 80)"
    else
        FAILED_NAMES="$FAILED_NAMES $name"
        printf "  [FAIL] %-46s HTTP %s (expected %s)  body=%s\n" "$name" "$http" "$expected_status" "$(echo "$body" | head -c 200)"
    fi
}

echo "▶ exercising OTLP scenarios"
echo ""

# Per OTLP/HTTP spec §7.1 (rivers-otlp-view-spec), framework wraps a
# successful handler return (no `rejected`) to `200 {}`. We assert the
# empty-success envelope rather than the handler's internal counters.

# 1. JSON metrics → 200 {}
run_test "01 JSON metrics → 200 {}" 200 '^{}$' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: application/json" \
    --data "$OTLP_METRICS"

# 2. JSON logs → 200 {}
run_test "02 JSON logs → 200 {}" 200 '^{}$' \
    -X POST "$BASE/v1/logs" \
    -H "Content-Type: application/json" \
    --data "$OTLP_LOGS"

# 3. JSON traces → 200 {}
run_test "03 JSON traces → 200 {}" 200 '^{}$' \
    -X POST "$BASE/v1/traces" \
    -H "Content-Type: application/json" \
    --data "$OTLP_TRACES"

# 4. Gzipped JSON
GZIPPED="$WORK/metrics.gz"
echo -n "$OTLP_METRICS" | gzip > "$GZIPPED"
run_test "04 gzip metrics → 200 {}" 200 '^{}$' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: application/json" \
    -H "Content-Encoding: gzip" \
    --data-binary "@$GZIPPED"

# 5. Deflated JSON. Per RFC 9110 §8.4.1.2, Content-Encoding `deflate`
# means zlib format (RFC 1950) — zlib.compress() produces exactly that.
DEFLATED="$WORK/metrics.deflate"
python3 -c "import sys, zlib; sys.stdout.buffer.write(zlib.compress(sys.stdin.buffer.read()))" \
    <<< "$OTLP_METRICS" > "$DEFLATED"
run_test "05 deflate metrics → 200 {}" 200 '^{}$' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: application/json" \
    -H "Content-Encoding: deflate" \
    --data-binary "@$DEFLATED"

# 6. Protobuf — empty body is a valid ExportMetricsServiceRequest (all
# fields default; decodes to {resourceMetrics: []}).
EMPTY_PROTO="$WORK/empty.proto"
: > "$EMPTY_PROTO"  # zero bytes
run_test "06 protobuf empty → 200 {}" 200 '^{}$' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: application/x-protobuf" \
    --data-binary "@$EMPTY_PROTO"

# 7. Handler-side partial success
run_test "07 partial success → 200 {partialSuccess:{rejectedDataPoints:1}}" 200 '"rejectedDataPoints":1' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: application/json" \
    --data "$PROBE_REJECT"

# 8. Oversized body (3 MB vs max_body_mb=2). Framework rejects with 413
# and "body exceeds OTLP size limit" before reading the full body.
BIG="$WORK/big.json"
python3 -c "print('{\"x\":\"' + ('a' * (3 * 1024 * 1024)) + '\"}', end='')" > "$BIG"
run_test "08 oversized (3MB > 2MB cap) → 413" 413 'exceeds OTLP size limit' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: application/json" \
    --data-binary "@$BIG"

# 9. Unsupported Content-Encoding
run_test "09 Content-Encoding: br → 415" 415 'not supported' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: application/json" \
    -H "Content-Encoding: br" \
    --data "$OTLP_METRICS"

# 10. Unknown signal path → 404 (router catchall)
run_test "10 unknown signal /v1/wat → 404" 404 "" \
    -X POST "$BASE/v1/wat" \
    -H "Content-Type: application/json" \
    --data '{}'

# 11. Unsupported Content-Type
run_test "11 Content-Type: text/plain → 415" 415 'application/json' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: text/plain" \
    --data "hello"

# 12. Malformed JSON
run_test "12 malformed JSON → 400" 400 'JSON parse failed' \
    -X POST "$BASE/v1/metrics" \
    -H "Content-Type: application/json" \
    --data '{not-json'

# ── Summary ──────────────────────────────────────────────────────
echo ""
echo "─── Summary ───"
echo "  passed: $PASSED / $TOTAL"
if (( PASSED == TOTAL )); then
    echo ""
    echo "RESULT: FEATURE WORKING (all $TOTAL scenarios pass)"
    exit 0
else
    echo ""
    echo "RESULT: REGRESSION — failed:$FAILED_NAMES"
    echo "Server log tail:"
    tail -40 "$LOG" | sed 's/^/    /'
    exit 1
fi
