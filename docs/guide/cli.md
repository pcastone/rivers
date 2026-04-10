# CLI Reference

**Rivers v0.54.0**

Rivers ships five binaries: `riversd`, `riversctl`, `rivers-lockbox`, `rivers-keystore`, and `riverpackage`.

> **v0.54.0:** In static builds (the default for `just deploy`), `riversd` is a single fat binary with all drivers and engines compiled in. Dynamic builds (`just deploy-dynamic` or `cargo deploy`) still produce a thin `riversd` plus engine dylibs in `lib/`. cdylib driver plugins are disabled — see the installation guide.

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
| `riversctl stop` | Stop a running riversd daemon. |
| `riversctl status` | Show riversd running state, PID, port, bundle. |
| `riversctl deploy <bundle_path>` | Deploy a bundle via admin API. |
| `riversctl doctor` | Pre-launch health checks. |
| `riversctl doctor --fix` | Auto-repair fixable issues. |
| `riverpackage validate <bundle>` | Four-layer bundle validation pipeline. |
| `riverpackage validate <bundle> --format json` | Machine-readable JSON validation output. |
| `riversctl tls gen` | Generate self-signed TLS certificate. |
| `riversctl tls renew` | Regenerate self-signed TLS certificate. |
| `riversctl tls request <domain>` | Generate CSR for a domain. |
| `riversctl tls import <cert> <key>` | Import certificate and key pair. |
| `riversctl tls show <cert>` | Display certificate details. |
| `riversctl tls list` | List certificates in data directory. |
| `riversctl tls expire <cert>` | Check certificate expiration. |

```sh
riversctl start
riversctl stop
riversctl status
riversctl deploy ./address-book-bundle
riversctl doctor
riversctl doctor --fix
riverpackage validate ./my-bundle
riverpackage validate ./my-bundle --format json
riversctl tls gen
riversctl tls renew
riversctl tls request api.example.com
riversctl tls import ./cert.pem ./key.pem
riversctl tls show ./cert.pem
riversctl tls list
riversctl tls expire ./cert.pem
```

### riversctl stop

Stop a running riversd daemon. Reads the PID file, sends SIGTERM, waits up to 30 seconds for graceful shutdown, then SIGKILL if needed.

```bash
riversctl stop
```

PID file location: `<RIVERS_HOME>/run/riversd.pid`

### riversctl status

Show whether riversd is running, its PID, port, and configured bundle path.

```bash
riversctl status
```

Output:
```
rivers: riversd is running (pid 12345)
  config: /opt/rivers/config/riversd.toml
  port:   8080
  bundle: /opt/rivers/apphome/my-bundle/
```

### riversctl doctor

Pre-launch health checks. Verifies riversd binary, config, TLS certs, log directories, lockbox, engine/plugin directories, and bundle path.

```bash
riversctl doctor                    # Run all checks
riversctl doctor --fix              # Auto-repair fixable issues
```

> **Note:** Bundle validation has moved to `riverpackage validate`. The `--lint` flag has been removed from `doctor`. Use `riverpackage validate <bundle_dir>` for bundle validation.

**`--fix` auto-repairs:**
- Lockbox missing -> runs `rivers-lockbox init`
- Lockbox permissions wrong -> `chmod 0600`
- TLS cert/key missing -> generates self-signed cert
- TLS cert expired -> regenerates cert
- Log directory missing -> `mkdir -p`
- App log directory missing -> `mkdir -p`

**`--lint` checks:**
- Bundle structure valid
- Views defined (warns about `[views.*]` vs `[api.views.*]`)
- Schema files exist
- Datasource references resolve

### riversctl tls renew

Regenerate the self-signed TLS certificate. Shows current cert info before renewal.

```bash
riversctl tls renew
```

Output:
```
Current cert: 365 days left
  Subject: CN=localhost
TLS certificate renewed:
  cert: /opt/rivers/config/tls/server.crt
  key:  /opt/rivers/config/tls/server.key
  expires: 2027-04-05
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

### riverpackage validate (v0.54.0 — 4-layer pipeline)

Runs the 4-layer bundle validation pipeline. This is the same pipeline `riversd` runs at startup, so a bundle that passes `riverpackage validate` will load cleanly on a correctly-configured server.

```sh
riverpackage validate ./address-book-bundle
riverpackage validate ./address-book-bundle --format json
riverpackage validate ./address-book-bundle --config /opt/rivers/config/riversd.toml
```

| Flag | Description |
|------|-------------|
| `--format <text\|json>` | Output format. Default `text`. |
| `--config <path>` | Path to `riversd.toml`. Required when you want the syntax layer to compile TS/JS handlers via the V8 engine located by the config. |

The pipeline:

1. **Structural** — TOML parse of bundle manifest, app manifests, `resources.toml`, `app.toml`
2. **Existence** — all referenced files (schemas, handler modules, libraries) exist on disk
3. **Cross-reference** — DataViews resolve to declared datasources, views resolve to DataViews, services resolve
4. **Syntax** — JSON schemas parse, TS/JS handler modules compile via embedded V8

Exit code is non-zero on any layer failure. JSON output includes per-layer results for scripting.

### riverpackage preflight

Legacy preflight checks (kept for backwards compatibility). New CI pipelines should use `validate` instead.

```sh
riverpackage preflight ./address-book-bundle
```

### riverpackage init

Scaffold a new Rivers application bundle.

```bash
riverpackage init my-app                    # Uses faker driver (default)
riverpackage init my-api --driver postgres  # PostgreSQL datasource
riverpackage init my-api --driver sqlite    # SQLite datasource
riverpackage init my-api --driver mysql     # MySQL datasource
```

Creates:
```
my-app/
├── manifest.toml
└── my-app/
    ├── manifest.toml
    ├── resources.toml
    ├── app.toml
    └── schemas/
        └── item.schema.json
```
