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
