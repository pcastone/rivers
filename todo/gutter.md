# Moved to tasks.md — 2026-04-30

## 2026-05-08 — Datasource wiring: WebSocket + SSE dispatch parity

CB-P1.13 fixed `wire_datasources` for the MCP `view=` dispatch path. The
same gap exists in two more sites:

- `crates/riversd/src/websocket.rs:497` and `:546`
- `crates/riversd/src/sse.rs:424`

Both call `task_enrichment::enrich` only — they don't call
`task_enrichment::wire_datasources`, so `Rivers.db.execute('<ds>', ...)`
in a WS or SSE handler will throw `CapabilityError`. Symptoms haven't
been reported, likely because most WS/SSE handlers lean on
`ctx.dataview(...)` rather than direct DB calls. Fix is mechanical: drop
in the `wire_datasources` call before `enrich` at each site, mirror REST
ordering. Track separately so the P1.13 PR stays narrow and CB can pin
to a specific version when their MCP `view=` flows go green.
