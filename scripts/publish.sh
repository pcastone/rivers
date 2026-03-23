#!/usr/bin/env bash
# publish.sh — Copy source files to rivers.pub, excluding binaries and build artifacts.
#
# Usage: ./scripts/publish.sh [destination]
#   Default destination: ../rivers.pub/

set -euo pipefail

SRC="/Users/pcastone/Projects/rust/rivers"
DEST="${1:-/Users/pcastone/Projects/rust/rivers.pub}"

echo "==> Publishing rivers → $DEST"
echo "    Excluding: target/, binaries, .dylib, .so, .a, .zip, .claude/, scratch/, data/, test keystores"

rsync -av --delete \
    --exclude='target/' \
    --exclude='.claude/' \
    --exclude='scratch/' \
    --exclude='data/' \
    --exclude='test/' \
    --exclude='crates/rivers-core/test' \
    --exclude='release/*/bin/' \
    --exclude='release/*/lib/' \
    --exclude='release/*/plugins/' \
    --exclude='release/dynamic/' \
    --exclude='*.zip' \
    --exclude='*.dylib' \
    --exclude='*.so' \
    --exclude='*.a' \
    --exclude='*.o' \
    --exclude='*.d' \
    --exclude='*.rmeta' \
    --exclude='*.rlib' \
    --exclude='*.exe' \
    --exclude='*.rkeystore' \
    --exclude='*.pem' \
    --exclude='*.key' \
    --exclude='.DS_Store' \
    --exclude='.git/' \
    --exclude='.worktrees/' \
    --exclude='logs/*.log' \
    --exclude='node_modules/' \
    --exclude='training/' \
    "$SRC/" "$DEST/"

echo ""
echo "==> Done. File count:"
find "$DEST" -type f | wc -l | xargs echo "   files:"
echo ""
echo "==> Size:"
du -sh "$DEST" | awk '{print "   " $1}'
