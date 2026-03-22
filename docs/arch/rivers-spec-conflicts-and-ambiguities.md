# Rivers Spec: Cross-Document Conflicts & High-Ambiguity Items

**Purpose:** Identify issues that would cause an agentic coding agent to hallucinate, make wrong assumptions, or produce inconsistent implementations.

**Severity scale:**
- **CONFLICT** — Two specs contradict each other directly. Agent will pick one and be wrong.
- **HIGH AMBIGUITY** — Spec is vague enough that two competent engineers would implement it differently.
- **FORWARD REF / DANGLING** — Spec references something that doesn't exist or is unresolved.

---

## Part 1: Cross-Document Conflicts

### CONFLICT-1: DriverError enum defined in two places with different variants

**Data Layer Spec** defines `DriverError` with 6 variants: `UnknownDriver`, `Connection`, `Query`, `Transaction`, `Unsupported`, `Internal`.

**Driver Spec** defines the same enum but uses it differently — the `Unsupported` variant is specifically used for "honest stubs" (Neo4j, Cassandra, etc.) while the Data Layer spec treats it as a general-purpose error for unimplemented operations.

**Agent trap:** An agent implementing a new driver plugin would be unclear whether `Unsupported` means "this driver exists but this operation isn't supported" vs "this is a placeholder stub." The semantic difference matters for health checks and admin dashboard reporting.

---

### CONFLICT-2: Session storage backend — who owns cross-node consistency?

**Auth/Session Spec** says sessions are stored in StorageEngine under `session:{session_id}` and that `in_memory` and `sqlite` backends are node-local.

**Application Spec** says auth scope carry-over automatically forwards `Authorization: Bearer {session_token}` to app-services, and that services validate tokens via StorageEngine.

**StorageEngine Spec** says for multi-node: Redis is required for shared state.

**The conflict:** Auth spec never explicitly requires Redis for multi-node. Application spec assumes cross-node session validation works. An agent implementing the session middleware would face: "Do I enforce Redis backend when multi-node is detected, or do I silently let sessions fail cross-node?" Neither spec makes this a startup validation rule.

---

### CONFLICT-3: Error response format — JSON envelope inconsistency

**HTTPD Spec** defines error responses as `{"error": "message"}`.

**Streaming REST Spec** defines error responses (before first yield) as `{"error": "message", "error_type": "HandlerError", "trace_id": "..."}`.

**View Layer Spec** doesn't define a specific error format — it relies on whatever the handler pipeline produces.

**Agent trap:** Three different error shapes across the codebase. An agent writing error handling middleware won't know which format to normalize to. Should `error_type` always be present? Is `trace_id` included in non-streaming errors?

---

### CONFLICT-4: Cache key derivation — two different approaches

**Data Layer Spec** says cache key is `SHA-256(view_name + ":" + canonical_json_params)` and uses `BTreeMap` iteration for ordering.

**Polling Views Spec** says poll loop key is `poll:{view_name}:{sha256(canonical_json(sorted_params))}` — same concept but the key format includes a prefix and the hashing scope is different (just params, not view_name + params).

**StorageEngine Spec** says L2 cache key is `cache:views:{view_name}:{parameter_hash}` where `parameter_hash` is SHA-256 of serialized params (order normalized before hashing).

**The conflict:** "Canonical JSON" is never defined. Three different components hash parameters, and if their serialization diverges (e.g., float precision, null handling, Unicode normalization), cache lookups will miss. An agent implementing any of these three would likely use `serde_json::to_string()` — but the specs don't guarantee that's the canonical form.

---

### CONFLICT-5: Credential redaction — two different keyword lists

**Logging Spec** defines general redaction keywords: `password, passwd, token, secret, authorization, api_key, apikey, api-key, access_key, private_key, credential, bearer, session_id, cookie, auth`.

**Data Layer Spec** defines DataViewEngine redaction keywords: `password, token, secret, authorization`.

**Agent trap:** An agent implementing redaction will use one list or the other depending on which spec they read first. The logging spec's list is a superset, but the Data Layer spec's redaction runs at a different layer (EventBus event construction vs log formatting). Inconsistent redaction = credential leaks in some code paths.

---

### CONFLICT-6: Circuit breaker — consecutive vs windowed failures

**Data Layer Spec** says circuit opens on `failure_threshold` **consecutive** failures.

**HTTP Driver Spec** says circuit breaker has `failure_threshold`, `window_ms`, `open_duration_ms`, `half_open_attempts` — implying a **time-windowed** failure count (failures within `window_ms`), not consecutive.

**Agent trap:** Consecutive and windowed are fundamentally different algorithms. An agent would implement one or the other, and the behavior diverges under intermittent failures.

---

### CONFLICT-7: LockBox credential reload — contradictory lifecycle

**LockBox Spec** explicitly states: "Reload: Requires `riversd` restart (no hot-reload in v1)."

**Data Layer Spec** says: "On `CredentialRotated` event: drain pool, rebuild with new credentials."

**Agent trap:** LockBox says no hot-reload, but the pool manager listens for `CredentialRotated` events. Either LockBox can hot-reload (contradicting its own spec) or this event is only emitted on restart (contradicting the word "rotate" which implies runtime). An agent implementing credential rotation would be stuck choosing which spec to trust.

---

### CONFLICT-8: Operation inference — fragile implicit behavior

**Data Layer Spec** says: "Operation inferred from first token of statement (lowercased)."

**Driver Spec** says: "Operation inference from statement first token if not explicit."

**Neither spec handles:** SQL comments before the first token (`-- comment\nSELECT ...`), CTEs (`WITH ... AS (...) SELECT ...`), whitespace/newlines, or driver-specific syntax (Redis `GET` vs SQL `GET`). An agent implementing the parser would have to guess at edge cases.

---

## Part 2: High-Ambiguity Items (Agent Traps)

### AMBIG-1: ProcessPool isolate reuse vs streaming — unresolved open question

**ProcessPool Spec** marks isolate reuse as an **Open Question**.

**Streaming REST Spec** explicitly says: "Streaming handlers follow the same isolate reuse policy as non-streaming handlers (Open Question #2 in the processpool spec)."

An agent given the task "implement streaming handler dispatch" would encounter a forward reference to an unresolved design decision. It would either guess (dangerous) or skip (incomplete).

---

### AMBIG-2: V8 snapshot content — marked "Open"

**ProcessPool Spec** says snapshot is "currently proposed: Rivers API stubs only" but marked **Open**.

An agent implementing the V8 pool would need to know exactly what's in the snapshot to build the `ObjectTemplate`. "Proposed" is not "decided."

---

### AMBIG-3: SSRF prevention — "post-DNS" not defined

**ProcessPool Spec** says `Rivers.http` performs "post-DNS RFC 1918 validation."

"Post-DNS" is not a standard term. Does this mean: resolve DNS, then check if the IP is private? What about DNS rebinding attacks (first resolve → public IP, second resolve → private IP)? An agent would likely implement a naive check that's bypassable.

---

### AMBIG-4: Handler pipeline parallel execution — snapshot semantics unclear

**View Layer Spec** says parallel `on_request` stages "share snapshot of ctx at start (don't see each other's deposits)."

But "snapshot" isn't defined. Is it a deep clone? Shallow copy with CoW? Arc reference? If two parallel stages both read `ctx.sources` and one deposits a key, what does the other see? The merge strategy after parallel completion isn't specified either.

---

### AMBIG-5: WebSocket binary frames — discard behavior

**View Layer Spec** says binary frames are "logged as warning, discarded."

Per-frame or per-connection logging? Under a binary flood, this could generate millions of log entries. An agent implementing the WebSocket handler would reasonably log per-frame (matching the spec literally) and create a DoS vector on the logging system.

---

### AMBIG-6: Polling `change_detect` timeout vs false — indistinguishable

**Polling Views Spec** says if `change_detect` handler times out, it's treated as no-change. If the handler returns `false`, it's also no-change.

There's no way to distinguish "the handler decided nothing changed" from "the handler crashed/timed out." An agent implementing monitoring or alerting would have no signal for a misbehaving change detector.

---

### AMBIG-7: `emit_on_connect` bypasses `on_change` — format mismatch risk

**Polling Views Spec** says when `emit_on_connect = true`, the raw DataView result is pushed without invoking `on_change`.

If `on_change` reshapes data (e.g., adds computed fields, filters sensitive data), the initial push has a different shape than subsequent pushes. An agent implementing the client-side consumer would see inconsistent payloads. The spec doesn't address this.

---

### AMBIG-8: `stream_terminated` field collision

**Streaming REST Spec** says: "Rivers validates this at dispatch — if the yielded value contains a `stream_terminated` key, it is an error."

But the error behavior is unclear. Is it a dispatch-time validation error (compile-time equivalent)? A runtime HandlerError? Does the generator get terminated? An agent implementing an LLM proxy that returns a JSON object from the upstream API containing `stream_terminated` would hit this validation unexpectedly.

---

### AMBIG-9: Retry-After header format

**HTTP Driver Spec** says `Retry-After` header is "honored if present, capped at max_delay_ms" but doesn't specify which format. RFC 7231 allows both `Retry-After: 120` (seconds) and `Retry-After: Fri, 31 Dec 2026 23:59:59 GMT` (HTTP-date). An agent would implement one format and silently ignore the other.

---

### AMBIG-10: EventBus topic creation — static only?

**View Layer Spec** deploy-time validation rule says "Referenced topic must exist in TopicRegistry."

But no spec defines how topics are created. Are they declared in config? Auto-created on first publish? An agent implementing a new EventBus producer would have no guidance on topic registration.

---

### AMBIG-11: StorageEngine queue `dequeue` — visibility timeout semantics

**StorageEngine Spec** says dequeued messages are "invisible until acked or pending_timeout elapses."

`pending_timeout` is never defined as a configuration option. Is it hardcoded? Per-topic? Global? An agent implementing the BrokerConsumerBridge would need this value to reason about at-least-once delivery guarantees.

---

### AMBIG-12: App startup phase ordering vs port binding

**Application Spec** says phase 2 starts app-services in parallel, phase 3 starts app-mains after required services are running. Port conflict detection says "If two apps declare same port, second deploy fails."

But the detection window is ambiguous — is it checked during resolution (phase 1) or during starting (phase 2/3)? If two app-services both bind port 9100 and start in parallel, which one "wins"?

---

## Part 3: RPS v2 — Substantially Incomplete

The RPS spec is roughly 50% complete and represents the biggest risk for agentic coding:

1. **Trust Bundle model** — Referenced as a replacement for 2-node Raft but never actually defined (format, validation, priority ordering, mismatch behavior).
2. **Secret Broker protocol** — Referenced as responsible for secret distribution but no request/response format, error cases, or retry semantics.
3. **Alias Registry query format** — Referenced multiple times, never defined.
4. **CodeComponent ProcessPool execution model** — Section 3 marked as "new" but content appears truncated/missing.
5. **v1→v2 migration** — LockBox spec references `rivers rps import-keystore` but RPS spec doesn't describe the import process.

An agentic coding agent given "implement RPS v2" would hallucinate most of the protocol details.

---

## Summary: Top 10 Agent-Killer Issues

| # | Issue | Risk | Specs Involved |
|---|-------|------|----------------|
| 1 | Canonical JSON never defined — cache key divergence | Silent data inconsistency | Data Layer, Polling, StorageEngine |
| 2 | Circuit breaker: consecutive vs windowed | Wrong failure detection algorithm | Data Layer vs HTTP Driver |
| 3 | LockBox no-reload vs CredentialRotated event | Contradictory credential lifecycle | LockBox vs Data Layer |
| 4 | Multi-node session validation has no enforcement | Auth bypass in production | Auth/Session, Application, StorageEngine |
| 5 | Error response format — 3 different shapes | Inconsistent API contract | HTTPD, Streaming REST, View Layer |
| 6 | RPS v2 is ~50% defined | Agent would hallucinate protocol | RPS |
| 7 | ProcessPool snapshot content marked "Open" | Can't implement V8 pool | ProcessPool |
| 8 | Streaming isolate reuse → forward ref to open question | Can't implement streaming dispatch | Streaming REST, ProcessPool |
| 9 | Redaction keyword lists diverge across layers | Credential leaks in some paths | Logging vs Data Layer |
| 10 | Operation inference from SQL first token — no edge case handling | Parser bugs on CTEs, comments, Redis | Data Layer, Driver |
