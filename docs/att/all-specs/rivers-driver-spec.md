# Rivers Driver Specification

**Document Type:** Implementation Specification  
**Scope:** DatabaseDriver contract, five-op model, Redis first-class, rps-client driver, plugin system  
**Status:** Reference / Ground Truth  
**Source audit:** `crates/rivers-driver-sdk/src/lib.rs`, `crates/riversd/src/drivers.rs`

---

## Table of Contents

1. [Driver Model](#1-driver-model)
2. [The Five-Op Contract](#2-the-five-op-contract)
3. [Built-in Database Drivers](#3-built-in-database-drivers)
4. [Redis — First-Class Driver](#4-redis--first-class-driver)
5. [rps-client Driver](#5-rps-client-driver)
6. [MessageBrokerDriver Contract](#6-messagebrokerdrivercontract)
7. [Plugin System](#7-plugin-system)
8. [DriverFactory](#8-driverfactory)
9. [Driver Registration and Discovery](#9-driver-registration-and-discovery)
10. [Configuration Reference](#10-configuration-reference)

---

## 1. Driver Model

Every datasource in Rivers is backed by a driver. A driver is a named, stateless factory that creates `Connection` instances. Connections are owned by the pool. Drivers never manage connection lifecycle — they only construct connections on demand.

```
DriverFactory (registry)
    │
    ├─ DatabaseDriver (name → factory)
    │       └─ Connection (pool-owned, execute operations)
    │
    └─ MessageBrokerDriver (name → factory)
            ├─ BrokerProducer (publish)
            └─ BrokerConsumer (receive/ack)
```

Two separate registries exist:
- `DriverFactory::drivers` — `DatabaseDriver` implementations
- `DriverFactory::broker_drivers` — `MessageBrokerDriver` implementations

A single crate can register in both. Which registry activates for a datasource is determined by config — specifically whether the datasource has a `[consumer]` block.

---

## 2. The Five-Op Contract

The `DatabaseDriver` + `Connection` contract is normalized around five fundamental operations. Every driver maps its native API onto these five regardless of the underlying system.

| Op | Semantic | Example native calls |
|---|---|---|
| `query` | Read data — returns rows | SELECT, GET, find, search, XREAD |
| `execute` | Write data — returns affected count | INSERT, UPDATE, DELETE, set, index, XADD |
| `ping` | Health check — returns empty or error | PING, SELECT 0, test connection |
| `begin` | Start transaction scope | BEGIN, START TRANSACTION |
| `stream` | Continuous read — returns channel/cursor | SUBSCRIBE, XREADGROUP, consume |

In practice, ops 4 and 5 are expressed differently per driver:

- **Transactions** — drivers that support them set `supports_transactions() → true`. Transaction state is managed via `Query { operation: "begin" | "commit" | "rollback" }`.
- **Streaming** — broker drivers express this through `BrokerConsumer::receive()`, not through `DatabaseDriver`. The two traits are kept separate precisely because streaming is a continuous lifecycle, not a discrete operation.

The `Query` struct carries the op:

```rust
pub struct Query {
    pub operation: String,   // "select" | "insert" | "get" | "set" | "ping" | ...
    pub target: String,      // table / collection / stream / key
    pub parameters: HashMap<String, QueryValue>,
    pub statement: String,   // raw native statement
}
```

Operation is inferred from the first whitespace-delimited token of `statement` (lowercased) when not set explicitly by the caller. Drivers dispatch to their native call based on `operation`. Unknown operations should return `DriverError::Unsupported`.

### Normalization contract

All results normalize to `QueryResult`:

```rust
pub struct QueryResult {
    pub rows: Vec<HashMap<String, QueryValue>>,
    pub affected_rows: u64,
    pub last_insert_id: Option<String>,
}
```

Write operations that produce no rows still return `QueryResult` with `rows = []` and `affected_rows` set. Read operations set `affected_rows = rows.len()` by convention. Drivers must never return `rows = None` — use empty vec.

### Error contract

Driver errors must be one of:

```rust
pub enum DriverError {
    UnknownDriver(String),   // DriverFactory lookup miss
    Connection(String),      // connect() failure
    Query(String),           // execute() failure — query error
    Transaction(String),     // begin/commit/rollback failure
    Unsupported(String),     // operation not implemented for this driver
    Internal(String),        // driver-internal error, unexpected state
}
```

Drivers must not panic. All errors must be returned as `DriverError`. Error messages must not contain credential material — password, token, secret strings should be redacted at the driver level before constructing the error string.

---

## 3. Built-in Database Drivers

Registered directly in `DriverFactory::new()`. No plugin loading required.

### 3.1 `faker` (test only)

Internal driver for unit tests. Returns configurable mock results. Not exposed in production builds.

### 3.2 `postgres`

Client: `tokio-postgres`.

- Positional `$1`, `$2` parameter binding
- `supports_transactions() → true`
- `supports_prepared_statements() → true`
- `last_insert_id` via `RETURNING id` — if the result set contains an `id` column, it is extracted as `last_insert_id`
- Connection string assembled from `ConnectionParams` fields

### 3.3 `mysql`

Client: `mysql_async`.

- Positional `?` parameter binding
- `supports_transactions() → true`
- `last_insert_id` from `last_insert_id()` on result

### 3.4 `sqlite`

Client: `rusqlite` (bundled SQLite).

- WAL mode, 5s busy timeout
- Named parameters: `:name`, `@name`, `$name` — auto-prefixed with `:` if no prefix
- Supports `:memory:` via `database = ":memory:"`
- `last_insert_id` from `last_insert_rowid()`
- Type mapping:

| SQLite affinity | QueryValue |
|---|---|
| INTEGER | `Integer(i64)` |
| REAL | `Float(f64)` |
| TEXT (valid JSON) | `Json(Value)` |
| TEXT | `String(String)` |
| BLOB | `String` (UTF-8 attempt, else hex) |
| NULL | `Null` |

### 3.5 `memcached`

Client: `async-memcached` (ASCII protocol).

Operations: `get`, `set`, `delete`, `ping`.

---

## 4. Redis — First-Class Driver

Redis is a first-class built-in driver, not a plugin. It exposes a broader operation set than most drivers because Redis commands map cleanly to the five-op model across multiple data structures.

### 4.1 Operation map

| `query.operation` | Redis command | Returns |
|---|---|---|
| `get` | `GET key` | Single row: `{value}` |
| `mget` | `MGET key [key...]` | One row per key: `{key, value}` |
| `hget` | `HGET key field` | Single row: `{field, value}` |
| `hgetall` | `HGETALL key` | One row per field: `{field, value}` |
| `lrange` | `LRANGE key start stop` | One row per element: `{index, value}` |
| `smembers` | `SMEMBERS key` | One row per member: `{member}` |
| `ping` | `PING` | Empty result |
| `set` | `SET key value [EX seconds]` | `affected_rows = 1` |
| `setex` | `SET key value EX seconds` | `affected_rows = 1` |
| `del` | `DEL key [key...]` | `affected_rows = count deleted` |
| `expire` | `EXPIRE key seconds` | `affected_rows = 1 or 0` |
| `lpush` | `LPUSH key value` | `affected_rows = new length` |
| `rpush` | `RPUSH key value` | `affected_rows = new length` |
| `rpop` | `RPOP key` | Single row: `{value}` |
| `lpop` | `LPOP key` | Single row: `{value}` |
| `hset` | `HSET key field value` | `affected_rows = 1` |
| `hdel` | `HDEL key field [field...]` | `affected_rows = count deleted` |
| `incr` | `INCR key` | Single row: `{value}` |
| `incrby` | `INCRBY key increment` | Single row: `{value}` |

Parameters are extracted from `query.parameters` by name: `key`, `field`, `value`, `start`, `stop`, `seconds`, `increment`.

### 4.2 Key namespacing

Redis keys are not namespaced by Rivers automatically. Applications should use explicit prefixes in their query statements. The `target` field in `Query` is used as the key when `statement` is empty.

### 4.3 Connection management

Redis connections use `redis::aio::Connection` with a single multiplexed connection per pool slot. Reconnection is handled by the pool circuit breaker, not the driver.

---

## 5. rps-client Driver

The `rps-client` is a built-in driver that allows application views to make authenticated requests to the Rivers Provisioning Service as a datasource. It is the application-facing complement to the RPS internal integration.

### 5.1 Purpose

Application code should not make raw HTTP calls to RPS. The `rps-client` driver provides a structured, authenticated, pool-managed datasource interface to RPS operations.

### 5.2 Operations

| `query.operation` | RPS endpoint | Description |
|---|---|---|
| `get_secret` | `POST /secret/fetch` | Fetch a named secret by lockbox URI |
| `validate_token` | `POST /auth/validate` | Validate a node certificate or session token |
| `health` | `GET /health` | RPS health probe |
| `ping` | `GET /health` | Alias for health |

### 5.3 Authentication

All requests to RPS are authenticated using the node's Trust Bundle-issued PASETO token. The token is held in the pool's connection state, not in `ConnectionParams`. On `CredentialRotated` events affecting the RPS datasource, the connection pool drains and reconnects, re-authenticating with the new token.

### 5.4 Configuration

```toml
[data.datasources.rps]
driver = "rps-client"
host   = "rps.internal"
port   = 9443
credentials_source = "lockbox://rps/node-cert"

[data.datasources.rps.connection_pool]
max_size              = 5
connection_timeout_ms = 2000
```

### 5.5 Security constraints

- `rps-client` connections are always mTLS — the driver enforces TLS and rejects plaintext connections
- Connection params must not include raw passwords — the `credentials_source` must resolve to a node certificate PEM, not a password
- All RPS responses are validated for signature before being returned as `QueryResult`
- Secret values returned from `get_secret` are wrapped in opaque handles — the raw value never appears in `QueryResult.rows`

---

## 6. MessageBrokerDriver Contract

Defined in `crates/rivers-driver-sdk/src/lib.rs`.

Broker drivers are stateless factories like database drivers. The difference is they produce `BrokerProducer` and `BrokerConsumer` instances instead of `Connection` instances. These are not pool-managed in the traditional sense — the `BrokerConsumerBridge` owns one consumer per datasource subscription.

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

Broker drivers that also implement `DatabaseDriver` register under both registries. The `DatabaseDriver` interface for brokers exposes discrete, request-scoped operations (produce single message, fetch by offset, list topics). The `MessageBrokerDriver` interface is for the continuous consumer lifecycle managed by the bridge.

---

## 7. Plugin System

### 7.1 Plugin discovery

At startup, Rivers scans `config.plugins.directory` for shared libraries (`.so`, `.dylib`, `.dll`). Each file is loaded via `libloading`. Canonical path deduplication prevents the same library from loading twice via symlinks.

### 7.2 ABI version check

Before calling `_rivers_register_driver`, Rivers checks for `_rivers_abi_version`:

```rust
type AbiVersionFn = unsafe extern "C" fn() -> u32;
```

If the returned version does not match `ABI_VERSION` in the SDK, the plugin is rejected with `PluginLoadFailed` event. The library is not unloaded (to avoid UB), but none of its drivers are registered.

### 7.3 Registration call

```rust
type RegisterFn = unsafe extern "C" fn(registrar: &mut dyn DriverRegistrar);
```

Called inside `std::panic::catch_unwind(AssertUnwindSafe(...))`. A plugin that panics during registration emits `PluginLoadFailed` with the panic message and does not bring down the server.

### 7.4 DriverRegistrar trait

```rust
pub trait DriverRegistrar {
    fn register_database_driver(&mut self, driver: Arc<dyn DatabaseDriver>);
    fn register_broker_driver(&mut self, driver: Arc<dyn MessageBrokerDriver>);
}
```

### 7.5 Plugin template

```rust
use rivers_driver_sdk::prelude::*;

pub struct MyDriver;

#[async_trait]
impl DatabaseDriver for MyDriver {
    fn name(&self) -> &str { "my-driver" }
    async fn connect(&self, params: &ConnectionParams) -> Result<Box<dyn Connection>, DriverError> {
        Ok(Box::new(MyConnection::new(params).await?))
    }
}

#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    rivers_driver_sdk::ABI_VERSION
}

#[no_mangle]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(MyDriver));
}
```

### 7.6 Honest stub pattern

Plugins that are planned but not yet implemented should register and return `DriverError::Unsupported` on all operations. This is preferred over not registering — it makes the driver visible in `GET /admin/drivers` and produces a clear error if accidentally configured.

```rust
async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
    Err(DriverError::Unsupported(
        format!("neo4j driver is not yet implemented — operation: {}", query.operation)
    ))
}
```

---

## 8. DriverFactory

`DriverFactory` is the registry used by the pool manager to create connections.

```rust
pub struct DriverFactory {
    drivers: HashMap<String, Arc<dyn DatabaseDriver>>,
    broker_drivers: HashMap<String, Arc<dyn MessageBrokerDriver>>,
}
```

Built-in drivers registered in `DriverFactory::new()`:
- `faker` — test driver
- `postgres` — PostgreSQL
- `mysql` — MySQL
- `redis` — Redis (first-class)
- `memcached` — Memcached
- `sqlite` — SQLite
- `eventbus` — EventBus driver (registered conditionally when any datasource uses `driver = "eventbus"`)
- `rps-client` — RPS client driver

Plugin drivers are merged in from the static plugin registries (`PLUGIN_DRIVERS`, `PLUGIN_BROKER_DRIVERS`) during `DriverFactory::new()`.

Plugin registries are `OnceLock<Mutex<HashMap>>` — populated during plugin loading, read during factory construction. The factory takes a snapshot; plugins loaded after factory construction are not visible to existing pool managers.

### 8.1 Driver lookup

```rust
pub async fn connect(
    &self,
    driver_name: &str,
    params: &ConnectionParams,
) -> Result<Box<dyn Connection>, DriverError>
```

Returns `DriverError::UnknownDriver` if the name is not registered. This fails the datasource at startup if the referenced driver is missing — you will not get a runtime error on first request.

---

## 9. Driver Registration and Discovery

### 9.1 Startup sequence

1. `DriverFactory::new()` — register all built-in drivers
2. `load_plugins()` — scan plugin directory, ABI check, `catch_unwind` registration call, publish `DriverRegistered` or `PluginLoadFailed` events
3. Pool manager construction — per-datasource `DriverFactory::connect()` called once to validate connectivity

### 9.2 Events

| Event | When | Payload |
|---|---|---|
| `DriverRegistered` | Successful plugin registration | `driver_name`, `source` (`builtin`/`plugin`), `plugin_path`, `load_time_ms` |
| `PluginLoadFailed` | Any error in loading | `path`, `reason` |

### 9.3 Admin discovery

```
GET /admin/drivers
```

Returns all registered driver names — built-in and plugin, database and broker — as a JSON array. Used by `riversctl doctor` to verify expected plugins are present.

---

## 10. Configuration Reference

### 10.1 Plugin configuration

```toml
[plugins]
enabled   = true
directory = "/var/rivers/plugins"
```

`enabled = false` skips all plugin loading. The directory is scanned non-recursively — plugins must be at the top level.

### 10.2 Datasource referencing a driver

```toml
[data.datasources.main_db]
driver   = "postgres"
host     = "db.internal"
port     = 5432
database = "myapp"
username = "app"
credentials_source = "lockbox://postgres/prod"
```

`driver` must match a registered driver name exactly — case-sensitive. If the driver is not registered at startup, the server fails to start with a clear error.

### 10.3 Datasource extra config

For drivers that need non-standard config (InfluxDB org, Redis key prefix, NATS stream name), use the `extra` table:

```toml
[data.datasources.metrics.extra]
org      = "my-org"
language = "flux"
bucket   = "telemetry"
```

`extra` is a `HashMap<String, toml::Value>` passed to the driver via `ConnectionParams.options` (string-serialized). Drivers access these via `params.options.get("org")`.

### 10.4 rps-client datasource

```toml
[data.datasources.rps]
driver             = "rps-client"
host               = "rps.internal"
port               = 9443
credentials_source = "lockbox://rps/node-cert"

[data.datasources.rps.connection_pool]
max_size              = 5
connection_timeout_ms = 2000
circuit_breaker.enabled           = true
circuit_breaker.failure_threshold = 3
circuit_breaker.open_timeout_ms   = 30000
```
