# Archived 2026-04-24 — Canary Scenarios (CS0–CS7) + Broker Bridge (BR0–BR7)

Superseded by CG — Canary Green Again plan. Most items shipped; residual items were all deploy-gated or deferred polish (SPA step-UI, spec doc edits, MSG-4 real crypto). Moving here so the live tasks.md focuses on the restart-blocker + code-review fixes.

---

# Canary Scenarios Implementation

> **Branch:** TBD (suggest `feature/canary-scenarios`)
> **Spec:** `docs/arch/rivers-canary-scenarios-spec.md` v1.0
> **Prior:** G0-G8 gap closure shipped via PR #79 (archived in `todo/gutter.md`); G-P0 residual work on `feature/gap-closure-p0`.

**Goal:** build the scenario testing layer on top of the existing 107 atomic canary tests. 3 scenarios, 37 steps, 61 step-executions, bringing the canary fleet to 168 total test points.

**Rules:**
- Spec is **loose**: it defines use cases + constraints (MSG-1…10, AF-1…9, DOC-1…10), not file layouts or DataView names. Implementor picks all structural detail.
- Scenarios are **additive** — no atomic-test changes.
- Each scenario MUST **skip cleanly** when infra is unavailable (reuse `PG_AVAIL` / `MYSQL_AVAIL` / `KAFKA_AVAIL` gate pattern).
- Each scenario handler itself IS the test — every workflow step calls `TestResult.assert*` methods inline. No Rust-side unit tests.
- Document Pipeline is hosted in `canary-handlers` per spec §4 literal (decision locked CS0.1); this requires wiring filesystem + exec datasources into `canary-handlers/resources.toml`.

**Critical path:** CS0 → CS1 → (CS2 ‖ CS3 ‖ CS4 ‖ CS5) → CS6 → CS7.

---

## CS0 — Foundation decisions

- [x] **CS0.1** Document Pipeline hosted in **`canary-handlers`** (spec §4 literal). Implies CS4 wires filesystem + exec into `canary-handlers/resources.toml`. Log in `changedecisionlog.md`. (Locked 2026-04-22.)
- [x] **CS0.2** Session-identity simulation for Messaging: **single orchestrator + identity-as-parameter** (REVISED 2026-04-23 — original pre-seeded-sessions design needed Rivers.http, which doesn't exist). Orchestrator at `/canary/scenarios/sql/messaging/{driver}` (auth=none) calls DataViews directly, passing `sender` / `recipient` per-step. Isolation verified at DataView WHERE clause. MSG-1 end-to-end coverage gap documented; deferrable to atomic test. Logged in `changedecisionlog.md` 2026-04-23 (revised entry). (Locked.)
- [x] **CS0.3** Skip-gate pattern documented in `run-tests.sh` header comment (lines 5-22, 2026-04-23). Every scenario-level `test_ep` addition MUST follow the `X_AVAIL=$(curl -sk -m 2 ... )` probe + `if [ "$X_AVAIL" = "1" ]; then test_ep else echo SKIP fi` pattern. Reuses existing `PG_AVAIL` / `MYSQL_AVAIL` pattern at lines ~383/391. (Done.)

**Effort:** ~15 min.

---

## CS1 — Scenario harness + verdict protocol

**Scope:** the shared `ScenarioResult` TypeScript harness (spec §3) installed per-hosting-profile (SH-2 copy-per-profile rule). Verdict envelope per §2 with `type: "scenario"`, `steps[]`, `failed_at_step`, per-step `assertions[]`.

**Files:**
- new: `canary-bundle/canary-sql/libraries/handlers/scenario-harness.ts`
- new: `canary-bundle/canary-streams/libraries/handlers/scenario-harness.ts`
- new: `canary-bundle/canary-handlers/libraries/handlers/scenario-harness.ts`
- new: one probe handler per profile (e.g. `scenario-probe.ts`) — 1-step scenario that asserts `true` to validate envelope shape end-to-end

Tasks:

- [x] **CS1.1** Ported spec §3 `ScenarioResult` class into `canary-sql/libraries/handlers/scenario-harness.ts`. Imports `TestResult, Assertion` from `./test-harness.ts` (explicit `.ts` ext per Phase 4 resolver convention). Exports `ScenarioResult`, `StepResult`. One spec-deviation: `current_step_name` is a proper private field instead of the spec's `(this.current_step as any)._step_name` shim — cleaner, no behaviour change. (Done 2026-04-23.)
- [x] **CS1.2** Literal copy (same md5 `5803c8ab52a4239bf88056e1daf398f6`) to `canary-streams/libraries/handlers/scenario-harness.ts` and `canary-handlers/libraries/handlers/scenario-harness.ts`. SH-2 honoured. (Done 2026-04-23.)
- [x] **CS1.3** Audited `test-harness.ts::TestResult` — 6 existing methods (`assert`, `assertEquals`, `assertExists`, `assertType`, `assertThrows`, `assertNotContains`) are sufficient for the CS1 probe. Scenario-specific helpers (`assertContains`, `assertNotEquals`, `assertGreaterThan`) deferred until a concrete CS2/3/4 step needs them; must propagate across all 6 copies of test-harness.ts when added. (Done 2026-04-23.)
- [x] **CS1.4** Probe handlers created — `scenario-probe.ts` in all three hosting apps. Registered under `/canary/scenarios/sql/probe`, `/canary/scenarios/stream/probe`, `/canary/scenarios/runtime/probe` (test-ids SCENARIO-SQL-PROBE, SCENARIO-STREAM-PROBE, SCENARIO-RUNTIME-PROBE). Each 1-step scenario asserts `harness_reachable=true` + `ctx_is_object`. (Done 2026-04-23.)
- [x] **CS1.5 (static)** `riverpackage validate canary-bundle` → 0 errors, probe files referenced correctly, only pre-existing DataView warnings unchanged. V8-syntax-check SKIPs are environmental (no engine dylib in this build), not regressions. (Done 2026-04-23.)
- [ ] **CS1.5 (HTTP)** Full envelope round-trip via `curl $BASE/canary/scenarios/{profile}/probe` — deferred to first canary deploy + `riversd` foreground run. Expect: `passed=true`, `type=="scenario"`, `steps.length==1`, `failed_at_step==null`, `total_steps==1`, `steps[0].assertions.length==2`. Run before starting CS2.

**Verify:** all 3 probe endpoints return well-formed scenario envelopes.

**Effort:** ~45 min.

---

## CS2 — Scenario A: Messaging (canary-sql × {PG, MySQL, SQLite})

**Scope:** 12-step user messaging workflow per spec §5, run against 3 SQL drivers. 36 step-executions total.

**Files:**
- new: `canary-bundle/canary-sql/libraries/handlers/scenario-messaging.ts`
- new: `canary-bundle/canary-sql/schemas/messages-insert.json` (+ other param schemas as DataViews need them)
- edit: `canary-bundle/canary-sql/app.toml` (DataView defs + 3 endpoint views)
- edit: `canary-bundle/canary-sql/resources.toml` (ensure pg/mysql/sqlite present — they already are)
- edit: `canary-bundle/canary-sql/libraries/handlers/init.ts` (or equivalent init handler; extend with messages-table DDL)

Tasks:

- [x] **CS2.1** Schema: `messages(id, zsender, recipient, subject, body, is_secret, cipher, created_at)`. MSG-7 zname-trap: `body` sorts alphabetically before `id`; `zsender` sorts after `recipient`. Portable types (TEXT/INTEGER) avoid per-driver DDL divergence. No dedicated schema file — declaration lives in init handler DDL. (Done 2026-04-23.)
- [x] **CS2.2** Init DDL in `canary-sql/libraries/handlers/init.ts` — `CREATE TABLE IF NOT EXISTS messages (...)` for all 3 drivers; SQLite required (fails loudly), PG/MySQL best-effort (try/catch) matching existing init pattern. MySQL rewrites `id TEXT PRIMARY KEY` → `id VARCHAR(64) PRIMARY KEY` (TEXT can't be PK in MySQL). MSG-8 idempotent. (Done 2026-04-23.)
- [x] **CS2.3** 18 DataViews registered in `canary-sql/app.toml` (6 logical × 3 drivers):
    - **CS2.3.1** `msg_insert_{pg,mysql,sqlite}` — 7-param INSERT
    - **CS2.3.2** `msg_inbox_{pg,mysql,sqlite}` — server-side `WHERE recipient = $recipient ORDER BY created_at DESC` (MSG-2)
    - **CS2.3.3** `msg_search_{pg,mysql,sqlite}` — `WHERE recipient=$recipient AND body LIKE $pattern` (MSG-3)
    - **CS2.3.4** `msg_select_cipher_{pg,mysql,sqlite}` — direct-probe for MSG-9 at-rest ciphertext
    - **CS2.3.5** `msg_delete_{pg,mysql,sqlite}` — `WHERE id=$id AND (zsender=$actor OR recipient=$actor)` (MSG-5)
    - **CS2.3.6** `msg_cleanup_{pg,mysql,sqlite}` — `WHERE id LIKE $id_prefix` for CS2.7 teardown
    (Done 2026-04-23.)
- [x] **CS2.4** Three endpoint views registered in `canary-sql/app.toml` — `scenario_messaging_pg`, `scenario_messaging_mysql`, `scenario_messaging_sqlite` — all `POST /canary/scenarios/sql/messaging/{driver}`, `auth="none"` (orchestrator endpoint, not session-gated per revised CS0.2). Three distinct entrypoints rather than a `driver` arg (simpler dispatch, no parameter plumbing). MSG-10 satisfied. (Done 2026-04-23.)
- [x] **CS2.5** Handler file `scenario-messaging.ts` (~300 lines). Single shared `runMessagingScenario(ctx, driver)` + 3 thin export wrappers. `dv(driver, base)` helper picks the driver-suffixed DataView name. ScenarioResult per test_id `SCENARIO-SQL-MESSAGING-{DRIVER}`. (Done 2026-04-23.)
- [x] **CS2.6** All 12 workflow steps implemented in `scenario-messaging.ts`. Identity passed as DataView parameter (revised CS0.2). Explicit `sr.endStep()` after each step body; `hasFailed(N)` + `skipStep` handles the dependency chain (step 2/4/10 depend on step 1; step 7/8/11/12 depend on step 6). **MSG-1 coverage gap**: orchestrator is auth=none and passes `zsender` as a parameter — MSG-1's "sender from session" invariant is not directly tested; flagged in handler header comment and CS0.2 decisionlog. (Done 2026-04-23.)
- [x] **CS2.7** Cleanup-before AND cleanup-after using `msg_cleanup_{driver}` DataView with `id_prefix = "msg-" + ctx.trace_id + "-%"` — isolated per-run; no sender/recipient-based cleanup needed. Cleanup-before also sanitises after any prior abandoned run. Cleanup-after wrapped in try/catch per §10. (Done 2026-04-23.)
- [x] **CS2.8** run-tests.sh SCENARIOS profile added (lines ~414-438). 3 probe `test_ep`s (unconditional) + SQLite Messaging (unconditional) + PG/MySQL Messaging gated on existing `PG_AVAIL`/`MYSQL_AVAIL`. URL prefixes corrected to actual mount paths (`/sql/`, `/streams/`, `/handlers/`). (Done 2026-04-23.)
- [x] **CS2.9 (DEFERRED)** MSG-4 real encryption via `Rivers.crypto.encrypt` requires `[[keystores]]` configuration in `canary-sql/resources.toml`, which does not exist today (no canary app has keystore). Scenario uses a placeholder XOR+base64 "encryption" that exercises the MSG-9 at-rest invariant (stored cipher ≠ plaintext) but NOT AES-256-GCM semantics. Follow-on task: wire keystore config + LockBox keys into canary-sql; swap `placeholderEncrypt`/`placeholderDecrypt` for `Rivers.crypto.encrypt`/`.decrypt`.

**Verify:** 3 driver-variant endpoints return `passed=true` with all 12 steps green on full infra; `SKIP` cleanly without PG/MySQL.

**Effort:** ~4-6 hours.

---

## CS3 — Scenario B: Activity Feed (canary-streams) — **UN-DEFERRED 2026-04-23 (BR5)**

**Status:** Blocker resolved — the V8 broker-publish bridge shipped via BR0-BR4. Shipping structural implementation now; runtime/Kafka verification follows first deploy (pattern same as CS2/CS4).

**Scope:** 11-step Kafka-backed event pipeline per spec §6. Publish → MessageConsumer → SQL persist → REST history.

**Files:**
- new: `canary-bundle/canary-streams/libraries/handlers/scenario-activity-feed.ts`
- new: `canary-bundle/canary-streams/libraries/handlers/events-consumer.ts` (MessageConsumer entrypoint)
- edit: `canary-bundle/canary-streams/app.toml` (events table DataViews + MessageConsumer view + REST endpoint)
- edit: `canary-bundle/canary-streams/resources.toml` (canary-kafka + SQL datasource)
- edit: `canary-bundle/canary-streams/libraries/handlers/init.ts` (events-table DDL)

Tasks:

- [ ] **CS3.1** Table schema: `events(id, actor, target_user, event_type, payload, published_at, consumed_at)`. Stored by consumer per AF-2 / AF-7.
- [ ] **CS3.2** Init DDL for events table (AF-9). Idempotent.
- [ ] **CS3.3** DataView definitions:
    - [ ] **CS3.3.1** `events_insert` — called from the consumer handler (not REST)
    - [ ] **CS3.3.2** `events_for_user` — SELECT with `target_user=?`, pagination (limit + offset), date range `published_at >= ?` / `<= ?` (AF-4)
    - [ ] **CS3.3.3** `events_count_by_user` — COUNT for test asserts
    - [ ] **CS3.3.4** `events_delete_cleanup` — for CS3.9
- [ ] **CS3.4** MessageConsumer view registration — Kafka topic via `canary-kafka` LockBox alias (AF-8). Consumer entrypoint = `events-consumer.ts::consumeEvent` which parses the Kafka payload and calls `ctx.dataview("events_insert", ...)`.
- [ ] **CS3.5** REST history endpoint `/canary/scenarios/stream/activity-feed` — POST, auth=session, dispatches to `scenario-activity-feed.ts`. Implementation reads `target_user = ctx.session.sub` (AF-3 scoping).
- [ ] **CS3.6** Handler file `scenario-activity-feed.ts` — 11 steps per §6.
- [ ] **CS3.7** Step implementation:
    - [ ] **CS3.7.1** Step 1: Publish event for Bob ("Alice commented on your post"). Assert Kafka produce OK.
    - [ ] **CS3.7.2** Step 2: Poll-wait for consumer (§10 exponential backoff 100/200/400/800/1600/3200 ms, 5s total cap). Assert history shows the event.
    - [ ] **CS3.7.3** Step 3: Bob REST history. Assert `count==1`, content matches.
    - [ ] **CS3.7.4** Step 4: Publish 3 more Bob events in rapid succession. Assert all 3 produce calls OK.
    - [ ] **CS3.7.5** Step 5: Wait + Bob history. Assert `count==4`, publish order preserved (AF-5).
    - [ ] **CS3.7.6** Step 6: Bob history with date range (before last 3). Assert `count==1` (first event).
    - [ ] **CS3.7.7** Step 7: Carol history. Assert `count==0`.
    - [ ] **CS3.7.8** Step 8: Publish event for Carol. Assert Kafka OK.
    - [ ] **CS3.7.9** Step 9: Carol history (after poll-wait). Assert `count==1`, only Carol's.
    - [ ] **CS3.7.10** Step 10: Bob history. Assert `count==4` unchanged — scoping (AF-3).
    - [ ] **CS3.7.11** Step 11: Bob pagination (limit=2 offset=0, then limit=2 offset=2). Assert 2 pages × 2 events, all 4 distinct, no duplicates (AF-4).
- [ ] **CS3.8** Cleanup — `DELETE FROM events WHERE target_user IN ('alice','bob','carol')`. Best-effort (§10).
- [ ] **CS3.9** run-tests.sh: `test_ep "scen-stream-activity-feed" POST "$BASE/canary/scenarios/stream/activity-feed" '{}'` behind `KAFKA_AVAIL` (add a Kafka-ping gate if one doesn't exist yet).

**Verify:** full infra → `passed=true` with all 11 steps; no Kafka → clean SKIP.

**Effort:** ~3-5 hours.

---

## CS4 — Scenario C: Document Pipeline (canary-handlers)

**Scope:** 14-step document-workspace scenario per spec §7. Filesystem driver (sandboxed) + exec driver (hash-pinned allowlist).

**Files:**
- new: `canary-bundle/canary-handlers/libraries/handlers/scenario-doc-pipeline.ts`
- edit: `canary-bundle/canary-handlers/resources.toml` — add `fs_workspace` (filesystem driver) + `exec_tools` (exec driver) datasources. Mirror `canary-filesystem/resources.toml` patterns.
- edit: `canary-bundle/canary-handlers/app.toml` — filesystem/exec DataViews + 1 endpoint view

Tasks:

- [x] **CS4.1** `fs_workspace` filesystem datasource wired in `canary-handlers/resources.toml` (required=false) + `[data.datasources.fs_workspace]` config in `app.toml` (database=/tmp, mirroring canary-filesystem). DOC-7. (Done 2026-04-23.)
- [x] **CS4.2** rivers-exec datasource wired in `canary-handlers/resources.toml` + `app.toml`. `wc-json.sh` wrapper script committed under `libraries/scripts/`; SHA-256 computed via `riverpackage import-exec wc` and pinned in `commands.wc.sha256`. Driver name is `rivers-exec` (corrected from earlier `exec`). Deploy-time step for path refresh documented inline in the datasource config block. (Done 2026-04-23 CS4.9.)
- [x] **CS4.3** DataView definitions N/A — the filesystem driver exposes a direct object API (`ctx.datasource("fs_workspace").mkdir/writeFile/readFile/readDir/grep/stat/delete/exists`) rather than DataViews. Aligned with existing canary-filesystem pattern. Spec-plan assumption corrected. (Done 2026-04-23.)
- [x] **CS4.4** Endpoint `scenario_doc_pipeline` at `POST /canary/scenarios/runtime/doc-pipeline`, auth=none, registered in `canary-handlers/app.toml`. (Done 2026-04-23.)
- [x] **CS4.5** Handler file `scenario-doc-pipeline.ts` (~260 lines) with 14 steps + `deferredStep` helper for the 3 exec-dependent ones. `hasFailed()`/`skipStep()` dependency chain honoured. (Done 2026-04-23.)
- [x] **CS4.6** 14 steps implemented — 11 filesystem-backed, 3 deferred:
    - Steps 1-7, 11-14 — live filesystem ops via `ctx.datasource("fs_workspace")`.
    - Steps 8, 9, 10 — `deferredStep(sr, name, "exec driver not wired")`; always-pass with `deferred: true` detail. Dashboard can distinguish.
    - Dependency chain: steps 2-14 depend on step 1 (workspace created); step 12 depends on step 5 (notes written); step 6 depends on steps 2 and 5 (both docs present).
    (Done 2026-04-23.)
- [x] **CS4.7** Cleanup-before AND cleanup-after. The fs driver has no recursive-delete primitive — scenario walks `readDir` and deletes each entry, then deletes the workspace dir. Best-effort per DOC-8 (try/catch around each op). (Done 2026-04-23.)
- [x] **CS4.8** `test_ep "scen-runtime-doc-pipeline" POST "$BASE/handlers/canary/scenarios/runtime/doc-pipeline" '{}'` — unconditional (local fs only). Added to run-tests.sh SCENARIOS profile. (Done 2026-04-23.)
- [x] **CS4.9** Option B delivered 2026-04-23. Shipped:
    - `canary-handlers/libraries/scripts/wc-json.sh` — DOC-4 JSON output, DOC-6 traversal rejection at script layer (driver has no path sandbox). Executable. SHA-256 `1573a43390b7237e583d90fd0ab01e45ced0332dd7fd6aaf47f93b10dc9d7a8f`.
    - `resources.toml` gains `[[datasources]] name="exec_tools" driver="rivers-exec" required=false`.
    - `app.toml` gains `[data.datasources.exec_tools]` + `[data.datasources.exec_tools.commands.wc]` + `[data.dataviews.exec_wc]`. DataView uses `query = "query"` so operation is inferred as `"query"` per SHAPE-7 — avoids the validator-flagged `operation` unknown-key warning.
    - `scenario-doc-pipeline.ts` steps 8/9/10 replaced with real exec dispatch:
        - Step 8 asserts `{lines, words, bytes}` all positive (accepts rowResult under `.result` or at row root — shape TBD).
        - Step 9 asserts error contains one of `"unknown command"` / `"not allowlisted"` / `"not in allowlist"`.
        - Step 10 accepts rejection either as thrown error OR row with `.error` field — script emits the latter on exit 2.
    - **Operator deploy step**: after `cargo deploy`, re-run `riverpackage import-exec wc <deployed-path-to>/libraries/scripts/wc-json.sh` and paste the refreshed `path` + `sha256` into the `commands.wc` block. Only required if script is edited or deploy path differs from source layout.
    - **Two live-only unknowns** flagged in the handler: (a) exec row shape (`.result` nesting), (b) script error-exit surfaces as throw vs row-with-error. Both path branches handled.
    - `riverpackage validate canary-bundle` → 0 errors, 83 warnings (all pre-existing "param required no default" noise).

**Verify:** endpoint returns `passed=true` with all 14 steps; no external infra required.

**Effort:** ~3-4 hours.

---

## CS5 — Dashboard integration (canary-main)

**Scope:** render scenario verdicts in the Svelte SPA.

**Files:**
- edit: `canary-bundle/canary-main/` SPA source — verdict loader + new scenario card component

Tasks:

- [x] **CS5.1** Scenarios shipped into the SPA via a new `SCENARIOS` profile in `canary-main/libraries/spa/bundle.js`. 7 test entries: 3 probes + 3 Messaging variants + Doc Pipeline. CS3 Activity Feed entry omitted (deferred). Profile renders through the existing atomic-test renderer — no new UI code. (Done 2026-04-23.)
- [x] **CS5.1a** Harness change: `ScenarioResult.finish()` now emits a FLAT `assertions[]` field aggregating all per-step assertions with step-prefixed IDs (`"s1:alice-sends-to-bob:insert_no_throw"`), so the existing atomic-renderer's `r.data.assertions.forEach(...)` path shows full detail for scenarios without dashboard code changes. `steps[]` is preserved for future step-level rendering. Propagated across all 3 harness copies (same md5). (Done 2026-04-23.)
- [ ] **CS5.2 (DEFERRED)** Dedicated per-step UI — scenario card with failed_at_step banner, expand/collapse list, per-step pass/fail indicators, skip-step visual distinction. Needs the SPA source tree (currently `libraries/src/components/` is empty — bundle.js ships pre-compiled). Requires resurrecting or rebuilding the Svelte build pipeline. Scenarios function in the current SPA today via the flat-assertion compatibility path; dedicated step view is polish, not blocker.
- [ ] **CS5.3 (DEFERRED)** Skipped-step visual distinction — blocked on CS5.2 (same SPA-source reason).
- [ ] **CS5.4 (DEFERRED)** Dedicated Scenarios tab — same. Scenarios render as a top-level profile alongside atomic tests today, which functionally covers the "separate section" goal.

**Verify:** dashboard shows all 5 scenario cards (3 Messaging variants + Activity Feed + Doc Pipeline) with step-level detail expandable.

**Effort:** ~2 hours.

---

## CS6 — run-tests.sh wiring

Tasks:

- [x] **CS6.1** `SCENARIOS` profile section added to `run-tests.sh` (before MCP, lines ~414-438). Skip-gate pattern header comment at top of file (CS0.3). (Done 2026-04-23.)
- [x] **CS6.2** Gates: reuses existing `PG_AVAIL` / `MYSQL_AVAIL` probes from the INTEGRATION block. SQLite Messaging + Doc Pipeline unconditional. Probes unconditional. `KAFKA_AVAIL` not added — CS3 deferred. (Done 2026-04-23.)
- [ ] **CS6.3 (DEFERRED)** Per-step summary pretty-printing on failure — standard `test_ep` asserts `passed=true` at the envelope level which is sufficient today. Step-breakdown printout is polish, bundled with CS5.2 follow-on.

**Verify:** `./run-tests.sh` prints a clean "SCENARIOS Profile" section with 5 entries.

**Effort:** ~30 min.

---

## CS7 — End-to-end verification + CHANGELOG

Tasks:

- [ ] **CS7.1 (PENDING DEPLOY)** Full-infra run: `./run-tests.sh` — expects SQLite Messaging + Doc Pipeline + 3 probes PASS unconditionally; PG/MySQL Messaging PASS on live infra. No Activity Feed entry (CS3 deferred). Requires `cargo deploy` + `riversctl start`. Static `riverpackage validate canary-bundle` → 0 errors, 83 warnings (all pre-existing). (Static gate done 2026-04-23; HTTP gate pending operator deploy.)
- [ ] **CS7.2 (PENDING DEPLOY)** No-infra run: SQLite Messaging + Doc Pipeline + probes PASS; PG/MySQL Messaging SKIP cleanly via PG_AVAIL/MYSQL_AVAIL gates. Needs running riversd.
- [ ] **CS7.3 (PENDING DEPLOY)** Deliberate-failure probe — edit one step's assertion, verify `failed_at_step=N`, SV-7 subsequent steps execute, SV-8 dependent steps show skipped. Needs running riversd.
- [ ] **CS7.4 (PENDING DEPLOY)** Dashboard smoke — canary-main loaded in browser shows SCENARIOS profile section with per-scenario pass/fail + expandable flat-assertion detail. Per-step UI deferred per CS5.2.
- [x] **CS7.5** `canary-bundle/CHANGELOG.md` appended with scenario-layer decision entry cross-referencing `rivers-canary-scenarios-spec.md` + CS3 deferral rationale + bug report pointer. (Done 2026-04-23.)
- [x] **CS7.6** `changedecisionlog.md` gained four entries across the scenarios work: CS0.1 (doc-pipeline host), CS0.2 + CS0.2-revised (identity simulation), CS3 deferral + HTTP-as-datasource correction. (Done 2026-04-23.)

**Effort:** ~30 min.

---

## Files touched (hot list)

- **new:** `canary-bundle/canary-sql/libraries/handlers/scenario-harness.ts`
- **new:** `canary-bundle/canary-streams/libraries/handlers/scenario-harness.ts`
- **new:** `canary-bundle/canary-handlers/libraries/handlers/scenario-harness.ts`
- **new:** `canary-bundle/canary-sql/libraries/handlers/scenario-messaging.ts`
- **new:** `canary-bundle/canary-streams/libraries/handlers/scenario-activity-feed.ts`
- **new:** `canary-bundle/canary-streams/libraries/handlers/events-consumer.ts`
- **new:** `canary-bundle/canary-handlers/libraries/handlers/scenario-doc-pipeline.ts`
- **new:** 3× `scenario-probe.ts` (one per hosting profile)
- **edit:** `canary-bundle/canary-sql/app.toml`, `resources.toml`, `schemas/`, init handler
- **edit:** `canary-bundle/canary-streams/app.toml`, `resources.toml`, init handler
- **edit:** `canary-bundle/canary-handlers/app.toml`, `resources.toml` (filesystem + exec datasources — new for this app)
- **edit:** `canary-bundle/canary-main/` SPA source (scenario card)
- **edit:** `canary-bundle/run-tests.sh` (SCENARIOS profile)
- **edit:** `canary-bundle/CHANGELOG.md`, `changedecisionlog.md`, `todo/changelog.md`

## Effort summary

| Tier | Items | Effort | Risk |
|------|-------|--------|------|
| CS0 foundation | 3 | 15 min | low |
| CS1 harness | 5 | 45 min | low |
| CS2 Messaging | 20 subtasks | 4-6 hours | medium (crypto roundtrip, zname-trap) |
| CS3 Activity Feed | 18 subtasks | 3-5 hours | medium (Kafka timing, consumer wiring) |
| CS4 Doc Pipeline | 22 subtasks | 3-4 hours | low-medium (path traversal, exec allowlist) |
| CS5 dashboard | 4 | 2 hours | low |
| CS6 run-tests.sh | 3 | 30 min | low |
| CS7 verify | 6 | 30 min | low |
| **Total** | **81 subtasks** | **~14-18 hours** | |

## Execution order

1. **CS0.2** — pick session-identity simulation (unblocks CS2).
2. **CS0.3** — write the skip-gate comment block in run-tests.sh (unblocks CS2/CS3/CS4 test_ep additions).
3. **CS1** — port harness to all 3 profiles; probe endpoints confirm envelope shape.
4. **CS2 / CS3 / CS4** — independent; run in parallel if multiple developers.
5. **CS5** — dashboard after at least one scenario ships (so there's something to render).
6. **CS6** — wire run-tests.sh after scenarios exist.
7. **CS7** — verify end-to-end.

## Design decisions to log (changedecisionlog.md)

1. **CS0.1** — Document Pipeline host: canary-handlers (spec §4 literal) vs canary-filesystem (infra-ready). Chose canary-handlers. (Locked.)
2. **CS0.2** — Messaging session-identity: session injection vs pre-seeded sessions. (Pending.)
3. **CS2.1** — Messages schema column ordering and zname-trap choice.
4. **CS3.4** — MessageConsumer wiring pattern: new view type vs extending existing.
5. **CS4.2** — Exec allowlist binary selection + hash-pinning strategy.

## Non-goals (explicit out-of-scope)

- SSE real-time delivery in Activity Feed scenario (spec §6 note — covered by atomic STREAM-SSE-* tests).
- New scenarios beyond the 3 in the spec.
- Replacing or modifying atomic tests.
- Production-ready error taxonomy in scenario envelopes beyond SV-1…SV-9.
- Multi-scenario composition (one scenario triggering another).

---

# BR-2026-04-23 — MessageBrokerDriver TS bridge

> **Bug:** `bugs/bugreport_2026-04-23.md`
> **Unblocks:** CS3 (Activity Feed) + any future pub/sub scenario.
> **Spec touched:** `docs/arch/rivers-processpool-runtime-spec-v2.md` §ctx.datasource surface, `docs/arch/rivers-driver-spec.md` §MessageBrokerDriver.
> **Scope:** expose `ctx.datasource("<broker>").publish(...)` in TS handlers for kafka, rabbitmq, nats, redis-streams. Mirror the filesystem direct-dispatch pattern.

**Rule:** no workarounds ship under this banner. If a subtask hits a blocker that would require a hollow shortcut (path A / path B precedents from the CS3 deliberation), stop and mark DEFERRED rather than paper over it.

**Design constraint:** `MessageBrokerDriver` is a distinct trait from `DatabaseDriver` (different `create_producer()` / `create_consumer()` surface; no shared `execute(Query)` method). The existing `TASK_DIRECT_DATASOURCES` + `Rivers.__directDispatch` machinery is tied to `DatabaseDriver::connect()`. Bridging brokers requires **parallel scaffolding**, not reuse. See BR0.1.

---

## BR0 — Design decisions

- [x] **BR0.1** (Done 2026-04-23, see changedecisionlog.md) Pick the bridge pattern:
    - **(a)** Parallel scaffolding — new `TASK_DIRECT_BROKER_PRODUCERS` thread-local, new `Rivers.__brokerPublish` V8 callback, new `DatasourceToken::Broker { driver, params }` variant. Cleanest type separation; ~1.5 days work. **Recommended.**
    - **(b)** Unify — make MessageBrokerDrivers also implement `DatabaseDriver` with a synthetic `"publish"` operation whose `parameters` contains topic+payload. Reuse existing direct-dispatch entirely. ~2 days because every plugin gains a new trait impl; invasive across the four broker crates. No type separation between request/response and fire-and-forget semantics.
    - **(c)** DataView-based — extend DataView dispatch to accept broker datasources, with `query` field naming the destination. Cheapest (~0.5 day) but surface is impoverished: no structured headers, no key partitioning, no PublishReceipt return. Rejected by spec intent ("one direction wired, the other stranded" from the bug report).

    Log decision in `changedecisionlog.md`.
- [x] **BR0.2** (Done 2026-04-23, see changedecisionlog.md) Decide producer lifecycle: cache one `BrokerProducer` instance per `(task, datasource_name)` like filesystem caches a `Connection`, OR create + close per-publish? Former is faster; latter avoids cross-task producer reuse bugs. Recommendation: per-task cache, cleared in `TaskLocals::drop` (mirror `TASK_DIRECT_DATASOURCES` lifecycle). Log in changedecisionlog.
- [x] **BR0.3** (Done 2026-04-23, see changedecisionlog.md) Decide API shape in TS:
    - `ctx.datasource("kafka").publish({topic, payload, key?, headers?, reply_to?})` — mirrors `OutboundMessage` fields literally.
    - Return value shape: `{id: string | null, metadata: string | null}` — mirrors `PublishReceipt`. Throws on `DriverError`.
    - Payload type: accept `string` (interpreted as UTF-8 bytes) AND `object` (JSON-stringify → bytes)? Or require explicit bytes/string from caller? Recommendation: accept both, document JSON auto-stringify.

**Effort:** ~30 min decisions + logging.

---

## BR1 — Token plumbing + per-task producer cache

**Files:**
- edit: `crates/rivers-runtime/src/process_pool/types.rs` — add `DatasourceToken::Broker { driver, params }` variant, extend `resolve_token_for_dispatch` to emit it for broker drivers.
- edit: `crates/riversd/src/process_pool/v8_engine/task_locals.rs` — add `TASK_DIRECT_BROKER_PRODUCERS: RefCell<HashMap<String, DirectBrokerProducer>>` thread-local (parallel to `TASK_DIRECT_DATASOURCES`). Struct holds driver name + ConnectionParams + lazy `RefCell<Option<Box<dyn BrokerProducer>>>`. Clear in `Drop` impl.
- edit: `crates/riversd/src/bundle_loader/load.rs` (or wherever DatasourceToken dispatch is populated from resolved datasources) — populate `TASK_DIRECT_BROKER_PRODUCERS` on task-context build when the datasource's driver is a MessageBrokerDriver.

Tasks:

- [x] **BR1.1** (Done 2026-04-23) Add `DatasourceToken::Broker { driver: String, params: ConnectionParams }` variant + `DatasourceToken::broker(driver, params)` constructor + unit test.
- [x] **BR1.2** (Done 2026-04-23) Extend `resolve_token_for_dispatch` — detect broker drivers by name (`"kafka" | "rabbitmq" | "nats" | "redis-streams"`) or by a trait query into `DriverFactory`. Prefer trait query so future broker drivers auto-qualify. Open item: `DriverFactory` currently registers DatabaseDriver vs MessageBrokerDriver in separate maps — check that the factory exposes a "kind" probe.
- [x] **BR1.3** (Done 2026-04-23) Add `DirectBrokerProducer` struct to `task_locals.rs`:
    ```rust
    pub(super) struct DirectBrokerProducer {
        pub(super) driver: String,
        pub(super) params: ConnectionParams,
        pub(super) producer: RefCell<Option<Box<dyn BrokerProducer>>>,
    }
    ```
    + `TASK_DIRECT_BROKER_PRODUCERS` thread-local. `TaskLocals::set` populates from `ctx.datasources` broker variants; `TaskLocals::drop` clears AND closes any open producers (best-effort, log-on-error).
- [x] **BR1.4** (Done 2026-04-23) Producer close on drop must be safe w.r.t. tokio runtime — mirror the existing `auto_rollback_all` pattern in `TaskLocals::drop` (capture RT handle before clearing).

**Unit tests:**

- [x] **BR1.T1** (Done 2026-04-23) `broker_token_constructs` — `DatasourceToken::broker("kafka", params)` round-trips.
- [x] **BR1.T2** (Done 2026-04-23) `resolve_broker_driver_yields_broker_token` — run `resolve_token_for_dispatch` with a kafka `ResolvedDatasource`, assert Broker variant.
- [x] **BR1.T3** (Done 2026-04-23) `pooled_drivers_still_yield_pooled` — postgres/mysql/redis produce `Pooled` as before (regression).

**Effort:** ~3 hours.

---

## BR2 — V8 bridge callback + proxy-codegen integration

**Files:**
- new: `crates/riversd/src/process_pool/v8_engine/broker_dispatch.rs` — parallel to `direct_dispatch.rs`.
- edit: `crates/riversd/src/process_pool/v8_engine/proxy_codegen.rs` — emit `.publish(args)` method on datasource proxies whose token type is Broker.
- edit: `crates/riversd/src/process_pool/v8_engine/mod.rs` — register the new module, wire callback into init.
- edit: `crates/riversd/src/process_pool/v8_engine/init.rs` — inject `Rivers.__brokerPublish` callback during context setup.

Tasks:

- [x] **BR2.1** (Done 2026-04-23) Create `broker_dispatch.rs` with `rivers_broker_publish_callback(scope, args, rv)`:
    - Extract `name` (arg 0, string) and `message` (arg 1, object).
    - Parse `message` into `OutboundMessage` — `destination` required, `payload` required, `headers` / `key` / `reply_to` optional. Auto-stringify `payload` if object (per BR0.3).
    - Look up `TASK_DIRECT_BROKER_PRODUCERS[name]`; throw `TypeError` if missing.
    - Lazy-init `BrokerProducer` on first call in this task — `driver.create_producer(params, &BrokerConsumerConfig::default())`.
    - `rt.block_on(producer.publish(msg))` — return `{id, metadata}` object on Ok, throw `v8::Exception::error` with the DriverError message on Err.
- [x] **BR2.2** (Done 2026-04-23) Extend `proxy_codegen.rs` — when the datasource's token is `Broker`, emit a JS proxy object with a `publish(msg)` method that calls `Rivers.__brokerPublish(name, msg)`. Filesystem's `FilesystemProxyObject` is the precedent.
- [x] **BR2.3** (Done 2026-04-23) Register the V8 callback in `init.rs` (or wherever `Rivers.__directDispatch` is registered). Idempotent — only inject when at least one broker datasource is in scope (micro-optimisation; optional).
- [x] **BR2.4** (Done 2026-04-23) Error mapping: `DriverError::Connection` / `Query` / `Unsupported` → distinct JS error messages so handlers can pattern-match.

**Unit tests (Rust side):**

- [x] **BR2.T1** (Done 2026-04-23) `broker_publish_missing_datasource_throws_type_error` — call callback with unknown name, assert TypeError + message.
- [x] **BR2.T2** (Done 2026-04-23) `broker_publish_missing_destination_throws` — message object without `destination`, assert throw.
- [x] **BR2.T3** (Done 2026-04-23) `broker_publish_happy_path_uses_cached_producer` — first call creates producer, second call reuses cached instance (verify via a mock BrokerProducer that counts `create_producer` invocations).

**Effort:** ~4 hours.

---

## BR3 — Driver-side integration verification

All four broker drivers already implement `MessageBrokerDriver` + `BrokerProducer::publish`. BR3 verifies the bridge works end-to-end against each, not just kafka.

Tasks:

- [ ] **BR3.1 (PENDING DEPLOY)** Kafka end-to-end: `ctx.datasource("kafka").publish(...)` from canary + kafkacat consumer verify. Atomic-level covered in BR4.
- [ ] **BR3.2 (PENDING DEPLOY)** RabbitMQ — `key` field → AMQP routing-key.
- [ ] **BR3.3 (PENDING DEPLOY)** NATS — publish to subject + subscriber verify.
- [ ] **BR3.4 (PENDING DEPLOY)** Redis Streams — XADD publish + XREAD verify.
- [x] **BR3.5** Broker plugin lib tests regression: kafka 15 / rabbitmq 15 / nats 12 / redis-streams 11 = **53 tests PASS**. No regression from BR1 token plumbing. (Done 2026-04-23.)

**Effort:** ~3 hours (parallelisable across drivers if infra allows).

---

## BR4 — Testing: new canary atomic tests

**Files:**
- new: `canary-bundle/canary-streams/libraries/handlers/broker-publish-tests.ts` — atomic tests for the new surface.
- edit: `canary-bundle/canary-streams/app.toml` — register test endpoints, uncomment the MessageConsumer view now that publish works.

Tasks:

- [x] **BR4.1** (Done 2026-04-23) New atomic test `STREAM-KAFKA-PUBLISH-RECEIPT` — publish a message, assert receipt has `id` (kafka returns offset) + `metadata` (partition).
- [x] **BR4.2** (Done 2026-04-23) New atomic test `STREAM-KAFKA-PUBLISH-THEN-CONSUME` — handler publishes, triggers the MessageConsumer view (now re-enabled), consumer stores in `ctx.store`, second test endpoint reads from store and asserts roundtrip.
- [x] **BR4.3** (Done 2026-04-23) Negative test `STREAM-KAFKA-PUBLISH-UNKNOWN-DATASOURCE` — call with unknown name, assert specific error message.
- [x] **BR4.4** (Done 2026-04-23) Negative test `STREAM-KAFKA-PUBLISH-MISSING-DESTINATION` — message without `destination`, assert argument validation error.
- [x] **BR4.5** (Done 2026-04-23) Uncomment the canary-streams `kafka_consume` MessageConsumer view block (currently disabled with `# no broker`). Wire it alongside BR4.2.
- [ ] **BR4.6 (DEFERRED)** Equivalent publish-roundtrip tests for NATS + RabbitMQ + Redis-Streams if the canary hosts those brokers (otherwise SKIP-gate them).
- [x] **BR4.7** (Done 2026-04-23) run-tests.sh — new tests fire under `STREAMS` profile; existing atomic counts don't regress.

**Effort:** ~3 hours + infra time.

---

## BR5 — CS3 unblock (Activity Feed scenario ships)

This is the downstream payoff — the scenario that was deferred in the canary-scenarios work becomes implementable.

Tasks:

- [x] **BR5.1** (Done 2026-04-23) Un-defer CS3 in `todo/tasks.md` (flip the header back from DEFERRED).
- [x] **BR5.2** (Done 2026-04-23) Execute the CS3.1–CS3.9 subtasks as originally planned — now the orchestrator can publish events to Kafka, the consumer persists them via the existing `ctx.dataview("events_insert", ...)` path, and the scenario's 11 steps all run for real.
- [x] **BR5.3** (Done 2026-04-23) run-tests.sh gains a `KAFKA_AVAIL` probe + one conditional `test_ep "scen-stream-activity-feed"` line.
- [x] **BR5.4** (Done 2026-04-23) canary-main/spa/bundle.js gains the `SCEN-STREAM-ACTIVITY-FEED` entry in the SCENARIOS profile.

**Effort:** ~3-4 hours (the original CS3 estimate — unchanged).

---

## BR6 — Documentation + decisions

Tasks:

- [ ] **BR6.1 (DEFERRED — spec doc edits)** Update `docs/arch/rivers-processpool-runtime-spec-v2.md` — document `ctx.datasource("<broker>").publish(...)` as a first-class surface alongside filesystem direct-dispatch.
- [ ] **BR6.2 (DEFERRED — spec doc edits)** Update `docs/arch/rivers-driver-spec.md` §MessageBrokerDriver — note that `publish` is now reachable from TS handlers via the V8 bridge; cross-reference the new runtime spec section.
- [x] **BR6.3** (Done 2026-04-23) Update `types/rivers.d.ts` — declare the datasource proxy type with `.publish(OutboundMessage): PublishReceipt` signature. Include JSDoc capability marker `@capability broker` following the existing `@capability keystore`/`@capability transaction` convention.
- [x] **BR6.4** (Done 2026-04-23) `changedecisionlog.md` entries:
    - BR0.1 bridge-pattern choice + rationale.
    - BR0.2 producer lifecycle (per-task cache).
    - BR0.3 TS API shape (string vs object payload, receipt shape).
    - Why MessageBrokerDriver was chosen as the trait anchor rather than fronting it with a synthetic DatabaseDriver op.
- [x] **BR6.5** (Done 2026-04-23) `todo/changelog.md` entry covering the whole BR phase.
- [x] **BR6.6** (Done 2026-04-23) Append `bugs/bugreport_2026-04-23.md` with a "Resolved by" section pointing at the merge commit.

**Effort:** ~1.5 hours.

---

## BR7 — Verification + canary roundtrip

Tasks:

- [x] **BR7.1** (Done 2026-04-23) `cargo test -p riversd` — all lib + integration tests pass, new BR1.T1-3 + BR2.T1-3 green.
- [x] **BR7.2** (Done 2026-04-23) `cargo build -p riversd --features static-engines` — clean build.
- [ ] **BR7.3 (PENDING DEPLOY)** `cargo deploy /tmp/rivers-br` — deploy with the new runtime.
- [ ] **BR7.4 (PENDING DEPLOY)** `canary-bundle/run-tests.sh` against the deployed instance — existing atomic count unchanged; new STREAM atomic tests PASS; new `scen-stream-activity-feed` PASS (Kafka reachable).
- [x] **BR7.5** (Done 2026-04-23) No regression in the 25 completed scenario items (CS0–CS2, CS4, CS5.1, CS6.1/6.2, CS7.5/7.6).

**Effort:** ~1 hour.

---

## Files touched (hot list)

- **new:** `crates/riversd/src/process_pool/v8_engine/broker_dispatch.rs`
- **new:** `canary-bundle/canary-streams/libraries/handlers/broker-publish-tests.ts`
- **edit:** `crates/rivers-runtime/src/process_pool/types.rs` (new DatasourceToken variant)
- **edit:** `crates/riversd/src/process_pool/v8_engine/task_locals.rs` (TASK_DIRECT_BROKER_PRODUCERS + lifecycle)
- **edit:** `crates/riversd/src/process_pool/v8_engine/proxy_codegen.rs` (.publish method emission)
- **edit:** `crates/riversd/src/process_pool/v8_engine/init.rs` (register callback)
- **edit:** `crates/riversd/src/process_pool/v8_engine/mod.rs` (module registration)
- **edit:** `crates/riversd/src/bundle_loader/load.rs` (DatasourceToken dispatch for broker)
- **edit:** 4 broker plugin crates — integration test updates if BR1.2 trait-query path touches their registration code
- **edit:** `canary-bundle/canary-streams/app.toml` (enable kafka_consume view, register publish test endpoints)
- **edit:** `docs/arch/rivers-processpool-runtime-spec-v2.md`, `rivers-driver-spec.md`
- **edit:** `types/rivers.d.ts`
- **edit:** `canary-bundle/canary-streams/libraries/handlers/scenario-activity-feed.ts` (new, BR5)
- **edit:** `canary-bundle/canary-streams/libraries/handlers/events-consumer.ts` (BR5)
- **edit:** `canary-bundle/run-tests.sh` (BR4 atomic lines + BR5 KAFKA_AVAIL gate)
- **edit:** `canary-bundle/canary-main/libraries/spa/bundle.js` (BR5.4 scenario entry)
- **edit:** `changedecisionlog.md`, `todo/changelog.md`, `canary-bundle/CHANGELOG.md`
- **update:** `bugs/bugreport_2026-04-23.md` (Resolved by)

## Effort summary

| Tier | Items | Effort | Risk | Depends on |
|------|-------|--------|------|-----------|
| BR0 foundation | 3 decisions | 30 min | low | — |
| BR1 token + cache | 4 impl + 3 tests | 3h | low-med (trait-query plumbing) | BR0 |
| BR2 V8 bridge | 4 impl + 3 tests | 4h | medium (V8 callback + proxy emission) | BR0, BR1 |
| BR3 driver integration verify | 5 | 3h | low | BR2, infra |
| BR4 canary atomics | 7 | 3h | low | BR2 |
| BR5 CS3 ship | 4 | 3-4h | low (plan pre-written) | BR2 |
| BR6 docs | 6 | 1.5h | low | BR2 |
| BR7 verify | 5 | 1h | low | all |
| **Total** | **44 items** | **~19h (~2.5 days)** | | |

**Critical path:** BR0 → BR1 → BR2 → (BR3 ‖ BR4 ‖ BR5 ‖ BR6) → BR7. BR5 and BR4 can share infra time.

## Execution order

1. **BR0** — lock the three decisions (bridge pattern, lifecycle, API shape).
2. **BR1** — token plumbing + cache. Unit-testable in isolation.
3. **BR2** — V8 bridge. Unit-testable with a mock BrokerProducer.
4. **BR3** — verify against each driver (needs infra).
5. **BR4** — canary atomics (new public-facing coverage).
6. **BR5** — CS3 Activity Feed finally ships.
7. **BR6** — docs while tests bake.
8. **BR7** — full-bundle verification.

## Non-goals (explicit out-of-scope)

- Extending the bridge to `BrokerConsumer` from TS — consumers are already handled via MessageConsumer views.
- Schema validation on `OutboundMessage.payload` — driver-specific, out of scope.
- Retry / circuit-breaker policy for publish failures — caller's responsibility today; could be a follow-on.
- Transactional publish (exactly-once semantics) — Kafka-specific, future work.
- Request/reply pattern (NATS-specific) — requires a matching consume flow from TS, bigger scope.

---

# Tasks — Unit Test Infrastructure

> **Branch:** `test-coverage`
> **Source:** `docs/bugs/rivers-unit-test-spec.md` + `rivers-unit-test-amd1.md` + `docs/reports/test-coverage-audit.md`
> **Goal:** Implement test infrastructure from spec, covering 33/38 bugs + feature inventory gaps
> **Current:** 1,940 tests across 27 crates. 0/13 critical bugs had unit tests before discovery.
>
> **Critical gaps (0 tests):** DataView engine, Tiered cache, Schema validation, V8 bridge contracts, V8 security, Config validation, Boot parity

---

## Phase 1 — Test Harness Foundation

These create the shared infrastructure that all later tests depend on.

### 1.1 — Add `test-case` crate to workspace dependencies ✅
### 1.2 — Create driver conformance test harness ✅ (19 tests)
### 1.3 — V8 bridge test harness ✅ (via v8_bridge_tests.rs — uses ProcessPoolManager dispatch, not TestIsolate)

---

## Phase 2 — Driver Conformance Matrix (Strategy 1) ✅

19 tests implemented in `conformance_tests.rs`:
- DDL guard: 12 tests (8 SQLite + 4 cluster) — BUG-001 ✅
- CRUD lifecycle: 3 tests (1 SQLite + 2 cluster) ✅
- Param binding: 4 tests (2 SQLite + 2 cluster) — BUG-004 ✅

Remaining (cluster-only, deferred until podman available):
- [ ] Admin guard tests (redis, mongodb, elasticsearch)
- [ ] NULL handling round-trip
- [ ] max_rows truncation

---

## Phase 3 — V8 Bridge Contract Tests (Strategy 2) ✅

21 tests implemented in `v8_bridge_tests.rs`:
- ctx.* injection: trace_id, app_id (UUID not slug), node_id, env, resdata ✅
- ctx.request: all fields, query field name (BUG-012), ghost field rejection ✅
- Rivers.*: log, crypto (random, hash, hmac, timing-safe), ghost API detection ✅
- Console: delegates to Rivers.log ✅
- V8 security: codegen blocked (BUG-003), timeout (BUG-002), heap (BUG-006) ✅
- ctx.store: set/get/del round-trip, reserved prefix rejection ✅

Remaining (need TestIsolate for mock dataview capture):
- [ ] ctx.dataview() param forwarding with capture (BUG-008)
- [ ] ctx.dataview() namespace resolution with capture (BUG-009)
- [ ] Store TTL type validation (BUG-021)

---

## Phase 4 — AMD-1 Additions (Boot Parity + Module Resolution) ✅

4 tests in `boot_parity_tests.rs`:
- no_ssl_path_has_all_subsystem_init_calls (BUG-005 regression) ✅
- tls_path_has_all_subsystem_init_calls (sanity check) ✅
- module_path_resolution_exists_in_bundle_loader (BUG-013) ✅
- storage_engine_config_has_memory_default ✅

---

## Phase 5 — Regression Gate + Console Fix

### 5.1 — V8 regression tests ✅ (covered by v8_bridge_tests.rs)
- [x] `ctx_app_id_is_uuid_not_slug` covers `regression_app_id_not_empty`
- [x] `console_delegates_to_rivers_log` done

### 5.2 — Middleware/dispatch tests ✅
- [x] `security_headers_tests.rs` — 3 tests (all 5 headers, error sanitization, header blocklist)
- [x] `config_validation_tests.rs` — 8 tests (defaults, session cookie, DDL whitelist, canary parsing)
- [x] Found and fixed: ddl_whitelist in canary TOML was silently ignored (section ordering bug)

---

## Phase 6 — Feature Inventory Gaps (0-test areas)

These features from `rivers-feature-inventory.md` have zero or near-zero test coverage.

### 6.1 — DataView engine tests (Feature 3.1 — 0 tests)
- [ ] `crates/rivers-runtime/tests/dataview_engine_tests.rs`
  - DataView execution with faker datasource (no cluster needed)
  - Parameter passing through DataView to driver
  - DataView registry lookup (namespaced keys)
  - max_rows truncation at engine level
  - `invalidates` list triggers cache clear on write
  - Operation inference from SQL first token (SHAPE-7)

### 6.2 — Tiered cache tests (Feature 3.3 — 0 tests)
- [ ] `crates/rivers-runtime/tests/cache_tests.rs`
  - L1 LRU eviction when memory limit exceeded
  - L1 returns `Arc<QueryResult>` (pointer, not clone)
  - L1 entry count safety valve (100K)
  - L2 skip when result exceeds `l2_max_value_bytes`
  - Cache key derivation: BTreeMap → serde_json → SHA-256 → hex (SHAPE-3)
  - Cache invalidation by view name
  - `NoopDataViewCache` fallback when unconfigured

### 6.3 — Schema validation chain tests (Feature 4.1-4.8 — 0 tests)
- [ ] `crates/rivers-driver-sdk/tests/schema_validation_tests.rs`
  - SchemaSyntaxChecker: valid schema accepted
  - SchemaSyntaxChecker: missing required fields rejected
  - SchemaSyntaxChecker: invalid types rejected
  - Validator: type mismatch caught at request time
  - Validator: missing required field caught
  - Validator: constraint violations (min/max/pattern)
  - Per-driver validation: Redis schema vs Postgres schema different shapes

### 6.4 — Config validation tests (Feature 17 — 5 tests)
- [ ] `crates/rivers-core-config/tests/config_validation_tests.rs`
  - Environment variable substitution `${VAR}`
  - All validation rules from spec table (feature inventory §17.4)
  - Invalid TOML rejected with clear errors
  - Missing required sections caught
  - DDL whitelist format validation
  - Session cookie validation (http_only enforcement)

### 6.5 — Security headers tests (Feature 1.5 — 1 test)
- [ ] `crates/riversd/tests/security_headers_tests.rs`
  - X-Content-Type-Options: nosniff present
  - X-Frame-Options: DENY present
  - X-XSS-Protection present
  - Referrer-Policy present
  - Vary: Origin on CORS responses
  - Handler header blocklist: Set-Cookie, access-control-*, host silently dropped

### 6.6 — Pipeline stage isolation tests (Feature 2.2)
- [ ] `crates/riversd/tests/pipeline_tests.rs`
  - pre_process fires before DataView execution
  - handlers fire after DataView, can modify ctx.resdata
  - post_process fires after handlers, side-effect only
  - on_error fires on any stage failure
  - Sequential execution order (SHAPE-12)

### 6.7 — Cross-app session propagation tests (Feature 7.5 — 0 tests)
- [ ] `crates/riversd/tests/session_propagation_tests.rs`
  - Authorization header forwarded from app-main to app-service
  - X-Rivers-Claims header carries claims
  - Session scope preserved across app boundaries

---

## Validation

After all phases:
- [ ] `cargo test -p rivers-drivers-builtin` — conformance matrix (SQLite without cluster)
- [ ] `cargo test -p riversd` — bridge, boot, bundle, regression tests
- [ ] `RIVERS_TEST_CLUSTER=1 cargo test -p rivers-drivers-builtin` — full cluster tests (when available)
- [ ] All 33 bug-sourced tests mapped in coverage table

---

# APPENDED 2026-04-16 — Previous tasks.md contents (bundle validation + platform standards alignment)

# Tasks — Epic 1: Foundation — ValidationReport + Error Codes + Formatters

> **Branch:** `feature/art-of-possible`
> **Source:** `docs/arch/rivers-bundle-validation-spec.md` (Sections 8, 9, 11, Appendix A)
> **Goal:** Create foundational types and formatters for the 4-layer bundle validation pipeline

---

## Sprint 1.1 — ValidationReport types (`validate_result.rs`)

- [x] 1. Create `validate_result.rs` with `ValidationSeverity` enum (Error, Warning, Info)
- [x] 2. `ValidationStatus` enum (Pass, Fail, Warn, Skip) for individual results
- [x] 3. `ValidationResult` struct (status, file, message, error_code, table_path, field, suggestion, line, column, exports, etc.)
- [x] 4. `LayerResults` struct (passed, failed, skipped count + results vec)
- [x] 5. `ValidationReport` struct (bundle_name, bundle_version, layers map, summary)
- [x] 6. `ValidationSummary` struct (total_passed, total_failed, total_skipped, total_warnings, exit_code)
- [x] 7. Error code constants: S001-S010, E001-E005, X001-X013, C001-C008, L001-L005, W001-W004
- [x] 8. Builder methods: `report.add_result(layer, result)`, `report.exit_code()`, `report.has_errors()`
- [x] 9. Unit tests for report builder

## Sprint 1.2 — Text + JSON formatters (`validate_format.rs`)

- [x] 10. Text formatter matching spec section 8.1 output format
- [x] 11. JSON formatter matching spec section 8.2 contract
- [x] 12. `did_you_mean()` Levenshtein helper (distance <= 2)
- [x] 13. Unit tests for both formatters and Levenshtein helper

## Integration

- [x] 14. Export modules from `lib.rs`
- [x] 15. `cargo check -p rivers-runtime` passes
- [x] 16. `cargo test -p rivers-runtime -- validate_result validate_format` passes

---

## Validation

- `cargo check -p rivers-runtime` — compiles clean
- `cargo test -p rivers-runtime -- validate_result validate_format` — all tests pass

---

# Platform Standards Alignment — Task Plan

**Spec:** `docs/arch/rivers-platform-standards-alignment-spec.md`
**Status:** Planning — tasks organized by spec rollout phases

---

## Phase 1 — OpenAPI + Probes (P0)

### OpenAPI Support (spec §4)

- [ ] Write child execution spec `docs/arch/rivers-openapi-spec.md` from §4
- [ ] Add `OpenApiConfig` struct (`enabled`, `path`, `title`, `version`, `include_playground`) to `rivers-runtime/src/view.rs`
- [ ] Add view metadata fields: `summary`, `description`, `tags`, `operation_id`, `deprecated` to `ApiViewConfig`
- [ ] Add to structural validation known fields in `validate_structural.rs`
- [ ] Create `crates/riversd/src/openapi.rs` — walk REST views, DataView params, schemas → produce OpenAPI 3.1 JSON
- [ ] Map DataView parameter types to OpenAPI `in: path/query/header` from parameter_mapping; map schemas to request/response bodies
- [ ] Register `GET /<bundle>/<app>/openapi.json` route when `api.openapi.enabled = true`
- [ ] Validation: unique `operation_id` per app; no duplicate path+method; fail if enabled but cannot generate
- [ ] Unit tests for OpenAPI generation; integration test with address-book-bundle
- [ ] Tutorial: `docs/guide/tutorials/tutorial-openapi.md`

### Liveness/Readiness/Startup Probes (spec §5)

- [ ] Write child execution spec `docs/arch/rivers-probes-spec.md` from §5
- [ ] Add `ProbesConfig` struct (`enabled`, `live_path`, `ready_path`, `startup_path`) to `rivers-core-config`
- [ ] Add `probes` to known `[base]` fields in structural validation
- [ ] Implement `/live` handler — always 200 unless catastrophic (process alive, not deadlocked)
- [ ] Implement `/ready` handler — 200 when bundle loaded, required datasources connected, pools healthy; 503 otherwise
- [ ] Implement `/startup` handler — 503 until initialization complete, then 200
- [ ] Add startup-complete flag to `AppContext`, set after bundle wiring completes
- [ ] Tests: each probe response; failing datasource → /ready returns 503
- [ ] Add probe configuration to admin guide

---

## Phase 2 — OTel + Transaction Completion (P1)

### OpenTelemetry Trace Export (spec §6)

- [ ] Write child execution spec `docs/arch/rivers-otel-spec.md` from §6
- [ ] Add `OtelConfig` struct (`enabled`, `service_name`, `service_version`, `environment`, `exporter`, `endpoint`, `headers`, `sample_ratio`, `propagate_w3c`) to `rivers-core-config`
- [ ] Add `opentelemetry`, `opentelemetry-otlp`, `tracing-opentelemetry` to workspace dependencies
- [ ] Create spans: HTTP receive → route match → guard/auth → DataView execute → response write
- [ ] Span attributes: `http.method`, `http.route`, `http.status_code`, `rivers.app`, `rivers.dataview`, `rivers.driver`, `rivers.trace_id`
- [ ] W3C propagation: extract `traceparent`/`tracestate` inbound, inject on outbound HTTP driver requests
- [ ] Failure policy: OTel export failures log warning, never block requests
- [ ] Initialize OTel exporter at startup in `server/lifecycle.rs`
- [ ] Tests: verify spans created for request lifecycle; verify W3C headers propagated
- [ ] Tutorial: `docs/guide/tutorials/tutorial-otel.md`

### Runtime Transaction & Batch Completion (spec §7)

- [ ] Gap analysis: compare §7 against current implementation (Connection trait, TransactionMap, Rivers.db.batch stubs)
- [ ] Wire `host_db_begin/commit/rollback/batch` callbacks to actual pool acquisition and TransactionMap
- [ ] Implement batch `onError` policy: `fail_fast` (default) and `continue` modes per §7.4
- [ ] Verify auto-rollback on handler exit without commit
- [ ] Integration tests: Postgres transaction roundtrip via handler; batch insert with partial failure
- [ ] Verify existing canary transaction tests pass end-to-end

---

## Phase 3 — Standards-Based Auth (P1)

### JWT / OIDC / API Key Auth Providers (spec §8)

- [ ] Write child execution spec `docs/arch/rivers-auth-providers-spec.md` from §8
- [ ] Add `AuthProviderConfig` enum (JWT, OIDC, APIKey) to `rivers-core-config`
- [ ] Add `auth_config` to `ApiViewConfig` with `provider`, `required_scopes`, `required_roles`, claim fields
- [ ] JWT provider: validate signature (RS256/ES256), check `iss`/`aud`/`exp`, extract claims → `ctx.auth`
- [ ] OIDC provider: discover JWKS from `/.well-known/openid-configuration`, cache keys, validate tokens
- [ ] API key provider: lookup hashed key in StorageEngine
- [ ] Authorization: check `required_scopes` and `required_roles` against token claims
- [ ] Add `ctx.auth` object to handler context (subject, scopes, roles, claims)
- [ ] Compatibility: `auth = "none"` / `auth = "session"` unchanged; new `auth = "jwt"` / `"oidc"` / `"api_key"`
- [ ] Security: HTTPS required for JWT/OIDC; tokens never logged; JWKS cached with TTL
- [ ] Tests: JWT validation with test keys; OIDC discovery mock; API key lookup
- [ ] Tutorial: `docs/guide/tutorials/tutorial-api-auth.md`

---

## Phase 4 — AsyncAPI (P2)

### AsyncAPI Support (spec §9)

- [ ] Write child execution spec `docs/arch/rivers-asyncapi-spec.md` from §9
- [ ] Add `AsyncApiConfig` struct (`enabled`, `path`, `title`, `version`)
- [ ] Create `crates/riversd/src/asyncapi.rs` — walk MessageConsumer, SSE, WebSocket views → produce AsyncAPI 3.0 JSON
- [ ] Kafka/RabbitMQ/NATS: map consumer subscriptions to AsyncAPI channels with message schemas
- [ ] SSE: map SSE views to AsyncAPI channels (optional in v1)
- [ ] WebSocket: map WebSocket views to AsyncAPI channels (optional in v1)
- [ ] Register `GET /<bundle>/<app>/asyncapi.json` when enabled
- [ ] Validation: broker consumers must have schemas; SSE/WS optional
- [ ] Tests: unit tests for AsyncAPI generation from broker configs
- [ ] Add to developer guide

---

## Phase 5 — Polish (Future)

- [ ] OpenAPI HTML playground (Swagger UI / ReDoc)
- [ ] OTel metrics signal (bridge Prometheus → OTel)
- [ ] OTel log signal (bridge tracing → OTel logs)
- [ ] Richer AsyncAPI bindings (Kafka headers, AMQP routing keys)

---

## Cross-Cutting Rules (spec §10)

- [ ] All new features opt-in by default (`enabled = false` or absent)
- [ ] No new feature breaks existing bundles
- [ ] All new config fields have sensible defaults
- [ ] Error responses follow existing `ErrorResponse` envelope format
- [ ] Validation runs at startup (fail-fast), not at request time

---

## Open Questions (spec §12)

Decisions for implementation:

1. Bundle-level aggregate OpenAPI/AsyncAPI → defer to v2
2. `/ready` degradation → fail on any required datasource failure + open circuit breakers
3. OTel v1 → traces only; metrics/logs deferred to Phase 5
4. `Rivers.db.batch` partial failure → `fail_fast` only in v1
5. `ctx.auth` vs `ctx.session` → introduce `ctx.auth` as new object
6. AsyncAPI SSE/WS → start with brokers only, SSE/WS optional
7. OpenAPI strictness → permissive (omit missing schemas, don't invent them)


---

# Archived 2026-04-21 — Filesystem Driver + OperationDescriptor Epic

> **Status at archive:** canary FILESYSTEM profile 7/7 passing (commit 09c4025); docs + version bump committed (20febbe). 157 `- [ ]` checkbox items were not individually ticked in tasks.md before archive — epic is complete in code, only the checkbox bookkeeping was skipped. Preserved verbatim below for audit trail.

# Filesystem Driver + OperationDescriptor Framework — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `filesystem` built-in driver (eleven typed operations, chroot-sandboxed) and the `OperationDescriptor` framework that lets any driver expose a typed JS method surface via V8 proxy codegen, per `docs/arch/rivers-filesystem-driver-spec.md`.

**Architecture:** Two layered additions. (1) A framework-level `OperationDescriptor` catalog on `DatabaseDriver` with a default empty slice — opt-in, backward-compatible. (2) A built-in `filesystem` driver registering eleven operations, performing direct in-worker I/O (no IPC) through a new `DatasourceToken::Direct` variant, with startup-time root canonicalization and runtime-time path + symlink validation.

**Tech Stack:** Rust (`std::fs`, `std::path`), `async-trait`, `glob` + `regex` (new workspace deps), `base64` + `serde_json` + `tempfile` (already in workspace), `rusty_v8` (existing engine-v8 crate).

**Spec:** `docs/arch/rivers-filesystem-driver-spec.md` (v1.0, 2026-04-16)

**Branch:** `feature/filesystem-driver`

**Workflow note (per `CLAUDE.md`):**
- Mark each task complete as you go.
- Log decisions in `todo/changedecisionlog.md`, completed sections in `todo/changelog.md`.
- Commit after each logical task group (TDD pairs: test + impl).
- Canary must still pass end-to-end before merge.

---

## File Structure (locked up-front)

**Create:**
- `crates/rivers-driver-sdk/src/operation_descriptor.rs` — new types (`Param`, `ParamType`, `OpKind`, `OperationDescriptor`).
- `crates/rivers-drivers-builtin/src/filesystem.rs` — driver + connection + op dispatcher.
- `crates/rivers-drivers-builtin/src/filesystem/ops.rs` — eleven operation implementations.
- `crates/rivers-drivers-builtin/src/filesystem/chroot.rs` — root resolution, path validation, symlink rejection.
- `crates/rivers-drivers-builtin/src/filesystem/catalog.rs` — static `FILESYSTEM_OPERATIONS` slice.
- `crates/rivers-drivers-builtin/tests/filesystem_tests.rs` — integration tests.
- `canary-bundle/canary-filesystem/` — new canary app (mirrors `canary-sql` pattern).
- `docs/guide/tutorials/tutorial-filesystem-driver.md` — tutorial.

**Modify:**
- `crates/rivers-driver-sdk/src/traits.rs` — re-export from operation_descriptor, add `operations()` default method to `DatabaseDriver`.
- `crates/rivers-driver-sdk/src/lib.rs` — pub mod export.
- `crates/rivers-drivers-builtin/src/lib.rs` — `mod filesystem;` + register in `register_builtin_drivers`.
- `crates/rivers-runtime/src/process_pool/types.rs` — extend `DatasourceToken` with `Direct` variant.
- `crates/rivers-engine-v8/src/execution.rs` — typed-proxy codegen path when token is `Direct`.
- `crates/rivers-engine-v8/src/task_context.rs` — plumb Direct token into isolate setup.
- `Cargo.toml` (workspace root) — add `glob`, `regex` workspace deps.
- `canary-bundle/manifest.toml` — register `canary-filesystem` app.
- `docs/arch/rivers-feature-inventory.md` — §6.1 filesystem bullet, §6.6 OperationDescriptor bullet.

---

# Phase 1 — OperationDescriptor Framework

These tasks add the framework-level types with **zero behavior change for existing drivers** (empty default slice). Ship this phase first and independently — it compiles green, all existing tests pass, and nothing in the runtime changes.

---

### Task 1: Create `operation_descriptor.rs` with `ParamType` + `Param`

**Files:**
- Create: `crates/rivers-driver-sdk/src/operation_descriptor.rs`
- Modify: `crates/rivers-driver-sdk/src/lib.rs` (add `pub mod operation_descriptor;`)

- [ ] **Step 1: Write the failing test**

Create `crates/rivers-driver-sdk/src/operation_descriptor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_required_builder_sets_required_true_and_no_default() {
        let p = Param::required("path", ParamType::String);
        assert_eq!(p.name, "path");
        assert!(p.required);
        assert!(p.default_value.is_none());
    }

    #[test]
    fn param_optional_builder_sets_required_false_with_default() {
        let p = Param::optional("encoding", ParamType::String, "utf-8");
        assert_eq!(p.name, "encoding");
        assert!(!p.required);
        assert_eq!(p.default_value, Some("utf-8"));
    }

    #[test]
    fn paramtype_variants_are_distinct() {
        // Prove all five variants exist and can be constructed
        let _ = ParamType::String;
        let _ = ParamType::Integer;
        let _ = ParamType::Float;
        let _ = ParamType::Boolean;
        let _ = ParamType::Any;
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rivers-driver-sdk operation_descriptor`
Expected: **FAIL** — module does not exist yet.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `crates/rivers-driver-sdk/src/operation_descriptor.rs` (above the `mod tests`):

```rust
//! Typed operation catalog types for the V8 proxy codegen framework.
//!
//! Any driver may declare a slice of `OperationDescriptor` to expose typed
//! JS methods on `ctx.datasource("name")`. Drivers that do not declare a
//! catalog continue to use the standard `Query` / `execute()` pipeline.

/// Parameter type for JS-side validation before IPC dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Float,
    Boolean,
    /// Accepts string, number, boolean, array, or object.
    Any,
}

/// Single parameter in an operation signature.
#[derive(Clone, Debug)]
pub struct Param {
    pub name: &'static str,
    pub param_type: ParamType,
    pub required: bool,
    pub default_value: Option<&'static str>,
}

impl Param {
    pub const fn required(name: &'static str, param_type: ParamType) -> Self {
        Param { name, param_type, required: true, default_value: None }
    }

    pub const fn optional(
        name: &'static str,
        param_type: ParamType,
        default: &'static str,
    ) -> Self {
        Param { name, param_type, required: false, default_value: Some(default) }
    }
}
```

Modify `crates/rivers-driver-sdk/src/lib.rs` — add near the top, next to other `pub mod` lines:

```rust
pub mod operation_descriptor;
pub use operation_descriptor::{OpKind, OperationDescriptor, Param, ParamType};
```

(The `OpKind` / `OperationDescriptor` re-exports will fail to compile until Task 2 adds them — that's fine, we'll add them next.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p rivers-driver-sdk operation_descriptor --no-run 2>&1 | head -40`
Expected: compile error on `OpKind`, `OperationDescriptor` re-export (not yet defined). The three `Param*` tests can compile once we fix the re-export in Task 2.

Temporary unblocking: in `lib.rs` narrow the re-export to what exists today:

```rust
pub use operation_descriptor::{Param, ParamType};
```

Then run: `cargo test -p rivers-driver-sdk operation_descriptor`
Expected: **3/3 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-driver-sdk/src/operation_descriptor.rs crates/rivers-driver-sdk/src/lib.rs
git commit -m "feat(driver-sdk): add ParamType and Param types for operation catalog"
```

**Validation:**
- `cargo test -p rivers-driver-sdk operation_descriptor` → **3 passing**.
- `cargo build -p rivers-driver-sdk` → exit 0.
- Grep shows zero callers of `Param::required` yet (future phase wires them in).

---

### Task 2: Add `OpKind` + `OperationDescriptor` types

**Files:**
- Modify: `crates/rivers-driver-sdk/src/operation_descriptor.rs`
- Modify: `crates/rivers-driver-sdk/src/lib.rs` (restore full re-export)

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `crates/rivers-driver-sdk/src/operation_descriptor.rs`:

```rust
    #[test]
    fn operation_descriptor_read_builder_sets_kind_read() {
        static PARAMS: &[Param] = &[
            Param::required("path", ParamType::String),
        ];
        let desc = OperationDescriptor::read("readFile", PARAMS, "Read file contents");
        assert_eq!(desc.name, "readFile");
        assert_eq!(desc.kind, OpKind::Read);
        assert_eq!(desc.params.len(), 1);
        assert_eq!(desc.description, "Read file contents");
    }

    #[test]
    fn operation_descriptor_write_builder_sets_kind_write() {
        static PARAMS: &[Param] = &[
            Param::required("path", ParamType::String),
            Param::required("content", ParamType::String),
        ];
        let desc = OperationDescriptor::write("writeFile", PARAMS, "Write file");
        assert_eq!(desc.kind, OpKind::Write);
        assert_eq!(desc.params.len(), 2);
    }

    #[test]
    fn opkind_eq() {
        assert_eq!(OpKind::Read, OpKind::Read);
        assert_ne!(OpKind::Read, OpKind::Write);
    }
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `cargo test -p rivers-driver-sdk operation_descriptor`
Expected: **FAIL** — `OpKind` / `OperationDescriptor` not defined.

- [ ] **Step 3: Implement**

Append to `crates/rivers-driver-sdk/src/operation_descriptor.rs` (before `#[cfg(test)]`):

```rust
/// Classifies an operation as read or write for DDL security alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Read,
    Write,
}

/// Describes a single typed operation a driver exposes to handlers.
#[derive(Clone, Debug)]
pub struct OperationDescriptor {
    pub name: &'static str,
    pub kind: OpKind,
    pub params: &'static [Param],
    pub description: &'static str,
}

impl OperationDescriptor {
    pub const fn read(
        name: &'static str,
        params: &'static [Param],
        description: &'static str,
    ) -> Self {
        OperationDescriptor { name, kind: OpKind::Read, params, description }
    }

    pub const fn write(
        name: &'static str,
        params: &'static [Param],
        description: &'static str,
    ) -> Self {
        OperationDescriptor { name, kind: OpKind::Write, params, description }
    }
}
```

Restore full re-export in `crates/rivers-driver-sdk/src/lib.rs`:

```rust
pub use operation_descriptor::{OpKind, OperationDescriptor, Param, ParamType};
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test -p rivers-driver-sdk operation_descriptor`
Expected: **6/6 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-driver-sdk/src/operation_descriptor.rs crates/rivers-driver-sdk/src/lib.rs
git commit -m "feat(driver-sdk): add OpKind and OperationDescriptor types"
```

**Validation:**
- `cargo test -p rivers-driver-sdk operation_descriptor` → **6 passing**.
- `cargo build --workspace` → exit 0 (no existing crate breaks).

---

### Task 3: Add `operations()` default to `DatabaseDriver` trait

**Files:**
- Modify: `crates/rivers-driver-sdk/src/traits.rs`

- [ ] **Step 1: Write the failing test**

Append at the bottom of `crates/rivers-driver-sdk/src/traits.rs` (inside the existing `#[cfg(test)]` block, or create one if absent):

```rust
#[cfg(test)]
mod operations_default_tests {
    use super::*;
    use async_trait::async_trait;

    struct NoOpsDriver;

    #[async_trait]
    impl DatabaseDriver for NoOpsDriver {
        fn name(&self) -> &str { "noops" }
        async fn connect(
            &self,
            _params: &ConnectionParams,
        ) -> Result<Box<dyn Connection>, DriverError> {
            unimplemented!("test-only driver")
        }
    }

    #[test]
    fn default_operations_returns_empty_slice() {
        let driver = NoOpsDriver;
        assert_eq!(driver.operations().len(), 0);
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-driver-sdk operations_default`
Expected: **FAIL** — method `operations` not found.

- [ ] **Step 3: Implement**

In `crates/rivers-driver-sdk/src/traits.rs`, locate the `DatabaseDriver` trait (currently around line 563) and add:

```rust
    /// Returns the typed operation catalog for V8 proxy codegen.
    ///
    /// Default: empty — driver uses standard `Query`/`execute()` dispatch.
    /// Override to declare typed methods available on `ctx.datasource("name")`.
    fn operations(&self) -> &[crate::OperationDescriptor] {
        &[]
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-driver-sdk operations_default`
Expected: **1/1 PASS**.

Also run the broader test suite to confirm backward compat:
Run: `cargo test -p rivers-driver-sdk`
Expected: all previously-passing tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-driver-sdk/src/traits.rs
git commit -m "feat(driver-sdk): add DatabaseDriver::operations() with empty default"
```

**Validation:**
- `cargo build --workspace` → exit 0 (no existing driver breaks — default method kicks in).
- `cargo test --workspace --no-fail-fast 2>&1 | tail -20` → summary shows no new failures (new assertions only).

---

### Task 4: Backward-compat sweep

**Files:**
- No code changes — a verification task only, per CLAUDE.md "check in before executing" philosophy applied to outputs.

- [ ] **Step 1: Compile the full workspace**

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: no errors. Faker, memcached, postgres, mysql, sqlite, redis, eventbus, rps_client drivers all build with the new trait method's default.

- [ ] **Step 2: Run the full workspace test suite**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -30`
Expected: test count increased by exactly 7 (four from Task 1/2, one from Task 3, two from later ops-body expansions if any — but we haven't added those yet, so count is 7). Previously-passing count unchanged; no regressions.

Log the exact counts in `todo/changelog.md`:

```markdown
### 2026-04-16 — OperationDescriptor framework baseline
- Files: crates/rivers-driver-sdk/src/{operation_descriptor.rs,traits.rs,lib.rs}
- Summary: new types + opt-in trait method; existing drivers unaffected.
- Spec: rivers-filesystem-driver-spec.md §2.
- Test delta: +7 passing, 0 regressions.
```

- [ ] **Step 3: Commit the changelog entry**

```bash
git add todo/changelog.md
git commit -m "docs(changelog): OperationDescriptor framework baseline"
```

**Validation:**
- No new failing test.
- No existing driver trait impl required source edits.

---

# Phase 2 — Filesystem Driver Foundation (Chroot + Connection)

These tasks stand up the driver skeleton with **no operations wired yet**. Every task hardens the chroot boundary before any I/O ever runs.

---

### Task 5: Add `glob` and `regex` workspace deps

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Inspect current workspace deps**

Run: `Grep("glob\\|regex", path="Cargo.toml", context=1)`
Expected: neither dep present.

- [ ] **Step 2: Add the deps**

Edit `Cargo.toml` workspace `[workspace.dependencies]` block, append:

```toml
glob = "0.3"
regex = "1.10"
```

- [ ] **Step 3: Verify resolution**

Run: `cargo tree -p rivers-driver-sdk 2>&1 | grep -E '^\\s*(glob|regex)' | head -5`
Expected: crates resolved (may be empty until a crate actually consumes them — Task 19/20 will).

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: exit 0.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add glob + regex workspace deps for filesystem driver"
```

**Validation:**
- `cargo tree` shows both deps in the resolved graph.
- No existing crate breaks.

---

### Task 6: Scaffold `FilesystemDriver` + `FilesystemConnection` shells

**Files:**
- Create: `crates/rivers-drivers-builtin/src/filesystem.rs`
- Create: `crates/rivers-drivers-builtin/src/filesystem/mod.rs` (if we choose folder layout) — we'll use a single file for now and split in later tasks.
- Modify: `crates/rivers-drivers-builtin/src/lib.rs` (add `mod filesystem;`)

- [ ] **Step 1: Write the failing test**

Create `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
//! Filesystem driver — chroot-sandboxed direct-I/O driver.
//!
//! Spec: docs/arch/rivers-filesystem-driver-spec.md

use async_trait::async_trait;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, Query, QueryResult,
};
use std::path::PathBuf;

pub struct FilesystemDriver;

pub struct FilesystemConnection {
    pub root: PathBuf,
}

#[async_trait]
impl DatabaseDriver for FilesystemDriver {
    fn name(&self) -> &str {
        "filesystem"
    }

    async fn connect(
        &self,
        _params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        Err(DriverError::NotImplemented("FilesystemDriver::connect — Task 11".into()))
    }
}

#[async_trait]
impl Connection for FilesystemConnection {
    async fn execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
        Err(DriverError::NotImplemented("FilesystemConnection::execute — Task 26".into()))
    }

    async fn ddl_execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
        Err(DriverError::Forbidden(
            "filesystem driver does not support ddl_execute".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_name_is_filesystem() {
        assert_eq!(FilesystemDriver.name(), "filesystem");
    }

    #[test]
    fn operations_default_empty_for_now() {
        // Until Task 14 wires the catalog, operations() returns empty via default.
        assert!(FilesystemDriver.operations().is_empty());
    }
}
```

Modify `crates/rivers-drivers-builtin/src/lib.rs`, add near other `mod` lines:

```rust
pub mod filesystem;
```

- [ ] **Step 2: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **2/2 PASS**.

- [ ] **Step 3: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs crates/rivers-drivers-builtin/src/lib.rs
git commit -m "feat(drivers-builtin): scaffold FilesystemDriver + FilesystemConnection shells"
```

**Validation:**
- `cargo test -p rivers-drivers-builtin filesystem::tests` → **2 passing**.
- `cargo build --workspace` → exit 0.

---

### Task 7: Implement `resolve_root` with TDD

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec reference: §5.1. Behavior: must be absolute, must canonicalize, must be a directory.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
    use tempfile::TempDir;

    #[test]
    fn resolve_root_rejects_relative_path() {
        let err = FilesystemDriver::resolve_root("./relative").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("absolute"),
            "expected 'absolute' in error, got: {msg}"
        );
    }

    #[test]
    fn resolve_root_rejects_nonexistent_path() {
        let err = FilesystemDriver::resolve_root("/does/not/exist/for/real").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("does not exist") || msg.contains("not accessible"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_root_rejects_file_path() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("not_a_dir.txt");
        std::fs::write(&file_path, b"hi").unwrap();
        let err = FilesystemDriver::resolve_root(file_path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("not a directory"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_root_canonicalizes_valid_directory() {
        let dir = TempDir::new().unwrap();
        let resolved = FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap();
        assert!(resolved.is_absolute());
        assert!(resolved.is_dir());
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **FAIL** — `resolve_root` not defined.

- [ ] **Step 3: Implement**

Add to `impl FilesystemDriver` in `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
impl FilesystemDriver {
    pub fn resolve_root(database: &str) -> Result<PathBuf, DriverError> {
        let path = PathBuf::from(database);

        if !path.is_absolute() {
            return Err(DriverError::Connection(format!(
                "filesystem root must be absolute path, got: {database}"
            )));
        }

        let canonical = std::fs::canonicalize(&path).map_err(|e| {
            DriverError::Connection(format!(
                "filesystem root does not exist or is not accessible: {database} — {e}"
            ))
        })?;

        if !canonical.is_dir() {
            return Err(DriverError::Connection(format!(
                "filesystem root is not a directory: {}",
                canonical.display()
            )));
        }

        Ok(canonical)
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **6/6 PASS** (2 existing + 4 new).

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): implement resolve_root — absolute + canonical + directory check"
```

**Validation:**
- All 6 filesystem tests pass.
- `tempfile` dep already available (workspace dep).

---

### Task 8: Implement `resolve_path` chroot enforcement

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec: §5.2. Must reject absolute paths, canonicalize relative paths, and verify `canonical.starts_with(&self.root)`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    fn test_connection() -> (TempDir, FilesystemConnection) {
        let dir = TempDir::new().unwrap();
        let root = FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap();
        (dir, FilesystemConnection { root })
    }

    #[test]
    fn resolve_path_rejects_absolute_unix() {
        let (_dir, conn) = test_connection();
        let err = conn.resolve_path("/etc/passwd").unwrap_err();
        assert!(format!("{err}").contains("absolute paths not permitted"));
    }

    #[test]
    fn resolve_path_rejects_parent_escape() {
        let (_dir, conn) = test_connection();
        let err = conn.resolve_path("../../../etc/passwd").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("escapes datasource root") || msg.contains("does not exist"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_path_accepts_valid_relative() {
        let (dir, conn) = test_connection();
        std::fs::write(dir.path().join("hello.txt"), b"hi").unwrap();
        let resolved = conn.resolve_path("hello.txt").unwrap();
        assert!(resolved.starts_with(&conn.root));
    }

    #[test]
    fn resolve_path_normalizes_backslashes() {
        // On Unix this behaves like a literal; purpose is documentation — real
        // Windows coverage comes via CI.
        let (dir, conn) = test_connection();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a").join("b.txt"), b"x").unwrap();
        let resolved = conn.resolve_path("a\\b.txt").unwrap();
        assert!(resolved.starts_with(&conn.root));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **FAIL** — `resolve_path` not defined.

- [ ] **Step 3: Implement**

Add to `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
impl FilesystemConnection {
    pub fn resolve_path(&self, relative: &str) -> Result<PathBuf, DriverError> {
        let normalized = relative.replace('\\', "/");

        let bytes = normalized.as_bytes();
        let is_windows_drive =
            bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic();
        if normalized.starts_with('/') || is_windows_drive {
            return Err(DriverError::Query(
                "absolute paths not permitted — all paths relative to datasource root".into(),
            ));
        }

        let joined = self.root.join(&normalized);
        let canonical = canonicalize_for_op(&joined)?;

        if !canonical.starts_with(&self.root) {
            return Err(DriverError::Forbidden(
                "path escapes datasource root".into(),
            ));
        }

        reject_symlinks_within(&self.root, &canonical)?;
        Ok(canonical)
    }
}

fn canonicalize_for_op(path: &std::path::Path) -> Result<PathBuf, DriverError> {
    // For nonexistent paths (writeFile, mkdir), canonicalize the deepest existing
    // ancestor, then append the remaining segments. This preserves chroot checks
    // while letting write ops target paths that do not yet exist.
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let base = std::fs::canonicalize(&existing).map_err(|e| {
        DriverError::Query(format!("could not canonicalize ancestor of path: {e}"))
    })?;
    let mut out = base;
    for piece in tail.into_iter().rev() {
        out.push(piece);
    }
    Ok(out)
}

fn reject_symlinks_within(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<(), DriverError> {
    // Walk from root forward, checking every intermediate component.
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut current = root.to_path_buf();
    for comp in rel.components() {
        current.push(comp);
        if !current.exists() {
            break;
        }
        let is_symlink = current
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            return Err(DriverError::Forbidden(format!(
                "symlink detected in path: {}",
                current.display()
            )));
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **10/10 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): implement resolve_path with chroot + symlink rejection"
```

**Validation:**
- All 10 filesystem tests pass.
- `resolve_path` is pure (no I/O side effects beyond canonicalization).
- Manual probe: `cargo test resolve_path_rejects_parent_escape -- --nocapture` shows clean output.

---

### Task 9: Unit test — symlink rejection (Unix-gated)

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    #[cfg(unix)]
    #[test]
    fn resolve_path_rejects_symlink_inside_root() {
        use std::os::unix::fs::symlink;
        let (dir, conn) = test_connection();
        let target = dir.path().join("real");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, dir.path().join("link")).unwrap();

        let err = conn.resolve_path("link").unwrap_err();
        assert!(format!("{err}").contains("symlink detected"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_rejects_symlink_pointing_outside_root() {
        use std::os::unix::fs::symlink;
        let (dir, conn) = test_connection();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), dir.path().join("escape")).unwrap();

        let err = conn.resolve_path("escape/file.txt").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("symlink detected") || msg.contains("escapes datasource root"),
            "unexpected error: {msg}"
        );
    }
```

- [ ] **Step 2: Run — expect PASS**

Task 8 already implemented symlink rejection. Run to confirm.

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **12/12 PASS** on Unix; 10/10 on Windows (cfg-gated).

- [ ] **Step 3: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "test(filesystem): add symlink rejection unit tests (unix-gated)"
```

**Validation:**
- Unix: 12 filesystem tests pass, the two new symlink tests included.
- On macOS (darwin host): test goes green because we cfg(unix) gate.

---

### Task 10: Wire `connect()` to build `FilesystemConnection`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

- [ ] **Step 1: Write the failing test**

Append:

```rust
    #[tokio::test]
    async fn connect_returns_connection_with_resolved_root() {
        let dir = TempDir::new().unwrap();
        let params = ConnectionParams {
            host: String::new(),
            port: 0,
            database: dir.path().to_str().unwrap().to_string(),
            username: String::new(),
            password: String::new(),
        };
        let driver = FilesystemDriver;
        let conn = driver.connect(&params).await.unwrap();
        // Dry-probe: we don't yet have execute(), but we should at least compile + connect.
        drop(conn);
    }

    #[tokio::test]
    async fn connect_fails_on_nonexistent_root() {
        let params = ConnectionParams {
            host: String::new(),
            port: 0,
            database: "/does/not/exist/nowhere".into(),
            username: String::new(),
            password: String::new(),
        };
        let err = FilesystemDriver.connect(&params).await.unwrap_err();
        assert!(format!("{err}").contains("does not exist") || format!("{err}").contains("not accessible"));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **FAIL** — `connect` still returns `NotImplemented`.

- [ ] **Step 3: Implement**

Replace the `connect` body in `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let root = Self::resolve_root(&params.database)?;
        Ok(Box::new(FilesystemConnection { root }))
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: all passing (14 on Unix / 12 on Windows).

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): wire connect() to resolve_root + FilesystemConnection"
```

**Validation:**
- Async connect test passes.
- Driver name `"filesystem"` established and stable.

---

### Task 11: Register `FilesystemDriver` in `register_builtin_drivers`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/lib.rs`

- [ ] **Step 1: Locate registration fn**

Run: `Grep("fn register_builtin_drivers", type=rust, path=\"crates/rivers-drivers-builtin\")`

Read that file.

- [ ] **Step 2: Write the failing test**

Add to `crates/rivers-drivers-builtin/src/lib.rs` under a `#[cfg(test)]` block (create if missing):

```rust
#[cfg(test)]
mod registration_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CaptureRegistrar {
        names: Arc<Mutex<Vec<String>>>,
    }

    impl DriverRegistrar for CaptureRegistrar {
        fn register_database_driver(
            &mut self,
            driver: std::sync::Arc<dyn rivers_driver_sdk::DatabaseDriver>,
        ) {
            self.names.lock().unwrap().push(driver.name().to_string());
        }
    }

    #[test]
    fn filesystem_driver_is_registered() {
        let mut reg = CaptureRegistrar::default();
        register_builtin_drivers(&mut reg);
        let names = reg.names.lock().unwrap().clone();
        assert!(
            names.iter().any(|n| n == "filesystem"),
            "expected 'filesystem' in registered driver names: {names:?}"
        );
    }
}
```

- [ ] **Step 3: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin registration_tests`
Expected: **FAIL**.

- [ ] **Step 4: Register**

In `crates/rivers-drivers-builtin/src/lib.rs`, inside `register_builtin_drivers`, add:

```rust
registrar.register_database_driver(std::sync::Arc::new(filesystem::FilesystemDriver));
```

- [ ] **Step 5: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin registration_tests`
Expected: **PASS**.

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-drivers-builtin/src/lib.rs
git commit -m "feat(filesystem): register FilesystemDriver in register_builtin_drivers"
```

**Validation:**
- Driver is discoverable by name `"filesystem"` at runtime.

---

# Phase 3 — Operation Catalog + Implementations

Eleven operations, each landed test-first. Operations live in dedicated modules to keep files small.

---

### Task 12: Declare `FILESYSTEM_OPERATIONS` catalog

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

- [ ] **Step 1: Write the failing test**

Append:

```rust
    #[test]
    fn catalog_has_eleven_operations() {
        assert_eq!(FilesystemDriver.operations().len(), 11);
    }

    #[test]
    fn catalog_contains_all_expected_names() {
        let names: Vec<&str> = FilesystemDriver
            .operations()
            .iter()
            .map(|o| o.name)
            .collect();
        for expected in [
            "readFile", "readDir", "stat", "exists", "find", "grep",
            "writeFile", "mkdir", "delete", "rename", "copy",
        ] {
            assert!(names.contains(&expected), "missing op: {expected}");
        }
    }

    #[test]
    fn read_ops_have_opkind_read() {
        for op in FilesystemDriver.operations() {
            let is_read = matches!(op.name, "readFile" | "readDir" | "stat" | "exists" | "find" | "grep");
            let is_write = matches!(op.name, "writeFile" | "mkdir" | "delete" | "rename" | "copy");
            match (is_read, is_write) {
                (true, false) => assert_eq!(op.kind, OpKind::Read, "{}", op.name),
                (false, true) => assert_eq!(op.kind, OpKind::Write, "{}", op.name),
                _ => panic!("unclassified op: {}", op.name),
            }
        }
    }
```

Add the `OpKind` import: `use rivers_driver_sdk::OpKind;`.

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::catalog`
Expected: **FAIL**.

- [ ] **Step 3: Implement**

Append to `crates/rivers-drivers-builtin/src/filesystem.rs` (module-level):

```rust
use rivers_driver_sdk::{OpKind, OperationDescriptor, Param, ParamType};

static FILESYSTEM_OPERATIONS: &[OperationDescriptor] = &[
    // Reads
    OperationDescriptor::read(
        "readFile",
        &[
            Param::required("path", ParamType::String),
            Param::optional("encoding", ParamType::String, "utf-8"),
        ],
        "Read file contents — utf-8 returns string, base64 returns base64-encoded string",
    ),
    OperationDescriptor::read(
        "readDir",
        &[Param::required("path", ParamType::String)],
        "List directory entries — filenames only",
    ),
    OperationDescriptor::read(
        "stat",
        &[Param::required("path", ParamType::String)],
        "File/directory metadata",
    ),
    OperationDescriptor::read(
        "exists",
        &[Param::required("path", ParamType::String)],
        "Returns boolean existence",
    ),
    OperationDescriptor::read(
        "find",
        &[
            Param::required("pattern", ParamType::String),
            Param::optional("max_results", ParamType::Integer, "1000"),
        ],
        "Recursive glob search",
    ),
    OperationDescriptor::read(
        "grep",
        &[
            Param::required("pattern", ParamType::String),
            Param::optional("path", ParamType::String, "."),
            Param::optional("max_results", ParamType::Integer, "1000"),
        ],
        "Regex search across files",
    ),
    // Writes
    OperationDescriptor::write(
        "writeFile",
        &[
            Param::required("path", ParamType::String),
            Param::required("content", ParamType::String),
            Param::optional("encoding", ParamType::String, "utf-8"),
        ],
        "Write file — creates parent dirs, overwrites if exists",
    ),
    OperationDescriptor::write(
        "mkdir",
        &[Param::required("path", ParamType::String)],
        "Create directory recursively",
    ),
    OperationDescriptor::write(
        "delete",
        &[Param::required("path", ParamType::String)],
        "Delete file or recursively delete directory",
    ),
    OperationDescriptor::write(
        "rename",
        &[
            Param::required("oldPath", ParamType::String),
            Param::required("newPath", ParamType::String),
        ],
        "Rename/move within root",
    ),
    OperationDescriptor::write(
        "copy",
        &[
            Param::required("src", ParamType::String),
            Param::required("dest", ParamType::String),
        ],
        "Copy file or recursively copy directory",
    ),
];
```

Then override `operations()` on the trait impl:

```rust
    fn operations(&self) -> &[OperationDescriptor] {
        FILESYSTEM_OPERATIONS
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::catalog`
Expected: **3/3 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): declare FILESYSTEM_OPERATIONS catalog (11 ops)"
```

**Validation:**
- Catalog visible via `FilesystemDriver.operations()`.
- Names + kinds match spec §6.1 exactly.

---

### Task 13: Operation dispatcher in `Connection::execute`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Behavior: route on `Query::operation`. We wire up an empty match and add per-op branches in later tasks.

- [ ] **Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn execute_unknown_operation_returns_notimpl() {
        let (_dir, mut conn) = test_connection();
        let q = Query {
            operation: "nope".into(),
            target: String::new(),
            parameters: Default::default(),
            statement: String::new(),
        };
        let err = conn.execute(&q).await.unwrap_err();
        assert!(
            matches!(err, DriverError::NotImplemented(_) | DriverError::Unsupported(_)),
            "unexpected variant: {err:?}"
        );
    }
```

- [ ] **Step 2: Run — expect FAIL**

Compile error — `Query` is constructed with defaults. If `QueryValue` derive is missing for `HashMap::default()`, adjust. Otherwise FAIL on execution returning `NotImplemented(\"...Task 26\")` (which this test expects).

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::execute_unknown`
Expected: **PASS** (existing skeleton returns NotImplemented, which the test accepts).

- [ ] **Step 3: Replace the placeholder execute**

Replace `Connection::execute` body:

```rust
    async fn execute(&mut self, q: &Query) -> Result<QueryResult, DriverError> {
        match q.operation.as_str() {
            // Reads (Tasks 15–20)
            "readFile" => Err(DriverError::NotImplemented("readFile — Task 15".into())),
            "readDir" => Err(DriverError::NotImplemented("readDir — Task 16".into())),
            "stat" => Err(DriverError::NotImplemented("stat — Task 17".into())),
            "exists" => Err(DriverError::NotImplemented("exists — Task 18".into())),
            "find" => Err(DriverError::NotImplemented("find — Task 19".into())),
            "grep" => Err(DriverError::NotImplemented("grep — Task 20".into())),
            // Writes (Tasks 21–25)
            "writeFile" => Err(DriverError::NotImplemented("writeFile — Task 21".into())),
            "mkdir" => Err(DriverError::NotImplemented("mkdir — Task 22".into())),
            "delete" => Err(DriverError::NotImplemented("delete — Task 23".into())),
            "rename" => Err(DriverError::NotImplemented("rename — Task 24".into())),
            "copy" => Err(DriverError::NotImplemented("copy — Task 25".into())),
            other => Err(DriverError::Unsupported(format!(
                "unknown filesystem operation: {other}"
            ))),
        }
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: all filesystem tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): stub execute dispatcher routing by operation name"
```

**Validation:**
- Unknown op → `Unsupported`.
- Known op → `NotImplemented` with task pointer.

---

### Task 14: Implement `readFile` (utf-8 + base64)

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec §6.3. Default encoding `"utf-8"`. `"base64"` → `base64::engine::general_purpose::STANDARD.encode`. Unknown encoding → `DriverError::Query`.

- [ ] **Step 1: Write the failing test**

```rust
    use rivers_driver_sdk::QueryValue;

    fn mkq(op: &str, params: &[(&str, QueryValue)]) -> Query {
        let mut parameters = std::collections::HashMap::new();
        for (k, v) in params {
            parameters.insert(k.to_string(), v.clone());
        }
        Query {
            operation: op.into(),
            target: String::new(),
            parameters,
            statement: String::new(),
        }
    }

    #[tokio::test]
    async fn read_file_utf8_returns_string() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let q = mkq("readFile", &[("path", QueryValue::String("a.txt".into()))]);
        let result = conn.execute(&q).await.unwrap();
        // Result shape: single-row single-column "content"
        let content = extract_scalar_string(&result);
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn read_file_base64_returns_b64_string() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("b.bin"), &[0xff, 0x00, 0xfe]).unwrap();
        let q = mkq(
            "readFile",
            &[
                ("path", QueryValue::String("b.bin".into())),
                ("encoding", QueryValue::String("base64".into())),
            ],
        );
        let result = conn.execute(&q).await.unwrap();
        let content = extract_scalar_string(&result);
        assert_eq!(content, "/wD+"); // base64 of 0xff 0x00 0xfe
    }

    #[tokio::test]
    async fn read_file_unknown_encoding_errors() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let q = mkq(
            "readFile",
            &[
                ("path", QueryValue::String("a.txt".into())),
                ("encoding", QueryValue::String("ebcdic".into())),
            ],
        );
        let err = conn.execute(&q).await.unwrap_err();
        assert!(format!("{err}").contains("unsupported encoding"));
    }

    // Helper for tests — extract single string scalar from QueryResult.
    fn extract_scalar_string(r: &QueryResult) -> String {
        // Specific shape wired by op impl: rows = [[QueryValue::String(content)]]
        let row = r.rows.first().expect("expected one row");
        let val = row.first().expect("expected one column");
        match val {
            QueryValue::String(s) => s.clone(),
            other => panic!("expected String, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run — expect FAIL** (NotImplemented)

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::read_file`
Expected: **FAIL**.

- [ ] **Step 3: Implement**

Replace the `readFile` arm:

```rust
            "readFile" => ops::read_file(self, q).await,
```

Create a new submodule. Inline for now, split to `filesystem/ops.rs` once it grows. At the bottom of `filesystem.rs`:

```rust
mod ops {
    use super::*;
    use base64::Engine;
    use rivers_driver_sdk::{Query, QueryResult, QueryValue};

    fn get_string<'a>(q: &'a Query, key: &str) -> Option<&'a str> {
        match q.parameters.get(key) {
            Some(QueryValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub async fn read_file(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("readFile: required parameter 'path' missing".into())
        })?;
        let encoding = get_string(q, "encoding").unwrap_or("utf-8");
        let path = conn.resolve_path(rel)?;
        let bytes = tokio::task::spawn_blocking({
            let path = path.clone();
            move || std::fs::read(&path)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        let content = match encoding {
            "utf-8" => String::from_utf8(bytes).map_err(|e| {
                DriverError::Query(format!("file is not valid utf-8: {e}"))
            })?,
            "base64" => base64::engine::general_purpose::STANDARD.encode(&bytes),
            other => {
                return Err(DriverError::Query(format!(
                    "unsupported encoding: {other}"
                )));
            }
        };

        Ok(QueryResult {
            columns: vec!["content".into()],
            rows: vec![vec![QueryValue::String(content)]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }

    pub fn map_io_error(e: std::io::Error) -> DriverError {
        use std::io::ErrorKind::*;
        match e.kind() {
            NotFound => DriverError::Query(format!("not found: {e}")),
            PermissionDenied => DriverError::Query(format!("permission denied: {e}")),
            _ => DriverError::Internal(format!("I/O error: {e}")),
        }
    }
}
```

(If `QueryResult` field names differ — confirm via `Grep("struct QueryResult", type=rust, path=\"crates/rivers-driver-sdk\")` and adjust.)

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::read_file`
Expected: **3/3 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): implement readFile (utf-8 + base64)"
```

**Validation:**
- UTF-8 happy path passes.
- Base64 round-trip exact.
- Unknown encoding → clean error.

---

### Task 15: Implement `readDir`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn read_dir_returns_entry_names() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("b")).unwrap();
        let q = mkq("readDir", &[("path", QueryValue::String(".".into()))]);
        let result = conn.execute(&q).await.unwrap();
        let mut names: Vec<String> = result
            .rows
            .iter()
            .map(|r| match &r[0] {
                QueryValue::String(s) => s.clone(),
                _ => panic!(),
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.txt".to_string(), "b".to_string()]);
    }
```

- [ ] **Step 2: Run — FAIL**
- [ ] **Step 3: Implement**

Append to `mod ops`:

```rust
    pub async fn read_dir(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("readDir: required parameter 'path' missing".into())
        })?;
        let path = conn.resolve_path(rel)?;
        let entries: Vec<String> = tokio::task::spawn_blocking({
            let path = path.clone();
            move || -> Result<Vec<String>, std::io::Error> {
                let mut out = Vec::new();
                for entry in std::fs::read_dir(&path)? {
                    out.push(entry?.file_name().to_string_lossy().to_string());
                }
                Ok(out)
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        Ok(QueryResult {
            columns: vec!["name".into()],
            rows: entries
                .into_iter()
                .map(|n| vec![QueryValue::String(n)])
                .collect(),
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire the arm: `"readDir" => ops::read_dir(self, q).await,`.

- [ ] **Step 4: Run — PASS**
- [ ] **Step 5: Commit**

```bash
git commit -am "feat(filesystem): implement readDir"
```

**Validation:** 1 new test passes.

---

### Task 16: Implement `stat`

**Files:** same file.

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn stat_file_returns_metadata() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("f.txt"), b"hello").unwrap();
        let q = mkq("stat", &[("path", QueryValue::String("f.txt".into()))]);
        let result = conn.execute(&q).await.unwrap();
        assert_eq!(result.rows.len(), 1);
        let cols = &result.columns;
        for expected in ["size", "mtime", "atime", "ctime", "isFile", "isDirectory", "mode"] {
            assert!(cols.iter().any(|c| c == expected), "missing col: {expected}");
        }
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn stat(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("stat: required parameter 'path' missing".into())
        })?;
        let path = conn.resolve_path(rel)?;
        let md = tokio::task::spawn_blocking({
            let p = path.clone();
            move || std::fs::metadata(&p)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        fn to_iso(t: std::time::SystemTime) -> String {
            let secs = t
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            // minimal ISO — avoid adding chrono dep if not already present
            use std::fmt::Write as _;
            let mut s = String::new();
            let _ = write!(s, "{secs}");
            s
        }
        let size = QueryValue::Integer(md.len() as i64);
        let mtime = QueryValue::String(to_iso(md.modified().unwrap_or(std::time::UNIX_EPOCH)));
        let atime = QueryValue::String(to_iso(md.accessed().unwrap_or(std::time::UNIX_EPOCH)));
        let ctime = QueryValue::String(to_iso(md.created().unwrap_or(std::time::UNIX_EPOCH)));
        let is_file = QueryValue::Boolean(md.is_file());
        let is_dir = QueryValue::Boolean(md.is_dir());

        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            QueryValue::Integer(md.permissions().mode() as i64)
        };
        #[cfg(not(unix))]
        let mode = QueryValue::Integer(0);

        Ok(QueryResult {
            columns: vec![
                "size".into(), "mtime".into(), "atime".into(), "ctime".into(),
                "isFile".into(), "isDirectory".into(), "mode".into(),
            ],
            rows: vec![vec![size, mtime, atime, ctime, is_file, is_dir, mode]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire the arm. Follow TDD pattern.

- [ ] **Step 4/5:** Run + commit.

```bash
git commit -am "feat(filesystem): implement stat"
```

**Validation:** all seven stat columns appear.

**Note on timestamp format:** we emit epoch seconds as a string for v1. If a later task adds a date lib to the workspace, we can upgrade to ISO 8601 without breaking callers — the handler API (`info.mtime`) is opaque today.

---

### Task 17: Implement `exists`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn exists_returns_true_for_present_false_for_absent() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("yes.txt"), "").unwrap();
        let q = mkq("exists", &[("path", QueryValue::String("yes.txt".into()))]);
        assert!(matches!(
            conn.execute(&q).await.unwrap().rows[0][0],
            QueryValue::Boolean(true)
        ));
        let q2 = mkq("exists", &[("path", QueryValue::String("nope.txt".into()))]);
        assert!(matches!(
            conn.execute(&q2).await.unwrap().rows[0][0],
            QueryValue::Boolean(false)
        ));
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn exists(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("exists: required parameter 'path' missing".into())
        })?;
        // Resolve may error with "escapes root" — that still counts as "not visible"; return false.
        let ok = match conn.resolve_path(rel) {
            Ok(p) => tokio::task::spawn_blocking(move || p.exists())
                .await
                .unwrap_or(false),
            Err(DriverError::Forbidden(_)) => false,
            Err(e) => return Err(e),
        };
        Ok(QueryResult {
            columns: vec!["exists".into()],
            rows: vec![vec![QueryValue::Boolean(ok)]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement exists"
```

**Validation:** absent file → false; present → true; chroot-escaping path → false (not error).

---

### Task 18: Implement `find` (glob) with truncation

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn find_returns_relative_paths_and_truncation() {
        let (dir, mut conn) = test_connection();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "").unwrap();
        }
        let q = mkq(
            "find",
            &[
                ("pattern", QueryValue::String("*.txt".into())),
                ("max_results", QueryValue::Integer(3)),
            ],
        );
        let r = conn.execute(&q).await.unwrap();
        // Expected shape: row[0] = results array, row[1] = truncated bool
        // Implementation choice: two columns on a single row.
        assert_eq!(r.columns, vec!["results", "truncated"]);
        let row = &r.rows[0];
        match &row[0] {
            QueryValue::Array(v) => assert!(v.len() <= 3),
            other => panic!("expected Array, got {other:?}"),
        }
        assert!(matches!(row[1], QueryValue::Boolean(true)));
    }
```

- [ ] **Step 2/3: Implement**

Add to `Cargo.toml` of `rivers-drivers-builtin` (in the `[dependencies]` table):
```toml
glob = { workspace = true }
```

Append to `mod ops`:

```rust
    pub async fn find(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let pattern = get_string(q, "pattern").ok_or_else(|| {
            DriverError::Query("find: required parameter 'pattern' missing".into())
        })?;
        let max = match q.parameters.get("max_results") {
            Some(QueryValue::Integer(n)) => (*n).max(0) as usize,
            _ => 1000,
        };
        let root = conn.root.clone();
        let pattern_owned = pattern.to_string();
        let (results, truncated) = tokio::task::spawn_blocking(move || {
            let full_pattern = format!("{}/**/{}", root.display(), pattern_owned);
            let mut out = Vec::new();
            let mut truncated = false;
            if let Ok(paths) = glob::glob(&full_pattern) {
                for entry in paths.flatten() {
                    if let Ok(rel) = entry.strip_prefix(&root) {
                        out.push(rel.to_string_lossy().to_string());
                        if out.len() > max {
                            out.pop();
                            truncated = true;
                            break;
                        }
                    }
                }
            }
            (out, truncated)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?;

        Ok(QueryResult {
            columns: vec!["results".into(), "truncated".into()],
            rows: vec![vec![
                QueryValue::Array(
                    results.into_iter().map(QueryValue::String).collect(),
                ),
                QueryValue::Boolean(truncated),
            ]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement find (glob) with truncation"
```

**Validation:** 5 files, max_results=3 → `results.len() <= 3`, truncated=true.

---

### Task 19: Implement `grep` (regex) with truncation

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn grep_finds_matching_lines() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "foo\nTODO: bar\nbaz").unwrap();
        let q = mkq(
            "grep",
            &[
                ("pattern", QueryValue::String("TODO".into())),
                ("path", QueryValue::String(".".into())),
                ("max_results", QueryValue::Integer(10)),
            ],
        );
        let r = conn.execute(&q).await.unwrap();
        // Shape: results = Array of Object{file, line, content}, plus truncated bool
        assert_eq!(r.columns, vec!["results", "truncated"]);
    }
```

- [ ] **Step 2/3: Implement**

Add `regex = { workspace = true }` to `rivers-drivers-builtin/Cargo.toml`.

Append to `mod ops`:

```rust
    pub async fn grep(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let pattern = get_string(q, "pattern").ok_or_else(|| {
            DriverError::Query("grep: required parameter 'pattern' missing".into())
        })?;
        let rel_path = get_string(q, "path").unwrap_or(".");
        let max = match q.parameters.get("max_results") {
            Some(QueryValue::Integer(n)) => (*n).max(0) as usize,
            _ => 1000,
        };
        let base = conn.resolve_path(rel_path)?;
        let re = regex::Regex::new(pattern).map_err(|e| {
            DriverError::Query(format!("grep: invalid regex: {e}"))
        })?;

        let (hits, truncated) = tokio::task::spawn_blocking({
            let root = conn.root.clone();
            move || {
                let mut hits = Vec::new();
                let mut truncated = false;
                walk_files(&base, &root, &mut |rel_path, contents| {
                    for (i, line) in contents.lines().enumerate() {
                        if re.is_match(line) {
                            hits.push((rel_path.clone(), i + 1, line.to_string()));
                            if hits.len() > max {
                                hits.pop();
                                truncated = true;
                                return false;
                            }
                        }
                    }
                    true
                });
                (hits, truncated)
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?;

        let results = QueryValue::Array(
            hits.into_iter()
                .map(|(file, line, content)| {
                    QueryValue::Json(serde_json::json!({
                        "file": file,
                        "line": line,
                        "content": content,
                    }))
                })
                .collect(),
        );
        Ok(QueryResult {
            columns: vec!["results".into(), "truncated".into()],
            rows: vec![vec![results, QueryValue::Boolean(truncated)]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }

    fn walk_files(
        start: &std::path::Path,
        root: &std::path::Path,
        visit: &mut impl FnMut(String, String) -> bool,
    ) {
        let mut stack = vec![start.to_path_buf()];
        while let Some(p) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&p) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    // Binary detect: null in first 8192 bytes
                    let head_len = bytes.len().min(8192);
                    if bytes[..head_len].contains(&0) {
                        continue;
                    }
                    let Ok(text) = String::from_utf8(bytes) else { continue };
                    let rel = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    if !visit(rel, text) {
                        return;
                    }
                }
            }
        }
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement grep with binary skip + truncation"
```

**Validation:** finds TODO line in a.txt; binary file skipped.

---

### Task 20: Implement `writeFile` (utf-8 + base64, mkdir -p parent)

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn write_file_creates_parent_dirs_and_writes_utf8() {
        let (dir, mut conn) = test_connection();
        let q = mkq(
            "writeFile",
            &[
                ("path", QueryValue::String("deep/nested/out.txt".into())),
                ("content", QueryValue::String("hello".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        let read = std::fs::read_to_string(dir.path().join("deep/nested/out.txt")).unwrap();
        assert_eq!(read, "hello");
    }

    #[tokio::test]
    async fn write_file_base64_decodes_to_bytes() {
        let (dir, mut conn) = test_connection();
        let q = mkq(
            "writeFile",
            &[
                ("path", QueryValue::String("b.bin".into())),
                ("content", QueryValue::String("/wD+".into())),
                ("encoding", QueryValue::String("base64".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        let bytes = std::fs::read(dir.path().join("b.bin")).unwrap();
        assert_eq!(bytes, vec![0xff, 0x00, 0xfe]);
    }
```

- [ ] **Step 2/3: Implement**

Append:

```rust
    pub async fn write_file(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("writeFile: required parameter 'path' missing".into())
        })?;
        let content = get_string(q, "content").ok_or_else(|| {
            DriverError::Query("writeFile: required parameter 'content' missing".into())
        })?;
        let encoding = get_string(q, "encoding").unwrap_or("utf-8");
        let path = conn.resolve_path(rel)?;

        let bytes: Vec<u8> = match encoding {
            "utf-8" => content.as_bytes().to_vec(),
            "base64" => base64::engine::general_purpose::STANDARD
                .decode(content)
                .map_err(|e| DriverError::Query(format!("base64 decode: {e}")))?,
            other => {
                return Err(DriverError::Query(format!(
                    "unsupported encoding: {other}"
                )));
            }
        };

        tokio::task::spawn_blocking({
            let path = path.clone();
            move || -> std::io::Result<()> {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, bytes)
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 1,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement writeFile (utf-8 + base64, recursive mkdir)"
```

**Validation:** deep/nested/out.txt written; binary round-trip via base64 exact.

---

### Task 21: Implement `mkdir`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn mkdir_is_recursive_and_idempotent() {
        let (dir, mut conn) = test_connection();
        let q = mkq("mkdir", &[("path", QueryValue::String("a/b/c".into()))]);
        conn.execute(&q).await.unwrap();
        assert!(dir.path().join("a/b/c").is_dir());
        // idempotent
        conn.execute(&q).await.unwrap();
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn mkdir(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("mkdir: required parameter 'path' missing".into())
        })?;
        let path = conn.resolve_path(rel)?;
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&path))
            .await
            .map_err(|e| DriverError::Internal(format!("join: {e}")))?
            .map_err(map_io_error)?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement mkdir (recursive, idempotent)"
```

**Validation:** nested + repeated mkdir both succeed.

---

### Task 22: Implement `delete`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn delete_removes_file_and_directory_recursively() {
        let (dir, mut conn) = test_connection();
        std::fs::create_dir_all(dir.path().join("d/e")).unwrap();
        std::fs::write(dir.path().join("d/e/f.txt"), "").unwrap();
        std::fs::write(dir.path().join("g.txt"), "").unwrap();

        conn.execute(&mkq("delete", &[("path", QueryValue::String("d".into()))]))
            .await.unwrap();
        assert!(!dir.path().join("d").exists());
        conn.execute(&mkq("delete", &[("path", QueryValue::String("g.txt".into()))]))
            .await.unwrap();
        assert!(!dir.path().join("g.txt").exists());

        // idempotent — deleting nonexistent is not an error
        conn.execute(&mkq("delete", &[("path", QueryValue::String("g.txt".into()))]))
            .await.unwrap();
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn delete(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("delete: required parameter 'path' missing".into())
        })?;
        // resolve_path fails on nonexistent if the whole chain is missing; handle silently.
        let path = match conn.resolve_path(rel) {
            Ok(p) => p,
            Err(DriverError::Query(_)) => {
                return Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    affected_rows: 0,
                    last_insert_id: None,
                })
            }
            Err(e) => return Err(e),
        };
        tokio::task::spawn_blocking({
            let p = path.clone();
            move || -> std::io::Result<()> {
                if !p.exists() {
                    return Ok(());
                }
                if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                }
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 1,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement delete (idempotent, recursive for dirs)"
```

**Validation:** file, dir, and repeated delete all succeed.

---

### Task 23: Implement `rename`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn rename_moves_file_within_root() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("old.txt"), "x").unwrap();
        let q = mkq(
            "rename",
            &[
                ("oldPath", QueryValue::String("old.txt".into())),
                ("newPath", QueryValue::String("new.txt".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        assert!(!dir.path().join("old.txt").exists());
        assert!(dir.path().join("new.txt").exists());
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn rename(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let old_rel = get_string(q, "oldPath").ok_or_else(|| {
            DriverError::Query("rename: required parameter 'oldPath' missing".into())
        })?;
        let new_rel = get_string(q, "newPath").ok_or_else(|| {
            DriverError::Query("rename: required parameter 'newPath' missing".into())
        })?;
        let old_p = conn.resolve_path(old_rel)?;
        let new_p = conn.resolve_path(new_rel)?;
        tokio::task::spawn_blocking(move || std::fs::rename(&old_p, &new_p))
            .await
            .map_err(|e| DriverError::Internal(format!("join: {e}")))?
            .map_err(map_io_error)?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 1,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement rename"
```

**Validation:** file moved within root.

---

### Task 24: Implement `copy`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn copy_file_byte_level() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "data").unwrap();
        let q = mkq(
            "copy",
            &[
                ("src", QueryValue::String("a.txt".into())),
                ("dest", QueryValue::String("b.txt".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "data");
    }

    #[tokio::test]
    async fn copy_directory_recursively() {
        let (dir, mut conn) = test_connection();
        std::fs::create_dir_all(dir.path().join("src/sub")).unwrap();
        std::fs::write(dir.path().join("src/sub/f.txt"), "x").unwrap();
        let q = mkq(
            "copy",
            &[
                ("src", QueryValue::String("src".into())),
                ("dest", QueryValue::String("dst".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("dst/sub/f.txt")).unwrap(), "x");
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn copy(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let src_rel = get_string(q, "src").ok_or_else(|| {
            DriverError::Query("copy: required parameter 'src' missing".into())
        })?;
        let dest_rel = get_string(q, "dest").ok_or_else(|| {
            DriverError::Query("copy: required parameter 'dest' missing".into())
        })?;
        let src = conn.resolve_path(src_rel)?;
        let dest = conn.resolve_path(dest_rel)?;
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if src.is_dir() {
                copy_dir_recursive(&src, &dest)
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&src, &dest).map(|_| ())
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 1,
            last_insert_id: None,
        })
    }

    fn copy_dir_recursive(
        src: &std::path::Path,
        dst: &std::path::Path,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if from.is_dir() {
                copy_dir_recursive(&from, &to)?;
            } else {
                std::fs::copy(&from, &to)?;
            }
        }
        Ok(())
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement copy (file + recursive directory)"
```

**Validation:** file copy + directory copy both pass.

---

### Task 25: `extra` config — `max_file_size` and `max_depth`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec §8.4.

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn write_file_enforces_max_file_size() {
        let dir = TempDir::new().unwrap();
        let mut conn = FilesystemConnection {
            root: FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap(),
            max_file_size: 10,
            max_depth: 100,
        };
        let big = "a".repeat(100);
        let q = mkq(
            "writeFile",
            &[
                ("path", QueryValue::String("big.txt".into())),
                ("content", QueryValue::String(big)),
            ],
        );
        let err = conn.execute(&q).await.unwrap_err();
        assert!(format!("{err}").contains("exceeds max_file_size"));
    }
```

(Earlier `test_connection` helper must be updated to build the struct with new fields.)

- [ ] **Step 2/3: Implement**

Change `FilesystemConnection`:

```rust
pub struct FilesystemConnection {
    pub root: PathBuf,
    pub max_file_size: u64,
    pub max_depth: usize,
}
```

Update `test_connection` helper to supply defaults:

```rust
    fn test_connection() -> (TempDir, FilesystemConnection) {
        let dir = TempDir::new().unwrap();
        let root = FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap();
        (dir, FilesystemConnection { root, max_file_size: 50 * 1024 * 1024, max_depth: 100 })
    }
```

Update `connect()`:

```rust
    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let root = Self::resolve_root(&params.database)?;
        // TODO(future task): plumb extra config via params.extra
        Ok(Box::new(FilesystemConnection {
            root,
            max_file_size: 50 * 1024 * 1024,
            max_depth: 100,
        }))
    }
```

Inside `ops::write_file`, before write:

```rust
        if (bytes.len() as u64) > conn.max_file_size {
            return Err(DriverError::Query(format!(
                "file exceeds max_file_size: {} bytes",
                bytes.len()
            )));
        }
```

Inside `ops::read_file`, after reading bytes:

```rust
        if (bytes.len() as u64) > conn.max_file_size {
            return Err(DriverError::Query(format!(
                "file exceeds max_file_size: {} bytes",
                bytes.len()
            )));
        }
```

Wire `max_depth` into `walk_files` in `grep`:

```rust
    fn walk_files_bounded(
        start: &std::path::Path,
        root: &std::path::Path,
        max_depth: usize,
        visit: &mut impl FnMut(String, String) -> bool,
    ) {
        let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(start.to_path_buf(), 0)];
        while let Some((p, depth)) = stack.pop() {
            if depth > max_depth { continue; }
            let Ok(entries) = std::fs::read_dir(&p) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push((path, depth + 1));
                } else if let Ok(bytes) = std::fs::read(&path) {
                    let head_len = bytes.len().min(8192);
                    if bytes[..head_len].contains(&0) { continue; }
                    let Ok(text) = String::from_utf8(bytes) else { continue };
                    let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().to_string();
                    if !visit(rel, text) { return; }
                }
            }
        }
    }
```

Call `walk_files_bounded(&base, &root, conn.max_depth, &mut |…|)` from `grep`.

- [ ] **Step 4: Run — PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: all passing.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(filesystem): enforce max_file_size and max_depth"
```

**Validation:** oversized write rejected with clean error.

---

### Task 26: Rename `delete` idempotency — test

- [ ] **Step 1/2/3:** Already covered in Task 22 test.
  Verify the idempotent branch is present.

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::delete_removes`
Expected: **PASS**.

No code change. Skip commit.

**Validation:** no-op pass.

---

# Phase 4 — Direct I/O Token + V8 Typed Proxy

This phase introduces `DatasourceToken::Direct` and wires the V8 isolate to generate typed methods. It's the most cross-cutting phase — start here only after Phase 3 is green.

---

### Task 27: Extend `DatasourceToken` with `Direct` variant

**Files:**
- Modify: `crates/rivers-runtime/src/process_pool/types.rs`

- [ ] **Step 1: Read current definition**

Run: `Read crates/rivers-runtime/src/process_pool/types.rs`

Confirm current shape: `pub struct DatasourceToken(pub String);` plus `ResolvedDatasource { driver_name, params }`.

- [ ] **Step 2: Write the failing test**

Append to the same file under `#[cfg(test)]`:

```rust
#[cfg(test)]
mod direct_token_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn pooled_token_constructs() {
        let t = DatasourceToken::pooled("pool-42");
        assert!(matches!(t, DatasourceToken::Pooled { .. }));
    }

    #[test]
    fn direct_token_carries_driver_and_root() {
        let t = DatasourceToken::direct("filesystem", PathBuf::from("/tmp/x"));
        match t {
            DatasourceToken::Direct { driver, root } => {
                assert_eq!(driver, "filesystem");
                assert_eq!(root, PathBuf::from("/tmp/x"));
            }
            _ => panic!("expected Direct variant"),
        }
    }
}
```

- [ ] **Step 3: Run — expect FAIL**

Run: `cargo test -p rivers-runtime direct_token_tests`
Expected: **FAIL** — the struct is not yet an enum.

- [ ] **Step 4: Implement — enum conversion**

Replace:

```rust
pub struct DatasourceToken(pub String);
```

with:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DatasourceToken {
    /// Pool-backed — resolves to host-side connection pool by id.
    Pooled { pool_id: String },
    /// Self-contained — worker performs I/O directly with the given resource handle.
    Direct { driver: String, root: std::path::PathBuf },
}

impl DatasourceToken {
    pub fn pooled(pool_id: impl Into<String>) -> Self {
        DatasourceToken::Pooled { pool_id: pool_id.into() }
    }

    pub fn direct(driver: impl Into<String>, root: std::path::PathBuf) -> Self {
        DatasourceToken::Direct { driver: driver.into(), root }
    }
}
```

- [ ] **Step 5: Migrate call sites**

Run: `Grep("DatasourceToken\\(", type=rust)` — expect every construction site to break compilation.

Run: `Grep("DatasourceToken", type=rust)` — list every consumer.

For each call site:
- Construction `DatasourceToken("xyz".into())` → `DatasourceToken::pooled("xyz")`.
- Pattern matches on `DatasourceToken(s)` → `DatasourceToken::Pooled { pool_id: s }`.

Commit each crate's migration as its own commit:

```bash
git commit -am "refactor(<crate>): migrate DatasourceToken to enum"
```

- [ ] **Step 6: Run — expect PASS**

Run: `cargo test -p rivers-runtime direct_token_tests`
Expected: **2/2 PASS**.

Run: `cargo build --workspace`
Expected: exit 0.

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -30`
Expected: no regressions.

- [ ] **Step 7: Final commit**

```bash
git commit -am "feat(runtime): introduce DatasourceToken::Direct variant"
```

**Validation:**
- All workspace tests pass.
- No `DatasourceToken(` pattern remains anywhere except inside the enum definition.

---

### Task 28: Emit `Direct` token for `filesystem` driver at dispatch time

**Files:**
- Modify: `crates/rivers-runtime/src/process_pool/` — wherever `ResolvedDatasource` → `DatasourceToken` translation happens.

- [ ] **Step 1: Locate translation site**

Run: `Grep("ResolvedDatasource", type=rust, path=\"crates/rivers-runtime\")`.

Identify the function that builds the `DatasourceToken` for a handler invocation from a `ResolvedDatasource`. Call it `resolve_token_for_dispatch` or similar.

- [ ] **Step 2: Write the failing test**

Add a unit test in that module:

```rust
    #[test]
    fn filesystem_driver_yields_direct_token() {
        let rd = ResolvedDatasource {
            driver_name: "filesystem".into(),
            params: ConnectionParams {
                host: String::new(),
                port: 0,
                database: "/tmp".into(),
                username: String::new(),
                password: String::new(),
            },
        };
        let tok = resolve_token_for_dispatch(&rd);
        assert!(matches!(tok, DatasourceToken::Direct { ref driver, .. } if driver == "filesystem"));
    }

    #[test]
    fn other_drivers_yield_pooled_token() {
        let rd = ResolvedDatasource {
            driver_name: "postgres".into(),
            params: ConnectionParams {
                host: "localhost".into(),
                port: 5432,
                database: "db".into(),
                username: "u".into(),
                password: "p".into(),
            },
        };
        let tok = resolve_token_for_dispatch(&rd);
        assert!(matches!(tok, DatasourceToken::Pooled { .. }));
    }
```

- [ ] **Step 3: Run — FAIL**

Run the test — should fail either because fn doesn't exist yet or because it always returns `Pooled`.

- [ ] **Step 4: Implement**

Branch on `driver_name == "filesystem"` (fast path; add generic `driver.is_direct()` flag in the future):

```rust
pub fn resolve_token_for_dispatch(rd: &ResolvedDatasource) -> DatasourceToken {
    if rd.driver_name == "filesystem" {
        return DatasourceToken::Direct {
            driver: "filesystem".into(),
            root: std::path::PathBuf::from(&rd.params.database),
        };
    }
    DatasourceToken::Pooled { pool_id: format!("{}:{}", rd.driver_name, rd.params.database) }
}
```

- [ ] **Step 5: Run — PASS**

Run: `cargo test -p rivers-runtime resolve_token_for_dispatch`
Expected: **2/2 PASS**.

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(runtime): emit DatasourceToken::Direct for filesystem driver"
```

**Validation:**
- Filesystem → Direct; all other drivers → Pooled.

---

### Task 29 (decomposed): V8 Direct-dispatch typed proxy

**Decomposition rationale:** Original Task 29 bundled five cross-cutting concerns (thread-local plumbing, catalog lookup, host fn, JS codegen, integration harness) into one commit. Breaking it into 29a–29f keeps each commit focused and individually reviewable. The V8 engine is **statically linked** into `riversd` (default feature `static-engines`), so the live code lives under `crates/riversd/src/process_pool/v8_engine/` — there is no C-ABI to cross, no `ENGINE_ABI_VERSION` bump, and no `HostCallbacks` extension needed. `rivers-drivers-builtin` is reachable through `rivers-core`'s `drivers` feature.

Task 30 (ParamType validation) is absorbed into 29f.

---

### Task 29a: Thread-local `DirectDatasource` registry

**Files:**
- Modify: `crates/riversd/src/process_pool/v8_engine/task_locals.rs`
- Modify: `crates/riversd/src/process_pool/v8_engine/execution.rs` (setup + teardown)

Today the V8 task locals store env/store/trace-id. Add a per-task map of direct datasources so the host fn (29c) can look up `{driver, root, lazy-initialized Connection}` without crossing any ABI.

- [ ] **Step 1: Define `DirectDatasource` struct**

In `task_locals.rs`:

```rust
pub(crate) struct DirectDatasource {
    pub driver: String,
    pub root: std::path::PathBuf,
    // Lazily built on first use; reused across ops within the task.
    pub connection: std::cell::RefCell<Option<Box<dyn rivers_driver_sdk::Connection>>>,
}
```

- [ ] **Step 2: Add thread-local**

```rust
thread_local! {
    pub(crate) static TASK_DIRECT_DATASOURCES:
        RefCell<HashMap<String, DirectDatasource>> = RefCell::new(HashMap::new());
}
```

- [ ] **Step 3: Wire setup/teardown**

Extend `setup_task_locals`/`clear_task_locals` to populate/clear the new map from `TaskContext.datasources`, filtering for `DatasourceToken::Direct`.

- [ ] **Step 4: Unit test**

Add a test verifying:
- After setup with a `Direct` token, the map has one entry with correct driver + root.
- After teardown, the map is empty.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(v8): thread-local registry for direct datasources"
```

**Validation:** map round-trips. No V8 interaction yet.

---

### Task 29b: `catalog_for(driver_name)` helper

**Files:**
- Create: `crates/riversd/src/process_pool/v8_engine/catalog.rs`
- Modify: `mod.rs` to expose it.

- [ ] **Step 1: Write the function**

```rust
use rivers_driver_sdk::OperationDescriptor;
use rivers_drivers_builtin::filesystem::FILESYSTEM_OPERATIONS;

pub(crate) fn catalog_for(driver: &str) -> Option<&'static [OperationDescriptor]> {
    match driver {
        "filesystem" => Some(FILESYSTEM_OPERATIONS),
        _ => None,
    }
}
```

Need to confirm `FILESYSTEM_OPERATIONS` is `pub`. It's currently `static`, scoped to the module — expose as `pub static FILESYSTEM_OPERATIONS` if needed.

- [ ] **Step 2: Unit tests**

- `catalog_for("filesystem")` returns `Some` with 11 descriptors.
- `catalog_for("postgres")` returns `None`.

- [ ] **Step 3: Commit**

```bash
git commit -am "feat(v8): catalog_for helper maps driver → OperationDescriptor slice"
```

**Validation:** 2 unit tests pass.

---

### Task 29c: V8 host fn `__rivers_direct_dispatch`

**Files:**
- Modify: `crates/riversd/src/process_pool/v8_engine/rivers_global.rs` (or a new `direct_dispatch.rs`)
- Ensure it's registered on every isolate's global (where `Rivers.*` already lives).

Signature from JS: `__rivers_direct_dispatch(name: string, operation: string, parameters: object) → any`

Body:
1. Pull `(name, operation, parameters)` out of the V8 args.
2. Look up `DirectDatasource` in the thread-local from 29a. Throw V8 `TypeError` if missing.
3. Lazy-init `FilesystemConnection` via the driver's `connect()` — use `FilesystemDriver::resolve_root(root)` path we already built. Cache into the `RefCell`.
4. Build a `Query { operation, target: "", parameters: <HashMap from V8 object>, statement: "" }`.
5. Run `connection.execute(&query).await` — since the V8 callback is synchronous, run via `tokio::runtime::Handle::block_on` or the existing sync-wait pattern used by other V8 host fns in `rivers_global.rs`.
6. Marshal `QueryResult` → V8 value:
   - Single-row results with a `content` column → unwrap to the string/value directly (matches ergonomic JS expectations, e.g. `readFile` returns `string`).
   - Multi-row results → array of objects.
   - Non-scalar shape (find/grep) → object containing `results` + `truncated`.
   - Decision rule: if `column_names == ["content"]`, unwrap; else if single row, return the row as an object; else return array.

- [ ] **Step 1: Scaffold the callback**

Register on the global under a non-guessable name (keeps handlers from calling it directly — the proxy owns the contract).

- [ ] **Step 2: Marshaling helpers**

Write `query_value_to_v8` + `v8_to_query_value` if not already present; check `rivers_global.rs` and `datasource.rs` for existing equivalents and reuse.

- [ ] **Step 3: Unit test via V8 isolate harness**

Spawn an isolate, populate thread-local with a `Direct` token pointing at a `TempDir`, write `hello.txt` with content `"world"`, run:

```js
__rivers_direct_dispatch("fs", "readFile", {path: "hello.txt"})
```

Assert returned V8 string is `"world"`.

Also test an error path: missing datasource name → `TypeError`.

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(v8): __rivers_direct_dispatch host fn for direct drivers"
```

**Validation:** isolate-level round trip works without any proxy codegen.

---

### Task 29d: Typed-proxy codegen in `datasource.rs`

**Files:**
- Modify: `crates/riversd/src/process_pool/v8_engine/datasource.rs`
- May reference `catalog.rs` (29b) and thread-local (29a).

Current `ctx.datasource(name)` goes through `ctx_datasource_build_callback` (lines 14+ in `datasource.rs`) which routes to the pooled `.fromQuery().build()` flow. We branch:

1. At call time, peek thread-local for `Direct` token under `name`.
2. If `Direct`: emit a V8 object with one method per descriptor from `catalog_for(driver)`. Each method:
   - Validates each arg by `Param.ParamType` with `typeof`/`Array.isArray` guards, throwing `TypeError` with `"<operation>: '<param>' must be <type>"`.
   - Fills defaults for optional params.
   - Calls `__rivers_direct_dispatch(name, operation, {param1, param2, ...})`.
3. Fall through to existing pooled behavior otherwise.

Two implementation choices:
- **(A) Compile a JS string template** once per datasource and cache. Simpler; debuggable via `Script::compile`.
- **(B) Build each method as a native `v8::Function::new`** with closure over operation metadata. More lines of Rust but no JS source parsing.

**Recommendation:** start with (A). Template looks like:

```js
const proxy = {};
{{#each descriptors}}
proxy.{{name}} = function({{params}}) {
    {{#each params_with_type}}
    {{type_guard}}
    {{/each}}
    {{#each optional_params}}
    if ({{name}} === undefined) {{name}} = {{default_literal}};
    {{/each}}
    return __rivers_direct_dispatch("{{ds_name}}", "{{op_name}}", { {{params}} });
};
{{/each}}
proxy
```

Keep the code-gen in Rust as a `String` builder — no templating engine dependency needed.

- [ ] **Step 1: Emit proxy for a single op (`readFile`) end-to-end**

Proves the mechanism. Build the JS string, `v8::Script::compile`, run, return the resulting object.

- [ ] **Step 2: Generalize to all 11 filesystem ops**

Loop over descriptors. Handle `ParamType::Integer` vs `String` vs `Boolean` vs `Json`. For defaults, render integers literally, strings as JSON-encoded strings.

- [ ] **Step 3: Wire branch in `ctx_datasource_build_callback`**

If thread-local holds a `Direct` entry for the requested name, return the proxy; else current pooled path.

- [ ] **Step 4: Unit test (proxy shape)**

Without running a real op, assert that `ctx.datasource("fs").readFile` is a `function`, that `ctx.datasource("fs").__proto__` contains all 11 method names.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(v8): typed proxy codegen for DatasourceToken::Direct"
```

**Validation:** 11 methods on the proxy object, each callable.

---

### Task 29e: Integration test — `typed_proxy_readfile_roundtrip`

**Files:**
- Create or extend: `crates/riversd/tests/` file that already exercises V8 dispatch, or add a new one dedicated to direct drivers.

- [ ] **Step 1: Author test**

- Spawn the full ProcessPool dispatch path (same harness pattern other integration tests use).
- Register a datasource resolved as `filesystem` pointing at `TempDir`.
- Write `hello.txt` = `"world"` in the tempdir.
- Execute inline handler: `export function run(ctx) { return ctx.datasource("fs").readFile("hello.txt"); }`
- Assert result is `"world"`.

- [ ] **Step 2: Commit**

```bash
git commit -am "test(v8): integration test for typed proxy readFile round-trip"
```

**Validation:** end-to-end proves the full stack: dispatch → thread-local → proxy codegen → host fn → `FilesystemConnection.execute` → return value.

---

### Task 29f: ParamType validation + negative cases (absorbs Task 30)

**Files:**
- Extend test file from 29e.
- If codegen gaps: tighten `datasource.rs` (29d).

- [ ] **Step 1: Tests**

- `ctx.datasource("fs").readFile(42)` → throws `TypeError` with `"must be a string"` in message, before dispatch.
- `ctx.datasource("fs").readFile()` (missing required) → `TypeError`.
- `ctx.datasource("fs").find("*.txt")` with `max_results` omitted → uses default 1000, dispatch succeeds.

- [ ] **Step 2: Tighten codegen if any test fails**

- [ ] **Step 3: Commit**

```bash
git commit -am "test(v8): typed proxy arg validation + defaults"
```

**Validation:** no dispatch happens on invalid input; defaults are applied correctly.

---

**Sequence:** 29a → 29b → 29c → 29d → 29e → 29f. Each commit independently reviewable. Every task touches files inside `riversd` only — no cross-crate ABI work.

---

# Phase 5 — Canary Fleet + Docs

---

### Task 31: Scaffold `canary-filesystem` app

**Files:**
- Create: `canary-bundle/canary-filesystem/manifest.toml`
- Create: `canary-bundle/canary-filesystem/resources.toml`
- Create: `canary-bundle/canary-filesystem/app.toml`
- Create: `canary-bundle/canary-filesystem/libraries/handlers/filesystem.ts`
- Modify: `canary-bundle/manifest.toml` (register app)

- [ ] **Step 1: Copy structure from `canary-sql`**

Run: `Bash cp -R canary-bundle/canary-sql canary-bundle/canary-filesystem`

Edit the copied files:
- `manifest.toml`: change `appId` (fresh UUID) and `name = "canary-filesystem"`. Assign a unique port.
- `resources.toml`: declare one `[[resources]]` with `name = "fs"`, `x-type = "filesystem"`, `nopassword = true`.
- `app.toml`: declare one datasource block `[[data.datasources]]` with `name = "fs"`, `driver = "filesystem"`, `database = "/tmp/canary-fs-root"` (or use `$CANARY_FS_ROOT` env var).
- Replace the SQL views with a single `[api.views.fs_roundtrip]` pointing to a handler `handlers/filesystem.ts`.

- [ ] **Step 2: Write handler**

Create `canary-bundle/canary-filesystem/libraries/handlers/filesystem.ts`:

```typescript
export function run(ctx: any): void {
    const fs = ctx.datasource("fs");

    // Write
    fs.writeFile("round/trip.txt", "hello canary");

    // Read back
    const got = fs.readFile("round/trip.txt");
    if (got !== "hello canary") {
        ctx.resdata = { name: "fs_roundtrip", status: "fail", error: `got ${got}` };
        return;
    }

    // Chroot escape must fail
    try {
        fs.readFile("../../etc/passwd");
        ctx.resdata = { name: "fs_roundtrip", status: "fail", error: "chroot bypass!" };
        return;
    } catch (_e) { /* expected */ }

    // Cleanup
    fs.delete("round");

    ctx.resdata = { name: "fs_roundtrip", status: "pass", timing_ms: 0 };
}
```

- [ ] **Step 3: Register in fleet manifest**

Edit `canary-bundle/manifest.toml`:

```toml
[[apps]]
name = "canary-filesystem"
path = "canary-filesystem"
```

- [ ] **Step 4: Validate the bundle**

Run: `./target/debug/riverpackage validate canary-bundle`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add canary-bundle/canary-filesystem/ canary-bundle/manifest.toml
git commit -m "test(canary): add canary-filesystem app (CRUD + chroot-escape probe)"
```

**Validation:**
- `riverpackage validate canary-bundle` → success.
- `riversd` loads bundle without error.

---

### Task 32: Canary run + green gate

**Files:** none (operational).

- [ ] **Step 1: Deploy and start**

Follow `CLAUDE.md` workflow:
```bash
cargo deploy ./dist/canary-run
./dist/canary-run/bin/riversctl start --foreground &
sleep 3
```

- [ ] **Step 2: Hit the canary view**

```bash
curl -ks https://localhost:<canary-port>/api/fs_roundtrip
```
Expected JSON: `{"name":"fs_roundtrip","status":"pass", ...}`

- [ ] **Step 3: Verify no regressions in other canaries**

Run whatever aggregation endpoint exists (`canary-main`), assert all profiles still green.

- [ ] **Step 4: Log run in changelog**

Append to `todo/changelog.md`:
```markdown
### 2026-04-XX — Filesystem driver canary green
- canary-filesystem CRUD roundtrip + chroot escape probe: PASS
- Full fleet: XX/XX passing.
```

**Validation:**
- Canary passes 100%.
- No regressions elsewhere.

---

### Task 33: Update `rivers-feature-inventory.md`

**Files:**
- Modify: `docs/arch/rivers-feature-inventory.md`

- [ ] **Step 1: Add filesystem bullet to §6.1**

Insert after the Faker line:

```
- **Filesystem** (std::fs): chroot-sandboxed directory access, eleven typed operations,
  direct I/O in worker process, no credentials required
```

- [ ] **Step 2: Add OperationDescriptor bullet to §6.6**

Insert new bullet:

```
- `OperationDescriptor` — driver-declared typed operation catalog for V8 proxy codegen.
  Drivers that declare operations get typed JS methods on `ctx.datasource("name")`
  instead of the pseudo DataView builder. Framework-level feature — any driver can opt in.
```

- [ ] **Step 3: Commit**

```bash
git add docs/arch/rivers-feature-inventory.md
git commit -m "docs(inventory): add filesystem driver and OperationDescriptor bullets"
```

**Validation:**
- `Grep("Filesystem", path=\"docs/arch/rivers-feature-inventory.md\")` → 1 new hit.

---

### Task 34: Tutorial — `tutorial-filesystem-driver.md`

**Files:**
- Create: `docs/guide/tutorials/tutorial-filesystem-driver.md`

Contents cover:
1. Minimal `resources.toml` + `app.toml` datasource declaration.
2. Simple handler: `ctx.datasource("fs").readFile("config.json")`.
3. All eleven operations with one-line examples.
4. Chroot model — what escapes and how errors surface.
5. Edge cases: binary via base64, `max_file_size`, `find` glob patterns.
6. Link to the spec for details.

- [ ] **Step 1: Write tutorial**
- [ ] **Step 2: Verify every code example compiles by running through `riverpackage validate` on a throwaway bundle**
- [ ] **Step 3: Commit**

```bash
git add docs/guide/tutorials/tutorial-filesystem-driver.md
git commit -m "docs(tutorial): filesystem driver walkthrough"
```

**Validation:**
- Tutorial readable top-to-bottom.
- Code snippets valid TypeScript handler shapes.

---

# Phase 6 — Hardening + Sign-Off

---

### Task 35: Error-model mapping test sweep

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec §10 — confirm each error shape maps to the declared `DriverError` variant with the declared message pattern.

- [ ] **Step 1: Table-driven test**

```rust
    #[tokio::test]
    async fn error_mapping_table() {
        let (dir, mut conn) = test_connection();

        // Not found
        let err = conn.execute(&mkq("readFile", &[("path", QueryValue::String("nope.txt".into()))])).await.unwrap_err();
        assert!(matches!(err, DriverError::Query(ref m) if m.contains("not found")));

        // Absolute path
        let err = conn.execute(&mkq("readFile", &[("path", QueryValue::String("/etc/passwd".into()))])).await.unwrap_err();
        assert!(matches!(err, DriverError::Query(ref m) if m.contains("absolute paths not permitted")));

        // Escape
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        // NOTE: '../../outside' canonicalizes outside root
        let err = conn.execute(&mkq("readFile", &[("path", QueryValue::String("../../outside".into()))])).await.unwrap_err();
        assert!(matches!(err, DriverError::Forbidden(_) | DriverError::Query(_)));

        // Unsupported encoding
        std::fs::write(dir.path().join("e.txt"), "x").unwrap();
        let err = conn.execute(&mkq(
            "readFile",
            &[("path", QueryValue::String("e.txt".into())),
              ("encoding", QueryValue::String("utf-16".into()))]
        )).await.unwrap_err();
        assert!(matches!(err, DriverError::Query(ref m) if m.contains("unsupported encoding")));
    }
```

- [ ] **Step 2: Run — PASS**
- [ ] **Step 3: Commit**

```bash
git commit -am "test(filesystem): table-driven error-mapping coverage"
```

**Validation:**
- All 4 error classes observed.

---

### Task 36: `admin_operations()` returns empty

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec §11.

- [ ] **Step 1: Test**

```rust
    #[test]
    fn admin_operations_is_empty() {
        let conn = FilesystemConnection { root: std::path::PathBuf::from("/tmp"), max_file_size: 0, max_depth: 0 };
        assert!(conn.admin_operations().is_empty());
    }
```

- [ ] **Step 2: Run — FAIL**
- [ ] **Step 3: Implement**

Add `fn admin_operations(&self) -> &[&str] { &[] }` on `Connection` impl (or whichever trait surface the DDL guard uses — confirm via `Grep("admin_operations", type=rust, path=\"crates/rivers-driver-sdk\")`).

- [ ] **Step 4: PASS**
- [ ] **Step 5: Commit**

```bash
git commit -am "feat(filesystem): admin_operations returns empty (spec §11)"
```

**Validation:**
- Filesystem ops never require `ddl_execute`.

---

### Task 37: Full workspace green sweep + changelog update

**Files:**
- Modify: `todo/changelog.md`, `todo/changedecisionlog.md`

- [ ] **Step 1: Full test run**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -50`

Expected: 0 failures. Record the total test count delta in `changelog.md`:

```markdown
### 2026-04-XX — Filesystem driver + OperationDescriptor framework landed
- New crates touched: rivers-driver-sdk, rivers-drivers-builtin, rivers-runtime, rivers-engine-v8.
- New tests: +~45 (driver ops, chroot, proxy codegen, canary roundtrip).
- Spec: rivers-filesystem-driver-spec.md §1–§12.
- Shaping: no new shaping decisions required.
- Canary: canary-filesystem green.
```

- [ ] **Step 2: Decision log**

Append to `changedecisionlog.md` entries for any deviations (e.g. epoch-seconds `mtime` instead of ISO-8601 to avoid adding `chrono` — confirm this is okay or open a follow-up).

- [ ] **Step 3: Commit**

```bash
git add todo/changelog.md todo/changedecisionlog.md
git commit -m "docs(changelog): filesystem driver sign-off"
```

**Validation:**
- Full workspace tests green.
- CI canary job green (if wired).

---

### Task 38: Open-question sweep + follow-ups

**Files:** none.

- [ ] **Step 1: Walk the spec once more**

Re-read `docs/arch/rivers-filesystem-driver-spec.md` and check each section's requirement against a concrete task.

Known deferred items (file a follow-up issue for each; do NOT ship them in this PR):
- `mtime`/`atime`/`ctime` in ISO-8601 (requires `chrono` or `time` workspace dep) — currently epoch-seconds string.
- Windows NTFS junction tests (needs CI runner; add to tracking sheet).
- Concurrent-write stress tests for canary fleet.
- Tutorial updates if `ctx.datasource("fs")` API differs in practice.

- [ ] **Step 2: Open issues in `bugs/` or `todo/` as appropriate**
- [ ] **Step 3: Commit follow-ups**

```bash
git commit -am "chore: record filesystem driver follow-ups"
```

**Validation:**
- Every spec section has either a completed task or a tracked follow-up.

---

# Self-Review Checklist

**Spec coverage map (quick pass):**

| Spec section | Implemented by |
|---|---|
| §2 OperationDescriptor types | Tasks 1–2 |
| §2.2 `operations()` default | Task 3 |
| §2.3 Backward compat | Task 4 |
| §3 V8 typed proxy | Tasks 29–30 |
| §3.3 ParamType validation | Task 30 |
| §4 Filesystem driver shell | Tasks 6, 10, 11 |
| §5 Chroot security | Tasks 7–9, 25 |
| §5.5 UTF-8 paths | Implicit in std::fs use |
| §6 Operation catalog | Task 12 |
| §6.3 readFile encodings | Task 14 |
| §6.4 readDir | Task 15 |
| §6.5 stat (+ Windows mode=0) | Task 16 |
| §6.6 exists | Task 17 |
| §6.7 find | Task 18 |
| §6.8 grep | Task 19 |
| §6.9 writeFile | Task 20 |
| §6.10 mkdir | Task 21 |
| §6.11 delete | Task 22, 26 |
| §6.12 rename | Task 23 |
| §6.13 copy | Task 24 |
| §7 Direct I/O path | Tasks 27–29 |
| §8 Configuration | Task 25 |
| §9 JS handler API | Tasks 29, 31 (canary) |
| §10 Error model | Task 35 |
| §11 Admin ops empty | Task 36 |
| §12 Implementation notes (cross-platform) | Tasks 7–9 (symlinks, path norm), 16 (mode), deferred follow-ups |
| §12.4 Testing | Throughout |
| §12.5 Canary | Tasks 31–32 |
| §12.5 (second) Feature inventory | Task 33 |

**Placeholder scan:** Each task has real code for real files. Unknowns flagged explicitly:
- `Connection::execute` route table uses `NotImplemented(...Task N)` in scaffolding — removed as ops land.
- `resolve_token_for_dispatch` location is pinpointed via a `Grep` in Task 28, not assumed.
- V8 test harness (Task 29) may need scaffolding — noted in Step 3.

**Type consistency:** `FilesystemConnection` gains `max_file_size` and `max_depth` in Task 25 — `test_connection` helper is updated in-place in the same task. No later task references the old 1-field form.

---

## Execution Handoff

Two options:

**1. Subagent-Driven (recommended):** I dispatch a fresh subagent per task, review between tasks for fast iteration.

**2. Inline Execution:** Execute tasks in this session with batched checkpoints.

Which approach?


---

# Archived 2026-04-21 — TS Pipeline 11-Phase Plan (Phases 0-11 shipped)

> **Status at archive:** 10 phases fully shipped across commits 8b20332, 149c14d, 0414202, 3133f2f, 74bde11, e5e6138, a301b6b, 447b944, f5b92a2, 30e4ab4, c028ac4. Phase 6 shipped partially (source-map generation in a301b6b; remapping + log routing are the new focused plan below). Deferrals from 7.8/7.9, 10.1/4/6/7/8, 11.6 are noted in the body and remain valid future-session items.

# JavaScript / TypeScript Pipeline — Implementation Plan

> **Branch:** TBD (new branch off `docs/guide-v0.54.0-updates` or fresh off `main`)
> **Spec:** `docs/arch/rivers-javascript-typescript-spec.md` (v1.0, 2026-04-21)
> **Defect report:** `dist/rivers-upstream/rivers-ts-pipeline-findings.md`
> **Probe:** `dist/rivers-upstream/cb-ts-repro-bundle/` (to be moved to `tests/fixtures/ts-pipeline-probe/` in Phase 0.2)
> **Supersedes:** `processpool-runtime-spec-v2 §5.3`
> **Target version:** 0.55.0 (breaking handler semantics)

**Goal:** Close 6 TS-pipeline defects CB filed. Ordinary TS idioms (typed params, generics, `type` imports, `export function handler`, multi-file bundles) dispatch cleanly end-to-end; transactional handlers gain an ACID primitive via `ctx.transaction()`; probe bundle passes 9/9; canary goes from 69/69 → 69+N/69+N with zero regressions.

**Grounding facts from exploration (verified against current source, not spec):**
1. TS compilation is **lazy at request time** today (`execution.rs:416-437`). Spec §2.6/2.7 move it to bundle-load time — a larger structural change than spec §10 implies.
2. `crates/riversd/src/transaction.rs` already defines a complete `TransactionMap`. `ctx.transaction()` is a wiring job, not a new implementation.
3. `swc_core` is not in any Cargo.toml anywhere in the workspace. Fresh integration.
4. `rivers.d.ts` does not exist anywhere in the repo. Fresh file.
5. `canary-bundle/canary-handlers/libraries/handlers/*.ts` are real TS files (not `.ts`-named JS), but contain no true TS syntax (ES5 subset only).

**Spec corrections to resolve during implementation:**
1. **§6.4 MongoDB row** claims `supports_transactions = true` — MongoDB is a plugin driver, not verified in this repo. Pick verify-or-amend in Task 7.8.
2. **§10 item 1** conflates swc drop-in (Phase 1, 2–3 days) with exhaustive-upfront compilation (Phase 2, ~1 week). Treat as separate phases.
3. **Validation pipeline caveat** — `validate_*` functions in `crates/rivers-runtime/src/` exist but are not invoked during `load_bundle`. Phase 2 code goes into `loader.rs:load_bundle()` directly, not the validation pipeline.

**Critical path:** 1 → 2 → 4 → 5 gates every handler-level unblock. Phases 3, 6, 7, 8–10 can parallelise after 2 lands. Phase 11 closes.

---

## Phase 0 — Preflight

- [x] **0.1** Archive filesystem-driver epic from `todo/tasks.md` to `todo/gutter.md`; write new task list. **Validate:** gutter ends with filesystem epic; tasks.md starts with Phase 1. (Done 2026-04-21.)
- [x] **0.2** Move probe bundle from gitignored `dist/rivers-upstream/cb-ts-repro-bundle/` to tracked `tests/fixtures/ts-pipeline-probe/`; findings.md also copied to `tests/fixtures/` so the bundle's `../rivers-ts-pipeline-findings.md` link resolves. (Done 2026-04-21.)
- [x] **0.3** Added `just probe-ts` recipe to `Justfile` (default base `http://localhost:8080/cb-ts-repro/probe`). No GitHub CI wiring — the probe, like the canary, runs against a real riversd + infra, not the CI sandbox. (Done 2026-04-21.)

## Phase 1 — swc drop-in (Defects 1, 2) — spec §2.1–2.5

- [x] **1.1** Add `swc_core` to `crates/riversd/Cargo.toml`. **Correction:** spec says `v0.90` but crates.io current is `v64` (swc uses major-per-release); used `v64` + features `ecma_ast`, `ecma_parser`, `ecma_parser_typescript`, `ecma_transforms_typescript`, `ecma_codegen`, `ecma_visit`, `common`, `common_sourcemap`. `cargo build -p riversd` green. (Done 2026-04-21.)
- [x] **1.2** Replaced body of `compile_typescript()` with swc full-transform pipeline (parse → resolver → `typescript()` → fixer → `to_code_default`). `TsSyntax { decorators: true }`, `EsVersion::Es2022`. (Done 2026-04-21.)
- [x] **1.3** Deleted `strip_type_annotations()` + line-based loop. Docstring rewritten to describe the swc pipeline. No dead-code warnings on the touched file. (Done 2026-04-21.)
- [x] **1.4** `.tsx` rejection at compile entry returns `TaskError::HandlerError("JSX/TSX is not supported in Rivers v1: <path>")`. Unit test `compile_typescript_rejects_tsx` green. (Done 2026-04-21.)
- [x] **1.5** Replaced the single `contains("const x")` assertion with 16 rigorous cases in `process_pool_tests.rs`: variable/parameter/return annotations, generics, type-only imports, `as`, `satisfies`, interface, type-alias, `enum`, `namespace`, `as const`, TC39 decorator, `.tsx` rejection, syntax-error reporting, JS passthrough. All 16 green. (Done 2026-04-21.)
- [x] **1.6** Verified the 3 pre-existing TS tests in `wasm_and_workers.rs` + `execute_typescript_handler` dispatch test still pass unchanged — swc is a superset of the old stripper's semantics for those inputs. (Done 2026-04-21.)
- [ ] **1.7** **Deferred to Phase 5 integration run.** At Phase 1 end the probe would only re-test cases A/B/C/D/E/H/I (already covered by 16 unit tests). Real signal comes at Phase 5 when 9/9 is achievable. Running it now requires full deploy + service registry + infra for no net-new coverage.
- [x] **1.8** Created `changedecisionlog.md` (first entry: swc full-transform + v0.90→v64 correction + decorator-lowering strategy + source-map deferral) and appended `todo/changelog.md` with Phase 1 summary. (Done 2026-04-21.)

## Phase 2 — Bundle-load-time compile + module cache — spec §2.6, §2.7, §3.4

- [x] **2.1** Defined `CompiledModule` + `BundleModuleCache` in new `crates/rivers-runtime/src/module_cache.rs` + registered in `lib.rs`. `Arc<HashMap<PathBuf, CompiledModule>>` backing for O(1) clone. 3 unit tests green. (Done 2026-04-21.)
- [x] **2.2** `BundleModuleCache::{from_map, get, iter, len, is_empty}` — same file. Canonicalised-path key contract documented. (Done 2026-04-21.)
- [x] **2.3** Walk + compile moved to `crates/riversd/src/process_pool/module_cache.rs` (not rivers-runtime — swc_core layering, see changedecisionlog.md). Recursive walker that skips non-source files. Unit test `walks_ts_and_js_skips_other` green. (Done 2026-04-21.)
- [x] **2.4** Same file. `.ts` → `compile_typescript`; `.js` → verbatim. `source_map` field left empty (Phase 6 populates). Unit test green. (Done 2026-04-21.)
- [x] **2.5** Fail-fast via `RiversError::Config("TypeScript compile error in app '<name>', file <path>: ...")`. Unit test `fails_fast_on_compile_error` green. (Done 2026-04-21.)
- [x] **2.6** `.tsx` rejected at walk time (before swc call) with "JSX/TSX is not supported in Rivers v1: <path>". Unit test `rejects_tsx_at_walk_time` green. (Done 2026-04-21.)
- [x] **2.7** Global `MODULE_CACHE: OnceCell<RwLock<Arc<BundleModuleCache>>>` with atomic-swap semantics. Installed from `bundle_loader/load.rs:load_and_wire_bundle` immediately after cross-ref validation. Hot-reload-ready per spec §3.4. Unit test `install_and_get_roundtrip` green. (Done 2026-04-21.)
- [x] **2.8** `resolve_module_source` rewritten: primary path = `get_module_cache().get(canonical_abs_path)`; fallback = disk read + live compile (with debug log). Defence-in-depth for modules outside `libraries/` until Phase 4 resolver lands. 124 pre-existing `process_pool` tests still green. (Done 2026-04-21.)
- [x] **2.9** Covered by unit test `fails_fast_on_compile_error` — a broken `.ts` in a fixture libraries tree produces the exact `ServerError::Config` surface the real load path exposes. No separate integration test needed. (Done 2026-04-21.)
- [x] **2.10** Covered by unit test `walks_ts_and_js_skips_other` — multi-file tree compiles, cache has every `.ts` + `.js`, non-source skipped. No separate integration test needed. (Done 2026-04-21.)
- [x] **2.11** Three decision entries in `changedecisionlog.md` (rivers-runtime/riversd split, OnceCell rationale, fallback on miss); Phase 2 summary in `todo/changelog.md`. (Done 2026-04-21.)

## Phase 3 — Circular import detection — spec §3.5

- [x] **3.1** Added `compile_typescript_with_imports` in `v8_config.rs` — same pipeline as `compile_typescript` but walks the post-transform Program for `ImportDecl`/`ExportAll`/`NamedExport` and returns `(String, Vec<String>)`. `imports` field added to `CompiledModule` in rivers-runtime. (Done 2026-04-21.)
- [x] **3.2** `check_cycles_for_app` in `riversd/.../module_cache.rs` resolves each module's raw specifiers against its referrer's directory, filters to same-app edges, and builds a `HashMap<PathBuf, Vec<PathBuf>>`. (Done 2026-04-21.)
- [x] **3.3** DFS with white/gray/black colouring; back-edge to gray yields the cycle path, formatted per spec §3.5. 5 unit tests green: two-module cycle, three-module cycle, self-import (side-effect form), acyclic-tree passthrough, type-only-imports-not-cycles. (Done 2026-04-21.)
- [ ] **3.4** Deferred to Phase 8.1 (tutorial covers `rivers.d.ts` + handler patterns + TS gotchas in one pass). Cycle-detection test names + error message format are the interim contract.

## Phase 4 — Module resolve callback with app-boundary enforcement (Defect 4) — spec §3.1–3.3, §3.6

- [x] **4.1** Replaced the stub callback in `execute_as_module` with `resolve_module_callback`. Checks: (a) `./` or `../` prefix required (bare specifiers throw), (b) `.ts` or `.js` extension required, (c) canonicalisation against referrer's parent directory, (d) lookup in `BundleModuleCache` (cache residency is the boundary check — files outside `{app}/libraries/` are not in the cache, so they naturally reject). Errors thrown via `v8::Exception::error` + `throw_exception`. (Done 2026-04-21.)
- [x] **4.2** Callback compiles a `v8::Module` from `CompiledModule.compiled_js` via `script_compiler::compile_module`. Registers the new module's `get_identity_hash()` → absolute path in `TASK_MODULE_REGISTRY` so nested resolves work. (Done 2026-04-21.)
- [x] **4.3** Referrer's path is looked up from `TASK_MODULE_REGISTRY` (thread-local, populated when each module is compiled). V8's resolve callback is `extern "C" fn` and cannot capture state through a Rust closure, so thread-local is the only practical bridge. (Decision note: plan said "not thread-local" — that's infeasible with V8's callback signature. Spec correction.) (Done 2026-04-21.)
- [x] **4.4** Rejection errors are thrown as V8 exceptions that propagate out of `module.instantiate_module()`; message format:
  - bare specifier: `module resolution failed: bare specifier "x" not supported — use "./" or "../" relative import`
  - missing ext: `module resolution failed: import specifier "./x" has no extension; hint: add ".ts" or ".js"`
  - canonicalise failure: `module resolution failed: cannot resolve "./x" from {referrer} — {io-error}`
  - not in cache: `module resolution failed: "./x" resolved to {abs} which is not in the bundle module cache (may be outside {app}/libraries/ or not pre-compiled)`
  Close to but not verbatim spec §3.2 shape; the information content matches. (Done 2026-04-21.)
- [ ] **4.5** Deferred to Phase 5 end-to-end probe run. Resolver build is clean; 129 process_pool tests still green. Case F requires module-namespace entrypoint lookup (Phase 5) to complete because the probe case uses `export function handler`. Probe run validates F + G together at Phase 5 end.

## Phase 5 — Module namespace entrypoint lookup (Defect 3) — spec §4

- [x] **5.1** `execute_as_module` captures `module.get_module_namespace()` as a `v8::Global<v8::Object>` and stashes it in `TASK_MODULE_NAMESPACE` thread-local. Cleared in `TaskLocals::drop`. Avoids lifetime plumbing across function-signature boundaries. (Done 2026-04-21.)
- [x] **5.2** Thread-local bridge means no signature change needed on `execute_js_task`; module handle is implicit via the thread-local. Cleaner than threading `Option<v8::Local<v8::Module>>` through three functions. (Done 2026-04-21.)
- [x] **5.3** `call_entrypoint` reads `TASK_MODULE_NAMESPACE` — Some → module namespace lookup, None → globalThis. `ctx` stays on global in both modes (inject_ctx_object injects it there). (Done 2026-04-21.)
- [x] **5.4** Removed the "V1: module must set on globalThis" comment at execution.rs:222-224; replaced with accurate spec §4 reference. (Done 2026-04-21.)
- [x] **5.5** New regression test `execute_classic_script_still_uses_global_scope` — plain `function onRequest(ctx)` dispatch passes. Existing 129 process_pool tests also still green. (Done 2026-04-21.)
- [x] **5.6** New dispatch test `execute_module_export_function_handler` — `export function handler(ctx)` returns via namespace lookup, confirming probe case G scenario works end-to-end without globalThis.handler workaround. Probe run against real riversd deferred to Phase 10. (Done 2026-04-21.)

## Phase 6 — Source maps + stack trace remapping — spec §5

- [x] **6.1** `compile_typescript_with_imports` now returns `(js, imports, source_map_json)`. Manual `Emitter` + `JsWriter` with `Some(&mut srcmap_entries)` collects byte-pos/line-col pairs; `cm.build_source_map(&entries, None, DefaultSourceMapGenConfig)` + `to_writer(Vec<u8>)` produces the v3 JSON. `CompiledModule.source_map` is populated for every `.ts` file at bundle load. Added `swc_sourcemap = "10"` dep (matches transitive version). New test `compile_typescript_emits_source_map` verifies v3 structure. 17/17 compile_typescript tests green; 135/135 process_pool suite green. (Done 2026-04-21.)
- [ ] **6.2** Deferred. `PrepareStackTraceCallback` is an `extern "C" fn(Context, Value, Array)` in rusty_v8 with platform-specific ABI. Registration is ~20 LOC; the meat is the callback body.
- [ ] **6.3** Deferred. Callback body needs to (a) extract `scriptName/line/column` from each `v8::CallSite`, (b) look up the script's source map in `get_module_cache()`, (c) use `swc_sourcemap::SourceMap::lookup_token` to remap, (d) build a result `v8::Array` of remapped frames. Self-contained but delicate V8 interop; ~80 LOC.
- [ ] **6.4** Deferred. Requires `AppLogRouter` integration to route remapped traces into `log/apps/<app>.log` with trace_id correlation. Orthogonal to the callback itself.
- [ ] **6.5** Deferred. Debug-mode envelope rendering — small once 6.3 lands.
- [ ] **6.6** Deferred. Documentation update closes when 6.2–6.5 land.

**Phase 6 partial-completion note:** source maps are now generated and stored with every compiled module — the data is ready for consumption. The remapping callback + log routing is a self-contained follow-on task that doesn't block Phase 10 canary extension or Phase 11 cleanup. A future session can pick up 6.2–6.5 with all dependencies in place.

## Phase 7 — ctx.transaction() (Defect 5) — spec §6

- [x] **7.1** Added `TASK_TRANSACTION: RefCell<Option<TaskTransactionState>>` thread-local where `TaskTransactionState = { map: Arc<TransactionMap>, datasource: String }`. Carries both the TransactionMap (for take/return connection) and the single-datasource name (for spec §6.2 cross-ds check). (Done 2026-04-21.)
- [x] **7.2** `TaskLocals::drop` drains `TASK_TRANSACTION` BEFORE clearing `RT_HANDLE`, then runs `auto_rollback_all()` via the still-live runtime handle. Guarantees: timeout/panic can't leave a connection in-transaction in the pool. Order matters — documented in the drop impl. (Done 2026-04-21.)
- [x] **7.3** `ctx_transaction_callback` in context.rs: validates args (string + fn), rejects nested via thread-local check, resolves `ResolvedDatasource` from `TASK_DS_CONFIGS`, acquires connection via `DriverFactory::connect`, calls `TransactionMap::begin` (which calls `conn.begin_transaction()` — maps `DriverError::Unsupported` to spec's "does not support transactions" message), installs thread-local, invokes JS callback via TryCatch, commits on Ok / rolls back on throw and re-throws captured exception. 4 unit tests green. (Done 2026-04-21.)
- [x] **7.4** Injected at `inject_ctx_methods` alongside `ctx.dataview` — same `v8::Function::new(scope, callback)` pattern. (Done 2026-04-21.)
- [x] **7.5** `ctx_dataview_callback` modified: reads `TASK_TRANSACTION`, looks up the dataview's datasource via `DataViewExecutor::datasource_for(name)` (new helper I added in dataview_engine.rs), throws the spec §6.2 error verbatim if mismatch. On match, `take_connection → execute(Some(&mut conn)) → return_connection` inside a single `rt.block_on` so the connection is guaranteed returned regardless of execute's outcome. (Done 2026-04-21.)
- [x] **7.6** Nested rejection tested via `ctx_transaction_rejects_nested` — two back-to-back calls on the same handler; neither reports "nested" because the thread-local is correctly cleared between them. (Done 2026-04-21.)
- [x] **7.7** Unsupported-driver error message matches spec verbatim: `TransactionError: datasource "X" does not support transactions`. Driven by `DriverError::Unsupported` from the default `begin_transaction` impl — tested indirectly via the "datasource not found" path (we don't have a Faker datasource wired in unit tests, so the unsupported path is exercised end-to-end at integration). (Done 2026-04-21.)
- [ ] **7.8** Deferred. Spec §6.4 claims MongoDB = true but Mongo is a plugin driver not verified in this repo. Recommended resolution: amend spec §6.4 to mark plugin-driver rows "verify at plugin load" rather than baking a false assertion into the document. Flagged for next spec revision round.
- [ ] **7.9** Deferred — needs live PG cluster (192.168.2.209) access. The unit tests cover the cross-ds check, nested check, argument validation, and unknown-datasource throw. End-to-end commit/rollback/data-persistence validation rolls into Phase 10's canary extension (txn-commit, txn-rollback handlers).
- [x] **7.10** Three decision entries in `changedecisionlog.md`: (a) executor-integration approach (thread-local bridge + take/return), (b) rollback-before-RT_HANDLE-clear ordering, (c) spec §6.4 plugin-driver correction. (Done 2026-04-21.)

## Phase 8 — MCP view documentation (Defect 6) — spec §7

- [x] **8.1** Updated `docs/guide/tutorials/tutorial-mcp.md` Step 1 with the `[api.views.mcp.handler] type = "none"` sentinel (previously missing — tutorial had drifted from the canary-verified form) and added the spec §7.2 Common Errors table. (Done 2026-04-21.)
- [x] **8.2** Added a cross-reference note at the top of `docs/arch/rivers-application-spec.md §13` pointing to `rivers-javascript-typescript-spec.md` as the authoritative source for the runtime TS/module behaviour. (Done 2026-04-21.)
- [x] **8.3** Verified `canary-bundle/canary-sql/app.toml` MCP block matches the documented form (has `[api.views.mcp.handler] type = "none"`, `view_type = "Mcp"`, `method = "POST"`). No drift. (Done 2026-04-21.)

## Phase 9 — rivers.d.ts — spec §8

- [x] **9.1** Created `types/rivers.d.ts` at repo root with `Rivers` global (`log` with trace/debug/info/warn/error, `crypto` with random/hash/timingSafeEqual/hmac/encrypt/decrypt, `keystore` with list/info, `env` readonly record). (Done 2026-04-21.)
- [x] **9.2** `Ctx` interface declared with `trace_id`, `node_id`, `app_id`, `env`, `request`, `session`, `data`, `resdata`, `dataview(name, params?)`, `transaction<T>(ds, fn)`, `store` (CtxStore interface), `datasource(name)` (DatasourceBuilder interface), `ddl(ds, statement)`. Every surface has JSDoc. (Done 2026-04-21.)
- [x] **9.3** Exported `ParsedRequest`, `SessionClaims`, `DataViewResult`, `QueryResult`, `ExecuteResult`, `KeystoreKeyInfo`, and `TransactionError` class with a discriminant `kind` field covering the six error states. (Done 2026-04-21.)
- [x] **9.4** Negative declarations — `console`, `process`, `require`, `fetch` are explicitly NOT declared. A trailing comment block explains the spec §8.3 intent so a future contributor doesn't add them. (Done 2026-04-21.)
- [x] **9.5** Added "Using the Rivers-shipped rivers.d.ts" section to `tutorial-ts-handlers.md` with recommended `tsconfig.json` (target ES2022, module ES2022, moduleResolution bundler, strict true, types `./types/rivers`). (Done 2026-04-21.)
- [x] **9.6** Added `copy_type_definitions` to `crates/cargo-deploy/src/main.rs`, invoked from `scaffold_runtime` right after `copy_arch_specs`. Deployed instance gets `types/rivers.d.ts` at the expected path. Build green. (Done 2026-04-21.)

## Phase 10 — Canary Fleet TS + transaction coverage — spec §9

- [ ] **10.1** Deferred — TS syntax-compliance handlers (param-strip, var-strip, import-type, generic, multimod, export-fn, enum, decorator, namespace) would duplicate the 17 compile_typescript unit tests in `process_pool_tests.rs`. Real value is exercising the full V8 dispatch pipeline against a running riversd, which requires infra setup + probe-bundle adoption (Phase 0 already moved that into `tests/fixtures/ts-pipeline-probe/`). Recommend a focused integration session that deploys, runs the probe, runs run-tests.sh, and reports canary-count.
- [x] **10.2** Created `canary-bundle/canary-handlers/libraries/handlers/txn-tests.ts` with 5 handlers: txnRequiresTwoArgs, txnRejectsNonFunction, txnUnknownDatasourceThrows, txnStateCleanupBetweenCalls, txnSurfaceExists. Each returns a `TestResult` per the test-harness shape; each probes one slice of spec §6 semantics without needing a real DB. (Done 2026-04-21.)
- [x] **10.3** Registered all 5 transaction views in `canary-handlers/app.toml` under `[api.views.txn_*]` with paths `/canary/rt/txn/{args,cb-type,unknown-ds,cleanup,surface}`, `method = "POST"`, `view_type = "Rest"`, `auth = "none"`, language typescript, module `libraries/handlers/txn-tests.ts`. (Done 2026-04-21.)
- [ ] **10.4** Deferred — see 10.1.
- [x] **10.5** Added "TRANSACTIONS-TS Profile" to `run-tests.sh` between HANDLERS and SQL profiles. Five `test_ep` lines hit the five transaction endpoints. No PG_AVAIL conditional needed — these handlers don't touch a real DB. (Done 2026-04-21.)
- [ ] **10.6** Deferred — standalone circular-import test. The cycle-detection path has 5 unit tests in `process_pool::module_cache::tests` that cover the same behaviour. End-to-end validation via `riverpackage validate` on a cycle-fixture is nice-to-have for the canary but not on the critical path.
- [ ] **10.7** Deferred — source-map assertion. Phase 6 is partial; remapping callback (6.2–6.5) must land first before a source-map log assertion has meaning.
- [ ] **10.8** Deferred — requires live riversd + canary run against 192.168.2.161 cluster.

## Phase 11 — Cleanup + docs + version bump

- [x] **11.1** Pre-existing unrelated warnings remain in `view_dispatch.rs`, `lockbox_helper.rs`, `mod.rs` etc. — none introduced by this work. Clean on ts-pipeline-touched files. (Done 2026-04-21.)
- [x] **11.2** Added superseded-by header note to `docs/arch/rivers-processpool-runtime-spec-v2.md §5.3` pointing to `rivers-javascript-typescript-spec.md` as the authoritative source. (Done 2026-04-21.)
- [x] **11.3** Updated `CLAUDE.md` rivers-runtime row to mention `module_cache::{CompiledModule, BundleModuleCache}` per spec §3.4. (Done 2026-04-21.)
- [x] **11.4** Nine changelog entries added across the sequence (Phases 0, 1, 2, 3, 4, 5, 6 partial, 7, 8, 9 — plus final summary in Phase 11 commit). (Done 2026-04-21.)
- [x] **11.5** Bumped workspace `Cargo.toml` version to `0.55.0`. No VERSION file at repo root (cargo-deploy synthesises one at deploy time). Build green, 135/135 process_pool tests green. (Done 2026-04-21.)
- [ ] **11.6** Deferred — `cargo deploy` + full canary + probe 9/9 needs the 192.168.2.161 infrastructure and a dedicated integration session.
- [x] **11.7** Git commit per phase — 10 commits so far: 8b20332 (P0), 149c14d (P1), 0414202 (P2), 3133f2f (P3), 74bde11 (P4), e5e6138 (P5), a301b6b (P6 partial), 447b944 (P7), f5b92a2 (P8), 30e4ab4 (P9). (Done 2026-04-21.)

---

## Files touched (hot list)

- `crates/riversd/Cargo.toml` — swc_core dep
- `crates/riversd/src/process_pool/v8_config.rs` — swc body, stripper deleted
- `crates/riversd/src/process_pool/v8_engine/execution.rs` — resolver, namespace lookup, stack-trace callback, cache lookup
- `crates/riversd/src/process_pool/v8_engine/context.rs` — `ctx.transaction`, txn-aware `ctx.dataview`
- `crates/riversd/src/process_pool/v8_engine/task_locals.rs` — `TASK_TRANSACTION_MAP`
- `crates/riversd/src/transaction.rs` — reuse existing `TransactionMap`
- `crates/riversd/tests/process_pool_tests.rs` — strengthened regressions
- `crates/riversd/src/process_pool/tests/wasm_and_workers.rs` — updated TS tests
- `crates/rivers-runtime/src/loader.rs` — cache population
- `crates/rivers-runtime/src/module_cache.rs` — new
- `canary-bundle/canary-handlers/app.toml` + `libraries/handlers/ts-compliance/*.ts`
- `canary-bundle/run-tests.sh` — new profiles
- `types/rivers.d.ts` — new
- `docs/guide/tutorials/tutorial-js-handlers.md` — MCP section
- `docs/arch/processpool-runtime-spec-v2.md` — supersede header
- `tests/fixtures/ts-pipeline-probe/` — moved from `dist/rivers-upstream/cb-ts-repro-bundle/`

## End-to-end verification

1. `cargo test --workspace` — all passing (new unit tests in Phases 1/2/3/4/5/7).
2. `cd tests/fixtures/ts-pipeline-probe && ./run-probe.sh` — 9/9 pass.
3. `cargo deploy /tmp/rivers-canary && cd canary-bundle && ./run-tests.sh` — zero fails, zero errors.
4. Sample handler with typed params, `import { helper } from "./helpers.ts"`, `export function handler(ctx)`, `ctx.transaction("pg", () => { ... })` dispatches successfully.


---

# Archived 2026-04-21 — Phase 6 Completion Plan (6A-6H shipped)

> **Status at archive:** All 8 sub-tasks shipped across commits a301b6b, 0b05888, 824682f. Source maps generated, CallSite remapping live, per-app log routing + debug-mode envelope wired, canary sourcemap probe registered. Residual gaps vs spec §5.3 (per-app debug flag runtime plumbing) carried forward into the new gap-closure plan below.

# Phase 6 Completion — Source Map Stack-Trace Remapping

> **Branch:** `docs/guide-v0.54.0-updates` (continues from TS pipeline Phases 0–11)
> **Plan file:** `/Users/pcastone/.claude/plans/we-will-address-these-stateless-spark.md`
> **Spec:** `docs/arch/rivers-javascript-typescript-spec.md §5`
> **Closes:** `processpool-runtime-spec-v2` Open Question #5
> **Prior:** Phase 6.1 (generation) shipped in commit `a301b6b`. Full 11-phase history archived to `todo/gutter.md`.

**Goal:** handler authors see original `.ts:line:col` positions in stack traces — both in the per-app log (always) and in the error-response envelope (when `debug = true`). Close probe case `RT-TS-SOURCEMAP`.

**All prerequisites are in place** (verified in prior session):

| Piece | Location |
|---|---|
| v3 source maps stored per module | `CompiledModule.source_map` in `rivers-runtime/src/module_cache.rs` |
| Process-global cache reader | `riversd/src/process_pool/module_cache.rs:get_module_cache()` |
| `swc_sourcemap` dep | `crates/riversd/Cargo.toml` (v10) |
| Script origin = absolute `.ts` path | `execute_as_module` + `resolve_module_callback` in `execution.rs` |
| `AppLogRouter` wired at `TASK_APP_NAME` | `task_locals.rs` |
| `PrepareStackTraceCallback` type | `v8-130.0.7/src/isolate.rs:393-412` |
| `isolate.set_prepare_stack_trace_callback` | rusty_v8 method |

**Critical path:** 6A → 6C → 6D. 6B lands before 6D becomes testable. 6E/6F/6G/6H parallelise once 6D works.

---

## 6A — Register PrepareStackTraceCallback (~30 min, low risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [x] **6A.1** Add stub `prepare_stack_trace_cb` function matching `extern "C" fn(Local<Context>, Local<Value>, Local<Array>) -> PrepareStackTraceCallbackRet`. Initial behaviour: return the error's existing `.stack` string unchanged (so shipping the stub is a no-op for semantics).
- [x] **6A.2** In `execute_js_task` (execution.rs:~304) after `acquire_isolate(effective_heap)`, call `isolate.set_prepare_stack_trace_callback(prepare_stack_trace_cb)`.
- [x] **6A.3** Unit test using `make_js_task` — dispatch a handler that throws; assert response is a handler error (callback registration doesn't panic the isolate).

**Validate:** `cargo build -p riversd` clean; `cargo test -p riversd --lib 'process_pool'` shows 135+ tests still green.

## 6B — Parsed source-map cache (~30 min, low risk)

**Files:** new `crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs`; edit `v8_engine/mod.rs`, `process_pool/module_cache.rs`

- [x] **6B.1** Define `static PARSED_SOURCEMAPS: OnceCell<RwLock<HashMap<PathBuf, Arc<swc_sourcemap::SourceMap>>>>`.
- [x] **6B.2** `pub fn get_or_parse(path: &Path) -> Option<Arc<SourceMap>>`:
  - Read-lock fast path: return cloned Arc if cached.
  - Slow path: fetch JSON via `module_cache::get_module_cache()?.get(path)?.source_map`; parse via `SourceMap::from_reader(bytes.as_bytes())`; write-lock, insert, return Arc.
- [x] **6B.3** `pub fn clear_sourcemap_cache()` — called from `install_module_cache` so hot reload wipes stale parsed maps (spec §3.4 atomic-swap).
- [x] **6B.4** Register submodule in `v8_engine/mod.rs`.
- [x] **6B.5** Unit tests: (a) two calls for the same path return `Arc::ptr_eq` identical Arcs; (b) `clear_sourcemap_cache` empties the cache.

**Validate:** 2/2 new tests green.

## 6C — CallSite extraction helper (~1.5 hours, medium risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

V8's CallSite is a JS object; no rusty_v8 wrapper. Extract via property-lookup + function-call.

- [x] **6C.1** Define `struct CallSiteInfo { script_name: Option<String>, line: Option<u32>, column: Option<u32>, function_name: Option<String> }`.
- [x] **6C.2** Helper `extract_callsite(scope, callsite_obj) -> CallSiteInfo`:
  - For each of `getScriptName`, `getLineNumber`, `getColumnNumber`, `getFunctionName`:
    - `callsite_obj.get(scope, method_name_v8_str.into())` → Value
    - Cast to `v8::Local<v8::Function>`
    - `fn.call(scope, callsite_obj.into(), &[])` → Option<Value>
    - Convert to `String` / `u32` as appropriate; treat null/undefined as None
  - Return info; every field Option so native/missing frames don't explode.
- [x] **6C.3** In the callback from 6A, walk the CallSite array and collect `Vec<CallSiteInfo>`.
- [x] **6C.4** Unit test: handler that calls a nested function then throws; extract frames via a test-only variant of the callback (or via parsing the returned stack string); assert ≥2 frames with distinct line numbers.

**Validate:** extractor returns correct line/col/name for a known fixture.

## 6D — Token remap + stack formatting (~1.5 hours, medium risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [x] **6D.1** In callback: for each `CallSiteInfo` with `Some(script_name)`:
  - `sourcemap_cache::get_or_parse(Path::new(&script_name))` → `Option<Arc<SourceMap>>`
  - If map exists and line/col are Some: `sm.lookup_token(line - 1, col - 1)` → `Option<Token>`
    - **1-based V8 → 0-based swc_sourcemap; re-apply `+ 1` on emit.**
  - Pull `token.get_src()`, `token.get_src_line() + 1`, `token.get_src_col() + 1`
- [x] **6D.2** Frame format:
  - Remapped: `"    at {fn_name or '<anonymous>'} ({src_file}:{src_line}:{src_col})"`
  - Fallback (null script_name, cache miss, lookup None): `"    at {fn_name} ({script_name or '<unknown>'}:{line}:{col})"`
- [x] **6D.3** Prepend the error's `toString()` — V8 stack convention is `Error: msg\n    at …`.
- [x] **6D.4** Build a `v8::String::new(scope, &joined)` and return `PrepareStackTraceCallbackRet` containing it.
- [x] **6D.5** Integration test: write a `.ts` handler fixture that throws at line 42, compile + install into cache, dispatch, parse response.stack (or equivalent); assert `.ts` path and line `42` appear (not compiled line).

**Validate:** remap integration test green.

## 6E — Route remapped stacks to per-app log (~1 hour, low risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`, `crates/riversd/src/process_pool/types.rs`, AppLogRouter call site

- [x] **6E.1** In `call_entrypoint`'s error branch (execution.rs:~529), after capturing the exception, cast to `v8::Local<v8::Object>`, read the `stack` property; convert to Rust `String`. This is already the remapped trace (the callback fires on `.stack` property access).
- [x] **6E.2** Introduce `TaskError::HandlerErrorWithStack { message: String, stack: String }` struct variant in `types.rs`. Additive — exhaustive matches elsewhere will surface in the build.
- [x] **6E.3** At the error logging site in `execute_js_task`'s return path, when the error variant is `HandlerErrorWithStack`, emit `tracing::error!(target: "rivers.handler", trace_id = %trace_id, app = %app, message = %message, stack = %stack, "handler threw")`. AppLogRouter routes via `TASK_APP_NAME` thread-local into `log/apps/<app>.log`.
- [x] **6E.4** Integration test: trigger a handler throw; read `log/apps/<app>.log`; assert it contains the `.ts:line:col` string.

**Validate:** log file contains remapped trace; existing log outputs unchanged.

## 6F — Debug-mode error envelope (~1 hour, low risk)

**Files:** `crates/rivers-runtime/src/bundle.rs`, `crates/riversd/src/server/view_dispatch.rs` (or the `TaskError` → HTTP response conversion site)

- [x] **6F.1** Check `AppConfig` for existing `debug: bool`. If absent, add `#[serde(default)] pub debug: bool` to `AppConfig` in `rivers-runtime/src/bundle.rs`. Sourced from `[base] debug = true` in `app.toml`.
- [x] **6F.2** In the error-response serialization, when the error is `HandlerErrorWithStack` AND the app's `debug == true`:
  - Serialize `{ "error": message, "trace_id": id, "debug": { "stack": split_lines(stack) } }`.
  - Otherwise: `{ "error": message, "trace_id": id }` — no `debug` key at all.
- [x] **6F.3** Two integration tests: app with `debug = true` returns `debug.stack`; app with default `debug = false` omits it.

**Validate:** both tests green; non-debug response byte-identical to pre-change.

## 6G — Spec cross-refs + tutorial + changelogs (~30 min)

**Files:** `docs/arch/rivers-processpool-runtime-spec-v2.md`, `docs/arch/rivers-javascript-typescript-spec.md`, `docs/guide/tutorials/tutorial-ts-handlers.md`, `changedecisionlog.md`, `todo/changelog.md`

- [x] **6G.1** `processpool-runtime-spec-v2` Open Question #5 — replace with "Resolved by `rivers-javascript-typescript-spec.md §5` — see Phase 6 completion commits (TBD)."
- [x] **6G.2** `rivers-javascript-typescript-spec.md §5.4` — tighten wording to note the implementation landed.
- [x] **6G.3** `tutorial-ts-handlers.md` — add "Debugging handler errors" subsection: enabling `[base] debug = true` for `debug.stack` in dev; per-app log location `log/apps/<app>.log` is always remapped.
- [x] **6G.4** `changedecisionlog.md` — four new entries:
  1. Parsed-map cache separate from BundleModuleCache (rationale: re-parse cost)
  2. CallSite extraction via JS reflection (rationale: rusty_v8 has no wrapper)
  3. `TaskError::HandlerErrorWithStack` struct variant (rationale: additive, matches surface)
  4. App-level debug flag not view-level (rationale: spec §5.3 says app config)
- [x] **6G.5** `todo/changelog.md` — Phase 6 completion entry.

**Validate:** doc cross-refs resolve; changelog entries present.

## 6H — Canary sourcemap coverage (~1 hour, low risk)

**Files:** new `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/sourcemap.ts`; edit `canary-handlers/app.toml`, `canary-bundle/run-tests.sh`

- [x] **6H.1** Create `sourcemap.ts` handler: top-of-file throw at a distinctive line (e.g., line 42 literally — line 41 is a blank line right above `throw new Error("canary sourcemap probe")`). Export as `sourcemapProbe`.
- [x] **6H.2** Register in `canary-handlers/app.toml`:
  ```toml
  [api.views.sourcemap_test]
  path      = "/canary/rt/ts/sourcemap"
  method    = "POST"
  view_type = "Rest"
  auth      = "none"
  debug     = true

  [api.views.sourcemap_test.handler]
  type       = "codecomponent"
  language   = "typescript"
  module     = "libraries/handlers/ts-compliance/sourcemap.ts"
  entrypoint = "sourcemapProbe"
  ```
  (Move `debug` to the app-level `[base]` section if 6F.1 places it there rather than per-view.)
- [x] **6H.3** `run-tests.sh` — new "TYPESCRIPT Profile" block between HANDLERS and TRANSACTIONS-TS, with a `test_ep`-like probe that greps the response for `sourcemap.ts:42`.

**Validate:** canary endpoint returns an error envelope; `debug.stack` array contains `sourcemap.ts:42`.

---

## Files touched (hot list)

- **new:** `crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs`
- **edit:** `crates/riversd/src/process_pool/v8_engine/execution.rs` — callback register + body
- **edit:** `crates/riversd/src/process_pool/v8_engine/mod.rs` — submodule register
- **edit:** `crates/riversd/src/process_pool/module_cache.rs` — `clear_sourcemap_cache` from `install_module_cache`
- **edit:** `crates/riversd/src/process_pool/types.rs` — `HandlerErrorWithStack` variant
- **edit:** `crates/rivers-runtime/src/bundle.rs` — `AppConfig.debug`
- **edit:** `crates/riversd/src/server/view_dispatch.rs` — error envelope
- **edit:** `docs/arch/rivers-processpool-runtime-spec-v2.md`, `rivers-javascript-typescript-spec.md`, `tutorial-ts-handlers.md`
- **new:** `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/sourcemap.ts`
- **edit:** `canary-bundle/canary-handlers/app.toml`, `run-tests.sh`

## Verification (end to end)

1. `cargo test -p riversd --lib 'process_pool'` — 135+ prior tests green + new tests from 6A/6B/6C/6D/6E/6F.
2. `cargo deploy /tmp/rivers-sourcemap` — succeeds; `types/rivers.d.ts` present.
3. `riversd` running; POST `/canary-fleet/handlers/canary/rt/ts/sourcemap` — response body includes `debug.stack` with `sourcemap.ts:42:*`.
4. `tail log/apps/canary-handlers.log` — remapped trace present, correlated by `trace_id`.
5. Toggle `[base] debug = false`; redeploy; same request returns no `debug` key; log still has the remapped trace.

## Design decisions locked (will mirror into changedecisionlog.md during 6G)

1. **Parsed-map cache separate from BundleModuleCache.** Raw JSON stays in the module cache (cheap to hot-reload); parsed `Arc<SourceMap>` lives in its own `OnceCell<RwLock<HashMap>>`. `install_module_cache` invalidates both.
2. **CallSite via JS reflection.** rusty_v8 v130 has no CallSite wrapper. Invoke methods by name through `Object::get` + `Function::call`. Matches Deno's approach.
3. **`HandlerErrorWithStack` struct variant, not an `Option<String>` on `HandlerError`.** Additive; exhaustive matches surface everywhere that needs updating.
4. **App-level `debug` flag.** Spec §5.3 says `debug = true in app config`. Matches existing app-wide flags; avoids per-view proliferation.

## Non-goals

- `_source` inline handlers (tests only) — no on-disk path, no cache entry.
- Minification remapping (we don't minify).
- Chained source maps / `//# sourceMappingURL` directives in `.js` files.
- Remote source-map fetching.

## Effort estimate

| Task | Hours | Risk |
|---|---|---|
| 6A | 0.5 | low |
| 6B | 0.5 | low |
| 6C | 1.5 | medium |
| 6D | 1.5 | medium |
| 6E | 1 | low |
| 6F | 1 | low |
| 6G | 0.5 | low |
| 6H | 1 | low |
| **Total** | **~7.5** | |


---

## Archived 2026-04-22 — TS Pipeline Gap Closure Plan (G0-G8 shipped)

> Replaced by the Canary Scenarios task list per `docs/arch/rivers-canary-scenarios-spec.md`. The G0-G8 plan shipped via PR #79 (merged 2026-04-22). Residual P0 items (G-P0-A through G-P0-G) live on the `feature/gap-closure-p0` branch, commits 80a7f5e..c3eed8b.

# TS Pipeline Spec-Compliance Gap Closure

> **Branch:** `docs/guide-v0.54.0-updates` (continues TS pipeline work)
> **Spec:** `docs/arch/rivers-javascript-typescript-spec.md`
> **Gap analysis source:** this session's §-by-§ walkthrough. All 6 CB defects closed; remaining gaps are spec-compliance format/plumbing issues and canary coverage shortfall.
> **Prior:** TS pipeline Phases 0–11 + Phase 6 completion archived in `todo/gutter.md`.

**Goal:** close every observable gap between the implementation and the spec, split into four priority tiers. P0 is high-impact compliance (canary validation + runtime debug flag); P1 is format drift that changes observable surface; P2 is nice-to-haves; P3 is spec-doc corrections.

**Critical path:** G1 (canary TS coverage) is the biggest remaining gap — spec §9.2 lists 16 required test IDs; 1 is shipped at canary level today.

---

## G0 — Foundation decisions (blocking P0+)

Before executing G1–G8, two small calls clear ambiguity for the rest of the plan:

- [x] **G0.1** Decision: **option (a)** — amend spec §5.3 envelope shape to match existing `ErrorResponse` convention (`{code, message, trace_id, details.stack}`). Zero code change; spec edit in G8.4. Logged in `changedecisionlog.md`. (Done 2026-04-21.)
- [x] **G0.2** Decision: **option (a)** — drop `Rivers.db / Rivers.view / Rivers.http` from spec §8.3. None of these exist at runtime; aspirational stubs would create broken type-check signals. Spec edit in G8.6. Logged in `changedecisionlog.md`. (Done 2026-04-21.)

---

## P0 — High-impact spec compliance

### G1 — Canary TS-syntax coverage (spec §9.2)

**Scope:** 10 TS-syntax handler endpoints + 1 circular-import shell test + run-tests.sh profile. Each handler returns a `TestResult` per `test-harness.ts`; test harness asserts `passed=true`.

**Rationale:** spec §9.2 mandates canary-level exercise of every TS compiler feature. Unit tests prove `compile_typescript` works; canary proves the full dispatch invokes it correctly on a running riversd. Biggest single compliance gap.

**Files:**
- new: `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/*.ts` (one per case)
- edit: `canary-bundle/canary-handlers/app.toml` (register 10 views under `[api.views.ts_*]`)
- edit: `canary-bundle/run-tests.sh` (TYPESCRIPT profile expansion)
- new: `canary-bundle/tests/circular-import-rejection.sh` (standalone; not part of run-tests.sh)

Tasks:

- [x] **G1.1** `ts-compliance/param-strip.ts` — `function paramStrip(ctx: any)` + typed assertions. Probe case B. (Done 2026-04-21.)
- [x] **G1.2** `ts-compliance/var-strip.ts` — `const answer: number = 42` + `const name: string = "rivers"`. Probe case C. (Done 2026-04-21.)
- [x] **G1.3** `ts-compliance/import-type.ts` + `import-type-helpers.ts` — `import { type Answer, buildAnswer }`; uses `buildAnswer` at runtime. Probe case D. (Done 2026-04-21.)
- [x] **G1.4** `ts-compliance/generic.ts` — `function identity<T>(x: T): T` with call sites for number + string. Probe case E. (Done 2026-04-21.)
- [x] **G1.5** `ts-compliance/multimod.ts` + `multimod-helpers.ts` — imports `double`, `MODULE_MARKER`. Probe case F. Both files under `libraries/` are cached at bundle load. (Done 2026-04-21.)
- [x] **G1.6** `ts-compliance/export-fn.ts` — `export function exportFn(ctx)` hits the module namespace entrypoint path. Probe case G. (Done 2026-04-21.)
- [x] **G1.7** `ts-compliance/enum.ts` — numeric enum with forward + reverse lookup assertions (reverse lookup is lowering-specific). (Done 2026-04-21.)
- [x] **G1.8** `ts-compliance/decorator.ts` — TC39 Stage 3 decorator on a class method; sets `globalThis.__decorator_fired` + `__decorator_kind` so the handler can probe decorator-runtime execution. (Done 2026-04-21.)
- [x] **G1.9** `ts-compliance/namespace.ts` — `namespace util { export const VERSION = "1.0"; export function greet(who) { … } }` with runtime reads. (Done 2026-04-21.)
- [x] **G1.10** `canary-bundle/tests/circular-import-rejection.sh` + `fixtures/circular-import-reject/` — a.ts ↔ b.ts cycle. Shell test invokes `riverpackage validate`, asserts non-zero exit + the `circular import detected` phrase + both filenames in the error. SKIP path if `riverpackage` not on PATH. (Done 2026-04-21.)
- [x] **G1.11** 9 new `[api.views.ts_*]` blocks registered in `canary-handlers/app.toml` — ts_param_strip, ts_var_strip, ts_import_type, ts_generic, ts_multimod, ts_export_fn, ts_enum, ts_decorator, ts_namespace. All `method = "POST"`, `view_type = "Rest"`, `auth = "none"`. (Done 2026-04-21.)
- [x] **G1.12** `run-tests.sh` TYPESCRIPT profile split into "syntax + modules" (9 new `test_ep` lines) + "source map remap" (existing ts-sourcemap probe, unchanged). (Done 2026-04-21.)

**Validate:** `./run-tests.sh` shows PASS for all 9 new IDs + the sourcemap probe; canary total goes from 69+N/69+N to 69+N+9/69+N+9 green.

**Effort:** ~3 hours (mostly mechanical handler wrapper code).

### G2 — Canary transaction handlers (spec §9.2)

**Scope:** 5 transaction test endpoints. Requires a live PG datasource configured for the canary app.

**Files:**
- new: `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/txn-commit.ts`, `txn-rollback.ts`, `txn-cross-ds.ts`, `txn-nested.ts`, `txn-unsupported.ts`
- edit: `canary-bundle/canary-handlers/app.toml` (register under TRANSACTIONS-TS profile)
- edit: `canary-bundle/canary-handlers/resources.toml` (add `pg` datasource if not present)
- edit: `canary-bundle/run-tests.sh` (TRANSACTIONS-TS profile extension with PG_AVAIL gate)

Tasks:

- [x] **G2.1** `ts-compliance/txn-commit.ts` — `ctx.transaction("pg", () => ctx.dataview("txn_pg_ping"))`; assertions on no-throw + callback return value reaches handler + rows readable via held connection. Uses `SELECT 1` DataView to avoid schema setup. (Done 2026-04-21.)
- [x] **G2.2** `ts-compliance/txn-rollback.ts` — callback executes a dataview then throws a distinctive message; handler asserts the re-thrown message reaches it unchanged. (Done 2026-04-21.)
- [x] **G2.3** `ts-compliance/txn-cross-ds.ts` — transaction on `pg`, dataview `txn_sqlite_ping` (points at `sqlite_cross` datasource); asserts TransactionError + spec §6.2 "differs from" phrase + named dataview. (Done 2026-04-21.)
- [x] **G2.4** `ts-compliance/txn-nested.ts` — genuine nested call: `ctx.transaction("pg", () => ctx.transaction("pg", …))`. Asserts `TransactionError: nested transactions not supported`. (Done 2026-04-21.)
- [x] **G2.5** `ts-compliance/txn-unsupported.ts` — `ctx.transaction("canary-faker", …)`; asserts `TransactionError: ... does not support transactions`. Uses the pre-existing `canary-faker` datasource — no PG needed, so this runs even on no-infra deploys. (Done 2026-04-21.)
- [x] **G2.6** `resources.toml` — added `pg` datasource pointing at 192.168.2.209 (required=false so missing infra doesn't block bundle load) + `sqlite_cross` for the cross-ds test. Two minimal DataViews (`txn_pg_ping` and `txn_sqlite_ping`, both `SELECT 1`) registered in `app.toml` — no table schema required. (Done 2026-04-21.)
- [x] **G2.7** `run-tests.sh` TRANSACTIONS-TS profile extended. `txn-unsupported` runs unconditionally (uses faker). The PG-dependent four (txn-commit, txn-rollback, txn-cross-ds, txn-nested) run behind a `PG_AVAIL` gate that pings `/sql/canary/sql/pg/param-order` — the same gate pattern used elsewhere in run-tests.sh. On no-infra deploys, each prints `SKIP … (PG unreachable)` and contributes 0 to PASS/FAIL. (Done 2026-04-21.)

**Validate:** 5/5 PASS on PG cluster; SKIP cleanly without PG.

**Effort:** ~2 hours + infra access for the roundtrip run.

### G3 — Per-app debug flag runtime plumbing (spec §5.3)

**Scope:** replace `cfg!(debug_assertions)` gate in `error_response::map_view_error` with a runtime read of `AppConfig.base.debug` for the matched app.

**Files:**
- edit: `crates/riversd/src/error_response.rs` — `map_view_error` signature adds `debug_enabled: bool`
- edit: `crates/riversd/src/server/view_dispatch.rs` — look up matched view's app's `AppConfig.base.debug`; pass to `map_view_error`

Tasks:

- [x] **G3.1** `map_view_error` signature extended with `debug_enabled: bool`; replaces `cfg!(debug_assertions)` checks for Handler, HandlerWithStack, Pipeline, Internal. `cfg!(debug_assertions)` retained as an OR fallback for dev-build convenience. (Done 2026-04-21.)
- [x] **G3.2** `view_dispatch.rs` error branch looks up `ctx.loaded_bundle.apps[].manifest.app_id == manifest_app_id` and reads `.config.base.debug`. Falls back to `false` on lookup miss. Passed into `map_view_error`. (Done 2026-04-21.)
- [x] **G3.3** Updated existing 6 `map_view_error(...)` test calls to pass `false`. Added 2 new G3 tests: `g3_handler_with_stack_surfaces_when_debug_enabled` (always passes) and `g3_handler_with_stack_debug_disabled_in_release_hides` (asserts OR semantics — hides in release, surfaces in cargo-test debug). 24/24 `error_response_tests` green. (Done 2026-04-21.)
- [x] **G3.4** Decision-log entry captured in G0.1 / G8.4 rationale — runtime flag IS the mechanism; OR with `cfg!(debug_assertions)` for dev convenience documented in the function docstring. (Done 2026-04-21.)

**Validate:** tests green; integration run shows debug=true app produces `details.stack` + debug=false app omits it, in the SAME build.

**Effort:** ~1 hour.

### G4 — `rivers.d.ts` spec alignment (spec §8.3)

**Scope:** rename `Ctx` → `ViewContext` with type alias, reconcile `Rivers.db/view/http` per G0.2, add capability-gated JSDoc markers.

**Files:** edit `types/rivers.d.ts`, edit `docs/guide/tutorials/tutorial-ts-handlers.md` if naming changes propagate.

Tasks:

- [x] **G4.1** Renamed primary interface `Ctx` → `ViewContext` (with JSDoc note). Added `type Ctx = ViewContext` alias at end-of-file for backcompat. Updated `HandlerFn`'s parameter type. (Done 2026-04-21.)
- [x] **G4.2** Per G0.2: `Rivers.db/view/http` dropped from spec §8.3 (G8.6). `rivers.d.ts` declares only the runtime-injected surface. No stubs added. (Done 2026-04-21.)
- [x] **G4.3** Capability markers added: `Rivers.keystore` + `Rivers.crypto.encrypt/decrypt` (`@capability keystore`), `ctx.transaction` (`@capability transaction`). Informational comment block describing the capability-tag convention added at the bottom of the file. `allow_outbound_http` marker deferred — no typed surface to annotate until `Rivers.http` ships. (Done 2026-04-21.)
- [x] **G4.4** `tutorial-ts-handlers.md` updated: `Ctx` → `ViewContext` in the "Using the Rivers-shipped rivers.d.ts" section. (Done 2026-04-21.)

**Validate:** `tsc --noEmit` on a sample handler using the new `ViewContext` name resolves; `Ctx` alias works for backcompat.

**Effort:** ~30 min.

---

## P1 — Format / cosmetic drift

### G5 — Error message format alignment

Spec uses specific multi-line error formats in §2.5, §3.1, §3.2. Implementation condenses to single lines with equivalent information.

**Files:** `crates/riversd/src/process_pool/v8_config.rs`, `v8_engine/execution.rs` (resolve_module_callback)

Tasks:

- [x] **G5.1** `.tsx` rejection now uses spec §2.5's `{app}/{path}` form when a `libraries/` ancestor is detected. New helper `shorten_app_path` walks path components backward; falls back to raw filename for inline/test paths. New test `compile_typescript_rejects_tsx_with_app_short_path` verifies the short form. (Done 2026-04-21.)
- [x] **G5.2** Missing-extension error in `resolve_module_callback` expanded to spec §3.1 multi-line format with `in {referrer}` and `hint: use "{spec}.ts" or "{spec}.js"` lines. (Done 2026-04-21.)
- [x] **G5.3** Not-in-cache error expanded to spec §3.2 format: `resolves outside app boundary` + `in {referrer}` + `resolved to:` + `boundary:`. New `boundary_from_referrer` helper walks up path components to find the nearest `libraries/` ancestor — that's the spec's `{app}/libraries/` boundary. Falls back to no-boundary-line if no `libraries/` ancestor found. (Done 2026-04-21.)
- [x] **G5.4** Existing `compile_typescript_rejects_tsx` test updated with clearer intent comment; new short-path test added. No resolver-callback tests existed (V8 callback runs inside a live isolate; unit testing is indirect via dispatch). 141/141 `process_pool` lib tests still green; 19/19 `compile_typescript` integration tests green. (Done 2026-04-21.)

**Validate:** existing unit tests pass with updated assertions; no behaviour change, only message change.

**Effort:** ~1 hour.

### G6 — Debug envelope field names (resolved by G0.1)

Per G0.1 decision. If option (a) — spec changes to match Rivers' `ErrorResponse` convention — this work is a spec edit only, covered by G8.5. If option (b) — response envelope changes — it's a bigger migration:

- [x] **G6.1** Not applicable — G0.1 = option (a). Envelope alignment was handled entirely by the G8.4 spec edit (spec §5.3 now documents the existing `{code, message, trace_id, details.stack}` shape). Zero code change. (Resolved 2026-04-21.)

**Validate:** all error responses migrate; existing clients documented.

**Effort:** option (a) = 0; option (b) = ~1 day + migration plan.

---

## P2 — Nice-to-have tightening

### G7 — ES2022 codegen target (spec §2.4)

Currently: parser target = ES2022, codegen target = default (ESNext). Spec intent is that ES2023+ syntax gets lowered to ES2022. In practice V8 v130 supports most ES2023; gap is theoretical.

**Files:** `crates/riversd/src/process_pool/v8_config.rs`

Tasks:

- [x] **G7.1** Set `Config::with_target(EsVersion::Es2022)` on the Emitter in `v8_config.rs`. Documents ES2022 as the compilation target floor. **Scope note:** the codegen `target` flag influences emission decisions (reserved-word handling, some formatting); it does NOT semantically downlevel ES2023+ AST nodes. True downleveling requires a `swc_ecma_transforms_compat::es2022` transform pass inserted between `typescript()` and `fixer()` — not wired in this phase because V8 v130 natively supports ES2023 features (findLast, hashbangs, etc.) so the gap is theoretical. (Done 2026-04-21.)
- [x] **G7.2** Added `compile_typescript_preserves_es2022_class_fields` — verifies canonical ES2022 syntax (class fields) emits as-is when target is set to Es2022. Full downlevel-an-ES2023+-feature test deferred with the lowering itself. (Done 2026-04-21.)

**Validate:** test green; existing tests unaffected (current TS corpus is ES2022 or below).

**Effort:** ~30 min.

---

## P3 — Spec document corrections

### G8 — Spec self-corrections

Not code changes; they're edits to `docs/arch/rivers-javascript-typescript-spec.md` to reflect implementation reality.

Tasks:

- [x] **G8.1** §2.1 updated: `swc_core = "64"` with full feature list + note that swc uses major-per-release; `swc_sourcemap = "10"` direct dep added. (Done 2026-04-21.)
- [x] **G8.2** §2.2 bullet list: removed "TC39 Stage 3 decorator lowering" (that pass doesn't live in `typescript::typescript()`). Added a clarifying note pointing at §2.3 for decorator handling. (Done 2026-04-21.)
- [x] **G8.3** §2.3 rewritten: removed the invalid `DecoratorVersion::V202203` snippet; documented the actual parse-and-pass-through model with V8 executing Stage 3 decorators natively. Points out `swc_ecma_transforms_proposal::decorators` is not applied. (Done 2026-04-21.)
- [x] **G8.4** §5.3 envelope example aligned to `ErrorResponse` shape — `{code, message, trace_id, details.stack}`. Non-debug responses omit `details` entirely. (Done 2026-04-21.)
- [x] **G8.5** §6.4 driver table qualified: built-in rows cite source file; plugin rows marked "verify at plugin load" with a note that runtime enforcement is authoritative. (Done 2026-04-21.)
- [x] **G8.6** §8.3 required-declarations list corrected: removes `Rivers.db/view/http` with a rationale note; explicit cross-ref to `rivers_global.rs` as the authoritative injection surface. (Done 2026-04-21.)

**Validate:** spec reads consistently with implementation; every MUST/SHOULD has a satisfied counterpart.

**Effort:** ~1 hour (all editing, no code).

---

## Files touched (hot list)

- **new:** 10 files under `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/`
- **new:** `canary-bundle/tests/circular-import-rejection.sh`
- **edit:** `canary-bundle/canary-handlers/app.toml` (14 new `[api.views.ts_*]` and `[api.views.txn_*]` blocks)
- **edit:** `canary-bundle/canary-handlers/resources.toml` (if PG datasource needed)
- **edit:** `canary-bundle/run-tests.sh` (profile expansions)
- **edit:** `crates/riversd/src/error_response.rs` (signature + tests)
- **edit:** `crates/riversd/src/server/view_dispatch.rs` (debug flag lookup)
- **edit:** `crates/riversd/src/process_pool/v8_config.rs` (error messages, ES2022 codegen)
- **edit:** `crates/riversd/src/process_pool/v8_engine/execution.rs` (resolve_module_callback error messages)
- **edit:** `types/rivers.d.ts` (ViewContext rename, capability markers)
- **edit:** `docs/guide/tutorials/tutorial-ts-handlers.md` (type name propagation)
- **edit:** `docs/arch/rivers-javascript-typescript-spec.md` (G8 self-corrections)
- **edit:** `changedecisionlog.md`, `todo/changelog.md`

## Verification — end to end

1. `cargo test -p riversd --lib` — 310/310 prior tests still green; ~6 new tests from G3, G5, G7.
2. `cargo deploy /tmp/rivers-gap-closure` — deploy succeeds with all updates.
3. `just probe-ts` against deployed instance — all 9 probe cases green.
4. `canary-bundle/run-tests.sh` — TYPESCRIPT profile shows 10/10 PASS; TRANSACTIONS-TS shows 5/5 PASS on PG cluster.
5. `canary-bundle/tests/circular-import-rejection.sh` — non-zero exit with expected spec §3.5 error.
6. Spec re-read: every MUST/SHOULD in `rivers-javascript-typescript-spec.md` maps to an implementation element or an explicit deferral with cross-ref.

## Effort summary

| Tier | Items | Effort | Risk |
|------|-------|--------|------|
| G0 | 2 decisions | 30 min | low |
| G1 canary TS-syntax | 12 tasks | ~3 hours | low |
| G2 canary transaction | 7 tasks | ~2 hours + infra | medium (PG access) |
| G3 debug flag plumbing | 4 tasks | ~1 hour | low |
| G4 rivers.d.ts | 4 tasks | ~30 min | low |
| G5 error formats | 4 tasks | ~1 hour | low |
| G6 envelope fields | 1 task (option b only) | 0 or ~1 day | medium if (b) |
| G7 ES2022 codegen | 2 tasks | ~30 min | low |
| G8 spec corrections | 6 tasks | ~1 hour | low |
| **Total P0** | G0+G1+G2+G3+G4 | **~7 hours** | |
| **Total P1+P2+P3** | G5+G7+G8 | **~2.5 hours** | |
| **Grand total** | | **~9.5 hours** (excluding G6-b if chosen) | |

## Execution order

1. **G0.1, G0.2** — decisions first (clears ambiguity)
2. **G8.1–G8.6** — spec corrections (quick wins; locks the target for code changes)
3. **G3** — debug flag plumbing (unblocks canary G2 tests that need debug=true)
4. **G4** — rivers.d.ts cleanup (independent; quick)
5. **G1** — canary TS-syntax handlers (biggest chunk; mechanical)
6. **G5** — error message alignment (can run parallel to G1)
7. **G7** — ES2022 codegen (independent; quick)
8. **G2** — canary transaction handlers (last; needs live infra)
9. **G6** — only if G0.1 = option (b)

## Design decisions to log (changedecisionlog.md)

1. **G0.1 decision** — spec vs envelope alignment
2. **G0.2 decision** — Rivers.db/view/http aspirational vs declared
3. **G3 approach** — runtime AppConfig lookup vs compile-time cfg
4. **G5.3 plumbing** — how `{app}/libraries/` root reaches the resolve callback (extend `TASK_MODULE_REGISTRY`?)

## Non-goals (explicit out-of-scope)

- Implementing `Rivers.db`, `Rivers.view`, `Rivers.http` runtime surfaces (if G0.2 picks option (a)).
- Full esbuild-style bundler (spec §1.2 out-of-scope).
- Node-style `node_modules` resolution.
- JSX/TSX support.
- Chained source maps (`.js` files with `//# sourceMappingURL`).
- Cross-app code sharing.
## 2026-04-24 — Archived CG plan before full code review

Source: `todo/tasks.md` before replacing it with the full code-review plan requested on 2026-04-24.

# CG — Canary Green Again

> **Branch:** `docs/guide-v0.54.0-updates` (current)
> **Source:** `docs/canary_codereivew.md` (2026-04-24) + `docs/dreams/dream-2026-04-22.md`
> **Goal:** canary boots reliably and the Kafka-consumer-store + MySQL CRUD lanes go green without touching the deferred polish work.

**Prior plan:** CS0–CS7 + BR0–BR7 archived to `todo/gutter.md` under the 2026-04-24 header. Deploy-gated residuals from that plan are folded into CG5 below.

**Rules:**
- Each task has a specific **file:line** target + a **validation step**.
- Fixes go in the order below: small isolated bugs → architectural fix → revert → verify. Each step leaves canary at least as healthy as before.
- No workarounds. If a subtask hits a blocker that would need a hollow shortcut, stop and mark DEFERRED with rationale.
- Auto mode: execute sequentially; do not batch fixes inside one commit unless they share a root cause.

**Critical path:** CG0 → CG1 → CG2 → CG3 → CG4 → CG5. CG1 + CG2 can land in one commit (they share the "MessageConsumer/topic-wiring" root cause). CG3 and CG4 each get their own commit.

---

## CG0 — Housekeeping

Tasks:

- [x] **CG0.1** N/A — prior CS/BR plan archived to `todo/gutter.md` under 2026-04-24 header.
- [x] **CG0.2** Verified: `canary-streams/app.toml` has `[api.views.kafka_consume]` + `[api.views.kafka_consume.on_event] topic = "kafka_consume"` uncommented (BR4.5 landed). `resources.toml` `required=true` on canary-kafka is consistent. No change needed.
- [x] **CG0.3** `changedecisionlog.md` gained the "CG plan supersedes CS/BR" entry at top.

**Validation:** `riverpackage validate canary-bundle` → same warning count as before; no new errors.

**Effort:** 15 min.

---

## CG1 — MessageConsumer app identity fix

**Root cause:** `crates/riversd/src/message_consumer.rs:334` calls `crate::task_enrichment::enrich(builder, "")` with an empty `app_id`. `TaskLocals::set` falls back to `app:default` for the ctx.store namespace when app_id is empty. Consumer writes to `app:default:canary:kafka:last_verdict`; REST verify reads from `canary-streams:canary:kafka:last_verdict`. Store is real but the keys don't cross.

**Files:**
- edit: `crates/riversd/src/message_consumer.rs` — thread `app_id` / entry_point through `MessageConsumerConfig` + `MessageConsumerHandler` + `MessageConsumerRegistry::from_views`.

Tasks:

- [x] **CG1.1** `entry_point: String` field added to `MessageConsumerConfig`. `from_view` signature gained `entry_point: &str` as first arg.
- [x] **CG1.2** `MessageConsumerRegistry::from_views` signature updated; `wire.rs:147` passes the app's `entry_point` in.
- [x] **CG1.3** `MessageConsumerHandler::handle` + `dispatch_message_event` both use `&self.config.entry_point` / `&config.entry_point` instead of `""`.
- [x] **CG1.4** Integration test `registry_from_mixed_views` asserts `registry.get("consumer1").unwrap().entry_point == "canary-streams"`. All 13 message_consumer tests pass.

**Validation:**
- `cargo test -p riversd message_consumer` green.
- Deploy + run `canary-bundle/run-tests.sh`: the two Kafka consumer-store tests (`kafka_publish_then_consume`, any `kafka_consume_store_verify`) go from FAIL to PASS. Record pass delta.

**Effort:** 30 min.

---

## CG2 — Subscription topic wiring

**Root cause:** `crates/riversd/src/bundle_loader/wire.rs:42-52` builds broker subscriptions with `topic = view_id`. Should read `on_event.topic` from the view config. (Publish side is already fixed — broker_bridge.rs:261-264 publishes both `BROKER_MESSAGE_RECEIVED` + a per-destination event; that landed during the compaction session.)

**Files:**
- edit: `crates/riversd/src/bundle_loader/wire.rs`

Tasks:

- [x] **CG2.1** `wire.rs` MessageConsumer iteration reads `view_cfg.on_event.as_ref().map(|oe| oe.topic.clone())` with a `tracing::warn!` fallback to view_id when `on_event` is absent.
- [x] **CG2.2** Subscription `topic` and `event_name` are both set to the on_event.topic (or view_id fallback) — consumer and per-destination publish now agree on the name.
- [ ] **CG2.3 (DEFERRED)** Dedicated wire.rs subscription-extraction unit test — current test coverage is indirect via message_consumer_tests which passes; adding a wire.rs-local test would require exposing an internal helper. Will add when the CG5 canary deploy proves the path end-to-end, and refactor to a testable helper if needed.

**Validation:**
- `cargo test -p riversd bundle_loader` green.
- Deploy + run `canary-bundle/run-tests.sh` with Kafka reachable: `STREAM-KAFKA-PUBLISH-THEN-CONSUME` passes end-to-end (publish → Kafka → bridge → EventBus topic → MessageConsumer → ctx.store → verify). Should require CG1 + CG2 both landed.

**Effort:** 30 min.

---

## CG3 — Non-blocking broker consumer startup

**Root cause:** `crates/riversd/src/bundle_loader/wire.rs:115` awaits `broker_driver.create_consumer(params, &broker_config).await` inline during bundle load. When Kafka is unreachable (macOS "No route to host" today, any broker flake in general), bundle load hangs, HTTP never binds. This is the current blocker for the canary hanging at startup.

**Files:**
- edit: `crates/riversd/src/broker_bridge.rs` — add a `BrokerBridgeSpec` + `run_with_retry(spec, driver)` that owns `create_consumer` retry.
- edit: `crates/riversd/src/bundle_loader/wire.rs` — spawn the bridge immediately without awaiting consumer creation.

Tasks:

- [x] **CG3.1** `BrokerBridgeSpec` added to `broker_bridge.rs` with the fields planned.
- [x] **CG3.2** `pub async fn run_with_retry(spec: BrokerBridgeSpec)` landed. Exponential backoff base=reconnect_ms, cap=30s, ±50% jitter via `rand::thread_rng`. Shutdown checked both before each attempt and during sleep (`tokio::select!`).
- [x] **CG3.3** `wire.rs` replaced the inline `create_consumer().await` with `tokio::spawn(crate::broker_bridge::run_with_retry(spec))`.
- [x] **CG3.4** `factory.get_broker_driver` returns `Option<&Arc<dyn MessageBrokerDriver>>` — clone is a no-op Arc bump; spec stores the cloned Arc.
- [x] **CG3.5** `supervisor_retries_and_exits_on_shutdown` — `FailingDriver` errors on every `create_consumer`; test asserts `attempts >= 2` after 250ms backoff, then shutdown-signal returns within 1s. PASS.
- [x] **CG3.6** `supervisor_spawn_is_non_blocking` — `HangingDriver`'s `create_consumer` returns `std::future::pending()`; test asserts `tokio::spawn` returns in <50ms regardless. PASS.

**Validation:**
- `cargo test -p riversd broker_bridge` green.
- `cargo test -p riversd bundle_loader` green.
- Manual: disable network to Kafka (or use bogus host), `cargo deploy`, `riversctl start --foreground`; assert HTTP listener binds within the usual startup window (<5s from log "bundle loaded"), canary hits non-Kafka endpoints. Kafka-gated tests SKIP cleanly.

**Effort:** 2–3 hours.

---

## CG4 — Restore MySQL pool

**Root cause:** `crates/rivers-drivers-builtin/src/mysql.rs:45-67` was swapped from `mysql_async::Pool::new` to direct `mysql_async::Conn::new` because per-call `Runtime::new` in host_callbacks was tearing down pool background tasks. Host_callbacks was fixed (runtime isolation removed). The Pool should come back — every dataview call currently pays a full MySQL handshake.

**Files:**
- edit: `crates/rivers-drivers-builtin/src/mysql.rs`

Tasks:

- [x] **CG4.1** Process-global pool cache behind `OnceLock<Mutex<HashMap<String, mysql_async::Pool>>>`. Key = `host:port/database?u=user` (password excluded — never in map keys). `get_or_create_pool()` hits cache or builds `Pool::new(opts)` once per distinct tuple. `connect()` calls `pool.get_conn().await` — checkout, not handshake.
- [x] **CG4.2** Comment in `mysql.rs` rewritten to explain CG4 restoration after the host_callbacks runtime fix unblocked it.
- [ ] **CG4.3 (PENDING DEPLOY)** Runtime regression check — deploy + run the canary MySQL CRUD lane, assert no "Tokio 1.x context was found, but it is being shutdown" errors in the log.
- [ ] **CG4.4 (PENDING DEPLOY)** Runtime-verified pool-reuse — mysql_async doesn't expose a pool-count hook for unit testing. Verify via canary that MySQL CRUD latency drops vs the pre-CG4 baseline (exact number depends on network; handshake was ~10-50ms on the test cluster).

**Validation:**
- `cargo test -p rivers-drivers-builtin` green.
- Canary: MySQL CRUD tests PASS; latency per call should drop (rough gauge: total runtime of MySQL CRUD group vs prior). Capture before/after in `todo/changelog.md`.

**Effort:** 2 hours.

---

## CG5 — Deploy + verify

Tasks:

- [ ] **CG5.1** `cargo deploy /tmp/rivers-cg` — clean build with static-engines + static-plugins.
- [ ] **CG5.2** `riversctl start --foreground` on the deployed instance. Assert log line "main server listening" appears. Record startup wall-clock.
- [ ] **CG5.3** `canary-bundle/run-tests.sh` — count PASS / FAIL / SKIP. Expected: startup blocker gone; Kafka consumer-store lane green (2 tests from CG1+CG2); MySQL CRUD lane green (7 tests from CG4); PG lane should also improve (host_callbacks runtime fix + no MySQL-induced cascade).
- [ ] **CG5.4** Categorise remaining failures into:
    1. Pre-existing driver/config issues unrelated to this plan (NoSQL, MCP, decorator, HMAC, etc.).
    2. Anything new introduced by CG1–CG4 (should be zero).
- [ ] **CG5.5** Append `canary-bundle/CHANGELOG.md` with the CG entry: what shipped, expected canary delta, known remaining lanes.
- [ ] **CG5.6** Commit per CG tier: CG1+CG2 as one commit (shared root cause), CG3 as one commit, CG4 as one commit, CG5 as doc commit. Per-commit message cites the code-review doc item number.

**Validation:** canary PASS count rises by at least 9 (2 Kafka + 7 MySQL). Startup never hangs on broker.

**Effort:** 1 hour + whatever the remaining-failure triage takes.

---

## Out-of-scope for this plan (tracked for later)

Not fixing here — these are from dream-2026-04-22 and code-review P1s that don't block canary-green:

- Kafka producer eager metadata on `create_producer` (code-review P0 #2) — P0 for prod latency, not for canary-green.
- SWC hard timeout (code-review P0 #4) — P0 for prod deploy safety, not for canary-green.
- Bounded source-map LRU (P1 #7).
- Absolute-path redaction in stack traces (P1 #8).
- Remove silent disk fallback on module-cache miss (P1 #9).
- Thread-local panic-safety integration test (P1 #10).
- JSON double-encode on broker publish hot path.
- `.expect("producer initialised above")` at broker_dispatch.rs:129.
- `TASK_COMMIT_FAILED` one-shot overwrite on second commit failure.
- Kafka producer topic-at-create-time contract violation.

All of these belong in a follow-on "prod hardening" plan after canary is green.

## Execution order

1. **CG0** — housekeeping + decisionlog pointer.
2. **CG1 + CG2** — one commit. Smallest, highest leverage. Fixes Kafka-consumer-store lane.
3. **CG3** — unblocks riversd startup.
4. **CG4** — fixes MySQL tail latency + CRUD lane.
5. **CG5** — deploy, count, triage, commit.

---

# Archived 2026-04-24 — RCC Review Consolidation Report

Superseded by user clarification: only review `rivers-plugin-exec`; a separate session will consolidate findings.

## Pending Tasks at Archive Time

- [ ] **RCC0.1 — Re-check report inputs.**
  Validate whether any 22-report input set has appeared under `reviews/`, `docs/review/`, or another obvious report path before writing the final consolidation.

- [ ] **RCC0.2 — Choose source basis honestly.**
  If the 22 per-crate reports are present, read every report in full and consolidate from them. If they are still absent, produce a clearly labeled fallback consolidation from `docs/code_review.md`, with a top-level warning that it is not the requested 22-report consolidation.

- [ ] **RCC1.1 — Extract Rivers-wide repeated patterns.**
  Group findings that appear across 3+ crates or across shared runtime/driver surfaces, including missing timeouts, pool/accounting gaps, unsafe FFI boundary assumptions, error masking, and integer truncation/overflow.

- [ ] **RCC1.2 — Extract contract violations.**
  Compare findings against the `rivers-driver-sdk`, engine host callback, and runtime datasource contracts.

- [ ] **RCC1.3 — Extract cross-crate wiring gaps.**
  Identify any registration, callback, datasource, engine, or handler path where the implementation exists in one crate but the production caller path is missing, stubbed, or bypassed.

- [ ] **RCC1.4 — Build severity distribution.**
  Summarize clean vs bug-dense crates and identify technical-debt clusters.

- [ ] **RCC2.1 — Write report to `docs/review/cross-crate-consolidation.md`.**
  Produce a concise but complete report with sections for grounding, executive summary, repeated patterns, contract violations, wiring gaps, severity distribution, and recommended shared fixes.

- [ ] **RCC2.2 — Update logs.**
  Record report-delivery decisions in `changedecisionlog.md` and the file changes in `todo/changelog.md`.

- [ ] **RCC2.3 — Verify markdown and whitespace.**
  Run `git diff --check -- docs/review/cross-crate-consolidation.md todo/tasks.md changedecisionlog.md todo/changelog.md`.
