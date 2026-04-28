#!/usr/bin/env bash
# scripts/lint-heuristics.sh — Code-review heuristic lint checks for CI.
#
# Enforces invariants from docs/review/rivers-wide-code-review-2026-04-27.md.
# Exits 1 when any check finds a violation.
#
# Usage:  ./scripts/lint-heuristics.sh [--verbose]
# Requires: rg (ripgrep)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERBOSE="${1:-}"
VIOLATIONS=0

log() { [[ -n "$VERBOSE" ]] && echo "  $*"; }

fail() {
    echo "LINT FAIL [$1]: $2" >&2
    VIOLATIONS=$((VIOLATIONS + 1))
}

pass() { echo "  OK  [$1]"; }

echo "=== Rivers heuristic lint ==="

# ── H1: check_schema callers ──────────────────────────────────────────────────
# Every broker plugin that defines `pub fn check_.*schema` must have a
# matching entry in rivers-runtime/src/validate_syntax.rs.  A plugin-level
# schema checker that is never called from validation is silently dead code.
echo "[H1] check_schema → validate_syntax.rs coverage"
SYNTAX="$ROOT/crates/rivers-runtime/src/validate_syntax.rs"
while IFS= read -r file; do
    crate_dir=$(dirname "$(dirname "$file")")
    driver=$(basename "$crate_dir" | sed 's/rivers-plugin-//')
    if ! grep -q "\"$driver\"" "$SYNTAX" 2>/dev/null; then
        fail H1 "$file defines check_schema but driver '$driver' has no entry in validate_syntax.rs"
    else
        log "H1: $driver OK"
    fi
done < <(rg 'pub fn check_.*schema' "$ROOT/crates/rivers-plugin-"* -l 2>/dev/null || true)
pass H1

# ── H2: ddl_execute overrides document their admin_operations ─────────────────
# The SDK default for ddl_execute returns Unsupported, so any crate relying
# on the default is safe.  The risk runs the other way: a crate that OVERRIDES
# ddl_execute with real logic must also declare admin_operations so the DDL
# guard in execute() can block those ops in user DataViews.
echo "[H2] ddl_execute overrides paired with admin_operations"
while IFS= read -r file; do
    crate_dir=$(dirname "$(dirname "$file")")
    crate=$(basename "$crate_dir")
    # Skip the SDK itself
    [[ "$crate" == "rivers-driver-sdk" ]] && continue
    if ! rg 'fn admin_operations' "$crate_dir/src/" -q 2>/dev/null; then
        fail H2 "$crate overrides ddl_execute but does not define admin_operations — document supported DDL ops"
    else
        log "H2: $crate OK"
    fi
done < <(rg 'async fn ddl_execute' "$ROOT/crates/rivers-plugin-"* -l 2>/dev/null || true)
pass H2

# ── H3: Client::new() in production source ────────────────────────────────────
# reqwest::Client::new() has no connect/request timeout.  Plugin and riversctl
# production code must use Client::builder() with explicit timeouts.
echo "[H3] Client::new() absent from production source"
VIOLATIONS_H3=0
while IFS= read -r match; do
    fail H3 "bare Client::new() in production source: $match"
    VIOLATIONS_H3=$((VIOLATIONS_H3 + 1))
done < <(rg 'Client::new\(\)' \
    "$ROOT/crates/rivers-plugin-"* \
    "$ROOT/crates/riversctl/src/" \
    --glob '!*/tests/*' \
    --glob '!*test*' \
    -n 2>/dev/null || true)
[[ $VIOLATIONS_H3 -eq 0 ]] && pass H3

# ── H4: unbounded response-body reads (per-file baseline) ─────────────────────
# .text() / .json() on an HTTP response reads the entire body into memory.
# Each occurrence must stay at or below the per-file baseline established at
# review time.  Update a baseline when intentionally adding a justified read.
echo "[H4] unbounded response reads (baseline check)"
H4_FAIL=0

check_h4() {
    local rel_path="$1"
    local baseline="$2"
    local file="$ROOT/$rel_path"
    [[ -f "$file" ]] || { log "H4: $rel_path not found — skipping"; return; }
    local count
    count=$(rg '\.json\(\)|\.text\(\)' "$file" -c 2>/dev/null || echo 0)
    if [[ "$count" -gt "$baseline" ]]; then
        fail H4 "$rel_path: $count unbounded response reads (baseline $baseline). Add a size-limited stream or update baseline with justification."
        H4_FAIL=$((H4_FAIL + 1))
    else
        log "H4: $rel_path $count/$baseline OK"
    fi
}

check_h4 "crates/rivers-plugin-couchdb/src/lib.rs"          9
check_h4 "crates/rivers-plugin-elasticsearch/src/lib.rs"    6
check_h4 "crates/rivers-plugin-influxdb/src/connection.rs"  3
check_h4 "crates/rivers-plugin-influxdb/src/batching.rs"    1

[[ $H4_FAIL -eq 0 ]] && pass H4

# ── H5: fs::write in secret / deploy crates ───────────────────────────────────
# fs::write is intentional in the lockbox CLI (manages encrypted secret files)
# and in cargo-deploy (writes config/cert files).  Baselines reflect the
# known-justified counts.  Any NEW write must be reviewed and the baseline
# updated with an explanation.
echo "[H5] fs::write baseline in secret/deploy crates"
H5_FAIL=0

check_h5() {
    local rel_path="$1"
    local baseline="$2"
    local file="$ROOT/$rel_path"
    [[ -f "$file" ]] || { log "H5: $rel_path not found — skipping"; return; }
    local count
    count=$(rg 'fs::write\(' "$file" -c 2>/dev/null || echo 0)
    if [[ "$count" -gt "$baseline" ]]; then
        fail H5 "$rel_path: $count fs::write calls (baseline $baseline). Review new write for secret-file safety, then update baseline."
        H5_FAIL=$((H5_FAIL + 1))
    else
        log "H5: $rel_path $count/$baseline OK"
    fi
}

check_h5 "crates/rivers-lockbox/src/main.rs"    8
check_h5 "crates/cargo-deploy/src/main.rs"       4

[[ $H5_FAIL -eq 0 ]] && pass H5

# ── H6: no derived Debug within 8 lines of a secret field ────────────────────
# Structs with a `key_material:` or `pub value:` secret field in secret crates
# must NOT use #[derive(Debug)] — that prints raw secret bytes in logs.
# They must implement fmt::Debug manually.
#
# Detection: if #[derive(Debug)] appears within 8 source lines BEFORE a secret
# field declaration, the struct owns that field and has derived Debug.
echo "[H6] no derived Debug on secret-bearing types"
H6_FAIL=0

check_h6_dir() {
    local src_dir="$1"
    [[ -d "$src_dir" ]] || return
    while IFS= read -r file; do
        # For each secret field, look back 8 lines for a derive(Debug)
        while IFS= read -r rg_line; do
            local line_no
            line_no=$(echo "$rg_line" | cut -d: -f1)
            [[ "$line_no" =~ ^[0-9]+$ ]] || continue
            local start_line=$(( line_no > 8 ? line_no - 8 : 1 ))
            # Check if #[derive(Debug)] appears in the preceding 8 lines
            if sed -n "${start_line},$((line_no-1))p" "$file" | grep -q '#\[derive(.*Debug'; then
                # Find struct name by scanning backwards from the field line
                local struct_name
                struct_name=$(sed -n "${start_line},$((line_no))p" "$file" | grep -E 'pub struct |pub enum ' | tail -1 | grep -oE '(struct|enum) [A-Za-z_]+' | awk '{print $2}')
                if [[ -z "$struct_name" ]]; then
                    struct_name="<unknown struct near line $line_no>"
                fi
                fail H6 "$file: '$struct_name' uses #[derive(Debug)] but contains a secret field (line $line_no). Implement fmt::Debug manually with redacted output."
                H6_FAIL=$((H6_FAIL + 1))
            fi
        done < <(rg 'key_material:|pub value:' "$file" -n 2>/dev/null || true)
    done < <(find "$src_dir" -name '*.rs' ! -path '*/tests/*')
}

check_h6_dir "$ROOT/crates/rivers-lockbox-engine/src"
check_h6_dir "$ROOT/crates/rivers-keystore-engine/src"

[[ $H6_FAIL -eq 0 ]] && pass H6

# ── H7: broker plugin tests reference SDK contract types (advisory) ───────────
echo "[H7] broker plugin tests reference SDK contract types (advisory)"
for driver in nats rabbitmq kafka; do
    test_file="$ROOT/crates/rivers-plugin-$driver/tests/${driver}_live_test.rs"
    [[ -f "$test_file" ]] || continue
    if ! grep -q 'rivers_driver_sdk\|rivers_runtime' "$test_file" 2>/dev/null; then
        echo "  WARN [H7]: $test_file does not import SDK contract types (AckOutcome, BrokerConsumer, etc.)" >&2
    else
        log "H7: $driver test imports SDK OK"
    fi
done
pass H7

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
if [[ $VIOLATIONS -eq 0 ]]; then
    echo "All heuristic checks passed."
    exit 0
else
    echo "FAILED: $VIOLATIONS violation(s). See LINT FAIL lines above." >&2
    exit 1
fi
