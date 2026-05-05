# Rivers Transaction & Multi-Query Specification

**Document Type:** Implementation Specification  
**Scope:** DataView single-statement enforcement, handler transaction API, multi-query orchestration, driver transaction support  
**Status:** Reference / Ground Truth  
**Crate:** `rivers_runtime`, `rivers_core`, `riversd`

---

## Table of Contents

1. [Design Philosophy](#1-design-philosophy)
2. [DataView Single-Statement Enforcement](#2-dataview-single-statement-enforcement)
3. [DataView Transaction Flag](#3-dataview-transaction-flag)
4. [Handler Transaction API](#4-handler-transaction-api)
5. [Transaction Connection Management](#5-transaction-connection-management)
6. [Transaction Result Model](#6-transaction-result-model)
7. [Peek — Intermediate Result Inspection](#7-peek--intermediate-result-inspection)
8. [Error Handling & Auto-Rollback](#8-error-handling--auto-rollback)
9. [Multi-Query Orchestration Patterns](#9-multi-query-orchestration-patterns)
10. [Driver Transaction Contract](#10-driver-transaction-contract)
11. [V8 Host Call Binding](#11-v8-host-call-binding)
12. [Validation Rules](#12-validation-rules)
13. [Cross-Datasource Boundaries](#13-cross-datasource-boundaries)

---

## 1. Design Philosophy

Rivers separates data declaration from data orchestration. A DataView is a single declarative SQL binding — one statement, one result. Multi-query orchestration belongs in handler code, not in DataView TOML.

Transactions are synchronous. The handler calls `tx.query()` and execution blocks until the driver returns. There are no promises, no `await`, no async machinery. This is deliberate: transactions are inherently sequential, and `await` adds ceremony for zero benefit.

The handler is an orchestrator. It calls DataViews by name. It never writes SQL. The SQL lives in TOML, validated at build time, enforced at deploy time.

### 1.1 Key Principles

| ID | Principle |
|---|---|
| TXN-P1 | One statement per query field. Multi-statement SQL is a validation error. |
| TXN-P2 | Handlers call DataViews by name. SQL lives in TOML. |
| TXN-P3 | Multi-query orchestration belongs in handlers, not DataView TOML. |
| TXN-P4 | Transactions are sync. No promises. No await. |
| TXN-P5 | Results accumulate. Nothing returns until commit. |
| TXN-P6 | `tx.peek()` provides interim visibility. Not final until commit. |

---

## 2. DataView Single-Statement Enforcement

### 2.1 Rule

Each query field (`query`, `get_query`, `post_query`, `put_query`, `delete_query`) MUST contain exactly one SQL statement. The presence of a semicolon (`;`) in a query field is a validation error.

### 2.2 Rationale

The SQLite driver's `rusqlite` crate calls `sqlite3_prepare_v2()`, which parses only the first statement up to the first `;`. Statements after the first are silently dropped. No error is raised. The driver returns HTTP 200 with `affected_rows` from the first statement only. This causes silent partial execution — the most dangerous failure mode in any database operation.

Other drivers MAY handle multi-statement SQL differently, but the enforcement is driver-agnostic. The single-statement rule prevents the entire class of partial-execution bugs regardless of driver.

### 2.3 Constraints

| ID | Rule |
|---|---|
| SS-1 | Each query field MUST contain exactly one SQL statement. |
| SS-2 | The presence of a `;` character in any query field string MUST produce a validation error at Gate 1 (`riverpackage`). |
| SS-3 | Gate 2 (`riversd`) MUST repeat the semicolon check on bundle load. |
| SS-4 | The validation error message MUST identify the DataView name and query field. Format: `DataView '{name}' field '{field}' contains multiple statements (semicolon detected). Use a handler with Rivers.db.tx for multi-query operations.` |
| SS-5 | Semicolons inside string literals (e.g., `WHERE name = 'foo;bar'`) MUST NOT trigger the validation error. The check MUST use SQL-aware parsing, not naive string search. |
| SS-6 | Comments containing semicolons (`-- this is a comment;`) MUST NOT trigger the validation error. |

### 2.4 Detection Algorithm

```
1. Strip SQL comments (-- line comments, /* block comments */)
2. Walk the remaining string character by character
3. Track quote state (single-quote open/closed, respecting '' escapes)
4. If a ';' is encountered outside a quoted string → validation error
5. Trailing whitespace-only content after a ';' is still a violation
```

---

## 3. DataView Transaction Flag

### 3.1 Declaration

```toml
[data.dataviews.critical_update]
datasource  = "primary_db"
transaction = true
post_query  = "UPDATE accounts SET balance = balance - $amount WHERE id = $id AND balance >= $amount"
```

### 3.2 Behavior

When `transaction = true` is set on a DataView, the DataViewEngine wraps the single query in an explicit transaction:

```
1. Check out connection from pool
2. Send BEGIN
3. Bind parameters, execute query
4. If success → send COMMIT
5. If error → send ROLLBACK
6. Return connection to pool
```

### 3.3 Constraints

| ID | Rule |
|---|---|
| TF-1 | `transaction = true` is valid on any DataView with a query field. |
| TF-2 | `transaction = true` is independent of handler-level transactions. A DataView called outside a handler transaction gets its own BEGIN/COMMIT wrapper. A DataView called inside a handler transaction (`tx.query()`) uses the handler's transaction — the DataView's `transaction = true` flag is ignored. |
| TF-3 | `transaction = true` on a DataView with a driver that returns `supports_transactions() = false` MUST produce a validation warning at Gate 1, not an error. The flag is silently ignored at runtime. |
| TF-4 | `transaction = true` is NOT required for multi-query operations. Multi-query transactions are handler-level. |

---

## 4. Handler Transaction API

### 4.1 API Surface

The transaction API is injected into the V8 global as `Rivers.db.tx`. It provides four methods:

| Method | Signature | Returns | Sync/Async |
|---|---|---|---|
| `begin` | `Rivers.db.tx.begin(datasource: string)` | `TransactionHandle` | Sync |
| `query` | `tx.query(dataview_name: string, params: object)` | `void` | Sync |
| `peek` | `tx.peek(dataview_name: string)` | `Array<QueryResult>` | Sync |
| `commit` | `tx.commit()` | `HashMap<string, Array<QueryResult>>` | Sync |
| `rollback` | `tx.rollback()` | `void` | Sync |

### 4.2 begin

```typescript
const tx = Rivers.db.tx.begin("datasource_name");
```

**Behavior:**
1. Look up datasource by name in the app's resource registry.
2. Check out a connection from the datasource's connection pool.
3. Send `BEGIN` on the connection.
4. Return a `TransactionHandle` bound to this connection.

**Errors:**
- Datasource not found → `CapabilityError: datasource '{name}' not declared in resources`
- Pool exhausted → `DriverError::Connection: connection pool exhausted for '{name}'`
- BEGIN fails → `DriverError::Transaction: BEGIN failed: {driver_error}`

### 4.3 Constraints

| ID | Rule |
|---|---|
| TX-1 | `tx.begin()` MUST check out exactly one connection from the pool. This connection is held for the lifetime of the transaction. |
| TX-2 | The checked-out connection MUST NOT be returned to the pool until `tx.commit()`, `tx.rollback()`, or auto-rollback fires. |
| TX-3 | `tx.begin()` MUST send a `BEGIN` statement (or driver-equivalent) before returning. |
| TX-4 | Only one active transaction per handler invocation. Calling `Rivers.db.tx.begin()` while a transaction is already active for the current task MUST throw `TransactionError: nested transactions not supported`. |

### 4.4 query

```typescript
tx.query("dataview_name", { param1: "value1", param2: "value2" });
```

**Behavior:**
1. Resolve `dataview_name` to its DataView definition.
2. Validate parameters against the DataView's parameter declarations.
3. Bind parameters to the DataView's query string.
4. Execute the query on the transaction's held connection.
5. Store the `QueryResult` internally, keyed by `dataview_name`.
6. Return void.

**Errors:**
- DataView not found → `DataViewError: DataView '{name}' not found`
- Parameter validation failure → `DataViewError::Parameter: required parameter '{name}' missing`
- DataView's datasource does not match transaction datasource → `TransactionError: DataView '{name}' uses datasource '{dv_ds}' but transaction is on '{tx_ds}'`
- Driver error → `DriverError::Query: {driver_message}` — triggers auto-rollback

### 4.5 Constraints

| ID | Rule |
|---|---|
| TQ-1 | `tx.query()` MUST execute on the transaction's held connection, not a fresh pool connection. |
| TQ-2 | `tx.query()` MUST validate that the DataView's declared datasource matches the transaction's datasource. Mismatch MUST throw `TransactionError`. |
| TQ-3 | `tx.query()` is synchronous. The V8 isolate blocks until the Rust host completes the query and returns. |
| TQ-4 | `tx.query()` returns void. The `QueryResult` is stored internally. |
| TQ-5 | If the same DataView name is called multiple times, each result is appended to an array keyed by that name. First call → `[result0]`. Second call → `[result0, result1]`. Nth call → `[result0, ..., resultN-1]`. |
| TQ-6 | If `tx.query()` throws (driver error), the transaction MUST auto-rollback before the exception propagates to handler code. |
| TQ-7 | The DataView's `transaction = true` flag is ignored when called via `tx.query()`. The handler's transaction governs. |
| TQ-8 | `tx.query()` resolves the DataView's default `query` field, not method-specific variants. There is no HTTP method context inside a transaction. |

---

## 5. Transaction Connection Management

### 5.1 Connection Lifecycle

```
tx.begin("ds")
  │  → pool.checkout()         — connection leaves pool
  │  → connection.execute("BEGIN")
  │
  ├─ tx.query("dv1", {...})    — uses held connection
  ├─ tx.query("dv2", {...})    — same connection
  ├─ tx.query("dv3", {...})    — same connection
  │
  ├─ tx.commit()
  │    → connection.execute("COMMIT")
  │    → pool.checkin()        — connection returns to pool
  │
  └─ OR tx.rollback() / auto-rollback
       → connection.execute("ROLLBACK")
       → pool.checkin()        — connection returns to pool
```

### 5.2 Constraints

| ID | Rule |
|---|---|
| CM-1 | The transaction connection MUST be exclusive to the transaction. No other task or handler MAY use this connection while the transaction is active. |
| CM-2 | The connection MUST be returned to the pool on commit, rollback, or auto-rollback. Leaked connections are a pool exhaustion vector. |
| CM-3 | If the connection is broken during a transaction (network failure, driver disconnect), all subsequent `tx.query()` calls MUST throw `DriverError::Connection`. The auto-rollback fires, but the ROLLBACK itself may fail (already disconnected). The connection MUST be discarded (not returned to pool). |
| CM-4 | Pool checkout timeout applies. If the pool is exhausted and no connection becomes available within `connection_timeout`, `tx.begin()` throws `DriverError::Connection: pool exhausted`. |

---

## 6. Transaction Result Model

### 6.1 Commit Return Type

`tx.commit()` returns a `HashMap<string, Array<QueryResult>>`.

```typescript
const results = tx.commit();

// Shape:
// {
//   "archive_wip":           [QueryResult],
//   "clear_wip":             [QueryResult],
//   "mark_goal_complete":    [QueryResult],
//   "get_goal":              [QueryResult],
// }

// Access:
results["get_goal"][0].rows         // Array of row objects
results["get_goal"][0].rows[0]      // First row
results["archive_wip"][0].affected_rows  // Number of rows affected
```

### 6.2 Multiple Calls to Same DataView

```typescript
tx.query("insert_task", { name: "Task A", goal_id });
tx.query("insert_task", { name: "Task B", goal_id });
tx.query("insert_task", { name: "Task C", goal_id });

const results = tx.commit();

results["insert_task"][0]  // QueryResult for Task A
results["insert_task"][1]  // QueryResult for Task B
results["insert_task"][2]  // QueryResult for Task C
results["insert_task"].length  // 3
```

### 6.3 Constraints

| ID | Rule |
|---|---|
| RM-1 | Every value in the results map MUST be an `Array<QueryResult>`, regardless of how many times the DataView was called. Single call → array of length 1. |
| RM-2 | The array order MUST match the call order. `results["name"][0]` is the result of the first `tx.query("name", ...)` call. |
| RM-3 | `QueryResult` MUST contain `rows: Array<object>`, `affected_rows: number`, and `last_insert_id: any | null`. |
| RM-4 | On commit failure, `tx.commit()` MUST throw `DriverError::Transaction`. The results map is NOT returned. Auto-rollback fires before the exception propagates. |

---

## 7. Peek — Intermediate Result Inspection

### 7.1 API

```typescript
const pending = tx.peek("dataview_name");
// Returns: Array<QueryResult>
```

### 7.2 Behavior

`tx.peek()` returns the accumulated `QueryResult` array for the given DataView name as it stands at the time of the call. These results reflect what the driver returned, but they are NOT final — subsequent queries in the same transaction may change the underlying data.

### 7.3 Use Case

`tx.peek()` enables conditional logic mid-transaction:

```typescript
const tx = Rivers.db.tx.begin("db");

tx.query("check_inventory", { product_id });

const inventory = tx.peek("check_inventory");
if (inventory[0].rows.length === 0 || inventory[0].rows[0].quantity < qty) {
    tx.rollback();
    return { status: 422, body: { error: "insufficient inventory" } };
}

tx.query("decrement_inventory", { product_id, qty });
tx.query("create_shipment", { order_id, product_id, qty });

const results = tx.commit();
```

### 7.4 Constraints

| ID | Rule |
|---|---|
| PK-1 | `tx.peek()` MUST return `Array<QueryResult>`. Same array structure as the commit result. |
| PK-2 | `tx.peek()` for a DataView name that has not been called MUST throw `TransactionError: no results for '{name}'`. |
| PK-3 | `tx.peek()` results are NOT final. They reflect the driver's response at the time the query executed, but the data may be modified by subsequent queries before commit. |
| PK-4 | `tx.peek()` MUST NOT send any SQL to the database. It reads from the transaction's internal result store only. |
| PK-5 | `tx.peek()` is callable multiple times for the same name. Each call returns the current accumulated array. |

---

## 8. Error Handling & Auto-Rollback

### 8.1 Error Sources

| Source | Error Type | Auto-Rollback |
|---|---|---|
| `tx.query()` driver error | `DriverError::Query` | YES |
| `tx.commit()` COMMIT failure | `DriverError::Transaction` | YES |
| Handler throws without commit/rollback | Uncaught exception | YES |
| Handler returns without commit/rollback | Normal return | YES |

### 8.2 Auto-Rollback Behavior

If the handler exits (via return or throw) while a transaction is active (begin called, but neither commit nor rollback called), the engine MUST:

1. Send ROLLBACK on the held connection.
2. Return the connection to the pool (or discard if broken).
3. Log at WARN: `transaction auto-rolled back: handler exited without commit or rollback (datasource='{ds}', trace_id='{trace_id}')`.

### 8.3 Constraints

| ID | Rule |
|---|---|
| AR-1 | Auto-rollback MUST fire on ANY handler exit path that did not call `commit()` or `rollback()`. This includes: normal return, thrown exception, timeout termination. |
| AR-2 | Auto-rollback MUST log at WARN level. Silent auto-rollback masks bugs. |
| AR-3 | If auto-rollback fails (connection broken, ROLLBACK SQL error), the failure MUST be logged at ERROR. The connection MUST be discarded (not returned to pool). |
| AR-4 | After auto-rollback, the handler's return value (if any) is still used as the response. The auto-rollback does not override the handler's HTTP response. |
| AR-5 | The `try/catch` pattern is RECOMMENDED but not enforced. Without it, auto-rollback fires on exception, the exception propagates to the view engine, and the view engine returns 500. |

### 8.4 Handler Pattern

```typescript
// RECOMMENDED: explicit error handling
const tx = Rivers.db.tx.begin("db");
try {
    tx.query("write_op", { params });
    const results = tx.commit();
    return { status: 200, body: results };
} catch (e) {
    // Auto-rollback already fired. Connection released.
    return { status: 500, body: { error: e.message } };
}

// ACCEPTABLE: rely on auto-rollback
const tx = Rivers.db.tx.begin("db");
tx.query("write_op", { params });
tx.query("read_op", { params });
const results = tx.commit();
// If any query throws, auto-rollback fires, exception propagates, view engine returns 500
return { status: 200, body: results["read_op"][0].rows };
```

---

## 9. Multi-Query Orchestration Patterns

### 9.1 Architectural Boundary

Multi-query orchestration MUST occur in handler code, not in DataView TOML. A DataView is a query registry — each entry is one statement, one result. The handler sequences DataView calls, applies business logic between steps, and manages transaction scope.

### 9.2 Without Transaction (Independent Queries)

```typescript
export async function handler(ctx: ViewContext): Promise<Rivers.Response> {
    const order = await Rivers.view.query("get_order", { order_id });
    const items = await Rivers.view.query("get_order_items", { order_id });

    return {
        status: 200,
        body: { ...order.rows[0], items: items.rows },
    };
}
```

Each `Rivers.view.query()` call is independent — separate connection from pool, no transaction. Use for read-only multi-query where atomicity is not required.

### 9.3 With Transaction (Atomic Multi-Write)

```typescript
export async function handler(ctx: ViewContext): Promise<Rivers.Response> {
    const { goal_id, project_id } = ctx.request.params;

    const tx = Rivers.db.tx.begin("cb_data");

    try {
        tx.query("archive_wip", { goal_id });
        tx.query("clear_wip", { goal_id, project_id });
        tx.query("mark_goal_complete", { goal_id, project_id });
        tx.query("clear_project_context", { project_id });
        tx.query("get_goal", { goal_id });

        const results = tx.commit();
        return { status: 200, body: results["get_goal"][0].rows[0] };

    } catch (e) {
        return { status: 500, body: { error: e.message } };
    }
}
```

All queries execute on the same connection, inside BEGIN/COMMIT. Any failure rolls back all changes.

### 9.4 With Peek (Conditional Multi-Write)

See §7.3 for the full pattern. Use when a write decision depends on the result of a previous query in the same transaction.

### 9.5 Constraints

| ID | Rule |
|---|---|
| MQ-1 | Multi-query orchestration MUST occur in handler code. DataView TOML MUST NOT contain multiple statements. |
| MQ-2 | Independent queries (no transaction) use `Rivers.view.query()` with `await`. These are async and use separate pool connections. |
| MQ-3 | Transactional queries use `Rivers.db.tx.query()` without `await`. These are sync and use the transaction's held connection. |
| MQ-4 | DO NOT mix `Rivers.view.query()` and `tx.query()` for the same datasource. `Rivers.view.query()` gets a separate connection and is outside the transaction boundary. |

---

## 10. Driver Transaction Contract

### 10.1 supports_transactions()

Every driver MUST implement `supports_transactions() -> bool`. This is queried at:
- Validation time: `transaction = true` on a DataView backed by a non-transactional driver → warning.
- Runtime: `tx.begin()` against a non-transactional driver → `TransactionError: driver '{name}' does not support transactions`.

### 10.2 Transaction Operations

Drivers that return `supports_transactions() = true` MUST support these Query operations:

| Operation | SQL Equivalent | Behavior |
|---|---|---|
| `begin` | `BEGIN` | Start a transaction on the connection |
| `commit` | `COMMIT` | Commit the active transaction |
| `rollback` | `ROLLBACK` | Rollback the active transaction |

These operations are dispatched through the standard `Connection::execute()` method with `Query { operation: "begin" }`, `Query { operation: "commit" }`, and `Query { operation: "rollback" }`.

### 10.3 Driver Transaction Support Matrix

| Driver | supports_transactions() | Notes |
|---|---|---|
| PostgreSQL | `true` | Full ACID |
| MySQL | `true` | InnoDB only |
| SQLite | `true` | WAL mode recommended |
| MongoDB | `true` | Multi-document transactions (4.0+) |
| Redis | `false` | MULTI/EXEC is not a true transaction |
| Elasticsearch | `false` | No transaction support |
| CouchDB | `false` | No transaction support |
| Cassandra | `false` | Lightweight transactions (LWT) not exposed |
| Kafka | `false` | Broker — not a database |
| LDAP | `false` | No transaction support |
| Faker | `false` | Read-only |
| HTTP | `false` | Proxy — no transaction semantics |
| Filesystem | `false` | No transaction support |
| ExecDriver | `false` | Command execution — no transaction semantics |

### 10.4 Constraints

| ID | Rule |
|---|---|
| DT-1 | `tx.begin()` against a driver where `supports_transactions() = false` MUST throw `TransactionError`. |
| DT-2 | Drivers MUST NOT silently ignore BEGIN/COMMIT/ROLLBACK operations. If the driver does not support transactions, `supports_transactions()` MUST return `false`. |
| DT-3 | The transaction isolation level is driver-default. Rivers does not expose isolation level configuration in v1. |

---

## 11. V8 Host Call Binding

### 11.1 Injection

`Rivers.db.tx` is installed on the V8 global by `inject_rivers_global()` in `crates/riversd/src/process_pool/v8_engine/execution.rs`. It is available in all handler contexts (REST, WebSocket, SSE, MessageConsumer, MCP, pipeline hooks, security hooks, validation hooks, polling runner).

### 11.2 Synchronous Execution Model

All `tx.*` methods are synchronous V8 host calls. When the handler calls `tx.query()`:

1. V8 calls the Rust host function.
2. The Rust host function blocks the current tokio task (using `block_on` or equivalent).
3. The driver executes the query.
4. The result is stored in the transaction's internal state.
5. Control returns to V8.

The V8 isolate is single-threaded and blocked for the duration of each `tx.*` call. This is intentional — transactions are sequential by nature.

### 11.3 Constraints

| ID | Rule |
|---|---|
| V8-1 | All `tx.*` methods MUST be synchronous from V8's perspective. No promises, no callbacks. |
| V8-2 | The transaction state (held connection, accumulated results) MUST be stored in the Rust host, not in V8 heap. V8 holds only a handle (opaque reference). |
| V8-3 | The task timeout watchdog MUST account for transaction wall-clock time. Long transactions that exceed `task_timeout_ms` trigger timeout termination, which fires auto-rollback. |
| V8-4 | `Rivers.db.tx` is available in all handler contexts. There is no per-context gating based on task kind or view type. |

---

## 12. Validation Rules

### 12.1 Gate 1 — riverpackage

| Rule | Severity | Message |
|---|---|---|
| Semicolon in query field | Error | `DataView '{name}' field '{field}' contains multiple statements` |
| `transaction = true` on non-transactional driver | Warning | `DataView '{name}' has transaction=true but driver '{driver}' does not support transactions` |

### 12.2 Gate 2 — riversd

| Rule | Severity | Message |
|---|---|---|
| Semicolon in query field | Error | Same as Gate 1 — defense in depth |
| `transaction = true` on non-transactional driver | Warning | Same as Gate 1 |

### 12.3 Runtime

| Condition | Error |
|---|---|
| `tx.begin()` on non-transactional driver | `TransactionError: driver '{name}' does not support transactions` |
| `tx.query()` DataView datasource mismatch | `TransactionError: DataView '{name}' uses datasource '{dv_ds}' but transaction is on '{tx_ds}'` |
| `tx.peek()` for uncalled DataView | `TransactionError: no results for '{name}'` |
| Nested `tx.begin()` | `TransactionError: nested transactions not supported` |
| BEGIN/COMMIT/ROLLBACK driver failure | `DriverError::Transaction: {message}` |

---

## 13. Cross-Datasource Boundaries

### 13.1 Rule

A transaction MUST be scoped to a single datasource. All DataViews called via `tx.query()` MUST use the same datasource as the `tx.begin()` call. Cross-datasource transactions are NOT supported.

### 13.2 Cross-Datasource Without Atomicity

For operations spanning multiple datasources without atomicity guarantees, use `Rivers.view.query()` (async, independent connections):

```typescript
const order = await Rivers.view.query("create_order", { params });       // orders_db
const notification = await Rivers.view.query("send_notification", { params }); // notifications_db
```

Each call is independent. If the notification fails, the order is already committed. The handler is responsible for compensation logic.

### 13.3 Constraints

| ID | Rule |
|---|---|
| CD-1 | `tx.query()` MUST reject DataViews whose declared datasource differs from the transaction's datasource. |
| CD-2 | Cross-datasource atomicity is NOT supported in v1. Handlers own compensation/recovery logic. |
| CD-3 | A future version MAY introduce saga-pattern support. This spec does not define it. |

---

## CHANGELOG

```markdown
## [Decision] — Transaction & Multi-Query Specification
**Date:** 2026-05-04
**Description:** Established DataView single-statement enforcement, sync handler transaction API (Rivers.db.tx), multi-query orchestration patterns, and driver transaction contract. Designed to prevent the silent partial-execution bug class discovered in multi-statement DataViews against rusqlite.
**Resolution:** Single-statement enforcement at Gate 1 and Gate 2. Multi-query orchestration in handler code via sync transaction API. Results accumulate and return on commit as HashMap<string, Array<QueryResult>>. tx.peek() for conditional logic mid-transaction.
```
