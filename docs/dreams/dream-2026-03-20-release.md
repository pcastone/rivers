# Dream - 2026-03-20 (Focus: What's Needed for Release)

## Summary

Rivers has ~1,175 passing tests across 17 crates with 16 drivers, mandatory TLS, LockBox credential encryption, and a complete TOML-to-runtime pipeline. The address-book bundle proves the zero-code contract end-to-end. But the project has a recurring pattern: **infrastructure gets built, tested in isolation, then never wired into the request pipeline**. This dream identifies every gap between "code exists" and "feature works" that must close before V1 release.

### What is Working

- **The declarative contract is real.** Bundle loading, DataView dispatch, REST routing, driver ecosystem, caching, cache invalidation, GraphQL queries, hot reload — all wired and tested.
- **Security posture is production-grade.** LockBox Age encryption, Ed25519 admin auth, CSRF, session lifecycle, capability-model SSRF prevention, zeroize-on-drop, mandatory TLS with auto-gen certs.
- **Test coverage is strong.** 1,175 tests, 48 test suites, 0 failures (1 pre-existing flaky redis-streams live test). JS/WASM execution, DataView dispatch, cache invalidation, GraphQL queries, hot reload, SSE channels, WebSocket hubs, polling loops — all have integration tests.
- **Bundle validation catches config mistakes at startup** — 9 validation checks prevent runtime surprises.

### What Can Be Done Better

- **`unwrap()` calls in the main request path** — `server.rs:652` in `view_dispatch_handler` will crash the server if body construction fails. Must be replaced with proper error handling before release.
- **ViewError status code mapping** — All view errors return 500 instead of correct codes (404 for NotFound, 405 for MethodNotAllowed, 422 for Validation).
- **Error envelope inconsistency** — success responses are raw JSON, error responses are wrapped in `ErrorResponse`. Clients see two shapes.
- **SSE Last-Event-ID** — `extract_last_event_id()` and `events_since()` exist but are never called in the SSE handler. Reconnecting clients miss events.

### What is NOT Working (Landmines)

- **Session/rate-limit middleware is NOT in the global stack** — `build_main_router()` has comment "pass-through — wired per-view at dispatch time" for rate limiting. Sessions are handled in `security_pipeline` per-view, not globally. This is architecturally intentional but contradicts the spec's middleware ordering (§4). Must be documented or aligned.
- **GraphQL mutations return `_noop` stub** — any user enabling GraphQL and trying mutations gets a placeholder. Subscriptions are completely unimplemented.
- **Redis Streams live test is flaky** — `redis_streams_produce_consume_roundtrip` fails on stale consumer group data from prior runs. Needs test isolation (unique group per run).

---

## Memory Consolidation

### Session-to-Session Progress (Phases AR → AU, this session)

| Phase | What Was Done | Tests Added |
|-------|---------------|-------------|
| **AR** | Wired cache invalidation (`invalidates` field), GraphQL endpoint (query + mutation stub), hot reload listener | 14 |
| **AS** | Integration tests for SSE, WebSocket, Streaming, Polling, Hot Reload, GraphQL | 30 |
| **AT** | Bundle validation: 9 checks (view types, driver names, schema files, invalidates targets, duplicates, service refs, TOML error context) | 14 |
| **AU** | JS/WASM gap coverage: file loading, Promise.all/race, ctx.dataview with executor, WASM computation | 18 |

**Total new tests this session: 76.** Test count went from ~1,100 to 1,175.

### Key Decisions Made This Session

1. **Cache invalidation is declarative, not heuristic.** The `invalidates` field on DataViewConfig lists which read views to invalidate — no need to detect "write" vs "read" operations.
2. **GraphQL mutations are V1 stub, subscriptions are V2.** Full CodeComponent dispatch for mutations requires ProcessPool wiring that's out of scope.
3. **Hot reload does NOT re-resolve LockBox or re-create pools.** Only view routing, DataView configs, and GraphQL schema are reloadable. New datasources require restart. This is documented.
4. **Driver name validation warns, doesn't block.** Unknown drivers may come from plugins loaded later.

---

## Code Archaeology

### Uncommitted Changes (45 files, +3,852 / -5,368 lines)

The current working tree has significant uncommitted work across 4 phases. Key files:

| File | Lines Changed | What |
|------|--------------|------|
| `rivers-data/src/validate.rs` | +100 | 6 new validation functions |
| `riversd/src/bundle_loader.rs` | +150 | Validation wiring, rebuild_views_and_dataviews |
| `riversd/src/graphql.rs` | +120 | build_schema_with_executor, mutation stub |
| `riversd/src/server.rs` | +60 | GraphQL route mounting, hot reload listener |
| `riversd/src/process_pool/mod.rs` | +250 | 13 new JS/WASM engine tests |
| `rivers-data/src/dataview_engine.rs` | +40 | Cache invalidation, EventBus integration |
| `rivers-data/src/dataview.rs` | +8 | `invalidates` field |
| `rivers-data/src/loader.rs` | +20 | `app_dir` field, error context |
| `rivers-core/src/config.rs` | +50 | GraphqlServerConfig |

### Architecture Pattern: Infrastructure Without Integration

This session's work explicitly targeted three instances of this pattern:
- **Cache invalidation**: `TieredDataViewCache::invalidate()` existed for 3 phases → now wired
- **GraphQL**: Schema builder, router, types all existed → now mounted and resolving DataViews
- **Hot reload**: FileWatcher + config swap existed → now triggers `rebuild_views_and_dataviews()`

The pattern is the project's biggest risk: something looks done in the code but isn't connected.

---

## Pattern Recognition

### The "Last 10%" Pattern

Rivers is at the stage where every remaining gap is an integration problem, not an implementation problem:

| Component | Infrastructure | Wiring | Status |
|-----------|---------------|--------|--------|
| Cache invalidation | `TieredDataViewCache` | `DataViewExecutor::run_cache_invalidation` | **DONE** (this session) |
| GraphQL queries | `build_dynamic_schema` | `build_main_router` mounting | **DONE** (this session) |
| Hot reload views | `FileWatcher` + `swap()` | `hot_reload_listener` task | **DONE** (this session) |
| Bundle validation | `validate_bundle()` | `load_and_wire_bundle()` call | **DONE** (this session) |
| SSE reconnection | `events_since()` | SSE handler (unused) | **NOT DONE** |
| GraphQL mutations | `build_mutation_type()` | CodeComponent dispatch | **STUB** |
| Polling persistence | `save_poll_state()` | Poll loop driver | **NOT WIRED** |
| Streaming generator | `run_streaming_generator()` | Frame yield loop | **PARTIAL** |

### Release Readiness Scorecard

| Category | Score | Notes |
|----------|-------|-------|
| **Core data path** | 9/10 | Request → View → DataView → Driver → Response is solid |
| **Security** | 9/10 | LockBox, TLS, CSRF, sessions, capability model all wired |
| **Test coverage** | 8/10 | 1,175 tests; JS/WASM execution verified; 1 flaky test |
| **Bundle validation** | 9/10 | 9 checks catch config errors at startup |
| **Error handling** | 5/10 | Unwrap() in request path; inconsistent status codes |
| **Streaming (SSE/WS)** | 6/10 | Channels work; reconnection and lifecycle hooks incomplete |
| **GraphQL** | 6/10 | Queries work; mutations stub; no subscriptions |
| **Documentation** | 4/10 | 22 spec docs exist but no user-facing guide or API reference |
| **Observability** | 5/10 | EventBus events, structured logging; no metrics/dashboards |
| **CLI tools** | 7/10 | riversctl, rivers-lockbox functional; riverpackage exists |

---

## Insights

### Release Blockers (Must Fix)

1. **Replace `unwrap()` on server.rs:652** — This is in `view_dispatch_handler`, the main request path. If Response::builder fails (malformed headers, body too large), the server panics. Replace with `.map_err()` → 500 response.

2. **Map ViewError to correct HTTP status codes** — NotFound→404, MethodNotAllowed→405, Validation→422, Handler→500, Pipeline→500. Currently all return 500.

3. **Fix the flaky redis-streams test** — Use unique consumer group per test run (`format!("test-{}", uuid)`) to avoid stale offset data.

### Release Recommended (Should Fix)

4. **Document session/rate-limit middleware model** — The per-view dispatch model is intentional and arguably better than global middleware. But it contradicts the spec. Either update the spec or add a sentence to the HTTPD spec explaining why.

5. **Wire SSE Last-Event-ID** — The code exists (`events_since()`). The handler extracts the header but ignores the variable. This is a 5-line fix to maintain an event buffer per channel and replay on reconnect.

6. **Decide GraphQL mutations scope** — Either implement real CodeComponent dispatch (2-3 days) or clearly mark mutations as V1.1 and remove the `_noop` stub (replace with disabled/error response explaining the limitation).

7. **Error envelope consistency** — Pick one format. The flat `{code, message, trace_id}` is already used by most error paths. Make success responses match or document the difference.

### V1.1 Candidates (After Release)

8. WebSocket lifecycle hooks (onConnect/onMessage/onDisconnect → CodeComponent)
9. Polling state persistence via StorageEngine
10. Streaming REST generator/yield semantics
11. Admin introspection endpoints (pool stats, cache rates, DataView latency)
12. GraphQL subscriptions via EventBus streams

---

## Art of the Possible

### 1. `riversctl validate` — Pre-deploy Config Linting

**What:** A CLI command that loads a bundle, runs all 9 validation checks, checks schema file existence, driver compatibility, and cross-app references — then prints a structured report without starting a server.

**Builds on:** `validate_bundle()`, `validate_known_drivers()`, `validate_schema_files()` all exist. Just need a CLI entrypoint that loads the factory for driver names.

**Complexity:** LOW (1-2 hours) | **Value:** HIGH — catches misconfigs before deploy

### 2. Bundle Diff for Hot Reload

**What:** When hot reload triggers, diff the old and new bundle configs and log exactly what changed: "2 new DataViews, 1 removed view, datasource 'pg' connection string changed (requires restart)."

**Builds on:** `HotReloadState::current_config()`, `rebuild_views_and_dataviews()`, `ReloadSummary`.

**Complexity:** MEDIUM (4-6 hours) | **Value:** MEDIUM — operations visibility

### 3. Health Check DataView Probes

**What:** `/health/verbose` already returns uptime and config. Add per-datasource connectivity probes: for each configured datasource, attempt a lightweight query (e.g., `SELECT 1`) and report latency + status.

**Builds on:** `DriverFactory::connect()`, `DataViewExecutor::datasource_info()`.

**Complexity:** MEDIUM (3-4 hours) | **Value:** HIGH — production readiness indicator

### 4. Config Schema Generation

**What:** Generate JSON Schema from `ServerConfig`, `AppConfig`, `DataViewConfig` structs. Publish alongside docs. IDEs auto-complete TOML configs. `riversctl validate --schema` outputs the schema.

**Builds on:** serde's `#[derive(Deserialize)]` — could use `schemars` crate to auto-derive JSON Schema.

**Complexity:** LOW (2-3 hours) | **Value:** HIGH — developer experience

### 5. Replay Mode for Debugging

**What:** Record all EventBus events to a log file during a request. `riversctl replay <log-file>` replays the events against a local bundle, showing exactly what each handler received and returned. Like a "flight recorder" for debugging production issues.

**Builds on:** EventBus publish already logs events. LogHandler writes structured JSON. Need: deserializer + replay harness.

**Complexity:** HIGH (2-3 days) | **Value:** HIGH — debugging in production
