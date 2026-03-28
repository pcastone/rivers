# Rivers ExecDriver Plugin Specification

**Document Type:** Implementation Specification  
**Scope:** ExecDriver plugin — command execution driver, integrity model, input modes, guardrails  
**Status:** Design / Pre-Implementation  
**Depends On:** Rivers Driver Specification (plugin system §7), rivers-driver-sdk  
**Addresses:** Controlled execution of admin-declared external commands from CodeComponent handlers

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Architecture Overview](#2-architecture-overview)
3. [Operation Mapping](#3-operation-mapping)
4. [Configuration Reference](#4-configuration-reference)
5. [Command Declaration](#5-command-declaration)
6. [Integrity Model](#6-integrity-model)
7. [Input Modes](#7-input-modes)
8. [Argument Templates](#8-argument-templates)
9. [JSON Schema Validation](#9-json-schema-validation)
10. [Execution Pipeline](#10-execution-pipeline)
11. [Process Isolation](#11-process-isolation)
12. [Concurrency Control](#12-concurrency-control)
13. [Output Handling](#13-output-handling)
14. [Error Model](#14-error-model)
15. [Logging and Audit](#15-logging-and-audit)
16. [Security Properties](#16-security-properties)
17. [Operational Guidance](#17-operational-guidance)
18. [Handler API Surface](#18-handler-api-surface)
19. [Plugin Registration](#19-plugin-registration)

---

## 1. Problem Statement

Rivers handlers execute inside process-isolated sandboxes with allowlist-injected capabilities. By design, handlers cannot access the host filesystem, spawn processes, or open raw sockets. This is correct for application logic.

However, legitimate operational use cases exist that require executing host-level tools: network scanning, certificate validation, DNS bulk lookups, LDAP queries, system health probes. These tools already exist as scripts and binaries in languages suited to the task (Python, Bash, Go, compiled Rust). Rewriting them as Rivers drivers or embedding their logic in the framework is wasteful and introduces unnecessary coupling.

The ExecDriver solves this by exposing admin-declared, integrity-verified external commands through the standard driver contract. Handlers call commands by name via `Rivers.view.query()`. The driver handles execution, isolation, I/O, and all guardrails. The handler never knows it is executing a script — it is just querying a datasource.

### 1.1 Design Principle

The ExecDriver is a **controlled RCE service**. Every design decision prioritizes constraint over convenience:

- Only admin-declared commands execute. The handler cannot specify arbitrary paths.
- Commands are pinned by SHA-256 hash. If the file on disk does not match the declared hash, execution is refused.
- Input is validated against JSON Schema before the process is spawned.
- Scripts run as a restricted OS user, not as the `riversd` process user.
- Output is bounded. Timeouts kill the process group. Concurrency is capped.

The ExecDriver is a plugin, not a core framework feature. Operators opt into it explicitly.

---

## 2. Architecture Overview

```
Handler (V8/WASM isolate)
    │
    │  Rivers.view.query("ops_tools", { command: "network_scan", args: {...} })
    │
    ▼
Driver Contract (host-side Rust)
    │
    ├─ 1. Command lookup (allowlist)
    ├─ 2. Integrity check (SHA-256)
    ├─ 3. Input validation (JSON Schema)
    ├─ 4. Semaphore acquisition
    ├─ 5. Process spawn (privilege-dropped)
    │     ├─ stdin: JSON params (stdin mode)
    │     ├─ args: template interpolation (args mode)
    │     └─ both: args + stdin (both mode)
    ├─ 6. Bounded I/O + timeout
    ├─ 7. Result parsing
    └─ 8. Semaphore release
    │
    ▼
Script / Binary (runs as restricted user)
    │
    ├─ reads stdin and/or argv
    ├─ does work (scan, query, check, etc.)
    └─ writes JSON to stdout
```

The driver never passes through to a shell. All process spawning uses `tokio::process::Command` with an explicit argument array. There is no shell interpretation at any point.

---

## 3. Operation Mapping

| Driver Operation | ExecDriver Behavior |
|---|---|
| `new` | Validate config. Hash all declared command files. Compare to pinned SHA-256 values. Build semaphores. Reject startup if any hash mismatches. |
| `query` | Execute a declared command. Full pipeline: integrity → validation → spawn → collect → parse. |
| `read` | `DriverError::Unsupported("exec driver does not support read")` |
| `write` | `DriverError::Unsupported("exec driver does not support write")` |
| `delete` | `DriverError::Unsupported("exec driver does not support delete")` |

The `query` operation expects a `command` key in the parameters identifying the declared command name, and an `args` key containing the parameter object for that command.

---

## 4. Configuration Reference

### 4.1 Global Configuration

```toml
[datasources.ops_tools]
driver = "plugin:rivers-exec"

# Process isolation
run_as_user        = "rivers-exec"          # required — privilege drop target
working_directory  = "/var/rivers/exec-scratch"  # cwd for spawned processes

# Global limits
default_timeout_ms = 30000                  # per-command override available
max_stdout_bytes   = 5242880                # 5MB — per-command override available
max_concurrent     = 10                     # global semaphore across all commands

# Global integrity default (per-command override available)
integrity_check    = "each_time"            # each_time | startup_only | every:N
```

| Field | Required | Default | Description |
|---|---|---|---|
| `run_as_user` | yes | — | OS user for spawned processes. Must exist. Must not be root. |
| `working_directory` | no | `/tmp` | Working directory for spawned processes. Must be writable by `run_as_user`. |
| `default_timeout_ms` | no | `30000` | Default timeout for command execution. |
| `max_stdout_bytes` | no | `5242880` | Default stdout read cap in bytes. |
| `max_concurrent` | no | `10` | Global concurrency limit across all commands. |
| `integrity_check` | no | `"each_time"` | Default integrity check mode. |

### 4.2 Startup Validation (`new`)

On driver initialization:

1. `run_as_user` must resolve to a valid OS user (via `getpwnam`). Must not be UID 0.
2. `working_directory` must exist and be writable by `run_as_user`.
3. Every declared command is validated (see §5).
4. If any validation fails, the driver refuses to initialize and emits `DriverError::Connection` with details.

---

## 5. Command Declaration

Each command is a named entry under `[datasources.<name>.commands.<command_name>]`:

```toml
[datasources.ops_tools.commands.network_scan]
path            = "/usr/lib/rivers/scripts/netscan.py"
sha256          = "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode      = "stdin"
args_schema     = "exec_schemas/netscan_args.json"
timeout_ms      = 60000
max_stdout_bytes = 10485760
max_concurrent  = 3
integrity_check = "interval:50"
env_clear       = true
env_allow       = ["PATH", "HOME"]
env_set         = { SCAN_LOG = "/var/log/rivers/scan.log" }
```

| Field | Required | Default | Description |
|---|---|---|---|
| `path` | yes | — | Absolute path to the executable. |
| `sha256` | yes | — | SHA-256 hex digest of the file at `path`. |
| `input_mode` | no | `"stdin"` | How parameters are delivered: `stdin`, `args`, or `both`. See §7. |
| `args_template` | conditional | — | Required when `input_mode` is `args` or `both`. See §8. |
| `stdin_key` | conditional | — | Required when `input_mode` is `both`. Key whose value is sent on stdin. See §7.3. |
| `args_schema` | no | — | Path to JSON Schema file for input validation. Relative to app bundle root. |
| `timeout_ms` | no | global `default_timeout_ms` | Timeout for this command's execution. |
| `max_stdout_bytes` | no | global `max_stdout_bytes` | Stdout read cap for this command. |
| `max_concurrent` | no | unlimited | Per-command concurrency limit (in addition to global limit). |
| `integrity_check` | no | global `integrity_check` | Per-command integrity check mode override. |
| `env_clear` | no | `true` | Clear environment before spawning. |
| `env_allow` | no | `[]` | Environment variables inherited from host (only when `env_clear = true`). |
| `env_set` | no | `{}` | Environment variables explicitly set for this command. |

### 5.1 Startup Validation Per Command

For each declared command, during driver `new`:

1. `path` must be an absolute path.
2. File at `path` must exist and be executable by `run_as_user`.
3. SHA-256 of file contents must match `sha256`. Mismatch → driver refuses to start.
4. If `args_schema` is declared, the schema file must exist and parse as valid JSON Schema.
5. If `input_mode` is `args` or `both`, `args_template` must be present and non-empty.
6. If `input_mode` is `both`, `stdin_key` must be present and non-empty.

---

## 6. Integrity Model

The SHA-256 hash declared in config is the **authorization mechanism**. The admin declares that exactly these bytes, at this path, are approved for execution. Any deviation — update, tampering, corruption — is a hard failure.

### 6.1 Integrity Check Modes

```toml
integrity_check = "each_time"       # hash before every execution
integrity_check = "startup_only"    # hash once at driver init
integrity_check = "every:50"        # hash every 50th execution of this command
```

| Mode | Cost | Tamper Window | Use Case |
|---|---|---|---|
| `each_time` | One SHA-256 per execution | Zero | High-security commands. Default. |
| `startup_only` | One SHA-256 at startup | Entire runtime | Immutable deployments (containers, read-only filesystems). |
| `every:N` | One SHA-256 per N executions | Up to N-1 executions | High-frequency commands where per-execution hashing is measurable. |

### 6.2 Implementation

Per-command state:

```rust
struct CommandIntegrity {
    mode:        IntegrityMode,
    exec_count:  AtomicU64,
    pinned_hash: [u8; 32],
}
```

Check logic:

```rust
fn should_check(&self) -> bool {
    match self.mode {
        IntegrityMode::EachTime     => true,
        IntegrityMode::StartupOnly  => false,
        IntegrityMode::Every(n)     => {
            let count = self.exec_count.fetch_add(1, Ordering::Relaxed) + 1;
            count % n == 0
        }
    }
}
```

### 6.3 Hash Computation

```rust
fn hash_file(path: &Path) -> Result<[u8; 32], DriverError> {
    let bytes = std::fs::read(path)
        .map_err(|e| DriverError::Internal(format!("cannot read {}: {}", path.display(), e)))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(digest.as_ref());
    Ok(hash)
}
```

### 6.4 Mismatch Behavior

At startup: driver refuses to initialize. Log entry includes command name, expected hash, actual hash, file path.

At runtime: `DriverError::Internal("integrity check failed for command '<name>': expected <expected>, got <actual>")`. The command does not execute. The log entry is emitted at `ERROR` level.

### 6.5 Startup Log Messages

The driver logs the integrity mode for every command at startup to make the security posture auditable:

```
INFO  datasource=ops_tools command=network_scan integrity_check=each_time
WARN  datasource=ops_tools command=dns_lookup integrity_check=startup_only 
      msg="script integrity checked at startup only — runtime tampering not detected"
WARN  datasource=ops_tools command=cert_check integrity_check=every:50
      msg="script integrity checked every 50 executions — tamper detection window applies"
```

`each_time` logs at `INFO`. All other modes log at `WARN`.

---

## 7. Input Modes

Three modes control how parameters are delivered to the spawned process.

### 7.1 `stdin` (default)

The driver serializes the handler's `args` object as JSON and writes it to the child process's stdin, then closes stdin.

```
Driver                          Script
  │                               │
  │── stdin: {"cidr":"10.0.1.0/24","ports":[22,80]} ──►│
  │   (close stdin)               │
  │                               │── reads json.load(sys.stdin)
  │◄── stdout: {"hosts":[...]} ──│
```

Scripts read stdin as a single JSON document. Language-agnostic. No injection surface.

### 7.2 `args`

The driver interpolates parameter values into a declared `args_template` (see §8) and passes them as the process's argument vector.

```
Driver                          Script
  │                               │
  │── argv: ["example.com", "--type", "A", "--timeout", "5"] ──►│
  │   (stdin closed immediately)  │
  │                               │── reads sys.argv / $1 $2 ...
  │◄── stdout: {"records":[...]} ─│
```

No shell involved. `tokio::process::Command::args()` passes each element as a discrete argument. Shell metacharacters in values are inert.

### 7.3 `both`

Combination mode for scripts that need simple flags on the command line and bulk data on stdin.

```toml
input_mode    = "both"
args_template = ["--mode", "{mode}", "--timeout", "{timeout}"]
stdin_key     = "targets"
```

The driver:

1. Removes the `stdin_key` (`"targets"`) from the params object.
2. Interpolates remaining params into `args_template` for the argument vector.
3. Serializes the `stdin_key`'s value as JSON on stdin.

```
Driver                          Script
  │                               │
  │── argv: ["--mode", "fast", "--timeout", "30"] ──►│
  │── stdin: [{"cidr":"10.0.1.0/24"},{"cidr":"10.0.2.0/24"}] ──►│
  │   (close stdin)               │
  │                               │── reads argv for flags, stdin for data
  │◄── stdout: {"results":[...]} ─│
```

---

## 8. Argument Templates

When `input_mode` is `args` or `both`, the `args_template` defines the fixed structure of the argument vector.

### 8.1 Template Format

```toml
args_template = ["{domain}", "--type", "{record_type}", "--timeout", "{timeout}"]
```

Each element is either:
- A literal string (e.g., `"--type"`, `"--timeout"`) — passed verbatim.
- A placeholder `"{key}"` — replaced with the string value of the corresponding key from the handler's `args` object.

### 8.2 Interpolation Rules

1. Every placeholder `{key}` must have a corresponding key in the handler's `args` object. Missing key → `DriverError::Query("missing required parameter: '<key>'")`.
2. Values are converted to strings via standard JSON stringification: strings pass through, numbers become their decimal representation, booleans become `"true"` / `"false"`.
3. Each placeholder produces exactly **one** argument. No splitting on whitespace. No glob expansion. No shell interpretation. A value of `"foo bar"` becomes the single argument `"foo bar"`, not two arguments.
4. Array and object values are not permitted in template placeholders. If a parameter value is an array or object, the driver returns `DriverError::Query("parameter '<key>' must be a scalar value for args template")`.
5. Extra keys in `args` that do not appear in any placeholder are silently ignored.

### 8.3 Security Properties

- **Fixed argument count.** The template determines the number of arguments. The handler cannot inject additional flags or options.
- **No shell.** `tokio::process::Command` passes args directly to `execve`. Shell metacharacters (`;`, `|`, `$()`, backticks) are literal characters in the argument, not operators.
- **Schema validation first.** When `args_schema` is declared, the handler's parameters are validated against the schema *before* template interpolation. Invalid values never reach the template engine.

---

## 9. JSON Schema Validation

When `args_schema` is declared for a command, the handler's `args` object is validated against the schema before any execution occurs.

### 9.1 Schema Loading

Schema files are loaded at driver startup (`new`). Invalid schemas cause startup failure.

### 9.2 Validation Timing

Validation occurs after command lookup and before integrity check. This ordering avoids unnecessary file I/O for requests that would fail validation anyway.

### 9.3 Example Schema

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

This is where domain-specific guardrails live. CIDR restrictions, port limits, allowed values — all expressed as JSON Schema constraints enforced before the script ever runs.

### 9.4 Validation Failure

Returns `DriverError::Query("schema validation failed: <details>")`. The process is not spawned.

---

## 10. Execution Pipeline

Complete pipeline for a `query` operation:

```
1. Extract "command" from params
   └─ missing → DriverError::Query("missing 'command' parameter")

2. Lookup CommandConfig by name
   └─ not found → DriverError::Unsupported("unknown command: '<name>'")

3. Validate args against JSON Schema (if args_schema declared)
   └─ invalid → DriverError::Query("schema validation failed: ...")

4. Integrity check (mode-dependent)
   └─ mismatch → DriverError::Internal("integrity check failed for command '<name>': ...")

5. Acquire semaphores (global + per-command)
   └─ timeout → DriverError::Query("concurrency limit reached for command '<name>'")

6. Build tokio::process::Command:
   ├─ path:        from CommandConfig
   ├─ args:        from template interpolation (args/both mode)
   ├─ uid/gid:     from run_as_user (resolved at startup)
   ├─ cwd:         from working_directory
   ├─ env:         from env_clear + env_allow + env_set
   ├─ stdin:       piped (for stdin/both mode) or null (args mode)
   ├─ stdout:      piped
   ├─ stderr:      piped
   ├─ kill_on_drop: true
   └─ process_group: new session (setsid) for clean group kill

7. Spawn process

8. Write stdin (stdin/both mode):
   ├─ serialize params (or stdin_key value) as JSON
   ├─ write to child stdin
   └─ close stdin

9. Bounded read with timeout:
   ├─ tokio::time::timeout(timeout_ms)
   ├─ read stdout up to max_stdout_bytes
   ├─ read stderr (bounded to 64KB)
   └─ wait for exit

10. Evaluate result:
    ├─ timeout fired     → SIGKILL process group → DriverError::Query("command timed out")
    ├─ stdout overflow   → SIGKILL process group → DriverError::Query("output exceeded limit")
    ├─ exit 0 + valid JSON on stdout → return parsed JSON as QueryResult
    ├─ exit 0 + invalid JSON         → DriverError::Query("command produced invalid JSON")
    └─ exit non-zero                 → DriverError::Query("command failed: exit <code>: <stderr>")

11. Release semaphores
```

### 10.1 Ordering Rationale

- Schema validation (step 3) before integrity check (step 4): avoids file I/O for invalid requests.
- Integrity check (step 4) before semaphore acquisition (step 5): avoids holding a semaphore slot during a hash computation.
- Semaphore acquisition (step 5) before process spawn (step 6-7): prevents over-commitment.

---

## 11. Process Isolation

### 11.1 Privilege Drop

The spawned process runs as `run_as_user`, not as the `riversd` process user. The driver resolves the user to a UID/GID at startup via `getpwnam` and applies it via `tokio::process::Command::uid()` / `gid()`.

`run_as_user` must not be UID 0. The driver rejects `root` at startup.

For capabilities that require elevated privileges (e.g., `CAP_NET_RAW` for ICMP), the recommended approach is setting file capabilities on the target script/binary:

```bash
sudo setcap cap_net_raw+ep /usr/lib/rivers/scripts/netscan
```

This grants the capability to the specific binary, not to `riversd` or `run_as_user` globally.

### 11.2 Environment Control

```toml
env_clear = true              # start with empty environment
env_allow = ["PATH", "HOME"]  # inherit only these from host
env_set   = { SCAN_LOG = "/var/log/rivers/scan.log" }  # explicit overrides
```

When `env_clear = true` (default):
1. Start with an empty environment.
2. Copy only `env_allow` variables from the host environment.
3. Apply `env_set` overrides (these win over `env_allow` if both declare the same key).

When `env_clear = false`:
1. Start with the full host environment.
2. Apply `env_set` overrides.
3. `env_allow` is ignored (meaningless when not clearing).

`env_clear = false` is discouraged and logs `WARN` at startup. It risks leaking credentials or config from the `riversd` process environment.

### 11.3 Process Group Isolation

The spawned process runs in a new session (`setsid`). On timeout or stdout overflow, the driver sends `SIGKILL` to the entire process group (`kill(-pgid, SIGKILL)`), ensuring child processes forked by the script are also terminated.

### 11.4 Filesystem Scope

The driver does not enforce filesystem restrictions beyond OS-level permissions. The `run_as_user` account's filesystem access is the boundary. Operators should configure this user with minimal permissions:

- Read + execute on script directories
- Write only to `working_directory` and designated log paths
- No access to `riversd` config, LockBox files, or TLS material

---

## 12. Concurrency Control

Two independent semaphore layers:

### 12.1 Global Semaphore

```toml
max_concurrent = 10   # datasource-level
```

Bounds the total number of concurrently executing commands across all command types for this datasource. Prevents fork-bombing the host from a burst of requests.

### 12.2 Per-Command Semaphore

```toml
[datasources.ops_tools.commands.network_scan]
max_concurrent = 3
```

Bounds concurrent executions of a specific command. A network scan should not run 10 parallel instances even if the global limit allows it.

### 12.3 Semaphore Interaction

Both must be acquired before execution. If either is full, the driver returns `DriverError::Query("concurrency limit reached")`. The driver does not queue — backpressure propagates to the caller immediately.

Acquisition order: global first, then per-command. This prevents deadlocks (consistent ordering). On failure, the global semaphore is released before returning the error.

---

## 13. Output Handling

### 13.1 Stdout

Stdout is the result channel. The driver reads up to `max_stdout_bytes` from the child's stdout pipe. If the limit is exceeded, the driver kills the process group and returns an error.

Stdout must be valid JSON. The driver parses it with `serde_json::from_slice`. The parsed value is returned as the query result.

### 13.2 Stderr

Stderr is the error/diagnostic channel. The driver reads up to 64KB from stderr. On non-zero exit, stderr content is included in the error message. On zero exit, stderr is discarded (scripts may emit warnings to stderr without causing failure).

### 13.3 Exit Code

| Exit Code | Behavior |
|---|---|
| 0 + valid JSON stdout | Success — return parsed JSON |
| 0 + invalid JSON stdout | `DriverError::Query("command produced invalid JSON")` |
| 0 + empty stdout | `DriverError::Query("command produced no output")` |
| Non-zero | `DriverError::Query("command failed: exit <code>: <stderr first 1024 chars>")` |

### 13.4 Result Mapping

The parsed JSON from stdout is wrapped in a `QueryResult`:

```rust
QueryResult {
    rows: vec![],                // not used — result is in raw_value
    affected_rows: 0,
    last_insert_id: None,
    raw_value: Some(parsed_json),  // the script's JSON output
}
```

The handler receives the parsed JSON directly via `Rivers.view.query()` return value.

---

## 14. Error Model

All errors use the standard `DriverError` enum:

| Error | Condition |
|---|---|
| `DriverError::Connection(msg)` | Startup failure: `run_as_user` invalid, working directory missing, file not found, schema parse failure |
| `DriverError::Unsupported(msg)` | Unknown command name. `read` / `write` / `delete` operations. |
| `DriverError::Query(msg)` | Schema validation failed. Command timed out. Output overflow. Non-zero exit. Invalid JSON output. Empty output. Concurrency limit reached. Missing parameters. |
| `DriverError::Internal(msg)` | Integrity check failure (hash mismatch at runtime). Process spawn failure. Unexpected I/O error. |

### 14.1 Integrity Failure Distinction

Integrity failures use `DriverError::Internal`, not `DriverError::Query`. This distinction is intentional — integrity failures are infrastructure/security events, not application-level errors. They should trigger alerts, not retries.

---

## 15. Logging and Audit

### 15.1 Startup Logging

| Event | Level | Fields |
|---|---|---|
| Command registered | `INFO` | datasource, command, path, integrity_check, input_mode |
| Weak integrity mode | `WARN` | datasource, command, integrity_check, warning message |
| `env_clear = false` | `WARN` | datasource, command, msg |
| Startup hash verified | `INFO` | datasource, command, sha256 (first 16 chars) |
| Startup hash mismatch | `ERROR` | datasource, command, path, expected, actual |

### 15.2 Runtime Logging

| Event | Level | Fields |
|---|---|---|
| Command execution start | `INFO` | datasource, command, trace_id |
| Command execution success | `INFO` | datasource, command, trace_id, duration_ms, exit_code |
| Command execution failure | `ERROR` | datasource, command, trace_id, duration_ms, exit_code, error |
| Integrity check failure | `ERROR` | datasource, command, trace_id, expected, actual |
| Concurrency limit hit | `WARN` | datasource, command, trace_id |
| Timeout kill | `WARN` | datasource, command, trace_id, timeout_ms |
| Output overflow kill | `WARN` | datasource, command, trace_id, max_stdout_bytes |

### 15.3 Audit Trail

Every command execution is logged with the request `trace_id`. Combined with Rivers' structured logging, this provides a complete audit trail: who called what command, with what parameters (via the view-level request log), at what time, with what result.

Parameter values are **not** logged by the exec driver itself — they may contain sensitive data (IP ranges, credentials passed as args). The view-level request log handles parameter logging per the application's logging config.

---

## 16. Security Properties

| Property | Mechanism |
|---|---|
| No arbitrary command execution | Admin-declared allowlist — only named commands with configured paths execute |
| No tampered binary execution | SHA-256 pinning with configurable check frequency |
| No privilege escalation | `run_as_user` enforced via `setuid`/`setgid`; must not be root |
| No shell injection | `tokio::process::Command` arg array — no shell involved |
| No argument injection | Fixed `args_template` structure — handler controls values, not shape |
| No environment leakage | `env_clear = true` default; explicit allowlist for inherited vars |
| No orphaned processes | `setsid` + `SIGKILL` to process group on timeout/overflow |
| No memory exhaustion via output | `max_stdout_bytes` cap with process kill on overflow |
| No fork bombs | Global + per-command concurrency semaphores |
| No unvalidated input | JSON Schema validation before process spawn |
| No credential leakage from errors | Stderr included in errors is truncated to 1024 chars; driver never logs parameter values |

### 16.1 Threat Model

| Threat | Mitigation |
|---|---|
| Malicious handler input | JSON Schema validation. Template interpolation prevents structural injection. |
| Script replacement on disk | SHA-256 integrity check (configurable frequency). File permissions + ownership. |
| Script escape / privilege escalation | Process runs as restricted user. File capabilities for specific needs. |
| Resource exhaustion | Timeouts, output caps, concurrency limits. Process group kill. |
| Information disclosure via env | `env_clear` default. Explicit allowlist. |
| TOCTOU between hash check and exec | Acceptable risk for `each_time` mode (microsecond window). File ownership controls. See §16.2. |

### 16.2 TOCTOU Mitigation

Between hash verification and `execve`, a theoretical window exists where the file could be swapped. Practical mitigations:

1. **File ownership**: scripts owned by root or deploy user, mode `0555`. `run_as_user` and `riversd` user cannot write.
2. **Immutable attribute** (optional hardening): `chattr +i` on script files.
3. **Detection on next check**: even if a TOCTOU race succeeds once, the next integrity check catches the modified file.

Document these as operational hardening recommendations, not as framework-enforced guarantees.

---

## 17. Operational Guidance

### 17.1 Hash Management

When a script is updated, the admin must update the `sha256` value in the datasource config.

**Manual workflow:**
```bash
sha256sum /usr/lib/rivers/scripts/netscan.py
# a1b2c3d4e5f6... /usr/lib/rivers/scripts/netscan.py
# Copy hash into TOML config, reload
```

**CLI helper:**
```bash
riversctl exec hash /usr/lib/rivers/scripts/netscan.py
# Output:
# sha256 = "a1b2c3d4e5f67890abcdef..."
```

Hash updates are always an explicit admin action. The driver never auto-updates hashes.

### 17.2 Recommended File Layout

```
/usr/lib/rivers/scripts/         # script/binary directory (root-owned, 0555)
    netscan.py
    dns_lookup.sh
    cert_check.py

/etc/rivers/exec-schemas/        # JSON Schema files (root-owned, 0444)
    netscan_args.json
    dns_args.json
    cert_args.json

/var/rivers/exec-scratch/        # working directory (rivers-exec-owned, 0700)
```

### 17.3 Script Contract

Scripts must follow this I/O contract:

- **Input**: read JSON from stdin (stdin mode) and/or parse argv (args mode).
- **Output**: write a single JSON document to stdout on success.
- **Errors**: write diagnostic output to stderr. Exit with non-zero code.
- **No interactivity**: scripts must not read from TTY or prompt for input.
- **Deterministic output**: given the same input, scripts should produce the same output structure (values may differ).

### 17.4 `riversctl exec` Commands

| Command | Description |
|---|---|
| `riversctl exec hash <path>` | Print SHA-256 hash of file in TOML-ready format |
| `riversctl exec verify <datasource>` | Verify all command hashes for a datasource against current files on disk |
| `riversctl exec list <datasource>` | List all declared commands with path, hash (first 16 chars), input mode, integrity mode |

---

## 18. Handler API Surface

From the handler's perspective, the ExecDriver is just another datasource:

```typescript
export async function onScanNetwork(req: Rivers.Request): Promise<Rivers.Response> {
    const results = await Rivers.view.query("ops_tools", {
        command: "network_scan",
        args: {
            cidr:  req.body.cidr,
            ports: req.body.ports,
        }
    });

    return { status: 200, body: results };
}
```

The handler does not know:
- That a script is being executed
- What language the script is written in
- What integrity mode is configured
- What concurrency limits apply
- What OS user runs the script

These are all admin-side concerns configured in the datasource TOML. The handler is a consumer of structured query results.

### 18.1 View Declaration

```toml
[api.views.scan_network]
path       = "/api/ops/scan"
view_type  = "Rest"
datasources = ["ops_tools"]

methods.POST.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/ops.ts",
    entrypoint_function = "onScanNetwork",
    resources           = ["ops_tools"]
}}
```

Standard view declaration. The `ops_tools` datasource token is injected into the handler's capability set via the normal ProcessPool injection model. The handler accesses it via `Rivers.view.query()` with the datasource alias.

---

## 19. Plugin Registration

The ExecDriver registers as a `DatabaseDriver` via the standard plugin ABI:

```rust
use rivers_driver_sdk::prelude::*;

pub struct ExecDriver { /* ... */ }

#[async_trait]
impl DatabaseDriver for ExecDriver {
    fn name(&self) -> &str { "rivers-exec" }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        Ok(Box::new(ExecConnection::new(params).await?))
    }
}

#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    rivers_driver_sdk::ABI_VERSION
}

#[no_mangle]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(ExecDriver));
}
```

Datasource config references the plugin with `driver = "plugin:rivers-exec"`.

---

## Appendix A: Complete Configuration Example

```toml
[datasources.ops_tools]
driver = "plugin:rivers-exec"

run_as_user        = "rivers-exec"
working_directory  = "/var/rivers/exec-scratch"
default_timeout_ms = 30000
max_stdout_bytes   = 5242880
max_concurrent     = 10
integrity_check    = "each_time"

[datasources.ops_tools.commands.network_scan]
path            = "/usr/lib/rivers/scripts/netscan.py"
sha256          = "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode      = "stdin"
args_schema     = "exec_schemas/netscan_args.json"
timeout_ms      = 60000
max_stdout_bytes = 10485760
max_concurrent  = 3
integrity_check = "every:50"
env_clear       = true
env_allow       = ["PATH", "HOME"]
env_set         = { SCAN_LOG = "/var/log/rivers/scan.log" }

[datasources.ops_tools.commands.dns_lookup]
path            = "/usr/lib/rivers/scripts/dns_lookup.sh"
sha256          = "b2c3d4e5f6a17890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode      = "args"
args_template   = ["{domain}", "--type", "{record_type}", "--timeout", "{timeout}"]
args_schema     = "exec_schemas/dns_args.json"
timeout_ms      = 10000
integrity_check = "startup_only"
env_clear       = true
env_allow       = ["PATH"]

[datasources.ops_tools.commands.bulk_scan]
path            = "/usr/lib/rivers/scripts/bulk_scan.py"
sha256          = "c3d4e5f6a1b27890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode      = "both"
args_template   = ["--mode", "{mode}", "--timeout", "{timeout}"]
stdin_key       = "targets"
args_schema     = "exec_schemas/bulk_scan_args.json"
timeout_ms      = 120000
max_concurrent  = 2
env_clear       = true
env_allow       = ["PATH", "HOME"]

[datasources.ops_tools.commands.cert_check]
path            = "/usr/lib/rivers/scripts/cert_check.py"
sha256          = "d4e5f6a1b2c37890abcdef1234567890abcdef1234567890abcdef1234567890"
input_mode      = "stdin"
args_schema     = "exec_schemas/cert_args.json"
timeout_ms      = 15000
env_clear       = true
env_allow       = ["PATH"]
```
