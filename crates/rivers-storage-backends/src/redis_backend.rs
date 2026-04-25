//! Redis-backed StorageEngine implementation.
//!
//! Uses the `redis` crate with async (tokio) support.
//! Key format: `{namespace}:{key}` (colon-separated).
//! TTL is handled natively by Redis via PEXPIRE.
//!
//! Supports both single-node and cluster modes. Cluster mode is activated
//! by providing a comma-separated list of URLs.

use async_trait::async_trait;
use redis::AsyncCommands;

use rivers_core_config::storage::{Bytes, StorageEngine, StorageError};

/// Internal connection wrapper — single-node or cluster.
enum RedisConn {
    Single(redis::aio::MultiplexedConnection),
    Cluster(redis::cluster_async::ClusterConnection),
}

/// Redis storage backend.
///
/// Keys are stored as `{key_prefix}{namespace}:{key}`. Values are raw bytes.
/// TTL uses Redis native PEXPIRE for millisecond precision.
///
/// Supports both single-node (`redis://host:port`) and cluster mode
/// (comma-separated URLs: `redis://h1:p1,redis://h2:p2,redis://h3:p3`).
pub struct RedisStorageEngine {
    /// Single or cluster connection, lazily reconnected.
    conn: tokio::sync::Mutex<Option<RedisConn>>,
    /// URLs for connection (single or multiple for cluster).
    urls: Vec<String>,
    /// Whether cluster mode is active.
    cluster: bool,
    /// Global key prefix (default: "rivers:").
    key_prefix: String,
}

impl RedisStorageEngine {
    /// Create a new Redis storage engine connected to the given URL.
    ///
    /// URL format: `redis://host:port` or `redis://host:port/db`.
    /// For cluster mode, pass comma-separated URLs:
    /// `redis://h1:p1,redis://h2:p2,redis://h3:p3`
    pub fn new(url: &str) -> Result<Self, StorageError> {
        Self::with_prefix(url, "rivers:")
    }

    /// Create with a custom key prefix.
    pub fn with_prefix(url: &str, prefix: &str) -> Result<Self, StorageError> {
        let urls: Vec<String> = url.split(',').map(|s| s.trim().to_string()).collect();
        let cluster = urls.len() > 1;

        // Validate URLs by attempting to parse them
        if cluster {
            redis::cluster::ClusterClient::new(urls.clone())
                .map_err(|e| StorageError::Backend(format!("redis cluster client: {e}")))?;
        } else {
            redis::Client::open(urls[0].as_str())
                .map_err(|e| StorageError::Backend(format!("redis connect: {e}")))?;
        }

        Ok(Self {
            conn: tokio::sync::Mutex::new(None),
            urls,
            cluster,
            key_prefix: prefix.to_string(),
        })
    }

    fn make_key(&self, namespace: &str, key: &str) -> String {
        format!("{}{namespace}:{key}", self.key_prefix)
    }

    /// Get or create a connection.
    async fn get_conn(&self) -> Result<tokio::sync::MutexGuard<'_, Option<RedisConn>>, StorageError> {
        let mut guard = self.conn.lock().await;
        if guard.is_none() {
            let conn = if self.cluster {
                let client = redis::cluster::ClusterClient::new(self.urls.clone())
                    .map_err(|e| StorageError::Backend(format!("redis cluster client: {e}")))?;
                let conn = client
                    .get_async_connection()
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis cluster connect: {e}")))?;
                RedisConn::Cluster(conn)
            } else {
                let client = redis::Client::open(self.urls[0].as_str())
                    .map_err(|e| StorageError::Backend(format!("redis connect: {e}")))?;
                let conn = client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis connection: {e}")))?;
                RedisConn::Single(conn)
            };
            *guard = Some(conn);
        }
        Ok(guard)
    }
}

#[async_trait]
impl StorageEngine for RedisStorageEngine {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Bytes>, StorageError> {
        let mut guard = self.get_conn().await?;
        let rkey = self.make_key(namespace, key);

        match guard.as_mut().unwrap() {
            RedisConn::Single(conn) => {
                let result: Option<Vec<u8>> = conn
                    .get(&rkey)
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis get: {e}")))?;
                Ok(result)
            }
            RedisConn::Cluster(conn) => {
                let result: Option<Vec<u8>> = conn
                    .get(&rkey)
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis cluster get: {e}")))?;
                Ok(result)
            }
        }
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<(), StorageError> {
        let mut guard = self.get_conn().await?;
        let rkey = self.make_key(namespace, key);

        match guard.as_mut().unwrap() {
            RedisConn::Single(conn) => {
                match ttl_ms {
                    Some(ttl) => {
                        // SET with PEXPIRE in a pipeline for atomicity
                        redis::pipe()
                            .set(&rkey, value.as_slice())
                            .pexpire(&rkey, ttl as i64)
                            .query_async::<()>(conn)
                            .await
                            .map_err(|e| StorageError::Backend(format!("redis set+pexpire: {e}")))?;
                    }
                    None => {
                        conn.set::<_, _, ()>(&rkey, value.as_slice())
                            .await
                            .map_err(|e| StorageError::Backend(format!("redis set: {e}")))?;
                    }
                }
            }
            RedisConn::Cluster(conn) => {
                match ttl_ms {
                    Some(ttl) => {
                        // Use PSETEX for atomic SET + millisecond TTL on cluster
                        // (pipelines may not work across cluster slots)
                        conn.pset_ex::<_, _, ()>(&rkey, value.as_slice(), ttl as u64)
                            .await
                            .map_err(|e| StorageError::Backend(format!("redis cluster psetex: {e}")))?;
                    }
                    None => {
                        conn.set::<_, _, ()>(&rkey, value.as_slice())
                            .await
                            .map_err(|e| StorageError::Backend(format!("redis cluster set: {e}")))?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
        let mut guard = self.get_conn().await?;
        let rkey = self.make_key(namespace, key);

        match guard.as_mut().unwrap() {
            RedisConn::Single(conn) => {
                conn.del::<_, ()>(&rkey)
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis del: {e}")))?;
            }
            RedisConn::Cluster(conn) => {
                conn.del::<_, ()>(&rkey)
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis cluster del: {e}")))?;
            }
        }

        Ok(())
    }

    async fn list_keys(
        &self,
        namespace: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<String>, StorageError> {
        let mut guard = self.get_conn().await?;
        let pattern = match prefix {
            Some(p) => format!("{}{namespace}:{p}*", self.key_prefix),
            None => format!("{}{namespace}:*", self.key_prefix),
        };
        let ns_prefix = format!("{}{namespace}:", self.key_prefix);

        match guard.as_mut().unwrap() {
            RedisConn::Single(conn) => {
                // Use SCAN to avoid blocking the server on large keyspaces
                let mut keys: Vec<String> = Vec::new();
                let mut iter: redis::AsyncIter<String> = conn
                    .scan_match(&pattern)
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis scan: {e}")))?;

                while let Some(full_key) = iter.next_item().await {
                    if let Some(bare) = full_key.strip_prefix(&ns_prefix) {
                        keys.push(bare.to_string());
                    }
                }

                Ok(keys)
            }
            RedisConn::Cluster(conn) => {
                // G_R1: cluster path now uses per-node SCAN instead of KEYS.
                // KEYS blocks the entire Redis server while it walks every key
                // — pathological on large keyspaces. SCAN is per-node and
                // cursor-based, so we fan out one SCAN per primary, then
                // continue each node's cursor independently until it returns
                // to 0. The async cluster connection's `route_command` with
                // `MultipleNodeRoutingInfo::AllMasters` and no response
                // policy returns a `Value::Map<addr, [cursor, [keys...]]>`,
                // letting us drive per-node cursors using `ByAddress` routing
                // for the follow-up calls.
                use redis::cluster_routing::{
                    MultipleNodeRoutingInfo, RoutingInfo, SingleNodeRoutingInfo,
                };

                let mut keys: Vec<String> = Vec::new();
                // First round: broadcast SCAN 0 to every primary and harvest
                // (address, cursor, keys) tuples.
                let mut first = redis::cmd("SCAN");
                first
                    .arg(0u64)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(500u64);
                let initial = conn
                    .route_command(
                        &first,
                        RoutingInfo::MultiNode((MultipleNodeRoutingInfo::AllMasters, None)),
                    )
                    .await
                    .map_err(|e| {
                        StorageError::Backend(format!("redis cluster scan (broadcast): {e}"))
                    })?;

                // Per-node continuation state: addr → next cursor (None when done).
                let mut pending: Vec<(String, u64)> = Vec::new();
                for (addr_val, payload) in extract_node_map(initial)? {
                    let addr = addr_val_to_string(addr_val).ok_or_else(|| {
                        StorageError::Backend(
                            "redis cluster scan: non-string node address in response".into(),
                        )
                    })?;
                    let (cursor, batch) = parse_scan_response(payload)?;
                    for k in batch {
                        if let Some(bare) = k.strip_prefix(&ns_prefix) {
                            keys.push(bare.to_string());
                        }
                    }
                    if cursor != 0 {
                        pending.push((addr, cursor));
                    }
                }

                // Drain remaining cursors per node. Each node is independent,
                // so we cycle through `pending` and reissue against each
                // `ByAddress` route until the cursor returns 0. A modest
                // safety bound prevents runaway loops on a misbehaving node.
                let max_iterations = 10_000;
                let mut iterations = 0;
                while !pending.is_empty() {
                    iterations += 1;
                    if iterations > max_iterations {
                        return Err(StorageError::Backend(format!(
                            "redis cluster scan: exceeded {max_iterations} iterations (node cursors did not drain)"
                        )));
                    }
                    let mut next_pending = Vec::with_capacity(pending.len());
                    for (addr, cursor) in pending.drain(..) {
                        let (host, port) = split_addr(&addr).ok_or_else(|| {
                            StorageError::Backend(format!(
                                "redis cluster scan: malformed node address '{addr}'"
                            ))
                        })?;
                        let mut cmd = redis::cmd("SCAN");
                        cmd.arg(cursor)
                            .arg("MATCH")
                            .arg(&pattern)
                            .arg("COUNT")
                            .arg(500u64);
                        let resp = conn
                            .route_command(
                                &cmd,
                                RoutingInfo::SingleNode(SingleNodeRoutingInfo::ByAddress {
                                    host,
                                    port,
                                }),
                            )
                            .await
                            .map_err(|e| {
                                StorageError::Backend(format!(
                                    "redis cluster scan (continue {addr} @ cursor {cursor}): {e}"
                                ))
                            })?;
                        let (next_cursor, batch) = parse_scan_response(resp)?;
                        for k in batch {
                            if let Some(bare) = k.strip_prefix(&ns_prefix) {
                                keys.push(bare.to_string());
                            }
                        }
                        if next_cursor != 0 {
                            next_pending.push((addr, next_cursor));
                        }
                    }
                    pending = next_pending;
                }

                Ok(keys)
            }
        }
    }

    async fn set_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<bool, StorageError> {
        let mut guard = self.get_conn().await?;
        let rkey = self.make_key(namespace, key);

        match guard.as_mut().unwrap() {
            RedisConn::Single(conn) => {
                // Use SET key value NX [PX ttl] — atomic set-if-not-exists
                let mut cmd = redis::cmd("SET");
                cmd.arg(&rkey).arg(value.as_slice()).arg("NX");
                if let Some(ttl) = ttl_ms {
                    cmd.arg("PX").arg(ttl);
                }
                let result: Option<String> = cmd
                    .query_async(conn)
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis set NX: {e}")))?;
                // SET NX returns "OK" on success, nil on failure
                Ok(result.is_some())
            }
            RedisConn::Cluster(conn) => {
                let mut cmd = redis::cmd("SET");
                cmd.arg(&rkey).arg(value.as_slice()).arg("NX");
                if let Some(ttl) = ttl_ms {
                    cmd.arg("PX").arg(ttl);
                }
                let result: Option<String> = cmd
                    .query_async(conn)
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis cluster set NX: {e}")))?;
                Ok(result.is_some())
            }
        }
    }

    async fn flush_expired(&self) -> Result<u64, StorageError> {
        // Redis handles TTL expiration natively; nothing to sweep.
        Ok(0)
    }
}

// ── G_R1: cluster SCAN helpers ──────────────────────────────────────

/// Extract the `(addr, payload)` pairs from a `Value::Map` returned by
/// `route_command(.., MultiNode(AllMasters, None))`. Returns an error for
/// any other shape.
fn extract_node_map(value: redis::Value) -> Result<Vec<(redis::Value, redis::Value)>, StorageError> {
    match value {
        redis::Value::Map(entries) => Ok(entries),
        // Single-node clusters might collapse to a single response; treat
        // that as a one-element map with an empty address.
        other => Err(StorageError::Backend(format!(
            "redis cluster scan: expected Map response from broadcast SCAN, got {other:?}"
        ))),
    }
}

/// Convert a `Value::BulkString`/`Value::SimpleString` (the address key in
/// the broadcast response Map) into a Rust string.
fn addr_val_to_string(v: redis::Value) -> Option<String> {
    match v {
        redis::Value::BulkString(bytes) => String::from_utf8(bytes).ok(),
        redis::Value::SimpleString(s) => Some(s),
        _ => None,
    }
}

/// Split a `host:port` address into its parts.
fn split_addr(addr: &str) -> Option<(String, u16)> {
    let (host, port) = addr.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    Some((host.to_string(), port))
}

/// Parse a SCAN response (`[cursor_string, [keys...]]`) into a `(cursor,
/// keys)` tuple. The cursor is decoded from its bulk-string ASCII form.
fn parse_scan_response(value: redis::Value) -> Result<(u64, Vec<String>), StorageError> {
    let arr = match value {
        redis::Value::Array(arr) => arr,
        other => {
            return Err(StorageError::Backend(format!(
                "redis cluster scan: expected Array reply, got {other:?}"
            )))
        }
    };
    if arr.len() != 2 {
        return Err(StorageError::Backend(format!(
            "redis cluster scan: expected 2-element reply, got {}",
            arr.len()
        )));
    }
    let mut it = arr.into_iter();
    let cursor_val = it.next().unwrap();
    let keys_val = it.next().unwrap();

    let cursor = match cursor_val {
        redis::Value::BulkString(bytes) => std::str::from_utf8(&bytes)
            .map_err(|e| StorageError::Backend(format!("redis cluster scan cursor utf8: {e}")))?
            .parse::<u64>()
            .map_err(|e| StorageError::Backend(format!("redis cluster scan cursor parse: {e}")))?,
        redis::Value::Int(i) => i as u64,
        other => {
            return Err(StorageError::Backend(format!(
                "redis cluster scan: unexpected cursor type {other:?}"
            )))
        }
    };

    let keys = match keys_val {
        redis::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| match v {
                redis::Value::BulkString(b) => String::from_utf8(b).ok(),
                redis::Value::SimpleString(s) => Some(s),
                _ => None,
            })
            .collect(),
        other => {
            return Err(StorageError::Backend(format!(
                "redis cluster scan: expected Array of keys, got {other:?}"
            )))
        }
    };

    Ok((cursor, keys))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_core_config::storage::StorageEngine;
    use std::time::Duration;

    const REDIS_URL: &str = "redis://127.0.0.1:6379";

    /// Create a Redis engine with a unique prefix per test to avoid collisions.
    fn new_engine(test_name: &str) -> RedisStorageEngine {
        let prefix = format!("test:{}:", test_name);
        RedisStorageEngine::with_prefix(REDIS_URL, &prefix)
            .expect("redis engine should construct")
    }

    /// Clean up all keys matching a test prefix.
    async fn cleanup(engine: &RedisStorageEngine, ns: &str) {
        let keys = engine.list_keys(ns, None).await.unwrap_or_default();
        for key in keys {
            let _ = engine.delete(ns, &key).await;
        }
    }

    #[tokio::test]
    #[ignore] // requires running Redis at 127.0.0.1:6379
    async fn get_set_round_trip() {
        let engine = new_engine("get_set_round_trip");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        let value = b"hello world".to_vec();
        engine.set(ns, "key1", value.clone(), None).await.unwrap();
        let got = engine.get(ns, "key1").await.unwrap();
        assert_eq!(got, Some(value));

        cleanup(&engine, ns).await;
    }

    #[tokio::test]
    #[ignore]
    async fn get_nonexistent_returns_none() {
        let engine = new_engine("get_nonexistent");
        let got = engine.get("test-ns", "no-such-key").await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    #[ignore]
    async fn del_removes_key() {
        let engine = new_engine("del_removes");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        engine.set(ns, "key1", b"data".to_vec(), None).await.unwrap();
        engine.delete(ns, "key1").await.unwrap();
        let got = engine.get(ns, "key1").await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    #[ignore]
    async fn del_nonexistent_is_ok() {
        let engine = new_engine("del_nonexistent");
        engine.delete("test-ns", "ghost").await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn overwrite_existing_key() {
        let engine = new_engine("overwrite");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        engine.set(ns, "key1", b"first".to_vec(), None).await.unwrap();
        engine.set(ns, "key1", b"second".to_vec(), None).await.unwrap();
        let got = engine.get(ns, "key1").await.unwrap();
        assert_eq!(got, Some(b"second".to_vec()));

        cleanup(&engine, ns).await;
    }

    #[tokio::test]
    #[ignore]
    async fn list_keys_with_prefix() {
        let engine = new_engine("list_prefix");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        engine.set(ns, "user:1", b"a".to_vec(), None).await.unwrap();
        engine.set(ns, "user:2", b"b".to_vec(), None).await.unwrap();
        engine.set(ns, "session:1", b"c".to_vec(), None).await.unwrap();

        let mut keys = engine.list_keys(ns, Some("user:")).await.unwrap();
        keys.sort();
        assert_eq!(keys, vec!["user:1", "user:2"]);

        cleanup(&engine, ns).await;
    }

    #[tokio::test]
    #[ignore]
    async fn ttl_expiration() {
        let engine = new_engine("ttl_expiration");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        engine
            .set(ns, "ephemeral", b"temp".to_vec(), Some(100))
            .await
            .unwrap();

        // Immediately should still be present.
        let got = engine.get(ns, "ephemeral").await.unwrap();
        assert_eq!(got, Some(b"temp".to_vec()));

        // Wait for Redis to expire the key.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let got = engine.get(ns, "ephemeral").await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    #[ignore]
    async fn set_empty_value() {
        let engine = new_engine("empty_value");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        engine.set(ns, "empty", vec![], None).await.unwrap();
        let got = engine.get(ns, "empty").await.unwrap();
        assert_eq!(got, Some(vec![]));

        cleanup(&engine, ns).await;
    }

    #[tokio::test]
    #[ignore]
    async fn binary_value_storage() {
        let engine = new_engine("binary_value");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        let value: Vec<u8> = (0..=255).collect();
        engine.set(ns, "bin", value.clone(), None).await.unwrap();
        let got = engine.get(ns, "bin").await.unwrap();
        assert_eq!(got, Some(value));

        cleanup(&engine, ns).await;
    }

    #[tokio::test]
    #[ignore]
    async fn namespace_isolation() {
        let engine = new_engine("ns_isolation");
        cleanup(&engine, "ns-a").await;
        cleanup(&engine, "ns-b").await;

        engine
            .set("ns-a", "key", b"from-a".to_vec(), None)
            .await
            .unwrap();
        engine
            .set("ns-b", "key", b"from-b".to_vec(), None)
            .await
            .unwrap();

        assert_eq!(
            engine.get("ns-a", "key").await.unwrap(),
            Some(b"from-a".to_vec())
        );
        assert_eq!(
            engine.get("ns-b", "key").await.unwrap(),
            Some(b"from-b".to_vec())
        );

        cleanup(&engine, "ns-a").await;
        cleanup(&engine, "ns-b").await;
    }

    #[tokio::test]
    #[ignore]
    async fn set_if_absent_inserts_when_missing() {
        let engine = new_engine("set_if_absent_insert");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        let inserted = engine
            .set_if_absent(ns, "key1", b"val".to_vec(), None)
            .await
            .unwrap();
        assert!(inserted);
        assert_eq!(
            engine.get(ns, "key1").await.unwrap(),
            Some(b"val".to_vec())
        );

        cleanup(&engine, ns).await;
    }

    #[tokio::test]
    #[ignore]
    async fn set_if_absent_does_not_overwrite() {
        let engine = new_engine("set_if_absent_no_overwrite");
        let ns = "test-ns";
        cleanup(&engine, ns).await;

        engine
            .set(ns, "key1", b"original".to_vec(), None)
            .await
            .unwrap();

        let inserted = engine
            .set_if_absent(ns, "key1", b"new".to_vec(), None)
            .await
            .unwrap();
        assert!(!inserted);
        assert_eq!(
            engine.get(ns, "key1").await.unwrap(),
            Some(b"original".to_vec())
        );

        cleanup(&engine, ns).await;
    }

    #[tokio::test]
    #[ignore]
    async fn flush_expired_is_noop() {
        let engine = new_engine("flush_noop");
        // Redis handles TTL natively, so flush_expired always returns 0.
        let removed = engine.flush_expired().await.unwrap();
        assert_eq!(removed, 0);
    }

    /// G_R1: cluster `list_keys` MUST use SCAN (not KEYS) and return every
    /// matching key across all primaries. Gated on `REDIS_CLUSTER_AVAILABLE=1`
    /// because it requires the test cluster at 192.168.2.206-208.
    #[tokio::test]
    async fn cluster_list_keys_uses_scan_for_large_keyspace() {
        if std::env::var("REDIS_CLUSTER_AVAILABLE").ok().as_deref() != Some("1") {
            println!(
                "SKIP cluster_list_keys_uses_scan_for_large_keyspace — set REDIS_CLUSTER_AVAILABLE=1 to enable (cluster at 192.168.2.206-208)"
            );
            return;
        }

        // Cluster credentials per `sec/test-infrastructure.md`.
        const CLUSTER_URL: &str = "redis://:rivers_test@192.168.2.206:6379,redis://:rivers_test@192.168.2.207:6379,redis://:rivers_test@192.168.2.208:6379";
        let prefix = format!("test:cluster_scan:{}:", uuid_like_suffix());
        let engine = RedisStorageEngine::with_prefix(CLUSTER_URL, &prefix)
            .expect("redis cluster engine should construct");
        let ns = "ns-scan";

        // Seed 250 keys (well above the per-SCAN COUNT hint of 500 to
        // exercise multi-cursor iteration without taking forever).
        for i in 0..250 {
            engine
                .set(ns, &format!("k:{i}"), b"v".to_vec(), None)
                .await
                .expect("set should succeed");
        }

        let keys = engine
            .list_keys(ns, Some("k:"))
            .await
            .expect("list_keys should succeed");
        assert_eq!(keys.len(), 250, "SCAN must surface every matching key");

        // Cleanup — same engine.delete loop as the helper above; we can't
        // call cleanup() because the prefix is unique per-test.
        for key in keys {
            let _ = engine.delete(ns, &key).await;
        }
    }

    /// Light pseudo-uuid for test isolation without pulling a uuid dep.
    fn uuid_like_suffix() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| format!("{}{}", d.as_secs(), d.subsec_nanos()))
            .unwrap_or_else(|_| "fallback".into())
    }

    #[tokio::test]
    #[ignore]
    async fn custom_prefix() {
        let engine = RedisStorageEngine::with_prefix(REDIS_URL, "custom:")
            .expect("custom prefix engine should construct");
        let ns = "pfx-test";

        // Verify the engine works with a custom prefix.
        engine.set(ns, "k", b"v".to_vec(), None).await.unwrap();
        let got = engine.get(ns, "k").await.unwrap();
        assert_eq!(got, Some(b"v".to_vec()));

        engine.delete(ns, "k").await.unwrap();
    }
}
