# Rivers — Haiku-Ready Buildout Task Plan

> **Audience:** Small agentic models (Haiku-class). Tasks are atomic, mechanical, and verifiable in a single tool-loop round.
> **Scope:** Full Rivers buildout vs `docs/arch/` (excluding `riverbed-*`).
> **As-of:** 2026-04-16, branch `docs/guide-v0.54.0-updates`.

---

## Task Template (every task uses these fields)

```
### <Sprint>.<N> — <verb + concrete object>
- **READ** — file paths to Read tool first (priming context)
- **LOCATE** — Grep/Glob patterns to find target code
- **EDIT** — ordered imperative steps (exact strings where known)
- **VERIFY** — Bash/cargo commands + expected pass condition
- **SPEC** — one-line pointer
- **Tokens** — per-iteration cost (Haiku-scale)
- **Confidence** — 1–100 (higher = more mechanical)
- **Split-if** — when to subdivide
```

**Rules for Haiku agents:**
1. Always `READ` listed files before editing. Never edit unread files.
2. Run every `LOCATE` command first. If hits differ wildly from the task's assumption, STOP and report.
3. Only use `Edit` tool for small diffs; `Write` only for new files.
4. After `EDIT`, run every `VERIFY` command. If any fails, report and do NOT proceed.
5. Never invent struct/field names not seen in READ or LOCATE output.
6. One task = one commit = one PR-sized change.

---

# SPRINT 0 — Shaping Compliance (must land first)

**Block:** all other sprints depend on Sprint 0.
**Spec root:** `docs/arch/rivers-shaping-and-gap-analysis.md`

### S0.1 — Delete `redact_sensitive_text`
- **READ** — `crates/rivers-core/src/logging.rs`
- **LOCATE** — `Grep("redact_sensitive_text", type=rust)` ; `Grep("fn redact", type=rust)`
- **EDIT** —
  1. Delete the `redact_sensitive_text` fn definition.
  2. Remove every call site (Grep result).
  3. Delete the 4-keyword DataView redaction helper (expect file under `crates/rivers-runtime/src/dataview/`).
- **VERIFY** —
  - `cargo build -p rivers-core` → exit 0
  - `cargo test -p rivers-core` → exit 0
  - `Grep("redact_", type=rust)` → 0 hits in `crates/`
- **SPEC** — SHAPE-4, logging §8.1/§8.2
- **Tokens** — 1.5K–3K | **Confidence** — 95

### S0.2 — Add `DriverError::NotImplemented`
- **READ** — `crates/rivers-driver-sdk/src/lib.rs`
- **LOCATE** — `Grep("enum DriverError", type=rust)`
- **EDIT** — Add variant `NotImplemented(String),` after `Unsupported(String),`. Add `impl Display` arm.
- **VERIFY** — `cargo build -p rivers-driver-sdk` → exit 0
- **SPEC** — SHAPE-6
- **Tokens** — 1K–2K | **Confidence** — 98

### S0.3 — Convert honest stubs to `NotImplemented` (built-in drivers)
- **READ** — `crates/rivers-drivers-builtin/src/**/*.rs` (open as Grep finds)
- **LOCATE** — `Grep("Unsupported\\(", glob="crates/rivers-drivers-builtin/**")`
- **EDIT** — For each hit where the surrounding comment or fn is a stub, replace `Unsupported(` → `NotImplemented(`. Leave genuine "not applicable" cases as `Unsupported`.
- **VERIFY** — `cargo test -p rivers-drivers-builtin` → exit 0
- **SPEC** — SHAPE-6
- **Tokens** — 2K–4K | **Confidence** — 85
- **Split-if** — >20 hits, do per-file.

### S0.4 — Convert honest stubs to `NotImplemented` (plugins, per-plugin task)
- **Split-if** — always. Produce one task per plugin: cassandra, couchdb, elasticsearch, exec, influxdb, kafka, ldap, mongodb, nats, neo4j, rabbitmq, redis-streams.
- **READ** — `crates/rivers-plugin-<name>/src/lib.rs`
- **LOCATE** — `Grep("Unsupported\\(", path="crates/rivers-plugin-<name>")`
- **EDIT** — same rule as S0.3.
- **VERIFY** — `cargo test -p rivers-plugin-<name>` → exit 0
- **Tokens** — 1.5K–3K each | **Confidence** — 90

### S0.5 — Add `window_ms` to `CircuitBreakerConfig`
- **READ** — `crates/rivers-core-config/src/lib.rs`
- **LOCATE** — `Grep("struct CircuitBreakerConfig", type=rust)`
- **EDIT** —
  1. Add field `pub window_ms: u64,` with `#[serde(default = "default_window_ms")]`.
  2. Rename `open_duration_ms` → `open_timeout_ms` (field + serde alias).
  3. Add `fn default_window_ms() -> u64 { 60_000 }`.
- **VERIFY** — `cargo build -p rivers-core-config` → exit 0
- **SPEC** — SHAPE-1, data-layer §5.2
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S0.6 — Rolling-window counter in pool circuit breaker
- **READ** — `crates/rivers-core/src/pool_manager.rs`
- **LOCATE** — `Grep("failure_threshold", type=rust)` ; `Grep("consecutive", type=rust)`
- **EDIT** —
  1. Replace `AtomicU32 consecutive` with `Mutex<VecDeque<Instant>>`.
  2. On failure: push `Instant::now()`, drain entries older than `window_ms`, open if `len() >= failure_threshold`.
  3. On success: clear deque.
- **VERIFY** — Add unit test: 5 failures in 30s opens breaker; 10 failures over 120s with `window_ms=60000` stays closed. Run `cargo test -p rivers-core pool_manager::` → exit 0.
- **SPEC** — SHAPE-1
- **Tokens** — 3K–5K | **Confidence** — 85

### S0.7 — HTTP driver: rename `open_duration_ms`
- **READ** — `crates/rivers-drivers-builtin/src/http/*.rs` (or wherever http driver lives)
- **LOCATE** — `Grep("open_duration_ms", type=rust)`
- **EDIT** — Replace `open_duration_ms` → `open_timeout_ms` everywhere in HTTP driver module.
- **VERIFY** — `cargo test -p rivers-drivers-builtin http::` → exit 0
- **SPEC** — SHAPE-1
- **Tokens** — 1K–2K | **Confidence** — 95

### S0.8 — Shared `canonical_key(view, params)` helper
- **READ** — any existing cache module (Grep first).
- **LOCATE** — `Grep("fn cache_key", type=rust)` ; `Grep("FNV", type=rust)` ; `Grep("fnv", type=rust)`
- **EDIT** —
  1. Create `crates/rivers-runtime/src/canonical_key.rs` with `pub fn canonical_key(view: &str, params: &serde_json::Value) -> String`.
  2. Impl: params → `BTreeMap<String, Value>` → `serde_json::to_string` → `Sha256` → hex.
  3. Export from `lib.rs`.
  4. Return string format: `cache:views:{view}:{hash}`.
  5. Add `pub fn poll_key(view, params) -> format!("poll:{view}:{hash}")`.
- **VERIFY** —
  - Unit test: same input → same output; permuted param order → same output.
  - `cargo test -p rivers-runtime canonical_key` → exit 0.
- **SPEC** — SHAPE-3
- **Tokens** — 2K–4K | **Confidence** — 95

### S0.9 — Swap DataView cache hash to canonical_key
- **Depends** — S0.8
- **READ** — the DataView cache file (Grep to find)
- **LOCATE** — `Grep("fnv\\|FNV", type=rust, path="crates/rivers-runtime")`
- **EDIT** — Replace FNV-1a hasher call with `canonical_key(view, params)`.
- **VERIFY** — `cargo test -p rivers-runtime dataview::cache` → exit 0
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S0.10 — `infer_operation(sql)` helper
- **READ** — `crates/rivers-runtime/src/lib.rs`
- **LOCATE** — `Grep("infer_operation\\|QueryOp", type=rust)`
- **EDIT** — New file `crates/rivers-runtime/src/op_inference.rs`:
  - Trim lead whitespace, strip `--` to newline, strip `/* */`.
  - First whitespace token lowercased.
  - Map: select/get/find/search → Read; insert/create/add/set/put/update/patch/replace → Write; delete/remove/del → Delete; else Read.
- **VERIFY** — Table-driven test (≥10 cases). `cargo test -p rivers-runtime op_inference` → exit 0.
- **SPEC** — SHAPE-7
- **Tokens** — 2K–4K | **Confidence** — 92

### S0.11 — ErrorResponse envelope on 429 rate-limit
- **READ** — `crates/riversd/src/server/middleware/rate_limit.rs` (locate first)
- **LOCATE** — `Grep("\\\"rate limit\\\"", type=rust)` ; `Grep("json!\\(.*error", type=rust)`
- **EDIT** — Replace `json!({"error": "rate limit exceeded"})` with `ErrorResponse { code: 429, message: "rate limit exceeded", details: None, trace_id }.into_response()`.
- **VERIFY** — Response contract test: `cargo test -p riversd rate_limit_response` → exit 0
- **SPEC** — SHAPE-2
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S0.12 — ErrorResponse envelope on 503 backpressure
- Same shape as S0.11; target `backpressure` middleware. `LOCATE` with `Grep("server overloaded")`.
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S0.13 — ErrorResponse envelope on 503 shutdown
- Same shape as S0.11; `LOCATE` with `Grep("shutting down")`.
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S0.14 — Remove `parallel = true` pipeline branch
- **READ** — `crates/riversd/src/handler_pipeline.rs` (or equivalent; Grep first)
- **LOCATE** — `Grep("parallel", type=rust, path="crates/riversd/src")` ; `Grep("join_all", type=rust)`
- **EDIT** — Delete parallel branch + `join_all` call. Keep only sequential path.
- **VERIFY** — `cargo test -p riversd pipeline` → exit 0 ; `Grep("parallel.*=.*true", type=rust)` → 0 hits in handler pipeline.
- **SPEC** — SHAPE-12
- **Tokens** — 2K–4K | **Confidence** — 90

### S0.15 — Delete `parallel = true` from TOML examples
- **LOCATE** — `Grep("parallel = true", glob="docs/arch/*.md")`
- **EDIT** — Delete the line in each hit; leave surrounding TOML example functional.
- **VERIFY** — `Grep("parallel = true", glob="docs/arch/*.md")` → 0 hits.
- **Tokens** — 1K–2K | **Confidence** — 98

### S0.16 — LockBox: index-only startup
- **READ** — `crates/rivers-lockbox-engine/src/lib.rs`
- **LOCATE** — `Grep("decrypt", type=rust, path="crates/rivers-lockbox-engine")`
- **EDIT** —
  1. Change `HashMap<String, Vec<u8>>` (plaintext values) → `HashMap<String, EntryIndex>` where `EntryIndex = { path: PathBuf, meta: Meta }`.
  2. Startup loader builds index only (no decrypt).
  3. Add `pub fn acquire(&self, name: &str) -> Result<ZeroizingVec<u8>>` — opens file, decrypts on demand, returns zeroizing buffer.
- **VERIFY** — `cargo test -p rivers-lockbox-engine` → exit 0 ; ensure no `decrypt` in startup code path.
- **SPEC** — SHAPE-5
- **Tokens** — 4K–7K | **Confidence** — 78
- **Split-if** — acquire API needs >1 file change → separate task.

### S0.17 — Remove `CredentialRotated` event + handlers
- **Depends** — S0.16
- **LOCATE** — `Grep("CredentialRotated", type=rust)`
- **EDIT** — Delete enum variant, emit sites, and pool-drain handler. Remove event level entry in logging spec table.
- **VERIFY** — `Grep("CredentialRotated", type=rust)` → 0 hits. `cargo build` → exit 0.
- **SPEC** — SHAPE-5
- **Tokens** — 2K–4K | **Confidence** — 92

### S0.18 — StorageEngine trait: drop queue methods
- **READ** — `crates/rivers-core-config/src/storage.rs` (or wherever trait lives; Grep first)
- **LOCATE** — `Grep("fn enqueue\\|fn dequeue\\|fn ack", type=rust)`
- **EDIT** — Remove trait method signatures; delete `StoredMessage` struct.
- **VERIFY** — `cargo build -p rivers-core-config` → exit 0
- **SPEC** — SHAPE-18
- **Tokens** — 1.5K–3K | **Confidence** — 95

### S0.19 — InMemoryStorageEngine: drop queue field
- **Depends** — S0.18
- **LOCATE** — `Grep("queues.*HashMap\\|VecDeque<Stored", type=rust)`
- **EDIT** — Remove `queues` field and its methods.
- **VERIFY** — `cargo test -p rivers-storage-backends` → exit 0
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S0.20 — Remove `emit_on_connect` from polling
- **LOCATE** — `Grep("emit_on_connect", type=rust)` ; `Grep("emit_on_connect", glob="**/*.toml")`
- **EDIT** — Delete field from `PollLoopState`. Add validation rule: unknown key error when present.
- **VERIFY** — `cargo test -p rivers-runtime polling` → exit 0 ; test config with `emit_on_connect = true` → validation fails.
- **SPEC** — SHAPE-14
- **Tokens** — 2K–3K | **Confidence** — 95

### S0.21 — Remove port-conflict preflight check
- **LOCATE** — `Grep("port.*conflict\\|port_conflict", type=rust, path="crates/riverpackage")`
- **EDIT** — Delete the check fn and its call site.
- **VERIFY** — `cargo test -p riverpackage` → exit 0
- **SPEC** — SHAPE-19
- **Tokens** — 1K–2K | **Confidence** — 95

### S0.22 — Remove EventBus topic registry lookup on publish
- **LOCATE** — `Grep("TopicRegistry\\|configure_topic_registry", type=rust)`
- **EDIT** — Remove lookup inside `publish()`. Remove startup call site. Keep struct as optional internal state.
- **VERIFY** — `cargo test -p rivers-core eventbus` → exit 0
- **SPEC** — SHAPE-17
- **Tokens** — 2K–3K | **Confidence** — 92

### S0.23 — WebSocket binary-frame rate-limited log
- **LOCATE** — `Grep("binary frame\\|Message::Binary", type=rust)`
- **EDIT** — Per-connection `AtomicU64 binary_frame_count`. First frame → `tracing::warn!`. Spawn tokio task every 60s: if >0, log summary `"{conn_id}: {n} binary frames suppressed"`, reset to 0.
- **VERIFY** — Unit test: burst 100 frames → exactly 1 WARN + 1 summary log.
- **SPEC** — SHAPE-13
- **Tokens** — 2K–4K | **Confidence** — 85

### S0.24 — Streaming REST poison guard
- **LOCATE** — `Grep("stream_terminated", type=rust)` ; `Grep("AsyncGenerator\\|stream_chunk", type=rust)`
- **EDIT** — In generator drive loop, before serializing chunk: if top-level key `stream_terminated` present → emit `{error: "handler yielded reserved key", error_type: "HandlerError", stream_terminated: true}`, close stream, WARN log.
- **VERIFY** — Integration test yields `{stream_terminated: true}` → stream closes with poison chunk.
- **SPEC** — SHAPE-15
- **Tokens** — 2K–4K | **Confidence** — 85

### S0.25 — HTTP driver `retry_after_format`
- **LOCATE** — `Grep("Retry-After\\|retry_after", type=rust)` ; `Grep("HttpRetryConfig", type=rust)`
- **EDIT** —
  1. Add `retry_after_format: String` (default `"seconds"`).
  2. Parser switches on declared format; parse failure → `tracing::warn!` + fall back.
- **VERIFY** — Unit tests: "seconds" with `30`, "http_date" with HTTP date, malformed fallback → all pass.
- **SPEC** — SHAPE-16
- **Tokens** — 2K–3K | **Confidence** — 90

### S0.26 — Strip V8 snapshot references from code
- **LOCATE** — `Grep("snapshot", type=rust, path="crates/rivers-engine-v8")` ; `Grep("snapshot", type=rust, path="crates/riversd")`
- **EDIT** — Delete any code building or loading snapshots. Leave comment `// snapshots removed per SHAPE-10` only where useful for readers.
- **VERIFY** — `cargo build -p rivers-engine-v8` → exit 0 ; `Grep("snapshot_blob\\|create_params.*snapshot", type=rust)` → 0 hits.
- **SPEC** — SHAPE-10
- **Tokens** — 2K–4K | **Confidence** — 85

### S0.27 — Remove SSRF IP validation
- **LOCATE** — `Grep("RFC ?1918\\|is_private\\|SSRF", type=rust)`
- **EDIT** — Delete IP validation helpers and call sites in HTTP fetch path.
- **VERIFY** — `cargo test -p rivers-drivers-builtin http::` → exit 0
- **SPEC** — SHAPE-11
- **Tokens** — 1.5K–3K | **Confidence** — 90

---

# SPRINT 1 — Spec Amendments (doc-only; Haiku-perfect)

**Rule:** one task = one spec file. All edits are text-replacements. Do NOT touch code.

### S1.1 — Amend `rivers-data-layer-spec.md`
- **READ** — `docs/arch/rivers-data-layer-spec.md`, rows for data-layer in `rivers-shaping-and-gap-analysis.md` Part 2.
- **EDIT** —
  1. §5.2: add `window_ms` to struct; replace "consecutive" → "within window_ms".
  2. §5.3: delete `CredentialRotated` paragraph.
  3. §6: delete redaction paragraph; insert operation-inference algorithm.
  4. §7: replace hash text with pointer to canonical-JSON appendix.
  5. §10: replace `StorageEngine.enqueue` text with "BrokerConsumerBridge → EventBus".
- **VERIFY** — `Grep("CredentialRotated\\|enqueue", path=this file)` → 0 hits.
- **Tokens** — 3K–5K | **Confidence** — 92

### S1.2 — Amend `rivers-http-driver-spec.md`
- **EDIT** — replace `open_duration_ms` → `open_timeout_ms`; add `retry_after_format` attribute; remove `parallel = true` lines.
- **VERIFY** — `Grep("open_duration_ms\\|parallel = true", path=this file)` → 0 hits.
- **Tokens** — 1.5K–3K | **Confidence** — 95

### S1.3 — Amend `rivers-httpd-spec.md`
- **EDIT** — confirm SHAPE-21/22/23/24 landed per Gap Analysis Part 2 rows. Update any remaining `{"error": "..."}` examples to ErrorResponse.
- **VERIFY** — `Grep("json!\\({\\\"error\\\"", path=this file)` → 0 hits; `Grep("\\[base\\.tls\\]", path=this file)` → ≥1 hit.
- **Tokens** — 3K–5K | **Confidence** — 88

### S1.4 — Amend `rivers-streaming-rest-spec.md`
- **EDIT** — rewrite Open Question #2 to "isolates reused, streaming gets long-lived context". Add `stream_terminated` runtime-guard subsection. Note poison chunks are wire-specific.
- **Tokens** — 2K–3K | **Confidence** — 90

### S1.5 — Amend `rivers-storage-engine-spec.md`
- **EDIT** — strike §1 queue overview; remove §2 enqueue/dequeue/ack; remove §2.3 StoredMessage; remove §2.4 dequeue semantics; remove §3.1 queue field; remove §3.2 queue table; remove §3.3 Streams ops; add SHAPE-8 sentinel subsection; replace §5 cache key with appendix pointer.
- **VERIFY** — `Grep("enqueue\\|dequeue\\|StoredMessage", path=this file)` → 0 hits.
- **Tokens** — 3K–5K | **Confidence** — 92

### S1.6 — Amend `rivers-logging-spec.md`
- **EDIT** — delete §8.1 and §8.2 redaction; delete `CredentialRotated` row.
- **VERIFY** — `Grep("redact\\|CredentialRotated", path=this file)` → 0 hits.
- **Tokens** — 2K–3K | **Confidence** — 95

### S1.7 — Amend `rivers-lockbox-spec.md`
- **EDIT** — rewrite §3, §5, §7, §8.4 per SHAPE-5 (index-only, per-access decrypt, no restart on rotate).
- **Tokens** — 3K–5K | **Confidence** — 88

### S1.8 — Amend `rivers-driver-spec.md`
- **EDIT** — §2: add `NotImplemented(String)`; insert driver-override hook note for inference; remove `CredentialRotated`.
- **Tokens** — 2K–3K | **Confidence** — 92

### S1.9 — Amend `rivers-application-spec.md`
- **EDIT** — §12: remove port conflict; add SHAPE-8 sentinel gate.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S1.10 — Amend `rivers-processpool-runtime-spec-v2.md`
- **EDIT** — rewrite §1 worker language, §3 V8 worker (remove snapshot text), §5 startup (remove snapshot load), close Open Questions #1/#2/#4. Add four-scope model subsection.
- **Tokens** — 4K–6K | **Confidence** — 82

### S1.11 — Amend `rivers-view-layer-spec.md`
- **EDIT** — remove parallel stage text/examples; replace §6 binary frame behavior with rate-limited rule; remove §11 topic-validation rule; remove §13 table row.
- **Tokens** — 2K–4K | **Confidence** — 90

### S1.12 — Amend `rivers-polling-views-spec.md`
- **EDIT** — remove `emit_on_connect` everywhere; replace §3 cache key with appendix pointer; add `PollChangeDetectTimeout` event in §3.5.
- **Tokens** — 2K–4K | **Confidence** — 92

### S1.13 — Add canonical-JSON appendix
- **EDIT** — new section (appendix file or inside data-layer spec): BTreeMap ordering, serde_json serialization, SHA-256, hex. Referenced from data-layer, storage-engine, polling specs.
- **Tokens** — 1K–2K | **Confidence** — 95

---

# SPRINT 2 — Core Wiring (no new deps)

### S2.1 — Wire CLI to `run()`
- **READ** — `crates/riversd/src/main.rs`, `crates/riversctl/src/main.rs`
- **LOCATE** — `Grep("fn run\\|fn serve", type=rust, path="crates/riversd")`
- **EDIT** — pipe config path, log level, foreground flag from clap args into `server::lifecycle::run()`. Use `rivers_runtime::home::discover_config()` when no `--config`.
- **VERIFY** — `riversctl start --foreground` boots with example config; exits cleanly on Ctrl-C.
- **Tokens** — 3K–5K | **Confidence** — 85

### S2.2 — Generate `trace_id` at middleware entry
- **READ** — `crates/riversd/src/server/middleware/*.rs`
- **LOCATE** — `Grep("trace_id", type=rust, path="crates/riversd")`
- **EDIT** — in first middleware layer: if `traceparent` header present, parse W3C; else mint `uuid::Uuid::new_v4()`. Attach to `Request::extensions_mut()`.
- **VERIFY** — integration test: response `X-Trace-Id` header present on every 200.
- **Tokens** — 2K–4K | **Confidence** — 88

### S2.3 — Propagate `trace_id` to `ctx`
- **Depends** — S2.2
- **LOCATE** — `Grep("struct .*Ctx\\|pub fn ctx_from", type=rust)`
- **EDIT** — read trace_id from extensions, attach to ctx builder.
- **VERIFY** — handler `Rivers.log("x")` → log includes `trace_id`.
- **Tokens** — 2K–3K | **Confidence** — 88

### S2.4 — Forward `trace_id` in HTTP driver outbound
- **LOCATE** — `Grep("reqwest\\|RequestBuilder", type=rust, path="crates/rivers-drivers-builtin/src/http")`
- **EDIT** — add `traceparent` header (W3C `00-{trace_id}-{span_id}-01`) to every outbound request.
- **VERIFY** — echo server test sees header.
- **Tokens** — 2K–3K | **Confidence** — 85

### S2.5 — File logging sink
- **READ** — existing logging init code
- **LOCATE** — `Grep("EnvFilter\\|tracing_subscriber::fmt", type=rust)`
- **EDIT** — add optional `tracing_appender::rolling::never(dir, file)` sink when `[base.logging].file_path` set; wrap in non-blocking async writer.
- **VERIFY** — integration: request to app → line appears in file.
- **Tokens** — 2K–4K | **Confidence** — 88

### S2.6 — `/health` handler wired
- **LOCATE** — `Grep("fn health\\|/health", type=rust)`
- **EDIT** — route always returns 200 `{status: "ok"}`. No auth. Subject to full middleware stack.
- **VERIFY** — `curl -i /health` → 200.
- **Tokens** — 1.5K–3K | **Confidence** — 95

### S2.7 — `/health/verbose` handler
- **Depends** — S2.6
- **EDIT** — returns pool snapshot JSON. Gated by `admin_ip_allowlist` when set. `?simulate_delay_ms=N` → sleep N before response.
- **VERIFY** — `curl /health/verbose?simulate_delay_ms=200` → 200 after 200ms.
- **Tokens** — 2K–4K | **Confidence** — 85

### S2.8 — CORS headers on ErrorResponse
- **Depends** — S0.11–S0.13
- **LOCATE** — CORS middleware file (Grep first)
- **EDIT** — ensure CORS layer wraps error responses too (reorder to post-handler).
- **VERIFY** — integration test: OPTIONS preflight + 429 response both carry `access-control-allow-origin`.
- **Tokens** — 2K–3K | **Confidence** — 82

### S2.9 — `datasource = "none"` null pattern
- **LOCATE** — `Grep("datasource", type=rust, path="crates/rivers-runtime")`
- **EDIT** — short-circuit Pool Manager resolution when datasource name == `"none"`. Empty `ctx.data`. Pass validation.
- **VERIFY** — example view with `datasource = "none"` + JS handler setting `ctx.resdata = {"ok": true}` returns correctly.
- **Tokens** — 2K–3K | **Confidence** — 88

### S2.10 — Admin localhost enforcement
- **LOCATE** — `Grep("public_key\\|admin_api", type=rust, path="crates/rivers-core-config")`
- **EDIT** — at startup: if `host == "0.0.0.0"` and `public_key == None` → `bail!`. If `127.0.0.1` + no key → WARN and continue.
- **VERIFY** — unit test for each case.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S2.11 — Always-on `NoopDataViewCache`
- **LOCATE** — `Grep("Option<.*DataViewCache>", type=rust)`
- **EDIT** — change `Option<Arc<dyn DataViewCache>>` to `Arc<dyn DataViewCache>`. Unconfigured path returns `Arc::new(NoopDataViewCache)`.
- **VERIFY** — `cargo test -p rivers-runtime cache` → exit 0.
- **Tokens** — 1.5K–3K | **Confidence** — 90

---

# SPRINT 3 — TLS / HTTP Server

### S3.1 — Add `rustls`, `rcgen`, `tokio-rustls`
- **EDIT** — `Cargo.toml` workspace deps. Pin versions.
- **VERIFY** — `cargo tree | grep rustls` → found.
- **Tokens** — 1K–2K | **Confidence** — 95

### S3.2 — Define `TlsConfig`, `TlsX509Config`, `TlsEngineConfig`
- **READ** — `crates/rivers-core-config/src/lib.rs`
- **EDIT** — add three structs with `serde(default)`. Required fields: `TlsConfig.cert: Option<String>, key: Option<String>, redirect: bool (default true)`. `TlsX509Config.common_name, san, days`. `TlsEngineConfig.min_version, ciphers`.
- **VERIFY** — round-trip TOML with all three structs parses.
- **Tokens** — 2K–3K | **Confidence** — 92

### S3.3 — Remove obsolete TLS fields from `Http2Config` + `SecurityConfig`
- **LOCATE** — `Grep("tls_cert\\|tls_key\\|cors_\\|rate_limit_", type=rust, path="crates/rivers-core-config")`
- **EDIT** — delete fields. `Http2Config` keeps only `enabled, initial_window_size, max_concurrent_streams`. `SecurityConfig` keeps only `admin_ip_allowlist`.
- **VERIFY** — `cargo build -p rivers-core-config` → exit 0.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S3.4 — `validate_tls_config` startup step
- **LOCATE** — `Grep("validate_http2_runtime", type=rust)`
- **EDIT** — replace with `validate_tls_config`: bail if `[base.tls]` missing. If `cert`/`key` declared, check file exists.
- **VERIFY** — startup without `[base.tls]` → hard error.
- **Tokens** — 2K–3K | **Confidence** — 90

### S3.5 — `maybe_autogen_tls_cert` with rcgen
- **READ** — rcgen docs / example
- **EDIT** — if `cert`/`key` absent but `[base.tls.x509]` present: call `rcgen::generate_simple_self_signed(san)` with configured CN/days. Write cert.pem + key.pem under instance home `tls/`. chmod 600 on key.
- **VERIFY** — missing cert → files created on boot; `openssl x509 -noout -subject` shows configured CN.
- **Tokens** — 3K–5K | **Confidence** — 82

### S3.6 — TLS termination via tokio-rustls
- **READ** — axum-server rustls example
- **EDIT** — main server: load cert/key, build `RustlsConfig`, call `axum_server::bind_rustls(addr, config).serve(...)`. Apply `min_version`, ciphers.
- **VERIFY** — `curl -k https://localhost:8080/health` → 200.
- **Tokens** — 3K–6K | **Confidence** — 78

### S3.7 — `maybe_spawn_http_redirect_server`
- **EDIT** — when `redirect != false`: spawn second Axum listener on :80 issuing 301 to `https://{host}:{base_port}{path}`. Bind failure → `tracing::warn!`, do not abort.
- **VERIFY** — `curl -I http://localhost/` → 301 Location header correct.
- **Tokens** — 2K–4K | **Confidence** — 85

### S3.8 — `riversctl tls gen`
- **LOCATE** — `crates/riversctl/src/cli.rs`
- **EDIT** — subcommand `gen` — calls same `autogen_tls_cert` fn, writes to paths in riversd.toml.
- **VERIFY** — `riversctl tls gen` writes files; `openssl x509` succeeds.
- **Tokens** — 2K–4K | **Confidence** — 88

### S3.9 — `riversctl tls show`
- **EDIT** — read cert via `rustls_pemfile`, parse with `x509-parser`, print CN, SAN, not-before, not-after, "N days left".
- **VERIFY** — `riversctl tls show` prints all fields.
- **Tokens** — 2K–4K | **Confidence** — 85

### S3.10 — `riversctl tls renew`
- **EDIT** — wrapper: show existing cert → generate new → replace atomically.
- **VERIFY** — manual.
- **Tokens** — 2K–3K | **Confidence** — 85

### S3.11 — `riversctl tls request` / `import` / `list` / `expire`
- **Split-if** — one task each.
- **Tokens** — 1.5K–3K each | **Confidence** — 82

### S3.12 — `--no-ssl` escape hatch
- **EDIT** — CLI flag disables TLS for process lifetime. Emit `tracing::warn!("TLS disabled via --no-ssl; do not use in production")` on boot. Never persisted.
- **Tokens** — 1K–2K | **Confidence** — 95

### S3.13 — Address-book bundle TLS config
- **LOCATE** — `address-book-bundle/*/app.toml`, `address-book-bundle/*/manifest.toml`
- **EDIT** — add `[base.tls]` + `[base.tls.x509]` to both apps. Service: `redirect = false`. Main datasource: `skip_verify = true`. Manifests: `entryPoint = "https://..."`.
- **VERIFY** — `just deploy-address-book` + `curl -k https://localhost:8080` → 200.
- **Tokens** — 2K–3K | **Confidence** — 90

---

# SPRINT 4 — StorageEngine Backends

### S4.1 — Add `sqlx` workspace dep (sqlite feature)
- **Tokens** — 1K–2K | **Confidence** — 95

### S4.2 — Add `redis` + `deadpool-redis` workspace deps
- **Tokens** — 1K–2K | **Confidence** — 95

### S4.3 — `SqliteStorageEngine` scaffolding
- **EDIT** — new `crates/rivers-storage-backends/src/sqlite.rs`. Init WAL mode. `CREATE TABLE kv_store(key TEXT PRIMARY KEY, value BLOB, expires_at INTEGER)`.
- **VERIFY** — unit test: init twice → idempotent.
- **Tokens** — 3K–5K | **Confidence** — 88

### S4.4 — Sqlite `get`/`set`/`del`
- **Depends** — S4.3
- **EDIT** — impl trait methods. `set_with_ttl` stores `expires_at = now + ttl`. `get` returns None when expired.
- **VERIFY** — test set+get round-trip; expired key returns None.
- **Tokens** — 2K–4K | **Confidence** — 90

### S4.5 — Sqlite background `flush_expired`
- **Depends** — S4.4
- **EDIT** — tokio task every 60s: `DELETE FROM kv_store WHERE expires_at < ?`.
- **VERIFY** — count decreases after TTL + sweep.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S4.6 — `RedisStorageEngine` scaffolding
- **EDIT** — new `crates/rivers-storage-backends/src/redis.rs`. Deadpool pool. Connection check on init.
- **VERIFY** — unit test with real 192.168.2.206 Redis.
- **Tokens** — 3K–5K | **Confidence** — 85

### S4.7 — Redis `get`/`set`/`del` with TTL
- **Depends** — S4.6
- **EDIT** — `SET key value EX <ttl>` when ttl; else `SET`. `DEL`.
- **VERIFY** — round-trip + TTL expire test.
- **Tokens** — 2K–3K | **Confidence** — 90

### S4.8 — Redis single-node sentinel (SHAPE-8)
- **Depends** — S4.6
- **EDIT** — startup: `KEYS rivers:node:*` (or SCAN). If match → bail `"Another Rivers node detected. Multi-node requires RPS."`. Then `SET rivers:node:{node_id} heartbeat EX 60`. Tokio task refreshes every 30s.
- **VERIFY** — start two instances against same Redis → 2nd fails.
- **Tokens** — 3K–5K | **Confidence** — 82

### S4.9 — `ctx.store` namespace enforcement
- **LOCATE** — host-side store binding (Grep for `ctx_store` or `Rivers.store`)
- **EDIT** — all reads/writes prefix with `app:{app_id}:`. Reject keys starting with reserved prefixes `session:`, `csrf:`, `cache:`, `raft:`, `rivers:`.
- **VERIFY** — handler writing `session:foo` → error.
- **Tokens** — 2K–4K | **Confidence** — 88

### S4.10 — L1 cache data structures
- **EDIT** — new `crates/rivers-runtime/src/dataview/l1.rs`. `HashMap<String, Arc<QueryResult>>` + `VecDeque<String>` LRU. `l1_max_bytes` + `l1_max_entries` cap (default 100k).
- **VERIFY** — unit test eviction once cap exceeded.
- **Tokens** — 4K–6K | **Confidence** — 80

### S4.11 — `QueryResult::estimated_bytes`
- **LOCATE** — `Grep("struct QueryResult", type=rust)`
- **EDIT** — method returning approximation (sum of row sizes).
- **VERIFY** — trivial unit test.
- **Tokens** — 1.5K–3K | **Confidence** — 88

### S4.12 — L2 oversize skip
- **Depends** — S4.10, S4.11
- **EDIT** — before L2 write, if `estimated_bytes > l2_max_value_bytes` → skip + metric counter inc.
- **VERIFY** — unit test large result → not stored.
- **Tokens** — 1.5K–3K | **Confidence** — 92

---

# SPRINT 5 — ProcessPool V8 Foundation

### S5.1 — Add `v8` workspace dep + platform init
- **EDIT** — `rivers-engine-v8/Cargo.toml`. `v8::Platform::new` called once in cdylib `init`. Set `--disallow-code-generation-from-strings` flag.
- **VERIFY** — `cargo build -p rivers-engine-v8` → exit 0.
- **Tokens** — 2K–4K | **Confidence** — 82

### S5.2 — V8 worker: create isolate
- **Depends** — S5.1
- **EDIT** — spawn N worker threads. Each holds one `v8::OwnedIsolate` with configured heap limits. Idle loop receives tasks via mpsc.
- **VERIFY** — pool start + shutdown test; no leaks.
- **Tokens** — 4K–6K | **Confidence** — 78

### S5.3 — V8: per-task context bind/unbind
- **Depends** — S5.2
- **EDIT** — per task: `v8::Context::new`, compile script, run entrypoint, drop context, return isolate.
- **VERIFY** — run 10 tasks → no state bleeds between them (assert via global counter in JS: should always be 1).
- **Tokens** — 4K–7K | **Confidence** — 72

### S5.4 — Inject Application scope (Rivers.*)
- **Depends** — S5.3
- **EDIT** — before script runs, create `Rivers` global object with empty-stub methods.
- **VERIFY** — `typeof Rivers` === `"object"`.
- **Tokens** — 3K–5K | **Confidence** — 80

### S5.5 — Inject Request scope (trace_id, request, resdata)
- **Depends** — S5.4
- **EDIT** — `ctx` global with request fields from TaskContext.
- **VERIFY** — handler `return ctx.trace_id` matches outer.
- **Tokens** — 2K–4K | **Confidence** — 82

### S5.6 — Inject Session scope (when present)
- **Depends** — S5.5
- **EDIT** — `ctx.session` populated when TaskContext has session.
- **Tokens** — 2K–4K | **Confidence** — 80

### S5.7 — Inject Connection scope (WS/SSE)
- **Depends** — S5.5
- **EDIT** — `ctx.ws = { connection_id, message }` only in WS/SSE handler execution.
- **Tokens** — 2K–4K | **Confidence** — 78

### S5.8 — Watchdog thread
- **EDIT** — one thread per pool. For each active worker, check elapsed vs `task_timeout_ms`. If exceeded, call `isolate.terminate_execution()`.
- **VERIFY** — handler `while(true){}` → killed within timeout+1s.
- **Tokens** — 3K–5K | **Confidence** — 78

### S5.9 — `NearHeapLimitCallback`
- **Depends** — S5.8
- **EDIT** — register callback. On near-limit: signal watchdog to terminate; return larger limit slightly so V8 survives until termination.
- **VERIFY** — allocating ballooning object triggers callback before OOM.
- **Tokens** — 2K–4K | **Confidence** — 75

### S5.10 — Isolate recycling at heap threshold
- **Depends** — S5.3
- **EDIT** — after unbind, if `heap_used > recycle_heap_threshold_pct * max_heap` OR task_count > `recycle_after_tasks` → drop isolate, spawn new.
- **VERIFY** — after N tasks, isolate counter increments.
- **Tokens** — 3K–5K | **Confidence** — 80

### S5.11 — swc TypeScript compiler at bundle load
- **EDIT** — new `crates/rivers-runtime/src/ts_compile.rs`. Add `swc_common`, `swc_ecma_parser`, `swc_ecma_codegen`. Compile `.ts` to JS. Cache on disk by mtime.
- **VERIFY** — sample `.ts` handler compiles + runs.
- **Tokens** — 4K–7K | **Confidence** — 72

### S5.12 — `Rivers.log` binding
- **EDIT** — host callback `host_log(level, msg)` → `AppLogRouter`. Stamps trace_id, app_id.
- **VERIFY** — handler `Rivers.log.info("x")` → line in `log/apps/<app>.log`.
- **Tokens** — 2K–4K | **Confidence** — 85

### S5.13 — `Rivers.crypto.hashPassword/verifyPassword`
- **EDIT** — argon2id via `argon2` crate, exposed as host callbacks.
- **VERIFY** — round-trip test passes.
- **Tokens** — 2K–4K | **Confidence** — 88

### S5.14 — `Rivers.crypto.randomHex/randomBase64url`
- **EDIT** — `getrandom::getrandom()` + encoding.
- **VERIFY** — distinct outputs; correct length.
- **Tokens** — 1.5K–3K | **Confidence** — 95

### S5.15 — `Rivers.crypto.hmac`
- **EDIT** — `hmac` crate, SHA-256 default.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S5.16 — `Rivers.crypto.timingSafeEqual`
- **EDIT** — constant-time XOR accumulation; no short-circuit.
- **Tokens** — 1K–2K | **Confidence** — 95

### S5.17 — `ctx.dataview(name, params)` capability binding
- **EDIT** — host callback resolves name to opaque token; rejects unknown with `CapabilityError`.
- **VERIFY** — handler calling unknown dataview → throws CapabilityError.
- **Tokens** — 3K–5K | **Confidence** — 80

### S5.18 — Worker crash recovery
- **EDIT** — supervise workers. Dead worker → respawn + emit `WorkerCrash` event. `WorkerPoolDegraded` when healthy < N/2.
- **VERIFY** — kill worker thread manually → new worker replaces it.
- **Tokens** — 3K–5K | **Confidence** — 78

---

# SPRINT 6 — ProcessPool WASM Foundation

### S6.1 — Add `wasmtime` dep
- **Tokens** — 1K–2K | **Confidence** — 95

### S6.2 — Engine config
- **EDIT** — `Config` with AOT, epoch interruption.
- **Tokens** — 1.5K–3K | **Confidence** — 88

### S6.3 — Instance pool + per-task context
- Similar to S5.2/S5.3 but Wasmtime.
- **Tokens** — 4K–6K | **Confidence** — 75

### S6.4 — Epoch preemption watchdog
- **EDIT** — increment engine epoch on timer; `Trap::Interrupt` on deadline.
- **Tokens** — 2K–4K | **Confidence** — 80

### S6.5 — WASI restriction
- **EDIT** — WasiCtx with no filesystem, stdio → log, network gated by capability.
- **Tokens** — 3K–5K | **Confidence** — 78

### S6.6 — Host function: `rivers.db_query`
- **Tokens** — 2K–4K | **Confidence** — 78

### S6.7 — Host function: `rivers.log_*`
- **Tokens** — 2K–3K | **Confidence** — 85

### S6.8 — Host functions: `rivers.crypto_*`
- Mirror S5.13-16.
- **Tokens** — 3K–5K | **Confidence** — 80

### S6.9 — Host functions: `rivers.store_*`
- **Tokens** — 2K–4K | **Confidence** — 80

### S6.10 — WASM → AppLogRouter
- **Tokens** — 2K–4K | **Confidence** — 88

---

# SPRINT 7 — Handler Pipeline

### S7.1 — Four-label executor
- **EDIT** — `execute_pipeline(ctx, view_config)` runs `pre_process` → DataViews → `handlers` → `post_process`. Each stage returns Result; on error invoke `on_error`.
- **VERIFY** — integration test of each stage firing in order.
- **Tokens** — 4K–6K | **Confidence** — 80

### S7.2 — `on_session_valid` stage
- **Depends** — S7.1
- **EDIT** — fires after session validation, before `pre_process`.
- **Tokens** — 2K–4K | **Confidence** — 85

### S7.3 — `on_error` observer
- **EDIT** — fire-and-forget tokio::spawn; never block response.
- **Tokens** — 2K–4K | **Confidence** — 85

### S7.4 — `on_timeout` observer
- **Tokens** — 2K–3K | **Confidence** — 85

### S7.5 — Handler header blocklist
- **EDIT** — filter out `set-cookie`, `access-control-*`, `host`, `transfer-encoding`, `connection`, `upgrade`, `x-forwarded-for` from handler-emitted headers.
- **VERIFY** — handler setting `set-cookie` → header not present in response.
- **Tokens** — 2K–3K | **Confidence** — 95

### S7.6 — Primary DataView → `ctx.resdata`
- **EDIT** — after DataView stage, if view has `primary`, copy its result to `ctx.resdata`; others under `ctx.data.{name}`.
- **VERIFY** — JS handler reads `ctx.resdata` without explicit fetch.
- **Tokens** — 3K–5K | **Confidence** — 88

### S7.7 — `ctx.streamDataview` host binding
- **EDIT** — host callback returns async iterator driving driver `stream` op.
- **VERIFY** — test stream iterates rows.
- **Tokens** — 4K–6K | **Confidence** — 72

---

# SPRINT 8 — DataView Engine Completion

### S8.1 — Per-method query fields
- **LOCATE** — `Grep("get_query\\|post_query", type=rust)`
- **EDIT** — `DataViewConfig` has `get_query`, `post_query`, `put_query`, `delete_query`. Serde aliases: `query → get_query`, `return_schema → get_schema`.
- **VERIFY** — legacy TOML with `query = "..."` still parses.
- **Tokens** — 3K–5K | **Confidence** — 88

### S8.2 — Per-method schemas
- **EDIT** — `get_schema`, `post_schema`, `put_schema`, `delete_schema` fields. Runtime picks based on HTTP method.
- **Tokens** — 2K–4K | **Confidence** — 90

### S8.3 — Per-method parameter arrays
- **EDIT** — `[[data.dataviews.X.get.parameters]]` arrays (new). Runtime uses method-specific set.
- **Tokens** — 3K–5K | **Confidence** — 85

### S8.4 — `$name` placeholder parser
- **EDIT** — new helper `translate_placeholders(sql, driver) -> (native_sql, param_order)`. Postgres: `$1, $2...` preserving declared order. MySQL: `?` with declared order. SQLite: `:name`.
- **VERIFY** — table tests per driver.
- **Tokens** — 4K–7K | **Confidence** — 78

### S8.5 — Pseudo DataView builder API (JS side)
- **EDIT** — expose `ctx.datasource(name)` returning chainable object: `fromQuery(sql).withGetSchema(json).build()`.
- **VERIFY** — JS test: build + execute in handler.
- **Tokens** — 4K–6K | **Confidence** — 78

### S8.6 — `.build()` syntax-checks schema
- **Depends** — S8.5
- **EDIT** — call `SchemaSyntaxChecker` at build(); throw on invalid.
- **Tokens** — 2K–4K | **Confidence** — 82

### S8.7 — `invalidates` cache clearing
- **EDIT** — after write DataView succeeds, clear L1+L2 for each name in `invalidates`.
- **VERIFY** — integration: post then get → bypasses cache.
- **Tokens** — 2K–4K | **Confidence** — 88

### S8.8 — `max_rows` truncation
- **EDIT** — after driver returns rows, `truncate(max_rows)`. Emit WARN event with count when truncated.
- **VERIFY** — query returning 10k rows with `max_rows=100` returns 100.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S8.9 — Datasource observer hook
- **EDIT** — before/after query, emit EventBus event `DatasourceQuery{name, op, duration}`.
- **Tokens** — 2K–4K | **Confidence** — 85

---

# SPRINT 9 — Transactions & Batch

### S9.1 — `TransactionMap`
- **EDIT** — `HashMap<TraceId, Connection>` under Mutex. Bound by request lifetime.
- **VERIFY** — unit test begin+commit cycle.
- **Tokens** — 3K–5K | **Confidence** — 82

### S9.2 — `host_db_begin`
- **Depends** — S9.1
- **EDIT** — host callback acquires pool conn, issues BEGIN, stores in TransactionMap keyed by trace_id.
- **VERIFY** — JS `Rivers.db.begin("dv")` returns token.
- **Tokens** — 3K–5K | **Confidence** — 80

### S9.3 — `host_db_commit` / `host_db_rollback`
- Similar. Clean map on return.
- **Tokens** — 3K–5K | **Confidence** — 82

### S9.4 — Auto-rollback on request end
- **EDIT** — middleware: on response, any lingering TransactionMap entry for this trace_id → rollback + WARN.
- **Tokens** — 2K–4K | **Confidence** — 82

### S9.5 — Driver `execute_batch`
- **EDIT** — trait method `execute_batch(stmts, on_error) -> Vec<Result>`. Postgres impl uses multi-stmt.
- **Tokens** — 3K–5K | **Confidence** — 78

### S9.6 — `Rivers.db.batch` JS API
- **Depends** — S9.5
- **EDIT** — host callback. `onError: "fail_fast" | "continue"`.
- **Tokens** — 3K–5K | **Confidence** — 80

### S9.7 — Prepared-statement cache per conn
- **EDIT** — `Connection` holds `HashMap<sql_hash, PreparedStatement>`. DataView `prepared = true` enables.
- **Tokens** — 3K–5K | **Confidence** — 78

### S9.8 — Integration test Postgres tx roundtrip
- **EDIT** — against 192.168.2.209. begin + insert + commit + select.
- **Tokens** — 3K–5K | **Confidence** — 85

### S9.9 — Integration test MySQL tx roundtrip
- **Tokens** — 3K–5K | **Confidence** — 85

### S9.10 — Integration test SQLite tx roundtrip
- **Tokens** — 2K–4K | **Confidence** — 88

---

# SPRINT 10 — Auth & Sessions

### S10.1 — Guard view registration
- **EDIT** — view config `view_type = "Guard"` + `[api.views.<name>]`. Framework wires guard before route dispatch.
- **Tokens** — 3K–5K | **Confidence** — 80

### S10.2 — `IdentityClaims` contract
- **EDIT** — shared struct; guard CodeComponent returns JSON matching.
- **Tokens** — 2K–3K | **Confidence** — 88

### S10.3 — Session mint with 256-bit CSPRNG
- **EDIT** — `session_id = hex(getrandom(32))`. Store in `session:` namespace with ttl.
- **VERIFY** — same token never repeats across 1M mints.
- **Tokens** — 2K–4K | **Confidence** — 90

### S10.4 — HttpOnly Secure cookie delivery
- **EDIT** — `Set-Cookie: session={token}; HttpOnly; Secure; SameSite=Lax; Path=/`.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S10.5 — API-mode body token delivery
- **EDIT** — if `Accept: application/json` → response body `{_response: {token}}`.
- **Tokens** — 1.5K–3K | **Confidence** — 88

### S10.6 — Session renewal on activity
- **EDIT** — middleware bumps `expires_at` within idle window.
- **Tokens** — 2K–4K | **Confidence** — 85

### S10.7 — Logout invalidation
- **EDIT** — endpoint clears `session:<id>` from store, clears cookie.
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S10.8 — CSRF double-submit cookie
- **EDIT** — mint 256-bit token on session create. Cookie `XSRF-TOKEN`. Middleware checks matching header `X-XSRF-TOKEN` on state-changing methods.
- **Tokens** — 3K–5K | **Confidence** — 82

### S10.9 — CSRF bearer exemption
- **EDIT** — skip CSRF when `Authorization: Bearer`.
- **Tokens** — 1K–2K | **Confidence** — 92

### S10.10 — WS session revalidation timer
- **EDIT** — per-connection tokio task checks session validity at configured interval; closes on expire.
- **Tokens** — 2K–4K | **Confidence** — 82

### S10.11 — SSE session revalidation
- **Tokens** — 2K–4K | **Confidence** — 82

### S10.12 — Forward `Authorization` + `X-Rivers-Claims`
- **EDIT** — HTTP driver injects both when handler calls app-service datasource.
- **Tokens** — 2K–4K | **Confidence** — 82

### S10.13 — MessageConsumer auth exemption default
- **EDIT** — unless `auth = "session"`, skip session requirement.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S10.14 — Invalid-session redirect
- **EDIT** — on missing/expired session + view declares `redirect_to`, 302 to path. Else 401 ErrorResponse.
- **Tokens** — 2K–3K | **Confidence** — 88

---

# SPRINT 11 — View Types

### S11.1 — WebSocket broadcast registry
- **EDIT** — `ConnectionRegistry` with `HashMap<ViewName, Vec<WsSender>>`. Broadcast fn fans out.
- **Tokens** — 4K–6K | **Confidence** — 80

### S11.2 — WebSocket direct send
- **Depends** — S11.1
- **EDIT** — lookup by `connection_id` in registry.
- **Tokens** — 2K–4K | **Confidence** — 82

### S11.3 — WS per-connection rate limit
- **Depends** — S11.1
- **EDIT** — token bucket per conn; drops frames exceeding.
- **Tokens** — 2K–4K | **Confidence** — 82

### S11.4 — WS `on_stream` handler dispatch
- **EDIT** — each inbound frame → handler invocation with `ctx.ws.message`.
- **Tokens** — 3K–5K | **Confidence** — 80

### S11.5 — WS lag detection + drain
- **Tokens** — 3K–5K | **Confidence** — 75

### S11.6 — SSE push loop
- **EDIT** — `tokio::select!` on tick interval + EventBus subscription. Emit `data: {json}\n\n`.
- **Tokens** — 4K–6K | **Confidence** — 80

### S11.7 — SSE `Last-Event-ID` reconnection
- **Tokens** — 2K–4K | **Confidence** — 82

### S11.8 — MessageConsumer subscribe + dispatch
- **EDIT** — per-view EventBus subscription → handler invocation.
- **Tokens** — 4K–6K | **Confidence** — 80

### S11.9 — Streaming REST generator drive loop
- **EDIT** — host binding returns AsyncGenerator; framework pulls chunks + writes NDJSON/SSE.
- **Tokens** — 5K–8K | **Confidence** — 75

### S11.10 — Streaming REST client-disconnect detection
- **EDIT** — body write errors → cancel generator.
- **Tokens** — 2K–4K | **Confidence** — 82

### S11.11 — GraphQL router at `/graphql`
- **EDIT** — add `async-graphql` + `async-graphql-axum`. Register route.
- **Tokens** — 3K–5K | **Confidence** — 78

### S11.12 — GraphQL resolver → CodeComponent bridge
- **Tokens** — 5K–8K | **Confidence** — 70

### S11.13 — MCP JSON-RPC dispatcher
- **READ** — `docs/arch/rivers-mcp-view-spec.md` §3
- **EDIT** — single POST /mcp route. Parse JSON-RPC. Route `initialize`, `tools/list`, `tools/call`, `resources/list`, `prompts/list`, `prompts/get`.
- **Tokens** — 6K–9K | **Confidence** — 72

### S11.14 — MCP tools whitelist → DataView execute
- **Depends** — S11.13
- **EDIT** — `[api.views.<mcp>.tools.<name>]` with `dataview = "..."`. On `tools/call`, validate whitelist and execute via engine.
- **Tokens** — 4K–6K | **Confidence** — 78

### S11.15 — MCP prompts + instructions
- **EDIT** — markdown templates with `{arg}` substitution; instructions served from declared md file.
- **Tokens** — 3K–5K | **Confidence** — 80

### S11.16 — MCP streaming tools (SSE mode)
- **Tokens** — 4K–6K | **Confidence** — 72

### S11.17 — MCP session management
- **EDIT** — optional session via `guard = "..."`. Framework creates MCP session on first call.
- **Tokens** — 3K–5K | **Confidence** — 78

---

# SPRINT 12 — Built-in Drivers

### S12.1 — Postgres `query` + prepared stmt
- **EDIT** — tokio-postgres execute. Cache `Statement` objects.
- **Tokens** — 4K–6K | **Confidence** — 85

### S12.2 — Postgres transactions
- **Tokens** — 3K–5K | **Confidence** — 85

### S12.3 — Postgres `introspect_columns`
- **EDIT** — `SELECT column_name FROM information_schema.columns WHERE table_name = $1`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S12.4 — MySQL `query` + `execute`
- **Tokens** — 4K–6K | **Confidence** — 82

### S12.5 — MySQL transactions
- **Tokens** — 3K–5K | **Confidence** — 82

### S12.6 — MySQL `introspect_columns`
- **Tokens** — 2K–4K | **Confidence** — 85

### S12.7 — SQLite WAL + named params
- **EDIT** — rusqlite. Enable WAL pragma. `:name` binding.
- **Tokens** — 3K–5K | **Confidence** — 88

### S12.8 — SQLite path fallback
- **EDIT** — honor `database=` OR `host=`. Create parent dir via `fs::create_dir_all`.
- **Tokens** — 2K–4K | **Confidence** — 90

### S12.9 — SQLite `:memory:` support
- **Tokens** — 1K–2K | **Confidence** — 95

### S12.10 — Redis built-ins
- **EDIT** — GET/MGET/HGET/HGETALL/LRANGE/SMEMBERS/SET/DEL/EXPIRE.
- **Tokens** — 4K–6K | **Confidence** — 85

### S12.11 — Redis admin denylist
- **EDIT** — `admin_operations()` returns `["flushdb", "flushall", "config_set"]`. Reject with `Forbidden`.
- **Tokens** — 2K–3K | **Confidence** — 92

### S12.12 — Memcached driver
- **EDIT** — async-memcached wrapper. GET/SET/DEL.
- **Tokens** — 3K–5K | **Confidence** — 82

### S12.13 — Faker: primitive generators
- **EDIT** — per field `faker` attribute → generate uuid/name/email/etc.
- **Tokens** — 3K–5K | **Confidence** — 88

### S12.14 — HTTP driver: `reqwest` execute
- **EDIT** — path templating, body templating, array→rows, object→row.
- **Tokens** — 5K–8K | **Confidence** — 78

### S12.15 — HTTP driver auth: Bearer
- **Tokens** — 2K–3K | **Confidence** — 90

### S12.16 — HTTP driver auth: Basic
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S12.17 — HTTP driver auth: API key
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S12.18 — HTTP driver auth: OAuth2 client_credentials
- **EDIT** — token fetch + refresh; cache until expiry.
- **Tokens** — 4K–6K | **Confidence** — 78

### S12.19 — HTTP driver retry + circuit breaker
- **Depends** — S0.6
- **Tokens** — 3K–5K | **Confidence** — 80

### S12.20 — DDL guard: `is_ddl_statement`
- **EDIT** — regex-ish prefix check for CREATE/ALTER/DROP/TRUNCATE.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S12.21 — DDL guard: `Connection::execute` gate
- **Depends** — S12.20
- **EDIT** — return `Forbidden` when DDL unless called via `ddl_execute`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S12.22 — DDL guard: `Connection::ddl_execute`
- **Depends** — S12.21
- **EDIT** — separate path; only called from ApplicationInit context.
- **Tokens** — 2K–4K | **Confidence** — 85

### S12.23 — DDL whitelist in `riversd.toml`
- **EDIT** — parse `ddl_whitelist = ["db@appId"]`. Check before ddl_execute.
- **Tokens** — 2K–3K | **Confidence** — 92

---

# SPRINT 13 — Plugin ABI v2 (per-plugin tasks)

**Rule:** one task per plugin + common ABI tasks.

### S13.C1 — New C-ABI exports shape
- **EDIT** — `rivers-driver-sdk/src/abi.rs`: C functions `_rivers_driver_connect(json_ptr, len) -> handle`, `_rivers_driver_execute(handle, json_ptr, len) -> (json_ptr, len)`, `_rivers_driver_close(handle)`, `_rivers_driver_free(ptr, len)`. JSON-over-buffers.
- **Tokens** — 5K–8K | **Confidence** — 75

### S13.C2 — Plugin-managed tokio runtime helper
- **EDIT** — `PluginRuntime` struct wrapping `tokio::Runtime`. `block_on` helper.
- **Tokens** — 3K–5K | **Confidence** — 82

### S13.C3 — `_rivers_compile_check` export
- **Depends** — S13.C1
- **EDIT** — checks schema structurally; returns JSON errors.
- **Tokens** — 3K–5K | **Confidence** — 85

### S13.C4 — ABI version bump
- **EDIT** — bump `_rivers_abi_version` constant. Add catch_unwind wrapper.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S13.P1–P12 — Migrate each plugin
- **Split** — 12 separate tasks: cassandra, couchdb, elasticsearch, exec, influxdb, kafka, ldap, mongodb, nats, neo4j, rabbitmq, redis-streams.
- **READ** — `crates/rivers-plugin-<name>/src/lib.rs`
- **EDIT** — wrap async methods with PluginRuntime::block_on. Expose C-ABI per S13.C1.
- **VERIFY** — `cargo build -p rivers-plugin-<name>` → exit 0 ; integration smoke test.
- **Tokens** — 4K–7K each | **Confidence** — 75

---

# SPRINT 14 — Polling Views

### S14.1 — Poll loop scheduler
- **EDIT** — `PollLoop { view, params, tick }` spawned per unique key.
- **Tokens** — 4K–6K | **Confidence** — 80

### S14.2 — Dedup by canonical key
- **Depends** — S0.8
- **EDIT** — `HashMap<poll_key, Arc<PollLoop>>`. Subscribe reuses.
- **Tokens** — 2K–4K | **Confidence** — 85

### S14.3 — Diff strategy: `hash`
- **EDIT** — SHA-256 compare prev vs current.
- **Tokens** — 2K–3K | **Confidence** — 92

### S14.4 — Diff strategy: `null`
- **EDIT** — trigger on null↔non-null transition.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S14.5 — Diff strategy: `change_detect`
- **EDIT** — invoke CodeComponent `change_detect(prev, curr) -> bool`.
- **Tokens** — 3K–5K | **Confidence** — 80

### S14.6 — `PollChangeDetectTimeout` event
- **Depends** — S14.5
- **EDIT** — on timeout, emit event with `consecutive_timeouts` counter.
- **Tokens** — 2K–4K | **Confidence** — 85

### S14.7 — Poll state persistence
- **EDIT** — `poll:{view}:{hash}` → JSON { last_result_hash, last_run }.
- **Tokens** — 2K–4K | **Confidence** — 85

---

# SPRINT 15 — Bundle Validation (4-Layer)

### S15.1 — `deny_unknown_fields` on all config structs
- **LOCATE** — `Grep("#\\[derive\\(.*Deserialize", type=rust, path="crates/rivers-core-config")`
- **EDIT** — add `#[serde(deny_unknown_fields)]` to each. Run `cargo test` and fix any existing tests.
- **Tokens** — 4K–7K | **Confidence** — 82

### S15.2 — Layer 1: structural validator
- **EDIT** — `validate_structural(path) -> Vec<ValidationResult>`. Catch unknown keys with S001–S010 codes.
- **Tokens** — 4K–6K | **Confidence** — 85

### S15.3 — Layer 2: existence validator
- **EDIT** — walk file refs; missing file → E001+.
- **Tokens** — 3K–5K | **Confidence** — 88

### S15.4 — Layer 3: crossref validator
- **EDIT** — DataView → datasource, View → DataView, primary resolves. Uniqueness of operation_id, path+method, port-in-bundle. X001–X013 codes.
- **Tokens** — 5K–7K | **Confidence** — 82

### S15.5 — Layer 4: syntax validator via engine dylib
- **EDIT** — dlopen engine cdylib, call `_rivers_compile_check`. Treat missing dylib as skip with W-code.
- **Tokens** — 5K–8K | **Confidence** — 72

### S15.6 — Levenshtein `did_you_mean` helper
- **EDIT** — distance ≤ 2, returned in `suggestion` field.
- **VERIFY** — unit test "datsource" → "datasource".
- **Tokens** — 2K–4K | **Confidence** — 92

### S15.7 — Error catalog completeness
- **EDIT** — ensure every code S001–S010, E001–E005, X001–X013, C001–C008, L001–L005, W001–W004 has message template.
- **Tokens** — 3K–5K | **Confidence** — 88

### S15.8 — `riverpackage validate --format json`
- **EDIT** — verify JSON output contract matches spec §8.2.
- **Tokens** — 2K–4K | **Confidence** — 90

### S15.9 — `riverpackage validate` exit codes
- **EDIT** — 0=pass, 1=errors, 2=bundle not found, 3=config error.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S15.10 — Deploy-time re-validation in riversd
- **EDIT** — on bundle load, re-run layers 3–4 against live drivers. Insert VALIDATING state in deploy state machine.
- **Tokens** — 4K–7K | **Confidence** — 78

### S15.11 — Remove `riversctl doctor --lint`
- **LOCATE** — `Grep("\\-\\-lint", type=rust, path="crates/riversctl")`
- **EDIT** — delete subcommand.
- **Tokens** — 1.5K–3K | **Confidence** — 92

---

# SPRINT 16 — Schema System v2

### S16.1 — JSON schema file loading
- **EDIT** — resolve relative paths from app.toml; parse JSON; cache by path+mtime.
- **Tokens** — 3K–5K | **Confidence** — 88

### S16.2 — `SchemaSyntaxChecker` trait
- **EDIT** — `rivers-driver-sdk::SchemaSyntaxChecker::check(schema: Value) -> Result<(), Vec<Issue>>`.
- **Tokens** — 2K–4K | **Confidence** — 90

### S16.3 — `Validator` trait (runtime)
- **EDIT** — `Validator::validate(data: Value, schema: Value) -> Result<(), Vec<Issue>>`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S16.4 — Primitive type coercion map
- **EDIT** — one fn per type: uuid/string/integer/float/decimal/boolean/email/phone/datetime/date/url/json/bytes. `min`/`max`/`pattern`.
- **Tokens** — 5K–8K | **Confidence** — 80

### S16.5 — Postgres schema checker
- **EDIT** — column-shape schema validation; type mapping table.
- **Tokens** — 3K–5K | **Confidence** — 82

### S16.6 — MySQL schema checker
- **Tokens** — 3K–5K | **Confidence** — 82

### S16.7 — SQLite schema checker
- **EDIT** — affinity model (looser).
- **Tokens** — 2K–4K | **Confidence** — 85

### S16.8 — Redis schema checker (data-structure aware)
- **EDIT** — per type: string/hash/list/set/sorted_set. Validate `key_pattern`.
- **Tokens** — 3K–5K | **Confidence** — 80

### S16.9 — Faker schema checker
- **EDIT** — verify `faker` attribute on each field; known generators.
- **Tokens** — 2K–4K | **Confidence** — 88

### S16.10 — HTTP schema checker
- **EDIT** — body object schemas, path-param schemas.
- **Tokens** — 3K–5K | **Confidence** — 82

### S16.11 — Broker schema checkers (kafka/rabbit/nats/eventbus)
- **Split-if** — 4 separate tasks.
- **Tokens** — 2K–4K each | **Confidence** — 78

### S16.12 — `x-type` build-time validation
- **EDIT** — schema field `x-type` compared against driver-declared accepted x-types.
- **Tokens** — 3K–5K | **Confidence** — 78

### S16.13 — `nopassword` annotation
- **EDIT** — schema-level flag allowed on faker/sqlite; validator rejects elsewhere.
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S16.14 — Schema introspection startup check
- **EDIT** — for each SQL datasource with `introspect = true` (default), call `introspect_columns(table)` and compare to schema. Mismatch → startup fail with Levenshtein hint.
- **Tokens** — 5K–8K | **Confidence** — 72

---

# SPRINT 17 — Admin API + Ed25519

### S17.1 — Admin Axum server init
- **EDIT** — second `axum_server` when `admin_api.enabled = true`. Subset middleware (trace, timeout, security_headers).
- **Tokens** — 3K–5K | **Confidence** — 85

### S17.2 — `GET /admin/status`
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S17.3 — `GET /admin/drivers`
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S17.4 — `GET /admin/datasources`
- **Tokens** — 2K–3K | **Confidence** — 90

### S17.5 — `POST /admin/deploy` (multipart upload)
- **Tokens** — 4K–6K | **Confidence** — 78

### S17.6 — `POST /admin/deploy/test`
- **Tokens** — 3K–5K | **Confidence** — 78

### S17.7 — `POST /admin/deploy/approve`
- **Tokens** — 2K–4K | **Confidence** — 82

### S17.8 — `POST /admin/deploy/reject`
- **Tokens** — 2K–3K | **Confidence** — 88

### S17.9 — `POST /admin/deploy/promote`
- **Tokens** — 3K–5K | **Confidence** — 78

### S17.10 — `GET /admin/deployments`
- **Tokens** — 2K–3K | **Confidence** — 88

### S17.11 — `POST /admin/shutdown`
- **EDIT** — signals ShutdownCoordinator.
- **Tokens** — 2K–3K | **Confidence** — 90

### S17.12 — Ed25519 signature verifier
- **EDIT** — add `ed25519-dalek`. Verify `X-Rivers-Signature` over canonicalized string `{method}\n{path}\n{sha256_hex(body)}\n{unix_ms}`. ±5min window.
- **Tokens** — 3K–5K | **Confidence** — 82

### S17.13 — Admin IP allowlist middleware
- **EDIT** — reject 403 if client IP outside CIDR list.
- **Tokens** — 2K–4K | **Confidence** — 88

### S17.14 — RBAC roles/permissions
- **EDIT** — `[security.admin_rbac]` roles + bindings. Deny-by-default for unknown paths.
- **Tokens** — 4K–6K | **Confidence** — 78

### S17.15 — `--no-admin-auth` flag
- **EDIT** — disable Ed25519 verify for process lifetime; WARN banner.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S17.16 — `riversctl` request signer
- **EDIT** — load Ed25519 private key; sign request body hash + timestamp.
- **Tokens** — 3K–5K | **Confidence** — 85

---

# SPRINT 18 — Circuit Breaker v2

### S18.1 — `circuitBreakerId` DataView config field
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S18.2 — App-scoped breaker registry
- **EDIT** — `HashMap<(app_id, breaker_id), Breaker>`.
- **Tokens** — 3K–5K | **Confidence** — 85

### S18.3 — `GET /admin/breakers`
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S18.4 — `POST /admin/breakers/:id/trip`
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S18.5 — `POST /admin/breakers/:id/reset`
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S18.6 — Persist state to StorageEngine
- **EDIT** — `breaker:{app}:{id} = {state, tripped_until}`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S18.7 — 503 + Retry-After when open
- **EDIT** — before DataView execute, check breaker; 503 response with `Retry-After: {remaining_seconds}`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S18.8 — `riversctl breaker list/trip/reset/status`
- **Tokens** — 3K–5K | **Confidence** — 85

---

# SPRINT 19 — Observability

### S19.1 — `ProbesConfig`
- **EDIT** — `[base.probes]` with `enabled, live_path, ready_path, startup_path`.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S19.2 — `/live` handler
- **EDIT** — always 200 `{status: "alive"}`.
- **Tokens** — 1K–2K | **Confidence** — 95

### S19.3 — `/ready` handler
- **EDIT** — 200 when bundle loaded + datasources connected; 503 otherwise.
- **Tokens** — 2K–4K | **Confidence** — 85

### S19.4 — `/startup` handler
- **EDIT** — 503 until `AppContext::startup_complete` atomic flipped.
- **Tokens** — 2K–3K | **Confidence** — 88

### S19.5 — Add `opentelemetry`, `opentelemetry-otlp`, `tracing-opentelemetry`
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S19.6 — `OtelConfig` struct
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S19.7 — OTel span: HTTP receive
- **EDIT** — enter span `http.receive` in middleware with `http.method, http.route, http.status_code`.
- **Tokens** — 3K–5K | **Confidence** — 80

### S19.8 — OTel span: DataView execute
- **EDIT** — span with `rivers.dataview, rivers.driver`.
- **Tokens** — 3K–5K | **Confidence** — 80

### S19.9 — OTel W3C propagation
- **EDIT** — extract `traceparent`; inject on outbound HTTP.
- **Tokens** — 3K–5K | **Confidence** — 80

### S19.10 — OTel exporter init
- **EDIT** — OTLP exporter at startup if `OtelConfig.enabled`. Failures → WARN, do not block.
- **Tokens** — 3K–5K | **Confidence** — 78

### S19.11 — Prometheus counters
- **EDIT** — `rivers_http_requests_total`, `rivers_engine_executions_total`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S19.12 — Prometheus histograms
- **EDIT** — `rivers_http_request_duration_ms`, `rivers_engine_execution_duration_ms`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S19.13 — Prometheus gauges
- **EDIT** — `rivers_active_connections`, `rivers_loaded_apps`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S19.14 — `/metrics` exporter on :9091
- **EDIT** — feature-gated. `metrics-exporter-prometheus`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S19.15 — AppLogRouter 10MB rotation
- **EDIT** — check file size on each write; rotate to `.log.1` when exceeding.
- **Tokens** — 2K–4K | **Confidence** — 85

---

# SPRINT 20 — OpenAPI

### S20.1 — `OpenApiConfig`
- **EDIT** — `[api.openapi]` with `enabled, path, title, version, include_playground`.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S20.2 — View metadata fields
- **EDIT** — add `summary, description, tags, operation_id, deprecated` to `ApiViewConfig`.
- **Tokens** — 2K–4K | **Confidence** — 90

### S20.3 — Add fields to known-field list
- **Depends** — S15.1
- **Tokens** — 1K–2K | **Confidence** — 95

### S20.4 — `build_openapi_document(app)` walker
- **EDIT** — new `crates/riversd/src/openapi.rs`. Walk REST views → paths/operations.
- **Tokens** — 5K–8K | **Confidence** — 75

### S20.5 — Parameter mapping → `in:` rules
- **EDIT** — path params → `in: path`, query → `in: query`, header → `in: header`.
- **Tokens** — 3K–5K | **Confidence** — 82

### S20.6 — Schema → request/response body
- **EDIT** — translate Rivers schema JSON to OpenAPI schema.
- **Tokens** — 4K–6K | **Confidence** — 78

### S20.7 — Auth modes → securitySchemes
- **Tokens** — 3K–5K | **Confidence** — 80

### S20.8 — Route registration
- **EDIT** — `GET /<bundle>/<app>/openapi.json`.
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S20.9 — Validation rules
- **EDIT** — unique `operation_id`; no duplicate path+method; fail on regen error.
- **Tokens** — 2K–4K | **Confidence** — 88

### S20.10 — Integration test address-book
- **Tokens** — 2K–3K | **Confidence** — 88

---

# SPRINT 21 — AsyncAPI

### S21.1 — `AsyncApiConfig`
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S21.2 — Walk WS/SSE/MessageConsumer/Streaming views
- **EDIT** — build channels document.
- **Tokens** — 5K–8K | **Confidence** — 72

### S21.3 — Message schemas → AsyncAPI message objects
- **Tokens** — 3K–5K | **Confidence** — 75

### S21.4 — Route registration
- **Tokens** — 1K–2K | **Confidence** — 90

---

# SPRINT 22 — Standards API Auth

### S22.1 — `AuthConfig` modes enum
- **EDIT** — `Mode::{Guard, Jwt, Oidc, ApiKey}`.
- **Tokens** — 2K–3K | **Confidence** — 90

### S22.2 — JWT verifier
- **EDIT** — add `jsonwebtoken`. Algorithm allowlist. aud/iss/exp/nbf check.
- **Tokens** — 4K–6K | **Confidence** — 80

### S22.3 — JWKS fetch + cache
- **Depends** — S22.2
- **EDIT** — HTTP GET with cache, 10-min TTL, refresh on `kid` miss.
- **Tokens** — 3K–5K | **Confidence** — 78

### S22.4 — OIDC discovery
- **EDIT** — fetch `/.well-known/openid-configuration`; extract `jwks_uri`.
- **Tokens** — 3K–5K | **Confidence** — 78

### S22.5 — API key verifier
- **EDIT** — header + DataView lookup. `timingSafeEqual` for comparison.
- **Tokens** — 3K–5K | **Confidence** — 82

---

# SPRINT 23 — Hot Reload

### S23.1 — Add `notify` workspace dep
- **Tokens** — 1K–2K | **Confidence** — 95

### S23.2 — mtime polling watcher
- **EDIT** — poll config dir every 2s; detect changes.
- **Tokens** — 3K–5K | **Confidence** — 82

### S23.3 — Atomic config swap
- **EDIT** — `RwLock<Arc<Config>>`. Writer replaces; readers see old via snapshot.
- **Tokens** — 3K–5K | **Confidence** — 80

### S23.4 — Reload surfaces: routes, DataViews, static, security
- **Tokens** — 3K–5K | **Confidence** — 78

### S23.5 — Explicit no-reload boundaries
- **EDIT** — pool, plugins, server socket → NOT reloaded.
- **Tokens** — 1.5K–3K | **Confidence** — 90

---

# SPRINT 24 — Deployment

### S24.1 — `cargo deploy` static mode
- **Tokens** — 3K–5K | **Confidence** — 85

### S24.2 — `cargo deploy` dynamic mode binary copy
- **Tokens** — 3K–5K | **Confidence** — 85

### S24.3 — Copy engine dylibs to `lib/`
- **Tokens** — 2K–4K | **Confidence** — 88

### S24.4 — Copy plugin dylibs
- **Tokens** — 2K–4K | **Confidence** — 88

### S24.5 — Absolute path rewrite in deployed config
- **Tokens** — 2K–4K | **Confidence** — 90

### S24.6 — Generate TLS cert at deploy target
- **Tokens** — 2K–3K | **Confidence** — 88

### S24.7 — Cross-compile macOS→Linux
- **EDIT** — `cross` toolchain; custom Docker with libc headers.
- **Tokens** — 4K–7K | **Confidence** — 72

### S24.8 — Docker runtime image
- **EDIT** — debian-slim; copy binary + dylibs; entrypoint.
- **Tokens** — 3K–5K | **Confidence** — 80

### S24.9 — `riverpackage init` scaffolder
- **EDIT** — emit manifest, resources.toml, app.toml, schemas for `--driver` choice.
- **Tokens** — 4K–7K | **Confidence** — 82

---

# SPRINT 25 — LockBox CLI

### S25.1 — `lockbox init`
- **Tokens** — 2K–4K | **Confidence** — 85

### S25.2 — `lockbox add`
- **Tokens** — 2K–4K | **Confidence** — 88

### S25.3 — `lockbox list` (names only)
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S25.4 — `lockbox show`
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S25.5 — `lockbox alias` / `unalias`
- **Tokens** — 2K–4K | **Confidence** — 85

### S25.6 — `lockbox rotate`
- **Tokens** — 2K–4K | **Confidence** — 82

### S25.7 — `lockbox remove`
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S25.8 — `lockbox rekey` (atomic)
- **EDIT** — tmp dir + rename to replace keystore.
- **Tokens** — 3K–5K | **Confidence** — 78

### S25.9 — `lockbox validate`
- **Tokens** — 2K–4K | **Confidence** — 88

### S25.10 — Key source: env var
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S25.11 — Key source: file (chmod 600 check)
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S25.12 — Key source: ssh-agent
- **EDIT** — `ssh-agent-client` crate; query for identity.
- **Tokens** — 3K–5K | **Confidence** — 75

### S25.13 — Credential metadata fields
- **EDIT** — optional `driver, username, hosts, database` on entries.
- **Tokens** — 2K–4K | **Confidence** — 85

### S25.14 — `.meta.json` sidecar loader for dev
- **Tokens** — 2K–4K | **Confidence** — 85

---

# SPRINT 26 — Application Keystore

### S26.1 — AES-256-GCM encrypt/decrypt helpers
- **EDIT** — in `rivers-keystore-engine`. Nonce 96-bit random per encrypt.
- **Tokens** — 3K–5K | **Confidence** — 82

### S26.2 — Master key from LockBox
- **EDIT** — key name convention; decrypt to retrieve.
- **Tokens** — 2K–4K | **Confidence** — 85

### S26.3 — `Rivers.keystore.get/set`
- **EDIT** — host callback; keys scoped per app.
- **Tokens** — 3K–5K | **Confidence** — 80

### S26.4 — `Rivers.crypto.encrypt/decrypt`
- **Tokens** — 3K–5K | **Confidence** — 82

### S26.5 — `rivers-keystore` CLI subcommands
- **Split** — init, generate, list, info, delete, rotate — 6 tasks.
- **Tokens** — 2K–4K each | **Confidence** — 85

### S26.6 — Key rotation re-encryption
- **Tokens** — 3K–5K | **Confidence** — 78

---

# SPRINT 27 — App Init Handler

### S27.1 — Init handler declaration in manifest
- **EDIT** — `[init] module, entrypoint`.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S27.2 — `ApplicationInit` execution context
- **EDIT** — new context enum variant; only this variant permits ddl_execute.
- **Tokens** — 3K–5K | **Confidence** — 80

### S27.3 — `init_timeout_s` enforcement
- **EDIT** — wrap init in timeout; on fail → app FAILED state, views unregistered.
- **Tokens** — 2K–4K | **Confidence** — 88

### S27.4 — `app.cors()` init API
- **EDIT** — host callback mutates app CORS config at init time.
- **Tokens** — 3K–5K | **Confidence** — 80

### S27.5 — `[app.rate_limit]` config
- **EDIT** — fields per SHAPE-24.
- **Tokens** — 2K–4K | **Confidence** — 88

### S27.6 — Zero-downtime redeploy sequencing
- **EDIT** — start new version, health-gate, drain old, swap.
- **Tokens** — 5K–8K | **Confidence** — 68

---

# SPRINT 28 — Canary Fleet

**Priority per CLAUDE.md — "canary is our production".**

### S28.1 — Verify seven profile skeletons
- **LOCATE** — `Glob("canary-bundle/*/app.toml")`
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S28.2 — `TestResult` self-report envelope
- **EDIT** — handler helper emits `{name, status, error?, timing_ms}`.
- **Tokens** — 2K–4K | **Confidence** — 88

### S28.3 — Aggregation endpoint
- **EDIT** — canary-main DataView collects all TestResults.
- **Tokens** — 3K–5K | **Confidence** — 85

### S28.4 — Harness test: PID file
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S28.5 — Harness test: `doctor --fix`
- **Tokens** — 1.5K–3K | **Confidence** — 88

### S28.6 — Harness test: TLS gen/renew
- **Tokens** — 1.5K–3K | **Confidence** — 88

### S28.7 — Harness test: `riverpackage validate`
- **Tokens** — 1.5K–3K | **Confidence** — 90

### S28.8 — Harness test: engine loader discovery
- **Tokens** — 1.5K–3K | **Confidence** — 85

### S28.9 — Harness test: plugin ABI version
- **Tokens** — 1.5K–3K | **Confidence** — 88

### S28.10 — Param binding non-alphabetical tests (Postgres)
- **EDIT** — deliberately non-alpha param names to catch Issue #54.
- **Tokens** — 3K–5K | **Confidence** — 82

### S28.11 — Param binding test (MySQL)
- **Tokens** — 3K–5K | **Confidence** — 82

### S28.12 — Param binding test (SQLite)
- **Tokens** — 2K–4K | **Confidence** — 85

### S28.13 — DDL rejection (driver guard)
- **Tokens** — 2K–4K | **Confidence** — 88

### S28.14 — DDL rejection (wrong context)
- **Tokens** — 2K–4K | **Confidence** — 88

### S28.15 — DDL rejection (missing whitelist)
- **Tokens** — 2K–4K | **Confidence** — 88

### S28.16 — OPS: metrics presence test
- **Tokens** — 2K–4K | **Confidence** — 85

### S28.17 — OPS: AppLogRouter routing test
- **Tokens** — 2K–4K | **Confidence** — 85

### S28.18 — OPS: log rotation test
- **Tokens** — 2K–4K | **Confidence** — 82

### S28.19 — SPA conformance grid
- **EDIT** — minimal static page in canary-main reading aggregation endpoint.
- **Tokens** — 4K–6K | **Confidence** — 78

### S28.20 — `just canary` CI target
- **EDIT** — Justfile recipe spins containers + runs bundles + checks 100% pass.
- **Tokens** — 3K–5K | **Confidence** — 75

### S28.21 — Fix session cookie bug (69/69 target per memory)
- **READ** — relevant canary failure logs
- **LOCATE** — `Grep("session.*cookie\\|Set-Cookie", type=rust)`
- **EDIT** — targeted fix once reproduced.
- **Tokens** — 3K–6K | **Confidence** — 70

---

# SPRINT 29 — Query Params & Request Surface

### S29.1 — RFC 3986 query parser
- **EDIT** — percent-decoding, repeated-key collection.
- **Tokens** — 3K–5K | **Confidence** — 85

### S29.2 — `ctx.request.query` binding
- **EDIT** — `Record<string, string>` (first value wins).
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S29.3 — `ctx.request.queryAll` binding
- **EDIT** — `Record<string, string[]>`.
- **Tokens** — 1.5K–3K | **Confidence** — 92

### S29.4 — `parameter_mapping.query` subtable
- **Tokens** — 2K–4K | **Confidence** — 88

### S29.5 — `parameter_mapping.path` subtable
- **Tokens** — 2K–4K | **Confidence** — 88

### S29.6 — `parameter_mapping.body` subtable
- **Tokens** — 2K–4K | **Confidence** — 88

### S29.7 — `parameter_mapping.header` subtable
- **Tokens** — 2K–4K | **Confidence** — 88

### S29.8 — Type coercion for params
- **EDIT** — string → integer/uuid/date/array/boolean.
- **Tokens** — 3K–5K | **Confidence** — 85

### S29.9 — Array param from repeated key + comma-separated
- **Tokens** — 2K–4K | **Confidence** — 85

---

# SPRINT 30 — EventBus & Gossip

### S30.1 — EventBus datasource driver
- **EDIT** — `DatabaseDriver` impl: `execute` → publish; `stream` → subscribe.
- **Tokens** — 3K–5K | **Confidence** — 82

### S30.2 — Priority tiers scheduler
- **EDIT** — Expect → Handle → Emit → Observe. Sync handlers sequential; observers `tokio::spawn`.
- **Tokens** — 4K–6K | **Confidence** — 78

### S30.3 — `GossipPayload` struct (single-node no-op)
- **EDIT** — stub send/recv; real transport deferred.
- **Tokens** — 2K–4K | **Confidence** — 88

### S30.4 — `LogHandler` subscribes at Observe
- **EDIT** — maps event variants → log level + formats output.
- **Tokens** — 3K–5K | **Confidence** — 82

### S30.5 — DriverRegistered + PluginLoadFailed events
- **Tokens** — 2K–3K | **Confidence** — 88

---

# SPRINT 31 — Docs

### S31.1 — `tutorial-openapi.md`
- **EDIT** — enable config, view metadata, check endpoint.
- **Tokens** — 3K–5K | **Confidence** — 88

### S31.2 — `tutorial-otel.md`
- **Tokens** — 3K–5K | **Confidence** — 88

### S31.3 — `tutorial-probes.md`
- **Tokens** — 2K–4K | **Confidence** — 90

### S31.4 — `tutorial-transactions.md`
- **Tokens** — 3K–5K | **Confidence** — 85

### S31.5 — `tutorial-circuit-breaker.md`
- **Tokens** — 2K–4K | **Confidence** — 88

### S31.6 — `tutorial-api-auth.md`
- **Tokens** — 3K–5K | **Confidence** — 82

### S31.7 — `tutorial-asyncapi.md`
- **Tokens** — 2K–4K | **Confidence** — 85

### S31.8 — AI guide: `Rivers.db.*`
- **Tokens** — 2K–4K | **Confidence** — 88

### S31.9 — AI guide: `Rivers.keystore`
- **Tokens** — 2K–4K | **Confidence** — 88

### S31.10 — AI guide: `ctx.streamDataview`
- **Tokens** — 2K–4K | **Confidence** — 85

### S31.11 — AI guide: MCP configuration
- **Tokens** — 3K–5K | **Confidence** — 85

### S31.12 — Spec cross-link audit
- **EDIT** — each spec references the shaping IDs it carries; each task list lists spec.
- **Tokens** — 2K–4K | **Confidence** — 88

---

# SPRINT 32 — Deferred (do not build yet)

- S32.1 — RPS v2 provisioning
- S32.2 — Clustering (gossip + membership)
- S32.3 — Neo4j plugin

---

# Task Catalogue Summary

| Sprint | Count | Median Tokens | Notes |
|--------|-------|----------------|-------|
| 0 | 27 | 2K–4K | mostly mechanical — Haiku-ideal |
| 1 | 13 | 2K–4K | doc-only — Haiku-ideal |
| 2 | 11 | 2K–4K | wiring |
| 3 | 13 | 2K–4K | TLS |
| 4 | 12 | 2K–4K | storage |
| 5 | 18 | 2K–5K | V8 — some 70-conf items; split further |
| 6 | 10 | 2K–5K | WASM — split as needed |
| 7 | 7 | 2K–5K | pipeline |
| 8 | 9 | 2K–5K | DataView |
| 9 | 10 | 2K–5K | transactions |
| 10 | 14 | 2K–4K | auth |
| 11 | 17 | 3K–6K | view types — split MCP further if needed |
| 12 | 23 | 2K–5K | drivers |
| 13 | 16 | 3K–6K | plugins — per-plugin |
| 14 | 7 | 2K–4K | polling |
| 15 | 11 | 3K–5K | validation |
| 16 | 14 | 2K–5K | schema v2 |
| 17 | 16 | 2K–4K | admin |
| 18 | 8 | 1.5K–4K | breaker |
| 19 | 15 | 2K–4K | observability |
| 20 | 10 | 2K–5K | OpenAPI |
| 21 | 4 | 2K–5K | AsyncAPI |
| 22 | 5 | 3K–5K | API auth |
| 23 | 5 | 2K–4K | hot reload |
| 24 | 9 | 2K–5K | deploy |
| 25 | 14 | 2K–4K | lockbox |
| 26 | 11 | 2K–4K | app keystore |
| 27 | 6 | 2K–6K | init handler |
| 28 | 21 | 2K–4K | canary |
| 29 | 9 | 2K–4K | query params |
| 30 | 5 | 2K–4K | eventbus |
| 31 | 12 | 2K–4K | docs |
| **Total** | **~330 tasks** | — | — |

---

# Haiku Execution Recipe

For each task:

1. **Read phase (no edits)** — fetch every file in READ; run every LOCATE command; compare hits to task assumptions.
2. **Decide** — if hits are wildly off (e.g. expected 1 hit, found 40), STOP and report. Do not improvise.
3. **Edit phase** — follow EDIT steps literally. Use Edit for small diffs, Write only for new files.
4. **Verify phase** — run every VERIFY command. If any fails, report and roll back.
5. **Report** — output: files changed, verify commands run + outputs, ready-to-commit message.

**If a task estimate exceeds ~5K input tokens after READ phase, call Split-if and ask for the larger task to be broken down.**

---

**Next:** pull Sprint 0 tasks (S0.1–S0.27) into `todo/tasks.md` as the active queue; advance to Sprint 1 only after all Sprint 0 items show green VERIFY.
