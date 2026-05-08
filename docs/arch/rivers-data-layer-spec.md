# Rivers Data Layer Specification

**Document Type:** Implementation Specification  
**Scope:** Driver SDK, Datasource Pool, StorageEngine, DataView Engine, Broker Drivers  
**Status:** Reference / Ground Truth  
**Source audit:** `crates/rivers-driver-sdk`, `crates/rivers-core`, `crates/rivers-data`, `crates/riversd`

---

## Table of Contents

1. [Architectural Overview](#1-architectural-overview)
2. [Driver SDK — Core Contracts](#2-driver-sdk--core-contracts)
3. [Driver SDK — Broker Contracts](#3-driver-sdk--broker-contracts)
4. [StorageEngine Layer](#4-storageengine-layer)
5. [Pool Manager](#5-pool-manager)
6. [DataView Engine](#6-dataview-engine)
7. [DataView Caching](#7-dataview-caching)
8. [Built-in Drivers](#8-built-in-drivers)
9. [Plugin Drivers](#9-plugin-drivers)
10. [Broker Consumer Bridge](#10-broker-consumer-bridge)
11. [Datasource Event Handlers](#11-datasource-event-handlers)
12. [Configuration Reference](#12-configuration-reference)

---

## 1. Architectural Overview

The data layer is a strict hierarchy. Each layer has one job. Nothing crosses layers except through the defined interface.

```
┌─────────────────────────────────────────────────────────────┐
│  View Layer  (routes, handlers, pipeline)                   │
└───────────────────────────┬─────────────────────────────────┘
                            │  DataViewRequest
┌───────────────────────────▼─────────────────────────────────┐
│  DataView Engine  (registry, param validation, cache, span) │
└───────────────────────────┬─────────────────────────────────┘
                            │  Query
┌───────────────────────────▼─────────────────────────────────┐
│  Pool Manager  (per-datasource pools, circuit breaker)      │
└───────────────────────────┬─────────────────────────────────┘
                            │  Box<dyn Connection>
┌───────────────────────────▼─────────────────────────────────┐
│  Driver  (DatabaseDriver or MessageBrokerDriver impl)       │
└─────────────────────────────────────────────────────────────┘

                 (orthogonal — not in request path)
┌─────────────────────────────────────────────────────────────┐
│  StorageEngine  (KV — internal infrastructure)              │
└─────────────────────────────────────────────────────────────┘
```

### Two driver contracts

Rivers separates request/response drivers from continuous-push brokers. Conflating them forces broker semantics into a query model that doesn't fit.

| Trait | Systems | Interaction pattern |
|---|---|---|
| `DatabaseDriver` | PostgreSQL, MySQL, SQLite, Redis, Elasticsearch, InfluxDB, etc. | Request in → Result out |
| `MessageBrokerDriver` | Kafka, RabbitMQ, NATS, Redis Streams | Continuous push delivery |

A single driver crate may implement both. Which path activates at runtime is determined by config — the presence or absence of a `[consumer]` block on the datasource.

### StorageEngine is not a datasource

<!-- SHAPE-18 amendment: StorageEngine is pure KV; queue references removed -->
`StorageEngine` is Rivers internal infrastructure — L2 DataView cache backing, session storage, poll state, and CSRF tokens. Application code never accesses it directly. It is not a datasource. It does not appear in DataView configs.

---

## 2. Driver SDK — Core Contracts

Defined in `crates/rivers-driver-sdk/src/lib.rs`. All driver plugins import from this crate.

### 2.1 DriverError

```rust
pub enum DriverError {
    UnknownDriver(String),
    Connection(String),
    Query(String),
    Transaction(String),
    Unsupported(String),      // driver exists, operation not applicable
    NotImplemented(String),   // stub placeholder, driver not yet wired  <!-- SHAPE-6 amendment -->
    Internal(String),
}
```

### 2.2 QueryValue

The universal value type crossing the adapter boundary. Every parameter and result column is a `QueryValue`.

```rust
pub enum QueryValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Null,
    Array(Vec<QueryValue>),
    Json(serde_json::Value),  // arbitrary structured payloads
}
```

`Json` variant is used for: InfluxDB batch write payloads, Kafka message bodies, MongoDB documents, and any driver operation requiring structured input that doesn't fit scalar types.

### 2.3 Query

The normalized query model passed from the DataView engine to the driver.

```rust
pub struct Query {
    pub operation: String,   // e.g. "select", "insert", "xadd", "publish"
    pub target: String,      // table / collection / stream / topic
    pub parameters: HashMap<String, QueryValue>,
    pub statement: String,   // raw native statement / command
}
```

<!-- SHAPE-7 amendment -->
`operation` is inferred from `statement` when not explicitly set, using the following algorithm:

1. Trim leading whitespace
2. Strip leading SQL comments (`--` to newline, `/* ... */`)
3. First whitespace-delimited token, lowercased
4. Map: `select|get|find|search` → Read, `insert|create|add|set|put` → Write, `update|patch|replace` → Write, `delete|remove|del` → Delete, else → Read (safe default)
5. Explicit `operation` on the DataView or Query wins over inference
6. Individual drivers may override inference for their own syntax

This drives driver-level routing between read and write paths.

### 2.4 QueryResult

```rust
pub struct QueryResult {
    pub rows: Vec<HashMap<String, QueryValue>>,
    pub affected_rows: u64,
    pub last_insert_id: Option<String>,
}
```

All drivers normalize their results to `QueryResult`. This is the type that crosses from driver → DataView engine → View layer → HTTP response serialization.

### 2.5 DatabaseDriver trait

```rust
#[async_trait]
pub trait DatabaseDriver: Send + Sync {
    fn name(&self) -> &str;
    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError>;
    fn supports_transactions(&self) -> bool { false }
    fn supports_prepared_statements(&self) -> bool { false }
}
```

Drivers are registered at startup into `DriverFactory` by name. Each datasource config references a driver by that name.

### 2.6 Connection trait

```rust
#[async_trait]
pub trait Connection: Send + Sync {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError>;
    async fn ping(&mut self) -> Result<(), DriverError>;
    fn driver_name(&self) -> &str;
}
```

The `execute` method handles all operations — reads, writes, DDL. The driver is responsible for dispatching to the correct native call based on `query.operation`.

Connections are owned by the pool. When `DataViewEngine` is done with a connection, it calls `release()` on the `DataConnection` wrapper — this returns the connection to the pool, not to the driver.

### 2.7 ConnectionParams

```rust
pub struct ConnectionParams {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,           // resolved from Lockbox at startup
    pub options: HashMap<String, String>,
}
```

<!-- SHAPE-5 amendment: LockBox uses index-only model; values read from disk per-access -->
`password` is always the resolved secret value. LockBox maintains an in-memory name/alias to entry index (no secret values in memory). The secret value is read from disk, decrypted, and zeroized after use on each connection creation. Drivers never interact with LockBox.

### 2.8 Plugin registration

Every driver plugin exports one symbol:

```rust
#[no_mangle]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(MyDriver));
}
```

For broker drivers:

```rust
registrar.register_broker_driver(Arc::new(MyBrokerDriver));
```

A crate may call both. ABI version is checked before registration — mismatched plugins are rejected with a `PluginLoadFailed` event, never a panic.

---

## 3. Driver SDK — Broker Contracts

### 3.1 InboundMessage

```rust
pub struct InboundMessage {
    pub id: MessageId,              // String
    pub destination: String,        // queue/topic/subject/stream name
    pub payload: Bytes,
    pub headers: HashMap<String, String>,
    pub timestamp: DateTime<Utc>,
    pub receipt: MessageReceipt,    // opaque — used for ack/nack only
    pub metadata: BrokerMetadata,
}
```

### 3.2 BrokerMetadata

Broker-specific envelope. Variant is determined by the driver.

```rust
pub enum BrokerMetadata {
    Kafka    { partition: i32, offset: i64, consumer_group: String },
    Rabbit   { delivery_tag: u64, exchange: String, routing_key: String },
    Nats     { sequence: u64, stream: String, consumer: String },
    Redis    { stream_id: String, group: String, consumer: String },
}
```

### 3.3 OutboundMessage

```rust
pub struct OutboundMessage {
    pub destination: String,
    pub payload: Bytes,
    pub headers: HashMap<String, String>,
    pub key: Option<String>,       // Kafka partition key, NATS subject suffix
    pub reply_to: Option<String>,  // NATS request/reply
}
```

### 3.4 MessageBrokerDriver trait

```rust
#[async_trait]
pub trait MessageBrokerDriver: Send + Sync {
    fn name(&self) -> &str;
    async fn create_producer(
        &self,
        params: &ConnectionParams,
        config: &ConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError>;
    async fn create_consumer(
        &self,
        params: &ConnectionParams,
        config: &ConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError>;
}
```

### 3.5 BrokerConsumer trait

```rust
#[async_trait]
pub trait BrokerConsumer: Send + Sync {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError>;
    async fn ack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError>;
    async fn nack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError>;
    async fn close(&mut self) -> Result<(), DriverError>;
}
```

### 3.6 BrokerProducer trait

```rust
#[async_trait]
pub trait BrokerProducer: Send + Sync {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError>;
    async fn close(&mut self) -> Result<(), DriverError>;
}
```

### 3.7 ConsumerConfig

```rust
pub struct ConsumerConfig {
    pub group_prefix: String,
    pub app_id: String,
    pub datasource_id: String,
    pub node_id: String,
    pub reconnect_ms: u64,
    pub subscriptions: Vec<SubscriptionConfig>,
}
```

Consumer group ID is derived: `{group_prefix}.{app_id}.{datasource_id}.{component}`. This is enforced by the bridge, not the driver.

### 3.8 FailurePolicy

Determines what happens when message processing fails after all retries are exhausted.

```rust
pub struct FailurePolicy {
    pub mode: FailureMode,
    pub destination: Option<String>,       // dead-letter target name
    pub handlers: Vec<FailurePolicyHandler>,
}

pub enum FailureMode {
    DeadLetter,  // route to destination datasource
    Requeue,     // return to source broker
    Redirect,    // publish to destination topic/queue
    Drop,        // discard silently
}
```

`handlers` are CodeComponent modules invoked fire-and-forget before the failure disposition executes.

---

## 4. StorageEngine Layer

Defined in `crates/rivers-core/src/storage.rs`. Rivers internal infrastructure only.

### 4.1 StorageEngine trait

```rust
#[async_trait]
pub trait StorageEngine: Send + Sync {
    // Key-value operations
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Bytes>, StorageError>;
    async fn set(&self, namespace: &str, key: &str, value: Bytes, ttl_ms: Option<u64>)
        -> Result<(), StorageError>;
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError>;
    async fn list_keys(&self, namespace: &str, prefix: Option<&str>)
        -> Result<Vec<String>, StorageError>;

    // Maintenance  <!-- SHAPE-18 amendment: queue operations (enqueue/dequeue/ack) removed -->
    async fn flush_expired(&self) -> Result<u64, StorageError>;
}
```

### 4.2 Backends

| Backend | Use case | Implementation |
|---|---|---|
<!-- SHAPE-18 amendment: queue references removed from backend descriptions -->
| `InMemoryStorageEngine` | Testing, development | `HashMap` under `Arc<Mutex>` |
| `SqliteStorageEngine` | Default single-node | `sqlx` + WAL mode, `kv_store` table |
| `RedisStorageEngine` | Cluster / shared cache | Redis `SET EX` for KV |

**SQLite schema:**

```sql
CREATE TABLE kv_store (
    namespace   TEXT    NOT NULL,
    key         TEXT    NOT NULL,
    value       BLOB    NOT NULL,
    expires_at  INTEGER,          -- unix ms, NULL = no expiry
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (namespace, key)
);

<!-- SHAPE-18 amendment: queue_store table removed -->
CREATE INDEX idx_kv_expires   ON kv_store(expires_at)   WHERE expires_at IS NOT NULL;
```

`flush_expired` deletes KV rows where `expires_at < now()`. Called by a background sweep task at `sweep_interval_s`.

### 4.3 Uses within Rivers

<!-- SHAPE-18 amendment: queue-based consumers removed -->
| Consumer | Namespace | Purpose |
|---|---|---|
| `TieredDataViewCache` | `cache:{view_name}` | L2 DataView result cache |
| `RaftLogStore` | `raft:log`, `raft:vote` | Cluster consensus persistence |

---

## 5. Pool Manager

Defined in `crates/riversd/src/pool.rs`.

### 5.1 PoolConfig

```rust
pub struct PoolConfig {
    pub max_size: usize,                  // default: 10
    pub min_idle: usize,                  // default: 0
    pub connection_timeout_ms: u64,       // default: 500
    pub idle_timeout_ms: u64,             // default: 30_000
    pub max_lifetime_ms: u64,             // default: 300_000
    pub health_check_interval_ms: u64,    // default: 5_000
    pub circuit_breaker: CircuitBreakerConfig,
}
```

Validation rejects: `max_size == 0`, `min_idle > max_size`, any timeout field == 0.

### 5.2 CircuitBreakerConfig

```rust
<!-- SHAPE-1 amendment: rolling window model replaces consecutive failure count -->
pub struct CircuitBreakerConfig {
    pub enabled: bool,
    pub failure_threshold: u32,     // failures within window before OPEN
    pub window_ms: u64,             // rolling failure window (default: 60_000)
    pub open_timeout_ms: u64,       // time in OPEN before attempting HALF_OPEN
    pub half_open_max_trials: u32,  // allowed trial calls in HALF_OPEN
}
```

**State machine:**

```
CLOSED  →(failure_threshold failures within window_ms)→  OPEN
OPEN    →(open_timeout_ms elapsed)→                 HALF_OPEN
HALF_OPEN →(trial succeeds)→                        CLOSED
HALF_OPEN →(trial fails)→                           OPEN
```

When the circuit is OPEN, `acquire()` returns `PoolError::CircuitOpen` immediately — no connection attempt is made. A `DatasourceCircuitOpened` event is emitted on the EventBus when the circuit opens.

### 5.3 Pool lifecycle

<!-- SHAPE-5 amendment: CredentialRotated event removed; connections cycle via max_lifetime -->
Each datasource has its own pool. Pools are created at server startup from `DatasourceConfig`. Credential rotation is handled at the LockBox layer — `rivers lockbox rotate` writes new credentials to disk; the next connection created by the pool reads the fresh value from LockBox automatically. Existing connections cycle out naturally via `max_lifetime_ms` or health check failure. No pool drain/rebuild is required for credential rotation.

On graceful shutdown, all pools drain before the process exits.

### 5.4 PoolSnapshot

Health snapshot accessible via `riversctl doctor` and the admin `/status` endpoint.

```rust
pub struct PoolSnapshot {
    pub datasource_id: String,
    pub active_connections: usize,
    pub idle_connections: usize,
    pub total_connections: usize,
    pub checkout_count: u64,
    pub avg_wait_ms: u64,
    pub max_size: usize,
    pub min_idle: usize,
}
```

---

## 6. DataView Engine

Defined in `crates/rivers-data/src/dataview_engine.rs`.

### 6.1 Purpose

A DataView is a named, parameterized, schema-validated query bound to a specific datasource. It is the primary way declarative Views expose data. The DataView engine is the execution facade: it resolves the name, validates parameters, checks cache, acquires a connection, executes, validates the result schema, populates cache, and releases the connection.

### 6.2 Execution sequence

```
DataViewRequest
    │
    ├─ 1. Registry lookup (DataViewConfig by name)
    │      → ViewNotFound if missing
    │
    ├─ 2. Parameter validation
    │      → type check, required check, strict mode rejects unknown params
    │      → fails BEFORE pool acquire (no wasted connection)
    │
    ├─ 3. Cache check (skip if cache_bypass = true)
    │      → L1 in-process LRU, then L2 StorageEngine
    │      → cache hit returns early, no pool acquire
    │
    ├─ 4. Pool acquire (DataConnectionManager::acquire)
    │      → circuit breaker check
    │      → timeout enforced by pool config
    │
    ├─ 5. driver.execute (tracing span: "driver.execute")
    │      → Query built from DataViewConfig + request parameters
    │
    ├─ 6. Connection release (always, even on error)
    │
    ├─ 7. Return schema validation (if return_schema configured)
    │      → validates result rows against JSON Schema
    │
    ├─ 8. Cache population (if not cache_bypass)
    │
    └─ 9. DataViewResponse returned
```

### 6.3 DataViewRequest / Response

```rust
pub struct DataViewRequest {
    pub name: String,
    pub parameters: HashMap<String, QueryValue>,
    pub timeout_ms: Option<u64>,
    pub trace_id: String,
    pub cache_bypass: bool,
}

pub struct DataViewResponse {
    pub query_result: QueryResult,
    pub execution_time_ms: u64,
    pub cache_hit: bool,
    pub trace_id: String,
}
```

Built via `DataViewRequestBuilder`. Builder validates that name is non-empty and timeout (if provided) is > 0. `build_for(view)` additionally applies parameter validation and optional parameter defaults.

### 6.4 DataViewConfig

```rust
pub struct DataViewConfig {
    pub datasource: String,                      // must exist in datasources map
    pub query: Option<String>,                   // legacy: raw statement passed to driver
    pub parameters: Vec<DataViewParameterConfig>,
    pub return_schema: Option<String>,           // JSON Schema id from schemas map
    pub validate_result: bool,                   // default: false
    pub strict_parameters: bool,                 // default: false — rejects unknown params
    pub invalidates: Vec<String>,                // DataViews to invalidate on write
    pub cache: Option<DataViewCachingPolicy>,
    pub on_event: Option<OnEventConfig>,         // event-driven trigger
    pub on_stream: Option<OnStreamConfig>,       // streaming DataView (WebSocket)

    // Per-method CRUD queries — override `query` based on HTTP method.
    // When present, these take precedence over the legacy `query` field.
    pub get_query: Option<String>,
    pub post_query: Option<String>,
    pub put_query: Option<String>,
    pub delete_query: Option<String>,

    // Per-method schemas — validate request/response by HTTP method.
    pub get_schema: Option<String>,
    pub post_schema: Option<String>,
    pub put_schema: Option<String>,
    pub delete_schema: Option<String>,

    // Per-method parameters — override `parameters` by HTTP method.
    pub get_parameters: Vec<DataViewParameterConfig>,
    pub post_parameters: Vec<DataViewParameterConfig>,
    pub put_parameters: Vec<DataViewParameterConfig>,
    pub delete_parameters: Vec<DataViewParameterConfig>,
}
```

When `execute()` receives an HTTP method, it resolves the query in order: method-specific query (e.g. `post_query`) → legacy `query` → error. This enables one DataView to serve CRUD operations across GET/POST/PUT/DELETE with different SQL statements per method.

### 6.5 DataViewParameterConfig

```rust
pub struct DataViewParameterConfig {
    pub name: String,
    pub param_type: DataViewParameterType,  // String | Integer | Float | Boolean | Array
    pub required: bool,
}
```

Optional parameters receive a zero-value default when absent: `""` for String, `0` for Integer, `0.0` for Float, `false` for Boolean, `[]` for Array.

### 6.6 Schema validation

`return_schema` references a JSON Schema document by id from `config.data.schemas`. If `validate_result = true`, every row in the `QueryResult` is validated against the schema. Validation runs after the driver executes and before the response is returned. A schema failure releases the connection cleanly and returns `DataViewError::Schema`.

<!-- SHAPE-4 amendment: redaction removed; LockBox + capability model is the security boundary -->
Error strings from validation failures and query errors are passed through to EventBus events without modification. LockBox ensures secret values never enter the driver layer, and the capability model prevents unauthorized access. Driver authors are responsible for their own error message hygiene.

### 6.7 Security boundary

Parameters are passed as `HashMap<String, QueryValue>` in `Query.parameters`. Drivers are responsible for parameterized binding to their native query interface. Rivers does not inspect or sanitize parameter values — the structural separation of statement from parameters is the security boundary.

### 6.8 Transactions

Both engines (V8 and the dynamic-engine cdylib path: V8/WASM via `librivers_engine_*.dylib`) expose the same handler API:

```js
await Rivers.db.begin("ds-name");
const r = await Rivers.db.execute("dataview-name", params);  // routed through txn conn
await Rivers.db.commit("ds-name");
// or: await Rivers.db.rollback("ds-name");
```

The two paths share semantics but maintain transaction state differently because their threading models differ.

#### V8 path (in-process)

Implemented in `crates/riversd/src/process_pool/v8_engine/context.rs::ctx_transaction_callback`. V8 isolates pin to a worker thread for the entire task, so a thread-local `TASK_TRANSACTION: Option<TaskTransactionState>` carries the per-task `TransactionMap` (keyed by datasource name only — task identity is implicit in the pinned thread).

#### Dynamic-engine path (cdylib via FFI)

Implemented across `engine_loader/host_callbacks.rs` (`host_db_begin`, `host_db_commit`, `host_db_rollback`, `host_dataview_execute`), `engine_loader/dyn_transaction_map.rs` (the map type), and `process_pool/mod.rs::dispatch_dyn_engine_task` (lifecycle hook).

Key differences from V8:
- **Map keying.** `(TaskId, datasource_name)` — dyn-engine cdylib host callbacks fire from engine threads that aren't 1:1 with task identity, so the map can't rely on a thread-local on the engine side. Instead, riversd issues a fresh `TaskId` per `dispatch_task` invocation, binds it to the riversd-side `spawn_blocking` worker thread via `TaskGuard::enter`, and host callbacks read `current_task_id()` (a `spawn_blocking`-thread-local) when they fire synchronously from the engine's C-ABI call.
- **Lifecycle.** `dispatch_dyn_engine_task` issues `TaskId`, snapshots the per-task datasource configs into `TASK_DS_CONFIGS` so `host_db_begin` can resolve `(driver_name, ConnectionParams)` without a roundtrip through the cdylib, then enters `TaskGuard`. The cdylib runs (calling `Rivers.db.begin/execute/commit/rollback` as host callbacks). On scope exit, `TaskGuard::drop` calls `DynTransactionMap::auto_rollback_all_for_task(task_id)` and clears `TASK_DS_CONFIGS` — guarantees no leaked transactions if the handler panics, errors out, or forgets to commit/rollback.
- **DataView routing.** `host_dataview_execute` reads `current_task_id()` and consults `dyn_txn_map().task_active_datasources(tid)`. When a transaction is active for the dataview's datasource, the call is routed via `with_conn_mut` (lock-free during the await — the conn is removed under the lock, then re-inserted on closure return). When a transaction is active on a *different* datasource, the call is rejected with a `DataViewError::Driver` carrying a `TransactionError:` prefix (spec §6.2).
- **Commit-failure financial-correctness gate.** Mirrors V8: on commit failure or commit timeout, `host_db_commit_inner` calls `signal_commit_failed(ds, msg)` to set a `spawn_blocking`-thread-local sentinel. After `spawn_blocking` resolves, `dispatch_dyn_engine_task` calls `take_commit_failed()` on the same thread and upgrades the result to `TaskError::TransactionCommitFailed { datasource, message }` regardless of what the handler returned.
- **Timeouts.** `HOST_CALLBACK_TIMEOUT_MS` (30s) bounds commit and rollback driver calls. Exceeded budgets produce a warning log; the connection is abandoned (Drop releases the pool slot, the server-side txn is reaped by driver/server idle timeouts).

The two paths converge on `Connection::begin_transaction / commit_transaction / rollback_transaction` as the underlying driver-trait methods (`crates/rivers-driver-sdk/src/traits.rs`). Built-in drivers (PostgreSQL, MySQL, SQLite) implement these natively via `BEGIN / COMMIT / ROLLBACK`; broker drivers and Redis return `DriverError::Unsupported` (their `supports_transactions()` returns `false`).

---

## 7. DataView Caching

Defined in `crates/rivers-data/src/tiered_cache.rs`.

### 7.1 Two-tier model

```
DataView execute request
    │
    ├─ L1: In-process LRU cache (per node, fast)
    │       cache hit → return immediately
    │
    ├─ L2: StorageEngine cache (shared, slower)
    │       L1 miss → L2 check → L1 warm on L2 hit
    │
    └─ Miss: execute driver, populate L2 then L1
```

<!-- SHAPE-3 amendment: canonical JSON + SHA-256 cache key derivation -->
Cache keys use the canonical key format `cache:views:{view_name}:{param_hash}`, where `param_hash` is derived by: (1) collecting parameters into a `BTreeMap<String, serde_json::Value>`, (2) serializing with `serde_json::to_string()`, (3) SHA-256 hashing, (4) hex-encoding the digest. Cache invalidation is event-driven — `CacheInvalidation` EventBus events clear matching entries.

### 7.2 DataViewCachingPolicy

```rust
pub struct DataViewCachingPolicy {
    pub ttl_seconds: u64,
    pub l1_enabled: bool,              // default: true
    pub l1_max_bytes: usize,           // default: 157,286,400 (150 MB)
    pub l1_max_entries: usize,         // default: 100,000 (safety valve)
    pub l2_enabled: bool,              // default: false
    pub l2_max_value_bytes: usize,     // default: 131,072 (128 KB) — results larger skip L2
}
```

L1 eviction is memory-bounded — entries are evicted LRU when total estimated bytes exceed `l1_max_bytes` or entry count exceeds `l1_max_entries`. Memory is tracked via `QueryResult::estimated_bytes()` which walks rows and values for a proportional size estimate. L2 skipping protects the StorageEngine from very large result payloads.

L1 uses a `HashMap<String, CachedEntry>` for O(1) key lookup paired with a `VecDeque<String>` for LRU eviction order.

### 7.3 DataViewCache trait

```rust
#[async_trait]
pub trait DataViewCache: Send + Sync {
    /// Returns Arc to avoid deep-cloning large result sets on cache hits.
    async fn get(
        &self,
        view_name: &str,
        parameters: &HashMap<String, QueryValue>,
    ) -> Result<Option<Arc<QueryResult>>, DataViewError>;

    /// `ttl_override` allows per-view TTL from DataViewCachingConfig.ttl_seconds.
    async fn set(
        &self,
        view_name: &str,
        parameters: &HashMap<String, QueryValue>,
        result: &QueryResult,
        ttl_override: Option<u64>,
    ) -> Result<(), DataViewError>;

    /// Invalidate cache entries for a specific view, or all entries if None.
    async fn invalidate(&self, view_name: Option<&str>);
}
```

Cache is always present as `Arc<dyn DataViewCache>` in `DataViewExecutor` — never `Option`. When no caching is configured, `NoopDataViewCache` is used as a zero-allocation fallback that always misses. `TieredDataViewCache` is always created at bundle load; L2 is only attached when a `StorageEngine` is available.

---

## 8. Built-in Drivers

Registered directly in `DriverFactory` at startup. No plugin loading required.

### 8.1 PostgreSQL (`postgres`)

Client: `tokio-postgres`. Parameterized query binding via `$1`, `$2` positional syntax. Transactions supported. `last_insert_id` via `RETURNING id` convention. Connection string assembled from `ConnectionParams`.

### 8.2 MySQL (`mysql`)

Client: `mysql_async`. Positional `?` binding. Transactions supported.

### 8.3 SQLite (`sqlite`)

Client: `rusqlite` (bundled SQLite). WAL mode, 5s busy timeout. Named parameters (`:name`, `@name`, `$name`) or auto-prefixed. Supports `:memory:` for in-memory databases. `last_insert_id` returned for write operations. Type mapping: INTEGER → `QueryValue::Integer`, REAL → `QueryValue::Float`, TEXT → `QueryValue::String`, BLOB → `QueryValue::Json` (if valid JSON) or `QueryValue::String`, NULL → `QueryValue::Null`.

### 8.4 Redis (`redis`)

Client: `redis` crate. Mapped operations: `get`, `set`, `del`, `expire`, `lpush`, `rpop`, `hset`, `hget`, `hdel`, `ping`. Generic `execute_query` dispatches by `query.operation`.

### 8.5 Memcached (`memcached`)

Operations: `get`, `set`, `delete`, `ping`.

### 8.6 EventBus (`eventbus`)

Built-in driver implementing `DatabaseDriver`. Operations: `PUBLISH`, `SUBSCRIBE`, `PING`. Used by views to write events to the EventBus through the standard datasource interface. Backed by `TopicRegistry` — per-topic `tokio::sync::broadcast` channels. Cross-node delivery via gossip.

---

## 9. Plugin Drivers

Loaded from the plugin directory via `libloading`. All implement `DatabaseDriver` or `MessageBrokerDriver` or both.

### 9.1 Real implementations

| Driver | Crate | Client | Status |
|---|---|---|---|
| MongoDB | `rivers-plugin-mongodb` | `mongodb` 3.x | Real |
| Elasticsearch | `rivers-plugin-elasticsearch` | `elasticsearch` 9.x | Real |
| Kafka | `rivers-plugin-kafka` | `rskafka` 0.6 | Real — DatabaseDriver + MessageBrokerDriver |
| RabbitMQ | `rivers-plugin-rabbitmq` | `lapin` 2.x | Real — DatabaseDriver + MessageBrokerDriver |
| NATS | `rivers-plugin-nats` | `async-nats` 0.35 | Real — MessageBrokerDriver |
| Redis Streams | `rivers-plugin-redis-streams` | `redis` 0.25 | Real — MessageBrokerDriver |
| InfluxDB | `rivers-plugin-influxdb` | `reqwest` 0.12 | Real |

<!-- SHAPE-6 amendment: honest stubs return NotImplemented, not Unsupported -->
### 9.2 Driver status

**Promoted to real implementations:** Neo4j (v0.52.7), Cassandra, LDAP — all fully implemented as plugin drivers.

**Removed from roadmap:** Solr, Hadoop/HDFS, ZooKeeper, ActiveMQ, PingIdentity — no demand, removed.

`DriverError::NotImplemented` is reserved for drivers that register but have pending operations. `Unsupported` is for operations not applicable to a driver type.

### 9.3 Kafka specifics

Operations as `DatabaseDriver`: `list_topics`, `produce`, `fetch` (direct offset read), `ping`.

As `MessageBrokerDriver`: consumer groups with `XREADGROUP` semantics, configurable `filter_subject` (pull consumer model), dead-letter via producer to a separate topic.

### 9.4 RabbitMQ specifics

Push consumer via `basic_consume` (not `basic_get` polling). Publisher confirms enabled. Multi-queue subscription per consumer config. Dead-letter exchange support via `FailurePolicy`.

### 9.5 NATS specifics

JetStream pub/sub, pull consumers with `filter_subject` wildcard support, request/reply via `execute_query`. Deferred manual ack.

### 9.6 Redis Streams specifics

`XREADGROUP` for consumer groups, `XAUTOCLAIM` for stale PEL recovery (messages acknowledged but never processed), `$` start position (new messages only on first connect). Operations as `DatabaseDriver`: `xadd`, `xread`, `xlen`, `ping`.

### 9.7 InfluxDB specifics

Flux query language (primary), InfluxQL also supported via `influxql` operation. Line protocol writes. `write_batch` operation buffers writes in-memory and flushes at `flush_interval_ms` or when `max_size` reached. Annotated CSV response parsing.

---

## 10. Broker Consumer Bridge

Defined in `crates/riversd/src/broker_bridge.rs`.

### 10.1 Purpose

<!-- SHAPE-18 amendment: StorageEngine buffering removed; bridge goes broker → EventBus directly -->
The `BrokerConsumerBridge` runs one async task per configured broker consumer datasource. It pulls from the broker, publishes to the EventBus, and handles failure policy when processing fails.

### 10.2 Message flow

```
Broker (Kafka/RabbitMQ/NATS/Redis Streams)
    │  InboundMessage
    ▼
BrokerConsumerBridge
    │
    ├─ 1. EventBus.publish(BrokerMessage event)
    │
    ├─ 2. broker.ack(receipt)  [AckMode::Auto or explicit]
    │
    └─ on failure:
        ├─ FailureMode::DeadLetter → producer.publish to dead-letter datasource
        ├─ FailureMode::Redirect   → producer.publish to redirect topic
        ├─ FailureMode::Requeue    → broker.nack(receipt)
        └─ FailureMode::Drop       → discard, log warning
```

### 10.3 Reconnection

The bridge runs a reconnection loop. On `BrokerConsumer::receive()` error, it waits `reconnect_ms`, publishes a `DatasourceReconnected` or `DatasourceConnectionFailed` event, and retries. The loop continues until shutdown signal.

### 10.4 Consumer lag detection

```rust
bridge.with_consumer_lag_threshold(threshold: usize)
```

`messages_pending` counter tracks inflight messages (increment on receive, decrement on ack). When `messages_pending >= threshold`, a `ConsumerLagDetected` event is published to the EventBus.

### 10.5 Drain on shutdown

```rust
bridge.with_drain_timeout_ms(ms: u64)
```

On shutdown signal, the bridge stops accepting new messages. Inflight messages complete processing. After drain timeout, the bridge closes the consumer.

---

## 11. Datasource Event Handlers

Configured per datasource. Handlers are observers — fire-and-forget CodeComponent modules invoked when lifecycle events fire. They cannot modify driver behavior, override retry logic, or inject into the request pipeline.

### 11.1 Subscribed events

| Event | Trigger |
|---|---|
| `DatasourceConnectionFailed` | Driver `connect()` error |
| `DatasourceReconnected` | Bridge reconnects after failure |
| `DatasourceCircuitClosed` | Circuit breaker returns to CLOSED |
| `PoolExhausted` | All connections checked out, acquire timeout |
| `ConsumerLagDetected` | Pending message count exceeds threshold |
| `PartitionRebalanced` | Kafka rebalance callback (deferred — rskafka 0.6 limitation) |
| `MessageFailed` | Failure policy triggered |

### 11.2 Handler execution

All datasource event handlers run in the Observe priority tier of the EventBus — fire-and-forget, never awaited, errors logged and emitted as `HandlerExecutionFailed` internal events.

---

## 12. Configuration Reference

### 12.1 Datasource config

```toml
[data.datasources.orders_db]
driver   = "postgres"
host     = "db.internal"
port     = 5432
database = "orders"
username = "app"

# Lockbox-resolved at startup — driver never sees the URI
credentials_source = "lockbox://postgres/orders-prod"

[data.datasources.orders_db.connection_pool]
max_size                  = 20
min_idle                  = 2
connection_timeout_ms     = 500
idle_timeout_ms           = 30000
max_lifetime_ms           = 300000
health_check_interval_ms  = 5000

[data.datasources.orders_db.connection_pool.circuit_breaker]
enabled              = true
failure_threshold    = 5
window_ms            = 60000    # <!-- SHAPE-1 amendment: rolling window -->
open_timeout_ms      = 10000
half_open_max_trials = 2

# Datasource lifecycle event handlers (observers)
[data.datasources.orders_db.event_handlers]
on_connection_failed = [
    { module = "handlers/ops.ts", entrypoint = "notifyOnCall" }
]
on_pool_exhausted = [
    { module = "handlers/ops.ts", entrypoint = "logPoolExhaustion" }
]
```

### 12.2 Broker datasource config

```toml
[data.datasources.orders_kafka]
driver = "kafka"
host   = "kafka.internal"
port   = 9092
credentials_source = "lockbox://kafka/prod"

[data.datasources.orders_kafka.consumer]
group_prefix = "rivers"
app_id       = "order-service"
reconnect_ms = 5000

[[data.datasources.orders_kafka.consumer.subscriptions]]
topic       = "orders"
event_name  = "order.created"   # becomes EventBus event name
ack_mode    = "auto"            # auto | manual
max_retries = 3

[data.datasources.orders_kafka.consumer.subscriptions.on_failure]
mode        = "dead_letter"
destination = "orders_dlq"
```

### 12.3 DataView config

```toml
[data.dataviews.get_order]
datasource    = "orders_db"
query         = "SELECT * FROM orders WHERE id = $1"
return_schema = "Order"
validate_result    = true
strict_parameters  = true

[[data.dataviews.get_order.parameters]]
name      = "id"
type      = "integer"
required  = true

[data.dataviews.get_order.caching]
ttl_seconds        = 60
l1_enabled         = true
l1_max_entries     = 500
l2_enabled         = true
l2_max_value_bytes = 131072  # 128 KB
```

### 12.4 StorageEngine config

```toml
[base.storage_engine]
backend          = "sqlite"             # sqlite | redis | memory
path             = "/var/rivers/rivers.db"
retention_ms     = 30000
max_events       = 100000
sweep_interval_s = 60

[environment_overrides.prod.storage_engine]
backend            = "redis"
url                = "redis://redis.internal:6379"
credentials_source = "lockbox://redis/prod"
key_prefix         = "rivers:"
pool_size          = 20
```

### 12.5 Write batch config (InfluxDB)

```toml
[data.datasources.metrics]
driver = "influxdb"
host   = "influx.internal"
port   = 8086
credentials_source = "lockbox://influx/prod"

[data.datasources.metrics.extra]
org      = "my-org"
language = "flux"

[data.datasources.metrics.write_batch]
enabled           = true
max_size          = 5000
flush_interval_ms = 1000
```

---

## 13. Single-Statement Enforcement & Transactions

> **Spec:** `docs/arch/rivers-transaction-multi-query-spec.md`

### 13.1 Single-statement rule (§2)

Each DataView query field (`query`, `get_query`, `post_query`, `put_query`, `delete_query`) **must contain exactly one SQL statement**. A semicolon (`;`) that is not inside a string literal or SQL comment is a validation error (C010) at both Gate 1 (`riverpackage validate`) and Gate 2 (riversd bundle load).

Correct message format: `DataView '{name}' field '{field}' contains multiple statements (semicolon detected). Use a handler with Rivers.db.tx for multi-query operations.`

Semicolons inside string literals (`'foo;bar'`) and SQL comments (`-- note;` or `/* note; */`) do **not** trigger the error.

### 13.2 DataView `transaction = true` (§3)

```toml
[data.dataviews.critical_update]
datasource  = "primary_db"
transaction = true
post_query  = "UPDATE accounts SET balance = balance - $amount WHERE id = $id"
```

When `transaction = true`, the DataViewEngine wraps the single query in BEGIN/COMMIT automatically. On error, ROLLBACK is sent. This flag is ignored when the DataView is called via `tx.query()` — the handler's transaction governs (TF-2).

Setting `transaction = true` on a datasource whose driver returns `supports_transactions() = false` emits a **warning** (W008), not an error.

### 13.3 Handler transaction API — `Rivers.db.tx` (§4–8)

For multi-query atomic operations, use the synchronous `Rivers.db.tx` API in handler code. All methods are sync — no `await`.

```typescript
const tx = Rivers.db.tx.begin("datasource_name");

try {
    tx.query("archive_wip",       { goal_id });
    tx.query("clear_wip",         { goal_id, project_id });
    tx.query("mark_goal_complete", { goal_id, project_id });
    tx.query("get_goal",           { goal_id });

    const results = tx.commit();
    return { status: 200, body: results["get_goal"][0].rows[0] };
} catch (e) {
    // Auto-rollback already fired on any tx.query() or tx.commit() failure.
    return { status: 500, body: { error: e.message } };
}
```

**`tx.commit()` return shape:**
```typescript
// HashMap<dataview_name, Array<QueryResult>>
results["get_goal"][0].rows         // Array of row objects
results["archive_wip"][0].affected_rows  // Number of rows affected
```

**`tx.peek(name)`** — read accumulated results mid-transaction for conditional logic:
```typescript
tx.query("check_inventory", { product_id });
const inv = tx.peek("check_inventory");
if (inv[0].rows[0].quantity < qty) {
    tx.rollback();
    return { status: 422, body: { error: "insufficient inventory" } };
}
tx.query("decrement_inventory", { product_id, qty });
const results = tx.commit();
```

**Rules:**
- One transaction at a time per handler. Nested `tx.begin()` throws.
- All DataViews called via `tx.query()` must use the same datasource as `tx.begin()`.
- If the handler exits without calling `commit()` or `rollback()`, auto-rollback fires and a WARN is logged.
- `tx.query()` uses the default `query` field (no HTTP method context inside a transaction).

### 13.4 Driver transaction support matrix (§10.3)

| Driver | `supports_transactions()` |
|---|---|
| PostgreSQL | `true` |
| MySQL | `true` |
| SQLite | `true` |
| MongoDB | `true` (v4.0+ multi-document) |
| Redis, Elasticsearch, CouchDB, Cassandra, Kafka, LDAP, Faker, HTTP, Filesystem, Exec | `false` |
