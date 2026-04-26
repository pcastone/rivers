# `rivers-plugin-exec` Review

**Crate:** `crates/rivers-plugin-exec`
**Tier:** A, highest risk
**Role reviewed:** Controlled command-execution driver plugin. SHA-256 hash pinning is the entire authorization model. Three integrity modes (`each_time`, `startup_only`, `every:N`), three input modes (`stdin`, `args`, `both`), Unix privilege drop, dual semaphore concurrency, bounded stdio with timeout, JSON in / JSON out.

## Grounding

Confirmed from source:

- Read in full: `crates/rivers-plugin-exec/Cargo.toml`.
- Read in full: `src/lib.rs`, `src/schema.rs`, `src/template.rs`, `src/integrity.rs`, `src/executor.rs`, `src/config/mod.rs`, `src/config/parser.rs`, `src/config/types.rs`, `src/config/validator.rs`, `src/connection/mod.rs`, `src/connection/driver.rs`, `src/connection/exec_connection.rs`, `src/connection/pipeline.rs`.
- Read in full: `tests/integration_test.rs`.
- Read in full for contract comparison: `crates/rivers-driver-sdk/src/traits.rs`.
- Read in full for review focus: `docs/review_inc/rivers-per-crate-focus-blocks.md` section 1.

Validation:

- `cargo check -p rivers-plugin-exec` passed (no warnings in this crate).
- `cargo test -p rivers-plugin-exec --lib` passed: 93 tests, 0 failures, 2 ignored.
  - Two ignored tests in `executor::tests` (`non_zero_exit_returns_error`, `empty_output_returns_error`) are flagged broken-pipe-on-Linux-CI, where the script exits before the writer flushes and stdin write returns EPIPE.

## Summary

The high-level shape is sound: every command is identified by name, every name maps to an absolute path with a 64-hex SHA-256 pin, every spawn goes through `tokio::process::Command` with explicit argv, no shell is involved at any point, every child gets its own session via `setsid`, and the timeout/output-overflow paths kill the whole process group with SIGKILL. The driver-SDK contract is implemented correctly: `ddl_execute` is left at the unsupported default, `admin_operations` returns `&[]`, transactions/`prepare`/`has_prepared` use the SDK defaults, and `Connection::execute` calls `check_admin_guard` before dispatching.

The bug density is concentrated in two areas:

- **Hash-pinning authorization vs. on-disk reality.** The hash is read from `path` with `std::fs::read`, which silently follows symlinks; the hash is computed from one inode and the kernel later runs `execve` on a path that is re-resolved. There is no `O_NOFOLLOW`, no `fstat` on a held fd, no read-from-fd-then-`fexecve`, and no symlink rejection in either the validator or the integrity checker. An attacker who controls any directory on the resolved path can race a swap between hash and exec. The pin is convincing on paper but does not actually authenticate the executed bytes.
- **`Every(n)` counter semantics.** `should_check()` increments the counter unconditionally and the rhythm `(n+1) % n == 0` skips the first `N-1` calls. With `every:5` the only checked calls are 5, 10, 15, …; calls 1–4 never check at runtime and rely entirely on the startup hash, which is itself TOCTOU-vulnerable.

Cleanest areas:

- Argument template interpolation. No shell, one placeholder per argument, scalar-only enforcement, missing-key fails, extra-keys silently ignored — the documented spec semantics match the implementation, and the tests exercise the edge cases (`{}`, `{partial`, partial-brace, special characters, types).
- Process group / `kill_on_drop` / SIGKILL on timeout / SIGKILL on overflow.
- `env_clear=true` defaults; warning at connect when `env_clear=false`; explicit allow-list.
- Driver-SDK contract: `Connection::execute` rejects every operation other than `query`, `admin_operations` returns `&[]`, `check_admin_guard` is called before dispatch, defaults inherited correctly for transactions/prepare.

Bug-dense areas:

- Hash verification (TOCTOU + symlink + `every:N` first-call gap).
- Stdio handling (UTF-8 boundary slice in `evaluate_result`, fixed 64 KB stderr buffer, single-read stderr, stdout overflow check after the `extend_from_slice`).
- Privilege drop wired up but incomplete: `cmd.uid(...)` is set, but `setgroups` is never called, so supplementary groups inherited from `riversd` cross into the child. Also no `sigprocmask` reset, no `umask` reset, no `RLIMIT_*` clamping.

## Tier 1 Findings

### RXE-T1-1: Hash pinning is TOCTOU-vulnerable and follows symlinks; a swap between hash and spawn defeats the entire authorization model

`hash_file()` in `integrity.rs` reads the file with `std::fs::read(path)`, which follows symlinks and returns the bytes from the resolved inode. Later, `tokio::process::Command::new(&config.path)` invokes `execve` on the **same path string**, which the kernel re-resolves at exec time and also follows symlinks. There is no opened file descriptor shared between hash and spawn, no `O_NOFOLLOW`, no `fexecve`, no inode/dev sanity check, and no symlink rejection in the validator. The validator does call `cmd.path.is_file()` (which follows symlinks) and `metadata.permissions().mode() & 0o111` (which also follows), so a symlink to a benign script passes validation.

Evidence:

- `hash_file()` reads with `std::fs::read(path)` at `crates/rivers-plugin-exec/src/integrity.rs:20`.
- `verify_at_startup()` and `CommandIntegrity::verify()` both go through `hash_file()` at `crates/rivers-plugin-exec/src/integrity.rs:67` and `crates/rivers-plugin-exec/src/integrity.rs:102`.
- `Command::new(&config.path)` at `crates/rivers-plugin-exec/src/executor.rs:56` re-resolves the path via the kernel at `execve` time.
- Validator uses `cmd.path.is_file()` and `std::fs::metadata` (both symlink-following) at `crates/rivers-plugin-exec/src/config/validator.rs:77` and `crates/rivers-plugin-exec/src/config/validator.rs:88`. Neither is `symlink_metadata`.
- Pipeline calls `cmd.integrity.verify(&cmd.config.path)` at `crates/rivers-plugin-exec/src/connection/pipeline.rs:79`, then `executor::execute_command` re-resolves the same path string at spawn time at `crates/rivers-plugin-exec/src/connection/pipeline.rs:123` to `executor.rs:56`.
- The focus block names this risk: "binary read for hashing vs binary actually executed. If there's any window between `check(path)` and exec(path), a symlink swap or file replacement breaks the guarantee" (`docs/review_inc/rivers-per-crate-focus-blocks.md:30-31`).

Impact:

The SHA-256 pin is the **only** authorization in this driver. If an attacker can rename, delete, or replace the file at `path` (or swap a symlink along the resolved path) between the hash and the spawn, the driver runs unauthorized bytes. The integration test at `tests/integration_test.rs:180` (`integrity_check_tampered_file_fails`) only verifies the case where the swap is committed before the next `should_check()` returns true — which in `startup_only` mode is **never**, and in `every:N` mode skips the first call (RXE-T1-2).

Fix direction:

- Open the file once with `OpenOptions::new().custom_flags(libc::O_NOFOLLOW)` at startup, hash from that file descriptor, and either `fexecve` from the same fd or assert that the hash matches **on the same fd** at every invocation by re-reading via `pread`. On Linux, `O_PATH | O_NOFOLLOW` plus `/proc/self/fd/<fd>` for the spawn target is a standard approach.
- Reject symlinks at validation time (`std::fs::symlink_metadata(...).file_type().is_symlink()` and fail closed).
- Reject paths whose parent directory is writable by anyone other than root or the configured `run_as_user` so the swap window cannot be opened.
- Hard-fail if `O_NOFOLLOW`/`fexecve` is not supported on the platform; do not silently degrade.
- Add a regression test that swaps a symlink target after the pin and asserts the swap is rejected.

### RXE-T1-2: `every:N` mode never checks the first invocation; combined with `startup_only` semantics, the runtime tamper window is wider than documented

`CommandIntegrity::should_check()` for `IntegrityMode::Every(n)` evaluates `(self.exec_count.fetch_add(1, Ordering::Relaxed) + 1) % n == 0`. The unit test at `crates/rivers-plugin-exec/src/integrity.rs:201` locks in the behavior: with `every:3`, calls 1 and 2 return `false`, call 3 returns `true`, and so on. The intent of `every:1` ("every call") works because `(0+1)%1 == 0` is true; the bug is `every:N` for `N >= 2`: calls `1..N` are never verified at runtime. They rely entirely on the startup hash, which is itself TOCTOU-vulnerable per RXE-T1-1.

The `startup_only` warning text is honest: "runtime tampering not detected" (`integrity.rs:128`). The `every:N` warning text at `integrity.rs:137` says "tamper detection window applies" without quantifying that the first `N-1` calls are the same as `startup_only`.

Evidence:

- `should_check()` increments unconditionally and uses modular equality at `crates/rivers-plugin-exec/src/integrity.rs:53-60`.
- Test asserts `every:3` produces `[false, false, true, false, false, true, ...]` at `crates/rivers-plugin-exec/src/integrity.rs:206-208`, confirming the first two calls never check.
- `every:N` warning at `crates/rivers-plugin-exec/src/integrity.rs:137` does not state that the first window goes unchecked.

Impact:

A reasonable operator interpretation of `every:5` is "no more than four unverified invocations between checks, ever". The implementation gives that property in steady state but **not** at startup: the first four invocations after a fresh connection (or after every reconnect/restart) are unverified at runtime. If startup verification is itself raceable per RXE-T1-1, those first calls have no integrity guarantee at all.

Fix direction:

- Make `should_check()` return `true` on the first call regardless of mode (excluding `startup_only`), then proceed with the `% n == 0` rhythm afterwards. `(self.exec_count.fetch_add(1, Ordering::Relaxed)) % n == 0` evaluated against the pre-increment value gives `[true, false, false, true, false, false, ...]` for `n=3` — sane "every 3" semantics that includes call 1.
- Update the warning text to state precisely how many invocations between checks the operator is signing up for.
- Add a unit test asserting "first call always checks in `Every(n)` mode".

### RXE-T1-3: Privilege drop is incomplete — `setgroups` is never called, so the child inherits riversd's supplementary groups

The executor sets `cmd.uid(uid)` and `cmd.gid(gid)` on Unix when running as root, using `(uid, gid)` resolved from `getpwnam`. There is no `setgroups([])` (or `initgroups`) call, so the child process keeps `riversd`'s supplementary group list. On a typical deployment, `riversd` runs as `root` (per the privilege-drop comment at `executor.rs:112`); the child therefore retains the root supplementary group set, defeating most of the value of dropping uid/gid.

The focus block names this gap: "`setgroups` call (commonly forgotten — leaves supplementary groups inherited)" at `docs/review_inc/rivers-per-crate-focus-blocks.md:33`.

Evidence:

- `cmd.uid(uid)` and `cmd.gid(gid)` at `crates/rivers-plugin-exec/src/executor.rs:129-130`.
- No `setgroups` or `initgroups` call anywhere in the crate (sweep `grep -rn "setgroups\|initgroups" crates/rivers-plugin-exec/src` returns nothing).
- The `pre_exec` hook only calls `libc::setsid()` at `crates/rivers-plugin-exec/src/executor.rs:120-123`.
- `tokio::process::Command::uid/gid` from `std::os::unix::process::CommandExt` does **not** automatically call `setgroups`.

Impact:

The "drop privileges" promise leaks group authority. On Linux, a process that retains group `0` (root) or other privileged supplementary groups can read group-readable files owned by those groups, write to group-writable directories, etc. The `run_as_user='root'` rejection at `validator.rs:22` and `validator.rs:46` is correct for the primary uid/gid but means nothing if the child still has every group `riversd` inherited at startup.

Fix direction:

- In the `pre_exec` hook, call `libc::setgroups(0, ptr::null())` *before* the kernel sets uid (because `setgroups` requires `CAP_SETGID`, lost after `setuid` from root to non-root). Right shape: `pre_exec` first calls `setsid`, then `setgroups(0, ptr)`. Std's `Command::uid/gid` performs the uid/gid drop *after* the user `pre_exec` runs, so adding `setgroups` in `pre_exec` is the right hook.
- Alternative: use `initgroups(username, gid)` so the child has *exactly* the target user's supplementary groups, not an empty set, if deployed scripts rely on the user's normal group memberships.
- Add an integration test that runs as root (CI-gated, e.g. `RIVERS_TEST_PRIVDROP=1`), spawns a child, and asserts the child's `/proc/self/status` Groups line is exactly the target's set.

### RXE-T1-4: `evaluate_result` slices a `Cow<str>` at byte index 1024 without UTF-8 boundary checking; can panic on multi-byte stderr

In `evaluate_result()`, on non-zero exit, the code does:

```rust
let stderr_str = String::from_utf8_lossy(stderr);
let truncated = &stderr_str[..stderr_str.len().min(1024)];
```

at `crates/rivers-plugin-exec/src/executor.rs:252-253`. `String::from_utf8_lossy` returns `Cow<'_, str>`, and slicing a `str` at a byte index that falls inside a multi-byte UTF-8 character panics. `stderr_str.len()` returns the byte length; `min(1024)` is a byte count; the slice `..1024` panics if byte 1024 is inside a `char`. The U+FFFD replacement character that `from_utf8_lossy` inserts on invalid input is itself 3 bytes, so this is not a hypothetical edge case — any non-ASCII stderr (e.g. localized error messages) longer than ~1 KB can trigger it.

Evidence:

- Slice at `crates/rivers-plugin-exec/src/executor.rs:253`.
- The unit test at `crates/rivers-plugin-exec/src/executor.rs:687-698` only feeds ASCII (`"x".repeat(2000)`), so the test never hits the panic path.

Impact:

A child script that emits localized UTF-8 stderr longer than 1024 bytes (German, Japanese, Spanish operator messages, escape sequences, anything with a 2- or 3-byte char straddling the boundary) panics the worker on the failure path. Because tokio runs wait+read in a task, the panic propagates as a join error and the request returns a 500 — but worse, it converts a *normal failure path* (script exited non-zero) into a panic. Panic-on-failure paths in driver code is a poor production posture for a plugin that is the only sandbox in the runtime.

Fix direction:

- Use a UTF-8-safe truncation: walk back from index `1024` to the nearest char boundary using `str::is_char_boundary`, or compute the cutoff via `char_indices`.
- Or use `stderr.iter().take(1024).copied().collect::<Vec<u8>>()` and pass that through `String::from_utf8_lossy` — `from_utf8_lossy` itself never panics and replaces the trailing partial char with U+FFFD.
- Add a regression test with multi-byte stderr (e.g. `"é".repeat(513)` is 1026 bytes — the boundary lands inside an `é`).

## Tier 2 Findings

### RXE-T2-1: stderr is read once into a fixed 64 KB buffer; reads beyond 64 KB are silently truncated and stderr is not drained, blocking the child if it exceeds the pipe buffer

In `executor.rs:215-221`:

```rust
let mut stderr_buf = vec![0u8; 65536];
let mut stderr_reader = child.stderr.take().ok_or_else(...)?;
let stderr_n = stderr_reader.read(&mut stderr_buf).await?;
```

This is a **single** `read()` call into a fixed 64 KB buffer. `tokio::io::AsyncReadExt::read` returns as soon as any data is available; the function is not `read_to_end`. So:

- A 100-byte stderr returns immediately with `stderr_n == 100`. Fine.
- A 200 KB stderr fills the kernel pipe buffer (typically 64 KB on Linux), the child blocks on its next `write()` to stderr, and our reader pulls only the first chunk available (often 64 KB but possibly less). The child cannot make progress because we never drain. We then call `child.wait().await`, which returns when the child exits — but the child is **stuck in `write()`**, so we deadlock until the timeout fires and SIGKILLs the group.
- Stdout is read in a chunked loop with a size cap, but stderr is not.

Evidence:

- Single-read pattern at `crates/rivers-plugin-exec/src/executor.rs:219`.
- Stdout uses a chunked loop (lines 199-212) — the asymmetry is deliberate but wrong.
- The 1024-byte truncation in `evaluate_result` operates on the *captured* stderr, so emitted stderr beyond 64 KB is lost forever (truncated to 64 KB by the read, then again to 1024 by the formatter).

Impact:

- A chatty failing script (compiler error logs, Python tracebacks, scanner stats) deadlocks the entire pipeline until the timeout. The user paid for a 30 s default timeout per the parser; that's 30 s of one global semaphore slot held.
- Operator visibility into failures truncates at the kernel pipe buffer, not at the configured `max_stdout_bytes`.
- The 64 KB buffer is allocated unconditionally, even on success.

Fix direction:

- Use `tokio::io::copy` with a `tokio::io::Take` adapter, or a chunked loop mirroring stdout's pattern, with its own `max_stderr_bytes` cap (default 64 KB), and on overflow kill the process group and report `"stderr exceeded limit"`.
- Drain in parallel with stdout via `tokio::join!` so the child cannot deadlock on either pipe.
- Add a config option `max_stderr_bytes` to `CommandConfig` and `ExecConfig` for parity.

### RXE-T2-2: stdout overflow is detected only after the buffer has already grown past the cap; `Vec` growth is unchecked and `with_capacity` ignores `max_stdout`

`execute_command` allocates `let mut stdout_buf = Vec::with_capacity(max_stdout.min(65536));` at `crates/rivers-plugin-exec/src/executor.rs:194`, then in the chunked read loop `stdout_buf.extend_from_slice(&chunk[..n]);` (line 207) followed by `if stdout_buf.len() > max_stdout { kill ... }` (line 208). The cap is enforced **after** the extend, so the vector can briefly hold up to `max_stdout + 8191` bytes before the kill. With a generous `max_stdout_bytes = 5 MB` default (`config/parser.rs:35`), that's fine; with a tight per-command cap of e.g. 4 KB, the vector can grow to 12 KB before the loop notices.

Independently, `Vec::with_capacity(max_stdout.min(65536))` is wrong: it caps the *initial* allocation at 64 KB but allows the vector to grow beyond `max_stdout` via `extend_from_slice`. The `min(65536)` is presumably an attempt to avoid a huge speculative allocation, which is fine, but it is unrelated to the cap.

Evidence:

- `with_capacity(max_stdout.min(65536))` at `crates/rivers-plugin-exec/src/executor.rs:194`.
- Chunked read loop at lines 199-212.
- Overflow check is after the extend at lines 208-211.

Impact:

Modest. The 8 KB overshoot is bounded and never grows unbounded. But for a security-sensitive plugin, the cap should be tight: a misconfigured `max_stdout_bytes = 100` should not silently allow up to 8 KB to be buffered.

Fix direction:

- Compute `remaining = max_stdout.saturating_sub(stdout_buf.len())`; read at most `min(8192, remaining + 1)` bytes; if the read produced more bytes than `remaining`, kill and error.
- Or check `stdout_buf.len() + n > max_stdout` *before* the extend, and slice the chunk to fit before extending so the captured-on-error excerpt is still meaningful.

### RXE-T2-3: The plugin builds in static-builtin mode but `lib.rs` only exports the ABI symbols under `#[cfg(feature = "plugin-exports")]` — same crate, two registration paths, only one of which calls `_rivers_register_driver`

`crates/rivers-plugin-exec/Cargo.toml` declares:

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[features]
plugin-exports = []
```

`lib.rs` at `crates/rivers-plugin-exec/src/lib.rs:27-38` only emits `_rivers_abi_version` and `_rivers_register_driver` when the `plugin-exports` feature is on. There is no rlib-side `register_with(&mut DriverFactory)` helper. So when the workspace links this crate as an rlib (the static-mode build path described in `CLAUDE.md`), the only handle to `ExecDriver` is the `pub use connection::ExecDriver;` re-export — the host has to manually `factory.register_database_driver(Arc::new(ExecDriver))` somewhere. If that wiring is forgotten, the driver silently does not exist in static builds.

Evidence:

- `Cargo.toml` declares both `cdylib` and `rlib`.
- `_rivers_register_driver` gated on `plugin-exports` at `crates/rivers-plugin-exec/src/lib.rs:33`.
- `pub use connection::ExecDriver;` at `crates/rivers-plugin-exec/src/lib.rs:18` is the only static-side handle.

Impact:

If the static build forgets to wire `ExecDriver` into the factory, the datasource just doesn't work — no clear error tying it back to "you need to register the driver". That's a usability/wiring fragility that intersects badly with this being the highest-risk plugin.

Fix direction:

- Add a `pub fn register(factory: &mut DriverFactory)` in `lib.rs` that both code paths can use; `_rivers_register_driver` calls it under the feature flag, and the static-mode wiring in `riversd` calls it too.
- Or document in this crate's `lib.rs` doc-comment exactly how it expects to be wired in static mode.

### RXE-T2-4: Schema validation uses `iter_errors().map(|e| e.to_string()).collect()`, leaking the offending JSON value into the error string

`CompiledSchema::validate` at `crates/rivers-plugin-exec/src/schema.rs:49-64` formats every validation error via `e.to_string()` and joins them with `"; "`. The `jsonschema` crate's error `Display` impl includes the offending value. The result is wrapped in `DriverError::Query(...)` and plumbed through to the handler, and from the handler to the HTTP client depending on how `riversd` formats driver errors.

Evidence:

- Error joining at `crates/rivers-plugin-exec/src/schema.rs:60-62`.
- The driver passes the user's `args` JSON value into `validate(&args)` at `crates/rivers-plugin-exec/src/connection/pipeline.rs:74` — this is exactly the user-supplied payload.

Impact:

If a handler routes a payload containing secrets (auth tokens passed as args, encrypted payloads, PII) through the exec driver and the schema rejects it, the rejected secret can land in the error message that flows up to logs and possibly HTTP responses. Whether this leaks externally depends on `riversd`'s error formatting policy — outside this crate's scope to verify — but the crate is making it possible.

Fix direction:

- Format errors using only the JSON Pointer / instance path, not the value: each `jsonschema::ValidationError` exposes `instance_path()` and `kind()` separately. Build a custom message that names the path and the kind ("required field missing", "type mismatch", and so on) without quoting the value.
- Alternatively, run the validator at the boundary closest to the handler (where the payload is already known to the request log) and collapse the driver-side error to a single generic "schema validation failed".

### RXE-T2-5: The atomic counter in `should_check()` is unsynchronized with `verify()` — two concurrent invocations can both pass the rhythm check while one of them pre-empts the other's read of the on-disk file

`should_check()` does `fetch_add(1, Ordering::Relaxed)`, then if the modular check passes, `verify()` is called separately. Two concurrent invocations of the same command can both pass the check (because the counter is just an `AtomicU64`), and both read the file from disk and hash it. That is mostly fine — the hashes will agree if the file is unchanged — but it doubles the I/O on each check window, and if an attacker swaps the file between the two reads, one invocation passes and one fails, which is a non-deterministic security boundary.

Evidence:

- `fetch_add(.., Relaxed)` at `crates/rivers-plugin-exec/src/integrity.rs:58`.
- No mutex around the verify / spawn pair.

Impact:

Lower than RXE-T1-1 because it requires a *concurrent* swap to flip outcomes. Bug-class adjacent to RXE-T1-1 — the same fix (open-fd-once, hash-via-fd, exec-via-fd) eliminates it because both threads read from the same fd.

Fix direction:

- Subsumed by RXE-T1-1's fix.

### RXE-T2-6: `working_directory` validation only checks `exists() && is_dir()` — does not validate that the directory is not writable by the run-as-user (so the child can drop a binary there) or that it is not a symlink

`config/validator.rs:53-64` calls `self.working_directory.exists()` and `is_dir()` (both symlink-following). There is no:

- symlink check on the working dir itself,
- mode check (e.g. reject `0o777` working dirs),
- ownership check.

Combined with `Command::current_dir(&global_config.working_directory)` at `executor.rs:78`, a child that runs in a world-writable working dir can write or stage files that subsequent invocations consume.

Evidence:

- Working-dir validation at `crates/rivers-plugin-exec/src/config/validator.rs:53-64`.
- `current_dir` at `crates/rivers-plugin-exec/src/executor.rs:78`.

Impact:

Indirect. The driver's contract is "spawn this exact pinned binary"; what the binary does inside its CWD is the binary's responsibility. But operators tend to use `/tmp` (the parser default at `config/parser.rs:31`), which is world-writable. A more conservative default would refuse `/tmp` outright in the validator and require an operator to be deliberate.

Fix direction:

- Reject `/tmp` (and other world-writable directories) as a default. Make `working_directory` mandatory with no default.
- Reject symlinks at `working_directory` validation time.
- Optionally validate ownership (`run_as_user` owns or has exclusive write).

### RXE-T2-7: Process privilege drop has no `umask`, `RLIMIT_*`, or signal-mask reset; an unsafe child can chew CPU/memory or inherit signal handlers

The `pre_exec` hook at `executor.rs:120-123` calls `setsid()` only. The child then runs with:

- `riversd`'s umask (commonly `0o022`).
- `riversd`'s rlimits (commonly unlimited for cpu, file size, address space, processes).
- `riversd`'s signal mask (whatever Tokio set up).

For a sandboxed exec plugin, this is too generous. A misbehaving child can spike CPU until the timeout, allocate as much memory as the host has, fork until the user's process limit is reached, etc.

Evidence:

- `pre_exec` at `crates/rivers-plugin-exec/src/executor.rs:118-124`.
- No `setrlimit`, `umask(0o077)`, `sigprocmask` calls anywhere in the crate.

Impact:

Per-invocation defenses against runaway children are absent. The only backstops are the timeout (kills the entire process group, good) and the global semaphore (limits concurrent children, good). Both work; the child can still cause local damage during its window.

Fix direction:

- In `pre_exec`, call `libc::setrlimit(RLIMIT_AS, ...)`, `RLIMIT_CPU`, `RLIMIT_NPROC`, `RLIMIT_FSIZE` with operator-configurable bounds. Defaults: 256 MB AS, 30 s CPU, 32 NPROC, 64 MB FSIZE.
- Also call `libc::umask(0o077)` so files the child creates default to private mode.
- Reset the signal mask: `libc::sigemptyset` + `libc::sigprocmask(SIG_SETMASK, &empty, NULL)`.
- Add `[command.*.rlimit_*]` config keys.

## Tier 3 Findings

### RXE-T3-1: Schema-validation result format leaks no field-by-field structure to callers; validator concatenates errors with `; ` rather than emitting a list

`schema.rs:49-64` joins errors with `"; "`. Callers receive a single flat string. For programmatic consumers (e.g. a handler that wants to surface "field X is missing" structurally), the only option is regex-parsing the message.

Evidence:

- `errors.join("; ")` at `crates/rivers-plugin-exec/src/schema.rs:61`.

Impact:

Quality-of-life, not security. Pairs with RXE-T2-4's redaction fix.

Fix direction:

- Return a `Vec<{path, kind}>` and let `riversd` decide how to render. Or expose a structured error variant on `DriverError` that the runtime preserves.

### RXE-T3-2: Parser silently defaults `working_directory` to `/tmp`

The parser at `crates/rivers-plugin-exec/src/config/parser.rs:28-32` does:

```rust
let working_directory = PathBuf::from(
    opts.get("working_directory")
        .map(|s| s.as_str())
        .unwrap_or("/tmp"),
);
```

A typo or missing `working_directory` silently lands in `/tmp`. The validator catches "does not exist" and "not a directory", but `/tmp` exists and is a directory, so it passes — and is world-writable per RXE-T2-6.

Evidence:

- Parser default at `crates/rivers-plugin-exec/src/config/parser.rs:31`.
- Validator does not require `working_directory` to be present at all.

Impact:

Operator footgun. Subsumed by RXE-T2-6's "make working_directory mandatory" fix.

Fix direction:

- Make `working_directory` mandatory; fail closed.

### RXE-T3-3: `nix_is_root()` is called per-spawn instead of once at connection time; minor cost and unnecessary `unsafe`

`executor.rs:127` calls `nix_is_root()` (which is `unsafe { libc::geteuid() == 0 }`) on every invocation. Privilege state of `riversd` does not change at runtime in any sane deployment. The check belongs in `connect()` once, with the result cached on the connection.

Evidence:

- `nix_is_root()` per-spawn at `crates/rivers-plugin-exec/src/executor.rs:127`.
- Definition at `crates/rivers-plugin-exec/src/executor.rs:277-279`.

Impact:

Tiny perf cost; minor `unsafe` exposure surface. Code-quality finding.

Fix direction:

- Cache the boolean on `ExecConnection` or in a `OnceLock<bool>`.

### RXE-T3-4: `tracing::info!` at command start logs `command_name` but not `args`; `tracing::error!` on failure logs `error` but not `args`. Reproducibility of failures from logs alone is limited

`pipeline.rs:50-55` logs `command` and `trace_id`. `pipeline.rs:175-182` logs `error` and `duration_ms`. Neither logs the input parameters. An operator debugging "command X failed" has to correlate with the application log or the request body to know what `args` triggered it.

Evidence:

- Start log at `crates/rivers-plugin-exec/src/connection/pipeline.rs:50-55`.
- Error log at `crates/rivers-plugin-exec/src/connection/pipeline.rs:175-182`.

Impact:

Operability, not security. And if RXE-T2-4 is fixed (no value leakage in errors), it's also a *good* property to *not* log args — they may be sensitive. Calling this T3 because the right policy is unclear and depends on data classification.

Fix direction:

- Decide a policy: log args at `debug!` level (off in production), log only args *paths*, or stay silent.

### RXE-T3-5: `ExecConfig` and `CommandConfig` derive `Debug` and `Clone`; in this driver none of the fields are secret, so this is informational

The `Debug` and `Clone` derive on `ExecConfig`/`CommandConfig` at `crates/rivers-plugin-exec/src/config/types.rs:11-58` is fine *for this crate* — there are no secrets in the parsed config. Flagging only because the same pattern in `rivers-keystore-engine` and `rivers-lockbox-engine` is a Tier 1 finding (RKE-T1-1, RLE-T1-1). The exec driver is not affected by that risk.

Evidence:

- Derives at `crates/rivers-plugin-exec/src/config/types.rs:11-58`.
- No secret fields among `run_as_user`, `working_directory`, `default_timeout_ms`, etc.

Impact:

None. Recorded so a future reviewer doesn't flag it on pattern-match.

Fix direction:

- No action.

## Non-Finding Observations

The following items were investigated against the focus block and the source, and produced no finding:

- **Shell injection.** No shell is invoked at any point. `tokio::process::Command::new` with `args(...)` passes argv directly via `execve`. The only template substitution is in `template::interpolate`, which is invoked exclusively to fill `cmd.args(&args)` and never concatenates strings into a command line. Confirmed by reading `crates/rivers-plugin-exec/src/executor.rs:56-94` and `crates/rivers-plugin-exec/src/template.rs:20-55`.
- **Argv injection.** Each placeholder produces exactly one argv slot (`template.rs:24` maps element-by-element), and array/object values are rejected (`template.rs:45-49`). Verified by tests `basic_interpolation`, `special_characters_pass_through`, `mixed_literals_and_placeholders`.
- **Stdin injection.** Stdin is `serde_json::to_vec(...)` of the params (or the `stdin_key` value), and tokio writes it as raw bytes. No template substitution into stdin.
- **Environment leakage.** With `env_clear=true` (the default at `config/parser.rs:186`), the executor calls `cmd.env_clear()` (executor.rs:82) then re-adds only `env_allow` and `env_set`. With `env_clear=false`, a `tracing::warn!` is emitted at connect time (`connection/driver.rs:71-77`) — documented operator footgun, not a silent leak.
- **`kill_on_drop`.** Set at `executor.rs:110`. If the future is dropped (timeout, riversd shutdown, request abort), the child receives SIGKILL.
- **Process group + SIGKILL on timeout / overflow.** `pre_exec` calls `setsid` (executor.rs:121), creating a new session whose PID is the PGID. `kill_process_group(child_pid)` sends `SIGKILL` to `-pid` (executor.rs:25-30), reaching all descendants. Verified in `timeout_kills_process` and `output_overflow_kills_process` tests.
- **Zombie reaping.** `tokio::process::Child` reaps via `wait()` automatically when polled or dropped; the explicit `child.wait().await` at `executor.rs:223` handles the success path; `kill_on_drop=true` plus tokio's internal reaper handles the abort path.
- **Semaphore correctness.** Acquisition is `try_acquire` — no queuing, immediate error on contention (no deadlock under load). Order is consistent: global first (`pipeline.rs:91`), then per-command (`pipeline.rs:106`). Both permits are RAII (`tokio::sync::SemaphorePermit`); they release on drop including panic. The only inconsistency-of-order risk would be if two callers acquired in different orders simultaneously — they don't, because there is a single code path. Verified at `crates/rivers-plugin-exec/src/connection/pipeline.rs:91-120`.
- **Driver-SDK `ddl_execute`.** Not overridden — the SDK default at `crates/rivers-driver-sdk/src/traits.rs:492-497` returns `DriverError::Unsupported`. Correct: exec has no notion of DDL.
- **Driver-SDK `admin_operations`.** Not overridden — returns `&[]`. Correct: exec uses the operation name `query` for everything; no admin operations.
- **Driver-SDK `check_admin_guard`.** Called at `crates/rivers-plugin-exec/src/connection/exec_connection.rs:33`. Verified.
- **Driver-SDK transactions / prepare / has_prepared.** All defaulted via the SDK trait. Exec is not a transactional datasource; correct.
- **Plugin ABI registration.** `_rivers_abi_version` and `_rivers_register_driver` exported under `plugin-exports` feature at `crates/rivers-plugin-exec/src/lib.rs:27-38`. Test `abi_version_matches` asserts the ABI constant.
- **Path traversal in `path` field.** Validated to be absolute at `crates/rivers-plugin-exec/src/config/validator.rs:69-74` and to exist + be a regular file. The hash pin then constrains *which* file at that path. (Spec coverage of the symlink follow-through is the RXE-T1-1 finding above; the absolute-path check itself is correct.)
- **Concurrency under `&mut self`.** `Connection::execute` takes `&mut self`, so a single connection cannot truly run two queries in parallel from the application side. But the pipeline only borrows `&self` from `&mut self` once it gets past the dispatcher (`exec_connection.rs:39` calls `self.execute_command(query).await`, and `execute_command` takes `&self` per `pipeline.rs:20`). With multiple connections, both global and per-command semaphores still apply because they're shared `Arc`s — the global one is per-connection (built in `connect()` at `connection/driver.rs:101`), so two connections to the same datasource each get their own pool. That is a *contract* gap with the operator's expectation of "global concurrency limit" — but since the spec explicitly says global semaphore is per-`ExecConnection` and `ExecDriver::connect` is called once per pool, it's consistent. Recorded as a non-finding because the wiring matches what the spec describes; the pool-multiplexing question is `riversd`'s, not this crate's.
- **`bare_braces_not_placeholder`.** `{}` (length 2) is treated as a literal because of `element.len() > 2` at `template.rs:33`. Confirmed by test at `template.rs:194-198`. Not a finding; consistent.
- **`getpwnam` reentrancy.** `getpwnam` is not thread-safe per POSIX; `getpwnam_r` is the safe variant. The validator and executor both call `getpwnam` directly. In practice, this is called at connect time (validator) and at spawn time (executor); collisions are unlikely but possible. Recorded here as a known C-API gotcha; in this crate's call patterns, no concurrent caller is competing for the static buffer.
- **Counter overflow in `Every(n)`.** `AtomicU64` will not realistically overflow at any human-driven invocation rate.

## Repeated Pattern

The repeated pattern in this crate is **path-string-based file identification**. The crate identifies files by `PathBuf`/`&Path` everywhere — at parse, validation, hash, and spawn — and never holds an open `FileDescriptor` that would tie hash and spawn to the same inode. Fixing RXE-T1-1 requires changing the data flow so that one fd is held open from `verify_at_startup` through every `verify()` call through `Command::spawn` (or `fexecve`). That is a non-trivial refactor but it is the only structural fix.

Shared fix:

- Introduce a `PinnedExecutable` type that owns `(File, [u8; 32], PathBuf)` and exposes `verify(&self) -> Result<()>` (re-hashes via the held fd) and `spawn(&self) -> Command` (uses `fexecve`-equivalent on Linux, falls back to `/proc/self/fd/<fd>` if needed). Replace every callsite that holds `&Path` plus a hash with this single owner.
- Reject symlinks once, in `PinnedExecutable::open`.
- Compose this with the privilege-drop hardening (RXE-T1-3, RXE-T2-7) inside the `pre_exec` hook so the new fd is consumed by `fexecve` after `setsid + setgroups + setuid`.

## Coverage Notes

Covered by tests:

- Argument template: literal-only, missing key, scalars (string/number/bool/null), arrays/objects rejected, mixed literals/placeholders, special characters, bare braces, partial braces, floats, empty templates.
- Schema validation: happy path, missing required, invalid pattern, out-of-range, additional properties, load failures (missing file, invalid JSON, invalid schema).
- Integrity hash: correct hash, wrong hash, invalid hex, wrong length, mismatch at startup, `should_check` rhythm for `EachTime`/`StartupOnly`/`Every(3)`.
- Config parsing: minimal, with overrides, missing run_as_user, args_template indexed list, env_set flat keys.
- Config validation: empty user, root user, non-existent user (Unix), non-absolute path, non-existent path, working dir non-existent, working dir not a dir, sha256 wrong length / non-hex / empty, args mode requires template, both mode requires stdin_key, executable bit check.
- Executor: stdin echo, args echo, both mode, env_clear, timeout SIGKILL, output overflow SIGKILL, invalid JSON output, evaluate_result happy/non-zero/empty/invalid-JSON.
- Connection pipeline: connect with valid hash, connect with bad hash, full pipeline stdin, command-from-statement, unknown command, unsupported operation, missing command param, ping, global concurrency limit, per-command concurrency limit, schema validation in pipeline, args mode pipeline.
- Integration: stdin round-trip, args mode interpolation, integrity correct/tampered, timeout, non-zero exit, unknown command, concurrency.

Not covered:

- Symlink swap between hash and spawn (RXE-T1-1).
- Symlink at `path` validation time (RXE-T1-1).
- Symlink at `working_directory` (RXE-T2-6).
- Working directory permission bits (RXE-T2-6).
- `every:N` first-call hash check (RXE-T1-2 — the existing `every:3` test in fact *confirms* the gap rather than rejecting it).
- `setgroups` is empty after privilege drop (RXE-T1-3).
- `RLIMIT_*` / `umask` / signal-mask in child (RXE-T2-7).
- Multi-byte UTF-8 stderr panic in `evaluate_result` (RXE-T1-4).
- Stderr larger than 64 KB / stderr deadlock (RXE-T2-1).
- Stdout overflow boundary (off-by-up-to-8 KB before the cap, RXE-T2-2).
- Static-build registration path (RXE-T2-3) — there is no `register_into` helper to test.
- Schema validator does not leak the offending value (RXE-T2-4).
- Concurrent `verify()` race (RXE-T2-5).
- Shutdown / orphan: what happens to in-flight children when `riversd` shuts down? `kill_on_drop` is set, but only fires if the future is dropped; if `riversd` SIGTERMs, tokio task drop is best-effort. Worth a focused integration test.
- Per-command `max_concurrent` interaction with global `max_concurrent` under contention (only tested in isolation).

## Bug Density Assessment

Tier 1: **4** findings (TOCTOU/symlink, `every:N` first-call, `setgroups` missing, UTF-8 boundary panic).
Tier 2: **7** findings (stderr deadlock, stdout overflow boundary, static-build registration, schema-error leakage, concurrent verify race, working_directory hardening, rlimit/umask/sigmask).
Tier 3: **5** findings (schema error format, working_directory default `/tmp`, per-spawn `geteuid`, log args policy, Debug/Clone on non-secret types).

Compared to the prior two reviews:

- `rivers-lockbox-engine`: 3 T1 / 4 T2 / 1 T3 — tightly clustered around secret-lifecycle.
- `rivers-keystore-engine`: 3 T1 / 3 T2 / 2 T3 — split between secret-lifecycle and cross-crate wiring.
- `rivers-plugin-exec`: 4 T1 / 7 T2 / 5 T3 — broader surface, density concentrated in process-isolation hardening rather than cryptographic correctness. The crypto primitive (SHA-256 file hash) is correct; it is the file-identity binding around it that is weak.

The most concerning findings are **RXE-T1-1** (TOCTOU/symlink defeats the entire pinning model) and **RXE-T1-3** (`setgroups` undermines privilege drop). Together, an attacker who has any foothold on the host that allows directory writes in a path the operator chose to pin can run privileged-group code as the configured non-root user — which on a typical deployment is not much less than the operator wanted to permit. The other Tier 1 findings (`every:N` first call, UTF-8 panic) are sharper but narrower.

## Recommended Fix Order

1. **RXE-T1-1 + RXE-T2-5 + RXE-T1-2 (combined): introduce `PinnedExecutable` and rework `every:N` first-call semantics.** Single refactor, eliminates the structural TOCTOU/symlink class and the concurrent-verify race, and includes the off-by-one fix on `every:N`. High effort (~2 days), highest impact, blocks every other improvement.
2. **RXE-T1-3 + RXE-T2-7 (combined): harden `pre_exec`.** Add `setgroups([])` (or `initgroups`), `setrlimit`, `umask(0o077)`, `sigprocmask` reset. Lower effort (~half day) once the `pre_exec` block is touched.
3. **RXE-T1-4: UTF-8-safe stderr truncation in `evaluate_result`.** Trivial (10-minute fix) but converts a panic path into a controlled error. Should ship before any production deployment.
4. **RXE-T2-1: drain stderr in parallel with stdout, with a `max_stderr_bytes` cap.** Fixes the chatty-failure deadlock. Modest effort (~half day); unblocks operability.
5. **RXE-T2-2: tighten stdout overflow boundary.** Minor; rolls in with #4.
6. **RXE-T2-4 + RXE-T3-1: structured schema-validation errors that don't leak the offending value.** Modest. Reduces external information leakage; pairs with `riversd`'s error-formatting policy.
7. **RXE-T2-6 + RXE-T3-2: make `working_directory` mandatory and reject world-writable / symlinked dirs.** Easy validator change.
8. **RXE-T2-3: add `pub fn register(&mut DriverFactory)` so static and dynamic builds use one wiring path.** Easy.
9. **RXE-T3-3, T3-4, T3-5: code-quality cleanups.** Cache `geteuid`, decide a logging policy for `args`, leave `Debug`/`Clone` derives but document why they are safe on this crate's types.
