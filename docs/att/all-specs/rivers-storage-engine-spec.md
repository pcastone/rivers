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

Its two functions are:
- **KV store** — scoped key-value with optional TTL. Used for L2 DataView cache, Raft log persistence, and RPS state.
- **Queue** — durable-enough message queue with dequeue-and-ack semantics. Used for broker message buffering and InfluxDB write batching.

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

    // ---------- Queue operations ----------
    async fn enqueue(
        &self,
        topic: &str,
        payload: Bytes,
    ) -> Result<MessageId, StorageError>;

    async fn dequeue(
        &self,
        topic: &str,
        limit: usize,
    ) -> Result<Vec<StoredMessage>, StorageError>;

    async fn ack(
        &self,
        topic: &str,
        id: &str,
    ) -> Result<(), StorageError>;

    // ---------- Maintenance ----------
    async fn flush_expired(&self) -> Result<u64, StorageError>;
}
```

### 2.1 Key semantics

`namespace` + `key` is the compound primary key for KV operations. Namespaces provide isolation between consumers — `RaftLogStore` writes to `raft:*`, cache writes to `cache:*`. Consumers must use their designated namespace and must not read from other namespaces.

`list_keys` returns keys within the namespace matching the optional prefix. Keys are returned without the namespace component.

### 2.2 Queue semantics

`dequeue(topic, limit)` is a soft cap — returns up to `limit` messages without blocking. If fewer than `limit` messages are available, returns what's available.

Dequeued messages are invisible to subsequent `dequeue` calls — they are in a pending-ack state. They remain pending until:
- `ack(topic, id)` is called — message is permanently removed
- The message's `pending_timeout` elapses (backend-specific) — message becomes available for redelivery

`enqueue` returns a `MessageId` (String). Callers should store this ID to call `ack` after successful processing.

### 2.3 StoredMessage

```rust
pub struct StoredMessage {
    pub id: MessageId,
    pub topic: String,
    pub payload: Bytes,
    pub enqueued_at: DateTime<Utc>,
}
```

### 2.4 StorageError

```rust
pub enum StorageError {
    NotFound { namespace: String, key: String },
    Serialization(String),
    Backend(String),
    Capacity(String),
    Unavailable(String),
}
```

`Unavailable` is returned when the backend is temporarily unreachable (Redis connection lost). Callers must handle this without crashing — the BrokerConsumerBridge falls back to unbuffered delivery on `Unavailable`.

---

## 3. Backends

### 3.1 InMemoryStorageEngine

```rust
pub struct InMemoryStorageEngine {
    kv: Arc<Mutex<HashMap<(String, String), KvEntry>>>,
    queues: Arc<Mutex<HashMap<String, VecDeque<StoredMessage>>>>,
    pending: Arc<Mutex<HashMap<String, StoredMessage>>>,
}
```

No persistence. Data lost on restart. Intended for unit tests and development.

TTL is enforced lazily — `get` checks expiry on each call. `flush_expired` sweeps the entire KV store. No background sweep task for InMemory.

Queue is a `VecDeque`. `dequeue` moves entries from the queue into `pending`. `ack` removes from pending.

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

CREATE TABLE queue_store (
    id          TEXT    PRIMARY KEY,
    topic       TEXT    NOT NULL,
    payload     BLOB    NOT NULL,
    enqueued_at INTEGER NOT NULL,
    acked       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_kv_expires  ON kv_store(expires_at)  WHERE expires_at IS NOT NULL;
CREATE INDEX idx_queue_topic ON queue_store(topic)     WHERE acked = 0;
```

**KV**: `set` does `INSERT OR REPLACE`. `get` checks `expires_at < unixtime_ms()` inline. `list_keys` uses `LIKE 'prefix%'`.

**Queue**: `enqueue` inserts with `acked = 0`. `dequeue` selects `WHERE acked = 0 AND topic = ? LIMIT ?` and returns. Messages remain `acked = 0` until `ack` sets `acked = 1`. `flush_expired` deletes `WHERE acked = 1`.

**Concurrency**: `sqlx::Pool` with `max_connections = 1` for SQLite (serialized writes). Reads are concurrent via WAL mode.

### 3.3 RedisStorageEngine

Client: `redis` crate with async support.

**KV**: `set` uses `SET namespace:key value EX ttl_seconds` (or `SET namespace:key value` with no EX if `ttl_ms = None`). `get` uses `GET`. `delete` uses `DEL`. `list_keys` uses `SCAN` with pattern `namespace:prefix*` — avoids `KEYS` for production safety.

**Queue**: Uses Redis Streams internally.
- `enqueue`: `XADD topic_stream * payload <bytes>` — returns stream entry ID as `MessageId`
- `dequeue`: `XREADGROUP GROUP rivers-storage consumer BLOCK 0 COUNT limit STREAMS topic_stream >` — uses a dedicated consumer group `rivers-storage`
- `ack`: `XACK topic_stream rivers-storage id`
- `flush_expired`: `XTRIM topic_stream MAXLEN ~ retention_count` + KV key scan with TTL check

**Key prefix**: All Redis keys are prefixed with `key_prefix` from config (default: `"rivers:"`).

---

## 4. Internal Consumers

These are the only systems that interact with the StorageEngine. Each has its own namespace and topic conventions.

### 4.1 BrokerConsumerBridge

**Use**: Durable message buffer between broker receive and EventBus publish.

**Operation**:
1. `enqueue("eventbus:{event_name}", message_payload)` — before EventBus publish
2. `EventBus.publish(event)`
3. `ack("eventbus:{event_name}", id)` — after successful publish

On `StorageError::Unavailable` during enqueue: bridge logs a warning and publishes to EventBus without buffering. This is the graceful degradation path — delivery is still attempted, just without durability.

**Why**: If the server crashes after EventBus publish but before broker ack, the broker redelivers. If it crashes after enqueue but before EventBus publish, the queue entry is recovered on restart. The combination provides at-least-once delivery without requiring two-phase commit.

### 4.2 WriteBatchConnectionProvider (InfluxDB)

**Use**: Client-side write batching for InfluxDB line protocol writes.

**Namespace**: KV not used. Topic: `writebatch:{datasource_id}`

**Operation**:
1. Write operation intercepted by `BatchingPoolConnection`
2. `enqueue("writebatch:{datasource_id}", line_protocol_bytes)`
3. Background `spawn_flush_task` dequeues when `max_size` reached or `flush_interval_ms` elapses
4. Dequeued entries assembled into a single batch write
5. `ack` called after successful batch write to InfluxDB

On shutdown: drain loop processes remaining queued writes before exiting.

### 4.3 RaftLogStore

**Use**: Persistence for Raft consensus log and vote state.

**Namespace**: `raft:{node_id}`

**KV keys**:
- `raft:{node_id}:vote` — current voted-for node ID
- `raft:{node_id}:last_purged` — last purged log index
- `raft:{node_id}:log:{index}` — individual log entries

**Queue**: Not used by Raft. Log entries are stored as KV.

**Restore**: On startup, `RaftLogStore::new()` calls `list_keys("raft:{node_id}", Some("log:"))` to restore log entries, then reads `vote` and `last_purged`.

### 4.4 TieredDataViewCache (L2)

**Use**: Shared L2 cache for DataView query results across nodes.

**Namespace**: `cache`

**Key format**: `cache:views:{view_name}:{parameter_hash}`

`parameter_hash` is a stable SHA-256 of the serialized parameters map. Parameter order is normalized before hashing.

**TTL**: Set from `DataViewCachingPolicy.ttl_seconds * 1000` as `ttl_ms`.

**Size gate**: Results exceeding `l2_max_value_bytes` are not stored in L2. They are still stored in L1.

---

## 5. DataView Cache — L1/L2

Defined in `crates/rivers-data/src/tiered_cache.rs`.

### 5.1 Two-tier model

```
DataView execute
    │
    ├─ L1: TieredDataViewCache.l1 (LRU, in-process, per-node)
    │       cache hit → return immediately, no L2 access
    │
    ├─ L2: StorageEngine.get(namespace="cache", key=cache_key)
    │       L1 miss + L2 hit → warm L1, return result
    │
    └─ Driver execute
            result → populate L2, then L1
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

```
cache_key = SHA-256(view_name + ":" + sorted_params_json)
```

Stable across nodes. `BTreeMap` iteration order is alphabetical — parameter ordering in config has no effect on cache sharing.

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

```
flush_expired() removes:
- KV entries where expires_at < now()
- Queue entries where acked = 1
```

The sweep interval is configurable. Default: 60 seconds. On SQLite, this runs as a single `DELETE` statement per table. On Redis, it runs `XTRIM` on each known stream + key expiry is handled natively by Redis TTL.

### 6.2 Bounded growth

Without sweep, both KV (expired but not yet acked or accessed) and queue (acked but not deleted) grow unboundedly. The sweep task is mandatory for production deployments.

`max_events` sets a soft cap on total queue entries. When exceeded, `enqueue` returns `StorageError::Capacity`. Callers should treat `Capacity` as a signal to shed load or fail gracefully.

### 6.3 Startup restore

On server restart with SQLite backend:
- `RaftLogStore` restores log from `kv_store`
- `WriteBatchConnectionProvider` checks `queue_store` for unprocessed write batches and schedules them for the next flush cycle
- `BrokerConsumerBridge` checks `queue_store` for unacked EventBus entries and re-publishes them

InMemory backend provides no restore — data is lost on restart. This is acceptable for development; unacceptable for any production use of the queue operations.

---

## 7. Configuration Reference

```toml
[base.storage_engine]
backend          = "sqlite"             # sqlite | redis | memory
path             = "/var/rivers/rivers.db"  # SQLite only
retention_ms     = 30000               # max age of pending queue entries
max_events       = 100000              # soft cap on total queue entries
sweep_interval_s = 60                  # flush_expired interval

[environment_overrides.prod.storage_engine]
backend            = "redis"
url                = "redis://redis.internal:6379"
credentials_source = "lockbox://redis/prod"
key_prefix         = "rivers:"         # all Redis keys prefixed with this
pool_size          = 20                # Redis connection pool size
retention_ms       = 86400000          # 24 hours
max_events         = 1000000
```

### 7.1 Backend selection guidance

| Deployment | Recommended backend | Reason |
|---|---|---|
| Development / local | `memory` | Zero deps, fast |
| Single-node production | `sqlite` | Durable, zero deps |
| Multi-node cluster | `redis` | Shared across nodes, required for L2 cache sharing and distributed queue |

### 7.2 What breaks without StorageEngine

| Feature | Degradation without StorageEngine |
|---|---|
| DataView L2 cache | L2 skipped, L1 still works |
| Broker bridge buffering | Unbuffered delivery (at-most-once instead of at-least-once) |
| InfluxDB write batching | Falls back to immediate unbatched writes |
| Raft log persistence | Cluster cannot survive restart (in-memory Raft state lost) |
| RPS state | RPS cannot persist Trust Bundle rotations across restarts |

For cluster deployments, Redis StorageEngine is not optional — it is required for Raft log persistence and L2 cache sharing.
