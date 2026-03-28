# Tasks — Dream Pattern Fixes

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address the 5 patterns of concern identified in the 2026-03-28 dream analysis. Focus on structural fixes that prevent recurring problems and close test gaps in security-critical crates.

**Source:** `docs/dreams/dream-2026-03-28.md`

---

## File Structure

### Modified Files

| File | Changes |
|------|---------|
| `crates/riversd/src/view_engine.rs` | Extract `enrich_task_context()` helper, replace 6 builder sites |
| `crates/riversd/src/guard.rs` | Use `enrich_task_context()` at 3 builder sites |
| `crates/riversd/src/graphql.rs` | Use `enrich_task_context()` at 1 builder site |
| `crates/riversd/src/websocket.rs` | Use `enrich_task_context()` at 2 builder sites |
| `crates/riversd/src/sse.rs` | Use `enrich_task_context()` at 1 builder site |
| `crates/riversd/src/streaming.rs` | Use `enrich_task_context()` at 1 builder site |
| `crates/riversd/src/polling.rs` | Use `enrich_task_context()` at 1 builder site |
| `crates/riversd/src/message_consumer.rs` | Use `enrich_task_context()` at 2 builder sites |
| `crates/riversd/src/deployment.rs` | Use `enrich_task_context()` at 1 builder site |
| `crates/riversd/src/server.rs` | Extract AppContext into sub-structs |
| `crates/riversd/src/error_response.rs` | Consolidate error construction |
| `crates/rivers-storage-backends/src/redis_backend.rs` | Add tests |
| `crates/rivers-storage-backends/src/sqlite_backend.rs` | Add tests |
| `crates/rivers-lockbox-engine/src/lib.rs` | Add tests |

### New Files

| File | Responsibility |
|------|---------------|
| `crates/riversd/src/task_enrichment.rs` | `enrich_task_context()` — auto-wire capabilities from AppContext |

---

## 1. TaskContext Auto-Enrichment Layer (Pattern 1 fix)

**Problem:** 18 production dispatch sites call `TaskContextBuilder::new()` but only use `.entrypoint()`, `.args()`, `.trace_id()`. Capabilities like keystore, lockbox, storage, driver_factory, and dataview_executor are never wired. Every new capability requires auditing all 18 sites.

**Fix:** Create a single `enrich_task_context()` function that takes a builder and wires all available capabilities from AppContext. Call it from every dispatch site.

**Create:** `crates/riversd/src/task_enrichment.rs`
**Modify:** All dispatch site files, `crates/riversd/src/lib.rs`

### Design

```rust
use rivers_runtime::process_pool::TaskContextBuilder;

/// Enrich a TaskContextBuilder with all capabilities available from AppContext.
///
/// Called from every dispatch site instead of manually wiring each capability.
/// New capabilities added to AppContext are automatically available to all handlers.
pub fn enrich_task_context(
    mut builder: TaskContextBuilder,
    ctx: &AppContext,
    app_id: &str,
) -> TaskContextBuilder {
    // App identity
    builder = builder.app_id(app_id.into());

    // Storage engine (for ctx.store)
    if let Some(ref engine) = ctx.storage_engine {
        builder = builder.storage(engine.clone());
    }

    // Driver factory (for ctx.datasource().build())
    if let Some(ref factory) = ctx.driver_factory {
        builder = builder.driver_factory(factory.clone());
    }

    // DataView executor (for ctx.dataview() dynamic)
    if let Some(ref executor) = *ctx.dataview_executor.blocking_read() {
        builder = builder.dataview_executor(Arc::new(executor.clone()));
    }

    // Keystore (for Rivers.keystore + Rivers.crypto.encrypt/decrypt)
    // Uses shared resolver with entry_point lookup
    if let Some(ref resolver) = ctx.keystore_resolver {
        if let Some(ks) = resolver.get_for_entry_point(app_id) {
            builder = builder.keystore(ks.clone());
        }
    }

    // Lockbox (for Rivers.crypto.hmac key resolution)
    // ... wire if lockbox fields available

    builder
}
```

### Steps

- [x] **T1.1** Create `crates/riversd/src/task_enrichment.rs` with `enrich()` function
- [x] **T1.2** Add `pub mod task_enrichment;` to `crates/riversd/src/lib.rs`
- [x] **T1.3** Update `view_engine.rs` — all 6 builder sites enriched (pre_process, codecomponent, handlers, post_process, on_error, on_session_valid)
- [x] **T1.4** Update `guard.rs` — 3 sites enriched (execute_guard_handler, on_failed, lifecycle_hook)
- [x] **T1.5** Update `graphql.rs` — 1 site enriched (mutation handler)
- [x] **T1.6** Update `websocket.rs` — 2 sites enriched (on_stream, lifecycle)
- [x] **T1.7** Update `sse.rs` — 1 site enriched
- [x] **T1.8** Update `streaming.rs` — 1 site enriched
- [x] **T1.9** Update `polling.rs` — 1 site enriched (change_detect)
- [x] **T1.10** Update `message_consumer.rs` — 2 sites enriched (dispatch + EventHandler)
- [x] **T1.11** Update `deployment.rs` — 1 site enriched (init handler, already had app_id)
- [x] **T1.11b** Update `bundle_loader.rs` — 1 site enriched (datasource event handler, discovered during audit)
- [ ] **T1.12** SKIPPED — Keep shared `OnceLock<KeystoreResolver>` fallback in `process_pool/mod.rs` for defense in depth (v8_engine.rs still uses it as a secondary path)
- [x] **T1.13** Verified: `cargo build -p riversd` succeeds, `cargo test -p riversd --lib -- engine_tests` passes all 119 tests

**Validation:**
```bash
cargo build -p riversd
cargo test -p riversd --lib -- engine_tests
# All existing tests still pass
# Keystore V8 tests still pass (now via direct wiring, not shared resolver fallback)
```

---

## 2. Add Tests to Storage Backends (Pattern 4 fix)

**Problem:** `crates/rivers-storage-backends/` has 0 tests. This crate provides `RedisStorageEngine` and `SqliteStorageEngine` — the backends for sessions, cache, polling state, and `ctx.store`. Zero test coverage on security-critical persistence code.

**Modify:** `crates/rivers-storage-backends/src/redis_backend.rs`, `crates/rivers-storage-backends/src/sqlite_backend.rs`

- [x] **T2.1** Add unit tests to `sqlite_backend.rs`:
  - `get`/`set`/`del` round-trip
  - TTL expiration (set with TTL, verify expiry)
  - `list_keys` with prefix filtering
  - Non-existent key returns None
  - Overwrite existing key
  - Binary value storage (Bytes)

- [x] **T2.2** Add unit tests to `redis_backend.rs` (using mock or conditional on Redis availability):
  - Same test scenarios as SQLite
  - Gate live tests behind `#[cfg(feature = "redis-live-test")]` or `#[ignore]`
  - At minimum: test the `StorageEngine` trait interface consistency

- [x] **T2.3** Verify: `cargo test -p rivers-storage-backends`

**Validation:**
```bash
cargo test -p rivers-storage-backends
# SQLite tests pass (no external deps)
# Redis tests pass when Redis is available (or skipped)
```

---

## 3. Add Tests to LockBox Engine (Pattern 4 fix)

**Problem:** `crates/rivers-lockbox-engine/` has 0 tests in the crate itself. Tests exist in `crates/rivers-core/tests/lockbox_tests.rs` but the engine crate has no standalone coverage.

**Modify:** `crates/rivers-lockbox-engine/src/lib.rs`

- [x] **T3.1** Add unit tests to `rivers-lockbox-engine`:
  - Create/load keystore round-trip (same pattern as keystore-engine tests)
  - Entry add/resolve/fetch round-trip
  - Alias resolution
  - Duplicate entry detection
  - Invalid entry name rejection
  - Wrong key decryption failure
  - LockBoxResolver metadata-only model (no values in memory)
  - File permission enforcement (Unix)

- [x] **T3.2** Verify: `cargo test -p rivers-lockbox-engine` — 28 tests pass

**Validation:**
```bash
cargo test -p rivers-lockbox-engine
# All tests pass
```

---

## 4. Consolidate Error Response Envelope (Pattern 2 fix)

**Problem:** 7+ ad-hoc error construction sites bypass the `ErrorResponse` struct. Some use `{"error": msg}`, others use `{code, message}`, others use `{status: "error"}`. The `ErrorResponse` struct exists at `error_response.rs:14` but isn't used consistently.

**Modify:** `crates/riversd/src/error_response.rs` and affected files

- [x] **T4.1** Audit all error response construction in riversd — list every site that builds a JSON error response
- [x] **T4.2** Add named constructors to `ErrorResponse` + `IntoResponse` impl
- [x] **T4.3** Replace `ErrorResponse::new(code, msg)` calls with convenience constructors in `middleware.rs`, `server.rs`
- [x] **T4.4** Add comments to engine_loader.rs, sse.rs, streaming.rs explaining why ad-hoc formats are intentional
- [x] **T4.5** Verify: `cargo build -p riversd` succeeds with no new warnings

**Validation:**
```bash
cargo build -p riversd
# Grep for ad-hoc error construction — should be zero remaining
```

---

## 5. Clean Up Dead Code (Pattern 5 fix)

**Problem:** `SCRIPT_CACHE` and `clear_script_cache()` in `v8_engine.rs` are only used in tests, generating persistent compiler warnings. `TaskTerminator::Callback` and `active_tasks` are actually used (Wave 10 watchdog) but generate warnings because they're behind cfg gates.

**Modify:** `crates/riversd/src/process_pool/v8_engine.rs`, `crates/riversd/src/process_pool/mod.rs`

- [x] **T5.1** Move `SCRIPT_CACHE` and `clear_script_cache()` into `#[cfg(test)]` block since they're only used in tests
- [x] **T5.2** Add `#[allow(dead_code)]` to `TaskTerminator::Callback` with a comment explaining it's used by dynamic engine plugins
- [x] **T5.3** Add `#[allow(dead_code)]` to `ProcessPool.active_tasks` with a comment explaining it's used by the watchdog thread
- [x] **T5.4** Verify: `cargo build -p riversd 2>&1 | grep warning` shows no dead_code warnings from our code

**Validation:**
```bash
cargo build -p riversd 2>&1 | grep "dead_code" | grep -v "plugin"
# Zero warnings from riversd process_pool modules
```

---

## 6. Document AppContext Decomposition Plan (Pattern 3 prep)

**Problem:** AppContext has 26 fields across 13 logical groups. Full decomposition is a large refactor that touches every file. For now, document the planned extraction so it can be done incrementally.

**Note:** This is documentation only — no code changes. The actual extraction should happen after Waves 0-5 complete (when all fields are finalized).

- [x] **T6.1** Add a `// TODO(wave-6): Extract into sub-structs` comment block at the top of AppContext with the planned grouping:
  ```rust
  // Planned decomposition (after Wave 5):
  //   AppContext.security  → lockbox_resolver, keystore_resolver, csrf_manager, admin_auth_config, session_manager
  //   AppContext.storage   → storage_engine, event_bus
  //   AppContext.routing   → view_router, dataview_executor, graphql_schema
  //   AppContext.engines   → pool, driver_factory
  //   AppContext.streaming → sse_manager, ws_manager
  //   AppContext.lifecycle → shutdown, uptime, deployment_manager, hot_reload_state, config_path, loaded_bundle, guard_view_id, shutdown_tx
  //   AppContext.config    → config, log_controller
  ```

**Validation:** Comment only — no build impact.

---

## 7. Tutorial: Application Keystore

**Problem:** No tutorial exists for the Application Keystore feature (v0.52.2). Developers need a step-by-step guide covering provisioning, encryption, decryption, key rotation, and AAD.

**Create:** `docs/guide/tutorials/tutorial-app-keystore.md`

- [x] **T7.1** Write tutorial covering:
  - Provision master key in LockBox
  - Create keystore with `rivers-keystore` CLI (init, generate)
  - Declare `[[keystores]]` in `resources.toml`
  - Configure `[data.keystore.*]` in `app.toml`
  - Handler: encrypt data with `Rivers.crypto.encrypt()`
  - Handler: decrypt data with `Rivers.crypto.decrypt()`
  - Key rotation with `rivers-keystore rotate` + lazy re-encryption pattern
  - AAD (Additional Authenticated Data) binding to record IDs
  - Key metadata with `Rivers.keystore.has()` and `.info()`
  - Complete example with full bundle structure
  - Security notes

- [x] **T7.2** Update `docs/guide/tutorials/tutorial-js-handlers.md`:
  - Add `Rivers.crypto.encrypt/decrypt` to crypto section
  - Add `Rivers.keystore.has/info` section

- [x] **T7.3** Update `docs/guide/tutorials/tutorial-ts-handlers.md`:
  - Add `encrypt/decrypt` type signatures to `Rivers.crypto` interface
  - Add `keystore.has/info` type signatures to `Rivers` interface

**Validation:**
```bash
# Review tutorial follows existing format (heading structure, TOML/JS blocks, step numbering)
# All code examples reference correct API signatures from v0.52.5
```

---

## 8. Tutorial: ExecDriver Datasource

**Problem:** No tutorial exists for the ExecDriver plugin (v0.52.5). Operators and developers need a guide covering script setup, integrity hashing, datasource configuration, input modes, and security hardening.

**Create:** `docs/guide/tutorials/datasource-exec.md`

- [x] **T8.1** Write tutorial covering:
  - Set up execution environment (restricted OS user, directories)
  - Create a script following the stdin JSON I/O contract
  - Compute SHA-256 hash with `riversctl exec hash`
  - Declare `[[datasources]]` with `driver = "rivers-exec"` in `resources.toml`
  - Configure commands in `app.toml` (path, sha256, input_mode, timeout, concurrency)
  - JSON Schema validation for input parameters (optional)
  - Handler: query the exec datasource with `ctx.datasource().fromQuery()`
  - View configuration with CodeComponent handler
  - Args mode example with `args_template` (DNS lookup use case)
  - Both mode example (args + stdin combined)
  - Verification and deployment (`riversctl exec verify`, `riversctl validate`)
  - Complete example with full bundle structure and two commands
  - Security checklist (run_as_user, file permissions, env_clear, integrity mode)

**Validation:**
```bash
# Review tutorial follows existing datasource tutorial format
# TOML config examples match actual rivers-plugin-exec config parsing
# Handler examples match the tested patterns from engine_tests.rs
```

---

## Acceptance Criteria

- [ ] AC1: All 18 production dispatch sites use `enrich_task_context()` — no manual capability wiring
- [ ] AC2: New capabilities added to AppContext automatically flow to all handlers (verified by adding a test capability)
- [ ] AC3: `rivers-storage-backends` has unit tests for SQLite backend (get/set/del/TTL/list_keys)
- [ ] AC4: `rivers-lockbox-engine` has unit tests for core operations
- [ ] AC5: All error responses in riversd use `ErrorResponse` struct — no ad-hoc JSON construction
- [ ] AC6: Zero `dead_code` warnings from riversd process_pool modules
- [ ] AC7: AppContext decomposition plan documented as comments for Wave 6
- [ ] AC8: All existing tests still pass (no regressions)
- [x] AC9: Application Keystore tutorial exists with encrypt/decrypt examples, key rotation, and AAD
- [x] AC10: ExecDriver tutorial exists with stdin/args mode examples, SHA-256 hashing, and security checklist
- [x] AC11: JS and TS handler tutorials updated with keystore and encrypt/decrypt APIs

- [ ] AC1: All 18 production dispatch sites use `enrich_task_context()` — no manual capability wiring
- [ ] AC2: New capabilities added to AppContext automatically flow to all handlers (verified by adding a test capability)
- [ ] AC3: `rivers-storage-backends` has unit tests for SQLite backend (get/set/del/TTL/list_keys)
- [ ] AC4: `rivers-lockbox-engine` has unit tests for core operations
- [ ] AC5: All error responses in riversd use `ErrorResponse` struct — no ad-hoc JSON construction
- [ ] AC6: Zero `dead_code` warnings from riversd process_pool modules
- [ ] AC7: AppContext decomposition plan documented as comments for Wave 6
- [ ] AC8: All existing tests still pass (no regressions)
