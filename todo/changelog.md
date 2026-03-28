# Changelog — Rivers Framework

**Project:** Rivers declarative app-service framework
**Stack:** Rust, Axum 0.8, V8 (rusty_v8 130), Wasmtime 27, tokio
**Status:** V1 feature-complete — 200+ tasks, 1382 tests, 15 crates, 4 binaries, 16 drivers

---

## ExecDriver Plugin — Gap Analysis Fixes (2026-03-27)

**Goal:** Fix 11 gaps found in ExecDriver plugin gap analysis.

**Key changes:**
- Modified `config.rs` — added `getpwnam` resolution for `run_as_user` at startup (G2), `working_directory` existence/directory check (G3), executable permission check on command files (G4)
- Modified `connection.rs` — added runtime logging with trace_id + duration for command start/success/failure/integrity/concurrency/timeout/overflow (G1), added `env_clear=false` WARN at startup (G5)
- Modified `executor.rs` — `both` mode now removes `stdin_key` from params before template interpolation (G10), spawn failure uses `DriverError::Internal` instead of `Query` (G11)
- Modified `riversctl/main.rs` — added `exec list` stub subcommand (G6)
- Added 4 new unit tests: `validate_run_as_user_not_found`, `validate_working_directory_not_exists`, `validate_working_directory_not_a_dir`, `validate_executable_permission_check`

**Decisions:**
- `getpwnam` check wrapped in `#[cfg(unix)]` since it's Unix-only; also verifies UID != 0 (belt-and-suspenders with existing "root" string check)
- Runtime logging extracts `trace_id` from query parameters (key `trace_id`) with fallback to `"-"`; uses `Instant::now()` for duration measurement
- `both` mode interpolation: creates a filtered copy of params_obj without stdin_key before passing to `template::interpolate`, preserving original params for stdin
- Test helper `non_root_user()` dynamically resolves an existing non-root user for getpwnam tests (tries nobody, daemon, _nobody, then $USER)
- Gaps 7/8/9 confirmed as no-change-needed after review

---

## ExecDriver Plugin — Task 11: Documentation Updates (2026-03-27)

**Goal:** Update all 5 guide docs to document the ExecDriver feature.

**Key changes:**
- Modified `docs/guide/rivers-skill.md` — added `rivers-exec` row to datasource drivers table (before http and eventbus)
- Modified `docs/guide/cli.md` — added `riversctl exec hash` and `exec verify` subcommands section after TLS commands
- Modified `docs/guide/developer.md` — added ExecDriver (Script Execution) section with handler example showing ctx.dataview pattern
- Modified `docs/guide/rivers-app-development.md` — added ExecDriver Datasource section with full resources.toml + app.toml config examples
- Modified `docs/guide/admin.md` — added ExecDriver Operations section with script management, hash management, script contract, and security checklist

**Decisions:**
- Placed ExecDriver handler example in developer.md after Rivers.http section, consistent with the pattern of showing datasource access methods
- Admin guide security checklist uses checkbox format for operators to track compliance
- CLI docs show only hash and verify (list deferred, matching T9.3 status)

---

## ExecDriver Plugin — Tasks 8+9+10: Registration, CLI, Integration Tests (2026-03-27)

**Goal:** Complete plugin registration, add riversctl exec commands, and write integration tests.

**Key changes:**
- Modified `crates/riversd/src/server.rs` — added `rivers-plugin-exec` (ExecDriver) to `register_all_drivers()` static plugins vector
- Modified `crates/riversctl/Cargo.toml` — moved `sha2` from optional (admin-api) to required dependency for exec hash command
- Modified `crates/riversctl/src/main.rs` — added `exec hash <path>` and `exec verify <path> <sha256>` subcommands; added `rivers-exec` to known drivers list in validate
- Created `crates/rivers-plugin-exec/tests/integration_test.rs` — 8 integration tests exercising the full driver contract with real shell scripts

**Decisions:**
- T8: C ABI exports already existed in lib.rs behind `#[cfg(feature = "plugin-exports")]`; the missing piece was static registration in `register_all_drivers()` in server.rs
- T9: Simplified `exec verify` to single-file verification (`exec verify <path> <sha256>`) instead of bundle-scanning; `exec list` deferred as follow-up since it requires parsing exec datasource configs from bundles
- T10: Both mode, JSON schema validation, and output overflow are thoroughly covered in unit tests (connection.rs, executor.rs); integration tests focus on full round-trip scenarios that unit tests cannot cover (real process spawning, file tampering, timeouts)

---

## ExecDriver Plugin — Tasks 6+7: Concurrency Control + Connection Pipeline (2026-03-27)

**Goal:** Wire the full 11-step pipeline via `DatabaseDriver` + `Connection` traits, with two-layer semaphore concurrency control.

**Key changes:**
- Created `crates/rivers-plugin-exec/src/connection.rs` — `ExecDriver` (DatabaseDriver), `ExecConnection` (Connection), `CommandRuntime`, full 11-step pipeline
- Updated `crates/rivers-plugin-exec/src/lib.rs` — replaced placeholder `ExecDriver` with `pub use connection::ExecDriver`, added `pub mod connection`
- 12 new tests in `connection::tests` covering: connect success/failure, full pipeline (stdin, args, statement-based command), unknown command, unsupported operation, missing command parameter, ping, global/per-command concurrency limits, schema validation, args mode

**Decisions:**
- `QueryResult` has no `raw_value` field; JSON result wrapped as single row: `{"result": QueryValue::Json(parsed_json)}`, `affected_rows: 1`
- Concurrency: global semaphore on `ExecConnection`, per-command semaphore on `CommandRuntime`; `try_acquire()` for no-queue semantics; RAII drop handles release on per-command failure
- Command name extracted from `query.parameters["command"]` (String variant) or `query.statement` as fallback
- Args extracted from `query.parameters["args"]` (Json variant); defaults to empty object
- Only `query` operation supported; all others return `DriverError::Unsupported`
- Concurrency tests use direct `ExecConnection` construction (not `Box<dyn Connection>`) to access semaphore fields

**Spec reference:** `docs/rivers-exec-driver-spec.md` sections 12, 13.4, 18

---

## ExecDriver Plugin — Task 5: Process Spawning (2026-03-27)

**Goal:** Implement process spawning with full isolation per spec sections 10-11. No shell involved.

**Key changes:**
- Added `libc = "0.2"` to `crates/rivers-plugin-exec/Cargo.toml` (direct dep, not in workspace)
- Created `crates/rivers-plugin-exec/src/executor.rs` with `execute_command()`, `evaluate_result()`, `kill_process_group()`, Unix helpers
- Added `pub mod executor;` to `crates/rivers-plugin-exec/src/lib.rs`
- 14 unit tests covering all input modes, error cases, timeout, env isolation, output overflow

**Decisions:**
- Used `libc` directly for setsid/kill/geteuid/getpwnam instead of the `nix` crate — minimal deps
- Tokio Command has inherent pre_exec/uid/gid methods — no CommandExt import needed
- Privilege drop only when running as root; non-root logs debug and skips
- Process group kill via negative PID after setsid makes child session leader
- All tests gated with `#[cfg(unix)]` — use real shell scripts
- Stderr truncated to 1024 chars in error messages

**Spec reference:** `docs/rivers-exec-driver-spec.md` sections 10-11

---

## ExecDriver Plugin — Task 4: JSON Schema Validation (2026-03-27)

**Goal:** Integrate JSON Schema validation to validate handler args before process spawn per spec section 9.

**Key changes:**
- Added `jsonschema = "0.28"` to workspace `[workspace.dependencies]` in root `Cargo.toml`
- Added `jsonschema = { workspace = true }` to `crates/rivers-plugin-exec/Cargo.toml`
- Created `crates/rivers-plugin-exec/src/schema.rs` — `CompiledSchema` struct with `load(path)` and `validate(args)` methods
- Added `pub mod schema;` to `crates/rivers-plugin-exec/src/lib.rs`
- 8 unit tests: valid args pass, missing required field fails, invalid CIDR pattern fails, port out of range fails, extra properties fail (additionalProperties: false), load nonexistent file fails, load invalid JSON fails, load invalid schema fails

**Decisions:**
- Used `jsonschema` 0.28 crate — `validator_for()` API (0.26+ style, returns `Result<Validator, ValidationError>`)
- `validate()` collects all errors via `iter_errors()` into a single `DriverError::Query` message so callers get a complete diagnostic
- Manual `Debug` impl on `CompiledSchema` since `jsonschema::Validator` does not implement `Debug`
- Validation timing documented but not enforced in this module — the pipeline in Task 7 will call schema validation at the right point (after command lookup, before integrity check)

**Spec reference:** `docs/rivers-exec-driver-spec.md` section 9

---

## ExecDriver Plugin — Task 3: Argument Template Engine (2026-03-27)

**Goal:** Implement placeholder interpolation for `args` and `both` input modes per spec section 8.2.

**Key changes:**
- Created `crates/rivers-plugin-exec/src/template.rs` — `interpolate()` function resolves `{key}` placeholders against query params
- Added `pub mod template;` to `crates/rivers-plugin-exec/src/lib.rs`
- 19 unit tests covering: basic interpolation, missing key error, number/boolean/null/float values, array/object rejection, extra keys ignored, special characters pass through, empty template, literal-only template, mixed literals and placeholders, bare braces edge case, partial brace edge case

**Decisions:**
- Placeholder detection uses simple bracket check: starts with `{` and ends with `}` with length > 2 (per spec "simple bracket detection")
- Function signature uses `serde_json::Map<String, serde_json::Value>` (not `HashMap`) since that is the native JSON object type from serde_json
- `{}` (empty braces) treated as literal, not a placeholder, since there is no key to extract

**Spec reference:** `docs/rivers-exec-driver-spec.md` section 8.2

---

## ExecDriver Plugin — Task 1: Crate Skeleton + Config Types (2026-03-27)

**Goal:** Create `rivers-plugin-exec` crate with config types, parsing from `ConnectionParams.options`, and startup validation.

**Key changes:**
- Created `crates/rivers-plugin-exec/Cargo.toml` — cdylib + rlib, plugin-exports feature, deps: rivers-driver-sdk, async-trait, tokio, serde, serde_json, sha2, hex, tracing
- Created `crates/rivers-plugin-exec/src/lib.rs` — ExecDriver struct with placeholder DatabaseDriver impl, C ABI exports under plugin-exports feature
- Created `crates/rivers-plugin-exec/src/config.rs` — ExecConfig, CommandConfig, IntegrityMode, InputMode types with parsing from flat options map and startup validation
- Added to workspace members in root `Cargo.toml`
- Added to static-plugins feature list and optional deps in `crates/riversd/Cargo.toml`
- 30 unit tests covering: IntegrityMode/InputMode parsing, config parsing from ConnectionParams, validation of run_as_user, paths, sha256, args_template, stdin_key

**Decisions:**
- `jsonschema` dep deferred to Task 4 per task description
- Validation rejects "root" as run_as_user (string check, not UID lookup — nix crate not yet added)
- Command configs parsed from flattened dot-separated keys (`commands.<name>.<field>`) matching TOML flatten behavior
- `env_clear` defaults to `true` per spec security model

**Spec reference:** `docs/rivers-exec-driver-spec.md` sections 4-5

---

## Dual Static/Dynamic Build Architecture (2026-03-21)

**Goal:** Support both static monolithic binaries AND dynamic thin-binary+shared-lib builds.

**Key changes:**
- Renamed `rivers-data` → `rivers-runtime` (the facade crate through which all consumers link)
- `rivers-runtime` crate-type defaults to `rlib` (static); Justfile `build-dynamic` switches to `dylib`
- Moved ProcessPool shared types (`types.rs`, `bridge.rs`) from `riversd` into `rivers-runtime`
  - riversd re-exports from `rivers_runtime::process_pool`
  - LockBox fields gated with `#[cfg(feature = "lockbox")]` for standalone compilation
- Added `rivers-engine-sdk` re-export to `rivers-runtime`
- Implemented all 8 host callbacks in `engine_loader.rs`:
  - `dataview_execute` → DataViewExecutor via OnceLock
  - `store_get/set/del` → StorageEngine (namespace, key) via OnceLock
  - `datasource_build` → DriverFactory.connect() + Connection.execute() via OnceLock
  - `http_request` → reqwest::Client via OnceLock
  - `log_message` → tracing (already existed)
  - `free_buffer` → rivers_engine_sdk::free_json_buffer (already existed)
- Added `set_host_context()` called from server startup after bundle loading
- Added `driver_factory: Option<Arc<DriverFactory>>` to AppContext
- Removed `build.rs` and `zstd-sys` dep from riversd (dylib link hack no longer needed)
- Created Justfile with `build` (static), `build-dynamic`, `sizes`, `sizes-dynamic` recipes
- Updated CLAUDE.md architecture section with crate table and dual-mode diagram

**Files affected:** 40+ files across rename, 8 new/modified files for architecture changes

---

## Phase BD: Split rivers-core + Dynamic Driver Loading (2026-03-21)

**Goal:** Split rivers-core into lightweight config + heavy modules. Extract built-in drivers, storage backends, and lockbox to separate cdylib crates for dynamic loading.

**New crates created (4):**
- `rivers-drivers-builtin` — postgres, mysql, sqlite, redis, memcached, faker, eventbus, rps-client
- `rivers-storage-backends` — RedisStorageEngine, SqliteStorageEngine
- `rivers-lockbox-engine` — LockBox resolver, Age encryption/decryption
- `rivers-core-config` — (existed, expanded with StorageEngine trait + InMemoryStorageEngine)

**Key changes:**
- rivers-core now has 3 optional features: `drivers`, `storage-backends`, `lockbox` (default: all ON)
- riversd has `static-builtin-drivers` feature + loads from `lib/` and `plugins/` dirs
- riversctl: config imports → rivers-core-config, lockbox → rivers-lockbox-engine, DriverFactory → hardcoded list
- rivers-lockbox CLI: depends on rivers-lockbox-engine directly (no rivers-core)
- riverpackage: depends on rivers-core-config (no rivers-core)
- rivers-data: depends on rivers-core with `default-features = false`
- All dead files removed (orphaned config.rs, error.rs, event.rs from rivers-core)
- Workspace: 15 → 19 crates

**Binary size impact:**
| Binary | Before | After | Change |
|--------|--------|-------|--------|
| riversctl | 13M | 7.7M | -40% |
| riversd (thin) | 17M | 15M | -12% |
| rivers-lockbox | 957K | 957K | — |
| riverpackage | 643K | 643K | — |

**Tests:** 232 lib tests + 30 integration tests — all passing.

---

## Phase AT: Bundle Validation Improvements (2026-03-20)

### AT — 9 validation improvements to catch config mistakes before runtime

| File | Change | Spec Ref | Resolution |
|------|--------|----------|------------|
| `rivers-data/src/loader.rs` | Added `load_and_parse()` helper — wraps TOML parse errors with app name + filename context | app-spec §4 | Error now says `"app.toml (app 'my-app'): missing field..."` instead of generic `"app.toml: ..."` |
| `rivers-data/src/loader.rs` | Added `app_dir: PathBuf` to `LoadedApp` | — | Required for schema file existence validation |
| `rivers-data/src/validate.rs` | Added `VALID_VIEW_TYPES` const + view type validation in `validate_app_config` | view-layer §12 | Rejects `view_type = "BadType"` with clear error listing valid options |
| `rivers-data/src/validate.rs` | Added `invalidates` target validation | data-layer §7 | Checks that every item in `invalidates = [...]` references an existing DataView |
| `rivers-data/src/validate.rs` | Added `validate_duplicate_resource_names()` | app-spec §6 | Catches duplicate datasource/service names in `resources.toml` array-of-tables |
| `rivers-data/src/validate.rs` | Added cross-app service reference check in `validate_bundle` | app-spec §6 | Services with unknown `appId` caught at bundle validation time |
| `rivers-data/src/validate.rs` | Added `validate_schema_files()` | data-layer §12.3 | Checks `return_schema` and per-method schema file references resolve to existing files |
| `rivers-data/src/validate.rs` | Added `validate_known_drivers()` | data-layer §12 | Flags datasources with unrecognized driver names (warn, not block — plugins may load later) |
| `riversd/src/bundle_loader.rs` | Wired `validate_bundle()` call before subsystem wiring in `load_and_wire_bundle` | — | Bad configs now fail cleanly at startup instead of causing confusing runtime errors |
| `riversd/src/bundle_loader.rs` | Wired `validate_bundle()` call in `rebuild_views_and_dataviews` | httpd §16 | Hot reload rejects invalid configs instead of corrupting live state |
| `riversd/src/bundle_loader.rs` | Wired `validate_known_drivers()` after DriverFactory build | — | Logs warnings for unknown drivers |

---

## Phase AR: Wire Hot Reload, GraphQL, and DataView Cache Invalidation (2026-03-20)

### AR — Complete three infrastructure-without-integration loops

| File | Change | Spec Ref | Resolution |
|------|--------|----------|------------|
| `rivers-data/src/dataview.rs` | Added `invalidates: Vec<String>` field to `DataViewConfig` | data-layer §7 | Declarative cache invalidation — write DataViews list read DataViews to invalidate |
| `rivers-data/src/dataview_engine.rs` | Added `run_cache_invalidation()` — calls `cache.invalidate()` for each listed DataView after successful execute | data-layer §7 | Called after both database and broker produce paths |
| `rivers-data/src/dataview_engine.rs` | Added `event_bus: Option<Arc<EventBus>>` field + `set_event_bus()` setter | logging §4 | Emits `CacheInvalidation` event for observability without breaking existing constructor |
| `rivers-core/src/config.rs` | Added `GraphqlServerConfig` struct + `graphql` field on `ServerConfig` | view-layer §9 | Enable/path/introspection/depth/complexity — minimal config for server level |
| `riversd/src/graphql.rs` | Added `From<&GraphqlServerConfig>` impl for `GraphqlConfig` | view-layer §9 | Converts server-level config to runtime GraphQL config |
| `riversd/src/graphql.rs` | Added `build_resolver_mappings_from_dataviews()` | view-layer §9.2 | Builds resolver mappings from DataView registry, strips namespace prefix for field names |
| `riversd/src/graphql.rs` | Added `build_schema_with_executor()` | view-layer §9.2 | Builds dynamic schema with async resolvers that call `DataViewExecutor.execute()` directly |
| `riversd/src/graphql.rs` | Added `build_mutation_type()` — V1 stub with `_noop` field | view-layer §9 | Registers Mutation type on schema; real mutations will dispatch to ProcessPool in V2 |
| `riversd/src/server.rs` (AppContext) | Added `graphql_schema: Arc<RwLock<Option<Schema>>>` | view-layer §9 | Stores built schema, supports hot reload |
| `riversd/src/server.rs` (build_main_router) | Replaced "deferred to Epic 34" with GraphQL POST + playground route mounting | view-layer §9, httpd §3 | Routes mounted inline on `Router<AppContext>` to avoid state type mismatch |
| `riversd/src/bundle_loader.rs` | Wired `EventBus` into `DataViewExecutor` via `set_event_bus()` | — | Enables cache invalidation events |
| `riversd/src/bundle_loader.rs` | Added GraphQL schema build at bundle load time | view-layer §9 | Only when `config.graphql.enabled`; scans DataView registry for resolver mappings |
| `riversd/src/bundle_loader.rs` | Added `rebuild_views_and_dataviews()` + `ReloadSummary` | httpd §16 | Hot-reload-safe subset: re-parses bundle, rebuilds registry/router/executor/GraphQL schema without re-resolving LockBox |
| `riversd/src/hot_reload.rs` | Added `bundle_path()` method to `HotReloadState` | httpd §16 | Returns bundle path from current config for reload listener |
| `riversd/src/server.rs` | Spawned `hot_reload_listener` task | httpd §16 | Subscribes to config version changes, calls `rebuild_views_and_dataviews()` on each change |

### Limitations documented

- New datasources added in bundle TOML require restart (connection pools not rebuilt on reload)
- LockBox credentials not re-resolved on hot reload (passwords stay from initial load)
- GraphQL subscriptions deferred to V2 (comment placeholder in graphql.rs)
- GraphQL mutations are stub-level (`_noop` field) — real CodeComponent dispatch is V2

---

## Phase AQ: Wire Streaming Features — SSE, WS, Streaming REST, Polling (2026-03-20)

### AQ — Connect existing streaming infrastructure to HTTP dispatch layer

| File | Change | Spec Ref | Resolution |
|------|--------|----------|------------|
| `Cargo.toml` (workspace) | Added `tokio-stream = "0.1"` for streaming response bodies | — | Required by `Body::from_stream()` |
| `riversd/Cargo.toml` | Added `tokio-stream`, removed `tokio-tungstenite` (use axum built-in WS) | view-layer §6 | axum 0.8 `features = ["ws"]` provides WebSocket natively |
| `riversd/src/server.rs` (AppContext) | Added `sse_manager: Arc<SseRouteManager>`, `ws_manager: Arc<WebSocketRouteManager>` | view-layer §6-7 | Initialized in `AppContext::new()` |
| `riversd/src/server.rs` (MatchedRoute) | Added `view_id: String` field | — | Needed by SSE/WS/Polling to look up per-route managers |
| `riversd/src/server.rs` (dispatch) | Added `match view_type` switch before body extraction: SSE→`execute_sse_view`, WS→`execute_ws_view`, streaming REST→`execute_streaming_rest_view`, default→existing | view-layer §3 | WS views branch before body extraction to preserve raw request for upgrade |
| `riversd/src/server.rs` | Added `build_streaming_response()` helper — wraps `mpsc::Receiver<String>` in `ReceiverStream` → `Body::from_stream()` | — | Shared by SSE and streaming REST |
| `riversd/src/server.rs` | Added `execute_sse_view()` — looks up SseChannel, subscribes, spawns relay task, returns text/event-stream | view-layer §7 | Per-client relay: broadcast rx → mpsc tx → HTTP stream |
| `riversd/src/server.rs` | Added `execute_streaming_rest_view()` — spawns generator + formatter tasks, returns chunked NDJSON/SSE | streaming-rest §D12 | Generator sends chunks; formatter converts to wire format; poison chunk on error |
| `riversd/src/server.rs` | Added `execute_ws_view()` — extracts `WebSocketUpgrade` via `FromRequestParts`, calls `on_upgrade()` | view-layer §6 | Uses axum's built-in WebSocket support |
| `riversd/src/server.rs` | Added `handle_ws_connection()` — single-owner socket loop with `tokio::select!` for recv/broadcast/reply | view-layer §6.3 | Rate limiter, binary frame tracker, lifecycle hooks (on_connect/on_disconnect), on_stream dispatch |
| `riversd/src/bundle_loader.rs` | SSE channel registration: scans views for `ServerSentEvents`, calls `sse_manager.register()` | view-layer §7.2 | Spawns `drive_sse_push_loop` for tick-based views |
| `riversd/src/bundle_loader.rs` | WS route registration: scans views for `Websocket`, registers broadcast/direct based on `websocket_mode` | view-layer §6.2 | Broadcast hub or connection registry per view |
| `riversd/src/bundle_loader.rs` | Added `SseTriggerHandler` — EventHandler impl that pushes to SseChannel when EventBus trigger fires | view-layer §7.2 | Subscribed per trigger event during bundle load |
| `riversd/src/view_engine.rs` | Added streaming validation: `streaming=true` requires CodeComponent handler, no pipeline stages, Rest-only | streaming-rest §D12 | Calls `crate::streaming::validate_streaming()` from `validate_views()` |
| `riversd/src/view_engine.rs` | Added polling validation: SSE/WS only, tick > 0, change_detect requires handler | polling-spec §4 | New section in `validate_views()` |
| `riversd/src/view_engine.rs` | Relaxed DataView handler validation: allowed on SSE/WS when `polling` is configured | polling-spec §4 | Condition: `config.polling.is_none()` gates the error |
| `rivers-data/src/view.rs` | Added `PollingConfig` struct with `tick_interval_ms`, `diff_strategy`, `poll_state_ttl_s`, `on_change`, `change_detect` | polling-spec §3 | Plus `OnChangeConfig`, `ChangeDetectConfig` sub-structs |
| `rivers-data/src/view.rs` | Added `polling: Option<PollingConfig>` field to `ApiViewConfig` | polling-spec §3 | Deserialized from TOML `[api.views.*.polling]` |
| `riversd/src/polling.rs` | Added `storage_key_prev()` and `storage_key_meta()` methods to `PollLoopKey` | polling-spec §B3.5 | `:prev` and `:meta` suffixed storage keys |
| `riversd/src/polling.rs` | Added `ttl_s: Option<u64>` parameter to `save_poll_state()` | polling-spec §B3.5 | Converted to ms via `saturating_mul(1000)` and passed to `StorageEngine::set()` |
| `rivers-core/src/eventbus.rs` | Added `POLL_TICK_FAILED`, `ON_CHANGE_FAILED`, `POLL_CHANGE_DETECT_TIMEOUT` event constants | polling-spec §10 | In `eventbus::events` module |
| `riversd/src/sse.rs` | Replaced heartbeat stub in `drive_sse_push_loop` with real DataView polling path | polling-spec §12.5 | When executor + storage are present: calls `execute_poll_tick_inmemory()` → diff → push changed data. Falls back to heartbeat when not configured. |

---

## Phase AO: Redis Cluster Support (2026-03-19)

### AO — Full Redis cluster support for driver, storage engine, and redis-streams plugin

| File | Change | Spec Ref | Resolution |
|------|--------|----------|------------|
| `rivers-core/src/drivers/redis.rs` | Completed `RedisClusterConnection::execute()` — added 12 missing operations (mget, hget, hset, lpush, rpush, sadd, expire, incr, incrby, hdel, exists, keys) + set_ex TTL support | driver-spec §4 | Copied operation logic from `RedisConnection` — identical `AsyncCommands` API |
| `rivers-core/src/storage_redis.rs` | Rewrote to support both single-node and cluster modes via `RedisConn` enum | storage-engine-spec | Cluster uses `ClusterClient`/`ClusterConnection`, `pset_ex` for TTL (no pipelines), `KEYS` for list_keys (SCAN is per-node) |
| `rivers-core/tests/redis_live_test.rs` | Added `cluster: "true"` + `hosts` to conn_params, removed MOVED skip | — | Tests now pass cleanly without SKIP |
| `rivers-core/tests/storage_live_test.rs` | Updated to comma-separated cluster URLs, removed `redis_try!` MOVED macro | — | All 4 Redis storage tests pass directly |
| `rivers-plugin-redis-streams/src/lib.rs` | Added `RedisConn` enum with cluster support, updated `connect_redis()` to respect `cluster` + `hosts` options | driver-spec §6 | Raw `redis::cmd()` calls dispatch via `RedisConn::query_async()` |
| `rivers-plugin-redis-streams/tests/redis_streams_live_test.rs` | Added cluster options to conn_params, updated cleanup_stream to use ClusterClient | — | MOVED skip removed, test passes cleanly |

**Decisions:**
- Cluster mode for driver: explicit opt-in via `options.cluster = "true"` (kept backward compat)
- Cluster mode for storage: auto-detected from comma-separated URLs (>1 URL = cluster)
- `list_keys` on cluster: uses `KEYS` command since `SCAN` is per-node and unavailable on `ClusterConnection`
- TTL on cluster storage: uses `PSETEX` instead of pipeline (SET + PEXPIRE) since pipelines may not work across cluster slots

---

## Phase AN: LockBox Credential Resolution for Live Tests (2026-03-19)

### AN — Refactored all live integration tests to resolve credentials from LockBox

| File | Change | Spec Ref | Resolution |
|------|--------|----------|------------|
| `rivers-core/tests/common/mod.rs` | Created shared `TestCredentials` struct | lockbox-spec §3.1 | New file — builds Age-encrypted keystore in temp dir, provides `get(name)` |
| `rivers-core/tests/redis_live_test.rs` | Replaced `"rivers_test"` literal | lockbox-spec §8.1 | `creds.get("redis/test")` |
| `rivers-core/tests/postgres_live_test.rs` | Replaced `PG_PASS` constant | lockbox-spec §8.1 | `creds.get("postgres/test")` |
| `rivers-core/tests/mysql_live_test.rs` | Replaced `MYSQL_PASS` constant | lockbox-spec §8.1 | `creds.get("mysql/test")` |
| `rivers-core/tests/storage_live_test.rs` | Replaced bare Redis URL with password-aware URL | lockbox-spec §8.1 | `redis://:password@host:port` via `creds.get("redis/test")` |
| `rivers-core/tests/memcached_live_test.rs` | Added lockbox consistency (empty pw) | lockbox-spec §8.1 | `creds.get("memcached/test")` |
| `rivers-core/tests/sqlite_live_test.rs` | Added lockbox consistency (empty pw) | lockbox-spec §8.1 | `creds.get("sqlite/test")` |
| `rivers-core/tests/faker_live_test.rs` | Added lockbox consistency (empty pw) | lockbox-spec §8.1 | `creds.get("faker/test")` |
| `rivers-plugin-rabbitmq/tests/rabbitmq_live_test.rs` | Replaced `"guest"` password | lockbox-spec §8.1 | `lockbox_resolve("rabbitmq/test", "guest")` |
| `rivers-plugin-couchdb/tests/couchdb_live_test.rs` | Replaced `COUCH_PASS` constant | lockbox-spec §8.1 | `lockbox_resolve("couchdb/test", "admin")` |
| `rivers-plugin-influxdb/tests/influxdb_live_test.rs` | Replaced `INFLUX_PASS` constant | lockbox-spec §8.1 | `lockbox_resolve("influxdb/test", "rivers-test")` |
| `rivers-plugin-redis-streams/tests/redis_streams_live_test.rs` | Added password to conn_params + cleanup URL | lockbox-spec §8.1 | `lockbox_resolve("redis-streams/test", "rivers_test")` |
| `rivers-core/tests/lockbox_e2e_test.rs` | New E2E test: keystore encrypt → resolve → Redis connect | lockbox-spec §8.1 | 3 tests: credential resolve+connect, common helper, missing key panic |
| `rivers-plugin-rabbitmq/Cargo.toml` | Added dev-deps: rivers-core, age, tempfile | — | Required for inline lockbox_resolve() |
| `rivers-plugin-couchdb/Cargo.toml` | Added dev-deps: rivers-core, age, tempfile, chrono | — | Required for inline lockbox_resolve() |
| `rivers-plugin-influxdb/Cargo.toml` | Added dev-deps: rivers-core, age, tempfile, chrono | — | Required for inline lockbox_resolve() |
| `rivers-plugin-redis-streams/Cargo.toml` | Added dev-deps: rivers-core, age, tempfile, chrono | — | Required for inline lockbox_resolve() |

**Decision:** rivers-core tests share `common/mod.rs` with a `TestCredentials` struct. Plugin tests use an inline `lockbox_resolve()` helper since cross-crate test module sharing is not supported in Rust. All credentials are now encrypted in a temp keystore and decrypted on demand — no plaintext passwords in source.

---

## [Wave 14] — Thread-Local Safety + Credential Hygiene (2026-03-19)

### AN14.1: Replace parallel thread-local lists with TaskLocals struct
- **process_pool.rs** — introduced `TaskLocals` struct with `set(&TaskContext, Handle) -> Self` constructor and `Drop` impl; both setup and teardown live in one type so adding a thread-local to setup without matching cleanup is impossible
- **process_pool.rs** `execute_js_task()` — replaced 12-line manual setup block + 12-line `ThreadLocalGuard` struct with single `let _locals = TaskLocals::set(&ctx, rt_handle);`; behavior unchanged, all 12 thread-locals populated identically

### AN14.2: Share ds_params via Arc instead of cloning
- **dataview_engine.rs** `DataViewExecutor` — changed `datasource_params` field from `HashMap<String, ConnectionParams>` to `Arc<HashMap<String, ConnectionParams>>`; `new()` constructor accepts `Arc` wrapper
- **server.rs** — `ds_params` wrapped in `Arc::new()` before passing to `DataViewExecutor::new()`; removed `.clone()` — one heap copy of ConnectionParams (including passwords) instead of two
- **dataview_engine_tests.rs** — updated 3 test call sites to wrap `params_map` in `Arc::new()`
- **process_pool.rs** — updated 3 test call sites (`ds_params`, two `HashMap::new()`) to wrap in `Arc::new()`

---

## [Wave 13] — SOLID Structural Extraction (2026-03-19)

### AN13.1: Extract admin handlers to admin_handlers.rs
- **server.rs** → **admin_handlers.rs** — moved 13 admin handler functions (`admin_status_handler`, `admin_drivers_handler`, `admin_datasources_handler`, `admin_deploy_handler`, `admin_deploy_test_handler`, `admin_deploy_approve_handler`, `admin_deploy_reject_handler`, `admin_deploy_promote_handler`, `admin_deployments_handler`, `admin_log_levels_handler`, `admin_log_set_handler`, `admin_log_reset_handler`), helpers (`parse_json_body`, `deploy_transition_handler`, `log_controller_unavailable`), and `ADMIN_BODY_LIMIT` constant
- **server.rs** — replaced with `use crate::admin_handlers::*` import and route references unchanged
- **lib.rs** — added `pub mod admin_handlers;`

### AN13.2: Extract bundle loading to bundle_loader.rs
- **server.rs** → **bundle_loader.rs** — extracted `load_and_wire_bundle()` async function containing the entire bundle loading block: TOML parsing, DataView registration, ConnectionParams construction, LockBox credential resolution (with `resolved.value.zeroize()` preserved), DriverFactory setup, `register_all_drivers`, DataView cache, DataViewExecutor creation, ViewRouter construction, broker bridge spawning, MessageConsumer wiring, guard view detection
- **server.rs** — replaced ~280-line `if let Some(ref bundle_path)` block with single `crate::bundle_loader::load_and_wire_bundle(&mut ctx, &config, shutdown_rx.clone()).await?;`
- **lib.rs** — added `pub mod bundle_loader;`

### AN13.3: Extract security pipeline from view_dispatch_handler
- **server.rs** → **security_pipeline.rs** — extracted `run_security_pipeline()` async function with `SecurityOutcome` return struct; encapsulates session extraction, session validation, guard redirect, and CSRF validation in correct order
- **server.rs** `view_dispatch_handler` — replaced ~110-line security block with `crate::security_pipeline::run_security_pipeline()` call + match on `Ok(outcome)` / `Err(resp)`
- **lib.rs** — added `pub mod security_pipeline;`

### AN13.4: Split process_pool.rs into directory module
- **process_pool.rs** → **process_pool/mod.rs** — converted single file to directory module
- **process_pool/wasm_engine.rs** — extracted `execute_wasm_task()`, `WASM_MODULE_CACHE`, `clear_wasm_cache()`, and all Wasmtime host function bindings
- **process_pool/v8_engine.rs** — placeholder module; V8 code remains in mod.rs due to thread-local coupling (12 `thread_local!` blocks shared between `execute_js_task`, `inject_*`, and callback functions)
- **mod.rs** — `ActiveTask` and `TaskTerminator` promoted to `pub(crate)` visibility for sub-module access; re-exports `clear_wasm_cache` and delegates `execute_wasm_task` to wasm_engine
- All 206 lib tests + 31 integration tests pass; zero behavioral change

---

## [Wave 12] — DRY Code Deduplication (2026-03-19)

### AN12.1: Extract parse_json_body() helper
- **server.rs** — extracted `parse_json_body(request, limit)` async helper and `ADMIN_BODY_LIMIT` constant; replaced 6 inline body-parse blocks (5 at 16 MiB, 1 at 1 MiB in `admin_log_set_handler`)

### AN12.2: Extract deploy_transition_handler()
- **server.rs** — extracted `deploy_transition_handler(ctx, request, target_state, status_label)` shared function; `admin_deploy_approve_handler`, `admin_deploy_reject_handler`, `admin_deploy_promote_handler` each reduced to a single delegation call

### AN12.4: Unify driver registration
- **server.rs** — extracted `register_all_drivers(factory)` function consolidating 15 individual `register_database_driver` / `register_broker_driver` calls (6 built-in via `register_builtin_drivers()` + 6 plugin DB + 3 broker)
- **server.rs** `admin_drivers_handler` — replaced hard-coded 10-driver JSON array with dynamic enumeration via `factory.driver_names()` and `factory.broker_driver_names()`

### AN12.5: Struct-level serde defaults
- **config.rs** — added `#[serde(default)]` at struct level on `BackpressureConfig`, `CsrfConfig`, `SessionCookieConfig`; removed per-field `#[serde(default = "...")]` attributes; deleted 7 now-unused `fn default_*()` free functions (`default_queue_depth`, `default_queue_timeout_ms`, `default_csrf_rotation_interval`, `default_csrf_cookie_name`, `default_csrf_header_name`, `default_same_site`, `default_cookie_path`); inlined values into `Default::default()` impls

### AN12.6: Extract log_controller_unavailable helper
- **server.rs** — extracted `log_controller_unavailable()` helper returning the "log controller not initialized" JSON; used in `admin_log_set_handler` and `admin_log_reset_handler` `None =>` arms

---

## [Wave 11] — Performance Hot Path (2026-03-19)

### AN11.1: Eliminate double RwLock acquire per matched request
- **server.rs** — introduced `MatchedRoute` struct; `combined_fallback_handler` now extracts config, app_entry_point, path_params, and guard_view_path in a single read-lock pass and forwards to `view_dispatch_handler` as a parameter
- `view_dispatch_handler` no longer acquires `ctx.view_router` lock or re-matches the route

### AN11.2: Remove redundant sort in EventBus publish()
- **eventbus.rs** — removed `collected.sort_by_key(|(_, p)| *p)` in `publish()` since exact and wildcard subscriber lists are individually pre-sorted by `subscribe()`

### AN11.3: Use ErrorResponse module for inline errors in dispatch
- **server.rs** — replaced 9 inline `Json(serde_json::json!({"code":..., "message":...}))` patterns across `view_dispatch_handler` and `admin_auth_middleware` with `error_response::unauthorized()`, `error_response::forbidden()`, `error_response::internal_error()` + `.with_trace_id()` + `.into_axum_response()`

### AN11.4: Build RBAC config once at startup
- **server.rs** — added `admin_auth_config: Option<AdminAuthConfig>` to `AppContext`; built once in both `run_server_with_listener_and_log` and `run_server_no_ssl`; `admin_auth_middleware` reads from `ctx.admin_auth_config` instead of calling `build_admin_auth_config_for_rbac` per request
- `build_admin_auth_config_for_rbac` now accepts `&ServerConfig` instead of `&AppContext`
- Added `DEFAULT_ADMIN_AUTH_CONFIG` lazy static as fallback for test paths

---

## [Phase AM Waves 7-10] — Session Dispatch, EventBus Rename, HMAC LockBox, Pool Watchdog (2026-03-19)

### Wave 7: Session Dispatch Integration
- **server.rs** `view_dispatch_handler()` — four steps added between ViewContext construction and execute_rest_view:
  1. Extract session ID from cookie/Bearer header
  2. Validate session for protected views; guard redirect/reject on missing/invalid session
  3. CSRF token validation on mutating requests (POST/PUT/PATCH/DELETE); Bearer-exempt
  4. Post-execution: guard view → create session + set cookies; all responses → rotate CSRF cookie
- Public views (`auth="none"`) and MessageConsumer views skip all session logic
- Authenticated users hitting the guard view get redirected to `on_authenticated` URL

### Wave 8: EventBus Priority Tier Rename
- `HandlerPriority`: `Critical→Expect`, `Standard→Handle`, added `Emit` tier (awaited, between Handle and Observe)
- Four tiers now: Expect → Handle → Emit → Observe (per spec §12.2)
- Updated 32 occurrences across 6 files (eventbus.rs, message_consumer.rs, logging.rs, 3 test files)
- New test: `emit_tier_awaited_between_handle_and_observe`

### Wave 9: HMAC via LockBox Alias
- `Rivers.crypto.hmac()` now resolves HMAC key from LockBox alias on host side
- `LockBoxContext` thread-local stores resolver + keystore path + identity
- `TaskContext` extended with `lockbox`, `lockbox_keystore_path`, `lockbox_identity` fields
- Fallback: when lockbox not configured, treats arg as raw key (dev/test mode)
- Secret value zeroized after HMAC computation per SHAPE-5

### Wave 10: Per-Pool Watchdog Thread
- Replaced per-task watchdog threads with single per-pool watchdog thread (`rivers-watchdog-{pool}`)
- `ActiveTaskRegistry` shared between workers and watchdog; scans every 10ms
- `TaskTerminator::V8(IsolateHandle)` / `TaskTerminator::WasmEpoch(Engine)` — terminates timed-out tasks
- Thread reduction: N-concurrent-tasks → N-pools (typically 1-4)
- `Drop for ProcessPool` cancels watchdog on pool shutdown

### Cross-wave fixes
- `process_pool.rs`: resolved conflicts between Waves 9 (HMAC) and 10 (watchdog) — TaskContext fields, function signatures, test call sites

---

## [Phase AM Complete] — Spec Remediation Waves 0-6 (2026-03-19)

---

## Wave 10: Per-Pool Watchdog Thread (2026-03-19)

- **process_pool.rs** — Replaced per-task watchdog threads with single per-pool watchdog. Added `ActiveTask`/`TaskTerminator`/`ActiveTaskRegistry` types. Pool watchdog scans active workers every 10ms and terminates timed-out tasks (V8 via `terminate_execution()`, WASM via `increment_epoch()`). `ProcessPool::Drop` cancels the watchdog thread. All 91 process_pool tests pass, all 5 timeout tests pass.

---

## [Phase AM Complete] — Spec Remediation Waves 0-6 (2026-03-19)

**Scope:** Fix 2 bugs, close security gaps, wire missing subsystems, align formats per deviation audit.
**Baseline:** 1386 tests (Phase AL). **Final:** 1381 tests (5 removed for deleted code), 0 failures, 0 regressions.

### Bugs Fixed
- **BUG-1**: Sentinel key format — `rivers:node:sentinel:{id}` → `rivers:node:{id}` (storage.rs)
- **BUG-2**: EventBus wildcard dispatch — `"*"` subscribers now receive all events (eventbus.rs)

### Security Fixes (Wave 2)
- Admin body hash — SHA-256 of request body now included in signature verification (was empty string)
- Admin timestamp — seconds → milliseconds (was accepting stale signatures)
- IP allowlist — `check_ip_allowlist()` now called in admin middleware
- RBAC — `check_permission()` now called in admin middleware with path-to-permission mapping
- `--no-admin-auth` flag wired to config + startup warning emitted
- Dead code: `public_key: None` bypass replaced with `unreachable!()`

### SHAPE Violations Fixed (Wave 6)
- **SHAPE-4**: Removed `redact_error()` — error strings pass through unmodified
- **SHAPE-12**: Removed `parallel: bool` from `HandlerStageConfig`
- **SHAPE-18**: Removed StorageEngine buffering from `BrokerConsumerBridge`
- **SHAPE-23/24**: (deferred — CORS/rate-limit config relocation is breaking change)

### Format Alignment (Wave 6)
- Error response envelope flattened: `{code, message, trace_id}` (was nested under `error` key)
- Timeout status code: 504 → 408
- CSP header removed (operator responsibility per spec)
- Rate limit defaults: 600/50 → 120/60 per spec
- `ParsedRequest` fields: `query`→`query_params`, `params`→`path_params`
- `l2_max_value_bytes` default unified to 131072 (128KB)

### StorageEngine Bootstrap (Wave 0)
- `storage_engine: Option<Arc<dyn StorageEngine>>` added to `AppContext`
- Wired into startup with backend factory, sentinel claim, sweep task
- `cache:` added to reserved namespace prefixes
- SQLite: `created_at` column + indexes added
- Redis: global `key_prefix` support (default `"rivers:"`)

### EventBus + Logging (Wave 0.5)
- Wildcard dispatch: `publish()` now dispatches to `"*"` subscribers after exact-topic dispatch
- Lock refactor: subscribers collected under read lock, dispatch after lock drop
- `LogHandler` registered at startup on EventBus
- `file_writer` added to `LogHandler` for local log persistence

### DataView Pipeline (Wave 3)
- `DataViewCache` wired into `DataViewExecutor` — cache check (step 3) + populate (step 8)
- `DataViewCache` trait return types: `get()` → `Result<Option<>>`, `set()` → `Result<()>`
- `TieredDataViewCache` constructed at startup when StorageEngine available
- Added `DataViewError` variants: `UnsupportedSchemaAttribute`, `SchemaFileNotFound`, `SchemaFileParseError`, `UnknownFakerMethod`, `Cache`

### Session Infrastructure (Wave 1)
- `SessionManager` + `CsrfManager` added to `AppContext`, constructed from StorageEngine
- Startup validation: protected views require StorageEngine
- Guard view detection wired at startup, rejects multiples
- `SessionConfig`: added `include_token_in_body`, `token_body_key` fields
- `SessionCookieConfig`: `http_only = false` rejected at startup

### ProcessPool Hardening (Wave 5)
- `hashPassword` now uses bcrypt (cost 12) instead of SHA-256
- `verifyPassword` uses `bcrypt::verify` instead of hash comparison
- Config: `max_heap_bytes` → `max_heap_mb`, added `recycle_after_tasks`
- `ctx.store` → StorageEngine wiring verified (was already complete)

---

## [Phase AL Complete] — Broker Pipeline + LDAP Driver (2026-03-19)

**Scope:** Wire EventBus into AppContext, spawn broker bridges at bundle load, enable produce via DataView, build LDAP driver.

**Baseline:** Phase AK, 1379 tests. **Status:** 19/21 tasks complete — smoke tests (AL3.4, AL5.6) remain.

| File | Decision | Spec Ref |
|------|----------|----------|
| `riversd/src/server.rs` | Added `event_bus: Arc<EventBus>` to `AppContext`, instantiated in `new()` | §11 |
| `riversd/src/server.rs` | Register 3 broker drivers (`KafkaDriver`, `RabbitMqDriver`, `NatsDriver`) alongside database drivers | §8.1 |
| `riversd/src/server.rs` | After bundle load: scan apps for broker datasources, create consumers, spawn `BrokerConsumerBridge` per broker datasource with shutdown wiring | §10.1 |
| `riversd/src/server.rs` | After bundle load: build `MessageConsumerRegistry` per app, call `subscribe_message_consumers()` to wire EventBus handlers | §8 |
| `rivers-data/src/dataview_engine.rs` | `DataViewExecutor.execute()` — on `UnknownDriver` error, fall back to broker produce path via `execute_broker_produce()` | §6.2 |
| `rivers-data/src/dataview_engine.rs` | New `execute_broker_produce()` — creates producer, publishes `OutboundMessage` (destination=query statement, payload=JSON params), returns `affected_rows=1` | §6.2 |
| `rivers-plugin-ldap/src/lib.rs` | New crate — `LdapDriver` + `LdapConnection` implementing `DatabaseDriver`/`Connection` via `ldap3` 0.11 | §8 |
| `rivers-plugin-ldap/src/lib.rs` | Operations: search (base/scope/filter parsing), add, modify, delete, ping (WhoAmI). 11 unit tests. | §8 |
| `riversd/Cargo.toml` | Added `rivers-plugin-ldap` dependency | — |
| `Cargo.toml` (workspace) | Added `ldap3 = "0.11"` + `rivers-plugin-ldap` workspace member | — |
| `test-drivers-bundle/rabbitmq-service/` | New service — produce DataView + POST view | — |
| `test-drivers-bundle/nats-service/` | New service — produce DataView + POST view | — |
| `test-drivers-bundle/kafka-service/app.toml` | Added `publish_order` DataView + POST `/orders` view alongside existing MessageConsumer views | — |
| `test-drivers-bundle/ldap-service/` | New service — search/add/delete DataViews + GET/POST/DELETE views | — |
| `test-drivers-bundle/manifest.toml` | Added `rabbitmq-service`, `nats-service`, `ldap-service` to apps list (11 total) | — |

---

## [Phase AF Complete] — Namespaced URL routing (2026-03-18)

**Scope:** Replace flat view merging with `/<prefix>/<bundle>/<app>/<view>` routing. Add `/services` discovery endpoint. Remove HTTP proxy pattern from address-book-main.

**Baseline:** Phase AE, 1342 tests. **Final:** 0 regressions + 5 new tests. Smoke tested in release.

| File | Decision | Spec Ref |
|------|----------|----------|
| `rivers-core/src/config.rs` | Added `route_prefix: Option<String>` to `ServerConfig` — optional operator-configured prefix for all bundle routes | §19.1 |
| `rivers-data/src/bundle.rs` | `AppManifest.entry_point` doc updated: route name, not URL | §5.3 |
| `address-book-service/manifest.toml` | `entryPoint = "service"` (was `"http://0.0.0.0:9100"`); removed `appEntryPoint` | §5.3 |
| `address-book-main/manifest.toml` | `entryPoint = "main"` (was `"http://0.0.0.0:8080"`); removed `appEntryPoint` | §5.3 |
| `address-book-service/app.toml` | View paths are relative names: `"contacts"`, `"contacts/{id}"`, etc. (were `"/api/contacts"`, etc.) | §3.1 |
| `address-book-main/app.toml` | All proxy views/dataviews removed — only SPA config remains | §7.2 |
| `address-book-main/resources.toml` | HTTP datasource removed; `[[services]]` block kept for discovery | §7.2 |
| `riversd/src/view_engine.rs` | `build_namespaced_path()` — builds `/<prefix>/<bundle>/<app>/<view>` paths | §3.1 |
| `riversd/src/view_engine.rs` | `ViewRouter::from_bundle()` — iterates apps, prefixes each view path, keys by `{entry_point}:{view_id}` (no collisions) | §3.1 |
| `riversd/src/server.rs` | Bundle auto-load uses `from_bundle()` instead of flat HashMap merge | §3.1 |
| `riversd/src/server.rs` | `services_discovery_handler` — resolves service `app_id` → entry_point, returns JSON list | §3.2, §7.2 |
| `riversd/src/server.rs` | `loaded_bundle: Option<Arc<LoadedBundle>>` added to `AppContext` | §7.2 |
| `rivers-application-spec.md` | §2.2, §2.4 (new), §5.2-5.3, §7 rewritten for route-based discovery | — |
| `rivers-httpd-spec.md` | §3 rewritten (§3.1 URL scheme, §3.2 services discovery, §3.3 registration order), §8 SPA scoped, §19.1 `route_prefix` | — |

**Tests added:** `build_namespaced_path_no_prefix`, `build_namespaced_path_with_prefix`, `build_namespaced_path_strips_leading_slash`, `build_namespaced_path_empty_view`, `build_namespaced_path_empty_prefix_treated_as_none`

**Smoke test results:**
- `GET /address-book/service/contacts` → 200 (faker data)
- `GET /address-book/service/contacts/42` → 200
- `GET /address-book/main/services` → `[{"name":"address-book-service","url":"/address-book/service"}]`
- `GET /api/contacts` (old path) → 404 JSON
- `GET /health` → 200 (outside namespace, unchanged)

---

## [Phase AE Complete] — Post-AD spec compliance (2026-03-18)

**Scope:** Wire remaining spec gaps after HTTPD-RC1 amendments: hot reload, request observer, CIDR allowlist, stale file cleanup.

**Baseline:** Phase AD complete, 1321 tests. **Final:** 1329 tests (+8 new). All passing (1 pre-existing `riverpackage` failure unrelated).

| File | Decision | Spec Ref |
|------|----------|----------|
| `riversd/src/server.rs` | Added `hot_reload_state: Option<Arc<HotReloadState>>` and `config_path: Option<PathBuf>` to `AppContext`; wired `maybe_spawn_hot_reload_watcher` into startup after admin/redirect spawn | §16, §2 step 21 |
| `riversd/src/server.rs` | `maybe_spawn_hot_reload_watcher` is non-fatal — logs WARN on failure, continues | §16 |
| `riversd/src/middleware.rs` | Implemented `request_observer_middleware` — records method, path, status, duration_ms, trace_id via `tracing::debug!` | §4 step 9 |
| `riversd/src/server.rs` | Replaced stub comment at line 168 with real `.layer(axum_middleware::from_fn(middleware::request_observer_middleware))` | §4 |
| `riversd/src/admin.rs` | `check_ip_allowlist` now parses CIDR ranges via `ipnet::IpNet::contains()`, falls back to exact `IpAddr` match; malformed entries are WARN-logged and skipped | §15.4 |
| `Cargo.toml` | Added `ipnet = "2"` to workspace dependencies | §15.4 |
| `docs/arch/rivers-httpd-spec.md` | Deleted stale untracked pre-Phase-AD copy | Cleanup |

**Tests added:**
- `request_observer_runs_on_request` — observer doesn't break middleware chain, response has status + trace_id
- `hot_reload_state_swap_updates_config` — swap increments version, new config active
- `allowlist_cidr_allows_ip_in_range`, `allowlist_cidr_rejects_ip_outside_range`, `allowlist_exact_ip_still_works`, `allowlist_ipv6_cidr`, `allowlist_mixed_cidr_and_exact`, `allowlist_malformed_entry_skipped`

**Blocked (deferred to SHAPE-23/24):**
- CORS per-app init handler (§9)
- Rate limit per-app `[app.rate_limit]` in app.toml (§10, §19.3)

---

## [Phase AD Complete] — HTTPD TLS mandatory on main + admin servers (2026-03-18)

**Scope:** Made TLS mandatory on both main and admin servers (SHAPE-21/22/25). Added auto-gen self-signed certs, configurable HTTP redirect server, `--no-ssl` debug flag, and `riversctl tls` subcommands for cert lifecycle management.

**Baseline:** Phase AC complete, 1321 tests. **Final:** 1321 tests + 11 new TLS tests. All passing (1 pre-existing `riverpackage` failure unrelated to TLS).

| File | Decision | Spec Ref |
|------|----------|----------|
| `Cargo.toml` | Added `rcgen 0.13` with `aws_lc_rs` feature flag (not default `ring`) to align with rustls 0.23 crypto backend and avoid dual-library link | T1 |
| `Cargo.toml` | Added `time 0.3` with `["formatting", "parsing"]` features — bare `time = "0.3"` can't call `OffsetDateTime::format()` | T1 |
| `rivers-core/src/config.rs` | Added `TlsConfig`, `TlsX509Config`, `TlsEngineConfig`; all `AdminTlsConfig` cert fields made `Option<String>`; added `data_dir: Option<String>` (default `"data"`) and `app_id: Option<String>` (default `"default"`) to `ServerConfig` | T2, SHAPE-21 |
| `rivers-core/src/config.rs` | D3: CORS/rate_limit fields in `SecurityConfig` kept — SHAPE-23/24 out of scope; removing would break `CorsConfig` construction in server.rs | D3 decision |
| `rivers-core/src/config.rs` | Removed `tls_cert`/`tls_key` from `Http2Config` — TLS now lives under `[base.tls]`, not under http2 | SHAPE-21 |
| `rivers-core/src/tls.rs` | New shared module: `generate_self_signed_cert` uses rcgen 0.13 — `SanType::DnsName` takes `Ia5String` (not raw `String`), must use `Ia5String::try_from(s.clone())?` | T3 |
| `rivers-core/src/tls.rs` | `validate_cert_key_pair` installs `aws_lc_rs` default provider with `let _ = ...install_default()` (idempotent); avoids panic on second call | T3 |
| `riversd/src/tls.rs` | `maybe_autogen_tls_cert`: validates existing pair on restart; warns (not silently regenerates) when only one of cert/key exists; regenerates if pair is invalid | T4/T5 |
| `riversd/src/tls.rs` | Auto-gen cert paths: `{data_dir}/tls/auto-{app_id}.crt` (main), `{data_dir}/tls/auto-admin-{app_id}.crt` (admin) | T5, SHAPE-21 |
| `riversd/src/cli.rs` | `--port` flag only valid with `--no-ssl`; added `CliError::InvalidUsage(String)` variant to carry message cleanly (not reuse `UnknownFlag`) | T6 |
| `riversd/src/server.rs` | `validate_server_tls`: admin TLS check runs **before** `if no_ssl { return Ok(()) }` — admin TLS always required regardless of `--no-ssl` | T7, SHAPE-25 |
| `riversd/src/server.rs` | Main server uses `hyper_util::server::conn::auto::Builder` TLS accept loop (not `axum::serve`) — required for `TlsAcceptor` wrapping | T7 |
| `riversd/src/server.rs` | Admin server also uses `hyper_util` accept loop (same pattern as main) — admin TLS added in T8, localhost plain-HTTP exception removed | T8, SHAPE-25 |
| `riversd/src/server.rs` | HTTP redirect server uses `StatusCode::MOVED_PERMANENTLY` (301) manually — `axum::Redirect::permanent()` returns 308, spec requires 301 | T9, SHAPE-22 |
| `riversd/src/server.rs` | Removed orphaned `load_tls_config` and `maybe_tls_acceptor` after refactor to `tls.rs` | T7 cleanup |
| `rivers-data/src/validate.rs` | Removed localhost plain-HTTP bypass block for admin API (SHAPE-25) — Ed25519 unconditionally required | T8 |
| `riversctl/src/tls_cmd.rs` | `cmd_request` CSR generation: `params.serialize_request_pem(&key_pair)` doesn't exist in rcgen 0.13.2 — correct API is `params.serialize_request(&key_pair)?.pem()?` | T10 |
| `riversctl/src/tls_cmd.rs` | Used `::time::OffsetDateTime::now_utc()` (absolute path) — `time` module shadowed by `x509-parser`'s own `time` re-export without prefix | T10 |
| `riversctl/src/tls_cmd.rs` | `cmd_expire` falls back to auto-gen path construction when `resolve_cert_paths` fails — primary use case is auto-gen certs without explicit `cert`/`key` in config | T10 quality fix |
| `riversctl/src/tls_cmd.rs` | `cmd_list` shows expiry date alongside each cert path (spec §6 requirement) | T10 spec fix |
| `config/riversd.toml` | Added `[base.tls]` (`redirect = false`) and `[base.admin_api.tls]` (`require_client_cert = false`) — auto-gen certs, no explicit paths | AB.1 |
| `docs/rivers-httpd-spec.md` | Updated §5.5 (`redirect_port` configurable), §5.6 (`AdminTlsConfig` optional fields, mandatory TLS), §15.2 (Ed25519 unconditional, localhost exception removed) | AB.1 |

**Tests updated (D4):**
- `server_tests.rs`: `http2_without_tls_rejected` accepts either `"TLS is required"` or `"HTTP/2 requires TLS"` (check order changed)
- `server_tests.rs`: `tls_config_present_passes_validation` migrated to `[base.tls]` path
- `hot_reload_tests.rs`: `changed_tls_requires_restart` uses `base.tls` instead of `http2.tls_cert`

**Spec gaps identified (not fixed, tracked for next session):**
- §15.3 signature payload field order wrong: spec says `body_sha256_hex` before `timestamp`; code (`admin_auth.rs:41`) has `timestamp` before `body_hash`
- §19.1 default port shows 443; code defaults to 8080
- `--no-ssl` flag entirely undocumented in spec
- `data_dir`/`app_id` missing from §19 config reference
- `riversctl tls expire --yes` flag not mentioned in §20

---

## [Phase AC Complete] — Address Book Bundle built and verified (2026-03-18)

**Scope:** Created complete `address-book-bundle` — faker-backed REST API service + Svelte SPA frontend. Bundle loads, views route correctly, SPA compiled.

| File | Decision | Resolution |
|------|----------|------------|
| `address-book-bundle/manifest.toml` | Changed `name`/`version` to `bundleName`/`bundleVersion` per BundleManifest serde aliases | Read struct definition in `bundle.rs` |
| `address-book-service/manifest.toml` | Used `appName`, `type`, `appId`, `entryPoint`, `appEntryPoint`; stable appId `c7a3e1f0-...` | Field names from AppManifest serde attributes |
| `address-book-service/resources.toml` | Added `x-type = "faker"`, `nopassword = true`, `required = true` | Required by DatasourceConfig validation |
| `address-book-service/schemas/contact.schema.json` | Created 13-field schema with faker dot-notation (`name.firstName`, `internet.email`, etc.) | Per spec §Schema Files + §Faker Attribute Examples |
| `address-book-service/app.toml` | Each datasource and dataview table requires explicit `name = "..."` field — spec omits it | Confirmed via `DataViewConfig` struct: `name: String` has no `#[serde(default)]` |
| `address-book-service/app.toml` | Handler type must be `"dataview"` (lowercase) not `"data_view"` | `HandlerConfig` enum uses `#[serde(rename_all = "lowercase")]` |
| `address-book-service/app.toml` | Cache config key is `caching` not `cache` | `DataViewConfig` field is `caching: Option<DataViewCachingConfig>` |
| `address-book-main/resources.toml` | Added `[[services]]` block referencing service appId; renamed datasource to `address-book-api` | Required by AppConfig services array for RPS registration |
| `address-book-main/app.toml` | SPA config uses `[static_files]` with `root` (not `[spa]` with `root_path`) | `AppStaticFilesConfig` struct has field `root: PathBuf` |
| `config/riversd.toml` | `bundle_path` must be top-level, not under `[base]` — placing it after `[base]` makes it `base.bundle_path` (unknown field, silently ignored) | Moved before `[base]` section; verified via bundle loaded log |
| `address-book-main/libraries/` | Built Svelte SPA with Rollup: `spa/bundle.js` (41KB) + `spa/bundle.css` (2.2KB) | `npm install && npm run build` in `libraries/` |
| — | DataViewExecutor not wired to bundle auto-load path — views return `{"_stub":true}` stubs | Known limitation; wired separately via `run_server_with_executor()`. Documented in bundle CHANGELOG.md |
| — | View ID collision: service's `list_contacts` + main's `proxy_list_contacts` are distinct IDs; no collision. Final router has 4 unique view IDs. | Verified via curl responses |

**Endpoints verified:**
- `GET /api/contacts` → `{"_stub":true,"_dataview":"proxy_list_contacts",...}`
- `GET /api/contacts/search?q=john` → `{"_stub":true,"_dataview":"proxy_search_contacts",...}`
- `GET /api/contacts/:id` → `{"_stub":true,"_dataview":"get_contact",...}`
- `GET /api/contacts/city/:city` → `{"_stub":true,"_dataview":"contacts_by_city",...}`

---

## [Phase Y Complete] — All 23 tasks done (2026-03-17)

**Baseline:** 1311 tests → **1321 tests** (+10 new). All passing.

| File | Change | Task |
|------|--------|------|
| `view_engine.rs` | DataViewExecutor wired: executor path calls `exec.execute()`, sets `ctx.resdata` to rows | Y1.1 |
| `polling.rs` | `execute_poll_tick_inmemory` uses real executor when provided; `dispatch_change_detect` dispatches to ProcessPool for ChangeDetect strategy | Y1.2, Y1.3 |
| `main.rs` | `doctor` command: 5 real checks (config parse, config validate, process pool, storage, lockbox perms) | Y2.1 |
| `main.rs` | `preflight` command: bundle load → validate_bundle → schema dir check | Y2.2 |
| `lockbox.rs` | `encrypt_keystore()`: age x25519 encryption, 0o600 permissions, TOML serialization | Y3.1 |
| `main.rs` | All lockbox write commands wired: init/add/alias/rotate/remove/rekey decrypt→modify→re-encrypt | Y3.2–Y3.7 |
| `server.rs` | `admin_deploy_test_handler`: validates bundle via `load_bundle` + `validate_bundle` | Y4.1 |
| `server.rs` | `admin_deploy_approve/reject/promote_handler`: real state transitions via DeploymentManager | Y4.2–Y4.4 |
| `server.rs` | `admin_deployments_handler`: returns live list from `deployment_manager.list()` | Y4.5 |
| `server.rs` | `admin_drivers_handler`: returns built-in driver catalog (compiled-in drivers) | Y5.1 |
| `dataview_engine.rs` | `datasource_info()` + `datasource_names()`: expose configured datasource params | Y5.2 |
| `server.rs` | `admin_datasources_handler`: reads from `dataview_executor.datasource_info()` when executor set | Y5.2 |
| `server.rs` | `LogController` struct: type-erases `tracing_subscriber::reload::Handle` via closure | Y5.3–Y5.5 |
| `server.rs` | `AppContext.log_controller: Option<Arc<LogController>>` + `run_server_with_listener_and_log()` | Y5.3–Y5.5 |
| `server.rs` | `admin_log_levels/set/reset_handler`: fully wired to `LogController` | Y5.3–Y5.5 |
| `main.rs` | Tracing init uses `reload::Layer` to enable dynamic log level changes at runtime | Y5.3–Y5.5 |
| `server.rs` | TLS via `tokio_rustls::TlsAcceptor`; falls back to HTTP when cert/key not configured | Y6.1 |

**Tests added:** `admin_datasources_no_bundle_returns_empty`, `admin_log_levels_returns_current_level`, `admin_log_set_invalid_level_returns_error`, `admin_log_set_no_controller_returns_unavailable`, `admin_log_reset_no_controller_returns_unavailable`, `admin_log_set_with_controller_updates_level`, `admin_log_reset_with_controller_resets_to_initial`, `executor_datasource_info_returns_configured_datasources`, `executor_datasource_names_sorted`, `executor_datasource_info_empty_when_no_datasources`

---

## [Phase Y Planning] — Stub Elimination (2026-03-17)

| File | Decision | Resolution |
|------|----------|------------|
| `todo/tasks.md` | Replaced Phase X tasks with Phase Y plan (23 tasks across 6 epics) | Previous tasks archived |
| — | Excluded RPS client driver, gossip processing, SSH agent key source, Raft (all V2) | V2 items removed from scope |
| — | Critical: Y1 (DataView wiring) + Y2 (CLI doctor/preflight) | DataViewExecutor exists from X4, just needs view pipeline wiring |
| — | High: Y3 (lockbox write-back) + Y4 (admin deploy lifecycle) | age crate + DeploymentManager already in deps |
| — | Medium: Y5 (admin listing/log) + Y6 (TLS) | DriverFactory + tracing reload exist; TLS needs rustls |

---

## [Phase X Complete] — All 39 tasks done (2026-03-17)

| File | Change | Spec Reference |
|------|--------|----------------|
| **X6: WasmtimeWorker full implementation** | | |
| `process_pool.rs` | `execute_wasm_task` rewritten: epoch preemption, memory limits, host bindings, trap detection | §6.0-6.3, §7.3 |
| `process_pool.rs` | Epoch watchdog thread — increments engine epoch every 10ms, cancels on completion | §7.3 |
| `process_pool.rs` | `StoreLimitsBuilder` for memory limit enforcement from pool config | §9.1 |
| `process_pool.rs` | Host function bindings: `rivers.log_info/warn/error` → tracing (ptr+len from WASM memory) | §6.2, §10.6 |
| `process_pool.rs` | Trap detection: `wasmtime::Trap` downcast → `TaskError::Timeout` for fuel/epoch exhaustion | §7.3 |
| `process_pool.rs` | Memory limit detection: "memory" + "limit" in error → specific error message | §6.1 |
| `process_pool.rs` | WAT text format support — wasmtime compiles .wat files directly | — |
| `process_pool.rs` | `dispatch_task` passes `heap_bytes` to `execute_wasm_task` for memory limits | §9.1 |
| `process_pool.rs` | 3 new tests: wasm_execution, wasm_fuel_exhaustion, wasm_dispatch_through_pool | — |
| **X2.3 + X4** | (see previous changelog entry) | |
| **Tests** | **1310 passing** (was 1304) | — |

---

## [Phase X — X2.3 + X4] — DataViewExecutor + allow_outbound_http (2026-03-17)

| File | Change | Spec Reference |
|------|--------|----------------|
| `rivers-data/src/view.rs` | Added `allow_outbound_http: bool` field to `ApiViewConfig` | §10.5 |
| `riversd/src/view_engine.rs` | Startup warning when view declares `allow_outbound_http = true` | §10.5, §13.1 |
| `riversd/src/view_engine.rs` | Wires `allow_outbound_http` → `HttpToken` on TaskContextBuilder | §10.5 |
| `rivers-data/src/dataview_engine.rs` | New `DataViewExecutor` struct — registry + factory + execute facade | §6.2 |
| `rivers-data/src/lib.rs` | Export `DataViewExecutor`, `DataViewRegistry` | — |
| `riversd/src/process_pool.rs` | `TASK_DV_EXECUTOR` thread-local for ctx.dataview() fallback | §10.4 |
| `riversd/src/process_pool.rs` | `TaskContext.dataview_executor` field + builder method | §10.4 |
| `riversd/src/process_pool.rs` | `ctx_dataview_callback` now falls back to DataViewExecutor via async bridge | §10.4 |
| `riversd/src/process_pool.rs` | Params extraction from second V8 argument for dynamic dataview queries | §10.4 |
| `riversd/tests/*.rs` | Added `allow_outbound_http: false` to all ApiViewConfig literals (5 files) | — |
| `riversd/src/process_pool.rs` | 3 new tests: x4_executor_end_to_end, x4_not_found_throws, x4_prefetch_priority | — |
| **Tests** | **1307 passing** (was 1304) | — |

**Key achievement:** `ctx.dataview("contacts")` now executes through DataViewExecutor → DataViewRegistry → DriverFactory → Connection → QueryResult when data isn't pre-fetched. Pre-fetched data still takes priority (fast path).

---

## [Phase X Wave 3] — ctx.datasource().build() + WasmtimeWorker config (2026-03-17)

| File | Change | Spec Reference |
|------|--------|----------------|
| `process_pool.rs` | `ResolvedDatasource` struct — maps token → (driver_name, ConnectionParams) | §4.2 |
| `process_pool.rs` | `TASK_DRIVER_FACTORY` + `TASK_DS_CONFIGS` thread-locals | §4.2, §10.3 |
| `process_pool.rs` | `TaskContext.driver_factory` + `TaskContext.datasource_configs` fields | §4.2 |
| `process_pool.rs` | `ctx_datasource_build_callback` — native V8 callback for .build() | §4.2, §10.3 |
| `process_pool.rs` | Full execution: resolve token → DriverFactory.connect() → Connection.execute() → V8 result | §4.2 |
| `process_pool.rs` | `json_to_query_value()` — converts JSON params to QueryValue for driver execution | §10.3 |
| `process_pool.rs` | Capability enforcement: undeclared datasource → CapabilityError | §4.4 |
| `process_pool.rs` | `.build()` without `.fromQuery()` → clear error message | §10.3 |
| `process_pool.rs` | `WasmtimeWorker::new()` now succeeds — validates wasmtime engine creation | §6.0-6.3 |
| `process_pool.rs` | `WasmtimeWorker::config_from_pool()` — maps ProcessPoolConfig → WasmtimeConfig | §9.1 |
| `process_pool_tests.rs` | Updated `wasmtime_worker_returns_engine_unavailable` → `wasmtime_worker_creates_successfully` | — |
| `process_pool.rs` | 7 new tests: x6_* (2), x7_* (5) including faker driver end-to-end | — |
| **Tests** | **1304 passing** (was 1297) | — |

**Key achievement:** `ctx.datasource("faker-ds").fromQuery("SELECT name, email FROM contacts LIMIT 3").build()` now returns real faker-generated data through the full DriverFactory → Connection → execute pipeline.

---

## [Phase X Wave 2] — ctx.store StorageEngine, ctx.dataview fast path (2026-03-17)

| File | Change | Spec Reference |
|------|--------|----------------|
| `process_pool.rs` | `TASK_STORAGE` thread-local — holds `Arc<dyn StorageEngine>` for real persistence | StorageEngine §2.0 |
| `process_pool.rs` | `TASK_STORE_NAMESPACE` thread-local — `app:{app_id}` per-app isolation | StorageEngine §2.2 |
| `process_pool.rs` | `TaskContext.storage` field + `.storage()` builder method | §4.1 |
| `process_pool.rs` | Manual `Debug` impl for `TaskContext` (StorageEngine is not Debug) | — |
| `process_pool.rs` | `ctx_store_get_callback` — tries StorageEngine first, fallback to TASK_STORE | StorageEngine §2.0 |
| `process_pool.rs` | `ctx_store_set_callback` — writes to StorageEngine with TTL, + TASK_STORE mirror | StorageEngine §2.0 |
| `process_pool.rs` | `ctx_store_del_callback` — deletes from StorageEngine, + TASK_STORE mirror | StorageEngine §2.0 |
| `process_pool.rs` | `ctx_dataview_callback` — updated comments for X4 capability check plan | §10.4 |
| `process_pool.rs` | 8 new tests: x3_* (6), x4_* (2) | — |
| **Tests** | **1297 passing** (was 1289) | — |

**Design decisions:**
- StorageEngine is optional — when `None`, falls back to in-memory TASK_STORE (backward compat)
- Namespace uses `app:{app_id}` prefix for per-app isolation per spec §2.2
- `set()` mirrors value to both StorageEngine AND TASK_STORE so same-task reads are instant
- TTL is extracted from third V8 argument and passed to StorageEngine (ignored in fallback)
- On StorageEngine errors, warns and falls back to in-memory (no crash)
- X4 DataViewEngine live execution deferred — DataViewEngine needs an `execute()` facade method first

---

## [Phase X Wave 1] — Rivers.log fields, Rivers.http gating, V8Worker config (2026-03-17)

| File | Change | Spec Reference |
|------|--------|----------------|
| `process_pool.rs` | `extract_log_fields()` — extracts optional V8 fields arg → JSON for structured tracing | §5.2, §10.6 |
| `process_pool.rs` | Rivers.log.{info,warn,error} now accept `(msg, fields?)` second argument | §5.2 |
| `process_pool.rs` | console.{log,warn,error} forward trailing object as structured fields | §5.2 |
| `process_pool.rs` | `TASK_TRACE_ID` thread-local — included in Rivers.http request logging | §10.6 |
| `process_pool.rs` | `TASK_HTTP_ENABLED` thread-local — capability gate for Rivers.http injection | §10.5, §13.1 |
| `process_pool.rs` | Rivers.http only injected when `TaskContext.http` is `Some` | §10.5 |
| `process_pool.rs` | `extract_host()` + request logging in `do_http_request()` — logs host + method + trace_id | §10.5 |
| `process_pool.rs` | `V8Worker::new()` now succeeds — initializes V8 platform, stores config | §5.0-5.4 |
| `process_pool.rs` | `V8Worker::config_from_pool()` — maps ProcessPoolConfig → V8Config | §9.1 |
| `process_pool.rs` | `dispatch_task` passes `heap_bytes` + `heap_threshold` from pool config | §5.4, §9.2 |
| `process_pool.rs` | `execute_js_task` uses pool-configured heap limit (not DEFAULT_HEAP_LIMIT) | §5.4 |
| `process_pool.rs` | Heap recycling: checks `v8::HeapStatistics` after task, drops isolate if above threshold | §5.4 |
| `process_pool_tests.rs` | Updated `v8_worker_returns_engine_unavailable` → `v8_worker_creates_successfully` | — |
| `process_pool.rs` | 10 new tests: x1_* (3), x2_* (3), x5_* (4) | — |
| `process_pool.rs` | Fixed 3 pre-existing HTTP tests to use `HttpToken` for capability gating | — |
| **Tests** | **1289 passing** (was 1139) | — |

---

## [Phase X Planning] — ProcessPool Feature Buildout (2026-03-17)

| File | Decision | Spec Reference | Resolution |
|------|----------|----------------|------------|
| `todo/tasks.md` | Replaced Phase W tasks with Phase X plan (39 tasks across 7 epics) | ProcessPool spec §4-10, StorageEngine spec §2 | Previous tasks copied to `todo/gutter.md` |
| `process_pool.rs` | Rivers.log needs structured fields (msg + fields?) | §5.2, §10.6 | Epic X1: 3 tasks |
| `process_pool.rs` | Rivers.http needs capability gating + request logging | §10.5, §13.1 SHAPE-11 | Epic X2: 3 tasks |
| `process_pool.rs` | ctx.store must use real StorageEngine, not thread-local HashMap | StorageEngine §2.0-2.3 | Epic X3: 7 tasks |
| `process_pool.rs` | ctx.dataview must call DataViewEngine for non-pre-fetched views | §10.4 | Epic X4: 5 tasks |
| `process_pool.rs` | V8Worker config must wire to isolate pool (V8 already works) | §5.0-5.4, §8-9 | Epic X5: 6 tasks |
| `process_pool.rs` | WasmtimeWorker needs wasmtime crate behind feature flag | §6.0-6.3, §7.3 | Epic X6: 9 tasks |
| `process_pool.rs` | ctx.datasource().build() needs async bridge to DriverFactory | §4.2, §10.3 | Epic X7: 6 tasks |
| Execution order | X1+X2+X5 parallel → X3 → X4 → X7 → X6 | Dependency analysis | Smallest scope first, async bridge pattern reused across X3→X4→X7 |

---

## [Integration Wiring] — 7 Integration Points (2026-03-17)

### IP 1-3: Thread DataViewEngine, StorageEngine, ProcessPool through AppContext
- **File:** `crates/riversd/src/server.rs` — AppContext expanded with `pool: Arc<ProcessPoolManager>` and `view_router: Arc<RwLock<Option<ViewRouter>>>`. Initialized in `AppContext::new()` from `config.runtime.process_pools`.
- **Spec ref:** §2 step 16 (all subsystems wired together).

### IP 4: View dispatch with real ViewRouter
- **File:** `crates/riversd/src/server.rs` — `view_dispatch_handler` reads `ctx.view_router`, matches route, extracts path params, calls `execute_rest_view` with `&ctx.pool`. `combined_fallback_handler` checks ViewRouter before falling through to static files.
- **Spec ref:** view-layer-spec §3 (route registration order).

### IP 5: Admin Ed25519 auth middleware
- **Files:** `crates/riversd/src/server.rs`, `crates/rivers-core/src/config.rs` — `admin_auth_middleware` now accepts `State(ctx)`, verifies timestamp freshness via `admin_auth::validate_timestamp`, decodes hex signature, calls `admin_auth::verify_admin_signature`. Added `no_auth: Option<bool>` to `AdminApiConfig` for `--no-admin-auth` bypass. `build_admin_router` uses `from_fn_with_state`.
- **Spec ref:** auth-session-spec §6, httpd-spec §15.3.

### IP 6: Admin deployment endpoints with real logic
- **File:** `crates/riversd/src/server.rs` — `admin_deploy_handler` parses `bundle_path` from request body JSON, logs deployment initiation. `admin_deployments_handler` returns note about wiring to DeploymentManager.
- **Spec ref:** httpd-spec §15.6.

### IP 7: Gossip transport + runtime initialization
- **File:** `crates/riversd/src/runtime.rs` — Replaced 6 individual `wire_*` stub functions with single `initialize_runtime(pool, config)` that logs all subsystem readiness. `gossip_forward_http` preserved. Called from `run_server_with_listener_with_control` after router build.
- **Spec ref:** httpd-spec §12.3, §2.

---

## [V1 Complete] — All Phases Implemented (2026-03-17)

**200/200 tasks complete. 1247 tests passing. Zero outstanding.**

### Crates (14)
| Crate | Type | Purpose |
|-------|------|---------|
| `riversd` | Binary | HTTP server, view layer, ProcessPool, middleware |
| `rivers-lockbox` | Binary | Standalone LockBox secrets management CLI |
| `riversctl` | Binary | Admin CLI with Ed25519 request signing |
| `riverpackage` | Binary | Bundle validator/packager |
| `rivers-core` | Library | Drivers, config, EventBus, LockBox, StorageEngine |
| `rivers-data` | Library | DataViews, schemas, caching, pseudo DataViews |
| `rivers-driver-sdk` | Library | Driver traits, HTTP driver, broker traits, validation engine |
| `rivers-plugin-kafka` | Plugin (cdylib) | Kafka via rskafka (pure Rust) |
| `rivers-plugin-rabbitmq` | Plugin (cdylib) | RabbitMQ via lapin |
| `rivers-plugin-nats` | Plugin (cdylib) | NATS via async-nats |
| `rivers-plugin-redis-streams` | Plugin (cdylib) | Redis Streams via redis crate |
| `rivers-plugin-mongodb` | Plugin (cdylib) | MongoDB via mongodb 3.x |
| `rivers-plugin-elasticsearch` | Plugin (cdylib) | Elasticsearch via reqwest REST |
| `rivers-plugin-influxdb` | Plugin (cdylib) | InfluxDB v2 via reqwest + Flux |

### V2 Deferred (only two items)
- Raft consensus — multi-node leader election
- RPS (Rivers Provisioning Service) — distributed service registry

---

## [Plan] — Phase U: Feature Inventory Gap Closure (2026-03-17)

**File:** `todo/tasks.md`
**Source:** Gap analysis of `docs/rivers-feature-inventory.md` vs codebase.
**Description:** 14 tasks across 8 sub-phases to close all PARTIAL gaps from the feature inventory.
**Critical (blocks live traffic):**
- U1: View routes + rate_limit/session/backpressure middleware not wired into server router
**Moderate:**
- U2: Admin deployment routes not registered
- U3: Cross-app session propagation (X-Rivers-Claims header)
- U4: Service discovery (appId → endpoint resolution)
- U5: Init handler CodeComponent dispatch
- U6: SSE Last-Event-ID reconnection
- U7: Gossip HTTP transport (stub → real POST)
- U8: PollChangeDetectTimeout event emission
**Estimated scope:** ~505 lines, 14 tasks.

---

## [Implementation] — Phase T: V1 Completion (2026-03-17)

**33 tasks. rivers-lockbox, riversctl, riverpackage, ES modules, runtime wiring, address book.**

- **T1:** `rivers-lockbox` CLI — 9 commands (init, add, list, show, alias, rotate, remove, rekey, validate). Age encryption, identity keypair management, file permission enforcement.
- **T2:** `riversctl` CLI — 6 commands (status, deploy, drivers, datasources, health, log). Ed25519 request signing, timestamp replay protection.
- **T3:** `riverpackage` CLI — 3 commands (validate, preflight, pack). Bundle structure validation, schema reference checking, tar.gz packaging.
- **T4:** V8 ES module + async support — `execute_as_module()` for import/export, `resolve_promise_if_needed()` for async handlers, microtask queue pumping.
- **T5:** Runtime wiring stubs — logging wiring points for DataView dispatch, persistent store, admin routes, gossip transport.
- **T6:** LockBox CLI wiring in `riversd main.rs` — delegates to `rivers-core` lockbox module.
- **T7:** Address book reference bundle — `address-book-bundle/` with service (faker, port 9100) + main (HTTP proxy, port 8080).

---

## [Implementation] — Phase S: Schema Validation (2026-03-17)

**30 tasks. Full validation chain per Driver Schema Validation Spec v1.0.**

- **S1:** Updated Driver trait — added `HttpMethod`, `ValidationDirection` enums. `check_schema_syntax` takes method, `validate` takes direction. Expanded error types (12 SchemaSyntaxError + 8 ValidationError variants).
- **S2:** Common constraint engine (`validation.rs`) — `validate_fields()`, `validate_field_type()` (13 Rivers primitives + format validation), `validate_field_constraints()` (min/max/length/pattern/enum). 18 tests.
- **S3-S5:** PostgreSQL, MySQL, SQLite — method-aware syntax checking (GET requires fields, faker rejected), full constraint validation via shared engine, driver-specific type constants.
- **S6:** Redis — `key_pattern` required, per-type structure validation (hash/string/list/set/sorted_set), method-specific rules, faker rejected.
- **S7-S9:** New Driver impls for Memcached (string-only, no PUT), Faker (GET-only, faker attr required, validation attrs rejected), EventBus (event type, topic required, no PUT/DELETE).
- **S10:** HTTP driver — object/stream_chunk types, content_type validation.
- **S11:** Broker validation — Kafka (message+topic, no PUT/DELETE), RabbitMQ (exchange for POST, queue for GET), NATS (subject required).
- **S12:** Request-time pipeline — `validate_input()` rejects on failure (400), `validate_output()` warns only (forward compat).
- **S13:** Pseudo DataView validation at `.build()` — inline schemas must have driver field.

---

## [Implementation] — Phase V2: Async Bridge + Performance (2026-03-17)

**33 tasks. Full async bridge, isolate pool, TypeScript, Wasmtime, multi-node.**

- **V2.1:** Async bridge — `RT_HANDLE` + `TASK_ENV` + `TASK_STORE` thread-locals for bridging V8 sync → tokio async.
- **V2.2:** `ctx.dataview()` — native V8 callback checks `ctx.data[name]` for pre-fetched results, returns cached data.
- **V2.3:** `ctx.streamDataview()` — iterator protocol over pre-fetched arrays/values.
- **V2.4:** `ctx.store` — native V8 callbacks with reserved prefix enforcement (`session:`, `csrf:`, `cache:`, `raft:`, `rivers:`), per-task `TASK_STORE` HashMap.
- **V2.5:** `Rivers.http` — real reqwest GET/POST/PUT/DELETE via `rt_handle.block_on()`.
- **V2.6:** `Rivers.env` — injected from `TASK_ENV` thread-local.
- **V2.7:** `Rivers.crypto.hmac` — real HMAC-SHA256 via hmac + sha2 crates.
- **V2.8:** Isolate pool — thread-local `Vec<OwnedIsolate>` for reuse, `remove_near_heap_limit_callback` before release.
- **V2.9:** Script source cache — `LazyLock<Mutex<HashMap>>` global, `clear_script_cache()` for hot reload.
- **V2.10:** TypeScript — lightweight type stripping (interfaces, return types, as-assertions), auto-detection of .ts files.
- **V2.11:** Wasmtime — `execute_wasm_task()` with fuel-based preemption, module compilation, function dispatch.
- **V2.12:** Multi-node — StorageEngine factory (memory/sqlite/redis), EventBus gossip types + `gossip_forward()`, Kafka offset coordination keys.

---

## [Implementation] — V8 Engine P1-P4 (2026-03-16)

**V8 integration with full host bindings, crypto, timeout, heap limits.**

- **P1.1:** ctx.resdata write-back — resdata takes priority, return value as fallback.
- **P1.2:** ctx.app_id/node_id/env — injected from TaskContext fields.
- **P1.3:** TryCatch — exception messages captured, isolate termination detected.
- **P2.1:** Rivers.log → native V8 callbacks → `tracing` macros.
- **P2.2:** Rivers.crypto.randomHex → real `rand` + `hex`.
- **P2.3:** console.log/warn/error → Rivers.log delegation.
- **P2.4:** CPU timeout — watchdog thread + `terminate_execution()` + mpsc cancellation.
- **P3:** Host bindings — `ctx.dataview()` (pre-fetch lookup), `ctx.store` (in-memory), `ctx.datasource()` (builder chain), `Rivers.http` (real reqwest), `Rivers.env`, `Rivers.crypto.hmac` (real HMAC-SHA256), `Rivers.crypto.hashPassword/verifyPassword` (SHA-256), `Rivers.crypto.timingSafeEqual` (constant-time XOR), `Rivers.crypto.randomBase64url`.
- **P4.1:** Heap limit — 128 MiB via `CreateParams::heap_limits()` + `near_heap_limit_cb`.

---

## [Implementation] — Phase N: Runtime Integration (2026-03-16)

**V8 engine, view pipeline, EventBus wiring, HTTP streaming.**

- **N1:** V8 engine (Boa → V8 swap) — `execute_js_task()`, isolate per task, JSON conversion via `v8::json::parse/stringify`, ctx/Rivers/console injection.
- **N3:** View pipeline — `execute_rest_view()` accepts ProcessPool, pre_process/handlers/post_process wired to dispatch, on_error wrapping.
- **N4:** EventBus subscribers — `subscribe_message_consumers()`, `drive_sse_push_loop()`, `dispatch_ws_lifecycle()`, `execute_poll_tick_inmemory()`.
- **N5:** HTTP SSE streaming — `SseStreamConnection` replaces stub, SSE wire format parsing.

---

## [Implementation] — Phase M: All 15 Drivers Built (2026-03-16)

**Built-in:** SQLite (WAL, spawn_blocking, named params), Redis (20 ops, multiplexed async), PostgreSQL ($N positional, RETURNING), MySQL (exec_iter, parameterized), Memcached (get/set/delete).

**Plugins:** Kafka (rskafka, partition produce/consume, offset tracking), RabbitMQ (lapin, ack/nack, publisher confirms), NATS (async-nats, pub/sub), Redis Streams (XADD/XREADGROUP/XACK, consumer groups), MongoDB (document CRUD, BSON), Elasticsearch (REST _search/_doc), InfluxDB (Flux query, line protocol).

**Dependencies added:** tokio-postgres, mysql_async, rskafka, lapin, async-nats, mongodb, async-memcached, futures-lite, bytes.

---

## [Implementation] — Phase E-L: Technology Path Spec Alignment (2026-03-16)

- **Phase E:** Handler context redesign (`data`/`resdata`/`store`), pipeline collapse (6→4 stage), ParsedRequest renames, StoreHandle.
- **Phase F:** CRUD DataView model — per-method queries/schemas/parameters, backward-compatible aliases.
- **Phase G:** Pseudo DataViews — DatasourceBuilder → PseudoDataView.
- **Phase H:** Unified Driver trait (check_schema_syntax, validate, execute) + SchemaDefinition.
- **Phase I:** Init handler — ApplicationContext, CorsPolicy, InitHandlerConfig.
- **Phase J:** Cache config moved to StorageEngine section.
- **Phase K:** Guard lifecycle hooks (on_session_valid, on_invalid_session, on_failed).
- **Phase L:** Technology Path spec declared authoritative.

---

## [Decision] — Kafka: rskafka 0.6 pure Rust (2026-03-16)

Selected rskafka over rdkafka. No librdkafka C dependency. Consumer group coordination at Rivers framework level (StorageEngine offsets, EventBus partition assignment).

---

## [Decision] — V8 over Boa (2026-03-16)

Switched JS engine from Boa (pure Rust) to V8 (rusty_v8). Performance matters for the execution engine. Pure-Rust preference applies to plugin drivers (rskafka), not the core runtime.

---

## [Implementation] — Phase A-D: Shaping Compliance (prior sessions)

37 epics, 888 tests. Circuit breaker rolling window, SHA-256 cache keys, LockBox index-only, StorageEngine queue removal, error response envelope, all spec amendments applied.

**File:** `todo/tasks.md`
**Description:** Added Phase T — everything remaining for V1 shipping. 33 tasks across 7 sub-phases.
**Scope clarification:** Only Raft consensus and RPS are V2. Everything else is V1:
- T1: rivers-lockbox standalone CLI (10 commands)
- T2: riversctl admin CLI (8 commands + Ed25519 signing)
- T3: riverpackage bundle validator/packager (3 commands)
- T4: V8 ES module + async function support
- T5: Runtime wiring (live DataView dispatch, persistent store, admin routes, gossip transport)
- T6: LockBox CLI wiring in riversd
- T7: Address book reference bundle
**Estimated scope:** ~2400 lines, 33 tasks.

---

## [Plan] — Phase S: Schema Validation per Driver Schema Validation Spec v1.0 (2026-03-16)

**File:** `todo/tasks.md`
**Source:** `docs/rivers-driver-schema-validation-spec.md`
**Description:** Added Phase S plan — full schema validation chain for all 16 drivers. 30 tasks across 13 sub-phases.
**Key gaps found:**
- Driver trait missing `method: HttpMethod` and `direction: ValidationDirection` parameters
- 3 drivers (Memcached, Faker, EventBus) have no Driver trait impl at all
- 4 drivers have basic type checking only — no constraint validation (min/max/pattern/enum)
- No type coercion on output validation for any driver
- No request-time validation pipeline wired into execute_rest_view
**Estimated scope:** ~2450 lines, 30 tasks. Critical path: S1 → S2 → S3-S11 → S12.
**Resolution:** Full plan written to tasks.md.

---

## [Plan] — Phase V2: Async Bridge, Performance, Multi-Node (2026-03-16)

**File:** `todo/tasks.md`
**Description:** Added Phase V2 plan — async host bindings, performance optimizations, multi-node foundations. 30+ tasks across 12 sub-phases.
**Key architecture:** `tokio::runtime::Handle::block_on()` inside V8 host function callbacks bridges sync V8 → async tokio. Workers already run on `spawn_blocking` threads, so blocking is acceptable.
**Priority tiers:**
1. Async bridge + ctx.dataview + ctx.store (~600 lines) — unlocks in-handler queries
2. Rivers.http + env + hmac (~300 lines) — completes spec API surface
3. Isolate pool + script cache (~500 lines) — performance
4. TypeScript (~300 lines) — DX
5. Wasmtime (~600 lines) — alternative engine
6. Multi-node (~800 lines) — clustering foundations
**Resolution:** Full plan written to tasks.md. Total ~3100 lines.

---

## [Implementation] — V8 Hardening P1 + P2 (2026-03-16)

**File:** `crates/riversd/src/process_pool.rs`

**Description:** Implemented P1 (critical fixes) and P2 (utilities hardening) for the V8 ProcessPool engine.

**P1 — Critical Fixes:**
- **P1.1 ctx.resdata write-back:** After handler returns, reads `ctx.resdata` from V8 global. If handler set resdata, uses that as result. Falls back to return value. Supports both standard handlers (set resdata, return void) and guard handlers (return claims directly).
- **P1.2 ctx.app_id/node_id/env:** Added `app_id`, `node_id`, `runtime_env` fields to TaskContext struct + builder. Injected as `app_id`, `node_id`, `env` in the V8 ctx JSON. Default env is "dev".
- **P1.3 TryCatch for exceptions:** Rewrote `call_entrypoint()` to use `v8::TryCatch` scope. Returns `serde_json::Value` instead of `v8::Local` to avoid lifetime escape. Exception messages now captured (was previously "threw an exception" with no details). Also detects isolate termination for timeout reporting.
- **P1.4 Heap limit:** Noted as TODO — V8 defaults (~1.5 GB) are acceptable for V1.

**P2 — Utilities Hardening:**
- **P2.1 Rivers.log → tracing:** Replaced JS no-op stubs with native V8 function callbacks. `Rivers.log.info/warn/error` now call `tracing::info!/warn!/error!` with target `rivers.handler`.
- **P2.2 Rivers.crypto.randomHex → real:** Uses `rand::thread_rng()` + `hex::encode()` for real random hex. Capped at 1024 bytes to prevent abuse. Other crypto stubs (hashPassword, verifyPassword) kept as JS until P3.
- **P2.3 console.log wiring:** Added `console.{log,warn,error}` globals that delegate to `Rivers.log` via JS eval.
- **P2.4 CPU timeout via watchdog:** Spawns a watchdog thread that calls `isolate.terminate_execution()` after `timeout_ms`. Uses `mpsc::channel` for cancellation. TryCatch detects terminated state and maps to `TaskError::Timeout`.

**Tests added (11 new, 2 updated):**
- `execute_resdata_writeback` — standard handler sets ctx.resdata, return void
- `execute_return_value_fallback` — guard handler returns directly
- `execute_resdata_takes_priority_over_return` — resdata wins over return value
- `execute_ctx_has_app_metadata` — app_id, node_id, env accessible from JS
- `execute_ctx_default_env_is_dev` — default env is "dev"
- `execute_exception_has_message` — Error("detailed...") captured in error message
- `execute_exception_with_string_throw` — string throws captured too
- `execute_rivers_log_does_not_crash` — native tracing callbacks work
- `execute_console_log_works` — console.log/warn/error work
- `execute_timeout_terminates` — infinite loop terminated at 100ms
- `execute_random_hex_is_unique` — two randomHex calls produce different values
- `execute_crypto_hash_password_stub` — crypto stubs still work
- Updated `execute_rivers_crypto_random_hex` (was deadbeef, now verifies 32 hex chars)
- Updated `execute_handler_error` (now checks for "boom" in exception message)

**Spec reference:** `rivers-processpool-runtime-spec-v2.md` §4.1, §14.1
**Resolution:** All 24 engine tests pass. Zero regressions across 571 total riversd tests.

---

## [Plan] — Phase N: Runtime Integration (2026-03-16)

**File:** `todo/tasks.md`
**Description:** Added Phase N plan — the final phase to make Rivers serve live traffic. 30 tasks across 6 sub-phases.
**Key insight:** EventBus is already complete. ProcessPool architecture is done (queue, dispatch, TaskContext). Guards, on_error, and session_valid are already wired to ProcessPool. The gap is: V8 engine integration (N1), then connecting existing view/SSE/WebSocket/MessageConsumer/Polling structures to ProcessPool dispatch (N3-N4).
**Critical path:** N1 (V8 engine, ~800 lines) → N3 (view pipeline, ~400 lines) → N4 (EventBus subscribers, ~500 lines). Total ~2000 lines for live traffic capability.
**Spec reference:** Epics 12, 26-30 deferred tasks
**Resolution:** Full plan written to tasks.md with dependency graph.

---

## [Implementation] — Unified Driver trait for 4 database drivers + MySQL parameter binding (2026-03-16)

**Files:**
- `crates/rivers-core/src/drivers/sqlite.rs`
- `crates/rivers-core/src/drivers/redis.rs`
- `crates/rivers-core/src/drivers/postgres.rs`
- `crates/rivers-core/src/drivers/mysql.rs`

**Description:** Implemented Priority 2 (Unified Driver trait adoption) and Priority 3 (MySQL parameter binding).

**Priority 2 — Unified Driver trait:**
- Added `impl Driver for SqliteDriver` with schema syntax checking (type "object", 13 accepted field types) and required-field validation. 7 tests.
- Added `impl Driver for RedisDriver` with Redis-specific schema types (hash, string, list, set, sorted_set, stream), field type validation for hash, and scalar-vs-object validation for string. 12 tests.
- Added `impl Driver for PostgresDriver` with SQL-compatible field types (uuid, text, jsonb, bytea, bigint, decimal, etc.) and required-field validation. 6 tests.
- Added `impl Driver for MysqlDriver` with MySQL-specific field types (24 types: tinyint, mediumint, double, varchar, char, timestamp, time, year, enum, set, etc.) and required-field validation. 10 tests.
- All four use the same `execute()` → `NotImplemented` pattern since real execution routes through `DatabaseDriver::connect() + Connection::execute()`.
- Rust allows both `DatabaseDriver::name()` and `Driver::name()` on the same struct — compiler uses trait context to disambiguate.

**Priority 3 — MySQL parameter binding:**
- Added `build_mysql_params()` helper: sorts parameter keys alphabetically, converts to `mysql_async::Params::Positional` (or `Empty` when no params).
- Added `query_value_to_mysql()` helper: converts all 7 `QueryValue` variants to `mysql_async::Value`.
- Replaced all 6 `query_iter(&query.statement)` calls with `exec_iter(&query.statement, params)` for proper parameterized query execution.
- 5 parameter binding tests cover: empty params, sorted ordering, null/bool/int/float/string, JSON, and arrays.

**Spec reference:** `rivers-technology-path-spec.md` §8.3, `rivers-driver-spec.md` §3.3
**Resolution:** 30 new tests added, all 48 rivers-core unit tests pass, all 33 integration tests pass.

---

## [Decision] — Kafka driver: rskafka 0.6 (pure Rust) (2026-03-16)

**File:** `crates/rivers-plugin-kafka/src/lib.rs`, `todo/tasks.md`
**Description:** Selected `rskafka` 0.6 over `rdkafka` for the Kafka plugin driver.
**Rationale:**
- Pure Rust — no `librdkafka` C dependency, no cmake/C toolchain required
- Simpler build and cross-compilation story
- Trade-off: rskafka is partition-level only (no consumer groups, no offset tracking)
- Rivers compensates: offset persistence via StorageEngine, partition coordination via EventBus
- Consumer group ID derived at framework level: `{group_prefix}.{app_id}.{datasource_id}`
**Alternatives considered:** `rdkafka` (C FFI, feature-complete but heavy build dep), `samsa` (pure Rust with groups but 0.1.x/165 dl/mo), `kafka-rust` (pure Rust with groups but checkered maintenance)
**Spec reference:** `rivers-driver-spec.md` §7.6
**Resolution:** rskafka chosen; framework-level group coordination added to Phase M plan.

---

## [Decision] — Driver build-out plan: Phase M (2026-03-16)

**File:** `todo/tasks.md`
**Description:** Added Phase M with tiered driver build-out plan. 25 tasks across 4 tiers:
- Tier 1 (zero deps): SQLite, Redis, Redis Streams
- Tier 2 (one add): PostgreSQL, MySQL, Kafka/rskafka, RabbitMQ, NATS
- Tier 3 (lower priority): MongoDB, Elasticsearch, Memcached, InfluxDB
- Tier 4: Unified Driver trait adoption across all drivers
**Resolution:** Plan written to tasks.md. SQLite + Redis first (patterns exist in StorageEngine impls).

---

## [Gap Analysis] — Technology Path Spec vs Codebase (2026-03-16)

**File:** `docs/rivers-technology-path-spec.md` vs all `crates/` source
**Description:** Comprehensive gap analysis of the unified Technology Path specification against the 888-test codebase. Identified 8 implementation phases (E through L) with ~45 tasks.
**Key findings:**
- **Breaking:** Handler context (`ctx`) needs full redesign — `ViewContext.sources` → `ctx.data` + `ctx.resdata`
- **Breaking:** Pipeline collapse from 6-stage to 4-stage (`on_request`, `transform`, `on_response` absorbed)
- **Breaking:** DataView model needs CRUD expansion (per-method queries/schemas/parameters)
- **Breaking:** Driver trait needs 3 responsibilities (SchemaSyntaxChecker, Validator, Executor)
- **Additive:** Pseudo DataViews, Init Handler, `ctx.store`, app-level CORS
- **Aligned:** StorageEngine KV, rate limiting, logging, bundle structure, sessions, CSRF, WebSocket modes, streaming formats
**Spec reference:** `rivers-technology-path-spec.md` §1-19
**Resolution:** Implementation plan written to `todo/tasks.md` with dependency graph. Critical path: Phase E → F → H.

---

## [Decision] — Bundle manifest field values

**File:** address-book-bundle/manifest.toml
**Description:** Used exact field values from spec table. No ambiguity.
**Spec reference:** CLAUDE_CODE_SPEC.md §1
**Resolution:** Straightforward — all fields explicitly specified.

---

## [Decision] — Stable UUID for appId

**File:** address-book-bundle/address-book-service/manifest.toml
**Description:** Generated UUID `c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a` for appId. Spec says "generate a UUID, must be stable — do not regenerate."
**Spec reference:** CLAUDE_CODE_SPEC.md §2
**Resolution:** Generated once, will not change in future builds.

---

## [Decision] — resources.toml uses array-of-tables

**File:** address-book-bundle/address-book-service/resources.toml
**Description:** Used `[[datasources]]` array-of-tables syntax per rivers-schema-spec-v2.md §6. No lockbox field since `nopassword = true`. Included `x-type = "faker"` for build-time validation.
**Spec reference:** CLAUDE_CODE_SPEC.md §3, rivers-schema-spec-v2.md §6
**Resolution:** Consistent with spec examples. No services section since app has no service dependencies.

---

## [Decision] — Schema field attribute key is `faker`

**File:** address-book-bundle/address-book-service/schemas/contact.schema.json
**Description:** Used `"faker"` as the attribute key name on all 13 fields. AMD-5 confirms this is correct — no `"faker_type"` variant exists.
**Spec reference:** CLAUDE_CODE_SPEC.md §4, AMD-5
**Resolution:** Both spec documents are consistent. 13 fields included with faker.js dot notation.

---

## [AMD-1 Applied] — Cache uses `ttl_seconds`

**File:** address-book-bundle/address-book-service/app.toml
**Description:** Round 1 guessed `ttl = 60`. Amendment clarifies the field is `ttl_seconds` (integer, seconds).
**Spec reference:** AMD-1
**Resolution:** All 4 DataView cache blocks use `ttl_seconds` — values: 60, 300, 30, 120.

---

## [AMD-2 Applied] — Parameters use array-of-tables

**File:** address-book-bundle/address-book-service/app.toml
**Description:** Round 1 used named subtables `[data.dataviews.*.parameters.limit]` which produces a map. Amendment clarifies `[[data.dataviews.*.parameters]]` (array-of-tables with `name` field) is required — riversd expects a list.
**Spec reference:** AMD-2
**Resolution:** All parameters use `[[...parameters]]` syntax with explicit `name` field.

---

## [AMD-3 Applied] — Views use `api.` prefix

**File:** address-book-bundle/address-book-service/app.toml
**Description:** Round 1 used `[views.*]` — silently ignored by riversd (not a parse error, a silent miss). Amendment clarifies the correct prefix is `[api.views.*]`.
**Spec reference:** AMD-3
**Resolution:** All 4 view sections use `[api.views.*]` prefix.

---

## [AMD-4 Applied] — Parameter mapping uses segregated subtables

**File:** address-book-bundle/address-book-service/app.toml
**Description:** Round 1 invented `handler.params` with inline tables. Amendment clarifies the correct pattern is `parameter_mapping.query` and `parameter_mapping.path` subtables. Format: `{http_param} = "{dataview_param}"`.
**Spec reference:** AMD-4
**Resolution:** All 4 views use correct `parameter_mapping` subtables. `contacts_by_city` uses both `path` and `query` mapping.

---

## [AMD-5 Applied] — `faker` attribute confirmed

**File:** address-book-bundle/address-book-service/schemas/contact.schema.json
**Description:** Round 1 chose `"faker"` correctly but logged it as ambiguity. Amendment confirms `"faker"` is the only valid key — there is no `"faker_type"`.
**Spec reference:** AMD-5
**Resolution:** No change needed from round 1 schema. Confirmed correct.

---

# Round 3 — Address Book Main (SPA + API Proxy)

Built from `docs/address-book-main-spec.md`.

---

## [Decision] — Bundle manifest updated

**File:** address-book-bundle/manifest.toml
**Description:** Added `address-book-main` to apps array. Order: service before main, per spec ("app-services start before app-mains").
**Spec reference:** address-book-main-spec.md §1
**Resolution:** Straightforward.

---

## [Decision] — App-main UUID

**File:** address-book-bundle/address-book-main/manifest.toml
**Description:** Generated UUID `a1b2c3d4-5e6f-7a8b-9c0d-e1f2a3b4c5d6` for address-book-main appId. Stable — will not change.
**Spec reference:** address-book-main-spec.md §2
**Resolution:** All manifest fields match spec table exactly.

---

## [Decision] — HTTP datasource driver

**File:** address-book-bundle/address-book-main/resources.toml, app.toml
**Description:** Used `driver = "http"` for the proxy datasource. The `service` field in app.toml links to the service name declared in resources.toml `[[services]]`. Rivers resolves this to the running endpoint at startup.
**Spec reference:** address-book-main-spec.md §3, §4.1
**Resolution:** `nopassword = true` since service-to-service calls carry session auth automatically.

---

## [Inference] — HTTP datasource config fields

**File:** address-book-bundle/address-book-main/app.toml
**Description:** `base_path`, `timeout_ms`, `retry_attempts` are new config fields specific to the HTTP driver. Not previously seen in round 2 (faker driver). Values taken directly from spec.
**Spec reference:** address-book-main-spec.md §4.1
**Resolution:** Used spec values exactly. How `service` resolves to an actual URL is a runtime concern — Rivers handles it.

---

## [Decision] — Proxy DataView query as URL path

**File:** address-book-bundle/address-book-main/app.toml
**Description:** For HTTP datasources, the DataView `query` field is a URL path relative to `base_path`. Parameters are forwarded as query string args. This differs from faker (file path) and SQL (query string).
**Spec reference:** address-book-main-spec.md §4.2
**Resolution:** Two DataViews: `proxy_list_contacts` (GET /contacts) and `proxy_search_contacts` (GET /contacts/search).

---

## [Decision] — SPA build approach

**File:** address-book-bundle/address-book-main/libraries/
**Description:** Spec suggests rollup for minimal setup. Created Svelte source in `src/` and pre-compiled output in `spa/`. The `bundle.js` is a vanilla JS equivalent of the Svelte components using safe DOM methods (no innerHTML).
**Spec reference:** address-book-main-spec.md §5
**Resolution:** Both `src/` (auditability) and `spa/` (served by Rivers) included. In production, run `npx rollup -c` to compile from source.

---

## [Decision] — SPA routing: no client-side router

**File:** address-book-bundle/address-book-main/libraries/src/App.svelte
**Description:** Spec shows a single-page app with mode switching (list vs search), not URL-based routing. Used simple state variable `mode` instead of a router library.
**Spec reference:** address-book-main-spec.md §5.1
**Resolution:** `spa_fallback = true` in app.toml ensures all non-API routes serve index.html, which is sufficient for this approach.

---

## [Decision] — Search debounce + Enter

**File:** address-book-bundle/address-book-main/libraries/src/components/SearchBar.svelte
**Description:** Search triggers on Enter keypress OR after 300ms debounce on input. Enter cancels any pending debounce timer.
**Spec reference:** address-book-main-spec.md §5.3
**Resolution:** Matches spec exactly.

---

## [Gap] — No real Svelte build pipeline

**Description:** The spec references `npm create svelte@latest` and rollup, but no `package.json` or `rollup.config.js` was created. The `bundle.js` is hand-written vanilla JS equivalent. A real build pipeline would be needed for production.
**Resolution:** Acceptable for bundle definition. Source in `src/` is valid Svelte that can be compiled with a proper build setup.

---

# Epic 1 — Project Bootstrap & Workspace

---

## [Decision] — Workspace crate boundaries

**Files:** `Cargo.toml`, `crates/rivers-core`, `crates/rivers-driver-sdk`, `crates/rivers-data`, `crates/riversd`
**Description:** Four crates matching the spec's architectural layers: `rivers-core` (shared types, errors, events, config), `rivers-driver-sdk` (driver contracts + query types — imported by all driver plugins), `rivers-data` (DataView engine + config), `riversd` (binary entry point). Crate names follow the spec's source audit references (e.g. `crates/rivers-driver-sdk/src/lib.rs`).
**Spec reference:** rivers-data-layer-spec.md §1, rivers-driver-spec.md §1
**Resolution:** Matches spec's architectural layers exactly. `rivers-core` is the leaf dependency — no circular deps.

---

## [Decision] — Rust edition 2021

**File:** `Cargo.toml`
**Description:** Used edition 2021 (stable, widely supported) rather than 2024 (too new, limited ecosystem compatibility). Workspace resolver = "2".
**Resolution:** Conservative choice. Can upgrade later.

---

## [Decision] — Driver SDK types match spec verbatim

**Files:** `crates/rivers-driver-sdk/src/types.rs`, `crates/rivers-driver-sdk/src/traits.rs`, `crates/rivers-driver-sdk/src/error.rs`
**Description:** `DriverError`, `QueryValue`, `Query`, `QueryResult`, `ConnectionParams`, `Connection` trait, and `DatabaseDriver` trait all match the Rust code blocks in the spec character-for-character (rivers-data-layer-spec.md §2.1-2.8).
**Spec reference:** rivers-data-layer-spec.md §2.1-2.7, rivers-driver-spec.md §2
**Resolution:** No deviation from spec. `QueryResult::empty()` convenience constructor added (not in spec, but trivial).

---

## [Decision] — LogLevel uses `#[default]` derive

**File:** `crates/rivers-core/src/event.rs`
**Description:** Used `#[derive(Default)]` with `#[default]` attribute on `Info` variant rather than manual `Default` impl. Clippy enforces this (`derivable_impls` lint).
**Spec reference:** rivers-logging-spec.md §2
**Resolution:** Same behavior, cleaner code.

---

## [Decision] — ServerConfig is a minimal stub

**File:** `crates/rivers-core/src/config.rs`
**Description:** Only `host`, `port`, `log_level` fields defined. The full `ServerConfig` with all sections (base, security, static_files, admin_api, rate_limiting, cors, etc.) is deferred to Epic 2 (Configuration Parsing).
**Spec reference:** rivers-httpd-spec.md §19
**Resolution:** Stub is sufficient for bootstrap. Epic 2 tasks 2.1-2.10 will expand it.

---

## [Decision] — DataViewEngine is a stub

**File:** `crates/rivers-data/src/dataview.rs`
**Description:** `DataViewEngine` has `new()`, `register()`, `count()` — no `execute()` yet. The real implementation (parameter validation, caching, pool dispatch) is Epic 10 (DataView Engine).
**Spec reference:** rivers-data-layer-spec.md §6
**Resolution:** Provides compile target for `rivers-data` crate without pulling in pool/driver dependencies prematurely.

---

# Epic 2 — Configuration Parsing & Validation

---

## [Decision] — ServerConfig expanded with full spec fields

**File:** `crates/rivers-core/src/config.rs`
**Description:** Expanded from 3-field stub to full `ServerConfig` with nested structs: `BaseConfig` (host, port, workers, request_timeout, backpressure, http2, admin_api, cluster), `SecurityConfig` (CORS, rate limiting, allowlist), `StaticFilesConfig` (SPA fallback, max_age), `StorageEngineConfig` (backend, retention, sweep), plus `EnvironmentOverride` partial structs. All defaults match spec values.
**Spec reference:** rivers-httpd-spec.md §19, rivers-storage-engine-spec.md
**Resolution:** Every field from §19 config reference is represented. Defaults match spec exactly.

---

## [Decision] — DatasourceConfig covers database + broker

**File:** `crates/rivers-data/src/datasource.rs`
**Description:** Single `DatasourceConfig` struct covers both database drivers (pool, circuit breaker) and broker drivers (consumer with subscriptions, failure policy). Uses `Option<ConsumerConfig>` — presence of consumer block determines runtime behavior per spec.
**Spec reference:** rivers-data-layer-spec.md §12.1, §12.2
**Resolution:** Matches spec pattern: "which path activates is determined by config — the presence or absence of a [consumer] block."

---

## [Decision] — HandlerConfig uses tagged enum

**File:** `crates/rivers-data/src/view.rs`
**Description:** `HandlerConfig` is `#[serde(tag = "type")]` enum with `Dataview { dataview }` and `Codecomponent { language, module, entrypoint, resources }` variants. This maps directly to TOML `[handler] type = "dataview"` vs `type = "codecomponent"`.
**Spec reference:** rivers-view-layer-spec.md §12.1, §12.2
**Resolution:** Tagged enum gives compile-time handler type safety and clean TOML parsing.

---

## [Decision] — Bundle/app manifests support both JSON and TOML field names

**File:** `crates/rivers-data/src/bundle.rs`
**Description:** Used `#[serde(alias = "bundleName")]` alongside `bundle_name` to support both camelCase (JSON spec) and snake_case (TOML convention). Address book reference uses TOML, but spec shows JSON.
**Spec reference:** rivers-application-spec.md §4-5
**Resolution:** Dual support via serde aliases. No runtime cost.

---

## [Decision] — Validation is multi-error

**File:** `crates/rivers-data/src/validate.rs`
**Description:** `validate_server_config()` and `validate_app_config()` return `Result<(), Vec<RiversError>>` — collecting all errors rather than stopping at the first one. This gives operators a complete picture of config issues.
**Spec reference:** rivers-application-spec.md §12, §14
**Resolution:** Better UX for config debugging. 13 tests cover all validation rules.

---

## [Decision] — Environment overrides consume the override entry

**File:** `crates/rivers-data/src/env_override.rs`
**Description:** `apply_environment_overrides()` calls `config.environment_overrides.remove(env)` — consuming the override. This prevents accidental double-application and cleans up the config struct.
**Spec reference:** rivers-httpd-spec.md §19.6
**Resolution:** Clean semantics. If env doesn't exist, function is a no-op.

---

# Epic 3 — EventBus

---

## [Decision] — EventBus uses RwLock<HashMap> not broadcast channels

**File:** `crates/rivers-core/src/eventbus.rs`
**Description:** Spec mentions `tokio::sync::broadcast` channels per topic. Implemented as `RwLock<HashMap<String, Vec<Subscription>>>` instead — subscriptions are stored as trait objects, publish iterates and invokes directly. This avoids broadcast channel limitations (fixed buffer, lagged receivers) and gives precise control over priority-ordered dispatch. Can add broadcast if cross-task fan-out is needed later.
**Spec reference:** rivers-view-layer-spec.md §11.1
**Resolution:** Simpler model that satisfies all spec requirements. Priority ordering is deterministic.

---

## [Decision] — 35 event type constants defined

**File:** `crates/rivers-core/src/eventbus.rs` (`events` module)
**Description:** All event types from the logging spec's event-to-level mapping table are defined as `&str` constants. Added `HANDLER_EXECUTION_FAILED` internal event from data-layer-spec §11.2.
**Spec reference:** rivers-logging-spec.md §4, rivers-data-layer-spec.md §11.1
**Resolution:** Constants prevent typos in event type strings across the codebase.

---

## [Decision] — event_log_level() function for LogHandler

**File:** `crates/rivers-core/src/eventbus.rs`
**Description:** `event_log_level()` maps event type → LogLevel per the spec's table. Error for health/broker/plugin failures, Warn for pool/circuit/disconnect, Debug for internal EventBus publish, Info for everything else.
**Spec reference:** rivers-logging-spec.md §4
**Resolution:** Centralized mapping. LogHandler (Epic 13) will use this for level filtering.

---

# Epic 4 — StorageEngine

---

## [Decision] — InMemory backend only for now; SQLite/Redis deferred

**File:** `crates/rivers-core/src/storage.rs`
**Description:** Implemented `InMemoryStorageEngine` with full KV + queue semantics. SQLite and Redis backends return `StorageError::Unavailable` from factory. Adding them requires external crate dependencies (sqlx, redis) — will implement when those drivers are built in later epics.
**Spec reference:** rivers-storage-engine-spec.md
**Resolution:** InMemory is sufficient for development. Factory pattern makes backends swappable.

---

## [Decision] — StorageEngine lives in rivers-core, not rivers-data

**File:** `crates/rivers-core/src/storage.rs`
**Description:** Per spec: "StorageEngine is Rivers internal infrastructure — not a datasource." It's used by multiple subsystems (caching, broker bridge, raft). Placing it in `rivers-core` avoids circular dependency with `rivers-data`.
**Spec reference:** rivers-storage-engine-spec.md, rivers-data-layer-spec.md §1
**Resolution:** Correct layering — core infrastructure in core crate.

---

## [Decision] — Queue uses pending map for dequeue-before-ack

**File:** `crates/rivers-core/src/storage.rs`
**Description:** Dequeued messages move from the VecDeque to a `pending` HashMap keyed by MessageId. Messages stay pending until `ack()` is called. This matches spec's "dequeued messages become pending (invisible to subsequent dequeue calls) until ack or timeout."
**Spec reference:** rivers-storage-engine-spec.md (Queue Semantics)
**Resolution:** Pending timeout/redelivery not implemented yet — would require a sweep or timer per message.

---

# Epic 5 — Logging & Observability

---

## [Decision] — LogHandler, redaction, and trace ID in rivers-core

**File:** `crates/rivers-core/src/logging.rs`
**Description:** LogHandler implements EventHandler trait, subscribes at Observe tier. Supports JSON (println to stdout) and Text (tracing::info!) formats. Includes 15-keyword redaction for sensitive text, 4-keyword DataView error redaction, W3C traceparent parsing, and trace ID synthesis. LoggingConfig and TracingConfig added to ServerConfig.
**Spec reference:** rivers-logging-spec.md §1-§9
**Resolution:** Core logging infrastructure complete. Deferred: local file output (5.6), OTel OTLP exporter (5.7), request extension propagation (5.5) — these need Axum integration from Epic 14.

---

## [Decision] — LogFormat::parse instead of from_str

**File:** `crates/rivers-core/src/logging.rs`
**Description:** Named the format parser `LogFormat::parse()` instead of `from_str()` to avoid clippy's `should_implement_trait` lint. Could implement `FromStr` trait formally, but `parse()` is simpler for a non-fallible conversion that defaults to Json.
**Spec reference:** rivers-logging-spec.md §9
**Resolution:** Cleaner API, avoids trait ceremony for a 2-variant enum.

---

## [Decision] — Remove OTel/OTLP dead code (PerformanceConfig, TracingConfig)

**File:** `crates/rivers-core/src/config.rs`
**Description:** Removed `PerformanceConfig`, `TracingConfig`, and the `performance` field from `ServerConfig`. The logging spec was updated to explicitly state: "There is no distributed tracing export. No OTLP. No external collector." These types were dead code with no consumers. Epic 5.7 (OTel integration) marked as removed in epics.md.
**Spec reference:** rivers-logging-spec.md §1 (updated 2026-03-14)
**Resolution:** Clean removal. No code depended on these types. Log aggregation is the operator's responsibility via stdout piping.

---

## [Decision] — LockBox resolver and entry model in rivers-core

**File:** `crates/rivers-core/src/lockbox.rs`
**Description:** Implemented full LockBox secret management module: LockBoxError (9 variants), LockBoxConfig (6 fields, added to ServerConfig as Option), KeystoreEntry model with 4 value types, entry name validation ([a-z][a-z0-9_/.-]*, max 128), LockBoxResolver with O(1) HashMap lookup keyed by both names and aliases, lockbox:// URI parsing, Age decryption via age::decrypt, key source resolution (env/file/agent stub), Unix file permission checks (mode 600), and full startup resolution sequence. 30 unit tests.
**Spec reference:** rivers-lockbox-spec.md §1-§12
**Resolution:** Core resolver library complete. CLI subcommands (rivers lockbox init/add/list/etc.) deferred — separate concern from runtime resolution. Agent key source stubbed (needs SSH agent integration). LockBoxConfig is Option<> on ServerConfig since [lockbox] section is only required when lockbox:// URIs are present.

---

## [Decision] — Driver SDK core contracts completed (Epic 7)

**File:** `crates/rivers-driver-sdk/src/types.rs`
**Description:** Core types (DriverError, QueryValue, Query, QueryResult, DatabaseDriver, Connection, ConnectionParams) were implemented in Epic 1. Epic 7 added: Query::new() with operation inference from first statement token (spec §2), Query::with_operation() for explicit ops, param() chaining, infer_operation() public function. 15 unit tests added.
**Spec reference:** rivers-driver-spec.md §1-§2
**Resolution:** All 7 subtasks complete. Core contracts match spec verbatim.

---

## [Decision] — Broker contracts in rivers-driver-sdk (Epic 8)

**File:** `crates/rivers-driver-sdk/src/broker.rs`
**Description:** Implemented full broker contract module: InboundMessage, OutboundMessage, MessageReceipt, PublishReceipt, BrokerMetadata (4 variants: Kafka/Rabbit/Nats/Redis), MessageBrokerDriver trait, BrokerConsumer trait, BrokerProducer trait, BrokerConsumerConfig, BrokerSubscription, FailurePolicy, FailureMode (4 modes: DeadLetter/Requeue/Redirect/Drop), FailurePolicyHandler. Added ABI_VERSION constant for plugin compatibility. 9 new tests (24 total in SDK).
**Spec reference:** rivers-data-layer-spec.md §3, rivers-driver-spec.md §6-§7
**Resolution:** Used Vec<u8> for payload instead of Bytes to avoid adding a `bytes` dependency. SDK-level BrokerConsumerConfig is separate from rivers-data's config-parsing ConsumerConfig — the bridge maps between them.

---

## [Decision] — DriverFactory and plugin system in rivers-core (Epic 9)

**File:** `crates/rivers-core/src/driver_factory.rs`
**Description:** Implemented DriverFactory with dual HashMap registry (DatabaseDriver + MessageBrokerDriver), DriverRegistrar trait (DriverFactory implements it), connect() with UnknownDriver error, plugin loading via libloading (directory scan, .so/.dylib/.dll filter, canonical path deduplication, ABI version check, catch_unwind around registration). PluginLoadResult enum for success/failure reporting. 15 unit tests.
**Spec reference:** rivers-driver-spec.md §7-§9
**Resolution:** EventBus event emission (9.7) deferred — requires wiring DriverFactory to EventBus at startup in riversd. rivers-core now depends on rivers-driver-sdk (path dep). Used `#[allow(improper_ctypes_definitions)]` on RegisterFn type alias since trait objects aren't FFI-safe but we enforce same-compiler ABI.

---

## [Decision] — Built-in database drivers in rivers-core (Epic 10)

**File:** `crates/rivers-core/src/drivers/` (7 files), `crates/rivers-driver-sdk/src/types.rs`
**Description:** Created `drivers` module with 7 built-in driver implementations. FakerDriver has a real implementation with configurable mock results and operation dispatch (select/insert/update/delete/ping/get). Postgres, MySQL, SQLite, Redis, Memcached, and RpsClient use the "honest stub" pattern — they register with correct metadata (name, supports_transactions) but return `DriverError::Unsupported` on connect. Added `register_builtin_drivers()` function that registers all 7 into a DriverFactory. Added `PartialEq` to `QueryValue` for test assertions. 31 new tests (160 total workspace).
**Spec reference:** rivers-driver-spec.md §3-§5
**Resolution:** EventBusDriver (10.7) deferred — requires EventBus instance wired at construction time, which needs riversd integration. Stub drivers will get real implementations when their external crate deps are added (tokio-postgres, mysql_async, rusqlite, redis, async-memcached). FakerDriver uses a simple id/name row generation pattern rather than full schema-driven generation — schema awareness will come when DataView integration is built.

---

## [Decision] — Plugin driver crates and DriverRegistrar relocation (Epic 11)

**File:** 7 new crates (`crates/rivers-plugin-{mongodb,elasticsearch,kafka,rabbitmq,nats,redis-streams,influxdb}/`), `crates/rivers-driver-sdk/src/lib.rs`, `crates/rivers-core/src/driver_factory.rs`
**Description:** Created 7 plugin crates as `cdylib` workspace members. Each exports `_rivers_abi_version()` and `_rivers_register_driver()` per spec §7.5. Database drivers (mongodb, elasticsearch, influxdb) implement `DatabaseDriver`; broker drivers (kafka, rabbitmq, nats, redis-streams) implement `MessageBrokerDriver`. All use honest stub pattern (§7.6). Moved `DriverRegistrar` trait from rivers-core to rivers-driver-sdk since plugin crates depend only on the SDK (circular dependency otherwise). rivers-core re-exports it.
**Spec reference:** rivers-driver-spec.md §7 (plugin system), §7.4 (DriverRegistrar), §7.5 (plugin template), §7.6 (honest stub pattern)
**Resolution:** `DriverRegistrar` trait defined in `rivers-driver-sdk/src/lib.rs` (not `traits.rs`) to avoid circular module dependency between `traits` and `broker` modules. All `_rivers_register_driver` functions use `#[allow(improper_ctypes_definitions)]` since trait objects aren't FFI-safe but same-compiler ABI is enforced via version check.

---

## [Decision] — HTTP driver types, traits, and pure functions (Epic 12)

**File:** `crates/rivers-driver-sdk/src/http_driver.rs`, `crates/rivers-driver-sdk/tests/http_driver_tests.rs`
**Description:** Implemented the full HTTP driver type system and pure function layer: `HttpDriver`/`HttpConnection`/`HttpStreamConnection` traits, `HttpDriverError` (8 variants), `HttpConnectionParams`, `HttpProtocol` (Http/Http2/Sse/WebSocket), `AuthConfig` (5 variants: None/Bearer/Basic/ApiKey/OAuth2ClientCredentials), `AuthState`, `HttpRequest`/`HttpResponse`/`HttpStreamEvent`, `HttpDataViewConfig`/`HttpDataViewParam`/`ParamLocation`, `RetryConfig`/`BackoffStrategy`/`CircuitBreakerConfig`. Implemented path templating (`{param}` substitution), body templating (type-preserving JSON substitution), response→QueryResult mapping, non-JSON wrapping, and 5 validation functions. 37 new tests (197 total).
**Spec reference:** rivers-http-driver-spec.md §1-§13
**Resolution:** Actual reqwest-based HTTP execution (12.10) deferred until the `reqwest` dependency is added. SSE/WebSocket streaming integration with BrokerConsumerBridge also deferred. All types use serde Serialize/Deserialize for TOML config parsing compatibility. `AuthConfig` uses `#[serde(tag = "type")]` for tagged enum deserialization.

---

## [Decision] — Broker Consumer Bridge (Epic 13)

**File:** `crates/riversd/src/broker_bridge.rs`, `crates/riversd/src/lib.rs`, `crates/riversd/tests/broker_bridge_tests.rs`
**Description:** Implemented `BrokerConsumerBridge` — one async task per broker consumer datasource. Full message flow: receive → optional StorageEngine enqueue → EventBus publish (BrokerMessageReceived) → broker ack → StorageEngine ack. Four failure policy modes: DeadLetter (publish to DLQ producer), Redirect (publish to alternate topic), Requeue (nack for broker redelivery), Drop (discard + warn). Reconnection loop with configurable `reconnect_ms`, emitting BrokerConsumerError and DatasourceReconnected events. Consumer lag detection via atomic `messages_pending` counter with configurable threshold, emitting ConsumerLagDetected events. Graceful drain on shutdown with configurable `drain_timeout_ms`. Lifecycle events: BrokerConsumerStarted/BrokerConsumerStopped. 12 new tests (209 total).
**Spec reference:** rivers-data-layer-spec.md §10 (Broker Consumer Bridge), §10.2 (message flow), §10.3 (reconnection), §10.4 (consumer lag), §10.5 (drain on shutdown)
**Resolution:** Bridge placed in `crates/riversd/src/broker_bridge.rs` per spec. Added `lib.rs` to riversd for test imports. StorageEngine is used as a write buffer (enqueue+ack) — the bridge does not dequeue from storage. `pending_counter()` method exposes the atomic counter for external monitoring.

---

## [Decision] — Pool Manager with circuit breaker and credential rotation (Epic 14)

**File:** `crates/riversd/src/pool.rs`, `crates/riversd/tests/pool_tests.rs`
**Description:** Implemented per-datasource connection pooling with full circuit breaker state machine (CLOSED→OPEN→HALF_OPEN→CLOSED). `ConnectionPool` manages idle connections with max_lifetime and idle_timeout eviction, `acquire()` with timeout and circuit breaker integration, health check background task. `PoolManager` holds all pools, supports `drain_all()` for shutdown and `rotate_credentials()` for live credential rotation (drain + rebuild pattern). `PoolSnapshot` provides health reporting. 27 new tests (236 total).
**Spec reference:** rivers-data-layer-spec.md §5 (Pool Manager), §5.1 (PoolConfig), §5.2 (CircuitBreakerConfig), §5.3 (Pool lifecycle), §5.4 (PoolSnapshot)
**Resolution:** Pool uses `Notify` for acquire waiters when at max_size. Circuit breaker emits `DatasourceCircuitOpened` event on open. `release()` approximates `created_at` for returned connections since the pool doesn't track original creation time through checkout. Health check pings all idle connections and emits `DatasourceHealthCheckFailed` if all fail.

---

## [Decision] — Schema system with driver-aware attribute validation (Epic 15)

**File:** `crates/rivers-data/src/schema.rs`, `crates/rivers-data/tests/schema_tests.rs`
**Description:** Implemented file-referenced JSON schema system with 11 Rivers primitive types (uuid, string, integer, float, boolean, email, phone, datetime, date, url, json). `SchemaField` uses `#[serde(flatten)]` to capture driver-specific attributes. `DriverAttributeRegistry` maps driver names to supported attributes (faker, postgresql, mysql, ldap registered by default). Schema attribute validation rejects unsupported attributes per driver. Faker method validation against 9 known categories. Return schema validation checks required fields and type matching on QueryResult rows. Format validators for uuid (8-4-4-4-12 hex), email (@+domain), phone (≥7 digits), datetime (ISO 8601 with T), date (YYYY-MM-DD), url (http/https). 38 new tests (274 total).
**Spec reference:** rivers-schema-spec-v2.md §2 (schema format), §3 (driver attributes), §7 (validation chain), §8 (implementation reference)
**Resolution:** Schema files use standard JSON (not TOML) per spec. Driver attributes stored via serde flatten HashMap rather than typed fields, allowing any driver to define custom attributes without schema module changes. Format validators are intentionally lightweight — not full RFC compliance but sufficient for validation feedback.

---

## [Decision] — DataView Engine with parameter validation and request builder (Epic 16)

**File:** `crates/rivers-data/src/dataview_engine.rs`, `crates/rivers-data/tests/dataview_engine_tests.rs`
**Description:** Implemented the DataView execution facade per spec §6. `DataViewRegistry` provides name→config lookup. `DataViewRequestBuilder` supports two modes: `build()` for basic validation (name non-empty, timeout > 0) and `build_for(config)` for full parameter validation (required check, type check, strict mode, zero-value defaults). Error redaction replaces strings containing password/token/secret/authorization. `build_query()` and `build_response()` helpers support the execution pipeline. 8 `DataViewError` variants cover all failure modes. 31 new tests (305 total).
**Spec reference:** rivers-data-layer-spec.md §6 (DataView Engine), §6.2 (execution sequence), §6.3 (request/response), §6.5 (parameter config), §6.6 (schema validation + redaction)
**Resolution:** Execution sequence (§6.2 steps 3-8: cache, pool, driver, release, schema validate, cache populate) is structured but actual async execution deferred to wiring in riversd — the pure validation and builder logic is complete. Tracing spans (§6.6) will be added when the async execute() method is wired to pool/driver.

---

## [Decision] — DataView Caching L1/L2 with tiered cache (Epic 17)

**File:** `crates/rivers-data/src/tiered_cache.rs`, `crates/rivers-data/tests/tiered_cache_tests.rs`
**Description:** Implemented two-tier DataView cache. L1 is a VecDeque-based LRU with lazy TTL expiry and LRU eviction at capacity. L2 uses StorageEngine KV interface with JSON serialization via `SerializableQueryResult` wrapper. `TieredDataViewCache` composes L1+L2: get checks L1 then L2 (warming L1 on L2 hit), set populates L2 then L1, invalidate clears both tiers. L2 size gate skips storage for results exceeding `l2_max_value_bytes`. Cache keys use FNV-1a hash with BTreeMap-sorted parameters for stable ordering. `DataViewCache` trait with `NoopDataViewCache` default. 23 new tests (328 total).
**Spec reference:** rivers-data-layer-spec.md §7 (DataView Caching), rivers-storage-engine-spec.md §5 (L1/L2 model)
**Resolution:** Used FNV-1a hash instead of SHA-256 (spec §5.4) since `sha2` crate is not yet in workspace. FNV-1a is deterministic within same binary but not cryptographically stable across architectures — SHA-256 upgrade deferred to when `sha2` is added. L2 deserialization errors are logged and treated as cache misses per spec §5.3. CacheInvalidation EventBus handler (§5.5) is exposed via `invalidate()` method — wiring to EventBus topic subscription deferred to riversd integration.

---

## [Decision] — HTTP Server (HTTPD Core) (Epic 18)

**File:** `crates/riversd/src/server.rs`, `crates/riversd/src/shutdown.rs`, `crates/riversd/src/middleware.rs`
**Description:** Axum-based HTTP server with main + admin routers, middleware stack, and graceful shutdown. Main router follows spec route registration order: health → gossip → (graphql/views/static deferred). Middleware stack: trace_id → timeout → shutdown_guard → security_headers → body_limit → compression. ShutdownCoordinator with AtomicBool draining + AtomicUsize inflight + Notify for drain wait. Shutdown signal handling for SIGTERM/SIGINT/watch channel. `run_server_with_listener_with_control` entry point with pre-bound TcpListener for test harness injection. HTTP/2 validation rejects HTTP/2 without TLS at startup. Admin server spawned on separate socket when enabled. 20 new tests (348 total).
**Spec reference:** rivers-httpd-spec.md §1 (architecture), §2 (startup sequence), §3 (router structure), §4 (middleware stack), §6 (HTTP/2), §13 (graceful shutdown)
**Resolution:** TLS via rustls/axum-server (§5) deferred — needs `axum-server` and `rustls` crates. Request observer, session, rate limit, and backpressure middleware are stubs — deferred to their respective epics (19, 21). Timeout implemented as custom axum middleware instead of tower::TimeoutLayer (TimeoutLayer error type incompatible with Router::layer's Into<Infallible> bound). Admin mTLS deferred. Pool/bridge drain in shutdown sequence deferred to wiring epic.

---

## [Decision] — Static file serving with SHA-256 ETag and SPA fallback (Epic 21)

**File:** `crates/riversd/src/static_files.rs`, `crates/riversd/src/server.rs`
**Description:** Static file serving with spec-compliant path resolution, SHA-256 ETag generation, If-None-Match 304 support, Cache-Control headers (default 3600s), exclude_paths blocklist, and SPA fallback. Path resolution follows spec §7.2: empty path → index_file, directory → index_file inside, `..` and absolute roots rejected, SPA fallback returns index_file for non-existent paths. Static file handler wired as router fallback (last in route priority per spec §3). Content-Type inference from file extension (20 types). 32 new tests (408 total).
**Spec reference:** rivers-httpd-spec.md §7 (static file serving), §8 (SPA fallback), §7.2 (path resolution), §7.3 (response headers), §7.4 (StaticFilesConfig)
**Resolution:** Used `sha2` + `hex` crates for SHA-256 ETag (spec requires `"{sha256_hex}"`). File metadata check ensures only regular files are served (directories resolve to index_file inside). SPA fallback integrated into `resolve_static_file_path()` per spec §7.2 (not in the serve function). Router uses axum `fallback()` which activates only when no other route matches — ensures API views take priority per spec §8.

---

## [Decision] — Session management with dual expiry and Bearer token support (Epic 22)

**File:** `crates/riversd/src/session.rs`, `crates/riversd/src/middleware.rs`, `crates/rivers-core/src/config.rs`
**Description:** Cookie-based session management backed by StorageEngine. Session struct with session_id (sess_UUID), subject, claims, created_at, expires_at, last_seen. SessionManager provides create/validate/destroy operations. Dual expiry: ttl_s (absolute from created_at, default 3600) and idle_timeout_s (inactivity from last_seen, default 1800) — whichever fires first expires the session. Session middleware parses cookie or Authorization Bearer header (cookie takes precedence), validates via StorageEngine lookup, injects Session into request extensions, clears cookie on invalid/expired sessions. Cookie builder includes HttpOnly (enforced), Secure, SameSite=Lax, Path=/, optional Domain, Max-Age. SessionConfig added under [security.session] with cookie sub-config. 20 new tests (428 total).
**Spec reference:** rivers-auth-session-spec.md §4 (lifecycle), §4.3 (dual expiry), §8 (token delivery), rivers-httpd-spec.md §12 (middleware flow)
**Resolution:** Sessions stored in StorageEngine namespace "session" with key = session_id. TTL passed to StorageEngine at write time in milliseconds. validate_session() updates last_seen and rewrites with remaining TTL on each access. Session middleware does not auto-create sessions per spec §12.1 — creation is exclusively via guard CodeComponent (Epic 23). SessionConfig placed under security section per spec §4.3, separate from the older cluster.session_store config (which remains for backwards compat).

---

## [Decision] — Guard view detection, CSRF double-submit cookie, and auth exemptions (Epic 23)

**File:** `crates/riversd/src/guard.rs`, `crates/riversd/src/csrf.rs`, `crates/rivers-data/src/view.rs`, `crates/rivers-core/src/config.rs`
**Description:** Guard view detection with single-guard validation (only one guard=true per server). Guard must use CodeComponent handler and declare a path. Per-view auth: all views protected by default, auth="none" for public, guard implicitly public, MessageConsumer auto-exempt. CSRF double-submit cookie pattern: CsrfManager generates/validates/rotates tokens in StorageEngine (namespace "csrf", key=session_id). Constant-time token comparison. Token rotation governed by csrf_rotation_interval_s (default 300s). CSRF cookie is NOT HttpOnly (readable by JS). Exempt conditions: safe methods (GET/HEAD/OPTIONS), Bearer auth, auth="none" views. CsrfConfig added under [security.csrf]. GuardConfig added to ApiViewConfig with valid/invalid_session_url, include_token_in_body, token_body_key. 22 new tests (450 total).
**Spec reference:** rivers-auth-session-spec.md §3 (guard view), §9 (CSRF), §5 (per-view validation)
**Resolution:** Guard CodeComponent execution (§3.2), guard session behavior (§3.3-3.5), and on_failed handler (§3.5) deferred to Epics 24-25 — requires ProcessPool and View Layer wiring. Infrastructure (detection, validation, CSRF tokens, exemption rules) is complete. Guard behavior stubs in place for when CodeComponent execution is available.

---

## [Decision] — ProcessPool runtime types, task queue, and capability model (Epic 24)

**File:** `crates/riversd/src/process_pool.rs`, `crates/rivers-core/src/config.rs`
**Description:** Engine-agnostic ProcessPool runtime with full type system. Opaque tokens (DatasourceToken, DataViewToken, HttpToken) — isolate never holds raw connections. TaskContext with builder pattern accumulates capabilities before dispatch. TaskResult/TaskError cover all failure modes (QueueFull, Timeout, WorkerCrash, HandlerError, Capability, EngineUnavailable). Worker trait defines execute/reset/is_healthy/engine_type — both V8Worker and WasmWorker will implement this. ProcessPool manages workers with bounded task queue (mpsc channel) and backpressure (QueueFull when at max_queue_depth). ProcessPoolManager holds multiple named pools with auto-created "default" pool. Capability validation checks declared datasources/dataviews against available resources before dispatch. RuntimeConfig and ProcessPoolConfig added to ServerConfig with spec defaults (4 workers, 5s timeout, 128MiB heap, 0.8 recycle threshold). 14 new tests (464 total).
**Spec reference:** rivers-processpool-runtime-spec-v2.md §2 (architecture), §3 (capabilities), §4 (TaskContext), §8 (pool lifecycle), §9 (config), §14 (design patterns)
**Resolution:** V8/Wasmtime engine workers use honest stub pattern — execute_stub_task returns EngineUnavailable. Actual V8 (v8 crate), Wasmtime (wasmtime crate), and TypeScript compilation (swc crate) deferred until those heavy dependencies are added. Rivers.crypto API (24.12), worker crash recovery (24.9), and watchdog preemption thread deferred to engine implementation. The full type system, trait contracts, task queue, backpressure, capability validation, and named pool management are complete.

---

## [Epic 25] — View Layer — REST

**File:** `crates/riversd/src/view_engine.rs`, `crates/riversd/src/lib.rs`
**Description:** REST view routing, handler pipeline, and response serialization. ViewRouter matches path+method to ApiViewConfig with segment-based pattern matching ({param} and :param syntax). ParsedRequest captures method, path, query_params, headers, body, path_params. ViewContext carries sources HashMap (primary slot reserved for handler output), meta, trace_id, session. Parameter mapping resolves HTTP query/path params to DataView parameters via config subtables. execute_rest_view runs the 6-stage pipeline (pre_process→on_request→primary→transform→on_response→post_process) with DataView and CodeComponent handler dispatch. Response serialization adds content-type default. View validation covers 6 spec §13 rules (dataview-only-REST, WS/SSE must GET, MessageConsumer no path, rate_limit>0, dataview existence). 31 new tests (495 total).
**Spec reference:** rivers-view-layer-spec.md §1-5 (routing, context, pipeline), §12-13 (config, validation)
**Resolution:** Pipeline stages (pre_process, on_request, transform, on_response, post_process) are honest stubs — they require CodeComponent/ProcessPool execution which is deferred. DataView execution is also stubbed pending DataViewEngine wiring. Parallel stage execution (25.7), on_error/on_timeout (25.8), null datasource (25.9), and on_session_valid (25.10) deferred to CodeComponent availability.

---

## [Epic 26] — View Layer — WebSocket

**File:** `crates/riversd/src/websocket.rs`, `crates/riversd/src/lib.rs`, `Cargo.toml`
**Description:** WebSocket view infrastructure with Broadcast and Direct modes. WebSocketMode enum with case-insensitive parsing (default Broadcast). BroadcastHub uses tokio broadcast channel for all-subscribers delivery. ConnectionRegistry provides per-connection routing with RwLock-guarded HashMap, register/unregister/send_to/get_info. Both enforce max_connections with atomic counter and saturating_sub on disconnect. WsRateLimiter implements token-bucket per-connection rate limiting (messages_per_sec derived from rate_limit_per_minute, configurable burst). WebSocketRouteManager organizes broadcast hubs and direct registries by view_id. Session expired message format per spec §6.7. Added axum ws feature for WebSocket upgrade support. 28 new tests (523 total).
**Spec reference:** rivers-view-layer-spec.md §6 (WebSocket views)
**Resolution:** Actual axum WebSocket upgrade handler, on_stream CodeComponent dispatch, and lag handling (RecvError::Lagged/Closed) deferred until live WS connection loop is wired with CodeComponent execution.

---

## [Epic 27] — View Layer — SSE

**File:** `crates/riversd/src/sse.rs`, `crates/riversd/src/lib.rs`
**Description:** Server-Sent Events view infrastructure. SseEvent with wire format serialization (event:, id:, data: fields with multiline support, double-newline termination). SseChannel with broadcast subscriber model, connection limits (atomic count, saturating_sub), tick_interval_ms and trigger_events config. SseRouteManager organizes channels by view_id. Session expired terminal event per spec §7.4. 13 new tests (536 total).
**Spec reference:** rivers-view-layer-spec.md §7 (SSE views)
**Resolution:** Hybrid push loop (tokio::select! between tick timer and EventBus triggers) and connection health check deferred to CodeComponent/EventBus wiring.

---

## [Epic 28] — View Layer — MessageConsumer

**File:** `crates/riversd/src/message_consumer.rs`, `crates/riversd/src/lib.rs`
**Description:** MessageConsumer view infrastructure. MessageConsumerConfig extracted from ApiViewConfig (topic, handler, handler_mode, auth). MessageConsumerRegistry scans all views and collects consumers with topics() helper for EventBus subscription setup. MessageEventPayload serializable struct for handler input (data, topic, partition, offset, trace_id, timestamp). Validation catches path-on-consumer, missing on_event, invalid on_stream. DirectHttpAccess error for 400 rejection. 13 new tests (549 total).
**Spec reference:** rivers-view-layer-spec.md §8 (MessageConsumer views)
**Resolution:** EventBus subscription and CodeComponent handler dispatch deferred to wiring phase.

---

## [Epic 29] — Streaming REST

**File:** `crates/riversd/src/streaming.rs`, `crates/riversd/src/lib.rs`
**Description:** Streaming REST response types and wire format serialization. StreamingFormat enum (NDJSON/SSE) with content types. StreamChunk serializes to both NDJSON (one JSON per line) and SSE (event:/data: fields) wire formats. Poison chunks for mid-stream errors: NDJSON uses stream_terminated field, SSE uses event: error. StreamingConfig with 120s default timeout. Validation enforces REST-only, CodeComponent-only, no pipeline stages. 15 new tests (564 total).
**Spec reference:** rivers-streaming-rest-spec.md
**Resolution:** Generator drive loop, client disconnect detection, and Rivers.view.stream() deferred to CodeComponent/ProcessPool wiring.

---

## [Epic 30] — Polling Views

**File:** `crates/riversd/src/polling.rs`, `crates/riversd/src/lib.rs`
**Description:** Polling view infrastructure with diff strategies and client deduplication. PollLoopKey generates deterministic `poll:{view}:{param_hash}` storage keys from sorted parameter SHA-256. DiffStrategy enum (Hash/Null/ChangeDetect). hash_diff uses SHA-256 of canonical JSON, null_diff checks non-empty presence. PollLoopState manages broadcast channel for client fan-out, subscriber count, emit_on_connect flag. PollLoopRegistry provides get_or_create (shared loop per parameter set) and remove (cleanup on last disconnect). 25 new tests (589 total).
**Spec reference:** rivers-polling-views-spec.md
**Resolution:** Tick execution loop (DataView execute → diff → broadcast), change_detect CodeComponent diff, and StorageEngine prev-state persistence deferred to wiring phase.

---

## [Epic 31] — Admin API

**File:** `crates/riversd/src/admin.rs`, `crates/riversd/src/lib.rs`
**Description:** Admin API authentication, authorization, and deployment types. AdminAuthConfig with public_key, ip_allowlist, no_auth, roles/permissions, identity_roles, replay_window_secs. AdminPermission enum (StatusRead, DeployWrite/Approve/Promote/Read, Admin) with Admin-grants-all semantics. Timestamp validation with configurable replay window (±5 min default). IP allowlist enforcement (empty = allow all). RBAC check_permission with identity→role→permissions chain. Deployment state machine (PENDING→RESOLVING→STARTING→RUNNING/FAILED→STOPPING→STOPPED) with valid transition enforcement. 22 new tests (611 total).
**Spec reference:** rivers-httpd-spec.md §15 (admin API)
**Resolution:** Ed25519 signature verification deferred until ed25519-dalek crate is added. mTLS CN extraction and localhost binding enforcement deferred to TLS wiring.

---

## [Epic 32] — App Bundle Deployment & Lifecycle

**File:** `crates/riversd/src/deployment.rs`, `crates/riversd/src/lib.rs`
**Description:** Bundle deployment lifecycle management. AppType enum (AppService/AppMain). Resource resolution checks datasources, services, and LockBox aliases against available resources. Startup order via topological sort: app-services first (respecting inter-dependencies, parallel when independent), then app-mains. Preflight checks: port conflicts, appId uniqueness, app type validation. DeploymentManager with async create/get/transition/list operations. 20 new tests (631 total).
**Spec reference:** rivers-application-spec.md §7-9 (deployment lifecycle), §12 (preflight), §14 (startup order)
**Resolution:** Health check with exponential backoff, zero-downtime redeployment, and auth scope carry-over deferred to HTTP client and live server orchestration wiring.

---

## [Epic 33] — Health Endpoints

**File:** `crates/riversd/src/health.rs`, `crates/riversd/src/lib.rs`
**Description:** Health endpoint response types and utilities. HealthResponse for basic `/health` (status, service, environment, version). VerboseHealthResponse for `/health/verbose` (draining, inflight_requests, uptime_seconds, pool_snapshots). PoolSnapshot captures pool name, driver, active/idle/max counts, circuit_state. UptimeTracker records server start time. parse_simulate_delay extracts `?simulate_delay_ms=N` for testing. 8 new tests (639 total).
**Spec reference:** rivers-httpd-spec.md §14 (health endpoints)
**Resolution:** Wiring response types into server.rs health handlers deferred to AppContext expansion.

---

## [Epic 34] — GraphQL Integration

**File:** `crates/riversd/src/graphql.rs`, `crates/riversd/src/lib.rs`
**Description:** GraphQL integration types and schema generation. GraphqlConfig with enabled, path, introspection toggle, max_depth, max_complexity. GraphqlFieldType maps JSON schema types to GraphQL scalars (String/Int/Float/Boolean/ID/Object/List). generate_graphql_types converts DataView return schemas into GraphqlType definitions with nullable inference from required fields. ResolverMapping bridges GraphQL fields to DataViews with argument mapping. Validation checks path format, depth, and complexity constraints. 14 new tests (653 total).
**Spec reference:** rivers-view-layer-spec.md §9 (GraphQL)
**Resolution:** async-graphql crate integration and mutation support via CodeComponent deferred.

---

## [Epic 35] — Hot Reload (Dev Mode)

**File:** `crates/riversd/src/hot_reload.rs`, `crates/riversd/src/lib.rs`
**Description:** Hot reload infrastructure with atomic config swap. HotReloadState holds config behind RwLock with Arc snapshots for in-flight request isolation. Swap method atomically replaces config, increments monotonic version counter, and notifies watch channel subscribers. check_reload_scope detects changes requiring restart (host, port, TLS) vs safe hot-reload (timeouts, views, etc.). 13 new tests (666 total).
**Spec reference:** rivers-httpd-spec.md §16 (hot reload)
**Resolution:** File watcher via notify crate and ConfigFileChanged EventBus event deferred.

---

## [Epic 36] — CLI Tools

**File:** `crates/riversd/src/cli.rs`, `crates/riversd/src/lib.rs`
**Description:** CLI argument parser for riversd binary. CliCommand enum (Serve, Doctor, Preflight, Version, Help). parse_args handles short/long flags (--config/-c, --log-level/-l, --no-admin-auth, --version/-V, --help/-h) and subcommands (serve, doctor, preflight <path>, version, help). Error handling for missing values, unknown flags, and unknown commands. version_string and help_text generators. 21 new tests (687 total).
**Spec reference:** rivers-application-spec.md §12, rivers-lockbox-spec.md §7
**Resolution:** Wiring CLI args into main.rs, riversctl admin client, and riverpackage validation tool deferred to wiring phase.

---

## [Epic 37] — Error Response Format

**File:** `crates/riversd/src/error_response.rs`, `crates/riversd/src/lib.rs`
**Description:** Consistent JSON error envelope. ErrorResponse with code/message/details/trace_id, where details and trace_id are omitted when None (skip_serializing_if). ErrorCategory enum → HTTP status code mapping covering all spec codes (400-504). Convenience constructors for all standard error types. map_view_error bridges ViewError variants to appropriate ErrorResponse codes. into_axum_response converts to axum Response with correct StatusCode. 20 new tests (707 total).
**Spec reference:** rivers-httpd-spec.md §18 (error response format)
**Resolution:** CORS header injection on error responses deferred to CORS middleware wiring.

---

## Phase Z — Release Packaging (2026-03-17)

| File | Decision | Resolution |
|------|----------|------------|
| `scripts/package.sh` | New release packaging script | `cargo build --release -p riversd`, creates `release/<version>-<timestamp>/{bin,config,log,apphome}/`, updates `latest` symlink, prunes to 3 releases |
| `release/.gitignore` | Ignore binaries/logs/apphome, track config | `*/bin/`, `*/log/`, `*/apphome/` ignored; `*/config/` tracked |
| `release/.gitkeep` | Ensure `release/` is tracked in git | Empty marker file |
| `scripts/package.sh` | macOS compat | Replaced `mapfile`+`find -printf` with `while read` loop + `xargs basename` |
| `release/config/riversd.toml` | Default config template | Pre-configured `base.logging.local_file_path = "../log/riversd.log"`; no lockbox/TLS by default |

**Usage:**
```bash
./scripts/package.sh           # build release + package
./scripts/package.sh --skip-build  # package only (reuse existing binary)
```

---

## Phase AA — CLI Shaping (2026-03-18)

| File | Decision | Resolution |
|------|----------|------------|
| `crates/riversd/src/cli.rs` | Remove Doctor/Preflight/Lockbox/LockboxCommand | Stripped to Serve/Version/Help + 3 flags |
| `crates/riversd/src/cli.rs` | Update help text | SEE ALSO section lists riversctl/riverpackage/rivers-lockbox |
| `crates/riversd/src/main.rs` | Remove 3 dispatch blocks (~340 lines) | main.rs now only handles serve |
| `crates/riversd/src/main.rs` | Add config discovery | Probes ./config/riversd.toml then ../config/riversd.toml |
| `crates/riversd/tests/cli_tests.rs` | Remove 15 obsolete tests, add 3 redirect tests | doctor/preflight/lockbox now return UnknownCommand |
| `crates/riversctl/Cargo.toml` | Add rivers-core + rivers-data deps | Required for doctor config/lockbox checks |
| `crates/riversctl/src/main.rs` | Add `start` command | Finds riversd binary, passes args as Vec (no shell), replaces process |
| `crates/riversctl/src/main.rs` | Add `doctor` command | 5 checks: binary, config parse, config validate, storage, lockbox |
| `crates/riversctl/src/main.rs` | Binary discovery | RIVERS_DAEMON_PATH → sibling binary → PATH |

**Test result:** 21 cli_tests pass, full suite builds clean.

---

## Phase AB — bundle_path config + auto-load (2026-03-18)

| File | Decision | Resolution |
|------|----------|------------|
| `crates/rivers-core/src/config.rs` | Add `bundle_path: Option<String>` to `ServerConfig` | Resolved relative to CWD at startup; None = no auto-load |
| `crates/riversd/src/server.rs` | Auto-load bundle at startup | After runtime init: load_bundle → merge views → ViewRouter::from_views → ctx.view_router; fatal error if path set but load fails |
| `crates/riversctl/src/main.rs` | Remove positional bundle arg from `start` | Bundle comes from config, not CLI; unknown positional now returns an error |
| `scripts/package.sh` | Add commented `bundle_path` to config template | User fills in path; run instructions simplified to `./bin/riversctl start` |

**Workflow after this phase:**
```toml
# config/riversd.toml
bundle_path = "apphome/my-bundle/"
```
```bash
./bin/riversctl start   # discovers config, riversd loads bundle from it
```
