# Tutorial: Transactions & Multi-Query Operations

**Covers:** `Rivers.db.tx`, DataView `transaction = true`, `tx.peek()`, cross-datasource patterns.

**Spec reference:** `docs/arch/rivers-transaction-multi-query-spec.md`

---

## 1. Single-statement rule

Each DataView query field must contain **exactly one SQL statement**. A semicolon outside a string literal or comment is a validation error (C010).

```toml
# WRONG — riverpackage validate will fail with C010
[data.dataviews.bad]
datasource = "db"
query = "INSERT INTO log VALUES ($x); SELECT 1"

# RIGHT — one statement per DataView
[data.dataviews.write_log]
datasource = "db"
post_query = "INSERT INTO log VALUES ($x)"

[data.dataviews.read_last]
datasource = "db"
query = "SELECT * FROM log ORDER BY id DESC LIMIT 1"
```

---

## 2. DataView `transaction = true` — single-query wrapper

For cases where a single write must be atomic, set `transaction = true` on the DataView:

```toml
[data.dataviews.debit_account]
datasource  = "payments_db"
transaction = true
post_query  = "UPDATE accounts SET balance = balance - $amount WHERE id = $id AND balance >= $amount"
```

Rivers automatically sends `BEGIN` before the query and `COMMIT` on success. On driver error it sends `ROLLBACK`.

> **Note:** `transaction = true` is silently ignored (with a validation warning W008) if the driver does not support transactions (Redis, Elasticsearch, Faker, etc.).

---

## 3. Multi-query atomic writes with `Rivers.db.tx`

When you need multiple DataViews to succeed or fail together, use the synchronous `Rivers.db.tx` API.

### 3.1 Basic pattern

```typescript
export function handler(ctx) {
    const { goal_id, project_id } = ctx.request.params;

    const tx = Rivers.db.tx.begin("cb_data");

    try {
        tx.query("archive_wip",        { goal_id });
        tx.query("clear_wip",          { goal_id, project_id });
        tx.query("mark_goal_complete", { goal_id, project_id });
        tx.query("clear_project_ctx",  { project_id });
        tx.query("get_goal",           { goal_id });

        const results = tx.commit();

        return {
            status: 200,
            body: results["get_goal"][0].rows[0],
        };
    } catch (e) {
        // Auto-rollback already fired before this catch block.
        return { status: 500, body: { error: e.message } };
    }
}
```

**`tx.commit()` returns** `HashMap<string, Array<QueryResult>>`:

```typescript
results["get_goal"][0].rows               // first call's rows
results["archive_wip"][0].affected_rows   // rows affected
```

If the same DataView is called multiple times, results are appended in order:

```typescript
tx.query("insert_task", { name: "A", goal_id });
tx.query("insert_task", { name: "B", goal_id });
tx.query("insert_task", { name: "C", goal_id });

const results = tx.commit();
results["insert_task"][0]  // result for "A"
results["insert_task"][1]  // result for "B"
results["insert_task"][2]  // result for "C"
```

### 3.2 Key rules

| Rule | Detail |
|------|--------|
| Sync | `tx.query()` blocks V8 until the driver responds. No `await`. |
| Single datasource | All DataViews called via `tx.query()` must use the same datasource as `tx.begin()`. |
| No nesting | Calling `Rivers.db.tx.begin()` while a transaction is active throws `TransactionError: nested transactions not supported`. |
| Auto-rollback | If the handler exits without `commit()` or `rollback()`, Rivers sends ROLLBACK and logs WARN. |
| DataView `transaction = true` ignored | When called via `tx.query()`, the DataView's own flag is overridden by the handler transaction. |
| Default query field | `tx.query()` always uses the DataView's default `query` field — no HTTP method context inside a transaction. |

---

## 4. Conditional writes with `tx.peek()`

`tx.peek(name)` reads accumulated results **without touching the database**, enabling conditional mid-transaction logic:

```typescript
export function handler(ctx) {
    const { product_id, qty, order_id } = ctx.request.body;

    const tx = Rivers.db.tx.begin("inventory_db");

    try {
        tx.query("check_inventory", { product_id });

        const inv = tx.peek("check_inventory");
        if (inv[0].rows.length === 0 || inv[0].rows[0].quantity < qty) {
            tx.rollback();
            return { status: 422, body: { error: "insufficient inventory" } };
        }

        tx.query("decrement_inventory", { product_id, qty });
        tx.query("create_shipment",     { order_id, product_id, qty });

        const results = tx.commit();
        return { status: 200, body: results["create_shipment"][0].rows[0] };

    } catch (e) {
        return { status: 500, body: { error: e.message } };
    }
}
```

> `tx.peek()` for an uncalled DataView throws `TransactionError: no results for '{name}'`.

---

## 5. Cross-datasource operations (no atomicity)

A `Rivers.db.tx` transaction cannot span multiple datasources. For operations across different datasources, call each independently using the async `Rivers.view.query()` API:

```typescript
export async function handler(ctx) {
    // Each call uses a separate pool connection — no shared transaction
    const order = await Rivers.view.query("create_order", ctx.request.body);
    const notification = await Rivers.view.query("queue_notification", {
        order_id: order.rows[0].id,
    });

    // If notification fails, the order is already committed.
    // The handler owns compensation logic.
    return { status: 200, body: { order: order.rows[0] } };
}
```

---

## 6. DataView config reference

```toml
[data.dataviews.my_write]
datasource  = "primary_db"
transaction = true          # wrap single query in BEGIN/COMMIT

post_query  = "INSERT INTO events (type, payload) VALUES ($type, $payload)"

[[data.dataviews.my_write.parameters]]
name     = "type"
type     = "string"
required = true

[[data.dataviews.my_write.parameters]]
name     = "payload"
type     = "string"
required = true
```
