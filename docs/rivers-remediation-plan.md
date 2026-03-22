# Rivers — Remediation Plan

**Generated:** 2026-03-19
**Baseline:** 1386 tests passing, 15 crates, 78 deviations, 37 missing, 25 partial, 2 bugs
**Locked Decisions:** Session per-view (D1), Streaming callback-channel (D2), V8 default+guards (D3)
**Supersedes:** Shaping §Phase A–C execution order (Part 5)

---

## Dependency Graph

```
Wave 0: StorageEngine Bootstrap
    ├── Wave 1: Session + Auth Chain (per-view — D1)
    │       ├── Guard view dispatch wiring
    │       ├── CSRF double-submit
    │       └── on_session_valid hooks
    ├── Wave 3: DataView Pipeline (cache + validation)
    │       └── Polling state persistence
    └── Wave 5: ProcessPool ctx.store backend

Wave 0.5: EventBus Wildcard Fix
    └── LogHandler registration + file writer

Wave 2: Admin Security (independent of Wave 0)

Wave 4: Streaming Subsystems (independent, three parallel tracks)
    ├── 4A: SSE push loop
    ├── 4B: Streaming REST — Rivers.stream.push() (D2)
    └── 4C: WebSocket frame loop

Wave 5: ProcessPool Hardening (depends on Wave 0 for ctx.store)

Wave 6: Format Alignment + SHAPE Cleanup (independent)
```

Waves 0 and 0.5 are sequential prerequisites. Waves 1–6 can run in parallel after their prerequisites complete, except where noted.

---

## Wave 0: StorageEngine Bootstrap

**Why first:** SessionManager, CsrfManager, polling persistence, ctx.store, and sentinel claim all require `Arc<dyn StorageEngine>`. Nothing in Waves 1, 3, or 5 can start without this.

**Estimated scope:** ~150 lines new, ~30 lines modified

### 0.1 — Add StorageEngine to AppContext

**File:** `riversd/src/server.rs`
**Change:** Add field `pub storage_engine: Option<Arc<dyn StorageEngine>>` to `AppContext`. Initialize to `None` in `AppContext::new()`.

### 0.2 — StorageEngine factory function

**File:** `rivers-core/src/storage.rs` (new function)
**Change:** Add `pub async fn create_storage_engine(config: &StorageEngineConfig) -> Result<Arc<dyn StorageEngine>, StorageError>` that reads `config.backend` and returns `StorageSqlite::new()` or `StorageRedis::new()` accordingly. Default to SQLite if backend is `"sqlite"` or unset.

### 0.3 — Wire into startup

**File:** `riversd/src/server.rs` — `run_server_with_listener_and_log()`
**Change:** After config validation (line ~1134), before bundle loading:

```
if config.storage_engine.backend != "none" {
    let engine = create_storage_engine(&config.storage_engine).await?;
    ctx.storage_engine = Some(engine);
}
```

### 0.4 — Sentinel claim at startup

**File:** `riversd/src/server.rs` — `run_server_with_listener_and_log()`
**Change:** After StorageEngine construction, call `claim_sentinel()` if storage is Redis-backed.

### 0.5 — Fix sentinel key format (BUG-1)

**File:** `rivers-core/src/storage.rs`
**Change:** Line 244: `format!("sentinel:{}", node_id)` → `format!("{}", node_id)`. The namespace is already `rivers:node`, so the final key becomes `rivers:node:{node_id}` as spec requires. Also fix `list_keys` prefix filter on line 247 from `"sentinel:"` to `""`.

### 0.6 — Add `cache:` to reserved prefixes

**File:** `rivers-core/src/storage.rs`
**Change:** Add `"cache:"` to `RESERVED_PREFIXES` array (line ~47).

### 0.7 — SQLite `created_at` column

**File:** `rivers-core/src/storage_sqlite.rs`
**Change:** Add `created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))` to CREATE TABLE. Add index on `(namespace, created_at)`.

### 0.8 — Redis key_prefix

**File:** `rivers-core/src/storage_redis.rs`
**Change:** All Redis key operations prefix with `config.key_prefix` (default `"rivers:"`). Key format: `{key_prefix}{namespace}:{key}`.

### Traps

- Existing tests (1386) assume no StorageEngine. Some test helpers create `AppContext` directly. Those will need `storage_engine: None` added to their construction. Grep for `AppContext::new(` and `AppContext {` in test files.
- `StorageSqlite::new()` creates a file. Test harness needs temp dirs via `tempfile` crate.

---

## Wave 0.5: EventBus Wildcard Fix + LogHandler

**Why now:** One fix unblocks logging, hot reload events, and all EventBus observability. Smallest wave, largest cascade.

**Estimated scope:** ~40 lines new, ~10 lines modified

### 0.5.1 — Wildcard dispatch in publish()

**File:** `rivers-core/src/eventbus.rs` — `publish()`
**Change:** After dispatching to exact topic subscribers (line ~198), add wildcard dispatch:

```rust
// Dispatch to wildcard subscribers
if event.event_type != "*" {
    if let Some(wildcard_subs) = topics.get("*") {
        for sub in wildcard_subs {
            // same Critical/Standard/Observe dispatch logic
        }
    }
}
```

**Trap:** The read lock on `topics` is held for the duration of all handler invocations. Refactor to: collect subscriber list under lock → drop lock → dispatch. This prevents slow Observe handlers (file I/O) from blocking all other publishes.

### 0.5.2 — Register LogHandler at startup

**File:** `riversd/src/server.rs` — `run_server_with_listener_and_log()`
**Change:** After EventBus creation, before bundle loading:

```rust
let log_handler = Arc::new(LogHandler::from_config(
    &config.base.logging,
    config.app_id.clone().unwrap_or_default(),
    config.node_id.clone().unwrap_or_default(),
));
log_handler.register(&ctx.event_bus).await;
```

### 0.5.3 — Add file_writer to LogHandler

**File:** `rivers-core/src/logging.rs`
**Change:** Add `file_writer: Option<Arc<Mutex<BufWriter<File>>>>` field. In `handle()`, if file_writer is present, write the formatted line to it. Flush periodically (every N events or on timer). Construct from `config.local_file_path` if set.

### 0.5.4 — Fix EventBus priority tier names

**File:** `rivers-core/src/eventbus.rs`
**Change:** Rename `Critical → Expect`, `Standard → Handle`, add `Emit` tier between Handle and Observe. Four tiers: Expect → Handle → Emit → Observe per spec.

**Trap:** `HandlerPriority` is used throughout broker_bridge, message_consumer, pool, and logging. Renaming variants requires updating all match arms. Grep for `HandlerPriority::Critical` and `HandlerPriority::Standard`.

---

## Wave 1: Session + Auth Chain (Per-View)

**Depends on:** Wave 0 (StorageEngine)
**Estimated scope:** ~300 lines new, ~100 lines modified
**Spec amendment required:** Session moves from middleware stack position 5 to view dispatch path.

### 1.1 — Construct SessionManager in startup

**File:** `riversd/src/server.rs` — `run_server_with_listener_and_log()`
**Change:** After StorageEngine construction:

```rust
let session_manager = if let Some(ref engine) = ctx.storage_engine {
    Some(Arc::new(SessionManager::new(engine.clone(), config.session.clone())))
} else {
    None
};
```

Add `pub session_manager: Option<Arc<SessionManager>>` to `AppContext`.

### 1.2 — Startup validation: protected views require StorageEngine

**File:** `riversd/src/server.rs`
**Change:** After bundle loading, scan views. If any view has `protected: true` or a guard view is detected, and `ctx.storage_engine` is `None`, return `ServerError::Config("protected views require [storage_engine] to be configured")`.

### 1.3 — Startup validation: detect guard view, reject multiples

**File:** `riversd/src/server.rs`
**Change:** Call `detect_guard_view()` on loaded views. If `GuardDetection::Multiple`, return startup error. Store detected guard in AppContext.

### 1.4 — Session lookup in view dispatch path

**File:** `riversd/src/server.rs` — `view_dispatch_handler()`
**Change:** After route match, before `execute_rest_view`:

```
1. If view is protected (or default protection policy):
   a. Extract session ID from cookie/bearer (reuse session::extract_session_id)
   b. If session ID present, validate via SessionManager
   c. If valid, populate view_ctx.session
   d. If invalid/expired, clear cookie on response
2. If no valid session and view is protected:
   a. Execute guard handler
   b. If guard returns allow=true with session_claims, create session
   c. If guard returns allow=false, return redirect/401
```

### 1.5 — CSRF double-submit cookie

**File:** `riversd/src/server.rs` — `view_dispatch_handler()`
**Change:** After successful session validation, call `build_csrf_cookie()` and set on response. On mutating requests (POST/PUT/DELETE), validate CSRF token from header matches cookie.

### 1.6 — Construct CsrfManager

**File:** `riversd/src/server.rs`
**Change:** `CsrfManager::new(engine.clone(), config.csrf.clone())` — add to AppContext alongside SessionManager.

### 1.7 — Wire on_session_valid

**File:** `riversd/src/view_engine.rs`
**Change:** `execute_on_session_valid` exists and works. Call it from the dispatch path when `session_revalidation_interval_s` is configured and the interval has elapsed since last check.

### 1.8 — Validate http_only=false rejection

**File:** `rivers-core/src/config.rs`
**Change:** In `SessionCookieConfig` validation, reject `http_only = false` with an error message.

### 1.9 — Session token in body

**File:** `rivers-core/src/config.rs`
**Change:** Add `include_token_in_body: bool` and `token_body_key: String` to `SessionConfig`. In the dispatch path, if enabled, include session token in the JSON response body under the configured key.

### 1.10 — Remove session middleware from global stack comment

**File:** `riversd/src/server.rs`
**Change:** Update comment block at line ~187 to remove session from the numbered list. Add comment: "Session validation is per-view at dispatch time (DECISION-1)."

### Traps

- `session_middleware` function at `middleware.rs:154` becomes dead code. Don't delete it — it's well-implemented and can be repurposed as a helper called from the dispatch path. Extract the cookie/bearer parsing logic into `session::extract_and_validate()`.
- Body hash for CSRF: the request body is consumed at line 373 of the dispatch handler. CSRF validation must happen after body parse but before pipeline execution.

---

## Wave 2: Admin Security

**Depends on:** Nothing (independent of Wave 0)
**Estimated scope:** ~80 lines modified

### 2.1 — Fix body hash (CRITICAL)

**File:** `riversd/src/server.rs` — `admin_auth_middleware()`
**Change:** Line 961: replace `let body_hash = "";` with actual SHA-256 of request body. This requires consuming the body in the middleware, hashing it, and reconstructing the request.

Pattern:
```rust
let (parts, body) = request.into_parts();
let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024).await?;
let body_hash = hex::encode(Sha256::digest(&bytes));
let request = Request::from_parts(parts, Body::from(bytes));
```

### 2.2 — Fix signing payload order

**File:** `riversd/src/admin_auth.rs`
**Change:** Verify line 41 matches spec: `method\npath\ntimestamp_ms\nbody_sha256`. Current code shows `method\npath\ntimestamp\nbody_hash` which is correct order. The deviation report item on payload order appears to be a false positive. **No change needed — verify only.**

### 2.3 — Fix timestamp unit: seconds → milliseconds

**File:** `riversd/src/admin_auth.rs`
**Change:** Line 98: `.as_secs()` → `.as_millis() as u64`. Update the max_age parameter from seconds to milliseconds (300 → 300_000). Update comment at line 86.

### 2.4 — Wire IP allowlist

**File:** `riversd/src/server.rs` — `admin_auth_middleware()`
**Change:** After signature verification succeeds, call `admin::check_ip_allowlist()` with the request's remote address. Return 403 if not in allowlist. If no allowlist is configured, skip the check.

### 2.5 — Wire RBAC

**File:** `riversd/src/server.rs` — `admin_auth_middleware()`
**Change:** After IP allowlist check, call `admin::check_permission()` with the authenticated identity and the request path/method. Return 403 if denied.

### 2.6 — --no-admin-auth warning

**File:** `riversd/src/server.rs`
**Change:** When `no_auth = Some(true)`, emit `tracing::warn!("--no-admin-auth is active: admin API authentication is DISABLED")`. Verify the flag is actually copied from CLI args to config (deviation report says it isn't — check `cli.rs` → config propagation).

### 2.7 — Dead code: public_key None bypass

**File:** `riversd/src/server.rs`
**Change:** Lines 909-912 are dead code (startup validates public_key is present via `validate_admin_access_control`). Replace the `None` arm with an unreachable panic or remove the match entirely — the startup validation guarantees `Some`.

### Traps

- Body consumption in middleware: Axum's body can only be consumed once. After hashing, the bytes must be put back. The pattern above using `Request::from_parts` handles this, but the body limit layer (outermost) must have already run, or the middleware needs its own size check.
- `riversctl` admin client (Category 4 task 36.4) must be updated to match the new timestamp unit and body hash. This is a separate binary but any existing admin scripts will break.

---

## Wave 3: DataView Pipeline

**Depends on:** Wave 0 (StorageEngine for L2 cache)
**Estimated scope:** ~200 lines new, ~60 lines modified

### 3.1 — Add cache to DataViewExecutor

**File:** `rivers-data/src/dataview_engine.rs`
**Change:** Add `cache: Option<Arc<dyn DataViewCache>>` to `DataViewExecutor`. Update constructor to accept optional cache.

### 3.2 — Wire cache check and populate in execute()

**File:** `rivers-data/src/dataview_engine.rs` — `execute()`
**Change:** After step 2 (parameter validation), before step 4 (driver connect):

```
// Step 3: Cache check
if let Some(ref cache) = self.cache {
    let key = compute_cache_key(name, &params);
    match cache.get(&key) {
        Ok(Some(cached)) => return Ok(build_response(cached, start, true, trace_id)),
        Ok(None) => { /* cache miss, continue */ }
        Err(e) => tracing::warn!("cache get error: {e}"),
    }
}
```

After step 6 (execute query), before returning:

```
// Step 8: Cache populate
if let Some(ref cache) = self.cache {
    let key = compute_cache_key(name, &params);
    if let Err(e) = cache.set(&key, &query_result) {
        tracing::warn!("cache set error: {e}");
    }
}
```

### 3.3 — Fix DataViewCache return types

**File:** `rivers-data/src/tiered_cache.rs`
**Change:** `get()` returns `Result<Option<QueryResult>, DataViewError>` (fallible). `set()` returns `Result<(), DataViewError>` (fallible). Remove silent error swallowing.

### 3.4 — Collapse l2_max_value_bytes to spec value

**File:** `rivers-data/src/tiered_cache.rs` and `rivers-data/src/dataview.rs`
**Change:** Unify to 131072 (128KB) per spec. Remove the 524288 and 65536 values.

### 3.5 — Wire schema validation at steps 2 and 7

**File:** `rivers-data/src/dataview_engine.rs` — `execute()`
**Change:** Step 2: call `validate_fields()` on input parameters against the DataView's input schema. Step 7: if `validate_result = true` on the DataView config, call `validate_fields()` on the result against `return_schema`.

### 3.6 — Add missing DataViewError variants

**File:** `rivers-data/src/dataview_engine.rs`
**Change:** Add `UnsupportedSchemaAttribute`, `SchemaFileNotFound`, `SchemaFileParseError`, `UnknownFakerMethod` variants to `DataViewError`.

### 3.7 — Remove redact_error() (SHAPE-4 violation)

**File:** `rivers-data/src/dataview_engine.rs`
**Change:** Delete `redact_error()` function (lines ~292-309). Replace call sites with direct error string passthrough.

### 3.8 — Wire TieredDataViewCache in startup

**File:** `riversd/src/server.rs` — bundle loading section
**Change:** When constructing `DataViewExecutor`, if StorageEngine is available and caching is configured, create `TieredDataViewCache` with L1 (moka) and L2 (StorageEngine) and pass to executor constructor.

### 3.9 — Fix DataViewParameterConfig param_type

**File:** `rivers-data/src/dataview.rs`
**Change:** Replace `param_type: String` with `param_type: DataViewParameterType` enum. Define the enum with variants matching spec.

### Traps

- `invalidate()` was added to the cache trait (an Addition in the deviation report). Keep it — it's useful and the spec can be amended.
- The cache key function `compute_cache_key` in `tiered_cache.rs` uses the correct algorithm (SHAPE-3 canonical JSON) but is never called. Don't rewrite it — just call it.

---

## Wave 4: Streaming Subsystems

**Depends on:** Wave 0.5 (EventBus) for SSE event-driven mode. SSE polling mode and WS are independent.
**Three parallel tracks.**

### 4A: SSE Push Loop

**Estimated scope:** ~120 lines replaced

#### 4A.1 — Replace heartbeat stub with real execution

**File:** `riversd/src/sse.rs` — `drive_sse_push_loop()`
**Change:** Replace hardcoded heartbeat (lines 393-424) with:

```
1. On tick: execute DataView via DataViewExecutor
2. Compute diff against previous result (hash comparison or full diff per strategy)
3. If changed: push data as SSE event to channel
4. Store current result as previous state
5. If no change: push heartbeat (keep-alive)
```

Requires: DataViewExecutor reference, StorageEngine for prev state (if persistence across restarts is needed), diff strategy from view config.

#### 4A.2 — Wire Last-Event-ID reconnection

**File:** `riversd/src/sse.rs`
**Change:** `events_since()` is implemented but never called. On client reconnection with `Last-Event-ID` header, call `events_since()` to replay missed events before entering the live push loop. Requires an event buffer (bounded ring buffer or StorageEngine-backed).

#### 4A.3 — Fix session_expired format

**File:** `riversd/src/sse.rs`
**Change:** Line ~108-110: Replace `{"rivers_session_expired":true}` with `event: session_expired\ndata: {"code": 4401, "reason": "session expired"}`.

### 4B: Streaming REST — Callback-Channel (DECISION-2)

**Estimated scope:** ~200 lines new, ~100 lines replaced
**Spec amendment required:** AsyncGenerator → `Rivers.stream.push()` callback API.

#### 4B.1 — Register Rivers.stream.push() host function

**File:** `riversd/src/process_pool.rs` — `inject_rivers_global()`
**Change:** Add `Rivers.stream` object with `push(chunk)` method. The host function:

```rust
// Create mpsc channel before task dispatch
let (tx, rx) = mpsc::channel::<serde_json::Value>(64);

// Host function closure captures tx
|scope, args, mut rv| {
    let chunk = v8_to_json(scope, args.get(0));
    let _ = tx.blocking_send(chunk); // non-blocking from V8's perspective
}
```

The channel sender is stored in a thread-local alongside the task context.

#### 4B.2 — Replace single-dispatch with channel-driven streaming

**File:** `riversd/src/streaming.rs` — `run_streaming_generator()`
**Change:** Replace lines 181-235:

```
1. Create mpsc channel (tx, rx)
2. Store tx in thread-local for Rivers.stream.push() to find
3. Spawn task dispatch on ProcessPool (handler calls push() N times)
4. Read from rx in a loop, sending each chunk to the HTTP response stream
5. Handler return = stream complete
6. Timeout applies to the entire stream, not individual chunks
```

#### 4B.3 — Add poison chunk error_type field

**File:** `riversd/src/streaming.rs`
**Change:** Lines ~84-96: Add `error_type` field to both NDJSON and SSE poison chunk formats per spec.

#### 4B.4 — Set streaming response headers

**File:** `riversd/src/streaming.rs`
**Change:** On streaming responses, set `Cache-Control: no-cache, no-store` and `X-Accel-Buffering: no`.

#### 4B.5 — Implement remaining validation rules

**File:** `riversd/src/streaming.rs`
**Change:** Lines ~136-171: implement the three missing validation rules (six total, three present).

### 4C: WebSocket Frame Loop

**Estimated scope:** ~250 lines new

#### 4C.1 — Implement upgrade handler

**File:** `riversd/src/websocket.rs`
**Change:** Add `pub async fn ws_upgrade_handler()` that:

```
1. Upgrade HTTP connection to WebSocket via axum::extract::ws::WebSocketUpgrade
2. Register connection in WebSocketRegistry
3. Spawn reader task (frame loop)
4. Spawn writer task (drain from broadcast channel)
```

#### 4C.2 — Implement frame read loop

**File:** `riversd/src/websocket.rs`
**Change:** Reader task:

```
loop {
    match ws_stream.next().await {
        Some(Ok(Message::Text(text))) => dispatch to on_stream handler
        Some(Ok(Message::Ping(data))) => send Pong
        Some(Ok(Message::Close(_))) => unregister, break
        Some(Ok(Message::Binary(_))) => record via BinaryFrameTracker (SHAPE-13)
        Some(Err(e)) => log error, break
        None => break
    }
}
```

#### 4C.3 — Wire WebSocket route registration

**File:** `riversd/src/server.rs`
**Change:** In view dispatch, detect `view_type = "WebSocket"` and route to `ws_upgrade_handler` instead of `execute_rest_view`.

#### 4C.4 — Session revalidation rejection for REST views

**File:** `riversd/src/view_engine.rs` — `validate_views()`
**Change:** If `session_revalidation_interval_s` is set on a REST view, return validation error.

### Traps (Wave 4)

- **SSE:** The push loop needs both DataViewExecutor and ProcessPool references. Currently `drive_sse_push_loop` takes only an `SseChannel` and `view_id`. Signature change cascades to all callers.
- **Streaming REST:** `mpsc::Sender::blocking_send()` blocks the current thread. Inside V8 this is fine (V8 is synchronous within a task), but verify it doesn't deadlock the tokio runtime. Use `try_send()` with backpressure as fallback.
- **WebSocket:** `tokio-tungstenite` vs `axum::extract::ws` — Axum has built-in WebSocket support. Use Axum's, not tungstenite directly. The existing `WebSocketRegistry` and `BroadcastChannel` types are compatible.

---

## Wave 5: ProcessPool Hardening

**Depends on:** Wave 0 (StorageEngine for ctx.store)
**Estimated scope:** ~150 lines modified

### 5.1 — Replace hashPassword SHA-256 with bcrypt

**File:** `riversd/src/process_pool.rs` — lines ~1899-1916
**Change:** Replace `sha2::Sha256` with `bcrypt::hash(password, 12)`. This is a blocking operation — wrap in `tokio::task::spawn_blocking` or accept the block since V8 dispatch is already on a blocking thread. Prefix changes from `sha256:` to `$2b$` (standard bcrypt prefix).

`verifyPassword` (lines ~1924-1927): replace SHA-256 comparison with `bcrypt::verify(password, hash)`.

### 5.2 — HMAC via LockBox alias

**File:** `riversd/src/process_pool.rs` — lines ~1978-2010
**Change:** First argument to `Rivers.crypto.hmac()` changes from raw key to LockBox alias string. Host function resolves alias via `LockBoxResolver::get()`, decrypts on host side, uses for HMAC, zeroizes. Raw key never enters V8.

Requires: `LockBoxResolver` reference accessible from the host function. Store in a thread-local or pass through TaskContext.

### 5.3 — Wire ctx.store to StorageEngine

**File:** `riversd/src/process_pool.rs`
**Change:** The `TASK_STORAGE` thread-local (line 59) is never populated with a real backend. In task dispatch, if StorageEngine is available in AppContext, clone the Arc and set it in the thread-local before execution. Clear after execution.

### 5.4 — Per-pool watchdog thread (not per-task)

**File:** `riversd/src/process_pool.rs`
**Change:** Lines ~1044-1057: Replace per-task watchdog thread spawn with a single per-pool watchdog thread that scans all active workers on a timer. Reduces thread count from N-tasks to N-pools.

### 5.5 — Worker crash recovery

**File:** `riversd/src/process_pool.rs`
**Change:** Lines ~1186-1189: When a `spawn_blocking` task panics, detect the failure, spawn a replacement worker, emit `WorkerCrash` event to EventBus, and if crash rate exceeds threshold, emit `WorkerPoolDegraded`.

### 5.6 — Fix max_heap_mb config key

**File:** `rivers-core/src/config.rs`
**Change:** Rename `max_heap_bytes` → `max_heap_mb` in the config struct. Add `recycle_after_tasks` field. Update deserialization.

### 5.7 — V8 heap limit enforcement (D3: Default+Guards)

**File:** `riversd/src/process_pool.rs`
**Change:** Set V8 heap limit callback using `isolate.set_heap_limit()` based on `max_heap_mb * 1024 * 1024`. On limit exceeded, terminate execution. This is the "guards" part of Decision 3.

### Traps

- bcrypt is CPU-intensive (~100ms at cost factor 12). If handlers call `hashPassword` in a hot path, this will dominate latency. Document this in handler API docs.
- LockBox resolver access from host functions: the thread-local pattern works but adds another thread-local alongside `TASK_STORAGE` and the stream channel sender. Consider a single `TASK_CONTEXT` thread-local struct that holds all of these.

---

## Wave 6: Format Alignment + SHAPE Cleanup

**Depends on:** Nothing (can run in parallel with anything)
**Estimated scope:** ~200 lines modified across many files

### 6.1 — Error response envelope: flatten

**File:** `riversd/src/error_response.rs`
**Change:** Remove `ErrorResponse` wrapper struct. `ErrorBody` becomes the top-level serialized type (flat `{code, message, details?, trace_id}`). Rename `ErrorBody` → `ErrorResponse`. Update all callers — grep for `ErrorResponse::new`.

### 6.2 — Timeout status code: 504 → 408

**File:** `riversd/src/middleware.rs`
**Change:** Line ~87: `StatusCode::GATEWAY_TIMEOUT` → `StatusCode::REQUEST_TIMEOUT`.

### 6.3 — CSP: remove unconditional injection

**File:** `riversd/src/middleware.rs`
**Change:** Lines 140-143: Remove `content-security-policy` header injection. Per spec, this is the operator's responsibility.

### 6.4 — Rate limit defaults

**File:** `riversd/src/rate_limit.rs`
**Change:** Lines 41-47: `requests_per_minute: 600` → `120`, `burst_size: 50` → `60`.

### 6.5 — Rate limit config location

**File:** `rivers-core/src/config.rs`
**Change:** Move rate limit fields from `SecurityConfig` to `[app.rate_limit]` in app config. Add `session` strategy variant to `RateLimitStrategy` enum.

### 6.6 — CORS config location

**File:** `rivers-core/src/config.rs`
**Change:** Move CORS fields out of `SecurityConfig` per SHAPE-23/24. CORS becomes a standalone config section.

### 6.7 — Remove parallel field (SHAPE-12)

**File:** `rivers-data/src/view.rs`
**Change:** Line 198: Remove `pub parallel: bool` from `HandlerStageConfig`.

### 6.8 — Fix ViewContext field names

**File:** `riversd/src/view_engine.rs`
**Change:** `query` → `query_params`, `params` → `path_params` in `ParsedRequest`. Update all references.

### 6.9 — Fix handler stage names

**File:** `rivers-data/src/view.rs`
**Change:** If sticking with the collapsed four-stage model (pre_process, handlers, post_process, on_error), document as intentional deviation. If expanding to six stages, add `on_request`, `transform`, `on_response` back.

**Decision needed:** This is a spec-vs-implementation alignment question. The collapsed model is simpler and matches the "find the simplest way" principle. Recommend: keep four stages, amend spec.

### 6.10 — LockBox keystore format unification

**File:** `rivers-lockbox/src/main.rs`
**Change:** Rewrite CLI to use the same `.rkeystore` single-file TOML-in-Age format as the runtime (`rivers-core/src/lockbox.rs`). This is the larger change — the CLI currently uses per-entry `.age` files + `aliases.json`. The runtime format is the canonical one. CLI adapts to it.

Also: add `--type` flag, confirmation prompts, exit codes, `unalias` subcommand, and `validate --config` per spec.

### 6.11 — Fix BrokerConsumerBridge SHAPE-18 violation

**File:** `riversd/src/broker_bridge.rs`
**Change:** Remove `storage: Option<Arc<dyn StorageEngine>>` field (line 41). Remove all storage buffering code (lines ~97-100, 259-276). Broker messages go directly to EventBus as spec'd.

### 6.12 — Bundle file format: JSON vs TOML

**File:** `rivers-data/src/bundle.rs`, `rivers-data/src/loader.rs`
**Change:** Spec says `manifest.json` and `resources.json`. Code uses TOML exclusively. Two options: (a) amend spec to TOML, or (b) change code to JSON. TOML is already working and is more human-friendly for config files. **Recommend: amend spec to TOML.**

### 6.13 — Handler type value

**File:** `rivers-data/src/view.rs`
**Change:** `"dataview"` → `"data_view"` per spec. Or amend spec to match code. **Recommend: amend spec — `dataview` is cleaner.**

### 6.14 — Cache config key

**File:** `rivers-data/src/dataview.rs`
**Change:** `caching` → `cache` per spec. Line ~150.

### 6.15 — Address book view paths

**File:** `address-book-service/app.toml`
**Change:** `"contacts"` → `"/api/contacts"`, `"contacts/{id}"` → `"/api/contacts/{id}"`.

---

## Spec Amendments Required

These decisions and deviations require spec document updates:

| ID | Spec File | Change |
|----|-----------|--------|
| AMD-19 | rivers-httpd-spec | Session removed from middleware stack; per-view at dispatch (D1) |
| AMD-20 | rivers-auth-session-spec | Session validation is per-view, not global middleware |
| AMD-21 | rivers-streaming-rest-spec | AsyncGenerator → `Rivers.stream.push()` callback API (D2) |
| AMD-22 | rivers-processpool-runtime-spec | V8 isolation deferred; Default+Guards for v1 (D3) |
| AMD-23 | rivers-httpd-spec | Handler pipeline: four stages canonical (not six) |
| AMD-24 | rivers-application-spec | Bundle format: TOML canonical (not JSON) |
| AMD-25 | rivers-view-layer-spec | `handler type = "dataview"` (no underscore) |
| AMD-26 | rivers-logging-spec | `Trace` level added (five levels, not four) |
| AMD-27 | rivers-auth-session-spec | Admin timestamp unit: milliseconds (verify spec and code agree after fix) |

---

## Execution Order Summary

| Wave | Blocked By | Estimated Scope | Priority |
|------|-----------|-----------------|----------|
| **0: StorageEngine** | Nothing | ~150 new, ~30 modified | **P0 — do first** |
| **0.5: EventBus + Logging** | Nothing | ~40 new, ~10 modified | **P0 — do second** |
| **1: Session Chain** | Wave 0 | ~300 new, ~100 modified | **P1 — security** |
| **2: Admin Security** | Nothing | ~80 modified | **P1 — security** |
| **3: DataView Pipeline** | Wave 0 | ~200 new, ~60 modified | **P2 — data integrity** |
| **4A: SSE** | Wave 0.5 | ~120 replaced | **P2 — functionality** |
| **4B: Streaming REST** | Nothing | ~200 new, ~100 replaced | **P2 — functionality** |
| **4C: WebSocket** | Nothing | ~250 new | **P3 — functionality** |
| **5: ProcessPool** | Wave 0 | ~150 modified | **P2 — security hardening** |
| **6: Format Alignment** | Nothing | ~200 modified | **P3 — cleanup** |
| **Spec Amendments** | Decisions locked | 9 amendments | **P1 — do alongside code** |

**Critical path:** Wave 0 → Wave 1 (session chain is the longest dependency chain and the highest-impact security gap).

**Parallel work:** Waves 2, 4B, 4C, and 6 can start immediately — they have no dependencies on Wave 0.
