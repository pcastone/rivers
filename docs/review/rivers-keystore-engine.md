# `rivers-keystore-engine` Review

**Crate:** `crates/rivers-keystore-engine`
**Tier:** A, highest risk
**Role reviewed:** Application encryption key storage engine, key versioning, encrypted file I/O, handler-facing encrypt/decrypt wiring through `riversd`.

## Grounding

Confirmed from source:

- Read in full: `crates/rivers-keystore-engine/Cargo.toml`.
- Read in full: `crates/rivers-keystore-engine/src/lib.rs`, `src/types.rs`, `src/crypto.rs`, `src/key_management.rs`, `src/io.rs`.
- Read in full: `crates/rivers-keystore-engine/tests/crypto_tests.rs`, `tests/key_management_tests.rs`, `tests/io_tests.rs`, `tests/integration_test.rs`.
- Read in full for CLI/runtime wiring: `crates/rivers-keystore/src/main.rs`, `crates/riversd/src/keystore.rs`, `crates/riversd/src/bundle_loader/load.rs`, `crates/riversd/src/task_enrichment.rs`, `crates/rivers-runtime/src/process_pool/types.rs`, `crates/rivers-runtime/src/process_pool/bridge.rs`, `crates/riversd/src/process_pool/mod.rs`, `crates/riversd/src/process_pool/v8_engine/task_locals.rs`, `crates/riversd/src/process_pool/v8_engine/rivers_global.rs`, `crates/riversd/src/process_pool/wasm_engine.rs`, `crates/riversd/src/engine_loader/host_context.rs`, `crates/riversd/src/engine_loader/host_callbacks.rs`, `crates/riversd/src/server/lifecycle.rs`, `crates/rivers-runtime/src/bundle.rs`, `crates/rivers-runtime/src/validate.rs`.
- Read in full for public contract comparison: `docs/guide/tutorials/tutorial-app-keystore.md`, `docs/guide/developer.md`, `docs/arch/rivers-javascript-typescript-spec.md`.

Validation:

- `cargo check -p rivers-keystore-engine` passed.
- `cargo test -p rivers-keystore-engine` passed: 98 tests plus doc-tests, 0 failures.
- `cargo check -p riversd` passed with pre-existing warnings.

## Summary

The crypto happy path is comparatively solid: AES-256-GCM uses random 96-bit nonces from `OsRng`, Age handles at-rest encryption/authentication, generated raw key bytes are zeroized after Base64 encoding, and the convenience wrappers zeroize decoded key bytes after encrypt/decrypt.

The bug density is high at the edges, not inside the AES primitive. The crate exposes key-bearing structs as public, `Debug`, and `Clone`; accepts decrypted TOML without validating key invariants; writes without durability or locking; and runtime wiring silently picks an arbitrary app keystore when more than one exists. Dynamic engine callbacks are also wired to a `HOST_KEYSTORE` global that is never set.

Cleanest areas:

- Nonce generation and AEAD usage.
- Basic key generation/rotation for crate-created keystores.
- Basic CLI/runtime round-trip format compatibility.

Bug-dense areas:

- Secret lifecycle and debug/clone exposure.
- Cross-crate app-keystore scoping.
- File update safety and concurrent CLI operation.
- Load-time validation of authenticated but malformed state.

## Tier 1 Findings

### RKE-T1-1: Secret-bearing keystore structs are public, cloneable, and debug-printable

`AppKeystore`, `AppKeystoreKey`, and `KeyVersion` all derive `Debug` and `Clone`; `KeyVersion.key_material` is the Base64-encoded AES-256 key material. `AppKeystore.keys`, `AppKeystoreKey.versions`, and `KeyVersion.key_material` are public fields, so any caller can print or clone the entire decrypted keystore. The crate does implement `Drop`/`Zeroize`, but derived `Clone` creates additional independent key-material allocations, and derived `Debug` prints the key material before drop.

Evidence:

- `AppKeystore` derives `Debug, Clone` and exposes `pub keys` at `crates/rivers-keystore-engine/src/types.rs:97`.
- `AppKeystoreKey` derives `Debug, Clone` and exposes `pub versions` at `crates/rivers-keystore-engine/src/types.rs:108`.
- `KeyVersion` derives `Debug, Clone` and exposes `pub key_material: String` at `crates/rivers-keystore-engine/src/types.rs:125`.
- Zeroization only happens on drop for the current allocation at `crates/rivers-keystore-engine/src/types.rs:169`, `crates/rivers-keystore-engine/src/types.rs:181`, and `crates/rivers-keystore-engine/src/types.rs:196`.
- `current_key_bytes()` and `versioned_key_bytes()` return raw `Vec<u8>` and rely on caller discipline to zeroize at `crates/rivers-keystore-engine/src/key_management.rs:179` and `crates/rivers-keystore-engine/src/key_management.rs:220`.

Impact:

One accidental `tracing::debug!("{:?}", keystore)`, panic dump, test failure output, or clone leaks every app encryption key. This directly contradicts the public tutorial guarantee that key bytes never leave Rust memory; the raw bytes may not cross into handler memory, but key material is easy to duplicate and print in host-side Rust.

Fix direction:

- Remove `Debug` and `Clone` from `AppKeystore`, `AppKeystoreKey`, and `KeyVersion`.
- Implement redacted `Debug` manually.
- Make key-material fields private.
- Store key material in `Zeroizing<String>` or a dedicated secret wrapper that is redacted, non-cloneable by default, and zeroizes automatically.
- Prefer closure APIs such as `with_current_key_bytes(name, |key| ...)` over returning raw `Vec<u8>`.

### RKE-T1-2: Multiple app keystores are allowed by config but handler dispatch picks an arbitrary first match

The config model allows `resources.toml` to declare multiple `[[keystores]]` entries. `load_and_wire_bundle()` loads each one under `"{entry_point}:{keystore_name}"`. But handler wiring cannot choose by keystore name: `KeystoreResolver::get_for_entry_point()` scans a `HashMap` and returns the first scoped key whose name starts with `"{entry_point}:"`. V8 and WASM both use that fallback, while `Rivers.crypto.encrypt("key-name", ...)` takes only a key name.

Evidence:

- `ResourcesConfig.keystores` is a `Vec<ResourceKeystore>` at `crates/rivers-runtime/src/bundle.rs:153`.
- Bundle validation checks matching declarations, non-empty lockbox aliases, and duplicate keystore names, but does not reject multiple distinct keystores for one app at `crates/rivers-runtime/src/validate.rs:237`.
- `load_and_wire_bundle()` inserts every app keystore under `"{entry_point}:{name}"` at `crates/riversd/src/bundle_loader/load.rs:242`.
- `get_for_entry_point()` returns the first `HashMap` entry matching the prefix at `crates/riversd/src/keystore.rs:38`.
- V8 fallback calls `resolver.get_for_entry_point(&ctx.app_id)` at `crates/riversd/src/process_pool/v8_engine/task_locals.rs:233`.
- WASM uses the same fallback at `crates/riversd/src/process_pool/wasm_engine.rs:93`.
- The tutorial promises application-scoped keys isolated from other apps at `docs/guide/tutorials/tutorial-app-keystore.md:13` and shows handlers referring only to `"credential-key"` at `docs/guide/tutorials/tutorial-app-keystore.md:207`.

Impact:

An app with two keystores can encrypt with one keystore on one run and a different one after a hash-map order change, hot reload, or unrelated insertion pattern change. Decrypt can then fail for existing ciphertext, or worse, a same-named key in the wrong keystore can be used silently. This is exactly the kind of code that passes single-keystore tests and fails the first serious multi-app bundle.

Fix direction:

- Either validate that each app declares at most one app keystore, or make the handler API name the keystore explicitly.
- If multiple keystores are intended, change runtime context to carry a deterministic map of keystore name to `Arc<AppKeystore>`.
- Add tests with two keystores in one app and same key names in both.

### RKE-T1-3: Dynamic engine keystore callbacks are registered but no code ever wires `HOST_KEYSTORE`

Dynamic engine callbacks include `keystore_has`, `keystore_info`, `crypto_encrypt`, and `crypto_decrypt`, all reading the global `HOST_KEYSTORE`. `set_host_keystore()` exists, but workspace search found no caller other than the re-export. So dynamic engine callbacks always return "not configured" style failures. Even if it were called, `HOST_KEYSTORE` is a single `OnceLock<Arc<AppKeystore>>`, which cannot represent per-app or hot-reloaded keystores.

Evidence:

- `HOST_KEYSTORE` is a process-global `OnceLock` at `crates/riversd/src/engine_loader/host_context.rs:22`.
- `set_host_keystore()` exists at `crates/riversd/src/engine_loader/host_context.rs:53`.
- `rg set_host_keystore` found only the function and its re-export in `crates/riversd/src/engine_loader/mod.rs`; no runtime caller.
- Dynamic callbacks read `HOST_KEYSTORE` for metadata and crypto operations at `crates/riversd/src/engine_loader/host_callbacks.rs:521`, `crates/riversd/src/engine_loader/host_callbacks.rs:552`, `crates/riversd/src/engine_loader/host_callbacks.rs:592`, and `crates/riversd/src/engine_loader/host_callbacks.rs:637`.

Impact:

Static V8/WASM paths can use app keystores, but dynamic engine mode has a registered callback table that looks supported and then fails at runtime. That is a cross-crate wiring gap: the callback exists, the engine SDK exposes it, the runtime advertises dynamic mode, but the state is never populated.

Fix direction:

- Remove the single `HOST_KEYSTORE` global.
- Pass app identity through dynamic callback inputs and resolve against the same `KeystoreResolver` used by static engines.
- If dynamic callbacks cannot be made app-scoped yet, omit these callbacks from `HostCallbacks` in dynamic mode and fail loudly during engine capability negotiation.

## Tier 2 Findings

### RKE-T2-1: `load()` authenticates the file but does not validate keystore invariants

`AppKeystore::load()` decrypts the Age envelope and deserializes TOML, then returns the struct as-is. It does not validate schema version, duplicate key names, supported `key_type`, duplicate version numbers, version `0`, empty version lists, or whether `current_version` exists. The mutation methods validate only state they create themselves.

Evidence:

- `load()` returns the deserialized `AppKeystore` immediately after TOML parsing at `crates/rivers-keystore-engine/src/io.rs:51`.
- `generate_key()` rejects unsupported key types and duplicates only for newly generated keys at `crates/rivers-keystore-engine/src/key_management.rs:23`.
- `get_key()` returns the first name match at `crates/rivers-keystore-engine/src/key_management.rs:68`.
- `get_key_version()` returns the first version match at `crates/rivers-keystore-engine/src/key_management.rs:73`.
- `rotate_key()` computes `key.current_version + 1` without overflow or consistency checks at `crates/rivers-keystore-engine/src/key_management.rs:149`.

Impact:

Authenticated-but-malformed state can silently change behavior. Duplicate key names or duplicate versions make "first one wins" decisions. Unsupported key types can sit in memory and appear in metadata. A malformed `current_version = 4294967295` can panic in debug builds or wrap in release during rotation. This is not an attacker-forged-file problem; it is a migration, manual edit, old bug, or bad CLI-version problem.

Fix direction:

- Add `AppKeystore::validate()` and call it at the end of `load()` and before `save()`.
- Enforce version `1`, unique names, supported key types, unique positive version numbers, current version existence, monotonic version order, and valid Base64 length for every version.
- Add regression tests that handcraft malformed encrypted TOML and assert load fails closed.

### RKE-T2-2: Keystore file writes are rename-based but not durable, locked, or permission-checked on load

`save()` writes to a temp file and persists it, which is good for avoiding a simple torn overwrite. But it never `sync_all()`s the temp file or parent directory, does not lock the keystore during load-modify-save, and `load()` does not reject world-readable keystore files. The CLI performs a classic load-modify-save sequence for generate/delete/rotate with no interprocess lock.

Evidence:

- `load()` reads the path directly with `std::fs::read` and performs no permission check at `crates/rivers-keystore-engine/src/io.rs:24`.
- `save()` writes to `NamedTempFile`, sets mode `0o600`, and persists at `crates/rivers-keystore-engine/src/io.rs:89`.
- There is no `sync_all`, directory fsync, lock file, or advisory lock in `io.rs`.
- CLI `generate`, `delete`, and `rotate` each load, mutate, and save without locking at `crates/rivers-keystore/src/main.rs:122`, `crates/rivers-keystore/src/main.rs:166`, and `crates/rivers-keystore/src/main.rs:176`.
- Tests assert final file mode only, not load rejection, durability, or concurrent updates.

Impact:

Two admins or automation jobs rotating/generating keys at the same time can lose one update. A crash after rename but before directory metadata is durable can lose the new keystore on filesystems that require explicit sync. A world-readable keystore file is still accepted at runtime if someone creates or copies it with bad permissions.

Fix direction:

- Reject insecure modes on `load()` on Unix, mirroring LockBox's stricter posture.
- Use a sidecar lock file or platform file locks around load-modify-save CLI operations.
- `sync_all()` the temp file before persist and fsync the parent directory after persist where supported.
- Consider keeping a backup of the previous encrypted file on successful write.

### RKE-T2-3: Master-key identity strings are copied and not zeroized in CLI and runtime unlock paths

The CLI reads `RIVERS_KEYSTORE_KEY` into a `String`, trims it into a second `String`, and returns it without a zeroizing wrapper. Runtime unlock resolves a LockBox value, clones it into `master_key`, zeroizes only the original `resolved.value`, and passes the clone to `AppKeystore::load()`. The engine API accepts `&str`, so it cannot own cleanup of the Age identity.

Evidence:

- CLI `read_identity()` creates `key` and `trimmed` `String`s at `crates/rivers-keystore/src/main.rs:87`.
- Runtime unlock creates `identity_str`, fetches the LockBox entry, clones `resolved.value` into `master_key`, and zeroizes only `resolved.value` at `crates/riversd/src/bundle_loader/load.rs:220`.
- `AppKeystore::load()` takes `identity_str: &str` and parses it at `crates/rivers-keystore-engine/src/io.rs:24`.

Impact:

The Age identity that unlocks the keystore lives in ordinary heap strings outside the engine's zeroization discipline. This repeats the same bare-`String` secret-lifecycle pattern found in the LockBox review and makes the stated "master key lifecycle" boundary dependent on caller luck.

Fix direction:

- Use `Zeroizing<String>` or a shared redacted secret-string wrapper for Age identities.
- Have CLI and runtime pass a secret wrapper into `load()` or use a closure-based unlock API that owns zeroization.
- Avoid cloning the LockBox value into `master_key`; move it into a zeroizing holder.

## Tier 3 Findings

### RKE-T3-1: Standalone `decrypt()` says all failures are generic but exposes nonce parse/length details

The Rust API comment says "On any failure returns a generic `DecryptionFailed` error (no oracle)." Ciphertext Base64 errors and tag failures do return `DecryptionFailed`, but nonce Base64 and nonce length errors return `InvalidNonce` with detail. V8/WASM wrappers mostly normalize these for handler code, but direct Rust callers see the distinction.

Evidence:

- Generic-failure comment at `crates/rivers-keystore-engine/src/crypto.rs:49`.
- Invalid nonce Base64 and length return `InvalidNonce` at `crates/rivers-keystore-engine/src/crypto.rs:69`.
- V8 decrypt normalizes most non-key lookup errors to `"decryption failed"` at `crates/riversd/src/process_pool/v8_engine/rivers_global.rs:439`.

Impact:

This is low severity because nonce format is not secret and handler-facing paths normalize it. But the crate-level contract is false, and secrets code should not have misleading security comments.

Fix direction:

- Either make standalone `decrypt()` return `DecryptionFailed` for nonce parse/length errors, or update the comment to say malformed message framing is reported separately.

### RKE-T3-2: No memory locking or secure allocator for resident app keys

Unlocked app keystores live in `Arc<AppKeystore>` for the app lifetime. They are zeroized on drop, but there is no page locking, secure allocator, or swap avoidance.

Evidence:

- Runtime stores unlocked keystores in `Arc<AppKeystore>` at `crates/riversd/src/keystore.rs:11`.
- Mechanical sweep found no `mlock`, `VirtualLock`, secure allocator, or `Zeroizing` wrapper for resident key material.

Impact:

Keys can be paged out or captured by process memory snapshots. This may be acceptable for v1, but it should be an explicit posture for a Tier A key engine.

Fix direction:

- Decide whether memory locking is a product requirement.
- If yes, centralize resident app key material behind a wrapper that can request page locking and redacted diagnostics.

## Repeated Pattern

The repeated Rivers-wide bug class is **manual secret lifecycle via bare `String`/`Vec<u8>`**. It appears here in `KeyVersion.key_material`, returned raw key byte vectors, CLI `RIVERS_KEYSTORE_KEY`, runtime `master_key`, and the LockBox values already documented in `docs/review/rivers-lockbox-engine.md`.

Shared fix:

- Add a small shared secret type with redacted `Debug`, no accidental clone, `Zeroize` on drop, optional memory-lock policy, and explicit expose/closure methods.
- Use it for LockBox values, Age identities, app keystore key material, datasource passwords, and any decoded AES key bytes.

## Coverage Notes

Covered by tests:

- AES-GCM round trips, AAD, wrong-key/tamper failures, nonce length failures.
- Key generation, duplicate generated key rejection, rotation, old-version decryption.
- Persistence round trip through Age-encrypted file.
- Final Unix file mode `0600`.
- Handler-facing V8 keystore happy paths in `riversd` tests.

Not covered:

- Debug output never prints key material.
- Secret-bearing clone is impossible or tracked.
- Loaded keystore structural validation for duplicate names/versions, bad key type, empty versions, or missing current version.
- Concurrent CLI update races.
- `load()` rejection of insecure file permissions.
- File and directory fsync durability.
- Multiple keystores in one app.
- Dynamic engine keystore callback availability.

## Recommended Fix Order

1. Lock down secret-bearing types: private fields, redacted debug, remove clone, secret wrappers.
2. Add load/save invariant validation.
3. Fix app-keystore scoping: one-keystore validation or explicit keystore-name API.
4. Add load permission checks, fsync, and CLI locking.
5. Replace dynamic `HOST_KEYSTORE` with app-scoped resolver wiring or remove unsupported callbacks.
