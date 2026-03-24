# Tasks — Cache Performance Fix

**Source:** Investigation showed L1 cache using O(n) VecDeque scan, cloning full results, and silently disabling when StorageEngine init fails
**Branch:** `feature/performance`

---

## Fix 1: O(1) LRU cache lookup (tiered_cache.rs)

Replace `VecDeque<(String, CachedEntry)>` with `HashMap<String, CachedEntry>` + `VecDeque<String>` for LRU order.

- [x] **C1.1** Replace `LruDataViewCache` internals: `HashMap<String, CachedEntry>` for O(1) key lookup + `VecDeque<String>` for LRU eviction order
- [x] **C1.2** Update `get()` — HashMap lookup O(1), move key to back of VecDeque for recency
- [x] **C1.3** Update `set()` — HashMap insert, push key to VecDeque back, evict front on capacity
- [x] **C1.4** Update `invalidate()` — drain matching keys from both structures
- [x] **C1.5** Verify existing tests in `tiered_cache_tests.rs` still pass

## Fix 2: Arc-wrap cached results (tiered_cache.rs + dataview_engine.rs)

Avoid deep-cloning `QueryResult` on every cache hit.

- [x] **C2.1** Change `CachedEntry.result` to `Arc<QueryResult>`
- [x] **C2.2** Update `DataViewCache` trait: `get()` returns `Arc<QueryResult>`, `set()` takes `Arc<QueryResult>`
- [x] **C2.3** Update `LruDataViewCache` — store/return `Arc<QueryResult>` (clone is cheap Arc bump)
- [x] **C2.4** Update `TieredDataViewCache` — pass Arc through L1/L2
- [x] **C2.5** Update `DataViewExecutor::execute()` — wrap driver result in Arc before cache set, deref Arc for response
- [x] **C2.6** Update `NoopDataViewCache` to match new trait signature
- [x] **C2.7** Update `DataViewResponse.query_result` to `Arc<QueryResult>`
- [x] **C2.8** Update consumers: `view_engine.rs`, `engine_loader.rs`, `v8_engine.rs`, `graphql.rs`, `polling.rs` — Arc auto-deref, no changes needed

## Fix 3: Always-on cache — never silently None (bundle_loader.rs)

L1 doesn't need StorageEngine. Always create TieredDataViewCache; only attach L2 when storage is available.

- [x] **C3.1** Change `bundle_loader.rs` — always create `TieredDataViewCache::new(policy)`, conditionally call `.with_storage()` if `ctx.storage_engine` is `Some`
- [x] **C3.2** Change `DataViewExecutor.cache` from `Option<Arc<dyn DataViewCache>>` to `Arc<dyn DataViewCache>` — no more `if let Some(ref cache)` guards
- [x] **C3.3** Update `execute()` in `dataview_engine.rs` — remove Option unwrap, call cache directly
- [x] **C3.4** Log a warning if StorageEngine is None and L2 is configured

## Validation

- [x] **C4.1** `cargo test -p rivers-runtime` passes (23/23)
- [x] **C4.2** `cargo test --workspace --lib` passes (232/232)
- [x] **C4.3** `cargo build` succeeds
