#!/bin/bash
# Rivers release build script
# Builds all binaries, engine shared libraries, and plugin shared libraries
# Assembles into release/{version}-{timestamp}/ directory

set -e

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
RELEASE_DIR="release/${VERSION}-${TIMESTAMP}"

echo "=== Rivers Release Build v${VERSION} ==="
echo ""

# Detect platform
case "$(uname -s)" in
    Darwin*) DYLIB_EXT="dylib" ;;
    Linux*)  DYLIB_EXT="so" ;;
    *)       DYLIB_EXT="so" ;;
esac

# Build all binaries
echo "[1/4] Building binaries..."
cargo build --release -p riversd -p riversctl -p rivers-lockbox -p riverpackage

# Build engine shared libraries
echo "[2/4] Building engine shared libraries..."
cargo build --release -p rivers-engine-v8 -p rivers-engine-wasm

# Build plugin shared libraries
echo "[3/4] Building plugin shared libraries..."
cargo build --release \
    -p rivers-plugin-cassandra \
    -p rivers-plugin-couchdb \
    -p rivers-plugin-elasticsearch \
    -p rivers-plugin-influxdb \
    -p rivers-plugin-exec \
    -p rivers-plugin-kafka \
    -p rivers-plugin-ldap \
    -p rivers-plugin-mongodb \
    -p rivers-plugin-nats \
    -p rivers-plugin-neo4j \
    -p rivers-plugin-rabbitmq \
    -p rivers-plugin-redis-streams

# Assemble release directory
echo "[4/4] Assembling release..."
mkdir -p "${RELEASE_DIR}/bin" "${RELEASE_DIR}/lib" "${RELEASE_DIR}/plugins" "${RELEASE_DIR}/config" "${RELEASE_DIR}/docs"

# Binaries
cp target/release/riversd "${RELEASE_DIR}/bin/"
cp target/release/riversctl "${RELEASE_DIR}/bin/"
cp target/release/rivers-lockbox "${RELEASE_DIR}/bin/"
cp target/release/riverpackage "${RELEASE_DIR}/bin/"

# Engine shared libraries
cp "target/release/librivers_engine_v8.${DYLIB_EXT}" "${RELEASE_DIR}/lib/" 2>/dev/null || true
cp "target/release/librivers_engine_wasm.${DYLIB_EXT}" "${RELEASE_DIR}/lib/" 2>/dev/null || true

# Plugin shared libraries
for plugin in cassandra couchdb elasticsearch exec influxdb kafka ldap mongodb nats neo4j rabbitmq redis_streams; do
    src="target/release/librivers_plugin_${plugin}.${DYLIB_EXT}"
    if [ -f "$src" ]; then
        cp "$src" "${RELEASE_DIR}/plugins/"
    fi
done

# Config
cp config/riversd.toml "${RELEASE_DIR}/config/" 2>/dev/null || true

# Docs
cp release/docs/*.md "${RELEASE_DIR}/docs/" 2>/dev/null || true

# Version file
cat > "${RELEASE_DIR}/VERSION" << EOF
Rivers v${VERSION}
Build: release (optimized)
Date: $(date +%Y-%m-%d)
Platform: $(uname -s) $(uname -m)
EOF

# Update latest symlink
ln -sfn "${VERSION}-${TIMESTAMP}" release/latest

# Summary
echo ""
echo "=== Release Summary ==="
echo "Directory: ${RELEASE_DIR}"
echo ""
echo "Binaries:"
ls -lh "${RELEASE_DIR}/bin/"
echo ""
echo "Engine Libraries:"
ls -lh "${RELEASE_DIR}/lib/" 2>/dev/null || echo "  (none)"
echo ""
echo "Plugin Libraries:"
ls -lh "${RELEASE_DIR}/plugins/" 2>/dev/null || echo "  (none)"
echo ""
echo "Version:"
cat "${RELEASE_DIR}/VERSION"
echo ""
echo "Done."
