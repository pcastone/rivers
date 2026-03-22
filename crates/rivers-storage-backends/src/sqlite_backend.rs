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
