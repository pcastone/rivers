use std::collections::HashMap;

use riversd::polling::{
    compute_data_hash, compute_diff, hash_diff, null_diff, DiffStrategy, PollLoopKey,
    PollLoopRegistry, PollLoopState, PollUpdate,
};

// ── DiffStrategy ────────────────────────────────────────────────

#[test]
fn diff_strategy_default_is_hash() {
    assert_eq!(DiffStrategy::from_str_opt(None), DiffStrategy::Hash);
}

#[test]
fn diff_strategy_from_str() {
    assert_eq!(DiffStrategy::from_str_opt(Some("hash")), DiffStrategy::Hash);
    assert_eq!(DiffStrategy::from_str_opt(Some("null")), DiffStrategy::Null);
    assert_eq!(
        DiffStrategy::from_str_opt(Some("change_detect")),
        DiffStrategy::ChangeDetect
    );
}

// ── PollLoopKey ─────────────────────────────────────────────────

#[test]
fn key_storage_format() {
    let mut params = HashMap::new();
    params.insert("page".to_string(), "1".to_string());
    let key = PollLoopKey::new("orders_list", &params);
    assert!(key.storage_key().starts_with("poll:orders_list:"));
}

#[test]
fn key_deterministic_param_hash() {
    let mut params1 = HashMap::new();
    params1.insert("a".to_string(), "1".to_string());
    params1.insert("b".to_string(), "2".to_string());

    let mut params2 = HashMap::new();
    params2.insert("b".to_string(), "2".to_string());
    params2.insert("a".to_string(), "1".to_string());

    let key1 = PollLoopKey::new("v", &params1);
    let key2 = PollLoopKey::new("v", &params2);
    assert_eq!(key1.param_hash, key2.param_hash);
}

#[test]
fn key_different_params_different_hash() {
    let mut params1 = HashMap::new();
    params1.insert("a".to_string(), "1".to_string());

    let mut params2 = HashMap::new();
    params2.insert("a".to_string(), "2".to_string());

    let key1 = PollLoopKey::new("v", &params1);
    let key2 = PollLoopKey::new("v", &params2);
    assert_ne!(key1.param_hash, key2.param_hash);
}

#[test]
fn key_empty_params() {
    let key = PollLoopKey::new("v", &HashMap::new());
    assert!(!key.param_hash.is_empty());
}

// ── Hash Diff ───────────────────────────────────────────────────

#[test]
fn compute_hash_deterministic() {
    let data = serde_json::json!({"a": 1, "b": 2});
    let h1 = compute_data_hash(&data);
    let h2 = compute_data_hash(&data);
    assert_eq!(h1, h2);
}

#[test]
fn compute_hash_different_data() {
    let d1 = serde_json::json!({"a": 1});
    let d2 = serde_json::json!({"a": 2});
    assert_ne!(compute_data_hash(&d1), compute_data_hash(&d2));
}

#[test]
fn hash_diff_first_poll_always_changed() {
    let data = serde_json::json!({"v": 1});
    let (changed, _hash) = hash_diff(None, &data);
    assert!(changed);
}

#[test]
fn hash_diff_same_data_not_changed() {
    let data = serde_json::json!({"v": 1});
    let hash = compute_data_hash(&data);
    let (changed, _) = hash_diff(Some(&hash), &data);
    assert!(!changed);
}

#[test]
fn hash_diff_different_data_changed() {
    let prev = serde_json::json!({"v": 1});
    let current = serde_json::json!({"v": 2});
    let hash = compute_data_hash(&prev);
    let (changed, _) = hash_diff(Some(&hash), &current);
    assert!(changed);
}

// ── Null Diff ───────────────────────────────────────────────────

#[test]
fn null_diff_null_is_no_change() {
    assert!(!null_diff(&serde_json::Value::Null));
}

#[test]
fn null_diff_empty_array_is_no_change() {
    assert!(!null_diff(&serde_json::json!([])));
}

#[test]
fn null_diff_empty_object_is_no_change() {
    assert!(!null_diff(&serde_json::json!({})));
}

#[test]
fn null_diff_empty_string_is_no_change() {
    assert!(!null_diff(&serde_json::json!("")));
}

#[test]
fn null_diff_non_empty_array_is_change() {
    assert!(null_diff(&serde_json::json!([1, 2])));
}

#[test]
fn null_diff_non_empty_object_is_change() {
    assert!(null_diff(&serde_json::json!({"a": 1})));
}

#[test]
fn null_diff_number_is_change() {
    assert!(null_diff(&serde_json::json!(42)));
}

#[test]
fn null_diff_bool_is_change() {
    assert!(null_diff(&serde_json::json!(true)));
}

// ── PollLoopState ───────────────────────────────────────────────

#[test]
fn poll_state_subscribe_count() {
    let key = PollLoopKey::new("v", &HashMap::new());
    let state = PollLoopState::new(key, DiffStrategy::Hash, 1000);

    assert_eq!(state.client_count(), 0);
    let _rx = state.subscribe();
    assert_eq!(state.client_count(), 1);
}

#[test]
fn poll_state_unsubscribe_decrements() {
    let key = PollLoopKey::new("v", &HashMap::new());
    let state = PollLoopState::new(key, DiffStrategy::Hash, 1000);

    let _rx = state.subscribe();
    state.unsubscribe();
    assert_eq!(state.client_count(), 0);
}

#[test]
fn poll_state_push_update() {
    let key = PollLoopKey::new("v", &HashMap::new());
    let state = PollLoopState::new(key, DiffStrategy::Hash, 1000);

    let mut rx = state.subscribe();
    state
        .push_update(PollUpdate {
            data: serde_json::json!({"count": 5}),
            changed: true,
        })
        .unwrap();

    let update = rx.try_recv().unwrap();
    assert!(update.changed);
    assert_eq!(update.data["count"], 5);
}

// ── PollLoopRegistry ────────────────────────────────────────────

#[tokio::test]
async fn registry_get_or_create() {
    let registry = PollLoopRegistry::new();
    let key = PollLoopKey::new("v", &HashMap::new());

    let state = registry
        .get_or_create(key.clone(), DiffStrategy::Hash, 1000)
        .await;
    assert_eq!(registry.active_loops().await, 1);

    // Same key returns same instance
    let state2 = registry
        .get_or_create(key, DiffStrategy::Hash, 1000)
        .await;
    assert_eq!(registry.active_loops().await, 1);
    // Both point to same underlying state
    assert_eq!(state.key.storage_key(), state2.key.storage_key());
}

#[tokio::test]
async fn registry_remove() {
    let registry = PollLoopRegistry::new();
    let key = PollLoopKey::new("v", &HashMap::new());
    let storage_key = key.storage_key();

    registry
        .get_or_create(key, DiffStrategy::Hash, 1000)
        .await;
    assert_eq!(registry.active_loops().await, 1);

    registry.remove(&storage_key).await;
    assert_eq!(registry.active_loops().await, 0);
}

#[tokio::test]
async fn registry_get() {
    let registry = PollLoopRegistry::new();
    assert!(registry.get("nonexistent").await.is_none());

    let key = PollLoopKey::new("v", &HashMap::new());
    let storage_key = key.storage_key();
    registry
        .get_or_create(key, DiffStrategy::Hash, 1000)
        .await;

    assert!(registry.get(&storage_key).await.is_some());
}

// ── Integration: Poll loop state full lifecycle ──────────────────

#[test]
fn poll_loop_state_multi_client_subscribe_push_unsubscribe() {
    let key = PollLoopKey::new("orders", &HashMap::new());
    let state = PollLoopState::new(key, DiffStrategy::Hash, 1000);

    // Subscribe 3 clients
    let mut rx1 = state.subscribe();
    let mut rx2 = state.subscribe();
    let mut rx3 = state.subscribe();
    assert_eq!(state.client_count(), 3);

    // Push update → all receive
    state.push_update(PollUpdate {
        data: serde_json::json!({"count": 10}),
        changed: true,
    }).unwrap();

    for rx in [&mut rx1, &mut rx2, &mut rx3] {
        let update = rx.try_recv().unwrap();
        assert!(update.changed);
        assert_eq!(update.data["count"], 10);
    }

    // Unsubscribe all
    state.unsubscribe();
    state.unsubscribe();
    state.unsubscribe();
    assert_eq!(state.client_count(), 0);

    // Saturating — doesn't go negative
    state.unsubscribe();
    assert_eq!(state.client_count(), 0);
}

// ── Integration: Registry dedup with different keys ──────────────

#[tokio::test]
async fn registry_different_keys_get_different_loops() {
    let registry = PollLoopRegistry::new();

    let mut params_a = HashMap::new();
    params_a.insert("page".to_string(), "1".to_string());

    let mut params_b = HashMap::new();
    params_b.insert("page".to_string(), "2".to_string());

    let key_a = PollLoopKey::new("orders", &params_a);
    let key_b = PollLoopKey::new("orders", &params_b);

    let state_a = registry.get_or_create(key_a, DiffStrategy::Hash, 1000).await;
    let state_b = registry.get_or_create(key_b, DiffStrategy::Hash, 1000).await;

    assert_eq!(registry.active_loops().await, 2);
    assert_ne!(state_a.key.param_hash, state_b.key.param_hash);
}

// ── Integration: PollLoopKey storage key variants ────────────────

#[test]
fn poll_loop_key_storage_key_variants() {
    let key = PollLoopKey::new("my_view", &HashMap::new());

    let base = key.storage_key();
    let prev = key.storage_key_prev();
    let meta = key.storage_key_meta();

    assert!(base.starts_with("poll:my_view:"));
    assert!(prev.ends_with(":prev"));
    assert!(meta.ends_with(":meta"));
    assert!(prev.starts_with(&base));
    assert!(meta.starts_with(&base));
}

// ── Integration: compute_diff end-to-end ─────────────────────────

#[test]
fn compute_diff_combined_add_remove_modify() {
    let prev = serde_json::json!({"a": 1, "b": 2, "c": 3});
    let curr = serde_json::json!({"a": 99, "c": 3, "d": 4});

    let result = compute_diff(&prev, &curr);
    assert!(result.changed);
    assert_eq!(result.added_count, 1);   // d
    assert_eq!(result.removed_count, 1); // b
    assert_eq!(result.modified_count, 1); // a changed
}

#[test]
fn compute_diff_empty_objects_no_change() {
    let result = compute_diff(&serde_json::json!({}), &serde_json::json!({}));
    assert!(!result.changed);
}

// ── Integration: Hash diff round-trip with storage key ───────────

#[test]
fn hash_diff_roundtrip_with_updated_data() {
    let data_v1 = serde_json::json!({"version": 1, "items": [1, 2]});
    let data_v2 = serde_json::json!({"version": 2, "items": [1, 2, 3]});

    // First poll
    let (changed1, hash1) = hash_diff(None, &data_v1);
    assert!(changed1);

    // Same data
    let (changed2, _hash2) = hash_diff(Some(&hash1), &data_v1);
    assert!(!changed2);

    // Updated data
    let (changed3, _hash3) = hash_diff(Some(&hash1), &data_v2);
    assert!(changed3);
}
