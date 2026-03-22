%global version 0.50.2
%global release 0

Name:           rivers
Version:        %{version}
Release:        %{release}%{?dist}
Summary:        Declarative app-service framework — build APIs with TOML config

License:        Apache-2.0
URL:            https://github.com/pcastone/rivers
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo >= 1.75
BuildRequires:  rust >= 1.75
BuildRequires:  gcc
BuildRequires:  python3

%description
Rivers is a declarative app-service framework written in Rust.
Define REST APIs, WebSocket, SSE, and GraphQL endpoints using
TOML configuration and JSON schemas — no application code required.

# ── Base package ────────────────────────────────────────────────────

%package -n rivers
Summary:        Rivers application server binaries
Requires:       rivers-lib = %{version}-%{release}
Recommends:     rivers-plugins = %{version}-%{release}

%description -n rivers
Server binaries, CLI tools, systemd service, and default configuration
for the Rivers application server.

Includes: riversd, riversctl, rivers-lockbox, riverpackage.

%pre -n rivers
getent group rivers > /dev/null 2>&1 || groupadd -r rivers
getent passwd rivers > /dev/null 2>&1 || \
    useradd -r -g rivers -d /var/lib/rivers -s /sbin/nologin \
    -c "Rivers application server" rivers

%post -n rivers
%systemd_post riversd.service

%preun -n rivers
%systemd_preun riversd.service

%postun -n rivers
%systemd_postun_with_restart riversd.service

%files -n rivers
%license LICENSE
%doc README.md NOTICE
%{_bindir}/riversd
%{_bindir}/riversctl
%{_bindir}/rivers-lockbox
%{_bindir}/riverpackage
%dir %{_sysconfdir}/rivers
%config(noreplace) %{_sysconfdir}/rivers/riversd.toml
%{_unitdir}/riversd.service
%dir %attr(0755, rivers, rivers) %{_sharedstatedir}/rivers
%dir %attr(0755, rivers, rivers) %{_localstatedir}/log/rivers
%dir %attr(0700, rivers, rivers) %{_sharedstatedir}/rivers/lockbox

# ── Lib package ─────────────────────────────────────────────────────

%package -n rivers-lib
Summary:        Rivers shared libraries and engines

%description -n rivers-lib
Shared runtime library, V8 JavaScript engine, Wasmtime WASM engine,
built-in database drivers, storage backends, and LockBox encryption
engine for the Rivers application server.

%files -n rivers-lib
%license LICENSE
%dir %{_libdir}/rivers
%{_libdir}/rivers/librivers_runtime.*
%{_libdir}/rivers/librivers_engine_v8.*
%{_libdir}/rivers/librivers_engine_wasm.*
%{_libdir}/rivers/librivers_drivers_builtin.*
%{_libdir}/rivers/librivers_storage_backends.*
%{_libdir}/rivers/librivers_lockbox_engine.*

# ── Plugins package ─────────────────────────────────────────────────

%package -n rivers-plugins
Summary:        Rivers datasource driver plugins
Requires:       rivers-lib = %{version}-%{release}

%description -n rivers-plugins
Optional datasource driver plugins for Rivers: MongoDB, Elasticsearch,
Cassandra, CouchDB, InfluxDB, Kafka, RabbitMQ, NATS, Redis Streams,
and LDAP.

%files -n rivers-plugins
%license LICENSE
%dir %{_libdir}/rivers/plugins
%{_libdir}/rivers/plugins/librivers_plugin_*

# ── Build ───────────────────────────────────────────────────────────

%build
cargo build --release

%install
rm -rf %{buildroot}

# Binaries
install -D -m 0755 target/release/riversd       %{buildroot}%{_bindir}/riversd
install -D -m 0755 target/release/riversctl      %{buildroot}%{_bindir}/riversctl
install -D -m 0755 target/release/rivers-lockbox %{buildroot}%{_bindir}/rivers-lockbox
install -D -m 0755 target/release/riverpackage   %{buildroot}%{_bindir}/riverpackage

# Config
install -D -m 0640 packaging/config/riversd.toml %{buildroot}%{_sysconfdir}/rivers/riversd.toml

# Systemd
install -D -m 0644 packaging/systemd/riversd.service %{buildroot}%{_unitdir}/riversd.service

# Libraries
install -d %{buildroot}%{_libdir}/rivers
for ext in so dylib; do
    for f in target/release/librivers_runtime.$ext \
             target/release/librivers_engine_v8.$ext \
             target/release/librivers_engine_wasm.$ext \
             target/release/librivers_drivers_builtin.$ext \
             target/release/librivers_storage_backends.$ext \
             target/release/librivers_lockbox_engine.$ext; do
        [ -f "$f" ] && install -m 0755 "$f" %{buildroot}%{_libdir}/rivers/ || true
    done
done

# Plugins
install -d %{buildroot}%{_libdir}/rivers/plugins
for ext in so dylib; do
    for f in target/release/librivers_plugin_*.$ext; do
        [ -f "$f" ] && install -m 0755 "$f" %{buildroot}%{_libdir}/rivers/plugins/ || true
    done
done

# State directories
install -d -m 0755 %{buildroot}%{_sharedstatedir}/rivers
install -d -m 0755 %{buildroot}%{_localstatedir}/log/rivers
install -d -m 0700 %{buildroot}%{_sharedstatedir}/rivers/lockbox

%changelog
* Sat Mar 22 2025 Paul Castone <paul.castone@gmail.com> - 0.50.2-0
- Initial public release
