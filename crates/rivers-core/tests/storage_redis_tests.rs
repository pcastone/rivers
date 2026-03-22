//! Redis StorageEngine integration tests.
//!
//! These tests require a running Redis server at localhost:6379.
//! All tests are `#[ignore]` by default.
//!
//! Run with: cargo test --test storage_redis_tests -- --ignored

use rivers_core::config::StorageEngineConfig;
use rivers_core::storage::{create_storage_engine, StorageEngine};
use rivers_core::RedisStorageEngine;

const REDIS_URL: &str = "redis://127.0.0.1:6379";

/// Create an engine and flush the test keyspace prefix to avoid collisions.
fn engine() -> RedisStorageEngine {
    RedisStorageEngine::new(REDIS_URL).expect("connect to redis")
}

// ── KV operations ───────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn kv_set_and_get() {
    let e = engine();
    e.set("test_ns", "key1", b"hello".to_vec(), None)
        .await
        .unwrap();
    let val = e.get("test_ns", "key1").await.unwrap();
    assert_eq!(val, Some(b"hello".to_vec()));
    // cleanup
    e.delete("test_ns", "key1").await.unwrap();
}

#[tokio::test]
#[ignore]
async fn kv_get_missing_returns_none() {
    let e = engine();
    let val = e.get("test_ns", "missing_key_xyz").await.unwrap();
    assert_eq!(val, None);
}

#[tokio::test]
#[ignore]
async fn kv_delete() {
    let e = engine();
    e.set("test_ns", "del_key", b"data".to_vec(), None)
        .await
        .unwrap();
    e.delete("test_ns", "del_key").await.unwrap();
    assert_eq!(e.get("test_ns", "del_key").await.unwrap(), None);
}

#[tokio::test]
#[ignore]
async fn kv_overwrite() {
    let e = engine();
    e.set("test_ns", "ow_k", b"v1".to_vec(), None)
        .await
        .unwrap();
    e.set("test_ns", "ow_k", b"v2".to_vec(), None)
        .await
        .unwrap();
    assert_eq!(
        e.get("test_ns", "ow_k").await.unwrap(),
        Some(b"v2".to_vec())
    );
    e.delete("test_ns", "ow_k").await.unwrap();
}

#[tokio::test]
#[ignore]
async fn kv_namespace_isolation() {
    let e = engine();
    e.set("test_ns1", "iso_k", b"a".to_vec(), None)
        .await
        .unwrap();
    e.set("test_ns2", "iso_k", b"b".to_vec(), None)
        .await
        .unwrap();
    assert_eq!(
        e.get("test_ns1", "iso_k").await.unwrap(),
        Some(b"a".to_vec())
    );
    assert_eq!(
        e.get("test_ns2", "iso_k").await.unwrap(),
        Some(b"b".to_vec())
    );
    e.delete("test_ns1", "iso_k").await.unwrap();
    e.delete("test_ns2", "iso_k").await.unwrap();
}

#[tokio::test]
#[ignore]
async fn kv_list_keys_all() {
    let e = engine();
    e.set("test_list", "alpha", b"1".to_vec(), None)
        .await
        .unwrap();
    e.set("test_list", "beta", b"2".to_vec(), None)
        .await
        .unwrap();
    let mut keys = e.list_keys("test_list", None).await.unwrap();
    keys.sort();
    assert!(keys.contains(&"alpha".to_string()));
    assert!(keys.contains(&"beta".to_string()));
    // cleanup
    e.delete("test_list", "alpha").await.unwrap();
    e.delete("test_list", "beta").await.unwrap();
}

#[tokio::test]
#[ignore]
async fn kv_list_keys_with_prefix() {
    let e = engine();
    e.set("test_pfx", "user:1", b"a".to_vec(), None)
        .await
        .unwrap();
    e.set("test_pfx", "user:2", b"b".to_vec(), None)
        .await
        .unwrap();
    e.set("test_pfx", "order:1", b"c".to_vec(), None)
        .await
        .unwrap();
    let keys = e.list_keys("test_pfx", Some("user:")).await.unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(|k| k.starts_with("user:")));
    // cleanup
    e.delete("test_pfx", "user:1").await.unwrap();
    e.delete("test_pfx", "user:2").await.unwrap();
    e.delete("test_pfx", "order:1").await.unwrap();
}

// ── TTL ─────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn kv_ttl_expired_returns_none() {
    let e = engine();
    e.set("test_ttl", "ephemeral", b"gone".to_vec(), Some(50))
        .await
        .unwrap();
    // Wait for Redis to expire the key
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(e.get("test_ttl", "ephemeral").await.unwrap(), None);
}

#[tokio::test]
#[ignore]
async fn kv_ttl_not_expired_returns_value() {
    let e = engine();
    e.set("test_ttl", "alive", b"here".to_vec(), Some(60_000))
        .await
        .unwrap();
    assert_eq!(
        e.get("test_ttl", "alive").await.unwrap(),
        Some(b"here".to_vec())
    );
    e.delete("test_ttl", "alive").await.unwrap();
}

// ── Sentinel Key ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn sentinel_claim_and_release() {
    let e = engine();
    rivers_core::storage::claim_sentinel(&e, "redis-node-1")
        .await
        .unwrap();
    rivers_core::storage::release_sentinel(&e, "redis-node-1")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn sentinel_blocks_second_node() {
    let e = engine();
    rivers_core::storage::claim_sentinel(&e, "redis-node-1")
        .await
        .unwrap();
    let result = rivers_core::storage::claim_sentinel(&e, "redis-node-2").await;
    assert!(result.is_err());
    // cleanup
    rivers_core::storage::release_sentinel(&e, "redis-node-1")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn sentinel_same_node_reclaims() {
    let e = engine();
    rivers_core::storage::claim_sentinel(&e, "redis-node-1")
        .await
        .unwrap();
    rivers_core::storage::refresh_sentinel(&e, "redis-node-1")
        .await
        .unwrap();
    rivers_core::storage::release_sentinel(&e, "redis-node-1")
        .await
        .unwrap();
}

// ── flush_expired is a no-op for Redis ──────────────────────────────

#[tokio::test]
#[ignore]
async fn flush_expired_is_noop() {
    let e = engine();
    let removed = e.flush_expired().await.unwrap();
    assert_eq!(removed, 0);
}

// ── Factory ─────────────────────────────────────────────────────────

#[test]
fn factory_redis_requires_url() {
    let mut config = StorageEngineConfig::default();
    config.backend = "redis".to_string();
    config.url = None;
    assert!(create_storage_engine(&config).is_err());
}
