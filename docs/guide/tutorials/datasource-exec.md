# Tutorial: ExecDriver Datasource

**Rivers v0.52.5**

## Overview

The ExecDriver is a plugin that exposes admin-declared scripts and binaries through the standard datasource query pattern. Handlers call commands by name -- they do not know a script is being executed, what language it is written in, or what OS user runs it. Those are all admin-side concerns configured in TOML.

Use the ExecDriver when:
- You need to run network scanning tools (nmap, custom probes) from handler code
- You need to execute DNS lookups, certificate checks, or LDAP queries via existing scripts
- You have operational scripts in Python, Bash, or Go that should be accessible from an API
- You want controlled execution of host-level tools without embedding them in the framework

The ExecDriver is a **controlled RCE service**. Every command is admin-declared, pinned by SHA-256 hash, validated against JSON Schema, and executed as a restricted OS user. Handlers cannot specify arbitrary paths or inject commands.

---

## Prerequisites

- A dedicated OS user for script execution (e.g., `rivers-exec`)
- Scripts or binaries installed at known absolute paths
- SHA-256 hashes computed for each script
- The `rivers-exec` plugin enabled in your Rivers deployment

---

## Step 1: Set Up the Execution Environment

Create a dedicated OS user and directory structure for script execution.

```bash
# Create a restricted user (no login shell)
sudo useradd -r -s /usr/sbin/nologin rivers-exec

# Create the script directory (root-owned, read+execute only)
sudo mkdir -p /usr/lib/rivers/scripts
sudo chmod 0555 /usr/lib/rivers/scripts

# Create the working directory (owned by rivers-exec)
sudo mkdir -p /var/rivers/exec-scratch
sudo chown rivers-exec:rivers-exec /var/rivers/exec-scratch
sudo chmod 0700 /var/rivers/exec-scratch
```

The `run_as_user` must not be root. The driver rejects UID 0 at startup.

---

## Step 2: Create a Script

Scripts follow a simple I/O contract: read JSON from stdin (or parse argv), do work, write JSON to stdout. Errors go to stderr with a non-zero exit code.

Create a network scan script at `/usr/lib/rivers/scripts/netscan.sh`:

```bash
#!/bin/bash
# netscan.sh — reads CIDR and ports from stdin, returns scan results as JSON
set -euo pipefail

INPUT=$(cat)
CIDR=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['cidr'])")
PORTS=$(echo "$INPUT" | python3 -c "import sys,json; print(','.join(str(p) for p in json.load(sys.stdin)['ports']))")

# Run the scan (simplified — replace with your actual scan logic)
RESULTS=$(nmap -sT -p "$PORTS" "$CIDR" -oX - 2>/dev/null | python3 -c "
import sys, xml.etree.ElementTree as ET, json
tree = ET.parse(sys.stdin)
hosts = []
for host in tree.findall('.//host'):
    addr = host.find('address').get('addr', '')
    ports = []
    for port in host.findall('.//port'):
        if port.find('state').get('state') == 'open':
            ports.append(int(port.get('portid')))
    if ports:
        hosts.append({'ip': addr, 'ports': ports})
print(json.dumps({'hosts': hosts, 'scanned': True}))
")

echo "$RESULTS"
```

Make it executable:

```bash
sudo cp netscan.sh /usr/lib/rivers/scripts/netscan.sh
sudo chmod 0555 /usr/lib/rivers/scripts/netscan.sh
```

### Script Contract

| Requirement | Description |
|-------------|-------------|
| **Input** | Read JSON from stdin (`stdin` mode) and/or parse argv (`args` mode) |
| **Output** | Write a single JSON document to stdout on success |
| **Errors** | Write diagnostics to stderr, exit with non-zero code |
| **No interactivity** | Never read from TTY or prompt for input |

---

## Step 3: Compute the SHA-256 Hash

The SHA-256 hash is the authorization mechanism. It pins exactly which bytes at which path are approved for execution.

```bash
riversctl exec hash /usr/lib/rivers/scripts/netscan.sh
# Output:
# sha256 = "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890"
```

Copy the hash value into your TOML configuration. When the script is updated, the hash must be updated too -- the driver never auto-updates hashes.

---

## Step 4: Declare the Datasource

In your app's `resources.toml`, declare an exec datasource.

```toml
# resources.toml

[[datasources]]
name       = "ops_tools"
driver     = "plugin:rivers-exec"
nopassword = true
required   = true
```

- `driver = "plugin:rivers-exec"` -- the exec driver is a plugin, not a built-in driver
- `nopassword = true` -- the exec driver has no credentials
- `required = true` -- app startup fails if the driver cannot initialize (hash mismatches, missing scripts, invalid user)

---

## Step 5: Configure Commands in app.toml

Each command is a named entry under `[data.datasources.<name>.commands.<command_name>]`.

```toml
# app.toml

[data.datasources.ops_tools]
name              = "ops_tools"
driver            = "plugin:rivers-exec"
run_as_user       = "rivers-exec"
working_directory = "/var/rivers/exec-scratch"
default_timeout_ms = 30000
max_stdout_bytes  = 5242880
max_concurrent    = 10
integrity_check   = "each_time"

[data.datasources.ops_tools.commands.network_scan]
path             = "/usr/lib/rivers/scripts/netscan.sh"
sha256           = "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode       = "stdin"
args_schema      = "exec_schemas/netscan_args.json"
timeout_ms       = 60000
max_stdout_bytes = 10485760
max_concurrent   = 3
integrity_check  = "every:50"
env_clear        = true
env_allow        = ["PATH", "HOME"]
env_set          = { SCAN_LOG = "/var/log/rivers/scan.log" }
```

### Global Configuration

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `run_as_user` | yes | -- | OS user for spawned processes. Must exist. Must not be root. |
| `working_directory` | no | `/tmp` | Working directory for spawned processes. Must be writable by `run_as_user`. |
| `default_timeout_ms` | no | `30000` | Default timeout for command execution. |
| `max_stdout_bytes` | no | `5242880` | Default stdout read cap in bytes (5MB). |
| `max_concurrent` | no | `10` | Global concurrency limit across all commands. |
| `integrity_check` | no | `"each_time"` | Default integrity check mode. |

### Per-Command Configuration

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `path` | yes | -- | Absolute path to the executable. |
| `sha256` | yes | -- | SHA-256 hex digest of the file at `path`. |
| `input_mode` | no | `"stdin"` | How parameters are delivered: `stdin`, `args`, or `both`. |
| `args_template` | conditional | -- | Required when `input_mode` is `args` or `both`. |
| `args_schema` | no | -- | Path to JSON Schema file for input validation. |
| `timeout_ms` | no | global default | Timeout for this command's execution. |
| `max_stdout_bytes` | no | global default | Stdout read cap for this command. |
| `max_concurrent` | no | unlimited | Per-command concurrency limit. |
| `integrity_check` | no | global default | Per-command integrity check mode override. |
| `env_clear` | no | `true` | Clear environment before spawning. |
| `env_allow` | no | `[]` | Environment variables inherited from host (only when `env_clear = true`). |
| `env_set` | no | `{}` | Environment variables explicitly set for this command. |

### Integrity Check Modes

| Mode | Description |
|------|-------------|
| `"each_time"` | Hash the file before every execution. Zero tamper window. Default. |
| `"startup_only"` | Hash once at driver init. For immutable deployments (containers, read-only filesystems). |
| `"every:N"` | Hash every Nth execution. Balance between security and performance for high-frequency commands. |

---

## Step 6: Create a JSON Schema

Input validation happens before the process spawns. Create a schema at `exec_schemas/netscan_args.json`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["cidr", "ports"],
  "additionalProperties": false,
  "properties": {
    "cidr": {
      "type": "string",
      "pattern": "^[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}/[0-9]{1,2}$"
    },
    "ports": {
      "type": "array",
      "items": { "type": "integer", "minimum": 1, "maximum": 65535 },
      "minItems": 1,
      "maxItems": 20
    }
  }
}
```

This is where domain-specific guardrails live. CIDR format restrictions, port range limits, allowed values -- all enforced as JSON Schema constraints before the script ever runs. Invalid input returns a `DriverError::Query` without spawning a process.

---

## Step 7: Write the Handler

From the handler's perspective, the ExecDriver is just another datasource. The handler does not know a script is being executed.

```javascript
// libraries/handlers/ops.js

function onScanNetwork(ctx) {
    var body = ctx.request.body;

    if (!body) throw new Error("request body required");
    if (!body.cidr) throw new Error("cidr is required");
    if (!body.ports) throw new Error("ports is required");

    var result = ctx.dataview("run_network_scan", {
        command: "network_scan",
        args: {
            cidr: body.cidr,
            ports: body.ports
        }
    });

    Rivers.log.info("network scan completed", { cidr: body.cidr });

    ctx.resdata = result.rows[0].result;
}
```

The `query` operation expects:
- `command` -- the declared command name (e.g., `"network_scan"`)
- `args` -- the parameter object passed to the script

The script's stdout JSON is returned in `rows[0].result`.

---

## Step 8: Configure the View and DataView

```toml
# app.toml (continued)

# ─────────────────────────
# DataView for the scan command
# ─────────────────────────

[data.dataviews.run_network_scan]
name       = "run_network_scan"
datasource = "ops_tools"
query      = "query"

[[data.dataviews.run_network_scan.parameters]]
name     = "command"
type     = "string"
required = true

[[data.dataviews.run_network_scan.parameters]]
name     = "args"
type     = "object"
required = true

# ─────────────────────────
# View
# ─────────────────────────

[api.views.scan_network]
path            = "ops/scan"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.scan_network.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/ops.js"
entrypoint = "onScanNetwork"
resources  = ["ops_tools"]
```

The `resources` array must include `"ops_tools"`. Accessing an undeclared datasource from handler code throws `CapabilityError`.

---

## Step 9: Args Mode Example -- DNS Lookup

For scripts that take command-line arguments instead of stdin, use `input_mode = "args"` with an `args_template`.

### The Script

```bash
#!/bin/bash
# dns_lookup.sh — DNS lookup via dig, returns JSON
set -euo pipefail

DOMAIN="$1"
TYPE="$3"      # after --type flag
TIMEOUT="$5"   # after --timeout flag

RESULT=$(dig +short "$DOMAIN" "$TYPE" +time="$TIMEOUT" 2>/dev/null)

python3 -c "
import json, sys
records = [r.strip() for r in '''$RESULT'''.strip().split('\n') if r.strip()]
print(json.dumps({'domain': '$DOMAIN', 'type': '$TYPE', 'records': records}))
"
```

### Command Configuration

```toml
[data.datasources.ops_tools.commands.dns_lookup]
path            = "/usr/lib/rivers/scripts/dns_lookup.sh"
sha256           = "b2c3d4e5f6a17890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode      = "args"
args_template   = ["{domain}", "--type", "{record_type}", "--timeout", "{timeout}"]
args_schema     = "exec_schemas/dns_args.json"
timeout_ms      = 10000
integrity_check = "startup_only"
env_clear       = true
env_allow       = ["PATH"]
```

### Template Rules

- Each element in `args_template` is either a literal string (`"--type"`) or a placeholder (`"{domain}"`).
- Placeholders are replaced with the string value of the corresponding key from the handler's `args` object.
- Each placeholder produces exactly **one** argument. No whitespace splitting, no glob expansion, no shell interpretation.
- Array and object values are not permitted in template placeholders -- only scalar values.
- No shell is involved. `tokio::process::Command` passes each element directly to `execve`.

### Handler

```javascript
function onDnsLookup(ctx) {
    var result = ctx.dataview("run_dns_lookup", {
        command: "dns_lookup",
        args: {
            domain: ctx.request.body.domain,
            record_type: ctx.request.body.type || "A",
            timeout: ctx.request.body.timeout || "5"
        }
    });

    ctx.resdata = result.rows[0].result;
}
```

### Schema

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["domain", "record_type"],
  "additionalProperties": false,
  "properties": {
    "domain": {
      "type": "string",
      "pattern": "^[a-zA-Z0-9][a-zA-Z0-9.-]+[a-zA-Z]{2,}$"
    },
    "record_type": {
      "type": "string",
      "enum": ["A", "AAAA", "MX", "CNAME", "TXT", "NS", "SOA", "PTR"]
    },
    "timeout": {
      "type": "string",
      "pattern": "^[0-9]{1,3}$"
    }
  }
}
```

---

## Step 10: Verify and Deploy

Before deploying, verify script integrity and validate the bundle.

```bash
# Verify a script matches its declared hash
riversctl exec verify /usr/lib/rivers/scripts/netscan.sh \
    a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890
# Output: OK

# Validate the bundle configuration
riversctl validate my-ops-bundle/
```

When a script is updated, recompute the hash and update the TOML config:

```bash
# Recompute hash after a script update
riversctl exec hash /usr/lib/rivers/scripts/netscan.sh
# Update sha256 in app.toml, then reload riversd
```

---

## Complete Example

### Bundle Structure

```
ops-bundle/
├── manifest.toml
├── ops-service/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── exec_schemas/
│   │   ├── netscan_args.json
│   │   └── dns_args.json
│   └── libraries/
│       └── handlers/
│           └── ops.js
```

### manifest.toml (bundle)

```toml
[bundle]
name    = "ops-bundle"
version = "1.0.0"

[[apps]]
name = "ops-service"
path = "ops-service"
```

### manifest.toml (app)

```toml
[app]
appId   = "f1e2d3c4-b5a6-7890-abcd-ef1234567890"
appName = "ops-service"
type    = "service"
port    = 9300
```

### resources.toml

```toml
[[datasources]]
name       = "ops_tools"
driver     = "plugin:rivers-exec"
nopassword = true
required   = true
```

### app.toml

```toml
# ─────────────────────────
# ExecDriver Datasource
# ─────────────────────────

[data.datasources.ops_tools]
name               = "ops_tools"
driver             = "plugin:rivers-exec"
run_as_user        = "rivers-exec"
working_directory  = "/var/rivers/exec-scratch"
default_timeout_ms = 30000
max_stdout_bytes   = 5242880
max_concurrent     = 10
integrity_check    = "each_time"

[data.datasources.ops_tools.commands.network_scan]
path             = "/usr/lib/rivers/scripts/netscan.sh"
sha256           = "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode       = "stdin"
args_schema      = "exec_schemas/netscan_args.json"
timeout_ms       = 60000
max_stdout_bytes = 10485760
max_concurrent   = 3
integrity_check  = "every:50"
env_clear        = true
env_allow        = ["PATH", "HOME"]
env_set          = { SCAN_LOG = "/var/log/rivers/scan.log" }

[data.datasources.ops_tools.commands.dns_lookup]
path            = "/usr/lib/rivers/scripts/dns_lookup.sh"
sha256          = "b2c3d4e5f6a17890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode      = "args"
args_template   = ["{domain}", "--type", "{record_type}", "--timeout", "{timeout}"]
args_schema     = "exec_schemas/dns_args.json"
timeout_ms      = 10000
integrity_check = "startup_only"
env_clear       = true
env_allow       = ["PATH"]

# ─────────────────────────
# DataViews
# ─────────────────────────

[data.dataviews.run_network_scan]
name       = "run_network_scan"
datasource = "ops_tools"
query      = "query"

[[data.dataviews.run_network_scan.parameters]]
name     = "command"
type     = "string"
required = true

[[data.dataviews.run_network_scan.parameters]]
name     = "args"
type     = "object"
required = true

# ─────────────────────────

[data.dataviews.run_dns_lookup]
name       = "run_dns_lookup"
datasource = "ops_tools"
query      = "query"

[[data.dataviews.run_dns_lookup.parameters]]
name     = "command"
type     = "string"
required = true

[[data.dataviews.run_dns_lookup.parameters]]
name     = "args"
type     = "object"
required = true

# ─────────────────────────
# Views
# ─────────────────────────

[api.views.scan_network]
path            = "ops/scan"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.scan_network.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/ops.js"
entrypoint = "onScanNetwork"
resources  = ["ops_tools"]

# ─────────────────────────

[api.views.dns_lookup]
path            = "ops/dns"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.dns_lookup.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/ops.js"
entrypoint = "onDnsLookup"
resources  = ["ops_tools"]
```

### libraries/handlers/ops.js

```javascript
function onScanNetwork(ctx) {
    var body = ctx.request.body;

    if (!body) throw new Error("request body required");
    if (!body.cidr) throw new Error("cidr is required");
    if (!body.ports) throw new Error("ports is required");

    var result = ctx.dataview("run_network_scan", {
        command: "network_scan",
        args: {
            cidr: body.cidr,
            ports: body.ports
        }
    });

    Rivers.log.info("network scan completed", { cidr: body.cidr });

    ctx.resdata = result.rows[0].result;
}

function onDnsLookup(ctx) {
    var body = ctx.request.body;

    if (!body) throw new Error("request body required");
    if (!body.domain) throw new Error("domain is required");

    var result = ctx.dataview("run_dns_lookup", {
        command: "dns_lookup",
        args: {
            domain: body.domain,
            record_type: body.type || "A",
            timeout: body.timeout || "5"
        }
    });

    Rivers.log.info("dns lookup completed", { domain: body.domain });

    ctx.resdata = result.rows[0].result;
}
```

### Testing

```bash
# Run a network scan
curl -X POST http://localhost:9300/ops/scan \
  -H "Content-Type: application/json" \
  -d '{"cidr":"10.0.1.0/24","ports":[22,80,443]}'

# Run a DNS lookup
curl -X POST http://localhost:9300/ops/dns \
  -H "Content-Type: application/json" \
  -d '{"domain":"example.com","type":"A"}'

# DNS lookup with MX records
curl -X POST http://localhost:9300/ops/dns \
  -H "Content-Type: application/json" \
  -d '{"domain":"example.com","type":"MX"}'
```

---

## Security Checklist

| Concern | Recommended Configuration |
|---------|--------------------------|
| **Privilege drop** | `run_as_user = "rivers-exec"` -- a dedicated, non-root, no-login-shell user |
| **File permissions** | Scripts owned by root, mode `0555`. `rivers-exec` cannot write to script directories. |
| **Environment** | `env_clear = true` (default). Explicitly allowlist only needed variables via `env_allow`. |
| **Integrity mode** | `"each_time"` for high-security commands. `"startup_only"` only for immutable deployments. |
| **Concurrency** | Set `max_concurrent` at both global and per-command levels to prevent resource exhaustion. |
| **Timeouts** | Set `timeout_ms` per command. The driver kills the entire process group on timeout. |
| **Output limits** | Set `max_stdout_bytes` to prevent memory exhaustion from runaway scripts. |
| **Input validation** | Always declare `args_schema` with tight constraints (patterns, enums, bounds). |
| **Working directory** | Use a dedicated scratch directory writable only by `rivers-exec`. No access to `riversd` config, LockBox files, or TLS material. |
| **No shell** | All process spawning uses `tokio::process::Command` with an explicit argument array. Shell metacharacters are inert. |

---

## Error Reference

| Condition | Error |
|-----------|-------|
| Missing `command` parameter | `DriverError::Query("missing 'command' parameter")` |
| Unknown command name | `DriverError::Unsupported("unknown command: '<name>'")` |
| Schema validation failed | `DriverError::Query("schema validation failed: <details>")` |
| Integrity check failed | `DriverError::Internal("integrity check failed for command '<name>': ...")` |
| Concurrency limit reached | `DriverError::Query("concurrency limit reached for command '<name>'")` |
| Command timed out | `DriverError::Query("command timed out")` |
| Output exceeded limit | `DriverError::Query("output exceeded limit")` |
| Non-zero exit code | `DriverError::Query("command failed: exit <code>: <stderr>")` |
| Invalid JSON output | `DriverError::Query("command produced invalid JSON")` |
| Empty output | `DriverError::Query("command produced no output")` |
