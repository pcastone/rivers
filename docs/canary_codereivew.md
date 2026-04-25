# Canary Code Review Recommendations

Date: 2026-04-24

Scope: static review of the current canary-blocking paths in `riversd`, broker drivers, V8/TypeScript compilation, task-local capability wiring, and canary stream configuration. This is a recommendation file only; no production code changes are included here.

## Executive Summary

The highest-confidence blocker is the broker startup path. `load_and_wire_bundle` calls `wire_streaming_and_events` before completing app wiring, and `wire_streaming_and_events` awaits `broker_driver.create_consumer(...)` inline. The Kafka implementation performs network/metadata work in `create_consumer`, so one unreachable or flaky Kafka broker can delay or prevent all of `riversd` startup. That matches the reported hang at `wire_streaming_and_events -> create_consumer().await`.

The most urgent recommendation is to make broker bridge startup supervision-owned and non-blocking: spawn a bridge task immediately, move consumer creation into that task, and retry with bounded backoff until shutdown. Bundle load should register the desired broker bridge, not synchronously prove the broker is healthy.

The next two production-risk items are also broker-related: Kafka producer construction eagerly connects to Kafka at create time, and Kafka `publish()` ignores `OutboundMessage.destination` because the `PartitionClient` is bound to a single topic during producer creation. Both violate the expected "message destination drives publish target" contract and create avoidable worker-thread stalls when producer lazy-init occurs inside a V8 callback.

## P0 Recommendations

### 1. Make broker consumer creation non-blocking during startup

Evidence:

- `load_and_wire_bundle` describes broker bridge spawning as part of bundle load, before the HTTP router/listener can complete startup (`crates/riversd/src/bundle_loader/load.rs:22-34`).
- `wire_streaming_and_events` awaits `broker_driver.create_consumer(params, &broker_config).await` inline (`crates/riversd/src/bundle_loader/wire.rs:115`).
- Kafka `create_consumer` performs `ClientBuilder::build().await` and then `partition_client(...).await` (`crates/rivers-plugin-kafka/src/lib.rs:75-85`).
- The bridge already has a receive-loop retry concept, but only after a consumer object exists (`crates/riversd/src/broker_bridge.rs:172-219`).

Recommendation:

Move broker driver connection and consumer construction inside the spawned bridge supervisor. Startup should construct a `BrokerBridgeSpec` containing driver name, params, `BrokerConsumerConfig`, failure policy, and shutdown handle, then `tokio::spawn` a loop that:

1. Attempts `create_consumer`.
2. On success, runs `BrokerConsumerBridge::run()`.
3. On failure, publishes/logs `BrokerConsumerError`, sleeps `reconnect_ms` or exponential backoff with jitter, and retries until shutdown.

Do not treat broker reachability as a bundle-load precondition unless the datasource declares an explicit future `startup_required = true` or health policy. Canary needs HTTP endpoints to bind even when Kafka is unhealthy.

Suggested tests:

- Unit test `wire_streaming_and_events` with a broker driver whose `create_consumer` never resolves; assert the wiring future returns quickly.
- Integration test with a bad Kafka host; assert `riversd` binds HTTP and health/admin endpoints while the bridge reports degraded broker health.
- Regression test that shutdown cancels a bridge still retrying consumer creation.

### 2. Stop eager Kafka producer metadata lookup from blocking V8 callbacks

Evidence:

- Kafka `create_producer` currently calls `ClientBuilder::build().await` and `partition_client(...).await` before returning (`crates/rivers-plugin-kafka/src/lib.rs:43-53`).
- V8 handlers run inside blocking worker threads and bridge async host work through runtime handles; blocking those workers on external metadata round trips is high-risk under load (`crates/riversd/src/process_pool/v8_engine/execution.rs:315-329`, `crates/riversd/src/process_pool/v8_engine/task_locals.rs:41-50`).

Recommendation:

Return a lightweight producer handle from `create_producer` without Kafka metadata I/O. Perform connection/topic client acquisition in an async producer supervisor/cache, outside the V8 callback's critical path. If lazy connection is still necessary, make it bounded by timeout and publish a recoverable `DriverError::Connection` instead of tying up a V8 worker indefinitely.

Suggested tests:

- A bad-host producer creation test should return promptly without a 10-second timeout allowance.
- A V8 handler invoking broker publish against an unavailable broker should fail within the configured request timeout and leave the worker reusable.

### 3. Restore persistent MySQL pooling semantics

Evidence:

- `MysqlDriver::connect` creates a new `mysql_async::Pool::new(opts)` for every driver connect call, immediately checks out one connection, and stores that per-call pool on `MysqlConnection` (`crates/rivers-drivers-builtin/src/mysql.rs:45-64`, `crates/rivers-drivers-builtin/src/mysql.rs:160-167`).

Recommendation:

Reintroduce a driver-level or datasource-level pool cache keyed by resolved `ConnectionParams`, or route MySQL through the existing `ConnectionPool` layer so repeated dataview calls do not perform a full MySQL handshake. If the prior pool removal was only to avoid host callback lifetime/runtime issues, keep the `host_callbacks` fix and revert the pool behavior.

Suggested tests:

- Instrument a fake or local MySQL connection factory and assert multiple dataview calls reuse the pool instead of constructing a new `mysql_async::Pool`.
- Canary CRUD should be rerun after this independently from the broker startup fix, because current MySQL failures may still include the known cdylib/Tokio conflict.

### 4. Add a hard timeout around SWC compile, not only `catch_unwind`

Evidence:

- `compile_typescript_with_imports` wraps SWC in `catch_unwind` (`crates/riversd/src/process_pool/v8_config.rs:154-193`).
- It still executes parse/transform/codegen synchronously in-process with no wall-clock or CPU timeout (`crates/riversd/src/process_pool/v8_config.rs:196-302`).
- Bundle load aborts on module cache population errors and waits on all TypeScript compilation before continuing (`crates/riversd/src/bundle_loader/load.rs:116-125`).

Recommendation:

Run SWC compilation under a bounded supervisor. Options, in order of robustness:

1. Process isolation with a timeout for untrusted/pathological TypeScript.
2. Dedicated worker thread plus timeout and "poison worker" discard on overrun.
3. At minimum, make bundle compilation cancellable from the startup path and emit an actionable app failure instead of hanging the deploy pipeline.

Suggested tests:

- Corpus test with deeply nested/pathological TS that previously stresses SWC; assert compilation errors or times out within the configured bound.
- Startup test proving one bad TS app enters failed state without preventing unrelated apps from binding.

## P1 Recommendations

### 5. Fix MessageConsumer app identity and storage namespace propagation

Evidence:

- MessageConsumer dispatch enriches task capabilities with an empty app id: `crate::task_enrichment::enrich(builder, "")` (`crates/riversd/src/message_consumer.rs:313-318`).
- `TaskLocals::set` uses `ctx.app_id` to choose the `ctx.store` namespace, falling back to `app:default` when empty (`crates/riversd/src/process_pool/v8_engine/task_locals.rs:177-190`).
- Canary's Kafka consumer writes `canary:kafka:last_verdict` with `ctx.store.set`, and the verify endpoint reads the same key later (`canary-bundle/canary-streams/libraries/handlers/kafka-consumer.ts:47-62`).

Recommendation:

Carry the app entry point or app id into `MessageConsumerConfig`, then call `task_enrichment::enrich(builder, entry_point)` and include the same `_dv_namespace` convention used by HTTP CodeComponent dispatch. This gives MessageConsumer handlers the same `ctx.store`, keystore, dataview, and log routing identity as REST handlers in the same app.

Suggested tests:

- MessageConsumer handler writes `ctx.store.set("x", ...)`; REST handler in same app reads it; REST handler in another app cannot read it.
- MessageConsumer `ctx.dataview("bare_name")` resolves against the owning app namespace.

### 6. Align EventBus topic wiring with `on_event.topic`, not view ids

Evidence:

- `MessageConsumerConfig::from_view` correctly reads `on_event.topic` (`crates/riversd/src/message_consumer.rs:49-57`).
- Broker bridge subscription construction instead pushes every MessageConsumer `view_id` as a broker subscription topic (`crates/riversd/src/bundle_loader/wire.rs:40-52`).
- Broker bridge publishes framework event type `BrokerMessageReceived`, not the subscription's configured event name or actual topic (`crates/riversd/src/broker_bridge.rs:245-255`).

Recommendation:

Normalize one event contract:

- Broker subscription topic should come from datasource consumer config or `view.on_event.topic`, not from the view id.
- EventBus publish should use the topic/event name that MessageConsumer subscribed to, while including broker metadata in payload.
- Keep `BrokerMessageReceived` as an observability event if useful, but do not make it the only dispatch event for application consumers.

Suggested tests:

- A MessageConsumer with `on_event.topic = "canary.kafka.test"` dispatches when Kafka receives that topic.
- A view id different from topic still works.

### 7. Bound or evict the parsed source-map cache

Evidence:

- Parsed source maps are kept in a process-global `HashMap<PathBuf, Arc<SourceMap>>` (`crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs:21-24`).
- Entries are cleared on module cache install/hot reload, but there is no size cap or per-app accounting (`crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs:75-83`).

Recommendation:

Add a bounded LRU or cap by module count/bytes. Since source maps can be large and are parsed on error paths, a cap prevents repeated bad apps or large bundles from turning stack-trace support into unbounded resident memory growth.

Suggested tests:

- Load more source maps than the cap; assert older entries are evicted and can be reparsed.
- Hot reload still clears the cache.

### 8. Sanitize absolute paths in stack traces and module errors

Evidence:

- V8 module origins use absolute paths (`crates/riversd/src/process_pool/v8_engine/execution.rs:241-256`).
- Stack trace fallback prints the raw script name (`crates/riversd/src/process_pool/v8_engine/execution.rs:851-855`).
- Module cache miss and module resolution errors also include absolute paths (`crates/riversd/src/process_pool/v8_engine/execution.rs:231-238`, `crates/riversd/src/process_pool/v8_engine/execution.rs:696-702`).

Recommendation:

Introduce a single path redaction helper that maps bundle files to `{entry_point}/libraries/...` or `{app}/...` before logging or returning errors. Keep absolute paths only in debug logs gated for local development.

Suggested tests:

- Release-mode error response for a thrown TS handler contains app-relative paths only.
- Module resolution errors do not reveal workspace paths.

### 9. Remove silent disk fallback for module-cache misses in production

Evidence:

- `resolve_module_source` falls back to reading and live-compiling from disk on a module cache miss (`crates/riversd/src/process_pool/v8_engine/execution.rs:696-710`).

Recommendation:

Treat cache misses as hard errors for bundle-managed modules. Keep `_source` for tests and possibly a clearly named dev-only flag for legacy modules, but production dispatch should not silently execute code that escaped bundle-time validation, cycle detection, and source-map population.

Suggested tests:

- Cached module executes.
- Non-cached on-disk module fails in production mode.
- `_source` test injection remains available to unit tests.

### 10. Add a thread-local panic-safety integration test

Evidence:

- V8 now has many task locals managed by `TaskLocals` (`crates/riversd/src/process_pool/v8_engine/task_locals.rs:41-150`).
- `TaskLocals::drop` clears them and performs transaction auto-rollback before clearing `RT_HANDLE` (`crates/riversd/src/process_pool/v8_engine/task_locals.rs:242-272`).

Recommendation:

Add an integration test that forces handler panic/error/timeout and then dispatches a second handler on the same worker/thread, asserting no stale storage namespace, driver factory, dataview namespace, transaction state, module registry, keystore, or direct datasource survives.

Suggested tests:

- Panic/timeout task sets every capability it can; next task observes clean defaults or its own app-specific values.
- Transaction state is rolled back before `RT_HANDLE` clears.

## Canary Failure Categorization Guidance

Keep the canary board split into these lanes:

- Startup blocker: broker consumer creation during bundle load. This is the one issue that prevents useful test signal because `riversd` may never bind HTTP.
- BR-related behavior: broker consumer dispatch and store persistence/visibility. Current code supports the reported suspicion that app identity/namespace wiring is incomplete for MessageConsumer.
- Pre-existing infra/harness: PG, MySQL runtime conflict, NoSQL driver/credential issues, MCP schema check, decorator syntax flag, and HMAC lockbox alias. These should not block merging the broker startup fix if canary boot and unrelated test execution are restored.

## Recommended Fix Order

1. Make broker consumer startup non-blocking and retrying inside its own task.
2. Add a startup regression test proving bad Kafka cannot prevent HTTP bind.
3. Fix MessageConsumer app identity/topic wiring so Kafka consume/verify has a real chance to pass.
4. Make Kafka producer creation/publish destination semantics non-blocking and contract-correct.
5. Restore MySQL pooling separately from canary startup.
6. Add SWC timeout supervision before treating TypeScript compilation as production-safe.
7. Address source-map bounds, path redaction, module-cache strictness, and thread-local panic tests as hardening work before broadening canary gates.

## Notes On Existing Canary Config

The checked-in `canary-bundle/canary-streams/app.toml` currently has Kafka MessageConsumer and verify views commented out (`canary-bundle/canary-streams/app.toml:157-185`), while `resources.toml` still marks Kafka required (`canary-bundle/canary-streams/resources.toml:1-7`). If the active canary bundle differs from this worktree, verify that the live bundle's MessageConsumer config includes explicit `on_event.topic`; the code path expects it for `MessageConsumerConfig`, but the bridge startup path currently builds broker subscriptions from view ids instead.
