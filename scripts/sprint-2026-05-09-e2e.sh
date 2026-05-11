#!/usr/bin/env bash
#
# End-to-end test harness for Sprint 2026-05-09 (CB unblock).
#
# Exercises the deliverables of all three tracks against the actual
# `riverpackage` CLI — i.e., the same binary CB will run from their
# probe — not the Rust lib internals.
#
# Companion: crates/riversd/tests/sprint_2026_05_09_e2e.rs (lib-level
# tests, including the Cron scheduler tick-fire + dedupe tests).
#
# Usage:
#   ./scripts/sprint-2026-05-09-e2e.sh
#
# Exit code 0 on full pass, non-zero on first failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RVP="$ROOT/target/release/riverpackage"

# ── Colors (only when stdout is a tty) ─────────────────────────────
if [[ -t 1 ]]; then
    R="$(printf '\033[31m')"; G="$(printf '\033[32m')"
    Y="$(printf '\033[33m')"; B="$(printf '\033[34m')"
    N="$(printf '\033[0m')"
else
    R=""; G=""; Y=""; B=""; N=""
fi

PASS=0; FAIL=0; CASES=()

pass() { CASES+=("${G}PASS${N} $*"); PASS=$((PASS+1)); }
fail() { CASES+=("${R}FAIL${N} $*"); FAIL=$((FAIL+1)); }

build_riverpackage() {
    echo "${B}# Building riverpackage (release)…${N}"
    (cd "$ROOT" && cargo build --release -p riverpackage --quiet) \
        || { echo "${R}cargo build failed${N}"; exit 2; }
    [[ -x "$RVP" ]] || { echo "${R}$RVP not executable${N}"; exit 2; }
}

# Build a minimal valid bundle in $1, with $2 spliced into app.toml.
write_bundle() {
    local dir="$1"; local fragment="$2"
    rm -rf "$dir"
    mkdir -p "$dir/test-app"
    cat > "$dir/manifest.toml" <<EOF
bundleName = "e2e"
bundleVersion = "1.0.0"
source = "https://example.invalid/e2e"
apps = ["test-app"]
EOF
    cat > "$dir/test-app/manifest.toml" <<EOF
appName = "test-app"
version = "1.0.0"
type = "app-service"
appId = "00000000-0000-0000-0000-000000000001"
entryPoint = "test-app"
source = "https://example.invalid/e2e"
EOF
    cat > "$dir/test-app/resources.toml" <<EOF
[[datasources]]
name = "data"
driver = "faker"
x-type = "faker"
required = true
nopassword = true
EOF
    {
        cat <<EOF
[data.dataviews.items]
name = "items"
datasource = "data"
query = "SELECT 1"

EOF
        echo "$fragment"
    } > "$dir/test-app/app.toml"
}

# Run riverpackage against $1 (bundle dir). Returns 0 on validate success,
# else writes the validator stderr to stdout for the caller to grep.
validate() {
    local dir="$1"
    "$RVP" validate "$dir" 2>&1
}

assert_clean() {
    local label="$1"; local dir="$2"
    local out; out="$(validate "$dir" || true)"
    if grep -qE "RESULT: 0 errors" <<< "$out"; then
        pass "$label"
    else
        fail "$label — expected clean validate; got:"
        echo "$out" | grep -E "RESULT|FAIL" | sed 's/^/      /' >&2
    fi
}

assert_rejects() {
    local label="$1"; local dir="$2"; local pattern="$3"
    local out; out="$(validate "$dir" || true)"
    if grep -qE "$pattern" <<< "$out"; then
        pass "$label"
    else
        fail "$label — expected rejection matching /$pattern/; got:"
        echo "$out" | grep -E "RESULT|FAIL|S005" | sed 's/^/      /' >&2
    fi
}

# ── Cases ─────────────────────────────────────────────────────────

run_track2_validator_hardening() {
    echo
    echo "${B}═══ Track 2: validator hardening (auth + view_type enums) ═══${N}"

    # G — auth = "bearer" (P1.12 closed-as-superseded) → S005
    write_bundle "/tmp/sprint-e2e-G" '
[api.views.case_g]
path      = "/x"
method    = "GET"
view_type = "Rest"
auth      = "bearer"

[api.views.case_g.handler]
type = "dataview"
dataview = "items"
'
    assert_rejects "Track 2.G — auth='bearer' rejected" \
        "/tmp/sprint-e2e-G" \
        "auth 'bearer' is not one of \[none, session\]"

    # I — view_type = "QuantumStreamer" → S005 with canonical set incl. Cron
    write_bundle "/tmp/sprint-e2e-Ibad" '
[api.views.case_i]
path      = "/x"
method    = "GET"
view_type = "QuantumStreamer"
auth      = "none"

[api.views.case_i.handler]
type = "dataview"
dataview = "items"
'
    assert_rejects "Track 2.I-bad — view_type='QuantumStreamer' rejected listing Cron" \
        "/tmp/sprint-e2e-Ibad" \
        "view_type 'QuantumStreamer' is not one of .*Cron"

    # Cron-only fields on Rest view → S005 each
    write_bundle "/tmp/sprint-e2e-cronfields" '
[api.views.case_cf]
path             = "/x"
method           = "GET"
view_type        = "Rest"
auth             = "none"
schedule         = "0 */5 * * * *"
interval_seconds = 60

[api.views.case_cf.handler]
type = "dataview"
dataview = "items"
'
    assert_rejects "Track 2.X — schedule on Rest view rejected" \
        "/tmp/sprint-e2e-cronfields" \
        "\.schedule is only valid when view_type=\"Cron\""
    assert_rejects "Track 2.X — interval_seconds on Rest view rejected" \
        "/tmp/sprint-e2e-cronfields" \
        "\.interval_seconds is only valid when view_type=\"Cron\""
}

run_track3_cron_canonical() {
    echo
    echo "${B}═══ Track 3: Cron view canonical TOML accepts ═══${N}"

    # I — view_type = "Cron" canonical (post Track 3) → clean
    write_bundle "/tmp/sprint-e2e-Igood" '
[api.views.case_i]
view_type        = "Cron"
schedule         = "0 */5 * * * *"
overlap_policy   = "skip"

[api.views.case_i.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
'
    # The handler module file does not exist on disk in this synthetic
    # bundle — Layer 2 (existence) will flag it but Layer 1 (structural)
    # is the load-bearing assertion. Filter out E001 missing-file noise.
    local out
    out="$("$RVP" validate "/tmp/sprint-e2e-Igood" --format json 2>/dev/null || true)"
    # Filter out E001 (file-not-found) — Layer 2 noise from synthetic
    # bundle that doesn't have the handler module on disk. Look for any
    # S00x error which would indicate the structural layer rejected the
    # Cron shape itself.
    if echo "$out" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); fails=[r for r in d.get('results',[]) if r.get('status')=='fail' and (r.get('error_code') or '').startswith('S')]; sys.exit(1 if fails else 0)" 2>/dev/null; then
        pass "Track 3.I — canonical Cron view accepted at structural layer"
    else
        fail "Track 3.I — canonical Cron view emitted S00x unexpectedly"
        echo "$out" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); [print('     ',r.get('error_code'),r.get('message')) for r in d.get('results',[]) if r.get('status')=='fail']" 2>/dev/null >&2
    fi

    # Cron view with both schedule and interval_seconds → S005 mutex
    write_bundle "/tmp/sprint-e2e-cronmutex" '
[api.views.case_i]
view_type        = "Cron"
schedule         = "0 */5 * * * *"
interval_seconds = 300

[api.views.case_i.handler]
type       = "codecomponent"
language   = "typescript"
module     = "h.ts"
entrypoint = "t"
resources  = []
'
    assert_rejects "Track 3.mutex — schedule+interval_seconds together rejected" \
        "/tmp/sprint-e2e-cronmutex" \
        "declares both"

    # Cron view with neither → S005
    write_bundle "/tmp/sprint-e2e-cronnone" '
[api.views.case_i]
view_type = "Cron"

[api.views.case_i.handler]
type       = "codecomponent"
language   = "typescript"
module     = "h.ts"
entrypoint = "t"
resources  = []
'
    assert_rejects "Track 3.none — Cron view without schedule or interval rejected" \
        "/tmp/sprint-e2e-cronnone" \
        "requires exactly one of"

    # Cron view with path/method/auth → 3× S005
    write_bundle "/tmp/sprint-e2e-cronforbidden" '
[api.views.case_i]
view_type = "Cron"
schedule  = "0 */5 * * * *"
path      = "/oops"
method    = "POST"
auth      = "session"

[api.views.case_i.handler]
type       = "codecomponent"
language   = "typescript"
module     = "h.ts"
entrypoint = "t"
resources  = []
'
    for f in path method auth; do
        assert_rejects "Track 3.forbidden — $f on Cron view rejected" \
            "/tmp/sprint-e2e-cronforbidden" \
            "\.$f is not allowed on view_type=\"Cron\""
    done

    # Cron view with invalid cron expression → S005 schedule
    write_bundle "/tmp/sprint-e2e-cronbadexpr" '
[api.views.case_i]
view_type = "Cron"
schedule  = "every 5 minutes please"

[api.views.case_i.handler]
type       = "codecomponent"
language   = "typescript"
module     = "h.ts"
entrypoint = "t"
resources  = []
'
    assert_rejects "Track 3.parse — invalid cron expression rejected" \
        "/tmp/sprint-e2e-cronbadexpr" \
        "not a valid cron expression"

    # Cron view with bad overlap_policy → S005
    write_bundle "/tmp/sprint-e2e-cronbadoverlap" '
[api.views.case_i]
view_type      = "Cron"
schedule       = "0 */5 * * * *"
overlap_policy = "abandon"

[api.views.case_i.handler]
type       = "codecomponent"
language   = "typescript"
module     = "h.ts"
entrypoint = "t"
resources  = []
'
    assert_rejects "Track 3.overlap — overlap_policy='abandon' rejected" \
        "/tmp/sprint-e2e-cronbadoverlap" \
        "overlap_policy 'abandon' is not one of \[skip, queue, allow\]"
}

run_track1_probe_migration_shapes() {
    echo
    echo "${B}═══ Track 1: probe migration canonical shapes accept ═══${N}"

    # P1.10 named guard via guard_view (not guard string overload)
    write_bundle "/tmp/sprint-e2e-P110" '
[api.views.guard_target]
path      = "/internal/g"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.guard_target.handler]
type       = "codecomponent"
language   = "typescript"
module     = "h.ts"
entrypoint = "guard"
resources  = []

[api.views.protected]
path       = "/protected"
method     = "POST"
view_type  = "Mcp"
auth       = "none"
guard_view = "guard_target"

[api.views.protected.handler]
type = "none"
'
    local out
    out="$("$RVP" validate "/tmp/sprint-e2e-P110" --format json 2>/dev/null || true)"
    # X014 cross-ref will fire because the guard target's handler module
    # file doesn't exist on disk in this synthetic bundle (Layer 2). Filter
    # to S-codes only — that's the structural-acceptance assertion.
    if echo "$out" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); fails=[r for r in d.get('results',[]) if r.get('status')=='fail' and (r.get('error_code') or '').startswith('S')]; sys.exit(1 if fails else 0)" 2>/dev/null; then
        pass "Track 1.P1.10 — guard_view canonical shape accepted at structural layer"
    else
        fail "Track 1.P1.10 — guard_view canonical shape unexpectedly emitted S00x"
        echo "$out" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); [print('     ',r.get('error_code'),r.get('message')) for r in d.get('results',[]) if r.get('status')=='fail' and (r.get('error_code') or '').startswith('S')]" 2>/dev/null >&2
    fi

    # P1.11 response_headers flat (not [response.headers] nested)
    write_bundle "/tmp/sprint-e2e-P111" '
[api.views.legacy]
path      = "/legacy"
method    = "GET"
view_type = "Rest"
auth      = "none"

[api.views.legacy.response_headers]
"Deprecation" = "true"
"Sunset"      = "Wed, 01 Apr 2026 00:00:00 GMT"

[api.views.legacy.handler]
type = "dataview"
dataview = "items"
'
    out="$("$RVP" validate "/tmp/sprint-e2e-P111" --format json 2>/dev/null || true)"
    if echo "$out" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); fails=[r for r in d.get('results',[]) if r.get('status')=='fail' and 'response_headers' in (r.get('message') or '')]; sys.exit(1 if fails else 0)" 2>/dev/null; then
        pass "Track 1.P1.11 — response_headers (flat) accepted"
    else
        fail "Track 1.P1.11 — response_headers (flat) unexpectedly errored"
        echo "$out" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); [print('     ',r.get('error_code'),r.get('message')) for r in d.get('results',[]) if r.get('status')=='fail' and 'response_headers' in (r.get('message') or '')]" 2>/dev/null >&2
    fi

    # P1.11 reserved-header rejection (Track 1.P1.11.reserved)
    write_bundle "/tmp/sprint-e2e-P111-reserved" '
[api.views.legacy]
path      = "/legacy"
method    = "GET"
view_type = "Rest"
auth      = "none"

[api.views.legacy.response_headers]
"Content-Type" = "application/json"

[api.views.legacy.handler]
type = "dataview"
dataview = "items"
'
    assert_rejects "Track 1.P1.11.reserved — Content-Type rejected as framework-managed" \
        "/tmp/sprint-e2e-P111-reserved" \
        "framework-managed header"
}

# ── Run ───────────────────────────────────────────────────────────

build_riverpackage

run_track2_validator_hardening
run_track3_cron_canonical
run_track1_probe_migration_shapes

echo
echo "${B}═══ Sprint 2026-05-09 E2E Summary ═══${N}"
for c in "${CASES[@]}"; do echo "  $c"; done
echo
echo "  ${G}Passed${N}: $PASS"
echo "  ${R}Failed${N}: $FAIL"

# Cleanup tmpdirs
rm -rf /tmp/sprint-e2e-*

exit $FAIL
