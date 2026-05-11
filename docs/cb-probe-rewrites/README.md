# cb-rivers-feature-validation — Rivers regression suite for CB-relevant features

Filed: 2026-05-09 · Last migration: 2026-05-10
Reporter: Circuit Breaker team
Targeted at: Rivers v0.60.12 and onward

A minimal Rivers bundle that probes every framework behavior CB depends on.
`run-probe.sh` produces PASS / FAIL / EXPECTED-FAIL output per case so the
Rivers team can verify a release against CB's contract without re-deriving
the test matrix each time.

## Why

CB has hit four meaningful Rivers behaviors in the last week:

- **v0.58.0** — P1.13 (capability propagation through MCP `view = "..."` dispatch) discovered as broken
- **v0.60.11** — transient regression: a new "only one MCP view per app" rule broke CB's role-split
- **v0.60.12** — P1.13 fixed AND v0.60.11 regression reverted

Each release cycle we re-derive what works. This suite makes that one command instead.

The cases in this bundle are derived from concrete CB issues, every one with its own standalone handoff doc in `../`:

- `case-rivers-mcp-view-capability-propagation.md` (P1.13 — RESOLVED v0.60.12)
- `case-rivers-scheduled-task-primitive.md` (P1.14 — pending Rivers Sprint 2026-05-09 Track 3)
- Plus P1.9 / P1.10 / P1.11 / P1.12 in `cb-rivers-feature-request.md`

## Cases (post-2026-05-10 migration)

| # | What it proves | Sentinel for | v0.60.12 |
|---|---|---|---|
| **A** | Multiple MCP views per app coexist | regression (broke v0.60.11) | ✅ PASS |
| **B** | MCP `view = "<name>"` dispatches to a codecomponent REST view | sentinel since v0.58 | ✅ PASS |
| **C** | Codecomponent invoked via MCP can call `Rivers.db.query` | P1.13 RESOLVED | ✅ PASS |
| **D** | Bearer token reaches codecomponent via `ctx.session` `{kind: "bearer", token: ...}` | sentinel | ✅ PASS |
| **E** | DataView optional params bind as `''` (empty string) not `NULL`; documented idiom needed | doc gotcha | ⚠️ depends on intent — see below |
| **F** | Named guards on MCP views (`guard_view = "..."`, both allow + deny branches) | **P1.10 RESOLVED** | ✅ PASS |
| **G** | Bearer-token auth via §11.5 named-guard recipe (no/wrong/correct token) | **P1.12 closed-as-superseded** | ✅ PASS |
| **H** | `path_params` reachable in MCP-dispatched codecomponent via `args.path_params` | **P1.9 RESOLVED** | ✅ PASS |
| **I** | Scheduled-task primitive (`view_type = "Cron"`) | **P1.14 RESOLVED** | ✅ PASS (v0.61.0+) |
| **J** | Per-view response-header injection in config (`response_headers` flat) | **P1.11 RESOLVED** | ✅ PASS |

When an EXPECTED-FAIL case starts passing, the runner labels it 🎉 NEWLY PASSING — that's the signal to close the corresponding Rivers ask.

**Migration notes (2026-05-10):** F, G, H, J flipped from EXPECTED FAIL to
PASS after aligning the probe to canonical v0.60.12 shapes. See
`../../rivers-pub/docs/cb-probe-v0.60.12-migration.md` for the side-by-side
diffs and rationale.

- **F** uses `guard_view = "name"` (per `rivers-mcp-view-spec.md` §13.5),
  not `guard = "name"` overload. `guard: bool` and `guard_view: string`
  are distinct fields.
- **G** is no longer "test for `auth = \"bearer\"` rejection" — that ask
  was closed-as-superseded. G now exercises the §11.5 named-guard bearer
  recipe directly. Once Rivers Track 2 (validator hardening) lands,
  `auth = "bearer"` will produce a clean S005 — no need to probe for it.
- **H** handler reads `args.path_params` (top-level, MCP dispatch surface
  per `rivers-mcp-view-spec.md` §10.4) before falling back to
  `ctx.request.path_params` (REST). The MCP view path was templated as
  `/case-h/{id}/_mcp` so `MatchedRoute.path_params` populates.
- **J** uses `[api.views.X.response_headers]` (flat, per
  `rivers-view-layer-spec.md` §5.4), not `[api.views.X.response.headers]`.

## Running

```bash
# 1. Set up the SQLite probe DB (idempotent)
./setup-db.sh

# 2. Validate the bundle structure (F/G/I/J fragments are spliced in by
#    run-probe; the bundle should validate clean as-is on v0.60.12)
riverpackage validate .

# 3. Start riversd with this bundle (bundle_path = absolute path to this dir).

# 4. Run the probes
./run-probe.sh                      # full suite
./run-probe.sh --validate-only      # only the F/G/I/J fragment splices
./run-probe.sh --base http://...    # custom base URL
```

## Layout

```
cb-rivers-feature-validation-bundle/
├── README.md                       — this file
├── manifest.toml                   — bundle manifest (apps = ["app"])
├── setup-db.sh                     — create probe.db with seed rows
├── run-probe.sh                    — exercise every case + summary
├── app/
│   ├── manifest.toml               — app manifest
│   ├── resources.toml              — sqlite probe_db datasource
│   ├── app.toml                    — cases A, B, C, D, E, H
│   ├── data/probe.db               — created by setup-db.sh (gitignored)
│   └── libraries/handlers/cases.ts — codecomponent for B, C, D, F-guard, G-guard, H, I-tick
└── expected-fail/
    ├── F-named-guard.toml          — splice → validate PASSES (P1.10 shipped)
    ├── G-auth-bearer.toml          — splice → validate PASSES (P1.12 §11.5 recipe)
    ├── I-cron-view-type.toml       — splice → validate FAILS (P1.14 pending)
    └── J-response-headers.toml     — splice → validate PASSES (P1.11 shipped)
```

(The `expected-fail/` dirname is now historical for F/G/J — kept for
compatibility with run-probe.sh's splicing logic. Only I is genuinely
expected-fail today.)

## What "PASS" looks like on v0.61.0+

When Rivers Sprint 2026-05-09 Track 3 ships (P1.14), Case I flips to ✅ PASS
and the bundle reports zero EXPECTED FAIL. Sample summary:

```
═══ Summary ═══
  ✅ pass:             9     (A, B, C, D, E, F, G, H, I, J)
  ❌ fail:             0
  ⏳ expected-fail:    0
  🎉 newly-passing:    5     (F, G, H, I, J — flipped from migration + Track 3)
```

## What "PASS" looks like on v0.60.12 post-migration (Track 1 only, Track 3 not yet shipped)

```
═══ Live cases ═══
── Case A — multiple MCP views per app coexist ──
  ✅ two distinct MCP views accept initialize (sids differ)
── Case B — MCP view = ... routes to codecomponent ──
  ✅ codecomponent invoked through MCP view= dispatch
── Case C — Rivers.db.query reachable through MCP dispatch (P1.13) ──
  ✅ Rivers.db.query succeeded through MCP-dispatched codecomponent
── Case D — bearer reaches codecomponent via ctx.session ──
  ✅ ctx.session.kind == 'bearer' and token surfaces
── Case E — DataView optional-param empty-string-vs-NULL gotcha ──
  ✅ documented empty-string idiom needed for optional = comparisons
── Case H — path_params reachable via MCP dispatch (P1.9) ──
  ✅ args.path_params populated; source=args

═══ Splice-validate cases ═══
── Case F — P1.10 named guards on MCP views ──
  ✅ guard allow path returns tool body
  ✅ guard deny path returns HTTP 401
── Case G — P1.12 §11.5 bearer recipe ──
  ✅ no token  → 401
  ✅ wrong token → 401
  ✅ correct token → 200
── Case I — P1.14 scheduled-task primitive ──
  ⏳ EXPECTED FAIL — P1.14 view_type = "Cron" not yet shipped
── Case J — P1.11 per-view response headers ──
  ✅ Deprecation, Sunset, Link headers present on response

═══ Summary ═══
  ✅ pass:             8
  ❌ fail:             0
  ⏳ expected-fail:    1     (I — P1.14 still pending)
  🎉 newly-passing:    4     (F, G, H, J — flipped from migration)
```

When Rivers Sprint 2026-05-09 Track 3 ships P1.14, Case I flips to ✅ PASS
and the bundle reports zero EXPECTED FAIL.

## Contact

CB maintainer: paul.castone@gmail.com

Existing handoff docs: `../case-rivers-*.md` and `../../arch/cb-rivers-feature-request.md`.
Migration guide (Rivers side): `rivers-pub/docs/cb-probe-v0.60.12-migration.md`.
