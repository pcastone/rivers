# Tasks — Phase BD: Split rivers-core + Dynamic Driver Loading

**Source:** Binary size analysis — riversctl is 13M because rivers-core bundles all drivers
**Baseline:** Phase BC complete, thin riversd is 17M, 232 lib tests passing
**Scope:** Split rivers-core into config (light) + drivers (heavy), make built-in drivers dynamic too

**Target:**
```
bin/
  riversd          (~5M)   — config + HTTP server + engine/plugin loader
  riversctl         (~3M)   — config + reqwest + TLS + crypto
  rivers-lockbox   (~1M)   — config + age encryption
  riverpackage    (~500K)   — config + validation only
lib/
  librivers_engine_v8.dylib     (23M)
  librivers_engine_wasm.dylib    (9M)
  librivers_drivers_builtin.dylib (~8M)  — postgres, mysql, sqlite, redis, memcached
plugins/
  librivers_plugin_*.dylib       (1-6M each)
```

---

## Phase 1: Split rivers-core into two crates

**Problem:** `rivers-core` is 1 crate with config types AND 7 database drivers + StorageEngine + EventBus + LockBox. Every binary that imports config types also links all drivers.

### What goes where:

**`rivers-core-config` (NEW, ~light):**
- `config.rs` — ServerConfig and all sub-types
- `event.rs` — Event, LogLevel
- `error.rs` — RiversError
- `lockbox.rs` — LockBoxConfig type (NOT the resolver/encryption)
- Re-exports for backward compat

**`rivers-core` (keeps heavy stuff, depends on rivers-core-config):**
- `driver_factory.rs` — DriverFactory, load_plugins
- `drivers/` — postgres, mysql, sqlite, redis, memcached, faker, eventbus
- `storage.rs` — StorageEngine trait + InMemory
- `storage_redis.rs`, `storage_sqlite.rs`
- `eventbus.rs` — EventBus, EventHandler
- `lockbox.rs` — resolver, encryption, fetch_secret_value
- `logging.rs` — LogHandler
- `tls.rs` — cert generation

### Steps:

- [x] **BD1.1**: Create `crates/rivers-core-config/` with config types, Event, LogLevel, RiversError
- [x] **BD1.2**: Make `rivers-core` depend on `rivers-core-config` and re-export everything for backward compat
- [~] **BD1.3**: Change `riversctl` to depend on `rivers-core-config` instead of `rivers-core`
  - PARTIAL: Config-type imports switched to rivers-core-config. Still needs rivers-core for tls, lockbox, DriverFactory, StorageEngine. Full switch deferred to Phase 5.
- [x] **BD1.4**: Change `riverpackage` to depend on `rivers-core-config` instead of `rivers-core`
- [x] **BD1.5**: Change `rivers-data` to depend on `rivers-core-config` for types (keep `rivers-core` for validation that needs drivers)
- [x] **BD1.6**: Fix all imports — nothing should break since rivers-core re-exports everything
  - Removed orphaned config.rs, error.rs, event.rs from rivers-core/src
  - **Validated:** cargo check --workspace (clean), cargo test --workspace --lib (232 pass), cargo test --package rivers-data --test config_tests (30 pass)

## Phase 2: Extract built-in drivers to a cdylib

**Problem:** postgres, mysql, sqlite, redis, memcached are compiled into riversd even with `--no-default-features`. They live in `rivers-core/src/drivers/`.

### Steps:

- [x] **BD2.1**: Create `crates/rivers-drivers-builtin/` — `crate-type = ["cdylib", "rlib"]`
- [x] **BD2.2**: Move `rivers-core/src/drivers/` contents into new crate
- [x] **BD2.3**: Export `_rivers_abi_version()` + `_rivers_register_driver()`
- [x] **BD2.4**: Add `static-builtin-drivers` feature to riversd + `drivers` feature to rivers-core
- [x] **BD2.5**: `register_all_drivers()` loads from `lib/` dir dynamically + `static-builtin-drivers` for static
  - Removed tokio-postgres, mysql_async, async-memcached from rivers-core deps
  - **Validated:** cargo check --workspace (clean), cargo test --workspace --lib (232 pass)

## Phase 3: Extract StorageEngine backends to cdylib

**Problem:** `storage_redis.rs` and `storage_sqlite.rs` pull in `redis` and `rusqlite` crates.

### Steps:

- [x] **BD3.1**: Create `crates/rivers-storage-backends/` — `crate-type = ["cdylib", "rlib"]`
- [x] **BD3.2**: Move `storage_redis.rs` and `storage_sqlite.rs` into new crate
- [x] **BD3.3**: StorageEngine trait + InMemoryStorageEngine moved to `rivers-core-config`
- [x] **BD3.4**: `create_storage_engine()` uses feature-gated backends; removed redis/rusqlite from rivers-core deps
  - **Validated:** cargo check --workspace (clean), cargo test --workspace --lib (232 pass)

## Phase 4: Extract LockBox encryption to cdylib

**Problem:** `age` encryption crate adds ~1M. Only needed when LockBox is configured.

### Steps:

- [x] **BD4.1**: Create `crates/rivers-lockbox-engine/` — `crate-type = ["cdylib", "rlib"]`
- [x] **BD4.2**: Move `lockbox.rs` into new crate; rivers-core re-exports behind `lockbox` feature
- [x] **BD4.3**: `rivers-lockbox` CLI depends on `rivers-lockbox-engine` directly (no rivers-core)
- [x] **BD4.4**: rivers-core `lockbox` feature (default ON) for backward compat; `age`/`zeroize` now optional
  - **Validated:** cargo check --workspace (clean), cargo test --workspace --lib (232 pass)

## Phase 5: Slim down riversctl

**After Phases 1-4, riversctl depends on:**
- `rivers-core-config` (~light config types)
- `rivers-data` (validation — but rivers-data still depends on rivers-core for DataViewExecutor...)

### Steps:

- [ ] **BD5.1**: Split `rivers-data` validation into `rivers-data-validate` (no executor dependency)
  - DEFERRED: Requires deeper rivers-data refactor. rivers-data uses rivers-core with no-default-features already.
- [~] **BD5.2**: `riversctl` depends on `rivers-core-config` + `rivers-lockbox-engine` + `rivers-core` (TLS only)
  - Config types → rivers-core-config, InMemoryStorage → rivers-core-config, lockbox → rivers-lockbox-engine
  - rivers-core kept only for tls module. Full removal requires extracting TLS to separate crate.
- [x] **BD5.3**: `riversctl validate` uses lightweight driver name list (hardcoded) instead of importing DriverFactory
- [~] **BD5.4**: Build and measure: riversctl = 7.7M (was 13M, 40% reduction). Target 3-4M requires BD5.1.
  - **Validated:** ls -lh target/release/riversctl → 7.7M

## Phase 6: Slim down riversd thin binary

**After all phases, riversd --no-default-features has:**
- Axum HTTP server (~2M)
- reqwest (~2M) — for Rivers.http outbound and health probes
- TLS (rustls + rcgen) (~1M)
- async-graphql (~1M)
- rivers-core-config (~light)
- engine_loader + libloading (~100K)

### Steps:

- [~] **BD6.1**: Build `riversd --no-default-features` = 15M (was 17M, 12% reduction). Target < 8M requires further dep analysis.
- [ ] **BD6.2**: Update `scripts/build-release.sh` to build thin binary + all dylibs
- [ ] **BD6.3**: Final release layout:

```
bin/
  riversd            (5-8M)
  riversctl           (3-4M)
  rivers-lockbox      (~1M)
  riverpackage       (~500K)
lib/
  librivers_engine_v8.dylib         (23M)
  librivers_engine_wasm.dylib        (9M)
  librivers_drivers_builtin.dylib   (~8M)
  librivers_storage_backends.dylib  (~3M)
  librivers_lockbox_engine.dylib    (~1M)
plugins/
  librivers_plugin_cassandra.dylib   (2M)
  ... (10 plugin drivers)
```

  - **Validate:** full release test, address-book bundle runs

---

## Expected Size Impact

| Binary | Before | After |
|--------|--------|-------|
| `riversd` (thin) | 17M | ~5-8M |
| `riversctl` | 13M | ~3-4M |
| `rivers-lockbox` | 957K | ~500K |
| `riverpackage` | 643K | ~400K |

**Total deployed size stays ~100M** but split across independently updatable, independently deployable components. Operators deploy only what they need.

## Risk Notes

- **Phase 1 is the foundation** — everything else depends on the config/driver split
- **Backward compat** — `rivers-core` re-exports everything so existing `use rivers_core::*` code doesn't break
- **Phase 5 is the hardest** — rivers-data has tight coupling to rivers-core types (DataViewExecutor references DriverFactory, ConnectionParams). May need a trait boundary or split rivers-data too.
- **Recommended order:** Phase 1 first (immediate value for riversctl), then Phase 2 (drivers), then 3-6
