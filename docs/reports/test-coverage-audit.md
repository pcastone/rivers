# Rivers Test Coverage Audit — v0.52.8

**Date:** 2026-04-03
**Total tests:** 1,940 across 27 crates
**Sources:** Feature inventory (21 features), bug report (38 bugs), codebase scan

---

## Coverage Summary

| Feature Area | Tests | Coverage | Gaps |
|-------------|-------|----------|------|
| 1. HTTP Server | 112 | **Good** | Security headers (1 test), backpressure (inline only) |
| 2. View System | 163 | **Good** | Pipeline stages (pre/post_process) not isolated |
| 3. Data Access | 26 | **POOR** | DataViews (0), Caching (0), Param binding (SDK only) |
| 4. Schema Validation | 0 | **NONE** | No validation chain tests |
| 6. Drivers | 327+ | **Good** | No cross-driver conformance matrix |
| 7. Auth & Sessions | 48 | **Good** | Cross-app propagation untested |
| 8. LockBox | 64 | **Good** | Rotation untested |
| 9. ProcessPool/V8 | 39 | **POOR** | No bridge contract tests, no sandbox security tests |
| 10. Polling | 31 | **Good** | — |
| 11. Storage Engine | 87 | **Good** | — |
| 12. EventBus | 15 | **Fair** | Gossip untested |
| 13. App Architecture | 48 | **Fair** | Boot parity untested, module resolution untested |
| 14-15. RPS/Cluster | 0 | **NONE** | Not yet implemented |
| 16. Logging | inline | **Fair** | No structured output verification |
| 17. Configuration | 5 | **POOR** | Env substitution, validation rules untested |
| 18. Admin API | 40 | **Good** | RBAC coverage light |
| 19. CORS | 14 | **Good** | — |
| **Security (bugs)** | **0** | **NONE** | DDL guard, V8 timeout/heap/codegen, timing-safe — all untested |

---

## Detailed Gap Analysis

### CRITICAL — No Tests At All

| Feature | Inventory Ref | What's Missing | Impact |
|---------|--------------|----------------|--------|
| **DataView engine** | 3.1 | No tests for DataView execution, param binding at engine level, query dispatch | Core data path untested |
| **Tiered cache** | 3.3 | No L1 LRU tests, no L2 tiered tests, no invalidation tests | Cache correctness unverified |
| **Schema validation** | 4.1-4.8 | No SchemaSyntaxChecker tests, no Validator tests, no per-driver validation | Entire validation chain untested |
| **V8 bridge contracts** | 9.8 | No ctx.* injection tests, no dataview bridge tests, no store namespace tests | Rust↔JS boundary untested |
| **V8 sandbox security** | 9.7 | No timeout test, no heap limit test, no codegen blocking test, no timing-safe test | All 4 security bugs were found by audit, not tests |
| **Config validation** | 17.4 | Only 5 tests in rivers-core-config, no env var substitution, no validation rules | Config errors reach production |
| **Boot path parity** | — | BUG-005: --no-ssl path was missing 7 subsystems, no test caught it | Dev mode completely broken |

### HIGH — Tests Exist But Gaps in Bug-Dense Areas

| Feature | Tests | Gap |
|---------|-------|-----|
| **Drivers — conformance** | 86 inline (per-driver) | No cross-driver matrix. Each driver tested in isolation. BUG-004 (param binding) affected ALL SQL drivers — per-driver tests didn't catch it. |
| **ProcessPool dispatch** | 27 tests | No test verifies ctx.app_id value (BUG-010), ctx.node_id (BUG-011), ctx.request.query field name (BUG-012) |
| **Module resolution** | 0 | BUG-013: module paths relative to CWD. Zero tests for path resolution. |
| **Driver DDL guard** | 19 (SDK level) | Tests in SDK only. No test verifies guard on actual driver Connection::execute() |
| **Store TTL** | via canary only | BUG-021: object vs number TTL. No unit test for argument type validation. |

### MEDIUM — Functional Coverage Weak

| Feature | Tests | Gap |
|---------|-------|-----|
| **Streaming REST** | 23 | No poison chunk test, no timeout test |
| **Cross-app session** | 0 | Feature 7.5: session propagation via X-Rivers-Claims untested |
| **EventBus as datasource** | 0 | Feature 12.4: EventBus driver publish/subscribe untested |
| **Pseudo DataViews** | 0 | Feature 3.2: ctx.datasource() builder untested |
| **Handler pipeline stages** | via view_engine | pre_process, handlers, post_process, on_error not tested in isolation |
| **RBAC** | 12 (admin_auth) | No test for deny-by-default on unknown paths (BUG-016) |
| **LockBox rotation** | 0 | Feature 8.7: rotation without restart |

---

## Feature-by-Feature Test Inventory

### Feature 1: HTTP Server (112 tests)

| Sub-feature | File | Tests | Status |
|-------------|------|-------|--------|
| 1.3 Static files | `static_files_tests.rs` | 32 | Covered |
| 1.4 Middleware — rate limit | `rate_limit_tests.rs` | 14 | Covered |
| 1.4 Middleware — CORS | `cors_tests.rs` | 14 | Covered |
| 1.5 Security headers | `server_tests.rs` | 1-2 | **Gap: needs dedicated tests** |
| 1.6 Error envelope | `error_response_tests.rs` | 22 | Covered |
| 1.7 Backpressure | `backpressure.rs` (inline) | 5 | Inline only |
| 1.8 Graceful shutdown | `server_tests.rs` | 29 | Covered |
| 1.9 Rate limiting | `rate_limit_tests.rs` | 14 | Covered |
| 1.10 Hot reload | `hot_reload_tests.rs` | 21 | Covered |

### Feature 2: View System (163 tests)

| Sub-feature | File | Tests | Status |
|-------------|------|-------|--------|
| 2.1 REST views | `view_engine_tests.rs` | 31 | Covered |
| 2.2 Pipeline stages | `view_engine_tests.rs` | (included) | **Gap: stages not isolated** |
| 2.4 WebSocket | `websocket_tests.rs` | 38 | Covered |
| 2.5 SSE | `sse_tests.rs` | 19 | Covered |
| 2.6 MessageConsumer | `message_consumer_tests.rs` | 13 | Covered |
| 2.7 Streaming REST | `streaming_tests.rs` | 23 | Covered |
| 2.8 GraphQL | 3 test files | 39 | Covered |

### Feature 3: Data Access (26 tests)

| Sub-feature | File | Tests | Status |
|-------------|------|-------|--------|
| 3.1 DataView engine | — | **0** | **CRITICAL GAP** |
| 3.2 Pseudo DataViews | — | **0** | **Gap** |
| 3.3 Tiered cache | — | **0** | **CRITICAL GAP** |
| 3.4 Connection pool | `pool_tests.rs` | 26 | Covered |

### Feature 6: Drivers (327+ tests)

| Sub-feature | File | Tests | Status |
|-------------|------|-------|--------|
| 6.1 Built-in (inline) | per-driver `src/*.rs` | 86 | Per-driver only |
| 6.2 HTTP driver | `http_driver_tests.rs` + `http_executor_tests.rs` | 49 | Covered |
| 6.3-6.4 Plugins | per-plugin `tests/` | 241 | Covered (with live tests) |
| 6.6 DDL guard | `ddl_guard_tests.rs` | 19 | SDK level only — **need driver-level tests** |
| 6.6 Param translation | `param_translation_tests.rs` | 8 | SDK level — **need cross-driver matrix** |
| Cross-driver conformance | — | **0** | **HIGH GAP** |

### Feature 7: Auth & Sessions (48 tests)

| Sub-feature | File | Tests | Status |
|-------------|------|-------|--------|
| 7.1 Guard view | `guard_csrf_tests.rs` | 28 | Covered |
| 7.2 Session lifecycle | `session_tests.rs` | 20 | Covered |
| 7.3 CSRF protection | `guard_csrf_tests.rs` | (included) | Covered |
| 7.5 Cross-app propagation | — | **0** | **Gap** |

### Feature 9: ProcessPool (39 tests)

| Sub-feature | File | Tests | Status |
|-------------|------|-------|--------|
| 9.1 V8 isolate pool | `rivers-engine-v8/src/lib.rs` | 12 | Basic |
| 9.4 Variable injection | — | **0** | **CRITICAL: no ctx.* tests** |
| 9.7 Timeout/heap/codegen | — | **0** | **CRITICAL: security untested** |
| 9.8 Handler context | `process_pool_tests.rs` | 27 | Dispatch-level only |
| 9.10 Rivers.* APIs | — | **0** | **Gap: no API contract tests** |

### Feature 11: Storage Engine (87 tests)

| Sub-feature | File | Tests | Status |
|-------------|------|-------|--------|
| 11.1 Backends | `storage_backends/src/` | 32 | Covered |
| 11.3 Namespace scoping | — | **0** | **Gap: via canary only** |
| 11.4 TTL/expiration | `storage_tests.rs` | 16+ | Covered |
| 11.5 Application KV | — | **0** | **Gap: via canary only** |

---

## Bug Coverage Map

| Bug | Has Unit Test? | Has Canary Test? | Needs Unit Test? |
|-----|---------------|-----------------|-----------------|
| BUG-001 DDL unchecked | SDK-level only | Not testable | **Yes — driver-level** |
| BUG-002 V8 timeout | No | Yes (HTTP 500) | **Yes** |
| BUG-003 V8 codegen | No | Yes (RT-V8-CODEGEN) | **Yes** |
| BUG-004 Param binding | SDK-level (8) | Yes (canary-sql) | **Yes — cross-driver** |
| BUG-005 --no-ssl init | No | Partially (boots) | **Yes — boot parity** |
| BUG-006 V8 heap | No | Yes (HTTP 500) | **Yes** |
| BUG-007 timingSafeEqual | No | Yes (RT-CRYPTO-TIMING) | **Yes** |
| BUG-008 dataview params | No | Yes (RT-CTX-DATAVIEW-PARAMS) | **Yes** |
| BUG-009 dataview namespace | No | Yes (RT-CTX-DATAVIEW) | **Yes** |
| BUG-010 app_id slug | No | Yes (RT-CTX-APP-ID) | **Yes** |
| BUG-011 node_id empty | No | Yes (RT-CTX-NODE-ID) | **Yes** |
| BUG-012 query field name | No | Yes (RT-CTX-REQUEST) | **Yes** |
| BUG-013 module paths | No | Indirectly | **Yes** |
| BUG-014-024 Security | No | Some via canary | **Partial** |

**0 of 13 critical/high bugs had a unit test before discovery.** All were caught by audits, user testing, or canary fleet.

---

## Recommendations

### Immediate (blocks next release)
1. **V8 bridge contract tests** — 0 tests for the Rust↔JS injection boundary that had 5 bugs
2. **V8 security tests** — timeout, heap, codegen blocking. These are security regressions waiting to happen.
3. **Boot parity test** — BUG-005 affected every dev user. One test prevents recurrence.

### High Priority
4. **Driver conformance matrix** — cross-driver tests using `#[test_case]`. Catches BUG-004 class bugs instantly.
5. **DataView engine tests** — 0 tests for the core query dispatch pipeline
6. **Cache tests** — 0 tests for L1 LRU, L2 tiered, invalidation

### Medium Priority
7. **Config validation tests** — only 5 tests for the entire config system
8. **Schema validation chain** — 0 tests across 3 stages
9. **Module resolution tests** — CWD-independent path resolution
10. **Store namespace enforcement** — unit-level (currently canary-only)

---

## Test Counts by Crate

| Crate | Tests | Notes |
|-------|-------|-------|
| riversd | 785 | Primary app — 26 test files + 18 inline modules |
| rivers-core | 210 | Storage, drivers, eventbus, lockbox integration |
| rivers-driver-sdk | 174 | HTTP driver, DDL guard, param translation, SDK |
| rivers-runtime | 163 | DataView engine, bundle loading, view config |
| rivers-plugin-exec | 103 | ExecDriver (most tested plugin) |
| rivers-keystore-engine | 98 | AES-256-GCM, key management |
| rivers-drivers-builtin | 86 | Per-driver inline tests |
| rivers-lockbox-engine | 58 | Age encryption, resolver |
| rivers-storage-backends | 32 | SQLite + Redis KV backends |
| rivers-plugin-influxdb | 30 | InfluxDB driver |
| rivers-plugin-mongodb | 25 | MongoDB driver |
| All other plugins | 109 | 10 plugins combined |
| Other crates | 67 | CLI tools, engine SDK, WASM, config |
| **Total** | **1,940** | |
