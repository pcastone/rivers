# RXE — `rivers-plugin-exec` Review

> **Branch:** current worktree
> **Source:** user request on 2026-04-24: focus only on `rivers-plugin-exec`; consolidation will happen in a separate session.
> **Goal:** produce a source-grounded per-crate review report at `docs/review/rivers-plugin-exec.md`.

**Grounding confirmed:**
- Crate path: `crates/rivers-plugin-exec`.
- Crate type: `cdylib` + `rlib`.
- Production Rust source: 13 files, 3,375 lines under `src/`.
- Key dependencies: `rivers-driver-sdk`, `tokio`, `serde`, `serde_json`, `sha2`, `hex`, `tracing`, `jsonschema`, `libc`.
- Review focus from `docs/review_inc/rivers-per-crate-focus-blocks.md`: command execution, SHA-256 hash pinning, integrity modes, stdin/args input modes, privilege drop, process lifecycle, dual semaphore concurrency, stdio bounds, and environment sanitization.

## Pending Tasks

- [ ] **RXE0.1 — Read crate manifest and focus block.**
  Read `crates/rivers-plugin-exec/Cargo.toml` and the `rivers-plugin-exec` block in `docs/review_inc/rivers-per-crate-focus-blocks.md`.
  Validation: report grounding lists crate role, source files, dependencies, and high-risk review axes.

- [ ] **RXE0.2 — Run mechanical sweeps.**
  Run review sweeps against `crates/rivers-plugin-exec/src`: panic paths, unsafe/FFI, discarded errors, lock usage, casts, format/query construction, unbounded collections, spawns, blocking calls, dead-code allowances, public API, and registration/bootstrap functions.
  Validation: sweep output is inspected before findings are drafted; raw hits are not reported without source confirmation.

- [ ] **RXE0.3 — Run compiler validation.**
  Run `cargo check -p rivers-plugin-exec` and, if feasible without unrelated workspace breakage, `cargo test -p rivers-plugin-exec`.
  Validation: report records exact commands and whether they passed or failed.

- [ ] **RXE1.1 — Read all production source files in full.**
  Read every file under `crates/rivers-plugin-exec/src/` in full:
  `lib.rs`, `schema.rs`, `template.rs`, `integrity.rs`, `executor.rs`, `config/{mod.rs,parser.rs,types.rs,validator.rs}`, and `connection/{mod.rs,driver.rs,exec_connection.rs,pipeline.rs}`.
  Validation: no finding is based on grep alone.

- [ ] **RXE1.2 — Check hash authorization and integrity modes.**
  Trace configured command hash validation from parsing through startup validation and runtime execution.
  Validation: explicitly cover TOCTOU risk, `each_time`, `startup_only`, `every:N`, counter behavior, symlink/file replacement behavior, and config reload implications if visible in this crate.

- [ ] **RXE1.3 — Check command invocation safety.**
  Trace how user-controlled parameters become stdin, argv, env, working directory, and process command.
  Validation: explicitly cover shell invocation, argument separation, template substitution, env inheritance/sanitization, stdout/stderr limits, and timeout behavior.

- [ ] **RXE1.4 — Check privilege drop and child lifecycle.**
  Trace Unix-only isolation code and child cleanup.
  Validation: explicitly cover `setgid`/`setuid` order, supplementary groups, process groups, timeout kill scope, zombie prevention, and shutdown/orphan behavior where source allows.

- [ ] **RXE1.5 — Check concurrency and resource bounds.**
  Trace global/per-command semaphores and any buffers/collections.
  Validation: identify whether permits are acquired in a consistent order, released on all paths, and whether stdout/stderr/input/output sizes are bounded.

- [ ] **RXE1.6 — Check driver-sdk contract compliance.**
  Compare `ExecDriver` / `ExecConnection` behavior with `rivers-driver-sdk` expectations: `prepare`, `execute`, DDL behavior, errors, operation names, query values, connection lifecycle, transaction support, and plugin exports.
  Validation: every contract issue cites both the exec implementation and the SDK contract source.

- [ ] **RXE1.7 — Read integration tests for coverage context.**
  Read `crates/rivers-plugin-exec/tests/integration_test.rs` to separate tested invariants from untested risk.
  Validation: report observations note major high-risk behavior covered or missing from tests.

- [ ] **RXE2.1 — Write per-crate review report.**
  Create `docs/review/rivers-plugin-exec.md` using the established finding format: one-line summary, Tier 1/2/3 findings, evidence snippets, impact, fix direction, and non-finding observations.
  Validation: report only includes confirmed issues or explicitly labeled non-findings.

- [ ] **RXE2.2 — Update logs.**
  Record the single-crate scope decision and final report delivery in `changedecisionlog.md`; record file changes in `todo/changelog.md`.
  Validation: logs name `docs/review/rivers-plugin-exec.md` and the exact source basis.

- [ ] **RXE2.3 — Mark tasks complete and verify whitespace.**
  Mark completed RXE tasks with high-level notes, then run `git diff --check -- docs/review/rivers-plugin-exec.md todo/tasks.md todo/gutter.md changedecisionlog.md todo/changelog.md`.
  Validation: command passes.

---

# CR — Gap Remediation from `docs/code_review.md` (2026-04-24)

> **Source:** gap analysis run on 2026-04-25 against `docs/code_review.md`.
> **Already fixed (no task needed):** P0-1 (security pipeline fail-closed), P0-4 (broker bridge supervisor).
> **Sequencing rationale:** P0-3 + P1-1 share the pool subsystem and ship together. P0-2 is small and unblocks tightening V8 surface. P1-5 closes a silent-failure hole. P1-6 closes a path-traversal hole. Remaining P1s grouped by subsystem so each can be picked up independently.

## P0 — Production blockers

- [ ] **CR-P0-3 / P1-1 — Wire DataView through PoolManager and fix lifetime accounting.**
  Replace direct `factory.connect(...)` in `crates/rivers-runtime/src/dataview_engine.rs:721` with a pool acquire via `PoolManager` (see `crates/riversd/src/pool.rs`). In the same change, fix `PoolGuard::drop` (`pool.rs:113-137`) so `created_at` is preserved across release rather than reset to `Instant::now()` at line 127 — otherwise `max_lifetime` never fires.
  Validation:
  - DataView execute path no longer calls `DriverFactory::connect` directly (grep confirms).
  - Add an integration test: 100 sequential DataView calls on the faker datasource produce ≤ `max_idle` distinct connections (observable via pool metrics or driver-side counter).
  - Add a test that a connection older than `max_lifetime` is evicted on next acquire.
  - Verbose health endpoint reports a non-empty pool snapshot for the active datasource.
  - Canary suite still passes 135/135.

- [ ] **CR-P0-2 — Gate `ctx.ddl()` to ApplicationInit task kind.**
  In `crates/riversd/src/process_pool/v8_engine/context.rs:497-575` (`ctx_ddl_callback`), reject calls unless the current `SerializedTaskContext` has `task_kind == ApplicationInit` and a bound `app_id` + `datasource_id`. Throw a JS `Error` ("ctx.ddl is only available during application init") rather than silently no-op.
  Validation:
  - Negative test: a REST handler, MessageConsumer, validation hook, and security hook each fail with the explicit JS error when calling `ctx.ddl(...)`.
  - Positive test: an ApplicationInit hook can still execute `CREATE TABLE` against a faker/sqlite datasource.
  - No change to the surface for init handlers in canary.

## P1 — High-priority

- [ ] **CR-P1-2 — Audit all V8 dispatch paths thread app identity.**
  MessageConsumer was fixed (`crates/riversd/src/message_consumer.rs:293`). Verify the same `enrich(builder, entry_point)` call exists for: REST handler dispatch, security pipeline hook dispatch, validation hook dispatch, lifecycle hook dispatch, and SSE/WS handler dispatch. Any path missing `app_id` lands `ctx.store` in `app:default`.
  Validation:
  - Grep confirms every `TaskContextBuilder` construction site sets `app_id` from the owning bundle/app.
  - Add a test that writes `ctx.store.set("k","v")` from each handler kind and verifies the key lands under the correct `app:<id>` namespace.

- [ ] **CR-P1-5 — Make `ctx.store` fail loudly when storage backend is unavailable.**
  In `crates/riversd/src/process_pool/v8_engine/context.rs:362,367` and matching set/delete paths: replace `tracing::warn!(...) ; fall back to TASK_STORE` with throwing a JS exception when a StorageEngine handle is configured but errors. Keep the in-memory fallback only when the bundle explicitly opted into ephemeral mode (if any). Default = throw.
  Validation:
  - Test: stop the StorageEngine; `ctx.store.get("k")` rejects with a propagated JS error instead of returning `null`.
  - Test: with no runtime handle wired, the same call rejects rather than reading task-local memory.
  - Existing canary tests still pass against a healthy StorageEngine.

- [ ] **CR-P1-6 — Block symlink escape in static file serving.**
  In `crates/riversd/src/static_files.rs:22-83`, replace the syntactic `resolved.starts_with(root)` check at line 51 with `tokio::fs::canonicalize` on both `root` and `resolved`, then compare. Reject with 404 (not 403, to avoid revealing structure) on mismatch.
  Validation:
  - Test: place a symlink inside the static root pointing to `/etc/passwd`; request via the static handler; assert 404 and that the file is not read.
  - Test: a regular file inside the root still serves with 200.

- [ ] **CR-P1-3 — Honor OutboundMessage destination in Kafka producer.**
  Trace `execute_broker_produce` → `crates/rivers-plugin-kafka/src/lib.rs` producer path. Confirm the `OutboundMessage.destination` field (topic) is used per-message rather than baked into the producer at creation time.
  Validation:
  - Test: from a single producer, publish two messages with different `destination` topics; assert both topics receive their respective message.

- [ ] **CR-P1-4 — Global priority ordering for EventBus wildcard subscribers.**
  In `crates/rivers-core/src/eventbus.rs`, merge exact-match and wildcard subscriber sets into a single list before sorting by priority, so a high-priority wildcard is not skipped behind a low-priority exact subscriber (or vice versa).
  Validation:
  - Test: subscribe `"order.*"` at priority 100 and `"order.created"` at priority 10; publish `"order.created"`; assert wildcard runs first.

- [ ] **CR-P1-7 — Add wall-clock timeout around SWC TypeScript compilation.**
  Wrap the SWC compile call in `tokio::time::timeout(...)` with a configurable budget (default 5s). Reject with a structured "compile timed out" error so a pathological input cannot hang a worker.
  Validation:
  - Test: feed a known-pathological/`while(true)` macro-expansion or a large generated input that exceeds budget; expect the timeout error.
  - Normal handler compilation finishes well under budget in canary.

- [ ] **CR-P1-8 — Module cache miss must not fall through to disk + live compile.**
  Per `rivers-javascript-typescript-spec.md §3.4`, the bundle module cache is populated at load. A miss at request time indicates a bundle validation gap. Replace the disk-read-and-compile fallback with a structured error that names the missing module path. Log a single high-severity event.
  Validation:
  - Test: artificially evict a module from the cache and request it; assert error response, no disk read, no live compile.
  - Bundle validation continues to populate cache on load (no regression).

- [ ] **CR-P1-9 — Strip absolute host paths from error responses and stack traces.**
  Audit error-formatting paths in `crates/riversd/src` (handlers, view pipeline, V8 engine, SWC compile errors) for `format!("{}", path)` of absolute paths. Replace with bundle-relative paths or opaque module IDs in user-facing responses; keep absolute paths only in server logs.
  Validation:
  - Test: trigger a handler error and a compile error; assert response body contains no leading `/` paths matching the deploy root.
  - Server log still contains the absolute path for operator debugging.

- [ ] **CR-P1-10 — Bound runtime of DataView execution and health checks.**
  Add `tokio::time::timeout` (configurable per-datasource, default 30s for DataView, 5s for health) around DataView execute and the health-check probe path. Surface timeout as a 504-equivalent driver error rather than hanging the request.
  Validation:
  - Test: configure a slow faker that sleeps 60s; DataView call returns timeout error in ~30s.
  - Health endpoint returns `degraded` rather than hanging when a probe stalls.

- [ ] **CR-P1-11 — Harden PostgreSQL connection string construction.**
  Replace ad-hoc string concat with a typed builder that URL-encodes each parameter (user, password, host, dbname, options). Reject params containing characters that break libpq/`tokio-postgres` parsing.
  Validation:
  - Test: passwords containing `@`, `:`, `/`, space, and `%` connect successfully; control chars are rejected.

- [ ] **CR-P1-12 — Validate handler response `status` and `headers` from JS.**
  In the V8 response marshalling path, reject status codes outside 100–599, header names that violate RFC 7230 token grammar, and CRLF/NUL in header values. Throw a structured JS error so the handler can correct it; if uncaught, return 500 with a sanitized message.
  Validation:
  - Test: handler returns `{ status: 999 }` → 500 with structured error.
  - Test: handler returns header `"X-Foo\r\nInjected: yes"` → rejected before write.

## P2 — Hardening

- [ ] **CR-P2 batch — Schedule per item; bundle into one PR if convenient.**
  - P2-1 Replace Redis cluster `KEYS` with `SCAN` cursor iteration.
  - P2-2 EventBus subscription removal + bounded queue / drop policy.
  - P2-3 Reconcile reserved storage prefixes (document the canonical list, gate writes outside it).
  - P2-4 Make view-lifecycle observer hooks truly fire-and-forget (or remove the comment).
  - P2-5 Replace string-based JS module detection with proper AST/heuristic.
  - P2-6 Remove the busy-loop promise resolution; drive on real wakers.
  - P2-7 Split MySQL-specific behavior out of `DriverFactory` into the MySQL driver.
  - P2-8 Decide and document SQLite file path policy (relative-to-bundle vs absolute) and enforce in validation.
  Validation per item: targeted test or explicit non-test rationale; spec doc updated where behavior changes.

## CR-Crate spot-check follow-ups

- [ ] **CR-KS-1 — `rivers-keystore init` must not silently overwrite an existing keystore.**
  Validation: test that `init` against a populated path errors unless `--force`; `--force` rotates with backup.

- [ ] **CR-KS-2 — Zeroize Age identity in `rivers-keystore`.**
  Replace plain `String` storage of the private key with a zeroizing wrapper; never place key material in the process environment.
  Validation: code review confirms no `env::set_var` of key material; drop test asserts memory is zeroed (best-effort via `secrecy`/`zeroize` crate audit).

## Tracking & wrap-up

- [ ] **CR-Z.1 — Per-task changelog + decision log entries.**
  For every CR-* task completed, append a line to `todo/changelog.md` (file affected, summary, spec ref) and `changedecisionlog.md` (decision rationale, especially for P0-3 pool ownership and P0-2 gating).

- [ ] **CR-Z.2 — Re-run gap analysis after P0/P1 batch lands.**
  Re-verify each P0 and P1 in `docs/code_review.md` against current code; update verdicts and close any obsolete items.
  Validation: write the updated gap report inline as a reply or as `docs/code_review_gaps_<date>.md`.
