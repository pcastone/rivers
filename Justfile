# Rivers — Build, Test, Package, Deploy
#
# Usage:
#   just build              # Static monolithic binaries (default)
#   just build-dynamic      # Thin binaries + shared libs
#   just deploy <path>      # Deploy via cargo deploy (dynamic)
#   just deploy-static <p>  # Deploy via cargo deploy (static)
#   just dist-source        # Source code zip (like GitHub release)
#   just dist-zip           # Binary zip via cargo deploy
#   just package-deb        # Debian packages
#   just package-all        # All package formats

# ── Variables ────────────────────────────────────────────────────

version := `grep '^version' Cargo.toml | head -1 | sed 's/version = "//;s/"//'`
arch := `uname -m`
os := if os() == "macos" { "darwin" } else { os() }

# ── Build ────────────────────────────────────────────────────────

# Build monolithic static binaries (~80MB riversd)
build:
    cargo build --release

# Build in debug mode
build-debug:
    cargo build

# Run all tests
test:
    cargo test

# Check the workspace compiles
check:
    cargo check

# Clean all build artifacts
clean:
    cargo clean

# ── Dynamic Build ────────────────────────────────────────────────

# Build dynamic mode: thin binaries + shared runtime dylib + cdylib engines/plugins
build-dynamic: _set-dylib _build-dynamic-crates _set-rlib _assemble-dynamic

_set-dylib:
    sed -i '' 's/crate-type = \["rlib"\]/crate-type = ["dylib"]/' crates/rivers-runtime/Cargo.toml

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
        -p rivers-plugin-exec \
        -p rivers-plugin-mongodb \
        -p rivers-plugin-nats \
        -p rivers-plugin-neo4j \
        -p rivers-plugin-rabbitmq \
        -p rivers-plugin-redis-streams

_set-rlib:
    sed -i '' 's/crate-type = \["dylib"\]/crate-type = ["rlib"]/' crates/rivers-runtime/Cargo.toml

_assemble-dynamic:
    #!/usr/bin/env bash
    set -euo pipefail
    RELEASE_DIR="release/dynamic"
    rm -rf "$RELEASE_DIR"
    mkdir -p "$RELEASE_DIR/bin" "$RELEASE_DIR/lib" "$RELEASE_DIR/plugins"

    echo "==> Assembling dynamic release..."
    cp target/release/riversd "$RELEASE_DIR/bin/"
    cp target/release/riversctl "$RELEASE_DIR/bin/"

    for ext in dylib so; do
        [ -f "target/release/librivers_runtime.$ext" ] && \
            cp "target/release/librivers_runtime.$ext" "$RELEASE_DIR/lib/" || true
    done

    SYSROOT=$(rustc --print sysroot)
    TRIPLE=$(rustc -vV | awk '/^host:/{print $2}')
    for f in "$SYSROOT/lib/rustlib/$TRIPLE/lib"/libstd-*.dylib "$SYSROOT/lib/rustlib/$TRIPLE/lib"/libstd-*.so; do
        [ -f "$f" ] && cp "$f" "$RELEASE_DIR/lib/" && break
    done

    for ext in dylib so; do
        for f in target/release/librivers_engine_*.$ext; do
            [ -f "$f" ] && cp "$f" "$RELEASE_DIR/lib/" || true
        done
    done

    for ext in dylib so; do
        for f in target/release/librivers_plugin_*.$ext; do
            [ -f "$f" ] && cp "$f" "$RELEASE_DIR/plugins/" || true
        done
    done

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

    echo ""
    "$RELEASE_DIR/bin/riversd" --version
    "$RELEASE_DIR/bin/riversctl" version

# ── Deploy (via cargo deploy) ────────────────────────────────────

# Deploy dynamic dist to a target directory
deploy path:
    cargo deploy {{path}}

# Deploy static dist to a target directory
deploy-static path:
    cargo deploy {{path}} --static

# ── Distribution ─────────────────────────────────────────────────

# Create source code zip (mirrors GitHub release source archive)
dist-source:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION="{{version}}"
    OUTDIR="release"
    ZIPNAME="rivers-${VERSION}-source.zip"
    mkdir -p "$OUTDIR"
    echo "==> Creating source dist: ${OUTDIR}/${ZIPNAME}"
    git archive --format=zip --prefix="rivers-${VERSION}/" HEAD -o "${OUTDIR}/${ZIPNAME}"
    echo "  -> ${OUTDIR}/${ZIPNAME}"
    ls -lh "${OUTDIR}/${ZIPNAME}"

# Create binary zip via cargo deploy (dynamic mode)
dist-zip:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION="{{version}}"
    OS="{{os}}"
    ARCH="{{arch}}"
    STAGING="dist/zip/rivers-${VERSION}-${OS}-${ARCH}"
    OUTDIR="release"
    ZIPNAME="rivers-${VERSION}-${OS}-${ARCH}.zip"
    rm -rf "$STAGING"
    cargo deploy "$STAGING"
    cp LICENSE "$STAGING/" 2>/dev/null || true
    cp README.md "$STAGING/" 2>/dev/null || true
    mkdir -p "$OUTDIR"
    (cd dist/zip && zip -r "../../${OUTDIR}/${ZIPNAME}" "rivers-${VERSION}-${OS}-${ARCH}/")
    echo ""
    echo "  -> ${OUTDIR}/${ZIPNAME}"
    ls -lh "${OUTDIR}/${ZIPNAME}"

# Create binary zip via cargo deploy (static mode)
dist-zip-static:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION="{{version}}"
    OS="{{os}}"
    ARCH="{{arch}}"
    STAGING="dist/zip/rivers-${VERSION}-${OS}-${ARCH}-static"
    OUTDIR="release"
    ZIPNAME="rivers-${VERSION}-${OS}-${ARCH}-static.zip"
    rm -rf "$STAGING"
    cargo deploy "$STAGING" --static
    cp LICENSE "$STAGING/" 2>/dev/null || true
    cp README.md "$STAGING/" 2>/dev/null || true
    mkdir -p "$OUTDIR"
    (cd dist/zip && zip -r "../../${OUTDIR}/${ZIPNAME}" "rivers-${VERSION}-${OS}-${ARCH}-static/")
    echo ""
    echo "  -> ${OUTDIR}/${ZIPNAME}"
    ls -lh "${OUTDIR}/${ZIPNAME}"

# ── Local Release (versioned layout with symlink) ────────────────

# Create local release layout in release/<version>-<timestamp>/ using cargo deploy
release:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION="{{version}}"
    TIMESTAMP=$(date +%Y%m%d-%H%M%S)
    RELEASE_NAME="${VERSION}-${TIMESTAMP}"
    DEST="release/${RELEASE_NAME}"

    # Ensure cargo-deploy is installed
    if ! command -v cargo-deploy &> /dev/null; then
        echo "→ installing cargo-deploy..."
        cargo install --path crates/cargo-deploy --quiet
    fi

    echo "→ deploying to ${DEST}/"
    cargo deploy "$DEST"

    # Symlink release/latest → this release
    ln -sfn "${RELEASE_NAME}" release/latest
    echo "   latest -> ${RELEASE_NAME}"

    # Prune to 3 releases (exclude symlinks and non-versioned dirs)
    RELEASES=()
    while IFS= read -r name; do
        RELEASES+=("$name")
    done < <(find release -maxdepth 1 -mindepth 1 -type d ! -name '.*' ! -name dynamic ! -name zip | xargs -I{} basename {} | sort)

    EXCESS=$(( ${#RELEASES[@]} - 3 ))
    if (( EXCESS > 0 )); then
        echo "→ pruning ${EXCESS} old release(s)..."
        for (( i=0; i<EXCESS; i++ )); do
            echo "   removing ${RELEASES[$i]}"
            rm -rf "release/${RELEASES[$i]}"
        done
    fi

    echo ""
    echo "✓ release ready: ${DEST}"
    echo "  cd ${DEST} && ./bin/riversctl doctor"

# Create local release layout (static mode)
release-static:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION="{{version}}"
    TIMESTAMP=$(date +%Y%m%d-%H%M%S)
    RELEASE_NAME="${VERSION}-${TIMESTAMP}"
    DEST="release/${RELEASE_NAME}"

    if ! command -v cargo-deploy &> /dev/null; then
        echo "→ installing cargo-deploy..."
        cargo install --path crates/cargo-deploy --quiet
    fi

    echo "→ deploying (static) to ${DEST}/"
    cargo deploy "$DEST" --static

    ln -sfn "${RELEASE_NAME}" release/latest
    echo "   latest -> ${RELEASE_NAME}"

    echo ""
    echo "✓ release ready: ${DEST}"

# ── Packaging (deb/rpm/tarball/windows) ──────────────────────────

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

# ── Info ─────────────────────────────────────────────────────────

# List binary sizes for the static build
sizes:
    @ls -lh target/release/riversd target/release/riversctl 2>/dev/null || echo "No release binaries — run 'just build' first"

# List binary/lib sizes for the dynamic build
sizes-dynamic:
    @echo "==> Binaries:"
    @ls -lh release/dynamic/bin/* 2>/dev/null || echo "  (none)"
    @echo "==> Libraries:"
    @ls -lh release/dynamic/lib/* 2>/dev/null || echo "  (none)"

# Show all available recipes
help:
    @just --list
