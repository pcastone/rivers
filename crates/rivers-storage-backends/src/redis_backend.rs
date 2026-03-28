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
                // SCAN is per-node and not supported on ClusterConnection.
                // Use KEYS command instead (acceptable for storage engine use case
                // where key counts are bounded).
                let vals: Vec<String> = conn
                    .keys(&pattern)
                    .await
                    .map_err(|e| StorageError::Backend(format!("redis cluster keys: {e}")))?;

                let keys: Vec<String> = vals
                    .into_iter()
                    .filter_map(|full_key| {
                        full_key
                            .strip_prefix(&ns_prefix)
                            .map(|bare| bare.to_string())
                    })
                    .collect();

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
