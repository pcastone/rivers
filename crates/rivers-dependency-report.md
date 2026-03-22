# Rivers Dependency Replacement Report

**Project:** Rivers v0.50.1
**Codebase:** 168 Rust source files, ~62,500 lines
**Date:** 2026-03-21

**Excluded from analysis:** tokio (all), serde (all), toml, and all database/LDAP/Kafka libs (rusqlite, redis, tokio-postgres, mysql_async, rskafka, lapin, async-nats, mongodb, async-memcached, scylla, ldap3).

---

## Summary Table

| Library | Version | References | Files | Crates Used In | Effort (hrs) | Difficulty |
|---------|---------|-----------|-------|----------------|-------------|------------|
| **v8** | 130 | ~298 | 3 | rivers-engine-v8, riversd | 120–160 | Very High |
| **axum** | 0.8 | ~101 | 12 | riversd | 80–120 | Very High |
| **wasmtime** | 42 | ~35 | 4 | rivers-engine-wasm, riversd | 60–80 | High |
| **tracing** / tracing-subscriber / tracing-appender | 0.1 / 0.3 / 0.2 | ~216 | 36+ | Nearly all | 40–60 | High |
| **thiserror** | 2 | ~190 | 27 | Nearly all | 16–24 | Medium |
| **async-trait** | 0.1 | ~87 | 37 | Nearly all | 12–20 | Medium |
| **async-graphql** / async-graphql-axum | 7 | ~39 | 2 | riversd | 40–60 | High |
| **reqwest** | 0.12 | ~47 | 9 | rivers-driver-sdk, plugins, riversctl, riversd | 24–40 | Medium-High |
| **chrono** | 0.4 | ~57 | 28 | Most crates | 16–24 | Medium |
| **schemars** | 0.8 | ~77 | 7 | rivers-core-config, rivers-core, rivers-data | 12–16 | Medium |
| **age** | 0.11 | ~20 | 4 (prod) | rivers-lockbox, rivers-lockbox-engine | 24–40 | High |
| **uuid** | 1 | ~18 | 8 | rivers-core, rivers-engine-v8, riversd, plugins | 4–6 | Low |
| **sha2** | 0.10 | ~12 | 8 | rivers-core, rivers-data, rivers-engine-v8, riversctl | 4–6 | Low |
| **hex** | 0.4 | ~18 | 11 | rivers-core, rivers-data, rivers-engine-v8, plugins | 2–4 | Low |
| **base64** | 0.22 | ~8 | 4 | rivers-engine-v8, rivers-driver-sdk, plugins, riversd | 2–4 | Low |
| **rcgen** | 0.13 | ~4 | 1 | rivers-core (tls feature) | 8–12 | Medium |
| **rustls** / rustls-pemfile / tokio-rustls | 0.23 / 2 / 0.26 | ~10 | 2–3 | rivers-core, riversd | 16–24 | Medium-High |
| **ed25519-dalek** | 2 | ~3 | 3 | riversctl, riversd | 6–10 | Medium |
| **rand** | 0.8 | ~9 | 3 | rivers-engine-v8, riversctl, riversd | 3–5 | Low |
| **hyper** / hyper-util | 1 / 0.1 | ~8 | 1 | riversd | 8–12 | Medium |
| **tower** / tower-http | 0.5 / 0.6 | ~6 | 3 | riversd | 6–10 | Medium |
| **http** | 1 | ~5 | 2 | riversd | 2–4 | Low |
| **libloading** | 0.8 | ~10 | 3 | rivers-core, rivers-engine-sdk, riversd | 8–12 | Medium |
| **notify** | 7 | ~4 | 1 | riversd (hot_reload.rs, 262 lines) | 8–12 | Medium |
| **time** | 0.3 | ~4 | 1 | rivers-core (tls.rs) | 2–3 | Low |
| **x509-parser** | 0.16 | ~2 | 1 | rivers-core (tls.rs) | 4–6 | Medium |
| **hmac** | 0.12 | ~2 | 2 | rivers-engine-v8, riversd | 2–3 | Low |
| **bcrypt** | 0.15 | ~4 | 2 | rivers-engine-v8, riversd | 2–4 | Low |
| **zeroize** | 1 | ~2 | 2 | rivers-lockbox, rivers-lockbox-engine | 2–4 | Low |
| **ipnet** | 2 | ~1 | 1 | riversd (admin.rs) | 1–2 | Low |
| **percent-encoding** | 2 | ~2 | 1 | riversd | 1 | Low |
| **tempfile** | 3 | ~29 | 17 | Test-only (dev-dependencies) | 2–4 | Low |
| **futures-lite** | 2 | ~2 | 2 | plugins (nats, rabbitmq) | 1–2 | Low |
| **bytes** | 1 | ~1 | 1 | plugin (nats) | 1 | Low |

---

## Detailed Analysis by Library

### Tier 1 — Core Architecture (would require significant redesign)

**v8 (130)** — 298 references across 3 files (~2,668 lines of v8-touching code)
The JavaScript engine powering Rivers' transform/scripting layer. Deeply embedded in `rivers-engine-v8/src/lib.rs` (852 lines) and `riversd/src/process_pool/v8_engine.rs` (1,816 lines). Every V8 isolate lifecycle, script compilation, host function binding, and memory management call goes through this. Replacing it means rewriting the entire JS runtime: isolate pooling, host function injection (crypto, HTTP, logging), memory limits, and epoch-based interruption. The only realistic replacement is Deno's `deno_core` or embedding QuickJS/Boa, each with its own API surface.
**Effort: 120–160 hours**

**axum (0.8)** — 101 references across 12 files
The HTTP framework for `riversd`. Underpins routing, middleware, extractors, WebSocket upgrades, CORS, backpressure, static files, GraphQL mounting, and all admin/API handlers. The server.rs alone is 2,260 lines. Replacing axum means migrating every route definition, every extractor type, all middleware (tower layers stay but the glue changes), and WebSocket handling. Alternatives: actix-web, poem, or raw hyper—all require full handler rewrites.
**Effort: 80–120 hours**

**wasmtime (42)** — 35 references across 4 files (~632 lines)
The WASM engine for Rivers' plugin/transform system. Used for module compilation, store/linker setup, host function imports (`rivers.log_info`, etc.), memory access, fuel metering, and epoch interruption. Replacing means migrating to wasmer or wasm3—different APIs for stores, linkers, and host bindings.
**Effort: 60–80 hours**

**tracing + tracing-subscriber + tracing-appender** — 216+ references across 36+ files
Pervasive structured logging throughout every crate. `info!`, `warn!`, `error!`, `debug!`, `trace!` macros everywhere, plus `#[instrument]` annotations. The subscriber setup in `riversd/src/main.rs` configures JSON output, file appenders, env-filter, and dynamic log level reload. Replacing means touching nearly every file in the project and rebuilding the subscriber pipeline. Alternatives: `log` + `env_logger` (loses structured fields), `slog`, or rolling your own.
**Effort: 40–60 hours**

**async-graphql + async-graphql-axum (7)** — 39 references across 2 files
Powers the GraphQL API in `riversd/src/graphql.rs` (866 lines) and is mounted in `server.rs`. Includes query/mutation/subscription resolvers, the schema builder, and the playground endpoint. Replacing means rewriting all resolvers with `juniper` or building a custom GraphQL layer.
**Effort: 40–60 hours**

### Tier 2 — Significant but Contained

**reqwest (0.12)** — 47 references across 9 files
HTTP client used in the driver SDK's `http_executor.rs` (1,314 lines), multiple plugins (CouchDB, Elasticsearch, InfluxDB), `riversctl`, and `riversd`. Handles request building, TLS, JSON deserialization, auth headers. Replacing with `hyper` + manual client setup or `ureq` (blocking) would touch every HTTP callsite.
**Effort: 24–40 hours**

**age (0.11)** — ~20 real references across 4 production files
Encryption library for the Lockbox subsystem. Used for x25519 key generation, encrypt/decrypt operations, and identity/recipient management in `rivers-lockbox` and `rivers-lockbox-engine`. The API surface is small but the cryptographic semantics are specific. Replacing means implementing age-compatible encryption with `x25519-dalek` + `chacha20poly1305` + the age file format, or switching to a different envelope encryption scheme entirely.
**Effort: 24–40 hours**

**thiserror (2)** — 190 references across 27 files
`#[derive(Error)]` and `#[error("...")]` on every error enum in the project. Purely a derive macro—mechanical replacement with manual `impl Display` + `impl Error`. Tedious but zero-risk.
**Effort: 16–24 hours**

**chrono (0.4)** — 57 references across 28 files
Date/time types (`Utc::now()`, `DateTime<Utc>`, `NaiveDate`) used throughout for timestamps, scheduling, and data formatting. The `time` crate is a viable replacement but the API differs enough that every callsite needs review.
**Effort: 16–24 hours**

**rustls + rustls-pemfile + tokio-rustls** — ~10 references across 2–3 files
TLS stack for the server. Used in `rivers-core/src/tls.rs` (298 lines) and `riversd` for certificate loading, TLS acceptor setup, and PEM parsing. Replacing with `native-tls` / `openssl` changes the entire TLS config approach.
**Effort: 16–24 hours**

**async-trait (0.1)** — 87 `#[async_trait]` annotations across 37 files
Used on every async trait definition and impl. Rust 1.75+ supports native async traits, so this is a mechanical removal IF your MSRV allows it. Each site needs checking for `Send` bounds and object safety.
**Effort: 12–20 hours** (or 4–6 hours with automated tooling if MSRV ≥ 1.75)

**schemars (0.8)** — 77 references across 7 files
`#[derive(JsonSchema)]` on config types for JSON Schema generation. Replacing means either hand-writing schemas or switching to `utoipa` / `typify`. Moderate because it's mostly derive macros on structs.
**Effort: 12–16 hours**

### Tier 3 — Moderate, Localized

**rcgen (0.13)** — 4 references in 1 file (`rivers-core/src/tls.rs`)
Self-signed certificate generation. Small surface area but crypto-sensitive. Could replace with `openssl` bindings or shell out to `openssl` CLI.
**Effort: 8–12 hours**

**libloading (0.8)** — 10 references across 3 files
Dynamic library loading for the plugin system. Used in `driver_factory.rs`, `engine_loader.rs`, and `rivers-engine-sdk`. Could replace with `dlopen2` or raw `libc::dlopen`. Small surface but FFI-sensitive.
**Effort: 8–12 hours**

**notify (7)** — 4 references in 1 file (`riversd/src/hot_reload.rs`, 262 lines)
Filesystem watcher for config hot-reload. Self-contained module. Could replace with `inotify` (Linux-only) or poll-based approach.
**Effort: 8–12 hours**

**hyper + hyper-util** — 8 references in 1 file
Used under axum for the low-level server binding. If you replace axum, this comes along for free. Standalone replacement: minimal.
**Effort: 8–12 hours** (standalone) / **0 hours** (if replaced with axum)

**ed25519-dalek (2)** — 3 references across 3 files
Admin API request signing in `riversctl` and `riversd/src/admin_auth.rs`. Small, contained crypto usage. Could swap to `ring` ed25519 or `p256`.
**Effort: 6–10 hours**

**tower + tower-http** — 6 references across 3 files
Middleware layers (compression, body limits, timeouts). Tightly coupled to axum—if axum stays, these stay. If axum goes, replacement depends on new framework.
**Effort: 6–10 hours** (standalone) / **0 hours** (bundled with axum replacement)

### Tier 4 — Low Effort / Quick Swaps

| Library | Notes | Effort |
|---------|-------|--------|
| **uuid** | `Uuid::new_v4()` calls. Trivial to replace with hand-rolled or `nanoid`. | 4–6 hrs |
| **sha2** | `Sha256::digest()` calls. Swap to `ring::digest` or `blake3`. | 4–6 hrs |
| **x509-parser** | 2 calls in tls.rs for cert expiry checking. | 4–6 hrs |
| **hex** | `hex::encode/decode`. Could inline with `format!("{:02x}")` or use `data-encoding`. | 2–4 hrs |
| **base64** | `base64::engine::general_purpose::STANDARD.encode/decode`. Swap to `data-encoding`. | 2–4 hrs |
| **rand** | `OsRng`, `thread_rng()`, random bytes. Swap to `getrandom` + manual. | 3–5 hrs |
| **hmac** | HMAC-SHA256 for request signing. 2 files. Swap to `ring::hmac`. | 2–3 hrs |
| **bcrypt** | Password hashing in v8 engine host functions. Swap to `argon2` or `ring`. | 2–4 hrs |
| **time** | 4 calls in tls.rs for `OffsetDateTime`. Only used because rcgen requires it. | 2–3 hrs |
| **zeroize** | `#[derive(Zeroize)]` on 2 secret-holding structs. Could `impl Drop` manually. | 2–4 hrs |
| **http** | `HeaderValue`, `StatusCode` types. Comes with axum; standalone swap trivial. | 2–4 hrs |
| **tempfile** | Test-only. `NamedTempFile` / `TempDir`. Could use `std::env::temp_dir` + manual cleanup. | 2–4 hrs |
| **ipnet** | 1 call parsing CIDR ranges for admin allow-lists. | 1–2 hrs |
| **percent-encoding** | 1 file, URL encoding. `urlencoding` crate or hand-roll. | 1 hr |
| **futures-lite** | 2 uses in plugin crates. Replace with `futures` or inline. | 1–2 hrs |
| **bytes** | 1 use in NATS plugin. Already in tokio ecosystem. | 1 hr |

---

## Total Estimated Effort

| Tier | Range |
|------|-------|
| Tier 1 — Core Architecture | 340–480 hrs |
| Tier 2 — Significant but Contained | 100–164 hrs |
| Tier 3 — Moderate, Localized | 36–56 hrs |
| Tier 4 — Low Effort | 30–50 hrs |
| **Total** | **506–750 hrs** |

This is roughly **3–5 engineer-months** to replace every non-excluded dependency. The overwhelming majority of the effort (67%+) lives in the Tier 1 libraries: v8, axum, wasmtime, tracing, and async-graphql.

---

*Report generated from static analysis of the Rivers workspace. Estimates assume a senior Rust engineer familiar with the codebase. Actual effort may vary based on test coverage requirements and desired API compatibility.*
