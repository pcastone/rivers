# Rivers Code Review — Per-Crate Focus Blocks

Use these with **Prompt 2** from `rivers-code-review-prompt-kit.md`. For each crate, replace:

- `{{CRATE_NAME}}` with the crate name
- `{{CRATE_PATH}}` with the path
- `{{DISCOVERY_CONTEXT}}` with the full focus block below

One Claude Code session per crate. Don't mix.

---

## Tier A — Highest Risk (Review First)

These crates handle secrets, FFI boundaries, command execution, or define contracts that other crates depend on. Bugs here have the largest blast radius.

---

### 1. `rivers-plugin-exec`

**Path:** `crates/rivers-plugin-exec`
**Tier:** A — Highest Risk
**Role:** Controlled command execution plugin with SHA-256 hash pinning as authorization. Three integrity modes (`each_time`, `startup_only`, `every:N` execution-count-based), three input modes (`stdin`, `args`, `both`), process isolation via privilege drop, dual semaphore concurrency control.

**Why this is the highest-risk crate in the review:** This is the only component in Rivers that executes arbitrary external binaries. The hash pinning is the *entire* authorization model — if the verification has a TOCTOU window, an injection path, or a skipped code branch, the sandbox collapses.

**Key risks specific to this crate:**
- **Hash verification TOCTOU** — binary read for hashing vs binary actually executed. If there's any window between `check(path)` and `exec(path)`, a symlink swap or file replacement breaks the guarantee.
- **Integrity mode correctness** — `every:N` counter wraparound, `startup_only` skipping re-verification on config reload, `each_time` not actually running on every call.
- **Argument / stdin injection** — how is user-controlled data passed to the child? Is there any shell invocation (`sh -c`)? Are args properly separated or joined into a single string?
- **Privilege drop** — `setuid`/`setgid` ordering, `setgroups` call (commonly forgotten — leaves supplementary groups inherited), capability drops on Linux.
- **Signal and lifecycle handling** — zombies on orphaned children, SIGKILL on timeout not reaching the whole process group, child processes surviving `riversd` shutdown.
- **Semaphore correctness** — the dual semaphore for concurrency control: can both be acquired in inconsistent order? Deadlock on `acquire` under contention? Release on all exit paths including panic?
- **stdio buffer bounds** — unbounded stdout/stderr capture is a DoS vector; how is this capped?
- **Environment sanitization** — inherited env vars from the `riversd` process leaking into the child.

---

### 2. `rivers-lockbox-engine`

**Path:** `crates/rivers-lockbox-engine`
**Tier:** A — Highest Risk
**Role:** Core secrets engine. Secrets are read from disk, decrypted, used, and zeroized on every access — never held in memory for longer than necessary.

**Why it matters:** Every secret in Rivers flows through this crate. A leak here leaks everything — database passwords, API keys, session signing keys, admin tokens.

**Key risks specific to this crate:**
- **Zeroization completeness** — `Drop` impls that don't zero all fields, `Vec<u8>` resizes leaving old buffer unzero'd, `String::clone()` creating an unzero'd copy, compiler eliding the memset (use `zeroize` crate explicitly).
- **Constant-time comparison** — secrets compared with `==`, `slice::eq`, or early-return loops instead of `subtle::ConstantTimeEq`.
- **Memory locking** — is `mlock`/`VirtualLock` used on pages holding secret material? Not required but expected for this threat model.
- **Log/error message leakage** — secrets appearing in `Debug` impls, error messages (`anyhow::Error` will chain; check what gets chained), panic messages.
- **Key derivation** — correctness of KDF parameters (Argon2 cost, scrypt N/r/p, PBKDF2 iterations), salt handling, IV reuse.
- **File permission checks** — reading a key file world-readable should be an error, not a warning.
- **TOCTOU on read-decrypt-use** — can a secret be swapped between read and use? Is the canonical on-disk format authenticated (AEAD with associated data)?

---

### 3. `rivers-keystore-engine`

**Path:** `crates/rivers-keystore-engine`
**Tier:** A — Highest Risk
**Role:** Application encryption key storage engine (distinct from LockBox — keystore holds keys that encrypt app-level data; LockBox holds secrets used by the framework).

**Key risks specific to this crate:**
- **Same secret-handling risks as `rivers-lockbox-engine`** — zeroization, constant-time comparison, error leakage.
- **Key rotation safety** — old keys retained long enough to decrypt existing data, but not indefinitely; atomic swap of active key without gap where neither key works.
- **Key serialization to disk** — format versioning, authenticated encryption of the keystore itself (master key handling), backup file creation (temp file world-readable before rename?).
- **Master key lifecycle** — where does the master key come from? Environment, prompt, file? Is it zero'd after unlocking the keystore?
- **Concurrent access** — if two threads need the same key simultaneously, is there serialization? Are reads/writes to the keystore file atomic (fsync, rename-atomic)?

---

### 4. `rivers-lockbox` (CLI)

**Path:** `crates/rivers-lockbox`
**Tier:** A — Highest Risk
**Role:** Command-line tool for LockBox administration (create, add secret, rotate, etc.).

**Why it matters:** CLI tools for secrets are notorious for leaking through process listings, shell history, and terminal scrollback.

**Key risks specific to this crate:**
- **Secrets on command line** — any flag that accepts a secret value inline leaks via `ps`, shell history, and audit logs. Must use stdin, TTY prompt, or file reference only.
- **TTY handling** — passphrase entry with echo disabled; correct behavior when stdin isn't a TTY (piped input — should accept it, but not prompt).
- **File permissions on writes** — newly-created LockBox files must be `0600`. `umask` dependency is a classic footgun — set mode explicitly.
- **Error output** — panic messages, `eprintln!` of error chains that include secret values, `Debug` derivations on types that hold secrets.
- **Atomicity on updates** — write to temp file, fsync, rename; never truncate-and-write the real file (interrupted writes = corrupted lockbox).
- **Subcommand authorization** — are destructive operations (delete, rotate, unlock) gated behind confirmation or flag? Easy to `rivers-lockbox delete $wrong_thing`.

---

### 5. `rivers-keystore` (CLI)

**Path:** `crates/rivers-keystore`
**Tier:** A — Highest Risk
**Role:** Command-line tool for keystore administration.

**Key risks specific to this crate:**
- **All the same concerns as `rivers-lockbox` CLI** — command-line secret leakage, TTY handling, file permissions, atomic writes, error output.
- **Key export/import** — if the CLI supports exporting keys (e.g., for backup or migration), is the export format encrypted? Is the export file created with `0600`?
- **Key generation RNG** — `rand::thread_rng()` vs `OsRng`? For cryptographic key material, `OsRng` (or `rand_core::OsRng`) is the correct choice.

---

### 6. `rivers-driver-sdk`

**Path:** `crates/rivers-driver-sdk`
**Tier:** A — High Risk
**Role:** Defines the `DatabaseDriver` / `Connection` trait contracts, the plugin ABI (`_rivers_abi_version`, `_rivers_register_driver`), and shared types (`QueryValue`, `Query`, `QueryResult`, `DriverError`). This crate is the contract every driver plugin implements.

**Why it matters:** Bugs here propagate to every driver. ABI mismatches can cause segfaults in plugin loading. The DDL guard lives here at the trait level.

**Key risks specific to this crate:**
- **ABI stability** — is `ABI_VERSION` bumped on every breaking change? Is the version check actually a strict equality (not `>=`)?
- **`extern "C"` boundary safety** — any `#[no_mangle]` function must use `catch_unwind` at the outermost layer; a Rust panic crossing FFI is undefined behavior.
- **Trait object soundness** — `dyn DatabaseDriver` must be `Send + Sync` for the pool to hand it across tasks. Bounds must be correct on all trait methods.
- **`DriverError` chain** — does it carry enough context for diagnostics without leaking secrets? Is `Display` implementation sanitized?
- **Blanket implementations** — default trait methods that forward to other methods — check for infinite recursion via method override misses.
- **`Query` construction** — is the `QueryValue` variant set exhaustive? Adding a variant later is a breaking change; check for `#[non_exhaustive]`.
- **DDL guard at the trait level** — `execute()` vs `ddl_execute()` split — can a driver accidentally forward DDL through `execute()` and bypass the guard?

---

### 7. `rivers-engine-sdk`

**Path:** `crates/rivers-engine-sdk`
**Tier:** A — High Risk
**Role:** Defines the execution engine contract (`Worker` trait, `TaskContext`, capability tokens) that both the V8 and Wasmtime engines implement. Also defines the token opacity boundary — the guarantee that isolates never see raw credentials.

**Key risks specific to this crate:**
- **Token opacity** — is `DatasourceToken` / `DataViewToken` / `HttpToken` genuinely opaque? Can anything in the type expose the underlying resource (`Deref`, public fields, `Debug` derivation that prints internals)?
- **`Send + Sync` bounds on `Worker`** — pools dispatch workers across tasks; missing bounds can cause compile errors far from the root cause, or worse, allow data races if `unsafe impl` is used to paper over a missing bound.
- **`TaskContext` lifetime** — does it outlive the task dispatch? Any borrows into caller-side state that could outlive their source?
- **Capability flag defaults** — if a new capability is added later, what's the default? Must default to *denied* (allowlist model); defaulting to allowed is a CVE.
- **Error types crossing engine boundaries** — `TaskError` variants — does every engine map its internal errors uniformly, or can a V8-specific error leak to handler code?

---

### 8. `rivers-plugin-kafka`

**Path:** `crates/rivers-plugin-kafka`
**Tier:** A — FFI Risk
**Role:** Kafka driver using `rdkafka` (which wraps `librdkafka`, a C library). Per your memories, this is being migrated from `rskafka` to `rdkafka` with static linking.

**Why it's Tier A:** Any crate wrapping a C library inherits C's safety profile. `rdkafka` is well-tested, but the Rivers wrapper code on top of it can still introduce footguns.

**Key risks specific to this crate:**
- **FFI panic safety** — callbacks passed to `librdkafka` (delivery, rebalance, error callbacks) must never panic across the C boundary. Use `catch_unwind` at the outermost frame.
- **Consumer group lifecycle** — rebalance callback correctness; offset commit semantics (at-least-once vs at-most-once — which did the driver promise?); partition assignment/revocation correctness.
- **Producer flush on shutdown** — dropping the producer without flush = silent message loss. Is there an explicit flush in the shutdown path with a bounded timeout?
- **Thread safety of callbacks** — `librdkafka` calls callbacks from its own threads; any shared state touched by callbacks needs correct synchronization.
- **Static linking correctness** — if `rdkafka` is statically linked with SSL/SASL, are the OpenSSL/Cyrus-SASL versions pinned? Dynamic loading of a second OpenSSL at runtime is a segfault waiting to happen.
- **Error propagation from `librdkafka`** — the C library returns error codes; are all non-success codes handled, or does the code path assume success past certain API calls?

---

## Tier B — Core Infrastructure (Review Second)

Utility and tooling crates. Not as dangerous as Tier A, but used constantly and therefore high-impact if buggy.

---

### 9. `rivers-core-config`

**Path:** `crates/rivers-core-config`
**Tier:** B — High Use
**Role:** TOML configuration parsing and validation for `riversd.toml`, app `manifest.toml`, `resources.toml`, and `app.toml`. Upstream of nearly every runtime subsystem.

**Key risks specific to this crate:**
- **`#[serde(default)]` hiding missing required fields** — if a field has a default, a missing value silently gets the default; may mask misconfigurations that should be hard errors.
- **Path traversal** — any config field that's a file path (cert path, key path, app bundle path, log dir) — does the code validate against `..` and absolute paths where expected?
- **Unbounded string fields** — deserializing untrusted TOML with `String` fields allows arbitrary-size allocation. Not a practical attack on a local file, but still worth checking.
- **Validation gap between parse and use** — a TOML value can parse successfully but be semantically invalid (port 0, negative timeouts, empty required arrays). Is there a `validate()` phase, and is it called on *every* config load path (first load, hot reload, test loads)?
- **Hot reload correctness** — if config can be reloaded at runtime, is the reload atomic? Can a partial reload leave the system in a hybrid state?
- **Default value traps** — defaults that are reasonable in development but dangerous in production (permissive CORS, auth disabled, admin API on public interface).

---

### 10. `riversctl`

**Path:** `crates/riversctl`
**Tier:** B
**Role:** Admin CLI that talks to the `riversd` admin API — status, drivers, datasources, deploy/promote/approve workflow, TLS cert management.

**Key risks specific to this crate:**
- **Admin API authentication** — how does `riversctl` authenticate to the admin API? Ed25519 signatures per the specs. Is the signing key loaded from a well-defined location with `0600` perms? Any path where the key material ends up in `argv` or env vars?
- **Output formatting of sensitive data** — admin API responses may contain credentials, tokens, trace IDs, internal paths. Is there a sanitization layer, or is response content printed verbatim?
- **JSON injection in CLI args** — if `riversctl` accepts JSON blobs on the command line (common for deploy manifests), is it validated before being shipped to the admin API?
- **Subcommand dispatch** — destructive operations (reject deploy, delete app, rotate keys) gated behind `--yes` or confirmation? Easy to fat-finger the wrong target.
- **Config file handling** — if `riversctl` has its own config (endpoint URL, auth key path), where does it live? Permissions? Override order (flag > env > file > default)?

---

### 11. `cargo-deploy`

**Path:** `crates/cargo-deploy`
**Tier:** B
**Role:** Cargo extension that builds and deploys Rivers to a target directory. Dynamic mode ships thin binaries + cdylib engines/plugins; static mode links everything into one fat binary. Generates self-signed TLS cert at deploy target. Cross-compiles from macOS to Linux via `cross` + custom Docker image.

**Key risks specific to this crate:**
- **File permissions on deploy target** — binaries `0755`, keys and certs `0600`, config files readable only by the service user. Default `umask` is a footgun.
- **Overwrite protection** — what happens if you `cargo deploy` to a target that already has a running `riversd`? Are binaries swapped atomically (write + rename) or truncate-in-place?
- **Self-signed cert generation** — RNG source for key generation (must be `OsRng`), cert validity period sane, cert file permissions `0600`.
- **Docker invocation** — any command-line construction for `docker run` that accepts user input? Paths with spaces, shell metacharacters in env var values?
- **Symlink handling** — if the deploy target is a symlink, does the tool follow it unconditionally? Could break out of an expected deploy root.
- **Cross-compilation toolchain validation** — if the custom Docker image isn't present, does the tool fail clearly or silently produce a broken binary?
- **Incomplete deploy recovery** — if the deploy fails mid-copy, is there rollback, or is the target left in a half-updated state that won't start?

---

### 12. `riverpackage`

**Path:** `crates/riverpackage`
**Tier:** B
**Role:** App bundle packaging tool. Bundles ship as zip files with `manifest.toml` + per-app directories. `riverpackage --pre-flight` does build-time validation.

**Key risks specific to this crate:**
- **Zip slip (path traversal on extract)** — every zip extraction must validate that extracted paths don't contain `..` or absolute components. Classic CVE pattern; easy to miss.
- **Zip bomb resistance** — bounded extraction size, bounded decompression ratio. Unbounded decompression into a temp dir is a denial-of-service vector.
- **Symlink handling in archives** — does the tool refuse symlinks in archives, or follow them blindly (enables path traversal via symlink target)?
- **Manifest validation depth** — does preflight actually check everything that `riversd` checks at load time, or does it stop at "parses as TOML"? If they diverge, deploys pass preflight and fail at runtime.
- **Temp file cleanup** — temp dirs for extraction cleaned up on error paths, including panic? Use `tempfile` crate's `TempDir` (auto-cleanup on drop) or manually ensure `Drop` impl covers it.
- **Reproducibility** — if the tool embeds timestamps or RNG state into the archive, bundles aren't reproducible. Not a bug in the security sense, but a signing/verification footgun.

---

## Tier C — Driver Plugins (Review Last)

Driver plugins share a common bug class profile: parameter binding order, connection pool leaks on error, DDL guard enforcement, NULL handling, type coercion, circuit breaker behavior, and backend-specific footguns. The review pattern is similar across all of them; the backend-specific risks differ.

---

### 13. `rivers-plugin-ldap`

**Path:** `crates/rivers-plugin-ldap`
**Tier:** C — Auth-adjacent
**Role:** LDAP driver built on `ldap3` (pinned upgrade 0.11 → 0.12 per roadmap).

**Key risks specific to this crate:**
- **Bind credential handling** — credentials read from LockBox must not hang around in memory post-bind; after successful bind, the credential should be zero'd.
- **LDAP injection in search filters** — user input interpolated into filter strings (`(uid=${user})`) must be escaped per RFC 4515. Easy to miss; the filter syntax is subtle.
- **TLS validation** — LDAPS vs StartTLS; cert verification defaults to *on*, but is there any config path that disables it?
- **Anonymous bind fallback** — if auth fails, does the code accidentally fall through to anonymous bind?
- **Paged search handling** — unbounded result sets from a search can blow memory; is there a page limit enforced?

---

### 14. `rivers-plugin-neo4j`

**Path:** `crates/rivers-plugin-neo4j`
**Tier:** C — Pre-release dependency
**Role:** Neo4j driver built on `neo4rs 0.9 RC` (Bolt protocol). Per your memories, this is a release candidate — less battle-tested than stable crates.

**Key risks specific to this crate:**
- **Wrapper around RC code** — assume `neo4rs` has rough edges; where does Rivers catch its panics or handle unexpected errors? Any direct `unwrap()` on a `neo4rs` result is a server crash path.
- **Cypher parameter binding** — parameters vs string interpolation in query construction. Cypher injection is a real thing.
- **Transaction lifecycle** — explicit transactions with rollback on error path. Orphaned transactions hold cluster locks.
- **Connection leak on Bolt protocol errors** — Bolt has stateful connections; a protocol error mid-query can leave the connection in an unrecoverable state. Is it returned to the pool or discarded?
- **Type coercion** — Neo4j temporal types (Date, LocalDateTime, Duration) — are they mapped cleanly to `QueryValue`, or is there precision loss?

---

### 15. `rivers-plugin-cassandra`

**Path:** `crates/rivers-plugin-cassandra`
**Tier:** C
**Role:** Cassandra/Scylla CQL driver.

**Key risks specific to this crate:**
- **Prepared statement cache** — unbounded cache grows per unique query text; bounded LRU?
- **Parameter binding order** — Cassandra is strict about bind order matching `?` placeholders; mismatches are silent (wrong column filled with wrong value).
- **Paging cursor handling** — paging state propagation across calls; leaks if state isn't closed.
- **Type coercion** — UUIDs, timestamps, counters, collections (list/set/map), tuples; each has encoding edge cases.
- **Retry policies** — CQL drivers typically have configurable retry; misconfigured retries on idempotent-unsafe operations cause duplicates.
- **Load balancing / token-aware routing** — if enabled, failover correctness when nodes disappear.

---

### 16. `rivers-plugin-mongodb`

**Path:** `crates/rivers-plugin-mongodb`
**Tier:** C
**Role:** MongoDB driver.

**Key risks specific to this crate:**
- **BSON construction from user input** — operator injection (`$where`, `$ne` vs direct equality) if raw Value input is merged into query documents.
- **Aggregation pipeline stages from user input** — similar injection risk; stages like `$lookup` can exfiltrate from arbitrary collections.
- **ObjectId generation** — if driver generates ObjectIds, RNG source must be strong.
- **Connection string parsing** — URI injection if user-controlled fragments are concatenated into the URI.
- **Read/write concern defaults** — default `w=1` vs `w=majority`; default read preference. Wrong defaults = wrong durability guarantees.
- **Cursor lifecycle** — unbounded cursors that aren't closed leak server-side resources.

---

### 17. `rivers-plugin-elasticsearch`

**Path:** `crates/rivers-plugin-elasticsearch`
**Tier:** C
**Role:** Elasticsearch driver, HTTP-based.

**Key risks specific to this crate:**
- **Query DSL construction** — string interpolation of user input into query JSON is an injection risk; parameters should always go through structured builders.
- **Scroll context leaks** — open scroll IDs hold server-side memory until timeout; must be explicitly closed on all exit paths.
- **Bulk API batching** — batch size limits (payload size and document count), partial failure handling (bulk can return per-doc errors).
- **Auth header handling** — basic auth vs API key vs SSO; credentials not logged in retries.
- **Response size limits** — unbounded JSON response body = unbounded allocation.

---

### 18. `rivers-plugin-couchdb`

**Path:** `crates/rivers-plugin-couchdb`
**Tier:** C
**Role:** CouchDB driver, HTTP-based.

**Key risks specific to this crate:**
- **Request timeouts** — HTTP client connect + read + total timeouts all set; any long-poll or changes-feed endpoint with explicit handling (usually wants longer timeout than CRUD ops).
- **View/index query construction** — user input in `startkey`/`endkey` or view names must be properly encoded.
- **Auth cookie lifecycle** — if session cookies are used, refresh on expiry; not re-authenticating on every request.
- **JSON body size limits** — incoming response size caps.
- **Changes feed** — if used, it's a long-lived HTTP connection; reconnect semantics on network errors, sequence tracking.

---

### 19. `rivers-plugin-influxdb`

**Path:** `crates/rivers-plugin-influxdb`
**Tier:** C
**Role:** InfluxDB driver, line protocol over HTTP.

**Key risks specific to this crate:**
- **Line protocol escaping** — measurement names, tag keys/values, field keys all have different escape rules for spaces, commas, equals signs. Easy to miss one.
- **Timestamp precision** — ns/μs/ms/s selected per write; mismatches silently store wrong-timestamp points.
- **Batch flush semantics** — if the driver batches writes, flush on shutdown; unflushed writes = data loss.
- **Query construction** — InfluxQL or Flux parameter handling if user input is involved.

---

### 20. `rivers-plugin-redis-streams`

**Path:** `crates/rivers-plugin-redis-streams`
**Tier:** C
**Role:** Redis Streams driver (`XADD`, `XREAD`, `XREADGROUP`).

**Key risks specific to this crate:**
- **Consumer group lifecycle** — group creation race (`XGROUP CREATE` is not idempotent without `MKSTREAM`), consumer tombstones not cleaned up.
- **Pending Entry List (PEL) management** — messages delivered but not ack'd pile up; is there PEL reclaim via `XAUTOCLAIM` or `XPENDING` + `XCLAIM`?
- **Stream trimming** — `MAXLEN`/`MINID` trim strategy; unbounded streams eat memory.
- **Last delivered ID tracking** — consumer-side tracking vs server-side; loss or corruption on crash/restart.
- **Blocking reads** — `XREAD BLOCK` with explicit timeout; missing timeout = hung consumer.

---

### 21. `rivers-plugin-nats`

**Path:** `crates/rivers-plugin-nats`
**Tier:** C
**Role:** NATS driver.

**Key risks specific to this crate:**
- **Subscription lifecycle** — unsubscribe on error / shutdown paths, sid (subscription ID) cleanup.
- **Request-reply timeout** — NATS requests without timeout hang forever; every request-reply must have an explicit deadline.
- **Message ack/nack** — if using JetStream, ack semantics; missing ack = redelivery storm.
- **Subject wildcards** — if user input is used in subjects, `*` and `>` can subscribe to more than intended.
- **Reconnection handling** — in-flight requests during reconnect, subscription restoration.

---

### 22. `rivers-plugin-rabbitmq`

**Path:** `crates/rivers-plugin-rabbitmq`
**Tier:** C
**Role:** RabbitMQ (AMQP 0.9.1) driver, likely via `lapin`.

**Key risks specific to this crate:**
- **Channel vs connection lifecycle** — channels are cheap but bounded; channel leak on error paths exhausts the connection's channel quota.
- **Consumer tag collisions** — explicit or auto-generated; duplicates silently take over existing subscriptions.
- **Ack / nack correctness** — `multiple=true` acks everything up to delivery tag (rarely what you want per-message); `nack` with `requeue=true` can cause poison-message loops.
- **Publisher confirms** — without confirm mode, publish is fire-and-forget; with confirms, must wait for ack/nack with timeout.
- **Prefetch (QoS)** — consumer prefetch too high = memory pressure; too low = throughput collapse.
- **Connection recovery** — lapin's auto-recovery behavior vs manual; in-flight consumers on recovery.

---

## Running the Reviews

Suggested workflow:

1. Start fresh Claude Code session.
2. Paste Prompt 2 template from the prompt kit.
3. Fill in `{{CRATE_NAME}}`, `{{CRATE_PATH}}`, `{{DISCOVERY_CONTEXT}}` with values from the block above.
4. Let it run through Phase 1 (sweep) → Phase 2 (confirm) → Phase 3 (deep read).
5. Save the report to `reviews/<crate-name>.md`.
6. Close session, start next crate fresh.

After all 22 are done, a consolidation pass can cross-reference findings (e.g., "every HTTP-based driver has the same timeout gap" — that's a Rivers-wide finding worth surfacing separately).
