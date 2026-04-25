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

# Build monolithic static binaries for the host (~80MB riversd)
build:
    cargo build --release

# Build in debug mode
build-debug:
    cargo build

# ── Targeted (cross) builds ─────────────────────────────────────
#
# Aliases:
#   linux-x64  → x86_64-unknown-linux-gnu  (uses `cross` via Docker)
#   linux-arm  → aarch64-unknown-linux-gnu (uses `cross` via Docker)
#   mac-arm    → aarch64-apple-darwin      (native cargo)
#   mac-x64    → x86_64-apple-darwin       (native cargo, needs target)
#   host       → cargo build --release (no --target)
#
# Cross-targets require: Docker running + `cross` installed.
build-target target="linux-x64":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{target}}" in
        host)
            echo "→ host build (no --target)"
            cargo build --release
            ;;
        linux-x64)
            echo "→ cross build x86_64-unknown-linux-gnu (Docker)"
            cross build --release --target x86_64-unknown-linux-gnu \
                -p riversd -p riversctl -p rivers-lockbox -p riverpackage
            ;;
        linux-arm)
            echo "→ cross build aarch64-unknown-linux-gnu (Docker)"
            cross build --release --target aarch64-unknown-linux-gnu \
                -p riversd -p riversctl -p rivers-lockbox -p riverpackage
            ;;
        mac-arm)
            echo "→ native build aarch64-apple-darwin"
            cargo build --release --target aarch64-apple-darwin \
                -p riversd -p riversctl -p rivers-lockbox -p riverpackage
            ;;
        mac-x64)
            echo "→ native build x86_64-apple-darwin"
            cargo build --release --target x86_64-apple-darwin \
                -p riversd -p riversctl -p rivers-lockbox -p riverpackage
            ;;
        *)
            echo "error: unknown target '{{target}}'" >&2
            echo "       use one of: host | linux-x64 | linux-arm | mac-arm | mac-x64" >&2
            exit 1
            ;;
    esac
    echo "✓ build complete"

# Run all tests
test:
    cargo test

# Check the workspace compiles
check:
    cargo check

# Run TS pipeline probe bundle against a running riversd
# Prereq: riversd is serving tests/fixtures/ts-pipeline-probe/ on $BASE
# Default BASE targets a local instance on port 8080 with the probe mount path.
probe-ts base="http://localhost:8080/cb-ts-repro/probe":
    tests/fixtures/ts-pipeline-probe/run-probe.sh {{base}}

# Clean all build artifacts
clean:
    cargo clean

# ── Dynamic Build ────────────────────────────────────────────────

# Build dynamic mode: statically-linked drivers + engine cdylibs
# (cdylib driver plugins disabled — they cause SIGABRT on connect)
build-dynamic: _build-dynamic-crates _assemble-dynamic

_build-dynamic-crates:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "==> Building binaries (static drivers)..."
    cargo build --release --no-default-features --features static-builtin-drivers \
        -p riversd -p riversctl
    echo "==> Building engine cdylibs..."
    cargo build --release -p rivers-engine-v8 -p rivers-engine-wasm

_assemble-dynamic:
    #!/usr/bin/env bash
    set -euo pipefail
    RELEASE_DIR="release/dynamic"
    rm -rf "$RELEASE_DIR"
    mkdir -p "$RELEASE_DIR/bin" "$RELEASE_DIR/lib"

    echo "==> Assembling dynamic release..."
    cp target/release/riversd "$RELEASE_DIR/bin/"
    cp target/release/riversctl "$RELEASE_DIR/bin/"

    for ext in dylib so; do
        for f in target/release/librivers_engine_*.$ext; do
            [ -f "$f" ] && cp "$f" "$RELEASE_DIR/lib/" || true
        done
    done

    echo "==> Dynamic release assembled at $RELEASE_DIR/"
    echo "    bin/     $(ls "$RELEASE_DIR/bin/" | tr '\n' ' ')"
    echo "    lib/     $(ls "$RELEASE_DIR/lib/" | tr '\n' ' ')"

    echo ""
    "$RELEASE_DIR/bin/riversd" --version
    "$RELEASE_DIR/bin/riversctl" version

# ── Deploy (via cargo deploy) ────────────────────────────────────

# Deploy static dist to a target directory (single binary, all drivers compiled in)
deploy path:
    cargo deploy {{path}} --static

# Deploy dynamic dist (thin binary + engine dylibs, static drivers)
deploy-dynamic path:
    cargo deploy {{path}}

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

# Build .rpm packages (rivers, rivers-lib, rivers-plugins) — legacy, requires rpmbuild on Linux
package-rpm:
    ./scripts/build-packages.sh rpm

# Build a static-only RPM for a target arch via cargo-generate-rpm.
# Works from macOS (no rpmbuild needed). Default target = linux-x64.
# Output: dist/rpm/rivers-<version>-1.x86_64.rpm
package-rpm-target target="linux-x64":
    #!/usr/bin/env bash
    set -euo pipefail
    just build-target {{target}}
    case "{{target}}" in
        linux-x64) triple="x86_64-unknown-linux-gnu"; rpm_arch="x86_64" ;;
        linux-arm) triple="aarch64-unknown-linux-gnu"; rpm_arch="aarch64" ;;
        host)      triple=""; rpm_arch="$(uname -m)" ;;
        *)         echo "error: rpm only supports linux targets (got '{{target}}')" >&2; exit 1 ;;
    esac
    mkdir -p dist/rpm
    if [ -n "$triple" ]; then
        echo "→ generating rpm (target=$triple, arch=$rpm_arch)"
        cargo generate-rpm -p crates/riversd --target "$triple" --arch "$rpm_arch" \
            --output dist/rpm/
    else
        echo "→ generating rpm (host, arch=$rpm_arch)"
        cargo generate-rpm -p crates/riversd --arch "$rpm_arch" --output dist/rpm/
    fi
    echo ""
    echo "✓ rpm built:"
    ls -lh dist/rpm/*.rpm

# Build the linux-x64 RPM, ship to beta-01 (192.168.2.170), and prompt before
# restarting riversd. Skips the prompt with `BETA_RESTART=yes` env var.
ship-beta01:
    #!/usr/bin/env bash
    set -euo pipefail
    HOST="root@192.168.2.170"

    just package-rpm-target linux-x64
    RPM=$(ls -t dist/rpm/rivers-*.x86_64.rpm | head -1)
    if [ -z "$RPM" ]; then
        echo "error: no rpm found in dist/rpm/" >&2
        exit 1
    fi
    BASENAME=$(basename "$RPM")
    echo ""
    echo "→ shipping $BASENAME to $HOST:/tmp/"
    scp "$RPM" "${HOST}:/tmp/${BASENAME}"

    echo ""
    echo "→ installing on beta-01 via dnf"
    ssh "$HOST" "dnf install -y /tmp/${BASENAME}"

    echo ""
    if [ "${BETA_RESTART:-}" = "yes" ]; then
        echo "→ BETA_RESTART=yes — restarting riversd without prompt"
        ssh "$HOST" "systemctl restart riversd && systemctl status riversd --no-pager"
    else
        printf "Restart riversd on beta-01 now? [y/N] "
        read -r REPLY
        if [[ "$REPLY" =~ ^[Yy] ]]; then
            ssh "$HOST" "systemctl restart riversd && systemctl status riversd --no-pager | head -15"
        else
            echo "→ skipped restart. Run manually:  ssh ${HOST} 'systemctl restart riversd'"
        fi
    fi

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
