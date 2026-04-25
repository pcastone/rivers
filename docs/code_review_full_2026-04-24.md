# Rivers Full Code Review

Date: 2026-04-24

Scope requested: security, V8 JavaScript/TypeScript execution, database drivers, connection pooling, event bus, storage engine, datasource wiring for handlers, dataview, and view function wiring.

This review was performed as a static code review of the current worktree. I did not make production code changes. The most important theme is that several reliability and security controls exist in isolation, but the hot request and startup paths often bypass them. That makes canary failures harder to interpret because the system can appear configured while the runtime is still doing direct connects, synchronous broker startup, in-memory storage fallback, or context-less handler dispatch.

## Executive Summary

The highest-risk items are:

1. Protected views can fail open if session management is missing or miswired.
2. `ctx.ddl()` is injected in the V8 context without an obvious task-type guard in the host callback path.
3. DataView execution bypasses the `ConnectionPool` entirely and connects directly on every call.
4. Broker consumer creation is synchronous during bundle load, so one unreachable broker can prevent `riversd` from booting.
5. Message consumer and some lifecycle dispatch paths lose app identity, which breaks storage namespaces and datasource wiring.
6. Storage failures in `ctx.store` can silently fall back to task-local memory, creating false success and data loss.
7. Module cache misses can fall through to disk and live TypeScript compilation, bypassing bundle validation.
8. Several error paths expose absolute host filesystem paths.

## Severity Legend

- P0: can block production boot, fail open security, or allow broad data/host impact.
- P1: high likelihood production incident, data loss, contract violation, or serious operability issue.
- P2: correctness, reliability, performance, or hardening issue that should be scheduled.

## P0 Findings

### P0-1: Protected views fail open when session management is absent

Files:
- `crates/riversd/src/security_pipeline.rs`

`run_security_pipeline` rejects missing sessions only when a `session_manager` exists. If a view is not public but `ctx.session_manager` is `None`, the function does not fail closed and execution can continue.

Impact:
- A production misconfiguration can turn protected routes into public routes.
- This is especially dangerous because it presents as a wiring/config issue rather than an explicit auth failure.

Recommendation:
- Treat `!view.public && session_manager.is_none()` as an immediate 500 or 401/403 fail-closed condition.
- Add an integration test with a non-public view and no session manager.
- Consider startup validation that refuses bundles containing protected views unless session management is configured.

### P0-2: `ctx.ddl()` appears injectable for all V8 handlers

Files:
- `crates/riversd/src/process_pool/v8_engine/context.rs`
- `crates/riversd/src/view_engine/pipeline.rs`

`inject_ctx_methods` installs `ctx.ddl` generally, and `ctx_ddl_callback` uses the driver factory and datasource parameters directly. The reviewed callback does not appear to verify that the current task is an application init hook or that the requested DDL is allowed for the current phase.

Impact:
- If this V8 context path is reachable by normal request handlers, application code can execute DDL outside the init lifecycle.
- That violates the expected separation between handler execution and schema/bootstrap operations.

Recommendation:
- Gate `ctx.ddl()` in the host callback itself, not only at higher layers.
- Allow DDL only for an explicit `ApplicationInit` task kind with an app id and datasource id bound into the task context.
- Add a negative integration test proving that REST handlers, message consumers, validation hooks, and security hooks cannot call `ctx.ddl()`.

### P0-3: DataView execution bypasses the connection pool

Files:
- `crates/rivers-runtime/src/dataview_engine.rs`
- `crates/riversd/src/pool.rs`
- `crates/riversd/src/server/handlers.rs`

`DataViewExecutor::execute` calls `self.factory.connect(driver_name, ds_params).await` directly for datasource queries. The `ConnectionPool` and `PoolManager` implementations are not used on the production DataView path. The verbose health handler also reports an empty pool snapshot.

Impact:
- Pool limits, idle reuse, health checks, circuit breaking, and observability do not apply to the main query path.
- MySQL and PostgreSQL can perform a full connection handshake for each dataview call.
- Canary failures can look like driver instability when the actual issue is the missing pool integration.

Recommendation:
- Make DataView execution acquire connections through a datasource-scoped pool.
- Move pool ownership into the application runtime context so handlers, DataViews, health, and lifecycle hooks share the same path.
- Add a canary or integration test that verifies repeated DataView calls reuse pool state and obey max connection limits.

### P0-4: Broker consumer startup blocks bundle load

Files:
- `crates/riversd/src/bundle_loader/wire.rs`
- `crates/rivers-plugin-kafka/src/lib.rs`

`wire_streaming_and_events` awaits `broker_driver.create_consumer(...).await` inline. The Kafka implementation performs async client and partition setup during `create_consumer`. If a broker route is flaky or unavailable, bundle load never completes and the HTTP listener never binds.

Impact:
- One broker outage can prevent the whole node from starting.
- This matches the current canary blocker where `rskafka` reports `No route to host` while basic TCP probes can still succeed.

Recommendation:
- Spawn the consumer bridge supervisor immediately during wiring.
- Move `create_consumer` into that supervisor loop with retry/backoff and health reporting.
- Surface broker readiness separately from process readiness so `riversd` can boot while broker consumers recover.

## P1 Findings

### P1-1: ConnectionPool accounting and lifetime logic are unsafe if enabled

Files:
- `crates/riversd/src/pool.rs`

`PoolGuard::drop` returns a `PooledConnection` with a fresh `created_at`, so `max_lifetime` is effectively reset on every checkout. `acquire` counts active and idle connections but not the `idle_return` queue, which can undercount total connections and allow over-creation under load. `PoolManager` stores pools in a `Vec`, allowing duplicate datasource ids and O(n) lookup.

Impact:
- Long-lived connections may never expire.
- Burst traffic can exceed configured pool limits.
- Pool behavior will be difficult to reason about once DataView starts using it.

Recommendation:
- Preserve original connection creation time through guard return.
- Include the return queue in capacity accounting or use a single synchronized pool state.
- Replace `Vec<Arc<ConnectionPool>>` with a `HashMap<String, Arc<ConnectionPool>>` keyed by datasource id.

### P1-2: Message consumer and lifecycle dispatch lose app identity

Files:
- `crates/riversd/src/message_consumer.rs`
- `crates/riversd/src/security_pipeline.rs`
- `crates/riversd/src/view_engine/validation.rs`
- `crates/riversd/src/process_pool/v8_engine/task_locals.rs`

Message consumer dispatch enriches task context with an empty app id. Some security and validation lifecycle hooks do the same. `TaskLocals::set` falls back to `"app:default"` when the app id is empty.

Impact:
- `ctx.store`, lockbox, datasource, logs, and event wiring can land in the wrong namespace.
- Consumer writes may not be visible to HTTP handlers for the intended app.
- This explains the observed Kafka consumer to store verification gap.

Recommendation:
- Centralize handler dispatch through a helper that always binds app id, view id, handler id, datasource map, storage engine, lockbox, and driver factory consistently.
- Add integration tests for REST, ApplicationInit, security hooks, validation hooks, and MessageConsumer that assert `ctx.store` and datasource access use the app namespace.

### P1-3: Kafka producer violates the OutboundMessage destination contract

Files:
- `crates/rivers-runtime/src/dataview_engine.rs`
- `crates/rivers-plugin-kafka/src/lib.rs`

`execute_broker_produce` builds an `OutboundMessage` with `destination` from the dataview statement, but Kafka `create_producer` resolves and binds the topic at producer creation time. `publish` then ignores `message.destination`.

Impact:
- The runtime contract says the message destination controls routing, but Kafka routing is fixed at producer creation.
- Producer creation performs metadata work in the request path.
- Dynamic per-message destinations cannot work correctly.

Recommendation:
- Make producer initialization lazy and nonblocking, but route per `OutboundMessage.destination`.
- Cache partition clients by topic inside the producer with bounded TTL/backoff.
- Add tests proving two messages from one producer can publish to two destinations.

### P1-4: EventBus wildcard subscribers can violate priority ordering

Files:
- `crates/rivers-core/src/eventbus.rs`

Exact subscribers are sorted by priority, but wildcard subscribers are appended afterward. If a wildcard subscriber has `Expect` or `Handle` priority, it can run after exact `Emit` or `Observe` handlers.

Impact:
- Event validation and handling order can be wrong.
- The bug will be intermittent because it depends on exact versus wildcard subscription mix.

Recommendation:
- Collect exact and wildcard subscribers into one list, then sort globally by priority before dispatch.
- If wildcard subscribers must be observe-only, enforce that at subscribe time.

### P1-5: `ctx.store` masks persistent storage failures with in-memory fallback

Files:
- `crates/riversd/src/process_pool/v8_engine/context.rs`

The V8 store callbacks attempt persistent storage, but on errors or missing runtime handles they warn and fall back to `TASK_STORE`. This makes `ctx.store.set` appear successful even when the configured storage backend is unavailable.

Impact:
- Data can be lost silently.
- Tests may pass against ephemeral task-local memory while production persistence is broken.
- Message consumer to HTTP handler persistence becomes unreliable.

Recommendation:
- If a storage engine is configured, storage operation failures should throw a JS exception.
- Reserve task-local fallback only for explicit no-storage development mode.
- Add tests that force backend failure and assert `ctx.store.set/get/delete` fail visibly.

### P1-6: Static file serving can follow symlinks outside the root

Files:
- `crates/riversd/src/static_files.rs`

The path traversal guard checks that the syntactic resolved path starts with the root. It does not canonicalize the final path after symlink resolution.

Impact:
- A symlink inside the static root can expose files outside the intended directory.

Recommendation:
- Canonicalize both the root and resolved file path before serving.
- Reject symlinks if static assets are expected to be immutable bundle content.
- Add tests for `../` traversal and symlink escape.

### P1-7: SWC TypeScript compilation has panic containment but no timeout

Files:
- `crates/riversd/src/process_pool/v8_config.rs`
- `crates/riversd/src/process_pool/module_cache.rs`

TypeScript compilation is wrapped in `catch_unwind`, but parse, transform, and codegen are still synchronous and unbounded. Bundle loading compiles modules during startup.

Impact:
- Pathological or malicious TypeScript can hang the deploy pipeline or node startup.
- Panic safety does not protect against infinite or extremely expensive compiler work.

Recommendation:
- Run SWC compilation in a supervised worker with a hard timeout.
- Treat timeout as a bundle validation failure with a sanitized error.
- Add property/fuzz corpus coverage for pathological TypeScript inputs.

### P1-8: Module cache misses fall back to disk and live compilation

Files:
- `crates/riversd/src/process_pool/v8_engine/execution.rs`
- `crates/riversd/src/process_pool/module_cache.rs`

The module resolver first checks the compiled cache, but a miss can read from disk and compile live. This bypasses the startup validation boundary.

Impact:
- A module can execute despite not being part of the validated module cache.
- Source map, cycle detection, and policy checks can diverge between startup and runtime.

Recommendation:
- In production mode, make module cache miss a hard error.
- Keep disk fallback only behind an explicit development flag.
- Add a test where an import is missing from the cache and assert runtime execution fails.

### P1-9: Errors and stack traces expose absolute host paths

Files:
- `crates/riversd/src/process_pool/v8_engine/execution.rs`
- `crates/riversd/src/process_pool/module_cache.rs`

V8 module origins and module resolver errors use absolute filesystem paths. Some compile errors and stack trace formatting can expose raw script names.

Impact:
- Production errors can leak host paths, workspace layout, usernames, and deployment structure.

Recommendation:
- Introduce a single app-relative path redaction helper.
- Use app/module logical ids for V8 script origins where possible.
- Assert in tests that public handler errors never contain the workspace root.

### P1-10: DataView and health checks lack bounded runtime behavior

Files:
- `crates/rivers-runtime/src/dataview_engine.rs`
- `crates/riversd/src/server/handlers.rs`

`DataViewRequest` carries timeout configuration, but the reviewed execution path does not enforce a query timeout around connect and execute. `/health/verbose` probes each datasource by opening fresh connections sequentially with a timeout per datasource.

Impact:
- Slow datasources can tie up request workers.
- Verbose health can become expensive or slow exactly when the system is degraded.

Recommendation:
- Enforce datasource timeouts at the executor level with `tokio::time::timeout`.
- Use pool health state for health endpoints.
- If active probing is required, make it bounded, parallel, and cached.

### P1-11: PostgreSQL connection string construction is fragile

Files:
- `crates/rivers-drivers-builtin/src/postgres.rs`

The PostgreSQL driver builds a connection string with direct string interpolation of host, user, password, and dbname.

Impact:
- Spaces or special characters in credentials can break connections.
- Depending on parsing rules, crafted values may alter connection options.

Recommendation:
- Use `tokio_postgres::Config` builder APIs instead of interpolating a connection string.
- Add tests for passwords and database names containing spaces, quotes, and option-like substrings.

### P1-12: Handler responses should validate status and headers from JS

Files:
- `crates/riversd/src/view_engine/validation.rs`

`parse_handler_view_result` accepts a JS-provided numeric status and response headers. The reviewed code should explicitly reject invalid status codes and unsafe header names/values.

Impact:
- Invalid status values can cause downstream response construction failures.
- Header injection or policy override risks depend on downstream validation.

Recommendation:
- Accept only status codes in `100..=599`.
- Validate header names and values before response construction.
- Consider blocking handler-supplied security headers unless explicitly allowed by policy.

## P2 Findings

### P2-1: Redis cluster key listing uses `KEYS`

Files:
- `crates/rivers-storage-backends/src/redis_backend.rs`
- `crates/rivers-core/src/storage.rs`

Single-node Redis listing uses `SCAN`, but the cluster path uses `KEYS`. Sentinel claiming and cache/session operations may list broad namespaces.

Recommendation:
- Replace cluster `KEYS` with node-local `SCAN` across primaries.
- Avoid broad key listing for hot paths where an index set or explicit ownership record can be used.

### P2-2: EventBus subscriptions have no removal/backpressure story

Files:
- `crates/rivers-core/src/eventbus.rs`

Subscriptions are appended but there is no reviewed unsubscribe path. Broadcast forwarding creates additional subscribers each time.

Recommendation:
- Return subscription handles that unregister on drop.
- Add metrics for subscriber counts and dispatch duration.
- Bound broadcast subscribers or tie them to request/session lifetime.

### P2-3: Reserved storage prefixes are inconsistent

Files:
- `crates/rivers-core-config/src/storage.rs`
- `crates/riversd/src/process_pool/v8_engine/context.rs`

Core storage reserves `poll:`, while the V8 context reserved list omits `poll:` and adds `raft:`.

Recommendation:
- Define reserved prefixes in one shared module.
- Add tests that every public storage entry point enforces the same reserved set.

### P2-4: View lifecycle observer hooks are awaited despite fire-and-forget comments

Files:
- `crates/riversd/src/view_engine/pipeline.rs`

Pre-process and post-process observers are described as fire-and-forget, but the dispatch calls are awaited.

Recommendation:
- Either make the hooks truly asynchronous background work with bounded queues, or update the contract and apply short timeouts so observers cannot dominate request latency.

### P2-5: JavaScript module detection is string-based

Files:
- `crates/riversd/src/process_pool/v8_engine/execution.rs`

Module syntax detection uses string containment checks like `export ` and `import `. Comments or strings can trigger the module path incorrectly.

Recommendation:
- Treat files as modules based on bundle metadata or extension.
- If content detection remains, use parser-level detection.

### P2-6: Promise resolution loop does not model real async work

Files:
- `crates/riversd/src/process_pool/v8_engine/execution.rs`

Promise resolution runs microtask checkpoints for a fixed number of ticks and returns a timeout with `0` for pending promises. This does not reflect configured task timeouts or timer/I/O progress.

Recommendation:
- Tie promise resolution to the task timeout budget.
- Make pending promise errors include the configured timeout and handler identity.
- Add tests for async handlers using timers and host callbacks.

### P2-7: MySQL and DriverFactory runtime strategy need separation

Files:
- `crates/rivers-drivers-builtin/src/mysql.rs`
- `crates/rivers-core/src/driver_factory.rs`

The MySQL driver creates a `mysql_async::Pool` per connect and keeps it behind one returned connection. `DriverFactory::connect` runs driver connects inside `spawn_blocking` and creates a fresh Tokio runtime.

Recommendation:
- Once DataView uses real pooling, make MySQL pool ownership datasource-scoped, not per connection.
- Use the extra runtime isolation only for plugin drivers that require it; keep built-in async drivers on the active runtime where safe.

### P2-8: SQLite path behavior needs deployment policy

Files:
- `crates/rivers-drivers-builtin/src/sqlite.rs`

SQLite creates parent directories for configured paths and logs the database path.

Recommendation:
- Ensure SQLite datasource paths are restricted to an approved data directory.
- Redact absolute paths in production logs.
- Avoid silently creating directories for bundle-provided datasource paths unless an operator explicitly enabled it.

## Recommended Remediation Sequence

1. Unblock boot: make broker consumer startup nonblocking and move `create_consumer` into the bridge retry loop.
2. Fail closed: fix protected-view behavior when session management is absent.
3. Lock down V8 host capabilities: guard `ctx.ddl`, storage fallback, module cache miss behavior, and absolute path leakage.
4. Restore datasource identity: centralize task enrichment so every handler path carries app id, datasource map, lockbox, storage engine, and driver factory.
5. Integrate DataView with datasource-scoped connection pools and enforce query timeouts.
6. Fix pool internals before broad adoption.
7. Correct Kafka producer destination semantics and lazy metadata behavior.
8. Tighten EventBus priority ordering and subscription lifecycle.
9. Harden storage backends and static file path handling.
10. Expand integration tests around canary-critical paths: REST, DataView, ApplicationInit, MessageConsumer, security hooks, validation hooks, storage persistence, and broker degraded startup.

## Test Recommendations

Add focused tests for:

- Non-public view with missing session manager fails closed.
- REST handler cannot call `ctx.ddl`.
- ApplicationInit can call allowed DDL and cannot call disallowed DDL.
- MessageConsumer `ctx.store.set` is readable from the same app's HTTP handler.
- DataView calls reuse datasource pool state and obey max connections.
- DataView connect and query timeout paths.
- Broker consumer startup succeeds even when Kafka is unreachable.
- Kafka producer routes by `OutboundMessage.destination`.
- Wildcard EventBus `Expect` handler runs before exact `Emit` or `Observe`.
- Static file symlink escape is rejected.
- Module cache miss fails in production mode.
- Public errors redact absolute workspace paths.
- Redis cluster list uses scan-style iteration rather than `KEYS`.

## Closing Assessment

The codebase has many of the right pieces: explicit storage traits, datasource drivers, pool primitives, event priorities, V8 task locals, and security pipeline hooks. The current risk is mostly at the wiring boundaries. The runtime needs one authoritative path for app identity, datasource access, storage, and host capabilities. Once that exists, the canary failures should become much easier to separate into real driver issues versus runtime wiring regressions.
