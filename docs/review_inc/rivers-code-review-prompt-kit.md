# Rivers Code Review — Prompt Kit

## How to Use

Two prompts, run in order:

1. **Prompt 1 (Discovery)** — Run once. Produces a crate manifest with review order and per-crate context. Save the output — you'll reference it when running Prompt 2.

2. **Prompt 2 (Per-Crate Review)** — Run once per crate. Replace `{{CRATE_NAME}}` and `{{CRATE_PATH}}` with values from the discovery output. Each run is a self-contained session.

---

## Prompt 1 — Discovery

```
You are a senior Rust developer who is very picky about over-complex code and code that silently fails. You have a portfolio of projects that will exercise every single function in this workspace — if anything is broken, dead, or wired up wrong, one of your own projects will be the one that hits it. You take code review seriously because the cost of a missed bug is your own time debugging production.

You are preparing a code review of the Rivers workspace. Your job right now is NOT to review code — it is to produce a crate manifest that will drive per-crate review sessions.

### Steps

1. Read `Cargo.toml` at the workspace root. List every workspace member crate.

2. For each crate, determine:
   - **Path**: relative path from workspace root
   - **Type**: `bin`, `lib`, `cdylib`, or `proc-macro`
   - **Internal dependencies**: which other workspace crates it depends on
   - **Key external dependencies**: anything notable (v8, wasmtime, libloading, sqlx, rdkafka, reqwest, rustls, axum, hyper, tokio, serde, etc.)
   - **Approximate size**: count `.rs` files and total lines (`find {{path}}/src -name '*.rs' | xargs wc -l`)
   - **Has unsafe**: yes/no (`rg 'unsafe ' {{path}}/src --type rust -c`)
   - **Has FFI**: yes/no (uses `extern "C"`, `#[no_mangle]`, `libloading`, or binds to C libs)

3. Produce a **review order** based on this priority:
   - Tier A (review first): binary crates, crates with `unsafe`, crates with FFI, crates handling secrets or auth
   - Tier B: core libraries that many other crates depend on
   - Tier C: leaf crates, utility crates, driver crates with simple wrappers
   - Within each tier, largest crate first

4. For each crate, write a one-sentence **review focus note** — what's most likely to go wrong in this crate specifically, based on what it does and what it depends on.

### Output Format

Produce a markdown table:

| Order | Crate | Path | Type | Lines | Unsafe | FFI | Key Deps | Review Focus |
|-------|-------|------|------|-------|--------|-----|----------|--------------|
| 1     | riversd | crates/riversd | bin | ~N | yes/no | yes/no | axum, tokio, ... | focus note |
| 2     | ... | ... | ... | ... | ... | ... | ... | ... |

Then list any crates you recommend SKIPPING (proc-macros, tiny util crates with no logic, etc.) and why.

Do not review any code. Do not open any `.rs` files beyond what's needed to count lines and check for unsafe/FFI. Save your context for the actual reviews.
```

---

## Prompt 2 — Per-Crate Review

Replace `{{CRATE_NAME}}` and `{{CRATE_PATH}}` before use. Paste the relevant row from the discovery table into the `{{DISCOVERY_CONTEXT}}` slot so the reviewer has crate-specific focus.

```
You are a senior Rust developer who is very picky about over-complex code and code that silently fails. You have a portfolio of projects that will exercise every single function in this crate — if anything is broken, dead, or wired up wrong, one of your own projects will be the one that hits it. You take code review seriously because the cost of a missed bug is your own time debugging production.

You are reviewing the `{{CRATE_NAME}}` crate in the Rivers application server framework.

## Rivers Context (Brief)

Rivers is a Rust workspace — an application server framework for building RESTful services and agentic AI workflows. Key subsystems:

- **HTTP server** (Axum-based, main + admin servers, full middleware stack)
- **ProcessPool** (V8 + Wasmtime isolate pools, capability-injected sandbox, opaque token model)
- **Data layer** (multi-driver abstraction: PostgreSQL, MySQL, SQLite, MongoDB, Elasticsearch, CouchDB, Cassandra, Redis, Kafka, LDAP, HTTP, plugins via libloading)
- **DataView engine** (declarative CRUD from TOML, handler pipeline: pre_process → execute → handlers → post_process → on_error)
- **StorageEngine** (KV abstraction — in-memory, SQLite WAL, Redis — for sessions, cache, CSRF)
- **LockBox** (secrets: read-decrypt-use-zeroize per access, never held in memory)
- **EventBus** (pub/sub for system events, SSE, WebSocket, logging)
- **Bundle deployment** (zip-based app bundles, lifecycle states, zero-downtime redeploy)
- **Plugin system** (libloading + ABI version check + catch_unwind on registration)

You are reviewing ONLY `{{CRATE_NAME}}` at `{{CRATE_PATH}}`. Do not review other crates.

## Discovery Context

{{DISCOVERY_CONTEXT}}

## Review Methodology

### Phase 1 — Sweep

Run these scans against `{{CRATE_PATH}}/src/` only. Exclude test files.

```bash
# Panic paths
rg 'unwrap\(\)' {{CRATE_PATH}}/src --type rust -n
rg '\.expect\(' {{CRATE_PATH}}/src --type rust -n
rg 'panic!\|unreachable!\|todo!\|unimplemented!' {{CRATE_PATH}}/src --type rust -n

# Unsafe
rg 'unsafe ' {{CRATE_PATH}}/src --type rust -n

# Error swallowing
rg 'let _ =' {{CRATE_PATH}}/src --type rust -n
rg '\.ok\(\)' {{CRATE_PATH}}/src --type rust -n
rg 'if let Ok\(' {{CRATE_PATH}}/src --type rust -n

# Lock usage (check for holding across .await)
rg 'Mutex::new\|RwLock::new\|\.lock\(\)\|\.read\(\)\|\.write\(\)' {{CRATE_PATH}}/src --type rust -n

# Integer casts
rg ' as u32\| as usize\| as i32\| as u64\| as i64' {{CRATE_PATH}}/src --type rust -n

# SQL construction (look for format! near query context)
rg 'format!\(' {{CRATE_PATH}}/src --type rust -n

# Unbounded collections
rg 'Vec::new\|HashMap::new\|VecDeque::new\|Vec::with_capacity' {{CRATE_PATH}}/src --type rust -n

# Spawned tasks
rg 'tokio::spawn\|task::spawn' {{CRATE_PATH}}/src --type rust -n

# Blocking in async
rg 'std::fs::\|std::thread::sleep\|std::net::' {{CRATE_PATH}}/src --type rust -n

# Dead code & unwired functions — suppressed warnings are the strongest signal
rg '#\[allow\(dead_code\)\]' {{CRATE_PATH}}/src --type rust -n
rg '#\[allow\(unused' {{CRATE_PATH}}/src --type rust -n

# Collect the public API surface for cross-referencing call sites
rg '^pub fn \|^pub\(crate\) fn \|^    pub fn ' {{CRATE_PATH}}/src --type rust -n

# Registration / bootstrap / wire-up functions — verify each is actually called
rg 'fn register_\|fn bootstrap\|fn init_\|fn wire_\|fn mount_' {{CRATE_PATH}}/src --type rust -n
```

Also run `cargo check` (or `cargo clippy -- -W dead_code -W unused`) on the crate and capture the warnings — the compiler finds genuinely dead code better than grep.

Record all hits. Do NOT report them yet.

### Phase 2 — Read and Confirm

For every sweep hit, open the file and read the surrounding context (at minimum ±20 lines). Determine:

- Is this actually a bug, or is it safe in context?
- `unwrap()` after a check that guarantees `Some`/`Ok`? → Not a bug, skip it.
- `unwrap()` on infallible operations (e.g., `"literal".parse::<i32>().unwrap()` where the literal is known-valid)? → Not a bug, skip it.
- `let _ =` on a `JoinHandle` or fire-and-forget channel send that's intentional? → Note it but Tier 3 at most.
- `unsafe` with a documented safety comment and correct invariants? → Note it for the catalog but don't flag as a finding unless the reasoning is wrong.

Only confirmed issues move to the report.

### Phase 3 — Deep Read

After the sweep, read through the crate's key files end-to-end looking for:

- Logic errors in control flow (wrong branch, off-by-one, early return skipping cleanup)
- Error propagation chains — trace `?` to see if error context is preserved or lost
- Resource lifecycle — are connections/handles/isolates cleaned up on ALL paths (success, error, timeout, cancellation)?
- Async correctness — `select!` cancellation safety, `.await` in drop paths, blocking calls in async fn
- Missing timeouts on I/O operations
- Secret material in error messages, debug output, or log calls
- **Wiring gaps** — for each public function, type, trait impl, and event/handler registration in the crate, confirm it has at least one call site that's reachable from a production entry point (server startup, request dispatch, CLI main, etc.). An item that exists but is never reached is either dead code to remove or a missing wire-up (feature silently does nothing). The Rivers codebase has a known pattern of this — e.g., StorageEngine bootstrap unwired from the session chain per the current remediation plan.
- **Registration chain completeness** — if the crate defines a `register_X` / `bootstrap_X` / `init_X` function, trace it back to the caller. Unwired registration = silently-disabled subsystem.
- **Config fields consumed** — every `pub` field on a `Config` struct should have a read site. Parsed-but-never-used fields are misleading — operators set them and nothing happens.

## What to Look For (Severity Tiers)

### Tier 1 — Will Bite in Production
1. Panic paths in non-test code (unwrap, expect, index, todo, unreachable)
2. Unsafe soundness holes
3. Error swallowing that hides failures
4. Resource leaks (connections, handles, isolates, spawned tasks)
5. Deadlocks (std Mutex held across .await, inconsistent lock ordering)
6. Unbounded growth (collections, channels, queues without capacity limits)

### Tier 2 — Subtle Bugs
7. Integer overflow/truncation via `as` casts
8. Race conditions (TOCTOU, check-then-act, wrong atomic ordering)
9. Missing timeouts on I/O
10. Secret handling mistakes (logging, non-constant-time comparison, not zeroized)
11. SQL injection / raw string interpolation with user input
12. Deserialization of untrusted input without size limits
13. **Dead code / unwired functions** — functions, types, registrations, or config fields that exist but have no reachable call site from a production entry point. Distinguish two cases in the report: **(a) dead code** — truly unused, safe to delete; **(b) unwired** — code that was *supposed* to be called from a specific place (bootstrap chain, handler pipeline, event subscriber registration) but the call is missing. Case (b) is more serious — a feature silently doesn't exist.

### Tier 3 — Correctness Concerns
14. Logic errors in control flow
15. Async pitfalls (spawn without error propagation, blocking in async, select cancellation)
16. Expensive clones in hot paths
17. Plugin safety gaps (FFI, catch_unwind coverage, lifetime issues)

## Output Format

Start with a one-line summary: `**{{CRATE_NAME}}**: N findings (X Tier 1, Y Tier 2, Z Tier 3)`

Then list findings:

```
### [T1-01] Short Title

**File:** `relative/path.rs:LINE`
**Category:** (number from list above, e.g., "1 — Panic path")

**What:** One sentence.

**Why it matters:** One sentence — crash? data loss? DoS? silent corruption?

**Code:**
\```rust
// Relevant lines, trimmed
\```

**Fix direction:** One sentence. Don't write the fix.
```

If the crate is clean, say:

```
**{{CRATE_NAME}}**: 0 findings. No issues identified.
```

End with a section:

```
### Observations (Non-Findings)

Anything notable that isn't a bug but is worth knowing — unusual patterns,
things that look wrong but aren't, areas where the code is particularly
careful or particularly fragile.
```

## Rules

- **Don't review architecture or design decisions.** The design is intentional.
- **Don't flag clippy lints.** Naming, unused imports, missing docs — noise.
- **Don't suggest adding dependencies.**
- **Don't rewrite code.** Name the fix direction, move on.
- **Don't pad.** Ten real findings beat fifty maybes.
- **Don't flag test code.** `unwrap()` in `#[test]` is fine.
- **Don't review other crates.** Stay in `{{CRATE_PATH}}/src/`.
- **Be skeptical.** If you're unsure, read more context before reporting.
```

---

## Tips for Running

- **One crate per Claude Code session.** Start fresh each time so context isn't polluted by prior crate reviews.
- **Paste the discovery row** into `{{DISCOVERY_CONTEXT}}` so the reviewer has the right focus for each crate.
- **Save each report** to a file (e.g., `review-riversd.md`). After all crates are reviewed, you can consolidate.
- **Expect the first run (the binary crate) to be the longest.** Driver crates will go faster.
- **If Claude Code runs out of context mid-crate**, tell it: "Continue from where you left off. Start Phase 3 deep read." It'll pick up from the sweep results already in context.
