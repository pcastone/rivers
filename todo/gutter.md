# Moved to tasks.md — 2026-04-30

## 2026-05-08 — Gutter cleanup decisions

### Closed: pre-existing test fixture failure (PR pending)

`crates/riversd/tests/view_engine_tests.rs::slow_observer_does_not_extend_request_latency`
was failing on `main` because the test passed empty `dv_namespace` to
`ViewContext::new`, which trips the dispatcher's empty-app_id check
after the canary-sprint RT-CTX-APP-ID fix. Resolution: pass
`"test-app"` (matching the existing app_id arg) for the dv_namespace.
One-line fixture change.

### Closed-as-deprioritized: polling change-detect datasource wiring

Initial gutter note suggested adding `wire_datasources` to
`crates/riversd/src/polling/runner.rs::dispatch_change_detect` for
parity with REST/MCP/WS/SSE. On closer look, threading a real
`DataViewExecutor` through the polling path requires changing three
function signatures (`drive_sse_push_loop` →
`execute_poll_tick_inmemory` → `dispatch_change_detect`) for a
callback whose contract is "fast diff function comparing prev to
current" — `Rivers.db.execute(...)` from a change-detect handler is
unusual. The slug-equivalent fix is already in place (line 89,
`app_id_from_qualified_name`), so keystore lookups work. Deferring
the datasource-wiring extension until a real bundle author asks for
it; the surface change isn't justified by speculation.

## 2026-05-08 — Plan H follow-ups

### Closed: lift v1 chain prohibition

Shipped. `guard_view` chains are now allowed up to
`MAX_GUARD_CHAIN_DEPTH` (5 hops). Validator replaces the chain
prohibition with cycle detection (DFS visited set) + depth cap;
runtime walks the chain inside-out (deepest leaf first) and
short-circuits on the first `allow: false`. Claims propagation
across chain levels remains v1-not-shipped — chains compose
allow/deny decisions only.

### Per-view-type runtime tests for `guard_view`

Plan H ships with config-side validator tests (X014, W009, W010, plus
the original guard-passing-path tests) but no end-to-end runtime
tests that hit a real REST/WS/SSE/streaming-REST view, fire a guard
codecomponent, and observe `{ allow: true/false }` causing
proceed/401. The MCP path is exercised by the existing canary, but
non-MCP transports lack an integrated test harness.

**Why deferred:** the test harness work is broader than the test
itself. Each transport (REST, WS, SSE) needs a way to:
1. Spin up a riversd instance with a bundle declaring two views
   (guard + protected).
2. Issue requests with bearer tokens / cookies.
3. Inspect responses for HTTP 401 vs handler output.

The closest existing infrastructure is the canary suite, which is
production-shape (bundle on disk, `riversd` running) — heavier than
unit tests want. A focused harness would be valuable but is its own
PR or initiative. Re-open if a guard regression surfaces in
production.
