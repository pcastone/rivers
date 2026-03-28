# Tutorial: Application Keystore

**Rivers v0.52.5**

## Overview

The Application Keystore provides application-scoped encryption keys for encrypting and decrypting data in handler code. Keys are managed per-application, versioned for rotation, and never exposed as raw bytes to JavaScript or WASM handlers.

Use the Application Keystore when:
- Storing credentials or secrets in a database (encrypt at rest, decrypt on retrieval)
- Encrypting PII fields to meet compliance requirements
- Managing OAuth tokens or API keys for third-party integrations
- Implementing per-tenant data encryption in multi-tenant applications

The keystore follows the standard Rivers resource pattern: the master key lives in **LockBox** (just like a database password), and the keystore itself is declared in `resources.toml` and configured in `app.toml`. Handler code references keys by name -- raw key bytes never leave Rust memory.

---

## Prerequisites

- A Rivers app bundle with a valid `manifest.toml`
- LockBox configured and accessible (`rivers-lockbox` CLI installed)
- `rivers-keystore` CLI installed

---

## Step 1: Provision the Master Key in LockBox

The master key unlocks the application keystore at startup. Ops provisions it once -- the same way they provision a database password.

```bash
# Generate a 256-bit master key (hex-encoded)
rivers-lockbox add \
    --name myapp/keystore-master-key \
    --type string \
    --alias myapp/keystore-master-key
# Value: **** (enter or paste a 64-char hex string at the hidden prompt)
```

Alternatively, generate and store in one step:

```bash
rivers-lockbox add \
    --name myapp/keystore-master-key \
    --type string \
    --alias myapp/keystore-master-key \
    --value "$(openssl rand -hex 32)"
```

After this step, `rivers-lockbox list` should show the entry:

```
myapp/keystore-master-key   string   alias=myapp/keystore-master-key
```

---

## Step 2: Create the Application Keystore

The `rivers-keystore` CLI manages the keystore file. Set the master key in the environment so the CLI can encrypt/decrypt the keystore.

```bash
# Export the master key (same value stored in LockBox)
export RIVERS_KEYSTORE_KEY="<64-char-hex-master-key>"

# Initialize a new keystore file
rivers-keystore init --path data/app.keystore

# Generate an AES-256 encryption key named "credential-key"
rivers-keystore generate credential-key --type aes-256 --path data/app.keystore

# Verify the key was created
rivers-keystore list --path data/app.keystore
# Output:
# NAME              TYPE      VERSION   CREATED
# credential-key    aes-256   1         2026-03-27T14:30:00Z

# View key metadata (never shows raw bytes)
rivers-keystore info credential-key --path data/app.keystore
# Output:
# name=credential-key type=aes-256 version=1 created=2026-03-27T14:30:00Z
```

The keystore file (`data/app.keystore`) ships with your app bundle. It is encrypted at rest using the master key.

---

## Step 3: Declare the Keystore in resources.toml

Declare the keystore as a resource, just like a datasource. The `lockbox` field tells Rivers which LockBox entry holds the master key.

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
lockbox  = "myapp/keystore-master-key"
required = true
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Logical name -- referenced in `app.toml` and handler code |
| `lockbox` | yes | LockBox alias for the master key that encrypts the keystore at rest |
| `required` | yes | If `true`, app startup fails if the keystore cannot be unlocked |

---

## Step 4: Configure the Keystore in app.toml

Point the keystore to its file path:

```toml
# app.toml

[data.keystore.app-keys]
path = "data/app.keystore"
```

| Field | Required | Description |
|-------|----------|-------------|
| `path` | yes | Path to the keystore file, relative to the app directory |

At startup, Rivers resolves the master key from LockBox, decrypts the keystore file, and holds the decrypted keys in Rust memory for the app's lifetime.

---

## Step 5: Use in Handler Code -- Encrypt

Create a handler that encrypts a credential before storing it in the database.

```javascript
// libraries/handlers/credentials.js

function createCredential(ctx) {
    var body = ctx.request.body;

    if (!body) throw new Error("request body required");
    if (!body.id) throw new Error("id is required");
    if (!body.password) throw new Error("password is required");

    // Encrypt the password using the application keystore key
    var enc = Rivers.crypto.encrypt("credential-key", body.password);

    // Store ciphertext + nonce + key_version in the database
    ctx.dataview("save_credential", {
        id: body.id,
        encrypted_pass: enc.ciphertext,
        pass_nonce: enc.nonce,
        pass_key_ver: enc.key_version
    });

    Rivers.log.info("credential created", { id: body.id, key_version: enc.key_version });

    ctx.resdata = { success: true, id: body.id };
}
```

`Rivers.crypto.encrypt` returns:

```json
{
  "ciphertext": "base64-encoded-ciphertext",
  "nonce": "base64-encoded-96-bit-nonce",
  "key_version": 1
}
```

All three values must be stored in the database. The `key_version` is critical for decryption after key rotation.

---

## Step 6: Use in Handler Code -- Decrypt

Retrieve the ciphertext from the database and decrypt it:

```javascript
// libraries/handlers/credentials.js (continued)

function getCredential(ctx) {
    var id = ctx.request.path_params.id;

    var cred = ctx.dataview("get_credential", { id: id });
    if (!cred) throw new Error("credential not found");

    // Decrypt using the stored key version
    var password = Rivers.crypto.decrypt(
        "credential-key",
        cred.encrypted_pass,
        cred.pass_nonce,
        { key_version: cred.pass_key_ver }
    );

    ctx.resdata = { id: cred.id, password: password };
}
```

`Rivers.crypto.decrypt` returns the plaintext string, or throws a generic error on authentication failure (wrong key, wrong version, tampered ciphertext, or AAD mismatch).

---

## Step 7: Key Rotation

Rotate the key to create a new version. Old versions are retained for decrypting existing data.

```bash
export RIVERS_KEYSTORE_KEY="<64-char-hex-master-key>"
rivers-keystore rotate credential-key --path data/app.keystore

# Verify
rivers-keystore info credential-key --path data/app.keystore
# Output:
# name=credential-key type=aes-256 version=2 created=2026-03-27T14:30:00Z
```

After rotation:
- New `encrypt()` calls automatically use the latest version (version 2)
- Existing ciphertext still decrypts using the stored `key_version` (version 1)
- No downtime, no bulk re-encryption required

### Lazy Re-encryption Pattern

Re-encrypt old data on read to migrate to the latest key version over time:

```javascript
// libraries/handlers/credentials.js (continued)

function getCredentialWithReencrypt(ctx) {
    var id = ctx.request.path_params.id;

    var cred = ctx.dataview("get_credential", { id: id });
    if (!cred) throw new Error("credential not found");

    // Decrypt using the stored key version
    var password = Rivers.crypto.decrypt(
        "credential-key",
        cred.encrypted_pass,
        cred.pass_nonce,
        { key_version: cred.pass_key_ver }
    );

    // Re-encrypt if using an old key version
    var currentVersion = Rivers.keystore.info("credential-key").version;
    if (cred.pass_key_ver < currentVersion) {
        var reencrypted = Rivers.crypto.encrypt("credential-key", password);
        ctx.dataview("update_credential_encryption", {
            id: cred.id,
            encrypted_pass: reencrypted.ciphertext,
            pass_nonce: reencrypted.nonce,
            pass_key_ver: reencrypted.key_version
        });
        Rivers.log.info("re-encrypted credential", {
            id: cred.id,
            old_version: cred.pass_key_ver,
            new_version: reencrypted.key_version
        });
    }

    ctx.resdata = { id: cred.id, password: password };
}
```

---

## Step 8: Using AAD (Additional Authenticated Data)

AAD binds ciphertext to a specific context -- typically a record ID. If someone copies ciphertext from one record to another, decryption fails because the AAD does not match.

```javascript
// Encrypt with AAD bound to the device ID
function createDeviceCredential(ctx) {
    var body = ctx.request.body;
    var deviceId = body.device_id;

    var enc = Rivers.crypto.encrypt("credential-key", body.password, {
        aad: deviceId
    });

    ctx.dataview("save_device_credential", {
        device_id: deviceId,
        encrypted_pass: enc.ciphertext,
        pass_nonce: enc.nonce,
        pass_key_ver: enc.key_version
    });

    ctx.resdata = { success: true, device_id: deviceId };
}

// Decrypt with the same AAD
function getDeviceCredential(ctx) {
    var deviceId = ctx.request.path_params.device_id;

    var cred = ctx.dataview("get_device_credential", { device_id: deviceId });
    if (!cred) throw new Error("credential not found");

    var password = Rivers.crypto.decrypt(
        "credential-key",
        cred.encrypted_pass,
        cred.pass_nonce,
        { key_version: cred.pass_key_ver, aad: deviceId }
    );

    ctx.resdata = { device_id: deviceId, password: password };
}
```

The AAD is authenticated but not encrypted. If the `aad` value provided during decrypt does not match the value used during encrypt, the decryption fails with a generic `DecryptionFailed` error.

---

## Step 9: Key Metadata

Query key existence and metadata from handler code. Raw key bytes are never exposed.

```javascript
function checkKeyStatus(ctx) {
    // Check if a key exists
    var exists = Rivers.keystore.has("credential-key");

    if (!exists) {
        throw new Error("credential-key not found in keystore");
    }

    // Get key metadata
    var meta = Rivers.keystore.info("credential-key");
    // Returns: { name: "credential-key", type: "aes-256", version: 2, created_at: "..." }

    ctx.resdata = {
        key_name: meta.name,
        key_type: meta.type,
        current_version: meta.version,
        created_at: meta.created_at
    };
}
```

---

## Complete Example

### Bundle Structure

```
myapp-bundle/
├── manifest.toml
├── myapp-service/
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
```

### manifest.toml (bundle)

```toml
[bundle]
name    = "myapp-bundle"
version = "1.0.0"

[[apps]]
name = "myapp-service"
path = "myapp-service"
```

### manifest.toml (app)

```toml
[app]
appId   = "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
appName = "myapp-service"
type    = "service"
port    = 9200
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
lockbox  = "myapp/keystore-master-key"
required = true
```

### app.toml

```toml
# ─────────────────────────
# Datasource
# ─────────────────────────

[data.datasources.inventory_db]
name       = "inventory_db"
driver     = "sqlite"
nopassword = true

[data.datasources.inventory_db.config]
path = "data/inventory.db"

# ─────────────────────────
# Keystore
# ─────────────────────────

[data.keystore.app-keys]
path = "data/app.keystore"

# ─────────────────────────
# DataViews
# ─────────────────────────

[data.dataviews.save_credential]
name       = "save_credential"
datasource = "inventory_db"
query      = "INSERT INTO credentials (id, encrypted_pass, pass_nonce, pass_key_ver) VALUES (:id, :encrypted_pass, :pass_nonce, :pass_key_ver)"

[[data.dataviews.save_credential.parameters]]
name     = "id"
type     = "string"
required = true

[[data.dataviews.save_credential.parameters]]
name     = "encrypted_pass"
type     = "string"
required = true

[[data.dataviews.save_credential.parameters]]
name     = "pass_nonce"
type     = "string"
required = true

[[data.dataviews.save_credential.parameters]]
name     = "pass_key_ver"
type     = "integer"
required = true

# ─────────────────────────

[data.dataviews.get_credential]
name       = "get_credential"
datasource = "inventory_db"
query      = "SELECT id, encrypted_pass, pass_nonce, pass_key_ver FROM credentials WHERE id = :id"

[[data.dataviews.get_credential.parameters]]
name     = "id"
type     = "string"
required = true

# ─────────────────────────

[data.dataviews.update_credential_encryption]
name       = "update_credential_encryption"
datasource = "inventory_db"
query      = "UPDATE credentials SET encrypted_pass = :encrypted_pass, pass_nonce = :pass_nonce, pass_key_ver = :pass_key_ver WHERE id = :id"

[[data.dataviews.update_credential_encryption.parameters]]
name     = "id"
type     = "string"
required = true

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

# ─────────────────────────
# Views
# ─────────────────────────

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

# ─────────────────────────

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

[api.views.get_credential.parameter_mapping.path]
id = "id"
```

### libraries/handlers/credentials.js

```javascript
function createCredential(ctx) {
    var body = ctx.request.body;

    if (!body) throw new Error("request body required");
    if (!body.id) throw new Error("id is required");
    if (!body.password) throw new Error("password is required");

    var enc = Rivers.crypto.encrypt("credential-key", body.password);

    ctx.dataview("save_credential", {
        id: body.id,
        encrypted_pass: enc.ciphertext,
        pass_nonce: enc.nonce,
        pass_key_ver: enc.key_version
    });

    Rivers.log.info("credential created", { id: body.id, key_version: enc.key_version });

    ctx.resdata = { success: true, id: body.id };
}

function getCredential(ctx) {
    var id = ctx.request.path_params.id;

    var cred = ctx.dataview("get_credential", { id: id });
    if (!cred) throw new Error("credential not found");

    var password = Rivers.crypto.decrypt(
        "credential-key",
        cred.encrypted_pass,
        cred.pass_nonce,
        { key_version: cred.pass_key_ver }
    );

    // Re-encrypt if using an old key version
    var currentVersion = Rivers.keystore.info("credential-key").version;
    if (cred.pass_key_ver < currentVersion) {
        var reencrypted = Rivers.crypto.encrypt("credential-key", password);
        ctx.dataview("update_credential_encryption", {
            id: cred.id,
            encrypted_pass: reencrypted.ciphertext,
            pass_nonce: reencrypted.nonce,
            pass_key_ver: reencrypted.key_version
        });
        Rivers.log.info("re-encrypted credential", {
            id: cred.id,
            old_version: cred.pass_key_ver,
            new_version: reencrypted.key_version
        });
    }

    ctx.resdata = { id: cred.id, password: password };
}
```

### Testing

```bash
# Create a credential
curl -X POST http://localhost:9200/credentials \
  -H "Content-Type: application/json" \
  -d '{"id":"switch-core-01","password":"s3cret-p@ss"}'

# Retrieve the credential (decrypted)
curl http://localhost:9200/credentials/switch-core-01
```

---

## Security Notes

- **Key bytes never enter handler memory.** Handlers reference keys by name. `Rivers.crypto.encrypt` and `Rivers.crypto.decrypt` execute in Rust -- the V8/WASM isolate never sees raw key material.
- **Nonce is random per call.** Every `encrypt()` call generates a fresh 96-bit random nonce via `OsRng`. Callers cannot supply their own nonce.
- **Errors are generic.** Decryption failures (wrong key, tampered ciphertext, AAD mismatch) all produce the same `DecryptionFailed` error. No padding oracle.
- **Algorithm.** AES-256-GCM with 256-bit keys, 96-bit random nonces, and 128-bit authentication tags. Ciphertext and nonces are base64-encoded.
- **Store `key_version` alongside ciphertext.** This is required for decryption after key rotation. If you lose the key version, you must try all versions -- which is slow and may fail silently if multiple versions produce valid plaintext (extremely unlikely with AES-GCM, but store the version anyway).
- **Keystore scope.** Each application has its own keystore instance. Other apps in the same bundle cannot access another app's keys.
- **Plaintext never logged.** Rivers' structured logging never captures plaintext passed to `encrypt()` or returned by `decrypt()`. Log the operation and the record ID, not the data.

---

## Error Reference

| Condition | Error |
|-----------|-------|
| No keystore declared for this app | `KeystoreNotConfigured: no keystore resource declared` |
| Keystore file not found | `KeystoreNotFound: keystore file 'X' does not exist` |
| Master key not in LockBox | `KeystoreLocked: lockbox alias 'X' not found` |
| Key name not found | `KeyNotFound: key 'X' does not exist in application keystore` |
| Key version not found | `KeyVersionNotFound: key 'X' version N does not exist` |
| Decryption failure (any cause) | `DecryptionFailed: authentication tag mismatch` |
| Nonce missing or malformed | `InvalidNonce: expected 12-byte base64-encoded nonce` |
