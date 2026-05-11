# CB feature-validation probe — v0.60.12 migration guide

**Audience:** Circuit Breaker team maintaining `cb-rivers-feature-validation-bundle`.
**Targets:** Rivers v0.60.12 (current `main`).
**Related sprint:** `todo/tasks.md` → "Sprint 2026-05-09 — CB unblock", Track 1.

---

## TL;DR

The current probe bundle's EXPECTED-FAIL cases for **P1.9, P1.10, P1.11**
will keep flagging EXPECTED FAIL forever — not because the asks are
unresolved, but because the probe encodes config shapes that don't match
what shipped:

| Ask | Probe writes | v0.60.12 actually accepts | Result on probe |
|---|---|---|---|
| **P1.9** | TS handler reads `ctx.request.path_params` | reads `args.path_params` (top-level) | runtime FAIL — wrong field path |
| **P1.10** | `guard = "case_f_advisor_guard"` (string overload of `guard`) | `guard_view = "case_f_advisor_guard"` (new field) | TOML parse error: `guard` is bool-only |
| **P1.11** | `[api.views.X.response.headers]` (nested under `response`) | `[api.views.X.response_headers]` (flat) | silent PASS — `response` is unknown key, ignored |
| **P1.12** | `auth = "bearer"` (EXPECTED FAIL) | superseded — closed-as-not-shipping | silent PASS today, hard-rejected after Track 2 |
| **P1.14** | `view_type = "Cron"` (EXPECTED FAIL) | not yet shipped | confirmed FAIL — keep as EXPECTED FAIL until Track 3 ships |

This doc gives the canonical shape for each, with side-by-side diffs you
can lift directly into the probe bundle.

---

## P1.9 — `path_params` via MCP dispatch

**Spec ref:** [`rivers-mcp-view-spec.md` §10.4](../docs/arch/rivers-mcp-view-spec.md)
(Codecomponent Handler Args).
**Shipped in:** [v0.60.x #101](https://github.com/pcastone/rivers/pull/101).

### Where it lives in the handler

When MCP dispatches a tool with `view = "..."`, the codecomponent's args
object is shaped:

```json
{
  "request":     { /* tool arguments — same as MCP tools/call `arguments` */ },
  "session":     { /* resolved caller identity, or null if no auth */ },
  "path_params": { /* matched URL path variables, possibly empty */ }
}
```

`path_params` is **always an object** (never null). It mirrors the
matched route of the **MCP view**, not the inner `view = "..."` referent.
That means: to exercise P1.9, the **MCP view's** path must have a
template segment like `/{id}`. Templating only the inner REST view does
nothing because MCP dispatch resolves through the MCP route, which is
where `MatchedRoute.path_params` is captured.

### Required probe changes

**`app/libraries/handlers/cases.ts`** — `caseH` reads from the wrong place:

```diff
 export async function caseH(ctx: Ctx): Promise<void> {
-    const pp = ctx.request.path_params ?? {};
+    // P1.9 (CB-P1.9, shipped v0.60.x): MCP dispatch threads matched
+    // path-segment values onto `args.path_params` (top-level), not
+    // `ctx.request.path_params`. REST dispatch puts them on
+    // `ctx.request.path_params`. The probe must read both surfaces
+    // and report which populated.
+    const fromArgs = (typeof args !== "undefined" && (args as any).path_params) || {};
+    const fromCtx  = ctx.request.path_params ?? {};
+    const pp = Object.keys(fromArgs).length > 0 ? fromArgs : fromCtx;
     ok(ctx, {
         case: "H",
-        path_params: pp,
+        path_params: pp,
+        source: Object.keys(fromArgs).length > 0 ? "args" :
+                Object.keys(fromCtx).length > 0  ? "ctx"  : "missing",
         marker: Object.keys(pp).length > 0 ? "path-params-OK" : "path-params-MISSING",
     });
 }
```

**`app/app.toml`** — the MCP view at `case-h/_mcp` is non-templated, so
P1.9 cannot fire on it. Template the MCP path:

```diff
 [api.views.case_h_mcp]
-path         = "case-h/_mcp"
+# P1.9: MCP route templated so path_params actually populates.
+path         = "case-h/{id}/_mcp"
 method       = "POST"
 view_type    = "Mcp"
 auth         = "none"
```

Then the probe should call `POST /case-h/PROJ-42/_mcp` with the MCP
JSON-RPC envelope and assert `path_params.id === "PROJ-42"` from the
handler's response.

**`run-probe.sh`** — the MCP-route call needs a real {id} segment:

```diff
- curl -X POST "$BASE/case-h/_mcp" -H 'Content-Type: application/json' -d "$mcp_envelope"
+ curl -X POST "$BASE/case-h/PROJ-42/_mcp" -H 'Content-Type: application/json' -d "$mcp_envelope"
```

After these three changes, Case H flips from ⏳ EXPECTED FAIL to 🎉 NEWLY
PASSING.

---

## P1.10 — Per-view named guards

**Spec ref:** [`rivers-mcp-view-spec.md` §13.5](../docs/arch/rivers-mcp-view-spec.md)
+ MCP-5; cross-cutting in
[`rivers-view-layer-spec.md` §14](../docs/arch/rivers-view-layer-spec.md).
**Shipped in:** [#103](https://github.com/pcastone/rivers/pull/103) initial,
extended uniformly across REST/WS/SSE/MCP in [#107](https://github.com/pcastone/rivers/pull/107),
chains lifted up to depth 5 in [#109](https://github.com/pcastone/rivers/pull/109).

### Field name + shape

The shipped field is `guard_view = "name"` (a sibling of the existing
boolean `guard` — they are **distinct** fields):

- `guard = true`  → marks **the** server-wide auth gate. Exactly one
  view per server may set this. Pre-existing.
- `guard_view = "name"`  → **per-view** named guard. References another
  view in the same app whose codecomponent runs as a pre-flight before
  this view dispatches. Returns `{ allow: bool }`; `allow: true` proceeds,
  anything else → HTTP 401.

The probe attempted to overload `guard` as a string, which still parses
strictly as bool — that's the TOML parse error you observe.

### Required probe changes

**`expected-fail/F-named-guard.toml` → live case `case-F-named-guard.toml`**
(or splice into `app/app.toml`):

```toml
# Case F — Per-view named guards (P1.10, shipped v0.60.x).
#
# A protected MCP view declares `guard_view = "..."` referencing a
# sibling REST view whose codecomponent returns { allow: bool }.
# Validator (X014) rejects: missing target, non-codecomponent target,
# self-reference, mutual recursion, chain depth > 5.

# The guard target — must be a codecomponent REST view in the same app.
[api.views.case_f_advisor_guard]
path      = "case-f/_guard"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.case_f_advisor_guard.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/cases.ts"
entrypoint = "caseFGuard"
resources  = []

# The protected MCP view — references the guard by name.
[api.views.case_f_named_guard]
path       = "case-f/named-guard"
method     = "POST"
view_type  = "Mcp"
auth       = "none"
guard_view = "case_f_advisor_guard"

[api.views.case_f_named_guard.handler]
type = "none"

[api.views.case_f_named_guard.tools.case_f_tool]
dataview    = "case_a_dv"
description = "Case F: only fires if guard returns { allow: true }."
hints       = { read_only = true }
```

**`app/libraries/handlers/cases.ts`** — add the guard handler:

```typescript
// Case F guard — returns allow=true only if X-Case-F-Allow header is "yes".
// Probe sends the header (PASS path) and omits it (DENY path) to exercise both branches.
export async function caseFGuard(ctx: Ctx): Promise<void> {
    const flag = ctx.request.headers?.["x-case-f-allow"] ?? "";
    ok(ctx, { allow: flag === "yes" });
}
```

**`run-probe.sh`** — Case F now has two sub-cases:

```bash
# F.1 — guard allows: header present → tool result returned
# F.2 — guard denies: header absent → HTTP 401, no tool body
```

Result: ⏳ EXPECTED FAIL → 🎉 NEWLY PASSING. The case becomes a positive
regression sentinel for guard pass/deny semantics.

---

## P1.11 — Per-view static response headers

**Spec ref:** [`rivers-view-layer-spec.md` §5.4](../docs/arch/rivers-view-layer-spec.md).
**Shipped in:** [#102](https://github.com/pcastone/rivers/pull/102).

### Field path + shape

The shipped table is `[api.views.X.response_headers]` — flat, no
`response.` parent table. Headers are appended to every response;
**handler-set headers win** when the same name is set on both sides.

### Validation gates already in place

- Header names must match RFC 7230 token grammar (alphanumerics + `-`).
- Values must be ASCII-printable (`\x20`–`\x7E`).
- Four names are framework-managed and rejected with `S005`:
  `Content-Type`, `Content-Length`, `Transfer-Encoding`, `Mcp-Session-Id`.

### Required probe changes

```diff
 [api.views.case_j_response_headers]
 path      = "case-j/response-headers"
 method    = "GET"
 view_type = "Rest"
 auth      = "none"

-[api.views.case_j_response_headers.response.headers]
+[api.views.case_j_response_headers.response_headers]
 "Deprecation" = "true"
 "Sunset"      = "Wed, 01 Apr 2026 00:00:00 GMT"
+"Link"        = "</api/v2/replacement>; rel=\"successor-version\""

 [api.views.case_j_response_headers.handler]
 type       = "codecomponent"
 language   = "typescript"
 module     = "libraries/handlers/cases.ts"
 entrypoint = "caseB"
 resources  = []
```

**`run-probe.sh`** — assert the headers actually arrive on the response:

```bash
curl -sD - "$BASE/case-j/response-headers" -o /dev/null \
  | grep -iE '^(deprecation|sunset|link):' \
  | wc -l   # expect 3
```

Result: silent ⏳ EXPECTED FAIL → 🎉 NEWLY PASSING with positive header
assertions. Note: once Track 2 (validator hardening) lands, the
**unrewritten** probe's `[api.views.X.response.headers]` will start
emitting `S005`-or-equivalent for the unknown `response` key — the
silent pass goes away. So this rewrite is on the critical path before
Track 2.

---

## P1.12 — `auth = "bearer"` (closed as superseded)

**Spec ref:** [`rivers-auth-session-spec.md` §11.5](../docs/arch/rivers-auth-session-spec.md).
**Closed in:** [#104](https://github.com/pcastone/rivers/pull/104).

### Why this won't ship

`auth = "bearer"` as a first-class view mode would freeze a single
lookup table, hash algorithm, identity-claims schema, and audit shape
into the framework. The named-guard primitive (P1.10) gives operators
strictly more flexibility — same enforcement boundary, all that policy
held in their codecomponent. So Rivers ships **no** new `auth` mode;
the sanctioned answer is the §11.5 recipe.

### Required probe changes

Replace `expected-fail/G-auth-bearer.toml` with a **live case** using
the named-guard recipe — Case G becomes a positive sentinel for the
bearer pattern, not a fail probe.

```toml
# Case G — Bearer-token auth via named guard (P1.12 closed-as-superseded
# by P1.10). The §11.5 recipe: a small codecomponent attached as
# `guard_view` validates Authorization: Bearer <token>.

# The bearer-validating guard.
[api.views.case_g_bearer_guard]
path      = "case-g/_guard"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.case_g_bearer_guard.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/cases.ts"
entrypoint = "caseGBearerGuard"
resources  = []

# The protected route — references the guard.
[api.views.case_g_protected]
path       = "case-g/protected"
method     = "POST"
view_type  = "Rest"
auth       = "none"
guard_view = "case_g_bearer_guard"

[api.views.case_g_protected.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/cases.ts"
entrypoint = "caseB"
resources  = []
```

**`app/libraries/handlers/cases.ts`** — add the bearer guard:

```typescript
// Case G — bearer guard recipe (§11.5 of rivers-auth-session-spec.md).
// Probe-only: hardcoded acceptable token. Real CB code hashes against api_keys.
export async function caseGBearerGuard(ctx: Ctx): Promise<void> {
    const auth = (ctx.request.headers?.["authorization"] ?? "").trim();
    const prefix = "Bearer ";
    if (!auth.startsWith(prefix)) return ok(ctx, { allow: false });
    const token = auth.slice(prefix.length).trim();
    ok(ctx, { allow: token === "test-bearer-value-12345" });
}
```

**`run-probe.sh`** — Case G now has three sub-cases:

```bash
# G.1 — no Authorization header → 401
# G.2 — Authorization with wrong token → 401
# G.3 — Authorization with correct token → 200, handler returns sentinel
```

Result: silent ⏳ EXPECTED FAIL → 🎉 PASS as a closed-as-superseded
positive sentinel.

---

## P1.14 — Scheduled-task primitive (still pending)

**Spec ref:** `case-rivers-scheduled-task-primitive.md` (CB filing 2026-05-09).
**Status:** Track 3 of this sprint. **Not yet shipped.**

Keep `expected-fail/I-cron-view-type.toml` as-is until Track 3 lands.
After Track 2 (validator hardening) ships, the failure mode shifts from
"unknown key 'schedule'" + missing-field errors to a clean `S005:
view_type 'Cron' not in {Rest,Mcp,WebSocket,Sse,Streaming}`. That's
still EXPECTED FAIL — same intent, more useful message.

When Track 3 ships:

```toml
[api.views.case_i_cron]
view_type        = "Cron"
schedule         = "*/5 * * * *"          # cron OR
# interval_seconds = 300                  # interval (mutually exclusive)
overlap_policy   = "skip"

[api.views.case_i_cron.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/cases.ts"
entrypoint = "caseICronTick"
resources  = []
```

Probe should write a sentinel into a known table on each tick and
poll for it.

---

## Validation against v0.60.12

After applying the rewrites above, re-run from the bundle root:

```bash
./setup-db.sh
riverpackage validate .
./run-probe.sh
```

Expected `run-probe.sh` summary on v0.60.12 with the migration applied:

```
═══ Summary ═══
  ✅ pass:             8     (A, B, C, D, E, F, G, H, J)
  ❌ fail:             0
  ⏳ expected-fail:    1     (I — P1.14 still pending)
  🎉 newly-passing:    4     (F, G, H, J — flipped from migration)
```

Once Track 3 ships, Case I flips to ✅ PASS and the bundle reports zero
EXPECTED FAIL.

---

## Rivers-side follow-up: validator hardening (Track 2)

The two silent passes the probe accidentally found —
`auth = "bearer"` and `[api.views.X.response.headers]` — both come from
permissive deserialization of `auth`/`view_type` and `[api.views.*]`
unknown-key warnings. Track 2 of this sprint tightens both:

- `auth ∈ {"none","session"}` → `S005` on anything else (incl. `"bearer"`).
- `view_type ∈ {"Rest","Mcp","WebSocket","Sse","Streaming"}` → `S005` on
  anything else (incl. `"Cron"` until Track 3 adds it).

This means the migration above should land **before** Track 2 ships,
or the unrewritten probe will produce noisier failures than necessary
between the Track 2 patch bump and the migration PR.

---

## Contacts

- Rivers maintainer: paul.castone@gmail.com
- Sprint plan: [`todo/tasks.md`](../todo/tasks.md) → "Sprint 2026-05-09 — CB unblock"
- Decision log: [`todo/changedecisionlog.md`](../todo/changedecisionlog.md) (entries CB-PROBE-D1 .. D4)
