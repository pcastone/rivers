//! Tiered DataView cache tests — L1 LRU, L2 StorageEngine, cache key, invalidation.

use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::tiered_cache::*;
use rivers_driver_sdk::types::{QueryResult, QueryValue};

// ── Helpers ───────────────────────────────────────────────────────

fn sample_result() -> QueryResult {
    QueryResult {
        rows: vec![[
            ("id".to_string(), QueryValue::Integer(1)),
            ("name".to_string(), QueryValue::String("Alice".into())),
        ]
        .into_iter()
        .collect()],
        affected_rows: 1,
        last_insert_id: None,
    }
}

fn params(kvs: &[(&str, QueryValue)]) -> HashMap<String, QueryValue> {
    kvs.iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

// ── Cache Key ─────────────────────────────────────────────────────

#[test]
fn cache_key_deterministic() {
    let p = params(&[("limit", QueryValue::Integer(10))]);
    let k1 = cache_key("list_contacts", &p);
    let k2 = cache_key("list_contacts", &p);
    assert_eq!(k1, k2, "same inputs should produce same key");
}

#[test]
fn cache_key_different_views() {
    let p = params(&[("limit", QueryValue::Integer(10))]);
    let k1 = cache_key("view_a", &p);
    let k2 = cache_key("view_b", &p);
    assert_ne!(k1, k2, "different views should produce different keys");
}

#[test]
fn cache_key_different_params() {
    let p1 = params(&[("limit", QueryValue::Integer(10))]);
    let p2 = params(&[("limit", QueryValue::Integer(20))]);
    let k1 = cache_key("view_a", &p1);
    let k2 = cache_key("view_a", &p2);
    assert_ne!(k1, k2, "different params should produce different keys");
}

#[test]
fn cache_key_param_order_independent() {
    let p1 = params(&[
        ("a", QueryValue::Integer(1)),
        ("b", QueryValue::Integer(2)),
    ]);
    let p2 = params(&[
        ("b", QueryValue::Integer(2)),
        ("a", QueryValue::Integer(1)),
    ]);
    let k1 = cache_key("view", &p1);
    let k2 = cache_key("view", &p2);
    assert_eq!(k1, k2, "parameter order should not affect cache key");
}

#[test]
fn cache_key_contains_view_prefix() {
    let p = HashMap::new();
    let key = cache_key("my_view", &p);
    assert!(key.starts_with("cache:views:my_view:"), "key should contain view name prefix");
}

// ── Noop Cache ────────────────────────────────────────────────────

#[tokio::test]
async fn noop_cache_always_misses() {
    let cache = NoopDataViewCache;
    let p = HashMap::new();
    assert!(cache.get("view", &p).await.unwrap().is_none());
}

#[tokio::test]
async fn noop_cache_set_noop() {
    let cache = NoopDataViewCache;
    let p = HashMap::new();
    cache.set("view", &p, &sample_result(), None).await.unwrap();
    assert!(cache.get("view", &p).await.unwrap().is_none());
}

// ── L1 LRU Cache ─────────────────────────────────────────────────

#[tokio::test]
async fn l1_cache_set_and_get() {
    let cache = LruDataViewCache::new(10, 60);
    let key = "views:test:abc";

    cache.set(key.to_string(), sample_result(), None).await;
    let result = cache.get(key).await;
    assert!(result.is_some());
    assert_eq!(result.unwrap().affected_rows, 1);
}

#[tokio::test]
async fn l1_cache_miss() {
    let cache = LruDataViewCache::new(10, 60);
    assert!(cache.get("nonexistent").await.is_none());
}

#[tokio::test]
async fn l1_cache_evicts_lru() {
    let cache = LruDataViewCache::new(2, 60);

    cache.set("key1".to_string(), sample_result(), None).await;
    cache.set("key2".to_string(), sample_result(), None).await;
    cache.set("key3".to_string(), sample_result(), None).await; // evicts key1

    assert!(cache.get("key1").await.is_none(), "key1 should be evicted");
    assert!(cache.get("key2").await.is_some(), "key2 should still exist");
    assert!(cache.get("key3").await.is_some(), "key3 should exist");
}

#[tokio::test]
async fn l1_cache_lru_access_refreshes() {
    let cache = LruDataViewCache::new(2, 60);

    cache.set("key1".to_string(), sample_result(), None).await;
    cache.set("key2".to_string(), sample_result(), None).await;

    // Access key1 to make it most recently used
    cache.get("key1").await;

    // Add key3 — should evict key2 (least recently used), not key1
    cache.set("key3".to_string(), sample_result(), None).await;

    assert!(cache.get("key1").await.is_some(), "key1 should still exist (accessed recently)");
    assert!(cache.get("key2").await.is_none(), "key2 should be evicted");
}

#[tokio::test]
async fn l1_cache_ttl_expiry() {
    let cache = LruDataViewCache::new(10, 0); // TTL = 0 seconds → expires immediately

    cache.set("key".to_string(), sample_result(), None).await;
    // Sleep briefly to ensure expiry
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(cache.get("key").await.is_none(), "entry should be expired");
}

#[tokio::test]
async fn l1_cache_overwrite_same_key() {
    let cache = LruDataViewCache::new(10, 60);

    let mut result1 = sample_result();
    result1.affected_rows = 1;

    let mut result2 = sample_result();
    result2.affected_rows = 99;

    cache.set("key".to_string(), result1, None).await;
    cache.set("key".to_string(), result2, None).await;

    let result = cache.get("key").await.unwrap();
    assert_eq!(result.affected_rows, 99, "should have overwritten value");
    assert_eq!(cache.len().await, 1, "should not duplicate entries");
}

#[tokio::test]
async fn l1_cache_invalidate_by_view() {
    let cache = LruDataViewCache::new(10, 60);

    cache.set("cache:views:contacts:a".to_string(), sample_result(), None).await;
    cache.set("cache:views:contacts:b".to_string(), sample_result(), None).await;
    cache.set("cache:views:orders:c".to_string(), sample_result(), None).await;

    cache.invalidate(Some("contacts")).await;

    assert!(cache.get("cache:views:contacts:a").await.is_none());
    assert!(cache.get("cache:views:contacts:b").await.is_none());
    assert!(cache.get("cache:views:orders:c").await.is_some(), "other view should survive");
}

#[tokio::test]
async fn l1_cache_invalidate_all() {
    let cache = LruDataViewCache::new(10, 60);

    cache.set("cache:views:a:1".to_string(), sample_result(), None).await;
    cache.set("cache:views:b:2".to_string(), sample_result(), None).await;

    cache.invalidate(None).await;
    assert!(cache.is_empty().await);
}

// ── Tiered Cache (L1 only) ────────────────────────────────────────

#[tokio::test]
async fn tiered_l1_only_hit() {
    let policy = DataViewCachingPolicy {
        ttl_seconds: 60,
        l1_enabled: true,
        l1_max_entries: 100,
        l2_enabled: false,
        l2_max_value_bytes: 524_288,
    };
    let cache = TieredDataViewCache::new(policy);
    let p = params(&[("limit", QueryValue::Integer(10))]);

    cache.set("list_contacts", &p, &sample_result(), None).await.unwrap();
    let result = cache.get("list_contacts", &p).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().affected_rows, 1);
}

#[tokio::test]
async fn tiered_l1_only_miss() {
    let policy = DataViewCachingPolicy::default();
    let cache = TieredDataViewCache::new(policy);
    let p = HashMap::new();

    assert!(cache.get("nonexistent", &p).await.unwrap().is_none());
}

#[tokio::test]
async fn tiered_invalidate_view_scoped() {
    let policy = DataViewCachingPolicy::default();
    let cache = TieredDataViewCache::new(policy);
    let p1 = params(&[("id", QueryValue::Integer(1))]);
    let p2 = params(&[("id", QueryValue::Integer(2))]);

    cache.set("contacts", &p1, &sample_result(), None).await.unwrap();
    cache.set("contacts", &p2, &sample_result(), None).await.unwrap();
    cache.set("orders", &p1, &sample_result(), None).await.unwrap();

    cache.invalidate(Some("contacts")).await;

    assert!(cache.get("contacts", &p1).await.unwrap().is_none());
    assert!(cache.get("contacts", &p2).await.unwrap().is_none());
    assert!(cache.get("orders", &p1).await.unwrap().is_some());
}

#[tokio::test]
async fn tiered_invalidate_all() {
    let policy = DataViewCachingPolicy::default();
    let cache = TieredDataViewCache::new(policy);
    let p = HashMap::new();

    cache.set("a", &p, &sample_result(), None).await.unwrap();
    cache.set("b", &p, &sample_result(), None).await.unwrap();

    cache.invalidate(None).await;
    assert_eq!(cache.l1_len().await, 0);
}

// ── Tiered Cache (L1 + L2) ───────────────────────────────────────

#[tokio::test]
async fn tiered_l2_storage_roundtrip() {
    use rivers_core::storage::InMemoryStorageEngine;

    let storage = Arc::new(InMemoryStorageEngine::new());
    let policy = DataViewCachingPolicy {
        ttl_seconds: 60,
        l1_enabled: false, // disable L1 to test L2 in isolation
        l1_max_entries: 100,
        l2_enabled: true,
        l2_max_value_bytes: 524_288,
    };
    let cache = TieredDataViewCache::new(policy).with_storage(storage);
    let p = params(&[("id", QueryValue::Integer(42))]);

    cache.set("get_contact", &p, &sample_result(), None).await.unwrap();
    let result = cache.get("get_contact", &p).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().affected_rows, 1);
}

#[tokio::test]
async fn tiered_l2_warms_l1() {
    use rivers_core::storage::InMemoryStorageEngine;

    let storage = Arc::new(InMemoryStorageEngine::new());
    let policy = DataViewCachingPolicy {
        ttl_seconds: 60,
        l1_enabled: true,
        l1_max_entries: 100,
        l2_enabled: true,
        l2_max_value_bytes: 524_288,
    };
    let cache = TieredDataViewCache::new(policy).with_storage(storage.clone());
    let p = params(&[("id", QueryValue::Integer(1))]);

    // Manually populate L2 only by using a separate cache with L1 disabled
    {
        let l2_only_policy = DataViewCachingPolicy {
            l1_enabled: false,
            l2_enabled: true,
            ..Default::default()
        };
        let l2_cache = TieredDataViewCache::new(l2_only_policy)
            .with_storage(storage);
        l2_cache.set("warmtest", &p, &sample_result(), None).await.unwrap();
    }

    // Now read from tiered cache — should miss L1, hit L2, warm L1
    assert_eq!(cache.l1_len().await, 0);
    let result = cache.get("warmtest", &p).await.unwrap();
    assert!(result.is_some(), "should hit L2");
    assert_eq!(cache.l1_len().await, 1, "should have warmed L1");
}

#[tokio::test]
async fn tiered_l2_size_gate() {
    use rivers_core::storage::{InMemoryStorageEngine, StorageEngine};

    let storage = Arc::new(InMemoryStorageEngine::new());
    let policy = DataViewCachingPolicy {
        ttl_seconds: 60,
        l1_enabled: true,
        l1_max_entries: 100,
        l2_enabled: true,
        l2_max_value_bytes: 10, // very small — should skip L2
    };
    let cache = TieredDataViewCache::new(policy).with_storage(storage.clone());
    let p = params(&[("id", QueryValue::Integer(1))]);

    cache.set("big_view", &p, &sample_result(), None).await.unwrap();

    // L1 should have it
    assert_eq!(cache.l1_len().await, 1);

    // L2 should NOT have it (too large)
    let key = cache_key("big_view", &p);
    let l2_result = storage.get("cache", &key).await.unwrap();
    assert!(l2_result.is_none(), "result should be too large for L2");
}

// ── Policy Defaults ───────────────────────────────────────────────

#[test]
fn caching_policy_defaults() {
    let policy = DataViewCachingPolicy::default();
    assert_eq!(policy.ttl_seconds, 60);
    assert!(policy.l1_enabled);
    assert_eq!(policy.l1_max_entries, 1000);
    assert!(!policy.l2_enabled);
    assert_eq!(policy.l2_max_value_bytes, 131_072);
}
