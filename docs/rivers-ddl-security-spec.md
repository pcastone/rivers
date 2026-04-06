# Rivers DDL Security Specification

**Document Type:** Specification Amendment  
**Scope:** DDL and admin operation prevention at driver level, per-driver admin operation denylists, DDL execution path for application init, admin whitelist  
**Status:** Design / Pre-Implementation  
**Amends:** `rivers-driver-spec.md`, `rivers-data-layer-spec.md`, `rivers-httpd-spec.md`, `rivers-application-spec.md`  
**Depends On:** Driver SDK, Connection trait, DataViewEngine, Application deployment lifecycle

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Design Principles](#2-design-principles)
3. [Three-Gate Enforcement Model](#3-three-gate-enforcement-model)
4. [Gate 1 — Driver-Level DDL Guard](#4-gate-1--driver-level-ddl-guard)
5. [Gate 2 — Execution Context Restriction](#5-gate-2--execution-context-restriction)
6. [Gate 3 — Admin DDL Whitelist](#6-gate-3--admin-ddl-whitelist)
7. [Application Init Handler](#7-application-init-handler)
8. [JS API Surface](#8-js-api-surface)
9. [Startup Sequence Changes](#9-startup-sequence-changes)
10. [Validation Rules](#10-validation-rules)
11. [Logging](#11-logging)
12. [Configuration Reference](#12-configuration-reference)
13. [Examples](#13-examples)
14. [Spec Amendment Index](#14-spec-amendment-index)

---

## 1. Problem Statement

### 1.1 SQL Injection — Not Vulnerable

Rivers is not vulnerable to classic SQL injection. All three SQL drivers (PostgreSQL, MySQL, SQLite) use native parameterized queries — `$1`/`?`/`:name` binding. User input from HTTP requests flows through `QueryValue` (typed enum) and never touches the SQL statement string. The statement itself always comes from TOML config, not user input.

### 1.2 DDL via Configuration — The Real Problem

The `Connection::execute()` method handles all operations indiscriminately — reads, writes, and DDL. The `Query.operation` field is inferred from the first token of the statement, and the driver dispatches without restriction. DDL operations (`CREATE TABLE`, `ALTER TABLE`, `DROP TABLE`, `TRUNCATE`) are classified as valid operations and execute without restriction.

An admin or anyone who can modify bundle config could define:

```toml
[data.dataviews.nuke]
datasource = "production_db"
query      = "DROP TABLE users CASCADE"
```

And it executes. This is the gap.

### 1.3 The `create` Ambiguity

The operation token `"create"` is ambiguous — it can mean `INSERT` (create a record) or `CREATE TABLE` (DDL). Operation-token-based filtering alone cannot distinguish between the two. Statement inspection is required.

### 1.4 Non-SQL Drivers — Same Problem, Different Shape

The DDL gap is not SQL-specific. Non-SQL drivers dispatch on operation tokens, not SQL statements. `is_ddl_statement()` only catches SQL patterns. Non-SQL drivers have their own admin/destructive operations that bypass it entirely:

| Driver | Admin/Destructive Operations |
|---|---|
| Redis | `FLUSHDB`, `FLUSHALL`, `CONFIG SET` |
| MongoDB | `dropCollection`, `dropDatabase`, `createIndex`, `dropIndex` |
| Elasticsearch | index deletion, mapping changes |
| Kafka | topic creation/deletion |
| RabbitMQ | exchange/queue declaration and deletion |
| NATS | stream creation/deletion |
| InfluxDB | bucket creation/deletion |

Each driver has its own concept of "schema-level" vs "data-level" operations. The guard must be driver-specific — there is no universal pattern that covers SQL statements and operation tokens simultaneously.

---

## 2. Design Principles

- **Lock it down at the driver.** The driver is the last gate before the backend. No config to misconfigure, no allowlist to forget. Admin operations do not execute through `Connection::execute()`. Period.
- **Each driver owns its own security boundary.** SQL drivers guard via statement inspection (`is_ddl_statement()`). Non-SQL drivers guard via per-driver admin operation denylists (`admin_operations()`). No shared registry to coordinate — each driver declares what it blocks.
- **Separate execution path for DDL.** `Connection::ddl_execute()` is a distinct method. It exists on the trait, but only the application init execution context can call it. This covers both SQL DDL and non-SQL admin operations.
- **Admin controls access.** The `ddl_whitelist` in `riversd.toml` scopes DDL permission to specific `database@appId` pairs. The app bundle cannot self-authorize.
- **Ops-tier config only.** The whitelist uses database names and app IDs — values ops knows without lockbox access or app-internal knowledge. Clean separation between admin tier (lockbox, sensitive data) and operations tier (config, non-sensitive data).

---

## 3. Three-Gate Enforcement Model

All three gates MUST pass for a DDL or admin operation to execute. Any single gate failing is a hard reject.

```
DDL / Admin Operation
    │
    ├─ Gate 1: Driver — execute() detects admin operation → DriverError::Forbidden
    │          SQL drivers: is_ddl_statement() on statement text
    │          Non-SQL drivers: admin_operations() denylist on query.operation
    │          Only ddl_execute() accepts admin operations
    │
    ├─ Gate 2: Context — ddl_execute() only callable from ApplicationInit
    │          View handlers do not have DDL on their API surface
    │
    └─ Gate 3: Whitelist — riversd.toml ddl_whitelist must contain
               "{database}@{appId}" for the target datasource + app pair
```

---

## 4. Gate 1 — Driver-Level Admin Operation Guard

### 4.1 Two Guard Mechanisms

SQL and non-SQL drivers use different detection strategies. Both converge to the same result: `execute()` rejects, `ddl_execute()` accepts.

| Driver Family | Guard Mechanism | Detection Target |
|---|---|---|
| SQL (postgres, mysql, sqlite) | `is_ddl_statement()` — statement text inspection | `CREATE`, `ALTER`, `DROP`, `TRUNCATE` prefixes |
| Non-SQL (redis, mongodb, kafka, etc.) | `admin_operations()` — operation token denylist | Driver-declared admin operation strings |

### 4.2 SQL Guard — `is_ddl_statement()`

A shared utility function for SQL drivers:

```rust
/// Returns true if the statement is a DDL operation.
/// Checks the actual statement text, not the inferred operation token.
pub fn is_ddl_statement(statement: &str) -> bool {
    let upper = statement.trim_start().to_uppercase();
    upper.starts_with("CREATE ")
        || upper.starts_with("ALTER ")
        || upper.starts_with("DROP ")
        || upper.starts_with("TRUNCATE ")
}
```

This function lives in `crates/rivers-driver-sdk/src/lib.rs` and is available to all drivers including plugins.

### 4.3 Non-SQL Guard — `admin_operations()`

Each driver declares which operation tokens are admin-tier. The `Connection` trait includes a method that returns the driver's admin operation denylist:

```rust
/// Returns the list of operation tokens this driver considers admin/DDL-like.
/// execute() MUST reject any query whose operation matches this list.
/// ddl_execute() accepts these operations.
/// Default: empty (no admin operations — appropriate for drivers with no schema concept).
fn admin_operations(&self) -> &[&str] {
    &[]
}
```

Drivers override this to declare their admin operations. The list is static per driver — it does not change at runtime.

### 4.4 Per-Driver Admin Operation Denylists

#### Redis

```rust
fn admin_operations(&self) -> &[&str] {
    &["flushdb", "flushall", "config_set", "config_rewrite"]
}
```

`CONFIG SET` and `CONFIG REWRITE` are server-level mutations. `FLUSHDB`/`FLUSHALL` are destructive wipes. None of these are in the current operation map (Section 4.1 of `rivers-driver-spec.md`), but the guard prevents them from being added to `execute()` without going through `ddl_execute()`.

#### MongoDB (plugin)

```rust
fn admin_operations(&self) -> &[&str] {
    &[
        "create_collection", "drop_collection", "drop_database",
        "create_index", "drop_index", "rename_collection",
    ]
}
```

#### Elasticsearch (plugin)

```rust
fn admin_operations(&self) -> &[&str] {
    &[
        "create_index", "delete_index",
        "put_mapping", "update_settings",
    ]
}
```

#### Kafka (plugin)

```rust
fn admin_operations(&self) -> &[&str] {
    &["create_topic", "delete_topic", "alter_topic"]
}
```

#### RabbitMQ (plugin)

```rust
fn admin_operations(&self) -> &[&str] {
    &[
        "declare_exchange", "delete_exchange",
        "declare_queue", "delete_queue",
        "bind_queue", "unbind_queue",
    ]
}
```

#### NATS (plugin)

```rust
fn admin_operations(&self) -> &[&str] {
    &["create_stream", "delete_stream", "update_stream"]
}
```

#### InfluxDB (plugin)

```rust
fn admin_operations(&self) -> &[&str] {
    &["create_bucket", "delete_bucket"]
}
```

#### Memcached, Faker, EventBus, rps-client

Default (empty list). These drivers have no schema-level operations.

### 4.5 New DriverError Variant

```rust
pub enum DriverError {
    UnknownDriver(String),
    Connection(String),
    Query(String),
    Transaction(String),
    Unsupported(String),
    Internal(String),
    Forbidden(String),       // NEW — operation rejected by security policy
}
```

`Forbidden` is semantically distinct from `Unsupported`. `Unsupported` means the driver doesn't implement the operation. `Forbidden` means the operation is implemented but rejected by security policy.

### 4.6 Connection Trait Amendment

```rust
#[async_trait]
pub trait Connection: Send + Sync {
    /// Execute a DML query (SELECT, INSERT, UPDATE, DELETE, or equivalent).
    /// MUST reject DDL statements (SQL) and admin operations (non-SQL)
    /// with DriverError::Forbidden.
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError>;

    /// Execute a DDL statement or admin operation.
    /// Only callable from ApplicationInit context.
    /// The caller (DataViewEngine) is responsible for whitelist enforcement.
    async fn ddl_execute(&mut self, query: &Query) -> Result<QueryResult, DriverError>;

    /// Returns operation tokens this driver considers admin/DDL-like.
    /// execute() MUST reject queries matching these operations.
    /// Default: empty slice.
    fn admin_operations(&self) -> &[&str] {
        &[]
    }

    async fn ping(&mut self) -> Result<(), DriverError>;
    fn driver_name(&self) -> &str;
}
```

### 4.7 Default Trait Implementation — `ddl_execute()`

`ddl_execute()` has a default implementation that returns `Unsupported`. Drivers that support admin operations override it.

```rust
async fn ddl_execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
    Err(DriverError::Unsupported(
        format!("{} does not support DDL/admin operations", self.driver_name())
    ))
}
```

### 4.8 `execute()` Guard — Combined Check

Every `Connection::execute()` implementation MUST check for both SQL DDL and admin operations:

```rust
async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
    // SQL DDL guard — statement text inspection
    if is_ddl_statement(&query.statement) {
        return Err(DriverError::Forbidden(
            format!(
                "DDL statement rejected on execute() — use application init handler. statement prefix: '{}'",
                &query.statement.chars().take(40).collect::<String>()
            )
        ));
    }

    // Non-SQL admin operation guard — operation token check
    if self.admin_operations().contains(&query.operation.as_str()) {
        return Err(DriverError::Forbidden(
            format!(
                "admin operation '{}' rejected on execute() — use application init handler",
                query.operation
            )
        ));
    }

    // ... normal dispatch
}
```

The SDK exports a convenience function that performs both checks, so plugin authors don't need to reimplement:

```rust
/// Check if a query is an admin operation (DDL or driver-declared admin op).
/// Returns Some(reason) if blocked, None if allowed.
pub fn check_admin_guard(query: &Query, admin_ops: &[&str]) -> Option<String> {
    if is_ddl_statement(&query.statement) {
        return Some(format!(
            "DDL statement rejected — statement prefix: '{}'",
            &query.statement.chars().take(40).collect::<String>()
        ));
    }
    if admin_ops.contains(&query.operation.as_str()) {
        return Some(format!(
            "admin operation '{}' rejected",
            query.operation
        ));
    }
    None
}
```

Plugin `execute()` implementations use it as:

```rust
async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
    if let Some(reason) = check_admin_guard(query, self.admin_operations()) {
        return Err(DriverError::Forbidden(
            format!("{} — use application init handler", reason)
        ));
    }
    // ... normal dispatch
}
```

### 4.9 Plugin Driver Obligation

Plugin drivers that implement `Connection` MUST:

1. Override `admin_operations()` to declare their admin operation tokens
2. Call `check_admin_guard()` (or equivalent) at the top of `execute()`

The `check_admin_guard()` utility is exported from the SDK for this purpose. Plugins that do not guard `execute()` against admin operations are considered non-compliant. Rivers cannot enforce this at the ABI level — it is a contractual obligation documented in the plugin development guide.

### 4.10 Driver Support Matrix

| Driver | Guard Type | `admin_operations()` | `ddl_execute()` |
|---|---|---|---|
| `postgres` | SQL statement | `&[]` (uses `is_ddl_statement()`) | Override — full DDL |
| `mysql` | SQL statement | `&[]` (uses `is_ddl_statement()`) | Override — full DDL |
| `sqlite` | SQL statement | `&[]` (uses `is_ddl_statement()`) | Override — full DDL |
| `redis` | Operation token | `flushdb`, `flushall`, `config_set`, `config_rewrite` | Override — executes admin commands |
| `mongodb` (plugin) | Operation token | `create_collection`, `drop_collection`, `drop_database`, `create_index`, `drop_index`, `rename_collection` | Override — admin API calls |
| `elasticsearch` (plugin) | Operation token | `create_index`, `delete_index`, `put_mapping`, `update_settings` | Override — index management |
| `kafka` (plugin) | Operation token | `create_topic`, `delete_topic`, `alter_topic` | Override — topic admin |
| `rabbitmq` (plugin) | Operation token | `declare_exchange`, `delete_exchange`, `declare_queue`, `delete_queue`, `bind_queue`, `unbind_queue` | Override — exchange/queue management |
| `nats` (plugin) | Operation token | `create_stream`, `delete_stream`, `update_stream` | Override — JetStream admin |
| `influxdb` (plugin) | Operation token | `create_bucket`, `delete_bucket` | Override — bucket management |
| `memcached` | None | `&[]` | Default (Unsupported) |
| `faker` | None | `&[]` | Default (Unsupported) |
| `eventbus` | None | `&[]` | Default (Unsupported) |
| `rps-client` | None | `&[]` | Default (Unsupported) |

---

## 5. Gate 2 — Execution Context Restriction

### 5.1 Execution Contexts

Rivers defines two execution contexts for datasource operations:

| Context | When | DDL Allowed |
|---|---|---|
| `ApplicationInit` | App startup, before the app enters RUNNING | Yes (if whitelisted) |
| `ViewRequest` | Request dispatch through the handler pipeline | No — never |

The execution context is set by the caller (DataViewEngine or application lifecycle manager) and is not controllable by handler code.

### 5.2 DataViewEngine Enforcement

The `DataViewEngine` is the sole caller of `Connection::execute()` and `Connection::ddl_execute()`. It determines which method to call based on the execution context.

In `ViewRequest` context, the DataViewEngine calls `execute()` only. The `ddl_execute()` method is never called from this context regardless of the statement or operation. If `execute()` detects a DDL statement or admin operation internally (Gate 1), it rejects.

In `ApplicationInit` context, the DataViewEngine checks the query. If `is_ddl_statement()` returns true or the operation matches the driver's `admin_operations()` list, it calls `ddl_execute()` after verifying the whitelist (Gate 3). If the query is a normal DML/data operation, it calls `execute()` normally.

### 5.3 JS API Surface Separation

View handlers receive `Rivers.db` and `Rivers.view` — neither exposes DDL.

The application init handler receives `Rivers.app.ddl()` — a method that only exists in the init context. It is not hidden behind a flag. It is not a restricted method on `Rivers.db`. It simply does not exist in the view handler API surface.

See [Section 8](#8-js-api-surface) for the full API contract.

---

## 6. Gate 3 — Admin DDL Whitelist

### 6.1 Configuration

The `ddl_whitelist` is an array of `"{database}@{appId}"` strings in `riversd.toml`, under the `[security]` section.

```toml
[security]
ddl_whitelist = [
    "orders_db@f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "sessions_db@a3f8c21d-9b44-4e71-b823-1c04d5e6f789",
]
```

### 6.2 Identifier Resolution

- **`database`** — the actual database name from `ConnectionParams.database`, resolved at connection time. This is the name ops provisioned in the database server. Ops knows this because they created it.
- **`appId`** — the app's UUID from `manifest.json`. Ops can discover this via the admin API (`GET /admin/deployments`) or from the bundle manifest.

Neither value requires lockbox access, alias knowledge, or familiarity with the app's internal datasource naming.

### 6.3 Whitelist Check

At `ddl_execute()` dispatch time:

1. The DataViewEngine resolves the datasource's `ConnectionParams.database` field
2. The DataViewEngine resolves the current app's `appId` from the deployment context
3. It constructs the key: `"{database}@{appId}"`
4. It checks `ddl_whitelist` for an exact match
5. No match → `DriverError::Forbidden`

### 6.4 Empty Whitelist

If `ddl_whitelist` is absent or empty, no application can execute DDL. This is the safe default. DDL is opt-in at the infrastructure level.

### 6.5 Wildcard — Not Supported

There is no wildcard syntax. Every database+app pair must be explicitly listed. This is deliberate — DDL authorization should be precise, not broad.

---

## 7. Application Init Handler

### 7.1 Concept

The application init handler is a CodeComponent that runs once during application startup, after resources are resolved and before the app enters RUNNING state. It is the sole execution context where DDL is permitted.

### 7.2 Manifest Declaration

Declared in the app's `manifest.json`:

```json
{
  "appName": "orders-service",
  "type": "app-service",
  "appId": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "entryPoint": "http://0.0.0.0:9001",
  "init": {
    "module": "handlers/init.ts",
    "entrypoint": "initialize"
  }
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `init` | object | no | Application init handler declaration |
| `init.module` | string | yes (if `init` present) | Path to CodeComponent module relative to `libraries/` |
| `init.entrypoint` | string | yes (if `init` present) | Exported function name |

### 7.3 Init Handler Contract

The init handler is an async function that receives `Rivers.InitContext`:

```typescript
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    // SQL DDL — statement-based
    await ctx.ddl("orders_db", "CREATE TABLE IF NOT EXISTS orders (id SERIAL PRIMARY KEY, status TEXT)");

    // Non-SQL admin — operation-based
    await ctx.admin("search_db", "create_index", { name: "orders_idx", mappings: { ... } });

    // DML also available — seed data, verify state
    await ctx.query("orders_db", "SELECT COUNT(*) as count FROM orders");
}
```

### 7.4 Init Handler Lifecycle

```
Resource resolution complete
    │
    ├─ Pool connections established
    ├─ DataView registry populated
    │
    ▼
Init handler dispatched (ApplicationInit context)
    │
    ├─ Success (function returns void) → app continues to RUNNING
    │
    └─ Failure (function throws) → app enters FAILED state
         Error logged with init handler module, entrypoint, and error message
```

### 7.5 Init Handler Constraints

- Runs exactly once per deployment (not per restart — per `appDeployId`)
- Runs in the ProcessPool like any CodeComponent — same heap limits, same timeout enforcement
- Has access to datasources declared in the app's `resources.json`
- Does NOT have access to view-layer constructs (`Rivers.view`, `Rivers.stream`, session, request/response)
- Does NOT have access to other apps' datasources
- Timeout defaults to `init_timeout_s` in `riversd.toml` (default: 60 seconds)
- If the init handler fails, the app enters FAILED state. No views are registered. No traffic is routed.

### 7.6 No Init Handler

If `init` is not declared in `manifest.json`, the app has no init phase. Startup proceeds directly from resource resolution to RUNNING (after health check for app-services). This is the common case — most apps do not need DDL.

---

## 8. JS API Surface

### 8.1 Rivers.InitContext (init handler only)

```typescript
interface InitContext {
    /**
     * Execute a DDL statement or admin operation against a named datasource.
     * SQL drivers: accepts CREATE, ALTER, DROP, TRUNCATE statements.
     * Non-SQL drivers: accepts admin operations (e.g. createIndex, create_topic).
     * Requires database@appId in ddl_whitelist.
     * Only available in the init handler.
     */
    ddl(datasource: string, statement: string): Promise<QueryResult>;

    /**
     * Execute an admin operation by operation token (non-SQL drivers).
     * Equivalent to ddl() but dispatches via operation token instead of statement.
     * Requires database@appId in ddl_whitelist.
     * Only available in the init handler.
     */
    admin(datasource: string, operation: string, params?: Record<string, any>): Promise<QueryResult>;

    /**
     * Execute a DML query against a named datasource.
     * Standard query — same as Rivers.db.query in view handlers.
     */
    query(datasource: string, statement: string, params?: Record<string, any>): Promise<QueryResult>;
}
```

`ctx.ddl()` is for SQL DDL statements where the statement text carries the operation. `ctx.admin()` is for non-SQL drivers where the operation token is the dispatch key and there is no statement. Both route through `ddl_execute()` on the driver.

### 8.2 View Handler API — No DDL, No Admin

View handlers continue to use `Rivers.db.query()` and `Rivers.view.query()`. Neither method accepts DDL or admin operations. If a DDL statement or admin operation is passed, the driver-level guard (Gate 1) rejects it with `DriverError::Forbidden`, which surfaces as a handler error.

There is no `Rivers.db.ddl()`, `Rivers.db.admin()`, or equivalent in the view handler context. The methods do not exist.

---

## 9. Startup Sequence Changes

### 9.1 Amendment to `rivers-httpd-spec.md` Section 2

The startup sequence gains a new step between resource resolution (Phase 1) and app-service startup (Phase 2):

```
bundle deployed
    │
    ├─ Phase 1: resolve all resources for all apps
    │       any required resource unresolvable → that app enters FAILED
    │
    ├─ Phase 1.5: run init handlers (NEW)
    │       for each app with init declared:
    │           validate ddl_whitelist for any DDL the init handler attempts
    │           dispatch init handler in ApplicationInit context
    │           success → proceed
    │           failure → app enters FAILED
    │
    ├─ Phase 2: start app-services (parallel where no inter-service dependencies)
    │
    └─ Phase 3: start app-mains (after required app-services RUNNING)
```

### 9.2 Init Handler Ordering

Init handlers run in the same order as app startup:

1. app-service init handlers run before app-main init handlers
2. Within app-services, init handlers respect dependency order (same rules as startup order in `rivers-application-spec.md` Section 9.1)
3. If an app-service init handler fails, dependent app-mains also enter FAILED state

### 9.3 DDL Whitelist Validation at Startup

At `riversd` startup, the `ddl_whitelist` entries are parsed and validated:

- Each entry must match the format `{database}@{appId}`
- `database` must be a non-empty string
- `appId` must be a valid UUID
- Duplicate entries emit a `WARN` log event but are not fatal
- Entries referencing appIds not in any deployed bundle emit a `WARN` log event but are not fatal (the app may be deployed later)

---

## 10. Validation Rules

| Rule | When | Error |
|---|---|---|
| `execute()` receives DDL statement (SQL) | Runtime — any context | `DriverError::Forbidden` |
| `execute()` receives admin operation (non-SQL) | Runtime — any context | `DriverError::Forbidden` |
| `ddl_execute()` called outside ApplicationInit | Runtime | `DriverError::Forbidden` — internal error, should never happen |
| DDL/admin op attempted on datasource+app not in whitelist | Runtime — init handler | `DriverError::Forbidden` with `database@appId` in message |
| `ddl_whitelist` entry invalid format | Startup — config validation | `RiversError::Config` with entry text and expected format |
| Init handler module not found | Startup — deployment | App enters FAILED, error logged |
| Init handler entrypoint not exported | Startup — deployment | App enters FAILED, error logged |
| Init handler throws | Startup — init phase | App enters FAILED, error and stack trace logged |
| Init handler timeout | Startup — init phase | App enters FAILED, timeout logged |
| `ddl_execute()` on driver that doesn't support DDL/admin ops | Runtime — init handler | `DriverError::Unsupported` |

---

## 11. Logging

### 11.1 DDL/Admin Guard Events

| Event | Level | When | Payload |
|---|---|---|---|
| `DdlStatementBlocked` | WARN | `execute()` rejects SQL DDL | `driver`, `datasource`, `statement_prefix` (first 40 chars), `app_id` |
| `AdminOperationBlocked` | WARN | `execute()` rejects non-SQL admin op | `driver`, `datasource`, `operation`, `app_id` |
| `DdlWhitelistDenied` | WARN | Whitelist check fails in init | `database`, `app_id`, `datasource` |
| `DdlExecuted` | INFO | DDL/admin op successfully executed via init | `database`, `app_id`, `datasource`, `statement_prefix` or `operation` |
| `InitHandlerStarted` | INFO | Init handler dispatch begins | `app_id`, `app_name`, `module`, `entrypoint` |
| `InitHandlerCompleted` | INFO | Init handler returns successfully | `app_id`, `app_name`, `duration_ms` |
| `InitHandlerFailed` | ERROR | Init handler throws or times out | `app_id`, `app_name`, `error`, `duration_ms` |

### 11.2 Security — No Statement Logging

DDL statements are never logged in full. Only the first 40 characters of the statement are included in log events as `statement_prefix`. Admin operation tokens are logged in full since they are generic operation names, not schema details.

---

## 12. Configuration Reference

### 12.1 riversd.toml — DDL Whitelist

```toml
[security]
# Existing fields unchanged (cors, rate limiting, etc.)

# DDL whitelist — authorizes specific database+app pairs for DDL execution
# during application init handlers only.
# Format: "database_name@app_uuid"
# Default: [] (empty — no DDL permitted)
ddl_whitelist = [
    "orders_db@f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "inventory_db@b92dc31e-44aa-4817-9123-8f01a2b3c4d5",
]
```

### 12.2 riversd.toml — Init Timeout

```toml
[base]
# Maximum time an application init handler may run before timeout.
# Default: 60 seconds
init_timeout_s = 60
```

---

## 13. Examples

### 13.1 App with DDL Init — Database Migration

**manifest.json:**

```json
{
  "appName": "orders-service",
  "type": "app-service",
  "appId": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "entryPoint": "http://0.0.0.0:9001",
  "init": {
    "module": "handlers/init.ts",
    "entrypoint": "migrate"
  }
}
```

**handlers/init.ts:**

```typescript
export async function migrate(ctx: Rivers.InitContext): Promise<void> {
    // Create tables if they don't exist
    await ctx.ddl("orders_db",
        `CREATE TABLE IF NOT EXISTS orders (
            id SERIAL PRIMARY KEY,
            customer_id INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )`
    );

    await ctx.ddl("orders_db",
        `CREATE TABLE IF NOT EXISTS order_items (
            id SERIAL PRIMARY KEY,
            order_id INTEGER REFERENCES orders(id),
            product_id INTEGER NOT NULL,
            quantity INTEGER NOT NULL
        )`
    );

    // Verify tables exist (DML — always permitted)
    const result = await ctx.query("orders_db",
        "SELECT COUNT(*) as count FROM orders"
    );

    Rivers.log.info("migration complete", { order_count: result.rows[0].count });
}
```

**riversd.toml:**

```toml
[security]
ddl_whitelist = [
    "orders@f47ac10b-58cc-4372-a567-0e02b2c3d479",
]
```

### 13.2 DDL Blocked in View Handler

```typescript
// handlers/admin.ts — view handler
export async function resetTable(req: Rivers.Request): Promise<Rivers.Response> {
    // This will be rejected by Gate 1 — execute() detects DDL
    const result = await Rivers.db.query("orders_db", "DROP TABLE orders");
    // DriverError::Forbidden thrown — handler receives error
}
```

Log output:

```
WARN  rivers::driver: ddl_statement_blocked
  driver           = "postgres"
  datasource       = "orders_db"
  statement_prefix = "DROP TABLE orders"
  app_id           = "f47ac10b-58cc-4372-a567-0e02b2c3d479"
```

### 13.3 DDL Blocked by Whitelist

Init handler attempts DDL on a database not in the whitelist:

```typescript
// handlers/init.ts
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    // analytics_db is not in ddl_whitelist
    await ctx.ddl("analytics_db", "CREATE TABLE metrics (...)");
    // DriverError::Forbidden — whitelist check failed
    // Init handler fails → app enters FAILED state
}
```

Log output:

```
WARN  rivers::dataview: ddl_whitelist_denied
  database   = "analytics"
  app_id     = "f47ac10b-58cc-4372-a567-0e02b2c3d479"
  datasource = "analytics_db"

ERROR rivers::app: init_handler_failed
  app_id     = "f47ac10b-58cc-4372-a567-0e02b2c3d479"
  app_name   = "orders-service"
  error      = "DDL operation not permitted: 'analytics' not in ddl_whitelist for app f47ac10b-..."
  duration_ms = 12
```

### 13.4 Non-SQL Init — MongoDB Collection Setup

**handlers/init.ts:**

```typescript
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    // Create collections with validation schemas
    await ctx.admin("catalog_db", "create_collection", {
        name: "products",
        validator: { $jsonSchema: { bsonType: "object", required: ["name", "price"] } }
    });

    // Create indexes
    await ctx.admin("catalog_db", "create_index", {
        collection: "products",
        keys: { name: 1 },
        options: { unique: true }
    });

    Rivers.log.info("MongoDB schema initialized");
}
```

**riversd.toml:**

```toml
[security]
ddl_whitelist = [
    "catalog@c81ef4a2-7b33-4912-a56e-de09f1a2b3c4",
]
```

### 13.5 Admin Operation Blocked in View Handler — Redis

```typescript
// handlers/maintenance.ts — view handler
export async function flushCache(req: Rivers.Request): Promise<Rivers.Response> {
    // This will be rejected by Gate 1 — execute() checks admin_operations()
    const result = await Rivers.db.query("cache", "flushdb", {});
    // DriverError::Forbidden thrown
}
```

Log output:

```
WARN  rivers::driver: admin_operation_blocked
  driver     = "redis"
  datasource = "cache"
  operation  = "flushdb"
  app_id     = "f47ac10b-58cc-4372-a567-0e02b2c3d479"
```

### 13.6 App Without Init Handler

Most apps have no init handler and no DDL needs:

```json
{
  "appName": "api-gateway",
  "type": "app-main",
  "appId": "a3f8c21d-9b44-4e71-b823-1c04d5e6f789",
  "entryPoint": "https://0.0.0.0"
}
```

No `init` field. No DDL whitelist entry needed. Startup proceeds directly from resource resolution to RUNNING.

---

## 14. Spec Amendment Index

This specification requires amendments to four existing specs. Below is the precise location of each change.

### 14.1 rivers-driver-spec.md

| Section | Change |
|---|---|
| §2 (Five-Op Contract) | Add `DriverError::Forbidden` variant to error enum |
| §2 (Five-Op Contract) | Update `Connection` trait — add `ddl_execute()`, `admin_operations()` methods with default impls |
| §2 (Five-Op Contract) | Add DDL/admin guard requirement to `execute()` contract documentation |
| §2 (Five-Op Contract) | Add `is_ddl_statement()` and `check_admin_guard()` utility functions to SDK exports |
| §3 (Built-in Database Drivers) | Note DDL support and admin operations for each built-in driver |
| §4 (Redis) | Declare admin operations: `flushdb`, `flushall`, `config_set`, `config_rewrite` |
| §7 (Plugin System) | Add plugin admin guard obligation — must override `admin_operations()` and call `check_admin_guard()` |

### 14.2 rivers-data-layer-spec.md

| Section | Change |
|---|---|
| §2.6 (Connection trait) | Mirror trait change — add `ddl_execute()`, `admin_operations()`, update `execute()` doc comment |
| §6 (DataView Engine) | Add execution context awareness — `ApplicationInit` vs `ViewRequest` |
| §6 (DataView Engine) | Add whitelist check before `ddl_execute()` dispatch; check both `is_ddl_statement()` and `admin_operations()` |
| §9 (Plugin Drivers) | Document admin operation declarations for MongoDB, Elasticsearch, Kafka, RabbitMQ, NATS, InfluxDB |

### 14.3 rivers-httpd-spec.md

| Section | Change |
|---|---|
| §2 (Startup Sequence) | Insert Phase 1.5 — init handler execution between resource resolution and app-service startup |
| §19.3 (Security config) | Add `ddl_whitelist` to `[security]` configuration reference |
| §19.1 (Main server config) | Add `init_timeout_s` to `[base]` configuration reference |

### 14.4 rivers-application-spec.md

| Section | Change |
|---|---|
| §5 (App Manifest) | Add optional `init` field to manifest schema |
| §9 (Startup Order) | Amend startup sequence — init handlers run between Phase 1 and Phase 2 |
| New section | Application Init Handler — lifecycle, constraints, API surface |
