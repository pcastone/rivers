# Rivers Cookbook — Architecture Reference (Opus)

**Purpose:** Complete pattern library for building Rivers applications. Each recipe includes decision logic for when to select it, dependency and composition rules for chaining recipes, architectural constraints, and failure modes. Templates and examples are concrete and complete.

**Architectural Invariants:**
1. ONE statement per query field. Semicolons → validation error.
2. Handlers call DataViews by name via `Rivers.view.query()`. SQL lives in TOML, never in handler code.
3. `query` is the default query field (handler dispatch). Method-specific variants (`get_query`, `post_query`, `put_query`, `delete_query`) activate on REST dispatch.
4. Multi-query orchestration belongs in handlers, not in DataView TOML.
5. Transactions are sync: `Rivers.db.tx.begin()` / `tx.query()` / `tx.commit()`. No promises, no await.
6. `Rivers.db.query()` exists for raw SQL but is NOT the intended path. Monitor usage; DataViews are the correct door.
7. `transaction = true` on a DataView wraps its single query in BEGIN/COMMIT. Independent of multi-query transactions.

---

## Table of Contents

### Data Access
- [RECIPE:SINGLE-READ](#recipesingle-read)
- [RECIPE:SINGLE-WRITE](#recipesingle-write)
- [RECIPE:PARAMETERIZED-READ](#recipeparameterized-read)
- [RECIPE:FILTERED-LIST](#recipefiltered-list)
- [RECIPE:MULTI-QUERY-READ](#recipemulti-query-read)
- [RECIPE:LOOKUP-THEN-WRITE](#recipelookup-then-write)
- [RECIPE:CONDITIONAL-WRITE](#recipeconditional-write)
- [RECIPE:ATOMIC-MULTI-WRITE](#recipeatomic-multi-write)
- [RECIPE:CROSS-DATASOURCE](#recipecross-datasource)
- [RECIPE:PSEUDO-DATAVIEW](#recipepseudo-dataview)

### View Patterns
- [RECIPE:REST-CRUD](#reciperest-crud)
- [RECIPE:REST-READONLY](#reciperest-readonly)
- [RECIPE:REST-HANDLER-BACKED](#reciperest-handler-backed)
- [RECIPE:VIEW-PIPELINE](#recipeview-pipeline)
- [RECIPE:SPA-WITH-API](#recipespa-with-api)
- [RECIPE:MULTI-DATASOURCE-VIEW](#recipemulti-datasource-view)

### MCP Tools
- [RECIPE:MCP-READ-TOOL](#recipemcp-read-tool)
- [RECIPE:MCP-WRITE-TOOL](#recipemcp-write-tool)
- [RECIPE:MCP-HANDLER-TOOL](#recipemcp-handler-tool)
- [RECIPE:MCP-MULTI-STEP](#recipemcp-multi-step)

### Realtime
- [RECIPE:WEBSOCKET-VIEW](#recipewebsocket-view)
- [RECIPE:SSE-VIEW](#recipesse-view)
- [RECIPE:MESSAGE-CONSUMER](#recipemessage-consumer)
- [RECIPE:POLLING-VIEW](#recipepolling-view)

### Auth & Security
- [RECIPE:AUTH-REQUIRED-VIEW](#recipeauth-required-view)
- [RECIPE:AUTH-NONE-VIEW](#recipeauth-none-view)
- [RECIPE:SESSION-HANDLER](#recipesession-handler)
- [RECIPE:API-KEY-AUTH](#recipeapi-key-auth)

### Transactions
- [RECIPE:SIMPLE-TRANSACTION](#recipesimple-transaction)
- [RECIPE:TRANSACTION-WITH-PEEK](#recipetransaction-with-peek)
- [RECIPE:TRANSACTION-ROLLBACK](#recipetransaction-rollback)
- [RECIPE:DATAVIEW-TRANSACTION-FLAG](#recipedataview-transaction-flag)

### Caching
- [RECIPE:CACHED-DATAVIEW](#recipecached-dataview)
- [RECIPE:CACHE-INVALIDATION](#recipecache-invalidation)
- [RECIPE:NO-CACHE](#recipeno-cache)

### Init & Lifecycle
- [RECIPE:INIT-HANDLER-DDL](#recipeinit-handler-ddl)
- [RECIPE:INIT-HANDLER-SEED](#recipeinit-handler-seed)
- [RECIPE:INIT-HANDLER-NOSQL](#recipeinit-handler-nosql)

### Error Handling
- [RECIPE:HANDLER-ERROR-RESPONSE](#recipehandler-error-response)
- [RECIPE:DATAVIEW-NOT-FOUND](#recipedataview-not-found)
- [RECIPE:DRIVER-ERROR-HANDLING](#recipedriver-error-handling)

### Schema & Validation
- [RECIPE:REQUEST-SCHEMA](#reciperequest-schema)
- [RECIPE:RESPONSE-SCHEMA](#reciperesponse-schema)
- [RECIPE:PARAMETER-DEFAULTS](#recipeparameter-defaults)

### Bundle & Project Setup
- [RECIPE:NEW-BUNDLE](#recipenew-bundle)
- [RECIPE:DATASOURCE-SQL](#recipedatasource-sql)
- [RECIPE:DATASOURCE-REDIS](#recipedatasource-redis)
- [RECIPE:DATASOURCE-NOSQL](#recipedatasource-nosql)
- [RECIPE:DATASOURCE-FAKER](#recipedatasource-faker)
- [RECIPE:DATASOURCE-HTTP](#recipedatasource-http)
- [RECIPE:DATASOURCE-LDAP](#recipedatasource-ldap)
- [RECIPE:DATASOURCE-KAFKA](#recipedatasource-kafka)
- [RECIPE:LOCKBOX-CREDENTIALS](#recipelockbox-credentials)
- [RECIPE:DDL-WHITELIST](#recipeddl-whitelist)

### Filesystem & Execution
- [RECIPE:FILESYSTEM-OPS](#recipefilesystem-ops)
- [RECIPE:EXEC-DRIVER](#recipeexec-driver)

### Message Broker Patterns
- [RECIPE:KAFKA-PRODUCE](#recipekafka-produce)
- [RECIPE:KAFKA-CONSUME](#recipekafka-consume)

### Plugin Development
- [RECIPE:PLUGIN-DRIVER](#recipeplugin-driver)

### Infrastructure & Server Config
- [RECIPE:STORAGE-ENGINE](#recipestorage-engine)
- [RECIPE:CONNECTION-POOL](#recipeconnection-pool)
- [RECIPE:RATE-LIMITING](#reciperate-limiting)
- [RECIPE:CORS-CONFIG](#recipecors-config)
- [RECIPE:LOGGING-CONFIG](#recipelogging-config)
- [RECIPE:TRACING-CONFIG](#recipetracing-config)
- [RECIPE:ENVIRONMENT-OVERRIDES](#recipeenvironment-overrides)
- [RECIPE:HOT-RELOAD](#recipehot-reload)

### Application Patterns (Extended)
- [RECIPE:GRAPHQL-SETUP](#recipegraphql-setup)
- [RECIPE:STREAMING-REST](#recipestreaming-rest)
- [RECIPE:MULTI-APP-BUNDLE](#recipemulti-app-bundle)
- [RECIPE:SERVICE-TO-SERVICE](#recipeservice-to-service)
- [RECIPE:OUTBOUND-HTTP](#recipeoutbound-http)
- [RECIPE:WASM-HANDLER](#recipewasm-handler)

### Schema Patterns
- [RECIPE:SCHEMA-FILE](#recipeschema-file)

### Operations
- [RECIPE:BUNDLE-VALIDATION](#recipebundle-validation)
- [RECIPE:GRACEFUL-SHUTDOWN](#recipegraceful-shutdown)
- [RECIPE:ERROR-RESPONSE-FORMAT](#recipeerror-response-format)

### Anti-Patterns
- [ANTI:MULTI-STATEMENT-SQL](#antimulti-statement-sql)
- [ANTI:RAW-SQL-IN-HANDLER](#antiraw-sql-in-handler)
- [ANTI:SPLIT-TOOL-SEQUENCING](#antisplit-tool-sequencing)
- [ANTI:HANDLER-FOR-SIMPLE-CRUD](#antihandler-for-simple-crud)
- [ANTI:RETURNING-CLAUSE](#antireturning-clause)

---

# Data Access Recipes

---

## RECIPE:SINGLE-READ

### Decision
Select when the requirement is: expose data from one datasource, one query, no transformation. Keywords: "get," "fetch," "read," "look up," "show," "retrieve." If the user describes a simple GET endpoint returning a database row, start here. No handler needed — the View binds directly to the DataView.

### Dependencies
- Depends on: nothing
- Depended on by: `RECIPE:MULTI-QUERY-READ` (as a component query), `RECIPE:REST-CRUD` (as the GET leg), `RECIPE:VIEW-PIPELINE` (as primary DataView)
- Combines with: `RECIPE:CACHED-DATAVIEW` for caching, `RECIPE:RESPONSE-SCHEMA` for output validation

### Template

```toml
[data.dataviews.{dv_name}]
datasource    = "{datasource_name}"
query         = "SELECT {columns} FROM {table} WHERE {column} = ${param}"
return_schema = "schemas/{schema_file}.schema.json"

[[data.dataviews.{dv_name}.parameters]]
name     = "{param_name}"
type     = "{type}"
required = true

[api.views.{view_name}]
path      = "/api/{resource}/{path_param}"
method    = "GET"
view_type = "Rest"
auth      = "{auth_mode}"

[api.views.{view_name}.handler]
type     = "dataview"
dataview = "{dv_name}"

[api.views.{view_name}.parameter_mapping.path]
{path_param} = "{dv_param_name}"
```

### Concrete Example

```toml
[data.dataviews.get_order]
datasource    = "orders_db"
query         = "SELECT id, customer_id, amount, status FROM orders WHERE id = $id"
return_schema = "schemas/order.schema.json"

[[data.dataviews.get_order.parameters]]
name     = "id"
type     = "uuid"
required = true

[api.views.get_order]
path      = "/api/orders/{id}"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.get_order.handler]
type     = "dataview"
dataview = "get_order"

[api.views.get_order.parameter_mapping.path]
id = "id"
```

### Constraints
- No handler code. View binds directly to DataView.
- One statement per query field.
- Use `parameter_mapping` to wire path/query params to DataView params.

### Failure Modes
- Writing a passthrough handler → `ANTI:HANDLER-FOR-SIMPLE-CRUD`. Delete the handler.
- Multiple statements in `query` → `ANTI:MULTI-STATEMENT-SQL`. Split into separate DataViews.
- Need 404 on empty result → this recipe returns 200 with empty data. Escalate to `RECIPE:REST-HANDLER-BACKED`.

### Escalation

| Signal | Escalate to |
|--------|-------------|
| Data from a second table | `RECIPE:MULTI-QUERY-READ` |
| Reshape the result | `RECIPE:VIEW-PIPELINE` |
| Per-row permission checks | `RECIPE:REST-HANDLER-BACKED` |
| External service call | `RECIPE:MULTI-DATASOURCE-VIEW` |

---

## RECIPE:SINGLE-WRITE

### Decision
Select when the requirement is: one insert, update, or delete against one datasource. Keywords: "create," "add," "update," "modify," "delete," "remove." No read-back needed (or the read-back is a separate concern). No conditional logic.

### Dependencies
- Depends on: nothing
- Depended on by: `RECIPE:REST-CRUD` (as POST/PUT/DELETE legs), `RECIPE:ATOMIC-MULTI-WRITE` (each step is a single-write DataView), `RECIPE:MCP-WRITE-TOOL`
- Combines with: `RECIPE:CACHE-INVALIDATION` to clear stale read caches, `RECIPE:REQUEST-SCHEMA` for input validation

### Template

```toml
[data.dataviews.{dv_name}]
datasource = "{datasource_name}"
post_query = "INSERT INTO {table} ({columns}) VALUES ({$params})"

[[data.dataviews.{dv_name}.post.parameters]]
name     = "{param_name}"
type     = "{type}"
required = true

[api.views.{view_name}]
path      = "/api/{resource}"
method    = "POST"
view_type = "Rest"
auth      = "{auth_mode}"

[api.views.{view_name}.handler]
type     = "dataview"
dataview = "{dv_name}"

[api.views.{view_name}.parameter_mapping.body]
{body_field} = "{dv_param_name}"
```

### Concrete Example

```toml
[data.dataviews.create_order]
datasource  = "orders_db"
post_query  = "INSERT INTO orders (customer_id, amount, status) VALUES ($customer_id, $amount, $status)"
post_schema = "schemas/order_create.schema.json"
invalidates = ["list_orders", "get_order"]

[[data.dataviews.create_order.post.parameters]]
name     = "customer_id"
type     = "uuid"
required = true

[[data.dataviews.create_order.post.parameters]]
name     = "amount"
type     = "decimal"
required = true

[[data.dataviews.create_order.post.parameters]]
name     = "status"
type     = "string"
required = false
default  = "pending"

[api.views.create_order]
path      = "/api/orders"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.create_order.handler]
type     = "dataview"
dataview = "create_order"

[api.views.create_order.parameter_mapping.body]
customer_id = "customer_id"
amount      = "amount"
status      = "status"
```

### Constraints
- No `RETURNING *` → see `ANTI:RETURNING-CLAUSE`. Use follow-up read if needed.
- `method = "POST"` on the View triggers `post_query`. Without it, the engine calls `get_query` (which doesn't exist).
- `invalidates` clears listed DataView caches on successful write.

### Failure Modes
- Using `RETURNING *` → driver error on SQLite. Drop it.
- Forgetting `method = "POST"` on write View → engine tries `get_query` → "unknown operation" error.
- Need read-back after write → escalate to `RECIPE:LOOKUP-THEN-WRITE`.

### Escalation

| Signal | Escalate to |
|--------|-------------|
| Write + read the result back | `RECIPE:LOOKUP-THEN-WRITE` |
| Multiple writes atomically | `RECIPE:ATOMIC-MULTI-WRITE` |
| Cross-datasource writes | `RECIPE:CROSS-DATASOURCE` |
| Conditional logic before write | `RECIPE:REST-HANDLER-BACKED` |

---

## RECIPE:PARAMETERIZED-READ

### Decision
Select when the read query takes multiple required parameters from mixed sources (path + query string). Same structural pattern as `RECIPE:SINGLE-READ` but with richer parameter mapping.

### Dependencies
- Same as `RECIPE:SINGLE-READ`

### Concrete Example

```toml
[data.dataviews.get_order_item]
datasource    = "orders_db"
query         = "SELECT id, product_name, quantity, price FROM order_items WHERE order_id = $order_id AND item_id = $item_id"
return_schema = "schemas/order_item.schema.json"

[[data.dataviews.get_order_item.parameters]]
name     = "order_id"
type     = "uuid"
required = true

[[data.dataviews.get_order_item.parameters]]
name     = "item_id"
type     = "uuid"
required = true

[api.views.get_order_item]
path      = "/api/orders/{order_id}/items/{item_id}"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.get_order_item.handler]
type     = "dataview"
dataview = "get_order_item"

[api.views.get_order_item.parameter_mapping.path]
order_id = "order_id"
item_id  = "item_id"
```

### Constraints
- Use named parameters (`$param_name`), not positional (`$1, $2`).
- Every `$variable` in query must have a matching `[[parameters]]` entry.
- Deliberately use non-alphabetical parameter order in tests to catch binding bugs (Issue #54 class).

---

## RECIPE:FILTERED-LIST

### Decision
Select when the requirement is: list endpoint with pagination and optional filters. If the user says "list," "search," "browse," "paginated," this is the recipe. No handler needed unless filter logic is complex.

### Dependencies
- Depends on: nothing
- Combines with: `RECIPE:CACHED-DATAVIEW`, `RECIPE:CACHE-INVALIDATION`

### Concrete Example

```toml
[data.dataviews.list_orders]
datasource    = "orders_db"
query         = "SELECT id, customer_id, amount, status, created_at FROM orders WHERE (status = $status OR $status IS NULL) ORDER BY created_at DESC LIMIT $limit OFFSET $offset"
return_schema = "schemas/order.schema.json"

[[data.dataviews.list_orders.parameters]]
name     = "status"
type     = "string"
required = false

[[data.dataviews.list_orders.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[[data.dataviews.list_orders.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0

[api.views.list_orders]
path            = "/api/orders"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "session"

[api.views.list_orders.handler]
type     = "dataview"
dataview = "list_orders"

[api.views.list_orders.parameter_mapping.query]
status = "status"
limit  = "limit"
offset = "offset"
```

### Constraints
- Known issue: parameter `default` declaration may not substitute at runtime — Rivers passes NULL. Use `COALESCE($param, value)` in SQL as workaround.
- `response_format = "envelope"` wraps results in `{ "data": [...], "meta": { "count", "total", "limit", "offset" } }`.

### Failure Modes
- Complex filter combinations (AND/OR/nested) → escalate to `RECIPE:REST-HANDLER-BACKED` where handler builds dynamic filter logic.
- Relying on `default` declaration without COALESCE → NULL hits NOT NULL column → row rejected.

---

## RECIPE:MULTI-QUERY-READ

### Decision
Select when the response requires data from multiple DataViews — joining order + items, product + reviews, user + permissions. A handler calls each DataView by name and merges results. All reads, no writes.

### Dependencies
- Depends on: N instances of `RECIPE:SINGLE-READ` or `RECIPE:PARAMETERIZED-READ` (each constituent DataView)
- Depended on by: `RECIPE:VIEW-PIPELINE` (as an alternative pattern)

### Composition
First create each DataView individually (each is a single-read recipe). Then create the handler that calls them by name. Then create the View pointing to the handler.

### Concrete Example

```toml
[data.dataviews.get_order]
datasource = "orders_db"
query      = "SELECT id, customer_id, amount, status FROM orders WHERE id = $order_id"

[[data.dataviews.get_order.parameters]]
name     = "order_id"
type     = "uuid"
required = true

[data.dataviews.get_order_items]
datasource = "orders_db"
query      = "SELECT id, product_name, quantity, price FROM order_items WHERE order_id = $order_id"

[[data.dataviews.get_order_items.parameters]]
name     = "order_id"
type     = "uuid"
required = true

[api.views.get_order_detail]
path      = "/api/orders/{id}"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.get_order_detail.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "getOrderDetail"
resources  = ["orders_db"]
```

```typescript
export async function getOrderDetail(ctx: ViewContext): Promise<Rivers.Response> {
    const order_id = ctx.request.params.id;

    const order = await Rivers.view.query("get_order", { order_id });
    const items = await Rivers.view.query("get_order_items", { order_id });

    if (order.rows.length === 0) {
        return { status: 404, body: { error: "order not found" } };
    }

    return {
        status: 200,
        body: { ...order.rows[0], items: items.rows },
    };
}
```

### Constraints
- Handler calls DataViews by name — no raw SQL.
- `resources` must list all datasources.
- Each constituent DataView is independently cacheable.

### Failure Modes
- Writing raw SQL in the handler → `ANTI:RAW-SQL-IN-HANDLER`.
- If atomicity is needed across reads → `RECIPE:SIMPLE-TRANSACTION`.

---

## RECIPE:LOOKUP-THEN-WRITE

### Decision
Select when: read some data, use it to validate or compute, then write. The read informs the write. Handler required for the logic bridge.

### Dependencies
- Depends on: `RECIPE:SINGLE-READ` (for lookup DataView) + `RECIPE:SINGLE-WRITE` (for write DataView)
- Escalates to: `RECIPE:ATOMIC-MULTI-WRITE` if atomicity between read and write matters

### Concrete Example

```toml
[data.dataviews.get_customer]
datasource = "orders_db"
query      = "SELECT id, credit_limit, current_balance FROM customers WHERE id = $customer_id"

[[data.dataviews.get_customer.parameters]]
name     = "customer_id"
type     = "uuid"
required = true

[data.dataviews.create_order]
datasource = "orders_db"
query      = "INSERT INTO orders (customer_id, amount, status) VALUES ($customer_id, $amount, $status)"

[[data.dataviews.create_order.parameters]]
name     = "customer_id"
type     = "uuid"
required = true

[[data.dataviews.create_order.parameters]]
name     = "amount"
type     = "decimal"
required = true

[[data.dataviews.create_order.parameters]]
name     = "status"
type     = "string"
required = true
```

```typescript
export async function placeOrder(ctx: ViewContext): Promise<Rivers.Response> {
    const { customer_id, amount } = ctx.request.body;

    const customer = await Rivers.view.query("get_customer", { customer_id });
    if (customer.rows.length === 0) {
        return { status: 404, body: { error: "customer not found" } };
    }

    const { credit_limit, current_balance } = customer.rows[0];
    if (current_balance + amount > credit_limit) {
        return { status: 422, body: { error: "exceeds credit limit" } };
    }

    await Rivers.view.query("create_order", { customer_id, amount, status: "pending" });
    return { status: 201, body: { message: "order placed" } };
}
```

### Constraints
- Without a transaction, the read and write are NOT atomic — another request could change balance between the read and write.
- If that race condition matters, wrap in a transaction → `RECIPE:ATOMIC-MULTI-WRITE`.

---

## RECIPE:CONDITIONAL-WRITE

### Decision
Select when: a write operation depends on the result of a previous query in the same transaction. The distinguishing feature is `tx.peek()` — inspecting intermediate results to decide whether to proceed.

### Dependencies
- Depends on: `RECIPE:SINGLE-READ` + `RECIPE:SINGLE-WRITE` DataViews
- Is a specialization of: `RECIPE:ATOMIC-MULTI-WRITE` with branching logic

### Concrete Example

```toml
[data.dataviews.check_inventory]
datasource = "store_db"
query      = "SELECT quantity FROM inventory WHERE product_id = $product_id"

[[data.dataviews.check_inventory.parameters]]
name     = "product_id"
type     = "uuid"
required = true

[data.dataviews.decrement_inventory]
datasource = "store_db"
query      = "UPDATE inventory SET quantity = quantity - $qty WHERE product_id = $product_id"

[[data.dataviews.decrement_inventory.parameters]]
name     = "product_id"
type     = "uuid"
required = true

[[data.dataviews.decrement_inventory.parameters]]
name     = "qty"
type     = "integer"
required = true

[data.dataviews.create_shipment]
datasource = "store_db"
query      = "INSERT INTO shipments (order_id, product_id, quantity) VALUES ($order_id, $product_id, $qty)"

[[data.dataviews.create_shipment.parameters]]
name     = "order_id"
type     = "uuid"
required = true

[[data.dataviews.create_shipment.parameters]]
name     = "product_id"
type     = "uuid"
required = true

[[data.dataviews.create_shipment.parameters]]
name     = "qty"
type     = "integer"
required = true
```

```typescript
export async function fulfillOrder(ctx: ViewContext): Promise<Rivers.Response> {
    const { order_id, product_id, qty } = ctx.request.body;

    const tx = Rivers.db.tx.begin("store_db");

    try {
        tx.query("check_inventory", { product_id });

        const inventory = tx.peek("check_inventory");
        if (inventory[0].rows.length === 0 || inventory[0].rows[0].quantity < qty) {
            tx.rollback();
            return { status: 422, body: { error: "insufficient inventory" } };
        }

        tx.query("decrement_inventory", { product_id, qty });
        tx.query("create_shipment", { order_id, product_id, qty });

        const results = tx.commit();
        return { status: 201, body: { shipment: results["create_shipment"][0] } };
    } catch (e) {
        return { status: 500, body: { error: e.message } };
    }
}
```

### Key API: `tx.peek()`
- Returns `Array<QueryResult>` — access via `[0]`
- Shows the result the driver returned, but it is NOT final until `tx.commit()`
- Use for branching decisions, not as the authoritative result

---

## RECIPE:ATOMIC-MULTI-WRITE

### Decision
Select when: multiple write operations that must succeed or fail together. This is the workhorse pattern for complex mutations. Keywords: "atomically," "all or nothing," "transaction," "batch update."

### Dependencies
- Depends on: N instances of `RECIPE:SINGLE-WRITE` (each DataView is a single-statement write)
- Optionally depends on: `RECIPE:SINGLE-READ` DataViews (for a final read-back)
- Depended on by: `RECIPE:MCP-MULTI-STEP`

### Composition
1. Create each write DataView individually (one statement each)
2. Optionally create a read DataView for the final state
3. Create the handler with `Rivers.db.tx.begin()` / `tx.query()` / `tx.commit()`
4. Create the View pointing to the handler

### Concrete Example

```toml
# Each DataView is ONE statement
[data.dataviews.archive_wip]
datasource = "cb_data"
query      = "INSERT INTO tasks(name, project_id) SELECT name, project_id FROM wip WHERE work_item_goal_id = $goal_id"

[[data.dataviews.archive_wip.parameters]]
name     = "goal_id"
type     = "string"
required = true

[data.dataviews.clear_wip]
datasource = "cb_data"
query      = "DELETE FROM wip WHERE work_item_goal_id = $goal_id AND project_id = $project_id"

[[data.dataviews.clear_wip.parameters]]
name     = "goal_id"
type     = "string"
required = true

[[data.dataviews.clear_wip.parameters]]
name     = "project_id"
type     = "string"
required = true

[data.dataviews.mark_goal_complete]
datasource = "cb_data"
query      = "UPDATE work_item_goals SET status = 'complete', completed_at = datetime('now') WHERE id = $goal_id AND project_id = $project_id AND status = 'active'"

[[data.dataviews.mark_goal_complete.parameters]]
name     = "goal_id"
type     = "string"
required = true

[[data.dataviews.mark_goal_complete.parameters]]
name     = "project_id"
type     = "string"
required = true

[data.dataviews.clear_project_context]
datasource = "cb_data"
query      = "UPDATE projects SET active_work_item_goal_id = NULL, active_wip_id = NULL WHERE id = $project_id"

[[data.dataviews.clear_project_context.parameters]]
name     = "project_id"
type     = "string"
required = true

[data.dataviews.get_goal]
datasource = "cb_data"
query      = "SELECT * FROM work_item_goals WHERE id = $goal_id"

[[data.dataviews.get_goal.parameters]]
name     = "goal_id"
type     = "string"
required = true
```

```typescript
export async function completeGoal(ctx: ViewContext): Promise<Rivers.Response> {
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

### Transaction API Reference
- `Rivers.db.tx.begin(datasource)` — sync. Checks out connection, sends BEGIN.
- `tx.query(dataview_name, params)` — sync. Executes on txn connection, returns void.
- `tx.peek(name)` — returns `Array<QueryResult>` of pending results. Not final.
- `tx.commit()` — sync. Sends COMMIT, returns `HashMap<string, Array<QueryResult>>`. These are final.
- `tx.rollback()` — sync. Sends ROLLBACK, releases connection.
- Auto-rollback on handler exit without commit/rollback — logged at WARN.
- Same DataView name called N times: `results["name"][0]`, `results["name"][1]`, etc.

### Constraints
- All DataViews MUST use the same datasource as `tx.begin()`.
- Transactions are sync — no `await`. Each `tx.query()` blocks until the driver returns.
- One statement per DataView. The multi-write behavior comes from the handler's sequence of `tx.query()` calls.

### Failure Modes
- Stuffing all SQL into one query field → `ANTI:MULTI-STATEMENT-SQL`. SQLite silently drops statements after the first.
- Splitting into N MCP tools → `ANTI:SPLIT-TOOL-SEQUENCING`. Partial failure is unrecoverable.
- Cross-datasource in one transaction → not supported. Escalate to `RECIPE:CROSS-DATASOURCE` with manual compensation.

---

## RECIPE:CROSS-DATASOURCE

### Decision
Select when the handler needs data from or writes to multiple datasources. There is NO cross-datasource atomicity — each datasource is its own transaction boundary. The handler is responsible for compensation logic if a step fails after a previous step committed.

### Dependencies
- Depends on: DataViews on each datasource

### Concrete Example

```toml
[data.dataviews.get_order]
datasource = "orders_db"
query      = "SELECT id, customer_id, amount FROM orders WHERE id = $order_id"

[[data.dataviews.get_order.parameters]]
name     = "order_id"
type     = "uuid"
required = true

[data.dataviews.get_shipping]
datasource = "shipping_db"
query      = "SELECT tracking_number, status, eta FROM shipments WHERE order_id = $order_id"

[[data.dataviews.get_shipping.parameters]]
name     = "order_id"
type     = "uuid"
required = true

[api.views.order_with_shipping]
path      = "/api/orders/{id}/full"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.order_with_shipping.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "getOrderWithShipping"
resources  = ["orders_db", "shipping_db"]
```

```typescript
export async function getOrderWithShipping(ctx: ViewContext): Promise<Rivers.Response> {
    const order_id = ctx.request.params.id;

    const order = await Rivers.view.query("get_order", { order_id });
    const shipping = await Rivers.view.query("get_shipping", { order_id });

    if (order.rows.length === 0) {
        return { status: 404, body: { error: "order not found" } };
    }

    return {
        status: 200,
        body: { ...order.rows[0], shipping: shipping.rows[0] || null },
    };
}
```

### Constraints
- `resources` MUST list ALL datasources.
- No cross-datasource transactions.
- For cross-datasource writes, handler owns compensation/recovery.

---

## RECIPE:PSEUDO-DATAVIEW

### Decision
Select when prototyping a one-off query that may not survive iteration. The pseudo DataView builder lets you skip TOML declaration and write SQL in the handler. This is the escape hatch — intentionally inconvenient to discourage permanent use.

### Dependencies
- Depends on: nothing
- Promotion path: pseudo → declared TOML DataView → handler may disappear

### Concrete Example

```typescript
export async function adHocReport(ctx: ViewContext): Promise<Rivers.Response> {
    const reportView = ctx.datasource("analytics_db")
        .fromQuery("SELECT department, SUM(amount) as total FROM expenses WHERE year = $1 GROUP BY department")
        .withGetSchema({
            driver: "postgresql",
            type: "object",
            fields: [
                { name: "department", type: "string", required: true },
                { name: "total", type: "decimal", required: true },
            ],
        })
        .build();

    const result = await reportView({ year: ctx.request.query.year });
    return { status: 200, body: result };
}
```

### Constraints
- No caching, no cache invalidation, no streaming, no EventBus.
- `.build()` creates, does NOT execute. Schema syntax-checked at build time.
- If the query persists → promote to TOML.

---

# View Patterns

---

## RECIPE:REST-CRUD

### Decision
Select when: standard CRUD resource with all four operations. One DataView, four method-specific queries, no handler. This is the most declarative pattern in Rivers — entire REST API from config.

### Dependencies
- Internally composes: `RECIPE:SINGLE-READ` (GET leg) + `RECIPE:SINGLE-WRITE` (POST/PUT/DELETE legs)

### Concrete Example

```toml
[data.dataviews.orders]
datasource = "orders_db"

get_query    = "SELECT id, customer_id, amount, status FROM orders WHERE id = $id"
post_query   = "INSERT INTO orders (customer_id, amount, status) VALUES ($customer_id, $amount, $status)"
put_query    = "UPDATE orders SET amount = $amount, status = $status WHERE id = $id"
delete_query = "DELETE FROM orders WHERE id = $id"

[[data.dataviews.orders.get.parameters]]
name     = "id"
type     = "uuid"
required = true

[[data.dataviews.orders.post.parameters]]
name     = "customer_id"
type     = "uuid"
required = true

[[data.dataviews.orders.post.parameters]]
name     = "amount"
type     = "decimal"
required = true

[[data.dataviews.orders.post.parameters]]
name     = "status"
type     = "string"
required = false
default  = "pending"

[[data.dataviews.orders.put.parameters]]
name     = "id"
type     = "uuid"
required = true

[[data.dataviews.orders.put.parameters]]
name     = "amount"
type     = "decimal"
required = true

[[data.dataviews.orders.put.parameters]]
name     = "status"
type     = "string"
required = true

[[data.dataviews.orders.delete.parameters]]
name     = "id"
type     = "uuid"
required = true

[api.views.orders]
path      = "/api/orders/{id}"
view_type = "Rest"
auth      = "session"

[api.views.orders.handler]
type     = "dataview"
dataview = "orders"

[api.views.orders.parameter_mapping.path]
id = "id"

[api.views.orders.parameter_mapping.body]
customer_id = "customer_id"
amount      = "amount"
status      = "status"
```

### Constraints
- HTTP method determines which query fires automatically.
- Each method has its own parameter set.
- No `RETURNING *` on write queries.

---

## RECIPE:REST-READONLY

### Decision
Alias for `RECIPE:SINGLE-READ`. Use shorthand fields: `query` = `get_query`, `return_schema` = `get_schema`, `parameters` = `get.parameters`.

---

## RECIPE:REST-HANDLER-BACKED

### Decision
Select when the view needs logic that pure DataView declaration can't express: conditional responses, multiple DataView calls, data transformation, side effects. The handler is a CodeComponent.

### Dependencies
- Depends on: DataViews the handler calls
- Combines with: any transaction or caching recipe

### Template

```toml
[api.views.{view_name}]
path      = "/api/{resource}/{path_param}"
method    = "{method}"
view_type = "Rest"
auth      = "{auth_mode}"

[api.views.{view_name}.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/{file}.ts"
entrypoint = "{function}"
resources  = ["{datasource1}"]
```

```typescript
export async function {function}(ctx: ViewContext): Promise<Rivers.Response> {
    const data = await Rivers.view.query("{dv_name}", { /* params */ });

    if (data.rows.length === 0) {
        return { status: 404, body: { error: "not found" } };
    }

    return { status: 200, body: data.rows };
}
```

### Constraints
- `resources` must list all datasources.
- Handler calls DataViews by name — no raw SQL.

---

## RECIPE:VIEW-PIPELINE

### Decision
Select when: a primary DataView result needs enrichment from other sources, or pre/post processing hooks. The pipeline stages are: `pre_process` → `on_request` → Primary → `transform` → `on_response` → `post_process`.

### Concrete Example

```toml
[api.views.order_detail]
path      = "/api/orders/{id}"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.order_detail.handler]
type     = "dataview"
dataview = "get_order"

[[api.views.order_detail.pre_process]]
module     = "handlers/audit.ts"
entrypoint = "logRequest"

[[api.views.order_detail.on_request]]
module     = "handlers/orders.ts"
entrypoint = "fetchCustomer"
key        = "customer"

[[api.views.order_detail.on_response]]
module     = "handlers/orders.ts"
entrypoint = "mergeOrderData"
key        = "merged"

[api.views.order_detail.parameter_mapping.path]
id = "id"
```

```typescript
export async function fetchCustomer(ctx: ViewContext): Promise<{ key: string; data: any } | null> {
    const order = ctx.sources["primary"];
    if (!order) return null;
    const customer = await Rivers.view.query("get_customer", { customer_id: order.customer_id });
    return { key: "customer", data: customer.rows[0] };
}

export async function mergeOrderData(ctx: ViewContext): Promise<{ key: string; data: any }> {
    return {
        key: "merged",
        data: { ...ctx.sources["primary"], customer: ctx.sources["customer"] },
    };
}
```

### Constraints
- `pre_process` / `post_process` — observers, void return, fire-and-forget.
- `on_request` / `on_response` — accumulators, return `{ key, data }` or null.
- Stages execute sequentially in declaration order.
- `ctx.sources["primary"]` populated by primary handler.

---

## RECIPE:SPA-WITH-API

### Decision
Select when serving a single-page application (React, Vue, Angular, Svelte) with API endpoints from the same app.

### Concrete Example

```json
{
  "appName": "dashboard",
  "type": "app-main",
  "appId": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "entryPoint": "https://0.0.0.0:3000",
  "spa_config": {
    "root_path": "dist/",
    "index_file": "index.html",
    "spa_fallback": true,
    "max_age": 86400
  }
}
```

### Constraints
- `/api/*` routes ALWAYS take precedence over SPA fallback.
- SPA config is ONLY valid on `app-main`.
- `spa_fallback = true` → unmatched paths return `index.html`.

---

## RECIPE:MULTI-DATASOURCE-VIEW

### Decision
Select when: primary data comes from SQL, enrichment from HTTP DataViews or other datasources. The View pipeline handles the merge. From the View layer's perspective, SQL and HTTP DataViews are identical.

### Concrete Example

```toml
[data.dataviews.get_product]
datasource = "products_db"
query      = "SELECT id, name, price FROM products WHERE id = $id"

[[data.dataviews.get_product.parameters]]
name     = "id"
type     = "uuid"
required = true

[data.dataviews.get_reviews]
datasource = "reviews_api"
query      = "/reviews?product_id={product_id}"
method     = "GET"

[[data.dataviews.get_reviews.parameters]]
name     = "product_id"
type     = "uuid"
required = true

[api.views.product_detail]
path      = "/api/products/{id}"
method    = "GET"
view_type = "Rest"

[api.views.product_detail.handler]
type     = "dataview"
dataview = "get_product"

[[api.views.product_detail.on_request]]
module     = "handlers/product.ts"
entrypoint = "fetchReviews"
key        = "reviews"

[[api.views.product_detail.on_response]]
module     = "handlers/product.ts"
entrypoint = "mergeProductData"
key        = "merged"

[api.views.product_detail.parameter_mapping.path]
id = "id"
```

### Constraints
- HTTP DataView `query` field is a URL path, not SQL.
- Enrichment stages run sequentially.

---

# MCP Tools

---

## RECIPE:MCP-READ-TOOL

### Decision
Select when: MCP tool reads data from a DataView. No handler needed. Same as `RECIPE:SINGLE-READ` but with `view_type = "Mcp"`.

### Concrete Example

```toml
[api.views.search_tasks]
path      = "/mcp/tools/search_tasks"
method    = "GET"
view_type = "Mcp"

[api.views.search_tasks.handler]
type     = "dataview"
dataview = "search_tasks_dv"

[api.views.search_tasks.mcp]
name        = "search_tasks"
description = "Search tasks by status and project"

[api.views.search_tasks.mcp.hints]
read_only   = true
destructive = false

[api.views.search_tasks.parameter_mapping.query]
status     = "status"
project_id = "project_id"
```

### Constraints
- `view_type = "Mcp"` — CamelCase, NOT `"MCP"`.
- `destructive = false` must be explicit on read tools — default is `true`.
- Without explicit `method`, Rivers dispatches to `get_query`/`query`.

---

## RECIPE:MCP-WRITE-TOOL

### Decision
Select when: MCP tool performs a single write operation. Same as `RECIPE:SINGLE-WRITE` with `view_type = "Mcp"`.

### Concrete Example

```toml
[api.views.add_task]
path      = "/mcp/tools/add_task"
method    = "POST"
view_type = "Mcp"

[api.views.add_task.handler]
type     = "dataview"
dataview = "add_task_dv"

[api.views.add_task.mcp]
name        = "add_task"
description = "Add a new task to a project"

[api.views.add_task.mcp.hints]
read_only   = false
destructive = false

[api.views.add_task.parameter_mapping.body]
name       = "name"
project_id = "project_id"
```

### Constraints
- MUST set `method = "POST"` for write tools.
- No `RETURNING *`.

---

## RECIPE:MCP-HANDLER-TOOL

### Decision
Select when: MCP tool needs conditional logic, calls multiple DataViews, or shapes the response.

### Concrete Example

```toml
[api.views.get_project_status]
path      = "/mcp/tools/get_project_status"
method    = "GET"
view_type = "Mcp"

[api.views.get_project_status.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/project.ts"
entrypoint = "getProjectStatus"
resources  = ["cb_data"]

[api.views.get_project_status.mcp]
name        = "get_project_status"
description = "Get full project status including active goals and WIP"

[api.views.get_project_status.mcp.hints]
read_only   = true
destructive = false
```

```typescript
export async function getProjectStatus(ctx: ViewContext): Promise<Rivers.Response> {
    const project_id = ctx.request.query.project_id;

    const project = await Rivers.view.query("get_project", { project_id });
    const goals = await Rivers.view.query("list_active_goals", { project_id });
    const wip = await Rivers.view.query("list_active_wip", { project_id });

    if (project.rows.length === 0) {
        return { status: 404, body: { error: "project not found" } };
    }

    return {
        status: 200,
        body: { project: project.rows[0], active_goals: goals.rows, active_wip: wip.rows },
    };
}
```

---

## RECIPE:MCP-MULTI-STEP

### Decision
Select when: MCP tool requires multiple atomic writes. This is the most important MCP pattern — it prevents the `ANTI:SPLIT-TOOL-SEQUENCING` failure mode. One tool, one handler, one transaction.

### Dependencies
- Depends on: N instances of `RECIPE:SINGLE-WRITE` DataViews
- Uses: `RECIPE:ATOMIC-MULTI-WRITE` transaction pattern

### Concrete Example

```toml
[api.views.complete_goal]
path      = "/mcp/tools/complete_goal"
method    = "POST"
view_type = "Mcp"

[api.views.complete_goal.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/goals.ts"
entrypoint = "completeGoal"
resources  = ["cb_data"]

[api.views.complete_goal.mcp]
name        = "complete_goal"
description = "Archive all WIP for a goal, mark goal complete, clear project context"

[api.views.complete_goal.mcp.hints]
read_only   = false
destructive = false
```

```typescript
export async function completeGoal(ctx: ViewContext): Promise<Rivers.Response> {
    const { goal_id, project_id } = ctx.request.body;

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

### Constraints
- ONE tool, ONE handler, ONE transaction. Never split.
- All referenced DataViews must exist as individual single-statement declarations.

### Failure Modes
- Splitting into N MCP tools → `ANTI:SPLIT-TOOL-SEQUENCING`. Partial failure with no recovery.
- Multi-statement SQL in one DataView → `ANTI:MULTI-STATEMENT-SQL`. Silent partial execution.

---

# Realtime

---

## RECIPE:WEBSOCKET-VIEW

### Decision
Select when: bidirectional real-time communication. Chat, gaming, collaborative editing.

### Concrete Example

```toml
[api.views.chat]
path      = "/ws/chat"
method    = "GET"
view_type = "Websocket"
auth      = "session"

[api.views.chat.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/chat.ts"
entrypoint = "onConnect"

[api.views.chat.ws_hooks]
on_connect.module      = "handlers/chat.ts"
on_connect.entrypoint  = "onConnect"
on_message.module      = "handlers/chat.ts"
on_message.entrypoint  = "onMessage"
on_disconnect.module   = "handlers/chat.ts"
on_disconnect.entrypoint = "onDisconnect"
```

### Constraints
- `method = "GET"` required.
- Handler MUST be CodeComponent.
- Text frames only — binary frames logged and discarded.
- Session revalidation: `session_revalidation_interval_s` for long-lived connections.

---

## RECIPE:SSE-VIEW

### Decision
Select when: server-to-client push (one-directional). Live dashboards, notifications, price feeds.

### Concrete Example

```toml
[api.views.dashboard_events]
path                 = "/api/events/dashboard"
method               = "GET"
view_type            = "ServerSentEvents"
auth                 = "session"
sse_tick_interval_ms = 1000
sse_event_buffer_size = 100

[api.views.dashboard_events.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/dashboard.ts"
entrypoint = "streamEvents"
resources  = ["events_db"]
```

### Constraints
- `method = "GET"` required.
- Handler MUST be CodeComponent.
- `sse_tick_interval_ms = 0` → pure event-driven, no polling.
- Supports `Last-Event-ID` reconnection replay.
- Combine with `RECIPE:POLLING-VIEW` for DataView-driven change detection.

---

## RECIPE:MESSAGE-CONSUMER

### Decision
Select when: processing broker messages (Kafka, etc.) asynchronously. No HTTP route.

### Concrete Example

```toml
[api.views.process_order_events]
view_type  = "MessageConsumer"

[api.views.process_order_events.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/order-events.ts"
entrypoint = "processOrderEvent"
resources  = ["orders_db", "events_kafka"]

[api.views.process_order_events.on_event]
topic       = "order.created"
handler     = "handlers/order-events.ts"
```

### Constraints
- NO HTTP route. Requests to path → `400 Bad Request`.
- Handler MUST be CodeComponent.
- EventBus event payload arrives as `request.body`.

---

## RECIPE:POLLING-VIEW

### Decision
Select when: SSE or WebSocket view should poll a DataView for changes and push updates to connected clients.

### Concrete Example

```toml
[api.views.price_feed]
path      = "/api/events/prices"
method    = "GET"
view_type = "ServerSentEvents"
auth      = "session"

[api.views.price_feed.handler]
type     = "dataview"
dataview = "get_latest_prices"

[api.views.price_feed.polling]
tick_interval_ms = 5000
diff_strategy    = "hash"
poll_state_ttl_s = 3600

[api.views.price_feed.polling.on_change]
module     = "handlers/prices.ts"
entrypoint = "onPriceChange"
```

### Constraints
- ONLY valid for `ServerSentEvents` and `Websocket` — NOT `Rest`.
- `diff_strategy`: `"hash"` (default), `"null"` (always fire), `"change_detect"` (custom handler).
- `on_change` handler REQUIRED.
- StorageEngine MUST be configured.
- Multi-node → Redis-backed StorageEngine required.

---

# Auth & Security

---

## RECIPE:AUTH-REQUIRED-VIEW

### Decision
Select when endpoint requires authenticated session.

### Template
```toml
auth = "session"
```

Session identity in handler: `ctx.session.identity.username`, `ctx.session.identity.groups`.

---

## RECIPE:AUTH-NONE-VIEW

### Decision
Select when endpoint is public.

### Template
```toml
auth = "none"
```

`on_session_valid` hook skipped. `ctx.session` not populated.

---

## RECIPE:SESSION-HANDLER

### Decision
Select when custom logic runs after session validation — loading permissions, tenant context.

### Concrete Example

```toml
[api.views.dashboard.on_session_valid]
module     = "handlers/auth.ts"
entrypoint = "loadUserContext"
```

```typescript
export async function loadUserContext(ctx: ViewContext): Promise<void> {
    const permissions = await Rivers.view.query("get_user_permissions", {
        username: ctx.session.identity.username,
    });
    ctx.meta["permissions"] = permissions.rows;
}
```

---

## RECIPE:API-KEY-AUTH

### Decision
Select when: machine-to-machine communication via API key.

### Template
```toml
auth = "apikey"
```

Key validated against `key_hash` table. `ctx.session.apikey` populated.

---

# Transactions

---

## RECIPE:SIMPLE-TRANSACTION

### Decision
Select when: multiple operations must succeed or fail together, single datasource, no conditional branching. This is `RECIPE:ATOMIC-MULTI-WRITE` without `tx.peek()`.

See `RECIPE:ATOMIC-MULTI-WRITE` for the full pattern.

---

## RECIPE:TRANSACTION-WITH-PEEK

### Decision
Select when: need to inspect intermediate results before proceeding. Adds `tx.peek()` to the transaction pattern.

See `RECIPE:CONDITIONAL-WRITE` for the full pattern.

---

## RECIPE:TRANSACTION-ROLLBACK

### Decision
Select when: need explicit error handling and rollback control.

### Key patterns:

```typescript
// Explicit rollback on business logic failure
const tx = Rivers.db.tx.begin("db");
tx.query("check_something", { id });
const pending = tx.peek("check_something");
if (pending[0].rows.length === 0) {
    tx.rollback();  // explicit — handler decides
    return { status: 404, body: { error: "not found" } };
}
tx.query("do_something", { id });
const results = tx.commit();
```

```typescript
// Auto-rollback on exception
const tx = Rivers.db.tx.begin("db");
try {
    tx.query("risky_op", { id });  // throws on driver error
    const results = tx.commit();
    return { status: 200, body: results };
} catch (e) {
    // auto-rollback already happened, logged at WARN
    return { status: 500, body: { error: e.message } };
}
```

### Constraints
- `tx.query()` throw → auto-rollback + WARN log.
- Handler exit without commit/rollback → auto-rollback + WARN log.
- Explicit `tx.rollback()` for controlled early exit.
- Rollback failures → `DriverError::Transaction`.

---

## RECIPE:DATAVIEW-TRANSACTION-FLAG

### Decision
Select when: a single DataView query needs explicit transaction wrapping. Not for multi-query — that's handler-level.

### Template

```toml
[data.dataviews.{dv_name}]
datasource  = "{datasource}"
transaction = true
post_query  = "UPDATE {table} SET {column} = {value} WHERE {condition}"
```

### Constraints
- `transaction = true` wraps ONE query in BEGIN/COMMIT.
- Independent of handler-level transactions.

---

# Caching

---

## RECIPE:CACHED-DATAVIEW

### Decision
Select when: read DataView benefits from caching to reduce database load.

### Template

```toml
[data.dataviews.{dv_name}.cache]
enabled     = true
ttl_seconds = {seconds}
```

### Constraints
- L1: in-process LRU, 150 MB default.
- Cache key: canonical JSON → SHA-256 → hex.
- Pseudo DataViews are never cached.

---

## RECIPE:CACHE-INVALIDATION

### Decision
Select when: write DataView should clear cached entries for related read DataViews.

### Template

```toml
[data.dataviews.create_order]
invalidates = ["list_orders", "get_order"]
```

### Constraints
- `invalidates` targets must be valid DataView names.
- Fires on successful execution only.

---

## RECIPE:NO-CACHE

### Decision
Select when: data must always be fresh. Omit the `[cache]` block entirely.

---

# Init & Lifecycle

---

## RECIPE:INIT-HANDLER-DDL

### Decision
Select when: app needs schema creation at startup. SQL tables, indexes.

### Concrete Example

```typescript
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    await ctx.ddl("orders_db", "CREATE TABLE IF NOT EXISTS orders (id UUID PRIMARY KEY, amount DECIMAL NOT NULL)");
    await ctx.ddl("orders_db", "CREATE INDEX IF NOT EXISTS idx_orders_customer ON orders(customer_id)");
    Rivers.log.info("Schema initialized");
}
```

### Constraints
- `ctx.ddl()` in init handler — NOT `Rivers.db.query()`.
- `database@appId` MUST be in `ddl_whitelist` in `riversd.toml`.
- DDL only in init context — never in view handlers.
- Missing whitelist entry → `DriverError::Forbidden` → FAILED state.

---

## RECIPE:INIT-HANDLER-SEED

### Decision
Select when: dev/test seed data at startup.

### Concrete Example

```typescript
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    await ctx.ddl("db", "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, name TEXT)");
    await ctx.query("db", "INSERT INTO users (id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING", ["admin", "Admin User"]);
    Rivers.log.info("Seed data loaded");
}
```

### Constraints
- DDL uses `ctx.ddl()`, seed data uses `ctx.query()`.
- `ctx.query()` does NOT require whitelist.
- Use `ON CONFLICT DO NOTHING` for idempotency.

---

## RECIPE:INIT-HANDLER-NOSQL

### Decision
Select when: MongoDB collection setup, Elasticsearch index creation, etc.

### Concrete Example

```typescript
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    await ctx.admin("catalog_db", "create_collection", {
        name: "products",
        validator: { $jsonSchema: { bsonType: "object", required: ["name", "price"] } },
    });
    await ctx.admin("catalog_db", "create_index", {
        collection: "products",
        keys: { name: 1 },
        options: { unique: true },
    });
}
```

### Constraints
- `ctx.admin()` for non-SQL admin operations.
- Both `ctx.ddl()` and `ctx.admin()` require whitelist entry.

---

# Error Handling

---

## RECIPE:HANDLER-ERROR-RESPONSE

### Decision
Select when: handler needs to return structured errors with appropriate HTTP status codes.

### Template
```typescript
if (data.rows.length === 0) {
    return { status: 404, body: { error: "item not found" } };
}
```

Error body format: `{ "error": "message" }`.

---

## RECIPE:DATAVIEW-NOT-FOUND

### Decision
Understand that: DataView returning zero rows is HTTP 200 with empty data, NOT 404. To return 404 on empty, you MUST use a CodeComponent handler.

Pure DataView-backed views return: `{ "data": [], "meta": { "count": 0 } }`.

---

## RECIPE:DRIVER-ERROR-HANDLING

### Decision
Select when: handler needs to catch and handle database errors gracefully.

### Template
```typescript
try {
    const result = await Rivers.view.query("risky_query", { params });
    return { status: 200, body: result };
} catch (e) {
    Rivers.log.error("query failed", { error: e.message });
    return { status: 500, body: { error: "internal error" } };
}
```

### Constraints
- Drivers never panic — all errors as `DriverError`.
- DO NOT expose raw driver errors to clients.

---

# Schema & Validation

---

## RECIPE:REQUEST-SCHEMA

### Decision
Select when: need to validate incoming request body against a schema.

### Template
```toml
post_schema = "schemas/{input_schema}.schema.json"
```

Schema validated BEFORE query execution.

---

## RECIPE:RESPONSE-SCHEMA

### Decision
Select when: need to validate query results match expected shape.

### Template
```toml
return_schema = "schemas/{output_schema}.schema.json"
```

Schema validated AFTER driver executes, BEFORE response returned.

---

## RECIPE:PARAMETER-DEFAULTS

### Decision
Understand that: `default` declaration in parameter blocks may NOT substitute at runtime — Rivers passes NULL. Always use COALESCE in SQL as a workaround.

```sql
SELECT * FROM orders WHERE status = COALESCE($status, 'active')
```

---

# Bundle & Project Setup

---

## RECIPE:NEW-BUNDLE

### Decision
Select when starting a new Rivers application. This is the first recipe for any project — it produces the directory structure, manifest, resources, and initial app config. Every other recipe depends on this skeleton existing.

### Dependencies
- Depends on: nothing — this is the root
- Depended on by: everything

### Directory Structure

```
{bundle_name}/
└── {app_name}/
    ├── manifest.toml
    ├── resources.toml
    ├── app.toml
    ├── schemas/
    │   └── {schema_name}.schema.json
    └── libraries/
        └── handlers/
            └── {handler_name}.ts
```

### manifest.toml

```toml
[app]
appName    = "{app_name}"
type       = "app-main"           # or "app-service"
appId      = "{uuid}"
entryPoint = "https://0.0.0.0:3000"

[app.init]
module     = "libraries/handlers/init.ts"
entrypoint = "initialize"

[app.spa_config]
root_path    = "dist/"
index_file   = "index.html"
spa_fallback = true
max_age      = 86400
```

### resources.toml

```toml
[[datasources]]
name               = "{datasource_name}"
driver             = "{driver}"
host               = "{host}"
port               = {port}
database           = "{database}"
username           = "{username}"
credentials_source = "lockbox://{lockbox_path}"
required           = true

[[services]]
name     = "{service_name}"
appId    = "{service_app_uuid}"
required = true
```

### Constraints
- `appId` must be unique UUID across all apps
- `app-service` cannot declare `[spa_config]`
- Every datasource referenced in DataViews must exist in `resources.toml`
- Create `CHANGELOG.md` at bundle root

---

## RECIPE:DATASOURCE-SQL

### Decision
Select when connecting to PostgreSQL, MySQL, or SQLite. These are the three built-in SQL drivers — no plugins needed.

### Dependencies
- Depends on: `RECIPE:NEW-BUNDLE` (resources.toml must exist)
- Depended on by: any DataView recipe

### PostgreSQL

```toml
[[datasources]]
name               = "orders_db"
driver             = "postgres"
host               = "db.internal"
port               = 5432
database           = "orders"
username           = "app"
credentials_source = "lockbox://postgres/orders-prod"
required           = true
```

### MySQL

```toml
[[datasources]]
name               = "users_db"
driver             = "mysql"
host               = "mysql.internal"
port               = 3306
database           = "users"
username           = "app"
credentials_source = "lockbox://mysql/users-prod"
required           = true
```

### SQLite

```toml
[[datasources]]
name       = "local_db"
driver     = "sqlite"
host       = "data/app.db"
nopassword = true
required   = true
```

### Constraints
- PostgreSQL/MySQL require `credentials_source` — LockBox resolves at startup
- SQLite: `nopassword = true`, `host` is file path
- All SQL drivers support transactions

---

## RECIPE:DATASOURCE-REDIS

### Decision
Select when using Redis as key-value store or cache. Redis is a first-class driver, not a plugin.

### Template

```toml
[[datasources]]
name               = "cache"
driver             = "redis"
host               = "redis.internal"
port               = 6379
credentials_source = "lockbox://redis/cache-prod"
required           = true
```

### Constraints
- Admin commands (`FLUSHDB`, `FLUSHALL`, `CONFIG SET`) blocked in view context
- Redis operations use operation tokens, not SQL

---

## RECIPE:DATASOURCE-NOSQL

### Decision
Select when connecting to MongoDB, Elasticsearch, CouchDB, or Cassandra. These are plugin drivers.

### MongoDB

```toml
[[datasources]]
name               = "catalog_db"
driver             = "mongodb"
host               = "mongo.internal"
port               = 27017
database           = "catalog"
credentials_source = "lockbox://mongodb/catalog-prod"
required           = true
```

### Elasticsearch

```toml
[[datasources]]
name               = "search"
driver             = "elasticsearch"
host               = "es.internal"
port               = 9200
credentials_source = "lockbox://elasticsearch/search-prod"
required           = true
```

### Constraints
- Plugin drivers must be in `plugins` directory
- Admin operations use `ctx.admin()` in init handler

---

## RECIPE:DATASOURCE-FAKER

### Decision
Select for synthetic test data without a real database. Faker generates data from schema definitions. Ideal for prototyping and development.

### Template

```toml
[[datasources]]
name       = "fake_data"
driver     = "faker"
nopassword = true
required   = true

[datasources.config]
locale                = "en_US"
seed                  = 42
max_records_per_query = 500
```

### DataView

```toml
[data.dataviews.list_contacts]
datasource    = "fake_data"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"
```

### Constraints
- `query` is a FILE PATH, not SQL — the schema defines both generation and return shape
- `seed` produces reproducible data — omit for random
- Faker attributes (`faker: "name.fullName"`) on non-faker schemas → validation error

---

## RECIPE:DATASOURCE-HTTP

### Decision
Select when calling an external REST API as a datasource or for service-to-service proxy calls. From the View layer, HTTP DataViews are identical to SQL DataViews.

### Template

```toml
[[datasources]]
name               = "reviews_api"
driver             = "http"
service            = "reviews-service"
credentials_source = "lockbox://http/reviews-api-key"
required           = true
```

### DataView

```toml
[data.dataviews.get_reviews]
datasource = "reviews_api"
query      = "/reviews?product_id={product_id}"
method     = "GET"

[[data.dataviews.get_reviews.parameters]]
name     = "product_id"
type     = "uuid"
required = true
```

### Constraints
- `query` is a URL path, not SQL
- Auth modes: bearer, basic (`username:password`), API key header
- No broker infrastructure required

---

## RECIPE:DATASOURCE-LDAP

### Decision
Select when querying LDAP/Active Directory for user lookups, authentication integration, or directory searches.

### Template

```toml
[[datasources]]
name               = "directory"
driver             = "ldap"
host               = "ldap.internal"
port               = 636
credentials_source = "lockbox://ldap/bind-creds"
required           = true
```

### DataView

```toml
[data.dataviews.find_user]
datasource = "directory"
query      = "(&(objectClass=person)(uid=$username))"
base_dn    = "ou=users,dc=company,dc=com"
scope      = "subtree"

[[data.dataviews.find_user.attributes]]
ldap_name  = "uid"
field_name = "username"
type       = "string"

[[data.dataviews.find_user.parameters]]
name     = "username"
type     = "string"
required = true
```

### Constraints
- `query` is LDAP filter string with `$param` substitution
- Escape user input per RFC 4515 — LDAP injection is real
- LDAPS preferred — cert verification ON by default

---

## RECIPE:DATASOURCE-KAFKA

### Decision
Select when publishing or consuming messages via Kafka. The presence or absence of a `[consumer]` block determines whether the datasource acts as producer or consumer.

### Producer

```toml
[[datasources]]
name               = "events_kafka"
driver             = "kafka"
host               = "kafka.internal"
port               = 9092
credentials_source = "lockbox://kafka/events-prod"
required           = true
```

### Consumer

```toml
[[datasources]]
name               = "events_consumer"
driver             = "kafka"
host               = "kafka.internal"
port               = 9092
credentials_source = "lockbox://kafka/events-prod"
required           = true

[datasources.consumer]
topics      = ["order.created", "order.updated"]
group_id    = "order-processor"
auto_offset = "earliest"
```

### Constraints
- `[consumer]` block → `MessageBrokerDriver` trait activates
- Without `[consumer]` → producer only, use via DataView `post_query`
- Consumer wires to `MessageConsumer` view type

---

## RECIPE:LOCKBOX-CREDENTIALS

### Decision
Select when wiring credentials for any datasource. LockBox is the credential store — secrets never appear in config files.

### Commands

```bash
rivers lockbox add postgres/orders-prod --value "postgresql://app:secret@db:5432/orders"
rivers lockbox alias postgres/orders-prod orders-db
rivers lockbox rotate postgres/orders-prod --value "postgresql://app:newsecret@db:5432/orders"
```

### Constraints
- Names: `[a-z][a-z0-9_/.-]*`, max 128 chars
- Names and aliases share one namespace
- Secrets decrypted per access — never cached in memory
- Rotation: no restart needed — next pool connection picks up new creds
- `nopassword = true` for credential-free datasources

---

## RECIPE:DDL-WHITELIST

### Decision
Select when configuring `riversd.toml` to allow DDL operations in init handlers. Without this, no app can execute DDL.

### Template

```toml
[security]
ddl_whitelist = [
    "orders@f47ac10b-58cc-4372-a567-0e02b2c3d479",
]
```

### Constraints
- `{database}@{appId}` — exact match required
- Empty/absent → no DDL allowed (safe default)
- No wildcard syntax

---

# Filesystem & Execution

---

## RECIPE:FILESYSTEM-OPS

### Decision
Select when the application needs sandboxed file operations. The filesystem driver provides chroot-like isolation — all paths resolve relative to the configured workspace root.

### Datasource

```toml
[[datasources]]
name       = "workspace"
driver     = "filesystem"
host       = "/var/data/workspace"
nopassword = true
required   = true
```

### DataViews

```toml
[data.dataviews.write_file]
datasource = "workspace"
post_query = "WRITE $path $content"

[[data.dataviews.write_file.post.parameters]]
name = "path"
type = "string"
required = true

[[data.dataviews.write_file.post.parameters]]
name = "content"
type = "string"
required = true

[data.dataviews.read_file]
datasource = "workspace"
query      = "READ $path"

[[data.dataviews.read_file.parameters]]
name = "path"
type = "string"
required = true

[data.dataviews.list_files]
datasource = "workspace"
query      = "LIST $directory"

[[data.dataviews.list_files.parameters]]
name    = "directory"
type    = "string"
required = false

[data.dataviews.stat_file]
datasource = "workspace"
query      = "STAT $path"

[[data.dataviews.stat_file.parameters]]
name = "path"
type = "string"
required = true
```

### Constraints
- Path traversal (`../`) rejected at driver level — not handler-level checks
- Workspace root is a datasource config — never hardcoded in handlers
- Search returns file references, not full content
- Stat returns structured metadata

---

## RECIPE:EXEC-DRIVER

### Decision
Select when the application needs to run whitelisted system commands. ExecDriver uses hash-pinning for security — only commands whose SHA-256 matches the allowlist execute.

### Datasource

```toml
[[datasources]]
name       = "exec"
driver     = "exec"
nopassword = true
required   = true

[datasources.config]
allowlist = [
    { command = "/usr/bin/wc", hash = "sha256:{hash_of_binary}" },
]
```

### DataView

```toml
[data.dataviews.word_count]
datasource = "exec"
query      = "wc $file_path"

[[data.dataviews.word_count.parameters]]
name     = "file_path"
type     = "string"
required = true
```

### Constraints
- Hash-pinned — non-whitelisted commands rejected
- Output returned as structured data, not raw stdout
- Path traversal in arguments blocked by driver
- Privilege drop if configured

---

# Message Broker Patterns

---

## RECIPE:KAFKA-PRODUCE

### Decision
Select when publishing messages to Kafka from handler code. Kafka producer is a datasource without a `[consumer]` block.

### Concrete Example

```typescript
export async function createOrder(ctx: ViewContext): Promise<Rivers.Response> {
    // ... create order ...
    await Rivers.view.query("publish_order_event", {
        payload: { order_id: newOrderId, customer_id, amount },
    });
    return { status: 201, body: { order_id: newOrderId } };
}
```

### Constraints
- No `[consumer]` block → producer mode
- Use `post_query` for publish operations

---

## RECIPE:KAFKA-CONSUME

### Decision
Select when processing Kafka messages asynchronously via the EventBus. Consumer datasource + MessageConsumer view type.

### Dependencies
- Depends on: `RECIPE:DATASOURCE-KAFKA` (consumer config), `RECIPE:MESSAGE-CONSUMER` (view type)

### Template

```toml
[api.views.process_events]
view_type = "MessageConsumer"

[api.views.process_events.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/events.ts"
entrypoint = "processEvent"
resources  = ["db", "events_consumer"]

[api.views.process_events.on_event]
topic   = "order.created"
handler = "handlers/events.ts"
```

### Constraints
- Events persisted in consumer handler, not producer
- MessageConsumer has no HTTP route

---

# Plugin Development

---

## RECIPE:PLUGIN-DRIVER

### Decision
Select when creating a custom driver plugin in Rust. Plugin drivers extend Rivers' datasource support without modifying the core.

### Configuration

```toml
[plugins]
enabled   = true
directory = "/var/rivers/plugins"
```

### Plugin Template

```rust
use rivers_driver_sdk::prelude::*;

pub struct MyDriver;

#[async_trait]
impl DatabaseDriver for MyDriver {
    fn name(&self) -> &str { "my-driver" }
    async fn connect(&self, params: &ConnectionParams) -> Result<Box<dyn Connection>, DriverError> {
        Ok(Box::new(MyConnection::new(params).await?))
    }
}

#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 { rivers_driver_sdk::ABI_VERSION }

#[no_mangle]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(MyDriver));
}
```

### Honest Stub Pattern

```rust
async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
    Err(DriverError::NotImplemented(
        format!("my-driver not yet implemented — operation: {}", query.operation)
    ))
}
```

### Constraints
- Exports `_rivers_abi_version` and `_rivers_register_driver`
- ABI mismatch → plugin rejected
- `execute()` MUST call `check_admin_guard()` — contractual obligation
- `NotImplemented` for stubs, `Unsupported` for permanent inability
- Registration wrapped in `catch_unwind` — panics don't crash the server
- All results normalize to `QueryResult { rows, affected_rows, last_insert_id }`

---

# Infrastructure & Server Config

---

## RECIPE:STORAGE-ENGINE

### Decision
Select when configuring session storage, L2 cache, polling state, or CSRF tokens. Required for any app using sessions or polling views.

### Templates

```toml
# Dev
[storage_engine]
backend = "memory"

# Single-node prod
[storage_engine]
backend = "sqlite"
path    = "/var/data/rivers.db"

# Multi-node prod
[storage_engine]
backend      = "redis"
url          = "redis://localhost:6379"
retention_ms = 172800000
```

### Constraints
- `memory` — dev only, lost on restart
- `sqlite` — WAL mode, single-node
- `redis` — required for multi-node shared state
- Polling views require StorageEngine — validation error without it

---

## RECIPE:CONNECTION-POOL

### Decision
Select when tuning datasource connection pools or circuit breaker behavior for resilience.

### Template

```toml
[data.datasources.orders_db.connection_pool]
min_idle           = 2
max_size           = 20
connection_timeout = 5000
idle_timeout       = 600000
max_lifetime       = 300000

[data.datasources.orders_db.connection_pool.circuit_breaker]
enabled              = true
failure_threshold    = 5
window_ms            = 60000
open_timeout_ms      = 10000
half_open_max_trials = 2
```

### Constraints
- Rolling window circuit breaker, not fixed-window
- Pool changes require full restart — hot reload doesn't touch pools

---

## RECIPE:RATE-LIMITING

### Decision
Select when configuring request rate limits. Two levels: app-wide default and per-view override.

### App Default

```toml
[app.rate_limit]
per_minute = 120
burst_size = 60
strategy   = "ip"
```

### Per-View Override

```toml
[api.views.search]
rate_limit_per_minute = 60
rate_limit_burst_size = 20
```

### Constraints
- Strategies: `ip` (default), `header` (custom header), `session` (username, falls back to ip)
- In-memory token bucket — resets on restart
- Per-app, not per-server

---

## RECIPE:CORS-CONFIG

### Decision
Select when configuring CORS. CORS is an init handler concern, not per-view config.

### Template

```typescript
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    ctx.app.cors({
        origins: ["https://app.example.com"],
        methods: ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
        headers: ["Content-Type", "Authorization"],
        credentials: false,
    });
}
```

### Constraints
- `origins: ["*"]` + `credentials: true` → rejected at startup
- Handler-set CORS headers blocked (SEC-8)

---

## RECIPE:LOGGING-CONFIG

### Decision
Select when configuring log output. Two fields, stdout only.

### Template

```toml
[base.logging]
level  = "info"
format = "json"
```

---

## RECIPE:TRACING-CONFIG

### Decision
Select when configuring OpenTelemetry distributed tracing.

### Template

```toml
[performance.tracing]
enabled       = true
provider      = "otlp"
endpoint      = "http://otel-collector:4317"
service_name  = "riversd"
sampling_rate = 0.1
```

---

## RECIPE:ENVIRONMENT-OVERRIDES

### Decision
Select when different environments need different config values.

### Template

```toml
[environment_overrides.prod.base]
host = "0.0.0.0"
port = 443

[environment_overrides.prod.security]
rate_limit_per_minute = 300
```

---

## RECIPE:HOT-RELOAD

### Decision
Select for dev mode auto-reload. Disabled in production.

### Template

```toml
[hot_reload]
enabled    = true
watch_path = "./app.toml"
```

### Constraints
- Reloads: views, DataViews, static files, security config, GraphQL
- Does NOT reload: HTTP server, pools, plugins, LockBox credentials

---

# Application Patterns (Extended)

---

## RECIPE:GRAPHQL-SETUP

### Decision
Select when enabling a GraphQL endpoint alongside REST. Schema auto-generates from DataViews.

### Template

```toml
[graphql]
enabled        = true
path           = "/graphql"
introspection  = true
max_depth      = 10
max_complexity = 1000
```

### Constraints
- DataViews with `return_schema` → auto-generated GraphQL types
- Mutations require CodeComponent resolvers
- Subscriptions via EventBus → SSE views
- Consider disabling introspection in production

---

## RECIPE:STREAMING-REST

### Decision
Select when a REST endpoint needs to stream large result sets or real-time data over a single HTTP request.

### Template

```toml
[api.views.export_data]
path             = "/api/data/export"
method           = "GET"
view_type        = "Rest"
streaming        = true
streaming_format = "ndjson"
stream_timeout_ms = 30000

[api.views.export_data.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/export.ts"
entrypoint = "exportData"
resources  = ["db"]
```

### Constraints
- Handler uses `{ chunk, done }` protocol
- `streaming_format`: `"ndjson"` or `"sse"`
- Handler must be CodeComponent

---

## RECIPE:MULTI-APP-BUNDLE

### Decision
Select when the application has a frontend (app-main) and backend service (app-service). This is the standard composition for SPA + API architectures.

### Dependencies
- Depends on: `RECIPE:NEW-BUNDLE` × 2 (one per app)
- Combines with: `RECIPE:SERVICE-TO-SERVICE` for inter-app calls

### Structure

```
my-bundle/
├── frontend/          # app-main
│   ├── manifest.toml
│   ├── resources.toml  # declares service dependency
│   └── app.toml
└── api-service/       # app-service
    ├── manifest.toml
    ├── resources.toml
    └── app.toml
```

### Constraints
- Each app: unique `appId`, unique `entryPoint` port
- `[[services]]` declares dependencies by `appId`
- Start services before main
- `app-service` cannot declare SPA config

---

## RECIPE:SERVICE-TO-SERVICE

### Decision
Select when one app calls another app's endpoints. Uses HTTP driver with `service` reference.

### Template

```toml
[[services]]
name     = "orders-api"
appId    = "f47ac10b-58cc-4372-a567-0e02b2c3d479"
required = true

[[datasources]]
name    = "orders-proxy"
driver  = "http"
service = "orders-api"
```

### Constraints
- Auth scope carry-over: `Authorization: Bearer` forwarded automatically
- Proxy DataViews identical to SQL DataViews from View layer perspective

---

## RECIPE:OUTBOUND-HTTP

### Decision
Select when calling a third-party API with no Rivers driver. This is an escape hatch — `Rivers.http` is not the default HTTP client.

### Template

```toml
[api.views.webhook_relay.handler]
type                = "codecomponent"
allow_outbound_http = true
```

```typescript
const result = await Rivers.http.post("https://external.com/webhook", body, {
    headers: { "Content-Type": "application/json" },
});
```

### Constraints
- `allow_outbound_http = true` required — logged at WARN on startup
- Each call logged at INFO with destination host
- Document why HTTP driver DataView doesn't suffice
- SSRF prevention via capability model

---

## RECIPE:WASM-HANDLER

### Decision
Select when a handler needs native-speed compute. Wasmtime runtime instead of V8.

### Template

```toml
[runtime.process_pools.wasm]
engine             = "wasmtime"
workers            = 4
max_memory_mb      = 128
task_timeout_ms    = 10000

[api.views.process]
process_pool = "wasm"

[api.views.process.handler]
type     = "codecomponent"
language = "wasm"
module   = "libraries/processors/compute.wasm"
```

### Constraints
- No dynamic imports at runtime
- Same capability model as V8
- Omitted `process_pool` → default V8 pool

---

# Schema Patterns

---

## RECIPE:SCHEMA-FILE

### Decision
Select when creating schema files. Every schema includes a `driver` field that routes to the correct validation engine. Schemas are driver-specific — a Redis schema and a Postgres schema are fundamentally different shapes.

### PostgreSQL

```json
{
    "driver": "postgresql",
    "type": "object",
    "fields": [
        { "name": "id", "type": "uuid", "required": true },
        { "name": "amount", "type": "decimal", "required": true, "min": 0 }
    ]
}
```

### Redis

```json
{
    "driver": "redis",
    "type": "hash",
    "key_pattern": "session:{session_id}",
    "fields": [
        { "name": "user_id", "type": "string", "required": true }
    ]
}
```

### Kafka

```json
{
    "driver": "kafka",
    "type": "message",
    "topic": "orders",
    "key": { "type": "uuid" },
    "value": {
        "fields": [
            { "name": "order_id", "type": "uuid", "required": true }
        ]
    }
}
```

### Constraints
- Schema validated at build time AND deploy time
- Faker attributes on non-faker schemas → validation error
- Per-method schemas: `get_schema`, `post_schema`, `put_schema`, `delete_schema`

---

# Operations

---

## RECIPE:BUNDLE-VALIDATION

### Decision
Select before deployment. Catches errors at build time.

### Command

```bash
riversctl validate <bundle_path>
```

### 9 Checks
View types, driver names, datasource refs, DataView refs, invalidates targets, duplicate names, schema files, cross-app service refs, TOML parse errors.

---

## RECIPE:GRACEFUL-SHUTDOWN

### Decision
Understanding only — Rivers handles SIGTERM/SIGINT automatically.

### Sequence
1. Stop accepting connections
2. 503 for new requests
3. Drain in-flight requests
4. Close datasource connections
5. Stop ProcessPool workers
6. Exit

---

## RECIPE:ERROR-RESPONSE-FORMAT

### Decision
Understanding only — Rivers' standard error envelope (not handler-produced errors).

### Format

```json
{
    "code": 500,
    "message": "human-readable error message",
    "details": "optional diagnostic info",
    "trace_id": "abc-123"
}
```

Key status codes: 400 (bad request), 401 (auth), 403 (RBAC), 404 (not found), 422 (schema validation), 429 (rate limit), 500 (runtime), 503 (draining/circuit open).

---

# Anti-Patterns

---

## ANTI:MULTI-STATEMENT-SQL

### Rule
NEVER put multiple SQL statements in one query field.

### What happens
SQLite `prepare()` parses to the first `;` only. Statements 2..N are silently dropped. No error. HTTP 200. Partial execution.

### Fix
One DataView per statement. Orchestrate in handler with transactions.

---

## ANTI:RAW-SQL-IN-HANDLER

### Rule
Handlers call DataViews by name, not raw SQL.

### What happens
`Rivers.db.query()` bypasses schema validation, caching, operation inference, and single-statement enforcement.

### Fix
Declare a DataView. Call it via `Rivers.view.query(name, params)`.

---

## ANTI:SPLIT-TOOL-SEQUENCING

### Rule
NEVER split one logical operation into multiple MCP tools and rely on the caller to sequence them.

### What happens
Step 2 fails after step 1 committed. No rollback. Inconsistent state. The caller (CC/agent) has no recovery mechanism.

### Fix
One tool, one handler, one transaction. See `RECIPE:MCP-MULTI-STEP`.

---

## ANTI:HANDLER-FOR-SIMPLE-CRUD

### Rule
DO NOT write a passthrough handler.

### What happens
Handler adds nothing — just calls the DataView and returns. The DataView + View declaration already does this.

### Fix
Bind View directly to DataView. Delete the handler.

---

## ANTI:RETURNING-CLAUSE

### Rule
DO NOT use `RETURNING *` in write queries.

### What happens
SQLite driver dispatches writes through `execute()` which expects zero rows. `RETURNING` produces rows → driver error.

### Fix
Follow-up read DataView, or `tx.query("get_item", { id })` after the write in a transaction.
