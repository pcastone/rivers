# Epic: Rust Documentation — Crate-by-Crate

> **Goal:** Add comprehensive `///` doc comments and `//!` module docs to every public item across all 27 crates. Each crate is its own sprint.

**Standard per-crate:**
1. Add `#![warn(missing_docs)]` to crate root (`lib.rs` or `main.rs`)
2. Add crate-level `//!` module documentation
3. Add `//!` docs to each submodule
4. Document all public structs, enums, and type aliases
5. Document all public functions, methods, and trait impls
6. Document all public traits and their required methods
7. Add `# Examples` for key APIs (trait entry points, builders, public constructors)
8. Verify: `cargo doc -p <crate> --no-deps 2>&1 | grep warning` shows zero missing_docs warnings

**Convention:**
- Use `///` for items, `//!` for modules/crates
- First line is a one-sentence summary (imperative for functions, noun-phrase for types)
- Include `# Arguments`, `# Returns`, `# Errors`, `# Panics` sections where applicable
- Include `# Examples` with ```` ```rust ```` blocks for key public APIs
- Cross-reference related types with `[`TypeName`]` intra-doc links

---

## Phase 1: SDK Crates (Foundation)

These define the trait contracts other crates implement.

### Sprint 1 — rivers-driver-sdk ✅
**Crate:** `crates/rivers-driver-sdk` | 14 files | 89 pub items | 194 existing docs
**Effort:** Review & fill gaps — already well-documented
- [x] S1.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [x] S1.2 Add crate-level `//!` docs explaining the driver contract system
- [x] S1.3 Document all submodules (`//!` per mod file)
- [x] S1.4 Fill missing docs on public structs/enums (40 items fixed)
- [x] S1.5 Fill missing docs on public functions/methods
- [x] S1.6 Existing traits already have docs — no new examples needed
- [x] S1.7 Verify: `cargo doc -p rivers-driver-sdk --no-deps` — zero warnings, 38 tests pass

### Sprint 2 — rivers-engine-sdk ✅
**Crate:** `crates/rivers-engine-sdk` | 1 file | 11 pub items | 33 existing docs
**Effort:** Small — review & fill gaps
- [x] S2.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [x] S2.2 Enhanced crate-level `//!` docs with ABI symbol list and cross-references
- [x] S2.3 Documented `SerializedDatasource`, `SerializedLib`, `EngineConfig` fields (13 items)
- [x] S2.4 `# Safety` sections already present on `buffer_to_json` and `free_json_buffer`
- [x] S2.5 Verify: `cargo doc -p rivers-engine-sdk --no-deps` — zero warnings, 5 tests pass

---

## Phase 2: Core Infrastructure

### Sprint 3 — rivers-core-config
**Crate:** `crates/rivers-core-config` | 11 files | 64 pub items | 63 existing docs
**Effort:** Moderate — ~50% coverage, needs struct field docs
- [ ] S3.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S3.2 Add crate-level `//!` docs explaining config types and their role
- [ ] S3.3 Document all submodules
- [ ] S3.4 Document all config structs and their fields (ServerConfig, DatasourceConfig, etc.)
- [ ] S3.5 Document all enums and variants
- [ ] S3.6 Document the `StorageEngine` trait and its methods
- [ ] S3.7 Add `# Examples` for config construction patterns
- [ ] S3.8 Verify: `cargo doc -p rivers-core-config --no-deps` — zero warnings

### Sprint 4 — rivers-core
**Crate:** `crates/rivers-core` | 8 files | 53 pub items | 81 existing docs
**Effort:** Moderate
- [ ] S4.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S4.2 Add crate-level `//!` docs explaining core types (DriverFactory, StorageEngine, EventBus, LockBox integration)
- [ ] S4.3 Document all submodules
- [ ] S4.4 Document all public structs, enums, type aliases
- [ ] S4.5 Document all public functions and methods
- [ ] S4.6 Add `# Examples` for `DriverFactory` registration and lookup
- [ ] S4.7 Verify: `cargo doc -p rivers-core --no-deps` — zero warnings

### Sprint 5 — rivers-runtime
**Crate:** `crates/rivers-runtime` | 15 files | 132 pub items | 214 existing docs
**Effort:** Large — facade crate, many re-exports
- [ ] S5.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S5.2 Add crate-level `//!` docs explaining the runtime facade and re-export strategy
- [ ] S5.3 Document all submodules (ProcessPool types, shared types)
- [ ] S5.4 Document all public structs and enums
- [ ] S5.5 Document all public functions and methods
- [ ] S5.6 Document all re-exported items (ensure re-exports carry docs)
- [ ] S5.7 Add `# Examples` for `TaskContextBuilder`, `ProcessPool` usage
- [ ] S5.8 Verify: `cargo doc -p rivers-runtime --no-deps` — zero warnings

### Sprint 6 — rivers-storage-backends
**Crate:** `crates/rivers-storage-backends` | 3 files | 4 pub items | 9 existing docs
**Effort:** Small
- [ ] S6.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S6.2 Add crate-level `//!` docs explaining the storage backend implementations
- [ ] S6.3 Document `SqliteStorageEngine` and `RedisStorageEngine` structs and methods
- [ ] S6.4 Document any public configuration types
- [ ] S6.5 Verify: `cargo doc -p rivers-storage-backends --no-deps` — zero warnings

---

## Phase 3: Security Crates

### Sprint 7 — rivers-lockbox-engine
**Crate:** `crates/rivers-lockbox-engine` | 7 files | 33 pub items | 56 existing docs
**Effort:** Moderate
- [ ] S7.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S7.2 Add crate-level `//!` docs explaining Age-encrypted keystore, threat model, key sources
- [ ] S7.3 Document all submodules
- [ ] S7.4 Document all public types (`LockBox`, `LockBoxResolver`, `LockBoxError`, `ResolvedEntry`)
- [ ] S7.5 Document all public functions (`startup_resolve`, `resolve_key_source`, `collect_references`)
- [ ] S7.6 Add `# Security` sections for encryption/decryption functions
- [ ] S7.7 Verify: `cargo doc -p rivers-lockbox-engine --no-deps` — zero warnings

### Sprint 8 — rivers-keystore-engine
**Crate:** `crates/rivers-keystore-engine` | 5 files | 11 pub items | 18 existing docs
**Effort:** Small-moderate
- [ ] S8.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S8.2 Add crate-level `//!` docs explaining AES-256-GCM keystore, key lifecycle, AAD
- [ ] S8.3 Document all public types and methods
- [ ] S8.4 Add `# Security` sections for encrypt/decrypt functions
- [ ] S8.5 Add `# Examples` for key generation and encrypt/decrypt round-trip
- [ ] S8.6 Verify: `cargo doc -p rivers-keystore-engine --no-deps` — zero warnings

### Sprint 9 — rivers-lockbox (CLI)
**Crate:** `crates/rivers-lockbox` | 1 file | 0 pub items | 0 existing docs
**Effort:** Small — binary crate, document main and CLI structure
- [ ] S9.1 Add crate-level `//!` docs explaining the CLI tool and its subcommands
- [ ] S9.2 Document internal functions and types
- [ ] S9.3 Verify: `cargo doc -p rivers-lockbox --no-deps` — zero warnings

### Sprint 10 — rivers-keystore (CLI)
**Crate:** `crates/rivers-keystore` | 1 file | 0 pub items | 2 existing docs
**Effort:** Small — binary crate
- [ ] S10.1 Add crate-level `//!` docs explaining the CLI tool and its subcommands
- [ ] S10.2 Document internal functions and types
- [ ] S10.3 Verify: `cargo doc -p rivers-keystore --no-deps` — zero warnings

---

## Phase 4: Engine Crates

### Sprint 11 — rivers-engine-v8
**Crate:** `crates/rivers-engine-v8` | 4 files | 6 pub items | 2 existing docs
**Effort:** Moderate — poorly documented, needs full pass
- [ ] S11.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S11.2 Add crate-level `//!` docs explaining V8 isolate pool, C-ABI contract, host callbacks
- [ ] S11.3 Document all submodules
- [ ] S11.4 Document all public types and ABI export functions
- [ ] S11.5 Add `# Safety` sections for all `unsafe`/`extern "C"` functions
- [ ] S11.6 Verify: `cargo doc -p rivers-engine-v8 --no-deps` — zero warnings

### Sprint 12 — rivers-engine-wasm
**Crate:** `crates/rivers-engine-wasm` | 1 file | 6 pub items | 10 existing docs
**Effort:** Small
- [ ] S12.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S12.2 Add crate-level `//!` docs explaining Wasmtime engine, C-ABI contract
- [ ] S12.3 Document all public types and ABI export functions
- [ ] S12.4 Add `# Safety` sections for all `unsafe`/`extern "C"` functions
- [ ] S12.5 Verify: `cargo doc -p rivers-engine-wasm --no-deps` — zero warnings

---

## Phase 5: Built-in Drivers

### Sprint 13 — rivers-drivers-builtin
**Crate:** `crates/rivers-drivers-builtin` | 14 files | 34 pub items | 92 existing docs
**Effort:** Moderate — already well-documented, review & fill gaps
- [ ] S13.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S13.2 Add crate-level `//!` docs explaining built-in driver roster (Faker, Postgres, MySQL, SQLite, Redis, Memcached, RPS)
- [ ] S13.3 Document all submodules (one per driver)
- [ ] S13.4 Fill missing docs on public items
- [ ] S13.5 Add `# Examples` for `FakerDriver` (most commonly used in dev)
- [ ] S13.6 Verify: `cargo doc -p rivers-drivers-builtin --no-deps` — zero warnings

---

## Phase 6: Plugin Drivers (Batch 1 — larger crates)

### Sprint 14 — rivers-plugin-exec
**Crate:** `crates/rivers-plugin-exec` | 13 files | 29 pub items | 55 existing docs
**Effort:** Moderate — largest plugin
- [ ] S14.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S14.2 Add crate-level `//!` docs explaining ExecDriver (stdin JSON I/O, SHA-256 integrity, input modes)
- [ ] S14.3 Document all submodules
- [ ] S14.4 Document all public types and methods
- [ ] S14.5 Add `# Security` sections for integrity checking and sandboxing
- [ ] S14.6 Verify: `cargo doc -p rivers-plugin-exec --no-deps` — zero warnings

### Sprint 15 — rivers-plugin-influxdb
**Crate:** `crates/rivers-plugin-influxdb` | 5 files | 6 pub items | 19 existing docs
**Effort:** Small
- [ ] S15.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S15.2 Add crate-level `//!` docs
- [ ] S15.3 Document all public types and methods
- [ ] S15.4 Verify: `cargo doc -p rivers-plugin-influxdb --no-deps` — zero warnings

### Sprint 16 — rivers-plugin-elasticsearch
**Crate:** `crates/rivers-plugin-elasticsearch` | 1 file | 4 pub items | 4 existing docs
**Effort:** Small
- [ ] S16.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S16.2 Add crate-level `//!` docs
- [ ] S16.3 Document all public types and ABI exports
- [ ] S16.4 Verify: `cargo doc -p rivers-plugin-elasticsearch --no-deps` — zero warnings

### Sprint 17 — rivers-plugin-mongodb
**Crate:** `crates/rivers-plugin-mongodb` | 1 file | 4 pub items | 8 existing docs
**Effort:** Small
- [ ] S17.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S17.2 Add crate-level `//!` docs
- [ ] S17.3 Document all public types and ABI exports
- [ ] S17.4 Verify: `cargo doc -p rivers-plugin-mongodb --no-deps` — zero warnings

### Sprint 18 — rivers-plugin-redis-streams
**Crate:** `crates/rivers-plugin-redis-streams` | 1 file | 5 pub items | 11 existing docs
**Effort:** Small
- [ ] S18.1 Add `#![warn(missing_docs)]` to `lib.rs`
- [ ] S18.2 Add crate-level `//!` docs
- [ ] S18.3 Document all public types and ABI exports
- [ ] S18.4 Verify: `cargo doc -p rivers-plugin-redis-streams --no-deps` — zero warnings

---

## Phase 7: Plugin Drivers (Batch 2 — single-file crates)

### Sprint 19 — rivers-plugin-kafka
**Crate:** `crates/rivers-plugin-kafka` | 1 file | 7 pub items | 7 existing docs
- [ ] S19.1 Add `#![warn(missing_docs)]` + crate-level `//!` docs
- [ ] S19.2 Document all public types and ABI exports
- [ ] S19.3 Verify: `cargo doc -p rivers-plugin-kafka --no-deps` — zero warnings

### Sprint 20 — rivers-plugin-rabbitmq
**Crate:** `crates/rivers-plugin-rabbitmq` | 1 file | 6 pub items | 6 existing docs
- [ ] S20.1 Add `#![warn(missing_docs)]` + crate-level `//!` docs
- [ ] S20.2 Document all public types and ABI exports
- [ ] S20.3 Verify: `cargo doc -p rivers-plugin-rabbitmq --no-deps` — zero warnings

### Sprint 21 — rivers-plugin-nats
**Crate:** `crates/rivers-plugin-nats` | 1 file | 6 pub items | 5 existing docs
- [ ] S21.1 Add `#![warn(missing_docs)]` + crate-level `//!` docs
- [ ] S21.2 Document all public types and ABI exports
- [ ] S21.3 Verify: `cargo doc -p rivers-plugin-nats --no-deps` — zero warnings

### Sprint 22 — rivers-plugin-cassandra
**Crate:** `crates/rivers-plugin-cassandra` | 1 file | 4 pub items | 5 existing docs
- [ ] S22.1 Add `#![warn(missing_docs)]` + crate-level `//!` docs
- [ ] S22.2 Document all public types and ABI exports
- [ ] S22.3 Verify: `cargo doc -p rivers-plugin-cassandra --no-deps` — zero warnings

### Sprint 23 — rivers-plugin-couchdb
**Crate:** `crates/rivers-plugin-couchdb` | 1 file | 4 pub items | 0 existing docs
**Effort:** Needs full pass — zero docs
- [ ] S23.1 Add `#![warn(missing_docs)]` + crate-level `//!` docs
- [ ] S23.2 Document all public types and ABI exports
- [ ] S23.3 Verify: `cargo doc -p rivers-plugin-couchdb --no-deps` — zero warnings

### Sprint 24 — rivers-plugin-ldap
**Crate:** `crates/rivers-plugin-ldap` | 1 file | 4 pub items | 0 existing docs
**Effort:** Needs full pass — zero docs
- [ ] S24.1 Add `#![warn(missing_docs)]` + crate-level `//!` docs
- [ ] S24.2 Document all public types and ABI exports
- [ ] S24.3 Verify: `cargo doc -p rivers-plugin-ldap --no-deps` — zero warnings

---

## Phase 8: Binary Crates

### Sprint 25 — riversd
**Crate:** `crates/riversd` | 92 files | 384 pub items | 960 existing docs
**Effort:** Very large — largest crate, but already has decent coverage
- [ ] S25.1 Add `#![warn(missing_docs)]` to `main.rs`
- [ ] S25.2 Add crate-level `//!` docs explaining the server binary, startup sequence, architecture
- [ ] S25.3 Document all submodules (`//!` per mod file — server, view_engine, guard, graphql, websocket, sse, streaming, polling, deployment, middleware, etc.)
- [ ] S25.4 Fill missing docs on public structs and enums (gap analysis)
- [ ] S25.5 Fill missing docs on public functions and methods
- [ ] S25.6 Document the `AppContext` struct and all its fields
- [ ] S25.7 Document the middleware pipeline (order, purpose of each layer)
- [ ] S25.8 Verify: `cargo doc -p riversd --no-deps` — zero warnings

### Sprint 26 — riversctl
**Crate:** `crates/riversctl` | 8 files | 30 pub items | 26 existing docs
**Effort:** Moderate
- [ ] S26.1 Add `#![warn(missing_docs)]` to `main.rs`
- [ ] S26.2 Add crate-level `//!` docs explaining the CLI tool and subcommands
- [ ] S26.3 Document all submodules (command modules)
- [ ] S26.4 Document all public types and functions
- [ ] S26.5 Verify: `cargo doc -p riversctl --no-deps` — zero warnings

### Sprint 27 — riverpackage
**Crate:** `crates/riverpackage` | 1 file | 0 pub items | 0 existing docs
**Effort:** Small — binary crate with no public API
- [ ] S27.1 Add crate-level `//!` docs explaining the bundle packaging tool
- [ ] S27.2 Document internal functions
- [ ] S27.3 Verify: `cargo doc -p riverpackage --no-deps` — zero warnings

---

## Acceptance Criteria

- [ ] AC1: Every crate has `#![warn(missing_docs)]` (where applicable — lib crates)
- [ ] AC2: Every crate has crate-level `//!` documentation
- [ ] AC3: Every public item has a `///` doc comment
- [ ] AC4: All `unsafe` functions have `# Safety` sections
- [ ] AC5: Key traits have `# Examples` sections
- [ ] AC6: `cargo doc --workspace --no-deps` compiles with zero `missing_docs` warnings
- [ ] AC7: Intra-doc links resolve correctly (`cargo doc` produces no broken link warnings)

## Summary

| Phase | Sprints | Crates | Est. Pub Items | Current Doc Coverage |
|-------|---------|--------|----------------|---------------------|
| 1 — SDK | 1-2 | 2 | 100 | Good |
| 2 — Core | 3-6 | 4 | 253 | Moderate |
| 3 — Security | 7-10 | 4 | 44 | Mixed |
| 4 — Engines | 11-12 | 2 | 12 | Poor |
| 5 — Builtin | 13 | 1 | 34 | Good |
| 6 — Plugins 1 | 14-18 | 5 | 48 | Mixed |
| 7 — Plugins 2 | 19-24 | 6 | 31 | Low/None |
| 8 — Binaries | 25-27 | 3 | 414 | Moderate |
| **Total** | **27** | **27** | **~936** | |
