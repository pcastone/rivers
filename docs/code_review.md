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

<!-- BEGIN docs/review consolidated -->

## Crate-Specific Review Reports

Source reports: `docs/review/`. This section is generated by consolidating the individual crate reports into this full review document.

Included reports:

- [`rivers-keystore`](review/rivers-keystore)
- [`rivers-driver-sdk`](review/rivers-driver-sdk)
- [`rivers-engine-sdk`](review/rivers-engine-sdk)
- [`rivers-plugin-kafka`](review/rivers-plugin-kafka)
- [`rivers-core-config`](review/rivers-core-config)
- [`riversctl`](review/riversctl)
- [`cargo-deploy`](review/cargo-deploy)
- [`riverpackage`](review/riverpackage)
- [`rivers-plugin-ldap`](review/rivers-plugin-ldap)
- [`rivers-plugin-neo4j`](review/rivers-plugin-neo4j)
- [`rivers-plugin-cassandra`](review/rivers-plugin-cassandra)
- [`rivers-plugin-mongodb`](review/rivers-plugin-mongodb)
- [`rivers-plugin-elasticsearch`](review/rivers-plugin-elasticsearch)
- [`rivers-plugin-couchdb`](review/rivers-plugin-couchdb)
- [`rivers-plugin-influxdb`](review/rivers-plugin-influxdb)
- [`rivers-plugin-redis-streams`](review/rivers-plugin-redis-streams)
- [`rivers-plugin-nats`](review/rivers-plugin-nats)
- [`rivers-plugin-rabbitmq`](review/rivers-plugin-rabbitmq)

### `rivers-keystore`


Scope: `crates/rivers-keystore` CLI, with supporting checks through `crates/rivers-keystore-engine` because the CLI delegates file I/O and key generation there.

#### Severity distribution

The engine is cleaner than the CLI. Key generation and rotation use `OsRng` (`crates/rivers-keystore-engine/src/key_management.rs:39-45`, `crates/rivers-keystore-engine/src/key_management.rs:143-147`), keystore writes set `0600` before persist (`crates/rivers-keystore-engine/src/io.rs:94-102`), and the CLI exposes metadata only for `list`/`info` (`crates/rivers-keystore/src/main.rs:132-163`). I did not find export/import commands, so there is no plaintext export format to review in this crate.

Bug density:
- CLI secret handling: high.
- Destructive command behavior: high.
- Concurrent file mutation: medium.
- Engine crypto/file permissions: comparatively clean.

#### Findings

##### P1 - `init` can silently overwrite an existing keystore with an empty one

`cmd_init()` calls `AppKeystore::create(path, recipient)` without checking whether `path` already exists (`crates/rivers-keystore/src/main.rs:113-118`). The engine then writes an empty keystore via `save()` (`crates/rivers-keystore-engine/src/io.rs:14-20`), and `save()` uses `NamedTempFile::persist(path)` (`crates/rivers-keystore-engine/src/io.rs:89-102`). In tempfile 3.27, `persist()` uses overwrite mode, not no-clobber mode.

A mistyped or repeated `rivers-keystore init --path data/app.keystore` can therefore replace all application key versions with an empty keystore. Since the old keys are the only way to decrypt existing app data, this is a production data-loss bug.

Fix: make `init` fail if the target exists unless the user passes an explicit `--force`, and use a no-clobber create path for new keystores.

##### P1 - The Age identity is handled as a normal environment string and never zeroized

Every command reads the Age private identity from `RIVERS_KEYSTORE_KEY` (`crates/rivers-keystore/src/main.rs:85-94`). `read_identity()` stores it in an ordinary `String`, returns another ordinary `String`, and the CLI never removes the environment variable from the process. `load_keystore()` passes that raw string into the engine (`crates/rivers-keystore/src/main.rs:97-100`), while `save_keystore()` reparses the identity to derive the recipient (`crates/rivers-keystore/src/main.rs:103-108`).

This has two problems: the private identity remains in process environment/memory like any other string, and the documented workflow encourages `export RIVERS_KEYSTORE_KEY="AGE-SECRET-KEY-..."` (`docs/guide/cli.md:207`), which also leaves the secret in the parent shell environment. That is the same class of CLI secret exposure the LockBox spec tries to avoid by treating key sources carefully.

Fix: prefer a file descriptor, protected key file, or interactive/TTY input source for administrative use. If env remains supported, read it into a zeroizing secret wrapper, call `std::env::remove_var()` after capture where safe, and update docs away from shell-wide `export`.

##### P2 - Read-modify-write commands have no file lock, so concurrent admins can lose updates

`generate`, `delete`, and `rotate` all do `load_keystore() -> mutate -> save_keystore()` (`crates/rivers-keystore/src/main.rs:122-129`, `crates/rivers-keystore/src/main.rs:166-190`). `save()` is atomic at the single-write level, but there is no advisory lock around the whole read-modify-write transaction. Two concurrent rotations or a rotation racing a delete can both load the same old file and then last-writer-wins over the other command.

Fix: lock a sidecar or the keystore file itself for the full command transaction. Add a concurrency test that starts two mutations and proves both changes survive or one command fails cleanly.

##### P2 - File writes are atomic but not durable across crashes

`save()` writes encrypted bytes to a temp file, sets permissions, and persists it (`crates/rivers-keystore-engine/src/io.rs:89-102`), but it does not flush/sync the temp file or the parent directory before returning. The tempfile docs for `persist()` explicitly note that the file contents are not synchronized when `persist` returns.

This is less likely than the clobber bug, but keystores are high-value state. A crash or power loss at the wrong time can lose a just-generated or just-rotated key even though the command printed success.

Fix: `write_all`, `sync_all` the temp file, persist/rename, then sync the parent directory on Unix. Keep the existing `0600` behavior.

#### Repeated patterns

- The CLI has the same secret-source and read-modify-write risk class as other local secret administration tools. The engine handles generated key bytes carefully, but the CLI identity source is still plain env/string handling.
- No 3+ crate code pattern was proven from this crate alone; the repeated risk is operational: local admin CLIs need shared helpers for protected secret input, no-clobber init, file locking, and durable atomic writes.

#### Test gaps to add

- CLI test that `init` refuses to overwrite an existing keystore.
- CLI test that commands work without leaking key material to stdout/stderr.
- Engine/CLI test for concurrent rotations or generate+delete races.
- Durability-path unit test around the writer helper, if the file writer is refactored into an injectable component.

### `rivers-driver-sdk`


Scope: `crates/rivers-driver-sdk`, with cross-crate checks through driver registration, DataView parameter translation, and plugin ABI call sites.

#### Severity distribution

`rivers-driver-sdk` is bug-dense for a contract crate. The trait bounds are mostly sane (`DatabaseDriver` and `Connection` are `Send + Sync`, and plugin load uses strict ABI equality), but several safety rules are advisory helpers instead of enforced contracts. Those advisory rules are then repeated across many drivers, which is where the Rivers-wide risk comes from.

Clean areas:
- ABI version comparison is strict equality in `crates/rivers-core/src/driver_factory.rs:334`.
- `Connection` and `DatabaseDriver` include `Send + Sync` bounds in `crates/rivers-driver-sdk/src/traits.rs:477` and `crates/rivers-driver-sdk/src/traits.rs:563`.
- The default transaction/DDL methods return explicit `Unsupported` errors rather than silently no-oping.

Bug density:
- SDK/core ABI boundary: high.
- DDL/admin guard contract: high.
- Parameter translation: high.
- Shared error/secret hygiene: medium.
- Static driver inventory wiring: medium.

#### Findings

##### P0 - Plugin registration panics can still cross `extern "C"`

The loader says `_rivers_register_driver()` is called inside `catch_unwind` (`crates/rivers-core/src/driver_factory.rs:366-370`), but the registered functions themselves are plain `extern "C"` exports, for example `crates/rivers-plugin-cassandra/src/lib.rs:221-228`, `crates/rivers-plugin-kafka/src/lib.rs:311-321`, `crates/rivers-plugin-rabbitmq/src/lib.rs:391-401`, and `crates/rivers-drivers-builtin/src/lib.rs:52-62`. A Rust panic leaving an `extern "C"` function aborts before the host-side `catch_unwind` can recover. This repeats across the plugin fleet, so the SDK contract is not actually enforceable at the crate boundary.

Shared fix: put the unwind barrier inside the exported function body. The SDK should provide a macro/helper for plugin exports so every plugin has the same `catch_unwind` pattern and never unwinds over C ABI. The host-side `catch_unwind` can remain as a second layer, but it cannot be the only layer.

##### P1 - Leading SQL comments bypass the DDL guard in every SQL-style driver using the helper

`Query::new()` infers operation after stripping SQL comments (`crates/rivers-driver-sdk/src/types.rs:77-105`), but `is_ddl_statement()` only calls `trim_start()` and then checks prefixes (`crates/rivers-driver-sdk/src/lib.rs:58-64`). SQL drivers then call `check_admin_guard(query, self.admin_operations())` with empty SQL admin operation lists, for example Postgres (`crates/rivers-drivers-builtin/src/postgres.rs:168-172`), MySQL (`crates/rivers-drivers-builtin/src/mysql.rs:171-175`), SQLite (`crates/rivers-drivers-builtin/src/sqlite.rs:134-138`), Cassandra (`crates/rivers-plugin-cassandra/src/lib.rs:75-79`), and CouchDB (`crates/rivers-plugin-couchdb/src/lib.rs:126-130`).

That means a statement like `/* migration */ DROP TABLE users` can have operation `drop` but still return `None` from `check_admin_guard()` because the DDL text check sees `/`, not `DROP`. This is a trait-contract violation: `execute()` is documented as the DDL gate, but the shared helper lets DDL through.

Shared fix: make `is_ddl_statement()` use the same comment-stripping path as `infer_operation()`, add negative tests for line and block comments before DDL, and consider a guarded connection wrapper so individual drivers cannot skip or weaken Gate 1.

##### P1 - Parameter translation corrupts prefix-sharing placeholders and drops missing parameters silently

`translate_params()` collects placeholder names, then rewrites by global string replacement (`crates/rivers-driver-sdk/src/lib.rs:141-170`). If a query contains `$id` and `$id2`, replacing `$id` first rewrites the prefix inside `$id2`; for `DollarPositional`, `$id2` becomes `$12` and is no longer a real second bind. The same function also builds ordered parameters with `filter_map()` (`crates/rivers-driver-sdk/src/lib.rs:135-138`), so a placeholder with no value is silently omitted after the statement may already have been rewritten.

This is cross-crate because DataView calls the SDK translator for any registered driver with non-`None` `ParamStyle` (`crates/rivers-runtime/src/dataview_engine.rs:683-703`), and multiple drivers opt in: Postgres, MySQL, SQLite, Cassandra, CouchDB, and Neo4j all declare non-default styles.

Shared fix: replace the ad hoc replacement loop with a single lexer pass that emits the rewritten SQL and ordered parameters at the same time. Return `Result<TranslatedParams, DriverError>` and fail on missing parameters instead of dropping them.

##### P1 - The production driver inventory does not call all exported registrars

Dynamic plugin loading exists in `load_plugins()` (`crates/rivers-core/src/driver_factory.rs:227-295`), but production startup documents cdylib plugins as disabled and uses a static list instead (`crates/riversd/src/server/drivers.rs:1-6`). That static list registers Cassandra, CouchDB, MongoDB, Elasticsearch, InfluxDB, LDAP, Kafka, RabbitMQ, NATS, and Exec (`crates/riversd/src/server/drivers.rs:28-39`), but it does not register `Neo4jDriver` or `RedisStreamsDriver`. Those crates export `_rivers_register_driver()` (`crates/rivers-plugin-neo4j/src/lib.rs:336-347`, `crates/rivers-plugin-redis-streams/src/lib.rs:446-456`) with no production caller found.

This is a wiring gap, not a per-driver bug. Each crate can pass its own tests while bundles referencing `neo4j` or `redis-streams` fail at runtime because the driver was never put in the factory.

Shared fix: centralize the driver inventory in one generated/static registry and test it against every workspace crate that exports `_rivers_register_driver` or implements a public driver type.

##### P2 - `ConnectionParams` derives `Debug` with a plaintext password

`ConnectionParams` is the SDK object every driver receives after LockBox resolution, and it derives `Debug` while containing `pub password: String` (`crates/rivers-driver-sdk/src/traits.rs:387-405`). Any `?params` tracing, panic payload, or diagnostic dump in driver/core code can leak the resolved secret. The SDK contract says driver errors must not contain credential material (`crates/rivers-driver-sdk/src/error.rs:3-6`), but the shared type makes accidental leakage easy.

Shared fix: implement a custom `Debug` that redacts `password`, and consider using `secrecy::SecretString` or a Rivers-local secret wrapper so formatting is safe by default.

#### Repeated patterns

- Plugin crates repeat bare `extern "C"` registration exports without an internal unwind barrier.
- Drivers repeat the DDL guard manually instead of getting it from the trait contract.
- Parameter translation is shared SDK logic, so one string-rewrite bug affects every driver that opts into positional or named rewriting.

#### Test gaps to add

- SDK tests for DDL preceded by `--` and `/* ... */` comments.
- SDK tests for `$id` plus `$id2`, duplicate placeholders, and missing placeholder values.
- A workspace inventory test that compares static server registrations against exported/available plugin drivers.
- ABI panic tests using a test plugin whose registration panics, proving the process does not abort.

### `rivers-engine-sdk`


Scope: `crates/rivers-engine-sdk`, with cross-checks through `rivers-runtime` and the V8/WASM engines because the SDK crate only defines the serialized C-ABI surface.

#### Severity distribution

`rivers-engine-sdk` is deceptively small but high-risk. The crate does not define the `Worker` trait or the runtime token types described in the prompt; those live in `crates/rivers-runtime/src/process_pool/types.rs`. The SDK defines the JSON wire contract that engines deserialize, so contract bugs here become engine-wide.

Clean areas:
- The serialized task context owns its data; I found no borrowed caller-side state crossing the engine boundary.
- Runtime `Worker` has `Send + Sync` bounds (`crates/rivers-runtime/src/process_pool/types.rs:259`).
- Capability defaults in `TaskContextBuilder::new()` are mostly denied: no HTTP, no storage, no driver factory, no DataView executor, no LockBox, and no keystore unless explicitly added (`crates/rivers-runtime/src/process_pool/bridge.rs:113-138`).

Bug density:
- Token opacity: high.
- Engine capability enforcement: high.
- ABI unwind/memory ownership: high.
- Error normalization: medium.

#### Findings

##### P1 - Serialized tokens are not opaque; direct datasource roots and token internals cross into the engine

The SDK wire contract exposes `datasource_tokens: HashMap<String, String>` and `dataview_tokens: HashMap<String, String>` as public fields (`crates/rivers-engine-sdk/src/lib.rs:33-40`). The runtime bridge serializes `DatasourceToken::Direct` as `direct://<driver>?root=<path>` (`crates/rivers-runtime/src/process_pool/bridge.rs:16-23`), and the V8 engine parses that string back into `driver` and `root` (`crates/rivers-engine-v8/src/execution.rs:293-299`). The runtime token itself also derives `Debug` and has public fields (`crates/rivers-runtime/src/process_pool/types.rs:20-30`), and `DataViewToken` is a public tuple struct (`crates/rivers-runtime/src/process_pool/types.rs:50-52`).

That violates the stated opacity boundary. The isolate may not see passwords, but it can receive host resource topology: direct driver name, filesystem root, datasource names, DataView token strings, and serialized datasource configs with host/database/username/options (`crates/rivers-engine-sdk/src/lib.rs:84-99`). A token should be an unforgeable handle, not a parseable URL with a resource path in it.

Shared fix: make the SDK wire token an opaque random/id handle only. Keep driver/root/config host-side, keyed by that handle. Remove token internals from `Debug` output or replace with redacted debug implementations.

##### P1 - V8 injects capabilities even when the SDK flags say they are unavailable

`SerializedTaskContext` carries capability flags such as `http_enabled`, `storage_available`, `lockbox_available`, and `keystore_available` (`crates/rivers-engine-sdk/src/lib.rs:41-64`). But V8 injects `ctx.store`, `ctx.dataview`, and `ctx.ddl` unconditionally when constructing `ctx` (`crates/rivers-engine-v8/src/execution.rs:276-286`). `ctx.store` then falls back to an in-memory task-local map rather than failing when `storage_available` is false (`crates/rivers-engine-v8/src/execution.rs:611-681`). `ctx.dataview()` attempts the host callback for any name passed by handler code, not only names present in `dataview_tokens` (`crates/rivers-engine-v8/src/execution.rs:692-786`). `ctx.ddl()` is also always present and will call the DDL host callback if registered (`crates/rivers-engine-v8/src/execution.rs:790-873`).

This is a contract violation: the SDK has capability flags, but the engine implementation does not consistently enforce them as an allowlist. New capabilities must default denied; here some existing capabilities are visible by default and rely on later host failure.

Shared fix: have the SDK expose a single capability descriptor and require engines to gate injection from it. Add conformance tests where all flags/tokens are empty and `ctx.store`, dynamic `ctx.dataview`, `ctx.datasource`, `Rivers.http`, crypto, keystore, and `ctx.ddl` are unavailable.

##### P1 - Engine `extern "C"` exports lack outermost `catch_unwind`

The SDK documents the C-ABI engine exports (`crates/rivers-engine-sdk/src/lib.rs:290-300`), but the V8 and WASM engines export plain `extern "C"` functions without an outer `catch_unwind`, for example V8 execute/init (`crates/rivers-engine-v8/src/lib.rs:35-82`) and WASM execute/init (`crates/rivers-engine-wasm/src/lib.rs:41-97`). Rust panics crossing an `extern "C"` ABI abort the process. Returning `i32` error codes is not enough if any unwrap, V8/Wasmtime panic, allocation panic, or callback panic happens before the function returns.

Shared fix: the SDK should provide an export wrapper macro that catches unwind at every exported C entrypoint and converts panic payloads into an error buffer where possible.

##### P2 - Host callback table is not versioned by capability, so added callbacks can fail open or drift silently

`HostCallbacks` is a raw `repr(C)` struct with many `Option<extern "C" fn>` fields (`crates/rivers-engine-sdk/src/lib.rs:146-286`), while `ENGINE_ABI_VERSION` remains `1` (`crates/rivers-engine-sdk/src/lib.rs:23-25`). Adding, reordering, or semantically changing a field is an ABI break, but the SDK has no per-callback feature negotiation and no compile-time guard proving engines and host agree on the same table layout. This matters because several callbacks are powerful: `datasource_build`, `crypto_decrypt`, `ddl_execute`, and transaction callbacks.

Fix: bump ABI on every layout change, add a size/layout test shared by host and engines, and consider a callback table version/length field so older engines cannot misinterpret newer callback slots.

##### P2 - Task errors are not normalized by the SDK boundary

The SDK boundary only returns either `SerializedTaskResult` or an engine-produced string error from `_rivers_engine_execute` (`crates/rivers-engine-sdk/src/lib.rs:110-117`, `crates/rivers-engine-v8/src/lib.rs:71-82`, `crates/rivers-engine-wasm/src/lib.rs:85-96`). The richer runtime `TaskError` variants live outside the SDK (`crates/rivers-runtime/src/process_pool/types.rs:192-249`). As a result, V8/WASM-specific phrasing can leak through as `HandlerError`, and callers cannot reliably distinguish capability denial, host callback failure, compile failure, timeout, and engine-internal failure from the serialized contract alone.

Fix: add a serialized error envelope with stable `kind`, sanitized `message`, optional debug stack, and engine name. Map V8 and WASM errors into that envelope before crossing the ABI.

#### Repeated patterns

- Capability state exists in multiple layers, but the engine injection code does not consistently use it as the source of truth.
- Opaque runtime tokens become strings at the SDK bridge, then get parsed by engines.
- Both V8 and WASM engine exports repeat plain `extern "C"` entrypoints without internal unwind guards.

#### Test gaps to add

- Engine conformance tests for all capabilities denied by default.
- Token serialization tests proving direct roots, pool ids, usernames, and datasource config internals do not cross into `SerializedTaskContext`.
- ABI panic tests for every exported engine symbol.
- Stable serialized error tests for V8 and WASM compile/runtime/capability failures.

### `rivers-plugin-kafka`


Scope: `crates/rivers-plugin-kafka`.

Important correction: the current crate is not using `rdkafka`/`librdkafka`; it declares and uses `rskafka` (`crates/rivers-plugin-kafka/src/lib.rs:2`, `crates/rivers-plugin-kafka/Cargo.toml:18`). I did not find librdkafka callbacks, OpenSSL/SASL static-linking code, or rebalance callbacks in this crate. The current risk is broker semantics, not C callback safety.

#### Severity distribution

This crate is bug-dense for a broker driver. The code is small and easy to follow, but the advertised semantics around consumer groups, acks, and offsets are not implemented beyond in-memory single-partition polling.

Clean areas:
- It does not currently pass Rust callbacks into a C library.
- `produce()` is awaited before returning a receipt (`crates/rivers-plugin-kafka/src/lib.rs:137-148`).
- Connection errors from client/partition creation are mapped into `DriverError::Connection` (`crates/rivers-plugin-kafka/src/lib.rs:43-53`, `crates/rivers-plugin-kafka/src/lib.rs:75-85`).

Bug density:
- Consumer lifecycle/offset semantics: high.
- Group coordination: high.
- FFI/plugin export safety: medium.
- Topic/partition config validation: medium.

#### Findings

##### P1 - `receive()` advances the offset before `ack()`, so `nack()` cannot redeliver the message

`receive()` fetches from `self.offset + 1` (`crates/rivers-plugin-kafka/src/lib.rs:171-183`) and then sets `self.offset = rec_offset` before returning the message (`crates/rivers-plugin-kafka/src/lib.rs:214-218`). `ack()` sets the same offset again (`crates/rivers-plugin-kafka/src/lib.rs:224-230`), and `nack()` does nothing while claiming the message will be re-fetched (`crates/rivers-plugin-kafka/src/lib.rs:233-235`). That claim is false: because `receive()` already advanced the offset, the next call fetches `rec_offset + 1`.

This is an at-most-once failure mode hiding behind an at-least-once API. A handler crash between `receive()` and `ack()` loses the message for that consumer instance.

Fix: track `delivered_offset` separately from `committed_offset`. Only advance the committed offset on `ack()`. On `nack()` leave the committed offset unchanged so the same record can be fetched again.

##### P1 - Consumer group coordination is declared but not wired to storage or partition ownership

The crate-level docs say consumer group coordination is managed by Rivers (`crates/rivers-plugin-kafka/src/lib.rs:4-5`), and helper keys exist for offsets and ownership (`crates/rivers-plugin-kafka/src/lib.rs:244-264`). But `KafkaConsumer` only stores `group_id` and an in-memory `offset` (`crates/rivers-plugin-kafka/src/lib.rs:159-166`); neither `create_consumer()` nor `ack()` reads/writes the helper keys to a `StorageEngine`. There is also no partition assignment or ownership check in this driver.

After restart, every consumer starts at `-1` and fetches from offset `0` (`crates/rivers-plugin-kafka/src/lib.rs:92-98`, `crates/rivers-plugin-kafka/src/lib.rs:171-178`). With multiple nodes or consumers, every instance can read the same partition. The helper functions make this look implemented when it is not.

Fix: either implement storage-backed committed offsets and partition leases, or rename the current driver semantics as single-process, single-partition, in-memory polling and fail when group mode is requested.

##### P1 - Producer silently reports offset `0` if Kafka returns no produced offsets

After `produce()`, the code reads the first returned offset and falls back to `0` on an empty vector (`crates/rivers-plugin-kafka/src/lib.rs:137-146`). An empty offsets vector is not a successful publish receipt. Returning `orders:0:0` would make observability and retries believe a message was accepted at offset 0 when the driver has no such proof.

Fix: treat an empty offsets response as `DriverError::Query("kafka produce returned no offsets")`.

##### P2 - Plugin registration export has the same FFI unwind gap as the rest of the plugin fleet

The plugin exports `_rivers_register_driver()` as a plain `extern "C"` function and calls into Rust registration code directly (`crates/rivers-plugin-kafka/src/lib.rs:317-322`). If registration ever panics, the panic crosses an `extern "C"` boundary. This matches the SDK-wide plugin export finding: host-side `catch_unwind` cannot recover a panic after it has already crossed the C ABI.

Fix: use a shared SDK export wrapper that catches unwind inside the exported function.

##### P2 - Topic and partition defaults can silently publish to the wrong place

`resolve_topic()` falls back to `params.database` when no subscription is configured (`crates/rivers-plugin-kafka/src/lib.rs:102-109`), and partition defaults to `0` on missing or unparsable `options.partition` (`crates/rivers-plugin-kafka/src/lib.rs:37-41`, `crates/rivers-plugin-kafka/src/lib.rs:69-73`). For Kafka, a typo in partition config or an empty subscription should be a configuration error, not silent routing to database/partition 0.

Fix: validate topic and partition at `create_producer`/`create_consumer` time. Reject empty topics and invalid partition strings.

#### Repeated patterns

- The plugin export FFI unwind issue repeats across plugin crates.
- Broker drivers are likely to share the same semantic risk: ack/nack contracts need explicit at-least-once/at-most-once tests, not just live smoke tests.

#### Test gaps to add

- Unit test proving `nack()` causes the same offset to be fetched again.
- Restart test proving committed offsets persist through storage.
- Multi-consumer test proving partition ownership prevents duplicate consumers.
- Produce test for empty offset response mapped to an error.

### `rivers-core-config`


Scope: `crates/rivers-core-config`, with call-site checks in `rivers-runtime`, `riversd`, and `riversctl`.

#### Severity distribution

`rivers-core-config` is high-use and moderately bug-dense. The structs are readable, but too much validation is optional, delayed, or implemented outside the crate. The biggest risk is that a config can deserialize successfully and then only fail much later, or worse, run with a dangerous default.

Clean areas:
- Some security defaults are deny-by-default: CORS disabled, admin API disabled, session disabled, CSRF enabled, session cookies `http_only=true` and `secure=true` (`crates/rivers-core-config/src/config/security.rs:76-94`, `crates/rivers-core-config/src/config/security.rs:147-155`, `crates/rivers-core-config/src/config/security.rs:257-267`).
- Admin API host defaults to loopback (`crates/rivers-core-config/src/config/tls.rs:142-170`).
- Unknown top-level and `[base]` keys are at least warned by `load_server_config()` (`crates/rivers-runtime/src/loader.rs:36-45`).

Bug density:
- Parse/use validation gap: high.
- Dangerous production defaults: medium.
- Path validation: medium.
- Hot reload/environment override validation: high.

#### Findings

##### P1 - Server config validation exists but production startup does not call it

`validate_server_config()` rejects `base.port = 0`, zero request timeouts, enabled admin API without a port, admin/main port conflicts, and CORS enabled with empty origins (`crates/rivers-runtime/src/validate.rs:10-55`). But the production load path only calls `load_server_config()` and returns the parsed struct (`crates/rivers-runtime/src/loader.rs:36-48`), and `riversd` starts building the runtime from that parsed config without calling `validate_server_config()` (`crates/riversd/src/main.rs:40-84`). The only call site I found is `riversctl doctor` (`crates/riversctl/src/commands/doctor.rs:69-83`).

That means validation catches issues only when an operator remembers to run doctor. Startup can accept semantically invalid config and fail later or bind unexpectedly.

Shared fix: make `load_server_config()` either validate by default or provide a `load_validated_server_config()` used by `riversd`, hot reload, and `riversctl`.

##### P1 - Environment overrides can produce an invalid config after parse-time checks

`riversd` applies `RIVERS_ENV` overrides after loading config (`crates/riversd/src/main.rs:61-67`). Both override implementations mutate ports, timeouts, CORS, and storage backend fields directly (`crates/rivers-core-config/src/config/runtime.rs:123-151`, `crates/rivers-runtime/src/env_override.rs:18-81`). There is no validation pass after overrides in `riversd`.

An otherwise valid config can become invalid only in production, for example by setting `base.port = 0`, enabling CORS with empty origins, or switching storage to `redis` without a URL. That is exactly the kind of environment-specific footgun config validation should prevent.

Fix: validate after applying overrides and before building the tokio runtime or starting listeners.

##### P1 - Hot reload rebuilds views from a new config without validating it as a whole

The hot reload task receives `reload_config` and immediately rebuilds views/DataViews from it (`crates/riversd/src/server/lifecycle.rs:600-620`). I found no call to the same server validation path before the reload config is accepted. Since hot reload can change `bundle_path`, CORS/security fields, and storage/backend-related configuration, a partial or invalid reload can leave the process with a hybrid of old listeners and new app state.

Fix: parse and validate the full replacement config, classify restart-required changes, then atomically swap only reload-safe fields.

##### P2 - Dangerous or surprising defaults are encoded in config structs without environment policy

Several defaults are reasonable for development but risky if a production operator omits fields:
- Main server host defaults to `0.0.0.0` (`crates/rivers-core-config/src/config/server.rs:95-100`, `crates/rivers-core-config/src/config/server.rs:168-170`).
- Storage defaults to in-memory (`crates/rivers-core-config/src/config/storage.rs:12-14`, `crates/rivers-core-config/src/config/storage.rs:107-109`), which loses state on restart.
- GraphQL introspection defaults to enabled (`crates/rivers-core-config/src/config/runtime.rs:309-327`).
- TLS minimum defaults to `tls12` (`crates/rivers-core-config/src/config/tls.rs:108-130`).

Some of these may be acceptable in dev, but there is no production profile validation that forces explicit acknowledgement.

Fix: add an environment/profile-aware validation layer. In production, require explicit host, persistent storage when features need persistence, explicit GraphQL introspection policy, and TLS policy.

##### P2 - Path fields are plain strings with no traversal or absolute/relative policy

Config exposes many paths as `String`/`Option<String>`: `bundle_path`, `data_dir`, TLS cert/key paths, admin TLS paths, storage path, log file path, app log dir, engine dir, plugin dir, LockBox path/key files (`crates/rivers-core-config/src/config/server.rs:28-47`, `crates/rivers-core-config/src/config/tls.rs:10-31`, `crates/rivers-core-config/src/config/runtime.rs:224-230`, `crates/rivers-core-config/src/lockbox_config.rs:10-30`). I found no central path policy in this crate.

That leaves path traversal and deployment-root escape decisions to scattered consumers. For a config crate, typed paths plus validation are the right place to make the policy explicit.

Fix: use `PathBuf` for paths and validate each field: which must be absolute, which may be relative to `RIVERS_HOME`/bundle root, and which must not contain `..` after normalization.

#### Repeated patterns

- `#[serde(default)]` is heavily used to make config forgiving, but the compensating validation phase is not consistently called.
- Security-sensitive config is split between `rivers-core-config`, `rivers-runtime`, and `riversd`, so no single crate owns the full contract.

#### Test gaps to add

- Startup tests proving invalid `ServerConfig` is rejected by `riversd`, not only by `riversctl doctor`.
- Environment override tests that invalid post-override configs fail.
- Hot reload tests that invalid reload configs do not partially apply.
- Path policy tests for bundle, TLS, log, engine, plugin, and LockBox paths.

### `riversctl`


Scope: `crates/riversctl`.

#### Severity distribution

`riversctl` is small but under-protective for an admin CLI. The local daemon commands are straightforward, but the admin API client currently fails open when signing is not configured and prints server responses verbatim.

Clean areas:
- Local process launch uses `Command` with argument arrays, not shell strings (`crates/riversctl/src/commands/start.rs:109-116`, `crates/riversctl/src/commands/start.rs:140-146`).
- `tls expire` requires `--yes` (`crates/riversctl/src/tls_cmd.rs:41-47`).
- `exec verify` validates SHA-256 shape before comparing (`crates/riversctl/src/commands/exec.rs:36-42`).

Bug density:
- Admin auth: high.
- Sensitive output handling: high.
- Destructive admin operations: medium.
- TLS key file handling: medium.

#### Findings

##### P1 - Missing or invalid admin signing key silently sends unsigned admin requests

`sign_request()` always returns a timestamp header, but if `RIVERS_ADMIN_KEY` is unset, unreadable, invalid hex, or not 32 bytes, it simply omits `X-Rivers-Signature` and returns success (`crates/riversctl/src/commands/admin.rs:7-35`). `admin_get()` and `admin_post()` then send the request anyway (`crates/riversctl/src/commands/admin.rs:38-71`). There is no error telling the operator that the request is unauthenticated.

If the server was accidentally started with `no_auth`, or an endpoint does not enforce auth correctly, this turns a local CLI misconfiguration into unauthenticated admin control. Even when the server rejects it, the failure mode is late and ambiguous.

Fix: make signing return `Result<HashMap<_, _>, String>` and fail closed for admin commands unless an explicit `--no-admin-auth`/dev flag is provided.

##### P1 - The admin private key file has no permission check and key bytes are normal strings

The CLI reads the Ed25519 seed from the path in `RIVERS_ADMIN_KEY` with `read_to_string()` (`crates/riversctl/src/commands/admin.rs:14-22`). It does not enforce `0600` permissions, does not reject symlinks/world-readable files, and keeps the seed in ordinary `String`/`Vec<u8>` values. The config type even has an admin private key field for tool integration (`crates/rivers-core-config/src/config/tls.rs:148-152`), but this CLI does not read config fallback or validate key storage.

Fix: centralize admin key loading with permission checks, zeroizing buffers, clear error messages, and a single override order: flag, env path, config path.

##### P1 - Admin API output is printed verbatim, including datasources and error bodies

Most admin commands pretty-print the raw response body (`crates/riversctl/src/commands/admin.rs:76-116`, `crates/riversctl/src/commands/admin.rs:327-345`). Error responses are included verbatim in returned errors (`crates/riversctl/src/commands/admin.rs:45-50`, `crates/riversctl/src/commands/admin.rs:65-70`). `cmd_datasources()` is especially risky because datasource objects often contain hostnames, usernames, credential source names, internal paths, or future tokens (`crates/riversctl/src/commands/admin.rs:104-108`).

Fix: add a redaction layer for keys like `password`, `secret`, `token`, `key`, `authorization`, `credential`, `private`, and for known datasource/admin response shapes. Use it for both success and error output.

##### P2 - Destructive admin actions do not require confirmation

`api-stop` posts immediate shutdown and falls back to SIGKILL if the Admin API is unreachable (`crates/riversctl/src/commands/admin.rs:119-131`). Circuit breaker `--trip` and `--reset` mutate live runtime state without `--yes` (`crates/riversctl/src/main.rs:54-78`, `crates/riversctl/src/commands/admin.rs:299-325`). `deploy <path>` also sends a deployment request without a confirmation or dry-run path (`crates/riversctl/src/main.rs:34-37`, `crates/riversctl/src/commands/admin.rs:83-88`).

Fix: require `--yes` for destructive or runtime-mutating admin commands, or prompt on TTY. Keep a `--force`/CI path explicit.

##### P2 - Admin URL defaults to plaintext HTTP and is not constrained to loopback

`RIVERS_ADMIN_URL` defaults to `http://127.0.0.1:9090` (`crates/riversctl/src/main.rs:22-24`). Operators can set any URL and the CLI will send signed admin requests over plain HTTP. Signatures protect request integrity, not response confidentiality, and signed admin traffic over non-loopback HTTP can leak operational metadata.

Fix: default to HTTPS when admin TLS is configured, reject non-loopback HTTP unless `--insecure` is explicit, and show the resolved auth mode before mutating commands.

##### P2 - TLS import copies private keys without enforcing destination permissions

`tls import` validates the cert/key pair, then copies the key file into the configured destination (`crates/riversctl/src/tls_cmd.rs:212-225`). It does not set destination permissions after copy. Depending on umask, existing file mode, or filesystem behavior, the private key can end up broader than `0600`.

Fix: set `0600` on copied key files on Unix, use atomic temp+rename, and fail if the destination is a symlink unless explicitly allowed.

#### Repeated patterns

- Admin clients and secret CLIs share the same missing helper: load a private key from a protected path, enforce permissions, and zeroize material.
- Several commands print raw JSON bodies directly. That is convenient during development but brittle for admin surfaces.

#### Test gaps to add

- `sign_request()` tests for missing, unreadable, invalid-hex, wrong-length, and world-readable key files.
- Redaction tests for datasource/admin JSON responses and HTTP error bodies.
- Confirmation tests for `api-stop`, `deploy`, breaker trip/reset, and TLS key deletion/renewal paths.
- TLS import permission test proving server private keys land as `0600`.

### `cargo-deploy`


Crate: `crates/cargo-deploy`
Tier: B
Role reviewed: local deploy assembler for dynamic/static Rivers layouts.

#### Summary

This crate is small, but it is doing production-sensitive filesystem work with very little deploy-state discipline. The dominant issue is not Rust memory safety; it is silent partial deployment. Most writes go directly into the final target path, overwriting existing files in place, following whatever symlinks are already there, and then continuing after some setup failures.

Important scope correction: the Docker/cross-compilation flow described in the prompt is not implemented in `crates/cargo-deploy`. The crate only runs local `cargo build --release` (`crates/cargo-deploy/src/main.rs:94-111`). Cross image selection lives outside the crate in `Cross.toml:1-7` and packaging scripts such as `scripts/build-packages.sh:237-247`.

#### Findings

##### P1: Deploys overwrite the live target in place with no atomic swap or rollback

Dynamic mode creates `bin/` and `lib/` under the final target and copies binaries/libraries directly there (`crates/cargo-deploy/src/main.rs:137-163`). Static mode does the same for `bin/` (`crates/cargo-deploy/src/main.rs:181-193`). `copy_file()` is a thin `std::fs::copy(src, dst)` wrapper (`crates/cargo-deploy/src/main.rs:585-590`), and config, TLS files, and `VERSION` are written with `std::fs::write` directly into their final paths (`crates/cargo-deploy/src/main.rs:381-384`, `471-474`, `496-500`).

There is no staging directory, no temp-file-plus-rename, no previous-version backup, no service liveness check, and no rollback if scaffolding fails after binaries are replaced. A failed deploy can leave a running installation with new binaries, old libraries, regenerated TLS material, or a partially rewritten config.

##### P1: Target symlinks and existing file symlinks are followed without a deploy-root policy

The user-supplied target path is accepted as `PathBuf::from(path)` (`crates/cargo-deploy/src/main.rs:37`) and then used for `create_dir_all`, `copy`, and `write` throughout the deploy. `write_default_config()` even canonicalizes the target for generated absolute paths (`crates/cargo-deploy/src/main.rs:325-330`), but there is no earlier check that the canonical target is inside an expected root or not a symlink.

This makes the tool easy to point at the wrong live tree. More importantly, if a target file such as `config/tls/server.key` already exists as a symlink, the direct write path can modify the symlink target. A deploy tool should either reject symlinked targets and symlinked destination files or make following them an explicit option.

##### P1: Secret and config permissions depend on umask, with a key exposure window

Runtime directories are created with default process permissions (`crates/cargo-deploy/src/main.rs:578-583`). `riversd.toml` is written without chmod (`crates/cargo-deploy/src/main.rs:325-385`), so under a normal `022` umask the config is group/world-readable. The TLS private key is written first and chmodded to `0600` afterward (`crates/cargo-deploy/src/main.rs:471-480`), which still leaves a time-of-check gap and depends on the initial umask for the write.

The cert generation itself uses `rcgen::KeyPair::generate()` (`crates/cargo-deploy/src/main.rs:457-464`), so I did not find a `thread_rng()` footgun here. The file creation path is the weak part. Use open-with-mode (`0600`) or a secure temp file and atomic rename for private material; set explicit modes on config, key, binary, and runtime directories instead of inheriting the caller's environment.

##### P1: Lockbox initialization failure is downgraded to a warning

`scaffold_runtime()` treats lockbox initialization as part of deployment (`crates/cargo-deploy/src/main.rs:223-225`), but `init_lockbox()` only warns if the `rivers-lockbox` binary is missing, exits non-zero, or cannot be executed (`crates/cargo-deploy/src/main.rs:388-413`). The deploy summary still prints "Ready to run" afterward (`crates/cargo-deploy/src/main.rs:557-561`).

For a generated default config that points at `{root}/lockbox/identity.key` (`crates/cargo-deploy/src/main.rs:362-365`), missing lockbox identity is not optional setup. This should fail the deploy unless the user explicitly requests a no-lockbox scaffold.

##### P2: Dynamic deploy silently skips missing engine libraries

Dynamic mode builds `rivers-engine-v8` and `rivers-engine-wasm` (`crates/cargo-deploy/src/main.rs:130-135`), then copies `librivers_engine_v8.*` and `librivers_engine_wasm.*` only if those files exist. If a file is missing, it logs a warning and continues (`crates/cargo-deploy/src/main.rs:150-158`).

That is a bad failure mode for dynamic deploys. A naming mismatch, build artifact path change, or build-script issue produces a deploy that looks successful but cannot execute handlers at runtime.

##### P2: Cross-compilation behavior is documented elsewhere, not enforced here

The crate does not validate `cross`, Docker image availability, or Linux targets. The only build command is local `cargo build --release` (`crates/cargo-deploy/src/main.rs:94-111`). The x86 Linux cross image is pinned to the mutable `rivers-cross-x86_64:latest` tag (`Cross.toml:1-2`), and the Windows packaging script falls back from `cross` to local `cargo` (`scripts/build-packages.sh:240-247`).

This is a wiring/ownership gap: either `cargo-deploy` should not be described as the cross-compilation authority, or it should own a checked, explicit cross-build path with clear "image missing" errors and non-`latest` image pinning.

#### Repeated patterns

- Direct final-path writes appear in deploy tooling, TLS import paths, config generation, and package assembly. This should become a shared helper pattern: secure temp file, explicit mode, fsync where appropriate, then rename.
- Several CLI/admin tools continue after setup failures that should be fatal. Here, lockbox and missing engine libraries are the concrete examples.
- Symlink policy is not centralized. Deploy, package, config validation, and TLS import each make ad hoc filesystem decisions.

#### Test gaps

- No tests for permission modes on `server.key`, `riversd.toml`, deployed binaries, or runtime directories.
- No tests for deploy into an existing target, symlinked target, or symlinked destination file.
- No tests for partial-failure recovery after binary copy succeeds but TLS/config/lockbox setup fails.
- No tests asserting dynamic mode fails when required engine dylibs are missing.

### `riverpackage`


Crate: `crates/riverpackage`
Tier: B
Role reviewed: app bundle scaffolding, validation/preflight, import-exec snippet generation, and packaging.

#### Summary

The highest-risk issue is that this crate advertises bundle packaging as zip-based, but the implementation currently shells out to `tar` and does not define an archive safety policy. There is no zip extraction code in this crate, so I did not find a literal zip-slip extractor bug here. The practical problems are path traversal in validation inputs, archive symlink/reproducibility gaps, and CLI options that look meaningful but are ignored.

`riverpackage` delegates most validation to `rivers-runtime::validate_bundle_full()` (`crates/riverpackage/src/main.rs:276-290`). That is good for consistency, but it means path-policy bugs in the runtime validator are also `riverpackage` bugs.

#### Findings

##### P1: Bundle app names and file references are joined without rejecting `..` or absolute paths

`run_validate()` passes the user-selected bundle directory straight into the runtime validator (`crates/riverpackage/src/main.rs:279-290`). The runtime validator and loader then build paths by joining untrusted manifest values: app names from `apps` become `bundle_dir.join(app_name)` (`crates/rivers-runtime/src/validate_structural.rs:210-218`, `crates/rivers-runtime/src/loader.rs:82-96`), schema references become `app_dir.join(schema_ref)` (`crates/rivers-runtime/src/validate_syntax.rs:35-48`), and existence checks use `app_dir.join(relative_path)` (`crates/rivers-runtime/src/validate_existence.rs:397-414`).

The structural validator only checks that `apps` is an array of strings (`crates/rivers-runtime/src/validate_structural.rs:170-195`) and extracts those strings unchanged (`crates/rivers-runtime/src/validate_structural.rs:831-841`). I did not find a rule rejecting `../other-app`, absolute paths, Windows prefixes, or symlink escapes for bundle-level app names and schema/module references.

For a packaging/preflight tool, that is the same bug class as zip slip, just before archive extraction. Validation can read outside the bundle root and report those external files as valid bundle resources.

##### P1: `pack` archives symlinks and filesystem metadata without a bundle safety policy

`cmd_pack()` validates, counts files, and then runs system `tar -czf <tar_output> -C <bundle_dir> .` (`crates/riverpackage/src/main.rs:541-589`). There is no symlink rejection, no canonical root walk, no metadata normalization, and no bounded archive policy. `count_files()` also uses `path.is_dir()` while recursively walking (`crates/riverpackage/src/main.rs:557-570`), which follows symlinked directories during the count step.

Even if `tar` archives symlinks rather than following them by default, the package can still contain symlink entries. A later extractor that follows symlinks or extracts them before regular files can write outside the intended bundle root. This crate should decide whether symlinks are forbidden or represented explicitly and safely.

##### P1: `--config` is documented and parsed, but ignored

Usage advertises `riverpackage validate [dir] [--format text|json] [--config <path>]` (`crates/riverpackage/src/main.rs:66-71`). `cmd_validate()` parses `--config` into `_config_path` (`crates/riverpackage/src/main.rs:293-315`) and then discards it, constructing `ValidationConfig { engines: None }` (`crates/riverpackage/src/main.rs:333-338`).

That means users can believe they are validating against the same engine configuration that `riversd` will use, while Layer 4 remains unconfigured and can be skipped/warn-only. Silent no-op flags are exactly the kind of tooling bug that lets bad bundles pass CI and fail during deployment.

##### P2: `pack` says zip, but writes `.tar.gz`

The CLI help says `pack [dir] [output]` packages into a `.zip` file (`crates/riverpackage/src/main.rs:66-72`). The implementation prints that zip support is not present and rewrites an output ending in `.zip` to `.tar.gz` (`crates/riverpackage/src/main.rs:552-578`).

This is contract drift. It breaks callers that expect the requested output path to exist, and it makes the prompt's zip-slip/zip-bomb concerns hard to reason about because the command is no longer producing the archive format it advertises.

##### P2: Generated TOML snippets interpolate user-controlled strings without escaping

`cmd_init()` writes TOML strings using the directory basename as `bundleName`, app name, database name, and entry point (`crates/riverpackage/src/main.rs:101-145`, `170-227`). `cmd_import_exec()` prints a TOML table name and quoted path using raw CLI input/canonicalized path display (`crates/riverpackage/src/main.rs:423-482`), especially `[data.datasources.exec_tools.commands.{name}]` and `path = "{...}"` (`crates/riverpackage/src/main.rs:465-468`).

Names or paths containing quotes, brackets, dots, newlines, or other TOML-significant characters can produce invalid or structurally different TOML. Use TOML serialization or strict identifiers instead of format strings.

##### P2: Package output is not reproducible

`cmd_pack()` delegates to default `tar` behavior (`crates/riverpackage/src/main.rs:575-579`). That preserves filesystem mtimes, modes, owners/groups depending on platform tar behavior, and traversal order from the filesystem walk/tar. The earlier file count is also not sorted (`crates/riverpackage/src/main.rs:557-570`).

This is not a direct security bug, but it is a signing and verification footgun. If Rivers bundles are ever signed, reproducible packaging needs stable ordering, normalized timestamps, normalized uid/gid, normalized modes, and a documented archive format.

#### Repeated patterns

- Path references across bundle validation are stringly typed and joined directly. This same class showed up in config and deploy reviews: root policy belongs in a shared path validation helper, not scattered call sites.
- CLI tooling frequently accepts flags or mode names that do not map cleanly to behavior. Here, `--config` is ignored and "zip" means tarball.
- Archive/deploy code is missing a common "secure filesystem walk" abstraction: reject symlinks if policy says so, canonicalize under root, sort entries, and apply size/count limits.

#### Test gaps

- No tests for `apps = ["../escape"]`, absolute app names, schema references containing `..`, or symlink escapes.
- No tests asserting `--config` changes engine validation behavior or fails if the config cannot be read.
- No tests for symlink entries during `pack`.
- No tests that `pack foo out.zip` actually creates the requested path and format.
- No reproducibility tests for stable archive byte output across repeated runs.

### `rivers-plugin-ldap`


Crate: `crates/rivers-plugin-ldap`
Tier: C - Auth-adjacent

#### Summary

Bug density: medium. The crate is small and uses `ldap3` directly, but it sits close to auth and directory data, so string handling matters. The core risks are unescaped LDAP filters, LDAP-over-cleartext defaults, unbounded searches, and no cleanup path for bind secrets.

#### Findings

##### P1: Search filters are passed through raw, with no RFC 4515 escaping or parameter binding

Search statements are parsed as `base_dn scope filter` and `parts[2]` is sent directly to `ldap.search()` (`crates/rivers-plugin-ldap/src/lib.rs:121-160`, `189-195`). There is no substitution layer that escapes user input per RFC 4515, and the tests only assert that literal filters round-trip (`crates/rivers-plugin-ldap/src/lib.rs:422-450`).

Any caller that builds `(uid=${user})` before reaching the driver can inject filter syntax. LDAP filters are not "just strings"; `*`, `(`, `)`, `\`, and NUL need escaping before interpolation.

##### P1: TLS is not configurable and the default URL is plaintext LDAP

`connect()` always builds `ldap://host:port` and defaults to port 389 (`crates/rivers-plugin-ldap/src/lib.rs:40-44`). I did not find LDAPS, StartTLS, certificate validation, CA pinning, or a way to force TLS from `ConnectionParams.options`.

For an auth-adjacent driver, plaintext LDAP should not be the silent default. If cleartext is supported for dev, it should be explicit and loudly named.

##### P1: Paged search and result limits are absent

`exec_search()` performs one `ldap.search()` and then collects every result into memory (`crates/rivers-plugin-ldap/src/lib.rs:163-238`). There is no size limit, page size, server-side paged-results control, or caller-supplied hard cap.

A broad subtree search like `(objectClass=*)` can allocate arbitrarily large result sets in the Rivers process.

##### P2: Bind password is cloned from shared connection params and never zeroized

The bind path passes `&params.password` directly into `simple_bind()` (`crates/rivers-plugin-ldap/src/lib.rs:55-68`). There is no short-lived secret wrapper and no zeroize step after bind. Because `ConnectionParams.password` is a normal `String`, the credential can remain in memory after successful bind.

This is a Rivers-wide concern for drivers, but LDAP is one of the places where it matters most.

##### P2: Anonymous bind is explicit, but not policy-controlled

The code intentionally performs `simple_bind("", "")` when `username` is empty (`crates/rivers-plugin-ldap/src/lib.rs:55-68`). That avoids accidental fallthrough after a failed authenticated bind, which is good. The missing piece is policy: there is no `allow_anonymous=true` style guard, so omitted credentials silently become anonymous LDAP.

##### P2: Plugin exports can unwind across FFI

Both `_rivers_abi_version` and `_rivers_register_driver` are plain `extern "C"` functions (`crates/rivers-plugin-ldap/src/lib.rs:394-405`). The register function calls into caller-provided registrar code without `catch_unwind`. A panic crossing the plugin ABI is undefined behavior.

#### Repeated patterns

- FFI exports lack `catch_unwind`, matching the other driver plugins in this batch.
- Driver credential strings are ordinary `String`s across crates; none of these plugin wrappers zeroize post-connect copies.
- Query-language drivers rely on caller-built strings instead of a shared escaping/binding abstraction.

#### Test gaps

- No LDAP filter escaping tests for `*`, `(`, `)`, backslash, and NUL.
- No LDAPS/StartTLS tests or tests proving insecure LDAP must be explicitly enabled.
- No paged-search or max-result tests.
- No anonymous-bind policy test.

### `rivers-plugin-neo4j`


Crate: `crates/rivers-plugin-neo4j`
Tier: C - Pre-release dependency

#### Summary

Bug density: medium-high. The code has reasonable error propagation around `neo4rs`, but the transaction contract is wired incorrectly and NULL/type handling is lossy. Since this uses a release-candidate Bolt client, any wrapper behavior that assumes happy paths needs extra suspicion.

#### Findings

##### P1: `begin_transaction()` does not affect subsequent `execute()` calls

`Neo4jConnection` stores an optional `txn` (`crates/rivers-plugin-neo4j/src/lib.rs:79-84`) and advertises transaction support (`crates/rivers-plugin-neo4j/src/lib.rs:70-76`). `begin_transaction()` starts and stores a transaction (`crates/rivers-plugin-neo4j/src/lib.rs:178-185`), but `execute()` and `ddl_execute()` always run on `self.graph`, not the stored transaction (`crates/rivers-plugin-neo4j/src/lib.rs:97-163`, `214-241`).

That means a caller can successfully begin, execute mutations outside the transaction, and then commit or roll back an unused transaction. This violates the SDK transaction contract and can leave users believing rollback protected writes when it did not.

##### P1: NULL parameters are silently coerced to empty strings

`build_cypher()` maps `QueryValue::Null` to `""` (`crates/rivers-plugin-neo4j/src/lib.rs:247-252`). The tests encode that behavior as expected (`crates/rivers-plugin-neo4j/src/lib.rs:401-408`).

That is data corruption, not compatibility. `WHERE n.name = null` and `WHERE n.name = ""` are semantically different in Cypher. If `neo4rs` cannot bind Bolt null directly, the driver should reject null parameters with a clear error or route through a supported value type.

##### P1: Transactions are not rolled back on drop or commit failure

`commit_transaction()` takes the transaction out of `self.txn` before awaiting commit (`crates/rivers-plugin-neo4j/src/lib.rs:187-197`). If commit fails, the connection has already lost the handle and cannot roll it back. There is also no `Drop` path to abort an active transaction if the connection is discarded after an error.

Bolt transactions can hold locks. The wrapper should preserve or explicitly roll back the transaction on commit error and on connection teardown.

##### P2: Cypher injection is delegated entirely to callers

Parameters are bound safely when callers use `$name` placeholders (`crates/rivers-plugin-neo4j/src/lib.rs:243-265`), but `query.statement` itself is accepted as raw Cypher and executed (`crates/rivers-plugin-neo4j/src/lib.rs:103-141`). That is probably the intended low-level driver API, but it means schema/preflight must enforce that user input reaches Cypher only through parameters.

The review did not find string interpolation inside the driver. The missing piece is a test matrix that proves DataView translation always binds params for this driver.

##### P2: Complex Neo4j types are dropped or stringified

`row_to_map()` only extracts primitive scalar types and nodes with primitive properties (`crates/rivers-plugin-neo4j/src/lib.rs:267-311`). Relationships, paths, temporal values, durations, lists, maps, and nulls are not represented cleanly. Unsupported node properties are silently omitted.

For a graph database, silently dropping relationship/path fields is a bad default. Prefer explicit JSON conversion or an error for unsupported values.

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-neo4j/src/lib.rs:335-348`).

#### Repeated patterns

- Non-SQL drivers often expose raw query languages and rely on upstream validation for injection safety.
- NULL handling is inconsistent across drivers; Neo4j maps null to empty string while Cassandra maps null to `CqlValue::Empty`.
- FFI exports lack panic containment.

#### Test gaps

- No test proving mutations executed between `begin_transaction()` and rollback are rolled back.
- No null round-trip test against a live Neo4j instance.
- No relationship/path/temporal type conversion tests.
- No plugin-export panic-safety test.

### `rivers-plugin-cassandra`


Crate: `crates/rivers-plugin-cassandra`
Tier: C

#### Summary

Bug density: medium. The implementation is compact, but it leans on prepare-per-call and named binding in ways that need sharper contracts. Cassandra-specific correctness risks are parameter marker handling, null encoding, paging, and missing retry/idempotency policy.

#### Findings

##### P1: Positional `?` CQL is not supported safely despite Cassandra's common placeholder style

The driver advertises `ParamStyle::ColonNamed` (`crates/rivers-plugin-cassandra/src/lib.rs:59-63`) and binds a `HashMap<String, CqlValue>` into prepared statements (`crates/rivers-plugin-cassandra/src/lib.rs:107-120`, `145-156`, `165-176`). The comment says this avoids alphabetical positional corruption (`crates/rivers-plugin-cassandra/src/lib.rs:165-170`), but there is no validation that the prepared statement actually uses named markers rather than positional `?` markers.

If a caller provides raw CQL with `?` placeholders, a HashMap has no deterministic bind order. The driver should either reject positional markers or implement ordered binding from the SDK's translated parameter list.

##### P1: Paging is not implemented

Both reads and writes use `execute_unpaged()` (`crates/rivers-plugin-cassandra/src/lib.rs:117-121`, `154-157`). Query results are then collected into memory (`crates/rivers-plugin-cassandra/src/lib.rs:123-142`).

Large partitions or wide scans can allocate unbounded memory and cannot return a paging cursor. A Cassandra driver needs explicit page size, paging-state propagation, and a max-row cap.

##### P1: Prepared statements are prepared on every execution

Every query and write calls `session.prepare(query.statement.as_str()).await` (`crates/rivers-plugin-cassandra/src/lib.rs:111-115`, `148-152`). There is no bounded prepared-statement cache, despite `supports_prepared_statements()` returning true (`crates/rivers-plugin-cassandra/src/lib.rs:59-60`).

This avoids an unbounded local cache, but it also turns every request into a prepare+execute round trip. If caching is later added, it must be bounded LRU keyed by normalized query text.

##### P2: NULL is encoded as `CqlValue::Empty`, not necessarily CQL null

`query_value_to_cql()` maps `QueryValue::Null` to `CqlValue::Empty` (`crates/rivers-plugin-cassandra/src/lib.rs:178-187`). On the read side, both missing values and `CqlValue::Empty` map back to `QueryValue::Null` (`crates/rivers-plugin-cassandra/src/lib.rs:190-193`).

Cassandra distinguishes unset, null, and empty in driver APIs. This needs a deliberate contract because it can change whether an update clears a column or leaves it unchanged.

##### P2: Complex CQL types are lossy

UUIDs, timestamps, dates, counters, blobs, lists, and sets get partial mappings (`crates/rivers-plugin-cassandra/src/lib.rs:190-214`), but maps, tuples, durations, decimals, inet, and UDTs fall through to `Debug` strings (`crates/rivers-plugin-cassandra/src/lib.rs:214`). Timestamps become raw integer milliseconds and dates become raw day offsets (`crates/rivers-plugin-cassandra/src/lib.rs:204-205`).

That is acceptable only if documented as a lowest-common-denominator mode. Otherwise, callers lose type information silently.

##### P2: DDL/admin execution is blocked but not supported in init

`execute()` applies the SDK guard (`crates/rivers-plugin-cassandra/src/lib.rs:75-80`), but the connection does not override `ddl_execute()`. CQL `CREATE KEYSPACE` and `CREATE TABLE` statements will be rejected from normal execution and unsupported from init context.

If this is intentional, document it. If Cassandra init DDL is expected, the driver contract is incomplete.

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-cassandra/src/lib.rs:220-229`).

#### Repeated patterns

- Parameter binding behavior differs by driver and is not conformance-tested with adversarial parameter order.
- Complex backend-native types often degrade to strings or JSON without a visible compatibility contract.
- Plugin exports lack panic containment.

#### Test gaps

- No test rejecting or correctly binding positional `?` markers.
- No paging-state test or max-result test.
- No null/unset live test.
- No complex CQL type round-trip tests.

### `rivers-plugin-mongodb`


Crate: `crates/rivers-plugin-mongodb`
Tier: C

#### Summary

Bug density: high. The basic CRUD path is readable and uses the official MongoDB driver, but transaction methods are effectively disconnected from CRUD, and the driver accepts raw BSON/JSON query documents with no operator policy.

#### Findings

##### P1: Transaction methods do not apply to CRUD operations

`MongoConnection` stores `session: Option<ClientSession>` (`crates/rivers-plugin-mongodb/src/lib.rs:88-92`) and implements begin/commit/rollback (`crates/rivers-plugin-mongodb/src/lib.rs:130-170`). But `exec_find`, `exec_insert`, `exec_update`, and `exec_delete` call collection methods without using the session (`crates/rivers-plugin-mongodb/src/lib.rs:196-290`).

Like Neo4j, this violates the SDK transaction contract. A caller can begin a transaction, perform writes outside it, and then roll back an unused transaction.

##### P1: Driver implements transaction methods but `MongoDriver` does not advertise transaction support

`DatabaseDriver::supports_transactions()` defaults to false (`crates/rivers-driver-sdk/src/traits.rs:573-576`), and `MongoDriver` does not override it (`crates/rivers-plugin-mongodb/src/lib.rs:31-84`). That creates contract ambiguity: the connection exposes transaction methods, but the factory says the driver does not support transactions.

Either remove the connection methods or advertise support and route all operations through `*_with_session`.

##### P1: Raw filter documents allow operator injection by design

`resolve_filter()` accepts JSON statement `filter` and converts it directly into BSON (`crates/rivers-plugin-mongodb/src/lib.rs:184-194`). `split_filter_and_fields()` accepts `_filter` JSON and inserts keys directly (`crates/rivers-plugin-mongodb/src/lib.rs:380-410`). `QueryValue::Json` converts to BSON without filtering operators (`crates/rivers-plugin-mongodb/src/lib.rs:313-327`).

That enables `$ne`, `$where`, `$regex`, and other operators anywhere a caller can influence filter JSON. If this raw-power mode is intended, it needs an explicit "unsafe/raw query" boundary and a safe equality-filter builder for user input.

##### P1: Updates with no `_filter` update every document in the collection

`split_filter_and_fields()` returns `(Document::new(), params.clone())` when `_filter` is absent (`crates/rivers-plugin-mongodb/src/lib.rs:407-410`). `exec_update()` then calls `update_many(filter, update)` (`crates/rivers-plugin-mongodb/src/lib.rs:250-263`).

That makes `update` without `_filter` a collection-wide update. The driver should reject missing filters unless the caller explicitly opts into all-document updates.

##### P1: Deletes can also target every document

`exec_delete()` builds the filter from all parameters and calls `delete_many()` (`crates/rivers-plugin-mongodb/src/lib.rs:273-282`). If parameters are empty, the filter is `{}` and every document is deleted.

This is the same destructive-default bug class as the update path. Require a filter or an explicit `allow_all=true`.

##### P2: Connection URI options are hand-concatenated

The base URI is built with `format!("mongodb://{}:{}/", host, port)`, and `replicaSet` is appended by string formatting (`crates/rivers-plugin-mongodb/src/lib.rs:41-50`). Credentials are correctly kept out of the URI (`crates/rivers-plugin-mongodb/src/lib.rs:54-63`), but `replica_set` is not URL-encoded.

Prefer structured `ClientOptions` fields for replica set/read concern/write concern instead of query-string concatenation.

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-mongodb/src/lib.rs:413-426`).

#### Repeated patterns

- Transaction methods exist in multiple non-SQL drivers but are not actually threaded through execution.
- Raw JSON query documents bypass safe parameter/equality semantics.
- Destructive operations default to "all rows/documents" when filters are omitted.

#### Test gaps

- No transaction rollback test proving writes are reverted.
- No test rejecting update/delete with empty filter.
- No operator-injection tests for `$ne`, `$where`, `$lookup`, or `$regex`.
- No write concern/read concern default tests.

### `rivers-plugin-elasticsearch`


Crate: `crates/rivers-plugin-elasticsearch`
Tier: C

#### Summary

Bug density: medium-high. The driver is a direct REST wrapper. The main hazards are HTTP client defaults, unescaped path construction, unbounded responses, and raw Query DSL pass-through.

#### Findings

##### P1: HTTP client has no explicit timeout

`connect()` uses `Client::new()` (`crates/rivers-plugin-elasticsearch/src/lib.rs:49`). Every request then calls `.send().await` with no connect, read, or total deadline (`crates/rivers-plugin-elasticsearch/src/lib.rs:51-56`, `207-212`, `260-265`, `303-308`, `331-335`, `355-359`).

An Elasticsearch node or proxy that accepts a connection and stalls can hang a Rivers worker indefinitely. This repeats across the HTTP-backed plugins.

##### P1: Index names and document IDs are interpolated into paths without percent-encoding

Paths are built with `format!("/{}/_search", index)`, `format!("/{}/_doc", index)`, `format!("/{}/_update/{}", index, id)`, and `format!("/{}/_doc/{}", index, id)` (`crates/rivers-plugin-elasticsearch/src/lib.rs:188-191`, `255-258`, `288-292`, `326-329`). `resolve_index()` accepts JSON `index` or `query.target` directly (`crates/rivers-plugin-elasticsearch/src/lib.rs:171-186`), and `extract_id()` accepts raw string IDs (`crates/rivers-plugin-elasticsearch/src/lib.rs:419-430`).

Names containing `/`, `?`, `#`, or path traversal-looking segments can change the requested endpoint. Use URL path segment encoding or a typed request builder.

##### P1: Raw Query DSL body is accepted with no operation allowlist

`exec_search()` accepts `statement.body` directly and sends it to `_search` (`crates/rivers-plugin-elasticsearch/src/lib.rs:188-225`). This is structured JSON, so it avoids string interpolation, but it still allows callers to choose arbitrary DSL features.

That includes expensive queries, scripts if enabled by the cluster, wildcard-heavy clauses, and cross-index behaviors depending on the target. Rivers needs an explicit raw-query boundary or schema-level allowlist.

##### P1: Responses and error bodies are unbounded

Error paths call `resp.text().await.unwrap_or_default()` (`crates/rivers-plugin-elasticsearch/src/lib.rs:214-219`, `267-272`, `310-315`, `337-342`). Success paths deserialize full JSON bodies with `resp.json().await` (`crates/rivers-plugin-elasticsearch/src/lib.rs:222-225`, `275-278`) and collect all hits returned in the response (`crates/rivers-plugin-elasticsearch/src/lib.rs:227-253`).

There is no response byte limit, hit cap, or timeout, so a large response can allocate unbounded memory.

##### P2: Bulk and scroll APIs are not implemented

I did not find scroll or bulk support in this crate. That means there is no scroll-context leak in current code, but also no partial-failure handling for bulk writes. If those operations are added later, they need explicit close-scroll and per-document error accounting.

##### P2: Scheme defaults to plaintext HTTP

`scheme` defaults to `"http"` (`crates/rivers-plugin-elasticsearch/src/lib.rs:42-48`). For production deployments this should default to TLS or require explicit `scheme = "http"` for local/dev use.

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-elasticsearch/src/lib.rs:433-446`).

#### Repeated patterns

- HTTP plugins lack a shared client policy for timeout, TLS defaults, path encoding, and response-size limits.
- Structured JSON avoids string injection but still needs an operation allowlist for powerful backend DSLs.
- Plugin exports lack panic containment.

#### Test gaps

- No timeout tests with a server that accepts and stalls.
- No path-segment encoding tests for index and document IDs.
- No oversized response/error-body tests.
- No raw DSL policy tests.

### `rivers-plugin-couchdb`


Crate: `crates/rivers-plugin-couchdb`
Tier: C

#### Summary

Bug density: high. This driver has a timeout, which is better than the other HTTP wrappers, but it also performs unsafe JSON string replacement, misses status checks, and builds document/view URLs without path encoding.

#### Findings

##### P1: Mango selector parameter substitution is unsafe JSON string replacement

`exec_find()` clones the statement and replaces `$name` with a hand-built string for each parameter (`crates/rivers-plugin-couchdb/src/lib.rs:167-187`). String parameters are inserted without JSON quoting or escaping (`crates/rivers-plugin-couchdb/src/lib.rs:174-184`) before `serde_json::from_str()` parses the result (`crates/rivers-plugin-couchdb/src/lib.rs:187-188`).

This is both brittle and injectable. A user value containing quotes, braces, or CouchDB operators can alter the selector structure. Use `serde_json::Value` substitution, not string replacement.

##### P1: Document IDs, database names, design docs, and view names are interpolated into URLs

The base database URL is `format!("{}/{}", base_url, database)` (`crates/rivers-plugin-couchdb/src/lib.rs:109-112`). Document operations use `format!("{}/{}", self.db_url(), doc_id)` (`crates/rivers-plugin-couchdb/src/lib.rs:247-264`, `330-378`, `399-431`). Views use `format!("{}/_design/{}/_view/{}", ..., parts[0], parts[1])` (`crates/rivers-plugin-couchdb/src/lib.rs:451-466`).

None of those path segments are percent-encoded. IDs and view names containing `/`, `?`, or `#` can address a different resource than intended.

##### P1: Insert/get/view parse JSON without checking HTTP status first

`exec_insert()` sends POST and immediately parses JSON (`crates/rivers-plugin-couchdb/src/lib.rs:293-327`). `exec_get()` only special-cases 404, then parses any other status as JSON (`crates/rivers-plugin-couchdb/src/lib.rs:247-291`). `exec_view()` parses JSON without checking status (`crates/rivers-plugin-couchdb/src/lib.rs:482-523`).

That can report success for error envelopes or turn HTTP errors into confusing parse failures. Every operation should check status before consuming the body.

##### P1: Raw document bodies can set `_id`, `_rev`, and special fields without policy

Insert and update accept the whole statement as a JSON document when present (`crates/rivers-plugin-couchdb/src/lib.rs:293-306`, `359-376`). There is no check for `_id`, `_rev`, `_deleted`, attachment fields, or design-document writes.

Some of that may be legitimate admin functionality, but it is not separated from normal `execute()`. The SDK guard only knows operation tokens and SQL-like DDL, so it will not block a normal `"insert"` that writes a design document.

##### P2: TLS and auth-cookie lifecycle are not implemented

The base URL is always `http://host:port` (`crates/rivers-plugin-couchdb/src/lib.rs:43-45`), and auth uses basic auth per request (`crates/rivers-plugin-couchdb/src/lib.rs:46-64`, `114-121`). There is no HTTPS option, cookie session flow, cookie refresh, or explicit "basic auth only" policy.

##### P2: Response bodies are unbounded

The client has a 30 second timeout (`crates/rivers-plugin-couchdb/src/lib.rs:55-58`), but all `.json()` and `.text()` calls read full response bodies into memory (`crates/rivers-plugin-couchdb/src/lib.rs:217-225`, `279-282`, `315-318`, `349-352`, `421-424`, `489-492`).

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-couchdb/src/lib.rs:598-611`).

#### Repeated patterns

- HTTP wrappers need one shared URL/path encoding and response-limit helper.
- Raw backend JSON documents/DSLs need explicit safe-vs-raw policy.
- Plugin exports lack panic containment.

#### Test gaps

- No selector injection tests with quotes/operators in parameter values.
- No path encoding tests for document IDs and view names.
- No non-2xx status tests for insert/get/view.
- No HTTPS/session-cookie tests.

### `rivers-plugin-influxdb`


Crate: `crates/rivers-plugin-influxdb`
Tier: C

#### Summary

Bug density: medium-high. The line protocol helpers are better tested than many drivers here, but measurement escaping is incomplete, batching can silently lose writes, and the HTTP client has no timeout or body-size policy.

#### Findings

##### P1: Measurement names are not escaped in generated line protocol

`build_line_protocol()` escapes tag keys, tag values, and field keys, but it writes the measurement name directly from `measurement` or `query.target` (`crates/rivers-plugin-influxdb/src/protocol.rs:92-96`, `175`). The escape helper exists for keys (`crates/rivers-plugin-influxdb/src/protocol.rs:178-183`), but it is not applied to measurement.

Measurement names also need escaping for spaces and commas. This can silently write to the wrong measurement or produce invalid line protocol.

##### P1: Batched writes are acknowledged before they are durably sent

When batching is enabled, `execute("write")` pushes a line into memory and returns `affected_rows: 1` without sending unless size/time thresholds fire (`crates/rivers-plugin-influxdb/src/batching.rs:80-103`). `Drop` cannot await and only logs if buffered lines remain (`crates/rivers-plugin-influxdb/src/batching.rs:122-134`).

That means a successful write result can be lost on shutdown or connection drop. `close()` is not part of `Connection`, so the current design has no reliable flush-on-shutdown hook.

##### P1: Batched flush drops data before confirming write success

`flush_buffer()` joins the buffer, records the count, clears the buffer, then sends the HTTP request (`crates/rivers-plugin-influxdb/src/batching.rs:31-60`). If the request fails or returns non-success, the lines are already gone (`crates/rivers-plugin-influxdb/src/batching.rs:62-68`).

For time-series writes, this is silent data loss. Keep the batch until the server confirms success or return it to the buffer on failure.

##### P1: HTTP client has no explicit timeout

`connect()` uses `Client::new()` (`crates/rivers-plugin-influxdb/src/driver.rs:37`). Ping, query, write, and batch flush all await HTTP requests without explicit deadlines (`crates/rivers-plugin-influxdb/src/connection.rs:42-55`, `78-100`, `133-149`, `159-174`; `crates/rivers-plugin-influxdb/src/batching.rs:51-68`).

This repeats the HTTP-driver hang risk.

##### P2: Flux queries and raw line protocol are direct pass-throughs

Flux comes from `query.statement` and is sent as the request body (`crates/rivers-plugin-influxdb/src/connection.rs:64-87`). `_line_protocol` bypasses all escaping and validation (`crates/rivers-plugin-influxdb/src/protocol.rs:87-90`).

That may be necessary for power users, but it should be an explicit raw mode. Normal user input should use structured builders.

##### P2: Timestamp precision is assumed to be nanoseconds

The write URL omits InfluxDB's `precision` query parameter (`crates/rivers-plugin-influxdb/src/connection.rs:122-129`), and the timestamp comment says nanoseconds (`crates/rivers-plugin-influxdb/src/protocol.rs:168-173`). There is no option to specify ms/us/s precision, so callers passing millisecond timestamps will silently write points thousands or millions of times off.

##### P2: CSV parsing is ad hoc

`parse_csv_response()` splits lines on commas (`crates/rivers-plugin-influxdb/src/protocol.rs:19-79`). It does not implement CSV quoting/escaping, so string fields containing commas or quotes will parse incorrectly.

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-influxdb/src/lib.rs:28-39`).

#### Repeated patterns

- HTTP client timeout and response-size policy is missing across most HTTP drivers.
- Raw backend languages are accepted without a clear safe/raw split.
- Buffered/broker-like drivers need an explicit flush lifecycle in the SDK.

#### Test gaps

- No measurement escaping test for spaces/commas.
- No batch failure retention test.
- No shutdown flush test.
- No timestamp precision tests.
- No quoted CSV parsing tests.

### `rivers-plugin-redis-streams`


Crate: `crates/rivers-plugin-redis-streams`
Tier: C

#### Summary

Bug density: medium. The basic XADD/XREADGROUP/XACK path is clear and uses a finite block timeout. The missing pieces are PEL reclaim, stream trimming, subject/stream naming policy, and safe connection-string construction for passwords.

#### Findings

##### P1: Nack leaves messages in the PEL with no reclaim path

`nack()` intentionally does nothing but log (`crates/rivers-plugin-redis-streams/src/lib.rs:422-432`). `receive()` always reads new messages with `>` (`crates/rivers-plugin-redis-streams/src/lib.rs:362-383`). I did not find `XAUTOCLAIM`, `XPENDING`, `XCLAIM`, or a startup pass over pending entries.

That means a failed message stays pending indefinitely and will not be redelivered by this consumer loop. The comment says restart with `>` replaced by `0`, but the code never does that.

##### P1: Streams are unbounded by default

Producer `publish()` uses `XADD stream * payload ...` with no `MAXLEN` or `MINID` trimming (`crates/rivers-plugin-redis-streams/src/lib.rs:317-344`). There is no driver option for approximate max length.

Long-running deployments can grow Redis memory without bound.

##### P1: Passwords are interpolated into Redis URLs without percent-encoding

Cluster URLs use `format!("redis://:{}@{h}", params.password)` (`crates/rivers-plugin-redis-streams/src/lib.rs:166-175`), and single-node URLs use `format!("redis://:{}@{}:{}/{}", password, host, port, database)` (`crates/rivers-plugin-redis-streams/src/lib.rs:191-202`).

Passwords containing `@`, `/`, `:`, `%`, or `#` can break URI parsing. Use structured Redis connection info or URL-encode credentials.

##### P2: Consumer group creation handles BUSYGROUP, but group lifecycle is incomplete

`XGROUP CREATE ... MKSTREAM` handles existing groups by matching `BUSYGROUP` in the error string (`crates/rivers-plugin-redis-streams/src/lib.rs:103-133`). There is no consumer cleanup (`XGROUP DELCONSUMER`), tombstone management, or group deletion policy on close (`crates/rivers-plugin-redis-streams/src/lib.rs:434-440`).

Error-string matching is also brittle; use Redis error kinds/codes if available.

##### P2: `receive()` loops forever and ignores reconnect settings

The SDK config includes `reconnect_ms` (`crates/rivers-driver-sdk/src/broker.rs:130-143`), but this driver never uses it. `receive()` loops forever on Nil timeouts and returns errors directly on connection problems (`crates/rivers-plugin-redis-streams/src/lib.rs:362-399`).

The caller may own reconnect, but the driver should document the boundary and avoid tight failure loops.

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-redis-streams/src/lib.rs:444-457`).

#### Repeated patterns

- Broker drivers implement basic receive/ack but not recovery semantics.
- Connection URLs often embed credentials by string formatting.
- Plugin exports lack panic containment.

#### Test gaps

- No PEL reclaim/redelivery tests.
- No stream trimming tests.
- No password URL-encoding tests.
- No consumer cleanup tests.

### `rivers-plugin-nats`


Crate: `crates/rivers-plugin-nats`
Tier: C

#### Summary

Bug density: medium. This is a core NATS pub/sub driver, not JetStream. That is fine if documented as at-most-once fire-and-forget, but the metadata and ack/nack shape imply stronger broker semantics than the implementation provides.

#### Findings

##### P1: `receive()` can block forever

`receive()` awaits `subscriber.next().await` with no timeout or cancellation check (`crates/rivers-plugin-nats/src/lib.rs:176-181`). The SDK says receive blocks until a message is available (`crates/rivers-driver-sdk/src/broker.rs:218-225`), but in practice Rivers needs a way to shut down or reconnect workers cleanly.

Add an explicit receive timeout or ensure the bridge always wraps this in cancellation.

##### P1: Ack/nack are no-ops while metadata is shaped like durable stream metadata

`ack()` and `nack()` return `Ok(())` without broker interaction (`crates/rivers-plugin-nats/src/lib.rs:213-223`). The code comments say this is core NATS, not JetStream. But messages are emitted with `BrokerMetadata::Nats { sequence, stream, consumer }` (`crates/rivers-plugin-nats/src/lib.rs:196-210`), whose SDK docs call it JetStream-specific metadata (`crates/rivers-driver-sdk/src/broker.rs:104-112`).

This is a contract mismatch. Either expose core NATS as at-most-once with distinct metadata, or implement JetStream ack/nack semantics.

##### P1: Subject wildcards are accepted from config/user input without validation

Subjects come from `config.subscriptions.first().topic` or `params.database` (`crates/rivers-plugin-nats/src/lib.rs:83-90`), and publish destinations come from `message.destination` (`crates/rivers-plugin-nats/src/lib.rs:101-108`). There is no policy around `*` or `>` wildcards.

Wildcard subscription may be intended, but it should be explicit because a user-controlled subject can subscribe to more than the app is authorized to observe.

##### P2: Connect has no explicit timeout and ignores reconnect config

`nats_connect()` calls `connect_options.connect(&url).await` directly (`crates/rivers-plugin-nats/src/lib.rs:64-81`). Tests wrap bad hosts in a 10 second timeout (`crates/rivers-plugin-nats/src/lib.rs:360-402`), which is a clue the driver itself has no deadline. `BrokerConsumerConfig.reconnect_ms` is not used.

##### P2: Close drains the whole client but does not explicitly unsubscribe

`close()` relies on dropping `Subscriber` and drains the client (`crates/rivers-plugin-nats/src/lib.rs:225-233`). For shared or future multi-subscription clients, explicit unsubscribe is safer and easier to reason about.

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-nats/src/lib.rs:278-291`).

#### Repeated patterns

- Broker ack/nack semantics are inconsistent and under-specified across drivers.
- Reconnect settings are accepted by SDK config but not used by these driver implementations.
- Plugin exports lack panic containment.

#### Test gaps

- No receive timeout/cancellation test.
- No wildcard subject policy tests.
- No reconnect restoration tests.
- No ack/nack contract test distinguishing core NATS from JetStream.

### `rivers-plugin-rabbitmq`


Crate: `crates/rivers-plugin-rabbitmq`
Tier: C

#### Summary

Bug density: medium. This driver does several important things right: durable queue declaration, manual ack/nack, persistent messages, and publisher confirms. The high-risk gaps are no prefetch, no publish confirm timeout, requeue-on-nack poison loops, and no connection recovery behavior.

#### Findings

##### P1: Consumers do not set QoS/prefetch

The consumer channel declares the queue and calls `basic_consume()` (`crates/rivers-plugin-rabbitmq/src/lib.rs:70-116`), but there is no `basic_qos`/prefetch configuration. With manual ack, RabbitMQ can deliver too many unacked messages to one consumer depending on broker/channel defaults.

This can create memory pressure and unfair dispatch. Prefetch should be a required or defaulted option.

##### P1: Publisher confirms have no timeout

`publish()` awaits both `basic_publish()` and the returned confirm future (`crates/rivers-plugin-rabbitmq/src/lib.rs:203-215`). There is no timeout around either await.

If the broker/channel gets stuck during confirm handling, a Rivers publish can hang indefinitely.

##### P1: `nack()` always requeues, enabling poison-message loops

`nack()` calls `basic_nack` with `requeue: true` (`crates/rivers-plugin-rabbitmq/src/lib.rs:313-328`). There is no delivery-count policy, dead-letter exchange integration, delay/backoff, or option to drop.

A permanently bad message can spin forever and starve useful work.

##### P2: Consumer tags can collide across nodes

The consumer tag is derived as `{group_prefix}.{app_id}.{datasource_id}.consumer` (`crates/rivers-plugin-rabbitmq/src/lib.rs:96-109`). It does not include `node_id`, even though the SDK config provides one (`crates/rivers-driver-sdk/src/broker.rs:130-143`).

If multiple Rivers nodes consume the same queue with the same app/datasource IDs, tags may collide. Include `node_id` or let the broker generate tags.

##### P2: No connection recovery or reconnect policy

The driver creates a connection/channel once (`crates/rivers-plugin-rabbitmq/src/lib.rs:35-67`, `70-116`). `receive()` returns an error if the consumer stream ends or a delivery errors (`crates/rivers-plugin-rabbitmq/src/lib.rs:248-255`). I did not find recovery or use of `reconnect_ms`.

The caller may reconnect, but that boundary needs tests because AMQP channel/consumer recovery is subtle.

##### P2: Queue is used as routing key with default exchange only

Publisher sends to exchange `""` and routing key `message.destination` or default queue (`crates/rivers-plugin-rabbitmq/src/lib.rs:182-212`). Schema validation says POST requires an `exchange` field (`crates/rivers-plugin-rabbitmq/src/lib.rs:342-386`), but the runtime publish path does not use schema exchange metadata.

That is wiring drift between schema contract and driver behavior.

##### P2: Plugin exports can unwind across FFI

The plugin exports are plain `extern "C"` functions without `catch_unwind` (`crates/rivers-plugin-rabbitmq/src/lib.rs:389-402`).

#### Repeated patterns

- Broker drivers omit reconnect and shutdown semantics even though SDK config exposes reconnect timing.
- Ack/nack policies are fixed in code instead of tied to a failure policy.
- Plugin exports lack panic containment.

#### Test gaps

- No QoS/prefetch tests.
- No publish-confirm timeout test.
- No poison-message/dead-letter behavior tests.
- No consumer tag uniqueness test across nodes.
- No exchange routing test matching schema validation.

<!-- END docs/review consolidated -->
