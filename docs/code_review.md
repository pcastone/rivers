# Rivers Code Review

Date: 2026-04-24

Scope: Rust production code under `crates/`, excluding test-only code, build scripts, generated/vendor code, TOML/JSON config, and Markdown docs.

Grounding:
- Confirmed from source: root `Cargo.toml` workspace membership, workspace-wide `rg` sweeps for panic paths, unsafe blocks, discarded errors, locks, casts, spawns, unbounded collections/channels, SQL construction, and timeout coverage.
- Every finding below is grounded in source read around the cited lines.
- Crates marked "No issues found" were covered by workspace sweeps and targeted reads where a hit appeared; they were not all read line-by-line.

Workspace crates reviewed: `rivers-core-config`, `rivers-core`, `rivers-drivers-builtin`, `rivers-storage-backends`, `rivers-lockbox-engine`, `rivers-keystore-engine`, `rivers-driver-sdk`, `rivers-runtime`, `riversd`, all `rivers-plugin-*` driver crates, `rivers-engine-sdk`, `rivers-engine-v8`, `rivers-engine-wasm`, `rivers-lockbox`, `rivers-keystore`, `riversctl`, `riverpackage`, and `cargo-deploy`.

## riversd

### riversd — T1-1: Protected views fail open when session management is absent

**File:** `crates/riversd/src/security_pipeline.rs:50`
**Severity:** Tier 1
**Category:** Security / logic error

**What:** A non-public view only validates sessions when `ctx.session_manager` exists; if it is absent, execution continues.

**Why it matters:** A production wiring/config error can turn protected routes into public routes.

**Code:**
```rust
if !view.public {
    if let Some(session_manager) = &ctx.session_manager {
        // validation...
    }
}

Ok(SecurityDecision::Allow { context: SecurityContext { ... } })
```

**Fix direction:** Fail closed when `!view.public && ctx.session_manager.is_none()`.

### riversd — T1-2: Static file root check is bypassable with symlinks

**File:** `crates/riversd/src/static_files.rs:50`
**Severity:** Tier 1
**Category:** Security / TOCTOU file handling

**What:** The traversal guard checks the syntactic joined path before `metadata()` and file read follow symlinks.

**Why it matters:** A symlink inside the static root can expose files outside the bundle directory.

**Code:**
```rust
let root = Path::new(static_root);
let resolved = root.join(&sanitized);

if !resolved.starts_with(root) {
    return Err(StatusCode::FORBIDDEN);
}

let metadata = tokio::fs::metadata(&resolved).await?;
```

**Fix direction:** Canonicalize both root and resolved path after symlink resolution, then enforce containment before serving.

### riversd — T1-3: `ctx.ddl()` is injected into every V8 task context

**File:** `crates/riversd/src/process_pool/v8_engine/context.rs:121`
**Severity:** Tier 1
**Category:** Security / capability gating

**What:** `ctx.ddl` is installed unconditionally for V8 handlers even though the comment says it is for init handlers only.

**Why it matters:** Normal request, security, validation, or event handlers can reach DDL execution if they have a datasource context.

**Code:**
```rust
// ctx.ddl(datasource, statement) — init handlers only
let ddl_tmpl = v8::FunctionTemplate::new(scope, ctx_ddl_callback);
let ddl_val = ddl_tmpl.get_function(scope).unwrap();
ctx_obj.set(scope, v8_str_safe(scope, "ddl").into(), ddl_val.into());
```

**Fix direction:** Bind a task kind/capability into `TaskContext` and inject or execute DDL only for authorized application-init tasks.

### riversd — T1-4: V8-local `ctx.ddl()` bypasses the DDL whitelist path

**File:** `crates/riversd/src/process_pool/v8_engine/context.rs:543`
**Severity:** Tier 1
**Category:** Security / datasource wiring

**What:** The V8-local DDL callback resolves datasource params and calls `conn.ddl_execute()` directly instead of using the whitelist-enforced host/DataView path.

**Why it matters:** DDL authorization can be bypassed from the in-process V8 execution path.

**Code:**
```rust
let mut conn = factory.connect(&driver_name, &ds_params).await
    .map_err(|e| format!("DDL connect failed: {e}"))?;
let query = rivers_runtime::rivers_driver_sdk::Query::new("ddl", &statement);
conn.ddl_execute(&query).await
    .map_err(|e| format!("DDL execute failed: {e}"))?;
```

**Fix direction:** Route all DDL through one host-enforced path that checks task phase, app identity, datasource identity, and whitelist before execution.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** `ctx_ddl_callback` in `crates/riversd/src/process_pool/v8_engine/context.rs` now resolves the per-app `DDL_WHITELIST` and gates each statement through `is_ddl_permitted(database, app_id, &whitelist)` before reaching `conn.ddl_execute()` — the same authorization check the dynamic-engine `host_ddl_execute` path uses, so V8 init handlers can no longer bypass per-app/per-database allowlists. Negative coverage in `crates/riversd/tests/v8_ddl_whitelist_tests.rs` asserts that an unwhitelisted database produces the same `DDL operation not permitted` error operators see on the dyn-engine path.

### riversd — T1-5: Persistent `ctx.store` failures are masked with task-local memory

**File:** `crates/riversd/src/process_pool/v8_engine/context.rs:429`
**Severity:** Tier 1
**Category:** Error swallowing / data loss

**What:** When a configured storage backend fails, `ctx.store.set` warns and writes to `TASK_STORE` instead.

**Why it matters:** Handlers can report success while session/cache/message state is only stored in ephemeral per-task memory.

**Code:**
```rust
if let Err(e) = rt.block_on(engine.set(&namespace, &key, bytes, ttl_ms)) {
    tracing::warn!(error = %e, "storage set failed; falling back to task-local store");
    fallback = true;
}
if fallback {
    TASK_STORE.with(|s| {
        s.borrow_mut().insert(key, value);
    });
}
```

**Fix direction:** If a storage engine is configured, surface backend failures to JavaScript instead of falling back.

### riversd — T1-6: Synchronous V8 host bridges can block forever

**File:** `crates/riversd/src/process_pool/v8_engine/context.rs:547`
**Severity:** Tier 1
**Category:** Missing timeouts / resource leak

**What:** `ctx.ddl()` spawns async driver work, then blocks the V8 worker on `rx.recv()` with no timeout or cancellation.

**Why it matters:** A hung datasource connect or DDL call can pin a V8 worker indefinitely.

**Code:**
```rust
rt.spawn(async move {
    let result = async { /* connect + ddl */ }.await;
    let _ = tx.send(result);
});

match rx.recv() {
    Ok(Ok(())) => { /* success */ }
```

**Fix direction:** Use a bounded timeout and propagate cancellation/failure back to the handler.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** Every blocking host-bridge `recv()` in `crates/riversd/src/process_pool/v8_engine/context.rs` (DDL, DataView, transaction, store, log paths) now uses `recv_timeout(HOST_CALLBACK_TIMEOUT_MS)` (30 s) and converts the timeout into a JS-visible error naming the host-callback that stalled, so a hung driver/pool no longer pins a V8 worker indefinitely. The spawned async task is detached but its result is dropped on timeout; the worker is reclaimed within the configured task budget.

### riversd — T2-1: Handler response status truncates from `u64` to `u16`

**File:** `crates/riversd/src/view_engine/validation.rs:270`
**Severity:** Tier 2
**Category:** Integer truncation

**What:** JavaScript handler status values are cast with `as u16` without range validation.

**Why it matters:** A handler returning `65536`, `70000`, or another invalid status can wrap into a different response status.

**Code:**
```rust
let status = value.get("status")?.as_u64()? as u16;
```

**Fix direction:** Parse the status through an HTTP status type or reject values outside `100..=599`.

### riversd — T2-2: Connection limits race in WebSocket and SSE registries

**File:** `crates/riversd/src/websocket.rs:104`
**Severity:** Tier 2
**Category:** Race condition / unbounded growth

**What:** The max-connection check happens before insertion and increment, so concurrent registrations can all pass the limit.

**Why it matters:** Connection limits can be exceeded under a burst, weakening DoS protection.

**Code:**
```rust
if self.connection_count.load(Ordering::Relaxed) >= max {
    return Err(WebSocketError::ConnectionLimitExceeded);
}
let mut connections = self.connections.write().await;
connections.insert(connection_id.clone(), connection);
self.connection_count.fetch_add(1, Ordering::Relaxed);
```

**Fix direction:** Reserve capacity atomically or perform the check and insertion under one synchronized state update.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** Both `crates/riversd/src/websocket.rs` and `crates/riversd/src/sse.rs` registries now reserve a slot via `connection_count.fetch_add(1, Ordering::SeqCst)` and compare the resulting count against `max_connections` in a single step — on overflow the increment is reverted with `fetch_sub` before returning `ConnectionLimitExceeded`, eliminating the check-then-insert race. Burst-of-200 concurrent connect tests with a configured cap of 50 now reject exactly 150 attempts.

### riversd — T2-3: Pool lifetime resets on every return

**File:** `crates/riversd/src/pool.rs:125`
**Severity:** Tier 2
**Category:** Connection pool / resource lifetime

**What:** Returning a connection creates a new `PooledConnection` with `created_at: Instant::now()`.

**Why it matters:** `max_lifetime` is effectively extended on every checkout, so old connections may never age out.

**Code:**
```rust
self.pool.return_connection(PooledConnection {
    conn,
    created_at: Instant::now(),
    last_used: Instant::now(),
    use_count: self.use_count,
});
```

**Fix direction:** Preserve the original creation timestamp in the guard and return it unchanged.

### riversd — T2-4: Pool capacity accounting ignores the return queue

**Resolved 2026-04-25 by Phase D commit `2dfbb7b` (D1)** — verified Phase H (H16). The pool now keeps a single `Arc<StdMutex<PoolState>>` holding both `idle: VecDeque<PooledConnection>` and a unified `total: usize` counter (idle + active + in-flight create reservations). `acquire`, `PoolGuard::drop`, `PoolGuard::take`, `health_check`, and `drain` all mutate `total` under the same lock, so capacity checks see every release path. The dual-counter / `idle_return` shape this finding cited no longer exists.

**File:** `crates/riversd/src/pool.rs:517`
**Severity:** Tier 2
**Category:** Connection pool / race condition

**What:** `acquire()` counts active and idle connections but not connections waiting in `idle_return`.

**Why it matters:** Returned connections can be invisible during a burst, allowing over-creation beyond `max_connections`.

**Code:**
```rust
let total = self.active_count.load(Ordering::SeqCst) + self.idle.lock().await.len();
if total < self.config.max_connections {
    return self.create_connection().await;
}
```

**Fix direction:** Include the return queue in accounting or collapse pool state into one synchronized structure.

### riversd — T2-5: Pool health checks hold the idle mutex across network I/O

**Resolved 2026-04-25 by Phase D commit `2dfbb7b` (D1)** — verified Phase H (H17). `ConnectionPool::health_check` (current `crates/riversd/src/pool.rs:717-768`) drains the idle queue under the state lock via `std::mem::take(&mut state.idle)`, drops the lock at the closure end, calls `pooled.conn.ping().await` with no lock held, then re-acquires the lock to re-insert healthy entries and decrement `total`. The state lock is `std::sync::Mutex` (not `tokio::Mutex`), so holding it across `.await` is structurally prevented at compile time.

**File:** `crates/riversd/src/pool.rs:668`
**Severity:** Tier 2
**Category:** Async lock held across `.await`

**What:** `health_check()` holds the idle connection mutex while awaiting each `ping()`.

**Why it matters:** A slow or hung ping blocks other tasks from acquiring or returning idle connections.

**Code:**
```rust
let mut idle = self.idle.lock().await;
while let Some(mut pooled) = idle.pop_front() {
    match pooled.conn.ping().await {
        Ok(_) => { healthy.push_back(pooled); }
```

**Fix direction:** Drain the idle queue, drop the lock before pings, then reacquire it to return healthy connections.

### riversd — T2-6: Outbound HTTP callbacks have no timeout

**File:** `crates/riversd/src/process_pool/v8_engine/http.rs:130`
**Severity:** Tier 2
**Category:** Missing timeouts

**What:** `Rivers.http.*` creates a default `reqwest::Client` and awaits `send()`/`text()` without a timeout.

**Why it matters:** A slow remote server can hold a V8 worker and request path indefinitely.

**Code:**
```rust
rt.block_on(async {
    let client = reqwest::Client::new();
    let mut builder = match method { /* ... */ };
    let resp = builder.send().await.map_err(|e| e.to_string())?;
    let body_text = resp.text().await.map_err(|e| e.to_string())?;
```

**Fix direction:** Use a configured client with connect/read/request timeouts and bounded response size.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** `Rivers.http.*` in `crates/riversd/src/process_pool/v8_engine/http.rs` now builds the `reqwest::Client` via a shared helper with `.connect_timeout(...)`, `.timeout(...)`, and a bounded response-size policy; per-call overrides are accepted from the JS handler's optional `timeout` field. A handler fetching a black-hole address (TEST-NET-3 `203.0.113.1`) now returns a timeout error within budget instead of pinning the V8 worker.

### riversd — T2-7: Dynamic engine host HTTP callback also lacks a timeout

**File:** `crates/riversd/src/engine_loader/host_callbacks.rs:456`
**Severity:** Tier 2
**Category:** Missing timeouts

**What:** The cdylib host HTTP callback awaits `req.send()` and `resp.text()` on a default client with no request timeout.

**Why it matters:** A dynamic engine HTTP call can block the engine callback bridge indefinitely.

**Code:**
```rust
let resp = req.send().await.map_err(|e| e.to_string())?;
let status = resp.status().as_u16();
let resp_body = resp.text().await.map_err(|e| e.to_string())?;
```

**Fix direction:** Build the host HTTP client with explicit timeout and body-size limits.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** The cdylib host HTTP path in `crates/riversd/src/engine_loader/host_callbacks.rs` now consumes the same shared `reqwest::Client` builder used by H6 (configured connect/read/request timeouts plus body-size cap), so static and dynamic engines have identical outbound-HTTP timeout policy. WASM-engine fetch tests against an unresponsive endpoint surface a timeout error rather than blocking the engine callback bridge.

### riversd — T2-8: Transaction host callbacks are success-returning stubs

**File:** `crates/riversd/src/engine_loader/host_callbacks.rs:919`
**Severity:** Tier 2
**Category:** Datasource wiring / error swallowing

**What:** `db_begin`, `db_commit`, `db_rollback`, and `db_batch` return `{"ok": true}` without executing the operation.

**Why it matters:** Dynamic engine handlers can believe a transaction or batch operation succeeded when no transaction exists.

**Code:**
```rust
// TODO: Wire to TransactionMap in Task 8
tracing::debug!(datasource = %datasource, "Rivers.db.begin (stub)");
let result = serde_json::json!({"ok": true, "datasource": datasource});
write_output(out_ptr, out_len, &result);
0
```

**Fix direction:** Return an explicit unsupported error until the TransactionMap and batch execution are actually wired.

**Resolved 2026-04-25 by Phase I (this PR — branch `feature/phase-i-dyn-transactions`).** Phase I implemented the dyn-engine transaction map (`crates/riversd/src/engine_loader/dyn_transaction_map.rs::DynTransactionMap`), wired `host_db_begin`, `host_db_commit`, and `host_db_rollback` to begin/commit/rollback on real `Connection`s through the map (host_callbacks.rs:1062-1473), threaded `txn_conn` through `host_dataview_execute` via `execute_dataview_with_optional_txn` and `DynTransactionMap::with_conn_mut` (host_callbacks.rs:218-298), and added the `TaskGuard`-driven auto-rollback hook in `process_pool/mod.rs::dispatch_dyn_engine_task` (mod.rs:316-384). Mirrors V8 `ctx_transaction_callback` semantics including the financial-correctness commit-failure upgrade (`signal_commit_failed` → `TaskError::TransactionCommitFailed`) and the `HOST_CALLBACK_TIMEOUT_MS` (30s) budget on commit/rollback. The three `TODO: Wire to TransactionMap in Task 8` comments are removed; `host_db_batch` retains a clarifying comment that it is a DataView batch-execute primitive (each call its own transaction), not a transaction wrapper. Spec coverage in `docs/arch/rivers-data-layer-spec.md §6.8`. End-to-end coverage on real SQLite in `crates/riversd/src/process_pool/mod.rs::dyn_e2e_tests` (5 cases: commit persists, rollback discards, auto-rollback on engine error, cross-DS rejection, two-task isolation).

### riversd — T2-9: Engine log callback trusts UTF-8 with `from_utf8_unchecked`

**File:** `crates/riversd/src/engine_loader/host_callbacks.rs:496`
**Severity:** Tier 2
**Category:** Unsafe / FFI boundary

**What:** The host log callback converts engine-provided bytes to `&str` with `from_utf8_unchecked`.

**Why it matters:** A buggy or hostile dynamic engine can pass invalid UTF-8 and trigger undefined behavior in the host process.

**Code:**
```rust
let msg = unsafe {
    std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len))
};
```

**Fix direction:** Use `std::str::from_utf8` and log/return on invalid input.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** The host log callback in `crates/riversd/src/engine_loader/host_callbacks.rs` now decodes engine-supplied bytes via `std::str::from_utf8(...)`, logging a `warn!` and returning a non-zero status on invalid UTF-8 instead of constructing a `&str` through `from_utf8_unchecked`. A buggy or hostile dynamic engine can no longer trigger UB in the host process by passing malformed bytes.

### riversd — T3-1: Manual JSON log construction allows malformed app log lines

**File:** `crates/riversd/src/process_pool/v8_engine/rivers_global.rs:41`
**Severity:** Tier 3
**Category:** Serialization bug / log integrity

**What:** JavaScript-controlled log messages are interpolated into a JSON string without JSON escaping.

**Why it matters:** Quotes and control characters can corrupt per-app logs or spoof fields in downstream log processing.

**Code:**
```rust
let line = format!(
    r#"{{"timestamp":"{ts}","level":"{}","app":"{app_name}","trace_id":"{trace_id}","message":"{}"}}"#,
    level,
    msg.replace('\n', "\\n")
);
```

**Fix direction:** Serialize a structured log object with `serde_json` instead of hand-building JSON.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** `crates/riversd/src/process_pool/v8_engine/rivers_global.rs` now constructs each log line with `serde_json::json!({...}).to_string()` so quotes, control characters, and embedded newlines in JS-supplied messages or trace ids are properly escaped — handler-controlled input can no longer corrupt per-app log lines or spoof structured fields downstream.

## rivers-runtime

### rivers-runtime — T1-1: DataView execution bypasses connection pools and circuit state

**File:** `crates/rivers-runtime/src/dataview_engine.rs:720`
**Severity:** Tier 1
**Category:** Connection pool / resource exhaustion

**What:** The DataView hot path connects directly through `DriverFactory` for each execution.

**Why it matters:** Pool limits, reuse, health checks, and circuit-breaker-style connection controls do not protect the primary query path.

**Code:**
```rust
let mut conn = self.factory.connect(driver_name, ds_params).await
    .map_err(|e| DataViewError::ExecutionFailed { ... })?;

let prepared = conn.prepare(&view.query).await?;
let result = conn.execute(&prepared).await?;
```

**Fix direction:** Execute DataViews through datasource-scoped pooled connections with shared health/circuit accounting.

### rivers-runtime — T2-1: DataView request timeout is accepted but unused

**File:** `crates/rivers-runtime/src/dataview_engine.rs:149`
**Severity:** Tier 2
**Category:** Missing timeouts

**What:** `DataViewRequest.timeout_ms` is validated by the builder but not applied around connect, prepare, or execute.

**Why it matters:** A caller can configure a timeout and still hang on driver I/O.

**Code:**
```rust
pub struct DataViewRequest {
    pub timeout_ms: Option<u64>,
}

let mut conn = self.factory.connect(driver_name, ds_params).await?;
let prepared = conn.prepare(&view.query).await?;
let result = conn.execute(&prepared).await?;
```

**Fix direction:** Wrap the full connect/prepare/execute sequence in `tokio::time::timeout`.

### rivers-runtime — T2-2: Result schema validation silently disables itself on schema errors

**File:** `crates/rivers-runtime/src/dataview_engine.rs:1059`
**Severity:** Tier 2
**Category:** Error swallowing / serialization validation

**What:** If the configured schema file cannot be read or parsed, `validate_query_result()` returns `Ok(())`.

**Why it matters:** A broken or missing schema silently disables result validation for untrusted datasource output.

**Code:**
```rust
let schema_content = match std::fs::read_to_string(schema_path) {
    Ok(content) => content,
    Err(_) => return Ok(()),
};
let schema: serde_json::Value = match serde_json::from_str(&schema_content) {
    Ok(schema) => schema,
    Err(_) => return Ok(()),
};
```

**Fix direction:** Treat configured-but-unreadable or invalid schemas as validation errors.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** `validate_query_result()` in `crates/rivers-runtime/src/dataview_engine.rs` now surfaces both `std::fs::read_to_string` failures and `serde_json::from_str` failures as `DataViewError::SchemaInvalid` (was: `Ok(())` swallowing both) — a configured-but-unreadable or malformed schema now fails the request loudly instead of silently disabling validation for downstream datasource output.

## rivers-core

### rivers-core — T1-1: Plugin ABI function panics are not contained

**File:** `crates/rivers-core/src/driver_factory.rs:324`
**Severity:** Tier 1
**Category:** Plugin safety / unsafe FFI

**What:** `_rivers_abi_version` is called directly from an unsafe libloading symbol; only registration is wrapped in `catch_unwind`.

**Why it matters:** A buggy plugin that panics in the ABI-version function can unwind across FFI or abort the host process.

**Code:**
```rust
let abi_version = unsafe {
    let abi_fn: Symbol<unsafe extern "C" fn() -> u32> =
        lib.get(b"_rivers_abi_version")?;
    abi_fn()
};
```

**Fix direction:** Treat every plugin FFI call as hostile: wrap with unwind containment where possible and reject on panic.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** The `_rivers_abi_version` probe in `crates/rivers-core/src/driver_factory.rs` is now invoked through `std::panic::catch_unwind`, mirroring the existing protection on the registration call — a plugin that panics in its ABI-version function is rejected as `PluginLoadFailed("ABI probe panicked")` instead of unwinding across the FFI boundary and aborting the host process.

### rivers-core — T2-1: EventBus observe handlers spawn without backpressure

**File:** `crates/rivers-core/src/eventbus.rs:295`
**Severity:** Tier 2
**Category:** Unbounded growth / async task leak

**What:** Each `Observe` handler invocation gets a detached `tokio::spawn` with no concurrency limit or shutdown tracking.

**Why it matters:** An event storm can create unbounded background tasks and memory pressure.

**Code:**
```rust
HandlerPriority::Observe => {
    let handler = sub.handler.clone();
    let event_clone = event.clone();
    tokio::spawn(async move {
        if let Err(e) = handler.handle(&event_clone).await { ... }
    });
}
```

**Fix direction:** Run observers through a bounded worker queue or track and cancel them during shutdown.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** Observe-tier dispatch in `crates/rivers-core/src/eventbus.rs` now routes through a bounded `tokio::sync::Semaphore` (configurable per-bus capacity) and tracks every spawned handler in a `JoinSet`, so a flood of events can no longer create unbounded background tasks; on bus shutdown the `JoinSet` is awaited (with a deadline) before drop, ensuring observers don't leak past lifecycle. Detached `tokio::spawn` is gone from the dispatch path.

### rivers-core — T3-1: Wildcard EventBus subscribers can violate priority ordering

**File:** `crates/rivers-core/src/eventbus.rs:253`
**Severity:** Tier 3
**Category:** Logic error / event ordering

**What:** Exact subscribers are sorted by priority, then wildcard subscribers are appended afterward.

**Why it matters:** A wildcard `Expect` or `Handle` subscriber can run after exact lower-priority handlers.

**Code:**
```rust
if let Some(subs) = subs.get(&event.event_type) {
    subscribers.extend(subs.iter().cloned());
    subscribers.sort_by_key(|s| s.priority);
}
if let Some(wildcards) = subs.get("*") {
    subscribers.extend(wildcards.iter().cloned());
}
```

**Fix direction:** Merge exact and wildcard subscribers first, then sort the combined list.

## rivers-drivers-builtin

### rivers-drivers-builtin — T1-1: MySQL global pool cache ignores password

**File:** `crates/rivers-drivers-builtin/src/mysql.rs:39`
**Severity:** Tier 1
**Category:** Secret handling / connection pool isolation

**What:** MySQL pools are cached by host, port, database, and username, but not password.

**Why it matters:** Two datasources with the same user tuple but different passwords can reuse the first pool and authenticate as the wrong credential context.

**Code:**
```rust
// Pool cache key intentionally excludes password from logs/errors.
let pool_key = format!(
    "{}:{}/{}?u={}",
    params.host, params.port, params.database, params.username
);
```

**Fix direction:** Include a non-logged credential fingerprint or datasource identity in the pool key.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** The pool cache key in `crates/rivers-drivers-builtin/src/mysql.rs` now appends a SHA-256 fingerprint of the password (truncated to 16 hex chars) — `host:port/db?u=user&pwfp=<fp>` — so two datasources with the same `(host, port, database, username)` but different passwords no longer collide on the same pool. Raw password bytes are never logged or stored in the key. An auth-failure eviction path on the first checkout was added so a rotated password rebuilds the pool rather than retrying against the stale entry.

### rivers-drivers-builtin — T2-1: MySQL unsigned integers can wrap into negative values

**File:** `crates/rivers-drivers-builtin/src/mysql.rs:499`
**Severity:** Tier 2
**Category:** Integer truncation

**What:** `mysql_async::Value::UInt` is converted with `as i64`.

**Why it matters:** Values above `i64::MAX` silently wrap and corrupt query results.

**Code:**
```rust
Value::UInt(u) => QueryValue::Integer(*u as i64),
```

**Fix direction:** Represent oversized unsigned values as decimal strings or a JSON number type that preserves range.

**Resolved 2026-04-26 by Phase H18 — see commits on `feature/h18-mysql-uint`.** Added `QueryValue::UInt(u64)` variant in `rivers-driver-sdk` (commit `31a7d64`) with a custom `Serialize` impl that emits values ≤ `Number.MAX_SAFE_INTEGER` (2⁵³−1) as JSON numbers and larger values as JSON strings — same threshold rule applies to `QueryValue::Integer`. MySQL driver now emits `Value::UInt(u)` as `QueryValue::UInt(u)` (lossless) and binds `UInt` round-trip without truncation (commit `cfaca16`); 11 dependent crates rippled with explicit overflow handling on bind paths (Postgres/SQLite use `i64::try_from` and surface `DriverError::Connection`; MongoDB chains through `Decimal128`; InfluxDB emits the native `u`-suffixed line-protocol field). Live integration test `h18_mysql_uint_round_trip` against MySQL @ 192.168.2.215 verifies five representative values across the threshold (0, 42, 2⁵³−1, 2⁵³, 18_446_744_073_709_551_610) round-trip losslessly. Schema-spec coverage in `docs/arch/rivers-schema-spec-v2.md` §"Large integers and JSON precision". Decision rationale in `changedecisionlog.md::MYSQL-H18.1`.

### rivers-drivers-builtin — T2-2: PostgreSQL connection strings are built by interpolation

**File:** `crates/rivers-drivers-builtin/src/postgres.rs:44`
**Severity:** Tier 2
**Category:** Secret handling / connection parsing

**What:** User, password, database, and host are interpolated into a libpq-style connection string without escaping.

**Why it matters:** Spaces or option-like content in credentials can break parsing or alter connection parameters.

**Code:**
```rust
let conn_string = format!(
    "host={} port={} user={} password={} dbname={}",
    params.host, params.port, params.username, params.password, params.database
);
```

**Fix direction:** Build PostgreSQL connection configuration with structured parameters instead of a formatted string.

## rivers-storage-backends

### rivers-storage-backends — T2-1: Redis cluster `list_keys` uses blocking `KEYS`

**File:** `crates/rivers-storage-backends/src/redis_backend.rs:225`
**Severity:** Tier 2
**Category:** Unbounded growth / DoS

**What:** Cluster mode falls back to Redis `KEYS` for namespace listing.

**Why it matters:** `KEYS` can block Redis under a large keyspace and make storage operations a DoS vector.

**Code:**
```rust
let vals: Vec<String> = conn
    .keys(&pattern)
    .await
    .map_err(|e| StorageError::Backend(format!("redis cluster keys: {e}")))?;
```

**Fix direction:** Implement bounded per-node scan or avoid key listing for cluster-backed storage.

### rivers-storage-backends — T2-2: SQLite TTL arithmetic can overflow

**File:** `crates/rivers-storage-backends/src/sqlite_backend.rs:119`
**Severity:** Tier 2
**Category:** Integer overflow

**What:** Expiration timestamps are calculated as `now_ms() + ttl` without checked arithmetic.

**Why it matters:** A very large TTL can wrap the expiry time and make a value expire immediately or persist incorrectly.

**Code:**
```rust
let expires_at = ttl_ms.map(|ttl| now_ms() + ttl);
```

**Fix direction:** Use checked or saturating addition and reject TTLs above a configured maximum.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** Expiry computation in `crates/rivers-storage-backends/src/sqlite_backend.rs` now uses `now_ms().checked_add(ttl)` and rejects the operation with `StorageError::InvalidArgument("ttl_ms overflows expiry timestamp")` when the addition saturates — a malicious or buggy caller can no longer wrap the expiry into the past or persist a value with a corrupted timestamp.

## rivers-engine-v8

### rivers-engine-v8 — T2-1: Host callback table is copied with undocumented `ptr::read`

**File:** `crates/rivers-engine-v8/src/lib.rs:44`
**Severity:** Tier 2
**Category:** Unsafe / FFI boundary

**What:** `_rivers_engine_init_with_callbacks` copies `HostCallbacks` from a raw pointer with `std::ptr::read`, but the safety invariant is not documented and `HostCallbacks` is not marked `Copy`.

**Why it matters:** The struct currently contains function pointers, but future non-`Copy` fields would make this unsound at the ABI boundary.

**Code:**
```rust
pub extern "C" fn _rivers_engine_init_with_callbacks(callbacks: *const HostCallbacks) -> i32 {
    if !callbacks.is_null() {
        let cb = unsafe { std::ptr::read(callbacks) };
        let _ = HOST_CALLBACKS.set(cb);
    }
    _rivers_engine_init()
}
```

**Fix direction:** Make `HostCallbacks` explicitly `Copy + Clone`, or copy each function pointer field with documented safety.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** `HostCallbacks` in `crates/rivers-engine-sdk` is now declared `#[derive(Copy, Clone)]` and the `_rivers_engine_init_with_callbacks` entry point in `crates/rivers-engine-v8/src/lib.rs` carries a `// SAFETY:` doc-comment block stating the ABI invariant the host upholds (caller passes a fully-initialized `HostCallbacks` whose lifetime exceeds the engine). The `std::ptr::read` is preserved but is now sound by construction: any future non-`Copy` field would fail to compile here.

## rivers-engine-wasm

### rivers-engine-wasm — T2-1: WASM memory pointer math casts negative offsets to `usize`

**File:** `crates/rivers-engine-wasm/src/lib.rs:254`
**Severity:** Tier 2
**Category:** Integer truncation / host callback correctness

**What:** Host log callbacks cast guest `i32` pointers and lengths to `usize` before slicing memory.

**Why it matters:** Negative guest values become huge `usize` values; bounds checks prevent memory unsafety, but valid guest mistakes are silently ignored instead of trapped.

**Code:**
```rust
if let Some(slice) = data.get(ptr as usize..(ptr as usize + len as usize)) {
    let msg = String::from_utf8_lossy(slice);
    log_to_host(2, &msg);
}
```

**Fix direction:** Reject negative pointer or length values before casting and return a host-function error/trap.

**Resolved 2026-04-26 by PR #83 (sha `6ee5036`).** Host log/buffer callbacks in `crates/rivers-engine-wasm/src/lib.rs` now reject negative `ptr` or `len` values (returning a wasmtime trap with a clear "negative offset" message) before any `as usize` cast, so a buggy guest no longer has its mistakes silently coerced into huge `usize` offsets that pass bounds checks but reference the wrong bytes. Valid guest pointers continue through the existing `data.get(...)` slice path.

## rivers-engine-sdk

No issues found in this pass.

## rivers-core-config

No issues found in this pass.

## rivers-driver-sdk

No issues found in this pass.

## rivers-lockbox-engine

No issues found in this pass.

## rivers-keystore-engine

No issues found in this pass.

## rivers-plugin-cassandra

No issues found in this pass.

## rivers-plugin-couchdb

No issues found in this pass.

## rivers-plugin-elasticsearch

No issues found in this pass.

## rivers-plugin-exec

No issues found in this pass.

## rivers-plugin-influxdb

No issues found in this pass.

## rivers-plugin-kafka

No issues found in this pass.

## rivers-plugin-ldap

No issues found in this pass.

## rivers-plugin-mongodb

No issues found in this pass.

## rivers-plugin-nats

No issues found in this pass.

## rivers-plugin-neo4j

No issues found in this pass.

## rivers-plugin-rabbitmq

No issues found in this pass.

## rivers-plugin-redis-streams

No issues found in this pass.

## rivers-lockbox

No issues found in this pass.

## rivers-keystore

No issues found in this pass.

## riversctl

No issues found in this pass.

## riverpackage

No issues found in this pass.

## cargo-deploy

No issues found in this pass.
