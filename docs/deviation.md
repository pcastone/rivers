# Rivers — Spec vs Implementation Deviation Report

**Generated:** 2026-03-19
**Methodology:** Each spec document in `docs/arch/` was read end-to-end and compared against the corresponding Rust source code. Feature names follow `docs/rivers-feature-inventory.md`. Only non-matching rows are included.
**Baseline:** 1386 tests passing, 15 crates, 16 drivers

## Summary

| Group | Specs Audited | Deviations | Missing | Partial | Bugs | Additions |
|-------|---------------|------------|---------|---------|------|-----------|
| 1. HTTPD Core | httpd-spec, TLS design | 14 | 6 | 0 | 0 | 2 |
| 2. Views + Streaming + Polling | view-layer, streaming-rest, polling-views | 10 | 9 | 8 | 0 | 0 |
| 3. Data + Schema + Drivers | data-layer, driver, schema-validation, schema-v2 | 5 | 8 | 10 | 0 | 1 |
| 4. Auth + Session + Admin | auth-session, v1-admin | 19 | 2 | 0 | 0 | 0 |
| 5. HTTP + Plugin Drivers | http-driver, technology-path | 10 | 0 | 0 | 0 | 0 |
| 6. LockBox + Storage + Logging | lockbox, storage-engine, logging | 5 | 8 | 4 | 2 | 0 |
| 7. App + ProcessPool + RPS | application, processpool, rps, app-dev, address-book, shaping, conflicts | 15 | 4 | 3 | 0 | 0 |
| **Total** | **23 documents** | **78** | **37** | **25** | **2** | **3** |

---

## 1. HTTPD Core (rivers-httpd-spec, TLS design)

| Feature (Inventory Name) | Spec Says | Implementation Does | Files Involved | Status |
|---|---|---|---|---|
| **1.4 Middleware Stack — main server order** | §4: outermost→innermost: `compression → body_limit → security_headers → session → rate_limit → shutdown_guard → backpressure → timeout → request_observer → trace_id` | Actual: `request_observer → timeout → backpressure → shutdown_guard → security_headers → trace_id → body_limit → cors → compression`. Session and rate_limit layers absent from global stack; noted as "per-view" in comments | `riversd/src/server.rs:199-225` | Deviation |
| **1.4 Middleware Stack — admin server order** | §4: admin subset: `body_limit → security_headers → timeout → trace_id` | Admin stack: `timeout → admin_auth → security_headers → trace_id → body_limit`. `admin_auth` is extra; `body_limit` is innermost not outermost | `riversd/src/server.rs:253-266` | Deviation |
| **1.5 Security Headers — CSP** | §17: `Content-Security-Policy` not auto-set; operator/reverse-proxy responsibility | `security_headers_middleware` injects `content-security-policy: default-src 'self'` unconditionally | `riversd/src/middleware.rs:140-145` | Deviation |
| **1.6 Error Response Envelope (SHAPE-2) — wire format** | §18: flat `{code, message, details?, trace_id}` | `ErrorResponse` wraps in `{"error":{...}}`. View dispatch uses flat format — two inconsistent formats | `riversd/src/error_response.rs:18-30`, `riversd/src/server.rs:433-445` | Deviation |
| **1.6 Error Response — timeout status code** | §18: timeout → `408 Request Timeout` | `timeout_middleware` returns `504 Gateway Timeout` | `riversd/src/middleware.rs:87` | Deviation |
| **1.7 Backpressure — streaming permit lifetime** | §11.1: streaming responses hold permit for connection lifetime | Permit dropped after `next.run()` returns; no streaming-aware hold | `riversd/src/backpressure.rs:61-65` | Missing |
| **1.8 Graceful Shutdown — resource release ordering** | §13.5: pool drain → BrokerConsumerBridge drain → write batch flush | `wait_for_drain()` waits for inflight=0; no subsequent pool/broker/batch flush | `riversd/src/shutdown.rs:59-64` | Missing |
| **1.9 Rate Limiting — config location** | §10, §19.3: rate limiting in `[app.rate_limit]` in app.toml | Rate limit fields are in `SecurityConfig` as server-level config | `rivers-core/src/config.rs:399-411` | Deviation |
| **1.9 Rate Limiting — default values** | §19.3: `per_minute = 120`, `burst_size = 60` | Code defaults: `requests_per_minute: 600`, `burst_size: 50` | `riversd/src/rate_limit.rs:41-47` | Deviation |
| **1.9 Rate Limiting — session key strategy** | §10.2: three strategies: `ip`, `header`, `session` | Only `Ip` and `CustomHeader` variants; `session` absent | `riversd/src/rate_limit.rs:21-27` | Missing |
| **1.9 Rate Limiting — middleware in stack** | §4: `rate_limit_middleware` at position 5 globally | Function exists but not registered in `build_main_router`; comment says "per-view" | `riversd/src/server.rs:193-194` | Deviation |
| **1.10 Hot Reload — scope** | §16: selectively reloads views, DataViews, static files, security | All-or-nothing `ServerConfig` swap; no selective reload | `riversd/src/hot_reload.rs:220-235` | Deviation |
| **1.10 Hot Reload — ConfigFileChanged event** | §16 step 5: publish `ConfigFileChanged` to EventBus | Never publishes event to EventBus | `riversd/src/hot_reload.rs:171-202` | Missing |
| **1.10 Hot Reload — exclusive lock** | §16 step 1: acquire `hot_reload_lock` | Debounce via `Mutex<Instant>` (500ms); no exclusive lock | `riversd/src/hot_reload.rs:113-166` | Deviation |
| **19 CORS — config location** | §9.3: CORS not in `SecurityConfig` | CORS configured via `SecurityConfig` fields and applied globally | `rivers-core/src/config.rs:383-397` | Deviation |
| **19 CORS — max_age header** | No `max_age` TOML field in spec | `access-control-max-age: 86400` injected unconditionally, not configurable | `riversd/src/cors.rs:115` | Addition |
| **14.2 Health verbose — response fields** | §14.2: includes `cluster` sub-object, `datasource_id`, `active_connections`, `checkout_count`, `avg_wait_ms` | No `cluster` field; `PoolSnapshot` uses `name/driver/active/idle/max/circuit_state` — different names, missing fields | `riversd/src/health.rs:41-60` | Deviation |
| **18.2 Admin — public_key bypass** | §15.2: `public_key` required when `admin_api.enabled = true` | When `public_key` is `None`, middleware passes request through without auth | `riversd/src/server.rs:907-913` | Deviation |
| **18.3 Admin Ed25519 — signing payload order** | §15.3: `method\npath\ntimestamp_ms\nbody_sha256` | Code uses `method\npath\nbody_sha256\ntimestamp_ms` — body and timestamp swapped | `riversd/src/admin.rs` | Deviation |
| **TLS — --no-ssl port** | §1.1: binds on `redirect_port` with --no-ssl | Listener pre-binds to `base.port` before --no-ssl check | `riversd/src/main.rs:143-167` | Deviation |
| **TLS — admin server mandatory** | §1, §15.2: `[base.admin_api.tls]` absent → hard error | `validate_admin_tls_config` exists but not called in startup path | `riversd/src/tls.rs:37-59` | Missing |
| **TLS — redirect server** | §2 step 20: spawn HTTP redirect server on port 80 | No redirect server spawned; `redirect_port` config unused | `rivers-core/src/config.rs:182-185` | Missing |
| **TLS — min_version/cipher config** | §5.3: `[base.tls.engine]` controls min_version and ciphers | `load_tls_acceptor` uses hardcoded rustls defaults; `TlsEngineConfig` fields never read | `riversd/src/tls.rs:89-93` | Missing |
| **1.3 Static Files — root_path type** | §7.4: `root_path: String` (required) | `root_path: Option<String>`; returns 404 silently when `None` | `rivers-core/src/config.rs:594` | Deviation |
| **Session cookie attributes** | §12.3: hardcoded `HttpOnly`, `SameSite=Lax`, `Secure` | `SessionCookieConfig` exposes all as configurable TOML fields; `http_only=false` not validated | `rivers-core/src/config.rs:527-550` | Deviation |
| **Admin log management endpoints** | §15 lists 9 endpoints; no log management | `GET /admin/log/levels`, `POST /admin/log/set`, `POST /admin/log/reset` added | `riversd/src/server.rs:248-250` | Addition |

---

## 2. Views + Streaming + Polling (view-layer-spec, streaming-rest-spec, polling-views-spec)

| Feature (Inventory Name) | Spec Says | Implementation Does | Files Involved | Status |
|---|---|---|---|---|
| **2.2 Handler Pipeline — Stage Names** | Six stages: `pre_process`, `on_request`, `transform`, `on_response`, `post_process`, `on_error` | Collapsed to four: `pre_process`, `handlers`, `post_process`, `on_error` | `rivers-data/src/view.rs:176-189` | Deviation |
| **2.2 Handler Pipeline — on_timeout** | `on_timeout` fires when pipeline exceeds timeout; receives `TimeoutContext` | Entirely absent | `rivers-data/src/view.rs`, `riversd/src/view_engine.rs` | Missing |
| **2.2 Handler Pipeline — ViewContext shape** | `sources: HashMap`, `meta: HashMap`; primary in `sources["primary"]` | `data: HashMap`, `resdata: Value`; no `sources`/`meta` fields | `riversd/src/view_engine.rs:69-85` | Deviation |
| **2.2 Handler Pipeline — ParsedRequest fields** | `query_params`, `path_params` | Uses `query`, `params` | `riversd/src/view_engine.rs:17-24` | Deviation |
| **2.2 Handler Pipeline — parallel field** | SHAPE-12: `parallel` removed | `HandlerStageConfig` still has `pub parallel: bool` | `rivers-data/src/view.rs:198` | Deviation |
| **2.4 WebSocket — actual upgrade loop** | Spec requires WS frame loop with ping/pong, `on_stream`, `RecvError::Lagged` | No frame-reading loop; infrastructure types exist but no actual WS upgrade handler | `riversd/src/websocket.rs` | Missing |
| **2.5 SSE — Last-Event-ID reconnection** | Client reconnection with `Last-Event-ID` replay | `events_since` implemented but never called; no event buffer | `riversd/src/sse.rs:77-101` | Partial |
| **2.5 SSE — push loop stub** | Tick → CodeComponent → result → SSE event | `drive_sse_push_loop` emits hardcoded `"heartbeat"` instead of executing handler | `riversd/src/sse.rs:393-424` | Partial |
| **2.6 MessageConsumer — dispatch entrypoint** | Handler entrypoint from `on_event.handler` config | Hardcodes `function: "handle"` regardless of config | `riversd/src/message_consumer.rs:236` | Deviation |
| **2.6 MessageConsumer — HTTP 400** | Requests to MessageConsumer route → 400 | Router skips MessageConsumer views entirely → 404 not 400 | `riversd/src/view_engine.rs:187-189` | Deviation |
| **2.7 Streaming REST — generator drive loop** | AsyncGenerator `next()` loop; each yield flushed as chunk | Single dispatch, single chunk; no generator iteration | `riversd/src/streaming.rs:181-235` | Partial |
| **2.7 Streaming REST — poison chunk error_type** | NDJSON/SSE poison chunk must include `error_type` field | `error_type` omitted from both formats | `riversd/src/streaming.rs:84-96` | Deviation |
| **2.7 Streaming REST — Cache-Control header** | `Cache-Control: no-cache, no-store` + `X-Accel-Buffering: no` on SSE | Neither header set on streaming responses | `riversd/src/streaming.rs` | Missing |
| **2.7 Streaming REST — validation rules** | Six validation rules | Only three of six implemented | `riversd/src/streaming.rs:136-171` | Partial |
| **2.8 GraphQL — subscriptions** | Subscriptions route through EventBus | No Subscription type registered | `riversd/src/graphql.rs:347-363` | Missing |
| **2.8 GraphQL — mutations** | Mutations require CodeComponent resolver | Mutation type root is `None`; no mutation support | `riversd/src/graphql.rs:347` | Missing |
| **2.8 GraphQL — return types** | DataViews with `return_schema` generate GraphQL object types | All fields typed as `TypeRef::STRING` regardless of schema | `riversd/src/graphql.rs:292-300` | Deviation |
| **10.1 Poll Loops — key derivation** | SHAPE-3: `BTreeMap` + `serde_json::to_string` + SHA-256 canonical JSON | `compute_param_hash` hashes `key=value&` strings from `HashMap<String, String>` | `riversd/src/polling.rs:70-85` | Deviation |
| **10.1 Poll Loops — StorageEngine startup validation** | Server fails at startup if polling declared without storage_engine | No startup validation for this | `riversd/src/polling.rs` | Missing |
| **10.1 Poll Loops — poll_state_ttl_s** | Configurable per view, default 3600 | `save_poll_state` passes `None` TTL; config field doesn't exist | `riversd/src/polling.rs:419-431` | Missing |
| **10.2 Diff — change_detect dispatch** | `change_detect(prev, current)` receives full previous data | Passes `prev_hash: Option<&str>` (SHA-256 hex) not full data | `riversd/src/polling.rs:654-707` | Deviation |
| **10.4 Polling — on_change handler** | `on_change(current)` always required when polling configured | No `on_change` invoked anywhere; config block doesn't exist | `riversd/src/polling.rs:484-533` | Missing |
| **10.4 Polling — emit_on_connect validation** | SHAPE-14: reject `emit_on_connect` in config with error | No validation; no field | `rivers-data/src/view.rs` | Missing |
| **10.4 Polling — PollChangeDetectTimeout event** | SHAPE-20: structured `PollChangeDetectTimeout` event with `consecutive_timeouts` | `tracing::warn!` only; no structured event; no counter | `riversd/src/polling.rs:563-585` | Partial |
| **2.2 Handler Pipeline — on_session_valid wiring** | Fires after session validation, before `pre_process` | Function exists but never called from `execute_rest_view` | `riversd/src/view_engine.rs:850-884` | Partial |
| **2.5 SSE — session_expired format** | Polling spec: `event: session_expired` with `{code: 4401, reason: "..."}` | Only sends `data: {"rivers_session_expired":true}`; no event type, no code | `riversd/src/sse.rs:108-110` | Partial |
| **2.4 WebSocket — session revalidation on REST** | `session_revalidation_interval_s` on REST must be rejected | `validate_views` doesn't check for this | `riversd/src/view_engine.rs:684-781` | Missing |

---

## 3. Data + Schema + Drivers (data-layer-spec, driver-spec, schema-validation-spec, schema-v2)

| Feature (Inventory Name) | Spec Says | Implementation Does | Files Involved | Status |
|---|---|---|---|---|
| **3.1 DataView Execution — Cache Check (§6.2 step 3)** | Step 3: cache check; hit returns early | No cache layer; steps 3, 7 (schema validate), 8 (cache populate) absent | `rivers-data/src/dataview_engine.rs:424-483` | Missing |
| **3.1 DataView Execution — Return Schema Validation (§6.2 step 7)** | Validate result against `return_schema` if `validate_result = true` | Never calls validator; `validate_result` field ignored | `rivers-data/src/dataview_engine.rs:467-475` | Missing |
| **3.3 Two-Tier Cache — wired to executor** | `DataViewEngine::with_cache()` wires cache impl | `DataViewExecutor` has no `cache` field; `TieredDataViewCache` exists but unused | `rivers-data/src/dataview_engine.rs`, `rivers-data/src/tiered_cache.rs` | Missing |
| **3.3 Cache Key — canonical format** | `cache:views:{name}:{sha256}` from canonical JSON | `cache_key` function correct but never called from executor | `rivers-data/src/tiered_cache.rs:57-68` | Partial |
| **3.1 DataViewConfig — on_event/on_stream** | `on_event: Option<OnEventConfig>`, `on_stream: Option<OnStreamConfig>` | Neither type exists; `streaming: bool` used instead | `rivers-data/src/dataview.rs:80-159` | Missing |
| **3.1 DataViewParameterConfig — param_type** | `param_type: DataViewParameterType` enum | Uses `param_type: String`; enum doesn't exist | `rivers-data/src/dataview.rs:14-25` | Partial |
| **3.1 DataViewEngine vs DataViewExecutor** | Single `DataViewEngine` combining registry + execution + cache | Two separate types: `DataViewEngine` (stub) and `DataViewExecutor` (no cache) | `rivers-data/src/dataview.rs:224-252`, `rivers-data/src/dataview_engine.rs:399-583` | Partial |
| **3.3 l2_max_value_bytes default** | Spec: 131072 (128KB) | Three different values: `tiered_cache.rs`: 524288, `dataview.rs`: 65536, spec: 131072 | `rivers-data/src/tiered_cache.rs:46`, `rivers-data/src/dataview.rs:59-61` | Deviation |
| **3.3 DataViewCache::get return type** | `Result<Option<QueryResult>, DataViewError>` (fallible) | `Option<QueryResult>` (infallible); errors swallowed | `rivers-data/src/tiered_cache.rs:77-94` | Deviation |
| **3.3 DataViewCache::set return type** | `Result<(), DataViewError>` (fallible) | `()` (infallible); L2 write errors swallowed | `rivers-data/src/tiered_cache.rs:85-93` | Deviation |
| **3.3 DataViewCache — invalidate** | Not in spec trait definition | `invalidate()` added to trait and both impls | `rivers-data/src/tiered_cache.rs:92-94` | Addition |
| **6.7 Broker Bridge — StorageEngine buffering (SHAPE-18)** | SHAPE-18: buffering removed; broker → EventBus directly | `storage: Option<Arc<dyn StorageEngine>>` field present; actively buffers messages | `riversd/src/broker_bridge.rs:41,97-100,259-276` | **Deviation (SHAPE-18 violation)** |
| **6.7 InboundMessage — payload type** | `payload: Bytes` (bytes::Bytes) | `payload: Vec<u8>` | `rivers-driver-sdk/src/broker.rs:52,69` | Deviation |
| **6.7 ConsumerConfig struct name** | `ConsumerConfig` | `BrokerConsumerConfig` with extra `node_id` field | `rivers-driver-sdk/src/broker.rs:115-122` | Deviation |
| **6.5 DriverFactory — built-ins in new()** | Built-in drivers pre-registered in `new()` | `new()` creates empty factory; drivers registered externally | `rivers-core/src/driver_factory.rs:47-53` | Missing |
| **6.5 DriverFactory — OnceLock plugin registry** | Plugin registries are `OnceLock<Mutex<HashMap>>` | No static registries; `load_plugins()` registers directly into live factory | `rivers-core/src/driver_factory.rs:187-255` | Missing |
| **4.2 Driver Schema Validation — unified Driver trait** | Unified `Driver` trait supersedes separate traits | `Driver` trait defined but no concrete implementations; built-ins use `DatabaseDriver` | `rivers-driver-sdk/src/traits.rs:285-325` | Partial |
| **4.1 Schema Validation — runtime pipeline** | Input → Validator → Executor → Validator → Response | No runtime validation pipeline wired; `validate_fields` exists but unused | `rivers-data/src/dataview_engine.rs:467-475` | Missing |
| **4.6 RiversType — decimal/bytes** | 13 types including `decimal` and `bytes` | `RiversType` enum has 11 variants (missing `decimal`, `bytes`); `validation.rs` handles them as strings — inconsistent | `rivers-data/src/schema.rs:22-34`, `rivers-driver-sdk/src/validation.rs:301-304` | Deviation |
| **5.2 DriverAttributeRegistry** | Per-driver attributes for all drivers (PG, MySQL, SQLite, Redis, Kafka, etc.) | Only 4 drivers: `faker`, `postgresql`, `mysql`, `ldap` | `rivers-data/src/schema.rs:140-147` | Partial |
| **4.7 Pseudo DataView — schema syntax check** | `.build()` calls `SchemaSyntaxChecker` against inline schema | Only checks schema is JSON object with `driver` field; no driver-side check | `rivers-data/src/pseudo_dataview.rs:138-160` | Partial |
| **4.2 DataViewError — schema variants** | Extensions: `UnsupportedSchemaAttribute`, `SchemaFileNotFound`, `SchemaFileParseError`, `UnknownFakerMethod` | None of these variants exist in `DataViewError` | `rivers-data/src/dataview_engine.rs:26-54` | Missing |
| **6.6 Error Redaction (SHAPE-4)** | SHAPE-4: error strings passed through without modification | `redact_error()` scans for sensitive keywords and replaces — contradicts SHAPE-4 | `rivers-data/src/dataview_engine.rs:292-309,473` | **Deviation (SHAPE-4 violation)** |
| **6.7 Broker Bridge — reconnection events** | Publishes `DatasourceReconnected` or `DatasourceConnectionFailed` | Always publishes `DATASOURCE_RECONNECTED` unconditionally; `ConnectionFailed` never published | `riversd/src/broker_bridge.rs:207-232` | Partial |

---

## 4. Auth + Session + Admin (auth-session-spec, v1-admin)

| Feature (Inventory Name) | Spec Says | Implementation Does | Files Involved | Status |
|---|---|---|---|---|
| **7.1 Guard View — return contract** | Guard returns `IdentityClaims` with `_response` for API mode | `parse_guard_result` expects `{allow, redirect_url?, session_claims?}` — different shape | `riversd/src/guard.rs:86-103` | Deviation |
| **7.1 Guard View — hook overrides** | `on_valid_session`/`on_invalid_session` can override redirect | Hook return value ignored; fire-and-forget | `riversd/src/guard.rs:269-302` | Deviation |
| **7.1 Guard View — url validation** | Must reject missing `valid_session_url`/`invalid_session_url` | Only checks handler type and path; no URL validation | `riversd/src/guard.rs:155-188` | Deviation |
| **7.2 Session — middleware not global** | Session middleware runs globally in stack | Comment: "per-view at dispatch"; never runs; sessions never validated | `riversd/src/server.rs:187-226` | Deviation |
| **7.2 Session — token in body** | `include_token_in_body`, `token_body_key` for API clients | Fields absent from `SessionConfig` | `rivers-core/src/config.rs:490-563` | Missing |
| **7.3 CSRF — cookie on responses** | `rivers_csrf` cookie set on every session-validated response | `build_csrf_cookie()` exists but never called | `riversd/src/csrf.rs:141-148` | Deviation |
| **7.4 Per-View Session — hooks** | `on_session_valid`/`on_invalid_session` hooks per view | Implemented but never wired into request pipeline | `riversd/src/view_engine.rs:846-880` | Deviation |
| **7.5 Cross-App — header forwarding** | Authorization + `X-Rivers-Claims` forwarded inter-service | Helpers exist but no HTTP client code forwards them | `riversd/src/session.rs:256-267` | Deviation |
| **7.6 Persistent Connection Revalidation** | Configurable revalidation interval for WS/SSE | No revalidation loop; config field exists but unused | `riversd/src/websocket.rs:363-370` | Deviation |
| **18.3 Ed25519 — signing payload order** | `method\npath\ntimestamp_ms\nbody_sha256` | `method\npath\nbody_sha256\ntimestamp_ms` — swapped | `riversd/src/admin_auth.rs:35-42` | Deviation |
| **18.3 Ed25519 — timestamp unit** | `unix_timestamp_ms` (milliseconds) | Parses as `as_secs()` (seconds) | `riversd/src/admin_auth.rs:87-117` | Deviation |
| **18.3 Ed25519 — body hash** | Signature covers SHA-256 of body | `body_hash = ""` unconditionally; body never hashed | `riversd/src/server.rs:958-968` | Deviation |
| **18.4 Localhost bypass** | `127.0.0.1` + no key → allowed (dev mode) | SHAPE-25 removes exception; always requires key | `riversd/src/server.rs:1658-1667` | Deviation |
| **18.5 IP Allowlist** | Enforced on every admin request | `check_ip_allowlist` exists but never called | `riversd/src/admin.rs:108-134` | Deviation |
| **18.6 RBAC** | Per-endpoint permission enforcement | `check_permission` exists but never called; all authenticated requests pass | `riversd/src/admin.rs:136-168` | Deviation |
| **18.7 --no-admin-auth** | Startup warning when active | Flag parsed but never copied to config; warning never emitted | `riversd/src/main.rs:156-180` | Deviation |
| **13.2 Deploy — zip upload** | `POST /admin/deploy` accepts zip file upload | Handler reads JSON body with `bundle_path` string; no multipart/zip | `riversd/src/server.rs:607-640` | Deviation |
| **13.2 Deploy — reject transition** | Reject should work on PENDING deployments | `PENDING → Failed` not in transition matrix; returns `InvalidTransition` | `riversd/src/admin.rs:215-237` | Deviation |
| **Session — http_only=false validation** | Must reject `http_only = false` | No validation; defaults to true but accepts false | `rivers-core/src/config.rs:527-562` | Deviation |
| **Session — multiple guard detection** | Reject >1 guard view | `detect_guard_view` exists but never called at startup | `riversd/src/guard.rs:155-188` | Deviation |
| **Session — StorageEngine required** | Fail startup if protected views exist without storage | No such check anywhere | `riversd/src/server.rs:1126-1400` | Missing |

---

## 5. HTTP + Plugin Drivers (http-driver-spec, technology-path-spec)

| Feature (Inventory Name) | Spec Says | Implementation Does | Files Involved | Status |
|---|---|---|---|---|
| **HTTP Driver — Circuit Breaker field name** | §9.2: `open_timeout_ms` (per SHAPE-1) | Code uses pre-amendment `open_duration_ms` | `rivers-driver-sdk/src/http_driver.rs:470` | Deviation |
| **HTTP Driver — Retry-After parsing** | §9.1 (SHAPE-16): parse `Retry-After` per `retry_after_format` | `RetryConfig` has no `retry_after_format`; no header inspection | `rivers-driver-sdk/src/http_driver.rs:393-442` | Deviation |
| **HTTP Driver — WebSocket streaming** | §3, §8.2: `protocol = "websocket"` → `connect_stream()` via BrokerConsumerBridge | `connect_stream()` implements SSE only; no WebSocket path | `rivers-driver-sdk/src/http_executor.rs:370-406` | Deviation |
| **Five-Op Contract — begin/stream** | §6.5: `query`, `execute`, `ping`, `begin`, `stream` | `Connection` trait only has `execute` and `ping`; no `begin`/`stream` | `rivers-driver-sdk/src/traits.rs:338-348` | Deviation |
| **Stub Drivers — Cassandra** | §6.4: listed as stub (NotImplemented) | Real implementation using `scylla` crate | `rivers-plugin-cassandra/src/lib.rs` | Deviation |
| **Stub Drivers — CouchDB** | Not listed in spec at all | Real implementation (reqwest, Mango queries) | `rivers-plugin-couchdb/src/lib.rs` | Deviation |
| **Stub Drivers — LDAP** | §6.4: listed as stub | Real implementation using `ldap3` crate | `rivers-plugin-ldap/src/lib.rs` | Deviation |
| **RPS Client driver** | §6.8: built-in with mTLS, operations | Returns `NotImplemented` on every `connect()`; deferred to V2 | `rivers-core/src/drivers/rps_client.rs` | Deviation |
| **PostgreSQL TLS** | Tech path: TLS via `tokio-postgres-native-tls` or `rustls` | Connects with `tokio_postgres::NoTls` hardcoded | `rivers-core/src/drivers/postgres.rs:49` | Deviation |
| **HTTP Driver — connect_stream ignores protocol** | §3: `protocol` field determines SSE vs WebSocket | Always connects as SSE regardless of `HttpProtocol` variant | `rivers-driver-sdk/src/http_executor.rs:370-406` | Deviation |
| **Plugin drivers — unified Driver trait** | §4.8: plugins must implement `SchemaSyntaxChecker`, `Validator`, `Executor` | All plugins only implement `DatabaseDriver`; no unified `Driver` trait | All plugin `lib.rs` files | Deviation |
| **HTTP Driver — connect_stream path** | §8.1: `consumer.path` is endpoint path | Connects to `base_url` directly; path not appended | `rivers-driver-sdk/src/http_executor.rs:378` | Deviation |

---

## 6. LockBox + Storage Engine + Logging (lockbox-spec, storage-engine-spec, logging-spec)

| Feature (Inventory Name) | Spec Says | Implementation Does | Files Involved | Status |
|---|---|---|---|---|
| **8.2 LockBox — SSH Agent key source** | `key_source = "agent"` via SSH agent | Stubbed with "not yet supported" error | `rivers-core/src/lockbox.rs:502-506` | Missing |
| **8.3 LockBox CLI — unalias** | `rivers lockbox unalias` removes alias | Not present in CLI | `rivers-lockbox/src/main.rs:38-56` | Missing |
| **8.3 LockBox CLI — credential type flag** | `--type <string\|base64url\|pem\|json>` on add | No `--type` flag; type metadata not stored | `rivers-lockbox/src/main.rs:165-182` | Missing |
| **8.3 LockBox CLI — show confirmation** | Requires `[y/N]` prompt; `--yes` bypasses | Prints value directly, no prompt | `rivers-lockbox/src/main.rs:218-238` | Missing |
| **8.3 LockBox CLI — remove confirmation** | Requires name re-type to confirm | Deletes immediately | `rivers-lockbox/src/main.rs:272-289` | Missing |
| **8.3 LockBox CLI — list format** | Table: `NAME / TYPE / ALIASES / UPDATED` | Bare names + alias pairs; no type/timestamp | `rivers-lockbox/src/main.rs:184-216` | Partial |
| **8.3 LockBox CLI — validate config refs** | `--config <path>` resolves `lockbox://` URIs | Only checks decryptability; no config reference check | `rivers-lockbox/src/main.rs:328-361` | Partial |
| **8.3 LockBox CLI — exit codes** | 7 defined exit codes (0-6) | All errors exit with code 1 | `rivers-lockbox/src/main.rs:65-68` | Missing |
| **8.3 LockBox CLI — keystore format** | Single `.rkeystore` TOML file, Age-encrypted | Per-entry `.age` files + unencrypted `aliases.json` — incompatible format | `rivers-lockbox/src/main.rs:98-108` vs `rivers-core/src/lockbox.rs:377-415` | Deviation |
| **8.3 LockBox CLI — config integration** | Reads key source from `riversd.conf` | Reads from hardcoded `lockbox/identity.key` | `rivers-lockbox/src/main.rs:19-21` | Missing |
| **11.3 StorageEngine — cache: namespace** | `cache:` is reserved prefix | `RESERVED_PREFIXES` missing `"cache:"` | `rivers-core/src/storage.rs:47` | Missing |
| **11.7 Sentinel key format** | Key: `rivers:node:{node_id}` | Key: `rivers:node:sentinel:{node_id}` — wrong format | `rivers-core/src/storage.rs:231,244,248` | **Bug** |
| **11.1 StorageEngine — Redis key_prefix** | All Redis keys prefixed with `key_prefix` (default `"rivers:"`) | Keys are `{namespace}:{key}`; no global prefix | `rivers-core/src/storage_redis.rs:30-32` | Missing |
| **11.1 StorageEngine — SQLite created_at** | Schema includes `created_at INTEGER NOT NULL` + index | Table has only `namespace, key, value, expires_at`; no `created_at`/index | `rivers-core/src/storage_sqlite.rs:35-45` | Partial |
| **12.2 EventBus — priority tier names** | Expect → Handle → Emit → Observe (four tiers) | Critical → Standard → Observe (three tiers) | `rivers-core/src/eventbus.rs:116-120` | Deviation |
| **12.3 EventBus — cross-node gossip** | `GossipPayload` forwards events to peer HTTP endpoints | `gossip_forward` is a logged no-op placeholder | `rivers-core/src/eventbus.rs:311-333` | Partial |
| **16.2 Logging — LogLevel Trace** | Four levels: Debug, Info, Warn, Error | Five variants: adds `Trace` (undocumented) | `rivers-core/src/event.rs:7-14` | Deviation |
| **16.1 Logging — LogHandler wildcard** | LogHandler receives all events | Subscribes to `"*"` but `publish()` only dispatches to `event_type` subscribers; events never reach LogHandler | `rivers-core/src/logging.rs:63-66`, `rivers-core/src/eventbus.rs:194-196` | **Bug** |
| **16.1 Logging — file writer** | `LogHandler` has `file_writer: Option<AsyncFileWriter>` | No `file_writer` field; only stdout output | `rivers-core/src/logging.rs:23-28` | Missing |

---

## 7. App + ProcessPool + RPS (application-spec, processpool-spec, rps-spec, app-dev, address-book, shaping)

| Feature (Inventory Name) | Spec Says | Implementation Does | Files Involved | Status |
|---|---|---|---|---|
| **13.1 Bundle — file format** | `manifest.json`, `resources.json` (JSON) | TOML files exclusively (`manifest.toml`, `resources.toml`, `app.toml`) | `rivers-data/src/bundle.rs`, `rivers-data/src/loader.rs` | Deviation |
| **13.1 Bundle — entryPoint semantics** | Spec text: URL segment name; spec examples: full URLs — internally inconsistent | Code stores whatever provided; address-book uses simple names (matches text, not examples) | `rivers-data/src/bundle.rs:54-65` | Deviation |
| **13.1 Bundle — source field** | `source` marked required in both bundle and app manifest | Both are `Option<String>` (not required) | `rivers-data/src/bundle.rs:28,61` | Deviation |
| **13.2 Deployment — SHAPE-8 sentinel** | Startup with Redis: check `rivers:node:*`, hard-fail if found | No sentinel check in any startup path | `riversd/src/server.rs` | Missing |
| **9.1 ProcessPool — clean ObjectTemplate global** | Clean `ObjectTemplate`; no `globalThis` inheritance; no `console.log` | Default V8 context with full `globalThis`; `console.log` wired to `Rivers.log` | `riversd/src/process_pool.rs:1063-1075` | Deviation |
| **9.1 ProcessPool — TypeScript via swc at bundle load** | `swc` compiles TS at bundle load time | On-first-dispatch compilation with script cache; `swc` not integrated | `riversd/src/process_pool.rs:786-797` | Deviation |
| **9.4 ProcessPool — four-scope injection (SHAPE-10)** | Application, Session, Connection, Request scopes; narrower shadows broader | Single flat `ctx` object; no distinct scopes; no Connection scope | `riversd/src/process_pool.rs:1194-1227` | Deviation |
| **9.10 Rivers.crypto.hashPassword** | bcrypt with cost factor 12 (min 10) | SHA-256 with `sha256:` prefix; synchronous | `riversd/src/process_pool.rs:1899-1916` | Deviation |
| **9.10 Rivers.crypto.hmac — LockBox alias** | Key resolved via LockBox alias on host side; raw key never enters isolate | Accepts raw key value directly in V8 | `riversd/src/process_pool.rs:1978-2002` | Deviation |
| **9.7 V8 Watchdog — per pool** | One watchdog thread per pool scanning active workers | Per-task watchdog thread spawned (N tasks = N threads) | `riversd/src/process_pool.rs:1044-1057` | Deviation |
| **9.3 Wasmtime — AOT compilation** | AOT at bundle load time; cached | Lazy on-first-use with `WASM_MODULE_CACHE` | `riversd/src/process_pool.rs:560-588` | Deviation |
| **9.3 Wasmtime — rivers.db_query host function** | Host functions: `rivers.db_query`, `rivers.log_info` | Only `log_info/warn/error` registered; `db_query` absent | `riversd/src/process_pool.rs:630-662` | Missing |
| **9.12 Worker crash recovery** | Dead workers replaced; `WorkerCrash` event; `WorkerPoolDegraded` alert | `spawn_blocking` per task; no replacement, no events, no alerts | `riversd/src/process_pool.rs:1186-1189` | Missing |
| **9.1 Config — max_heap_mb** | TOML key `max_heap_mb`; `recycle_after_tasks` | Uses `max_heap_bytes`; `recycle_after_tasks` absent | `rivers-core/src/config.rs:751-770` | Deviation |
| **13.2 Deployment — health check before promotion** | `GET /health` with backoff before RUNNING | `HealthCheckBackoff` struct exists; no HTTP probe executed | `riversd/src/deployment.rs:330-364` | Missing |
| **13.5 Services discovery endpoint** | `GET /<bundle>/<main>/services` returns JSON | No endpoint registered | `riversd/src/server.rs` | Missing |
| **13.3 Preflight checks** | LockBox alias, driver registration, service deps, bundle structure, appId unique | Only appId uniqueness and app type checked | `riversd/src/deployment.rs:204-261` | Partial |
| **14 RPS — entire spec** | RPS Master, Relay, Alias Registry, Role System, Secret Broker, Trust Bundle | Nothing implemented; correctly deferred to V2 | — | Deferred |
| **App-dev — [spa] section** | `[spa]` with `root_path`, `index_file` | `[static_files]` with `root` — tracked as GAP-1 | `rivers-data/src/bundle.rs:163-171` | Deviation |
| **App-dev — cache config key** | `[data.dataviews.*.cache]` | Uses `[data.dataviews.*.caching]` | `rivers-data/src/dataview.rs:150` | Deviation |
| **App-dev — handler type value** | `type = "data_view"` | `type = "dataview"` (no underscore) | `rivers-data/src/view.rs` | Deviation |
| **SHAPE-21 — TLS mandatory** | `[base.tls]` absent → hard error | `tls: Option<TlsConfig>` defaults to `None`; no enforcement | `rivers-core/src/config.rs:95` | Deviation |
| **SHAPE-23/24 — SecurityConfig cleanup** | CORS/rate-limit fields removed from SecurityConfig | 8 CORS fields + 4 rate-limit fields still in SecurityConfig | `rivers-core/src/config.rs:383-437` | Deviation |
| **9.8 Handler ctx.ws** | `ctx.ws` for WebSocket context | `ctx.ws` not injected; WS-specific fields absent | `riversd/src/process_pool.rs:1194-1330` | Partial |
| **Address-book — resources.toml HTTP datasource** | app-main declares `[[datasources]]` HTTP entry | Only `[[services]]` declared; HTTP datasource missing | `address-book-main/resources.toml` | Deviation |
| **Address-book — view paths** | `/api/contacts`, `/api/contacts/{id}` | `"contacts"`, `"contacts/{id}"` — no `/api/` prefix | `address-book-service/app.toml` | Deviation |

---

## Cross-Cutting Observations

### SHAPE Decision Violations
- **SHAPE-4** (no error redaction): `DataViewExecutor` implements `redact_error()` contrary to the decision
- **SHAPE-18** (no StorageEngine buffering in broker bridge): `BrokerConsumerBridge` still has storage buffering
- **SHAPE-23/24** (CORS/rate-limit out of SecurityConfig): fields remain

### Bugs (functional correctness issues)
1. **Sentinel key format** (`storage.rs`): constructs `rivers:node:sentinel:{node_id}` instead of `rivers:node:{node_id}` — multi-node detection broken
2. **LogHandler wildcard** (`logging.rs` + `eventbus.rs`): subscribes to `"*"` but `publish()` only dispatches to exact topic — logging receives no events

### Highest-Impact Missing Features
1. **Session middleware** — never runs; sessions never validated on any request
2. **DataView cache** — L1/L2 cache built but never wired to executor
3. **Schema validation pipeline** — three-stage validation spec'd but none wired at runtime
4. **WebSocket upgrade loop** — infrastructure exists but no frame processing
5. **Admin body hash** — Ed25519 signature verification ignores request body (security gap)
