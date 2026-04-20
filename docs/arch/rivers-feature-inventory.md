# Rivers — Feature Inventory

Extracted from all specification documents. Top-level features with granular sub-feature breakdown.

---

## 1. HTTP Server (httpd)

### 1.1 Protocol Support
- HTTP/1.1 with keep-alive
- HTTP/2 with protocol upgrade negotiation
- TLS via rustls with configurable cert/key paths (through `axum-server`)

### 1.2 Two-Server Architecture
- **Main server:** application traffic — view routes, static files, health endpoint, gossip receiver; full middleware stack
- **Admin server:** operational endpoints on separate socket; separate middleware stack (subset); optional separate TLS; spawned only when `admin_api.enabled = true`
- Both Axum-based, built on `hyper` and `tokio`

### 1.3 Static File Serving
- Directory-based static asset serving
- SPA fallback: serves `index.html` for non-existent paths
- Content-type detection
- ETag-based caching with SHA-256 content hashing

### 1.4 Middleware Stack (ordered)
- `trace_id` → `request_observer` → `timeout` → `backpressure` → `shutdown_guard` → `rate_limit` → `session` → `security_headers` → `compression`
- Admin server subset: `trace_id` → `timeout` → `security_headers`

### 1.5 Security Headers
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY`
- `X-XSS-Protection: 1; mode=block`
- `Referrer-Policy: strict-origin-when-cross-origin`
- `Strict-Transport-Security: max-age=31536000; includeSubDomains`
- CSP is the operator's responsibility — not injected by default.
- Handler header blocklist (SEC-8): security-sensitive headers (`set-cookie`, `access-control-*`, `host`, `transfer-encoding`, `connection`, `upgrade`, `x-forwarded-for`) silently dropped from handler output

### 1.6 Error Response Envelope (SHAPE-2)
- Consistent JSON error format across all error responses: `{ code, message, details?, trace_id }`
- Applied to: 429 rate limit, 503 backpressure/shutdown, all framework-generated errors

### 1.7 Backpressure
- Semaphore-based request queue (`Arc<Semaphore>`)
- Configurable `queue_depth` (default: 512) and `queue_timeout_ms` (default: 100)
- 503 with `Retry-After: 1` when exhausted
- Streaming responses (SSE, WebSocket) hold permit for connection lifetime

### 1.8 Graceful Shutdown
- Signal sources: SIGTERM, SIGINT, watch channel (programmatic)
- `ShutdownCoordinator` with atomic draining flag and inflight counter
- In-flight request drainage; new requests during drain receive 503
- Connection close propagation
- Resource release ordering: pools → BrokerConsumerBridge drain → write batch flush

### 1.9 Rate Limiting
- Token bucket algorithm (global + per-view)
- Key strategies: IP-based (default) or custom header-based (e.g., API key header); no `X-Forwarded-For` trust without explicit proxy config
- Configurable burst size for traffic spikes
- Bucket eviction at 10,000 entries with stale/oldest removal
- WebSocket: per-connection rate limiting
- REST/SSE: per-IP rate limiting

### 1.10 Hot Reload (Development Mode)
- Config file watching via mtime polling (no external dependencies)
- Reloads: view routes, DataView configs, DataView engine, static file config, security config
- Atomic `RwLock::write()` swap — in-flight requests use snapshot, new requests see new config
- Does NOT restart server, rebind sockets, re-init pools, or reload plugins

---

## 2. View System

### 2.1 REST Views
- HTTP method + path routing
- DataView or CodeComponent as handler
- Per-view rate limiting and CORS overrides
- Fully declarative CRUD: one DataView declaration with per-method queries/schemas produces a complete REST resource with no handler code

### 2.2 Handler Pipeline (Collapsed Model)
- Four-label pipeline, all using the same `ctx` signature:
  - `pre_process` → ctx available, resdata empty
  - DataViews execute → results land on `ctx.data`, primary populates `ctx.resdata`
  - `handlers` → execute in declared order, modify `ctx.resdata`
  - `post_process` → `ctx.resdata` is final, side effects only
  - `on_error` → fires on failure at any step
- Sequential-only execution (SHAPE-12: parallel execution removed)
- Old stages (`on_request`, `transform`, `on_response`) absorbed into the collapsed model

### 2.3 Null Datasource Pattern
- `datasource = "none"` for views that don't require any backend datasource
- Useful for pure-logic handlers, aggregation endpoints, health checks

### 2.4 WebSocket Views
- Broadcast mode: fan-out to all connected clients
- Direct mode: targeted message delivery via ConnectionRegistry
- Session revalidation at configurable intervals on persistent connections
- Per-connection rate limiting
- Connection lifecycle hooks
- Binary frame rate-limited logging (SHAPE-13)

### 2.5 Server-Sent Events (SSE)
- Event-driven push from EventBus triggers
- Tick-based polling push at intervals
- Combined mode: tick + event triggers
- Client reconnection with `Last-Event-ID`

### 2.6 MessageConsumer Views
- EventBus-driven handler execution
- Broker message processing (Kafka, RabbitMQ, NATS)
- Fire-and-forget or acknowledgment-based consumption
- Auto-exempt from session requirements; opt-in via `auth = "session"` (MessageConsumer session exemption)

### 2.7 Streaming REST
- POST with streaming response body
- Wire formats: NDJSON (`application/x-ndjson`) and SSE (`text/event-stream`)
- `Rivers.view.stream()` API (returns `AsyncGenerator<StreamChunk>`) for handler-driven streaming
- Poison chunk error handling: final error chunk with `stream_terminated` field (SHAPE-15: runtime guard)
- `stream_timeout_ms` replaces standard `task_timeout_ms` for streaming handlers
- Mid-stream error delivery without premature connection close
- Generator drive loop with backpressure awareness

### 2.8 GraphQL Integration
- Served at `/graphql` endpoint on main server
- Powered by `async-graphql` crate
- Configured per-application

---

## 3. Data Access Layer

### 3.1 DataViews
- Named, parameterized query definitions bound to datasources
- CRUD model: one DataView, four operations — HTTP method determines which query/schema/parameters are active
- Per-method queries: `get_query`, `post_query`, `put_query`, `delete_query`
- Per-method schemas: `get_schema`, `post_schema`, `put_schema`, `delete_schema`
- Per-method parameters with `$variable` binding against declared parameter sets
- Backward compat aliases: `query` → `get_query`, `return_schema` → `get_schema`, `parameters` → `get.parameters`
- Operation inference from SQL statement first token (SHAPE-7)
- Primary DataView binding: view declares `primary` field for `ctx.resdata` population

### 3.2 Pseudo DataViews
- Runtime-constructed DataViews via `ctx.datasource(name)` builder chain
- Builder API: `fromQuery()`, `fromSchema()`, `withGetSchema()`, `withPostSchema()`, `withPutSchema()`, `withDeleteSchema()`, `build()`
- `.build()` creates the DataView object; does not execute — schema syntax-checked at build time
- Local, disposable, single-handler scope — no caching, no cache invalidation, no streaming, no EventBus registration
- Promotion path: prototype → harden (promote to TOML) → simplify (handler may disappear)

### 3.3 Two-Tier Caching
- L1: in-process LRU cache (per-node), memory-bounded (default 150 MB via `l1_max_bytes`)
- L1 uses HashMap for O(1) key lookup + VecDeque for LRU eviction order
- L1 returns `Arc<QueryResult>` — cache hits are pointer bumps, no deep clones
- L1 eviction: LRU entries evicted when total bytes exceed `l1_max_bytes` or count exceeds `l1_max_entries` (100K safety valve)
- Memory tracked via `QueryResult::estimated_bytes()` — proportional estimate, no allocator hooks
- L2: StorageEngine-backed (Redis or SQLite) with TTL
- Canonical JSON cache key derivation: `BTreeMap` ordering + `serde_json::to_string` + SHA-256 + hex encoding (SHAPE-3)
- L2 skip when result exceeds `l2_max_value_bytes`
- Cache always present as `Arc<dyn DataViewCache>` — never `Option`. Uses `NoopDataViewCache` fallback when unconfigured
- Cache invalidation by view name or full flush; write DataViews declare `invalidates` list
- Shared key derivation algorithm across cache, polling, and StorageEngine (see Appendix: Canonical JSON & Key Derivation)

### 3.4 Connection Pooling
- Per-datasource connection pools
- Circuit breaker: rolling window model (not fixed-window)
- Configurable pool size, idle timeout, max lifetime
- Health checks on connection checkout

### 3.5 Prepared Statements
- PostgreSQL and MySQL: native prepared statement support
- SQLite: named parameter binding
- Statement caching per connection

### 3.6 Datasource Event Handlers
- Fire-and-forget observer hooks on datasource operations
- Does not block the request path

---

## 4. Driver Schema Validation

### 4.1 Three-Stage Validation Chain
- **Build time** (`riverpackage --pre-flight`): `SchemaSyntaxChecker` — structural validation of schema document per driver
- **Deploy time** (`riversd`): `SchemaSyntaxChecker` — re-verified against real registered driver
- **Request time**: `Validator` (input) → `Executor` → `Validator` (output)
- By the time a request arrives, every schema has been validated twice

### 4.2 Driver Contract — Three Responsibilities
- `SchemaSyntaxChecker`: examines schema document only; catches missing required fields, invalid attributes, structural incompatibilities, orphan `$variable`/parameter mismatches
- `Validator`: examines data against schema at request time; catches type mismatches, missing required fields, constraint violations (min/max/pattern), unexpected fields
- `Executor`: runs the operation with validated input; output checked by Validator before reaching caller

### 4.3 Per-Method Schema Model
- Schemas scoped per HTTP method: `get_schema`, `post_schema`, `put_schema`, `delete_schema`
- GET request: output validator only; POST with both schemas: input validator before, output validator after; DELETE with no return schema: input validator only

### 4.4 Per-Driver Validation Rules
- **PostgreSQL**: row-and-column schemas; type mapping (uuid, text, integer, numeric, boolean, timestamptz, jsonb, bytea); column count validation
- **MySQL**: similar to PostgreSQL; MySQL-specific type mappings (varchar, int, decimal, datetime, blob, json)
- **SQLite**: relaxed typing (affinity model); named parameter validation
- **Redis**: data-structure–aware schemas (string, hash, list, set, sorted_set); key_pattern validation; no cross-type validation
- **Memcached**: simple KV — key + value type only; no structural validation beyond key format
- **Faker**: all schema types valid (synthetic generation); validates field generation rules
- **HTTP**: validates request/response body schemas; no SQL; path parameter validation
- **Kafka**: message schema with key/value/headers; topic declaration validation
- **RabbitMQ**: message schema with routing key and exchange; queue declaration validation
- **NATS**: message schema with subject pattern; subject wildcard validation
- **EventBus**: topic + payload schema; validates against EventBus topic contract

### 4.5 Common Schema Fields
- `driver`: routes to correct validation engine
- `type`: data shape (object, hash, string, message, etc.)
- `description`: human documentation
- `fields`: array of field descriptors (name, type, required, min, max, pattern, default)

### 4.6 Rivers Primitive Types
- `uuid`, `string`, `integer`, `float`, `decimal`, `boolean`, `email`, `phone`, `datetime`, `date`, `url`, `json`, `bytes`

### 4.7 Pseudo DataView Validation
- Schema syntax-checked at `.build()` time (same `SchemaSyntaxChecker` used for declared schemas)

### 4.8 Plugin Driver Requirements
- Plugin drivers must implement all three traits: `SchemaSyntaxChecker`, `Validator`, `Executor`

---

## 5. Schema System (v2)

### 5.1 File-Referenced Schemas
- Schemas live in `.schema.json` files, referenced by path in TOML — not inline
- Same JSON format whether in files or constructed via pseudo DataView builder

### 5.2 Driver-Aware Validation
- `driver` field in every schema routes to the correct driver's validation engine
- A Redis schema and a Postgres schema are fundamentally different shapes

### 5.3 Extended Fields
- `x-type`: driver-specific extended type hints
- `nopassword`: field-level security annotation

### 5.4 Two-Stage Validation Pipeline
- `riverpackage --pre-flight`: build-time structural validation
- `riversd`: deploy-time re-verification against live registered drivers

### 5.5 Resources Declaration
- `[[datasources]]` and `[[services]]` blocks in TOML resources file
- Per-app resource scoping

---

## 6. Datasource Drivers

### 6.1 Built-in Drivers
- **PostgreSQL** (tokio-postgres): transactions, prepared statements, connection pooling
- **MySQL** (mysql_async): transactions, connection pooling
- **SQLite** (rusqlite): WAL mode, named parameters, `:memory:` support
- **Redis** (redis crate): GET/MGET/HGET/HGETALL/LRANGE/SMEMBERS/SET/DEL/EXPIRE and more
- **Memcached** (async-memcached): standard KV operations
- **EventBus** (internal): publish/subscribe via standard datasource interface
- **Faker** (synthetic): schema-driven random data generation for testing/prototyping
- **Filesystem** (std::fs): chroot-sandboxed directory access, eleven typed operations (readFile, readDir, stat, exists, find, grep, writeFile, mkdir, delete, rename, copy), direct I/O in the V8 worker thread (no pool, no IPC), no credentials required. Configurable `max_file_size` and `max_depth` limits. See `rivers-filesystem-driver-spec.md`.

### 6.2 HTTP Driver (Separate Trait)
- Separate `HttpDriver` trait — distinct from `DatabaseDriver` and `MessageBrokerDriver`
- Protocol activation: `http`, `http2`, `sse`, `websocket`
- Auth models: `bearer`, `basic`, `api_key`, `oauth2_client_credentials` (with token lifecycle management)
- Path templating with `{param}` syntax; body templating
- Response mapping: JSON object → single row, JSON array → one row per element
- Connection pooling and keep-alive
- Retry with exponential/linear backoff; `Retry-After` header parsing (SHAPE-16: declared format)
- Circuit breaker integration
- Configurable timeout and redirect policy
- Streaming path: SSE/WebSocket datasources via `BrokerConsumerBridge`

### 6.3 Plugin Drivers (Real Implementations)
- MongoDB 3.x
- Elasticsearch 9.x
- Kafka (rskafka)
- RabbitMQ (lapin)
- NATS (async-nats)
- Redis Streams
- InfluxDB

### 6.4 Additional Plugin Drivers
- Cassandra (scylla)
- CouchDB (HTTP-based)
- LDAP (ldap3)

### 6.5 Planned Drivers
- Neo4j (Bolt protocol via `neo4rs` — planned for v0.53)

### 6.6 Five-Op Driver Contract
- Standard operations: `query`, `execute`, `ping`, `begin`, `stream`
- DriverError distinction: `Unsupported` vs `NotImplemented` (SHAPE-6)
- Operation inference algorithm from SQL first token (SHAPE-7)
- DriverFactory registration and discovery
- Plugin system: ABI version check, `catch_unwind` registration
- `DriverError::Forbidden` — operation rejected by security policy (DDL guard, admin op guard)
- `Connection::ddl_execute()` — separate execution path for DDL/admin operations
- `Connection::admin_operations()` — per-driver admin operation denylist
- `is_ddl_statement()` — SQL DDL detection utility (CREATE/ALTER/DROP/TRUNCATE)
- `check_admin_guard()` — combined SQL DDL + operation token guard
- `DatabaseDriver::operations()` → `&[OperationDescriptor]` (default empty) — optional, framework-level opt-in for drivers that want to expose a typed JS method surface. Declaring a non-empty catalog lets the V8 isolate emit a typed proxy on `ctx.datasource("name")` with per-op type-checked arguments, defaults, and direct dispatch. Used today by `filesystem`; open to any future driver. See `OperationDescriptor`, `Param`, `ParamType` in `rivers-driver-sdk`.
- `DatasourceToken::Direct { driver, root }` — self-contained dispatch token emitted by drivers whose ops don't need a pool. The V8 worker runs the driver's `Connection::execute` synchronously in-thread (no IPC, no pool round trip) via `Rivers.__directDispatch`. Contrasts with `Pooled { pool_id }` (the default for SQL/broker/HTTP drivers).

### 6.7 Two Driver Contracts
- **DatabaseDriver**: request/response (query → result)
- **MessageBrokerDriver**: continuous push (subscribe → stream of messages)

### 6.8 Broker Contracts
- `InboundMessage`, `OutboundMessage`, `BrokerMetadata`, `FailurePolicy`
- `BrokerConsumerBridge`: broker → EventBus directly (SHAPE-18: no StorageEngine buffering)
- Consumer lag detection and drain on shutdown

### 6.9 rps-client Driver
- Application-facing RPS access
- mTLS enforcement for RPS communication

---

## 7. Authentication & Session Management

### 7.1 Guard View Pattern
- Single entry-point view for credential validation
- Any auth mechanism supported (JWT, OAuth2, SAML, magic links, etc.)
- Guard CodeComponent returns `IdentityClaims` on success
- Framework creates signed session from claims
- Browser mode vs API/mobile mode: API mode returns `_response` key with token in response body

### 7.2 Session Lifecycle
- Session creation with configurable TTL (`ttl_s`) and idle timeout (`idle_timeout_s`)
- Token delivery: HttpOnly Secure cookie + optional response body token (for API clients)
- Session storage in StorageEngine (scoped `session:` namespace)
- Session expiration and renewal
- Session invalidation (logout)
- `Rivers.session` API for handler-side session access

### 7.3 CSRF Protection
- Double-submit cookie pattern with rotation interval
- Auto-validated on state-changing methods (POST, PUT, PATCH, DELETE)
- Bearer token requests exempt from CSRF
- CSRF token stored in StorageEngine (`csrf:` namespace)

### 7.4 Per-View Session Validation
- `on_session_valid` handler hook per view
- Context injection for RBAC, tenant resolution, custom claims
- Declarative session requirement per view (required, optional, none)

### 7.5 Cross-App Session Propagation
- Authorization header forwarded from app-main to app-services
- Claims carried in `X-Rivers-Claims` header
- Session scope carry-over across app boundaries

### 7.6 Persistent Connection Revalidation
- Configurable re-check interval for WebSocket connections
- Configurable re-check interval for SSE connections
- Session expiry terminates persistent connections

### 7.7 MessageConsumer Session Exemption
- MessageConsumer views auto-exempt from session requirements
- Opt-in session enforcement via `auth = "session"`

---

## 8. Secret Management (LockBox)

### 8.1 Encryption
- Age encryption: X25519 key agreement + ChaCha20-Poly1305
- Credential-free design: fetch → decrypt → use → zeroize per access
- Index-only in memory, values read from disk per-access (SHAPE-5)
- No in-memory value caching (secrets never persist in RAM)

### 8.2 Key Sources
- Environment variable
- File (600 permissions enforced)
- SSH agent

### 8.3 CLI Tooling
- `rivers lockbox init` — initialize keystore
- `rivers lockbox add` — add secret entry
- `rivers lockbox list` — list entries (names only)
- `rivers lockbox show` — decrypt and display a secret
- `rivers lockbox alias` / `unalias` — create/rename/remove stable references
- `rivers lockbox rotate` — rotate a secret value
- `rivers lockbox remove` — delete an entry
- `rivers lockbox rekey` — re-encrypt all entries with new master key
- `rivers lockbox validate` — verify keystore integrity

### 8.4 Alias Resolution
- Stable environment-independent references
- Per-environment alias → actual resource mapping
- Rename support without breaking references

### 8.5 Credential Types
- `string` (connection strings, bearer/API tokens)
- `base64url` (encoded keys)
- `pem` (certificates)
- `json` (structured credentials)

### 8.6 Credential Record Fields
- Optional non-secret metadata on entries: `driver`, `username`, `hosts`, `database`
- Enables full datasource connection resolution from keystore — bundles move between environments by swapping the LockBox
- Backward compatible: existing password-only keystores remain valid
- Meta sidecar pattern: `.meta.json` files alongside `.age` entries for test/dev credential metadata

### 8.7 Rotation
- No restart required: rotation writes to disk (SHAPE-5)
- New connections pick up updated credentials automatically

---

## 9. ProcessPool Runtime (Handler Execution)

### 9.1 V8 Isolate Pool
- JavaScript/TypeScript handler execution
- Clean isolate per invocation — allowlist capability injection (not blocklist)
- Sandboxed: handlers cannot access host filesystem or network directly
- Execution timeout via watchdog thread (default 5s) — terminates infinite loops
- Dynamic code generation from strings blocked (--disallow-code-generation-from-strings)
- NearHeapLimitCallback registered — OOM terminates execution gracefully instead of crashing
- Isolates above 50% heap usage discarded instead of recycled
- `console.log` not available — all logging through `Rivers.log`
- TypeScript compilation via embedded `swc` compiler at bundle load time (not per-request)

### 9.2 Isolate Reuse with Context Unbinding (SHAPE-9)
- Isolates are pooled and reused across requests
- Per-request isolation via context unbinding (bind fresh context → execute → unbind → return to pool)
- No V8 snapshots (SHAPE-10) — all state injected via globals
- Heap threshold recycling: if heap > `recycle_heap_threshold_pct` after unbinding, isolate is destroyed and recreated

### 9.3 Wasmtime WASM
- Native-speed WASM module execution via Wasmtime AOT compilation
- Resource isolation and memory limits
- Host function bindings: `rivers.db_query`, `rivers.log_info`, etc.
- Multi-language: any language with WASM target (Rust, C, C++, Go/TinyGo, AssemblyScript, Zig)
- WASI capabilities restricted per TaskContext (stdio → Rivers.log, no file access, network gated by `allow_outbound_http`)

### 9.4 Four-Scope Variable Injection Model (SHAPE-10)
- **Application** (permanent): `Rivers.*` APIs, app config, shared constants — all requests
- **Session**: session variables, identity — requests with active session
- **Connection**: connection-specific state — WS/SSE handlers only
- **Request**: capability tokens, request data, trace ID — current request only
- Narrower scope shadows broader on name collision
- REST handlers see: Application + Session + Request
- WebSocket/SSE handlers see: all four scopes

### 9.5 Capability Model
- Allowlist injection: handlers receive only declared capabilities
- Opaque tokens for credentials/datasources (raw values never escape isolate)
- Secrets resolved host-side, delivered as opaque handles
- Dispatch validation: all libs, datasources, dataviews verified before dispatch; `CapabilityError` on failure
- No dynamic imports — all imports resolved at dispatch

### 9.6 Multiple Named Pools
- Multiple pools per `riversd` instance with different worker counts, heap limits, engine types
- Views declare `process_pool = "<name>"` (default: `"default"`)
- Config: `engine`, `workers`, `max_heap_mb`/`max_memory_mb`, `task_timeout_ms`, `epoch_interval_ms`, `max_queue_depth`, `recycle_after_tasks`, `recycle_heap_threshold_pct`

### 9.7 Preemption & Safety
- V8: `TerminateExecution()` via watchdog thread — uncatchable `TerminationException`
- Wasmtime: epoch-based interruption — injected at AOT compilation (back-edges + function entry); `Trap::Interrupt` on deadline exceeded
- Single watchdog thread per pool (scans active workers)
- Memory limits per isolate (V8: `max_heap_mb`, Wasmtime: `max_memory_mb`)
- CPU time accounting

### 9.8 Handler Context (`ctx`)
- Single `ctx` object is the handler's entire world
- `ctx.trace_id`, `ctx.node_id`, `ctx.app_id`, `ctx.env`
- `ctx.session` — planned, not yet implemented in V8 engine
- `ctx.request` (read-only: method, path, headers, query, body, params)
- `ctx.data` (pre-fetched DataView results, nested namespace)
- `ctx.resdata` (mutable response payload)
- `ctx.dataview(name, params)` — call declared DataView
- `ctx.streamDataview(name, params)` — planned, not yet implemented
- `ctx.datasource(name)` — pseudo DataView builder
- `ctx.store` — application KV (get/set/del)
- `ctx.ws` — WebSocket context: connection_id, message (WS hooks only)

### 9.9 Three Handler Contracts
- **Standard handler**: receives `ctx`, modifies `ctx.resdata`, returns void
- **Guard handler**: receives `ctx`, returns `IdentityClaims` — framework creates session
- **Streaming handler**: receives `ctx`, yields chunks via `AsyncGenerator`
- All three have full capability parity (ctx.data, ctx.dataview, ctx.datasource, ctx.store, Rivers.log, Rivers.crypto)

### 9.10 Rivers Global APIs
- `Rivers.log` — structured logging (info, warn, error) with trace correlation
- `Rivers.crypto` — `hashPassword`, `verifyPassword`, `randomHex`, `randomBase64url`, `hmac`, `timingSafeEqual`
- `Rivers.http` — planned, not yet wired in V8 engine
- `Rivers.env` — planned, not yet implemented
- `Rivers.crypto.timingSafeEqual` — constant-time XOR comparison (fixed from short-circuit)

### 9.11 SSRF Prevention
- Capability model only — no IP validation (SHAPE-11)
- If `Rivers.http` is not injected, outbound HTTP doesn't exist

### 9.12 Worker Lifecycle
- Pool startup: spawn N worker threads → create empty isolates → inject Rivers API stubs → idle
- Task dispatch: build TaskContext → queue → worker binds context → loads libs → calls entrypoint → returns result → unbinds context
- Queue depth bounded by `max_queue_depth` (default: workers × 4); `TaskError::QueueFull` on overflow
- Crash recovery: dead workers replaced automatically; `WorkerCrash` event emitted; `WorkerPoolDegraded` alert on threshold

---

## 10. Polling & Change Detection

### 10.1 Poll Loops
- Automatic deduplication by view name + parameter hash
- Broadcast deduplication: N clients on same view/params = one poll loop
- Configurable tick interval

### 10.2 Diff Strategies
- `hash`: SHA-256 comparison of full result
- `null`: trigger when result is null/non-null transition
- `change_detect`: custom CodeComponent diff logic

### 10.3 State Persistence
- Previous poll state stored in StorageEngine (`poll:` namespace)
- Survives node restart (when backed by Redis/SQLite)
- Canonical JSON key derivation for poll loop identity (shared algorithm — see Appendix)

### 10.4 Diagnostic Events
- `PollChangeDetectTimeout` event (SHAPE-20)

### 10.5 `emit_on_connect` — Iceboxed
- Deferred to post-v1 (SHAPE-14)

---

## 11. Storage Engine (KV Backend)

### 11.1 Backend Implementations
- InMemory: testing and development
- SQLite: single-node default (WAL mode)
- Redis: cluster-capable, required for multi-node

### 11.2 Pure KV (SHAPE-18)
- Queue operations removed — pure key-value store only

### 11.3 Namespace Scoping
- Reserved key prefixes: `session:`, `csrf:`, `cache:`, `raft:`, `rivers:*`
- Application access via `Rivers.store` / `ctx.store` (custom namespace only, scoped to `app:{app_id}`)
- Host-layer enforcement: handlers cannot read/write reserved prefixes

### 11.4 TTL & Expiration
- Per-key TTL support
- Background `flush_expired` sweep task
- Automatic cleanup of expired entries

### 11.5 Application KV Access
- `ctx.store.get(key)` / `ctx.store.set(key, value, ttl_seconds?)` / `ctx.store.del(key)`
- Scoped to application namespace
- Available in all handler types

### 11.6 L1/L2 Tiered Cache
- L1/L2 cache keys use SHA-256 canonical JSON (SHAPE-3)
- Shared key derivation with DataView cache and polling state

### 11.7 Sentinel Key (SHAPE-8)
- Single-node enforcement via Redis sentinel key
- Prevents accidental multi-node without RPS configuration

---

## 12. EventBus & Gossip

### 12.1 Topic System
- On-demand topic creation (no upfront registration)
- Topics created on first publish
- Broadcast channel semantics

### 12.2 Subscription Priority
- Priority tiers: Expect → Handle → Emit → Observe
- Synchronous handlers block in pipeline order
- Fire-and-forget observers never block request path

### 12.3 Cross-Node Gossip
- `GossipPayload` carries events between cluster nodes
- Eventual consistency model
- Membership and health propagation

### 12.4 Integration Points
- EventBus as a datasource driver (publish/subscribe via standard interface)
- SSE trigger events from EventBus topics
- Logging driven by EventBus event subscription (Observe priority)
- BrokerConsumerBridge: broker messages → EventBus directly (SHAPE-18)

---

## 13. Application Architecture

### 13.1 Bundle Structure
- `manifest.json` at bundle root
- One `app-main` (SPA host) per bundle
- Zero or more `app-services` (backend APIs)
- `resources.json` per app for datasource/view declarations
- Libraries and schema files bundled per app

### 13.2 Deployment Lifecycle States
- `PENDING` → `RESOLVING` → `STARTING` → `RUNNING` / `FAILED` / `STOPPING` → `STOPPED`
- `appDeployId` assignment and reuse semantics
- Bundle deployed as zip file
- Startup order: app-services (parallel, respecting dependency graph) → app-main after health checks pass
- Zero-downtime redeployment with in-flight request drainage
- Atomized deploy: entire bundle or nothing

### 13.3 Preflight Checks
- Exit codes for validation stages (SHAPE-19: port conflict check removed)
- `riverpackage --pre-flight` for build-time validation

### 13.4 Module Resolution
- Relative paths within app's `libraries/` directory
- Cross-app imports forbidden — service composition via HTTP only

### 13.5 Service Discovery
- Automatic resolution by `appId`
- Health check gating before routing traffic
- HTTP-only inter-service communication (no in-process calls)

### 13.6 Health Endpoint
- `GET /health` — always returns 200, no auth, subject to full middleware stack
- `GET /health/verbose` — extended status with pool snapshots, cluster info; `?simulate_delay_ms=N` for testing
- Verbose endpoint restricted by `admin_ip_allowlist` if configured

### 13.7 Application Init Handler
- Per-application initialization CodeComponent that runs at startup (Phase 1.5)
- Declared in app manifest: `init.module` + `init.entrypoint`
- Runs in `ApplicationInit` execution context — sole context where DDL/admin ops are permitted
- Three-gate DDL enforcement: driver guard (Gate 1) + execution context (Gate 2) + admin whitelist (Gate 3)
- `ddl_whitelist` in `riversd.toml` — `"database@appId"` format, empty = no DDL permitted
- Init handler failure → app enters FAILED state, views not registered
- Timeout enforced via `init_timeout_s` config (default: 60s)

### 13.8 Deployment Tooling
- `cargo deploy <path>` — build and deploy Rivers to a target directory
- Dynamic mode (default): thin binaries + cdylib engine/plugin shared libraries
- Static mode (`--static`): single fat binary with all engines/plugins linked
- Generates self-signed TLS certificate at deploy target for immediate startup
- Cross-compilation from macOS to Linux via `cross` with custom Docker image
- Docker runtime container: debian-slim base, ~55 MB image
- `rivers-keystore` CLI included in deploy for application encryption key management

---

## 14. Provisioning Service (RPS v2)

### 14.1 Architecture
- Ships as App Bundles on `riversd` (zero additional infrastructure)
- Trust Bundle model: priority-ordered list for failover (replaces 2-node Raft)

### 14.2 Role System
- Nodes assigned roles containing capability declarations
- Roles define which apps, drivers, and resources a node provisions
- Nodes are provisioned, not self-aware

### 14.3 Alias Resolution
- Per-environment mapping: alias → actual resource/secret
- Environment-specific overrides (dev, staging, prod)

### 14.4 Secret Brokering
- Fetches secrets from backend vault
- Encrypts and delivers to requesting node
- Never stores secret values (pass-through only)

### 14.5 Topology Management
- CodeComponent execution for provisioning logic
- Bundle and plugin distribution to nodes
- Poll protocol: nodes observe sequence numbers for role/alias updates

---

## 15. Clustering

### 15.1 Gossip Protocol
- Membership discovery and propagation
- Event distribution across nodes
- Health state propagation

### 15.2 Multi-Node Constraints
- Redis StorageEngine required for session/cache consistency
- Single-node enforcement sentinel key prevents accidental multi-node without RPS (SHAPE-8)
- Shared state via StorageEngine, not in-process

---

## 16. Logging & Observability

### 16.1 Event-Driven Logging
- All system events published to EventBus
- LogHandler subscribes at Observe priority and formats output
- Decoupled: logging doesn't block request path

### 16.2 Output Formats
- JSON: structured, newline-delimited (machine-readable)
- Text: tracing-style spans (human-readable)

### 16.3 Trace Correlation
- Unique `trace_id` per request
- Propagated through entire handler pipeline
- Carried across inter-service HTTP calls
- W3C traceparent support

### 16.4 Security Boundaries
- No string-scanning redaction (SHAPE-4) — structural security via LockBox + capability model
- Error strings passed through without redaction
- LockBox values never logged

### 16.5 Operational Model
- No OTLP export (no OpenTelemetry agent)
- Stdout-only: operators pipe to their log aggregators
- Optional local file logging with async buffered writer
- Fixed event-to-level mapping (not reconfigurable per event)

### 16.6 Per-Application Log Files
- `AppLogRouter` routes handler logs to `<app_log_dir>/<app_name>.log`
- Both V8 and WASM engines write to per-app files
- 10MB rotation with single backup
- Config: `[base.logging] app_log_dir = "..."`

### 16.7 Prometheus Metrics (optional)
- Feature-gated: `[metrics] enabled = true, port = 9091`
- Counters: `rivers_http_requests_total`, `rivers_engine_executions_total`
- Histograms: `rivers_http_request_duration_ms`, `rivers_engine_execution_duration_ms`
- Gauges: `rivers_active_connections`, `rivers_loaded_apps`
- Implementation: `metrics` + `metrics-exporter-prometheus` crates

---

## 17. Configuration

### 17.1 Config File (`riversd.conf`)
- TOML format
- Sections: `base`, `environment_overrides`, `data` (datasources/dataviews), `api` (views), `security`, `plugins`, `rps`, `lockbox`, `runtime` (process_pools)

### 17.2 Hot Reload
- Config file changes trigger reload without restart (development mode only)
- Graceful transition: existing connections use snapshot, new config applies to new requests

### 17.3 Environment Variable Substitution
- `${VARIABLE_NAME}` interpolation in all string fields
- Environment-specific override blocks

### 17.4 Validation
- Full config validation at startup
- Syntax errors prevent server bind
- Schema validation for datasource and view declarations
- Admin API: localhost binding enforced when no `public_key` configured

---

## 18. Admin API

### 18.1 Separate Socket
- Admin endpoints bound to separate port/socket (not public-facing)
- Disabled by default (`admin_api.enabled = false`)

### 18.2 Management Endpoints
- `GET /admin/status` — server status, config summary, driver list
- `GET /admin/drivers` — all registered driver names
- `GET /admin/datasources` — pool snapshots for all datasources
- `POST /admin/deploy` — upload and begin deployment
- `POST /admin/deploy/test` — run deployment test stage
- `POST /admin/deploy/approve` — approve a tested deployment
- `POST /admin/deploy/reject` — reject a deployment
- `POST /admin/deploy/promote` — promote approved deployment to active
- `GET /admin/deployments` — list all deployments and their status
- `POST /admin/shutdown` — gracefully shutdown the server

### 18.3 Ed25519 Request Authentication
- Signing by `riversctl`: `{method}\n{path}\n{body_sha256_hex}\n{unix_timestamp_ms}` signed with Ed25519 private key
- Verification via `X-Rivers-Signature` and `X-Rivers-Timestamp` headers
- Timestamp within ±5 minutes of server clock (replay protection)

### 18.4 Localhost Binding Enforcement
- `host = "0.0.0.0"` + no public_key → validation error at startup
- `host = "127.0.0.1"` + no public_key → allowed (dev mode)
- `host = "0.0.0.0"` + public_key → allowed (authenticated)

### 18.5 IP Allowlist
- `security.admin_ip_allowlist` — list of IPs or CIDR ranges
- Enforced at application layer
- Also gates `/health/verbose` access

### 18.6 RBAC
- Roles → permissions mapping; identity → role bindings
- Identity from client certificate CN (mTLS) or static key
- Permissions are admin endpoint names (e.g., `deploy`, `status`)
- Validation rejects bindings referencing undefined roles

### 18.7 `--no-admin-auth` Escape Hatch
- Disables Ed25519 verification for process lifetime only
- Session-scoped — does not persist across restarts
- Emits warning at startup
- Intended for initial setup and break-glass scenarios

---

## 19. CORS

### 19.1 Per-View Configuration
- Preflight handling (`OPTIONS`)
- Configurable: allowed origins, methods, headers, max age, credentials

### 19.2 Framework-Managed
- CORS headers set by middleware — handlers cannot override (header blocklist includes `access-control-*`)

---

## 20. Shaping Decisions Register

Summary of shaping decisions that modified the specification corpus:

| ID | Decision | Impact |
|---|---|---|
| SHAPE-1 | (Reserved) | — |
| SHAPE-2 | ErrorResponse envelope | Consistent `{code, message, details?, trace_id}` across all errors |
| SHAPE-3 | Cache key — canonical JSON defined | BTreeMap + serde_json + SHA-256 shared algorithm |
| SHAPE-4 | No string-scanning redaction | Structural security via LockBox/capability model |
| SHAPE-5 | LockBox index-only in memory | Values read from disk per-access; no restart on rotation |
| SHAPE-6 | DriverError: Unsupported vs NotImplemented | Distinct error semantics |
| SHAPE-7 | Operation inference algorithm | SQL first-token determines query vs execute |
| SHAPE-8 | Single-node enforcement sentinel key | Redis sentinel prevents accidental multi-node |
| SHAPE-9 | Isolate reuse with context unbinding | Context unbound between tasks, isolate stays warm |
| SHAPE-10 | Four-scope injection, no V8 snapshots | Application/Session/Connection/Request scopes |
| SHAPE-11 | SSRF: capability model only | No IP validation — if Rivers.http not injected, it doesn't exist |
| SHAPE-12 | Sequential-only pipeline stages | Parallel execution removed |
| SHAPE-13 | WebSocket binary frame logging | Rate-limited logging for binary frames |
| SHAPE-14 | `emit_on_connect` iceboxed for v1 | Deferred to post-v1 |
| SHAPE-15 | Streaming REST poison chunk guard | `stream_terminated` field in final error chunk |
| SHAPE-16 | HTTP Driver retry format | Declared Retry-After format |
| SHAPE-17 | (Not referenced) | — |
| SHAPE-18 | BrokerConsumerBridge direct to EventBus | No StorageEngine buffering; queue ops removed from SE |
| SHAPE-19 | Preflight: port conflict removed | Port conflict check removed from preflight |
| SHAPE-20 | PollChangeDetectTimeout diagnostic | New diagnostic event for poll loops |

---

## 21. Security Hardening (v0.52.7)

### 21.1 DDL/Admin Operation Guards
- `Connection::execute()` rejects DDL statements and admin operations with `DriverError::Forbidden`
- SQL drivers: `is_ddl_statement()` checks for CREATE/ALTER/DROP/TRUNCATE prefixes
- Non-SQL drivers: `admin_operations()` per-driver denylist (Redis: flushdb/flushall/config_set; MongoDB: drop_collection/create_index; etc.)
- `ddl_execute()` separate execution path — only callable from ApplicationInit context
- Three-gate enforcement: driver guard + execution context + whitelist

### 21.2 V8 Sandbox Hardening
- Execution timeout via watchdog thread + `terminate_execution()` (default 5s)
- Dynamic code generation from strings blocked at V8 platform init
- NearHeapLimitCallback terminates execution on OOM (no process crash)
- Isolate recycling: discard above 50% heap usage
- `timingSafeEqual` uses constant-time XOR accumulation

### 21.3 HTTP/TLS Security
- Outbound TLS certificate verification enabled by default
- HSTS header: `max-age=31536000; includeSubDomains`
- CSRF cookie `Secure` flag
- CORS `Vary: Origin` header when origin dynamically reflected

### 21.4 Authentication & Authorization
- Admin RBAC deny-by-default for unknown paths (requires Admin permission)
- Session tokens: 256-bit CSPRNG (was UUID v4 122-bit)
- CSRF tokens: 256-bit CSPRNG

### 21.5 Resource Limits
- DataView `max_rows` (default 1000) — truncates unbounded query results
- Error response sanitization — generic messages to clients in production, full details in debug builds

### 21.6 Known Parameter Binding Issue (Issue #54)
- `$name` placeholders from spec examples do not work uniformly across SQL drivers
- PostgreSQL/MySQL sort params alphabetically and bind positionally — can cause silent data corruption
- SQLite auto-prefixes `:` but queries use `$` — prefix mismatch
- Fix planned: DataView engine parameter translation layer

---

## 22. CLI Tools (v0.53.0)

### riverpackage init
- `riverpackage init <name> [--driver faker|postgres|sqlite|mysql]`
- Scaffolds complete bundle with manifest, resources, app.toml, schemas

### riversctl stop / status
- `riversctl stop` — reads PID file, sends SIGTERM, waits 30s, SIGKILL fallback
- `riversctl status` — shows running state, PID, port, bundle
- PID file: `<RIVERS_HOME>/run/riversd.pid`

### riversctl doctor --lint / --fix
- `--lint` — validates bundle: views, schema files, datasource refs, conventions
- `--fix` — auto-repairs: lockbox init, permissions, TLS cert gen/renewal, log dirs

### riversctl tls renew
- Regenerates self-signed cert using configured x509 params
- Shows current cert info before renewal

---

*Extracted from 22 top-level specification documents. 20 shaping decisions (SHAPE-1 through SHAPE-20) applied. Security hardening: 14 of 17 audit findings resolved (v0.52.7). Known issue: parameter binding mismatch across SQL drivers (Issue #54).*
