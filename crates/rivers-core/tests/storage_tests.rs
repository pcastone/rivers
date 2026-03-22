//! StorageEngine integration tests (InMemory backend).

use rivers_core::storage::{
    create_storage_engine, is_reserved_namespace, InMemoryStorageEngine, StorageEngine,
};
use rivers_core::config::StorageEngineConfig;

fn engine() -> InMemoryStorageEngine {
    InMemoryStorageEngine::new()
}

// ── KV operations ───────────────────────────────────────────────────

#[tokio::test]
async fn kv_set_and_get() {
    let e = engine();
    e.set("ns", "key1", b"hello".to_vec(), None).await.unwrap();
    let val = e.get("ns", "key1").await.unwrap();
    assert_eq!(val, Some(b"hello".to_vec()));
}

#[tokio::test]
async fn kv_get_missing_returns_none() {
    let e = engine();
    let val = e.get("ns", "missing").await.unwrap();
    assert_eq!(val, None);
}

#[tokio::test]
async fn kv_delete() {
    let e = engine();
    e.set("ns", "key1", b"data".to_vec(), None).await.unwrap();
    e.delete("ns", "key1").await.unwrap();
    assert_eq!(e.get("ns", "key1").await.unwrap(), None);
}

#[tokio::test]
async fn kv_overwrite() {
    let e = engine();
    e.set("ns", "k", b"v1".to_vec(), None).await.unwrap();
    e.set("ns", "k", b"v2".to_vec(), None).await.unwrap();
    assert_eq!(e.get("ns", "k").await.unwrap(), Some(b"v2".to_vec()));
}

#[tokio::test]
async fn kv_namespace_isolation() {
    let e = engine();
    e.set("ns1", "k", b"a".to_vec(), None).await.unwrap();
    e.set("ns2", "k", b"b".to_vec(), None).await.unwrap();
    assert_eq!(e.get("ns1", "k").await.unwrap(), Some(b"a".to_vec()));
    assert_eq!(e.get("ns2", "k").await.unwrap(), Some(b"b".to_vec()));
}

#[tokio::test]
async fn kv_list_keys_all() {
    let e = engine();
    e.set("ns", "alpha", b"1".to_vec(), None).await.unwrap();
    e.set("ns", "beta", b"2".to_vec(), None).await.unwrap();
    e.set("other", "gamma", b"3".to_vec(), None).await.unwrap();
    let mut keys = e.list_keys("ns", None).await.unwrap();
    keys.sort();
    assert_eq!(keys, vec!["alpha", "beta"]);
}

#[tokio::test]
async fn kv_list_keys_with_prefix() {
    let e = engine();
    e.set("ns", "user:1", b"a".to_vec(), None).await.unwrap();
    e.set("ns", "user:2", b"b".to_vec(), None).await.unwrap();
    e.set("ns", "order:1", b"c".to_vec(), None).await.unwrap();
    let keys = e.list_keys("ns", Some("user:")).await.unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(|k| k.starts_with("user:")));
}

// ── TTL ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn kv_ttl_expired_returns_none() {
    let e = engine();
    // Set with 1ms TTL
    e.set("ns", "ephemeral", b"gone".to_vec(), Some(1)).await.unwrap();
    // Wait for expiry
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(e.get("ns", "ephemeral").await.unwrap(), None);
}

#[tokio::test]
async fn kv_ttl_not_expired_returns_value() {
    let e = engine();
    e.set("ns", "alive", b"here".to_vec(), Some(60_000)).await.unwrap();
    assert_eq!(e.get("ns", "alive").await.unwrap(), Some(b"here".to_vec()));
}

#[tokio::test]
async fn flush_expired_removes_stale_kv() {
    let e = engine();
    e.set("ns", "old", b"stale".to_vec(), Some(1)).await.unwrap();
    e.set("ns", "fresh", b"ok".to_vec(), None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let removed = e.flush_expired().await.unwrap();
    assert_eq!(removed, 1);
    assert_eq!(e.get("ns", "fresh").await.unwrap(), Some(b"ok".to_vec()));
}

// ── Reserved prefixes ───────────────────────────────────────────────

#[test]
fn reserved_namespace_check() {
    assert!(is_reserved_namespace("session:user123"));
    assert!(is_reserved_namespace("csrf:token456"));
    assert!(is_reserved_namespace("poll:loop1"));
    assert!(is_reserved_namespace("rivers:internal"));
    assert!(!is_reserved_namespace("cache"));
    assert!(!is_reserved_namespace("myapp:data"));
}

// ── Sentinel Key ─────────────────────────────────────────────────────

#[tokio::test]
async fn sentinel_claim_and_release() {
    let e = engine();
    rivers_core::storage::claim_sentinel(&e, "node-1").await.unwrap();
    rivers_core::storage::release_sentinel(&e, "node-1").await.unwrap();
}

#[tokio::test]
async fn sentinel_blocks_second_node() {
    let e = engine();
    rivers_core::storage::claim_sentinel(&e, "node-1").await.unwrap();
    let result = rivers_core::storage::claim_sentinel(&e, "node-2").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn sentinel_same_node_reclaims() {
    let e = engine();
    rivers_core::storage::claim_sentinel(&e, "node-1").await.unwrap();
    // Same node can refresh/reclaim
    rivers_core::storage::refresh_sentinel(&e, "node-1").await.unwrap();
}

// ── Factory ─────────────────────────────────────────────────────────

#[test]
fn factory_creates_memory_backend() {
    let config = StorageEngineConfig::default();
    assert!(create_storage_engine(&config).is_ok());
}

#[test]
fn factory_rejects_unknown_backend() {
    let mut config = StorageEngineConfig::default();
    config.backend = "nosql".to_string();
    assert!(create_storage_engine(&config).is_err());
}
