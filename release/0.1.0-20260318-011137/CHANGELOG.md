# Rivers 0.1.0 — Release Changelog

**Released:** 2026-03-18
**Build:** `0.1.0-20260318-011137`
**Stack:** Rust, Axum 0.8, V8 (rusty_v8 130), Wasmtime 27, tokio
**Tests:** 1321 passing across 14 crates

---

## Added

### TLS — Mandatory on main and admin servers (Phase AD)

- `[base.tls]` config section — mandatory at startup; absence is a hard error
- Auto-generated self-signed certs at `{data_dir}/tls/auto-{app_id}.crt/.key` when `cert`/`key` paths are absent
- Admin server TLS — always required; no plain-HTTP fallback even on localhost
- `[base.admin_api.tls]` config section — auto-gen if cert/key absent
- HTTP redirect server — listens on port 80, redirects HTTP → HTTPS (301); configurable via `redirect_port`
- `--no-ssl` debug flag — disables TLS on main server only for the session; admin TLS always enforced
- `--port` flag — valid only with `--no-ssl` (rejected otherwise)
- `rivers_core::tls` module — shared cert generation and cert/key pair validation
- `riversd::tls` module — startup validation (`validate_tls_config`, `validate_admin_tls_config`), auto-gen orchestration, TLS acceptor loading
- `[base.tls.x509]` config section — common_name, organization, country, state, locality, san, days for auto-gen and `riversctl tls gen`/`request`
- `[base.tls.engine]` config section — min_version (tls12/tls13), explicit cipher suites (empty = rustls defaults)
- `data_dir` and `app_id` top-level server config fields — used in auto-gen cert paths

### riversctl tls Subcommands (Phase AD)

- `riversctl tls gen` — generate self-signed cert from `[base.tls.x509]` fields
- `riversctl tls request` — generate a CSR for CA submission (prints to stdout)
- `riversctl tls import <cert> <key>` — validate and install a CA-signed cert pair
- `riversctl tls show` — display subject, SANs, fingerprint, validity window, time remaining
- `riversctl tls list` — list all cert files managed by Rivers with paths and expiry dates
- `riversctl tls expire --yes` — purge cert files to force re-gen on next startup
- `--port` targeting on all `tls` subcommands — select main server (default) or admin server cert

### Address Book Bundle (Phase AC)

- `address-book-bundle/` — two-app bundle shipped with this release
  - `address-book-service` (port 9100) — REST API backed by faker datasource; 4 DataViews, 4 endpoints
  - `address-book-main` (port 8080) — Svelte SPA proxying to address-book-service; static file serving with SPA fallback
- Compiled SPA assets: `spa/bundle.js` (41 KB) + `spa/bundle.css` (2.2 KB)
- `config/riversd.toml` — wired to load `address-book-bundle/` on startup

### ProcessPool — V8 and Wasmtime (Phase X)

- `V8Worker` — full implementation: heap limits, heap recycling above threshold, isolate-per-task isolation
- `WasmtimeWorker` — full implementation: epoch preemption (10ms watchdog), memory limits via `StoreLimitsBuilder`, WAT text format support
- `ctx.store` — real `StorageEngine` backend (`set`/`get`/`del` with TTL); falls back to in-memory on error
- `ctx.datasource().fromQuery().build()` — async bridge to `DriverFactory` → `Connection` → `QueryResult`
- `ctx.dataview()` — live execution via `DataViewExecutor` when data is not pre-fetched
- `ctx.http` — capability-gated outbound HTTP; only injected when view declares `allow_outbound_http = true`
- `Rivers.log.{info,warn,error}` — structured logging with optional fields second argument
- Host function bindings for Wasmtime: `rivers.log_info/warn/error` → tracing
- `DataViewExecutor` — registry + factory + execute facade; wired to bundle auto-load path

### Admin API (Phase Y)

- `/admin/deploy`, `/admin/deploy/test`, `/admin/deploy/approve`, `/admin/deploy/reject`, `/admin/deploy/promote` — real bundle deployment lifecycle via `DeploymentManager`
- `/admin/deployments` — live deployment list
- `/admin/drivers` — built-in driver catalog
- `/admin/datasources` — live pool snapshots from `DataViewExecutor`
- `/admin/log/levels`, `/admin/log/set`, `/admin/log/reset` — dynamic log level changes at runtime
- `LogController` — type-erased handle to tracing reload layer; wired through `AppContext`
- Ed25519 authentication required unconditionally when `admin_api.enabled = true` (SHAPE-25)

### CLI (Phase Y)

- `riversctl doctor` — 5 pre-launch checks: config parse, config validate, process pool engines, storage, lockbox permissions
- `riversctl preflight` — bundle load → validate → schema dir check
- `rivers-lockbox init/add/alias/rotate/remove/rekey` — full keystore write-back with age x25519 encryption, 0o600 permissions

---

## Improved

### TLS

- Main server now uses `hyper_util::server::conn::auto::Builder` TLS accept loop — required for `TlsAcceptor` wrapping; replaces `axum::serve`
- Admin server uses same accept loop pattern — consistent with main server
- HTTP redirect returns `301 Moved Permanently` — `axum::Redirect::permanent()` returns 308, spec requires 301
- Auto-gen cert reuse policy — validates existing pair on restart; regenerates if cert/key pair is invalid (not silent reuse)
- Ed25519 signature payload field order corrected — `timestamp` before `body_sha256_hex` (matches `admin_auth.rs`)

### Startup Sequence

- `validate_server_tls(&config, no_ssl)` replaces `config.validate()` — TLS check is first, before any resource allocation
- Admin TLS validation runs regardless of `--no-ssl` — admin server always requires TLS
- `maybe_autogen_admin_tls_cert` added to startup sequence (steps 4a/4b)

### Config

- `[base.tls]` replaces `http2.tls_cert`/`tls_key` — TLS belongs to the server, not the HTTP/2 protocol config
- Session config path corrected to `[session]` (not `[base.cluster.session_store]`)
- Default `port` corrected to `8080` (not 443)

### ProcessPool

- V8 heap recycling — checks `v8::HeapStatistics` after each task, drops isolate if above threshold
- Epoch preemption — separate watchdog thread increments engine epoch every 10ms
- Capability enforcement — undeclared datasource access returns `CapabilityError`
- `json_to_query_value()` — converts V8 JSON params to `QueryValue` for driver execution
- Wasmtime trap detection — downcasts `wasmtime::Trap` → `TaskError::Timeout` for fuel/epoch exhaustion

---

## Fixed

### Admin Auth

- Removed localhost plain-HTTP bypass — `127.0.0.1` binding no longer skips Ed25519 verification (SHAPE-25)
- Ed25519 unconditionally required when `admin_api.enabled = true` regardless of bind address

### Config Parsing

- `bundle_path` must be top-level in `riversd.toml` — placing it after `[base]` silently ignored it as `base.bundle_path`
- `AdminTlsConfig` cert fields made `Option<String>` — were previously required `String`, blocking auto-gen path
- `Http2Config.tls_cert`/`tls_key` removed — stale fields that shadowed new `[base.tls]` path

### CLI

- `--port` without `--no-ssl` now returns a clear error — previously accepted silently
- `riversctl tls expire` — requires `--yes` flag; without it prints warning and exits (was immediately destructive)
- `riversctl tls cmd_request` CSR generation — correct rcgen 0.13.2 API: `params.serialize_request(&key_pair)?.pem()?`
- `::time::OffsetDateTime::now_utc()` absolute path — `x509-parser` shadows the `time` module name

### Validation

- `redirect_port == base.port` now rejected at startup — previously would bind two servers on the same port
- `require_client_cert = true` without `ca_cert` now rejected — previously passed validation silently
- HTTP/2 without `[base.tls]` rejected at startup — cannot negotiate ALPN without TLS

### Tests

- `http2_without_tls_rejected` — updated to accept either `"TLS is required"` or `"HTTP/2 requires TLS"` after check order change
- `tls_config_present_passes_validation` — migrated to `[base.tls]` path from removed `http2.tls_cert`
- `hot_reload_tests.rs:changed_tls_requires_restart` — uses `base.tls` instead of removed `http2.tls_cert`
- `wasmtime_worker_returns_engine_unavailable` → `wasmtime_worker_creates_successfully` — Wasmtime worker now initializes successfully
- `v8_worker_returns_engine_unavailable` → `v8_worker_creates_successfully` — V8 worker now initializes successfully

---

## Known Limitations

- **CORS** — configured in `SecurityConfig` (server-level); per-app `app.cors()` init handler deferred to SHAPE-23
- **Rate limiting** — configured in `SecurityConfig` (server-level); per-app `[app.rate_limit]` in `app.toml` deferred to SHAPE-24
- **Hot reload** — module implemented but not wired into startup; deferred to Phase AE
- **Request observer middleware** — stub in middleware stack; deferred to Phase AE
- **Admin IP allowlist** — exact IP match only; CIDR range support deferred to Phase AE
- **RPS / Gossip processing** — gossip endpoint registered but processing deferred to V2
- **GraphQL** — stub handler; deferred

---

## Binaries

| Binary | Size | Purpose |
|--------|------|---------|
| `riversd` | 41.5 MB | HTTP server daemon |
| `riversctl` | 4.4 MB | Control CLI |
| `rivers-lockbox` | 979 KB | Secrets management CLI |
| `riverpackage` | 625 KB | Bundle validation CLI |
