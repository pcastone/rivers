# Large File Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split 20 files exceeding 600 LOC into focused modules of 300-400 lines max, preserving all public APIs via re-exports.

**Architecture:** Pure structural refactor — move code into submodules, add `mod.rs` facades with `pub use` re-exports. No logic changes. Each file becomes a directory with focused submodules.

**Tech Stack:** Rust module system (`mod`, `pub use`, `#[cfg(test)]`), `cargo check`/`cargo test` for verification.

**Spec:** `docs/superpowers/specs/2026-03-29-large-file-decomposition-design.md`

---

## Common Pattern

Every task follows this pattern (unless noted otherwise):

1. **Create directory** — rename `foo.rs` to `foo/mod.rs` (or create `foo/` alongside)
2. **Extract modules** — move code sections into new files under `foo/`
3. **Add imports** — each new file gets its own `use` block (copy relevant imports from original)
4. **Wire mod.rs** — declare submodules, re-export all `pub` items so external callers don't break
5. **cargo check -p <crate>** — must compile
6. **cargo test -p <crate>** — must pass
7. **Commit**

**Critical rule:** Every `pub` item that was importable as `crate::foo::Item` before must still be importable as `crate::foo::Item` after. The `mod.rs` file handles this via `pub use submodule::Item`.

---

## Phase 1: Leaf Crates (no internal dependents)

### Task 1: Split `rivers-core-config/src/config.rs` (762 LOC -> 5 modules)

**Files:**
- Rename: `crates/rivers-core-config/src/config.rs` -> `crates/rivers-core-config/src/config/mod.rs`
- Create: `crates/rivers-core-config/src/config/server.rs`
- Create: `crates/rivers-core-config/src/config/tls.rs`
- Create: `crates/rivers-core-config/src/config/security.rs`
- Create: `crates/rivers-core-config/src/config/storage.rs`
- Create: `crates/rivers-core-config/src/config/runtime.rs`

- [ ] **Step 1: Create directory and mod.rs**

```bash
cd crates/rivers-core-config/src
mkdir config_dir
```

Move original `config.rs` content into submodules. The `mod.rs` will re-export everything.

- [ ] **Step 2: Extract `server.rs`**

Move lines 14-171 from original: `ServerConfig`, `BaseConfig`, `BackpressureConfig`, `Http2Config`, and all their `Default` impls and helper functions (`default_host`, `default_port`, `default_request_timeout`).

- [ ] **Step 3: Extract `tls.rs`**

Move lines 177-348: `TlsConfig`, `TlsX509Config`, `TlsEngineConfig`, `AdminApiConfig`, `AdminTlsConfig`, `RbacConfig`, `ClusterConfig`, `SessionStoreConfig`, and all their `Default` impls and helper functions (`default_redirect_port`, `default_cn`, `default_san`, `default_days`, `default_min_version`, `default_admin_host`, `default_cookie_name`).

- [ ] **Step 4: Extract `security.rs`**

Move lines 386-591: `SecurityConfig`, `CsrfConfig`, `SessionConfig`, `SessionCookieConfig`, and all their `Default` impls and helper functions (`default_token_body_key`, `default_session_ttl`, `default_idle_timeout`, `default_rate_limit`, `default_burst_size`, `default_rate_strategy`).

- [ ] **Step 5: Extract `storage.rs`**

Move lines 639-735: `StorageEngineConfig`, `CacheConfig`, `DatasourceCacheConfig`, `DataViewCacheOverride`, and all `Default` impls and helpers (`default_cache_ttl`, `default_invalidation_strategy`, `default_storage_backend`, `default_retention_ms`, `default_max_events`, `default_sweep_interval`).

- [ ] **Step 6: Extract `runtime.rs`**

Move lines 742-1032: `RuntimeConfig`, `ProcessPoolConfig`, `EnvironmentOverride`, `BaseOverride`, `BackpressureOverride`, `SecurityOverride`, `StorageEngineOverride`, `LoggingConfig`, `EnginesConfig`, `PluginsConfig`, `GraphqlServerConfig`, `StaticFilesConfig`, and all `Default` impls and helpers. Also `default_true()` and `server_config_schema()`.

Note: `EnvironmentOverride::apply_to()` references `ServerConfig` — import from `super::server`.
Note: `StaticFilesConfig` is used by `ServerConfig` — it may need to live in `server.rs` or be imported. Check compile.

- [ ] **Step 7: Write `mod.rs`**

```rust
mod server;
mod tls;
mod security;
mod storage;
mod runtime;

pub use server::*;
pub use tls::*;
pub use security::*;
pub use storage::*;
pub use runtime::*;

#[cfg(test)]
mod tests {
    // Move test module here or into respective submodules
}
```

- [ ] **Step 8: Verify**

```bash
cargo check -p rivers-core-config
cargo test -p rivers-core-config
```

- [ ] **Step 9: Commit**

```bash
git add crates/rivers-core-config/src/config/
git rm crates/rivers-core-config/src/config.rs  # if not already handled by rename
git commit -m "refactor(rivers-core-config): split config.rs into 5 modules"
```

---

### Task 2: Split `rivers-lockbox-engine/src/lib.rs` (1,343 LOC -> 7 modules + 4 test modules)

**Files:**
- Modify: `crates/rivers-lockbox-engine/src/lib.rs` (becomes thin facade)
- Create: `crates/rivers-lockbox-engine/src/types.rs`
- Create: `crates/rivers-lockbox-engine/src/validation.rs`
- Create: `crates/rivers-lockbox-engine/src/resolver.rs`
- Create: `crates/rivers-lockbox-engine/src/crypto.rs`
- Create: `crates/rivers-lockbox-engine/src/key_source.rs`
- Create: `crates/rivers-lockbox-engine/src/secret_access.rs`
- Create: `crates/rivers-lockbox-engine/src/startup.rs`
- Create: `crates/rivers-lockbox-engine/tests/crypto_tests.rs`
- Create: `crates/rivers-lockbox-engine/tests/resolver_tests.rs`
- Create: `crates/rivers-lockbox-engine/tests/key_source_tests.rs`
- Create: `crates/rivers-lockbox-engine/tests/startup_tests.rs`

- [ ] **Step 1: Extract `types.rs`**

Move lines 26-191: `LockBoxError` enum, `Keystore` struct + Zeroize/Drop impls, `default_version()`, `KeystoreEntry` struct + Zeroize/Drop impls, `EntryType` enum + impl.

Also move the `pub use rivers_core_config::LockBoxConfig` re-export.

- [ ] **Step 2: Extract `validation.rs`**

Move lines 200-245: `validate_entry_name()`, `parse_lockbox_uri()`, `is_lockbox_uri()`.

Imports needed: `use crate::types::LockBoxError;`

- [ ] **Step 3: Extract `resolver.rs`**

Move lines 255-417: `EntryMetadata` struct + impl, `ResolvedEntry` struct, `LockBoxResolver` struct + Debug impl + impl block, `fetch_secret_value()`.

Imports needed: `use crate::types::*;`, `use crate::crypto::decrypt_keystore;`

- [ ] **Step 4: Extract `crypto.rs`**

Move lines 424-505: `decrypt_keystore()`, `encrypt_keystore()`.

Imports needed: `use crate::types::*;`

- [ ] **Step 5: Extract `key_source.rs`**

Move lines 512-601: `resolve_key_source()`, `check_file_permissions()` (both Unix and non-Unix versions).

Imports needed: `use crate::types::LockBoxError;`

- [ ] **Step 6: Extract `secret_access.rs`**

Note: `fetch_secret_value()` is part of the resolver module (it's tightly coupled). Keep it in `resolver.rs` instead of separate file. Skip this file if the resolver module stays under 400 lines with it included.

- [ ] **Step 7: Extract `startup.rs`**

Move lines 603-709: `LockBoxReference` struct, `collect_lockbox_references()`, `resolve_all_references()`, `startup_resolve()`.

Imports needed: `use crate::types::*;`, `use crate::resolver::*;`, `use crate::key_source::*;`, `use crate::crypto::*;`

- [ ] **Step 8: Write `lib.rs` facade**

```rust
pub mod types;
pub mod validation;
pub mod resolver;
pub mod crypto;
pub mod key_source;
pub mod startup;

// Re-export everything for backwards compatibility
pub use types::*;
pub use validation::*;
pub use resolver::*;
pub use crypto::*;
pub use key_source::*;
pub use startup::*;
```

- [ ] **Step 9: Split tests into `tests/` directory**

Move the `#[cfg(test)] mod tests` block (lines 713-1920) into separate test files grouped by module:
- `tests/crypto_tests.rs` — encrypt/decrypt roundtrip tests
- `tests/resolver_tests.rs` — lookup, metadata, entry query tests
- `tests/key_source_tests.rs` — key source resolution, file permission tests
- `tests/startup_tests.rs` — startup_resolve, collect/resolve reference tests

Each test file uses `use rivers_lockbox_engine::*;` for imports.

- [ ] **Step 10: Verify**

```bash
cargo check -p rivers-lockbox-engine
cargo test -p rivers-lockbox-engine
```

- [ ] **Step 11: Commit**

```bash
git add crates/rivers-lockbox-engine/
git commit -m "refactor(rivers-lockbox-engine): split lib.rs into 7 modules + 4 test modules"
```

---

### Task 3: Split `rivers-keystore-engine/src/lib.rs` (975 LOC -> 4 modules + 3 test modules)

**Files:**
- Modify: `crates/rivers-keystore-engine/src/lib.rs` (becomes thin facade)
- Create: `crates/rivers-keystore-engine/src/types.rs`
- Create: `crates/rivers-keystore-engine/src/io.rs`
- Create: `crates/rivers-keystore-engine/src/key_management.rs`
- Create: `crates/rivers-keystore-engine/src/crypto.rs`
- Create: `crates/rivers-keystore-engine/tests/io_tests.rs`
- Create: `crates/rivers-keystore-engine/tests/key_management_tests.rs`
- Create: `crates/rivers-keystore-engine/tests/crypto_tests.rs`

- [ ] **Step 1: Extract `types.rs`**

Move lines 26-170: `AppKeystoreError` enum, `AppKeystore` struct, `AppKeystoreKey` struct, `KeyVersion` struct, `KeyInfo` struct, `EncryptResult` struct, `default_version()`, all Zeroize/Drop impls, constants (`AES_256_KEY_SIZE`, `AES_GCM_NONCE_SIZE`, `SUPPORTED_KEY_TYPE`).

- [ ] **Step 2: Extract `io.rs`**

Move lines 174-269: `impl AppKeystore` block containing `create()`, `load()`, `save()`.

Imports needed: `use crate::types::*;`

- [ ] **Step 3: Extract `key_management.rs`**

Move lines 273-518: `impl AppKeystore` block containing `generate_key()`, `get_key()`, `get_key_version()`, `has_key()`, `rotate_key()`, `delete_key()`, `key_info()`, `list_keys()`, `current_key_bytes()`, `versioned_key_bytes()`.

Imports needed: `use crate::types::*;`

- [ ] **Step 4: Extract `crypto.rs`**

Move lines 520-659: standalone `encrypt()`, `decrypt()` functions + `impl AppKeystore` convenience wrappers (`encrypt_value()`, `decrypt_value()`).

Imports needed: `use crate::types::*;`

- [ ] **Step 5: Write `lib.rs` facade**

```rust
pub mod types;
pub mod io;
pub mod key_management;
pub mod crypto;

pub use types::*;
pub use io::*;
pub use key_management::*;
pub use crypto::*;

/// Test helper — creates an in-memory keystore for tests
pub fn create_test_keystore() -> AppKeystore {
    // Move the existing create_test_keystore() body here
}
```

- [ ] **Step 6: Split tests into `tests/` directory**

Move `#[cfg(test)] mod tests` (lines 671-1336) into:
- `tests/io_tests.rs` (~150 lines) — create/load/save roundtrip
- `tests/key_management_tests.rs` (~280 lines) — generate, rotate, delete, metadata
- `tests/crypto_tests.rs` (~230 lines) — encrypt/decrypt, AAD, tampered data

- [ ] **Step 7: Verify**

```bash
cargo check -p rivers-keystore-engine
cargo test -p rivers-keystore-engine
```

- [ ] **Step 8: Commit**

```bash
git add crates/rivers-keystore-engine/
git commit -m "refactor(rivers-keystore-engine): split lib.rs into 4 modules + 3 test modules"
```

---

### Task 4: Split `rivers-driver-sdk/src/http_executor.rs` (1,075 LOC -> 5 modules + 2 test modules)

**Files:**
- Rename: `crates/rivers-driver-sdk/src/http_executor.rs` -> `crates/rivers-driver-sdk/src/http_executor/mod.rs`
- Create: `crates/rivers-driver-sdk/src/http_executor/circuit_breaker.rs`
- Create: `crates/rivers-driver-sdk/src/http_executor/oauth2.rs`
- Create: `crates/rivers-driver-sdk/src/http_executor/connection.rs`
- Create: `crates/rivers-driver-sdk/src/http_executor/sse_stream.rs`
- Create: `crates/rivers-driver-sdk/src/http_executor/driver.rs`

- [ ] **Step 1: Extract `circuit_breaker.rs`**

Move lines 26-120: `CircuitState` enum, `CircuitBreaker` struct + impl.

No external imports needed beyond `std`.

- [ ] **Step 2: Extract `oauth2.rs`**

Move lines 126-344: `CachedToken`, `OAuth2Credentials`, `TokenResponse`, `default_expires_in()`, `fetch_oauth2_token()`.

Also move the `resolve_auth()` method from `impl ReqwestHttpDriver` (lines ~201-291) if it can be extracted as a standalone function, or keep it in `driver.rs` and have it call into `oauth2::fetch_oauth2_token()`.

- [ ] **Step 3: Extract `connection.rs`**

Move lines 435-685: `ReqwestHttpConnection` struct + both impl blocks (internal methods and `HttpConnection` trait impl).

Imports needed: `use super::circuit_breaker::CircuitBreaker;`

- [ ] **Step 4: Extract `sse_stream.rs`**

Move lines 687-796: `SseStreamConnection` struct, `HttpStreamConnection` impl, `parse_sse_event()`.

- [ ] **Step 5: Extract `driver.rs`**

Move lines 163-433: `ReqwestHttpDriver` struct, `impl ReqwestHttpDriver`, `impl Default`, `impl HttpDriver`.

Imports needed: `use super::oauth2::*;`, `use super::connection::*;`, `use super::circuit_breaker::*;`

- [ ] **Step 6: Write `mod.rs`**

```rust
mod circuit_breaker;
mod oauth2;
mod connection;
mod sse_stream;
mod driver;

pub use driver::*;
pub use connection::*;
pub use sse_stream::*;
// circuit_breaker and oauth2 are internal — no pub use
```

- [ ] **Step 7: Split tests**

Move `#[cfg(test)] mod tests` (lines 798-1315) into the respective submodules as `#[cfg(test)]` blocks, or into separate test files if they're integration tests.

- [ ] **Step 8: Verify**

```bash
cargo check -p rivers-driver-sdk
cargo test -p rivers-driver-sdk
```

- [ ] **Step 9: Commit**

```bash
git add crates/rivers-driver-sdk/src/http_executor/
git rm crates/rivers-driver-sdk/src/http_executor.rs
git commit -m "refactor(rivers-driver-sdk): split http_executor.rs into 5 modules"
```

---

### Task 5: Split `rivers-engine-v8/src/lib.rs` (649 LOC -> 3 modules)

**Files:**
- Modify: `crates/rivers-engine-v8/src/lib.rs` (keeps C-ABI exports)
- Create: `crates/rivers-engine-v8/src/task_context.rs`
- Create: `crates/rivers-engine-v8/src/v8_runtime.rs`
- Create: `crates/rivers-engine-v8/src/execution.rs`

- [ ] **Step 1: Extract `task_context.rs`**

Move lines 27-75: all `thread_local!` declarations, `setup_task_locals()`, `clear_task_locals()`.

- [ ] **Step 2: Extract `v8_runtime.rs`**

Move lines 79-131: `V8_INIT`, `ensure_v8_initialized()`, `SCRIPT_CACHE`, `DEFAULT_HEAP_LIMIT`, `ISOLATE_POOL`, `acquire_isolate()`, `release_isolate()`, `v8_str()`, `v8_to_json_value()`.

- [ ] **Step 3: Extract `execution.rs`**

Move lines 204-727: `execute_js()`, `inject_ctx_object()`, `inject_store_methods()`, all store callbacks, `inject_dataview_method()`, `dataview_callback()`, `inject_rivers_global()`, `log_callback()`, all crypto callbacks, `write_output()`, `write_error()`.

Imports needed: `use super::task_context::*;`, `use super::v8_runtime::*;`

- [ ] **Step 4: Update `lib.rs`**

Keep only: imports, `HOST_CALLBACKS` static, C-ABI exports (`_rivers_engine_abi_version`, `_rivers_engine_init`, `_rivers_engine_init_with_callbacks`, `_rivers_engine_execute`, `_rivers_engine_shutdown`, `_rivers_engine_cancel`), module declarations.

```rust
mod task_context;
mod v8_runtime;
mod execution;

use task_context::*;
use v8_runtime::*;
use execution::*;

// C-ABI exports remain here...
```

- [ ] **Step 5: Move tests to `execution.rs` or keep in `lib.rs`**

Tests (lines 731-854) test the full execution pipeline — keep as `#[cfg(test)]` in `lib.rs` since they test the public C-ABI interface.

- [ ] **Step 6: Verify**

```bash
cargo check -p rivers-engine-v8
cargo test -p rivers-engine-v8
```

- [ ] **Step 7: Commit**

```bash
git add crates/rivers-engine-v8/src/
git commit -m "refactor(rivers-engine-v8): split lib.rs into 3 modules"
```

---

## Phase 2: Driver Plugins

### Task 6: Split `rivers-drivers-builtin/src/redis.rs` (1,191 LOC -> 5 modules)

**Files:**
- Rename: `crates/rivers-drivers-builtin/src/redis.rs` -> `crates/rivers-drivers-builtin/src/redis/mod.rs`
- Create: `crates/rivers-drivers-builtin/src/redis/driver.rs`
- Create: `crates/rivers-drivers-builtin/src/redis/single.rs`
- Create: `crates/rivers-drivers-builtin/src/redis/cluster.rs`
- Create: `crates/rivers-drivers-builtin/src/redis/validation.rs`
- Create: `crates/rivers-drivers-builtin/src/redis/params.rs`

- [ ] **Step 1: Extract `params.rs`** (extract first — used by single.rs and cluster.rs)

Move lines 1029-1156: `inject_params_from_statement()`, `get_str_param()`, `get_int_param()`, `get_keys_param()`, `single_value_row()`.

- [ ] **Step 2: Extract `single.rs`**

Move lines 111-494: `RedisConnection` struct + `Connection` impl.

Imports: `use super::params::*;`

- [ ] **Step 3: Extract `cluster.rs`**

Move lines 496-876: `RedisClusterConnection` struct + `Connection` impl.

Imports: `use super::params::*;`

- [ ] **Step 4: Extract `validation.rs`**

Move lines 878-1027: `impl Driver for RedisDriver` (`driver_type`, `name`, `check_schema_syntax`).

- [ ] **Step 5: Extract `driver.rs`**

Move lines 18-109: `RedisDriver` struct, `impl Default`, `impl DatabaseDriver` (including `connect()` which creates `RedisConnection` or `RedisClusterConnection`).

Imports: `use super::single::RedisConnection;`, `use super::cluster::RedisClusterConnection;`

- [ ] **Step 6: Write `mod.rs`**

```rust
mod params;
mod single;
mod cluster;
mod driver;
mod validation;

pub use driver::*;
pub use single::*;
pub use cluster::*;
pub use validation::*;
// params is internal
```

- [ ] **Step 7: Check how redis.rs is referenced**

Search for `mod redis` or `use ...redis` in the crate to ensure the module path still resolves. If the crate uses `mod redis;` in `lib.rs`, it will now find `redis/mod.rs` automatically.

- [ ] **Step 8: Verify**

```bash
cargo check -p rivers-drivers-builtin
cargo test -p rivers-drivers-builtin
```

- [ ] **Step 9: Commit**

```bash
git add crates/rivers-drivers-builtin/src/redis/
git rm crates/rivers-drivers-builtin/src/redis.rs
git commit -m "refactor(rivers-drivers-builtin): split redis.rs into 5 modules"
```

---

### Task 7: Split `rivers-plugin-exec/src/config.rs` (733 LOC -> 3 modules)

**Files:**
- Rename: `crates/rivers-plugin-exec/src/config.rs` -> `crates/rivers-plugin-exec/src/config/mod.rs`
- Create: `crates/rivers-plugin-exec/src/config/types.rs`
- Create: `crates/rivers-plugin-exec/src/config/parser.rs`
- Create: `crates/rivers-plugin-exec/src/config/validator.rs`

- [ ] **Step 1: Extract `types.rs`**

Move lines 12-113: `ExecConfig` struct, `CommandConfig` struct, `IntegrityMode` enum + `parse()` impl, `InputMode` enum + `parse()` impl.

- [ ] **Step 2: Extract `parser.rs`**

Move lines 115-489: `ExecConfig::parse()`, `parse_u64_opt()`, `parse_usize_opt()`, `parse_commands()`, `parse_indexed_list()`, `parse_env_set()`.

Imports: `use super::types::*;`

- [ ] **Step 3: Extract `validator.rs`**

Move lines 159-302: `ExecConfig::validate()`.

Imports: `use super::types::*;`

Note: `parse()` and `validate()` are both `impl ExecConfig` methods. Rust allows splitting impl blocks across files as long as they're in the same crate. Each file gets its own `impl ExecConfig { ... }` block.

- [ ] **Step 4: Write `mod.rs` and distribute tests**

```rust
mod types;
mod parser;
mod validator;

pub use types::*;
pub use parser::*;
pub use validator::*;
```

Move test groups to their respective modules as `#[cfg(test)]` blocks.

- [ ] **Step 5: Verify**

```bash
cargo check -p rivers-plugin-exec
cargo test -p rivers-plugin-exec
```

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-plugin-exec/src/config/
git rm crates/rivers-plugin-exec/src/config.rs
git commit -m "refactor(rivers-plugin-exec): split config.rs into 3 modules"
```

---

### Task 8: Split `rivers-plugin-exec/src/connection.rs` (674 LOC -> 3 modules)

**Files:**
- Rename: `crates/rivers-plugin-exec/src/connection.rs` -> `crates/rivers-plugin-exec/src/connection/mod.rs`
- Create: `crates/rivers-plugin-exec/src/connection/driver.rs`
- Create: `crates/rivers-plugin-exec/src/connection/pipeline.rs`
- Create: `crates/rivers-plugin-exec/src/connection/exec_connection.rs`

- [ ] **Step 1: Extract `driver.rs`**

Move lines 28-113: `CommandRuntime` struct, `ExecDriver` struct + `DatabaseDriver` impl.

- [ ] **Step 2: Extract `pipeline.rs`**

Move lines 146-317: `impl ExecConnection` containing `execute_command()` — the 11-step pipeline.

Imports: `use super::exec_connection::ExecConnection;`, `use super::driver::CommandRuntime;`

- [ ] **Step 3: Extract `exec_connection.rs`**

Move lines 115-144: `ExecConnection` struct, `Connection` trait impl (`execute`, `ping`, `driver_name`).

- [ ] **Step 4: Write `mod.rs` and distribute tests**

Tests (lines 319+) distribute into respective modules.

- [ ] **Step 5: Verify**

```bash
cargo check -p rivers-plugin-exec
cargo test -p rivers-plugin-exec
```

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-plugin-exec/src/connection/
git rm crates/rivers-plugin-exec/src/connection.rs
git commit -m "refactor(rivers-plugin-exec): split connection.rs into 3 modules"
```

---

### Task 9: Split `rivers-plugin-influxdb/src/lib.rs` (704 LOC -> 4 modules)

**Files:**
- Modify: `crates/rivers-plugin-influxdb/src/lib.rs` (keeps ABI exports)
- Create: `crates/rivers-plugin-influxdb/src/driver.rs`
- Create: `crates/rivers-plugin-influxdb/src/connection.rs`
- Create: `crates/rivers-plugin-influxdb/src/batching.rs`
- Create: `crates/rivers-plugin-influxdb/src/protocol.rs`

- [ ] **Step 1: Extract `protocol.rs`** (extract first — used by connection and batching)

Move lines 383-585: `urlencoded()`, `parse_csv_response()`, `build_line_protocol()`, `escape_line_protocol_key()`, `escape_line_protocol_tag_value()`, `format_field_value()`, `format_query_value_as_field()`.

- [ ] **Step 2: Extract `connection.rs`**

Move lines 103-257: `InfluxConnection` struct + `Connection` impl.

Imports: `use crate::protocol::*;`

- [ ] **Step 3: Extract `batching.rs`**

Move lines 259-364: `BatchingInfluxConnection` struct, impl methods, `Connection` impl, `Drop` impl.

Imports: `use crate::connection::InfluxConnection;`, `use crate::protocol::*;`

- [ ] **Step 4: Extract `driver.rs`**

Move lines 25-101: `InfluxDriver` struct + `DatabaseDriver` impl.

Imports: `use crate::connection::InfluxConnection;`, `use crate::batching::BatchingInfluxConnection;`

- [ ] **Step 5: Update `lib.rs`**

Keep only: module declarations, ABI exports (`_rivers_abi_version`, `_rivers_register_driver`).

```rust
mod protocol;
mod connection;
mod batching;
mod driver;

pub use driver::*;
pub use connection::*;
pub use batching::*;
pub use protocol::*;

// ABI exports...
```

Distribute tests to respective modules.

- [ ] **Step 6: Verify**

```bash
cargo check -p rivers-plugin-influxdb
cargo test -p rivers-plugin-influxdb
```

- [ ] **Step 7: Commit**

```bash
git add crates/rivers-plugin-influxdb/src/
git commit -m "refactor(rivers-plugin-influxdb): split lib.rs into 4 modules"
```

---

## Phase 3: Runtime Tests

### Task 10: Split `rivers-runtime/tests/config_tests.rs` (660 LOC -> 4 test modules)

**Files:**
- Remove: `crates/rivers-runtime/tests/config_tests.rs`
- Create: `crates/rivers-runtime/tests/server_config_tests.rs`
- Create: `crates/rivers-runtime/tests/app_config_tests.rs`
- Create: `crates/rivers-runtime/tests/bundle_tests.rs`
- Create: `crates/rivers-runtime/tests/schema_tests.rs`

- [ ] **Step 1: Read the test file and identify test groupings**

Map each `#[test]` function to its target module based on what it tests (ServerConfig parsing, app config validation, bundle manifest, JSON schema).

- [ ] **Step 2: Create `server_config_tests.rs`**

Move ServerConfig parsing tests, validation tests, and environment override tests (~180 lines).

- [ ] **Step 3: Create `app_config_tests.rs`**

Move app config parsing and validation tests (~100 lines).

- [ ] **Step 4: Create `bundle_tests.rs`**

Move bundle manifest, resources config, and cache config tests (~100 lines).

- [ ] **Step 5: Create `schema_tests.rs`**

Move JSON schema generation tests (~80 lines).

- [ ] **Step 6: Verify**

```bash
cargo test -p rivers-runtime
```

- [ ] **Step 7: Commit**

```bash
git add crates/rivers-runtime/tests/
git rm crates/rivers-runtime/tests/config_tests.rs
git commit -m "refactor(rivers-runtime): split config_tests.rs into 4 test modules"
```

---

## Phase 4: `riversd` Crate

### Task 11: Split `riversd/src/graphql.rs` (644 LOC -> 3 modules)

**Files:**
- Rename: `crates/riversd/src/graphql.rs` -> `crates/riversd/src/graphql/mod.rs`
- Create: `crates/riversd/src/graphql/config.rs`
- Create: `crates/riversd/src/graphql/types.rs`
- Create: `crates/riversd/src/graphql/schema_builder.rs`

- [ ] **Step 1: Extract `config.rs`**

Move lines 14-80: `GraphqlConfig` struct, defaults, `From` conversion.

- [ ] **Step 2: Extract `types.rs`**

Move lines 82-244: `ResolverMapping`, `GraphqlType`, `GraphqlField`, `GraphqlFieldType` enum + impls, `generate_graphql_types()`, `to_pascal_case()`.

- [ ] **Step 3: Extract `schema_builder.rs`**

Move lines 246-868: `json_to_gql_value()`, `build_dynamic_schema()`, `gql_value_to_json()`, `graphql_router()`, `MutationMapping`, `build_mutation_mappings_from_views()`, `build_resolver_mappings_from_dataviews()`, `build_schema_with_executor()`, `build_mutation_type_with_pool()`, `SubscriptionMapping`, `build_subscription_mappings_from_views()`, `build_subscription_type()`, `validate_graphql_config()`, `GraphqlError` enum.

Note: This module is ~620 lines — still large. Consider further splitting into `schema_builder.rs` (core schema + queries, ~350 lines) and `mutations.rs` (mutation + subscription mapping, ~270 lines) if needed during implementation. The implementer should use their judgment here.

- [ ] **Step 4: Write `mod.rs`**

```rust
mod config;
mod types;
mod schema_builder;

pub use config::*;
pub use types::*;
pub use schema_builder::*;
```

- [ ] **Step 5: Verify**

```bash
cargo check -p riversd
cargo test -p riversd
```

- [ ] **Step 6: Commit**

```bash
git add crates/riversd/src/graphql/
git rm crates/riversd/src/graphql.rs
git commit -m "refactor(riversd): split graphql.rs into 3 modules"
```

---

### Task 12: Split `riversd/src/engine_loader.rs` (669 LOC -> 4 modules)

**Files:**
- Rename: `crates/riversd/src/engine_loader.rs` -> `crates/riversd/src/engine_loader/mod.rs`
- Create: `crates/riversd/src/engine_loader/loaded_engine.rs`
- Create: `crates/riversd/src/engine_loader/registry.rs`
- Create: `crates/riversd/src/engine_loader/loader.rs`
- Create: `crates/riversd/src/engine_loader/host_context.rs`

- [ ] **Step 1: Extract `loaded_engine.rs`**

Move lines 13-88: `LoadedEngine` struct + impl (`execute`, `cancel`).

- [ ] **Step 2: Extract `registry.rs`**

Move lines 90-136: `registry()`, `get_engine()`, `is_engine_available()`, `execute_on_engine()`, `loaded_engines()`.

Imports: `use super::loaded_engine::LoadedEngine;`

- [ ] **Step 3: Extract `loader.rs`**

Move lines 138-266: `EngineLoadResult` enum, `load_engines()`, `load_single_engine()`.

Imports: `use super::loaded_engine::LoadedEngine;`, `use super::registry::*;`

- [ ] **Step 4: Extract `host_context.rs`**

Move lines 268-859: `HostContext` struct, `set_host_context()`, `set_host_keystore()`, `build_host_callbacks()`, and all `extern "C"` callback functions (`write_output`, `read_input`, `dataview_execute`, `store_get`, `store_set`, `store_del`, `datasource_build`, `http_request`, `log_message`, `free_buffer`, `keystore_has`, `keystore_info`, `crypto_encrypt`, `crypto_decrypt`).

Note: This module will be ~590 lines — still large due to the many FFI callbacks. Consider splitting further into `host_context.rs` (context + setup, ~100 lines) and `host_callbacks.rs` (all extern "C" fns, ~490 lines). The implementer should assess during extraction.

- [ ] **Step 5: Write `mod.rs`**

```rust
mod loaded_engine;
mod registry;
mod loader;
mod host_context;

pub use loaded_engine::*;
pub use registry::*;
pub use loader::*;
pub use host_context::*;
```

- [ ] **Step 6: Verify**

```bash
cargo check -p riversd
cargo test -p riversd
```

- [ ] **Step 7: Commit**

```bash
git add crates/riversd/src/engine_loader/
git rm crates/riversd/src/engine_loader.rs
git commit -m "refactor(riversd): split engine_loader.rs into 4 modules"
```

---

### Task 13: Split `riversd/src/view_engine.rs` (1,049 LOC -> 5 modules)

**Files:**
- Rename: `crates/riversd/src/view_engine.rs` -> `crates/riversd/src/view_engine/mod.rs`
- Create: `crates/riversd/src/view_engine/types.rs`
- Create: `crates/riversd/src/view_engine/router.rs`
- Create: `crates/riversd/src/view_engine/pipeline.rs`
- Create: `crates/riversd/src/view_engine/validation.rs`

- [ ] **Step 1: Extract `types.rs`**

Move lines 11-108 + 429-442 + 1013-1036: `ParsedRequest` struct + impl, `StoreHandle` struct + impl, `ViewContext` struct + impl, `ViewResult` struct + impl, `ViewError` enum.

- [ ] **Step 2: Extract `router.rs`**

Move lines 110-362: `ViewRoute` struct, `PathSegment` enum, `build_namespaced_path()`, `ViewRouter` struct + impl (`from_bundle`, `from_views`, `match_route`, `routes`), `parse_path_pattern()`.

- [ ] **Step 3: Extract `pipeline.rs`**

Move lines 364-685: `apply_parameter_mapping()`, `json_value_to_query_value()`, `execute_rest_view()`, `serialize_view_result()`.

Imports: `use super::types::*;`, `use super::router::*;`

- [ ] **Step 4: Extract `validation.rs`**

Move lines 686-1012: `validate_views()`, `execute_on_error_handlers()`, `execute_on_session_valid()`, `parse_handler_view_result()`, `validate_input()`, `validate_output()`.

Imports: `use super::types::*;`

- [ ] **Step 5: Write `mod.rs` with tests**

```rust
mod types;
mod router;
mod pipeline;
mod validation;

pub use types::*;
pub use router::*;
pub use pipeline::*;
pub use validation::*;

#[cfg(test)]
mod tests {
    // Move lines 1037-1354 here, or into a tests.rs submodule
}
```

- [ ] **Step 6: Verify**

```bash
cargo check -p riversd
cargo test -p riversd
```

- [ ] **Step 7: Commit**

```bash
git add crates/riversd/src/view_engine/
git rm crates/riversd/src/view_engine.rs
git commit -m "refactor(riversd): split view_engine.rs into 5 modules"
```

---

### Task 14: Split `riversd/src/polling.rs` (953 LOC -> 5 modules)

**Files:**
- Rename: `crates/riversd/src/polling.rs` -> `crates/riversd/src/polling/mod.rs`
- Create: `crates/riversd/src/polling/diff.rs`
- Create: `crates/riversd/src/polling/state.rs`
- Create: `crates/riversd/src/polling/executor.rs`
- Create: `crates/riversd/src/polling/runner.rs`

- [ ] **Step 1: Extract `diff.rs`**

Move lines 18-241: `DiffStrategy` enum + impl, `PollLoopKey` struct + impl, `compute_param_hash()`, `compute_data_hash()`, `hash_diff()`, `null_diff()`, `DiffResult` struct, `compute_diff()`.

- [ ] **Step 2: Extract `state.rs`**

Move lines 243-434: `PollLoopState` struct + impl, `PollLoopRegistry` struct + impl + Default, `PollError` enum, `DataViewPollExecutor` trait, `DataViewPollExecutor` struct + impl.

Imports: `use super::diff::*;`

- [ ] **Step 3: Extract `executor.rs`**

Move lines 463-614: storage persistence functions (`save_poll_state`, `load_poll_state`, `delete_poll_state`), `PollTickResult` struct, `execute_poll_tick()`, `run_poll_tick_and_broadcast()`.

Imports: `use super::diff::*;`, `use super::state::*;`

- [ ] **Step 4: Extract `runner.rs`**

Move lines 616-809: `check_change_detect_timeout()`, `execute_poll_tick_inmemory()`, `dispatch_change_detect()`, `run_poll_loop_inmemory()`.

Imports: `use super::diff::*;`, `use super::state::*;`, `use super::executor::*;`

- [ ] **Step 5: Write `mod.rs` with tests**

```rust
mod diff;
mod state;
mod executor;
mod runner;

pub use diff::*;
pub use state::*;
pub use executor::*;
pub use runner::*;

#[cfg(test)]
mod tests {
    // Move lines 811-1297 here
}
```

- [ ] **Step 6: Verify**

```bash
cargo check -p riversd
cargo test -p riversd
```

- [ ] **Step 7: Commit**

```bash
git add crates/riversd/src/polling/
git rm crates/riversd/src/polling.rs
git commit -m "refactor(riversd): split polling.rs into 5 modules"
```

---

### Task 15: Split `riversd/src/bundle_loader.rs` (841 LOC -> 4 modules)

**Files:**
- Rename: `crates/riversd/src/bundle_loader.rs` -> `crates/riversd/src/bundle_loader/mod.rs`
- Create: `crates/riversd/src/bundle_loader/types.rs`
- Create: `crates/riversd/src/bundle_loader/load.rs`
- Create: `crates/riversd/src/bundle_loader/wire.rs`
- Create: `crates/riversd/src/bundle_loader/reload.rs`

- [ ] **Step 1: Extract `types.rs`**

Move lines 19-44 + 780-788 + 1003-1050: `SseTriggerHandler` struct + EventHandler impl, `ReloadSummary` struct, `DatasourceEventBusHandler` struct + EventHandler impl.

- [ ] **Step 2: Split `load_and_wire_bundle()` into `load.rs` and `wire.rs`**

This is the hardest split — one 730-line function needs to be split in two. Look for the natural boundary around line ~415 (after GraphQL/guard setup, before broker/SSE/WS wiring).

**Strategy:** Extract the second half of `load_and_wire_bundle()` into a separate async function (e.g., `wire_streaming_and_events()`) that receives the intermediate state as parameters. Then:

- `load.rs` contains the first half: bundle parsing, LockBox resolution, datasource setup, driver registration, cache, DataView executor, GraphQL schema, guard views.
- `wire.rs` contains the second half: broker bridges, message consumers, SSE/WS managers, event handlers.

The split point requires the implementer to identify which local variables from the first half are needed by the second half and pass them as function parameters.

- [ ] **Step 3: Extract `reload.rs`**

Move lines 797-1012: `rebuild_views_and_dataviews()`, `build_cache_policy_from_bundle()`.

- [ ] **Step 4: Write `mod.rs`**

```rust
mod types;
mod load;
mod wire;
mod reload;

pub use types::*;
pub use load::*;
pub use wire::*;
pub use reload::*;
```

- [ ] **Step 5: Verify**

```bash
cargo check -p riversd
cargo test -p riversd
```

- [ ] **Step 6: Commit**

```bash
git add crates/riversd/src/bundle_loader/
git rm crates/riversd/src/bundle_loader.rs
git commit -m "refactor(riversd): split bundle_loader.rs into 4 modules"
```

---

### Task 16: Split `riversd/src/server.rs` (1,846 LOC -> 9 modules)

**Files:**
- Rename: `crates/riversd/src/server.rs` -> `crates/riversd/src/server/mod.rs`
- Create: `crates/riversd/src/server/context.rs`
- Create: `crates/riversd/src/server/router.rs`
- Create: `crates/riversd/src/server/view_dispatch.rs`
- Create: `crates/riversd/src/server/streaming.rs`
- Create: `crates/riversd/src/server/handlers.rs`
- Create: `crates/riversd/src/server/admin_auth.rs`
- Create: `crates/riversd/src/server/drivers.rs`
- Create: `crates/riversd/src/server/lifecycle.rs`
- Create: `crates/riversd/src/server/validation.rs`

- [ ] **Step 1: Extract `context.rs`**

Move lines 42-204: `LogController` struct + impl, `AppContext` struct + impl.

- [ ] **Step 2: Extract `router.rs`**

Move lines 207-366: `build_main_router()`, `build_admin_router()`.

Imports: `use super::context::AppContext;`

- [ ] **Step 3: Extract `handlers.rs`**

Move lines 367-463 + 1339-1394: `health_handler()`, `health_verbose_handler()`, `gossip_receive_handler()`, `static_file_handler()`, `services_discovery_handler()`.

- [ ] **Step 4: Extract `view_dispatch.rs`**

Move lines 464-739: `MatchedRoute` struct, `combined_fallback_handler()` / `view_dispatch_handler()`, `parse_query_string()`.

- [ ] **Step 5: Extract `streaming.rs`**

Move lines 740-1338: `build_streaming_response()`, `execute_sse_view()`, `execute_streaming_rest_view()`, `execute_ws_view()`, `handle_ws_connection()`.

- [ ] **Step 6: Extract `admin_auth.rs`**

Move lines 1395-1571: `admin_auth_middleware()`, `path_to_admin_permission()`, `DEFAULT_ADMIN_AUTH_CONFIG`, `build_admin_auth_config_for_rbac()`.

- [ ] **Step 7: Extract `drivers.rs`**

Move lines 1572-1668: `register_all_drivers()`.

- [ ] **Step 8: Extract `lifecycle.rs`**

Move lines 1669-2268: `maybe_spawn_http_redirect_server()`, `run_http_redirect_server()`, `run_server_no_ssl()`, `run_server_with_listener_with_control()`, `run_server_with_listener_and_log()`.

- [ ] **Step 9: Extract `validation.rs`**

Move lines 2269-2409: `validate_admin_access_control()`, `validate_server_tls()`, `ServerError` enum, `shutdown_signal()`, `maybe_spawn_hot_reload_watcher()`, tests.

- [ ] **Step 10: Write `mod.rs`**

```rust
mod context;
mod router;
mod handlers;
mod view_dispatch;
mod streaming;
mod admin_auth;
mod drivers;
mod lifecycle;
mod validation;

pub use context::*;
pub use router::*;
pub use handlers::*;
pub use view_dispatch::*;
pub use streaming::*;
pub use admin_auth::*;
pub use drivers::*;
pub use lifecycle::*;
pub use validation::*;
```

- [ ] **Step 11: Verify**

```bash
cargo check -p riversd
cargo test -p riversd
```

- [ ] **Step 12: Commit**

```bash
git add crates/riversd/src/server/
git rm crates/riversd/src/server.rs
git commit -m "refactor(riversd): split server.rs into 9 modules"
```

---

### Task 17: Split `riversd/src/process_pool/v8_engine.rs` (1,529 LOC -> 7 modules)

**Files:**
- Rename: `crates/riversd/src/process_pool/v8_engine.rs` -> `crates/riversd/src/process_pool/v8/mod.rs`
- Create: `crates/riversd/src/process_pool/v8/task_locals.rs`
- Create: `crates/riversd/src/process_pool/v8/init.rs`
- Create: `crates/riversd/src/process_pool/v8/execution.rs`
- Create: `crates/riversd/src/process_pool/v8/context.rs`
- Create: `crates/riversd/src/process_pool/v8/datasource.rs`
- Create: `crates/riversd/src/process_pool/v8/rivers_global.rs`
- Create: `crates/riversd/src/process_pool/v8/http.rs`

Note: The parent module (`process_pool/mod.rs`) currently declares `mod v8_engine;`. This must change to `mod v8;` or `#[path = "v8/mod.rs"] mod v8_engine;` to preserve the module path. Check which approach causes fewer downstream changes.

- [ ] **Step 1: Extract `task_locals.rs`**

Move all thread-local declarations, `TaskLocals` guard struct + impl + Drop.

- [ ] **Step 2: Extract `init.rs`**

Move V8 initialization, isolate pool, script cache, heap limit callback.

- [ ] **Step 3: Extract `execution.rs`**

Move `execute_js_task()`, `call_entrypoint()`, ES module support (`execute_as_module`, `is_module_syntax`), `resolve_promise_if_needed()`, `resolve_module_source()`.

- [ ] **Step 4: Extract `context.rs`**

Move `inject_ctx_object()`, `inject_ctx_methods()`, `ctx_store_get/set/del` callbacks, `ctx_dataview_callback`.

- [ ] **Step 5: Extract `datasource.rs`**

Move `ctx_datasource_build_callback`, `json_to_query_value()`.

- [ ] **Step 6: Extract `rivers_global.rs`**

Move `inject_rivers_global()` and all its nested callbacks (log, crypto, keystore, env).

- [ ] **Step 7: Extract `http.rs`**

Move HTTP verb callbacks, `do_http_request()`, header/response helpers, `json_to_v8`/`v8_to_json`.

- [ ] **Step 8: Write `mod.rs`**

Re-export: `execute_js_task`, `ensure_v8_initialized`, `is_module_syntax`, and any other items that `process_pool/mod.rs` uses.

- [ ] **Step 9: Update parent module reference**

In `crates/riversd/src/process_pool/mod.rs`, change `mod v8_engine;` to either `mod v8;` with appropriate re-exports, or use `#[path = "v8/mod.rs"] mod v8_engine;`.

- [ ] **Step 10: Verify**

```bash
cargo check -p riversd
cargo test -p riversd
```

- [ ] **Step 11: Commit**

```bash
git add crates/riversd/src/process_pool/v8/
git rm crates/riversd/src/process_pool/v8_engine.rs
git commit -m "refactor(riversd): split v8_engine.rs into 7 modules"
```

---

### Task 18: Split `riversd/src/process_pool/engine_tests.rs` (2,394 LOC -> 8 test files)

**Files:**
- Remove: `crates/riversd/src/process_pool/engine_tests.rs`
- Create: `crates/riversd/src/process_pool/tests/mod.rs`
- Create: `crates/riversd/src/process_pool/tests/helpers.rs`
- Create: `crates/riversd/src/process_pool/tests/basic_execution.rs`
- Create: `crates/riversd/src/process_pool/tests/crypto.rs`
- Create: `crates/riversd/src/process_pool/tests/context_data.rs`
- Create: `crates/riversd/src/process_pool/tests/http_and_logging.rs`
- Create: `crates/riversd/src/process_pool/tests/wasm_and_workers.rs`
- Create: `crates/riversd/src/process_pool/tests/integration.rs`
- Create: `crates/riversd/src/process_pool/tests/exec_and_keystore.rs`

Note: This file is `#[cfg(test)] mod engine_tests;` in the parent. The new structure should be `#[cfg(test)] mod tests;` pointing to `tests/mod.rs`.

- [ ] **Step 1: Create `helpers.rs`**

Extract shared test helpers: `make_js_task()`, `make_http_js_task()`, `make_exec_script()`, `sha256_file()`, `make_exec_params()`, `make_test_keystore()`, `make_ks_task()`.

All helpers become `pub(super)` so sibling test modules can use them.

- [ ] **Step 2: Create `basic_execution.rs`**

Move tests for: simple return, args, errors, missing functions, duration, trace IDs, resdata write-back, ctx metadata, exception handling (~280 lines).

- [ ] **Step 3: Create `crypto.rs`**

Move tests for: `Rivers.crypto` — randomHex, bcrypt, HMAC, base64url, timing-safe (~300 lines).

- [ ] **Step 4: Create `context_data.rs`**

Move tests for: `ctx.dataview`, `ctx.store` (in-memory + StorageEngine), `ctx.datasource().build()`, DataViewExecutor (~340 lines).

- [ ] **Step 5: Create `http_and_logging.rs`**

Move tests for: `Rivers.http`, `Rivers.env`, `Rivers.log` (basic + structured fields), HTTP capability gating (~310 lines).

- [ ] **Step 6: Create `wasm_and_workers.rs`**

Move tests for: V8Worker/WasmtimeWorker config, WASM execution, isolate pool reuse, script cache, TypeScript compiler (~365 lines).

- [ ] **Step 7: Create `integration.rs`**

Move AU gap tests: JS/WASM comprehensive coverage, async/promises, complex data types, file loading (~335 lines).

- [ ] **Step 8: Create `exec_and_keystore.rs`**

Move tests for: ExecDriver subprocess execution, keystore encrypt/decrypt/AAD/tamper (~390 lines).

- [ ] **Step 9: Write `tests/mod.rs`**

```rust
mod helpers;
mod basic_execution;
mod crypto;
mod context_data;
mod http_and_logging;
mod wasm_and_workers;
mod integration;
mod exec_and_keystore;
```

- [ ] **Step 10: Update parent module**

In `crates/riversd/src/process_pool/mod.rs`, change `#[cfg(test)] mod engine_tests;` to `#[cfg(test)] mod tests;`.

- [ ] **Step 11: Verify**

```bash
cargo check -p riversd
cargo test -p riversd
```

- [ ] **Step 12: Commit**

```bash
git add crates/riversd/src/process_pool/tests/
git rm crates/riversd/src/process_pool/engine_tests.rs
git commit -m "refactor(riversd): split engine_tests.rs into 8 test modules"
```

---

### Task 19: Split `riversd/tests/graphql_tests.rs` (758 LOC -> 3 test modules)

**Files:**
- Remove: `crates/riversd/tests/graphql_tests.rs`
- Create: `crates/riversd/tests/graphql_config_tests.rs`
- Create: `crates/riversd/tests/graphql_schema_tests.rs`
- Create: `crates/riversd/tests/graphql_integration_tests.rs`

Note: Files in `tests/` are integration tests — each file is a separate test binary. No `mod.rs` needed. Shared helpers can go in a `tests/common/mod.rs` if needed.

- [ ] **Step 1: Read and map test functions to categories**

Identify which tests cover config, schema generation, and integration/execution.

- [ ] **Step 2: Create `graphql_config_tests.rs`** (~120 lines)

Move config parsing, field type, and validation tests.

- [ ] **Step 3: Create `graphql_schema_tests.rs`** (~280 lines)

Move schema generation, dynamic building, conversion, and resolver mapping tests.

- [ ] **Step 4: Create `graphql_integration_tests.rs`** (~350 lines)

Move executor, mutation, subscription, and introspection tests.

- [ ] **Step 5: Verify**

```bash
cargo test -p riversd --test graphql_config_tests
cargo test -p riversd --test graphql_schema_tests
cargo test -p riversd --test graphql_integration_tests
```

- [ ] **Step 6: Commit**

```bash
git add crates/riversd/tests/
git rm crates/riversd/tests/graphql_tests.rs
git commit -m "refactor(riversd): split graphql_tests.rs into 3 test modules"
```

---

## Phase 5: CLI

### Task 20: Split `riversctl/src/main.rs` (756 LOC -> 5 modules)

**Files:**
- Modify: `crates/riversctl/src/main.rs` (keeps main + dispatch)
- Create: `crates/riversctl/src/commands/mod.rs`
- Create: `crates/riversctl/src/commands/start.rs`
- Create: `crates/riversctl/src/commands/doctor.rs`
- Create: `crates/riversctl/src/commands/validate.rs`
- Create: `crates/riversctl/src/commands/admin.rs`

- [ ] **Step 1: Extract `commands/start.rs`**

Move lines 136-255: `cmd_start()`, `launch_riversd()` (Unix/Windows), `riversd_binary_name()`, `find_riversd_binary()`, `find_in_path()`.

- [ ] **Step 2: Extract `commands/doctor.rs`**

Move lines 256-408: `cmd_doctor()`, `load_config_for_tls()`.

- [ ] **Step 3: Extract `commands/validate.rs`**

Move lines 539-625: `cmd_validate()`.

- [ ] **Step 4: Extract `commands/admin.rs`**

Move lines 410-478 + 627-840: `sign_request()`, `admin_get()`, `admin_post()`, `cmd_status()`, `cmd_deploy()`, `cmd_drivers()`, `cmd_datasources()`, `cmd_health()`, `cmd_stop()`, `cmd_graceful()`, `signal_riversd()`, `find_riversd_pids()`, `kill_pid()`, `cmd_log()`.

- [ ] **Step 5: Extract `commands/mod.rs`**

```rust
pub mod start;
pub mod doctor;
pub mod validate;
pub mod admin;

// Also extract cmd_exec if applicable
```

- [ ] **Step 6: Update `main.rs`**

Keep only: `mod commands;`, `mod tls;`, `fn main()`, `fn print_usage()`, and the match dispatch that calls into `commands::*`.

- [ ] **Step 7: Verify**

```bash
cargo check -p riversctl
cargo test -p riversctl
```

- [ ] **Step 8: Commit**

```bash
git add crates/riversctl/src/
git commit -m "refactor(riversctl): split main.rs into 5 modules"
```

---

## Final Verification

### Task 21: Full workspace verification

- [ ] **Step 1: Full build**

```bash
cargo build --workspace
```

- [ ] **Step 2: Full test suite**

```bash
cargo test --workspace
```

- [ ] **Step 3: Verify no file exceeds 400 LOC**

```bash
find crates/ -name "*.rs" -exec awk '
BEGIN { in_block=0 }
/^[[:space:]]*\/\// { next }
/\/\*/ { in_block=1 }
in_block && /\*\// { in_block=0; next }
in_block { next }
/^[[:space:]]*$/ { next }
{ count++ }
END { if (count > 400) printf "%6d  %s\n", count, FILENAME }
' {} \;
```

Review any remaining files over 400 lines. Some may be acceptable (e.g., server/lifecycle.rs at ~400, engine_loader/host_context.rs with many FFI callbacks). Document exceptions.

- [ ] **Step 4: Commit final state**

```bash
git add -A
git commit -m "refactor: complete large file decomposition — 20 files split into ~75 modules"
```
