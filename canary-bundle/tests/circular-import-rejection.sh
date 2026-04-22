#!/usr/bin/env bash
# RT-TS-CIRCULAR — spec §3.5 circular-import rejection at bundle load.
#
# Runs `riverpackage validate` on a fixture bundle whose handler tree
# contains a two-module cycle (a.ts ↔ b.ts). Validation MUST fail with
# non-zero exit and the spec §3.5 error phrase.
#
# Prereq: `riverpackage` on PATH (ships with any Rivers install).
set -euo pipefail

FIXTURE="$(cd "$(dirname "$0")" && pwd)/fixtures/circular-import-reject"

if ! command -v riverpackage >/dev/null 2>&1; then
    echo "SKIP circular-import-rejection: riverpackage not on PATH"
    exit 0
fi

# Validate should fail; capture stderr + exit code.
output=$(riverpackage validate "$FIXTURE" 2>&1 || true)
exit_code=$?

if [ "$exit_code" -eq 0 ]; then
    echo "FAIL circular-import-rejection: validate exited 0"
    echo "$output"
    exit 1
fi

if ! echo "$output" | grep -q "circular import detected"; then
    echo "FAIL circular-import-rejection: expected 'circular import detected'"
    echo "$output"
    exit 1
fi

if ! echo "$output" | grep -qE "a\.ts"; then
    echo "FAIL circular-import-rejection: error must name a.ts"
    echo "$output"
    exit 1
fi

if ! echo "$output" | grep -qE "b\.ts"; then
    echo "FAIL circular-import-rejection: error must name b.ts"
    echo "$output"
    exit 1
fi

echo "PASS circular-import-rejection"
