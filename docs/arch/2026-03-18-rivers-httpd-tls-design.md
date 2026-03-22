# Rivers HTTPD TLS Design

**Date:** 2026-03-18
**Status:** Approved
**Scope:** TLS configuration, auto-gen certs, HTTP redirect server, admin server TLS, riversctl tls commands
**Shaping decisions:** SHAPE-21, SHAPE-22, SHAPE-25 (admin TLS mandatory)

> **SUPERSEDES** the following sections of `rivers-httpd-spec.md` on any conflict:
> - §5.5 (redirect server — adds configurable `redirect_port`)
> - §5.6 (admin server TLS — mandatory TLS, auto-gen, updated `AdminTlsConfig` struct)
> - §15.2 (admin localhost enforcement — plain-HTTP exception removed)
>
> These sections must be updated in the baseline spec as part of implementation task 18.5.

---

## 1. Core Architecture

TLS is mandatory on both the main server and the admin server. There is no plain HTTP path for either server under any configuration — with one debug escape hatch (see §1.1).

- `[base.tls]` table absent at startup → hard error
- `[base.admin_api.tls]` table absent at startup → hard error
- `[base.admin_api.tls]` table present but `server_cert`/`server_key` omitted → auto-gen self-signed cert
- Admin server localhost plain-HTTP exception is **removed**
- The only condition allowing non-localhost admin binding is: Ed25519 `public_key` must be configured

### 1.1 `--no-ssl` Escape Hatch

```
riversctl start --no-ssl                  # binds main server on redirect_port (default: 80)
riversctl start --no-ssl --port <port>    # binds main server on <port>
```

Bypasses TLS validation and starts the main server in plain HTTP mode for the lifetime of the process only. Session-scoped — does not persist across restarts.

**Port behavior:**
- Main server binds on `redirect_port` (default: 80) unless `--port` overrides it
- The HTTP redirect server is **not spawned** (there is no HTTPS endpoint to redirect to)
- `[base.tls]` validation is skipped entirely — no cert required

**Does not affect the admin server** — admin TLS and Ed25519 auth rules are unchanged.

Emits at startup:
```
WARN: --no-ssl: TLS is DISABLED for this session — do not use in production
```

Intended for local debugging only (e.g., inspecting traffic with a proxy that cannot handle TLS, or when certificates are unavailable in a dev environment). Never expose a `--no-ssl` process on a public network interface.

---

## 2. TLS Config Structure

### Main server

```toml
[base.tls]
cert          = "/etc/rivers/tls/server.crt"   # omit → auto-gen self-signed
key           = "/etc/rivers/tls/server.key"   # omit → auto-gen self-signed
# redirect = false                              # disable HTTP → HTTPS redirect
# redirect_port = 80                           # default: 80, configurable

[base.tls.x509]
common_name  = "localhost"
organization = "My Org"
country      = "US"
state        = "California"
locality     = "San Francisco"
san          = ["localhost", "127.0.0.1"]
days         = 365

[base.tls.engine]
min_version = "tls12"    # tls12 | tls13  (default: tls12)
ciphers     = []         # empty = rustls secure defaults (recommended)
```

### Admin server

```toml
[base.admin_api.tls]
server_cert          = "/etc/rivers/admin/server.crt"   # omit → auto-gen self-signed
server_key           = "/etc/rivers/admin/server.key"   # omit → auto-gen self-signed
ca_cert              = "/etc/rivers/admin/ca.crt"       # required when require_client_cert = true
require_client_cert  = false                            # default: false; set true to enforce mTLS
```

**Updated `AdminTlsConfig` struct** (all cert fields become optional to support auto-gen):

```rust
pub struct AdminTlsConfig {
    pub server_cert: Option<String>,      // None → auto-gen
    pub server_key: Option<String>,       // None → auto-gen
    pub ca_cert: Option<String>,          // required when require_client_cert = true
    pub require_client_cert: bool,        // default: false
}
```

### Cipher/version notes

- `ciphers = []` uses rustls secure defaults — recommended for all deployments
- Explicit cipher configuration is for compliance requirements (FIPS, PCI-DSS) only
- TLS 1.3: MAC is embedded in AEAD cipher — no separate MAC config
- TLS 1.2: MAC is part of the cipher suite name (e.g., `SHA384`)
- Min version defaults to `tls12` for broad client compatibility

---

## 3. Auto-gen Self-Signed Certificate

### Main server trigger

`cert` and `key` both absent from `[base.tls]` → auto-gen.

```
cert + key absent
    → generate from [base.tls.x509] fields (via rcgen)
    → write to {data_dir}/tls/auto-{appId}.crt and .key
    → log WARN: "TLS: using auto-generated self-signed cert — not for production"
    → on next startup: validate existing files (see reuse policy below)
    → reuse if valid; re-generate if invalid
```

### Admin server trigger

`[base.admin_api.tls]` table present but `server_cert`/`server_key` absent → auto-gen.
`[base.admin_api.tls]` table absent entirely → hard startup error.

Admin auto-gen uses fixed defaults (no `[base.admin_api.tls.x509]` config block):
```
CN=rivers-admin-{appId}
SAN=localhost, 127.0.0.1
days=365
```
Files written to `{data_dir}/tls/auto-admin-{appId}.crt` and `.key`.

### Reuse policy (both servers)

On restart, if auto-gen file already exists:
1. Parse the cert file — if parsing fails: log WARN, re-generate
2. Verify cert/key pair match (public key in cert == public key derived from private key) — if mismatch: log WARN, re-generate
3. Pair is valid → reuse, log INFO: "TLS: reusing existing auto-generated cert"

Auto-gen is for development only. Production deployments use `riversctl tls import`.

---

## 4. HTTP Redirect Server

Rivers spawns a redirect listener on `redirect_port` (default: 80) that issues `301 Moved Permanently` to the HTTPS main server.

**Config:**

```toml
[base.tls]
redirect_port = 80      # default, configurable
# redirect = false      # set to disable entirely
```

**Startup validation:**

- `redirect_port == base.port` → hard startup error ("redirect_port cannot equal base.port")
- `redirect = false` → redirect server not spawned (no bind attempt)
- Port bind failure (e.g., permission denied) → log WARN, continue; main HTTPS server still starts

**Behavior:**

- `Host` header and query string preserved in `301 Location` URL
- No middleware on the redirect server (no rate limiting, no session, no trace ID)
- Internal services must set `redirect = false` — they must not compete for the redirect port
- Shares shutdown signal with the main server

---

## 5. Admin Server Access Control

All admin access requires TLS. Ed25519 public_key is required when binding to any address.

```
admin_api.host = "0.0.0.0"  + no public_key  → validation error at startup
admin_api.host = "127.0.0.1" + no public_key  → validation error at startup
admin_api.host = "0.0.0.0"  + public_key set  → allowed (TLS + Ed25519)
admin_api.host = "127.0.0.1" + public_key set  → allowed (TLS + Ed25519)
```

`require_client_cert = false` is the default. Set `true` to enforce mTLS (client must present a cert signed by `ca_cert`). When `require_client_cert = true`, `ca_cert` must be set — validation rejects the combination of `require_client_cert = true` with `ca_cert` absent.

---

## 6. riversctl tls Commands

### Port-based targeting

`--port <port>` identifies which server's certs to operate on. Default (no flag) targets the main server.

**Lookup algorithm:**
- `--port` matches `base.port` → use `[base.tls]`; field names are `cert`/`key`
- `--port` matches `base.admin_api.port` → use `[base.admin_api.tls]`; field names are `server_cert`/`server_key`
- `--port` matches neither → error: "no server configured on port {P}"
- No `--port` → default to `base.port` (main server)

### Command table

| Command | `--port` | Action |
|---|---|---|
| `riversctl tls gen [--port P]` | optional | Generate self-signed cert from x509 fields, write to configured paths |
| `riversctl tls request [--port P]` | optional | Generate CSR from x509 fields, print to stdout for CA submission |
| `riversctl tls import <cert> <key> [--port P]` | optional | Validate and copy CA-signed cert + key into configured paths |
| `riversctl tls show [--port P]` | optional | Display cert details (see output format below) |
| `riversctl tls list` | n/a | List all cert files managed by Rivers (see enumeration scope below) |
| `riversctl tls expire [--port P] --yes` | optional | Purge cert files from disk |

### `tls show` output format

```
Subject:     CN=localhost, O=My Org, C=US
Issuer:      CN=localhost (self-signed)
SANs:        localhost, 127.0.0.1
Valid:       2026-03-18 → 2027-03-18
Expires:     365 days left
Fingerprint: SHA256:4A:3B:...
```

Expiry display rules (computed in UTC):
- ≥ 48h remaining → display in days (`365 days left`)
- < 48h remaining → display in hours (`23 hours left`)
- Past expiry → `EXPIRED 5 days ago`

### `tls list` enumeration scope

Reads `[base.tls]` and `[base.admin_api.tls]` for all explicitly configured cert paths, and scans `{data_dir}/tls/` for auto-gen files matching the `auto-{appId}.*` and `auto-admin-{appId}.*` naming patterns. Lists each file with its path and expiry date.

### `tls expire` behavior

Requires explicit `--yes` flag (no interactive/scripted distinction):

```
riversctl tls expire --yes            # expire main server cert
riversctl tls expire --yes --port 9443 # expire admin server cert
```

- Removes cert and key files at the configured paths
- Running server is not affected — TLS changes require restart
- Next startup: re-generates (auto-gen mode) or hard errors (explicit paths set but files missing)
- This applies to both main and admin servers: expiring an admin cert at explicit `server_cert`/`server_key` paths results in a hard startup error on next restart, not auto-gen fallback

---

## 7. Shaping Impact Summary

| Decision | Spec Section | Status |
|---|---|---|
| SHAPE-21: TLS mandatory, new `[base.tls]` structure | §5.1–5.4, §2 startup | Done in spec |
| SHAPE-22: HTTP redirect server + configurable `redirect_port` | §5.5 | Needs `redirect_port` field added |
| SHAPE-25: Admin server TLS mandatory (no plain HTTP exception) | §5.6, §15.2 | Needs update |
| SHAPE-25: Admin auto-gen certs + updated `AdminTlsConfig` struct | §5.6 | Needs update |
| SHAPE-25: `riversctl tls --port` targeting + `--yes` flag | §20 | Needs update |

---

## 8. Implementation Tasks

| ID | Description | Phase |
|---|---|---|
| 18.4 | TLS via `rustls` + `tokio-rustls` — `TlsAcceptor` for main server | Phase C |
| 18.NEW | TLS cert auto-gen via `rcgen` — main + admin, with reuse validation | Phase C |
| 18.5 | `TlsConfig`, `TlsX509Config`, `TlsEngineConfig`, updated `AdminTlsConfig` in `config.rs` | Phase D |
| 18.6 | `maybe_autogen_tls_cert` — main server auto-gen + reuse validation | Phase D |
| 18.6b | `maybe_autogen_admin_tls_cert` — admin server auto-gen + reuse validation | Phase D |
| 18.7 | `maybe_spawn_http_redirect_server` — configurable `redirect_port`, startup validation | Phase D |
| 31.2 | Admin access control update — TLS required, Ed25519 required unconditionally | Phase B |
| 36.6 | `riversctl tls` subcommands — `--port` targeting, `--yes` flag for expire | Phase D |
| 36.7 | `riversctl start --no-ssl` flag — bypass TLS validation, WARN log, session-scoped only | Phase D |
| AB.1 | Address-book bundle TLS config update | Phase D |
