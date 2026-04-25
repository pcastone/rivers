#!/bin/sh
# wc-json.sh — wrapper around /usr/bin/wc producing DOC-4 structured
# output. Invoked by rivers-exec driver (CS4.9). Reads {"path":"..."}
# from stdin; responds on stdout with {"lines":N,"words":M,"bytes":K}
# or {"error":"<reason>"} + non-zero exit on rejection.
#
# DOC-6 enforcement lives here (not in the exec driver): reject absolute
# paths and anything containing ".." so the scenario can't traverse out
# of the workspace. The ROOT env var (default /tmp) names where
# workspace-relative paths resolve.
#
# WHY a wrapper instead of hashing /usr/bin/wc directly: wc's binary
# hash varies across macOS/Linux/libc versions, making sha256 pinning
# impossible in a portable canary. This script is stable bytes; its
# SHA-256 is deterministic.

set -eu

input=$(cat)
# Primitive JSON `path` key extraction — canary target must not
# depend on jq being installed on every host.
path=$(printf '%s' "$input" | sed -n 's/.*"path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

if [ -z "$path" ]; then
    printf '{"error":"missing path field"}\n'
    exit 2
fi

# DOC-6: reject absolute and traversal-carrying paths.
case "$path" in
    /*)       printf '{"error":"absolute path rejected"}\n'; exit 2 ;;
    *..*)     printf '{"error":"path traversal rejected"}\n'; exit 2 ;;
esac

ROOT="${CANARY_WC_ROOT:-/tmp}"
full="$ROOT/$path"

if [ ! -f "$full" ]; then
    printf '{"error":"file not found"}\n'
    exit 2
fi

# wc output is typically `  N  M  K filename` with leading whitespace.
# awk handles the leading-whitespace + produces clean JSON.
wc "$full" | awk '{printf "{\"lines\":%d,\"words\":%d,\"bytes\":%d}\n", $1, $2, $3}'
