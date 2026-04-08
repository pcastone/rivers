# Missing Wiring Review

Date: 2026-04-07

This note captures a static analysis pass for functions that appear to exist in production code but are not called anywhere else in the Rust codebase.

Method used:

- Scanned production code under `crates/`
- Excluded docs, bugs, canary bundles, examples, and test-only code
- Re-checked each candidate with exact-name searches across `crates/`
- Kept only high-confidence cases where the symbol search returned the definition site only

This is still a static analysis result. It will not catch string-based dispatch, generated references, or macro-expanded call paths that do not preserve the function name in source. That said, the items below are strong candidates for missing integration work.

## Probably Missing Wiring

| File | Line | Function | Why it looks like missing wiring |
|---|---:|---|---|
| `crates/riversd/src/middleware.rs` | 163 | `session_middleware` | This is core request-path behavior, not a helper. It extracts cookie and bearer session IDs, validates the session, injects session state into request extensions, and clears invalid cookies. If the server intends to support centralized session handling, this should be attached to the request pipeline. No router or middleware layer wiring was found. |
| `crates/riversd/src/middleware.rs` | 223 | `rate_limit_middleware` | This is a complete token-bucket middleware with trusted-proxy and client IP resolution logic. It looks like a real runtime feature rather than an abandoned helper. No call site or router-layer integration was found, which strongly suggests the rate-limit path was implemented but never attached. |
| `crates/riversd/src/guard.rs` | 307 | `execute_guard_lifecycle_hook` | The function executes side-effect lifecycle hooks for guard flow. The surrounding guard system is active, so a dedicated lifecycle executor with no callers suggests the lifecycle phase was designed but never integrated into the guard pipeline. |
| `crates/riversd/src/deployment.rs` | 502 | `execute_init_handler` | This function dispatches an app init handler before the app accepts traffic. The implementation is substantial and matches a startup feature, not a utility. No startup or deployment path invoking it was found, so init handlers likely exist in code but are not actually executed. |
| `crates/riversd/src/pool.rs` | 693 | `spawn_health_check_task` | This creates a background task that periodically runs connection-pool health checks. Since health checks are typically started during pool creation or runtime boot, the lack of any caller suggests the health monitoring loop was implemented but never enabled. |
| `crates/riversd/src/runtime.rs` | 59 | `gossip_forward_http` | This is concrete cluster transport logic for forwarding gossip events to peer nodes. It does not look like dead utility code. The absence of call sites suggests gossip forwarding was planned or partially implemented but never connected to the event propagation path. |
| `crates/riversd/src/server/metrics.rs` | 6 | `record_request` | This helper records HTTP request counters and latency histograms. No request-path code calls it, so the request metrics integration appears unwired. |
| `crates/riversd/src/server/metrics.rs` | 12 | `set_active_connections` | This helper updates an active-connections gauge. No networking or connection-management path calls it, so the gauge appears defined but unused. |
| `crates/riversd/src/server/metrics.rs` | 17 | `record_engine_execution` | This helper records execution count and latency for runtime engines. No engine dispatch path calls it, suggesting instrumentation was started but not integrated. |
| `crates/riversd/src/server/metrics.rs` | 23 | `set_loaded_apps` | This helper updates a loaded-apps gauge. No startup or hot-reload path was found that updates it, so this metric appears unwired. |

## Lower-Risk Unused APIs

These also appear unused by exact-name search, but they look more like convenience wrappers or optional validation helpers than missing runtime wiring.

| File | Line | Function | Notes |
|---|---:|---|---|
| `crates/rivers-runtime/src/schema.rs` | 151 | `parse_schema_value` | Looks like a convenience variant of `parse_schema()` for callers that already have a JSON value. |
| `crates/rivers-runtime/src/loader.rs` | 45 | `load_app_manifest` | `load_bundle()` currently uses a generic `load_and_parse()` path instead. |
| `crates/rivers-runtime/src/loader.rs` | 51 | `load_resources_config` | Same pattern as `load_app_manifest`. |
| `crates/rivers-runtime/src/loader.rs` | 57 | `load_app_config` | Same pattern as `load_app_manifest`. |
| `crates/rivers-core-config/src/config/security.rs` | 108 | `validate_ddl_whitelist` | Startup/config validation helper exists, but no startup validation path appears to call it. This may still be worth wiring in, but it is less urgent than request-pipeline or runtime-bootstrap gaps. |

## Resolution Status (2026-04-07)

### Wired

| Function | Resolution |
|---|---|
| `record_request` | Wired into `request_observer_middleware` — `#[cfg(feature = "metrics")]` |
| `set_active_connections` | Wired via `AtomicUsize` in `request_observer_middleware` |
| `record_engine_execution` | Wired after `execute_rest_view` with handler-type labels (v8/dataview/none) |
| `set_loaded_apps` | Wired in `bundle_loader/load.rs` and `bundle_loader/reload.rs` |
| `rate_limit_middleware` | Per-view rate limiting wired in `view_dispatch_handler` with proxy-aware IP |
| `execute_guard_lifecycle_hook` | Replaced by inline `tokio::spawn` in `security_pipeline.rs` and `view_dispatch.rs` |
| `validate_ddl_whitelist` | Wired at startup in `lifecycle.rs`, warnings logged |

### Already Wired (discovered during review)

| Function | Resolution |
|---|---|
| `session_middleware` | Superseded by `security_pipeline::run_security_pipeline` (per-view session handling) |
| `execute_init_handler` | Superseded by `dispatch_init_handler` in `bundle_loader/load.rs:384` |

### Dead Code Removed

| Function | Resolution |
|---|---|
| `execute_init_handler` (deployment.rs) | Removed — superseded by `dispatch_init_handler` |
| `execute_guard_lifecycle_hook` (guard.rs) | Removed — replaced by inline `tokio::spawn` dispatch |

### Deferred (infrastructure not ready)

| Function | Why |
|---|---|
| `spawn_health_check_task` | `ConnectionPool` instances never created — pool infra exists but not connected to driver resolution |
| `gossip_forward_http` | Requires cluster peer discovery config (not implemented) |

### Kept As-Is (convenience APIs)

| Function | Decision |
|---|---|
| `parse_schema_value` | Public API — may be useful for downstream consumers |
| `load_app_manifest` / `load_resources_config` / `load_app_config` | Convenience wrappers over `parse_toml` — harmless, may be useful |

### DDL Whitelist Gate Added

`is_ddl_permitted` now called in `host_ddl_execute` (Gate 3). Whitelist stored in `DDL_WHITELIST` OnceLock, set at startup. Rejects DDL with `-4` error code and JSON error response.
