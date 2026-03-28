# CLI Reference

Rivers ships five binaries: `riversd`, `riversctl`, `rivers-lockbox`, `rivers-keystore`, and `riverpackage`.

---

## riversd

The main server. Loads config, resolves secrets, builds the runtime, and starts listening.

**Startup sequence:** config -> LockBox -> bundle -> keystore unlock -> drivers -> DataView executor -> view router -> GraphQL schema -> admin server -> hot reload watcher -> TLS listener.

**Exits on:** SIGTERM, SIGINT, ctrl+c.

| Flag | Description |
|------|-------------|
| `--config <path>` | Path to config file. Enables hot reload. |
| `--port <port>` | Override `base.port` from config. |
| `--no-ssl` | Disable TLS (development only). |
| `--no-admin-auth` | Disable Ed25519 admin auth (initial setup). |

```sh
riversd --config ./bundle/config.toml --port 8080
riversd --config ./bundle/config.toml --no-ssl --no-admin-auth
```

---

## riversctl

Admin client and management tool. Authenticates to the admin API using Ed25519 signatures.

**Auth mechanism:** Signs `{method}\n{path}\n{body_sha256_hex}\n{unix_timestamp_ms}` with private key. Sends `X-Rivers-Signature` and `X-Rivers-Timestamp` headers.

| Command | Description |
|---------|-------------|
| `riversctl start` | Start riversd (convenience wrapper). |
| `riversctl deploy <bundle_path>` | Deploy a bundle via admin API. |
| `riversctl tls gen` | Generate self-signed TLS certificate. |
| `riversctl tls request <domain>` | Generate CSR for a domain. |
| `riversctl tls import <cert> <key>` | Import certificate and key pair. |
| `riversctl tls show <cert>` | Display certificate details. |
| `riversctl tls list` | List certificates in data directory. |
| `riversctl tls expire <cert>` | Check certificate expiration. |

```sh
riversctl start
riversctl deploy ./address-book-bundle
riversctl tls gen
riversctl tls request api.example.com
riversctl tls import ./cert.pem ./key.pem
riversctl tls show ./cert.pem
riversctl tls list
riversctl tls expire ./cert.pem
```

### exec subcommands

| Command | Description |
|---------|-------------|
| `riversctl exec hash <path>` | Print SHA-256 hash of file in TOML-ready format. |
| `riversctl exec verify <path> <sha256>` | Verify file matches expected SHA-256 hash. |

```sh
riversctl exec hash /usr/lib/rivers/scripts/netscan.py
# sha256 = "a1b2c3d4..."

riversctl exec verify /usr/lib/rivers/scripts/netscan.py a1b2c3d4...
# [OK] hash matches
```

---

## rivers-lockbox

Secret management CLI. Manages an Age-encrypted keystore (X25519 + ChaCha20-Poly1305).

**Key sources:** environment variable, file (chmod 600 enforced), SSH agent.

| Command | Description |
|---------|-------------|
| `rivers-lockbox init` | Create a new encrypted keystore. |
| `rivers-lockbox add <name> <value>` | Add a secret. |
| `rivers-lockbox list` | List entry names (values not shown). |
| `rivers-lockbox show <name>` | Decrypt and display a secret. |
| `rivers-lockbox alias <name> <alias>` | Create an alias for an entry. |
| `rivers-lockbox unalias <alias>` | Remove an alias. |
| `rivers-lockbox rotate <name> <new_value>` | Rotate a secret's value. |
| `rivers-lockbox remove <name>` | Delete an entry. |
| `rivers-lockbox rekey` | Re-encrypt keystore with a new master key. |
| `rivers-lockbox validate` | Verify keystore integrity. |

```sh
rivers-lockbox init
rivers-lockbox add db_password "s3cret"
rivers-lockbox list
rivers-lockbox show db_password
rivers-lockbox alias db_password pg_pass
rivers-lockbox rotate db_password "n3w_s3cret"
rivers-lockbox rekey
rivers-lockbox validate
```

---

## rivers-keystore

Application keystore management CLI. Manages per-app AES-256 encryption keys stored in an Age-encrypted file. Master key sourced from `RIVERS_KEYSTORE_KEY` environment variable (Age identity string — typically provisioned via LockBox).

| Command | Description |
|---------|-------------|
| `rivers-keystore init --path <path>` | Create a new application keystore. |
| `rivers-keystore generate <name> --type aes-256 --path <path>` | Generate and store a new encryption key. |
| `rivers-keystore list --path <path>` | List key names and metadata (never raw material). |
| `rivers-keystore info <name> --path <path>` | Show key metadata (type, version, created). |
| `rivers-keystore rotate <name> --path <path>` | Create new key version (old versions kept for decryption). |
| `rivers-keystore delete <name> --path <path>` | Delete a key. |

```sh
export RIVERS_KEYSTORE_KEY="AGE-SECRET-KEY-..."
rivers-keystore init --path data/app.keystore
rivers-keystore generate credential-key --type aes-256 --path data/app.keystore
rivers-keystore list --path data/app.keystore
rivers-keystore info credential-key --path data/app.keystore
rivers-keystore rotate credential-key --path data/app.keystore
rivers-keystore delete credential-key --path data/app.keystore
```

Key differences from `rivers-lockbox`:
- **Stores** typed encryption keys (AES-256), not arbitrary secrets
- **Rotation** creates new versions (non-destructive), not replacement
- **Scoped** to a single application, not system-wide
- **Used by** handler code via `Rivers.crypto.encrypt/decrypt`

---

## riverpackage

Bundle packaging and validation tool.

| Flag | Description |
|------|-------------|
| `--pre-flight <bundle_dir>` | Validate bundle structure without starting the server. |

Checks: manifest, app configs, schema files, datasource references, DataView references.

```sh
riverpackage --pre-flight ./address-book-bundle
```
