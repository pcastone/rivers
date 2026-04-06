# Rivers Installation and Operations Guide

Version 0.1.0

---

## AW1.1 Prerequisites

Rivers is written in Rust and ships as four binaries. Build requirements:

| Requirement      | Minimum version | Notes                                    |
|------------------|-----------------|------------------------------------------|
| Rust toolchain   | 1.77+           | `rustup` recommended; edition 2021       |
| C/C++ compiler   | GCC 10+ / Clang 14+ | Required by `rusty_v8` and `aws_lc_rs` |
| Python 3         | 3.8+            | V8 build dependency                      |
| OpenSSL headers  | Not required    | Rivers uses `rustls` (pure Rust TLS)     |
| OS               | Linux (x86_64, aarch64), macOS (Apple Silicon, x86_64) | |
| RAM              | 4 GB minimum    | V8 build can consume 2+ GB               |

Runtime dependencies:

- None. All four binaries are statically linked.
- Optional: `age` CLI if you want to manage LockBox keystores outside of `rivers-lockbox`.

---

## AW1.2 Building from Source

The workspace contains 17 crates. A release build produces four binaries.

```bash
# Clone
git clone <repo-url> rivers && cd rivers

# Build all binaries (release)
cargo build --release

# Binaries are placed in target/release/
ls target/release/{riversd,riversctl,rivers-lockbox,riverpackage}
```

### Binaries

| Binary           | Purpose                                         |
|------------------|--------------------------------------------------|
| `riversd`        | Runtime daemon. Loads config, serves bundles.    |
| `riversctl`      | Control CLI. Launches riversd, runs doctor checks, issues admin API commands, manages TLS certs. |
| `rivers-lockbox` | Standalone secret management. Init, add, rotate, rekey encrypted keystores. |
| `riverpackage`   | Bundle validator and packager. Validates structure, runs preflight, creates archives. |

### Profile settings

Release builds use thin LTO with single codegen unit and stripped symbols (see workspace `Cargo.toml`):

```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = "symbols"
```

### Deploying with cargo deploy

`cargo deploy` builds and assembles a complete Rivers instance:

```bash
# Install the deploy tool (once)
cargo install --path crates/cargo-deploy

# Dynamic mode — thin binaries + shared engine/plugin libraries
cargo deploy /opt/rivers

# Static mode — single fat binary
cargo deploy /opt/rivers --static
```

Output structure:
```
/opt/rivers/
├── bin/           (riversd, riversctl, rivers-lockbox, rivers-keystore, riverpackage)
├── lib/           (librivers_engine_v8.dylib, librivers_engine_wasm.dylib)
├── plugins/       (12 driver plugin dylibs)
├── config/
│   ├── riversd.toml  (pre-configured with absolute paths)
│   └── tls/          (auto-generated self-signed cert)
├── lockbox/       (initialized with identity key)
├── log/
│   └── apps/      (per-app log files created at runtime)
├── run/           (PID file written on start)
├── apphome/       (place bundles here)
├── data/
└── VERSION
```

All paths in `riversd.toml` are absolute -- binaries work from any directory.

---

## AW1.3 Installation Layout

After building, copy the four binaries and create a working directory:

```
/opt/rivers/                    # or any install prefix
  bin/
    riversd
    riversctl
    rivers-lockbox
    riverpackage
  config/
    riversd.toml                # server configuration
  data/
    tls/                        # auto-generated certs (created at runtime)
  bundles/
    my-app-bundle/              # your application bundle(s)
  lockbox/                      # encrypted secrets (if using LockBox)
```

Config discovery order (when `--config` is not passed):

1. `./config/riversd.toml` -- same directory as the working directory
2. `../config/riversd.toml` -- parent directory (release layout: `bin/riversd` next to `config/`)

If neither is found, `riversd` starts with built-in defaults (port 8080, no bundle, memory storage engine).

---

## AW1.4 Starting the Server

### Direct launch

```bash
# Start with explicit config
riversd --config /opt/rivers/config/riversd.toml

# Start with auto-discovered config (from working directory)
cd /opt/rivers && riversd

# Start in plain HTTP mode (development only -- no TLS)
riversd --no-ssl --port 8080

# Override log level from CLI
riversd --config riversd.toml --log-level debug
```

### Via riversctl

`riversctl start` locates `riversd` and exec-replaces itself, so signals pass through cleanly:

```bash
riversctl start --config /opt/rivers/config/riversd.toml --log-level info

# Skip admin API auth (development only)
riversctl start --no-admin-auth

# Plain HTTP mode
riversctl start --no-ssl --port 8080
```

Binary discovery for `riversctl start`:

1. `RIVERS_DAEMON_PATH` environment variable (explicit path to `riversd`)
2. Sibling binary (same directory as `riversctl`)
3. `$PATH` lookup

### Pre-launch health check

```bash
riversctl doctor --config /opt/rivers/config/riversd.toml
riversctl doctor --config /opt/rivers/config/riversd.toml --fix
riversctl doctor --config /opt/rivers/config/riversd.toml --lint
riversctl doctor --config /opt/rivers/config/riversd.toml --fix --lint
```

Checks performed:

- `riversd` binary is locatable
- Config file found and parses as valid TOML
- Config passes validation rules
- Storage engine is available (in-memory always passes)
- LockBox keystore permissions (if configured)
- TLS certificate exists and is not expired
- Log directories exist and are writable
- Engine and plugin directories exist (dynamic mode)
- Bundle path is valid

**`--fix` auto-repairs:**
- Lockbox missing -> runs `rivers-lockbox init`
- Lockbox permissions wrong -> `chmod 0600`
- TLS cert/key missing -> generates self-signed cert
- TLS cert expired -> regenerates cert
- Log directory missing -> `mkdir -p`
- App log directory missing -> `mkdir -p`

**`--lint` validates bundle conventions:**
- Bundle structure valid
- Views defined (warns about `[views.*]` vs `[api.views.*]`)
- Schema files exist
- Datasource references resolve

### CLI flags reference

| Flag               | Short | Description                                            |
|--------------------|-------|--------------------------------------------------------|
| `--config <PATH>`  | `-c`  | Path to `riversd.toml`                                 |
| `--log-level <LVL>`| `-l`  | Log level: trace, debug, info, warn, error             |
| `--no-admin-auth`  |       | Disable Ed25519 auth on admin API (dev only)           |
| `--no-ssl`         |       | Run plain HTTP (no TLS). Debug/dev only.               |
| `--port <PORT>`    |       | Bind port for `--no-ssl` mode. Only valid with `--no-ssl`. |
| `--version`        | `-V`  | Print version and exit                                 |
| `--help`           | `-h`  | Print help and exit                                    |

---

## AW1.5 TLS Configuration

TLS is mandatory by default. If no cert/key paths are provided, `riversd` auto-generates a self-signed certificate on first start and stores it in the `data/tls/` directory.

### Auto-generated certificates

When `[base.tls]` is present but `cert` and `key` are omitted:

- A self-signed X.509 certificate is generated using the `[base.tls.x509]` parameters.
- Files are written to `<data_dir>/tls/` (default: `data/tls/`).
- Default SAN: `localhost`, `127.0.0.1`. Default validity: 365 days.
- Common name defaults to `localhost`.

### Providing your own certificates

```toml
[base.tls]
cert = "/etc/rivers/server.crt"      # PEM-encoded certificate
key  = "/etc/rivers/server.key"       # PEM-encoded private key
```

### HTTP-to-HTTPS redirect

When TLS is active, Rivers can redirect HTTP requests to HTTPS:

```toml
[base.tls]
redirect = true          # default: true
redirect_port = 80       # port to listen on for HTTP redirect (default: 80)
```

### TLS engine settings

```toml
[base.tls.engine]
min_version = "tls12"    # "tls12" (default) or "tls13"
ciphers = []             # empty = use rustls defaults (recommended)
```

### Managing certificates with riversctl

```bash
# Generate a self-signed cert
riversctl tls gen --port 8080

# Generate a Certificate Signing Request (CSR)
riversctl tls request --port 8080

# Import a signed cert/key pair
riversctl tls import /path/to/cert.pem /path/to/key.pem --port 8080

# Show certificate details (subject, SANs, expiry, fingerprint)
riversctl tls show --port 8080

# List all configured cert paths
riversctl tls list

# Force certificate re-generation on next start
riversctl tls expire --yes --port 8080
```

### Plain HTTP mode (development only)

For local development without TLS:

```bash
riversd --no-ssl --port 8080
```

This flag does **not** affect admin API TLS rules.

---

## AW1.6 LockBox -- Secrets Management

LockBox stores secrets in an Age-encrypted keystore. At runtime, `riversd` resolves `lockbox://` URIs in datasource configs to decrypted values. Secret values are **never** held in memory -- they are decrypted from disk per-access and zeroized after use.

### Initializing a keystore

```bash
rivers-lockbox init
# Creates:
#   lockbox/identity.key    (Age private key, mode 0600)
#   lockbox/entries/        (encrypted secret files)
#   lockbox/aliases.json    (alias mappings)
#
# Prints the Age public key.
```

The default directory is `./lockbox`. Override with `RIVERS_LOCKBOX_DIR`.

### Managing secrets

```bash
# Add a secret (reads from stdin)
rivers-lockbox add db/postgres-password

# Add a secret inline
rivers-lockbox add db/postgres-password --value "s3cret"

# List entries and aliases
rivers-lockbox list

# Decrypt and display a secret
rivers-lockbox show db/postgres-password

# Create an alias
rivers-lockbox alias pgpass db/postgres-password

# Rotate a secret value
rivers-lockbox rotate db/postgres-password --value "new-s3cret"

# Remove a secret (also removes aliases pointing to it)
rivers-lockbox remove db/postgres-password

# Re-encrypt all secrets with a new Age identity
rivers-lockbox rekey

# Validate keystore integrity
rivers-lockbox validate
```

### Referencing secrets in config

In `resources.toml` datasource definitions, use `lockbox://` URIs:

```toml
[[datasources]]
name = "primary-db"
driver = "postgres"
url = "postgres://user@db-host:5432/myapp"
credentials_source = "lockbox://db/postgres-password"
```

### Key source configuration

The `[lockbox]` section in `riversd.toml` tells `riversd` how to decrypt the keystore:

```toml
[lockbox]
path = "/opt/rivers/lockbox/secrets.rkeystore"  # absolute path required

# Option 1: Environment variable (default)
key_source = "env"
key_env_var = "RIVERS_LOCKBOX_KEY"              # default env var name

# Option 2: Identity file
key_source = "file"
key_file = "/opt/rivers/lockbox/identity.key"   # must be mode 0600

# Option 3: SSH agent (not yet implemented)
key_source = "agent"
```

### File permission requirements

LockBox enforces `0600` (owner read+write only) on:

- The `.rkeystore` file
- The `key_file` (when `key_source = "file"`)

Violation causes a startup error with a descriptive message.

---

## AW1.7 Storage Engine

The storage engine provides internal key-value and queue infrastructure for DataView caching and message buffering.

| Backend    | Config                             | Use case                      |
|------------|------------------------------------|-------------------------------|
| `memory`   | `backend = "memory"` (default)     | Development, single-instance  |
| `sqlite`   | `backend = "sqlite"`, `path = "..."` | Persistent single-node      |
| `redis`    | `backend = "redis"`, `url = "redis://..."` | Shared, multi-instance  |

```toml
[storage_engine]
backend = "sqlite"
path = "/opt/rivers/data/storage.db"
retention_ms = 86400000         # event retention: 24h (default)
max_events = 100000             # max stored events (default)
sweep_interval_s = 60           # cleanup interval (default)

# Cache policy for datasources
[storage_engine.cache.datasources.primary-db]
enabled = true
ttl_seconds = 120               # default cache TTL
invalidation_strategy = "dataview"  # "dataview" (default) or "datasource"

# Per-DataView TTL override
[storage_engine.cache.dataviews.contacts-list]
ttl_seconds = 30
```

---

## AW1.8 Admin API

The admin API runs on a separate socket, disabled by default. It provides operational endpoints for status, deployment, log management, driver inspection, and health checks.

```toml
[base.admin_api]
enabled = true
host = "127.0.0.1"             # loopback only (default)
port = 9090                     # must be set when enabled
```

### Authentication

Admin API requests are authenticated via Ed25519 signatures. Each request includes:

- `X-Rivers-Timestamp` header (epoch milliseconds)
- `X-Rivers-Signature` header (Ed25519 signature over `method\npath\ntimestamp\nbody_sha256`)

Configure the keypair:

```toml
[base.admin_api]
enabled = true
port = 9090
public_key = "/opt/rivers/admin-pub.key"
private_key = "/opt/rivers/admin-priv.key"
```

For development, authentication can be disabled:

```bash
riversd --no-admin-auth
```

Or in config:

```toml
[base.admin_api]
no_auth = true                  # NEVER use in production
```

### Admin API TLS

The admin API can have its own TLS configuration, including mutual TLS:

```toml
[base.admin_api.tls]
server_cert = "/opt/rivers/admin-cert.pem"
server_key = "/opt/rivers/admin-key.pem"
ca_cert = "/opt/rivers/ca.pem"
require_client_cert = true      # mutual TLS (default: false)
```

### RBAC

Role-based access control for admin API endpoints:

```toml
[base.admin_api.rbac]

[base.admin_api.rbac.roles]
viewer = ["/admin/status", "/admin/health"]
operator = ["/admin/status", "/admin/health", "/admin/deploy", "/admin/log/*"]

[base.admin_api.rbac.bindings]
"key-fingerprint-abc123" = "operator"
"key-fingerprint-def456" = "viewer"
```

### Admin API commands (via riversctl)

Set the `RIVERS_ADMIN_URL` and `RIVERS_ADMIN_KEY` environment variables:

```bash
export RIVERS_ADMIN_URL="http://127.0.0.1:9090"
export RIVERS_ADMIN_KEY="/opt/rivers/admin-priv.key"

riversctl status                  # server status
riversctl deploy /path/to/bundle  # deploy a bundle
riversctl drivers                 # list registered drivers
riversctl datasources             # list configured datasources
riversctl health                  # verbose health check

riversctl log levels              # view current log levels
riversctl log set <event> <level> # change a log level
riversctl log reset               # reset log levels to defaults
```

---

## AW1.9 Hot Reload

When `riversd` is started with `--config <path>`, it watches the config file for changes and hot-reloads safe fields without server restart.

### What reloads without restart

- Views (route definitions)
- DataViews (queries, parameters, caching)
- Security config (CORS, rate limits, session settings)
- GraphQL schema settings
- Logging level

### What requires a restart

- `base.host` -- bind address
- `base.port` -- listen port
- `base.tls.cert` / `base.tls.key` -- TLS certificate and key paths
- LockBox configuration

When a restart-required field changes, Rivers logs a warning and skips the reload:

```
WARN config change requires restart, skipping hot reload reason="base.port changed"
```

### Behavior during reload

- In-flight requests complete against the old config snapshot (Arc-based isolation).
- The config swap is atomic (RwLock-guarded).
- File change events are debounced at 500ms.

---

## AW1.10 Graceful Shutdown

Rivers handles `SIGTERM` and `SIGINT` for graceful shutdown:

1. Server enters **drain mode** -- new requests receive `503 Service Unavailable`.
2. In-flight requests continue processing to completion.
3. When all in-flight requests finish, the server exits cleanly.

No configuration is required. This behavior is always active.

---

## AW1.11 Operational Reference

### Logging

```toml
[base.logging]
level = "info"                  # trace | debug | info (default) | warn | error
format = "json"                 # "json" (default) or "text"
local_file_path = "/var/log/rivers/riversd.log"  # optional; stdout always active
```

The CLI flag `--log-level` overrides `base.logging.level`. Log levels can also be changed at runtime via the admin API (`riversctl log set`).

When `local_file_path` is set, logs are written to both stdout and the file using a non-blocking appender.

#### Per-Application Logging

When `app_log_dir` is configured, each loaded app gets its own log file:

```toml
[base.logging]
level           = "info"
format          = "json"
local_file_path = "/opt/rivers/log/riversd.log"
app_log_dir     = "/opt/rivers/log/apps"
```

Result:
```
log/
├── riversd.log        <- server logs (startup, config, driver loading)
└── apps/
    ├── my-api.log     <- Rivers.log.info/warn/error from my-api handlers
    └── admin.log      <- Rivers.log from admin handlers
```

App log files rotate automatically at 10MB (`<app>.log.1`).

#### Prometheus Metrics

Enable the built-in Prometheus metrics exporter:

```toml
[metrics]
enabled = true
port = 9091       # default
```

Scrape endpoint: `http://localhost:9091/metrics`

Available metrics:
- `rivers_http_requests_total` -- counter by method and status
- `rivers_http_request_duration_ms` -- histogram by method
- `rivers_engine_executions_total` -- counter by engine and success
- `rivers_engine_execution_duration_ms` -- histogram by engine
- `rivers_active_connections` -- gauge
- `rivers_loaded_apps` -- gauge

### Backpressure

Request queuing under load is enabled by default:

```toml
[base.backpressure]
enabled = true                  # default: true
queue_depth = 512               # max queued requests (default: 512)
queue_timeout_ms = 100          # max time a request waits in queue (default: 100ms)
```

When the queue is full, new requests are rejected immediately.

### Security

```toml
[security]
# CORS
cors_enabled = false            # default: false
cors_allowed_origins = ["https://app.example.com"]
cors_allowed_methods = ["GET", "POST", "PUT", "DELETE"]
cors_allowed_headers = ["Content-Type", "Authorization"]
cors_allow_credentials = false

# Rate limiting
rate_limit_per_minute = 120     # default: 120 (0 = disabled)
rate_limit_burst_size = 60      # default: 60
rate_limit_strategy = "ip"      # "ip" (default), "header", "session"
rate_limit_custom_header = "X-Forwarded-For"  # when strategy = "header"

# IP allowlist for admin endpoints
admin_ip_allowlist = ["10.0.0.0/8", "172.16.0.0/12"]
```

### CSRF protection

```toml
[security.csrf]
enabled = true                         # default: true
csrf_rotation_interval_s = 300         # minimum seconds between token rotations
cookie_name = "rivers_csrf"            # default
header_name = "X-CSRF-Token"           # default
```

### Sessions

```toml
[security.session]
enabled = false                 # default: false
ttl_s = 3600                    # absolute session lifetime (default: 1h)
idle_timeout_s = 1800           # inactivity timeout (default: 30min)
include_token_in_body = false   # include token in JSON response (for SPAs)
token_body_key = "token"        # JSON key name (default: "token")

[security.session.cookie]
name = "rivers_session"
http_only = true                # enforced; setting false is a validation error
secure = true                   # HTTPS-only (false emits warning)
same_site = "Lax"               # "Strict" | "Lax" | "None"
path = "/"
# domain =                      # omit for current domain only
```

### GraphQL

```toml
[graphql]
enabled = false                 # default: false
path = "/graphql"               # default
introspection = true            # default: true
max_depth = 10                  # default: 10
max_complexity = 1000           # default: 1000
```

### ProcessPool (CodeComponent runtime)

```toml
[runtime.process_pools.default]
engine = "v8"                   # "v8" (default) or "wasmtime"
workers = 4                     # worker threads (default: 4)
max_heap_mb = 128               # V8 heap limit per isolate (default: 128)
task_timeout_ms = 5000          # wall-clock timeout per task (default: 5000)
max_queue_depth = 0             # 0 = workers * 4
epoch_interval_ms = 10          # WASM preemption tick (default: 10)
heap_recycle_threshold = 0.8    # recycle V8 isolate above this usage (default: 0.8)
# recycle_after_tasks =         # recycle after N tasks (0 = never)
```

### HTTP/2

```toml
[base.http2]
enabled = false                 # default: false
initial_window_size = 65535     # optional; HTTP/2 flow control window
max_concurrent_streams = 100    # optional; max concurrent streams per connection
```

### Static files

```toml
[static_files]
enabled = false                 # default: false
root_path = "/opt/rivers/public"
index_file = "index.html"       # default
spa_fallback = false            # default: false (serve index.html for unknown routes)
max_age = 3600                  # Cache-Control max-age in seconds
exclude_paths = ["/api", "/health"]
```

### Environment overrides

Override specific config fields per environment without duplicating the entire file:

```toml
[environment_overrides.production]
[environment_overrides.production.base]
host = "0.0.0.0"
port = 443
workers = 16
log_level = "warn"

[environment_overrides.production.security]
cors_enabled = true
cors_allowed_origins = ["https://app.example.com"]
rate_limit_per_minute = 300

[environment_overrides.production.storage_engine]
backend = "redis"
url = "redis://redis-cluster:6379"
pool_size = 20

[environment_overrides.staging]
[environment_overrides.staging.base]
port = 8443
log_level = "debug"
```

---

## AW1.12 Complete Annotated `riversd.toml`

```toml
# ============================================================================
# riversd.toml -- Rivers runtime configuration
# ============================================================================

# Path to the application bundle to load at startup.
# Resolved relative to the working directory. If omitted, no bundle is loaded
# and you must deploy via `riversctl deploy <path>` at runtime.
bundle_path = "bundles/my-app-bundle"

# Data directory for auto-generated TLS certs and other runtime files.
# Default: "data"
data_dir = "data"

# Application ID for this instance (used in auto-gen cert filenames).
# Default: "default"
# app_id = "my-service"

# Optional route prefix prepended to all bundle routes.
# e.g. route_prefix = "v1" results in /<v1>/<app>/<view>
# route_prefix = "v1"

# ── [base] -- Core server settings ─────────────────────────────────────────

[base]
host = "0.0.0.0"                 # Bind address (default: "0.0.0.0")
port = 8080                      # Listen port (default: 8080)
# workers =                      # Tokio worker threads (default: auto = CPU count)
request_timeout_seconds = 30     # Per-request timeout (default: 30)

# ── [base.logging] -- Log output ───────────────────────────────────────────

[base.logging]
level = "info"                   # trace | debug | info | warn | error
format = "json"                  # "json" (default) or "text"
# local_file_path = "/var/log/rivers/riversd.log"  # Dual output: stdout + file

# ── [base.backpressure] -- Request queuing under load ──────────────────────

[base.backpressure]
enabled = true                   # Enable backpressure (default: true)
queue_depth = 512                # Max queued requests (default: 512)
queue_timeout_ms = 100           # Queue wait timeout in ms (default: 100)

# ── [base.tls] -- TLS configuration (mandatory in production) ─────────────
#
# If cert/key are omitted, a self-signed cert is auto-generated using the
# x509 settings below and stored in <data_dir>/tls/.

[base.tls]
# cert = "/etc/rivers/server.crt"   # PEM cert path (optional; auto-gen if absent)
# key  = "/etc/rivers/server.key"   # PEM key path  (optional; auto-gen if absent)
redirect = true                     # Redirect HTTP to HTTPS (default: true)
redirect_port = 80                  # HTTP redirect listener port (default: 80)

[base.tls.x509]
common_name = "localhost"        # CN for auto-gen cert (default: "localhost")
san = ["localhost", "127.0.0.1"] # Subject Alternative Names
days = 365                       # Cert validity in days (default: 365)
# organization = "My Corp"
# country = "US"
# state = "California"
# locality = "San Francisco"

[base.tls.engine]
min_version = "tls12"            # "tls12" (default) or "tls13"
ciphers = []                     # Empty = rustls defaults (recommended)

# ── [base.http2] -- HTTP/2 protocol settings ──────────────────────────────

[base.http2]
enabled = false                  # Default: false
# initial_window_size = 65535
# max_concurrent_streams = 100

# ── [base.admin_api] -- Operational admin server ──────────────────────────

[base.admin_api]
enabled = false                  # Default: false
host = "127.0.0.1"              # Loopback only (default)
# port = 9090                   # Required when enabled
# public_key = "/opt/rivers/admin-pub.key"
# private_key = "/opt/rivers/admin-priv.key"
# no_auth = false               # NEVER true in production

# [base.admin_api.tls]
# server_cert = "/opt/rivers/admin-cert.pem"
# server_key = "/opt/rivers/admin-key.pem"
# ca_cert = "/opt/rivers/ca.pem"
# require_client_cert = false

# ── [base.cluster] -- Clustering ──────────────────────────────────────────

[base.cluster.session_store]
enabled = false                  # Default: false
cookie_name = "rivers_session"

# ── [security] -- CORS, rate limiting, sessions ──────────────────────────

[security]
cors_enabled = false
# cors_allowed_origins = ["https://app.example.com"]
# cors_allowed_methods = ["GET", "POST"]
# cors_allowed_headers = ["Content-Type", "Authorization"]
# cors_allow_credentials = false

rate_limit_per_minute = 120      # Default: 120 (0 = disabled)
rate_limit_burst_size = 60       # Default: 60
rate_limit_strategy = "ip"       # "ip" | "header" | "session"
# rate_limit_custom_header = "X-Forwarded-For"

# admin_ip_allowlist = ["10.0.0.0/8"]

[security.csrf]
enabled = true                   # Default: true
csrf_rotation_interval_s = 300
cookie_name = "rivers_csrf"
header_name = "X-CSRF-Token"

[security.session]
enabled = false
ttl_s = 3600                     # Absolute lifetime (default: 1h)
idle_timeout_s = 1800            # Inactivity timeout (default: 30min)
# include_token_in_body = false
# token_body_key = "token"

[security.session.cookie]
name = "rivers_session"
http_only = true                 # Enforced; false is a validation error
secure = true                    # Set false only for local dev (emits warning)
same_site = "Lax"                # "Strict" | "Lax" | "None"
path = "/"
# domain = "example.com"

# ── [static_files] -- Static file serving ────────────────────────────────

[static_files]
enabled = false
# root_path = "/opt/rivers/public"
# index_file = "index.html"
# spa_fallback = false
# max_age = 3600
# exclude_paths = ["/api"]

# ── [storage_engine] -- Internal KV + queue backend ─────────────────────

[storage_engine]
backend = "memory"               # "memory" (default) | "sqlite" | "redis"
# path = "/opt/rivers/data/storage.db"  # sqlite backend
# url = "redis://localhost:6379"         # redis backend
# credentials_source = "lockbox://storage-redis-password"
# key_prefix = "rivers:"
# pool_size = 10
retention_ms = 86400000          # 24h event retention (default)
max_events = 100000              # Max stored events (default)
sweep_interval_s = 60            # Cleanup interval (default)

# [storage_engine.cache.datasources.my-datasource]
# enabled = true
# ttl_seconds = 120
# invalidation_strategy = "dataview"

# [storage_engine.cache.dataviews.my-dataview]
# ttl_seconds = 30

# ── [lockbox] -- Age-encrypted secret keystore ──────────────────────────

# [lockbox]
# path = "/opt/rivers/lockbox/secrets.rkeystore"
# key_source = "env"                    # "env" | "file" | "agent"
# key_env_var = "RIVERS_LOCKBOX_KEY"    # default env var
# key_file = "/opt/rivers/lockbox/identity.key"

# ── [graphql] -- GraphQL endpoint ────────────────────────────────────────

[graphql]
enabled = false
path = "/graphql"
introspection = true
max_depth = 10
max_complexity = 1000

# ── [runtime] -- ProcessPool configuration ───────────────────────────────

# [runtime.process_pools.default]
# engine = "v8"                  # "v8" | "wasmtime"
# workers = 4
# max_heap_mb = 128              # V8 isolate heap limit
# task_timeout_ms = 5000         # Wall-clock timeout per task
# max_queue_depth = 0            # 0 = workers * 4
# epoch_interval_ms = 10         # WASM preemption tick
# heap_recycle_threshold = 0.8   # Recycle isolate above this heap usage
# recycle_after_tasks = 0        # Recycle after N tasks (0 = never)

# ── [environment_overrides] -- Per-environment overrides ─────────────────
#
# Fields here selectively override the main config for a given environment.
# Only the fields you specify are overridden; everything else inherits.

# [environment_overrides.production.base]
# host = "0.0.0.0"
# port = 443
# workers = 16
# log_level = "warn"

# [environment_overrides.production.security]
# cors_enabled = true
# cors_allowed_origins = ["https://app.example.com"]
# rate_limit_per_minute = 300

# [environment_overrides.production.storage_engine]
# backend = "redis"
# url = "redis://redis-cluster:6379"
# pool_size = 20
```

### Bundle validation

Before deploying a bundle, validate it:

```bash
# Structure and TOML/JSON syntax check
riverpackage validate bundles/my-app-bundle

# Full preflight: validate + check schema/parameter orphans
riverpackage preflight bundles/my-app-bundle

# Package into a tar.gz archive
riverpackage pack bundles/my-app-bundle output.tar.gz
```
