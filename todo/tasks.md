# Tasks — Application Keystore & Encryption API

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an application-scoped keystore and AES-256-GCM encryption API accessible from CodeComponent handlers.

**Spec:** `docs/rivers-feature-request-app-keystore.md`

**Architecture:** New `rivers-keystore-engine` crate provides the keystore file format (Age-encrypted TOML), key management, and AES-256-GCM operations. A `rivers-keystore` CLI manages keystore files. The keystore integrates with the existing Lockbox resource pattern: declared in `resources.toml`, master key from Lockbox, config in `app.toml`. Host functions expose `Rivers.keystore.has/info` and `Rivers.crypto.encrypt/decrypt` to V8 and WASM handlers. Key bytes stay in Rust memory — never exposed to JS/WASM heap.

**Tech Stack:** Rust, `aes-gcm` crate, `age` (existing), V8/WASM host functions

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `crates/rivers-keystore-engine/Cargo.toml` | Crate manifest — depends on age, aes-gcm, serde, zeroize, chrono, toml, base64, rand |
| `crates/rivers-keystore-engine/src/lib.rs` | AppKeystore types, file format (Age-encrypted TOML), AES-256-GCM encrypt/decrypt, key generation/rotation |
| `crates/rivers-keystore/Cargo.toml` | CLI binary manifest — depends on rivers-keystore-engine, clap |
| `crates/rivers-keystore/src/main.rs` | CLI: init, generate, list, info, delete, rotate |

### Modified Files

| File | Changes |
|------|---------|
| `Cargo.toml` (workspace root) | Add new crates to members, add `aes-gcm = "0.10"` to workspace deps |
| `crates/rivers-runtime/Cargo.toml` | Add `rivers-keystore-engine` dep behind `keystore` feature |
| `crates/rivers-runtime/src/bundle.rs` | Add `ResourceKeystore` struct, `keystores` field to `ResourcesConfig`, `KeystoreDataConfig` to `AppDataConfig` |
| `crates/rivers-runtime/src/lib.rs` | Re-export keystore types |
| `crates/rivers-runtime/src/validate.rs` | Keystore validation checks (file existence, lockbox alias, name match) |
| `crates/rivers-runtime/src/process_pool/types.rs` | Keystore fields on `TaskContext` |
| `crates/rivers-engine-sdk/src/lib.rs` | `keystore_available` on `SerializedTaskContext`, new callback slots on `HostCallbacks` |
| `crates/riversd/src/bundle_loader.rs` | Keystore master key resolution from Lockbox, unlock at startup |
| `crates/riversd/src/server.rs` | `keystore_resolver` field on `AppContext` |
| `crates/riversd/src/process_pool/v8_engine.rs` | `TASK_KEYSTORE` thread-local, `Rivers.keystore.has/info`, `Rivers.crypto.encrypt/decrypt` |
| `crates/riversd/src/process_pool/wasm_engine.rs` | WASM host function bindings for keystore + encrypt/decrypt |
| `crates/riversd/src/engine_loader.rs` | Host callback implementations for dynamic engines |
| `crates/riversd/Cargo.toml` | Add `aes-gcm` dependency |
| `crates/riversctl/src/main.rs` | Keystore validation checks in `cmd_validate()` |

---

## 1. Keystore Engine Crate — Types, Errors, File Format

Create `rivers-keystore-engine` — the core library for app keystore management. Follows the same pattern as `rivers-lockbox-engine`: Age-encrypted TOML file, serde types, Zeroize on drop.

**Create:** `crates/rivers-keystore-engine/Cargo.toml`, `crates/rivers-keystore-engine/src/lib.rs`
**Modify:** `Cargo.toml` (workspace root)
**Reference:** `crates/rivers-lockbox-engine/src/lib.rs` (Keystore model, Age encryption pattern)

### Key Types

```rust
/// Plaintext TOML schema inside the Age envelope.
pub struct AppKeystore {
    pub version: u32,                    // file format version (1)
    pub keys: Vec<AppKeystoreKey>,
}

/// A named encryption key with version history.
pub struct AppKeystoreKey {
    pub name: String,                    // e.g. "credential-key"
    pub key_type: String,                // "aes-256"
    pub current_version: u32,            // latest version number
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub versions: Vec<KeyVersion>,       // all versions (old kept for decrypt)
}

/// A single version of a key's material.
pub struct KeyVersion {
    pub version: u32,
    pub key_material: String,            // base64-encoded raw bytes (zeroized on drop)
    pub created: DateTime<Utc>,
}

/// Result from an encrypt operation.
pub struct EncryptResult {
    pub ciphertext: String,              // base64
    pub nonce: String,                   // base64 (96-bit)
    pub key_version: u32,
}

pub enum AppKeystoreError {
    KeystoreNotFound { path: String },
    DecryptionFailed,                    // generic — no oracle
    MalformedKeystore { reason: String },
    KeyNotFound { name: String },
    KeyVersionNotFound { name: String, version: u32 },
    InvalidKeyType { expected: String, got: String },
    InvalidKeyLength { expected: usize, got: usize },
    InvalidNonce { reason: String },
    DuplicateKey { name: String },
    KeystoreNotConfigured,
    KeystoreLocked { alias: String },
    Io(std::io::Error),
}
```

### Steps

- [ ] **T1.1** Create `crates/rivers-keystore-engine/Cargo.toml`
  - `crate-type = ["rlib"]`
  - Dependencies: `rivers-core-config` (path), `age` (workspace), `aes-gcm = "0.10"`, `serde` (workspace), `zeroize` (workspace), `chrono` (workspace), `toml` (workspace), `thiserror` (workspace), `base64` (workspace), `rand` (workspace)

- [ ] **T1.2** Add `rivers-keystore-engine` to workspace `members` in root `Cargo.toml`; add `aes-gcm = "0.10"` to `[workspace.dependencies]`

- [ ] **T1.3** Implement `AppKeystoreError` enum with `thiserror` (all variants listed above)

- [ ] **T1.4** Implement `AppKeystore`, `AppKeystoreKey`, `KeyVersion` with serde Serialize/Deserialize. Add `Zeroize` + `Drop` on `KeyVersion` and `AppKeystoreKey` (zeroize `key_material`). Follow the `Keystore` impl in `rivers-lockbox-engine/src/lib.rs:84-109`.

- [ ] **T1.5** Implement `AppKeystore::create(path, recipient_key)` — creates empty keystore (version=1, keys=[]), serializes to TOML, encrypts with Age, writes to file. Follow the Age encryption pattern from `rivers-lockbox-engine`.

- [ ] **T1.6** Implement `AppKeystore::load(path, identity_str)` — reads file, decrypts with Age identity, parses TOML into `AppKeystore`. Return `AppKeystoreError::KeystoreNotFound` if file missing, `DecryptionFailed` if wrong key.

- [ ] **T1.7** Implement `AppKeystore::save(path, recipient_key)` — serializes to TOML, encrypts with Age, writes file (atomic via tempfile + rename).

- [ ] **T1.8** Implement key management methods on `AppKeystore`:
  - `generate_key(name, key_type) -> Result<&AppKeystoreKey>` — validates key_type is "aes-256", generates 32 random bytes via `OsRng`, creates version 1, appends to keys. Error if name already exists.
  - `get_key(name) -> Option<&AppKeystoreKey>` — lookup by name
  - `get_key_version(name, version) -> Result<&KeyVersion>` — specific version lookup
  - `has_key(name) -> bool`
  - `key_info(name) -> Result<KeyInfo>` — returns name, type, current_version, created_at (no raw bytes)
  - `list_keys() -> Vec<KeyInfo>`
  - `rotate_key(name) -> Result<u32>` — generates new version N+1, returns new version number
  - `delete_key(name) -> Result<()>` — removes key entirely (error if not found)
  - `current_key_bytes(name) -> Result<Vec<u8>>` — decodes current version's base64 key_material (for internal encrypt use)
  - `versioned_key_bytes(name, version) -> Result<Vec<u8>>` — decodes specific version (for internal decrypt use)

- [ ] **T1.9** Write unit tests:
  - Create → load round-trip
  - Generate key → verify type/version
  - Rotate → verify version increments, old version still accessible
  - Delete → verify removed
  - Duplicate name → error
  - Load with wrong key → `DecryptionFailed`
  - `key_info` returns metadata without raw bytes

**Validation:**
```bash
cd crates/rivers-keystore-engine && cargo test
# All tests pass
# Verify: create/load round-trip, generate, rotate, delete, list
# Verify: Zeroize on Drop (key_material field has Zeroize derive)
# Verify: duplicate key names rejected with DuplicateKey error
# Verify: only "aes-256" key_type accepted
```

---

## 2. Keystore Engine — AES-256-GCM Encrypt/Decrypt

Add the core crypto operations. These are standalone functions that take raw key bytes — the host function layer will handle key resolution.

**Modify:** `crates/rivers-keystore-engine/src/lib.rs`

- [x] **T2.1** Implement `encrypt(key_bytes: &[u8], plaintext: &[u8], aad: Option<&[u8]>) -> Result<EncryptResult, AppKeystoreError>`:
  - Validate key_bytes is exactly 32 bytes → `InvalidKeyLength` if not
  - Generate random 96-bit nonce via `aes_gcm::aead::OsRng`
  - Create `Aes256Gcm` cipher from key
  - If `aad` provided: use `Payload { msg: plaintext, aad }`, else use plaintext directly
  - Encrypt → base64-encode ciphertext and nonce
  - Return `EncryptResult { ciphertext, nonce, key_version: 0 }` (caller sets key_version)

- [x] **T2.2** Implement `decrypt(key_bytes: &[u8], ciphertext_b64: &str, nonce_b64: &str, aad: Option<&[u8]>) -> Result<Vec<u8>, AppKeystoreError>`:
  - Validate key_bytes is exactly 32 bytes
  - Base64-decode ciphertext and nonce
  - Validate nonce is exactly 12 bytes → `InvalidNonce` if not
  - Create `Aes256Gcm` cipher from key
  - Decrypt (with optional AAD) → return plaintext bytes
  - On any crypto failure: return `DecryptionFailed` (generic, no oracle)

- [x] **T2.3** Implement convenience wrappers that resolve keys from the keystore:
  - `encrypt_with_key(keystore: &AppKeystore, key_name: &str, plaintext: &[u8], aad: Option<&[u8]>) -> Result<EncryptResult>` — uses current version, sets `key_version` on result
  - `decrypt_with_key(keystore: &AppKeystore, key_name: &str, ciphertext_b64: &str, nonce_b64: &str, key_version: u32, aad: Option<&[u8]>) -> Result<Vec<u8>>` — looks up specified version

- [x] **T2.4** Write unit tests:
  - Encrypt → decrypt round-trip (with and without AAD)
  - Wrong key → `DecryptionFailed`
  - Tampered ciphertext → `DecryptionFailed`
  - AAD mismatch → `DecryptionFailed` (same error as wrong key)
  - Wrong key version → `KeyVersionNotFound`
  - Two encrypts of same plaintext → different ciphertexts (nonce uniqueness)
  - Invalid nonce length → `InvalidNonce`
  - Invalid key length (16 bytes, 64 bytes) → `InvalidKeyLength`

**Validation:**
```bash
cd crates/rivers-keystore-engine && cargo test
# All crypto tests pass
# Verify: encrypt then decrypt returns original plaintext
# Verify: different nonces per call (two encrypts of "hello" differ)
# Verify: wrong key, tampered data, AAD mismatch all return generic DecryptionFailed
# Verify: key_version flows correctly through encrypt/decrypt
```

---

## 3. `rivers-keystore` CLI

Create the CLI binary for managing application keystores. Pattern: `crates/rivers-lockbox/src/main.rs`.

**Create:** `crates/rivers-keystore/Cargo.toml`, `crates/rivers-keystore/src/main.rs`
**Modify:** `Cargo.toml` (workspace root)

- [x] **T3.1** Create `crates/rivers-keystore/Cargo.toml`:
  - `[[bin]] name = "rivers-keystore"`
  - Dependencies: `rivers-keystore-engine` (path), `clap` (derive feature), `age` (workspace)
  - Note: `base64` not needed — engine handles all base64 internally

- [x] **T3.2** Add `rivers-keystore` to workspace `members`

- [x] **T3.3** Implement CLI with clap derive:
  ```
  rivers-keystore <COMMAND>

  Commands:
    init       Initialize a new application keystore
    generate   Generate and store a new encryption key
    list       List keys (names and metadata only)
    info       Show key metadata
    delete     Delete a key
    rotate     Rotate a key (create new version)
  ```
  Master key sourced from `RIVERS_KEYSTORE_KEY` env var (Age identity string).

- [x] **T3.4** Implement `init --path <path>`:
  - Read master key from env
  - Create empty `AppKeystore`, encrypt, write to `path`
  - Print confirmation

- [x] **T3.5** Implement `generate <name> --type aes-256 --path <path>`:
  - Load keystore, generate key, save keystore
  - Print: `generated key "{name}" (aes-256, v1)`

- [x] **T3.6** Implement `list --path <path>`:
  - Load keystore, print table: name | type | version | created
  - Never print raw key material

- [x] **T3.7** Implement `info <name> --path <path>`:
  - Load keystore, print key metadata
  - Output: `name=<name> type=aes-256 version=<N> versions=<count> created=<ts>`

- [x] **T3.8** Implement `delete <name> --path <path>`:
  - Load keystore, delete key, save keystore

- [x] **T3.9** Implement `rotate <name> --path <path>`:
  - Load keystore, rotate key, save keystore
  - Print: `rotated key "{name}" (v{old} → v{new})`

**Validation:**
```bash
cargo build -p rivers-keystore
# Full lifecycle test:
export RIVERS_KEYSTORE_KEY=$(openssl rand -hex 32)
./target/debug/rivers-keystore init --path /tmp/test.appkeystore
./target/debug/rivers-keystore generate credential-key --type aes-256 --path /tmp/test.appkeystore
./target/debug/rivers-keystore list --path /tmp/test.appkeystore
# → credential-key  aes-256  v1  2026-03-27T...
./target/debug/rivers-keystore info credential-key --path /tmp/test.appkeystore
# → name=credential-key type=aes-256 version=1 ...
./target/debug/rivers-keystore rotate credential-key --path /tmp/test.appkeystore
# → rotated key "credential-key" (v1 → v2)
./target/debug/rivers-keystore info credential-key --path /tmp/test.appkeystore
# → version=2 versions=2
./target/debug/rivers-keystore delete credential-key --path /tmp/test.appkeystore
./target/debug/rivers-keystore list --path /tmp/test.appkeystore
# → (empty)
# Error cases:
./target/debug/rivers-keystore generate credential-key --type rsa-2048 --path /tmp/test.appkeystore
# → error: unsupported key type "rsa-2048" (only "aes-256" is supported)
```

---

## 4. Config Types — `[[keystores]]` and `[data.keystore.*]`

Add keystore resource and data config types to `rivers-runtime`. This enables TOML parsing of keystore declarations.

**Modify:** `crates/rivers-runtime/Cargo.toml`, `crates/rivers-runtime/src/bundle.rs`, `crates/rivers-runtime/src/lib.rs`
**Reference:** `ResourceDatasource` in `bundle.rs:100-111`, `AppDataConfig` in `bundle.rs:143-153`

- [ ] **T4.1** Add `rivers-keystore-engine` as optional dependency in `rivers-runtime/Cargo.toml`:
  ```toml
  [dependencies]
  rivers-keystore-engine = { path = "../rivers-keystore-engine", optional = true }

  [features]
  keystore = ["rivers-keystore-engine"]
  full = ["drivers", "storage-backends", "lockbox", "tls", "keystore"]
  ```

- [ ] **T4.2** Add `ResourceKeystore` struct to `bundle.rs` (after `ResourceDatasource`):
  ```rust
  /// A keystore declaration in `resources.toml`.
  #[derive(Debug, Clone, Deserialize, JsonSchema)]
  pub struct ResourceKeystore {
      pub name: String,
      /// Lockbox alias for the master key that encrypts this keystore at rest.
      pub lockbox: String,
      #[serde(default = "default_true")]
      pub required: bool,
  }
  ```

- [ ] **T4.3** Add `keystores` field to `ResourcesConfig`:
  ```rust
  pub struct ResourcesConfig {
      #[serde(default)]
      pub datasources: Vec<ResourceDatasource>,
      #[serde(default)]
      pub keystores: Vec<ResourceKeystore>,   // ← NEW
      #[serde(default)]
      pub services: Vec<ServiceDependency>,
  }
  ```

- [ ] **T4.4** Add `KeystoreDataConfig` struct to `bundle.rs`:
  ```rust
  /// Keystore configuration in `app.toml` under `[data.keystore.<name>]`.
  #[derive(Debug, Clone, Deserialize, JsonSchema)]
  pub struct KeystoreDataConfig {
      /// Path to the keystore file, relative to app directory.
      pub path: String,
  }
  ```

- [ ] **T4.5** Add `keystore` field to `AppDataConfig`:
  ```rust
  pub struct AppDataConfig {
      #[serde(default)]
      pub datasources: HashMap<String, DatasourceConfig>,
      #[serde(default)]
      pub keystore: HashMap<String, KeystoreDataConfig>,  // ← NEW
      #[serde(default)]
      pub dataviews: HashMap<String, DataViewConfig>,
  }
  ```

- [ ] **T4.6** Re-export `ResourceKeystore` and `KeystoreDataConfig` from `rivers-runtime/src/lib.rs`

- [ ] **T4.7** Write a deserialization test:
  ```rust
  #[test]
  fn parse_resources_with_keystores() {
      let toml_str = r#"
  [[datasources]]
  name = "db"
  driver = "sqlite"
  nopassword = true

  [[keystores]]
  name = "app-keys"
  lockbox = "netinventory/keystore-master-key"
  required = true
  "#;
      let config: ResourcesConfig = toml::from_str(toml_str).unwrap();
      assert_eq!(config.keystores.len(), 1);
      assert_eq!(config.keystores[0].name, "app-keys");
      assert_eq!(config.keystores[0].lockbox, "netinventory/keystore-master-key");
  }
  ```

**Validation:**
```bash
cargo test -p rivers-runtime --features keystore
# Deserialization test passes
# Verify: resources.toml with [[keystores]] parses correctly
# Verify: app.toml with [data.keystore.app-keys] path = "data/app.keystore" parses correctly
# Verify: existing bundles without keystores still parse (serde default)
cargo build -p rivers-runtime --features keystore
# No compile errors
```

---

## 5. Bundle Validation — Keystore Checks

Add keystore-specific validation rules to catch config errors before startup.

**Modify:** `crates/rivers-runtime/src/validate.rs`, `crates/riversctl/src/main.rs`
**Reference:** Existing validation patterns in `validate.rs:57-95`

- [ ] **T5.1** Add `validate_keystores()` function to `validate.rs`:
  - Every `[data.keystore.<name>]` must match a `[[keystores]]` entry by name
  - Every `[[keystores]]` `lockbox` alias must be non-empty
  - No duplicate keystore names in `[[keystores]]`

- [ ] **T5.2** Add file-existence check (used by `riversctl validate` with bundle path):
  - For each keystore, resolve `path` relative to app dir → check file exists
  - Return warning (not error) if file missing — file may be created later by CLI

- [ ] **T5.3** Wire `validate_keystores()` into `validate_bundle()` (called by both `riversctl validate` and `load_and_wire_bundle`)

- [ ] **T5.4** Add keystore checks to `riversctl/src/main.rs:cmd_validate()`:
  - Check keystore file existence at configured paths
  - Log warnings for missing files

- [ ] **T5.5** Write unit tests:
  - Valid config → passes
  - Name mismatch (keystore in app.toml not declared in resources.toml) → error
  - Empty lockbox alias → error
  - Duplicate keystore names → error

**Validation:**
```bash
cargo test -p rivers-runtime --features keystore -- keystore
# All validation tests pass
# Test with a real bundle:
cargo run -p riversctl -- validate test-bundle-with-keystore/
# No errors for valid config
# Appropriate warnings/errors for invalid config
```

---

## 6. Keystore Resolver — Startup Integration

Wire keystore resolution into the bundle loading sequence. This follows the exact pattern of LockBox credential resolution in `bundle_loader.rs:92-119`.

**Modify:** `crates/riversd/src/bundle_loader.rs`, `crates/riversd/src/server.rs`
**Reference:** LockBox resolution in `bundle_loader.rs:92-119`

### Design

```
Startup flow (new steps marked with ←):
1. Load bundle
2. Validate bundle
3. Collect LockBox refs from datasources
4. Resolve LockBox credentials
5. Collect keystore refs from resources.toml        ← NEW
6. For each keystore:                                ← NEW
   a. Resolve master key from LockBox via alias
   b. Load keystore file from app.toml path
   c. Decrypt keystore with master key
   d. Store unlocked keystore in resolver
7. Register DataViews, build ConnectionParams
8. Build DriverFactory, DataViewExecutor, etc.
```

- [ ] **T6.1** Define `KeystoreResolver` — a thin wrapper holding unlocked keystores per app:
  ```rust
  /// Holds unlocked application keystores, scoped by app.
  /// Key: "{entry_point}:{keystore_name}", Value: unlocked AppKeystore
  pub struct KeystoreResolver {
      keystores: HashMap<String, Arc<rivers_keystore_engine::AppKeystore>>,
  }
  impl KeystoreResolver {
      pub fn new() -> Self { ... }
      pub fn insert(&mut self, scoped_name: String, ks: AppKeystore) { ... }
      pub fn get(&self, scoped_name: &str) -> Option<&Arc<AppKeystore>> { ... }
  }
  ```
  Place in `riversd/src/bundle_loader.rs` or a new `riversd/src/keystore.rs` module.

- [ ] **T6.2** Add `keystore_resolver: Option<Arc<KeystoreResolver>>` to `AppContext` in `server.rs:119-158`. Initialize as `None` in `AppContext::new()`.

- [ ] **T6.3** In `load_and_wire_bundle()`, after LockBox resolution block (line ~119) and before the per-app DataView/datasource loop (line ~121), add keystore resolution:
  ```rust
  // ── Keystore: resolve master keys and unlock keystores ──
  let mut ks_resolver = KeystoreResolver::new();
  for app in &bundle.apps {
      let entry_point = app.manifest.entry_point.as_deref()
          .unwrap_or(&app.manifest.app_name);
      for ks_decl in &app.resources.keystores {
          // 1. Resolve master key from LockBox
          let master_key = /* fetch from lockbox using ks_decl.lockbox alias */;
          // 2. Resolve keystore file path from app.toml
          let ks_config = app.config.data.keystore.get(&ks_decl.name)
              .ok_or_else(|| /* error: keystore declared but not configured */)?;
          let ks_path = app.app_dir.join(&ks_config.path);
          // 3. Load and decrypt
          let keystore = AppKeystore::load(&ks_path, &master_key)
              .map_err(|e| /* wrap error */)?;
          // 4. Store with scoped name
          let scoped = format!("{}:{}", entry_point, ks_decl.name);
          ks_resolver.insert(scoped, keystore);
          tracing::info!(keystore = %ks_decl.name, app = %entry_point, "keystore: unlocked");
      }
  }
  if !ks_resolver.is_empty() {
      ctx.keystore_resolver = Some(Arc::new(ks_resolver));
  }
  ```

- [ ] **T6.4** Handle error cases with clear messages:
  - Missing lockbox alias → `KeystoreLocked: lockbox alias 'X' not found`
  - Missing keystore file → `KeystoreNotFound: keystore file 'X' does not exist`
  - Decryption failure → `KeystoreLocked: cannot unlock keystore 'X' — check master key`
  - If `required = true` on the keystore declaration, errors are fatal (startup fails)

**Validation:**
```bash
cargo build -p riversd --features full
# Test with a bundle that has a keystore:
# 1. Create test keystore via CLI
# 2. Provision master key in test Lockbox
# 3. Start riversd → log shows "keystore: unlocked"
# Error test: remove Lockbox alias → startup fails with clear error message
# Error test: corrupt keystore file → startup fails with DecryptionFailed
# Error test: wrong master key → startup fails with DecryptionFailed
```

---

## 7. TaskContext & Engine SDK — Keystore Fields

Add keystore context to the task dispatch path so handlers can access keystore operations.

**Modify:** `crates/rivers-runtime/src/process_pool/types.rs`, `crates/rivers-engine-sdk/src/lib.rs`
**Reference:** LockBox fields in `types.rs:86-94`, `SerializedTaskContext` in `engine-sdk/src/lib.rs:25-60`

- [ ] **T7.1** Add keystore fields to `TaskContext` in `process_pool/types.rs` (gated behind `#[cfg(feature = "keystore")]`):
  ```rust
  /// Unlocked application keystore for this task's app (App Keystore feature).
  #[cfg(feature = "keystore")]
  pub keystore: Option<Arc<rivers_keystore_engine::AppKeystore>>,
  ```

- [ ] **T7.2** Update `TaskContext` Debug impl to include keystore field (redacted, like other Arc fields)

- [ ] **T7.3** Add `keystore_available: bool` to `SerializedTaskContext` in `rivers-engine-sdk/src/lib.rs:25`

- [ ] **T7.4** Update the `SerializedTaskContext` round-trip test in engine-sdk to include `keystore_available: false`

- [ ] **T7.5** In `riversd`, where `TaskContext` is constructed for dispatch (in the request handler / ProcessPool), populate `keystore` from `AppContext.keystore_resolver` using the app's entry_point + keystore name

**Validation:**
```bash
cargo test -p rivers-engine-sdk
# SerializedTaskContext round-trip test passes with keystore_available field
cargo build -p rivers-runtime --features keystore
cargo build -p riversd --features full
# No compile errors
```

---

## 8. V8 Host Functions — `Rivers.keystore.has/info`

Add `Rivers.keystore` namespace to V8 with read-only metadata access.

**Modify:** `crates/riversd/src/process_pool/v8_engine.rs`
**Reference:** `LockBoxContext` pattern at `v8_engine.rs:19-24`, `TASK_LOCKBOX` at line 77-80, `TaskLocals::set` at lines 118-127

- [ ] **T8.1** Add `KeystoreContext` struct (after `LockBoxContext`, ~line 24):
  ```rust
  /// Application keystore context for V8 host functions.
  struct KeystoreContext {
      keystore: Arc<rivers_keystore_engine::AppKeystore>,
  }
  ```

- [ ] **T8.2** Add `TASK_KEYSTORE` thread-local (after `TASK_LOCKBOX`, ~line 80):
  ```rust
  /// Application keystore for encrypt/decrypt and key metadata (App Keystore feature).
  static TASK_KEYSTORE: RefCell<Option<KeystoreContext>> = RefCell::new(None);
  ```

- [ ] **T8.3** Populate `TASK_KEYSTORE` in `TaskLocals::set()` (~line 128):
  ```rust
  TASK_KEYSTORE.with(|ks| {
      *ks.borrow_mut() = ctx.keystore.as_ref().map(|k| KeystoreContext {
          keystore: k.clone(),
      });
  });
  ```

- [ ] **T8.4** Clear `TASK_KEYSTORE` in `TaskLocals::drop()` (~line 144):
  ```rust
  TASK_KEYSTORE.with(|ks| *ks.borrow_mut() = None);
  ```

- [ ] **T8.5** Add `Rivers.keystore.has(name)` V8 callback in `inject_rivers_global()` — insert after the `Rivers.crypto` block (~line 1426) and before `Rivers.http` (~line 1428):
  - Read `TASK_KEYSTORE` thread-local
  - Call `keystore.has_key(name)`
  - Return V8 boolean

- [ ] **T8.6** Add `Rivers.keystore.info(name)` V8 callback:
  - Read `TASK_KEYSTORE` thread-local
  - Call `keystore.key_info(name)`
  - Return V8 object: `{ name, type, version, created_at }` — never raw key bytes
  - Throw if key not found

- [ ] **T8.7** Wire the keystore object into `rivers_obj`, conditionally (only when `TASK_KEYSTORE` is `Some`):
  ```rust
  let ks_available = TASK_KEYSTORE.with(|ks| ks.borrow().is_some());
  if ks_available {
      let keystore_obj = v8::Object::new(scope);
      // ... register has_fn and info_fn ...
      let ks_key = v8_str(scope, "keystore")?;
      rivers_obj.set(scope, ks_key.into(), keystore_obj.into());
  }
  ```

**Validation:**
```bash
cargo build -p riversd --features full
# Integration test with JS handler:
#   Rivers.keystore.has("credential-key")   → true
#   Rivers.keystore.has("nonexistent")      → false
#   Rivers.keystore.info("credential-key")  → { name: "credential-key", type: "aes-256", version: 1, created_at: "..." }
#   Rivers.keystore.info("nonexistent")     → throws "KeyNotFound: key 'nonexistent' does not exist"
# Verify: returned info object has NO raw key bytes
# Verify: when no keystore configured, Rivers.keystore is undefined
```

---

## 9. V8 Host Functions — `Rivers.crypto.encrypt/decrypt`

Add AES-256-GCM encryption to `Rivers.crypto`. Key bytes stay in Rust — only results cross to V8.

**Modify:** `crates/riversd/src/process_pool/v8_engine.rs`, `crates/riversd/Cargo.toml`
**Reference:** `Rivers.crypto.hmac` pattern at `v8_engine.rs:1356-1423`

- [ ] **T9.1** Add `aes-gcm` dependency to `crates/riversd/Cargo.toml`
  (The actual AES-GCM work is in `rivers-keystore-engine`; riversd just needs it for the type references, or may only need `rivers-keystore-engine` as a dep.)

- [ ] **T9.2** Add `Rivers.crypto.encrypt(keyName, plaintext, options?)` V8 callback:
  - Extract args: `keyName` (string), `plaintext` (string), `options` (optional object with `aad` field)
  - Read `TASK_KEYSTORE` → error if `None` ("KeystoreNotConfigured")
  - Call `rivers_keystore_engine::encrypt_with_key(&keystore, key_name, plaintext.as_bytes(), aad)`
  - Convert `EncryptResult` to V8 object: `{ ciphertext: string, nonce: string, key_version: integer }`
  - On error: throw V8 exception with error message
  - **Security:** key bytes resolved internally — never returned to V8

  Insert into `crypto_obj` registration block, after `hmac` (~line 1423).

- [ ] **T9.3** Add `Rivers.crypto.decrypt(keyName, ciphertext, nonce, options?)` V8 callback:
  - Extract args: `keyName` (string), `ciphertext` (string), `nonce` (string), `options` (object with `key_version` integer, optional `aad` string)
  - `key_version` is required — throw if missing
  - Read `TASK_KEYSTORE` → error if `None`
  - Call `rivers_keystore_engine::decrypt_with_key(&keystore, key_name, ciphertext, nonce, key_version, aad)`
  - Return plaintext string to V8
  - On any failure: throw generic "DecryptionFailed: authentication tag mismatch" (no oracle)
  - **Security:** never log plaintext in tracing spans

- [ ] **T9.4** Wire into `inject_rivers_global()` inside the existing `crypto_obj` block. These functions are always registered on the crypto object but will throw `KeystoreNotConfigured` if no keystore is available.

**Security invariants (verify in code review):**
- [ ] Nonce generated by `OsRng`, never caller-supplied
- [ ] Key bytes resolved in Rust, never serialized to V8 (grep for key_material in v8_engine.rs — should not appear)
- [ ] Plaintext never appears in `tracing::info/warn/error` calls
- [ ] Auth tag failure → generic error string (same for wrong key, wrong AAD, tampered data)

**Validation:**
```bash
cargo build -p riversd --features full
# JS handler integration test:
var enc = Rivers.crypto.encrypt("credential-key", "my-secret-password");
assert(typeof enc.ciphertext === "string");  // base64
assert(typeof enc.nonce === "string");       // base64
assert(typeof enc.key_version === "number"); // integer

var dec = Rivers.crypto.decrypt("credential-key", enc.ciphertext, enc.nonce, {
    key_version: enc.key_version
});
assert(dec === "my-secret-password");

# Error cases:
Rivers.crypto.encrypt("nonexistent-key", "data");
# → throws "KeyNotFound: key 'nonexistent-key' does not exist"

Rivers.crypto.decrypt("credential-key", "invalid", "invalid", { key_version: 1 });
# → throws "DecryptionFailed: authentication tag mismatch"

Rivers.crypto.decrypt("credential-key", enc.ciphertext, enc.nonce, { key_version: 999 });
# → throws "KeyVersionNotFound: key 'credential-key' version 999 does not exist"

# AAD test:
var enc2 = Rivers.crypto.encrypt("credential-key", "data", { aad: "device-1" });
Rivers.crypto.decrypt("credential-key", enc2.ciphertext, enc2.nonce, {
    key_version: enc2.key_version, aad: "device-1"
}); // succeeds

Rivers.crypto.decrypt("credential-key", enc2.ciphertext, enc2.nonce, {
    key_version: enc2.key_version, aad: "device-2"
}); // throws DecryptionFailed

# No keystore test:
Rivers.crypto.encrypt("any-key", "data");
# → throws "KeystoreNotConfigured: no keystore resource declared"
```

---

## 10. WASM Host Function Bindings

Add keystore and encrypt/decrypt host functions for WASM handlers. Follows the WASM host function pattern in `wasm_engine.rs`.

**Modify:** `crates/riversd/src/process_pool/wasm_engine.rs`
**Reference:** Existing WASM linker bindings in `wasm_engine.rs:135-159`

- [ ] **T10.1** Add `rivers.keystore_has(name_ptr, name_len) -> i32` WASM import:
  - Read name from WASM memory
  - Resolve from `TASK_KEYSTORE`
  - Return 1 (true) or 0 (false)

- [ ] **T10.2** Add `rivers.keystore_info(name_ptr, name_len, out_ptr, out_len) -> i32` WASM import:
  - Read name, resolve metadata, write JSON to WASM output buffer
  - Return 0 on success, -1 on error

- [ ] **T10.3** Add `rivers.crypto_encrypt(input_ptr, input_len, out_ptr, out_len) -> i32` WASM import:
  - Input: JSON `{ key_name, plaintext, aad? }`
  - Output: JSON `{ ciphertext, nonce, key_version }`
  - Return 0 on success, -1 on error

- [ ] **T10.4** Add `rivers.crypto_decrypt(input_ptr, input_len, out_ptr, out_len) -> i32` WASM import:
  - Input: JSON `{ key_name, ciphertext, nonce, key_version, aad? }`
  - Output: plaintext string
  - Return 0 on success, -1 on error

**Validation:**
```bash
cargo build -p riversd --features full
# Verify: WASM linker bindings compile without errors
# Verify: same encrypt/decrypt round-trip behavior as V8
```

---

## 11. Engine SDK — HostCallbacks Update (Dynamic Engines)

Update `HostCallbacks` struct for the dynamic engine loading path.

**Modify:** `crates/rivers-engine-sdk/src/lib.rs`, `crates/riversd/src/engine_loader.rs`
**Reference:** Existing callback slots in `engine-sdk/src/lib.rs:122-179`, host implementations in `engine_loader.rs:298-667`

- [ ] **T11.1** Add callback slots to `HostCallbacks`:
  ```rust
  pub keystore_has: Option<extern "C" fn(input_ptr: *const u8, input_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32>,
  pub keystore_info: Option<extern "C" fn(input_ptr: *const u8, input_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32>,
  pub crypto_encrypt: Option<extern "C" fn(input_ptr: *const u8, input_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32>,
  pub crypto_decrypt: Option<extern "C" fn(input_ptr: *const u8, input_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32>,
  ```

- [ ] **T11.2** Implement `host_keystore_has()`, `host_keystore_info()`, `host_crypto_encrypt()`, `host_crypto_decrypt()` as `extern "C"` functions in `engine_loader.rs`, following the `host_store_get` pattern (JSON input → process → JSON output)

- [ ] **T11.3** Wire new callbacks in `build_host_callbacks()` function in `engine_loader.rs`

**Validation:**
```bash
cargo build -p rivers-engine-sdk
cargo build -p riversd --features full
# Dynamic engine loading path compiles
# If dynamic V8/WASM engines are available, test that keystore callbacks work through them
```

---

## 12. Integration Test

End-to-end test verifying the full keystore + encrypt/decrypt flow.

**Create:** Test in `crates/riversd/tests/` or as a test in an existing integration test file

- [ ] **T12.1** Create test fixtures:
  - Test Lockbox with a master key entry
  - Test app keystore (created via `rivers-keystore-engine` API) with a "test-key" (aes-256)
  - Test bundle with `[[keystores]]` and `[data.keystore.*]` configured

- [ ] **T12.2** Write integration test:
  1. Load test bundle via `load_and_wire_bundle()`
  2. Verify keystore resolver has the unlocked keystore
  3. Build a `TaskContext` with keystore populated
  4. Execute a JS handler that:
     - Calls `Rivers.keystore.has("test-key")` → true
     - Calls `Rivers.keystore.info("test-key")` → metadata
     - Encrypts: `Rivers.crypto.encrypt("test-key", "secret-data")`
     - Decrypts with returned values → gets "secret-data" back
     - Returns success

- [ ] **T12.3** Test key rotation flow:
  1. Encrypt with v1 key
  2. Rotate key (programmatically via keystore engine API)
  3. Encrypt with v2 key (automatic — encrypt uses current version)
  4. Decrypt v1 data with `key_version: 1` → succeeds
  5. Decrypt v2 data with `key_version: 2` → succeeds

- [ ] **T12.4** Test error cases:
  - Encrypt with nonexistent key → KeyNotFound
  - Decrypt with wrong version → KeyVersionNotFound
  - Decrypt with tampered ciphertext → DecryptionFailed
  - No keystore configured → KeystoreNotConfigured

- [ ] **T12.5** Test app isolation:
  - Two apps in same bundle with different keystores
  - App A's handler cannot access App B's keys

**Validation:**
```bash
cargo test -p riversd --features full -- keystore
# All integration tests pass
# Verify: encrypt → decrypt round-trip
# Verify: key rotation with version tracking works
# Verify: error cases produce correct error types
# Verify: cross-app isolation enforced
```

---

## Acceptance Criteria Checklist

Per spec `docs/rivers-feature-request-app-keystore.md` §13:

- [ ] AC1: `[[keystores]]` in `resources.toml` declares a keystore resource with a Lockbox alias for the master key
- [ ] AC2: `[data.keystore.*]` in `app.toml` configures the keystore file path
- [ ] AC3: Keystore master key is resolved from Lockbox at app startup — same resolution path as datasource credentials
- [ ] AC4: `rivers-keystore` CLI can init, generate, list, info, delete, and rotate keys — independent of `rivers-lockbox`
- [ ] AC5: `Rivers.keystore.has(name)` and `Rivers.keystore.info(name)` work from handler code
- [ ] AC6: `Rivers.crypto.encrypt(keyName, plaintext)` returns `{ ciphertext, nonce, key_version }` using AES-256-GCM
- [ ] AC7: `Rivers.crypto.decrypt(keyName, ciphertext, nonce, { key_version })` returns the original plaintext
- [ ] AC8: Decryption with wrong key, wrong version, or tampered ciphertext throws a generic error
- [ ] AC9: Key bytes never appear in handler memory, logs, or error messages
- [ ] AC10: Keys are scoped to the declaring app — other apps cannot access them
- [ ] AC11: Keystore file is encrypted at rest using the Lockbox-provided master key
- [ ] AC12: `riversctl validate` checks keystore file existence, Lockbox alias resolution, and key references
- [ ] AC13: Works identically in JavaScript and WASM handlers
- [ ] AC14: Key rotation creates a new version without destroying previous versions
