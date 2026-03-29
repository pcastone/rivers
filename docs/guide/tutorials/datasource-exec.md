# Tutorial: ExecDriver Datasource

**Rivers v0.52.5**

## Overview

The ExecDriver plugin (`rivers-plugin-exec`) exposes controlled execution of admin-declared external scripts and binaries through the standard `DatabaseDriver` contract. Handlers call commands by name via the datasource query interface. The driver handles execution, isolation, integrity verification, and all guardrails.

Use the ExecDriver when:
- You need to run existing host-level tools (network scanners, DNS lookups, certificate checks, system probes)
- The tools already exist as scripts in Python, Bash, Go, or compiled binaries
- Rewriting the tool as a Rivers driver or embedding it in the framework is wasteful
- You need controlled, audited command execution from handler code

The ExecDriver is a plugin, not a built-in driver. Operators opt into it explicitly. The plugin must be present in the configured plugin directory.

**Design principle:** The ExecDriver is a controlled RCE service. Only admin-declared commands execute. Commands are pinned by SHA-256 hash. Input is validated against JSON Schema before the process spawns. Scripts run as a restricted OS user, not as the `riversd` process user.

## Prerequisites

- The `rivers-plugin-exec` plugin present in the configured plugin directory
- A restricted OS user for running scripts (not root, not the `riversd` user)
- Scripts installed and accessible on the host filesystem
- `riversctl` CLI for computing SHA-256 hashes

---

## Step 1: Set Up the Execution Environment

Create a restricted OS user, script directory, and working directory.

```bash
# Create a restricted user for script execution
sudo useradd --system --shell /usr/sbin/nologin --no-create-home rivers-exec

# Create the script directory (root-owned, read+execute only)
sudo mkdir -p /usr/lib/rivers/scripts
sudo chmod 0555 /usr/lib/rivers/scripts

# Create the working directory (owned by rivers-exec)
sudo mkdir -p /var/rivers/exec-scratch
sudo chown rivers-exec:rivers-exec /var/rivers/exec-scratch
sudo chmod 0700 /var/rivers/exec-scratch

# Create the schema directory (root-owned, read only)
sudo mkdir -p /etc/rivers/exec-schemas
sudo chmod 0444 /etc/rivers/exec-schemas
```

The `rivers-exec` user should have minimal permissions:
- Read + execute on script directories
- Write only to the working directory and designated log paths
- No access to `riversd` config, LockBox files, or TLS material

---

## Step 2: Create a Script

Scripts must follow the stdin JSON contract: read JSON from stdin, do work, write JSON to stdout.

Create a network scan script at `/usr/lib/rivers/scripts/netscan.py`:

```python
#!/usr/bin/env python3
"""Network scan — reads JSON from stdin, writes JSON to stdout."""

import json
import sys
import subprocess

def main():
    params = json.load(sys.stdin)
    cidr = params["cidr"]
    ports = params.get("ports", [22, 80, 443])

    # Run nmap (must be installed and accessible to rivers-exec)
    port_arg = ",".join(str(p) for p in ports)
    result = subprocess.run(
        ["nmap", "-sT", "-p", port_arg, "--open", "-oX", "-", cidr],
        capture_output=True, text=True, timeout=55
    )

    if result.returncode != 0:
        print(json.dumps({"error": result.stderr.strip()}), file=sys.stderr)
        sys.exit(1)

    # Parse and return results
    hosts = parse_nmap_xml(result.stdout)
    json.dump({"hosts": hosts, "cidr": cidr, "ports_scanned": ports}, sys.stdout)

def parse_nmap_xml(xml_output):
    # Simplified — real implementation would parse XML
    return [{"ip": "10.0.1.1", "open_ports": [22, 80]}]

if __name__ == "__main__":
    main()
```

### Script Contract

| Rule | Description |
|------|-------------|
| **Input** | Read JSON from stdin (stdin mode) and/or parse argv (args mode) |
| **Output** | Write a single JSON document to stdout on success |
| **Errors** | Write diagnostics to stderr. Exit with non-zero code |
| **No interactivity** | Must not read from TTY or prompt for input |
| **Deterministic structure** | Same input should produce the same output structure (values may differ) |

Install the script:

```bash
sudo cp netscan.py /usr/lib/rivers/scripts/netscan.py
sudo chmod 0555 /usr/lib/rivers/scripts/netscan.py
sudo chown root:root /usr/lib/rivers/scripts/netscan.py
```

---

## Step 3: Compute the SHA-256 Hash

The SHA-256 hash pins the exact script content. If the file on disk does not match the declared hash, execution is refused.

```bash
riversctl exec hash /usr/lib/rivers/scripts/netscan.py
# Output:
# sha256 = "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890"
```

Copy the hash value into your configuration. Hash updates are always an explicit admin action -- the driver never auto-updates hashes.

To verify a file matches an expected hash:

```bash
riversctl exec verify /usr/lib/rivers/scripts/netscan.py \
    "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890"
```

---

## Step 4: Declare the Datasource in resources.toml

```toml
# resources.toml

[[datasources]]
name     = "ops_tools"
driver   = "rivers-exec"
x-type   = "database"
required = true
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Datasource name, referenced in app.toml and handler `resources` |
| `driver` | yes | Must be `"rivers-exec"` |
| `x-type` | yes | Must be `"database"` (ExecDriver implements DatabaseDriver) |
| `required` | no | Whether the app fails to start without this datasource |

---

## Step 5: Configure Commands in app.toml

Configure the datasource with global settings and per-command declarations.

```toml
# app.toml

# ─────────────────────────────────────────────
# Datasource: ExecDriver
# ─────────────────────────────────────────────

[data.datasources.ops_tools]
driver             = "rivers-exec"
run_as_user        = "rivers-exec"
working_directory  = "/var/rivers/exec-scratch"
default_timeout_ms = 30000
max_stdout_bytes   = 5242880
max_concurrent     = 10
integrity_check    = "each_time"

# ─────────────────────────────────────────────
# Command: Network Scan (stdin mode)
# ─────────────────────────────────────────────

[data.datasources.ops_tools.commands.network_scan]
path             = "/usr/lib/rivers/scripts/netscan.py"
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
| `working_directory` | no | `/tmp` | Working directory for spawned processes |
| `default_timeout_ms` | no | `30000` | Default timeout for command execution |
| `max_stdout_bytes` | no | `5242880` | Default stdout read cap in bytes (5MB) |
| `max_concurrent` | no | `10` | Global concurrency limit across all commands |
| `integrity_check` | no | `"each_time"` | Default integrity check mode |

### Per-Command Configuration

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `path` | yes | -- | Absolute path to the executable |
| `sha256` | yes | -- | SHA-256 hex digest of the file at `path` |
| `input_mode` | no | `"stdin"` | `"stdin"`, `"args"`, or `"both"` |
| `args_template` | conditional | -- | Required when `input_mode` is `"args"` or `"both"` |
| `stdin_key` | conditional | -- | Required when `input_mode` is `"both"` |
| `args_schema` | no | -- | Path to JSON Schema file for input validation |
| `timeout_ms` | no | global default | Timeout for this command |
| `max_stdout_bytes` | no | global default | Stdout cap for this command |
| `max_concurrent` | no | unlimited | Per-command concurrency limit (in addition to global) |
| `integrity_check` | no | global default | `"each_time"`, `"startup_only"`, or `"every:N"` |
| `env_clear` | no | `true` | Clear environment before spawning |
| `env_allow` | no | `[]` | Environment variables inherited from host (only when `env_clear = true`) |
| `env_set` | no | `{}` | Environment variables explicitly set for this command |

### Integrity Check Modes

| Mode | Behavior | Use Case |
|------|----------|----------|
| `"each_time"` | Hash before every execution | High-security commands (default) |
| `"startup_only"` | Hash once at driver init | Immutable deployments (containers, read-only filesystems) |
| `"every:N"` | Hash every Nth execution | High-frequency commands where per-execution hashing is measurable |

---

## Step 6: JSON Schema Validation (Optional)

Create a JSON Schema file to validate input parameters before the script executes.

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

Save as `exec_schemas/netscan_args.json` in your app directory. Schema validation occurs before the process spawns -- invalid requests never trigger execution.

---

## Step 7: Write a Handler

The handler calls the exec datasource using `ctx.datasource()`. The ExecDriver is accessed like any other datasource -- the handler does not know a script is being executed.

```javascript
// libraries/handlers/ops.js

function scanNetwork(ctx) {
    var body = ctx.request.body;

    if (!body.cidr) throw new Error("cidr is required");
    if (!body.ports) throw new Error("ports array is required");

    // Execute the network_scan command
    var result = ctx.datasource("ops_tools")
        .fromQuery("network_scan", {
            cidr: body.cidr,
            ports: body.ports
        })
        .build();

    Rivers.log.info("network scan complete", {
        cidr: body.cidr,
        host_count: result.hosts.length
    });

    ctx.resdata = result;
}
```

The handler passes `command` as the query name and the parameters as the args object. The driver handles command lookup, integrity verification, schema validation, process spawning, and output parsing.

---

## Step 8: Configure the View

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

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
entrypoint = "scanNetwork"
resources  = ["ops_tools"]
```

The `resources` list must include the exec datasource. Accessing an undeclared datasource throws `CapabilityError`.

---

## Step 9: Args Mode Example (DNS Lookup)

For scripts that accept command-line arguments instead of stdin, use `input_mode = "args"` with an `args_template`.

### The Script

```bash
#!/usr/bin/env bash
# dns_lookup.sh — DNS lookup with JSON output
# Usage: dns_lookup.sh <domain> --type <type> --timeout <seconds>

DOMAIN="$1"
TYPE="$3"     # after --type flag
TIMEOUT="$5"  # after --timeout flag

RESULT=$(dig +short "$DOMAIN" "$TYPE" +time="$TIMEOUT" 2>/dev/null)

if [ $? -ne 0 ]; then
    echo "DNS lookup failed" >&2
    exit 1
fi

# Build JSON output
echo "{\"domain\":\"$DOMAIN\",\"type\":\"$TYPE\",\"records\":["
FIRST=true
while IFS= read -r line; do
    if [ "$FIRST" = true ]; then FIRST=false; else echo ","; fi
    echo "  \"$line\""
done <<< "$RESULT"
echo "]}"
```

### Command Configuration

```toml
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
```

### Args Schema

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
      "enum": ["A", "AAAA", "MX", "CNAME", "TXT", "NS", "SOA"]
    },
    "timeout": {
      "type": "integer",
      "minimum": 1,
      "maximum": 30,
      "default": 5
    }
  }
}
```

### Handler

```javascript
function dnsLookup(ctx) {
    var body = ctx.request.body;

    var result = ctx.datasource("ops_tools")
        .fromQuery("dns_lookup", {
            domain: body.domain,
            record_type: body.record_type || "A",
            timeout: body.timeout || 5
        })
        .build();

    ctx.resdata = result;
}
```

### Template Interpolation Rules

- Each `{key}` placeholder is replaced with the string value of the corresponding parameter
- Each placeholder produces exactly one argument -- no splitting on whitespace, no glob expansion
- Array and object values are not permitted in template placeholders
- Extra keys in params that do not appear in any placeholder are silently ignored
- No shell involved -- `tokio::process::Command` passes args directly to `execve`

---

## Step 10: Verify and Deploy

### Verify Script Integrity

```bash
# Verify each script matches its declared hash
riversctl exec verify /usr/lib/rivers/scripts/netscan.py \
    "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890"

riversctl exec verify /usr/lib/rivers/scripts/dns_lookup.sh \
    "b2c3d4e5f6a17890abcdef1234567890abcdef1234567890abcdef1234567890"
```

### Validate the Bundle

```bash
riversctl validate
```

### Test the Endpoints

```bash
# Network scan (stdin mode)
curl -X POST http://localhost:9100/ops/scan \
  -H "Content-Type: application/json" \
  -d '{"cidr":"10.0.1.0/24","ports":[22,80,443]}'

# DNS lookup (args mode)
curl -X POST http://localhost:9100/ops/dns \
  -H "Content-Type: application/json" \
  -d '{"domain":"example.com","record_type":"A","timeout":5}'
```

---

## Complete Example

### Bundle Structure

```
ops-tools-bundle/
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
├── /usr/lib/rivers/scripts/          (host filesystem)
│   ├── netscan.py
│   └── dns_lookup.sh
└── /var/rivers/exec-scratch/         (host filesystem)
```

### manifest.toml (bundle)

```toml
[bundle]
name    = "ops-tools"
version = "1.0.0"

[[apps]]
name = "ops-service"
path = "ops-service"
```

### manifest.toml (service app)

```toml
[app]
appId = "d4e5f6a1-b2c3-7890-abcd-ef1234567890"
name  = "ops-service"
type  = "service"
port  = 9100
```

### resources.toml

```toml
[[datasources]]
name     = "ops_tools"
driver   = "rivers-exec"
x-type   = "database"
required = true
```

### app.toml

```toml
# ─────────────────────────────────────────────
# Datasource: ExecDriver
# ─────────────────────────────────────────────

[data.datasources.ops_tools]
driver             = "rivers-exec"
run_as_user        = "rivers-exec"
working_directory  = "/var/rivers/exec-scratch"
default_timeout_ms = 30000
max_stdout_bytes   = 5242880
max_concurrent     = 10
integrity_check    = "each_time"

# ── Command: Network Scan (stdin mode) ──

[data.datasources.ops_tools.commands.network_scan]
path             = "/usr/lib/rivers/scripts/netscan.py"
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

# ── Command: DNS Lookup (args mode) ──

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

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

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
entrypoint = "scanNetwork"
resources  = ["ops_tools"]

# ─────────────────────────────────────────────

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
entrypoint = "dnsLookup"
resources  = ["ops_tools"]

# ─────────────────────────────────────────────
# ProcessPool
# ─────────────────────────────────────────────

[runtime.process_pools.default]
engine          = "v8"
workers         = 4
task_timeout_ms = 5000
max_heap_mb     = 128
```

### libraries/handlers/ops.js

```javascript
// libraries/handlers/ops.js

function scanNetwork(ctx) {
    var body = ctx.request.body;

    if (!body.cidr) throw new Error("cidr is required");
    if (!body.ports) throw new Error("ports array is required");

    var result = ctx.datasource("ops_tools")
        .fromQuery("network_scan", {
            cidr: body.cidr,
            ports: body.ports
        })
        .build();

    Rivers.log.info("network scan complete", {
        cidr: body.cidr,
        host_count: result.hosts.length
    });

    ctx.resdata = result;
}

function dnsLookup(ctx) {
    var body = ctx.request.body;

    if (!body.domain) throw new Error("domain is required");

    var result = ctx.datasource("ops_tools")
        .fromQuery("dns_lookup", {
            domain: body.domain,
            record_type: body.record_type || "A",
            timeout: body.timeout || 5
        })
        .build();

    Rivers.log.info("dns lookup complete", {
        domain: body.domain,
        record_count: result.records.length
    });

    ctx.resdata = result;
}
```

### exec_schemas/netscan_args.json

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

### exec_schemas/dns_args.json

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
      "enum": ["A", "AAAA", "MX", "CNAME", "TXT", "NS", "SOA"]
    },
    "timeout": {
      "type": "integer",
      "minimum": 1,
      "maximum": 30,
      "default": 5
    }
  }
}
```

## Security Checklist

| Control | Configuration | Purpose |
|---------|--------------|---------|
| Restricted OS user | `run_as_user = "rivers-exec"` | Privilege drop -- scripts never run as root or riversd user |
| Script permissions | `chmod 0555`, `chown root:root` | Scripts are read-only, owned by root -- `rivers-exec` cannot modify them |
| SHA-256 pinning | `sha256 = "..."` on every command | Tampered or updated scripts are rejected |
| Integrity frequency | `integrity_check = "each_time"` | Hash verified before every execution (default) |
| Environment clearing | `env_clear = true` | Prevents leaking riversd credentials to scripts |
| Environment allowlist | `env_allow = ["PATH"]` | Explicit list of inherited environment variables |
| Input validation | `args_schema = "exec_schemas/..."` | JSON Schema enforced before process spawn |
| No shell | `tokio::process::Command` arg array | No shell interpretation -- metacharacters are inert |
| Fixed argument shape | `args_template = [...]` | Handler controls values, not argument structure |
| Output cap | `max_stdout_bytes = 5242880` | Process killed if output exceeds limit |
| Timeout | `timeout_ms = 60000` | Process group killed on timeout |
| Concurrency limits | `max_concurrent = 10` (global), `3` (per-command) | Prevents fork-bombing from request bursts |
| Process group isolation | `setsid` on spawn | Timeout kills the entire process group, including children |
| Audit trail | Structured logging with `trace_id` | Every execution logged with request correlation |

### When to Use `startup_only` Integrity

Use `integrity_check = "startup_only"` only when:
- The host filesystem is read-only (containers, immutable deployments)
- The per-execution hash cost is measurable for high-frequency commands
- File permissions and ownership prevent modification by non-root users

For all other cases, use the default `"each_time"` mode.

### Script Update Workflow

When a script is updated:

```bash
# 1. Deploy the new script
sudo cp netscan_v2.py /usr/lib/rivers/scripts/netscan.py
sudo chmod 0555 /usr/lib/rivers/scripts/netscan.py

# 2. Compute the new hash
riversctl exec hash /usr/lib/rivers/scripts/netscan.py

# 3. Update sha256 in app.toml

# 4. Reload or restart riversd
```

Hash updates are always an explicit admin action. The driver never auto-updates hashes.

## Configuration Reference

### Execution Pipeline

```
1. Extract "command" from params
2. Lookup command in allowlist
3. Validate args against JSON Schema (if declared)
4. Integrity check (mode-dependent)
5. Acquire semaphores (global + per-command)
6. Spawn process (privilege-dropped, env-controlled)
7. Write stdin / set args (mode-dependent)
8. Bounded read with timeout
9. Parse JSON stdout
10. Release semaphores
```

### Error Reference

| Condition | Error |
|-----------|-------|
| Missing `command` parameter | `DriverError::Query("missing 'command' parameter")` |
| Unknown command name | `DriverError::Unsupported("unknown command")` |
| Schema validation failed | `DriverError::Query("schema validation failed: ...")` |
| Integrity hash mismatch | `DriverError::Internal("integrity check failed")` |
| Concurrency limit reached | `DriverError::Query("concurrency limit reached")` |
| Command timed out | `DriverError::Query("command timed out")` |
| Output exceeded limit | `DriverError::Query("output exceeded limit")` |
| Non-zero exit code | `DriverError::Query("command failed: exit N: stderr")` |
| Invalid JSON output | `DriverError::Query("command produced invalid JSON")` |
| Empty output | `DriverError::Query("command produced no output")` |
| `run_as_user` is root | `DriverError::Connection("run_as_user must not be root")` |
| `read` / `write` / `delete` ops | `DriverError::Unsupported("exec driver does not support ...")` |

### Driver Capabilities

| Capability | Supported |
|------------|-----------|
| Query (command execution) | Yes |
| Read / Write / Delete | No |
| Transactions | No |
| Connection pooling | No (process-per-execution) |
| Concurrency control | Yes (global + per-command semaphores) |
| Input validation | Yes (JSON Schema) |
| Integrity verification | Yes (SHA-256) |
