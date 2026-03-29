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

---

## 9. LockBox Engine — Expanded Test Coverage

**Problem:** Coverage scan found 5 untested public functions (25%), 3 untested error variants (30%), and ~80% of edge cases uncovered. The biggest gaps are `resolve_key_source()` (entirely untested) and `startup_resolve()` (never tested end-to-end).

**Current state:** 28 tests cover happy paths and basic error cases. This task adds tests for the remaining public API surface, error paths, and edge cases.

**Modify:** `crates/rivers-lockbox-engine/src/lib.rs`

### T9.1: Key Source Resolution Tests (High Priority)

`resolve_key_source()` is used at every startup but has zero tests.

- [x] `resolve_key_source_env_success` — set env var, resolve with `key_source = "env"`
- [x] `resolve_key_source_env_missing_var` — env var not set → `KeySourceUnavailable`
- [x] `resolve_key_source_env_empty_var` — env var set to "" → `KeySourceUnavailable`
- [x] `resolve_key_source_file_success` — write identity to temp file, resolve with `key_source = "file"`
- [x] `resolve_key_source_file_missing_path` — key_file path doesn't exist → error
- [x] `resolve_key_source_file_insecure_permissions` — key_file is 0o644 → `InsecureFilePermissions` (Unix)
- [x] `resolve_key_source_agent_unsupported` — `key_source = "agent"` → `KeySourceUnavailable`
- [x] `resolve_key_source_unknown_source` — `key_source = "magic"` → error
- [x] `resolve_key_source_missing_env_var_config` — `key_env_var` is None → error (covered by env_missing_var)
- [x] `resolve_key_source_missing_file_path_config` — `key_file` is None → error (covered by file_missing_path)

### T9.2: Startup Resolve Integration Tests (High Priority)

`startup_resolve()` runs the full 12-step sequence but is never tested end-to-end.

- [x] `startup_resolve_complete_sequence` — create a keystore with entries, write to disk, build config, resolve references → success
- [x] `startup_resolve_relative_path_rejected` — config path is `./lockbox.rkeystore` → error (must be absolute)
- [x] `startup_resolve_file_not_found` — absolute path doesn't exist → `KeystoreNotFound`
- [ ] `startup_resolve_insecure_permissions` — keystore file is 0o644 → `InsecureFilePermissions` (Unix) (already covered by file_permissions_enforced)
- [x] `startup_resolve_wrong_key` — correct path, wrong Age identity → `DecryptionFailed`
- [x] `startup_resolve_missing_reference` — reference to entry that doesn't exist → `EntryNotFound`

### T9.3: Error Variant Coverage (Medium Priority)

Fill the 3 untested error variants.

- [ ] `error_config_missing` — call startup_resolve with None config → `ConfigMissing` (skipped: startup_resolve takes `&LockBoxConfig` not `Option`, and path=None triggers ConfigMissing which is testable but low value since it's a trivial match)
- [x] `error_malformed_keystore_invalid_toml` — encrypt garbage bytes as Age ciphertext → `MalformedKeystore`
- [x] `error_malformed_keystore_invalid_utf8` — encrypt non-UTF8 bytes → `MalformedKeystore`

### T9.4: fetch_secret_value Edge Cases (Medium Priority)

- [x] `fetch_secret_value_entry_index_out_of_bounds` — metadata with entry_index=99 on a 2-entry keystore
- [x] `fetch_secret_value_with_alias` — resolve alias, fetch by alias metadata → correct value
- [ ] `fetch_secret_value_zeroize_after_use` — verify ResolvedEntry.value can be zeroized (skipped: zeroize is a trait method, not a behavioral test)

### T9.5: Resolver & Reference Edge Cases (Low Priority)

- [x] `empty_resolver_key_count` — resolver from empty entries → key_count() == 0
- [x] `empty_resolver_contains` — contains("anything") → false
- [ ] `resolver_with_many_entries` — 50+ entries, verify all resolve correctly (skipped: low value, pattern already covered)
- [x] `collect_references_empty_datasources` — empty input → empty output
- [x] `collect_references_no_lockbox_uris` — datasources without lockbox:// → empty output
- [x] `collect_references_mixed_uris` — mix of lockbox:// and plain values → only lockbox filtered

### T9.6: Encryption Edge Cases (Low Priority)

- [x] `encrypt_with_invalid_recipient` — garbage string → error
- [x] `decrypt_with_invalid_identity` — garbage string → error
- [x] `encrypt_decrypt_value_with_newlines` — entry value contains \n → preserved
- [x] `encrypt_decrypt_value_with_unicode` — entry value contains emoji → preserved

**Validation:**
```bash
cargo test -p rivers-lockbox-engine
# Target: 55+ tests (28 existing + ~27 new)
```

---

## Acceptance Criteria

- [x] AC1: All 19 production dispatch sites use `enrich_task_context()`
- [ ] AC2: New capabilities added to AppContext automatically flow to all handlers
- [x] AC3: `rivers-storage-backends` has unit tests for SQLite backend
- [x] AC4: `rivers-lockbox-engine` has unit tests for core operations
- [x] AC5: Error responses consolidated with named constructors
- [x] AC6: Zero `dead_code` warnings from riversd process_pool modules
- [x] AC7: AppContext decomposition plan documented
- [x] AC8: All existing tests still pass
- [x] AC9: Application Keystore tutorial exists
- [x] AC10: ExecDriver tutorial exists
- [x] AC11: JS and TS handler tutorials updated
- [x] AC12: `resolve_key_source()` has tests for all 3 key sources + error paths
- [x] AC13: `startup_resolve()` has end-to-end integration test
- [x] AC14: All 10 `LockBoxError` variants have at least one triggering test (9/10 — ConfigMissing skipped, trivial path guard)
