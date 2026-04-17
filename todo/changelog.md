# Rivers Filesystem Driver — Implementation Changelog

### 2026-04-16 — OperationDescriptor framework baseline
- Files: crates/rivers-driver-sdk/src/{operation_descriptor.rs,traits.rs,lib.rs}
- Summary: new types (OpKind, OperationDescriptor, Param, ParamType) + opt-in DatabaseDriver::operations() method; all existing drivers build and test without modification.
- Spec: rivers-filesystem-driver-spec.md §2.
- Test delta: +1016 passing (0 failures, 17 ignored), backward compatible.

### 2026-04-17 — Filesystem driver + Direct dispatch typed proxy landed
- **Crates touched:** `rivers-driver-sdk`, `rivers-drivers-builtin`, `rivers-runtime`, `riversd`.
- **Scope:**
  - Eleven filesystem operations: readFile, readDir, stat, exists, find, grep, writeFile, mkdir, delete, rename, copy (spec §6).
  - Chroot sandbox with startup-time root canonicalization, per-op path validation, and symlink rejection — walking the pre-canonical path (spec §5).
  - `max_file_size` + `max_depth` connection-level limits (spec §8.4).
  - `DatasourceToken` converted from newtype struct to enum with `Pooled` and `Direct` variants (spec §7); `resolve_token_for_dispatch` emits `Direct` for filesystem, `Pooled` for all other drivers.
  - V8 typed-proxy pipeline: `TASK_DIRECT_DATASOURCES` thread-local, `catalog_for(driver)` lookup, `Rivers.__directDispatch` host fn with Option-B auto-unwrap, JS codegen from `OperationDescriptor` with ParamType guards + defaults (spec §3).
- **Canary:** `canary-bundle/canary-filesystem/` — 5 TestResult endpoints (CRUD round-trip, chroot escape, exists+stat, find+grep, arg validation). `riverpackage validate canary-bundle`: 0 errors. Live fleet run pending deploy (Task 32).
- **Docs:**
  - `docs/arch/rivers-feature-inventory.md` §6.1 + §6.6.
  - `docs/guide/tutorials/datasource-filesystem.md` (new, 197 lines, all 11 ops + chroot + limits + error table).
- **Tests:** ~85 new tests across driver ops, chroot enforcement, typed-proxy codegen, end-to-end V8 round-trip, and canary handlers. Scoped sweep of touched crates: 706/706 passing (sdk 67, drivers-builtin 140, runtime 187, riversd 312). Pre-existing workspace-level failures in live-infra tests (postgres/mysql/redis at 192.168.2.x) and two broken benches (`cache_bench`, `dataview_engine_tests`) are unrelated to this branch — verified via `git stash` on baseline.
- **Commits:** 29 commits from `f2c6db5` through `ad8819b` on `feature/filesystem-driver`.
