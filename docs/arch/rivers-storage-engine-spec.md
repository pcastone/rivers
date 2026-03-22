# Rivers StorageEngine Specification

**Document Type:** Implementation Specification
**Scope:** StorageEngine trait, backends, internal consumers, DataView L1/L2 caching
**Status:** Reference / Ground Truth
**Source audit:** `crates/rivers-core/src/storage.rs`, `crates/rivers-data/src/tiered_cache.rs`, `crates/riversd/src/sqlite_storage.rs`, `crates/riversd/src/redis_storage.rs`

---

## Table of Contents

1. [Purpose and Boundary](#1-purpose-and-boundary)
2. [StorageEngine Trait](#2-storageengine-trait)
3. [Backends](#3-backends)
4. [Internal Consumers](#4-internal-consumers)
5. [DataView Cache — L1/L2](#5-dataview-cache--l1l2)
6. [Lifecycle and Maintenance](#6-lifecycle-and-maintenance)
7. [Configuration Reference](#7-configuration-reference)

---

## 1. Purpose and Boundary

`StorageEngine` is Rivers internal infrastructure. It is not a datasource. Application code never accesses it directly. It does not appear in `data.dataviews` config. It has no HTTP endpoint. It has no driver registration.

<!-- SHAPE-18 amendment: StorageEngine is pure KV; queue operations removed -->
Its sole function is:
- **KV store** — scoped key-value with optional TTL. Used for L2 DataView cache, session storage, poll state, CSRF tokens, and Raft log persistence.

The StorageEngine must tolerate data loss on restart in the InMemory backend. SQLite and Redis backends provide durability appropriate for their use cases — SQLite for single-node, Redis for cluster.

---

## 2. StorageEngine Trait

Defined in `crates/rivers-core/src/storage.rs`.

```rust
#[async_trait]
pub trait StorageEngine: Send + Sync {
    // ---------- KV operations ----------
    async fn get(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<Bytes>, StorageError>;

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<(), StorageError>;

    async fn delete(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<(), StorageError>;

    async fn list_keys(
        &self,
        namespace: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<String>, StorageError>;

    // ---------- Maintenance ----------
    // SHAPE-18: queue operations (enqueue/dequeue/ack) removed — StorageEngine is pure KV
    async fn flush_expired(&self) -> Result<u64, StorageError>;
}
```

### 2.1 Key semantics

`namespace` + `key` is the compound primary key for KV operations. Namespaces provide isolation between consumers — `RaftLogStore` writes to `raft:*`, cache writes to `cache:*`. Consumers must use their designated namespace and must not read from other namespaces.

### 2.2 Reserved key prefixes

The following key prefixes are reserved for Rivers core use. No CodeComponent handler may read or write keys with these prefixes. Enforcement is at the host-side token resolution layer — if a handler constructs a key with a reserved prefix via the `Rivers.store` API, the host returns `CapabilityError` without touching the store.

| Prefix | Owner | Handler access |
|---|---|---|
| `session:` | Rivers core — session store | None |
| `csrf:` | Rivers core — CSRF token store | None |
| `poll:` | Rivers core — poll loop state | None |
| `rivers:` | Rivers core — internal use (includes sentinel keys) | None |
| All other keys | Application | Read/write per declared capability |

The restriction is not configurable. Applications must not use these prefixes for their own data.

`list_keys` returns keys within the namespace matching the optional prefix. Keys are returned without the namespace component.

### 2.3 StorageError

```rust
pub enum StorageError {
    NotFound { namespace: String, key: String },
    Serialization(String),
    Backend(String),
    Capacity(String),
    Unavailable(String),
}
```

`Unavailable` is returned when the backend is temporarily unreachable (Redis connection lost). Callers must handle this without crashing.

---

## 3. Backends

### 3.1 InMemoryStorageEngine

```rust
// SHAPE-18: queue and pending fields removed
pub struct InMemoryStorageEngine {
    kv: Arc<Mutex<HashMap<(String, String), KvEntry>>>,
}
```

No persistence. Data lost on restart. Intended for unit tests and development.

TTL is enforced lazily — `get` checks expiry on each call. `flush_expired` sweeps the entire KV store. No background sweep task for InMemory.

### 3.2 SqliteStorageEngine

Client: `sqlx` with SQLite feature. Bundled SQLite in WAL mode.

Schema:

```sql
CREATE TABLE kv_store (
    namespace   TEXT    NOT NULL,
    key         TEXT    NOT NULL,
    value       BLOB    NOT NULL,
    expires_at  INTEGER,           -- unix ms, NULL = no expiry
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (namespace, key)
);

-- SHAPE-18: queue_store table removed
CREATE INDEX idx_kv_expires  ON kv_store(expires_at)  WHERE expires_at IS NOT NULL;
```

**KV**: `set` does `INSERT OR REPLACE`. `get` checks `expires_at < unixtime_ms()` inline. `list_keys` uses `LIKE 'prefix%'`. `flush_expired` deletes KV entries where `expires_at < now()`.

**Concurrency**: `sqlx::Pool` with `max_connections = 1` for SQLite (serialized writes). Reads are concurrent via WAL mode.

### 3.3 RedisStorageEngine

Client: `redis` crate with async support.

**KV**: `set` uses `SET namespace:key value EX ttl_seconds` (or `SET namespace:key value` with no EX if `ttl_ms = None`). `get` uses `GET`. `delete` uses `DEL`. `list_keys` uses `SCAN` with pattern `namespace:prefix*` — avoids `KEYS` for production safety. `flush_expired` is a no-op for KV — Redis handles TTL-based expiry natively.

<!-- SHAPE-18: Redis Streams queue operations removed -->

**Key prefix**: All Redis keys are prefixed with `key_prefix` from config (default: `"rivers:"`).

<!-- SHAPE-8 amendment: sentinel key for single-node enforcement -->
### 3.4 Sentinel Key — Single-Node Enforcement

On startup, when the Redis backend is configured and RPS is not available, Rivers enforces single-node operation via a sentinel key:

1. Check for existing `rivers:node:*` keys via `SCAN`
2. If found: hard failure with message `"Another Rivers node detected on this Redis instance. Multi-node requires RPS."`
3. Write `rivers:node:{node_id}` with a TTL heartbeat (default: 30 seconds)
4. Background task refreshes the TTL at half the interval (every 15 seconds)
5. Key expires naturally on crash or shutdown — no cleanup required

The sentinel key format is `rivers:node:{node_id}` where `node_id` is the node's unique identifier (UUID generated at startup or configured). This mechanism prevents accidental multi-node deployments that would cause split-brain issues without a coordination layer (RPS).

---

## 4. Internal Consumers

These are the only systems that interact with the StorageEngine. Each has its own namespace conventions.

<!-- SHAPE-18 amendment: BrokerConsumerBridge and WriteBatchConnectionProvider no longer use StorageEngine queue operations -->

### 4.1 BrokerConsumerBridge

**Use**: Passes messages from broker to EventBus directly. No StorageEngine buffering.

**Operation**: Broker receive -> EventBus.publish(event). The broker's own ack/redelivery mechanism provides at-least-once semantics. StorageEngine is not involved in this path.

### 4.2 WriteBatchConnectionProvider (InfluxDB)

**Use**: Client-side write batching for InfluxDB line protocol writes.

**Operation**: Uses an in-process buffer (`VecDeque` or equivalent). Background `spawn_flush_task` flushes when `max_size` reached or `flush_interval_ms` elapses. On shutdown, drain loop processes remaining buffered writes before exiting. StorageEngine is not involved — batch state is ephemeral and acceptable to lose on crash.

### 4.3 RaftLogStore

**Use**: Persistence for Raft consensus log and vote state.

**Namespace**: `raft:{node_id}`

**KV keys**:
- `raft:{node_id}:vote` — current voted-for node ID
- `raft:{node_id}:last_purged` — last purged log index
- `raft:{node_id}:log:{index}` — individual log entries

**Restore**: On startup, `RaftLogStore::new()` calls `list_keys("raft:{node_id}", Some("log:"))` to restore log entries, then reads `vote` and `last_purged`.

### 4.4 TieredDataViewCache (L2)

**Use**: Shared L2 cache for DataView query results across nodes.

**Namespace**: `cache`

<!-- SHAPE-3 amendment: cache key uses SHA-256 of canonical JSON -->
**Key format**: `cache:views:{view_name}:{parameter_hash}`

`parameter_hash` is derived using canonical JSON key derivation: parameters are collected into a `BTreeMap<String, serde_json::Value>`, serialized via `serde_json::to_string()`, then SHA-256 hashed and hex-encoded. See `appendix-canonical-json-key-derivation.md` for the shared algorithm.

**TTL**: Set from `DataViewCachingPolicy.ttl_seconds * 1000` as `ttl_ms`.

**Size gate**: Results exceeding `l2_max_value_bytes` are not stored in L2. They are still stored in L1.

---

## 5. DataView Cache — L1/L2

Defined in `crates/rivers-data/src/tiered_cache.rs`.

### 5.1 Two-tier model

```
DataView execute
    |
    +- L1: TieredDataViewCache.l1 (LRU, in-process, per-node)
    |       cache hit -> return immediately, no L2 access
    |
    +- L2: StorageEngine.get(namespace="cache", key=cache_key)
    |       L1 miss + L2 hit -> warm L1, return result
    |
    +- Driver execute
            result -> populate L2, then L1
```

### 5.2 L1 — LruDataViewCache

```rust
pub struct LruDataViewCache {
    cache: Mutex<LruCache<CacheKey, CachedResult>>,
    max_entries: usize,
    ttl_ms: u64,
}
```

`CacheKey` = `(view_name: String, parameters: BTreeMap<String, QueryValue>)`. Parameters are stored in a `BTreeMap` for stable ordering.

L1 eviction: LRU eviction when `max_entries` is reached. TTL checked on each `get` — expired entries are evicted on access (lazy expiry).

### 5.3 L2 — StorageEngine-backed

L2 uses the `StorageEngine` KV interface. Results are serialized with `serde_json` before storage (as `Bytes`). Deserialization errors on L2 read are logged and treated as cache misses — they do not fail the request.

L2 is optional per view — if `l2_enabled = false` or no `StorageEngine` is configured, L2 is skipped entirely.

### 5.4 Cache key

<!-- SHAPE-3 amendment: SHA-256 of canonical JSON, not FNV-1a -->
Cache keys use the canonical JSON key derivation algorithm:

1. Parameters into `BTreeMap<String, serde_json::Value>` (deterministic key ordering)
2. `serde_json::to_string()` (canonical serialization)
3. SHA-256 hash, hex-encoded

Key format: `cache:views:{view_name}:{hex_sha256}`

Stable across nodes. `BTreeMap` iteration order is alphabetical — parameter ordering in config has no effect on cache sharing.

See `appendix-canonical-json-key-derivation.md` for the shared algorithm used by cache keys, polling state keys, and L2 storage keys.

### 5.5 Cache invalidation

`CacheInvalidation` EventBus events trigger cache clearing. Two invalidation scopes:

- **View-scoped**: `{ "view_name": "get_product" }` — clears all L1 entries for that view, and issues `StorageEngine.list_keys("cache", Some("views:get_product:"))` + delete for L2
- **Full**: `{}` or `{ "view_name": null }` — clears entire L1 cache, and issues `StorageEngine.list_keys("cache", None)` + delete for L2

L2 invalidation via `list_keys` + delete is eventually consistent on Redis (SCAN is non-blocking but not instantaneous). L1 invalidation is synchronous.

### 5.6 DataViewCachingPolicy

```rust
pub struct DataViewCachingPolicy {
    pub ttl_seconds: u64,
    pub l1_enabled: bool,           // default: true
    pub l1_max_entries: usize,      // default: 1000
    pub l2_enabled: bool,           // default: false
    pub l2_max_value_bytes: usize,  // default: 524288 (512 KB)
}
```

### 5.7 Cache bypass

`DataViewRequest.cache_bypass = true` skips both L1 and L2 read. Result is still written to cache after execution. Bypass is set by views that need real-time data (e.g., views driving write operations where stale data would be incorrect).

---

## 6. Lifecycle and Maintenance

### 6.1 Sweep task

A background task calls `StorageEngine::flush_expired()` at `sweep_interval_s` intervals.

<!-- SHAPE-18: sweep only covers KV, no queue -->
```
flush_expired() removes:
- KV entries where expires_at < now()
```

The sweep interval is configurable. Default: 60 seconds. On SQLite, this runs as a single `DELETE` statement. On Redis, key expiry is handled natively by Redis TTL — `flush_expired` is a no-op.

### 6.2 Bounded growth

Without sweep, KV entries (expired but not yet accessed) grow unboundedly on SQLite. The sweep task is mandatory for production SQLite deployments. Redis handles expiry natively.

### 6.3 Startup restore

On server restart with SQLite backend:
- `RaftLogStore` restores log from `kv_store`

InMemory backend provides no restore — data is lost on restart. This is acceptable for development.

---

## 7. Configuration Reference

```toml
[base.storage_engine]
backend          = "sqlite"             # sqlite | redis | memory
path             = "/var/rivers/rivers.db"  # SQLite only
sweep_interval_s = 60                  # flush_expired interval

[environment_overrides.prod.storage_engine]
backend            = "redis"
url                = "redis://redis.internal:6379"
credentials_source = "lockbox://redis/prod"
key_prefix         = "rivers:"         # all Redis keys prefixed with this
pool_size          = 20                # Redis connection pool size
```

### 7.1 Backend selection guidance

| Deployment | Recommended backend | Reason |
|---|---|---|
| Development / local | `memory` | Zero deps, fast |
| Single-node production | `sqlite` | Durable, zero deps |
| Multi-node cluster | `redis` | Shared across nodes, required for L2 cache sharing |

### 7.2 What breaks without StorageEngine

| Feature | Degradation without StorageEngine |
|---|---|
| DataView L2 cache | L2 skipped, L1 still works |
| Session storage | Sessions unavailable — guard views cannot create sessions |
| Poll state persistence | Poll loops restart from scratch on reconnect |
| Raft log persistence | Cluster cannot survive restart (in-memory Raft state lost) |

For cluster deployments, Redis StorageEngine is not optional — it is required for Raft log persistence and L2 cache sharing.

---

## Shaping Amendments

The following changes were applied to this spec per decisions in `rivers-shaping-and-gap-analysis.md`:

### SHAPE-3: SHA-256 Cache Keys

- **S4.4** — Cache key `parameter_hash` uses SHA-256 (not FNV-1a). Key format: `cache:views:{view_name}:{hex_sha256}`
- **S5.4** — Cache key derivation algorithm documented inline and references `appendix-canonical-json-key-derivation.md`

### SHAPE-8: Sentinel Key for Single-Node Enforcement

- **S3.4** (new) — Redis backend writes `rivers:node:{node_id}` sentinel key with TTL heartbeat on startup. If an existing sentinel key is found, startup fails with a hard error requiring RPS for multi-node.

### SHAPE-18: Pure KV — Queue Operations Removed

- **S1** — Overview rewritten: StorageEngine is pure KV, no queue
- **S2** — `enqueue()`, `dequeue()`, `ack()` removed from trait definition; `StoredMessage` struct removed
- **S3.1** — InMemoryStorageEngine `queues` and `pending` fields removed
- **S3.2** — `queue_store` table and `idx_queue_topic` index removed from SQLite schema
- **S3.3** — Redis Streams queue operations (`XADD`/`XREADGROUP`/`XACK`) removed
- **S4.1** — BrokerConsumerBridge rewritten: goes broker to EventBus directly, no StorageEngine buffering
- **S4.2** — WriteBatchConnectionProvider rewritten: uses in-process buffer, not StorageEngine queue
- **S6** — Sweep task updated: only KV expiry, no queue cleanup
- **S7** — Config reference updated: `retention_ms` and `max_events` (queue-related) removed
