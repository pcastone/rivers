# Filesystem Driver — Change Decision Log

Captures design decisions made during Phases 1–6 that deviate from the letter
of `rivers-filesystem-driver-spec.md` or that warrant explanation for future
readers.

---

### CB-OTLP-D3 — 2026-05-11 — Track O2: OTLP dispatcher reuses `TaskKind::Rest` (no new variant)

**File affected:** `crates/riversd/src/server/otlp_view.rs`
**Spec reference:** `rivers-otlp-view-spec.md` §14.1 (Code Reuse table)
**Decision:** OTLP handler dispatch goes through `process_pool::dispatch` with `TaskKind::Rest`, not a new `TaskKind::Otlp` variant. The dispatch payload differs from REST (carries `ctx.otel` alongside `ctx.request`), but the *capability surface* the handler sees is identical: `Rivers.db.*`, `Rivers.crypto.*`, `Rivers.log.*`, `Rivers.keystore` — same wire_datasources path, same task_enrichment::enrich, same TaskContextBuilder. A new TaskKind variant would have meant patching the engine ABI, every gating site in `host_callbacks`, and the polling/MCP/SSE switches that already enumerate the closed set. No capability difference justified that churn.

**Trade-off:** Per-signal Prometheus metrics that label by `task_kind` (none currently exist) would lump OTLP work in with REST. Mitigated by the dedicated `rivers_otlp_*` metric family (Track O5.1, 7 metrics with `view`/`signal`/`encoding` labels) — operators slice by view/signal, not by task_kind.

**Resolution method:** Read existing TaskKind sites (`process_pool::TaskKind` enum + every match in `task_enrichment`, `cron`, `mcp::dispatch`, `view_engine::pipeline`); confirmed no capability-gating site needed a new arm.

---

### CB-OTLP-D4 — 2026-05-11 — Track O2: protobuf-out negotiation deferred; OTLP responses are always JSON

**File affected:** `crates/riversd/src/server/otlp_view.rs` (response shaping)
**Spec reference:** `rivers-otlp-view-spec.md` §7 + §13 non-goals
**Decision:** Even when the inbound request was `application/x-protobuf`, the response is `application/json` (an empty `{}` or a `{partialSuccess: {...}}` envelope). The OTLP/HTTP spec allows protobuf responses, but every real-world client we sampled (OTel SDK JS/Python/Go, OTel Collector) accepts JSON responses without complaint. Symmetrically encoding the response shaved an entire prost roundtrip without measurable benefit.

**Reversibility:** trivial — add a content-negotiation step on the response side; the prost types are already wired for protobuf-in.

**Resolution method:** Sampled the OTel SDK accept-header behavior in their respective sources; confirmed all SDKs send `Accept: application/json` or accept-anything. Added the carve-out to spec §13 non-goals.

---

### CB-OTLP-D1 — 2026-05-11 — Track O1: OTLP errors use S005 + `[X-OTLP-N]` markers, not new per-rule codes

**File affected:** `crates/rivers-runtime/src/validate_structural.rs`, `crates/rivers-runtime/src/validate_result.rs`
**Spec reference:** `rivers-otlp-view-spec.md` §9; `rivers-bundle-validation-spec.md` §11.5.1
**Decision:** All OTLP-view structural failures emit the existing `S005` error code with a `[X-OTLP-N]` marker prepended to the human-readable message, rather than introducing 6 new error codes (`O001`-`O006`). The warning case `W-OTLP-1` was given its own code (`W012`) because the existing warning catalog numbers each warning individually and there was no "generic warning code" to piggyback on.

Rationale: the existing cron validator (`validate_cron_view`) uses the same pattern — every cron-specific rule emits `S005` with descriptive text. New per-rule codes would have to be registered in `validate_result::error_codes`, documented in `bundle-validation-spec §11`, and asserted in tests as separate strings — for no functional gain over `r.message.contains("[X-OTLP-N]")`. Markers stay searchable in docs and probes.

**Resolution method:** Read `validate_cron_view` and surrounding tests; confirmed the project precedent during O1.1. CB's request bundle uses `X-OTLP-N` as documentation labels (not wire codes) anyway.

---

### CB-OTLP-D2 — 2026-05-11 — `auth = "bearer"` on OTLP views rejected; spec §8 to be amended

**File affected:** `crates/rivers-runtime/src/validate_structural.rs` (`validate_otlp_view`), `docs/arch/rivers-otlp-view-spec.md` (§8 follow-up)
**Spec reference:** `rivers-otlp-view-spec.md` §8 (to be amended); existing project resolution at `validate_structural.rs:126-130`
**Decision:** The OTLP spec §8 calls for accepting `auth = "bearer"` with a runtime warning pending CB-P1.12. The validator code comments at `VALID_AUTH_MODES` (`validate_structural.rs:126-130`) make clear that P1.12 was *resolved* — not by adding `"bearer"` to the auth allowlist, but by routing bearer-style auth through `guard_view` (the per-view named-guard pattern). To stay consistent with that resolution: OTLP views accept only `auth = "none"` at structural validation; `auth = "bearer"` is rejected with an `[X-OTLP-3]` message that points the operator at `guard_view = "..."`.

**Implication:** Spec §8 needs amendment to drop the "P1.12 pending" language and document the `guard_view` pattern as the way to do bearer auth on OTLP views. Captured as a follow-up in the changelog entry; deferred to spec-amendment commit.

**Resolution method:** Read `VALID_AUTH_MODES` comment block during O1.1; cross-referenced `cb-rivers-feature-request.md` (claimed P1.12 still pending) against the actual code state.

---

### Sprint 2026-05-09 — CB unblock plan (probe-derived)

**CB-PROBE-D1 — Probe shape mismatch is a CB-side migration, not a Rivers code fix**
- Files: `/Users/pcastone/Projects/cb/docs/rivers-upstream/cb-rivers-feature-validation-bundle/{expected-fail/*.toml,app/libraries/handlers/cases.ts}`
- Finding: CB's regression bundle for v0.60.12 reports EXPECTED FAIL on P1.9/P1.10/P1.11 because the probes were written against a presumed config shape that differs from what we actually shipped (`guard_view` vs `guard`, `response_headers` vs `[response.headers]`, `args.path_params` vs `ctx.request.path_params`). Validator output confirmed the shipped fixes work; the probe just doesn't exercise them.
- Decision: Track 1 of this sprint is a doc + CB-side bundle PR migrating the probes to canonical shapes. Rivers ships nothing here — the contract is right.
- Resolution: `docs/cb-probe-v0.60.12-migration.md` (this repo) + PR against CB's bundle.

**CB-PROBE-D2 — `auth` and `view_type` will be enum-validated**
- Files: `crates/rivers-runtime/src/{view.rs,validate_structural.rs}`
- Finding: Probe revealed silent passes — `auth = "bearer"` and `view_type = "Cron"` produce no validator error. Both fields are typed `Option<String>` / `String` with no enum check, so any string slides through structural and is silently treated as a no-op at runtime. CB's probe relied on validator rejection to detect missing-feature gaps; it gets a green where it should get an `S005`.
- Decision: Add structural-layer enum validation. Field types stay `String` (forward-compat — runtime can grow new variants without struct changes) but the validator gates value to a closed allowlist with did-you-mean.
- Rationale: Reusing existing `S005` (Invalid value for field) keeps the error catalog stable. Bumping to enum types in the struct would force a coordinated change across every literal site for marginal benefit.
- Resolution: `validate_view_type()` and `validate_auth_mode()` in `validate_structural.rs`; canonical sets pinned in this sprint (`view_type ∈ {Rest,Mcp,WebSocket,Sse,Streaming}`, `auth ∈ {none,session}`).

**CB-PROBE-D3 — P1.12 stays closed-as-superseded (`auth = "bearer"` will not ship)**
- Files: `docs/arch/rivers-auth-session-spec.md` §11.5
- Finding: After Track 2 ships, `auth = "bearer"` will produce a hard `S005`. CB needs a signed-off path forward — the named-guard recipe (already documented in §11.5) IS the answer.
- Decision: Track 1 rewrites the CB probe's Case G fragment from "EXPECTED FAIL on `auth = "bearer"`" to "PASS via named-guard bearer recipe", with a clear comment that the validator hardening is intentional.
- Resolution: G fragment becomes a positive sentinel for the recipe; close-out cited in changelog.

**CB-PROBE-D4 — P1.14 design follows polling-view dedupe pattern, not external scheduler**
- Files: `crates/riversd/src/cron/mod.rs` (new), `crates/riversd/src/polling/runner.rs` (reference)
- Finding: CB's case explicitly suggests "synthetic always-subscribed client on top of polling infrastructure" as the implementation direction. Reviewing `polling/runner.rs` confirms the loop-key-as-StorageEngine-write-lock dedupe pattern is the right transplant — same multi-instance guarantees, same execution environment as REST/MCP handlers.
- Decision: New `CronScheduler` per app, one tokio task per Cron view. StorageEngine `set_if_absent` with key `cron:{app}:{view}:{tick_epoch}` for first-writer-wins multi-node dedupe. Overlap policy `skip` is default, `queue` is a bounded `mpsc`, `allow` just spawns. No retry in v1 — handlers retry internally if they want it.
- Rationale: Avoids re-implementing distributed-scheduler primitives we already have. Honors the case's pass/fail criteria 1–4 directly.
- Resolution: Implemented 2026-05-10. `cron 0.16` selected over `croner` (T3.2) — smaller dep tree (chrono + once_cell + phf + winnow + serde, all already-or-trivially in the workspace). 5-field POSIX cron was initially planned to be supported via "second 0 prepended" shim but `cron 0.16` rejects 5-field at parse — spec adjusted to require the leading seconds field. Scheduler + 12 unit tests live in `crates/riversd/src/cron/mod.rs`. Wiring at `crates/riversd/src/bundle_loader/load.rs::load_and_wire_bundle` after `ctx.loaded_bundle = Some(...)`. Storage-engine-not-configured logs a startup error and skips (does NOT crash riversd; non-Cron apps continue to serve).

**CB-PROBE-D5 — Cron does not retry, does not catch up, does not run timezones**
- Files: `docs/arch/rivers-cron-view-spec.md` §12
- Finding: Adding retry/dead-letter, catch-up replay, and timezone scheduling each introduces meaningful design surface (retry-state persistence, replay queue semantics, timezone library choice and DST behavior). None are required to satisfy CB's pass/fail criteria.
- Decision: All three are explicit non-goals in v1. Documented in spec §12. Operators wanting any of them either (a) implement in their handler (retry), (b) accept that ticks during downtime are gone (catch-up), or (c) compute UTC offsets themselves (timezones).
- Rationale: Honors "Don't add features beyond what the task requires" (CLAUDE.md). Each is its own conversation when a real use case surfaces.

---

### Original filesystem-driver entries follow ↓

---

### Canary Sprint — 2026-05-05

**KAFKA-D1 — Separate datasource for `canary.events` vs `kafka_consume` topic**
- File: `canary-bundle/canary-streams/app.toml`, `resources.toml`
- Decision: Added `canary-kafka-events` as a separate Kafka datasource (same broker, `database = "canary.events"`) with its own `canary_events_consumer` MessageConsumer view, instead of reusing the `canary-kafka` datasource.
- Rationale: `resolve_topic()` in `rivers-plugin-kafka` only uses `subscriptions.first()`. A single datasource with multiple MessageConsumer views would still only subscribe to the first topic (`kafka_consume`). A separate datasource is the simplest way to get a dedicated consumer for `canary.events`.
- Resolution: `canary-kafka-events` datasource in app.toml/resources.toml. `canary_events_consumer` MessageConsumer routes to `onMessage` handler (same handler, handles both topics).

**KAFKA-D2 — `OffsetOutOfRange` recovery resets to earliest, not latest**
- File: `crates/rivers-plugin-kafka/src/lib.rs`
- Decision: On `OffsetOutOfRange`, call `get_offset(OffsetAt::Earliest)` and reset the consumer's offset to `earliest - 1`, then retry from `earliest`.
- Rationale: Using earliest preserves message delivery guarantees (at-least-once). Using latest would skip messages already published since the last successful offset. For canary testing scenarios that publish and immediately wait to consume, earliest is the correct choice.
- Resolution: Pattern-match on `RskafkaError::ServerError { protocol_error: ProtocolError::OffsetOutOfRange }` in `receive()`.

**CTX-D1 — `ctx.app_id` should expose entry-point slug, not manifest UUID**
- Files: `pipeline.rs`, `validation.rs`, `view_dispatch.rs`
- Decision: All `enrich()` calls now pass `&ctx.dv_namespace` (the entry-point slug, e.g., "handlers") instead of `&ctx.app_id` (the manifest UUID, e.g., "aaaaaaaa-bbbb-...").
- Rationale: JS handlers and keystore lookup both expect the human-readable slug. The UUID is for internal circuit breaker / datasource scoping. Spec says `ctx.app_id` should be the entry point slug.
- Resolution: Changed all 4 enrich() sites in pipeline.rs and 1 in validation.rs and 1 in view_dispatch.rs.

### P2.8 — Framework Audit Stream (2026-04-30)

**P2.8-D1 — Emit status 200/500 rather than actual HTTP status**
- File: `crates/riversd/src/server/view_dispatch.rs`
- Decision: `AuditEvent::HandlerInvoked.status` emits 200 on `Ok(result)` and 500 on `Err`. The actual HTTP status code (which could be 201, 404, 400, etc.) requires serializing the response, which happens after the audit event is emitted.
- Rationale: Emitting before response serialization avoids cloning the result. The audit event captures success/failure semantics, not the exact HTTP status. If exact HTTP status is needed in a future pass, we can move the emit after `serialize_view_result` at the cost of another variable.
- Resolution method: Pragmatic — spec says "status" but does not require the exact HTTP code. Acceptable for a v1 audit stream.

**P2.8-D2 — MCP emit in `dispatch()` call site, not `handle_tools_call()`**
- File: `crates/riversd/src/mcp/dispatch.rs`
- Decision: The `AuditEvent::McpToolCalled` emit wraps the call to `handle_tools_call` in the `dispatch()` match arm rather than inside `handle_tools_call` itself.
- Rationale: `handle_tools_call` has two dispatch sub-paths (dataview and codecomponent via `dispatch_codecomponent_tool`). Emitting from the outer call site avoids duplicating the timing/emit logic in both branches and does not require threading `audit_bus` further into the call stack.
- Resolution method: Consistent with spec guidance: "emit from the surrounding call site rather than deep inside".

**P2.8-D3 — Broadcast capacity 512**
- File: `crates/riversd/src/audit.rs`
- Decision: `broadcast::channel(512)` — not a config knob.
- Rationale: 512 events is ample for single-connection SSE consumers. If the buffer fills, `RecvError::Lagged` silently drops events — the sender never blocks. The spec does not require capacity to be configurable.
- Resolution method: Fixed constant; can be promoted to config if operational feedback requires.

---

### P1.1 — MCP Resource Subscriptions (2026-04-30)

**P1.1-D1 — Poller GC by polling, not reference counting**
- File: `crates/riversd/src/mcp/poller.rs`
- Decision: The change poller checks `snapshot_subscriptions()` at the top of each sleep cycle; if zero subscribers remain for the `(app_id, uri)` key, the task exits. No `Arc` refcount wrapping around `JoinHandle`.
- Rationale: Ref-counting poller tasks requires synchronization between the registry (which holds subscriber counts) and the poller map (which holds handles). Polling `snapshot_subscriptions()` is already lock-under-`Mutex` — one extra call per cycle. The worst-case over-run is one extra poll interval, which is acceptable at ≥1s intervals.
- Resolution method: Design review; added `gc()` helper for testing.

**P1.1-D2 — One poller per `(app_id, uri)` pair, not per subscriber**
- File: `crates/riversd/src/mcp/poller.rs`, `crates/riversd/src/mcp/dispatch.rs`
- Decision: `ensure_running` is idempotent: if a poller already exists for `(app_id, uri)`, it is not replaced. All subscribers for the same URI receive notifications via the single `SubscriptionRegistry::notify_changed` fan-out call.
- Rationale: Fanning out at the registry layer is cheaper than N concurrent pollers all executing the same DataView. Consistent with the "one poller per resource" model in the design spec.
- Resolution method: `ensure_running` checks `handles.contains_key` before spawning.

**P1.1-D3 — SSE session registered by `Mcp-Session-Id`, not IP/connection identity**
- File: `crates/riversd/src/server/view_dispatch.rs`, `crates/riversd/src/mcp/subscriptions.rs`
- Decision: The session registry key is the `Mcp-Session-Id` header value (a UUID string). Reconnecting clients with the same session ID attach to the same registry entry.
- Rationale: Matches MCP spec §6 session identity semantics. Allows clients to reconnect the SSE stream after a network interruption without re-subscribing.
- Resolution method: MCP spec review; session-id extracted at `view_dispatch.rs` before branching into SSE vs POST handlers.

**P1.1-D4 — Slow-consumer notification dropped silently (WARN log only)**
- File: `crates/riversd/src/mcp/subscriptions.rs`
- Decision: If the subscriber's mpsc channel is full when `notify_changed` is called, the notification frame is dropped and a `WARN` is emitted. The subscription remains active.
- Rationale: The alternative (block until channel drains) risks cascading backpressure to the poller task, delaying notifications for all other sessions. Dropped notifications are recoverable — the client can call `resources/read` again. The channel capacity of 64 events absorbs typical burst traffic.
- Resolution method: Matches "drop + WARN on full channel" in the design spec.

---

### RW2 — Broker & Transaction Contracts (2026-04-28)

**RW2.1 — `semantics()` on `MessageBrokerDriver`, not `BrokerConsumer`**
- File: `crates/rivers-driver-sdk/src/broker.rs`
- Decision: Placed `semantics()` on the driver trait (not consumer), defaulting to `AtLeastOnce`.
- Rationale: Semantics are a property of the protocol/driver, not per-consumer connection. A driver that speaks AMQP is always at-least-once regardless of which consumer is active.
- Resolution method: trait design review during RW2.1 implementation.

**RW2.2 — NATS `nack()` returns `Err(BrokerError::Unsupported)`, not `Ok`**
- File: `crates/rivers-plugin-nats/src/lib.rs`
- Decision: `nack()` returns `Err(Unsupported)` (not `Ok(AlreadyAcked)` or silently drops).
- Rationale: NATS core has no nack/redelivery protocol. Returning an error is honest; callers can distinguish "unsupported" from transport failures.
- Resolution method: matches `AtMostOnce` semantics definition.

**RW2.3 — Kafka `nack()` rewinds offset, returns `Ok(Acked)` (not `Err`)**
- File: `crates/rivers-plugin-kafka/src/lib.rs`
- Decision: `nack()` decrements `self.offset` by 1 and returns `Ok(AckOutcome::Acked)`.
- Rationale: rskafka has no native nack command; cursor rewind is the only mechanism available. Rewind does cause redelivery on next poll (at-least-once). Returning `Ok` rather than `Err(Unsupported)` because redelivery is genuinely supported — just implicitly.
- Resolution method: rskafka API review; documented in code comment.

**RW2.4 — Redis Streams `nack()` returns `Ok(Acked)` via PEL passivity**
- File: `crates/rivers-plugin-redis-streams/src/lib.rs`
- Decision: `nack()` is a no-op that returns `Ok(AckOutcome::Acked)`.
- Rationale: Messages not ACKed remain in the Pending Entries List (PEL). XAUTOCLAIM will redeliver them after the visibility timeout. No explicit XNACK exists in Redis Streams. Redelivery is guaranteed passively.
- Resolution method: Redis Streams protocol review.

**RW2.6 — MongoDB `exec_find` duplicates iteration loop (two branches)**
- File: `crates/rivers-plugin-mongodb/src/lib.rs`
- Decision: Two entirely independent code paths for session vs non-session find, each with their own cursor variable.
- Rationale: `collection.find(filter).session(session)` returns `SessionCursor<Document>` and `collection.find(filter)` returns `Cursor<Document>`. These are distinct Rust types. They cannot be unified in a single if/else branch (the compiler rejects mismatched cursor types in each arm). Code duplication was unavoidable.
- Resolution method: MongoDB Rust driver 3.x type system review; SessionCursor::advance() requires &mut ClientSession argument.

**RW2.7 — Neo4j `nack()` maps to `Err(BrokerError::Unsupported)` (N/A — neo4j is a database driver)**
- File: `crates/rivers-plugin-neo4j/src/lib.rs`
- Decision: Neo4j is a `DatabaseDriver`, not a `MessageBrokerDriver`. RW2.7 fixes apply to transaction routing and Bolt type binding only, not broker ack/nack.
- Resolution method: RW2.7 scope clarification — neo4j was added as a DB driver registration fix (RW2.7.d) + txn routing + Bolt types.

**RW2.7 — neo4rs lazy connection: live tests treat ping failure as SKIP**
- File: `crates/rivers-plugin-neo4j/tests/neo4j_live_test.rs`
- Decision: Both live tests emit SKIP (not FAIL) when ping fails post-connect.
- Rationale: `neo4rs::Graph::connect()` is lazy — it doesn't TCP-connect until the first query. After RW2.7.b (ping error propagation), a correctly-propagated ping error now surfaces in CI without a live Neo4j server. The skip preserves CI green on machines without Neo4j.
- Resolution method: neo4rs 0.9.0-rc.9 API documentation review; mirrors pattern used by other live tests (NATS, Redis, RabbitMQ).

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

---

### MYSQL-H4.1 — Pool key password fingerprint approach

- **File:** `crates/rivers-drivers-builtin/src/mysql.rs`
- **Decision:** SHA-256 of password bytes, first 8 bytes (16 hex chars) appended to pool key as fragment (`#<fingerprint>`)
- **Rationale:** 64-bit fingerprint is far more than sufficient to distinguish a small number of credential sets in a process-local cache. Raw password excluded from key for security.
- **Alternative:** full password hash — rejected, overkill and slightly larger key string with no practical benefit for this use case
- **Resolution method:** code review finding, H4 from rivers-wide review 2026-04-27

---

### TXN-TQ8 — tx_query_callback must use DataView's default query field

**File:** `crates/riversd/src/process_pool/v8_engine/rivers_global.rs`, `crates/rivers-runtime/src/dataview.rs`
**Spec reference:** TXN spec §6 TQ-8
**Decision:** `tx_query_callback` was passing `"POST"` as the HTTP method to `executor.execute()`. Per TQ-8, `tx.query()` must use the DataView's default `query` field regardless of HTTP context. Added `"DEFAULT"` case to `DataViewConfig::query_for_method()` that returns `self.query.as_deref()` only. Updated `tx_query_callback` to pass `"DEFAULT"` instead of `"POST"`.
**Resolution method:** Bug discovered while writing TXN-D.6 V8 integration tests — `tx.query("insert_item", ...)` failed with "unknown operation" when DataView only had `query` (not `post_query`) set.

---

### TXN-TF3-RUNTIME — transaction=true on non-transactional driver must silently skip

**File:** `crates/rivers-runtime/src/dataview_engine.rs`
**Spec reference:** TXN spec §3 TF-3; tutorial note
**Decision:** Runtime did not honor TF-3 — if `begin_transaction()` returned `DriverError::Unsupported`, the DataView would fail with "BEGIN failed: unsupported". The tutorial explicitly says "silently ignored (with a validation warning W008)". Fixed: `use_txn_wrapper` is set to false when `begin_transaction()` returns `Unsupported`, allowing the query to run without a transaction wrapper.
**Resolution method:** Spec + tutorial review during TXN-B.5 test writing.
