# Filesystem Driver — Completion Design

**Date:** 2026-04-20
**Branch:** feature/filesystem-driver
**Status:** Approved for implementation
**Spec:** docs/arch/rivers-filesystem-driver-spec.md v1.1

---

## Scope

Close all outstanding gaps between the v1.0 filesystem driver implementation and its spec,
completing the FUP items captured at sign-off. Does not add new operations or new drivers.
After this cycle the driver is merge-ready and canary-verified.

---

## Items In Scope

### Item 1 — `readDir` return type (G4) — spec amendment

**Problem:** Spec §6.2 says `readDir` returns `string[]`. Implementation returns `[{name: string}]`
row objects (QueryResult row format). The canary handler compensates with `.map(e => e.name)`.
These disagree.

**Decision:** Amend the spec to match the implementation, not the other way around. Changing the
driver would require unwrapping row results inside the V8 bridge for a single operation — the row
object model is correct for the driver contract. The spec example was written against the ideal
JS surface before the row object shape was finalized.

**Change:** Update spec §6.2 table entry for `readDir` to `{ name: string }[]`. Update §6.4 prose
and JS example to show `entries[0].name`. Remove the `.map(e => e.name)` workaround comment in
the canary handler (it becomes the correct usage pattern).

---

### Item 2 — `ping()` implementation

**Problem:** `FilesystemConnection::ping()` returns `NotImplemented`. Spec §4.4 says ping
validates the root directory still exists.

**Change:** Implement `ping()` to check `self.root.is_dir()`. Return `Ok(())` if true,
`DriverError::Connection` if the root has been removed or is no longer a directory.

---

### Item 3 — `max_file_size` / `max_depth` config plumbing (FUP-3)

**Problem:** `connect()` ignores `ConnectionParams.extra` and hardcodes both constants. Users
cannot configure custom limits in their bundle config.

**Change:** In `FilesystemDriver::connect()`, read `params.options.get("max_file_size")` and
`params.options.get("max_depth")`, parse as `u64` / `usize`, fall back to defaults if absent or
unparseable (log a warning on parse failure). Spec §8.4 documents the key names.

`ConnectionParams.options` is `HashMap<String, String>` — string parsing is the correct approach.

---

### Item 4 — ISO-8601 timestamps in `stat` (FUP-1)

**Problem:** `stat` emits `mtime`/`atime`/`ctime` as epoch-seconds decimal strings. ISO-8601 is
more ergonomic for JS handler authors (`new Date(mtime)` works with ISO strings directly).

**Change:** Add `chrono` to `rivers-drivers-builtin` dependencies. In `ops::stat`, format the
three timestamps as RFC 3339 UTC strings (`2026-04-20T12:00:00Z`). Update spec §6.2 stat row
description to note `string (ISO 8601 / RFC 3339 UTC)`.

---

### Item 5 — Spec §3.2 naming amendment (G1)

**Problem:** Spec §3.2 example shows `__rivers_datasource_dispatch(...)` as the generated
dispatch call. Implementation generates `Rivers.__directDispatch(...)`. Inconsistency confuses
anyone reading the spec alongside the code.

**Change:** Update the generated JS example in §3.2 to use `Rivers.__directDispatch(...)`.
One-line doc change only — no code change.

---

### Item 6 — `readDir` canary endpoint

**Problem:** None of the 5 canary endpoints call `readDir`. It is the only operation with zero
live integration coverage.

**Change:** Add a 6th endpoint `GET /canary/fs/read-dir` to `canary-filesystem`. Handler creates
a temp dir with known entries, calls `readDir`, asserts on the returned `{name}` objects, cleans
up. Add to `run-tests.sh` FILESYSTEM profile.

---

### Item 7 — Concurrent-write canary test (FUP-5)

**Problem:** No test exercises concurrent access to the same filesystem datasource. The TOCTOU
acknowledgment in §5.4 is unverified under real load.

**Change:** Add endpoint `GET /canary/fs/concurrent-writes`. Handler spawns N parallel write
operations via `Promise.all` equivalent in the JS handler (sequential calls with overlapping
working dirs), verifies all succeed and no data corruption occurs. Validates that the driver's
lock-free model (each op is independent) holds in practice.

Note: V8 handlers are single-threaded per isolate — true concurrent OS-level writes require
multiple request dispatches. This test fires multiple sequential operations within one handler
to cover the intra-handler case. FUP-5's true cross-handler concurrency requires a load driver
outside the canary app, tracked separately.

---

### Item 8 — Deploy and run canary (FUP-6)

**Problem:** `canary-filesystem` is scaffolded and validated but not deployed. `run-tests.sh`
has never executed against a live instance with the filesystem profile.

**Change:** Run `cargo deploy /path/to/release/canary` from the feature branch, start riversd,
run `run-tests.sh`, confirm all FILESYSTEM profile tests PASS. Record pass count in changelog.

---

## Out of Scope

- **FUP-2 (Windows junction tests):** Blocked on Windows CI runner. No action.
- **FUP-4 (Direct token serialization):** Only relevant for cdylib engine paths. Deferred.
- New filesystem operations (watch, chmod, link, streaming reads).
- New drivers (S3, GCS, Azure Blob).

---

## Implementation Order

1. Item 5 — spec §3.2 fix (trivial, unblock spec reading)
2. Item 1 — spec §6.2 + §6.4 readDir amendment + update canary handler comment
3. Item 2 — `ping()` implementation
4. Item 3 — extra config plumbing
5. Item 4 — ISO-8601 timestamps
6. Item 6 — readDir canary endpoint
7. Item 7 — concurrent-write canary endpoint
8. Item 8 — deploy + run-tests.sh sign-off

---

## Validation

Each code item ships with:
- Unit tests in `filesystem.rs` (or existing tests updated)
- `cargo test -p rivers-drivers-builtin` passes
- Canary items verified via `run-tests.sh` FILESYSTEM profile

The driver is complete when `run-tests.sh` reports 0 FAIL / 0 ERR on the FILESYSTEM profile
against a live deployed instance.
