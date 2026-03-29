//! SQLite-backed StorageEngine implementation.
//!
//! Uses rusqlite (synchronous) wrapped with `tokio::task::spawn_blocking`.
//! WAL mode is enabled for concurrent read performance.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::Connection;

use rivers_core_config::storage::{Bytes, StorageEngine, StorageError};

/// SQLite storage backend.
///
/// Stores KV entries in a single `kv` table with composite (namespace, key) primary key.
/// TTL is stored as unix milliseconds in `expires_at` and checked on read.
pub struct SqliteStorageEngine {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStorageEngine {
    /// Open (or create) a SQLite database at `path`.
    ///
    /// Enables WAL mode and creates the `kv` table if it does not exist.
    /// Use `:memory:` for an ephemeral in-process database.
    pub fn new(path: &str) -> Result<Self, StorageError> {
        let conn = Connection::open(path)
            .map_err(|e| StorageError::Backend(format!("sqlite open: {e}")))?;

        // Enable WAL mode for better concurrent read performance
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| StorageError::Backend(format!("sqlite WAL: {e}")))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv (
                namespace  TEXT NOT NULL,
                key        TEXT NOT NULL,
                value      BLOB NOT NULL,
                expires_at INTEGER,
                created_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),
                PRIMARY KEY (namespace, key)
            );
            CREATE INDEX IF NOT EXISTS idx_kv_namespace_created
                ON kv (namespace, created_at);
            CREATE INDEX IF NOT EXISTS idx_kv_expires
                ON kv (expires_at) WHERE expires_at IS NOT NULL;",
        )
        .map_err(|e| StorageError::Backend(format!("sqlite schema: {e}")))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[async_trait]
impl StorageEngine for SqliteStorageEngine {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Bytes>, StorageError> {
        let conn = Arc::clone(&self.conn);
        let ns = namespace.to_string();
        let k = key.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| StorageError::Backend(format!("lock: {e}")))?;
            let now = now_ms();

            let mut stmt = conn
                .prepare("SELECT value, expires_at FROM kv WHERE namespace = ?1 AND key = ?2")
                .map_err(|e| StorageError::Backend(format!("sqlite prepare: {e}")))?;

            let result = stmt
                .query_row(rusqlite::params![ns, k], |row| {
                    let value: Vec<u8> = row.get(0)?;
                    let expires_at: Option<u64> = row.get(1)?;
                    Ok((value, expires_at))
                });

            match result {
                Ok((value, expires_at)) => {
                    if let Some(exp) = expires_at {
                        if now >= exp {
                            // Lazy delete of expired entry
                            conn.execute(
                                "DELETE FROM kv WHERE namespace = ?1 AND key = ?2",
                                rusqlite::params![ns, k],
                            )
                            .map_err(|e| StorageError::Backend(format!("sqlite delete expired: {e}")))?;
                            return Ok(None);
                        }
                    }
                    Ok(Some(value))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(StorageError::Backend(format!("sqlite get: {e}"))),
            }
        })
        .await
        .map_err(|e| StorageError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<(), StorageError> {
        let conn = Arc::clone(&self.conn);
        let ns = namespace.to_string();
        let k = key.to_string();
        let expires_at = ttl_ms.map(|ttl| now_ms() + ttl);

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| StorageError::Backend(format!("lock: {e}")))?;
            conn.execute(
                "INSERT OR REPLACE INTO kv (namespace, key, value, expires_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![ns, k, value, expires_at],
            )
            .map_err(|e| StorageError::Backend(format!("sqlite set: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| StorageError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
        let conn = Arc::clone(&self.conn);
        let ns = namespace.to_string();
        let k = key.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| StorageError::Backend(format!("lock: {e}")))?;
            conn.execute(
                "DELETE FROM kv WHERE namespace = ?1 AND key = ?2",
                rusqlite::params![ns, k],
            )
            .map_err(|e| StorageError::Backend(format!("sqlite delete: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| StorageError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn list_keys(
        &self,
        namespace: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<String>, StorageError> {
        let conn = Arc::clone(&self.conn);
        let ns = namespace.to_string();
        let prefix = prefix.map(|p| p.to_string());
        let now = now_ms();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| StorageError::Backend(format!("lock: {e}")))?;

            let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match &prefix {
                Some(p) => {
                    let pattern = format!("{}%", p);
                    (
                        "SELECT key FROM kv WHERE namespace = ?1 AND key LIKE ?2 AND (expires_at IS NULL OR expires_at > ?3)",
                        vec![
                            Box::new(ns) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(pattern),
                            Box::new(now),
                        ],
                    )
                }
                None => (
                    "SELECT key FROM kv WHERE namespace = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
                    vec![
                        Box::new(ns) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(now),
                    ],
                ),
            };

            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| StorageError::Backend(format!("sqlite prepare: {e}")))?;

            let keys = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |row| row.get(0))
                .map_err(|e| StorageError::Backend(format!("sqlite list_keys: {e}")))?
                .collect::<Result<Vec<String>, _>>()
                .map_err(|e| StorageError::Backend(format!("sqlite list_keys collect: {e}")))?;

            Ok(keys)
        })
        .await
        .map_err(|e| StorageError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn set_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<bool, StorageError> {
        let conn = Arc::clone(&self.conn);
        let ns = namespace.to_string();
        let k = key.to_string();
        let now = now_ms();
        let expires_at = ttl_ms.map(|ttl| now + ttl);

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| StorageError::Backend(format!("lock: {e}")))?;

            // First, delete any expired entry for this key so INSERT OR IGNORE
            // doesn't collide with a stale row.
            conn.execute(
                "DELETE FROM kv WHERE namespace = ?1 AND key = ?2 AND expires_at IS NOT NULL AND expires_at <= ?3",
                rusqlite::params![ns, k, now],
            )
            .map_err(|e| StorageError::Backend(format!("sqlite delete expired: {e}")))?;

            // INSERT OR IGNORE: if the key already exists (and is not expired,
            // since we just cleaned up), this inserts 0 rows.
            let rows = conn
                .execute(
                    "INSERT OR IGNORE INTO kv (namespace, key, value, expires_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![ns, k, value, expires_at],
                )
                .map_err(|e| StorageError::Backend(format!("sqlite set_if_absent: {e}")))?;

            Ok(rows > 0)
        })
        .await
        .map_err(|e| StorageError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn flush_expired(&self) -> Result<u64, StorageError> {
        let conn = Arc::clone(&self.conn);
        let now = now_ms();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| StorageError::Backend(format!("lock: {e}")))?;
            let deleted = conn
                .execute(
                    "DELETE FROM kv WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                    rusqlite::params![now],
                )
                .map_err(|e| StorageError::Backend(format!("sqlite flush: {e}")))?;
            Ok(deleted as u64)
        })
        .await
        .map_err(|e| StorageError::Backend(format!("spawn_blocking: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_core_config::storage::StorageEngine;

    /// Create an in-memory SQLite engine for tests (no file I/O needed).
    fn new_engine() -> SqliteStorageEngine {
        SqliteStorageEngine::new(":memory:").expect("in-memory sqlite should open")
    }

    #[tokio::test]
    async fn get_set_round_trip() {
        let engine = new_engine();
        let value = b"hello world".to_vec();

        engine.set("ns", "key1", value.clone(), None).await.unwrap();
        let got = engine.get("ns", "key1").await.unwrap();
        assert_eq!(got, Some(value));
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let engine = new_engine();
        let got = engine.get("ns", "no-such-key").await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn del_removes_key() {
        let engine = new_engine();
        engine.set("ns", "key1", b"data".to_vec(), None).await.unwrap();
        engine.delete("ns", "key1").await.unwrap();
        let got = engine.get("ns", "key1").await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn del_nonexistent_is_ok() {
        let engine = new_engine();
        // Deleting a key that was never set should not error.
        engine.delete("ns", "ghost").await.unwrap();
    }

    #[tokio::test]
    async fn overwrite_existing_key() {
        let engine = new_engine();
        engine.set("ns", "key1", b"first".to_vec(), None).await.unwrap();
        engine.set("ns", "key1", b"second".to_vec(), None).await.unwrap();
        let got = engine.get("ns", "key1").await.unwrap();
        assert_eq!(got, Some(b"second".to_vec()));
    }

    #[tokio::test]
    async fn list_keys_no_prefix() {
        let engine = new_engine();
        engine.set("ns", "alpha", b"1".to_vec(), None).await.unwrap();
        engine.set("ns", "beta", b"2".to_vec(), None).await.unwrap();
        engine.set("other", "gamma", b"3".to_vec(), None).await.unwrap();

        let mut keys = engine.list_keys("ns", None).await.unwrap();
        keys.sort();
        assert_eq!(keys, vec!["alpha", "beta"]);
    }

    #[tokio::test]
    async fn list_keys_with_prefix() {
        let engine = new_engine();
        engine.set("ns", "user:1", b"a".to_vec(), None).await.unwrap();
        engine.set("ns", "user:2", b"b".to_vec(), None).await.unwrap();
        engine.set("ns", "session:1", b"c".to_vec(), None).await.unwrap();

        let mut keys = engine.list_keys("ns", Some("user:")).await.unwrap();
        keys.sort();
        assert_eq!(keys, vec!["user:1", "user:2"]);
    }

    #[tokio::test]
    async fn ttl_expiration() {
        let engine = new_engine();
        // Set a key with a 50ms TTL.
        engine
            .set("ns", "ephemeral", b"temp".to_vec(), Some(50))
            .await
            .unwrap();

        // Immediately should still be present.
        let got = engine.get("ns", "ephemeral").await.unwrap();
        assert_eq!(got, Some(b"temp".to_vec()));

        // Wait for expiration.
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Should be gone (lazy delete on read).
        let got = engine.get("ns", "ephemeral").await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn ttl_list_keys_excludes_expired() {
        let engine = new_engine();
        engine.set("ns", "live", b"ok".to_vec(), None).await.unwrap();
        engine
            .set("ns", "dying", b"bye".to_vec(), Some(50))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(80)).await;

        let keys = engine.list_keys("ns", None).await.unwrap();
        assert_eq!(keys, vec!["live"]);
    }

    #[tokio::test]
    async fn set_empty_value() {
        let engine = new_engine();
        engine.set("ns", "empty", vec![], None).await.unwrap();
        let got = engine.get("ns", "empty").await.unwrap();
        assert_eq!(got, Some(vec![]));
    }

    #[tokio::test]
    async fn binary_value_storage() {
        let engine = new_engine();
        // All byte values 0x00..0xFF
        let value: Vec<u8> = (0..=255).collect();
        engine.set("ns", "bin", value.clone(), None).await.unwrap();
        let got = engine.get("ns", "bin").await.unwrap();
        assert_eq!(got, Some(value));
    }

    #[tokio::test]
    async fn namespace_isolation() {
        let engine = new_engine();
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
    }

    #[tokio::test]
    async fn set_if_absent_inserts_when_missing() {
        let engine = new_engine();
        let inserted = engine
            .set_if_absent("ns", "key1", b"val".to_vec(), None)
            .await
            .unwrap();
        assert!(inserted);
        assert_eq!(
            engine.get("ns", "key1").await.unwrap(),
            Some(b"val".to_vec())
        );
    }

    #[tokio::test]
    async fn set_if_absent_does_not_overwrite() {
        let engine = new_engine();
        engine
            .set("ns", "key1", b"original".to_vec(), None)
            .await
            .unwrap();

        let inserted = engine
            .set_if_absent("ns", "key1", b"new".to_vec(), None)
            .await
            .unwrap();
        assert!(!inserted);
        assert_eq!(
            engine.get("ns", "key1").await.unwrap(),
            Some(b"original".to_vec())
        );
    }

    #[tokio::test]
    async fn set_if_absent_replaces_expired_key() {
        let engine = new_engine();
        engine
            .set("ns", "key1", b"old".to_vec(), Some(50))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(80)).await;

        // Key expired, so set_if_absent should succeed.
        let inserted = engine
            .set_if_absent("ns", "key1", b"new".to_vec(), None)
            .await
            .unwrap();
        assert!(inserted);
        assert_eq!(
            engine.get("ns", "key1").await.unwrap(),
            Some(b"new".to_vec())
        );
    }

    #[tokio::test]
    async fn flush_expired_removes_stale_entries() {
        let engine = new_engine();
        engine.set("ns", "live", b"ok".to_vec(), None).await.unwrap();
        engine
            .set("ns", "stale1", b"x".to_vec(), Some(50))
            .await
            .unwrap();
        engine
            .set("ns", "stale2", b"y".to_vec(), Some(50))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(80)).await;

        let removed = engine.flush_expired().await.unwrap();
        assert_eq!(removed, 2);

        // Live key still present.
        assert_eq!(
            engine.get("ns", "live").await.unwrap(),
            Some(b"ok".to_vec())
        );
    }

    #[tokio::test]
    async fn flush_expired_returns_zero_when_nothing_expired() {
        let engine = new_engine();
        engine.set("ns", "key1", b"val".to_vec(), None).await.unwrap();
        let removed = engine.flush_expired().await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn file_based_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let path_str = db_path.to_str().unwrap();

        // Write data with one engine instance.
        {
            let engine = SqliteStorageEngine::new(path_str).unwrap();
            engine
                .set("ns", "persist", b"data".to_vec(), None)
                .await
                .unwrap();
        }

        // Open a fresh instance on the same file and verify data survives.
        {
            let engine = SqliteStorageEngine::new(path_str).unwrap();
            let got = engine.get("ns", "persist").await.unwrap();
            assert_eq!(got, Some(b"data".to_vec()));
        }
    }
}
