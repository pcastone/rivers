# Rivers-Wide Code Review

**Date:** 2026-04-27
**Scope:** 22 per-crate review focus blocks from `docs/review_inc/rivers-per-crate-focus-blocks.md`
**Reviewer stance:** senior Rust review focused on over-complicated code, silent failure, unwired functionality, driver contract drift, and repeated bug classes across crates.

## Grounding

Confirmed from review work:

- Used `docs/review_inc/rivers-code-review-prompt-kit.md` Prompt 2 methodology.
- Reviewed all Tier A crates from the requested block.
- Reviewed Tier B infrastructure crates: `rivers-core-config`, `riversctl`, `cargo-deploy`, `riverpackage`.
- Reviewed Tier C driver plugins, including broker and HTTP-backed drivers.
- Ran `cargo check` for reviewed crates where feasible during the review pass.
- Used per-crate scoped sweeps for panic paths, ignored errors, lock usage, casts, formatting, unbounded collections, blocking calls, public APIs, registration functions, and dead-code suppressions.

Existing detailed per-crate reports:

- `docs/review/rivers-lockbox-engine.md`
- `docs/review/rivers-keystore-engine.md`

Second-pass validation:

- Rechecked the consolidated findings against source on 2026-04-27.
- Applied count and wording corrections from `docs/review/rivers-wide-code-review-2026-04-27-validation-pass.md`.

This report consolidates the full workspace review findings into Rivers-wide patterns and a per-crate defect inventory.

## Executive Summary

The dominant problem is not ordinary Rust syntax quality. The code generally compiles, and many crates have tests. The problem is that too many subsystems look implemented but are either not wired, not enforcing their contract, or silently falling back to weaker behavior.

Highest-risk repeated classes:

1. Secret material is repeatedly stored in ordinary `String` / `Vec<u8>` values, cloned, debug-printable, or not zeroized on error paths.
2. Broker drivers disagree with the SDK ack/nack and consumer-group contract.
3. Several config and schema knobs parse successfully but do nothing.
4. Query/search drivers commonly materialize unbounded result sets.
5. Driver-level timeout policy is inconsistent.
6. Deployment and CLI tools report success while leaving incomplete, stale, or wrong artifacts.

The highest-priority crates to fix first are:

- `rivers-plugin-exec`
- `rivers-lockbox`
- `rivers-keystore-engine`
- `riversctl`
- `rivers-driver-sdk`
- `rivers-plugin-nats`
- `rivers-plugin-neo4j`
- `rivers-plugin-elasticsearch`

## Repeated Bug Classes

### 1. Secret Lifecycle Is Manual And Easy To Get Wrong

> **Partially resolved 2026-04-29 by PR #96 (RW1.4)** — `Secret<T>` wrapper with automatic zeroize-on-drop introduced in `rivers-driver-sdk`; applied to lockbox, keystore, and cargo-deploy sensitive values. `Debug` removed from `KeystoreEntry`. Remaining gaps (public field exposure, TTY echo, argv secret input) deferred.

Affected crates:

- `rivers-lockbox-engine`
- `rivers-keystore-engine`
- `rivers-lockbox`
- `rivers-keystore`
- `cargo-deploy`
- `riversctl`

Pattern:

- Secret-bearing types derive or expose `Debug`, `Clone`, or public fields.
- Plaintext buffers are zeroized only on success paths.
- CLI tools copy private identities into ordinary `String`s.
- Private key files are created first and chmodded afterward.
- Mutating secret stores use load-mutate-save without file locking.

Representative examples:

- `rivers-lockbox-engine/src/resolver.rs`: `ResolvedEntry` carries plaintext in a public `String`, derives `Debug` and `Clone`, and has no automatic zeroization.
- `rivers-keystore-engine/src/types.rs`: `AppKeystore`, `AppKeystoreKey`, and `KeyVersion` derive `Debug`; `KeyVersion` contains `key_material`.
- `rivers-lockbox/src/main.rs`: `--value` accepts secrets in argv; interactive input uses `read_line()` with terminal echo.
- `rivers-keystore/src/main.rs`: `RIVERS_KEYSTORE_KEY` is copied into normal `String`s and not zeroized.
- `cargo-deploy/src/main.rs`: generated TLS private key is written before `0600` is applied.

Fix direction:

- Introduce one small secret wrapper policy for Rivers: redacted `Debug`, no implicit `Clone`, zeroize on drop, explicit expose APIs.
- Use it in LockBox, app keystore, admin keys, TLS key handling, and CLI identity handling.
- Treat secret store mutation as a transaction: lock, write restrictive temp file, fsync, atomic rename, fsync parent directory.

### 2. Broker Drivers Do Not Share A Real Ack/Nack Contract

> **Partially resolved 2026-04-29 by PR #96 (RW2)** — `AckOutcome` enum and `BrokerSubscription` SDK type added to `rivers-driver-sdk`; Kafka, RabbitMQ, NATS, and redis-streams updated for conformance. RabbitMQ AMQP-406 double-ack detection and NATS per-subject fairness added. Full redelivery/PEL-reclaim semantics deferred.

Affected crates:

- `rivers-plugin-nats`
- `rivers-plugin-kafka`
- `rivers-plugin-redis-streams`
- `rivers-plugin-rabbitmq`

Pattern:

- `nack()` sometimes means no-op success.
- Consumer-group identity is derived but not always used.
- Redelivery/requeue semantics are inconsistent.
- Backpressure/prefetch is missing in some drivers.

Representative examples:

- `rivers-plugin-nats/src/lib.rs`: `ack()` and `nack()` always return `Ok(())`; consumer group is built but plain `subscribe()` is used.
- `rivers-plugin-kafka/src/lib.rs`: `receive()` advances offset before `ack()`, so `nack()` cannot cause redelivery.
- `rivers-plugin-redis-streams/src/lib.rs`: `nack()` leaves entries in the PEL, but the consumer only reads `>` and has no claim/reclaim path.
- `rivers-plugin-rabbitmq/src/lib.rs`: no `basic_qos` prefetch limit before consuming.

Fix direction:

- Write an SDK-level broker conformance matrix.
- For each driver, decide whether it supports at-least-once, at-most-once, or fire-and-forget.
- If a driver cannot support `nack` / `requeue`, return `Unsupported` instead of `Ok(())`.
- Add contract tests that exercise `receive -> nack -> redelivery` and multi-node consumer groups.

### 3. Registration And Config Wiring Gaps Are Common

> **Partially resolved 2026-04-29 by PR #96 (RW3)** — NATS, RabbitMQ, and Kafka schema checkers wired into `validate_syntax.rs`; Elasticsearch `ddl_execute` returns Unsupported; `riverpackage validate --config` flag wired into engine discovery. `rivers-core-config` nested key validation and storage policy enforcement deferred.

Affected crates:

- `rivers-plugin-neo4j`
- `rivers-plugin-elasticsearch`
- `rivers-plugin-nats`
- `rivers-plugin-rabbitmq`
- `riverpackage`
- `riversctl`
- `rivers-core-config`

Pattern:

- Public functions exist and are tested, but no production caller invokes them.
- Config fields parse but are ignored.
- CLI flags are accepted but do not influence behavior.
- Static plugin features compile crates that are not statically registered.

Representative examples:

- `rivers-plugin-neo4j`: static plugin can be compiled but is not registered in static driver inventory.
- `rivers-plugin-elasticsearch`: admin operations are declared but no `ddl_execute()` implements them.
- `rivers-plugin-nats` / `rivers-plugin-rabbitmq`: schema checker functions are public and tested but unwired.
- `riverpackage`: `--config` is parsed and ignored.
- `rivers-core-config`: storage retention/cache policy fields parse but are not enforced.
- `riversctl`: `[base.admin_api].private_key` is not loaded by the CLI admin signing path.

Fix direction:

- Add a CI check for public `check_*_schema`, `register_*`, `init_*`, `bootstrap_*`, and `ddl_execute`-related functions with no production caller.
- For config, add tests that set each public field and assert runtime behavior changes or validation rejects it.
- Make ignored config a hard error where possible.

### 4. Unbounded Reads And Result Materialization Repeat Across Drivers

> **Partially resolved 2026-04-30 (RW4.2.b, cb-p1-batch2 pending PR)** — Row cap enforcement added to cassandra, mongodb (via SDK `read_max_rows`), couchdb, and influxdb (CSV). Elasticsearch already had cap from PR #96. All caps truncate with `tracing::warn!` and use `read_max_rows(params)` from SDK defaults (10,000). Streaming/pagination, RabbitMQ prefetch, and byte caps remain deferred.

Affected crates:

- `rivers-plugin-ldap`
- `rivers-plugin-cassandra`
- `rivers-plugin-mongodb`
- `rivers-plugin-elasticsearch`
- `rivers-plugin-couchdb`
- `rivers-plugin-influxdb`
- `rivers-plugin-rabbitmq`

Pattern:

- Drivers read full response bodies or full result streams into memory.
- Some APIs have no driver-side row or byte caps.
- Backpressure defaults are missing.

Representative examples:

- LDAP search collects every returned entry into a `Vec`.
- Cassandra uses unpaged query execution and materializes all rows.
- MongoDB drains the cursor into `Vec`.
- Elasticsearch deserializes response JSON and collects all hits.
- CouchDB `_find` and views collect all docs/rows.
- InfluxDB reads the full CSV response into a `String`.
- RabbitMQ starts a consumer without prefetch.

Fix direction:

- Add shared driver defaults: `max_rows`, `max_response_bytes`, `max_prefetch`, and streaming/pagination expectations.
- Enforce limits in each plugin, not only above the driver.

### 5. Timeout Policy Is Inconsistent

> **Partially resolved 2026-04-29 by PR #96 (RW4)** — `rivers-driver-sdk/src/defaults.rs` added with shared `DEFAULT_TIMEOUT_MS`, `DEFAULT_MAX_ROWS`, `DEFAULT_MAX_RESPONSE_BYTES` constants; applied to elasticsearch, influxdb, and ldap. Shared `url_encode_path_segment` helper added and applied across drivers. Cassandra, MongoDB, CouchDB, Neo4j row/timeout gaps deferred.

Affected crates:

- `rivers-plugin-exec`
- `riversctl`
- `rivers-plugin-ldap`
- `rivers-plugin-elasticsearch`
- `rivers-plugin-rabbitmq`
- `rivers-plugin-influxdb`

Pattern:

- Some drivers use default clients with no total request timeout.
- Some long I/O paths sit outside configured timeout windows.
- Publisher confirmation waits can hang indefinitely.

Representative examples:

- `rivers-plugin-exec`: stdin write happens before command timeout starts.
- `riversctl`: admin API requests use a default reqwest client without explicit timeout.
- `rivers-plugin-ldap`: connect/bind/search/add/modify/delete await network I/O directly.
- `rivers-plugin-elasticsearch`: `Client::new()` with no connect/read/total timeout.
- `rivers-plugin-rabbitmq`: publish confirm wait has no timeout.
- `rivers-plugin-influxdb`: `Client::new()` and plain `.send().await`.

Fix direction:

- Define a shared timeout policy in `rivers-driver-sdk` or driver config helpers.
- Require connect timeout, request timeout, response-body timeout, and broker confirm timeout where applicable.

## Severity Distribution

| Crate | Findings | Tier 1 | Tier 2 | Tier 3 | Density |
|---|---:|---:|---:|---:|---|
| `rivers-lockbox` | 11 | 2 | 8 | 1 | High |
| `rivers-plugin-exec` | 8 | 3 | 4 | 1 | High |
| `rivers-keystore-engine` | 7 | 1 | 6 | 0 | High |
| `riversctl` | 7 | 2 | 5 | 0 | High |
| `rivers-plugin-elasticsearch` | 6 | 1 | 5 | 0 | High |
| `rivers-plugin-neo4j` | 5 | 2 | 3 | 0 | High |
| `rivers-plugin-nats` | 5 | 2 | 3 | 0 | High |
| `cargo-deploy` | 5 | 1 | 4 | 0 | Medium-high |
| `rivers-lockbox-engine` | 4 | 0 | 4 | 0 | Medium |
| `rivers-driver-sdk` | 4 | 1 | 3 | 0 | Medium |
| `rivers-core-config` | 4 | 0 | 4 | 0 | Medium |
| `rivers-keystore` | 3 | 1 | 2 | 0 | Medium |
| `rivers-plugin-ldap` | 3 | 1 | 2 | 0 | Medium |
| `riverpackage` | 3 | 0 | 1 | 2 | Medium |
| `rivers-plugin-rabbitmq` | 3 | 1 | 2 | 0 | Medium |
| `rivers-plugin-mongodb` | 3 | 1 | 2 | 0 | Medium |
| `rivers-plugin-influxdb` | 4 | 1 | 3 | 0 | Medium |
| `rivers-plugin-redis-streams` | 3 | 1 | 2 | 0 | Medium |
| `rivers-plugin-cassandra` | 2 | 1 | 1 | 0 | Low-medium |
| `rivers-plugin-couchdb` | 4 | 1 | 3 | 0 | Medium |
| `rivers-plugin-kafka` | 1 | 1 | 0 | 0 | Low |
| `rivers-engine-sdk` | 0 | 0 | 0 | 0 | Clean in focused pass |

## Per-Crate Findings

### `rivers-plugin-exec`

> **Resolved 2026-04-29 by PR #96 (RW1.2, commits `61c8549`–`37792d0`)** — All 8 findings addressed: stdin write moved inside unified `tokio::time::timeout` block (T1); `truncate_utf8()` helper for safe byte-boundary truncation (T1); `setgroups(0, NULL)` called before uid/gid drop (T1); `/proc/self/fd/N` fd-based exec with O_CLOEXEC cleared eliminates TOCTOU on Linux (T2); `every:N` counter moved after semaphore acquisition (T2); `tokio::join!` drains stdout/stderr concurrently with byte caps (T2); case-insensitive `env_clear` parsing fails closed on invalid values (T2); `kill()` and `setsid()` errors now logged (T3).

**Summary:** 8 findings. This is the highest-risk plugin because the authorization model is hash pinning and the payload is command execution.

Findings:

- **T1:** ~~Stdin write can hang outside timeout.~~ `executor.rs` writes stdin before the timeout block, so a child that does not read stdin can pin permits and leave the child alive.
- **T1:** ~~Non-UTF8 stderr truncation can panic.~~ Lossy UTF-8 string is sliced by byte length.
- **T1:** ~~Privilege drop leaves supplementary groups untouched.~~ `uid` and `gid` are set, but supplementary groups are not cleared.
- **T2:** ~~Hash verification is path-based TOCTOU.~~ The verified path is later executed by path, allowing replacement between hash and exec.
- **T2:** ~~`every:N` integrity counter advances before semaphore acquisition.~~ Rejected attempts can consume scheduled integrity checks.
- **T2:** ~~Stdout is drained before stderr.~~ A child filling stderr can block while stdout is being awaited.
- **T2:** ~~Invalid `env_clear` values disable sanitization.~~ Only exact `"true"` maps to true; typos silently inherit env.
- **T3:** ~~Process-group setup and kill errors are ignored.~~

Fix direction:

- Put stdin write, output draining, and child wait under one lifecycle controller.
- Execute the verified file handle or otherwise remove path replacement windows.
- Clear supplementary groups before dropping privileges.
- Drain stdout/stderr concurrently with byte caps.
- Fail closed on invalid config booleans and process-group setup errors.

### `rivers-lockbox-engine`

> **Partially resolved 2026-04-30 (RW1.4.b, cb-p1-batch2 pending PR)** — `ResolvedEntry.value` changed from `Zeroizing<String>` to `SecretBox<String>` (redacted `Debug`, no `Clone`, explicit `.expose_secret()` required). `Keystore` and `KeystoreEntry` `Clone` derives removed. `encrypt_keystore` wraps `toml_str` in `Zeroizing::new()` immediately (all error paths now zeroize). 3 new unit tests on redacted debug and explicit-access contract. Per-access fetch and permission recheck remain deferred.

Detailed report exists at `docs/review/rivers-lockbox-engine.md`.

Consolidated findings:

- **T2:** ~~Returned secrets are not automatically zeroized.~~ `SecretBox<String>` with `.expose_secret()` gate resolves this (RW1.4.b).
- **T2:** ~~Plaintext buffers skip zeroization on error paths.~~ `Zeroizing::new(toml_str)` at encryption entry resolves this (RW1.4.b).
- **T2:** Per-access fetch can return the wrong secret after rotation/reorder because metadata stores a stale entry index.
- **T2:** Runtime secret reads do not recheck keystore permissions.

Fix direction:

- Replace public plaintext `String` returns with an owning redacted zeroizing type.
- Resolve secrets by stable name/alias during per-access fetch.
- Move permission checks into the actual decrypt/read path.

### `rivers-keystore-engine`

> **Partially resolved 2026-04-29 by PR #96 (RW1.4, commit `56a552f`)** — `Secret<T>` wrapper zeroizes on drop, applied to key material. Derived `Debug` removed from `KeystoreEntry`; redacted manual impl added. Remaining findings (fsync, concurrent-save locking, checked version arithmetic) are deferred to a follow-up.

Detailed report exists at `docs/review/rivers-keystore-engine.md`.

Consolidated findings:

- **T1:** `save()` returns before replacement is durable; no file or parent-directory fsync.
- **T2:** Concurrent saves can lose key rotations.
- **T2:** Plaintext serialized keystore is not zeroized on save error paths.
- **T2:** Decrypted keystore bytes are not zeroized on parse error paths.
- **T2:** ~~Secret key material is exposed through derived `Debug`.~~ Resolved RW1.4.
- **T2:** Public accessors return secret-bearing types.
- **T2:** Key rotation version can overflow.

Fix direction:

- Add durable atomic save with lock/version guard.
- Use zeroizing wrappers for plaintext serialization/decryption.
- Make secret-bearing fields private and redacted.
- Use checked version arithmetic.

### `rivers-lockbox`

> **Partially resolved 2026-04-30 (RW1.4.h, cb-p1-batch2 pending PR)** — CLI completely rewritten to route through `rivers-lockbox-engine`. Storage now uses `keystore.rkeystore` (engine's `Keystore` TOML + Age encryption) replacing per-entry `.age` files. All writes atomic (temp+rename). `validate_entry_name` called on all names. No `--value` argv — `rpassword` TTY-only input. `cmd_rekey` uses staging dir pattern. 12 unit tests. Remaining: destructive-command confirmation, concurrent-save locking.

**Summary:** 10 findings. CLI format and behavior diverge from the engine and expected operator safety.

Findings:

- **T1:** ~~`rekey` can strand existing entries.~~ Staging dir pattern in `cmd_rekey` resolves this (RW1.4.h).
- **T1:** ~~Alias file read/parse failures silently overwrite with `{}`.~~ Engine-backed storage resolves alias mutation safety (RW1.4.h).
- **T2:** ~~CLI writes a bespoke directory/per-entry store instead of engine keystore format.~~ Resolved: now uses `rivers-lockbox-engine` (RW1.4.h).
- **T2:** ~~`--value` puts secrets in argv.~~ Resolved: `rpassword` TTY-only input (RW1.4.h).
- **T2:** ~~Interactive secret input echoes to terminal.~~ Resolved: `rpassword` (RW1.4.h).
- **T2:** Identity files are created before restrictive permissions are applied.
- **T2:** ~~Mutations rewrite live files in place.~~ Atomic temp+rename everywhere (RW1.4.h).
- **T2:** ~~User names used as paths without validation.~~ `validate_entry_name` on all names (RW1.4.h).
- **T2:** Alias creation can overwrite existing names or aliases.
- **T2:** Decrypted secrets are not zeroized after use.
- **T3:** Destructive commands do not require confirmation.

Fix direction:

- ~~Route CLI storage through `rivers-lockbox-engine`.~~ Done.
- Remove argv secret input.
- Use hidden TTY input.
- Add atomic writes and validated names everywhere.
- Make rekey transactional.

### `rivers-keystore`

Findings:

- **T1:** `init` can destroy an existing keystore because it does not check for an existing target or require explicit overwrite.
- **T2:** Age identity is copied into plain `String`s and not zeroized.
- **T2:** Mutating commands can lose concurrent updates due to unlocked load-mutate-save.

Fix direction:

- Fail if target exists unless an explicit confirmed overwrite mode is used.
- Use zeroizing identity handling.
- Lock the keystore across read-modify-write.

### `rivers-driver-sdk`

> **Resolved 2026-04-29 by PR #96 (RW1.1, commit `9454141`)** — All 4 findings addressed: comment-aware DDL classifier via `strip_sql_comments()` + `first_sql_token()` (T1); DDL error messages emit only the classified token, not raw statement content (T2); span-based parameter substitution replaces global string replace (T2); `saturating_pow` / `saturating_mul` in retry backoff (T2).

Findings:

- **T1:** ~~Leading SQL comments bypass the DDL guard~~ because `is_ddl_statement()` trims whitespace but not comments.
- **T2:** ~~Forbidden DDL errors can echo credential material~~ by including raw statement prefixes.
- **T2:** ~~Dollar positional parameter rewriting corrupts prefix-sharing names~~ via repeated global replacement.
- **T2:** ~~Exponential retry backoff can overflow~~ before max-delay capping.

Fix direction:

- Use the same comment-stripped leading-token parser for DDL guard and operation inference.
- Sanitize DDL rejection errors.
- Rewrite parameters from parsed spans, not global string replacement.
- Use saturating/checked backoff arithmetic.

### `rivers-engine-sdk`

Focused pass found no confirmed issues.

Notes:

- The crate is small and primarily data/ABI types.
- `cargo check -p rivers-engine-sdk` passed during review.
- Token opacity concerns mostly live in `rivers-runtime`/engine wiring rather than this crate.

### `rivers-plugin-kafka`

> **Partially resolved 2026-04-29 by PR #96 (RW2, commit `4efa50d`; RW3, commit `50dea1b`)** — `AckOutcome` enum and `BrokerSubscription` SDK contract defined; Kafka schema checker wired into `validate_syntax.rs`. Pre-commit offset-advance TOCTOU (T1) is deferred: the Rivers-managed consumer-group semantics would need JetStream or idempotent consumer migration.

**Summary:** 1 confirmed finding plus 1 architectural observation. The crate is lower FFI risk than the focus block assumed because it uses pure-Rust `rskafka`, but broker semantics still need attention.

Findings:

- **T1:** `receive()` advances the offset before `ack()`, so `nack()` cannot redeliver the message.

Observation:

- The plugin uses `rskafka`, not the review block's expected `rdkafka`; that removes the C FFI callback risk from this crate and shifts consumer-group correctness to Rivers-managed offset/ownership code.

Fix direction:

- Track delivered-but-unacked offset separately from committed/acknowledged offset.
- Make `nack()` reset/retry correctly or return unsupported if the driver cannot provide that contract.
- Keep the Kafka follow-up focused on Rivers-managed consumer-group semantics, not librdkafka callback safety.
- Clarify whether framework-level group coordination is sufficient and test it.

### `rivers-core-config`

> **Partially resolved 2026-04-30 (RW3.3.b, cb-p1-batch2 pending PR)** — `unenforced_storage_config_fields(config)` added (6 unit tests) returning names of parsed-but-unimplemented fields (`retention_ms`, `max_events`, `cache.datasources`, `cache.dataviews`). Both `riversd` storage-engine init paths emit `tracing::warn!` when these fields have non-default values. Unknown-key depth, cookie validation path, and field name typo remain deferred.

Findings:

- **T2:** Unknown-key validation stops after root and `[base]`; nested typos are silently accepted.
- **T2:** Unknown-key allowlist uses `init_timeout_seconds`, but actual field is `init_timeout_s`.
- **T2:** `SessionCookieConfig::validate()` is not bound to every config load path; hot reload can bypass `http_only` enforcement.
- **T2:** ~~Storage policy fields parse but are not enforced.~~ Fields now warn at startup when non-default (RW3.3.b).

Fix direction:

- Centralize full `ServerConfig` validation in the config loader.
- Recursively validate known nested sections.
- Add a test for every config field that should affect runtime behavior.

### `riversctl`

> **Partially resolved 2026-04-29 by PR #96 (RW1.3, commits `fab3e61`–`12b1510`)** — Stop fallback now gated on network unreachability only, not any HTTP error (T1); `kill()` return value checked, PID file removed only on confirmed exit (T1); `log set` request body corrected from `event` to `target` (T2); admin private key config key corrected from `admin_private_key_path` to `admin_key_path` (T2). Remaining findings (deploy lifecycle, timeout, TLS import permissions) are deferred.
>
> **Further resolved 2026-04-30 (RW5.3.c, cb-p1-batch2 pending PR)** — Golden tests added: 5 admin HTTP tests (401/403/500 = `Http` variant, not `Network`; connection-refused = `Network`); 4 stop-command tests (PID file discovery, parse, invalid content, missing). `ENV_LOCK` mutex serializes env-var mutation. Deploy lifecycle, request timeout, and TLS import permissions remain deferred.

Findings:

- **T1:** ~~Admin shutdown falls back to local OS signals after any API error, including auth/RBAC failure.~~ Fixed RW1.3.
- **T1:** ~~Local stop ignores `kill` failures and removes the PID file anyway.~~ Fixed RW1.3.
- **T2:** `deploy` only creates a pending deployment.
- **T2:** ~~`log set` sends `event` while server expects `target`.~~ Fixed RW1.3.
- **T2:** Admin HTTP requests have no explicit timeout.
- **T2:** ~~Configured admin private keys are never loaded; malformed env keys are silently ignored.~~ Fixed RW1.3.
- **T2:** TLS import does not lock down imported private-key permissions.

Fix direction:

- Distinguish network unreachable from HTTP/auth failure.
- Check signal return values and verify process state.
- Either expose staged deploy lifecycle explicitly or drive the full deploy/test/approve/promote flow.
- Use one typed admin API client with auth, timeout, and schema-tested request bodies.

### `cargo-deploy`

> **Resolved 2026-04-29 by PR #96 (RW5, commit `4c0dda7`)** — All 5 findings addressed: missing engine dylibs now abort deploy with a fatal error (T1); staging directory pattern (rename-live-to-old, copy-staging-to-live, cleanup) replaced direct writes (T2); TLS cert/key generation skipped when existing cert is still valid (T2); private key created with `0600` from the first write call (T2); actual Cargo target directory resolved via `CARGO_TARGET_DIR` env then `cargo metadata` (T2).

Findings:

- **T1:** ~~Dynamic deploy can succeed without required engine libraries.~~
- **T2:** ~~Deploy writes directly into the live target.~~
- **T2:** ~~Redeploy always replaces TLS certificate and key.~~
- **T2:** ~~Private key is created before restrictive permissions are applied.~~
- **T2:** ~~Cargo target directory is hard-coded to `target/release` despite `CARGO_TARGET_DIR`.~~

Fix direction:

- Make missing dynamic-mode engines fatal.
- Assemble deployments in a staging/versioned directory and atomically switch.
- Generate TLS only on bootstrap unless explicitly renewing.
- Create private keys with `0600` from the start.
- Resolve actual Cargo target directory.

### `riverpackage`

> **Resolved 2026-04-29 by PR #96 (RW5, commit `4c0dda7`)** — All 3 findings addressed: `--config` wired into `discover_engines()` for engine validation (T2); `init` templates updated to produce bundles that pass Layer 1 validation for all 4 drivers (T3); `pack` now produces a `.zip` archive with correct file extension (T3). Golden CLI tests added for `init → validate` round-trips.

Findings:

- **T2:** ~~`--config` is silently ignored~~ so engine validation can be skipped despite an explicit config path.
- **T3:** ~~`init` generates bundles that fail `validate`.~~
- **T3:** ~~`pack` advertises zip output but creates tar.gz and no requested zip.~~

Fix direction:

- Wire `--config` into engine config loading or remove/reject the flag.
- Update scaffold templates to the current validator schema.
- Implement actual zip packaging or change the command contract.

### `rivers-plugin-ldap`

> **Partially resolved 2026-04-29 by PR #96 (RW4, commit `1bada09`)** — Shared `DEFAULT_TIMEOUT_MS` applied to LDAP connect/bind/search/modify operations (T2 timeout). Unbounded result set (T1), LDAPS/StartTLS (T2), and row caps remain deferred.
>
> **Further resolved 2026-04-30 (RW4.4.i, cb-p1-batch2 pending PR)** — TLS support added via `tls` option (`ldaps` → `ldaps://` URL port 636; `starttls` → `set_starttls(true)`; `tls_verify=false` → `set_no_tls_verify(true)`). WARN emitted when credentials present with `tls=none`. 4 unit tests. Unbounded result set and row caps remain deferred.

Findings:

- **T1:** LDAP search materializes unbounded result sets.
- **T2:** ~~Bind credentials are sent over plain LDAP only.~~ LDAPS/StartTLS now supported (RW4.4.i).
- **T2:** ~~LDAP network operations have no driver-level timeouts.~~

Fix direction:

- Use paged search with page and total caps.
- Support LDAPS/StartTLS before bind with certificate verification on by default.
- Add configured/default timeouts around connect, bind, search, add, modify, and delete.

### `rivers-plugin-neo4j`

Findings:

- **T1:** Transaction queries bypass the active Neo4j transaction.
- **T1:** `ping()` swallows row-stream errors.
- **T2:** Static plugin can be compiled but not registered.
- **T2:** `Null`, `Array`, and `Json` parameters are coerced to strings.
- **T2:** Result conversion silently drops temporal and other unsupported Bolt values.

Fix direction:

- Route execution through `Txn` when active.
- Propagate stream errors in `ping()`.
- Register Neo4j in static plugin inventory or remove default static feature.
- Bind native Bolt values and fail loudly on unsupported result values.

### `rivers-plugin-cassandra`

> **Partially resolved 2026-04-30 (RW4.2.b, cb-p1-batch2 pending PR)** — `max_rows: usize` field added (read via `read_max_rows(params)` at connect, default 10,000). `exec_query` truncates result set with `tracing::warn!` when exceeded. 2 unit tests. Paged execution remains deferred.

Findings:

- **T1:** Query path uses unpaged execution and materializes all rows. Row cap truncates with WARN (RW4.2.b); true paged execution deferred.
- **T2:** Write result reports `affected_rows: 1` for all writes, even though CQL write acknowledgement is not a row-count result.

Fix direction:

- Use paged execution with row caps.
- Report affected rows as unknown/0 unless the driver can prove a count.

### `rivers-plugin-mongodb`

> **Partially resolved 2026-04-30 (RW4.2.b, cb-p1-batch2 pending PR)** — `max_rows` now read via SDK `read_max_rows(params)` (was local `DEFAULT_MAX_ROWS = 1_000`); default is now 10,000, consistent with other plugins. 11 admin-guard unit tests added (DDL blocked, `drop_collection`/`drop_database`/`create_collection` blocked, normal ops pass). Transaction session attachment and broad update/delete defaults remain deferred.

Findings:

- **T1:** Transactions are exposed but CRUD methods do not attach the active `ClientSession`, so work executes outside the transaction.
- **T2:** ~~`find()` drains the cursor into an unbounded `Vec`.~~ Row cap applied via `read_max_rows(params)` (RW4.2.b); cursor still fully drained before truncation, not streamed.
- **T2:** Update/delete defaults are broad: update with no `_filter` uses `{}` and can update many documents.

Fix direction:

- Use session-aware MongoDB operations whenever `self.session` is active.
- Add result caps.
- Require explicit filters for multi-document update/delete or make broad operations opt-in.

### `rivers-plugin-elasticsearch`

> **Partially resolved 2026-04-29 by PR #96 (RW3, commit `50dea1b`; RW4, commit `1bada09`)** — `ddl_execute` now returns `Unsupported` with a clear message instead of silently succeeding (T2 admin gap); shared `DEFAULT_TIMEOUT_MS` and `DEFAULT_MAX_ROWS` applied (T2 timeout/cap); shared `url_encode_path_segment` helper applied to document ID and index path segments (T2). Auth-aware ping (T1), default index preference (T2), and response body size cap remain deferred.

Findings:

- **T1:** Authenticated clusters fail during connect because initial ping does not use auth-aware request path.
- **T2:** Configured default index is ignored.
- **T2:** ~~Admin operations are declared but cannot execute.~~ Now returns Unsupported.
- **T2:** ~~HTTP requests have no driver-level timeouts.~~
- **T2:** Response bodies are read without size limits.
- **T2:** ~~Document IDs are interpolated into URL paths unescaped.~~

Fix direction:

- Use auth-aware ping.
- Store and prefer configured default index.
- ~~Implement or remove admin operation support.~~
- ~~Add timeouts~~ and response caps.
- ~~Percent-encode path segments.~~

### `rivers-plugin-couchdb`

> **Partially resolved 2026-04-30 (RW4.2.b, cb-p1-batch2 pending PR)** — `max_rows` added to `CouchDBConnection`; `exec_find` and `exec_view` use `.take(max_rows+1)` + truncate+WARN pattern. Selector JSON safety, URL encoding, and HTTP-status-before-parse remain deferred.

Findings:

- **T1:** Selector placeholder substitution is string-based and not JSON-safe. String parameters are spliced into JSON source without escaping; bare placeholders are unquoted, and quoted placeholders can still be broken by embedded quotes or backslashes.
- **T2:** Document IDs, design doc names, view names, and revision query values are interpolated into URLs without segment encoding.
- **T2:** ~~`_find` and view responses are read and collected without driver-side size/row caps.~~ Row cap truncates with WARN (RW4.2.b).
- **T2:** `insert` parses the response body and returns success without checking HTTP status first.

Fix direction:

- Build Mango selectors structurally instead of replacing strings.
- Percent-encode all path and query segments.
- Enforce response and row caps.
- Check status before parsing/returning insert success.

### `rivers-plugin-influxdb`

> **Partially resolved 2026-04-29 by PR #96 (RW4, commit `1bada09`)** — Batching now clears buffer only after confirmed HTTP success (T1); line protocol escaping for measurement names, tag keys/values, field keys, and field strings (including backslash) applied per line protocol spec (T2); shared `DEFAULT_TIMEOUT_MS` applied as HTTP request timeout (T2). Batching URL bucket gap and response body size cap remain deferred.
>
> **Further resolved 2026-04-30 (RW4.2.b, cb-p1-batch2 pending PR)** — `max_rows` added to `InfluxConnection`; `exec_query` truncates CSV response rows with WARN when exceeded. Both `InfluxConnection` and `BatchingInfluxConnection.inner` receive `max_rows` from `params` at driver connect. Response body size cap remains deferred.

Findings:

- **T1:** ~~Batching clears buffered writes before the HTTP batch write succeeds~~ so failed flushes lose data.
- **T2:** Batching write URL omits the target bucket, unlike non-batched writes.
- **T2:** ~~Line protocol escaping is incomplete~~ measurement names are not escaped, and field strings do not escape backslashes.
- **T2:** ~~HTTP client uses default timeouts~~ and query responses are read fully into memory.

Fix direction:

- Only clear buffered writes after successful flush.
- Carry bucket per buffered line or reject batching when target bucket varies.
- Escape measurement, tag, field key, field string, and backslash rules per line protocol.
- Add request timeout and response caps.

### `rivers-plugin-redis-streams`

> **Partially resolved 2026-04-29 by PR #96 (RW2, commit `6bd29ae`)** — Static plugin registration registry added so the driver can be loaded in static mode (structural gap); `AckOutcome` SDK conformance applied. PEL reclaim/redelivery (T1), MAXLEN trimming (T2), and header persistence (T2) remain deferred.

Findings:

- **T1:** `nack()` leaves messages in PEL, but the consumer reads only new messages with `>` and has no `XAUTOCLAIM` / `XCLAIM` path.
- **T2:** Streams are unbounded because `XADD` does not use `MAXLEN` or `MINID`.
- **T2:** `OutboundMessage.headers` are ignored; only `payload` is stored.

Fix direction:

- Implement PEL reclaim and redelivery semantics, or return unsupported for `nack`.
- Add stream trimming config/defaults.
- Persist headers into stream fields and restore them on receive.

### `rivers-plugin-nats`

> **Partially resolved 2026-04-29 by PR #96 (RW2, commit `6bd29ae`; RW3, commit `50dea1b`)** — NATS now uses per-subject `mpsc` channels for fair dispatch across concurrent receivers (T1 queue fairness); NATS schema checker wired into `validate_syntax.rs` (T2). `ack()`/`nack()` semantics, consumer-group migration, multi-subscription support, and key suffix handling remain deferred.

Findings:

- **T1:** `ack()` / `nack()` report success without broker disposition.
- **T1:** Consumer group is constructed but not used; plain subscribe duplicates messages across nodes.
- **T2:** Only the first configured subscription is active.
- **T2:** `OutboundMessage.key` is documented as NATS subject suffix but ignored.
- **T2:** ~~NATS schema checker is unwired~~ and incomplete.

Fix direction:

- Use NATS queue subscriptions or JetStream durable consumers.
- Return unsupported for ack/nack unless real disposition is implemented.
- Support or reject multi-subscription configs.
- Implement key suffix behavior or reject it.

### `rivers-plugin-rabbitmq`

> **Partially resolved 2026-04-29 by PR #96 (RW2, commit `6bd29ae`; RW3, commit `50dea1b`)** — AMQP-406 double-ack detection added and `nack()` now uses `basic_reject` (T1 double-ack class); RabbitMQ schema checker wired into `validate_syntax.rs` (T2). Prefetch limit (`basic_qos`) and confirm timeout remain deferred.

Findings:

- **T1:** Consumer has no prefetch limit.
- **T2:** Publisher confirm wait has no timeout.
- **T2:** ~~RabbitMQ schema checker is unwired.~~

Fix direction:

- Call `basic_qos` before `basic_consume`.
- Add timeout around publish and confirm wait.
- Wire schema checker into deploy validation or remove it.

## Recommended Remediation Plan

### Phase 1: Stop Silent Security Failures

1. Fix `rivers-driver-sdk` DDL guard and sanitize forbidden errors.
2. Fix `rivers-plugin-exec` timeout/lifecycle/TOCTOU/privilege-drop issues.
3. Fix `riversctl` shutdown fallback and stop signal error handling.
4. Fix LockBox and keystore secret wrapper/debug/zeroization issues.

### Phase 2: Make Contracts Real

1. Define broker ack/nack semantics in `rivers-driver-sdk`.
2. Add conformance tests for `ack`, `nack`, requeue/redelivery, consumer groups, and multi-subscription behavior.
3. Fix NATS, Kafka, Redis Streams, and RabbitMQ against that contract.
4. Fix MongoDB and Neo4j transaction execution to use active sessions/transactions.

### Phase 3: Kill Unwired Features

1. Wire or remove schema checkers for NATS/RabbitMQ and admin DDL support for Elasticsearch.
2. Add a static plugin registration inventory test.
3. Add config-field consumption tests for `rivers-core-config`, `riverpackage --config`, and driver defaults.

### Phase 4: Add Shared Driver Guardrails

1. Shared timeouts for HTTP, LDAP, broker confirms, and external process I/O.
2. Shared response byte cap and row cap policy.
3. Shared URL path-segment encoder helper.
4. Shared line protocol / query construction tests for drivers that expose structured query builders.

### Phase 5: Make Tooling Honest

1. Fix `cargo-deploy` staging/atomicity and dynamic engine requirements.
2. Fix `riverpackage init` templates so generated bundles validate.
3. Fix `riverpackage pack` to produce the requested artifact type.
4. Add CLI golden tests for deploy/package/admin workflows.

## Review Heuristics To Add To CI

- `rg 'pub fn check_.*schema' crates/rivers-plugin-*` must have a production caller or explicit allowlist.
- `rg 'fn admin_operations' crates/rivers-plugin-*` must be paired with an implemented `ddl_execute` or documented unsupported behavior.
- `rg 'Client::new\\(\\)' crates/rivers-plugin-* crates/riversctl` should fail unless a timeout wrapper is used.
- `rg 'resp\\.text\\(\\)|resp\\.json\\(\\)' crates/rivers-plugin-*` should require a response-size/row-limit justification.
- `rg 'fs::write\\(|std::fs::write\\('` in secret/deploy crates should require restrictive temp-file write or explicit non-secret justification.
- `rg '#\\[derive\\(.*Debug.*\\)\\]'` in secret crates should require redacted manual debug or no secret fields.
- Broker plugin tests should use the same SDK contract fixtures for ack/nack/group behavior.

## Bottom Line

Rivers has enough implementation in place that the missing pieces are dangerous: many failures are not compile-time failures, they are "looks wired, returns success, does the wrong thing" failures. The next improvement should not be more feature surface. It should be a shared contract-hardening pass that makes drivers, secret stores, config, and tooling fail closed when behavior is unsupported or unsafe.
