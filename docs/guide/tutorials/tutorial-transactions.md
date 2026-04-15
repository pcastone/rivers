# Tutorial: Transactions, Prepared Statements, and Batch Operations

**Rivers v0.54.0**

## Overview

Transactions let handlers group multiple DataView operations into atomic units, ensuring that either all operations succeed or all are rolled back. Prepared statements cache query plans at the driver level, reducing overhead for repeated queries. Batch operations execute a single DataView with multiple parameter sets using a single connection, inheriting any active transaction state.

---

## Transactions

Transactions ensure atomicity: if your handler groups multiple database operations together and one fails, all changes are rolled back automatically.

### Start a Transaction

Use `Rivers.db.begin()` to start a transaction on a specific datasource:

```javascript
function transferFunds(ctx) {
    var body = ctx.request.body;

    // Start a transaction
    Rivers.db.begin("postgres-accounts");

    try {
        // Debit account A
        ctx.dataview("debit_account", {
            account_id: body.from_id,
            amount: body.amount
        });

        // Credit account B
        ctx.dataview("credit_account", {
            account_id: body.to_id,
            amount: body.amount
        });

        // Both operations succeeded — commit the transaction
        Rivers.db.commit("postgres-accounts");

        ctx.resdata = {
            status: "success",
            message: "Transfer completed"
        };
    } catch (e) {
        // An error occurred — rollback the entire transaction
        Rivers.db.rollback("postgres-accounts");

        Rivers.log.error("transfer failed", {
            error: e.message,
            from_id: body.from_id,
            to_id: body.to_id
        });

        throw e;
    }
}
```

### Key Concepts

| Concept | Description |
|---------|-------------|
| **One transaction per datasource** | You can have multiple active transactions (one per datasource) in a single handler request. |
| **Datasource name** | The datasource name (e.g., `"postgres-accounts"`) is the identifier in your `resources.toml` file. |
| **Connection held** | When you call `begin()`, a connection is acquired from the pool and held until `commit()` or `rollback()`. |
| **Auto-rollback** | If your handler exits without calling `commit()`, all active transactions are rolled back automatically with a warning logged. |

### Supported Drivers

The following drivers support transactions:

- **PostgreSQL** — Full ACID support
- **MySQL** — Full ACID support (InnoDB)
- **SQLite** — Full ACID support
- **MongoDB** — Multi-document transactions (v4.0+)
- **Neo4j** — Full transaction support

Other drivers (Elasticsearch, Redis, Kafka, HTTP, Faker, etc.) will throw an error if you call `begin()` on them.

### Testing Transactions

Create a test scenario with two DataViews:

**File:** `resources.toml`

```toml
[[datasources]]
name     = "postgres-accounts"
driver   = "postgres"
pool     = { max = 10, min = 2 }
secrets  = "DB_URL"
```

**File:** `app.toml`

```toml
[data.dataviews.debit_account]
name       = "debit_account"
datasource = "postgres-accounts"
query      = "UPDATE accounts SET balance = balance - $1 WHERE id = $2"

[data.dataviews.credit_account]
name       = "credit_account"
datasource = "postgres-accounts"
query      = "UPDATE accounts SET balance = balance + $1 WHERE id = $2"

[api.views.transfer]
path      = "transfer"
method    = "POST"
view_type = "Rest"

[api.views.transfer.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/accounts.js"
entrypoint = "transferFunds"
resources  = ["postgres-accounts"]
```

**Test:**

```bash
curl -X POST http://localhost:8080/transfer \
  -H "Content-Type: application/json" \
  -d '{"from_id":"acc-001","to_id":"acc-002","amount":100}'
```

If the debit succeeds but the credit fails, the debit is rolled back. The account balances remain unchanged.

---

## Prepared Statements

Prepared statements cache query plans at the driver level, reducing parsing and planning overhead. This is especially useful for DataViews that are called frequently with different parameters.

### Enable Prepared Statements

Add the `prepared = true` field to a DataView in `app.toml`:

```toml
[data.dataviews.search_orders]
name       = "search_orders"
datasource = "postgres-main"
query      = "SELECT * FROM orders WHERE customer_id = $1 AND status = $2"
prepared   = true
```

### How It Works

1. **First execution** — The driver prepares the query and caches the plan
2. **Subsequent executions** — The cached plan is reused, skipping the parse and plan phases
3. **Pool connection reuse** — Prepared plans are tied to specific connections. To maximize reuse, Rivers keeps pool connections open between requests
4. **Transparent** — Your handler code does not change. Prepared statements work automatically

```javascript
function searchOrders(ctx) {
    // This uses the prepared statement on the first call
    var results = ctx.dataview("search_orders", {
        customer_id: "C001",
        status: "pending"
    });

    // Subsequent calls reuse the cached plan
    ctx.resdata = results;
}
```

### When to Use

Prepared statements are beneficial for:

- **High-frequency queries** — DataViews called many times per second
- **Complex queries** — Large SELECT statements with joins or aggregations
- **Parameter variance** — Same query with different WHERE clause parameters

Prepared statements have minimal benefit for:

- **One-off queries** — Infrequently executed DataViews
- **Simple queries** — Very fast to parse and plan anyway

### Performance Note

Prepared statements trade memory and connection utilization for reduced planning overhead. Each prepared statement consumes a small amount of driver memory. If your datasource has memory constraints, measure the trade-off before enabling prepared statements on every DataView.

---

## Batch Operations

Batch operations execute a single DataView multiple times with different parameter sets, using a single connection. This is faster than calling the DataView separately for each parameter set because it reuses the connection and avoids round-trip overhead.

### Batch Execution

Use `Rivers.db.batch()` to execute a DataView with multiple parameter sets:

```javascript
function createOrders(ctx) {
    var body = ctx.request.body;  // Array of order objects

    // Execute "insert_order" once for each item in the array
    var results = Rivers.db.batch("insert_order", [
        { customerId: "C001", amount: 100, status: "pending" },
        { customerId: "C002", amount: 250, status: "pending" },
        { customerId: "C003", amount: 75, status: "pending" }
    ]);

    Rivers.log.info("batch insert completed", {
        count: results.length
    });

    ctx.resdata = {
        status: "success",
        inserted: results.length,
        orders: results
    };
}
```

### Batch + Transactions

Batch operations inherit the active transaction state. If you start a transaction and then call `batch()`, all parameter sets in the batch execute within the same transaction:

```javascript
function bulkCreateOrders(ctx) {
    var body = ctx.request.body;

    // Start a transaction
    Rivers.db.begin("postgres-orders");

    try {
        // Batch execute within the transaction
        var orders = Rivers.db.batch("insert_order", body.orders);

        // Insert associated audit log entries
        var auditLogs = Rivers.db.batch("insert_audit_log", 
            orders.map(o => ({ order_id: o.id, event: "created" }))
        );

        // Both batches succeeded — commit
        Rivers.db.commit("postgres-orders");

        ctx.resdata = {
            status: "success",
            orders_created: orders.length,
            audit_logs: auditLogs.length
        };
    } catch (e) {
        // Rollback both batches
        Rivers.db.rollback("postgres-orders");
        throw e;
    }
}
```

### Batch Results

The `batch()` function returns an array of results, one per parameter set:

```javascript
var results = Rivers.db.batch("insert_order", [
    { customerId: "C001", amount: 100 },
    { customerId: "C002", amount: 250 },
    { customerId: "C003", amount: 75 }
]);

// results is an array of 3 elements
// results[0] = { id: "order-001", customerId: "C001", ... }
// results[1] = { id: "order-002", customerId: "C002", ... }
// results[2] = { id: "order-003", customerId: "C003", ... }
```

### Performance Note

Batch operations are efficient because they:

1. Use a single connection (no pool overhead)
2. Execute one round-trip per parameter set (not one per DataView call)
3. Reduce context switching between JavaScript and the driver

However, if a batch is very large (thousands of rows), consider splitting it into smaller batches to avoid memory spikes and connection timeouts.

---

## Auto-Rollback

If your handler exits without committing an active transaction, Rivers automatically rolls back all uncommitted changes and logs a warning.

### Scenario

```javascript
function risky(ctx) {
    Rivers.db.begin("postgres-main");

    // Perform some operations...
    ctx.dataview("update_data", { id: 1, value: "new" });

    // Handler exits without calling commit()
    // Rivers detects the unclosed transaction and rolls back
}
```

### Log Output

When auto-rollback occurs, you'll see a warning like this:

```
WARN: Auto-rollback of unclosed transaction on datasource 'postgres-main'
      (handler did not call Rivers.db.commit)
```

This prevents accidental data corruption from incomplete transactions. However, you should always explicitly handle transaction completion in production code.

### Best Practice

Always wrap transaction code in try-catch and explicitly commit or rollback:

```javascript
Rivers.db.begin("postgres-main");

try {
    // Perform operations
    ctx.resdata = ctx.dataview("do_something", params);
    Rivers.db.commit("postgres-main");
} catch (e) {
    Rivers.db.rollback("postgres-main");
    Rivers.log.error("transaction failed", { error: e.message });
    throw e;
}
```

---

## Complete Example: Order Processing

Here's a complete example combining transactions, batch operations, and prepared statements:

**File:** `resources.toml`

```toml
[[datasources]]
name     = "postgres-orders"
driver   = "postgres"
pool     = { max = 20, min = 5 }
secrets  = "DB_URL"
```

**File:** `app.toml`

```toml
[data.dataviews.insert_order]
name       = "insert_order"
datasource = "postgres-orders"
query      = "INSERT INTO orders (customer_id, amount, status) VALUES ($1, $2, $3) RETURNING id, created_at"
prepared   = true

[data.dataviews.update_order_status]
name       = "update_order_status"
datasource = "postgres-orders"
query      = "UPDATE orders SET status = $1 WHERE id = $2"
prepared   = true

[data.dataviews.insert_payment]
name       = "insert_payment"
datasource = "postgres-orders"
query      = "INSERT INTO payments (order_id, amount, method) VALUES ($1, $2, $3) RETURNING id"
prepared   = true

[data.dataviews.insert_audit_log]
name       = "insert_audit_log"
datasource = "postgres-orders"
query      = "INSERT INTO audit_logs (order_id, event, timestamp) VALUES ($1, $2, NOW())"
prepared   = true

[api.views.create_bulk_orders]
path      = "orders/bulk"
method    = "POST"
view_type = "Rest"

[api.views.create_bulk_orders.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/orders.js"
entrypoint = "createBulkOrders"
resources  = ["postgres-orders"]
```

**File:** `libraries/handlers/orders.js`

```javascript
function createBulkOrders(ctx) {
    var body = ctx.request.body;

    // Start a transaction for the entire bulk operation
    Rivers.db.begin("postgres-orders");

    try {
        // Batch insert all orders
        var orders = Rivers.db.batch("insert_order",
            body.orders.map(o => [o.customer_id, o.amount, "pending"])
        );

        // Batch insert associated payments
        var payments = Rivers.db.batch("insert_payment",
            orders.map((o, i) => [o.id, body.orders[i].amount, "credit_card"])
        );

        // Batch insert audit logs
        Rivers.db.batch("insert_audit_log",
            orders.map(o => [o.id, "order_created"])
        );

        // All batches succeeded — commit
        Rivers.db.commit("postgres-orders");

        Rivers.log.info("bulk order creation completed", {
            orders_created: orders.length,
            payments_created: payments.length
        });

        ctx.resdata = {
            status: "success",
            orders: orders,
            payments: payments
        };
    } catch (e) {
        // Rollback entire transaction
        Rivers.db.rollback("postgres-orders");

        Rivers.log.error("bulk order creation failed", {
            error: e.message,
            order_count: body.orders.length
        });

        throw e;
    }
}
```

**Test:**

```bash
curl -X POST http://localhost:8080/orders/bulk \
  -H "Content-Type: application/json" \
  -d '{
    "orders": [
      {"customer_id":"C001","amount":100},
      {"customer_id":"C002","amount":250},
      {"customer_id":"C003","amount":75}
    ]
  }'
```

**Response:**

```json
{
  "status": "success",
  "orders": [
    {"id":"order-001","customer_id":"C001","amount":100,"status":"pending","created_at":"2026-04-14T10:00:00Z"},
    {"id":"order-002","customer_id":"C002","amount":250,"status":"pending","created_at":"2026-04-14T10:00:01Z"},
    {"id":"order-003","customer_id":"C003","amount":75,"status":"pending","created_at":"2026-04-14T10:00:02Z"}
  ],
  "payments": [
    {"id":"pay-001"},
    {"id":"pay-002"},
    {"id":"pay-003"}
  ]
}
```

If any batch fails (e.g., a constraint violation), all changes are rolled back. The orders, payments, and audit logs are all reverted to their initial state.

---

## Summary

This tutorial covered:

1. **Transactions** — Group multiple DataView operations into atomic units with `Rivers.db.begin()`, `commit()`, and `rollback()`
2. **Supported drivers** — PostgreSQL, MySQL, SQLite, MongoDB, and Neo4j support transactions
3. **Prepared statements** — Set `prepared = true` on a DataView to cache query plans and reduce overhead
4. **Batch operations** — Use `Rivers.db.batch()` to execute one DataView with multiple parameter sets efficiently
5. **Batch + transactions** — Batches inherit active transaction state, allowing you to group multiple batches into a single atomic operation
6. **Auto-rollback** — Transactions are rolled back automatically if your handler exits without committing
7. **Best practices** — Always wrap transaction code in try-catch and explicitly handle success and failure cases

Together, these features enable efficient, reliable processing of complex multi-step operations in your Rivers handlers.
