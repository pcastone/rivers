//! Polling view support.
//!
//! Per `rivers-polling-views-spec.md`.
//!
//! Rivers-managed poll loops for SSE/WS views with diff strategies
//! and client deduplication.

mod diff;
mod executor;
mod runner;
mod state;

pub use diff::*;
pub use executor::*;
pub use runner::*;
pub use state::*;

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::rivers_core::storage::InMemoryStorageEngine;

    /// Mock DataView executor for testing.
    struct MockExecutor {
        /// Data to return on each call (cycled).
        responses: tokio::sync::Mutex<Vec<serde_json::Value>>,
    }

    impl MockExecutor {
        fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait::async_trait]
    impl PollDataViewExecutor for MockExecutor {
        async fn execute(
            &self,
            _dataview_name: &str,
            _params: &std::collections::HashMap<String, String>,
        ) -> Result<serde_json::Value, PollError> {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(serde_json::Value::Null)
            } else {
                Ok(responses.remove(0))
            }
        }
    }

    fn make_key() -> PollLoopKey {
        PollLoopKey {
            view_id: "test_view".into(),
            param_hash: "abc123".into(),
        }
    }

    #[tokio::test]
    async fn test_save_and_load_poll_state() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let data = serde_json::json!({"count": 42});

        // Save
        save_poll_state(&storage, &key, &data, None).await.unwrap();

        // Load
        let loaded = load_poll_state(&storage, &key).await.unwrap();
        assert_eq!(loaded, Some(data));
    }

    #[tokio::test]
    async fn test_load_poll_state_returns_none_for_missing() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();

        let loaded = load_poll_state(&storage, &key).await.unwrap();
        assert_eq!(loaded, None);
    }

    #[tokio::test]
    async fn test_delete_poll_state() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let data = serde_json::json!({"x": 1});

        save_poll_state(&storage, &key, &data, None).await.unwrap();
        delete_poll_state(&storage, &key).await.unwrap();

        let loaded = load_poll_state(&storage, &key).await.unwrap();
        assert_eq!(loaded, None);
    }

    #[tokio::test]
    async fn test_poll_tick_first_tick_always_changed() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = PollLoopState::new(key.clone(), DiffStrategy::Hash, 1000);

        let data = serde_json::json!({"items": [1, 2, 3]});
        let executor = MockExecutor::new(vec![data.clone()]);

        let result = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();

        assert!(result.changed);
        assert_eq!(result.current_data, data);
        assert!(result.new_hash.is_some());

        // State should now be persisted
        let persisted = load_poll_state(&storage, &key).await.unwrap();
        assert_eq!(persisted, Some(data));
    }

    #[tokio::test]
    async fn test_poll_tick_no_change_on_same_data() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = PollLoopState::new(key.clone(), DiffStrategy::Hash, 1000);

        let data = serde_json::json!({"stable": true});
        let executor = MockExecutor::new(vec![data.clone(), data.clone()]);

        // First tick — changed
        let r1 = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();
        assert!(r1.changed);

        // Second tick — same data, not changed
        let r2 = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();
        assert!(!r2.changed);
    }

    #[tokio::test]
    async fn test_poll_tick_detects_change() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = PollLoopState::new(key.clone(), DiffStrategy::Hash, 1000);

        let data1 = serde_json::json!({"version": 1});
        let data2 = serde_json::json!({"version": 2});
        let executor = MockExecutor::new(vec![data1, data2.clone()]);

        // First tick
        execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();

        // Second tick — data changed
        let r2 = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();
        assert!(r2.changed);
        assert_eq!(r2.current_data, data2);
    }

    #[tokio::test]
    async fn test_poll_tick_null_strategy() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = PollLoopState::new(key.clone(), DiffStrategy::Null, 1000);

        // Non-empty data — changed
        let executor = MockExecutor::new(vec![serde_json::json!({"x": 1})]);
        let r = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();
        assert!(r.changed);
        assert!(r.new_hash.is_none());

        // Null data — not changed
        let executor2 = MockExecutor::new(vec![serde_json::Value::Null]);
        let r2 = execute_poll_tick(
            &executor2,
            &storage,
            &loop_state,
            "dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();
        assert!(!r2.changed);
    }

    #[tokio::test]
    async fn test_broadcast_only_on_change() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = std::sync::Arc::new(PollLoopState::new(key.clone(), DiffStrategy::Hash, 1000));

        // Subscribe a client
        let mut rx = loop_state.subscribe();

        let data = serde_json::json!({"tick": 1});
        let executor = MockExecutor::new(vec![data.clone(), data.clone()]);

        // First tick — broadcasts
        run_poll_tick_and_broadcast(
            &executor,
            &storage,
            &loop_state,
            "dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();

        let update = rx.try_recv().unwrap();
        assert!(update.changed);
        assert_eq!(update.data, data);

        // Second tick — same data, no broadcast
        run_poll_tick_and_broadcast(
            &executor,
            &storage,
            &loop_state,
            "dv",
            &std::collections::HashMap::new(),
        )
        .await
        .unwrap();

        // Should not have a new message
        assert!(rx.try_recv().is_err());
    }

    // ── N4.6–N4.8: In-memory poll tick tests ──────────────

    #[tokio::test]
    async fn test_execute_poll_tick_inmemory_first_tick_changed() {
        let mut prev = None;
        let result =
            execute_poll_tick_inmemory("test_view", "hash1", &mut prev, &DiffStrategy::Hash, None, None)
                .await;
        assert!(result.is_some());
        assert!(prev.is_some());
    }

    #[tokio::test]
    async fn test_execute_poll_tick_inmemory_second_tick_unchanged() {
        let mut prev = None;
        // First tick — changed
        let _ =
            execute_poll_tick_inmemory("test_view", "hash1", &mut prev, &DiffStrategy::Hash, None, None)
                .await;

        // Second tick — same stub data, should not change
        let result =
            execute_poll_tick_inmemory("test_view", "hash1", &mut prev, &DiffStrategy::Hash, None, None)
                .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_execute_poll_tick_inmemory_null_strategy() {
        let mut prev = None;
        // Null strategy: non-null data is always "changed"
        let result =
            execute_poll_tick_inmemory("test_view", "hash1", &mut prev, &DiffStrategy::Null, None, None)
                .await;
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_execute_poll_tick_inmemory_change_detect_fallback_to_hash() {
        let mut prev = None;
        // First call — no previous state, always changed
        let result = execute_poll_tick_inmemory(
            "test_view",
            "hash1",
            &mut prev,
            &DiffStrategy::ChangeDetect,
            None,
            None,
        )
        .await;
        assert!(result.is_some());

        // Second call — without pool, ChangeDetect falls back to hash diff;
        // identical stub data produces the same hash → no change
        let result2 = execute_poll_tick_inmemory(
            "test_view",
            "hash1",
            &mut prev,
            &DiffStrategy::ChangeDetect,
            None,
            None,
        )
        .await;
        assert!(result2.is_none());
    }

    #[tokio::test]
    async fn test_run_poll_loop_inmemory_broadcasts() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = std::sync::Arc::new(PollLoopState::new(key, DiffStrategy::Hash, 50));

        // Subscribe a client
        let mut rx = loop_state.subscribe();

        let data = serde_json::json!({"tick": 1});
        let executor = MockExecutor::new(vec![data.clone()]);

        let ls = loop_state.clone();
        let handle = tokio::spawn(async move {
            run_poll_loop_inmemory(ls, &executor, &storage, "test_dv", &std::collections::HashMap::new()).await;
        });

        // Wait for at least one broadcast
        let update = tokio::time::timeout(
            tokio::time::Duration::from_millis(500),
            rx.recv(),
        )
        .await;
        assert!(update.is_ok());
        let update = update.unwrap().unwrap();
        assert!(update.changed);

        handle.abort();
    }

    #[tokio::test]
    async fn test_poll_state_key_format() {
        let key = PollLoopKey {
            view_id: "my_view".into(),
            param_hash: "deadbeef".into(),
        };
        assert_eq!(key.storage_key(), "poll:my_view:deadbeef");
    }

    // ── D15: compute_diff tests ─────────────────────────────

    #[test]
    fn test_compute_diff_identical_objects() {
        let a = serde_json::json!({"x": 1, "y": 2});
        let b = serde_json::json!({"x": 1, "y": 2});
        let result = compute_diff(&a, &b);
        assert!(!result.changed);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_added_key() {
        let a = serde_json::json!({"x": 1});
        let b = serde_json::json!({"x": 1, "y": 2});
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 1);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_removed_key() {
        let a = serde_json::json!({"x": 1, "y": 2});
        let b = serde_json::json!({"x": 1});
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_modified_value() {
        let a = serde_json::json!({"x": 1});
        let b = serde_json::json!({"x": 2});
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.modified_count, 1);
    }

    #[test]
    fn test_compute_diff_arrays_added() {
        let a = serde_json::json!([1, 2]);
        let b = serde_json::json!([1, 2, 3]);
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 1);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_arrays_removed() {
        let a = serde_json::json!([1, 2, 3]);
        let b = serde_json::json!([1, 2]);
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_arrays_modified() {
        let a = serde_json::json!([1, 2, 3]);
        let b = serde_json::json!([1, 99, 3]);
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.modified_count, 1);
    }

    #[test]
    fn test_compute_diff_scalar_change() {
        let a = serde_json::json!(42);
        let b = serde_json::json!(43);
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.modified_count, 1);
    }

    #[test]
    fn test_compute_diff_type_change() {
        let a = serde_json::json!(42);
        let b = serde_json::json!("hello");
        let result = compute_diff(&a, &b);
        assert!(result.changed);
    }

    #[test]
    fn test_compute_diff_identical_arrays() {
        let a = serde_json::json!([1, 2, 3]);
        let b = serde_json::json!([1, 2, 3]);
        let result = compute_diff(&a, &b);
        assert!(!result.changed);
    }

    #[test]
    fn test_compute_diff_complex_object() {
        let a = serde_json::json!({"a": 1, "b": 2, "c": 3});
        let b = serde_json::json!({"a": 1, "b": 99, "d": 4});
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 1);   // d added
        assert_eq!(result.removed_count, 1); // c removed
        assert_eq!(result.modified_count, 1); // b changed
    }

    // ── U8: PollChangeDetectTimeout tests ──────────────────

    #[test]
    fn change_detect_timeout_detected() {
        assert!(check_change_detect_timeout(6000));
        assert!(!check_change_detect_timeout(3000));
    }

    #[test]
    fn change_detect_timeout_boundary() {
        // Exactly at threshold should not trigger
        assert!(!check_change_detect_timeout(5000));
        // Just above threshold should trigger
        assert!(check_change_detect_timeout(5001));
    }
}
