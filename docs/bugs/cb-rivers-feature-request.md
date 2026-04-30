# Circuit Breaker — Feature Requests for Rivers Framework

**From:** Circuit Breaker (CB) project
**To:** Rivers framework team
**Date:** 2026-04-28
**Status:** Draft for submission

---

## Context

Circuit Breaker is a project management system providing structured oversight of Claude Code (CC) work. The governing philosophy is **"momentum over interruption"** — humans redirect work through markdown shaping documents and approval queues rather than automated stops.

CB is built on Rivers. cb-service is a Rivers bundle (handlers + DataViews + MCP); cb-main is a Rivers bundle (SPA + admin). CB is also dogfooding itself — once stable, the Rivers team's own project tracking will run on CB. We have a strong interest in Rivers shipping the right primitives because the better Rivers gets, the better CB gets, and vice versa.

This document is a greedy ask. Items are tagged P0 (pivot blockers), P1 (high-impact), P2 (quality-of-life). Each entry states current state, requested change, and why CB needs it.

---

## Architectural pivot driving these asks

CB has pivoted to make MCP CC's primary write surface. Per our existing decision log (Tier 2, 2026-04-26), MCP currently exposes 4 read-only DataView resources + 3 prompts. The 8 mutating MCP tools from spec §7.2 (`cb_add_task`, `cb_mark_complete`, `cb_log_decision`, `cb_log_change`, `cb_log_bug`, `cb_log_feature`, `cb_update_investigation`, plus newly-added `cb_flag_task_oversized` and `cb_flag_sprint_miscalibrated`) are deferred because Rivers MCP only accepts DataView refs (per `crates/rivers-runtime/src/validate_crossref.rs#MCP-VAL-1`).

Until the P0 items below are resolved, CC writes will continue to flow through REST endpoints rather than MCP, which:
- Splits CC's tool surface awkwardly (read = MCP, write = REST)
- Loses MCP's structured tool invocation semantics for the most-used operations
- Forces CC to maintain two different auth flows

We are not pursuing a temporary REST-proxy MCP server as a bridge. Per CB's guiding principle ("airplane gauges, not rushed gauges"), we'd rather wait for Rivers to ship the right thing than maintain throwaway infrastructure.

---

## P0 — Pivot blockers

### P0.1 — MCP tools backed by codecomponent views

**Current state:** `crates/rivers-runtime/src/validate_crossref.rs#MCP-VAL-1` rejects MCP tool definitions that reference codecomponent views; only DataView refs are accepted.

**Requested:** Allow `[[mcp.tools]]` entries to reference codecomponent views (handler-backed views), not just DataViews.

**Why CB needs it:**

CB's mutating tools cannot be expressed as DataViews because they require:
- **Transactional invariant checks across multiple tables.** `cb_mark_complete` writes a decision row, updates a WIP row, validates the decision references the task, validates validation-statement counts, and emits an OTel counter — all atomically.
- **Real-time scope-of-work (ScW) validation.** `cb_add_task` must reject WIP whose description matches banned scopes declared in the active sprint's ScW.
- **Sizer enforcement at write time.** `cb_add_task` rejects WIP exceeding sizing contracts (`estimated_files ≤ 3`, `estimated_loc ≤ 300`, single arch layer). DataViews can't encode "reject if this rule fails."
- **Foreign-key handler logic.** Investigation proof artifacts require manual unmap before retire. Investigation auto-commit predicates depend on linked-issue state across tables.
- **Handler-side OTel emission.** `cb_log_decision` emits `cb.decision.logged` counter on success — DataViews don't expose telemetry hooks.

**Suggested approach:** Mirror the DataView ref pattern but accept codecomponent view refs. The handler's request/response types (TypeScript) become the source of truth for the tool's input/output schema (see P0.2).

---

### P0.2 — MCP tool schema derivation from codecomponent signatures

**Current state:** DataView-backed MCP tools derive `inputSchema` from query parameter names and types. Codecomponent views currently have no equivalent path because they aren't accepted as tool refs (P0.1).

**Requested:** When P0.1 ships, derive MCP tool `inputSchema` and `outputSchema` from the codecomponent's TypeScript request/response type definitions.

**Why CB needs it:**

CB has TypeScript types for every handler request/response (e.g., `MarkCompleteRequest`, `MarkCompleteResponse`). Hand-maintaining a parallel JSON schema in `app.toml` for every MCP tool is duplication that will drift. Schema generation from existing types keeps the contract in one place.

**Suggested approach:** TypeScript → JSON Schema generation already exists as a tooling problem (libraries like `typescript-json-schema`, `ts-json-schema-generator`). Rivers could either bundle one of these or expose a hook so bundles can plug in their own generator.

---

### P0.3 — Authentication context propagation to MCP-invoked handlers

**Current state:** Per our G.1 decision (2026-04-26), CB switched three views from `auth = "api_key"` to `auth = "none"` and uses handler-side `resolveApiKey()` against the `api_keys` table. This works for REST. The MCP path is unspecified.

**Requested:** When a codecomponent handler is invoked via MCP, the calling identity (API key, session token, or equivalent) must be available to the handler in the same way it is for REST invocations. The handler should not need to special-case MCP vs REST.

**Why CB needs it:**
- API key is per-project. CB's authorization decisions (which project can this CC see, which approval groups apply) all key off the resolved API key.
- The `api_keys.last_used_at` audit column needs to update on every authenticated call regardless of source.
- Standalone decisions inherit the calling task's scope, which requires resolving the API key → project → active session.

**Open question we need Rivers to answer:** Is the model:
- (a) Per-MCP-request API key (key passed in every tool-call envelope)
- (b) Session-based (key established at MCP session init, propagated to subsequent calls)
- (c) Bundle-level config (key declared at MCP server level)

CB's preference is (b): the user authenticates once when CC connects, and Rivers propagates the resolved identity to every handler invocation.

---

## P1 — High-value enhancements

### P1.1 — MCP resource subscriptions / push notifications

**Current state:** Unclear whether Rivers' MCP implementation exposes the MCP spec's `subscriptions/list` and notification mechanisms. CB's existing implementation polls.

**Requested:** Confirm or implement subscription-based MCP resources, where a client can subscribe to a resource URI and receive push updates when the underlying data changes.

**Why CB needs it:**

CB has several event streams that fit the subscription model:
- **Ape Flag firing.** CC subscribes to its project's Ape Flag stream; when a flag fires (ScW violation, decision/edit ratio anomaly), CC sees it on next turn.
- **Investigation queue.** CC subscribes to its active task's investigation list; CD-opened investigations push to CC.
- **Approval queue updates.** CD subscribes to its approval queue; new pending actions push.
- **Sprint SoW/ScW drift.** CC subscribes to active sprint state; if a PM edits during sprint (governance violation, but possible), CC sees the new contract immediately.

Polling these works but burns turns and adds latency. Subscriptions are the right primitive.

---

### P1.2 — MCP tool annotations (destructive/idempotent/readOnly hints)

**Current state:** MCP spec defines tool annotations: `readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`. Unclear whether Rivers' `[[mcp.tools]]` config accepts these.

**Requested:** Allow tool annotations to be declared in `[[mcp.tools]]` config, propagated to MCP clients via the standard `tools/list` response.

**Why CB needs it:**

CB's tool surface includes a wide spectrum:
- `cb_search_decisions` — readOnly
- `cb_log_decision` — non-destructive write (creates only)
- `cb_mark_complete` — non-destructive write (state transition, no data loss)
- `cb_reset` — destructive (requires approval, removes data)
- `cb_flag_task_oversized` — readOnly from CB's POV (creates an issue, but the issue is itself a record, not a destructive op)

CC's MCP client (and CD's, eventually) can use these annotations to surface confirmation prompts only on destructive tools, and to skip permission flows on readOnly tools. Without annotations, every tool is treated identically.

---

### P1.3 — Parameterized MCP resources via URI templates

**Current state:** Unclear how Rivers handles MCP resource URI templates (per MCP spec, resources can declare `uriTemplate` with named variables). CB's read resources need parameters: project_id, since timestamp, search query, task_id filter.

**Requested:** Confirm or implement `uriTemplate` resources where path/query variables map to DataView SQL parameters.

**Why CB needs it:**

Without templates, every read needs a fully-qualified URI per project. CB has 4 read resources × N projects = explosion in the resource list. With templates, CB declares one resource per resource-type and CC instantiates with parameters.

Example:
```toml
[[mcp.resources]]
uriTemplate = "cb://{project_id}/decisions{?since,query,limit}"
view = "decisions_list"
```
Maps to DataView `decisions_list` with parameters `project_id`, `since`, `query`, `limit`.

---

### P1.4 — MCP prompt arguments

**Current state:** CB has 3 MCP prompts (Tier 2). MCP spec allows prompts to declare `arguments`. Unclear whether Rivers' `[[mcp.prompts]]` config supports argument schemas.

**Requested:** Allow prompt argument schemas in `[[mcp.prompts]]` config.

**Why CB needs it:**

CB's prompts include things like `cb_decompose_sprint(sprint_id)` and `cb_review_findings(task_id)`. Without argument support, the prompts have to scrape context from the conversation, which is fragile.

---

### P1.5 — DataView write support without `introspect = false` workaround

**Current state:** Per decision G.1 (Tier 3), CB had to set `introspect = false` on `cb_db` because Rivers wraps DataView queries in `SELECT * FROM (…) AS _introspect LIMIT 0` at startup, which fails for UPDATE/INSERT/DELETE DataViews.

**Requested:** A cleaner per-view mechanism. Suggested:
```toml
[[dataviews]]
name = "insert_decision"
type = "mutation"   # skips introspection; runtime validates on first use
```
Or a `[[dataviews]] introspect = false` per-view flag.

**Why it matters:**

The current bundle-wide opt-out is too coarse. A bundle with mixed read/write DataViews can't get per-view introspection on the read ones. Also, `introspect = false` is undiscoverable — it's not in the failure mode error message; we found it from canary-sql.

---

### P1.6 — OTLP protobuf support

**Current state:** Per F.7 decision (2026-04-21), MVP only accepts OTLP-HTTP JSON. Protobuf encoding is not detected or decoded.

**Requested:** Add protobuf support to the OTLP ingest path.

**Why CB needs it:**

CC's OTLP exporter defaults to JSON, so we're fine for now. But other producers (third-party tools, CI exporters, future Rivers-native exporters) typically default to protobuf. Requiring everyone to flip to JSON to talk to a CB-tracked project is friction.

---

### P1.7 — Codecomponent handler tracing / automatic OTel spans

**Current state:** Unclear whether Rivers wraps handler invocations in OTel spans automatically.

**Requested:** Auto-emit a span per handler invocation (entry → SQL queries → response). Spans should include attributes for handler name, request size, response status, duration.

**Why CB needs it:**

CB wants to be self-monitoring. Decision-rate and tool-call-rate are existing CB-emitted metrics, but spans for handler execution would let CB profile its own bottlenecks without manual instrumentation. Also, when CB starts managing Rivers itself, those spans become real production telemetry.

---

### P1.8 — Schema introspection in Admin/dev tooling

**Current state:** Rivers admin (per the admin spec we have in-session) has some introspection but coverage is uneven.

**Requested:** Expose a stable introspection API showing all DataViews, codecomponent views, MCP tools/resources/prompts, datasources, and their relationships in the active bundle.

**Why CB needs it:**

CB's wizard ("turn on modules") needs to know what's available in the current Rivers deployment. CB's portal admin section wants to show "what's configured vs what's running." Hand-maintaining a separate manifest is duplication.

---

## P2 — Quality of life

### P2.1 — Live-reload for codecomponent changes

**Current state:** Editing a handler requires restarting the bundle. Especially painful for the development loop where small handler changes need quick verification.

**Requested:** Hot-reload codecomponent views on file change in dev mode.

---

### P2.2 — Bulk handler invocation / batched MCP tool calls

**Current state:** MCP spec calls are one-at-a-time. CB's migration tooling (importing legacy tasks.md content into WIP) and bulk operations (closing all WIP from a retired sprint) would benefit from batching.

**Requested:** Either (a) a Rivers extension to MCP allowing batch tool calls in a single request, or (b) handler-side support for array inputs without forcing CB to hand-roll batch wrappers.

---

### P2.3 — Multi-bundle MCP federation

**Current state:** MCP exposure is per-bundle. CB has cb-service (data + MCP) and cb-main (SPA + admin); cb-main currently doesn't have MCP because the data lives in cb-service.

**Requested:** Allow cb-main to declare a federated MCP surface that re-exports selected cb-service tools/resources, possibly with auth/authz layered on top.

**Why CB might want it:**

cb-main is the human admin plane. If cb-main can re-export cb-service's MCP with admin-only annotations, the admin's MCP client gets one server URL instead of two.

---

### P2.4 — Migration tooling (framework-level)

**Current state:** Rivers projects appear to roll their own SQL migrations. CB has its own migration scripts in `bundle/cb-service/migrations/`.

**Requested:** A Rivers-recommended migration story (versioned, ordered, idempotent, transactional). Could be a thin wrapper around an existing tool (sqlx migrate, dbmate, etc.).

**Why we'd benefit:**

CB's schema is going to evolve aggressively as the pivot rolls out (new tables for `pending_actions`, `rules`, `groups`, `wip_module_link`, `decomposer_modules`, etc.). A standard migration tool reduces our exposure to migration bugs. Rivers' broader user base would benefit similarly.

---

### P2.5 — Schema-aware DataView parameter coercion

**Current state:** DataView parameters arrive as strings from the query string. Handlers coerce manually.

**Requested:** Let the DataView declare parameter types (`integer`, `timestamp`, `enum`) and have Rivers coerce + validate before invoking the query.

---

### P2.6 — MCP elicitation support

**Current state:** Unclear. MCP spec includes `elicitation/create` for tools that need to ask the user for input mid-call.

**Requested:** Support elicitation in tool handlers, so a tool can pause execution and request user input (e.g., a confirmation, a missing field, a disambiguation choice).

**Why CB might want it:**

`cb_reset` could elicit "type the project name to confirm" before proceeding. `cb_add_task` could elicit clarification if CC's task description is ambiguous. Right now CB has to pre-validate everything because it can't ask back.

---

### P2.7 — Cursor-based pagination on DataViews

**Current state:** DataView pagination uses LIMIT/OFFSET. Performance degrades on large tables as offset grows.

**Requested:** Support cursor-based pagination (`limit` + `after_cursor` keyed on a unique sortable column).

**Why it matters:**

CB's decision log will accumulate. After a year of CC working a project, decisions could be in the tens of thousands. Cursor pagination is the right primitive for "show me the next 20 decisions after the last one I saw."

---

### P2.8 — Audit log / framework event stream

**Current state:** Rivers does not expose a framework-wide audit event stream that we know of.

**Requested:** A Rivers-emitted event stream (handler invocations, MCP tool calls, DataView reads, auth resolutions) that bundles can subscribe to.

**Why CB might want it:**

CB has its own audit needs (decision log, telemetry). A complementary framework-level audit stream would let CB cross-reference: "the user said they did X via the API; did Rivers actually receive that call?" For debugging cross-system issues, this is gold.

Privacy controls obvious — bundles should opt in, sensitive payloads redacted.

---

### P2.9 — DataView composability / view-of-views

**Current state:** Unclear whether DataViews can reference other DataViews (views built on views).

**Requested:** Allow DataViews to compose. A "decisions with task names" view should be able to reference the base "decisions" view + the base "tasks" view rather than duplicating SQL.

---

### P2.10 — Better error surfacing for MCP/DataView misconfiguration

**Current state:** Bundle config errors sometimes surface as opaque messages ("app blocked — missing drivers" was the example we hit on Tier 3, where the actual issue was using `driver = "http"` which doesn't exist).

**Requested:** When a config references a non-existent driver/view/datasource, error messages should name the offending field and suggest valid alternatives ("driver 'http' not recognized; did you mean to use a `[[services]]` block?").

---

## What's working well (acknowledgments)

We want to be clear about what's good in Rivers today, since this document focuses on gaps.

- **Fast turnaround on bug fixes.** Two of CB's blockers in the last cycle shipped same-day in 0.55.2.
- **Handler ergonomics.** Codecomponent handlers in TypeScript with direct DataSource access are a clean primitive.
- **DataView layer.** Maps cleanly to CB's read endpoints. The query → MCP resource → CC pipeline is solid for reads.
- **Auth flexibility.** Handler-side `resolveApiKey()` lets CB use its own per-project API key model without fighting the framework.
- **Bundle layout.** In-tree bundles (no apphome staging) match how we want to develop.

---

## Summary of asks

| Priority | Item | Status |
|----------|------|--------|
| P0.1 | Codecomponent-backed MCP tools | Pivot blocker |
| P0.2 | Tool schema from TS types | Pivot blocker |
| P0.3 | Auth context to MCP handlers | Pivot blocker |
| P1.1 | MCP resource subscriptions | High value |
| P1.2 | MCP tool annotations | High value |
| P1.3 | URI-template MCP resources | High value |
| P1.4 | MCP prompt arguments | High value |
| P1.5 | Per-view introspection control | High value |
| P1.6 | OTLP protobuf | High value |
| P1.7 | Auto-OTel spans on handlers | High value |
| P1.8 | Stable introspection API | High value |
| P2.1 | Codecomponent hot-reload | QoL |
| P2.2 | Batch tool calls | QoL |
| P2.3 | Multi-bundle MCP federation | QoL |
| P2.4 | Framework migration tooling | QoL |
| P2.5 | Typed DataView parameters | QoL |
| P2.6 | MCP elicitation | QoL |
| P2.7 | Cursor pagination | QoL |
| P2.8 | Framework audit stream | QoL |
| P2.9 | DataView composability | QoL |
| P2.10 | Better config error messages | QoL |

---

## What we'll do in the meantime

For P0 items, CB will continue to:
- Use REST for write paths from CC (split surface; accepted cost)
- Keep the 4 read-only MCP DataView resources + 3 prompts shipped in Tier 2
- Maintain handler-side `resolveApiKey()` for both REST and any future MCP path
- Document the gap in CB's own decision log

We are not building a temporary REST-proxy MCP server. We'd rather wait for the right primitive than maintain a bridge we'll throw away.

---

## Contact

CB maintainer: [Paul]
Repository: [link]
Existing decision log entries referencing Rivers: see `changedecisionlog.md` (G.1, Tier 2, Tier 3, F.7).
