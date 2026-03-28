# Tasks — ExecDriver Plugin

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the ExecDriver plugin — controlled invocation of admin-declared, integrity-verified external commands from CodeComponent handlers.

**Spec:** `docs/rivers-exec-driver-spec.md`

**Architecture:** New `rivers-plugin-exec` crate (cdylib + rlib) implements the `DatabaseDriver` trait from `rivers-driver-sdk`. Handlers invoke commands via the standard `Rivers.view.query("datasource", { command, args })` pattern. The driver enforces an 11-step pipeline: command lookup, schema validation, SHA-256 integrity check, semaphore acquisition, process spawn (privilege-dropped, env-controlled), bounded I/O with timeout, JSON result parsing.

**Tech Stack:** Rust, `tokio::process::Command`, `sha2` crate, `jsonschema` crate, `tokio::sync::Semaphore`

**Security model:** This is a controlled RCE service. Only admin-declared commands run. No shell involved. All process spawning uses `tokio::process::Command` with an explicit argument array. No shell interpretation at any point.

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `crates/rivers-plugin-exec/Cargo.toml` | Plugin crate manifest (cdylib + rlib) |
| `crates/rivers-plugin-exec/src/lib.rs` | Plugin registration (C ABI exports), ExecDriver struct |
| `crates/rivers-plugin-exec/src/config.rs` | ExecConfig, CommandConfig — TOML config parsing |
| `crates/rivers-plugin-exec/src/integrity.rs` | SHA-256 integrity model (three check modes) |
| `crates/rivers-plugin-exec/src/template.rs` | Argument template engine (placeholder interpolation) |
| `crates/rivers-plugin-exec/src/executor.rs` | Process spawning — tokio::process::Command, I/O, timeout, kill |
| `crates/rivers-plugin-exec/src/connection.rs` | Connection trait impl — the 11-step pipeline |

### Modified Files

| File | Changes |
|------|---------|
| `Cargo.toml` (workspace root) | Add `rivers-plugin-exec` to members, add `jsonschema` to workspace deps |
| `crates/riversd/Cargo.toml` | Add `rivers-plugin-exec` to optional static-plugins list |
| `crates/riversctl/src/main.rs` | Add `riversctl exec hash/verify/list` subcommands |
| `docs/guide/cli.md` | Add ExecDriver section, `riversctl exec` commands |
| `docs/guide/developer.md` | Add ExecDriver datasource usage pattern |
| `docs/guide/rivers-app-development.md` | Add ExecDriver config in resources.toml / app.toml |
| `docs/guide/rivers-skill.md` | Add exec driver to datasource drivers table |
| `docs/guide/admin.md` | Add operational guidance for ExecDriver |

---

## 1. Crate Skeleton + Config Types

Create `rivers-plugin-exec` crate with config structs. Follows the plugin pattern from `rivers-plugin-mongodb`.

**Create:** `crates/rivers-plugin-exec/Cargo.toml`, `src/lib.rs`, `src/config.rs`
**Modify:** `Cargo.toml` (workspace root), `crates/riversd/Cargo.toml`
**Reference:** `crates/rivers-plugin-mongodb/` for plugin crate pattern

### Config Types (spec sections 4-5)

```rust
/// Global ExecDriver datasource configuration (spec section 4.1).
pub struct ExecConfig {
    pub run_as_user: String,
    pub working_directory: PathBuf,        // default: /tmp
    pub default_timeout_ms: u64,           // default: 30000
    pub max_stdout_bytes: usize,           // default: 5242880 (5MB)
    pub max_concurrent: usize,             // default: 10
    pub integrity_check: IntegrityMode,    // default: EachTime
    pub commands: HashMap<String, CommandConfig>,
}

/// Per-command configuration (spec section 5).
pub struct CommandConfig {
    pub path: PathBuf,                     // absolute path to binary/script
    pub sha256: String,                    // hex-encoded SHA-256 digest
    pub input_mode: InputMode,             // stdin | args | both
    pub args_template: Option<Vec<String>>,
    pub stdin_key: Option<String>,
    pub args_schema: Option<PathBuf>,      // path to JSON Schema file
    pub timeout_ms: Option<u64>,
    pub max_stdout_bytes: Option<usize>,
    pub max_concurrent: Option<usize>,
    pub integrity_check: Option<IntegrityMode>,
    pub env_clear: bool,                   // default: true
    pub env_allow: Vec<String>,
    pub env_set: HashMap<String, String>,
}

pub enum IntegrityMode { EachTime, StartupOnly, Every(u64) }
pub enum InputMode { Stdin, Args, Both }
```

- [ ] **T1.1** Create `crates/rivers-plugin-exec/Cargo.toml` — `crate-type = ["cdylib", "rlib"]`, deps: `rivers-driver-sdk`, `async-trait`, `tokio` (process + io + time + sync features), `serde`, `serde_json`, `sha2`, `tracing`, `jsonschema`
- [ ] **T1.2** Add `rivers-plugin-exec` to workspace members; add `jsonschema = "0.28"` to workspace deps
- [ ] **T1.3** Add `rivers-plugin-exec` to `crates/riversd/Cargo.toml` static-plugins feature list
- [ ] **T1.4** Implement `ExecConfig` and `CommandConfig` with deserialization from `ConnectionParams.options`
- [ ] **T1.5** Implement `IntegrityMode` parsing from string (`"each_time"`, `"startup_only"`, `"every:N"`)
- [ ] **T1.6** Implement `InputMode` parsing from string (`"stdin"`, `"args"`, `"both"`)
- [ ] **T1.7** Implement startup validation (spec section 4.2): `run_as_user` resolves via `nix::unistd::User::from_name()`, not UID 0, working_directory exists, per-command validation (spec section 5.1: absolute path, file exists+executable, args_template required for args/both modes, stdin_key required for both mode)
- [ ] **T1.8** Write unit tests for config parsing and startup validation

**Validation:**
```bash
cargo build -p rivers-plugin-exec
cargo test -p rivers-plugin-exec
```

---

## 2. SHA-256 Integrity Model

Implement the three integrity check modes from spec section 6.

**Create:** `src/integrity.rs`

```rust
pub struct CommandIntegrity {
    mode: IntegrityMode,
    exec_count: AtomicU64,
    pinned_hash: [u8; 32],
}
```

- [ ] **T2.1** Implement `hash_file(path) -> Result<[u8; 32]>` using `sha2::Sha256` (not ring — sha2 is in workspace deps)
- [ ] **T2.2** Implement `CommandIntegrity::new(mode, pinned_hash)` and `should_check(&self) -> bool` per spec section 6.2
- [ ] **T2.3** Implement `verify(path) -> Result<(), DriverError>` that hashes file and compares to pinned
- [ ] **T2.4** Implement startup hash verification for all commands (mismatch -> refuse to start with `DriverError::Connection`)
- [ ] **T2.5** Implement logging per spec section 6.5: `each_time` at INFO, others at WARN with tamper window message
- [ ] **T2.6** Write unit tests: hash computation, each_time always true, startup_only always false, every:N modular, mismatch detection

**Validation:**
```bash
cargo test -p rivers-plugin-exec -- integrity
```

---

## 3. Argument Template Engine

Implement placeholder interpolation from spec section 8.

**Create:** `src/template.rs`

- [ ] **T3.1** Implement `interpolate(template: &[String], params: &HashMap<String, serde_json::Value>) -> Result<Vec<String>, DriverError>` per spec section 8.2 rules:
  - `{key}` replaced with string value of corresponding param
  - Missing key -> `DriverError::Query("missing required parameter: '<key>'")`
  - Array/object values -> `DriverError::Query("parameter '<key>' must be a scalar value for args template")`
  - Numbers -> decimal string, booleans -> "true"/"false"
  - Each placeholder produces exactly one argument (no splitting)
  - Extra keys silently ignored
- [ ] **T3.2** Write unit tests: basic interpolation, missing key error, scalar-only enforcement, extra keys ignored, special characters pass through literally

**Validation:**
```bash
cargo test -p rivers-plugin-exec -- template
```

---

## 4. JSON Schema Validation

Integrate JSON Schema validation from spec section 9.

**Modify:** `src/config.rs` or new `src/schema.rs`

- [ ] **T4.1** Load and parse JSON Schema files at startup from `args_schema` paths
- [ ] **T4.2** Implement `validate_args(schema, args) -> Result<(), DriverError>` — returns `DriverError::Query("schema validation failed: <details>")` on failure
- [ ] **T4.3** Validation timing: after command lookup, before integrity check (spec section 9.2)
- [ ] **T4.4** Write unit tests with the example schema from spec section 9.3 (CIDR + ports validation)

**Validation:**
```bash
cargo test -p rivers-plugin-exec -- schema
```

---

## 5. Process Spawning

Core process spawning with full isolation from spec sections 10-11. All process spawning uses `tokio::process::Command` with an explicit argument array. No shell involved.

**Create:** `src/executor.rs`

- [ ] **T5.1** Build `tokio::process::Command` per spec section 10 step 6:
  - Path from CommandConfig
  - Args from template interpolation (args/both mode)
  - UID/GID from resolved `run_as_user` (via `nix::unistd::User::from_name`)
  - CWD from `working_directory`
  - Environment from `env_clear` + `env_allow` + `env_set` (spec section 11.2)
  - stdin: piped (stdin/both) or null (args)
  - stdout/stderr: piped
  - `kill_on_drop(true)`
  - Process group: `pre_exec(|| { libc::setsid(); Ok(()) })` for spec section 11.3

- [ ] **T5.2** Write stdin (spec section 10 step 8): serialize params as JSON, write to child stdin, close stdin. For `both` mode: extract `stdin_key` value, send remaining params on args, stdin_key value on stdin.

- [ ] **T5.3** Bounded read with timeout (spec section 10 step 9):
  - `tokio::time::timeout(timeout_ms)`
  - Read stdout up to `max_stdout_bytes` (kill process group on overflow)
  - Read stderr up to 64KB
  - Wait for exit

- [ ] **T5.4** Evaluate result (spec section 10 step 10):
  - Timeout -> SIGKILL process group -> `DriverError::Query("command timed out")`
  - Stdout overflow -> SIGKILL -> `DriverError::Query("output exceeded limit")`
  - Exit 0 + valid JSON -> success
  - Exit 0 + invalid JSON -> `DriverError::Query("command produced invalid JSON")`
  - Exit 0 + empty -> `DriverError::Query("command produced no output")`
  - Non-zero -> `DriverError::Query("command failed: exit <code>: <stderr first 1024 chars>")`

- [ ] **T5.5** Write unit tests: mock scripts (echo stdin back as JSON, exit codes, timeout simulation)

**Validation:**
```bash
cargo test -p rivers-plugin-exec -- executor
```

---

## 6. Concurrency Control

Two-layer semaphore system from spec section 12.

**Modify:** `src/connection.rs` or `src/executor.rs`

- [ ] **T6.1** Create global `tokio::sync::Semaphore` from `max_concurrent` config
- [ ] **T6.2** Create per-command `Semaphore` from command-level `max_concurrent` config
- [ ] **T6.3** Acquisition order: global first, then per-command (spec section 12.3). On failure, release global before returning error.
- [ ] **T6.4** No queuing — return `DriverError::Query("concurrency limit reached")` immediately if full (use `try_acquire`)
- [ ] **T6.5** Write unit tests: concurrent runs respect limits, over-limit returns error

**Validation:**
```bash
cargo test -p rivers-plugin-exec -- concurrency
```

---

## 7. Connection Trait — Wire the 11-Step Pipeline

Implement `DatabaseDriver` + `Connection` traits, composing all pieces.

**Create:** `src/connection.rs`
**Modify:** `src/lib.rs`

- [ ] **T7.1** Implement `ExecDriver` struct with `DatabaseDriver` trait:
  - `fn name() -> &str` returns `"rivers-exec"`
  - `async fn connect(params) -> Result<Box<dyn Connection>>` — parses config, runs startup validation, hashes all commands, builds semaphores

- [ ] **T7.2** Implement `ExecConnection` struct with `Connection` trait:
  - `async fn execute(query) -> Result<QueryResult>` — the 11-step pipeline
  - `async fn ping() -> Ok(())` (no-op, always succeeds)
  - Route: `query.operation == "query"` -> pipeline, everything else -> `DriverError::Unsupported`

- [ ] **T7.3** Pipeline implementation in `execute()`:
  1. Extract `command` from `query.parameters` or `query.statement`
  2. Lookup `CommandConfig` by name
  3. Validate args against schema (if `args_schema`)
  4. Integrity check (mode-dependent)
  5. Acquire semaphores
  6-9. Spawn process (delegate to executor)
  10. Parse result
  11. Release semaphores

- [ ] **T7.4** Map result to `QueryResult` per spec section 13.4 — parsed JSON in appropriate result format (check if `QueryResult` has a `raw_value` field; if not, wrap in a single-row result with key `"result"`)

- [ ] **T7.5** Write integration test: full pipeline with a real script

**Validation:**
```bash
cargo test -p rivers-plugin-exec
```

---

## 8. Plugin Registration (C ABI Exports)

Standard plugin ABI from spec section 19.

**Modify:** `src/lib.rs`

- [ ] **T8.1** Add `#[cfg(feature = "plugin-exports")]` block with:
  - `_rivers_abi_version() -> u32` returning `ABI_VERSION`
  - `_rivers_register_driver(registrar)` registering `Arc::new(ExecDriver)`
- [ ] **T8.2** Verify plugin loads in static mode (`static-plugins` feature)

**Validation:**
```bash
cargo build -p rivers-plugin-exec
cargo build -p riversd --features static-plugins
```

---

## 9. riversctl Extensions

Add `exec` subcommands to `riversctl` from spec section 17.4.

**Modify:** `crates/riversctl/src/main.rs`

- [ ] **T9.1** Add `exec hash <path>` — prints SHA-256 of file in TOML-ready format: `sha256 = "hex..."`
- [ ] **T9.2** Add `exec verify <datasource>` — loads datasource config, verifies all command hashes against current files on disk
- [ ] **T9.3** Add `exec list <datasource>` — lists all declared commands with path, hash (first 16 chars), input mode, integrity mode

**Validation:**
```bash
cargo build -p riversctl
riversctl exec hash /bin/echo
# Output: sha256 = "..."
```

---

## 10. Integration Tests

End-to-end tests with real script invocation.

**Create:** `crates/rivers-plugin-exec/tests/integration_test.rs`

- [ ] **T10.1** Create test scripts in temp dir:
  - `echo_stdin.sh` — reads stdin JSON, echoes it back on stdout
  - `args_script.sh` — echoes argv as JSON array
  - `failing_script.sh` — exits non-zero with stderr message
  - `slow_script.sh` — sleeps, used for timeout tests
  - `large_output.sh` — outputs more than max_stdout_bytes

- [ ] **T10.2** Test stdin mode: send JSON params, receive JSON result
- [ ] **T10.3** Test args mode: template interpolation -> argv -> JSON result
- [ ] **T10.4** Test both mode: args + stdin combined
- [ ] **T10.5** Test integrity check: correct hash passes, modified file fails
- [ ] **T10.6** Test JSON Schema validation: valid params pass, invalid rejected
- [ ] **T10.7** Test timeout: slow script killed, error returned
- [ ] **T10.8** Test output overflow: large output killed, error returned
- [ ] **T10.9** Test non-zero exit: error includes stderr
- [ ] **T10.10** Test unknown command: `DriverError::Unsupported`
- [ ] **T10.11** Test concurrency limits: over-limit returns error

**Validation:**
```bash
cargo test -p rivers-plugin-exec
# All tests pass
```

---

## 11. Documentation Updates

Update all relevant guide docs for the ExecDriver.

### Task 11.1: `docs/guide/rivers-skill.md`

- [ ] Add `rivers-exec` to the datasource drivers table:
  ```
  | `rivers-exec` | Plugin | exec | Controlled invocation of admin-declared scripts/binaries |
  ```

### Task 11.2: `docs/guide/cli.md`

- [ ] Add `riversctl exec` commands section (hash, verify, list)
- [ ] Add exec driver mention in the `riversd` startup sequence

### Task 11.3: `docs/guide/developer.md`

- [ ] Add ExecDriver handler usage example:
  ```javascript
  var result = Rivers.view.query("ops_tools", {
      command: "network_scan",
      args: { cidr: "10.0.1.0/24", ports: [22, 80] }
  });
  ```
- [ ] Explain the handler's view: it's just another datasource, no knowledge of scripts

### Task 11.4: `docs/guide/rivers-app-development.md`

- [ ] Add ExecDriver datasource configuration section with full TOML example
- [ ] Document command declaration fields
- [ ] Document the three input modes (stdin, args, both)

### Task 11.5: `docs/guide/admin.md`

- [ ] Add operational guidance section for ExecDriver:
  - Hash management workflow (update script -> update sha256 in config -> reload)
  - Recommended file layout (`/usr/lib/rivers/scripts/`, `/etc/rivers/exec-schemas/`)
  - Security hardening (file capabilities, immutable attribute, run_as_user setup)
  - Script contract (stdin JSON -> stdout JSON, non-zero exit for errors)
  - Integrity check mode selection guidance

**Validation:**
```bash
# Review all modified docs for accuracy and completeness
```

---

## Acceptance Criteria

Per spec sections:

- [ ] AC1: Only admin-declared commands run — handler cannot specify arbitrary paths (spec 1.1)
- [ ] AC2: Commands pinned by SHA-256 hash — mismatch refuses invocation (spec 6)
- [ ] AC3: Three integrity check modes: `each_time`, `startup_only`, `every:N` (spec 6.1)
- [ ] AC4: Input validated against JSON Schema before process spawn (spec 9)
- [ ] AC5: Three input modes: `stdin`, `args`, `both` (spec 7)
- [ ] AC6: Argument template interpolation — fixed structure, no injection surface (spec 8)
- [ ] AC7: Process runs as `run_as_user` (not root) with controlled env (spec 11)
- [ ] AC8: No shell involved — `tokio::process::Command` arg array only (spec 8.3)
- [ ] AC9: Bounded stdout + timeout with process group kill (spec 10 steps 9-10)
- [ ] AC10: Global + per-command concurrency semaphores (spec 12)
- [ ] AC11: Structured logging with trace_id for audit trail (spec 15)
- [ ] AC12: Standard driver contract — handler uses `Rivers.view.query()` (spec 18)
- [ ] AC13: Plugin registration via standard ABI (spec 19)
- [ ] AC14: `riversctl exec hash/verify/list` commands (spec 17.4)
- [ ] AC15: Comprehensive error model using `DriverError` variants (spec 14)
