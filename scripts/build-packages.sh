#!/usr/bin/env bash
# build-packages.sh — Build platform packages for Rivers.
#
# Usage:
#   ./scripts/build-packages.sh deb          # Build .deb packages (Debian/Ubuntu)
#   ./scripts/build-packages.sh rpm          # Build .rpm packages (RHEL/Fedora/CentOS)
#   ./scripts/build-packages.sh windows      # Build Windows .zip archive
#   ./scripts/build-packages.sh tarball      # Build portable .tar.gz archive
#   ./scripts/build-packages.sh all          # Build all formats
#
# Prerequisites:
#   deb:     dpkg-deb
#   rpm:     rpmbuild
#   windows: cross (cargo install cross) + x86_64-pc-windows-gnu target
#   tarball: tar
#
# Output: dist/<format>/
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/version = "//;s/"//')
ARCH=$(uname -m)
DIST_DIR="${REPO_ROOT}/dist"

# Map uname -m to Debian/RPM arch names
case "$ARCH" in
    x86_64)  DEB_ARCH="amd64"; RPM_ARCH="x86_64" ;;
    aarch64) DEB_ARCH="arm64"; RPM_ARCH="aarch64" ;;
    arm64)   DEB_ARCH="arm64"; RPM_ARCH="aarch64"; ARCH="aarch64" ;;
    *)       DEB_ARCH="$ARCH"; RPM_ARCH="$ARCH" ;;
esac

BINARIES=(riversd riversctl rivers-lockbox riverpackage)

# ── Helpers ─────────────────────────────────────────────────────────

build_dynamic() {
    echo "==> Building dynamic release..."
    just build-dynamic
}

ensure_static_build() {
    echo "==> Building static release..."
    cargo build --release
    for bin in "${BINARIES[@]}"; do
        if [[ ! -f "target/release/${bin}" ]]; then
            echo "error: binary not found: target/release/${bin}" >&2
            exit 1
        fi
    done
}

collect_libs() {
    local dest="$1"
    mkdir -p "$dest"
    for ext in so dylib; do
        for f in target/release/librivers_runtime.$ext \
                 target/release/librivers_engine_v8.$ext \
                 target/release/librivers_engine_wasm.$ext \
                 target/release/librivers_drivers_builtin.$ext \
                 target/release/librivers_storage_backends.$ext \
                 target/release/librivers_lockbox_engine.$ext; do
            [ -f "$f" ] && cp "$f" "$dest/" || true
        done
    done
}

collect_plugins() {
    local dest="$1"
    mkdir -p "$dest"
    for ext in so dylib; do
        for f in target/release/librivers_plugin_*.$ext; do
            [ -f "$f" ] && cp "$f" "$dest/" || true
        done
    done
}

# ── Debian .deb ─────────────────────────────────────────────────────

build_deb() {
    echo ""
    echo "=== Building Debian packages ==="
    ensure_static_build
    build_dynamic

    local out="${DIST_DIR}/deb"
    rm -rf "$out"
    mkdir -p "$out"

    # ── rivers (base) ──
    local base="${out}/rivers_${VERSION}_${DEB_ARCH}"
    mkdir -p "${base}/DEBIAN"
    mkdir -p "${base}/usr/bin"
    mkdir -p "${base}/etc/rivers"
    mkdir -p "${base}/lib/systemd/system"
    mkdir -p "${base}/var/lib/rivers"
    mkdir -p "${base}/var/log/rivers"
    mkdir -p "${base}/var/lib/rivers/lockbox"

    # Control file
    cat > "${base}/DEBIAN/control" << EOF
Package: rivers
Version: ${VERSION}
Architecture: ${DEB_ARCH}
Maintainer: Paul Castone <paul.castone@gmail.com>
Depends: rivers-lib (= ${VERSION})
Recommends: rivers-plugins
Section: net
Priority: optional
Homepage: https://github.com/pcastone/rivers
Description: Rivers application server — declarative API framework
 Rivers is a declarative app-service framework written in Rust.
 Define REST APIs, WebSocket, SSE, and GraphQL endpoints using
 TOML configuration and JSON schemas — no application code required.
EOF

    cp packaging/debian/postinst "${base}/DEBIAN/postinst"
    cp packaging/debian/prerm "${base}/DEBIAN/prerm"
    cp packaging/debian/conffiles "${base}/DEBIAN/conffiles"
    chmod 0755 "${base}/DEBIAN/postinst" "${base}/DEBIAN/prerm"

    for bin in "${BINARIES[@]}"; do
        cp "target/release/${bin}" "${base}/usr/bin/${bin}"
        chmod 0755 "${base}/usr/bin/${bin}"
    done
    cp packaging/config/riversd.toml "${base}/etc/rivers/riversd.toml"
    cp packaging/systemd/riversd.service "${base}/lib/systemd/system/riversd.service"

    dpkg-deb --build "$base" "${out}/rivers_${VERSION}_${DEB_ARCH}.deb"
    echo "  -> ${out}/rivers_${VERSION}_${DEB_ARCH}.deb"

    # ── rivers-lib ──
    local lib="${out}/rivers-lib_${VERSION}_${DEB_ARCH}"
    mkdir -p "${lib}/DEBIAN"
    mkdir -p "${lib}/usr/lib/rivers"

    cat > "${lib}/DEBIAN/control" << EOF
Package: rivers-lib
Version: ${VERSION}
Architecture: ${DEB_ARCH}
Maintainer: Paul Castone <paul.castone@gmail.com>
Section: libs
Priority: optional
Homepage: https://github.com/pcastone/rivers
Description: Rivers shared libraries and engines
 Shared runtime library, V8 JavaScript engine, Wasmtime WASM engine,
 built-in database drivers, storage backends, and LockBox encryption
 engine for the Rivers application server.
EOF

    collect_libs "${lib}/usr/lib/rivers"

    dpkg-deb --build "$lib" "${out}/rivers-lib_${VERSION}_${DEB_ARCH}.deb"
    echo "  -> ${out}/rivers-lib_${VERSION}_${DEB_ARCH}.deb"

    # ── rivers-plugins ──
    local plugins="${out}/rivers-plugins_${VERSION}_${DEB_ARCH}"
    mkdir -p "${plugins}/DEBIAN"
    mkdir -p "${plugins}/usr/lib/rivers/plugins"

    cat > "${plugins}/DEBIAN/control" << EOF
Package: rivers-plugins
Version: ${VERSION}
Architecture: ${DEB_ARCH}
Maintainer: Paul Castone <paul.castone@gmail.com>
Depends: rivers-lib (= ${VERSION})
Section: libs
Priority: optional
Homepage: https://github.com/pcastone/rivers
Description: Rivers datasource driver plugins
 Optional datasource driver plugins for Rivers: MongoDB, Elasticsearch,
 Cassandra, CouchDB, InfluxDB, Kafka, RabbitMQ, NATS, Redis Streams,
 and LDAP.
EOF

    collect_plugins "${plugins}/usr/lib/rivers/plugins"

    dpkg-deb --build "$plugins" "${out}/rivers-plugins_${VERSION}_${DEB_ARCH}.deb"
    echo "  -> ${out}/rivers-plugins_${VERSION}_${DEB_ARCH}.deb"

    echo ""
    echo "Debian packages:"
    ls -lh "${out}"/*.deb
}

# ── RPM .rpm ────────────────────────────────────────────────────────

build_rpm() {
    echo ""
    echo "=== Building RPM packages ==="
    ensure_static_build
    build_dynamic

    local out="${DIST_DIR}/rpm"
    rm -rf "$out"
    mkdir -p "$out"

    local rpmbuild_dir="${out}/rpmbuild"
    mkdir -p "${rpmbuild_dir}"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,BUILDROOT}

    # Create source tarball layout
    local src="${rpmbuild_dir}/SOURCES/rivers-${VERSION}"
    mkdir -p "$src"
    cp -r target packaging LICENSE README.md NOTICE Cargo.toml "$src/"

    # Copy spec
    cp packaging/rpm/rivers.spec "${rpmbuild_dir}/SPECS/rivers.spec"

    # Build RPMs
    rpmbuild --define "_topdir ${rpmbuild_dir}" \
             --define "version ${VERSION}" \
             --define "release 0" \
             --buildroot "${rpmbuild_dir}/BUILDROOT" \
             -bb "${rpmbuild_dir}/SPECS/rivers.spec"

    # Move RPMs out
    find "${rpmbuild_dir}/RPMS" -name '*.rpm' -exec mv {} "$out/" \;

    echo ""
    echo "RPM packages:"
    ls -lh "${out}"/*.rpm 2>/dev/null || echo "  (check rpmbuild output above)"
}

# ── Windows .zip ────────────────────────────────────────────────────

build_windows() {
    echo ""
    echo "=== Building Windows package ==="

    local target="x86_64-pc-windows-gnu"
    local out="${DIST_DIR}/windows"
    rm -rf "$out"
    mkdir -p "$out"

    echo "==> Cross-compiling for ${target}..."
    echo "    (requires: rustup target add ${target}, or 'cross' installed)"

    # Try cross first, fall back to cargo
    if command -v cross &> /dev/null; then
        cross build --release --target "$target" \
            -p riversd -p riversctl -p rivers-lockbox -p riverpackage
    else
        cargo build --release --target "$target" \
            -p riversd -p riversctl -p rivers-lockbox -p riverpackage
    fi

    local staging="${out}/rivers-${VERSION}-windows-x86_64"
    mkdir -p "${staging}/bin"
    mkdir -p "${staging}/config"

    for bin in "${BINARIES[@]}"; do
        cp "target/${target}/release/${bin}.exe" "${staging}/bin/${bin}.exe"
    done
    cp packaging/config/riversd.toml "${staging}/config/riversd.toml"
    cp LICENSE "${staging}/"
    cp README.md "${staging}/"

    # Create zip
    (cd "$out" && zip -r "rivers-${VERSION}-windows-x86_64.zip" "rivers-${VERSION}-windows-x86_64/")
    echo "  -> ${out}/rivers-${VERSION}-windows-x86_64.zip"
}

# ── Portable tarball ────────────────────────────────────────────────

build_tarball() {
    echo ""
    echo "=== Building portable tarball ==="
    ensure_static_build

    local out="${DIST_DIR}/tarball"
    rm -rf "$out"

    local os
    case "$(uname -s)" in
        Linux)  os="linux" ;;
        Darwin) os="darwin" ;;
        *)      os="$(uname -s | tr '[:upper:]' '[:lower:]')" ;;
    esac

    local staging="${out}/rivers-${VERSION}-${os}-${ARCH}"
    mkdir -p "${staging}/bin"
    mkdir -p "${staging}/config"

    for bin in "${BINARIES[@]}"; do
        cp "target/release/${bin}" "${staging}/bin/${bin}"
        chmod 0755 "${staging}/bin/${bin}"
    done
    cp packaging/config/riversd.toml "${staging}/config/riversd.toml"
    cp LICENSE "${staging}/"
    cp README.md "${staging}/"

    tar -czf "${out}/rivers-${VERSION}-${os}-${ARCH}.tar.gz" \
        -C "$out" "rivers-${VERSION}-${os}-${ARCH}"

    echo "  -> ${out}/rivers-${VERSION}-${os}-${ARCH}.tar.gz"
}

# ── Main ────────────────────────────────────────────────────────────

case "${1:-help}" in
    deb)     build_deb ;;
    rpm)     build_rpm ;;
    windows) build_windows ;;
    tarball) build_tarball ;;
    all)
        build_tarball
        build_deb
        build_rpm
        build_windows
        echo ""
        echo "=== All packages built ==="
        find "$DIST_DIR" -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.zip' -o -name '*.tar.gz' \) | sort
        ;;
    *)
        echo "Usage: $0 <deb|rpm|windows|tarball|all>"
        echo ""
        echo "  deb      Build .deb packages (rivers, rivers-lib, rivers-plugins)"
        echo "  rpm      Build .rpm packages (rivers, rivers-lib, rivers-plugins)"
        echo "  windows  Build Windows .zip (cross-compiled x86_64)"
        echo "  tarball  Build portable .tar.gz for current platform"
        echo "  all      Build all formats"
        exit 1
        ;;
esac
