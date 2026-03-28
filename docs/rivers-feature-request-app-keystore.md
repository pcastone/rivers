# Rivers Feature Request — Application Keystore & Encryption API

**Requested By:** Network Inventory Application Team
**Date:** 2026-03-27
**Rivers Version:** v0.50.1
**Priority:** High — blocks credential management in handler code

---

## 1. Summary

Rivers needs an **application-scoped keystore** and encryption API accessible from CodeComponent handlers.

The keystore follows the same resource pattern as every other Rivers dependency: it is **declared in `resources.toml`**, its master key is **stored in Lockbox** (because Lockbox's job is unlocking resources), and its configuration lives in **`app.toml`**. The keystore itself is a new subsystem — it stores application-owned encryption keys, scoped to an `appId`, and exposes encrypt/decrypt operations to handler code.

**Lockbox's role is narrow and correct here:** it holds the master key that unlocks the application keystore, the same way it holds the password that unlocks a PostgreSQL connection. It does not store the application's encryption keys, manage key rotation, or participate in encrypt/decrypt operations. Those are the keystore's job.

**Request:** A new `keystore` resource type, a `Rivers.keystore` handler API, and AES-256-GCM `encrypt`/`decrypt` functions in `Rivers.crypto` that reference application keystore keys.

---

## 2. Why a Separate Keystore (Not Keys in Lockbox)

Lockbox provides the master key. The application's encryption keys live in the keystore. This is the same separation that exists between "Lockbox holds a database password" and "the database holds application data."

| Concern | Lockbox | Application Keystore (Proposed) |
|---------|---------|--------------------------------|
| **Role** | Unlocks resources (DB passwords, TLS certs, master keys) | Stores and operates on application encryption keys |
| **Scope** | System-wide, shared across all apps | Per-application, scoped to `appId` |
| **Managed by** | Operations / sysadmin | Application developer |
| **Contains** | Credentials that unlock things | Typed encryption keys (AES-256, future: RSA, EC) |
| **API** | Resolve at startup, no runtime access from handlers | `Rivers.keystore.*` and `Rivers.crypto.encrypt/decrypt` from handlers |
| **Rotation** | Replace (destructive, requires restart) | Version (non-destructive, old versions kept for decryption) |
| **Blast radius** | Compromise exposes infrastructure credentials | Compromise exposes only one app's data keys |

Why not just put the AES keys directly in Lockbox?

1. **Wrong abstraction** — Lockbox is a secret store (store a value, get it back). An application keystore is a crypto service (encrypt this, decrypt that, rotate keys, track versions). Lockbox has no concept of key types, versioning, or rotation.
2. **No handler access** — Lockbox secrets are resolved at startup into datasource configs. There is no `Rivers.lockbox.get()` in handler code, and there shouldn't be — exposing raw Lockbox values to JS handlers leaks key material into the V8 heap.
3. **No isolation** — all Lockbox entries share one namespace. An app developer adding a key could collide with or access another app's secrets.
4. **Scope bleed** — app developers need ops access to Lockbox to manage their own keys. That's privilege escalation for an app-level concern.

---

## 3. Gap Analysis

### What Exists Today

| Component | What It Does | What It Doesn't Do |
|-----------|-------------|-------------------|
| **Lockbox** | Stores system secrets, resolves at startup for datasources | Expose secrets to handlers, encrypt/decrypt operations, key typing, key versioning |
| **`Rivers.crypto`** | Hash passwords, HMAC, random bytes, constant-time compare | Symmetric encryption, key management, reversible operations |
| **`ctx.store`** | Application KV store with TTL | Encryption, key protection, persistence across restarts |

### The Gap

There is no way to:

1. Declare an encryption key as part of an application bundle.
2. Encrypt a value in a handler and store the ciphertext in a database.
3. Retrieve that ciphertext later and decrypt it back to plaintext.
4. Do any of the above with keys that are application-scoped, versioned, and managed without direct ops involvement beyond the initial master key provisioning.

---

## 4. Use Case

### Network Inventory Application

The application stores credentials for network devices (switches, routers). Automation systems (Ansible, scripts) need to retrieve the actual password to connect to devices via SSH, SNMP, or HTTPS.

```
Create Credential:
  1. User submits plaintext password via API
  2. Handler calls Rivers.crypto.encrypt("credential-key", password)
  3. Handler stores ciphertext + nonce + key_version in SQLite
  4. Plaintext never persists to disk

Retrieve Credential:
  1. Authenticated user or automation system requests credential
  2. Handler reads ciphertext + nonce + key_version from SQLite
  3. Handler calls Rivers.crypto.decrypt("credential-key", ciphertext, nonce, { key_version })
  4. Plaintext returned in response (over TLS)
  5. Plaintext never logged
```

### Other Applications That Would Benefit

- **Secret management dashboards** — storing and retrieving shared secrets for teams.
- **Integration platforms** — storing OAuth tokens and API keys for third-party services.
- **Healthcare/compliance apps** — field-level encryption of PII to meet regulatory requirements.
- **Multi-tenant SaaS** — per-tenant encryption keys for data isolation.

---

## 5. Proposed Design

### 5.1 Resource Declaration

The application keystore is declared as a resource in `resources.toml`, just like a datasource. The master key that unlocks it lives in Lockbox.

**File:** `{app}/resources.toml`

```toml
[[datasources]]
name     = "inventory_db"
driver   = "sqlite"
x-type   = "sqlite"
nopassword = true
required = true

[[keystores]]
name     = "app-keys"
lockbox  = "netinventory/keystore-master-key"
required = true
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Logical name — referenced in `app.toml` and handler code |
| `lockbox` | yes | Lockbox alias for the master key that encrypts the keystore at rest |
| `required` | yes | If `true`, app startup fails if the keystore cannot be unlocked |

This follows the exact same pattern as a datasource with a `lockbox` alias. Ops provisions the master key in Lockbox once; the application developer manages everything else.

### 5.2 Keystore Configuration

**File:** `{app}/app.toml`

```toml
[data.keystore.app-keys]
path = "data/app.keystore"
```

| Field | Required | Description |
|-------|----------|-------------|
| `path` | yes | Path to the keystore file, relative to app directory |

Minimal config — the keystore is a single encrypted file. The master key comes from the resource declaration (Lockbox). Everything else is managed via the `rivers-keystore` CLI.

### 5.3 CLI: `rivers-keystore`

A new CLI tool, separate from `rivers-lockbox`, for managing application keystores.

```bash
# Initialize a new application keystore
rivers-keystore init --path data/app.keystore

# Generate and store an AES-256 key
rivers-keystore generate credential-key --type aes-256 --path data/app.keystore

# List keys (names and metadata only, never raw values)
rivers-keystore list --path data/app.keystore

# Show key metadata
rivers-keystore info credential-key --path data/app.keystore
# Output: name=credential-key type=aes-256 version=1 created=2026-03-27T...

# Delete a key
rivers-keystore delete credential-key --path data/app.keystore

# Rotate a key (creates new version, keeps old for decryption)
rivers-keystore rotate credential-key --path data/app.keystore
```

Key differences from `rivers-lockbox`:

| Aspect | `rivers-lockbox` | `rivers-keystore` |
|--------|-----------------|-------------------|
| Operates on | System keystore (`lockbox/keystore.rkeystore`) | App keystore (`data/app.keystore`) |
| Stores | Arbitrary secrets (passwords, tokens, strings) | Typed encryption keys (AES-256, future: RSA, EC) |
| Key generation | Manual (`--value` flag) | Built-in (`generate` with `--type`) |
| Key rotation | Replace (destructive) | Version (non-destructive, old versions kept) |
| Bundled with app | No — external to bundle | Yes — ships with the app |
| Unlocked by | `RIVERS_LOCKBOX_KEY` env var | Master key from Lockbox |

### 5.4 Handler API: `Rivers.keystore`

A new namespace for keystore operations from handler code.

```javascript
// Check if a key exists
var exists = Rivers.keystore.has("credential-key");

// Get key metadata (never raw bytes)
var meta = Rivers.keystore.info("credential-key");
// Returns: { name: "credential-key", type: "aes-256", version: 2, created_at: "..." }
```

When an app declares multiple keystores (rare, but possible), the key name is resolved against the app's declared keystores. If ambiguous, prefix with the keystore name: `"app-keys/credential-key"`.

### 5.5 Encryption API: `Rivers.crypto.encrypt` / `Rivers.crypto.decrypt`

Encrypt and decrypt functions added to `Rivers.crypto`, referencing application keystore keys.

```javascript
// Encrypt plaintext using an application keystore key
// Returns: { ciphertext: string, nonce: string, key_version: integer }
var result = Rivers.crypto.encrypt("credential-key", plaintext);

// Decrypt ciphertext using an application keystore key
// key_version tells the runtime which version of the key to use
var plaintext = Rivers.crypto.decrypt("credential-key", ciphertext, nonce, {
    key_version: result.key_version
});

// Encrypt with associated data (AEAD)
// aad is authenticated but not encrypted — binds ciphertext to a record ID
var result = Rivers.crypto.encrypt("credential-key", plaintext, {
    aad: deviceId
});

// Decrypt with associated data
var plaintext = Rivers.crypto.decrypt("credential-key", ciphertext, nonce, {
    key_version: 1,
    aad: deviceId
});
```

### 5.6 Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `keyName` | string | Key name in the application keystore (e.g., `"credential-key"`) |
| `plaintext` | string | Data to encrypt |
| `ciphertext` | string | Base64-encoded ciphertext to decrypt |
| `nonce` | string | Base64-encoded nonce/IV returned from encrypt |
| `options.key_version` | integer | Key version for decryption (returned by encrypt) |
| `options.aad` | string | Optional additional authenticated data (AEAD) |

### 5.7 Return Values

**`encrypt` returns:**

```json
{
  "ciphertext": "base64-encoded-ciphertext",
  "nonce": "base64-encoded-96-bit-nonce",
  "key_version": 1
}
```

The `key_version` MUST be stored alongside ciphertext in the database. This enables key rotation without re-encrypting existing data.

**`decrypt` returns:** plaintext string, or throws on authentication failure.

### 5.8 Error Cases

| Condition | Error |
|-----------|-------|
| No keystore declared for this app | `KeystoreNotConfigured: no keystore resource declared` |
| Keystore file not found | `KeystoreNotFound: keystore file 'X' does not exist` |
| Master key not in Lockbox | `KeystoreLocked: lockbox alias 'X' not found — cannot unlock keystore` |
| Key name not found | `KeyNotFound: key 'X' does not exist in application keystore` |
| Key version not found | `KeyVersionNotFound: key 'X' version N does not exist` |
| Key is wrong type for operation | `InvalidKeyType: encrypt requires an AES key, got 'X'` |
| Key is wrong length | `InvalidKeyLength: expected 32 bytes, got N` |
| Decryption auth tag mismatch | `DecryptionFailed: authentication tag mismatch` |
| Nonce missing or malformed | `InvalidNonce: expected 12-byte base64-encoded nonce` |
| AAD mismatch on decrypt | `DecryptionFailed: authentication tag mismatch` (same error — no oracle) |

### 5.9 Algorithm Specification

| Property | Value |
|----------|-------|
| Algorithm | AES-256-GCM |
| Key size | 256 bits (32 bytes) |
| Nonce size | 96 bits (12 bytes), randomly generated per encrypt call |
| Auth tag size | 128 bits (appended to ciphertext) |
| Encoding | Base64 for ciphertext and nonce |
| Key versioning | Integer version, monotonically increasing |

---

## 6. Key Rotation

Key rotation is first-class, not a future add-on.

### How It Works

```
1. Developer runs: rivers-keystore rotate credential-key
2. Keystore creates version N+1 of the key
3. Version N is retained (read-only) for decrypting existing data
4. New encrypt() calls automatically use version N+1
5. decrypt() uses the key_version stored alongside the ciphertext
6. Application can re-encrypt old data at its own pace (lazy rotation)
```

### Storage Pattern

Applications store `key_version` alongside ciphertext:

```sql
CREATE TABLE credentials (
    id             TEXT PRIMARY KEY,
    encrypted_pass TEXT NOT NULL,       -- base64 ciphertext
    pass_nonce     TEXT NOT NULL,       -- base64 nonce
    pass_key_ver   INTEGER NOT NULL,    -- key version used for encryption
    ...
);
```

### Lazy Re-encryption

Applications can re-encrypt old data on read:

```javascript
function getCredential(ctx) {
    var cred = ctx.dataview("get_credential", { id: ctx.request.params.id });

    var plaintext = Rivers.crypto.decrypt(
        "credential-key", cred.encrypted_pass, cred.pass_nonce,
        { key_version: cred.pass_key_ver }
    );

    // Re-encrypt if using an old key version
    var currentVersion = Rivers.keystore.info("credential-key").version;
    if (cred.pass_key_ver < currentVersion) {
        var reencrypted = Rivers.crypto.encrypt("credential-key", plaintext);
        ctx.dataview("update_credential_key_ver", {
            id: cred.id,
            encrypted_pass: reencrypted.ciphertext,
            pass_nonce: reencrypted.nonce,
            pass_key_ver: reencrypted.key_version
        });
    }

    ctx.resdata = { /* ... */ password: plaintext };
}
```

---

## 7. How the Pieces Fit Together

```
┌─────────────────────────────────────────────────────────────────┐
│                        Rivers Runtime                           │
│                                                                 │
│  ┌──────────────────────┐       ┌────────────────────────────┐  │
│  │       Lockbox        │       │   Application Keystore     │  │
│  │  (System Scope)      │       │   (App Scope)              │  │
│  ├──────────────────────┤       ├────────────────────────────┤  │
│  │ Contains:            │       │ Contains:                  │  │
│  │  • DB passwords      │       │  • AES-256 encryption keys │  │
│  │  • TLS certs         │       │  • Key version history     │  │
│  │  • API tokens        │       │  • Key metadata            │  │
│  │  • Keystore master   │──────▶│                            │  │
│  │    keys              │unlocks│ Scoped to: single appId    │  │
│  │                      │       │ Accessed by: handler code  │  │
│  │ Scoped to: system    │       │ CLI: rivers-keystore       │  │
│  │ Accessed by: startup │       │ File: *.appkeystore        │  │
│  │ CLI: rivers-lockbox  │       │                            │  │
│  │ File: *.rkeystore    │       │                            │  │
│  └──────────────────────┘       └────────────────────────────┘  │
│                                          │                      │
│                                          ▼                      │
│                               ┌──────────────────┐             │
│                               │  Rivers.crypto   │             │
│                               │  .encrypt()      │             │
│                               │  .decrypt()      │             │
│                               │                  │             │
│                               │  Key bytes stay  │             │
│                               │  in Rust memory  │             │
│                               └──────────────────┘             │
│                                                                 │
│  Resource resolution flow (same as datasources):               │
│  resources.toml → Lockbox resolves master key → app.toml       │
│  configures path → keystore unlocked at app startup            │
└─────────────────────────────────────────────────────────────────┘
```

Lockbox's role: **unlock the keystore** (same as unlocking a database).
Keystore's role: **manage and operate on application encryption keys**.
`Rivers.crypto`'s role: **perform encrypt/decrypt using keystore keys, never exposing raw key bytes**.

---

## 8. Bundle Structure

```
netinventory-bundle/
├── manifest.toml
├── CHANGELOG.md
├── netinventory-service/
│   ├── manifest.toml
│   ├── resources.toml              ← [[keystores]] declared here
│   ├── app.toml                    ← [data.keystore.app-keys] configured here
│   ├── data/
│   │   └── app.keystore            ← keystore file ships with bundle
│   ├── schemas/
│   └── libraries/
│       └── handlers/
└── netinventory-main/
    ├── manifest.toml
    ├── resources.toml
    ├── app.toml
    └── libraries/
        └── spa/
```

### Startup Sequence (Keystore Addition)

The existing Rivers startup sequence gains one step:

```
1.  Load and validate config
2.  Initialize EventBus
3.  Load plugins
4.  Configure Lockbox resolver
5.  Configure session manager
6.  Configure pool manager
6a. Resolve keystore master keys from Lockbox         ← NEW
6b. Unlock application keystores                      ← NEW
7.  Configure DataView engine
8.  Configure runtime factory (ProcessPool)
9.  Build main router
10. Maybe spawn admin server
11. Bind HTTP server
12. Wait for shutdown signal
```

### Validation

`riversctl validate` gains new checks:

| Check | Error |
|-------|-------|
| `[[keystores]]` declared but keystore file not found at configured `path` | `keystore file not found: 'data/app.keystore'` |
| `[[keystores]]` lockbox alias not provisioned | `lockbox alias 'X' not found for keystore 'Y'` |
| Handler references key name not in any declared keystore | warning: `key 'X' not found in application keystore` |

---

## 9. Lockbox Provisioning

Ops provisions the master key once. Everything after that is the app developer's domain.

```bash
# === Ops (one-time setup) ===

# Generate and store the keystore master key in Lockbox
rivers-lockbox add netinventory/keystore-master-key --value "$(openssl rand -hex 32)"

# Verify
rivers-lockbox list


# === App Developer (ongoing) ===

# Initialize the application keystore
rivers-keystore init --path data/app.keystore

# Generate an AES-256 encryption key for credential storage
rivers-keystore generate credential-key --type aes-256 --path data/app.keystore

# Later: rotate the key
rivers-keystore rotate credential-key --path data/app.keystore

# List keys
rivers-keystore list --path data/app.keystore
```

The separation is clean: ops never touches `rivers-keystore`, developers never touch `rivers-lockbox`. The only handoff is the master key alias.

---

## 10. Implementation Notes

### For the Rivers Team

**New components:**

1. **`[[keystores]]` resource type** — new section in `resources.toml`, resolved alongside `[[datasources]]` during startup. Lockbox provides the master key; the keystore file provides the encrypted key material.

2. **`rivers-keystore` CLI** — init, generate, list, info, delete, rotate. Separate binary. Keystore file format: `*.appkeystore` (distinct from `*.rkeystore`).

3. **Keystore resolver** — loads at startup (step 6a/6b), decrypts the keystore using the Lockbox-provided master key, holds decrypted keys in memory for the app lifetime. Per-app scoped — each app has its own keystore instance.

4. **`Rivers.keystore` host functions** — `has(name)`, `info(name)`. Read-only metadata. Never exposes raw key bytes to handler code.

5. **`Rivers.crypto.encrypt` / `Rivers.crypto.decrypt`** — V8 and WASM host functions. Resolve the named key from the application keystore internally, perform AES-256-GCM, return results. Key bytes stay in Rust memory.

**Rust crate:** `aes-gcm` (well-audited, widely used). Nonces via `OsRng`. Key bytes never cross the FFI boundary into V8/WASM heap.

**Security invariants:**

- Nonces MUST be randomly generated per call. Never accept a caller-supplied nonce.
- Plaintext MUST NOT appear in structured logs or trace spans.
- Key bytes MUST NOT be returned to handler code — handlers reference keys by name only.
- Auth tag failure errors MUST be generic (no padding oracle).
- Application keystore MUST NOT be readable by other apps in the same bundle.

---

## 11. Scope & Effort Estimate

| Component | Estimated Effort |
|-----------|-----------------|
| `[[keystores]]` resource type in resources.toml | 1–2 days |
| `rivers-keystore` CLI (init, generate, list, info, delete, rotate) | 3–4 days |
| Application keystore file format + encryption at rest | 2 days |
| Keystore resolver (Lockbox master key → unlock at startup) | 2 days |
| `Rivers.keystore` host functions (has, info) | 1 day |
| `Rivers.crypto.encrypt` / `decrypt` in Rust | 2–3 days |
| V8 host function bindings | 1 day |
| WASM host function bindings | 1 day |
| `riversctl validate` keystore checks | 1 day |
| Tests (unit + integration) | 3 days |
| Documentation | 1–2 days |
| **Total** | **~18–22 days** |

---

## 12. Alternatives Considered

| Alternative | Why Not |
|-------------|---------|
| **Store encryption keys directly in Lockbox** | Wrong abstraction — Lockbox has no key typing, versioning, rotation, or handler-accessible API. App developers need ops access. No app-level isolation. |
| **Expose raw Lockbox values to handlers** | Keys in JS heap leak via errors, logs, memory dumps. Handlers should never see raw key material. |
| **Env var / key file for master key (no Lockbox)** | Breaks the established Rivers resource pattern. Ops now has two root-of-trust systems to manage. Lockbox already solves this. |
| **Use `ctx.store` for key material** | Unencrypted KV store. No encryption at rest, no key typing, no rotation, lost on restart. |
| **WASM-only encryption module** | Fragments the API. JS handlers are the common case. Doesn't solve key management. |
| **External vault via HTTP datasource** | Adds latency, network dependency, operational complexity. Breaks single-node simplicity. |
| **Do nothing** | Every app reinvents key management badly. |

---

## 13. Acceptance Criteria

1. `[[keystores]]` in `resources.toml` declares a keystore resource with a Lockbox alias for the master key.
2. `[data.keystore.*]` in `app.toml` configures the keystore file path.
3. Keystore master key is resolved from Lockbox at app startup — same resolution path as datasource credentials.
4. `rivers-keystore` CLI can init, generate, list, info, delete, and rotate keys — completely independent of `rivers-lockbox`.
5. `Rivers.keystore.has(name)` and `Rivers.keystore.info(name)` work from handler code.
6. `Rivers.crypto.encrypt(keyName, plaintext)` returns `{ ciphertext, nonce, key_version }` using AES-256-GCM.
7. `Rivers.crypto.decrypt(keyName, ciphertext, nonce, { key_version })` returns the original plaintext.
8. Decryption with a wrong key, wrong version, or tampered ciphertext throws a generic error.
9. Key bytes never appear in handler memory, logs, or error messages.
10. Keys are scoped to the declaring app — other apps in the bundle cannot access them.
11. Keystore file is encrypted at rest using the Lockbox-provided master key.
12. `riversctl validate` checks keystore file existence, Lockbox alias resolution, and key references.
13. Works identically in JavaScript and WASM handlers.
14. Key rotation creates a new version without destroying previous versions.

---

## 14. Impact If Not Implemented

Applications needing field-level encryption are forced to either:

- Put application keys directly in Lockbox, conflating system and application concerns with no versioning or rotation support.
- Ship a custom WASM module for encryption, adding build complexity without solving key management.
- Depend on an external vault service, breaking single-node deployment simplicity.
- Store sensitive data in plaintext.

The pattern is already established: Lockbox unlocks resources, resources serve the application. The application keystore is just a new resource type that's been missing.
