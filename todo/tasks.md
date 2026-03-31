# Tasks — Cleanup & Commit

> **Branch:** `largefiles`

---

## Validated: Rust Documentation Epic

All 27 sprints verified complete (2026-03-30):
- 23 lib crates + 4 binary crates all have `#![warn(missing_docs)]`
- All 27 crates have crate-level `//!` documentation
- Spot-checked sprints 3, 4, 7, 11, 23, 25, 26, 27 — docs present and correct
- Epic tracker (`todo/epic-rust-docs.md`) checkboxes need updating but work is done

---

## Completed: Example & Tutorial Validation

- [x] **T5.1** All example bundles pass `riversctl validate` — 7 bundles, 18 apps, all pass
  - Fixed: kafka-service (dataview placement, missing view_type), rabbitmq-service, nats-service, ldap-service (same pattern), chat-app (missing handler section)
- [x] **T5.2** All JS handler examples have correct ctx/Rivers API usage
  - Fixed: V8 engine now injects `ctx.ws` from args (matching `ctx.request` pattern) so chat.js WebSocket hooks work correctly
  - Verified: todos.js, metrics.js, chat.js all use correct API surface
- [x] **T5.3** N/A — no tutorial documents exist; example configs validated in T5.1

---

## Pending: Commit & Branch Cleanup

- [ ] **T6.1** Stage and commit all changes on `largefiles` branch
