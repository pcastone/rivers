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
