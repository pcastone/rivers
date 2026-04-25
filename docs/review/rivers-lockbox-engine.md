# `rivers-lockbox-engine` Review

**Crate:** `crates/rivers-lockbox-engine`  
**Tier:** A, highest risk  
**Role reviewed:** Age-encrypted local secret resolver, key source resolution, startup reference validation, per-access secret fetch.

## Grounding

Confirmed from source:

- Read in full: `crates/rivers-lockbox-engine/Cargo.toml`.
- Read in full: `src/lib.rs`, `src/types.rs`, `src/crypto.rs`, `src/key_source.rs`, `src/resolver.rs`, `src/startup.rs`, `src/validation.rs`.
- Read in full: `tests/crypto_tests.rs`, `tests/key_source_tests.rs`, `tests/resolver_tests.rs`, `tests/startup_tests.rs`.
- Read in full for cross-crate wiring: `crates/riversd/src/bundle_loader/load.rs`, `crates/riversd/src/task_enrichment.rs`, `crates/riversd/src/process_pool/v8_engine/rivers_global.rs`, `crates/riversd/src/process_pool/v8_engine/task_locals.rs`, `crates/rivers-runtime/src/process_pool/types.rs`, `crates/rivers-runtime/src/process_pool/bridge.rs`, `crates/rivers-lockbox/src/main.rs`, `crates/rivers-core/src/lockbox.rs`, `crates/rivers-core-config/src/lockbox_config.rs`.
- Read in full for contract expectations: `docs/arch/rivers-lockbox-spec.md`.

Validation:

- `cargo check -p rivers-lockbox-engine` passed.
- `cargo test -p rivers-lockbox-engine` passed: 58 integration tests, 0 failures.

## Summary

The crate is small and the happy-path Age encryption/decryption tests pass. The weak point is not hand-rolled crypto; the weak point is secret lifecycle and cross-crate wiring. `rivers-lockbox-engine` exposes secret-bearing `String` types with derived `Debug`/`Clone`, relies on callers to zeroize returned values, and exports APIs that make it easy for runtime code to create extra unzeroized copies. The runtime then does exactly that in several places.

Bug density is concentrated in three areas:

- **Secret lifetime:** high density. Public secret containers are cloneable/debuggable, one has no `Drop`, and identity strings are cached outside the crate.
- **File handling:** medium density. Permission checks exist, but they are path-based and raceable; writes are non-atomic and chmod after write.
- **Crypto primitive choice:** comparatively clean. Age handles encryption/authentication; I did not find a custom KDF, nonce, or AEAD implementation inside this crate.

## Tier 1 Findings

### RLE-T1-1: Secret-bearing types are public, cloneable, debug-printable, and `ResolvedEntry` does not zeroize on drop

`Keystore` and `KeystoreEntry` derive `Debug` and `Clone`; `KeystoreEntry.value` is the plaintext secret field. `ResolvedEntry` also derives `Debug` and `Clone`, exposes `pub value: String`, and has no `Zeroize` or `Drop` impl. `fetch_secret_value()` returns `ResolvedEntry` by cloning `entry.value`, so the API creates a second plaintext allocation and leaves destruction to caller discipline.

Evidence:

- `Keystore` derives `Debug, Clone` at `crates/rivers-lockbox-engine/src/types.rs:96`.
- `KeystoreEntry` derives `Debug, Clone` and contains `pub value: String` at `crates/rivers-lockbox-engine/src/types.rs:128` and `crates/rivers-lockbox-engine/src/types.rs:136`.
- Only `KeystoreEntry.value` is zeroized in `Drop`; non-secret metadata is intentionally retained, but derived `Debug` still prints `value` before drop at `crates/rivers-lockbox-engine/src/types.rs:174`.
- `ResolvedEntry` derives `Debug, Clone`, contains `pub value: String`, and has no `Drop` at `crates/rivers-lockbox-engine/src/resolver.rs:51`.
- `fetch_secret_value()` clones the decrypted value into the returned entry at `crates/rivers-lockbox-engine/src/resolver.rs:183`.
- Runtime callers immediately clone again into longer-lived containers: app keystore master key at `crates/riversd/src/bundle_loader/load.rs:232` and datasource password at `crates/riversd/src/bundle_loader/load.rs:372`.

Impact:

Any `{:?}` on `Keystore`, `KeystoreEntry`, or `ResolvedEntry` prints plaintext secrets. Any clone creates an untracked plaintext allocation. `ResolvedEntry` can be dropped without zeroizing if a caller forgets the manual `resolved.value.zeroize()` step. That contradicts the crate-level claim that values are "read from disk, decrypted, used, and zeroized on every access."

Fix direction:

- Remove derived `Debug` from secret-bearing types; hand-write redacted debug impls.
- Remove `Clone` from secret-bearing types unless there is a specific redacted clone path.
- Change `value` fields to `Zeroizing<String>` or `SecretString`-style wrappers.
- Add `Drop`/`Zeroize` for `ResolvedEntry`.
- Prefer an API shape that accepts a closure, e.g. `with_secret_value(metadata, path, identity, |secret| ...)`, so the engine owns zeroization instead of trusting every caller.

### RLE-T1-2: Every handler can receive a LockBox resolver plus Age identity and use `Rivers.crypto.hmac()` as an arbitrary LockBox signing oracle

The spec says handler code does not interact with LockBox directly and that the isolate receives opaque datasource tokens, not credentials. The runtime now injects a whole LockBox capability into task state whenever a lockbox is configured: `sync_from_app_context()` reads the Age identity and stores it in a shared task capability; `enrich()` copies it into `TaskContext`; `TaskLocals::set()` clones it into `TASK_LOCKBOX`; `Rivers.crypto.hmac()` resolves whatever alias the handler passes and uses the secret as the HMAC key.

Evidence:

- Spec says raw credentials never enter the ProcessPool isolate and are resolved opaquely at `docs/arch/rivers-lockbox-spec.md:37`.
- Spec says no handler LockBox API exists in v1 at `docs/arch/rivers-lockbox-spec.md:679`.
- `SharedTaskCapabilities` stores `lockbox_identity: Option<String>` at `crates/riversd/src/task_enrichment.rs:23`.
- `sync_from_app_context()` calls `resolve_key_source()` and stores `identity.trim().to_string()` at `crates/riversd/src/task_enrichment.rs:34`.
- `enrich()` passes the resolver, path, and identity into the task builder at `crates/riversd/src/task_enrichment.rs:104`.
- `TaskContext` carries `lockbox_identity: Option<String>` at `crates/rivers-runtime/src/process_pool/types.rs:183`.
- `TaskContextBuilder::lockbox()` stores the identity string at `crates/rivers-runtime/src/process_pool/bridge.rs:192`.
- `TaskLocals::set()` clones that identity into `LockBoxContext` at `crates/riversd/src/process_pool/v8_engine/task_locals.rs:220`.
- `Rivers.crypto.hmac()` resolves `alias_or_key` against the resolver and calls `fetch_secret_value()` at `crates/riversd/src/process_pool/v8_engine/rivers_global.rs:261`.

Impact:

This is a capability boundary break. A handler that knows or guesses a LockBox alias cannot read the raw secret directly through this path, but it can make the host compute HMACs with that secret. That is still a use of the secret outside datasource connection setup, and it is not scoped to the app, datasource, or declared capability. The same wiring also keeps the Age identity in long-lived shared process state, task context state, and thread-local state.

Fix direction:

- Do not attach the global LockBox resolver and Age identity to every `TaskContext`.
- Remove LockBox alias resolution from `Rivers.crypto.hmac()` or require an explicit per-app capability allowlist mapping permitted aliases to intended operations.
- If HMAC-from-LockBox is a required feature, give it a dedicated scoped key resource, not the whole resolver.
- Store identity material in a zeroizing wrapper and avoid cloning it through shared task state.

### RLE-T1-3: The standalone `rivers-lockbox` CLI writes a different lockbox format than `rivers-lockbox-engine` reads

The spec says the LockBox CLI manages the Age-encrypted TOML `.rkeystore` consumed by `riversd`. The engine implements that `.rkeystore` model. The `rivers-lockbox` CLI does not call `encrypt_keystore()`/`decrypt_keystore()` for normal operations. It creates a directory with `identity.key`, `aliases.json`, and per-entry `entries/<name>.age` files, then `cmd_add()` writes each encrypted value as a separate `.age` file.

Evidence:

- Spec says secrets live in a local Age-encrypted keystore file managed by the CLI at `docs/arch/rivers-lockbox-spec.md:33`.
- Engine reads one configured `.rkeystore` path via `decrypt_keystore()` at `crates/rivers-lockbox-engine/src/crypto.rs:14`.
- CLI path helpers use `entries/`, `identity.key`, and `aliases.json` at `crates/rivers-lockbox/src/main.rs:111`.
- CLI `cmd_init()` creates an `entries` directory and writes `identity.key`, not an empty TOML `.rkeystore`, at `crates/rivers-lockbox/src/main.rs:147`.
- CLI `cmd_add()` writes `entries/<name>.age` at `crates/rivers-lockbox/src/main.rs:183`.
- CLI `cmd_show()` reads one per-entry `.age` file and prints the decrypted value at `crates/rivers-lockbox/src/main.rs:238`.
- The only engine function used by the CLI add path is `validate_entry_name()` at `crates/rivers-lockbox/src/main.rs:180`.

Impact:

An operator using `rivers-lockbox init/add` does not create the file shape that `riversd` expects in `[lockbox].path`. That is a production wiring break between the management tool and the engine. It also defeats several spec security properties: the CLI accepts `--value` at `crates/rivers-lockbox/src/main.rs:31` and `crates/rivers-lockbox/src/main.rs:52`, and `show` prints secrets without confirmation at `crates/rivers-lockbox/src/main.rs:248`, while the spec explicitly says stdin-only and confirmation-gated display.

Fix direction:

- Make `rivers-lockbox` use `rivers-lockbox-engine::{encrypt_keystore, decrypt_keystore, Keystore}` as its canonical storage layer.
- Add a CLI/runtime compatibility test: create via CLI, decrypt via engine, and startup-resolve via `riversd` config.
- Remove `--value` or loudly restrict it to a test-only flag behind an explicit unsafe option.
- Add confirmation gating for `show`.

## Tier 2 Findings

### RLE-T2-1: Permission checks are path-based and raceable; encryption writes before chmod and is not atomic

`check_file_permissions()` uses `std::fs::metadata(path)`, which follows symlinks, then the caller reads the path later with `std::fs::read()` or `read_to_string()`. There is no single opened file descriptor whose metadata is checked with `fstat` and then read. `encrypt_keystore()` writes directly to the target path and only afterwards sets `0o600`.

Evidence:

- Metadata check uses `std::fs::metadata(path)` at `crates/rivers-lockbox-engine/src/key_source.rs:72`.
- Key source file is checked, then read separately at `crates/rivers-lockbox-engine/src/key_source.rs:42`.
- Keystore startup checks permissions at `crates/rivers-lockbox-engine/src/startup.rs:106`, then decrypts by path at `crates/rivers-lockbox-engine/src/startup.rs:112`.
- `decrypt_keystore()` reads by path at `crates/rivers-lockbox-engine/src/crypto.rs:19`.
- `encrypt_keystore()` writes directly at `crates/rivers-lockbox-engine/src/crypto.rs:83`, then chmods at `crates/rivers-lockbox-engine/src/crypto.rs:86`.
- Spec expects CLI modifications to write a temp file and rename atomically; see `docs/arch/rivers-lockbox-spec.md:332`.

Impact:

An attacker with write access to a parent directory can race the permission check and file read, or can swap a symlink target between operations. On creation/update, there is a window where file permissions are governed by process umask before the explicit chmod. Direct overwrite also risks partial files on crash.

Fix direction:

- Open the file once with no-follow semantics where available, check metadata on the opened handle, then read from that handle.
- Reject symlinks for key files and keystore files unless there is a deliberately documented safe policy.
- Write encrypted output to a `0o600` temp file opened with restrictive permissions, `fsync`, then rename over the target.
- Add symlink and swap-race regression tests where platform support allows.

### RLE-T2-2: Unknown entry types silently downgrade to `String`

`EntryType::parse()` returns `None` for unknown strings, but `LockBoxResolver::from_entries()` uses `unwrap_or(EntryType::String)`. The test suite locks in that behavior.

Evidence:

- `EntryType::parse()` returns `None` for unknown values at `crates/rivers-lockbox-engine/src/types.rs:189`.
- Resolver silently defaults unknown types to `EntryType::String` at `crates/rivers-lockbox-engine/src/resolver.rs:94`.
- Test `unknown_entry_type_defaults_to_string` asserts this behavior in `crates/rivers-lockbox-engine/tests/resolver_tests.rs:309`.

Impact:

A typo like `base64_url` or `jsn` does not fail startup. It changes how the host and downstream driver interpret the credential. In a secrets engine, malformed secret metadata should fail closed.

Fix direction:

- Add a `LockBoxError::InvalidEntryType { name, entry_type }`.
- Make `from_entries()` fail on unknown `entry_type`.
- Update tests to assert rejection, not silent fallback.

### RLE-T2-3: `key_source = "agent"` is advertised by config and spec but is hard-failed by the engine

`LockBoxConfig` exposes `agent_socket` and `recipient_file`, and the spec describes agent support. The engine always returns `KeySourceUnavailable` for `"agent"`.

Evidence:

- Config exposes `agent_socket` and `recipient_file` at `crates/rivers-core-config/src/lockbox_config.rs:25`.
- Spec describes agent key source support at `docs/arch/rivers-lockbox-spec.md:308`.
- Engine returns "not yet supported" for `"agent"` at `crates/rivers-lockbox-engine/src/key_source.rs:50`.
- Test `resolve_key_source_agent_unsupported` locks in the failure in `crates/rivers-lockbox-engine/tests/key_source_tests.rs:136`.

Impact:

This is a contract violation that will surprise production operators choosing the most secure key-source mode. It is better to reject the config at validation time with a clearly documented unsupported feature than to present it as supported.

Fix direction:

- Either implement agent support or remove it from accepted config/spec examples.
- If deferred, make bundle/server config validation fail with a feature-status error before startup reaches secret resolution.

### RLE-T2-4: `key_source = "env"` no longer clears the process environment, contrary to the spec

The spec says Rivers reads `RIVERS_LOCKBOX_KEY`, decrypts the keystore, then clears the variable from the process environment. The engine explicitly stopped removing it because `std::env::remove_var` is unsafe once other threads can read environment variables.

Evidence:

- Spec says the env var is cleared after startup at `docs/arch/rivers-lockbox-spec.md:291`.
- Engine comment says the variable remains set at `crates/rivers-lockbox-engine/src/key_source.rs:20`.

Impact:

This is an honest Rust safety tradeoff, but the documented security property is false. In environments where process environment can be inspected by same-user processes, crash reporters, debuggers, or child processes, the Age identity persists longer than expected.

Fix direction:

- Update the spec and deployment docs to state that env source is development-only and is not cleared after multithreaded startup begins.
- Prefer file or agent source for production.
- If env clearing is still desired, read and clear it before Tokio/runtime thread startup, then pass the identity in a zeroizing process-local object.

## Tier 3 Findings

### RLE-T3-1: Memory locking is absent for secret-bearing pages

The crate does not use `mlock`, `VirtualLock`, or a secure allocator. This was expected but not required by the user-provided threat model.

Evidence:

- Mechanical sweep found no `mlock`, `VirtualLock`, secure allocator, `SecretString`, or `Zeroizing` usage in production lockbox code.

Impact:

Secrets can be paged out or captured by process memory snapshots. This is not a regression from the current design, but it is a gap for a highest-risk secrets engine.

Fix direction:

- Decide explicitly whether memory locking is a v1 requirement.
- If yes, centralize secret buffers behind a small wrapper that can zeroize and request page locking where platform support exists.

## Non-Finding Observations

- Age is delegated correctly for envelope encryption/authentication. I did not find custom nonce generation, custom AEAD, or custom KDF code inside this crate.
- The resolver does store metadata only, not secret values, after startup. `LockBoxResolver` has a redacted custom `Debug` impl.
- Unix mode checks reject files not exactly `0o600` and directories not exactly `0o700`.
- I did not find direct secret comparisons in this crate. Ordinary equality is used for names, aliases, and config strings, which are not treated as secret material.

## Coverage Notes

Covered by tests:

- Encrypt/decrypt round trip, wrong-key failure, malformed UTF-8/TOML, invalid identity/recipient.
- Unix permission rejection and final `0o600` mode after `encrypt_keystore()`.
- Resolver duplicate names/aliases, invalid names, alias lookup, metadata-only resolver behavior.
- Startup missing config, relative path, missing file, insecure permissions, wrong key, missing reference.

Not covered:

- Secret-bearing `Debug` output never includes values.
- `ResolvedEntry` zeroizes automatically on drop.
- Runtime callers do not create long-lived unzeroized clones.
- File permission check/read TOCTOU and symlink behavior.
- Atomic write semantics for `.rkeystore`.
- CLI-created lockboxes are readable by `rivers-lockbox-engine`.
- Handler-level access control for LockBox-backed `Rivers.crypto.hmac()`.

## Recommended Shared Fix

The repeated pattern is manual secret lifecycle management via bare `String`. Fixing only one caller will leave the same bug class elsewhere. Introduce a shared lockbox secret type with:

- redacted `Debug`,
- no accidental `Clone`,
- automatic `Zeroize` on drop,
- optional memory locking policy,
- closure-based access where practical,
- explicit conversion points for driver APIs that still require `String`.

Then replace `ResolvedEntry.value`, Age identity strings, app keystore master keys, and datasource passwords at the lockbox boundary with that type. The engine should own zeroization wherever it can; callers should not have to remember to clean up after every fetch.
