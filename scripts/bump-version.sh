#!/usr/bin/env bash
# Bump the workspace version per the Rivers versioning policy.
#
# Format: MAJOR.MINOR.PATCH+HHMMDDMMYY (UTC build stamp)
#   - MAJOR: leading 0 (pre-1.0)
#   - MINOR: incremented on major changes (e.g. 0.55.x → 0.56.x)
#   - PATCH: incremented on code-fix changes (e.g. 0.55.0 → 0.55.1)
#   - +HHMMDDMMYY: refreshed on every PR (UTC: hour, minute, day, month, year)
#
# Usage:
#   ./scripts/bump-version.sh build      # refresh build stamp only (every PR)
#   ./scripts/bump-version.sh patch      # bump PATCH + refresh stamp (code fix)
#   ./scripts/bump-version.sh minor      # bump MINOR, reset PATCH=0 + refresh stamp (major change)
#
# Naming note: the policy uses these labels (which differ from strict SemVer
# because Rivers is pre-1.0 with leading 0):
#   - "major change"  → bump MINOR (0.55.0 → 0.56.0)
#   - "code fix"      → bump PATCH (0.55.0 → 0.55.1)
#   - any PR          → refresh BUILD stamp
#
# See CLAUDE.md "Versioning" for the full policy.

set -euo pipefail

CARGO_TOML="${CARGO_TOML:-Cargo.toml}"
COMPONENT="${1:-build}"

# Generate UTC build stamp: HHMMDDMMYY (10 digits, all zero-padded)
BUILD_STAMP=$(date -u +"%H%M%d%m%y")

# Read current version from workspace [package] block
CURRENT=$(grep -E '^version = "' "$CARGO_TOML" | head -1 | sed 's/.*= *"\(.*\)".*/\1/')
if [[ -z "$CURRENT" ]]; then
    echo "ERROR: could not parse version from $CARGO_TOML" >&2
    exit 1
fi

# Strip existing build metadata (everything after +)
BASE="${CURRENT%%+*}"

# Split MAJOR.MINOR.PATCH
IFS='.' read -r MAJOR MINOR PATCH <<<"$BASE"
if [[ -z "$MAJOR" || -z "$MINOR" || -z "$PATCH" ]]; then
    echo "ERROR: version '$CURRENT' is not MAJOR.MINOR.PATCH" >&2
    exit 1
fi

case "$COMPONENT" in
    build)
        # Refresh build stamp only
        ;;
    patch)
        # "Code fix" — bump PATCH
        PATCH=$((PATCH + 1))
        ;;
    minor)
        # "Major change" (per policy naming) — bump MINOR, reset PATCH
        MINOR=$((MINOR + 1))
        PATCH=0
        ;;
    *)
        echo "ERROR: unknown component '$COMPONENT' (expected: build, patch, minor)" >&2
        exit 1
        ;;
esac

NEW="${MAJOR}.${MINOR}.${PATCH}+${BUILD_STAMP}"

# Replace the first `version = "..."` line in Cargo.toml using awk.
# Portable across BSD (macOS) and GNU sed dialects, which differ on the
# `0,/pattern/` first-match selector.
TMP="${CARGO_TOML}.bump.$$"
awk -v new="version = \"$NEW\"" '
    !done && /^version = "/ { print new; done=1; next }
    { print }
' "$CARGO_TOML" >"$TMP"
mv "$TMP" "$CARGO_TOML"

echo "$CURRENT → $NEW"
