#!/usr/bin/env bash
# run-probe.sh — exercise every probe case against a running riversd.
#
# Prereq: a riversd instance serving this bundle. Example:
#   mkdir -p data
#   sqlite3 data/probe.db "SELECT 1" >/dev/null    # create empty db
#   riverpackage validate .
#   riversd --config path/to/riversd.toml          # bundle_path = this dir
#
# Pass --base URL to override the default.
set -euo pipefail

BASE="${1:-http://localhost:8080/cb-ts-repro/probe}"
echo "▶ probing $BASE"
echo

for case in a b c d e f g h i; do
    printf "case-%s ... " "$case"
    resp=$(curl -sk -w "\n%{http_code}" "$BASE/probe/case-$case" || true)
    status="${resp##*$'\n'}"
    body="${resp%$'\n'*}"
    printf "%s\n" "$status"
    printf "  %s\n\n" "${body:0:250}"
done
