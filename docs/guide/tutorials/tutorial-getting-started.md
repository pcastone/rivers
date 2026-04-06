# Tutorial: Getting Started (First Deploy)

**Rivers v0.53.0**

## Overview

This tutorial walks through deploying Rivers from source to a running instance serving a live API. You will build the binaries, deploy them to an install prefix, scaffold a bundle, and verify everything works end-to-end.

The steps assume a fresh machine with no prior Rivers installation. By the end you will have a working Rivers instance serving a faker-backed REST API with TLS, per-app logging, and a clean shutdown path.

If you already have Rivers built and deployed, skip to Step 5 to scaffold a new bundle.

## Prerequisites

- Rust toolchain 1.77+ (install via [rustup](https://rustup.rs/), stable channel)
- C/C++ compiler (GCC 10+ or Clang 14+) -- required by `rusty_v8` and `aws_lc_rs`
- Python 3.8+ -- V8 build dependency
- 4 GB RAM minimum (V8 build can consume 2+ GB)
- Git

---

## Step 1: Build from Source

Clone the repository and build all binaries in release mode:

```bash
git clone https://github.com/acme/rivers.git
cd rivers
cargo build --release
```

This produces five binaries in `target/release/`:

| Binary | Purpose |
|--------|---------|
| `riversd` | Application server |
| `riversctl` | CLI management tool |
| `rivers-lockbox` | Secret management |
| `rivers-keystore` | Application keystore management |
| `riverpackage` | Bundle packaging and scaffolding |

A release build takes 5-10 minutes depending on hardware. The V8 engine is the largest compile-time dependency.

---

## Step 2: Install cargo-deploy

`cargo deploy` assembles a complete Rivers instance directory with all binaries, libraries, config, and directory structure:

```bash
cargo install --path crates/cargo-deploy
```

This installs the `cargo-deploy` subcommand into your Cargo bin directory.

---

## Step 3: Deploy to /opt/rivers

Run the deploy command targeting your install prefix:

```bash
cargo deploy /opt/rivers
```

This creates the full directory structure:

```
/opt/rivers/
├── bin/           riversd, riversctl, rivers-lockbox, rivers-keystore, riverpackage
├── lib/           librivers_engine_v8.dylib, librivers_engine_wasm.dylib
├── plugins/       12 driver plugin dylibs
├── config/
│   ├── riversd.toml    pre-configured with absolute paths
│   └── tls/            auto-generated self-signed cert
├── lockbox/       initialized with identity key
├── log/
│   └── apps/      per-app log files created at runtime
├── run/           PID file written on start
├── apphome/       place bundles here
├── data/
└── VERSION
```

All paths in `riversd.toml` are absolute -- binaries work from any directory.

For a single fat binary instead of shared libraries, use `cargo deploy /opt/rivers --static`.

---

## Step 4: Run Doctor

Before first start, run the doctor check to verify the installation:

```bash
/opt/rivers/bin/riversctl doctor --fix
```

Doctor checks and auto-repairs:

- `riversd` binary is locatable
- Config file parses as valid TOML
- TLS certificate exists (generates self-signed if missing)
- Log directories exist and are writable (creates if missing)
- App log directory exists (creates if missing)
- LockBox keystore permissions are correct
- Engine and plugin directories exist

Expected output:

```
Rivers Doctor
  [PASS] riversd binary found
  [PASS] config: /opt/rivers/config/riversd.toml
  [PASS] TLS certificate: /opt/rivers/config/tls/server.crt
  [PASS] log directory: /opt/rivers/log
  [PASS] app log directory: /opt/rivers/log/apps
  [PASS] lockbox: /opt/rivers/lockbox
  [PASS] engines directory: /opt/rivers/lib
  [PASS] plugins directory: /opt/rivers/plugins

8 checks passed, 0 failed
```

If any check shows `[FIXED]`, doctor repaired it automatically.

---

## Step 5: Scaffold a Bundle

Use `riverpackage init` to scaffold a new API bundle using the faker driver (no database required):

```bash
/opt/rivers/bin/riverpackage init my-api --driver faker
```

Output:

```
Bundle created: my-api/
  my-api/manifest.toml
  my-api/my-api/manifest.toml
  my-api/my-api/resources.toml
  my-api/my-api/app.toml
  my-api/my-api/schemas/item.schema.json
```

The scaffolded bundle structure:

```
my-api/
├── manifest.toml              # bundleName = "my-api"
└── my-api/
    ├── manifest.toml          # appName, appId (UUID), type = "service"
    ├── resources.toml          # faker datasource, nopassword = true
    ├── app.toml                # list_items DataView + /items View
    └── schemas/
        └── item.schema.json   # id, name, email with faker attributes
```

---

## Step 6: Copy Bundle to apphome and Edit Config

Move the scaffolded bundle into the apphome directory:

```bash
cp -r my-api /opt/rivers/apphome/
```

Edit `/opt/rivers/config/riversd.toml` to set the `bundle_path`. Uncomment and update the line near the top of the file:

```toml
# riversd.toml — Rivers server configuration

bundle_path = "/opt/rivers/apphome/my-api/"

[base]
host      = "0.0.0.0"
port      = 8080
log_level = "info"

[base.logging]
level           = "info"
format          = "json"
local_file_path = "/opt/rivers/log/riversd.log"
app_log_dir     = "/opt/rivers/log/apps"

[base.tls]
cert     = "/opt/rivers/config/tls/server.crt"
key      = "/opt/rivers/config/tls/server.key"
redirect = false

[base.tls.x509]
common_name = "localhost"
san         = ["localhost", "127.0.0.1"]
days        = 365

[storage_engine]
backend = "memory"

[lockbox]
path       = "/opt/rivers/lockbox"
key_source = "file"
key_file   = "/opt/rivers/lockbox/identity.key"

[engines]
dir = "/opt/rivers/lib"

[plugins]
dir = "/opt/rivers/plugins"
```

The key change is setting `bundle_path` to point at your bundle in `apphome/`.

---

## Step 7: Start Rivers

Start the server using `riversctl`:

```bash
/opt/rivers/bin/riversctl start
```

You should see startup logs confirming the bundle loaded:

```
INFO riversd starting version="0.53.0"
INFO config loaded path="/opt/rivers/config/riversd.toml"
INFO TLS certificate loaded cert="/opt/rivers/config/tls/server.crt"
INFO bundle loaded name="my-api" apps=1
INFO app loaded name="my-api" entry_point="my-api" datasources=1 views=1
INFO per-app logging enabled dir="/opt/rivers/log/apps"
INFO listening on https://0.0.0.0:8080
```

---

## Step 8: Verify Status

In a separate terminal, check that Rivers is running:

```bash
/opt/rivers/bin/riversctl status
```

Expected output:

```
rivers: riversd is running (pid 12345)
  config: /opt/rivers/config/riversd.toml
  port:   8080
  bundle: /opt/rivers/apphome/my-api/
```

---

## Step 9: Test the API

The scaffolded bundle creates a `/items` endpoint. Routes follow the pattern `/<bundleName>/<entryPoint>/<view_path>`. Since both the bundle name and entry point are `my-api`, the base path is `/my-api/my-api/`.

Use `-k` to accept the self-signed certificate:

```bash
curl -k https://localhost:8080/my-api/my-api/items
```

Expected response (envelope format with faker-generated data):

```json
{
  "data": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "name": "John Smith",
      "email": "john.smith@example.com"
    },
    {
      "id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
      "name": "Jane Doe",
      "email": "jane.doe@example.net"
    }
  ],
  "meta": {
    "limit": 20,
    "offset": 0
  }
}
```

Test with pagination:

```bash
curl -k "https://localhost:8080/my-api/my-api/items?limit=5"
```

---

## Step 10: Check Logs

Server logs go to the main log file:

```bash
cat /opt/rivers/log/riversd.log
```

Per-app logs (from handler `Rivers.log.*` calls) go to the app-specific log file:

```bash
cat /opt/rivers/log/apps/my-api.log
```

To watch logs in real time:

```bash
tail -f /opt/rivers/log/riversd.log
```

---

## Step 11: Stop Rivers

Stop the server gracefully:

```bash
/opt/rivers/bin/riversctl stop
```

This sends SIGTERM, waits for in-flight requests to complete (up to 30 seconds), then exits cleanly. New requests receive `503 Service Unavailable` during drain mode.

---

## Troubleshooting

### Port 8080 is in use

Edit `/opt/rivers/config/riversd.toml` and change the port:

```toml
[base]
port = 9080
```

Then restart: `/opt/rivers/bin/riversctl start`

### TLS certificate errors

Regenerate the self-signed certificate:

```bash
/opt/rivers/bin/riversctl tls renew
```

Or run doctor with `--fix`:

```bash
/opt/rivers/bin/riversctl doctor --fix
```

### Bundle not loading

Verify the bundle path is correct and the bundle validates:

```bash
/opt/rivers/bin/riverpackage validate /opt/rivers/apphome/my-api/
```

Common issues:
- `bundle_path` in `riversd.toml` must be an absolute path (or relative to the working directory)
- The bundle directory must contain a `manifest.toml` at the top level
- Each app directory must contain `manifest.toml`, `resources.toml`, and `app.toml`

### Permission denied on lockbox

LockBox enforces `0600` on keystore files:

```bash
chmod 0600 /opt/rivers/lockbox/identity.key
```

### Development mode (no TLS)

For local development without TLS:

```bash
/opt/rivers/bin/riversctl start --no-ssl --port 8080
```

Then use plain HTTP:

```bash
curl http://localhost:8080/my-api/my-api/items
```

---

## Summary

This tutorial covered:

1. Building Rivers from source with `cargo build --release`
2. Deploying a complete Rivers instance with `cargo deploy`
3. Running pre-launch health checks with `riversctl doctor --fix`
4. Scaffolding a new bundle with `riverpackage init`
5. Configuring `bundle_path` in `riversd.toml`
6. Starting, verifying, testing, and stopping the server
7. Checking server and per-app logs
8. Common troubleshooting steps
