# Tasks — Wire All Unwired Bundle Config Fields

**Source:** Config audit found ~35 fields parsed from TOML but never read by the server
**Total:** 7 PRs, 34 fields, 1 deferred (write_batch)

---

## PR 1: DataView Caching Policy (5 fields) — Low complexity
**Branch:** `feature/dataview-caching`

- [ ] **T1.1**: Scan DataView configs in bundle_loader.rs:229-237 to build `DataViewCachingPolicy` from aggregate caching settings instead of `::default()`
- [ ] **T1.2**: Apply same logic in hot reload path (bundle_loader.rs:640-648)
- [ ] **T1.3**: Verify `invalidates` field is already wired (confirmed in dataview_engine.rs:547)
- [ ] **T1.4**: Verify per-view `ttl_seconds` override already works (dataview_engine.rs:539)
- [ ] **T1.5**: Test: DataView with `l2_enabled = true` → cache constructed with L2 active

**Fields:** `caching.ttl_seconds`, `caching.l1_enabled`, `caching.l1_max_entries`, `caching.l2_enabled`, `caching.l2_max_value_bytes`

---

## PR 2: Per-Method CRUD Queries (12 fields) — Medium complexity
**Branch:** `feature/per-method-crud`

- [ ] **T2.1**: Add `method: &str` param to `DataViewExecutor::execute()` (dataview_engine.rs:465)
- [ ] **T2.2**: Pass `ctx.request.method` in view_engine.rs:501
- [ ] **T2.3**: Pass `"GET"` for queries, `"POST"` for mutations in graphql.rs:594
- [ ] **T2.4**: Update host callback in engine_loader.rs:361 — pass method from task context
- [ ] **T2.5**: Update V8 host callback in v8_engine.rs:962 — extract method from JS args
- [ ] **T2.6**: Update polling.rs:454,550,666 — pass `"GET"`
- [ ] **T2.7**: Test: DataView with `post_query = "INSERT..."`, POST request uses INSERT not GET query

**Fields:** `get_query`, `post_query`, `put_query`, `delete_query`, `get_schema`, `post_schema`, `put_schema`, `delete_schema`, `get_parameters`, `post_parameters`, `put_parameters`, `delete_parameters`

---

## PR 3: DataView Validation (2 fields) — Low-Medium complexity
**Branch:** `feature/dataview-validation`
**Depends on:** PR 2 (needs method param for schema_for_method)

- [ ] **T3.1**: Confirm `strict_parameters` already wired (dataview_engine.rs:207-217)
- [ ] **T3.2**: Pre-load schema JSON for DataViews with `validate_result = true` during bundle loading
- [ ] **T3.3**: After `conn.execute()`, validate result rows against loaded schema if `config.validate_result`
- [ ] **T3.4**: Return `DataViewError::Schema` on validation failure
- [ ] **T3.5**: Test: DataView with `validate_result = true` + schema requiring "id" → error on missing field

**Fields:** `validate_result`, `strict_parameters`

---

## PR 4: Broker Consumer Config (5 fields) — Low complexity
**Branch:** `feature/broker-consumer-config`

- [ ] **T4.1**: Read `DatasourceConfig.consumer` in bundle_loader.rs:373-395 instead of hardcoding
- [ ] **T4.2**: Use `consumer.group_prefix` (default "rivers") instead of hardcoded `"rivers"`
- [ ] **T4.3**: Use `consumer.reconnect_ms` (default 5000) instead of hardcoded `5000`
- [ ] **T4.4**: Convert `on_failure.mode` string to FailureMode enum instead of hardcoded `Drop`
- [ ] **T4.5**: Add `max_retries: u32` to BrokerConsumerBridge, wrap publish in retry loop
- [ ] **T4.6**: Log warning if `ack_mode = "manual"` (only "auto" supported)
- [ ] **T4.7**: Test: `consumer.group_prefix = "myapp"` → broker connects with "myapp" in logs

**Fields:** `consumer.group_prefix`, `consumer.reconnect_ms`, `consumer.subscriptions[].on_failure`, `consumer.subscriptions[].max_retries`, `consumer.subscriptions[].ack_mode`

---

## PR 5: Datasource Event Handlers (2 fields) — Medium complexity
**Branch:** `feature/datasource-event-handlers`
**Approach:** EventBus subscriber → ProcessPool CodeComponent execution

- [ ] **T5.1**: Read `DatasourceConfig.event_handlers` during bundle loading in bundle_loader.rs
- [ ] **T5.2**: For each handler ref (module + entrypoint), register EventBus subscriber at App priority
- [ ] **T5.3**: Map `on_connection_failed` handlers to `DATASOURCE_CIRCUIT_OPENED` + `DATASOURCE_HEALTH_CHECK_FAILED` events
- [ ] **T5.4**: Map `on_pool_exhausted` handlers to `CONNECTION_POOL_EXHAUSTED` event
- [ ] **T5.5**: Subscriber dispatches to ProcessPool to execute CodeComponent handler with event context in `ctx.data`
- [ ] **T5.6**: Test: `on_connection_failed` handler fires when circuit breaker opens

**Fields:** `event_handlers.on_connection_failed`, `event_handlers.on_pool_exhausted`
**Reuses:** ProcessPool dispatch (same as MessageConsumer), EventBus subscription pattern

---

## PR 6: Unify CircuitBreaker Config (1 field + refactor) — Medium complexity
**Branch:** `feature/unify-circuit-breaker`

- [ ] **T6.1**: Define canonical `CircuitBreakerConfig` in `rivers-driver-sdk/src/lib.rs` with fields: `enabled`, `failure_threshold`, `window_ms`, `open_timeout_ms`, `half_open_max_trials`
- [ ] **T6.2**: Replace `rivers-runtime/src/datasource.rs` CB config with import from driver-sdk
- [ ] **T6.3**: Replace `riversd/src/pool.rs` CB config with import from driver-sdk
- [ ] **T6.4**: Update `rivers-driver-sdk/src/http_driver.rs` — rename fields to match canonical names or impl `From`
- [ ] **T6.5**: Wire `window_ms` from datasource config through to pool construction
- [ ] **T6.6**: Test: `circuit_breaker.window_ms = 30000` → 30s rolling window used in pool

**Fields:** `CircuitBreakerConfig.window_ms` (+ unify 3 incompatible structs)
**Plugin impact:** `#[serde(default)]` on new fields — existing configs won't break

---

## PR 7: Session Revalidation (1 field) — Medium complexity
**Branch:** `feature/session-revalidation`
**Approach:** `tokio::interval` per SSE/WS connection

- [ ] **T7.1**: In SSE handler (server.rs ~line 950), after initial session auth, spawn revalidation interval task
- [ ] **T7.2**: In WS handler (server.rs ~line 1050), same for WebSocket connections
- [ ] **T7.3**: On each tick, call `SessionManager::validate_session(session_id)` against StorageEngine
- [ ] **T7.4**: If session invalid/expired, close connection gracefully (close frame for WS, end stream for SSE)
- [ ] **T7.5**: Cancel interval task automatically when connection drops (tokio::select! pattern)
- [ ] **T7.6**: Test: `session_revalidation_interval_s = 60` → session rechecked every 60s on SSE

**Field:** `session_revalidation_interval_s`

---

## Deferred — New Feature (not config wiring)

| Field | Reason |
|-------|--------|
| `write_batch` | Needs new BufferedWriter abstraction in driver pipeline (size/interval flush, backpressure). Separate feature PR. |

---

## Execution Order

1. **PR 1** (Caching) — fewest files, no signature changes
2. **PR 4** (Broker Consumer) — self-contained
3. **PR 6** (CB Unification) — refactor before more features depend on it
4. **PR 2** (Per-Method CRUD) — signature change on execute()
5. **PR 3** (Validation) — depends on PR 2
6. **PR 5** (Event Handlers) — depends on ProcessPool being stable
7. **PR 7** (Session Revalidation) — SSE/WS connection changes, do last
