# Tutorial: Application Keystore

**Rivers v0.52.5**

## Overview

The Application Keystore provides per-app encryption keys for field-level encryption in handler code. Keys are managed through the `rivers-keystore` CLI, stored in an encrypted keystore file that ships with the bundle, and unlocked at startup by a master key held in LockBox.

Use the Application Keystore when:
- You need to encrypt credentials, tokens, or PII before storing them in a database
- You need reversible encryption (not just hashing) -- e.g., retrieving a stored password for automation
- You need key rotation without re-encrypting all existing data at once
- You need application-scoped keys isolated from other apps in the same bundle

The keystore uses AES-256-GCM with randomly generated 96-bit nonces. Key bytes never leave Rust memory -- handlers reference keys by name, and all encryption operations happen on the host side.

LockBox's role is narrow: it holds the master key that unlocks the keystore, the same way it holds a password that unlocks a PostgreSQL connection. LockBox does not store application encryption keys or participate in encrypt/decrypt operations.

## Prerequisites

- LockBox initialized and accessible
- `rivers-keystore` CLI installed
- An app bundle with at least one datasource for persisting ciphertext

---

## Step 1: Provision the Master Key in LockBox

The master key encrypts the keystore file at rest. Operations provisions it once -- everything after that is the app developer's domain.

```bash
# Generate a random 256-bit master key and store it in LockBox
rivers-lockbox add netinventory/keystore-master-key \
    --value "$(openssl rand -hex 32)"

# Verify the entry exists
rivers-lockbox list
```

This is the only step that requires LockBox access. The master key alias (`netinventory/keystore-master-key`) is referenced in `resources.toml`.

---

## Step 2: Create the Keystore

Use the `rivers-keystore` CLI to initialize a keystore file and generate encryption keys.

```bash
# Initialize an empty keystore
rivers-keystore init --path data/app.keystore

# Generate an AES-256 encryption key for credential storage
rivers-keystore generate credential-key \
    --type aes-256 \
    --path data/app.keystore

# Verify the key was created
rivers-keystore list --path data/app.keystore
# Output:
#   credential-key  aes-256  version=1  created=2026-03-27T14:30:00Z

# Show detailed metadata
rivers-keystore info credential-key --path data/app.keystore
# Output:
#   name=credential-key type=aes-256 version=1 created=2026-03-27T14:30:00Z
```

The keystore file (`data/app.keystore`) ships with the app bundle. It contains encrypted key material -- the master key from LockBox is required to unlock it at runtime.

### CLI Reference

| Command | Description |
|---------|-------------|
| `rivers-keystore init --path <file>` | Create an empty keystore file |
| `rivers-keystore generate <name> --type aes-256 --path <file>` | Generate and store a new encryption key |
| `rivers-keystore list --path <file>` | List all keys (names and metadata only) |
| `rivers-keystore info <name> --path <file>` | Show detailed metadata for a key |
| `rivers-keystore rotate <name> --path <file>` | Create a new version of a key (see Step 7) |
| `rivers-keystore delete <name> --path <file>` | Delete a key and all its versions |

---

## Step 3: Declare the Keystore in resources.toml

Declare the keystore as a resource, just like a datasource. The `lockbox` field references the master key alias provisioned in Step 1.

```toml
# resources.toml

[[datasources]]
name       = "inventory_db"
driver     = "sqlite"
x-type     = "sqlite"
nopassword = true
required   = true

[[keystores]]
name     = "app-keys"
lockbox  = "netinventory/keystore-master-key"
required = true
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Logical name -- referenced in `app.toml` and handler code |
| `lockbox` | yes | LockBox alias for the master key that encrypts the keystore at rest |
| `required` | yes | If `true`, app startup fails if the keystore cannot be unlocked |

---

## Step 4: Configure the Keystore in app.toml

Point the keystore to the file created in Step 2.

```toml
# app.toml

[data.keystore.app-keys]
path = "data/app.keystore"
```

| Field | Required | Description |
|-------|----------|-------------|
| `path` | yes | Path to the keystore file, relative to the app directory |

At startup, Rivers resolves the master key from LockBox, unlocks the keystore file, and holds the decrypted keys in Rust memory for the app lifetime.

---

## Step 5: Encrypt Data in a Handler

Use `Rivers.crypto.encrypt()` to encrypt plaintext using a named key from the application keystore.

```javascript
// libraries/handlers/credentials.js

function createCredential(ctx) {
    var body = ctx.request.body;

    if (!body.device_id) throw new Error("device_id is required");
    if (!body.password) throw new Error("password is required");

    // Encrypt the password using the "credential-key" from the keystore
    var encrypted = Rivers.crypto.encrypt("credential-key", body.password);

    // Store ciphertext, nonce, and key version in the database
    var result = ctx.dataview("insert_credential", {
        device_id:      body.device_id,
        encrypted_pass: encrypted.ciphertext,
        pass_nonce:     encrypted.nonce,
        pass_key_ver:   encrypted.key_version
    });

    Rivers.log.info("credential created", { device_id: body.device_id });

    ctx.resdata = {
        id: result.id,
        device_id: body.device_id,
        key_version: encrypted.key_version
    };
}
```

### encrypt() Return Value

```json
{
  "ciphertext": "base64-encoded-ciphertext",
  "nonce": "base64-encoded-96-bit-nonce",
  "key_version": 1
}
```

All three values must be stored alongside the ciphertext in your database. The `key_version` is critical for decryption after key rotation.

### Database Schema

Store the encryption metadata alongside the ciphertext:

```sql
CREATE TABLE credentials (
    id             TEXT PRIMARY KEY,
    device_id      TEXT NOT NULL,
    encrypted_pass TEXT NOT NULL,       -- base64 ciphertext
    pass_nonce     TEXT NOT NULL,       -- base64 nonce
    pass_key_ver   INTEGER NOT NULL,    -- key version used for encryption
    created_at     TEXT NOT NULL
);
```

---

## Step 6: Decrypt Data in a Handler

Use `Rivers.crypto.decrypt()` to recover plaintext. Pass the `key_version` that was stored alongside the ciphertext.

```javascript
// libraries/handlers/credentials.js (continued)

function getCredential(ctx) {
    var id = ctx.request.path_params.id;

    var cred = ctx.dataview("get_credential", { id: id });
    if (!cred) throw new Error("credential not found");

    // Decrypt using the key version that was used during encryption
    var plaintext = Rivers.crypto.decrypt(
        "credential-key",
        cred.encrypted_pass,
        cred.pass_nonce,
        { key_version: cred.pass_key_ver }
    );

    Rivers.log.info("credential retrieved", { device_id: cred.device_id });

    ctx.resdata = {
        id: cred.id,
        device_id: cred.device_id,
        password: plaintext
    };
}
```

### decrypt() Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `keyName` | string | Key name in the application keystore |
| `ciphertext` | string | Base64-encoded ciphertext from encrypt() |
| `nonce` | string | Base64-encoded nonce from encrypt() |
| `options.key_version` | integer | Key version from encrypt() (required) |
| `options.aad` | string | Additional authenticated data (must match what was used during encryption) |

Decryption with a wrong key, wrong version, or tampered ciphertext throws a generic `DecryptionFailed` error. The error message does not reveal which component failed -- this prevents padding oracle attacks.

---

## Step 7: Key Rotation

Key rotation creates a new version of a key without destroying previous versions. Existing ciphertext remains decryptable using the old version.

### Rotate the Key

```bash
rivers-keystore rotate credential-key --path data/app.keystore

# Verify
rivers-keystore info credential-key --path data/app.keystore
# Output:
#   name=credential-key type=aes-256 version=2 created=2026-03-27T14:30:00Z
```

After rotation:
- New `encrypt()` calls automatically use version 2
- Old ciphertext encrypted with version 1 is still decryptable (the `key_version` stored with the ciphertext tells the runtime which version to use)
- Version 1 is retained read-only

### Lazy Re-encryption

Re-encrypt old data on read to migrate to the latest key version:

```javascript
function getCredential(ctx) {
    var id = ctx.request.path_params.id;
    var cred = ctx.dataview("get_credential", { id: id });
    if (!cred) throw new Error("credential not found");

    // Decrypt with the original key version
    var plaintext = Rivers.crypto.decrypt(
        "credential-key",
        cred.encrypted_pass,
        cred.pass_nonce,
        { key_version: cred.pass_key_ver }
    );

    // Re-encrypt if using an old key version
    var currentVersion = Rivers.keystore.info("credential-key").version;
    if (cred.pass_key_ver < currentVersion) {
        var reencrypted = Rivers.crypto.encrypt("credential-key", plaintext);
        ctx.dataview("update_credential_encryption", {
            id:             cred.id,
            encrypted_pass: reencrypted.ciphertext,
            pass_nonce:     reencrypted.nonce,
            pass_key_ver:   reencrypted.key_version
        });
        Rivers.log.info("credential re-encrypted", {
            device_id: cred.device_id,
            old_version: cred.pass_key_ver,
            new_version: reencrypted.key_version
        });
    }

    ctx.resdata = {
        id: cred.id,
        device_id: cred.device_id,
        password: plaintext
    };
}
```

This pattern migrates data incrementally -- each read upgrades one record. No downtime, no batch migration required.

---

## Step 8: Additional Authenticated Data (AAD)

AAD binds ciphertext to a context value (e.g., a record ID). The AAD is authenticated but not encrypted -- if the ciphertext is moved to a different record, decryption fails.

```javascript
function createCredential(ctx) {
    var body = ctx.request.body;

    // Bind the ciphertext to the device ID
    var encrypted = Rivers.crypto.encrypt("credential-key", body.password, {
        aad: body.device_id
    });

    var result = ctx.dataview("insert_credential", {
        device_id:      body.device_id,
        encrypted_pass: encrypted.ciphertext,
        pass_nonce:     encrypted.nonce,
        pass_key_ver:   encrypted.key_version
    });

    ctx.resdata = { id: result.id, device_id: body.device_id };
}

function getCredential(ctx) {
    var id = ctx.request.path_params.id;
    var cred = ctx.dataview("get_credential", { id: id });
    if (!cred) throw new Error("credential not found");

    // Must provide the same AAD used during encryption
    var plaintext = Rivers.crypto.decrypt(
        "credential-key",
        cred.encrypted_pass,
        cred.pass_nonce,
        { key_version: cred.pass_key_ver, aad: cred.device_id }
    );

    ctx.resdata = {
        id: cred.id,
        device_id: cred.device_id,
        password: plaintext
    };
}
```

If someone copies a ciphertext from one record to another, decryption fails because the AAD (device ID) no longer matches. The error is the same generic `DecryptionFailed` -- no oracle.

---

## Step 9: Key Metadata

Use `Rivers.keystore.has()` and `Rivers.keystore.info()` to check key existence and read metadata from handler code. These never expose raw key bytes.

```javascript
// Check if a key exists before using it
if (!Rivers.keystore.has("credential-key")) {
    throw new Error("encryption key not configured");
}

// Get key metadata
var meta = Rivers.keystore.info("credential-key");
// Returns: { name: "credential-key", type: "aes-256", version: 2, created_at: "2026-03-27T..." }

Rivers.log.info("using key", { name: meta.name, version: meta.version });
```

| Function | Returns |
|----------|---------|
| `Rivers.keystore.has(name)` | `true` if the key exists, `false` otherwise |
| `Rivers.keystore.info(name)` | `{ name, type, version, created_at }` or throws if key not found |

---

## Complete Example

### Bundle Structure

```
netinventory-bundle/
├── manifest.toml
├── netinventory-service/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── data/
│   │   └── app.keystore
│   ├── schemas/
│   │   └── credential.schema.json
│   └── libraries/
│       └── handlers/
│           └── credentials.js
└── netinventory-main/
    ├── manifest.toml
    ├── resources.toml
    ├── app.toml
    └── libraries/
        └── spa/
```

### manifest.toml (bundle)

```toml
[bundle]
name    = "netinventory"
version = "1.0.0"

[[apps]]
name = "netinventory-service"
path = "netinventory-service"

[[apps]]
name = "netinventory-main"
path = "netinventory-main"
```

### manifest.toml (service app)

```toml
[app]
appId = "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
name  = "netinventory-service"
type  = "service"
port  = 9100
```

### resources.toml

```toml
[[datasources]]
name       = "inventory_db"
driver     = "sqlite"
x-type     = "sqlite"
nopassword = true
required   = true

[[keystores]]
name     = "app-keys"
lockbox  = "netinventory/keystore-master-key"
required = true
```

### app.toml

```toml
# ─────────────────────────────────────────────
# Datasource
# ─────────────────────────────────────────────

[data.datasources.inventory_db]
name       = "inventory_db"
driver     = "sqlite"
path       = "data/inventory.db"
nopassword = true

# ─────────────────────────────────────────────
# Keystore
# ─────────────────────────────────────────────

[data.keystore.app-keys]
path = "data/app.keystore"

# ─────────────────────────────────────────────
# DataViews
# ─────────────────────────────────────────────

[data.dataviews.insert_credential]
name       = "insert_credential"
datasource = "inventory_db"
query      = "INSERT INTO credentials (id, device_id, encrypted_pass, pass_nonce, pass_key_ver, created_at) VALUES (hex(randomblob(16)), $1, $2, $3, $4, datetime('now')) RETURNING id, device_id, created_at"

[[data.dataviews.insert_credential.parameters]]
name     = "device_id"
type     = "string"
required = true

[[data.dataviews.insert_credential.parameters]]
name     = "encrypted_pass"
type     = "string"
required = true

[[data.dataviews.insert_credential.parameters]]
name     = "pass_nonce"
type     = "string"
required = true

[[data.dataviews.insert_credential.parameters]]
name     = "pass_key_ver"
type     = "integer"
required = true

# ─────────────────────────────────────────────

[data.dataviews.get_credential]
name       = "get_credential"
datasource = "inventory_db"
query      = "SELECT id, device_id, encrypted_pass, pass_nonce, pass_key_ver FROM credentials WHERE id = $1"

[[data.dataviews.get_credential.parameters]]
name     = "id"
type     = "string"
required = true

# ─────────────────────────────────────────────

[data.dataviews.update_credential_encryption]
name       = "update_credential_encryption"
datasource = "inventory_db"
query      = "UPDATE credentials SET encrypted_pass = $1, pass_nonce = $2, pass_key_ver = $3 WHERE id = $4"

[[data.dataviews.update_credential_encryption.parameters]]
name     = "encrypted_pass"
type     = "string"
required = true

[[data.dataviews.update_credential_encryption.parameters]]
name     = "pass_nonce"
type     = "string"
required = true

[[data.dataviews.update_credential_encryption.parameters]]
name     = "pass_key_ver"
type     = "integer"
required = true

[[data.dataviews.update_credential_encryption.parameters]]
name     = "id"
type     = "string"
required = true

# ─────────────────────────────────────────────

[data.dataviews.list_credentials]
name       = "list_credentials"
datasource = "inventory_db"
query      = "SELECT id, device_id, pass_key_ver, created_at FROM credentials ORDER BY created_at DESC LIMIT $1 OFFSET $2"

[[data.dataviews.list_credentials.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[[data.dataviews.list_credentials.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

[api.views.create_credential]
path            = "credentials"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.create_credential.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/credentials.js"
entrypoint = "createCredential"
resources  = ["inventory_db"]

# ─────────────────────────────────────────────

[api.views.get_credential]
path            = "credentials/{id}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_credential.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/credentials.js"
entrypoint = "getCredential"
resources  = ["inventory_db"]

# ─────────────────────────────────────────────

[api.views.list_credentials]
path            = "credentials"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_credentials.handler]
type     = "dataview"
dataview = "list_credentials"

[api.views.list_credentials.parameter_mapping.query]
limit  = "limit"
offset = "offset"

# ─────────────────────────────────────────────
# ProcessPool
# ─────────────────────────────────────────────

[runtime.process_pools.default]
engine          = "v8"
workers         = 4
task_timeout_ms = 5000
max_heap_mb     = 128
```

### libraries/handlers/credentials.js

```javascript
// libraries/handlers/credentials.js

function createCredential(ctx) {
    var body = ctx.request.body;

    if (!body.device_id) throw new Error("device_id is required");
    if (!body.password) throw new Error("password is required");

    // Encrypt the password
    var encrypted = Rivers.crypto.encrypt("credential-key", body.password, {
        aad: body.device_id
    });

    // Store ciphertext in the database
    var result = ctx.dataview("insert_credential", {
        device_id:      body.device_id,
        encrypted_pass: encrypted.ciphertext,
        pass_nonce:     encrypted.nonce,
        pass_key_ver:   encrypted.key_version
    });

    Rivers.log.info("credential created", { device_id: body.device_id });

    ctx.resdata = {
        id: result.id,
        device_id: body.device_id,
        key_version: encrypted.key_version
    };
}

function getCredential(ctx) {
    var id = ctx.request.path_params.id;

    var cred = ctx.dataview("get_credential", { id: id });
    if (!cred) throw new Error("credential not found");

    // Decrypt with the original key version and AAD
    var plaintext = Rivers.crypto.decrypt(
        "credential-key",
        cred.encrypted_pass,
        cred.pass_nonce,
        { key_version: cred.pass_key_ver, aad: cred.device_id }
    );

    // Lazy re-encryption to latest key version
    var currentVersion = Rivers.keystore.info("credential-key").version;
    if (cred.pass_key_ver < currentVersion) {
        var reencrypted = Rivers.crypto.encrypt("credential-key", plaintext, {
            aad: cred.device_id
        });
        ctx.dataview("update_credential_encryption", {
            id:             cred.id,
            encrypted_pass: reencrypted.ciphertext,
            pass_nonce:     reencrypted.nonce,
            pass_key_ver:   reencrypted.key_version
        });
        Rivers.log.info("credential re-encrypted", {
            device_id: cred.device_id,
            old_version: cred.pass_key_ver,
            new_version: reencrypted.key_version
        });
    }

    Rivers.log.info("credential retrieved", { device_id: cred.device_id });

    ctx.resdata = {
        id: cred.id,
        device_id: cred.device_id,
        password: plaintext
    };
}
```

## Testing

```bash
# Create a credential
curl -X POST http://localhost:9100/credentials \
  -H "Content-Type: application/json" \
  -d '{"device_id":"switch-core-01","password":"s3cret-p@ss"}'

# Retrieve and decrypt a credential
curl http://localhost:9100/credentials/abc123

# List credentials (metadata only -- no plaintext)
curl http://localhost:9100/credentials
```

## Security Notes

1. **Key bytes never enter handler memory.** Handlers reference keys by name. All AES-256-GCM operations happen in Rust. Raw key material never crosses the FFI boundary into the V8 heap.

2. **Nonces are randomly generated per call.** The runtime generates a fresh 96-bit nonce via `OsRng` for every `encrypt()` call. Callers cannot supply their own nonce.

3. **Decryption errors are generic.** Wrong key, wrong version, tampered ciphertext, and AAD mismatch all produce the same `DecryptionFailed` error. No padding oracle.

4. **Plaintext is never logged.** The encrypt/decrypt host functions do not emit plaintext in structured logs or trace spans.

5. **App isolation.** Each app's keystore is scoped to its `appId`. Other apps in the same bundle cannot access another app's keys.

6. **Keystore is encrypted at rest.** The keystore file is encrypted using the LockBox-provided master key. Without the master key, the file is opaque.

7. **Key rotation is non-destructive.** Old key versions are retained for decryption. Only new `encrypt()` calls use the latest version. Migrate data at your own pace using lazy re-encryption.

### Error Reference

| Condition | Error |
|-----------|-------|
| No keystore declared for this app | `KeystoreNotConfigured: no keystore resource declared` |
| Keystore file not found | `KeystoreNotFound: keystore file does not exist` |
| Master key not in LockBox | `KeystoreLocked: lockbox alias not found` |
| Key name not found | `KeyNotFound: key does not exist in application keystore` |
| Key version not found | `KeyVersionNotFound: key version N does not exist` |
| Wrong key type for operation | `InvalidKeyType: encrypt requires an AES key` |
| Decryption failure (any cause) | `DecryptionFailed: authentication tag mismatch` |
| Nonce missing or malformed | `InvalidNonce: expected 12-byte base64-encoded nonce` |

### Algorithm Specification

| Property | Value |
|----------|-------|
| Algorithm | AES-256-GCM |
| Key size | 256 bits (32 bytes) |
| Nonce size | 96 bits (12 bytes), randomly generated per encrypt call |
| Auth tag size | 128 bits (appended to ciphertext) |
| Encoding | Base64 for ciphertext and nonce |
| Key versioning | Integer version, monotonically increasing |
