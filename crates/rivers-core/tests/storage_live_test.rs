//! Live integration tests for StorageEngine backends.
//!
//! SQLite tests use tempfile — no external infrastructure needed.
//! Redis tests require a running Redis server. Set RIVERS_TEST_REDIS_HOST (default: localhost).
//! Credentials are resolved from a LockBox keystore (see `common/mod.rs`).
//!
//! Run with: cargo test --test storage_live_test

mod common;

use std::time::Duration;

use rivers_core::storage::StorageEngine;
use rivers_core::RedisStorageEngine;
use rivers_core::SqliteStorageEngine;

fn redis_hosts() -> Vec<String> {
    let h = std::env::var("RIVERS_TEST_REDIS_HOST").unwrap_or_else(|_| "localhost".to_string());
    vec![format!("{h}:6379"), format!("{h}:6379"), format!("{h}:6379")]
}

fn redis_url() -> String {
    let creds = common::TestCredentials::new();
    let password = creds.get("redis/test");
    redis_hosts()
        .iter()
        .map(|h| {
            if password.is_empty() {
                format!("redis://{h}")
            } else {
                format!("redis://:{}@{h}", password)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Generate a unique key prefix to avoid collisions between test runs.
fn unique_prefix() -> String {
    format!(
        "test_{}_{}_",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Try to create a Redis storage engine; returns None if unreachable.
async fn try_redis_engine(prefix: &str) -> Option<RedisStorageEngine> {
    let url = redis_url();
    match RedisStorageEngine::with_prefix(&url, prefix) {
        Ok(engine) => {
            // Verify connectivity with a probe GET
            match tokio::time::timeout(Duration::from_secs(5), engine.get("_probe", "_ping")).await
            {
                Ok(Ok(_)) => Some(engine),
                Ok(Err(e)) => {
                    eprintln!("SKIP: Redis unreachable — {e}");
                    None
                }
                Err(_) => {
                    eprintln!("SKIP: Redis connection timed out");
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("SKIP: Redis client creation failed — {e}");
            None
        }
    }
}

// ── SQLite StorageEngine live tests ─────────────────────────────────

#[tokio::test]
async fn storage_sqlite_set_get_delete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_storage.db");
    let engine = SqliteStorageEngine::new(path.to_str().unwrap()).unwrap();

    // SET
    engine
        .set("test_ns", "key1", b"value_one".to_vec(), None)
        .await
        .unwrap();

    // GET
    let val = engine.get("test_ns", "key1").await.unwrap();
    assert_eq!(val, Some(b"value_one".to_vec()));

    // Overwrite
    engine
        .set("test_ns", "key1", b"value_two".to_vec(), None)
        .await
        .unwrap();
    let val = engine.get("test_ns", "key1").await.unwrap();
    assert_eq!(val, Some(b"value_two".to_vec()));

    // DELETE
    engine.delete("test_ns", "key1").await.unwrap();
    let val = engine.get("test_ns", "key1").await.unwrap();
    assert_eq!(val, None);

    // GET missing key returns None
    let val = engine.get("test_ns", "nonexistent").await.unwrap();
    assert_eq!(val, None);
}

#[tokio::test]
async fn storage_sqlite_ttl_expiration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_ttl.db");
    let engine = SqliteStorageEngine::new(path.to_str().unwrap()).unwrap();

    // SET with short TTL (10ms)
    engine
        .set("ttl_ns", "ephemeral", b"temporary".to_vec(), Some(10))
        .await
        .unwrap();

    // Should be readable immediately
    let val = engine.get("ttl_ns", "ephemeral").await.unwrap();
    assert_eq!(val, Some(b"temporary".to_vec()));

    // Wait for expiration
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Should be gone (lazy TTL check on get)
    let val = engine.get("ttl_ns", "ephemeral").await.unwrap();
    assert_eq!(val, None, "value should have expired");

    // Verify flush_expired works too
    engine
        .set("ttl_ns", "another", b"also_temp".to_vec(), Some(1))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    let removed = engine.flush_expired().await.unwrap();
    assert!(removed >= 1, "flush_expired should remove at least 1 entry");
}

#[tokio::test]
async fn storage_sqlite_list_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_list.db");
    let engine = SqliteStorageEngine::new(path.to_str().unwrap()).unwrap();

    engine
        .set("ns", "user:alice", b"1".to_vec(), None)
        .await
        .unwrap();
    engine
        .set("ns", "user:bob", b"2".to_vec(), None)
        .await
        .unwrap();
    engine
        .set("ns", "order:100", b"3".to_vec(), None)
        .await
        .unwrap();

    // List all keys in namespace
    let mut all_keys = engine.list_keys("ns", None).await.unwrap();
    all_keys.sort();
    assert_eq!(all_keys, vec!["order:100", "user:alice", "user:bob"]);

    // List with prefix
    let user_keys = engine.list_keys("ns", Some("user:")).await.unwrap();
    assert_eq!(user_keys.len(), 2);
    assert!(user_keys.iter().all(|k| k.starts_with("user:")));
}

#[tokio::test]
async fn storage_sqlite_namespace_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_iso.db");
    let engine = SqliteStorageEngine::new(path.to_str().unwrap()).unwrap();

    engine
        .set("ns_a", "key", b"value_a".to_vec(), None)
        .await
        .unwrap();
    engine
        .set("ns_b", "key", b"value_b".to_vec(), None)
        .await
        .unwrap();

    assert_eq!(
        engine.get("ns_a", "key").await.unwrap(),
        Some(b"value_a".to_vec())
    );
    assert_eq!(
        engine.get("ns_b", "key").await.unwrap(),
        Some(b"value_b".to_vec())
    );
}

// ── Redis StorageEngine live tests ──────────────────────────────────

#[tokio::test]
async fn storage_redis_set_get_delete() {
    let prefix = unique_prefix();
    let Some(engine) = try_redis_engine(&prefix).await else {
        return;
    };

    // SET
    engine.set("rns", "key1", b"redis_value".to_vec(), None).await.unwrap();

    // GET
    let val = engine.get("rns", "key1").await.unwrap();
    assert_eq!(val, Some(b"redis_value".to_vec()));

    // Overwrite
    engine.set("rns", "key1", b"updated".to_vec(), None).await.unwrap();
    let val = engine.get("rns", "key1").await.unwrap();
    assert_eq!(val, Some(b"updated".to_vec()));

    // DELETE
    engine.delete("rns", "key1").await.unwrap();
    let val = engine.get("rns", "key1").await.unwrap();
    assert_eq!(val, None);

    // GET missing returns None
    let val = engine.get("rns", "nonexistent").await.unwrap();
    assert_eq!(val, None);
}

#[tokio::test]
async fn storage_redis_ttl_expiration() {
    let prefix = unique_prefix();
    let Some(engine) = try_redis_engine(&prefix).await else {
        return;
    };

    // SET with short TTL (100ms)
    engine.set("rttl", "ephemeral", b"temporary".to_vec(), Some(100)).await.unwrap();

    // Should be readable immediately
    let val = engine.get("rttl", "ephemeral").await.unwrap();
    assert_eq!(val, Some(b"temporary".to_vec()));

    // Wait for Redis to expire the key
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Should be gone (Redis handles TTL natively)
    let val = engine.get("rttl", "ephemeral").await.unwrap();
    assert_eq!(val, None, "value should have expired via Redis PEXPIRE");
}

#[tokio::test]
async fn storage_redis_list_keys() {
    let prefix = unique_prefix();
    let Some(engine) = try_redis_engine(&prefix).await else {
        return;
    };

    // Set multiple keys
    engine.set("rlk", "alpha", b"1".to_vec(), None).await.unwrap();
    engine.set("rlk", "beta", b"2".to_vec(), None).await.unwrap();
    engine.set("rlk", "gamma", b"3".to_vec(), None).await.unwrap();

    // List all keys in namespace
    let mut keys = engine.list_keys("rlk", None).await.unwrap();
    keys.sort();
    assert_eq!(keys.len(), 3, "expected 3 keys, got: {:?}", keys);
    assert_eq!(keys, vec!["alpha", "beta", "gamma"]);

    // Cleanup
    engine.delete("rlk", "alpha").await.unwrap();
    engine.delete("rlk", "beta").await.unwrap();
    engine.delete("rlk", "gamma").await.unwrap();
}

#[tokio::test]
async fn storage_sentinel_claim() {
    let prefix = unique_prefix();
    let Some(engine) = try_redis_engine(&prefix).await else {
        return;
    };

    // Claim sentinel for this node
    rivers_core::storage::claim_sentinel(&engine, "live-test-node")
        .await
        .unwrap();

    // The sentinel key should be under the rivers:node namespace
    // Verify by listing keys
    let keys = engine.list_keys("rivers:node", None).await.unwrap();
    assert!(
        keys.contains(&"live-test-node".to_string()),
        "sentinel key should exist, found: {:?}",
        keys
    );

    // Second node should be blocked
    let result = rivers_core::storage::claim_sentinel(&engine, "other-node").await;
    assert!(
        result.is_err(),
        "second node should be blocked from claiming sentinel"
    );

    // Release sentinel
    rivers_core::storage::release_sentinel(&engine, "live-test-node")
        .await
        .unwrap();

    // After release, another node should be able to claim
    rivers_core::storage::claim_sentinel(&engine, "other-node")
        .await
        .unwrap();

    // Cleanup
    rivers_core::storage::release_sentinel(&engine, "other-node")
        .await
        .unwrap();
}
