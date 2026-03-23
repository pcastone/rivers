# Rivers — Dual Static/Dynamic Build System
#
# Static mode (default): single monolithic binaries, everything statically linked.
# Dynamic mode: thin binaries + shared runtime dylib + cdylib engines/plugins.
#
# Usage:
#   just build            # Static monolithic binaries (default)
#   just build-dynamic    # Thin binaries + shared libs
#   just clean            # Clean build artifacts

# ── Static Build (default) ───────────────────────────────────────

# Build monolithic static binaries (~80MB riversd)
build:
    cargo build --release

# Build in debug mode
build-debug:
    cargo build

# Run all tests
test:
    cargo test

# ── Dynamic Build ────────────────────────────────────────────────

# Build dynamic mode: thin binaries + shared runtime dylib + cdylib engines/plugins.
# The rivers-runtime crate-type is temporarily switched to dylib for the build,
# then reverted back to rlib so the workspace stays in static mode by default.
build-dynamic: _set-dylib _build-dynamic-crates _set-rlib _assemble-dynamic

# Step 1: Switch rivers-runtime to dylib output
_set-dylib:
    sed -i '' 's/crate-type = \["rlib"\]/crate-type = ["dylib"]/' crates/rivers-runtime/Cargo.toml

# Step 2: Build all crates with the right flags
# Note: LTO=off required because prefer-dynamic is incompatible with LTO.
# prefer-dynamic ensures serde_json/tokio/etc symbols resolve through the dylib.
_build-dynamic-crates:
    #!/usr/bin/env bash
    set -euo pipefail
    export CARGO_PROFILE_RELEASE_LTO=off
    export RUSTFLAGS='-C link-arg=-Wl,-rpath,@executable_path/../lib -C prefer-dynamic'
    echo "==> Building rivers-runtime (dylib) + binaries..."
    cargo build --release -p rivers-runtime -p riversd -p riversctl --no-default-features
    echo "==> Building engine cdylibs..."
    cargo build --release -p rivers-engine-v8 -p rivers-engine-wasm
    echo "==> Building plugin cdylibs..."
    cargo build --release --features plugin-exports \
        -p rivers-plugin-cassandra \
        -p rivers-plugin-couchdb \
        -p rivers-plugin-elasticsearch \
        -p rivers-plugin-influxdb \
        -p rivers-plugin-kafka \
        -p rivers-plugin-ldap \
        -p rivers-plugin-mongodb \
        -p rivers-plugin-nats \
        -p rivers-plugin-rabbitmq \
        -p rivers-plugin-redis-streams

# Step 3: Revert rivers-runtime back to rlib
_set-rlib:
    sed -i '' 's/crate-type = \["dylib"\]/crate-type = ["rlib"]/' crates/rivers-runtime/Cargo.toml

# Step 4: Assemble the dynamic release directory
_assemble-dynamic:
    #!/usr/bin/env bash
    set -euo pipefail
    RELEASE_DIR="release/dynamic"
    rm -rf "$RELEASE_DIR"
    mkdir -p "$RELEASE_DIR/bin" "$RELEASE_DIR/lib" "$RELEASE_DIR/plugins"

    echo "==> Assembling dynamic release..."
    cp target/release/riversd "$RELEASE_DIR/bin/"
    cp target/release/riversctl "$RELEASE_DIR/bin/"

    # Runtime dylib
    for ext in dylib so; do
        [ -f "target/release/librivers_runtime.$ext" ] && \
            cp "target/release/librivers_runtime.$ext" "$RELEASE_DIR/lib/" || true
    done

    # Rust std dylib (needed for prefer-dynamic builds)
    SYSROOT=$(rustc --print sysroot)
    TRIPLE=$(rustc -vV | awk '/^host:/{print $2}')
    for f in "$SYSROOT/lib/rustlib/$TRIPLE/lib"/libstd-*.dylib "$SYSROOT/lib/rustlib/$TRIPLE/lib"/libstd-*.so; do
        [ -f "$f" ] && cp "$f" "$RELEASE_DIR/lib/" && break
    done

    # Engine cdylibs
    for ext in dylib so; do
        for f in target/release/librivers_engine_*.$ext; do
            [ -f "$f" ] && cp "$f" "$RELEASE_DIR/lib/" || true
        done
    done

    # Plugin cdylibs
    for ext in dylib so; do
        for f in target/release/librivers_plugin_*.$ext; do
            [ -f "$f" ] && cp "$f" "$RELEASE_DIR/plugins/" || true
        done
    done

    # Fix dylib install names on macOS (rpath → @executable_path/../lib)
    if [ "$(uname)" = "Darwin" ]; then
        DEPS_DYLIB="$(find target/release/deps -name 'librivers_runtime.dylib' -maxdepth 1 | head -1)"
        if [ -n "$DEPS_DYLIB" ]; then
            for bin in "$RELEASE_DIR"/bin/*; do
                install_name_tool -change "$DEPS_DYLIB" \
                    @executable_path/../lib/librivers_runtime.dylib "$bin" 2>/dev/null || true
            done
        fi
    fi

    echo "==> Dynamic release assembled at $RELEASE_DIR/"
    echo "    bin/     $(ls "$RELEASE_DIR/bin/" | tr '\n' ' ')"
    echo "    lib/     $(ls "$RELEASE_DIR/lib/" | tr '\n' ' ')"
    echo "    plugins/ $(ls "$RELEASE_DIR/plugins/" | tr '\n' ' ')"

    # Smoke test
    echo ""
    "$RELEASE_DIR/bin/riversd" --version
    "$RELEASE_DIR/bin/riversctl" version

# ── Utilities ────────────────────────────────────────────────────

# Clean all build artifacts
clean:
    cargo clean

# Check the workspace compiles
check:
    cargo check

# List binary sizes for the static build
sizes:
    @ls -lh target/release/riversd target/release/riversctl 2>/dev/null || echo "No release binaries — run 'just build' first"

# List binary/lib sizes for the dynamic build
sizes-dynamic:
    @echo "==> Binaries:"
    @ls -lh release/dynamic/bin/* 2>/dev/null || echo "  (none)"
    @echo "==> Libraries:"
    @ls -lh release/dynamic/lib/* 2>/dev/null || echo "  (none)"

# ── Packaging ──────────────────────────────────────────────────────

# Build .deb packages (rivers, rivers-lib, rivers-plugins)
package-deb:
    ./scripts/build-packages.sh deb

# Build .rpm packages (rivers, rivers-lib, rivers-plugins)
package-rpm:
    ./scripts/build-packages.sh rpm

# Build Windows .zip (cross-compiled x86_64)
package-windows:
    ./scripts/build-packages.sh windows

# Build portable .tar.gz for current platform
package-tarball:
    ./scripts/build-packages.sh tarball

# Build all package formats
package-all:
    ./scripts/build-packages.sh all
