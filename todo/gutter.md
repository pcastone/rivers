# Moved to tasks.md — 2026-04-30

## 2026-05-08 — Polling change-detect callback datasource wiring

Plan G (PR pending) closed the WS/SSE datasource-wiring gap.
`crates/riversd/src/polling/runner.rs::dispatch_change_detect` has an
analogous `enrich(builder, app_id, ...)` call but already passes a
slug-equivalent value (extracted via
`task_enrichment::app_id_from_qualified_name`). The remaining concern
is wiring per-app datasources via `task_enrichment::wire_datasources`
so the change-detect callback could call `Rivers.db.execute(...)` if a
bundle author's diff logic needs DB access. Currently a low-priority
follow-up — the change-detect handler is a small diff callback not
expected to need DB. Track for if/when reported.

## 2026-05-08 — Pre-existing test failure on `main`

`crates/riversd/tests/view_engine_tests.rs::slow_observer_does_not_extend_request_latency`
fails on bare `main` (verified by stashing Plan G work and
reproducing). Root cause: the test passes empty `dv_namespace` to
`ViewContext::new`, which trips the dispatcher's empty-app_id check
after the canary sprint's RT-CTX-APP-ID fix changed the
source-of-truth for what gets passed to `enrich`. Fix is to update
the test fixture to pass a non-empty `dv_namespace`. Small,
self-contained.
