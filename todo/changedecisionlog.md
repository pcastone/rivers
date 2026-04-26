# Filesystem Driver — Change Decision Log

Captures design decisions made during Phases 1–6 that deviate from the letter
of `rivers-filesystem-driver-spec.md` or that warrant explanation for future
readers.

---

### D1 — `mtime`/`atime`/`ctime` are epoch-seconds strings, not ISO-8601

**File:** `crates/rivers-drivers-builtin/src/filesystem.rs` (ops::stat)
**Spec reference:** §6.5
**Decision:** emit timestamps as epoch-seconds decimal strings instead of ISO-8601.
**Reason:** adding ISO-8601 formatting would require importing `chrono` or `time`
into `rivers-drivers-builtin`. The driver did not previously depend on either.
**Resolution:** the handler API contract treats timestamps as opaque strings, so
the wire shape is stable and an upgrade to ISO-8601 later is non-breaking. Tracked
as a Phase 6 follow-up item (Task 38).

---

### D2 — `QueryResult` uses `HashMap` rows, plan uses `Vec<Vec<QueryValue>>`

**File:** every `ops::*` function
**Spec reference:** N/A (plan artifact, not spec)
**Decision:** all operation impls and tests use the real `QueryResult` shape
(`rows: Vec<HashMap<String, QueryValue>>`, `column_names: Option<Vec<String>>`)
instead of the `Vec<Vec<QueryValue>>` + `columns: Vec<String>` shape the plan's
pseudocode assumed.
**Reason:** the plan was written before final `QueryResult` shape landed; the
actual type is keyed by column name, which is what all other drivers emit.
**Resolution:** adapted each test and impl in place during Phase 3. No spec deviation.

---

### D3 — Task 29 decomposed into 29a–29f

**File:** `todo/tasks.md`
**Spec reference:** plan hygiene
**Decision:** original Task 29 ("V8 codegen — detect Direct token") bundled five
cross-cutting concerns (thread-local plumbing, catalog lookup, host fn, JS codegen,
integration harness). Split into six focused sub-tasks, each independently reviewable.
**Reason:** scope too large for a single commit; each sub-task has its own TDD cycle
and validation.
**Resolution:** plan updated in place; 29a–29f executed sequentially.

---

### D4 — V8 typed proxy: template JS string (A), not native Functions (B)

**File:** `crates/riversd/src/process_pool/v8_engine/proxy_codegen.rs`
**Spec reference:** §3.1
**Decision:** emit a JS IIFE string per direct datasource, compiled once via
`v8::Script::compile` and stored on `__rivers_direct_proxies[name]`. Rejected the
alternative of building each method as a native `v8::Function::new` with a closure
over descriptor metadata.
**Reason:** template-string codegen is significantly simpler — the `proxy_codegen`
module is self-contained pure Rust (unit-testable without V8) and the emitted JS is
debuggable via `Script::compile` errors. Native `v8::Function::new` per method would
require a per-method trampoline and complicate argument marshaling.
**Resolution:** picked (A). 7 pure-Rust unit tests cover the emitted source; 10
integration tests (29e/29f) prove it round-trips through V8.

---

### D5 — V8 marshaling: Option-B auto-unwrap, not raw `QueryResult`

**File:** `crates/riversd/src/process_pool/v8_engine/direct_dispatch.rs::query_result_to_json`
**Spec reference:** §9 (handler API ergonomics)
**Decision:** marshal `QueryResult` to JS using shape-based unwrap rules:
  - 0 rows → `null`
  - 1 row × 1 column → scalar value
  - 1 row × N columns → object (the row)
  - N rows → array of row objects
**Reason:** preserves ergonomic JS — `readFile("x") === "world"`, `exists("x") === true`,
`readDir(".") === [{name}]`, `find("*.txt") === {results, truncated}`. Returning the
raw `QueryResult` shape would force every handler to write `result.rows[0].content`.
**Resolution:** confirmed by user. 4 unit tests lock the unwrap rules; integration
tests verify the shape at the handler level.

---

### D6 — `DatasourceToken::Direct` bridge serialization uses URL-style string

**File:** `crates/rivers-runtime/src/process_pool/bridge.rs`
**Spec reference:** §7.3
**Decision:** when converting `TaskContext → SerializedTaskContext`, `Direct` tokens
are encoded as `"direct://{driver}?root={path}"` into the existing `datasource_tokens:
HashMap<String, String>` field.
**Reason:** Phase 4 determined the V8 engine is statically linked (not a cdylib via
the `static-engines` feature), so no serialization crosses a process boundary today.
The string encoding is a placeholder that keeps the SDK type unchanged while the
typed-proxy path reads directly from `TASK_DIRECT_DATASOURCES` (a Rust-side
thread-local) instead of the serialized form.
**Resolution:** acceptable for v1 because nothing parses the serialized string. If
the WASM engine or a future cdylib path needs `Direct` tokens, extend
`SerializedTaskContext` with a structured `direct_datasources: HashMap<String,
SerializedDirectDatasource>` field at that point. Flagged as a latent follow-up.

---

### D7 — `datasource-filesystem.md` not `tutorial-filesystem-driver.md`

**File:** `docs/guide/tutorials/datasource-filesystem.md`
**Spec reference:** Task 34 in plan
**Decision:** named the tutorial `datasource-filesystem.md` instead of the plan's
`tutorial-filesystem-driver.md`.
**Reason:** every sibling driver tutorial in `docs/guide/tutorials/` uses the
`datasource-*.md` prefix. Following the existing convention improves discoverability.
**Resolution:** docs still cross-referenced from the feature inventory and the spec.

---

### H16 / H17 — Phase D pool rewrite closed both T2 pool findings

**File:** `crates/riversd/src/pool.rs`
**Spec reference:** `docs/code_review.md` Tier-2 findings T2-4 (capacity accounting) and T2-5 (health-check lock); Phase D commit `2dfbb7b` (D1)
**Decision:** close H16 and H17 without source changes after structural re-read.
**Reason:**
- T2-4: post-D1 the pool has a single `Arc<StdMutex<PoolState>>` (line 502) holding both `idle: VecDeque<PooledConnection>` and `total: usize`. `acquire`/`PoolGuard::drop`/`PoolGuard::take`/`health_check`/`drain` all read and write `total` under the same lock. The dual-counter shape the original review cited (separate atomics + sync return queue) no longer exists. Capacity check at line 596 reads the same field every release path writes.
- T2-5: `health_check` (lines 717-768) drains the idle queue under the lock via `std::mem::take(&mut state.idle)` (lines 720-723), drops the lock at the closure end, calls `pooled.conn.ping().await` with no lock held (lines 729-744), then re-acquires the lock to re-insert healthy entries (lines 749-756). The lock is `std::sync::Mutex` (not `tokio::Mutex`), so holding it across `.await` would not even compile.

**Resolution:** marked both tasks `[x]` in `todo/tasks.md` with file:line evidence. No edits to `pool.rs`. Update to `docs/code_review.md` to annotate T2-4/T2-5 as resolved is tracked by H-X.1.
