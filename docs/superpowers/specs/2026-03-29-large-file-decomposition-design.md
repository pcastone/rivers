# Large File Decomposition Design

**Date:** 2026-03-29
**Branch:** `largefiles`
**Scope:** Split 20 files exceeding 600 LOC (excluding comments) into focused modules of 300-400 lines max.

## Approach

Responsibility-based decomposition. Each new module owns one concern. Tests split to mirror source modules. `lib.rs` / `mod.rs` files become thin re-export facades. No behavioral changes — pure structural refactor.

## Constraints

- Target 300-400 lines per module (some utility modules ~80-120 lines are acceptable)
- No public API changes — re-exports preserve existing import paths
- Tests split alongside source modules
- No logic changes, no bug fixes, no feature additions
- Each file split is one commit

---

## File Inventory (20 files, ~19,400 LOC)

| # | File | LOC | New Modules |
|---|------|-----|-------------|
| 1 | `riversd/src/server.rs` | 1,846 | 9 |
| 2 | `riversd/src/process_pool/engine_tests.rs` | 2,394 | 8 (7 + helpers) |
| 3 | `riversd/src/process_pool/v8_engine.rs` | 1,529 | 7 |
| 4 | `riversd/src/view_engine.rs` | 1,049 | 5 |
| 5 | `riversd/src/polling.rs` | 953 | 5 |
| 6 | `riversd/src/bundle_loader.rs` | 841 | 4 |
| 7 | `riversd/src/engine_loader.rs` | 669 | 4 |
| 8 | `riversd/src/graphql.rs` | 644 | 3 |
| 9 | `riversd/tests/graphql_tests.rs` | 758 | 3 |
| 10 | `rivers-lockbox-engine/src/lib.rs` | 1,343 | 7 + 4 test modules |
| 11 | `rivers-keystore-engine/src/lib.rs` | 975 | 4 + 3 test modules |
| 12 | `rivers-drivers-builtin/src/redis.rs` | 1,191 | 5 |
| 13 | `rivers-driver-sdk/src/http_executor.rs` | 1,075 | 5 + 2 test modules |
| 14 | `rivers-core-config/src/config.rs` | 762 | 5 |
| 15 | `riversctl/src/main.rs` | 756 | 5 |
| 16 | `rivers-plugin-exec/src/config.rs` | 733 | 3 |
| 17 | `rivers-plugin-exec/src/connection.rs` | 674 | 3 |
| 18 | `rivers-plugin-influxdb/src/lib.rs` | 704 | 4 |
| 19 | `rivers-engine-v8/src/lib.rs` | 649 | 3 |
| 20 | `rivers-runtime/tests/config_tests.rs` | 660 | 4 |

**Total: ~75 new modules created.**

---

## Detailed Decomposition

### 1. `riversd/src/server.rs` (1,846 lines -> 9 modules)

Current file becomes `server/mod.rs` (re-exports only).

| Module | ~Lines | Contents |
|--------|--------|----------|
| `server/context.rs` | 160 | `LogController`, `AppContext` struct + impl |
| `server/router.rs` | 160 | `build_main_router()`, `build_admin_router()`, middleware wiring |
| `server/view_dispatch.rs` | 280 | `MatchedRoute`, `combined_fallback_handler()`, `parse_query_string()` |
| `server/streaming.rs` | 350 | `build_streaming_response()`, `execute_sse_view()`, `execute_ws_view()` |
| `server/handlers.rs` | 120 | `health_handler`, `health_verbose_handler`, `gossip_receive_handler`, `static_file_handler`, `services_discovery_handler` |
| `server/admin_auth.rs` | 180 | `admin_auth_middleware()`, `path_to_admin_permission()`, RBAC config builder |
| `server/drivers.rs` | 100 | `register_all_drivers()` |
| `server/lifecycle.rs` | 400 | `run_server_with_listener_and_log()`, `run_server_no_ssl()`, entry points |
| `server/validation.rs` | 100 | `shutdown_signal()`, `validate_admin_access_control()`, `validate_server_tls()`, `maybe_spawn_hot_reload_watcher()`, `ServerError` |

**Dependency flow:** `context` -> `router` -> `view_dispatch`/`streaming`/`handlers` -> `lifecycle`. No circular deps.

---

### 2. `riversd/src/process_pool/engine_tests.rs` (2,394 lines -> 8 files)

| Module | ~Lines | Contents |
|--------|--------|----------|
| `process_pool/tests/helpers.rs` | 50 | `make_js_task()`, `make_http_js_task()`, shared fixtures |
| `process_pool/tests/basic_execution.rs` | 280 | Return values, args, duration, trace IDs, errors, exceptions |
| `process_pool/tests/crypto.rs` | 300 | `Rivers.crypto` -- randomHex, bcrypt, HMAC, base64url, timing-safe |
| `process_pool/tests/context_data.rs` | 340 | `ctx.dataview`, `ctx.store`, `ctx.datasource().build()`, DataViewExecutor |
| `process_pool/tests/http_and_logging.rs` | 310 | `Rivers.http`, `Rivers.env`, `Rivers.log`, capability gating |
| `process_pool/tests/wasm_and_workers.rs` | 365 | V8Worker/WasmtimeWorker config, WASM execution, isolate pool, script cache, TypeScript |
| `process_pool/tests/integration.rs` | 335 | JS/WASM gap tests, async/promises, complex types, file loading |
| `process_pool/tests/exec_and_keystore.rs` | 390 | ExecDriver subprocess tests, keystore encrypt/decrypt/AAD/tamper |

---

### 3. `riversd/src/process_pool/v8_engine.rs` (1,529 lines -> 7 modules)

Current file becomes `process_pool/v8/mod.rs` (re-exports `execute_js_task`, `ensure_v8_initialized`, `is_module_syntax`).

| Module | ~Lines | Contents |
|--------|--------|----------|
| `process_pool/v8/task_locals.rs` | 170 | All thread-locals, `TaskLocals` guard, setup/teardown |
| `process_pool/v8/init.rs` | 100 | `ensure_v8_initialized()`, isolate pool, script cache, heap limit callback |
| `process_pool/v8/execution.rs` | 335 | `execute_js_task()`, `call_entrypoint()`, ES module support, promise resolution, `resolve_module_source()` |
| `process_pool/v8/context.rs` | 340 | `inject_ctx_object()`, `inject_ctx_methods()`, `ctx_store_*` callbacks, `ctx_dataview_callback` |
| `process_pool/v8/datasource.rs` | 150 | `ctx_datasource_build_callback`, `json_to_query_value()` |
| `process_pool/v8/rivers_global.rs` | 380 | `inject_rivers_global()` -- log, crypto, keystore, env bindings |
| `process_pool/v8/http.rs` | 190 | HTTP verb callbacks, `do_http_request()`, header/response helpers, `json_to_v8`/`v8_to_json` |

---

### 4. `riversd/src/view_engine.rs` (1,049 lines -> 5 modules)

Current file becomes `view_engine/mod.rs`.

| Module | ~Lines | Contents |
|--------|--------|----------|
| `view_engine/types.rs` | 110 | `ParsedRequest`, `StoreHandle`, `ViewContext`, `ViewResult`, `ViewError` |
| `view_engine/router.rs` | 250 | `ViewRouter`, `ViewRoute`, `PathSegment`, `parse_path_pattern()`, `build_namespaced_path()` |
| `view_engine/pipeline.rs` | 320 | `execute_rest_view()`, `apply_parameter_mapping()`, `json_value_to_query_value()`, `serialize_view_result()` |
| `view_engine/validation.rs` | 250 | `validate_views()`, `execute_on_error_handlers()`, `execute_on_session_valid()`, `validate_input/output()`, `parse_handler_view_result()` |
| `view_engine/tests.rs` | 320 | All test functions + `make_none_handler_view()` helper |

---

### 5. `riversd/src/polling.rs` (953 lines -> 5 modules)

Current file becomes `polling/mod.rs`.

| Module | ~Lines | Contents |
|--------|--------|----------|
| `polling/diff.rs` | 225 | `DiffStrategy`, `DiffResult`, `compute_diff()`, hash helpers |
| `polling/state.rs` | 220 | `PollLoopKey`, `PollLoopState`, `PollLoopRegistry`, `PollError`, `DataViewPollExecutor` trait + adapter |
| `polling/executor.rs` | 150 | `PollTickResult`, `execute_poll_tick()`, `run_poll_tick_and_broadcast()`, storage persistence fns |
| `polling/runner.rs` | 195 | `execute_poll_tick_inmemory()`, `dispatch_change_detect()`, `run_poll_loop_inmemory()` |
| `polling/tests.rs` | 140 | All unit/integration tests |

---

### 6. `riversd/src/bundle_loader.rs` (841 lines -> 4 modules)

Current file becomes `bundle_loader/mod.rs`.

| Module | ~Lines | Contents |
|--------|--------|----------|
| `bundle_loader/types.rs` | 50 | `SseTriggerHandler`, `DatasourceEventBusHandler`, `ReloadSummary` |
| `bundle_loader/load.rs` | 350 | `load_and_wire_bundle()` setup phase -- parsing, LockBox, keystore, datasource params, driver factory, cache |
| `bundle_loader/wire.rs` | 350 | `load_and_wire_bundle()` wiring phase -- GraphQL, guards, brokers, SSE/WS registration |
| `bundle_loader/reload.rs` | 200 | `rebuild_views_and_dataviews()`, `build_cache_policy_from_bundle()` |

**Split point in `load_and_wire_bundle()`:** After line ~420, after GraphQL/guard setup, before streaming view wiring. Setup phase is stateless; wiring phase subscribes events.

---

### 7. `riversd/src/engine_loader.rs` (669 lines -> 4 modules)

Current file becomes `engine_loader/mod.rs`.

| Module | ~Lines | Contents |
|--------|--------|----------|
| `engine_loader/loaded_engine.rs` | 100 | `LoadedEngine` struct, `execute()`, `cancel()` |
| `engine_loader/registry.rs` | 80 | Global engine registry, `get`/`list`/`execute` functions |
| `engine_loader/loader.rs` | 150 | Directory scanning, single engine loading, ABI checks |
| `engine_loader/host_context.rs` | 200 | `HostContext`, `set_host_context()`, keystore/subsystem wiring |

---

### 8. `riversd/src/graphql.rs` (644 lines -> 3 modules)

Current file becomes `graphql/mod.rs`.

| Module | ~Lines | Contents |
|--------|--------|----------|
| `graphql/config.rs` | 80 | `GraphqlConfig`, defaults, `From` conversion |
| `graphql/types.rs` | 160 | `ResolverMapping`, `GraphqlType`, `GraphqlField`, `GraphqlFieldType`, schema generation, `to_pascal_case()` |
| `graphql/schema_builder.rs` | 250 | `build_dynamic_schema()`, field resolvers, `json_to_gql_value()` |

---

### 9. `riversd/tests/graphql_tests.rs` (758 lines -> 3 test modules)

| Module | ~Lines | Contents |
|--------|--------|----------|
| `tests/graphql/config_tests.rs` | 120 | Config parsing, field types, validation |
| `tests/graphql/schema_tests.rs` | 280 | Schema generation, dynamic building, conversion, resolvers |
| `tests/graphql/integration_tests.rs` | 350 | Executor, mutations, subscriptions, introspection |

---

### 10. `rivers-lockbox-engine/src/lib.rs` (1,343 lines -> 7 modules + 4 test modules)

Current file becomes thin re-export facade (~30 lines).

| Module | ~Lines | Contents |
|--------|--------|----------|
| `types.rs` | 160 | `LockBoxError`, `Keystore`, `KeystoreEntry`, `EntryType`, Zeroize/Drop impls |
| `validation.rs` | 80 | `validate_entry_name()`, `parse_lockbox_uri()`, `is_lockbox_uri()` |
| `resolver.rs` | 280 | `EntryMetadata`, `ResolvedEntry`, `LockBoxResolver` struct + all query methods |
| `crypto.rs` | 200 | `decrypt_keystore()`, `encrypt_keystore()` |
| `key_source.rs` | 160 | `resolve_key_source()`, `check_file_permissions()` (Unix/non-Unix) |
| `secret_access.rs` | 100 | `fetch_secret_value()` |
| `startup.rs` | 230 | `LockBoxReference`, `collect_lockbox_references()`, `resolve_all_references()`, `startup_resolve()` |

**Test modules:**

| Module | ~Lines | Contents |
|--------|--------|----------|
| `tests/crypto_tests.rs` | 150 | Encrypt/decrypt roundtrip, edge cases |
| `tests/resolver_tests.rs` | 180 | Lookup, metadata, entry queries |
| `tests/key_source_tests.rs` | 150 | Key source resolution, file permissions |
| `tests/startup_tests.rs` | 180 | Integration: startup_resolve, collect/resolve references |

---

### 11. `rivers-keystore-engine/src/lib.rs` (975 lines -> 4 modules + 3 test modules)

Current file becomes re-export facade + `create_test_keystore()` helper (~50 lines).

| Module | ~Lines | Contents |
|--------|--------|----------|
| `types.rs` | 120 | `AppKeystoreError`, `AppKeystore`, `AppKeystoreKey`, `KeyVersion`, `KeyInfo`, `EncryptResult`, Zeroize/Drop, constants |
| `io.rs` | 100 | `create()`, `load()`, `save()` -- Age encryption + TOML I/O |
| `key_management.rs` | 300 | `generate_key`, `rotate_key`, `delete_key`, `get_key`, `get_key_version`, `has_key`, `key_info`, `list_keys`, `current_key_bytes`, `versioned_key_bytes` |
| `crypto.rs` | 150 | Standalone `encrypt()`/`decrypt()` + convenience wrappers |

**Test modules:**

| Module | ~Lines | Contents |
|--------|--------|----------|
| `tests/io_tests.rs` | 150 | Create/load/save roundtrip, file permissions |
| `tests/key_management_tests.rs` | 280 | Generate, rotate, delete, metadata, versioning |
| `tests/crypto_tests.rs` | 230 | Encrypt/decrypt with/without AAD, tampered data, wrong key |

---

### 12. `rivers-drivers-builtin/src/redis.rs` (1,191 lines -> 5 modules)

Current file becomes `redis/mod.rs`.

| Module | ~Lines | Contents |
|--------|--------|----------|
| `redis/driver.rs` | 110 | `RedisDriver` struct, `DatabaseDriver` impl, `connect()` |
| `redis/single.rs` | 380 | `RedisConnection` -- all 16 operations |
| `redis/cluster.rs` | 380 | `RedisClusterConnection` -- same 16 operations for cluster |
| `redis/validation.rs` | 160 | `Driver` impl, `check_schema_syntax()` |
| `redis/params.rs` | 120 | `inject_params_from_statement()`, `get_str_param()`, `get_int_param()`, `get_keys_param()`, `single_value_row()` |

**Note:** `single.rs` and `cluster.rs` are 99% identical. A future pass could deduplicate via generic `execute_operation<C: AsyncCommands>()` but that is out of scope for this refactor.

---

### 13. `rivers-driver-sdk/src/http_executor.rs` (1,075 lines -> 5 modules + 2 test modules)

Current file becomes `http/mod.rs` or stays as `http_executor.rs` with submodules.

| Module | ~Lines | Contents |
|--------|--------|----------|
| `http/circuit_breaker.rs` | 100 | `CircuitState`, `CircuitBreaker` struct + impl |
| `http/oauth2.rs` | 220 | `CachedToken`, `OAuth2Credentials`, `TokenResponse`, `fetch_oauth2_token()`, auth resolution |
| `http/connection.rs` | 280 | `ReqwestHttpConnection` struct, request building, retry logic, `execute()` |
| `http/sse_stream.rs` | 85 | `SseStreamConnection`, `parse_sse_event()` |
| `http/driver.rs` | 250 | `ReqwestHttpDriver` struct + impls, `build_connection()` factory |

**Test modules:**

| Module | ~Lines | Contents |
|--------|--------|----------|
| `http/tests/circuit_breaker_tests.rs` | 120 | State transitions, failure tracking |
| `http/tests/connection_tests.rs` | 160 | Retry, OAuth2, request building |

---

### 14. `rivers-core-config/src/config.rs` (762 lines -> 5 modules)

Current file becomes `config/mod.rs` (re-exports all public types).

| Module | ~Lines | Contents |
|--------|--------|----------|
| `config/server.rs` | 170 | `ServerConfig` root, `BaseConfig`, `BackpressureConfig`, `Http2Config`, defaults |
| `config/tls.rs` | 180 | TLS config, x509, engine, cipher suites, admin TLS, redirect |
| `config/security.rs` | 220 | CORS, rate limiting, session, CSRF, admin RBAC |
| `config/storage.rs` | 120 | `StorageEngineConfig`, cache, retention, sweep |
| `config/runtime.rs` | 250 | Process pools, environment overrides, logging, engines/plugins dirs, GraphQL, static files |

---

### 15. `riversctl/src/main.rs` (756 lines -> 5 modules)

| Module | ~Lines | Contents |
|--------|--------|----------|
| `main.rs` | 60 | Entry point, command dispatch |
| `commands/start.rs` | 120 | `start` command, `launch_riversd()`, binary discovery |
| `commands/doctor.rs` | 110 | Pre-launch health checks (binary, config, bundle, paths) |
| `commands/validate.rs` | 40 | Bundle schema validation |
| `commands/admin.rs` | 280 | All admin API commands (status, deploy, drivers, datasources, health, stop, graceful, log) |

---

### 16. `rivers-plugin-exec/src/config.rs` (733 lines -> 3 modules)

Current file becomes `config/mod.rs` (re-exports).

| Module | ~Lines | Contents |
|--------|--------|----------|
| `config/types.rs` | 150 | `ExecConfig`, `CommandConfig`, `IntegrityMode`, `InputMode`, parsing impls |
| `config/parser.rs` | 170 | `ExecConfig` parsing from `ConnectionParams`, `parse_commands()`, `parse_indexed_list()`, `parse_env_set()` |
| `config/validator.rs` | 170 | All validation: user checks, working_dir, path, sha256, args_template, stdin_key |

Tests (~250 lines) distribute into each module.

---

### 17. `rivers-plugin-exec/src/connection.rs` (674 lines -> 3 modules)

| Module | ~Lines | Contents |
|--------|--------|----------|
| `connection/driver.rs` | 100 | `ExecDriver` impl, `CommandRuntime`, connection factory, startup checks |
| `connection/pipeline.rs` | 250 | 11-step execution pipeline, semaphore logic, integrity check, result mapping |
| `connection/exec_connection.rs` | 50 | `ExecConnection` struct, `execute()` dispatch, `ping()` |

---

### 18. `rivers-plugin-influxdb/src/lib.rs` (704 lines -> 4 modules)

| Module | ~Lines | Contents |
|--------|--------|----------|
| `driver.rs` | 80 | `InfluxDriver` struct, trait impl, connection factory |
| `connection.rs` | 160 | `InfluxConnection`, execute/query/write methods |
| `batching.rs` | 130 | `BatchingInfluxConnection`, buffer management, Drop impl |
| `protocol.rs` | 180 | Line protocol building, CSV parsing, escaping, field formatting |

`lib.rs` keeps plugin ABI exports + module declarations (~30 lines). Tests distribute to each module.

---

### 19. `rivers-engine-v8/src/lib.rs` (649 lines -> 3 modules)

| Module | ~Lines | Contents |
|--------|--------|----------|
| `task_context.rs` | 100 | Thread-locals, setup/clear, task environment |
| `v8_runtime.rs` | 130 | V8 init, script cache, isolate pool, helpers |
| `execution.rs` | 280 | Core `execute_js`, context injection, script compilation, entrypoint call |

`lib.rs` keeps C-ABI exports + module declarations (~50 lines).

---

### 20. `rivers-runtime/tests/config_tests.rs` (660 lines -> 4 test modules)

| Module | ~Lines | Contents |
|--------|--------|----------|
| `tests/server_config_tests.rs` | 180 | ServerConfig parsing, validation, overrides |
| `tests/app_config_tests.rs` | 100 | App config parsing, validation, datasources |
| `tests/bundle_tests.rs` | 100 | Bundle manifest, resources, cache config |
| `tests/schema_tests.rs` | 80 | JSON schema generation |

---

## Execution Order

Split files in dependency order -- leaf crates first, then crates that depend on them:

1. **Phase 1 -- Leaf crates (no internal dependents):**
   - `rivers-core-config` (#14)
   - `rivers-lockbox-engine` (#10)
   - `rivers-keystore-engine` (#11)
   - `rivers-driver-sdk` (#13)
   - `rivers-engine-v8` (#19)

2. **Phase 2 -- Driver plugins:**
   - `rivers-drivers-builtin/redis` (#12)
   - `rivers-plugin-exec` (#16, #17)
   - `rivers-plugin-influxdb` (#18)

3. **Phase 3 -- Runtime tests:**
   - `rivers-runtime/tests` (#20)

4. **Phase 4 -- `riversd` (depends on all above):**
   - `graphql` (#8)
   - `engine_loader` (#7)
   - `view_engine` (#4)
   - `polling` (#5)
   - `bundle_loader` (#6)
   - `server` (#1)
   - `process_pool/v8_engine` (#3)
   - `process_pool/engine_tests` (#2)
   - `tests/graphql_tests` (#9)

5. **Phase 5 -- CLI:**
   - `riversctl` (#15)

## Verification

After each file split:
1. `cargo check` -- confirms compilation
2. `cargo test -p <crate>` -- confirms tests pass
3. Commit with message: `refactor(<crate>): split <file> into <N> modules`

After all splits:
1. `cargo build` -- full workspace build
2. `cargo test` -- full test suite
3. Verify no file exceeds 400 lines (excluding generated code)

## Summary

| Metric | Value |
|--------|-------|
| Files being split | 20 |
| New modules created | ~75 |
| Average module size | ~180 lines |
| Largest new module | ~400 lines |
| Smallest new module | ~40 lines |
| Phases | 5 |
| Commits | ~20 (one per file split) |
