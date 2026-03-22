# Epics — Rivers Runtime Implementation

Epics are ordered by dependency. Each epic should generate tasks small enough to implement in a single session. Specs are referenced for task generation.

---

## Epic 1: Project Bootstrap & Workspace

**Spec:** `rivers-application-spec.md` (§3), `rivers-data-layer-spec.md` (§1)
**Goal:** Create the Cargo workspace, define crate boundaries, and set up CI.

- [x] 1.1 Create root `Cargo.toml` workspace with member crates: `rivers-core`, `rivers-driver-sdk`, `rivers-data`, `riversd`
- [x] 1.2 Create `rivers-core` crate with shared types: `ServerConfig`, `RiversError`, `LogLevel`, `Event`
- [x] 1.3 Create `rivers-driver-sdk` crate with driver trait stubs and `QueryValue`/`Query`/`QueryResult` types
- [x] 1.4 Create `rivers-data` crate with `DataViewEngine` stub and `DataViewConfig` types
- [x] 1.5 Create `riversd` crate (binary) with `main()` entry point stub
- [x] 1.6 Add shared dependencies: `tokio`, `serde`, `serde_json`, `thiserror`, `tracing`, `async-trait`
- [x] 1.7 Set up basic CI (cargo build, cargo test, cargo clippy)

---

## Epic 2: Configuration Parsing & Validation

**Spec:** `rivers-httpd-spec.md` (§19), `rivers-data-layer-spec.md` (§12), `rivers-application-spec.md` (§4-6)
**Goal:** Parse `riversd.conf` (TOML), bundle manifests, and `resources.toml` into typed config structs with validation.

- [x] 2.1 Define `ServerConfig` struct with all top-level sections (`base`, `security`, `static_files`, `admin_api`)
- [x] 2.2 Define `DatasourceConfig` struct with pool, circuit breaker, and driver fields
- [x] 2.3 Define `DataViewConfig` and `DataViewParameterConfig` structs
- [x] 2.4 Define `ApiViewConfig` struct with all view-type-specific fields
- [x] 2.5 Implement `config.validate()` — all validation rules from specs (port conflicts, required fields, cross-references)
- [x] 2.6 Parse bundle-level `manifest.toml` and per-app `manifest.toml`
- [x] 2.7 Parse `resources.toml` (datasources, services, nopassword, x-type)
- [x] 2.8 Parse `app.toml` (dataviews, views, static files, datasource configs)
- [x] 2.9 Implement `apply_bundle_config()` — merge bundle config into ServerConfig
- [x] 2.10 Implement environment overrides (`environment_overrides.{env}.*`)

---

## Epic 3: EventBus

**Spec:** `rivers-view-layer-spec.md` (§11), `rivers-data-layer-spec.md` (§11), `rivers-logging-spec.md` (§4)
**Goal:** In-process pub/sub with priority tiers and per-topic broadcast channels.

- [x] 3.1 Define `Event` struct with `event_type`, `payload`, `trace_id`, `timestamp`
- [x] 3.2 Define `HandlerPriority` enum: `Critical`, `Standard`, `Observe`
- [x] 3.3 Implement `TopicRegistry` — per-topic `tokio::sync::broadcast` channels with configurable buffer size
- [x] 3.4 Implement `EventBus` — `publish(event)`, `subscribe(topic, handler, priority)`
- [x] 3.5 Implement priority-tier dispatch: Critical handlers awaited, Standard handlers awaited, Observe handlers fire-and-forget
- [x] 3.6 Define event type constants for all spec-defined events (RequestCompleted, DataViewExecuted, DatasourceCircuitOpened, etc.)

---

## Epic 4: StorageEngine

**Spec:** `rivers-storage-engine-spec.md`
**Goal:** Internal KV + queue infrastructure with three backends.

- [x] 4.1 Define `StorageEngine` trait (get, set, delete, list_keys, enqueue, dequeue, ack, flush_expired)
- [x] 4.2 Define `StorageError` enum and `StoredMessage` struct
- [x] 4.3 Implement `InMemoryStorageEngine` — HashMap + VecDeque under Arc<Mutex>
- [ ] 4.4 Implement `SqliteStorageEngine` — sqlx with WAL mode, kv_store + queue_store tables, index creation
- [ ] 4.5 Implement `RedisStorageEngine` — redis crate, SET EX for KV, Redis Streams for queue (XADD/XREADGROUP/XACK)
- [x] 4.6 Implement background sweep task (`flush_expired` at `sweep_interval_s`)
- [x] 4.7 Implement reserved key prefix enforcement (`session:`, `csrf:`, `poll:`, `rivers:`)
- [x] 4.8 Parse `[base.storage_engine]` config section and construct appropriate backend

---

## Epic 5: Logging & Observability

**Spec:** `rivers-logging-spec.md`
**Goal:** Structured logging via EventBus, trace correlation, redaction.

- [x] 5.1 Implement `LogHandler` as EventBus observer: event→level mapping, min_level filtering
- [x] 5.2 Implement JSON log format — mandatory fields (timestamp, level, message, trace_id, app_id, node_id, event_type) + event payload fields
- [x] 5.3 Implement Text log format via `tracing::info!`
- [x] 5.4 Implement `trace_id_middleware` — W3C traceparent extraction, x-trace-id fallback, UUID generation
- [ ] 5.5 Implement trace ID propagation through request extensions, response headers, EventBus events
- [ ] 5.6 Implement local file logging (optional, append mode)
- [x] 5.7 ~~Implement OTel integration~~ — **REMOVED**: spec updated to remove all OTel/OTLP. "There is no distributed tracing export. No OTLP. No external collector."
- [x] 5.8 Implement redaction: DataViewEngine error strings, V8/WASM runtime error strings (15-keyword list)

---

## Epic 6: LockBox — Secret Management

**Spec:** `rivers-lockbox-spec.md`
**Goal:** Age-encrypted keystore, CLI management, startup resolution, datasource credential integration.

- [x] 6.1 Define entry model (`name`, `value`, `type`, `aliases`, timestamps) and keystore TOML schema
- [x] 6.2 Implement keystore file read/decrypt using `age` crate (X25519 + ChaCha20-Poly1305)
- [x] 6.3 Implement key source resolution: `env`, `file`, `agent` (agent stubbed)
- [x] 6.4 Implement in-memory name+alias→value resolution map with O(1) lookup
- [x] 6.5 Implement alias resolution (exact name match → alias match → EntryNotFound)
- [x] 6.6 Implement file permission enforcement (600 on .rkeystore and key file)
- [x] 6.7 Integrate into `riversd` startup: collect lockbox:// URIs, decrypt, resolve, zeroize plaintext
- [ ] 6.8 Implement `rivers lockbox` CLI subcommands: `init`, `add`, `list`, `show`, `alias`, `unalias`, `rotate`, `remove`, `rekey`, `validate`
- [x] 6.9 Implement validation rules (naming rules, duplicate detection, URI resolution)

---

## Epic 7: Driver SDK — Core Contracts

**Spec:** `rivers-driver-spec.md` (§1-2), `rivers-data-layer-spec.md` (§2)
**Goal:** Define the `DatabaseDriver` and `Connection` traits, `QueryValue`, `Query`, `QueryResult`, `DriverError`.

- [x] 7.1 Define `DriverError` enum (UnknownDriver, Connection, Query, Transaction, Unsupported, Internal)
- [x] 7.2 Define `QueryValue` enum (String, Integer, Float, Boolean, Null, Array, Json)
- [x] 7.3 Define `Query` struct (operation, target, parameters, statement) with operation inference from first token
- [x] 7.4 Define `QueryResult` struct (rows, affected_rows, last_insert_id)
- [x] 7.5 Define `DatabaseDriver` trait (name, connect, supports_transactions, supports_prepared_statements)
- [x] 7.6 Define `Connection` trait (execute, ping, driver_name)
- [x] 7.7 Define `ConnectionParams` struct (host, port, database, username, password, options)

---

## Epic 8: Driver SDK — Broker Contracts

**Spec:** `rivers-data-layer-spec.md` (§3), `rivers-driver-spec.md` (§6)
**Goal:** Define `MessageBrokerDriver`, `BrokerConsumer`, `BrokerProducer` traits and supporting types.

- [x] 8.1 Define `InboundMessage`, `OutboundMessage`, `MessageReceipt` structs
- [x] 8.2 Define `BrokerMetadata` enum (Kafka, Rabbit, Nats, Redis variants)
- [x] 8.3 Define `MessageBrokerDriver` trait (name, create_producer, create_consumer)
- [x] 8.4 Define `BrokerConsumer` trait (receive, ack, nack, close)
- [x] 8.5 Define `BrokerProducer` trait (publish, close)
- [x] 8.6 Define `BrokerConsumerConfig`, `BrokerSubscription`, `FailurePolicy`, `FailureMode`

---

## Epic 9: DriverFactory & Plugin System

**Spec:** `rivers-driver-spec.md` (§7-9), `rivers-data-layer-spec.md` (§9)
**Goal:** Driver registry, dynamic plugin loading via libloading, ABI version check.

- [x] 9.1 Implement `DriverFactory` struct with `drivers` and `broker_drivers` HashMaps
- [x] 9.2 Implement `DriverRegistrar` trait (register_database_driver, register_broker_driver)
- [x] 9.3 Implement plugin directory scan and `libloading` shared library loading
- [x] 9.4 Implement ABI version check (`_rivers_abi_version` symbol)
- [x] 9.5 Implement `catch_unwind` around `_rivers_register_driver` calls
- [x] 9.6 Implement canonical path deduplication to prevent duplicate loads via symlinks
- [ ] 9.7 Emit `DriverRegistered` / `PluginLoadFailed` events to EventBus

---

## Epic 10: Built-in Database Drivers

**Spec:** `rivers-driver-spec.md` (§3-4), `rivers-data-layer-spec.md` (§8)
**Goal:** Implement built-in drivers that register in DriverFactory at startup.

- [x] 10.1 Implement `FakerDriver` — configurable mock results, operation dispatch (select/insert/update/delete/ping)
- [x] 10.2 Implement `PostgresDriver` stub — returns Unsupported, supports_transactions=true, supports_prepared_statements=true
- [x] 10.3 Implement `MysqlDriver` stub — returns Unsupported, supports_transactions=true
- [x] 10.4 Implement `SqliteDriver` stub — returns Unsupported, supports_transactions=true
- [x] 10.5 Implement `RedisDriver` stub — returns Unsupported (will get 18+ operation dispatch when redis crate added)
- [x] 10.6 Implement `MemcachedDriver` stub — returns Unsupported
- [ ] 10.7 Implement `EventBusDriver` — deferred: needs EventBus wired at construction, requires riversd integration
- [x] 10.8 Implement `RpsClientDriver` stub — returns Unsupported, wired in when RPS ships (v2)
- [x] 10.9 Implement `register_builtin_drivers()` — registers all 7 built-in drivers into DriverFactory
- [x] 10.10 Added `PartialEq` derive to `QueryValue` for test assertions

---

## Epic 11: Plugin Drivers

**Spec:** `rivers-driver-spec.md` (§7-9), `rivers-data-layer-spec.md` (§9)
**Goal:** External plugin crate implementations for non-built-in datasources and brokers. Each is an independent crate conforming to the plugin ABI.

- [x] 11.0 Move `DriverRegistrar` trait from rivers-core to rivers-driver-sdk (plugins can't depend on rivers-core)
- [x] 11.1 Implement `MongoDBDriver` plugin crate — cdylib stub, ABI exports, honest stub pattern
- [x] 11.2 Implement `ElasticsearchDriver` plugin crate — cdylib stub, ABI exports, honest stub pattern
- [x] 11.3 Implement `KafkaDriver` plugin crate — cdylib stub, MessageBrokerDriver, ABI exports
- [x] 11.4 Implement `RabbitMQDriver` plugin crate — cdylib stub, MessageBrokerDriver, ABI exports
- [x] 11.5 Implement `NATSDriver` plugin crate — cdylib stub, MessageBrokerDriver, ABI exports
- [x] 11.6 Implement `RedisStreamsBrokerDriver` plugin crate — cdylib stub, MessageBrokerDriver, ABI exports
- [x] 11.7 Implement `InfluxDBDriver` plugin crate — cdylib stub, ABI exports, honest stub pattern

---

## Epic 12: HTTP Driver

**Spec:** `rivers-http-driver-spec.md`
**Goal:** HTTP as a first-class datasource driver with auth models, connection pooling, retry, and streaming.

- [x] 12.1 Define `HttpDriver` trait (connect, connect_stream, refresh_auth)
- [x] 12.2 Define `HttpConnection` and `HttpStreamConnection` traits
- [x] 12.3 Define auth models: AuthConfig enum (none, bearer, basic, api_key, oauth2_client_credentials), AuthState
- [x] 12.4 Define HTTP connection config: HttpConnectionParams, HttpProtocol, TlsConfig
- [x] 12.5 Implement path templating (`{param}` substitution) and body templating (type-preserving JSON substitution)
- [x] 12.6 Define `HttpDataViewConfig`, `HttpDataViewParam`, `ParamLocation` with serde
- [x] 12.7 Implement response → QueryResult mapping (JSON object → 1 row, array → N rows, null → empty)
- [x] 12.8 Define `RetryConfig` (attempts, backoff strategy, retry_on_status) and `BackoffStrategy`
- [x] 12.9 Define `CircuitBreakerConfig` (failure_threshold, window_ms, open_duration_ms, half_open_attempts)
- [ ] 12.10 Implement reqwest-based HTTP execution — deferred until reqwest dep added
- [x] 12.11 Implement non-JSON response wrapping (`{ "raw": "...", "content_type": "..." }`)
- [x] 12.12 Implement validation rules (path params, auth_header, retry/CB bounds, success_status)

---

## Epic 13: Broker Consumer Bridge

**Spec:** `rivers-data-layer-spec.md` (§10)
**Goal:** Async task per broker consumer that pulls, buffers to StorageEngine, and publishes to EventBus.

- [x] 13.1 Implement bridge task: receive → optional StorageEngine enqueue → EventBus publish → broker ack → StorageEngine ack
- [x] 13.2 Implement failure policy dispatch: DeadLetter, Redirect, Requeue, Drop
- [x] 13.3 Implement reconnection loop with configurable reconnect_ms
- [x] 13.4 Implement consumer lag detection (messages_pending threshold → ConsumerLagDetected event)
- [x] 13.5 Implement drain on shutdown (with configurable drain_timeout_ms)

---

## Epic 14: Pool Manager

**Spec:** `rivers-data-layer-spec.md` (§5)
**Goal:** Per-datasource connection pooling with circuit breaker, health checks, and credential rotation support.

- [x] 14.1 Define `PoolConfig` struct (max_size, min_idle, timeouts, health_check_interval_ms)
- [x] 14.2 Define `CircuitBreakerConfig` and implement state machine (Closed → Open → Half-Open → Closed)
- [x] 14.3 Implement connection pool: checkout with timeout, idle management, max lifetime
- [x] 14.4 Implement `PoolSnapshot` for health reporting
- [x] 14.5 Implement health check background task per pool
- [x] 14.6 Implement `CredentialRotated` event handler: drain and rebuild affected pool
- [x] 14.7 Implement graceful drain on shutdown

---

## Epic 15: Schema System

**Spec:** `rivers-schema-spec-v2.md`
**Goal:** File-referenced JSON schemas with driver-aware attribute validation.

- [x] 15.1 Define schema file format (type, description, fields array with name/type/required + driver attrs)
- [x] 15.2 Implement schema file loader — load and parse `.schema.json` files from bundle paths
- [x] 15.3 Implement driver attribute registry — map of driver name → supported attributes
- [x] 15.4 Implement schema attribute validation against datasource driver type
- [x] 15.5 Implement return schema validation on QueryResult rows
- [x] 15.6 Implement Rivers primitive type validation (uuid, email, phone, datetime, url, etc.)

---

## Epic 16: DataView Engine

**Spec:** `rivers-data-layer-spec.md` (§6)
**Goal:** Named, parameterized, schema-validated query execution facade.

- [x] 16.1 Implement `DataViewRegistry` — name → DataViewConfig lookup
- [x] 16.2 Implement `DataViewRequestBuilder` with parameter validation (type check, required check, strict mode)
- [x] 16.3 Implement execution sequence: registry lookup → param validate → cache check → pool acquire → driver execute → release → schema validate → cache populate → return
- [x] 16.4 Implement parameter zero-value defaults for optional params ("" for String, 0 for Integer, etc.)
- [x] 16.5 Implement error redaction for sensitive strings (password, token, secret, authorization)
- [x] 16.6 Implement tracing spans: `dataview.execute`, `driver.execute`

---

## Epic 17: DataView Caching (L1/L2)

**Spec:** `rivers-data-layer-spec.md` (§7), `rivers-storage-engine-spec.md` (§5)
**Goal:** Two-tier cache with in-process LRU (L1) and StorageEngine-backed (L2).

- [x] 17.1 Define `DataViewCachingPolicy` struct (ttl_seconds, l1/l2 enabled, l1_max_entries, l2_max_value_bytes)
- [x] 17.2 Implement `LruDataViewCache` (L1) — VecDeque LRU, lazy TTL expiry on access
- [x] 17.3 Implement `TieredDataViewCache` — L1 check → L2 check → miss path with L2-then-L1 population
- [x] 17.4 Implement stable cache key: FNV-1a hash of `(view_name, sorted_params_json)`
- [x] 17.5 Implement `CacheInvalidation` via invalidate() (view-scoped and full invalidation)
- [x] 17.6 Implement L2 size gate (skip L2 for results exceeding l2_max_value_bytes)

---

## Epic 18: HTTP Server (HTTPD Core)

**Spec:** `rivers-httpd-spec.md` (§1-6, §13)
**Goal:** Axum-based main and admin servers with TLS, HTTP/2, and middleware stack.

- [x] 18.1 Implement `run_server_with_listener_with_control` entry point following the 22-step startup sequence
- [x] 18.2 Build main server router — route registration order (health → gossip → graphql → views → static)
- [x] 18.3 Implement middleware stack (trace_id, timeout, shutdown_guard, security_headers, compression, body_limit)
- [ ] 18.4 Implement TLS via rustls + axum-server (main server) — deferred, needs rustls/axum-server crates
- [x] 18.5 Implement HTTP/2 validation — rejects HTTP/2 without TLS at startup
- [x] 18.6 Implement `ShutdownCoordinator` (draining, inflight counter, shutdown_guard_middleware)
- [x] 18.7 Implement graceful shutdown: signal handling (SIGTERM/SIGINT/watch), drain wait

---

## Epic 19: Rate Limiting & Backpressure

**Spec:** `rivers-httpd-spec.md` (§10-11), `rivers-view-layer-spec.md` (§10)
**Goal:** Token bucket rate limiting and semaphore-based backpressure.

- [x] 19.1 Implement token bucket `RateLimiter` with per-key state (IP or custom header)
- [x] 19.2 Implement bucket eviction (10K max buckets, stale eviction > 5 min, 50% LRU eviction)
- [x] 19.3 Implement per-view rate limit override (separate budget per view ID)
- [x] 19.4 Implement 429 response with Retry-After header calculation
- [x] 19.5 Implement `backpressure_middleware` — semaphore with configurable queue_depth and queue_timeout_ms
- [x] 19.6 Implement 503 response when backpressure exhausted

---

## Epic 20: CORS & Security Headers

**Spec:** `rivers-httpd-spec.md` (§9, §17)
**Goal:** CORS origin matching, security header injection, handler header blocklist.

- [x] 20.1 Implement `security_headers_middleware` (done in Epic 18)
- [x] 20.2 Implement CORS header injection per view (cors_enabled, origin matching, wildcard + credentials conflict)
- [x] 20.3 Implement handler header blocklist (SEC-8) — strip set-cookie, access-control-*, host, etc.
- [x] 20.4 Implement default body limit (16 MiB, done in Epic 18)

---

## Epic 21: Static File Serving & SPA Fallback

**Spec:** `rivers-httpd-spec.md` (§7-8)
**Goal:** Serve static files with ETag, Cache-Control, SPA fallback, path traversal prevention.

- [x] 21.1 Implement `resolve_static_file_path` — normalize path components, reject `..` and absolute roots
- [x] 21.2 Implement static file handler: read file, SHA-256 ETag, If-None-Match → 304, Content-Type via mime_guess
- [x] 21.3 Implement SPA fallback (return index_file for non-matching paths when spa_fallback = true)
- [x] 21.4 Implement exclude_paths (404 for paths in list even if file exists)
- [x] 21.5 Implement Cache-Control max-age from config

---

## Epic 22: Session Management

**Spec:** `rivers-httpd-spec.md` (§12), `rivers-auth-session-spec.md` (§4-8)
**Goal:** Cookie-based sessions backed by StorageEngine, session middleware, token delivery.

- [x] 22.1 Implement `Session` struct (session_id, subject, claims, created_at, expires_at, last_seen)
- [x] 22.2 Implement session middleware: parse cookie → StorageEngine lookup → validate expiry → attach to request extensions
- [x] 22.3 Implement session creation (cryptographic random ID, StorageEngine write, cookie Set-Cookie)
- [x] 22.4 Implement dual expiry: ttl_s (absolute) and idle_timeout_s (from last_seen)
- [x] 22.5 Implement `Rivers.session.destroy()` — StorageEngine delete, clear cookie
- [x] 22.6 Implement cookie attributes: HttpOnly (enforced true), SameSite, Secure, Path, Domain
- [x] 22.7 Implement Bearer token acceptance (Authorization header as alternative to cookie)

---

## Epic 23: Auth — Guard View & CSRF

**Spec:** `rivers-auth-session-spec.md` (§3, §9)
**Goal:** Single guard view for credential validation, on_valid/on_invalid/on_failed handlers, CSRF protection.

- [x] 23.1 Implement guard view detection (`guard = true`) and single-guard validation
- [ ] 23.2 Implement guard CodeComponent contract — deferred to Epic 24 (ProcessPool)
- [ ] 23.3 Implement guard behavior on existing valid session — deferred to Epic 25 (View Layer)
- [ ] 23.4 Implement guard behavior on invalid session — deferred to Epic 25 (View Layer)
- [ ] 23.5 Implement guard on_failed handler — deferred to Epic 24 (ProcessPool)
- [x] 23.6 Implement per-view session validation: protected by default, auth="none" for public views
- [ ] 23.7 Implement automatic invalid session redirect — deferred to Epic 25 (View Layer)
- [x] 23.8 Implement CSRF double-submit cookie pattern (rivers_csrf cookie, X-CSRF-Token header validation)
- [x] 23.9 Implement CSRF token rotation interval and exempt conditions (Bearer auth, GET/HEAD/OPTIONS, MessageConsumer)

---

## Epic 24: ProcessPool Runtime

**Spec:** `rivers-processpool-runtime-spec-v2.md`
**Goal:** V8 isolate pool and Wasmtime instance pool with capability model, preemption, and worker lifecycle.

- [x] 24.1 Implement `TaskContext` struct with opaque DatasourceToken/DataViewToken/HttpToken
- [x] 24.2 Implement `ProcessPool` struct with task queue, worker pool, and watchdog thread
- [ ] 24.3 Implement V8 worker — deferred until `v8` crate added
- [ ] 24.4 Implement V8 context reset — deferred until `v8` crate added
- [ ] 24.5 Implement Wasmtime worker — deferred until `wasmtime` crate added
- [ ] 24.6 Implement V8 preemption — deferred until `v8` crate added
- [ ] 24.7 Implement Wasmtime preemption — deferred until `wasmtime` crate added
- [x] 24.8 Implement capability validation at dispatch (libs, datasources, dataviews all resolved before execution)
- [ ] 24.9 Implement worker crash recovery — deferred until engine workers available
- [x] 24.10 Implement queue backpressure (max_queue_depth, TaskError::QueueFull)
- [ ] 24.11 Implement TypeScript compilation via swc — deferred until `swc` crate added
- [ ] 24.12 Implement Rivers.crypto API — deferred until engine workers available
- [x] 24.13 Implement multiple named pools per riversd instance

---

## Epic 25: View Layer — REST

**Spec:** `rivers-view-layer-spec.md` (§1-5, §12-13)
**Goal:** REST view routing, handler pipeline, DataView and CodeComponent handler dispatch.

- [x] 25.1 Implement view router — match path + method to ApiViewConfig
- [x] 25.2 Implement `ParsedRequest` construction (path_params, query_params, headers, body)
- [x] 25.3 Implement `ViewContext` with sources HashMap, meta, trace_id
- [x] 25.4 Implement DataView handler dispatch — parameter mapping (query, path subtables), DataView execution, result into ctx.sources["primary"]
- [x] 25.5 Implement CodeComponent handler dispatch — resource declaration validation, ProcessPool submit (stub — engine unavailable)
- [x] 25.6 Implement handler pipeline stages: pre_process (observer), on_request (accumulator), primary, transform (chained), on_response (accumulator), post_process (observer) (stubs — CodeComponent deferred)
- [ ] 25.7 Implement parallel execution for contiguous `parallel = true` stages via join_all (deferred — requires CodeComponent)
- [ ] 25.8 Implement on_error and on_timeout observer stages (deferred — requires CodeComponent)
- [ ] 25.9 Implement null datasource pattern (`datasource = "none"`) (deferred — requires DataView wiring)
- [ ] 25.10 Implement on_session_valid stage (session_stage positioning) (deferred — requires CodeComponent)
- [x] 25.11 Implement response serialization (JSON by default, CodeComponent { status, headers, body } envelope)
- [x] 25.12 Implement view validation (spec §13 rules: dataview-only-for-REST, WS/SSE must be GET, MessageConsumer no path, rate_limit > 0, dataview exists)

---

## Epic 26: View Layer — WebSocket

**Spec:** `rivers-view-layer-spec.md` (§6)
**Goal:** WebSocket views with Broadcast and Direct modes.

- [x] 26.1 Implement WS types and error handling (actual axum ws upgrade deferred to handler wiring)
- [x] 26.2 Implement `WebSocketMode::Broadcast` — BroadcastHub with shared broadcast channel
- [x] 26.3 Implement `WebSocketMode::Direct` — ConnectionRegistry with per-connection routing
- [ ] 26.4 Implement `on_stream` handler for inbound client messages via EventBus (deferred — requires CodeComponent)
- [x] 26.5 Implement connection limits (max_connections, error on exceeded)
- [x] 26.6 Implement WS rate limiting (token bucket per connection, messages_per_sec)
- [ ] 26.7 Implement lag handling (deferred — requires live WS connection loop)
- [x] 26.8 Implement session revalidation message format and WebSocketRouteManager

---

## Epic 27: View Layer — SSE

**Spec:** `rivers-view-layer-spec.md` (§7)
**Goal:** Server-Sent Events views with hybrid push model.

- [x] 27.1 Implement SSE event types and wire format serialization
- [x] 27.2 Implement SseChannel with broadcast subscriber model, connection limits, tick_interval_ms, trigger_events
- [ ] 27.3 Implement hybrid push loop: `tokio::select!` between tick timer and EventBus triggers (deferred — requires CodeComponent + EventBus wiring)
- [x] 27.4 Implement session revalidation terminal event format (session_expired_event)
- [x] 27.5 Implement SseRouteManager for per-view channel registration

---

## Epic 28: View Layer — MessageConsumer

**Spec:** `rivers-view-layer-spec.md` (§8)
**Goal:** Event-driven views with no HTTP route, driven by EventBus events from BrokerConsumerBridge.

- [x] 28.1 Implement MessageConsumer config extraction and registry (no HTTP route)
- [ ] 28.2 Implement EventBus subscription for on_event topic (deferred — requires EventBus wiring)
- [ ] 28.3 Implement handler dispatch: event payload as request body, CodeComponent execution (deferred — requires CodeComponent)
- [x] 28.4 Implement DirectHttpAccess error type for 400 response
- [x] 28.5 Implement validation (no path, on_event required, no on_stream)
- [x] 28.6 Implement MessageEventPayload serialization for handler input

---

## Epic 29: Streaming REST

**Spec:** `rivers-streaming-rest-spec.md`
**Goal:** Streaming response bodies for REST views using AsyncGenerator handlers.

- [x] 29.1 Implement StreamingFormat enum (NDJSON, SSE) with content types and validation
- [x] 29.2 Implement StreamChunk with NDJSON and SSE wire format serialization
- [x] 29.3 Implement StreamingConfig with stream_timeout_ms default (120000)
- [x] 29.4 Implement NDJSON wire format (`application/x-ndjson`, newline-delimited JSON)
- [x] 29.5 Implement SSE wire format (`text/event-stream`, data:/event: fields)
- [x] 29.6 Implement poison chunks (stream_terminated for NDJSON, event: error for SSE)
- [x] 29.7 Implement streaming validation (REST-only, CodeComponent-only, no pipeline)
- [ ] 29.8 Implement generator drive loop in ProcessPool (deferred — requires CodeComponent)
- [ ] 29.9 Implement client disconnect detection (deferred — requires live HTTP connections)
- [ ] 29.10 Implement `Rivers.view.stream()` (deferred — requires ProcessPool API)

---

## Epic 30: Polling Views

**Spec:** `rivers-polling-views-spec.md`
**Goal:** Rivers-managed poll loops for SSE/WS views with diff strategies and client deduplication.

- [x] 30.1 Implement PollLoopRegistry: key = `poll:{view}:{param_hash}`, get_or_create/remove
- [ ] 30.2 Implement tick execution loop: DataView execute → load prev → diff → broadcast (deferred — requires DataView wiring)
- [x] 30.3 Implement `hash` diff strategy (SHA-256 of canonical JSON)
- [x] 30.4 Implement `null` diff strategy (non-empty presence check)
- [ ] 30.5 Implement `change_detect` diff strategy (deferred — requires CodeComponent)
- [x] 30.6 Implement client fan-out via PollLoopState broadcast channel with shared parameter deduplication
- [x] 30.7 Implement `emit_on_connect` flag in PollLoopState
- [ ] 30.8 Implement StorageEngine integration for prev state persistence (deferred — requires wiring)

---

## Epic 31: Admin API

**Spec:** `rivers-httpd-spec.md` (§15)
**Goal:** Separate admin server with status, drivers, datasources, deploy endpoints, Ed25519 auth, mTLS, RBAC.

- [x] 31.1 Admin server on separate socket already exists (server.rs build_admin_router)
- [ ] 31.2 Implement localhost binding enforcement when no public_key (deferred — requires TLS)
- [x] 31.3 Implement timestamp validation with ±5 min replay window
- [x] 31.4 Implement RBAC: AdminPermission enum, roles → permissions, identity → role, check_permission
- [x] 31.5 Admin status/drivers/datasources endpoints already exist (server.rs)
- [x] 31.6 Implement Deployment state machine (PENDING→RESOLVING→STARTING→RUNNING/FAILED→STOPPING→STOPPED)
- [x] 31.7 Implement IP allowlist enforcement (check_ip_allowlist)
- [x] 31.8 Implement `no_auth` flag that bypasses all permission checks
- [ ] 31.9 Implement Ed25519 signature verification (deferred — requires ed25519-dalek crate)

---

## Epic 32: App Bundle Deployment & Lifecycle

**Spec:** `rivers-application-spec.md` (§7-9, §12, §14)
**Goal:** Bundle deployment lifecycle: PENDING → RESOLVING → STARTING → RUNNING, startup order, service resolution.

- [x] 32.1 Deployment state machine already in admin.rs (reused by deployment module)
- [x] 32.2 Implement deploy_id assignment (deploy_UUID) in DeploymentManager.create
- [x] 32.3 Implement resource resolution: datasource/service/LockBox matching
- [x] 32.4 Implement service resolution checking against available_services map
- [x] 32.5 Implement startup order: topological sort, services before mains, parallel when independent
- [ ] 32.6 Implement health check with exponential backoff (deferred — requires HTTP client)
- [ ] 32.7 Implement redeployment zero-downtime (deferred — requires live server orchestration)
- [ ] 32.8 Implement auth scope carry-over (deferred — requires inter-service HTTP)
- [x] 32.9 Implement preflight checks: port conflicts, appId uniqueness, app type validation

---

## Epic 33: Health Endpoints

**Spec:** `rivers-httpd-spec.md` (§14)
**Goal:** /health and /health/verbose endpoints for load balancer probes and diagnostics.

- [x] 33.1 Implement HealthResponse with status/service/environment/version
- [x] 33.2 Implement VerboseHealthResponse with pool snapshots, draining, inflight, uptime_seconds
- [x] 33.3 Implement `parse_simulate_delay` for `?simulate_delay_ms=N` query parameter
- [x] 33.4 Implement UptimeTracker for uptime reporting
- [ ] 33.5 Wire health response types into server.rs handlers (deferred — requires AppContext expansion)

---

## Epic 34: GraphQL Integration

**Spec:** `rivers-view-layer-spec.md` (§9)
**Goal:** async-graphql integration with DataView-driven resolver bridge.

- [ ] 34.1 Integrate `async-graphql` with Axum router (deferred — requires async-graphql crate)
- [x] 34.2 Implement schema generation from JSON schemas → GraphQL types (generate_graphql_types)
- [x] 34.3 Implement ResolverMapping bridge type (field → DataView, argument mapping)
- [ ] 34.4 Implement mutation support via CodeComponent resolvers (deferred — requires CodeComponent)
- [x] 34.5 Implement GraphqlConfig with introspection toggle, max_depth, max_complexity, validation

---

## Epic 35: Hot Reload (Dev Mode)

**Spec:** `rivers-httpd-spec.md` (§16)
**Goal:** Config file watcher that swaps view routes, DataViews, and security config without server restart.

- [x] 35.1 Implement HotReloadState with RwLock-guarded config, watch channel notifications, version counter
- [ ] 35.2 Implement file watcher via `notify` crate (deferred — requires notify crate)
- [x] 35.3 Implement atomic swap with version increment and subscriber notification
- [x] 35.4 Verified in-flight requests use Arc<ServerConfig> snapshot unaffected by swap (tested)
- [x] 35.5 Implement check_reload_scope to detect changes requiring restart (host, port, TLS)

---

## Epic 36: CLI Tools

**Spec:** `rivers-lockbox-spec.md` (§7), `rivers-application-spec.md` (§12)
**Goal:** `riversd`, `riversctl`, and `riverpackage` command-line tools.

- [x] 36.1 Implement CLI argument parser (serve, doctor, preflight, version, help; --config, --log-level, --no-admin-auth)
- [x] 36.2 Implement `riversd` binary entry point (already exists in main.rs)
- [ ] 36.3 Wire CLI args into main.rs (config loading from path, log level, doctor/preflight dispatch)
- [ ] 36.4 Implement `riversctl` admin client with Ed25519 request signing (separate binary — deferred)
- [ ] 36.5 Implement `riverpackage` bundle validation tool (separate binary — deferred)

---

## Epic 37: Error Response Format

**Spec:** `rivers-httpd-spec.md` (§18)
**Goal:** Consistent JSON error envelope across all server-generated errors.

- [x] 37.1 Implement ErrorResponse envelope with code/message/details/trace_id, map_view_error
- [x] 37.2 Implement ErrorCategory → status code mapping (400/401/403/404/405/408/409/422/429/500/503/504)
- [x] 37.3 Implement convenience constructors (bad_request, unauthorized, forbidden, not_found, etc.)
- [ ] 37.4 Ensure CORS headers added to error responses (deferred — requires CORS middleware wiring)

---

## Deferred Epics (v2+)

### Epic D1: RPS — Rivers Provisioning Service

**Spec:** `rivers-rps-spec-v2.md`
**Status:** Deferred to v2. Large scope — trust bundle model, secret broker, node provisioning, alias registry, poll protocol.

### Epic D2: Clustering — Raft, Gossip, Multi-Node

**Spec:** `rivers-rps-spec-v2.md` (§2), `rivers-httpd-spec.md` (§19 cluster references)
**Status:** Deferred to v2. Blocked on D1 — requires RPS for trust bundle distribution. Gossip protocol, consensus runtime, cross-node session sharing.
