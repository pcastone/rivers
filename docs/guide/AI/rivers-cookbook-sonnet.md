# Rivers Cookbook — Build Reference (Sonnet)

**Purpose:** Complete pattern library for building Rivers applications. Each recipe is a self-contained, copy-paste template. Match the checklist, apply the template, obey the constraints.

**Rules:**
- ONE statement per query field. Semicolons in a query field → validation error.
- Handlers call DataViews by name. Never write SQL in handler code.
- `query` is the default query (handler dispatch via `Rivers.view.query()`).
- `get_query`, `post_query`, `put_query`, `delete_query` are for REST dispatch (HTTP method determines which fires).
- `transaction = true` on a DataView wraps its single query in BEGIN/COMMIT.
- Multi-query orchestration belongs in handlers, not in DataView TOML.
- `Rivers.db.query()` exists for raw SQL but is NOT the preferred path. Use DataViews.

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

### Match — use when ALL are true:
- [ ] One datasource
- [ ] One query (SELECT)
- [ ] No conditional logic
- [ ] No data transformation
- [ ] No side effects
- [ ] Parameters come from path or query string only

If any box is unchecked, STOP → see escalation table.

### Template

```toml
# --- DataView (app.toml) ---
[data.dataviews.{dv_name}]
datasource    = "{datasource_name}"
query         = "SELECT {columns} FROM {table} WHERE {column} = ${param}"
return_schema = "schemas/{schema_file}.schema.json"

[[data.dataviews.{dv_name}.parameters]]
name     = "{param_name}"
type     = "{type}"
required = true

# --- View (app.toml) ---
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
- DO NOT create a CodeComponent handler for passthrough reads
- DO NOT put multiple SQL statements in `query`
- DO NOT use `Rivers.db.query()` — the DataView handles dispatch
- DO NOT skip `parameter_mapping` and bind params in a handler

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Data from a second table | `RECIPE:MULTI-QUERY-READ` |
| Reshape the result | `RECIPE:VIEW-PIPELINE` |
| Per-row permission checks | `RECIPE:REST-HANDLER-BACKED` |
| External service call | `RECIPE:MULTI-DATASOURCE-VIEW` |

---

## RECIPE:SINGLE-WRITE

### Match — use when ALL are true:
- [ ] One datasource
- [ ] One write operation (INSERT, UPDATE, or DELETE)
- [ ] No reads needed after write (or follow-up read is separate)
- [ ] No conditional logic
- [ ] Parameters come from request body or path

If any box is unchecked, STOP → see escalation table.

### Template

```toml
# --- DataView (app.toml) ---
[data.dataviews.{dv_name}]
datasource = "{datasource_name}"
post_query = "INSERT INTO {table} ({columns}) VALUES ({$params})"

[[data.dataviews.{dv_name}.post.parameters]]
name     = "{param_name}"
type     = "{type}"
required = true

# --- View (app.toml) ---
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
- DO NOT use `RETURNING *` — it causes errors with `execute()`. Use a follow-up read DataView if you need the row back.
- DO NOT put multiple statements in `post_query`
- ONE statement per query field

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Write + read the result back | `RECIPE:LOOKUP-THEN-WRITE` |
| Multiple writes atomically | `RECIPE:ATOMIC-MULTI-WRITE` |
| Write to multiple datasources | `RECIPE:CROSS-DATASOURCE` |
| Conditional logic before write | `RECIPE:REST-HANDLER-BACKED` |

---

## RECIPE:PARAMETERIZED-READ

### Match — use when ALL are true:
- [ ] One datasource
- [ ] One query with multiple required parameters
- [ ] Parameters from path AND/OR query string
- [ ] No conditional logic
- [ ] No transformation

### Template

```toml
[data.dataviews.{dv_name}]
datasource    = "{datasource_name}"
query         = "SELECT {columns} FROM {table} WHERE {col1} = ${param1} AND {col2} = ${param2}"
return_schema = "schemas/{schema}.schema.json"

[[data.dataviews.{dv_name}.parameters]]
name     = "{param1}"
type     = "{type}"
required = true

[[data.dataviews.{dv_name}.parameters]]
name     = "{param2}"
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
{path_param} = "{dv_param1}"

[api.views.{view_name}.parameter_mapping.query]
{query_param} = "{dv_param2}"
```

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
- DO NOT use positional parameters ($1, $2) — use named parameters ($param_name)
- Every `$variable` in the query MUST have a matching `[[parameters]]` entry
- Deliberately use non-alphabetical parameter order in tests to catch binding bugs

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Optional filter parameters | `RECIPE:FILTERED-LIST` |
| Join across tables | `RECIPE:MULTI-QUERY-READ` |

---

## RECIPE:FILTERED-LIST

### Match — use when ALL are true:
- [ ] One datasource
- [ ] List/collection endpoint with pagination
- [ ] Optional filter parameters
- [ ] No handler logic needed

### Template

```toml
[data.dataviews.{dv_name}]
datasource    = "{datasource_name}"
query         = "SELECT {columns} FROM {table} WHERE ({col} = $filter OR $filter IS NULL) ORDER BY {sort_col} DESC LIMIT $limit OFFSET $offset"
return_schema = "schemas/{schema}.schema.json"

[[data.dataviews.{dv_name}.parameters]]
name     = "filter"
type     = "string"
required = false

[[data.dataviews.{dv_name}.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[[data.dataviews.{dv_name}.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0

[api.views.{view_name}]
path            = "/api/{resource}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "{auth_mode}"

[api.views.{view_name}.handler]
type     = "dataview"
dataview = "{dv_name}"

[api.views.{view_name}.parameter_mapping.query]
filter = "filter"
limit  = "limit"
offset = "offset"
```

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
- Use `COALESCE($param, default_value)` in SQL when the parameter `default` declaration doesn't substitute (known issue — Rivers passes NULL for omitted optional params)
- DO NOT create a handler just to add default values — handle in SQL
- `response_format = "envelope"` gives `{ "data": [...], "meta": { "count", "total", "limit", "offset" } }`

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Complex filter logic (AND/OR combos) | `RECIPE:REST-HANDLER-BACKED` |
| Full-text search | Driver-specific DataView (Elasticsearch) |

---

## RECIPE:MULTI-QUERY-READ

### Match — use when ALL are true:
- [ ] Need data from multiple DataViews in one response
- [ ] Single datasource OR multiple datasources
- [ ] Read-only (no writes)
- [ ] Handler merges results

If any box is unchecked, STOP → see escalation table.

### Template

```toml
# --- Multiple DataViews (app.toml) ---
[data.dataviews.{dv1_name}]
datasource = "{datasource_name}"
query      = "SELECT ... FROM {table1} WHERE ..."

[data.dataviews.{dv2_name}]
datasource = "{datasource_name}"
query      = "SELECT ... FROM {table2} WHERE ..."

# --- View with CodeComponent handler ---
[api.views.{view_name}]
path      = "/api/{resource}/{path_param}"
method    = "GET"
view_type = "Rest"
auth      = "{auth_mode}"

[api.views.{view_name}.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/{handler_file}.ts"
entrypoint = "{function_name}"
resources  = ["{datasource_name}"]
```

```typescript
// handlers/{handler_file}.ts
export async function {function_name}(ctx: ViewContext): Promise<Rivers.Response> {
    const {param} = ctx.request.params;

    const primary = await Rivers.view.query("{dv1_name}", { {param} });
    const related = await Rivers.view.query("{dv2_name}", { {param} });

    return {
        status: 200,
        body: {
            ...primary.rows[0],
            related: related.rows,
        },
    };
}
```

### Concrete Example

```toml
[data.dataviews.get_order]
datasource    = "orders_db"
query         = "SELECT id, customer_id, amount, status FROM orders WHERE id = $order_id"
return_schema = "schemas/order.schema.json"

[[data.dataviews.get_order.parameters]]
name     = "order_id"
type     = "uuid"
required = true

[data.dataviews.get_order_items]
datasource    = "orders_db"
query         = "SELECT id, product_name, quantity, price FROM order_items WHERE order_id = $order_id"
return_schema = "schemas/order_item.schema.json"

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
// handlers/orders.ts
export async function getOrderDetail(ctx: ViewContext): Promise<Rivers.Response> {
    const order_id = ctx.request.params.id;

    const order = await Rivers.view.query("get_order", { order_id });
    const items = await Rivers.view.query("get_order_items", { order_id });

    if (order.rows.length === 0) {
        return { status: 404, body: { error: "order not found" } };
    }

    return {
        status: 200,
        body: {
            ...order.rows[0],
            items: items.rows,
        },
    };
}
```

### Constraints
- Handler calls DataViews by name — NO raw SQL
- Each DataView is one query, one statement
- `resources` in the handler definition MUST list all datasources used

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Atomicity across the reads | `RECIPE:SIMPLE-TRANSACTION` |
| Writes mixed with reads | `RECIPE:ATOMIC-MULTI-WRITE` |
| Data from an external HTTP API | `RECIPE:MULTI-DATASOURCE-VIEW` |

---

## RECIPE:LOOKUP-THEN-WRITE

### Match — use when ALL are true:
- [ ] Need to read data, then use results as input to a write
- [ ] Single datasource
- [ ] Handler required for orchestration

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

[api.views.place_order]
path      = "/api/orders"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.place_order.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "placeOrder"
resources  = ["orders_db"]
```

```typescript
// handlers/orders.ts
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

    await Rivers.view.query("create_order", {
        customer_id,
        amount,
        status: "pending",
    });

    return { status: 201, body: { message: "order placed" } };
}
```

### Constraints
- Handler calls DataViews by name — NO raw SQL
- If the read+write must be atomic, wrap in a transaction → see `RECIPE:ATOMIC-MULTI-WRITE`

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Atomicity between read and write | `RECIPE:ATOMIC-MULTI-WRITE` |
| Multiple writes after lookup | `RECIPE:ATOMIC-MULTI-WRITE` |

---

## RECIPE:CONDITIONAL-WRITE

### Match — use when:
- [ ] Write operation depends on the result of a previous query in the same transaction
- [ ] Need to inspect intermediate results before deciding to proceed
- [ ] Single datasource

### Concrete Example

```toml
# DataViews — each is one statement
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
// handlers/shipments.ts
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

### Constraints
- `tx.peek()` returns pending results — NOT guaranteed to be final until `tx.commit()`
- `tx.peek("name")` returns an array — even if called once, access via `[0]`
- Auto-rollback happens if handler exits without `commit()` or `rollback()` — logged at WARN

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Cross-datasource atomicity | `RECIPE:CROSS-DATASOURCE` (no atomicity — manual compensation) |

---

## RECIPE:ATOMIC-MULTI-WRITE

### Match — use when ALL are true:
- [ ] Multiple write operations that must succeed or fail together
- [ ] Single datasource
- [ ] Handler required for orchestration

### Template

```typescript
export async function {function_name}(ctx: ViewContext): Promise<Rivers.Response> {
    const { /* params */ } = ctx.request.body;

    const tx = Rivers.db.tx.begin("{datasource}");

    try {
        tx.query("{write_dv_1}", { /* params */ });
        tx.query("{write_dv_2}", { /* params */ });
        tx.query("{write_dv_3}", { /* params */ });
        tx.query("{read_dv}", { /* params */ });

        const results = tx.commit();

        return { status: 200, body: results["{read_dv}"][0].rows };

    } catch (e) {
        return { status: 500, body: { error: e.message } };
    }
}
```

### Concrete Example

```toml
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
// handlers/goals.ts
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

### Constraints
- `tx.begin()` — sync, checks out one connection, sends BEGIN
- `tx.query(name, params)` — sync, executes on the txn connection, returns void
- `tx.commit()` — sync, sends COMMIT, returns `HashMap<string, Array<QueryResult>>`
- Every value in the results map is an ARRAY — even if called once, use `[0]`
- Calling same DataView name multiple times appends: `results["insert_task"][0]`, `results["insert_task"][1]`
- All DataViews MUST use the same datasource as the `tx.begin()` call
- Auto-rollback if handler exits without commit/rollback — logged at WARN

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Conditional logic between steps | `RECIPE:CONDITIONAL-WRITE` (add `tx.peek()`) |
| Cross-datasource writes | `RECIPE:CROSS-DATASOURCE` (no atomicity) |

---

## RECIPE:CROSS-DATASOURCE

### Match — use when:
- [ ] Need to read/write across multiple datasources
- [ ] No cross-datasource atomicity required (or manual compensation acceptable)
- [ ] Handler required

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
// handlers/orders.ts
export async function getOrderWithShipping(ctx: ViewContext): Promise<Rivers.Response> {
    const order_id = ctx.request.params.id;

    const order = await Rivers.view.query("get_order", { order_id });
    const shipping = await Rivers.view.query("get_shipping", { order_id });

    if (order.rows.length === 0) {
        return { status: 404, body: { error: "order not found" } };
    }

    return {
        status: 200,
        body: {
            ...order.rows[0],
            shipping: shipping.rows[0] || null,
        },
    };
}
```

### Constraints
- `resources` MUST list ALL datasources the handler accesses
- Transactions DO NOT span datasources — `tx.begin("orders_db")` cannot include queries against `shipping_db`
- Cross-datasource writes are NOT atomic — handle compensation/recovery in handler logic

---

## RECIPE:PSEUDO-DATAVIEW

### Match — use when:
- [ ] Prototyping a query that may not stick
- [ ] One-off query not worth declaring in TOML
- [ ] Handler code — not available in pure DataView context

### Concrete Example

```typescript
// handlers/admin.ts
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
- Pseudo DataViews have NO caching, NO cache invalidation, NO streaming, NO EventBus
- `.build()` creates the DataView — does NOT execute it
- Schema is syntax-checked at build time
- If the query persists across multiple handler iterations → promote to TOML

### Escalation

| If you need | Use instead |
|-------------|-------------|
| Caching | Promote to declared DataView in TOML |
| Cache invalidation | Promote to declared DataView in TOML |
| Reuse across handlers | Promote to declared DataView in TOML |

---

# View Patterns

---

## RECIPE:REST-CRUD

### Match — use when ALL are true:
- [ ] Standard CRUD resource (Create, Read, Update, Delete)
- [ ] One datasource
- [ ] All four operations on one path
- [ ] No handler logic needed

### Concrete Example

```toml
# --- DataView with per-method queries ---
[data.dataviews.orders]
datasource = "orders_db"

get_query    = "SELECT id, customer_id, amount, status FROM orders WHERE id = $id"
post_query   = "INSERT INTO orders (customer_id, amount, status) VALUES ($customer_id, $amount, $status)"
put_query    = "UPDATE orders SET amount = $amount, status = $status WHERE id = $id"
delete_query = "DELETE FROM orders WHERE id = $id"

# Per-method parameters
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

# --- View ---
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
- HTTP method determines which query fires — GET → `get_query`, POST → `post_query`, etc.
- DO NOT use `RETURNING *` on write queries
- Each method has its own parameter set
- One DataView, one View — full CRUD with zero handler code

---

## RECIPE:REST-READONLY

### Match — use when:
- [ ] GET-only resource (no writes)
- [ ] One datasource, one query

This is `RECIPE:SINGLE-READ`. Use the shorthand aliases:
- `query` → equivalent to `get_query`
- `return_schema` → equivalent to `get_schema`
- `parameters` → equivalent to `get.parameters`

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

---

## RECIPE:REST-HANDLER-BACKED

### Match — use when ANY are true:
- [ ] Need conditional logic
- [ ] Need to call multiple DataViews
- [ ] Need to transform data before response
- [ ] Need side effects (logging, events)

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
resources  = ["{datasource1}", "{datasource2}"]
```

```typescript
export async function {function}(ctx: ViewContext): Promise<Rivers.Response> {
    // Call DataViews by name
    const data = await Rivers.view.query("{dv_name}", { /* params */ });

    // Business logic
    if (data.rows.length === 0) {
        return { status: 404, body: { error: "not found" } };
    }

    return { status: 200, body: data.rows };
}
```

### Constraints
- `resources` MUST list ALL datasources the handler accesses
- Handler calls DataViews by name via `Rivers.view.query()` — NO raw SQL
- Undeclared datasource access → `CapabilityError`

---

## RECIPE:VIEW-PIPELINE

### Match — use when:
- [ ] Need pre/post processing around a primary DataView
- [ ] Need to enrich primary data with additional DataView results
- [ ] Pipeline stages: pre_process → on_request → Primary → transform → on_response → post_process

### Concrete Example

```toml
[api.views.order_detail]
path      = "/api/orders/{id}"
method    = "GET"
view_type = "Rest"
auth      = "session"

# Primary — DataView
[api.views.order_detail.handler]
type     = "dataview"
dataview = "get_order"

# Pre-process — observer, fire-and-forget
[[api.views.order_detail.pre_process]]
module     = "handlers/audit.ts"
entrypoint = "logRequest"

# On-request — accumulator, deposits into ctx.sources
[[api.views.order_detail.on_request]]
module     = "handlers/orders.ts"
entrypoint = "fetchCustomer"
key        = "customer"

[[api.views.order_detail.on_request]]
module     = "handlers/orders.ts"
entrypoint = "fetchShipping"
key        = "shipping"

# On-response — merge accumulated sources
[[api.views.order_detail.on_response]]
module     = "handlers/orders.ts"
entrypoint = "mergeOrderData"
key        = "merged"

[api.views.order_detail.parameter_mapping.path]
id = "id"
```

```typescript
// handlers/orders.ts
export async function fetchCustomer(ctx: ViewContext): Promise<{ key: string; data: any } | null> {
    const order = ctx.sources["primary"];
    if (!order) return null;

    const customer = await Rivers.view.query("get_customer", {
        customer_id: order.customer_id,
    });
    return { key: "customer", data: customer.rows[0] };
}

export async function fetchShipping(ctx: ViewContext): Promise<{ key: string; data: any } | null> {
    const shipping = await Rivers.view.query("get_shipping", {
        order_id: ctx.request.path_params.id,
    });
    return { key: "shipping", data: shipping.rows[0] };
}

export async function mergeOrderData(ctx: ViewContext): Promise<{ key: string; data: any }> {
    return {
        key: "merged",
        data: {
            ...ctx.sources["primary"],
            customer: ctx.sources["customer"],
            shipping: ctx.sources["shipping"],
        },
    };
}
```

### Constraints
- `pre_process` and `post_process` are observers — void return, fire-and-forget, errors logged but pipeline continues
- `on_request` and `on_response` are accumulators — return `{ key, data }` or `null`
- Pipeline stages execute sequentially in declaration order
- `ctx.sources["primary"]` is populated by the primary handler (DataView or CodeComponent)

---

## RECIPE:SPA-WITH-API

### Match — use when:
- [ ] Single-page application (React, Vue, Angular, Svelte)
- [ ] API endpoints served from same app

### Concrete Example

```json
// manifest.toml
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

```toml
# app.toml — API views
[api.views.list_orders]
path      = "/api/orders"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.list_orders.handler]
type     = "dataview"
dataview = "list_orders"
```

### Constraints
- `/api/*` routes ALWAYS take precedence over SPA fallback
- SPA config is ONLY valid on `app-main`, NOT `app-service`
- `spa_fallback = true` → any unmatched path returns `index.html`

---

## RECIPE:MULTI-DATASOURCE-VIEW

### Match — use when:
- [ ] Primary data from SQL DataView
- [ ] Enrichment from HTTP DataView or another datasource
- [ ] Need to merge results from different sources

### Concrete Example

```toml
# SQL DataView — primary
[data.dataviews.get_product]
datasource    = "products_db"
query         = "SELECT id, name, price, category FROM products WHERE id = $id"
return_schema = "schemas/product.schema.json"

[[data.dataviews.get_product.parameters]]
name     = "id"
type     = "uuid"
required = true

# HTTP DataView — enrichment
[data.dataviews.get_reviews]
datasource = "reviews_api"
query      = "/reviews?product_id={product_id}"
method     = "GET"

[[data.dataviews.get_reviews.parameters]]
name     = "product_id"
type     = "uuid"
required = true

# View with pipeline
[api.views.product_detail]
path      = "/api/products/{id}"
method    = "GET"
view_type = "Rest"
auth      = "none"

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

```typescript
// handlers/product.ts
export async function fetchReviews(ctx: ViewContext): Promise<{ key: string; data: any } | null> {
    const reviews = await Rivers.view.query("get_reviews", {
        product_id: ctx.request.path_params.id,
    });
    return { key: "reviews", data: reviews.rows };
}

export async function mergeProductData(ctx: ViewContext): Promise<{ key: string; data: any }> {
    return {
        key: "merged",
        data: {
            ...ctx.sources["primary"],
            reviews: ctx.sources["reviews"],
        },
    };
}
```

### Constraints
- SQL and HTTP DataViews are identical from the View layer's perspective
- HTTP DataView `query` field is the URL path, not SQL

---

# MCP Tools

---

## RECIPE:MCP-READ-TOOL

### Match — use when:
- [ ] MCP tool that reads data
- [ ] Backed by a single DataView
- [ ] No handler code needed

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

[data.dataviews.search_tasks_dv]
datasource = "cb_data"
query      = "SELECT id, name, status, project_id FROM tasks WHERE (status = $status OR $status IS NULL) AND project_id = $project_id"

[[data.dataviews.search_tasks_dv.parameters]]
name     = "status"
type     = "string"
required = false

[[data.dataviews.search_tasks_dv.parameters]]
name     = "project_id"
type     = "string"
required = true
```

### Constraints
- `view_type = "Mcp"` — note CamelCase, NOT all-caps `"MCP"`
- Set `destructive = false` explicitly on read tools — default is `true`
- Without `method = "POST"`, Rivers dispatches to `get_query` / `query`

---

## RECIPE:MCP-WRITE-TOOL

### Match — use when:
- [ ] MCP tool that writes data
- [ ] Single write operation
- [ ] No handler code needed

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

[data.dataviews.add_task_dv]
datasource = "cb_data"
post_query = "INSERT INTO tasks (name, project_id, status) VALUES ($name, $project_id, 'todo')"

[[data.dataviews.add_task_dv.post.parameters]]
name     = "name"
type     = "string"
required = true

[[data.dataviews.add_task_dv.post.parameters]]
name     = "project_id"
type     = "string"
required = true
```

### Constraints
- MUST set `method = "POST"` (or `"PUT"`) for write tools — without it, Rivers calls `get_query` which doesn't exist on a write DataView
- DO NOT use `RETURNING *` in `post_query` — causes driver error

---

## RECIPE:MCP-HANDLER-TOOL

### Match — use when:
- [ ] MCP tool needs conditional logic
- [ ] MCP tool calls multiple DataViews
- [ ] Need to shape the response

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
// handlers/project.ts
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
        body: {
            project: project.rows[0],
            active_goals: goals.rows,
            active_wip: wip.rows,
        },
    };
}
```

---

## RECIPE:MCP-MULTI-STEP

### Match — use when:
- [ ] MCP tool needs multiple writes atomically
- [ ] Complex operation that spans multiple DataViews

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
// handlers/goals.ts
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
- DO NOT split one logical operation into multiple MCP tools and rely on the caller to sequence them
- One tool, one handler, one transaction — the MCP client sees one atomic operation
- All DataViews referenced must exist as individual single-statement declarations

---

# Realtime

---

## RECIPE:WEBSOCKET-VIEW

### Match — use when:
- [ ] Bidirectional real-time communication
- [ ] Need connection lifecycle hooks

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
- `view_type = "Websocket"` requires `method = "GET"`
- Handler MUST be CodeComponent — DataView handlers not supported
- Binary frames are logged and discarded — text frames only
- Session revalidation: use `session_revalidation_interval_s` for long-lived connections

---

## RECIPE:SSE-VIEW

### Match — use when:
- [ ] Server-to-client push (one-directional)
- [ ] Event-driven or tick-based updates

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
- `view_type = "ServerSentEvents"` requires `method = "GET"`
- Handler MUST be CodeComponent
- `sse_tick_interval_ms = 0` → pure event-driven (no polling)
- Supports `Last-Event-ID` reconnection replay

---

## RECIPE:MESSAGE-CONSUMER

### Match — use when:
- [ ] Processing broker messages (Kafka, etc.)
- [ ] No HTTP route — driven by EventBus events

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
- MessageConsumer has NO HTTP route — requests to its path return `400 Bad Request`
- Handler MUST be CodeComponent
- EventBus event payload arrives as `request.body`

---

## RECIPE:POLLING-VIEW

### Match — use when:
- [ ] SSE or WebSocket view needs to poll a DataView for changes
- [ ] Push updates to connected clients on data change

### Concrete Example

```toml
[api.views.price_feed]
path                 = "/api/events/prices"
method               = "GET"
view_type            = "ServerSentEvents"
auth                 = "session"

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
- Polling is ONLY valid for `ServerSentEvents` and `Websocket` view types — NOT `Rest`
- `diff_strategy` options: `"hash"` (default), `"null"` (always fire), `"change_detect"` (custom handler)
- `on_change` handler is REQUIRED
- StorageEngine MUST be configured for polling views
- Multi-node deployments require Redis-backed StorageEngine

---

# Auth & Security

---

## RECIPE:AUTH-REQUIRED-VIEW

### Match — use when:
- [ ] Endpoint requires authenticated session

### Template

```toml
[api.views.{view_name}]
path      = "/api/{resource}"
method    = "{method}"
view_type = "Rest"
auth      = "session"

# ... handler config
```

### Constraints
- `auth = "session"` — requires valid session token
- Unauthenticated requests receive `401 Unauthorized`
- Session identity available in handler via `ctx.session.identity`

---

## RECIPE:AUTH-NONE-VIEW

### Match — use when:
- [ ] Public endpoint, no authentication needed

### Template

```toml
[api.views.{view_name}]
path      = "/api/{resource}"
method    = "{method}"
view_type = "Rest"
auth      = "none"

# ... handler config
```

### Constraints
- `auth = "none"` — no session validation
- `on_session_valid` hook is skipped entirely
- `ctx.session` is not populated

---

## RECIPE:SESSION-HANDLER

### Match — use when:
- [ ] Need custom logic after session validation
- [ ] Load user permissions, tenant context, etc.

### Concrete Example

```toml
[api.views.dashboard]
path      = "/api/dashboard"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.dashboard.on_session_valid]
module     = "handlers/auth.ts"
entrypoint = "loadUserContext"
```

```typescript
// handlers/auth.ts
export async function loadUserContext(ctx: ViewContext): Promise<void> {
    const permissions = await Rivers.view.query("get_user_permissions", {
        username: ctx.session.identity.username,
    });
    ctx.meta["permissions"] = permissions.rows;
}
```

### Constraints
- `on_session_valid` fires AFTER session validation, BEFORE `pre_process`
- Position configurable via `session_stage`: `"before_pre_process"` (default) or `"after_on_request"`

---

## RECIPE:API-KEY-AUTH

### Match — use when:
- [ ] API key authentication instead of session
- [ ] Machine-to-machine communication

### Concrete Example

```toml
[api.views.api_endpoint]
path      = "/api/data"
method    = "GET"
view_type = "Rest"
auth      = "apikey"

[api.views.api_endpoint.handler]
type     = "dataview"
dataview = "get_data"
```

### Constraints
- API key sent via `Authorization: Bearer {key}` header
- Key validated against `key_hash` in the keys table
- `ctx.session.apikey` populated on successful auth

---

# Transactions

---

## RECIPE:SIMPLE-TRANSACTION

### Match — use when:
- [ ] Multiple writes that must succeed or fail together
- [ ] Single datasource
- [ ] No conditional logic between steps

### Template

```typescript
export async function {function}(ctx: ViewContext): Promise<Rivers.Response> {
    const tx = Rivers.db.tx.begin("{datasource}");

    try {
        tx.query("{dv1}", { /* params */ });
        tx.query("{dv2}", { /* params */ });
        tx.query("{dv3}", { /* params */ });

        const results = tx.commit();
        return { status: 200, body: results };

    } catch (e) {
        return { status: 500, body: { error: e.message } };
    }
}
```

### Constraints
- `tx.begin()` — sync, checks out connection, sends BEGIN
- `tx.query()` — sync, returns void, result held internally
- `tx.commit()` — sync, sends COMMIT, returns `HashMap<string, Array<QueryResult>>`
- Auto-rollback on handler exit without commit — logged at WARN
- All DataViews must use the same datasource

---

## RECIPE:TRANSACTION-WITH-PEEK

### Match — use when:
- [ ] Need to inspect intermediate results before deciding to continue
- [ ] Branch logic mid-transaction

See `RECIPE:CONDITIONAL-WRITE` for the full pattern.

### Key API

```typescript
const tx = Rivers.db.tx.begin("db");

tx.query("check_something", { id });

const pending = tx.peek("check_something");
// pending is Array<QueryResult> — access via pending[0]
// NOT guaranteed final — data may change before commit

if (pending[0].rows.length === 0) {
    tx.rollback();
    return { status: 404, body: { error: "not found" } };
}

tx.query("do_something", { id });
const results = tx.commit();
```

### Constraints
- `tx.peek(name)` returns array — even if called once, use `[0]`
- Peek data is NOT final — only `tx.commit()` results are authoritative

---

## RECIPE:TRANSACTION-ROLLBACK

### Match — use when:
- [ ] Need explicit error handling on transaction failure
- [ ] Want control over rollback behavior

### Template

```typescript
const tx = Rivers.db.tx.begin("{datasource}");

try {
    tx.query("{dv1}", { /* params */ });
    tx.query("{dv2}", { /* params */ });

    const results = tx.commit();
    return { status: 200, body: results };

} catch (e) {
    // Auto-rollback already happened when the error threw
    // Error is logged by engine at WARN level
    Rivers.log.error("transaction failed", { error: e.message, trace_id: ctx.trace_id });
    return { status: 500, body: { error: e.message } };
}
```

### Constraints
- On `tx.query()` error: exception thrown, auto-rollback, logged at WARN
- On handler exit without commit/rollback: auto-rollback, logged at WARN
- Explicit `tx.rollback()` for controlled early exit (e.g., after peek reveals bad state)
- Rollback failures logged as `DriverError::Transaction`

---

## RECIPE:DATAVIEW-TRANSACTION-FLAG

### Match — use when:
- [ ] Single DataView query needs explicit transaction guarantees
- [ ] Not multi-query — just wrapping one statement in BEGIN/COMMIT

### Template

```toml
[data.dataviews.{dv_name}]
datasource  = "{datasource}"
transaction = true
post_query  = "UPDATE {table} SET {column} = {value} WHERE {condition}"
```

### Constraints
- `transaction = true` wraps the single query in BEGIN/COMMIT — that's all
- NOT required for multi-query — multi-query transactions are handler-level via `Rivers.db.tx`
- Useful when a single critical write needs explicit transaction guarantees

---

# Caching

---

## RECIPE:CACHED-DATAVIEW

### Match — use when:
- [ ] Read DataView with data that doesn't change frequently
- [ ] Want to reduce database load

### Template

```toml
[data.dataviews.{dv_name}]
datasource    = "{datasource}"
query         = "SELECT ... FROM ..."
return_schema = "schemas/{schema}.schema.json"

[data.dataviews.{dv_name}.cache]
enabled     = true
ttl_seconds = {seconds}
```

### Constraints
- L1 cache is in-process LRU per node — default 150 MB
- L2 cache requires StorageEngine (Redis or SQLite)
- Cache key: canonical JSON of params → SHA-256 → hex
- Pseudo DataViews are NEVER cached

---

## RECIPE:CACHE-INVALIDATION

### Match — use when:
- [ ] Write DataView should invalidate cached read DataViews

### Template

```toml
[data.dataviews.create_order]
datasource  = "orders_db"
post_query  = "INSERT INTO orders ..."
invalidates = ["list_orders", "get_order"]
```

### Constraints
- `invalidates` targets MUST reference valid DataView names — validation fails otherwise
- Invalidation fires on successful execution only
- Each target in `invalidates` clears ALL cached entries for that DataView

---

## RECIPE:NO-CACHE

### Match — use when:
- [ ] Data must always be fresh (real-time prices, live status)

### Template

```toml
[data.dataviews.{dv_name}]
datasource = "{datasource}"
query      = "SELECT ..."

# Simply omit the [cache] block — no caching is the default
```

### Constraints
- No `[cache]` block = no caching
- Every request hits the database

---

# Init & Lifecycle

---

## RECIPE:INIT-HANDLER-DDL

### Match — use when:
- [ ] Need to create tables/indexes at app startup
- [ ] SQL datasource

### Concrete Example

```json
// manifest.toml
{
  "appName": "orders-service",
  "type": "app-service",
  "appId": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "init": {
    "module": "handlers/init.ts",
    "entrypoint": "initialize"
  }
}
```

```toml
# riversd.toml
[security]
ddl_whitelist = [
    "orders@f47ac10b-58cc-4372-a567-0e02b2c3d479",
]
```

```typescript
// handlers/init.ts
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    await ctx.ddl("orders_db", `
        CREATE TABLE IF NOT EXISTS orders (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            customer_id UUID NOT NULL,
            amount DECIMAL NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TIMESTAMP DEFAULT now()
        )
    `);

    await ctx.ddl("orders_db", `
        CREATE INDEX IF NOT EXISTS idx_orders_customer ON orders(customer_id)
    `);

    Rivers.log.info("Schema initialized");
}
```

### Constraints
- Init handler uses `ctx.ddl()` — NOT `Rivers.db.query()`
- `database@appId` MUST be in `ddl_whitelist` in `riversd.toml`
- DDL is ONLY available in init context — never in view handlers
- Missing whitelist entry → `DriverError::Forbidden` → app enters FAILED state
- Empty or absent `ddl_whitelist` → no app can execute DDL (safe default)

---

## RECIPE:INIT-HANDLER-SEED

### Match — use when:
- [ ] Need to insert seed data at startup
- [ ] Dev/test environment setup

### Concrete Example

```typescript
// handlers/init.ts
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    // DDL first
    await ctx.ddl("db", "CREATE TABLE IF NOT EXISTS ...");

    // Seed data — uses ctx.query(), not ctx.ddl()
    await ctx.query("db", "INSERT INTO users (id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING", ["admin", "Admin User"]);

    Rivers.log.info("Seed data loaded");
}
```

### Constraints
- Seed data uses `ctx.query()` — normal DML, not DDL
- `ctx.query()` does NOT require whitelist entry
- Use `ON CONFLICT DO NOTHING` or `IF NOT EXISTS` for idempotency

---

## RECIPE:INIT-HANDLER-NOSQL

### Match — use when:
- [ ] MongoDB collection setup, Elasticsearch index creation, etc.

### Concrete Example

```typescript
// handlers/init.ts
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

    Rivers.log.info("MongoDB schema initialized");
}
```

### Constraints
- `ctx.admin()` for non-SQL admin operations (operation token dispatch)
- `ctx.ddl()` for SQL DDL statements
- Both require `database@appId` in `ddl_whitelist`

---

# Error Handling

---

## RECIPE:HANDLER-ERROR-RESPONSE

### Match — use when:
- [ ] Handler needs to return structured errors

### Template

```typescript
export async function handler(ctx: ViewContext): Promise<Rivers.Response> {
    const data = await Rivers.view.query("get_item", { id: ctx.request.params.id });

    if (data.rows.length === 0) {
        return { status: 404, body: { error: "item not found" } };
    }

    return { status: 200, body: data.rows[0] };
}
```

### Constraints
- Return `{ status, body }` — Rivers serializes body as JSON
- Use standard HTTP status codes
- Error body format: `{ "error": "message" }`

---

## RECIPE:DATAVIEW-NOT-FOUND

### Match — use when:
- [ ] DataView query returns empty results
- [ ] Need to distinguish "no data" from "error"

### Template

```typescript
const result = await Rivers.view.query("get_item", { id });

// Empty result is NOT an error — it's HTTP 200 with empty data
// If you need 404, you must use a handler
if (result.rows.length === 0) {
    return { status: 404, body: { error: "not found" } };
}
```

### Constraints
- DataView returning zero rows is HTTP 200 with empty data — NOT 404
- To return 404 on empty, you MUST use a CodeComponent handler
- Pure DataView-backed views return `{ "data": [], "meta": { "count": 0 } }` for no results

---

## RECIPE:DRIVER-ERROR-HANDLING

### Match — use when:
- [ ] Need to handle database errors in handler code

### Template

```typescript
try {
    const result = await Rivers.view.query("risky_query", { params });
    return { status: 200, body: result };
} catch (e) {
    // DriverError types:
    // - Connection: database unreachable
    // - Query: SQL error
    // - Transaction: begin/commit/rollback failure
    // - Unsupported: operation not supported by driver
    Rivers.log.error("query failed", { error: e.message });
    return { status: 500, body: { error: "internal error" } };
}
```

### Constraints
- Drivers never panic — all errors returned as `DriverError`
- Handler should NOT expose raw driver errors to clients
- Log the full error, return a sanitized message

---

# Schema & Validation

---

## RECIPE:REQUEST-SCHEMA

### Match — use when:
- [ ] Need to validate incoming request body

### Template

```toml
[data.dataviews.{dv_name}]
datasource  = "{datasource}"
post_query  = "INSERT INTO ..."
post_schema = "schemas/{input_schema}.schema.json"
```

### Constraints
- Schema validated BEFORE query execution
- Schema file must exist in bundle — validation check at deploy time
- Validation failure returns error before driver is called

---

## RECIPE:RESPONSE-SCHEMA

### Match — use when:
- [ ] Need to validate query results match expected shape

### Template

```toml
[data.dataviews.{dv_name}]
datasource    = "{datasource}"
query         = "SELECT ..."
return_schema = "schemas/{output_schema}.schema.json"
```

### Constraints
- `return_schema` validates AFTER driver executes, BEFORE response is returned
- Schema failure releases connection cleanly, returns `DataViewError::Schema`
- Every schema file includes a `driver` field that routes to the driver's validation engine

---

## RECIPE:PARAMETER-DEFAULTS

### Match — use when:
- [ ] Optional parameters with default values

### Template

```toml
[[data.dataviews.{dv_name}.parameters]]
name     = "status"
type     = "string"
required = false
default  = "active"
```

### Constraints
- KNOWN ISSUE: `default` declaration may not substitute — Rivers passes NULL for omitted optional params
- WORKAROUND: Use `COALESCE($param, 'default_value')` in SQL

```sql
-- Instead of relying on parameter default:
SELECT * FROM orders WHERE status = COALESCE($status, 'active')
```

---

# Bundle & Project Setup

---

## RECIPE:NEW-BUNDLE

### Match — use when:
- [ ] Starting a new Rivers application from scratch
- [ ] Need the full directory structure, manifest, resources, app config

### Template — Directory Structure

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
appId      = "{uuid}"             # unique UUID per app
entryPoint = "https://0.0.0.0:3000"

# Optional init handler
[app.init]
module     = "libraries/handlers/init.ts"
entrypoint = "initialize"

# Optional SPA config (app-main only)
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

[datasources.lockbox]
alias = "{lockbox_alias}"

# For credential-free datasources (faker, sqlite)
[[datasources]]
name       = "{datasource_name}"
driver     = "faker"
nopassword = true
required   = true

# For inter-app service references
[[services]]
name     = "{service_name}"
appId    = "{service_app_uuid}"
required = true
```

### Constraints
- `appId` MUST be a unique UUID — no two apps share an `appId`
- `entryPoint` port must not conflict with other apps
- `type = "app-service"` cannot declare `[spa_config]`
- Every datasource referenced in DataViews must exist in `resources.toml`
- Create `CHANGELOG.md` at bundle root

---

## RECIPE:DATASOURCE-SQL

### Match — use when:
- [ ] Connecting to PostgreSQL, MySQL, or SQLite

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
- PostgreSQL and MySQL: `credentials_source` required
- SQLite: `nopassword = true`, `host` is the file path
- All SQL drivers support transactions

---

## RECIPE:DATASOURCE-REDIS

### Match — use when:
- [ ] Using Redis as a key-value store or cache datasource

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
- Redis is a first-class driver — not a plugin
- Admin commands (`FLUSHDB`, `FLUSHALL`, `CONFIG SET`, `CONFIG REWRITE`) blocked in view context

---

## RECIPE:DATASOURCE-NOSQL

### Match — use when:
- [ ] Connecting to MongoDB, Elasticsearch, CouchDB, or Cassandra

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
- NoSQL drivers are plugins — must be in the `plugins` directory
- Admin operations blocked in view context — use init handler with `ctx.admin()`

---

## RECIPE:DATASOURCE-FAKER

### Match — use when:
- [ ] Synthetic test data without a real database

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

### DataView with Faker

```toml
[data.dataviews.list_contacts]
datasource    = "fake_data"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"
```

### Constraints
- `query` field is a FILE PATH to a schema, NOT SQL
- `return_schema` is the SAME file — faker generates what the schema describes
- `nopassword = true` required
- Faker attributes on non-faker driver schemas → validation error

---

## RECIPE:DATASOURCE-HTTP

### Match — use when:
- [ ] Calling an external REST API as a datasource
- [ ] Service-to-service proxy calls

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
- HTTP DataView `query` field is a URL path, NOT SQL
- Auth modes: bearer token, basic auth (`username:password`), API key header
- From the View layer, SQL and HTTP DataViews are identical

---

## RECIPE:DATASOURCE-LDAP

### Match — use when:
- [ ] Querying LDAP/Active Directory

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
- `query` is an LDAP filter string with `$param` substitution
- User input must be escaped per RFC 4515 to prevent LDAP injection
- LDAPS (port 636) preferred — cert verification ON by default

---

## RECIPE:DATASOURCE-KAFKA

### Match — use when:
- [ ] Publishing or consuming messages via Kafka

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

### Consumer (with consumer block)

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
- `[consumer]` block activates `MessageBrokerDriver` trait
- Without `[consumer]` → producer only (use via DataView `post_query`)
- Consumer wires to `MessageConsumer` view type (see `RECIPE:MESSAGE-CONSUMER`)

---

## RECIPE:LOCKBOX-CREDENTIALS

### Match — use when:
- [ ] Wiring credentials for any datasource

### Commands

```bash
rivers lockbox add postgres/orders-prod --value "postgresql://app:secret@db:5432/orders"
rivers lockbox alias postgres/orders-prod orders-db
rivers lockbox rotate postgres/orders-prod --value "postgresql://app:newsecret@db:5432/orders"
```

### Constraints
- Credential names: `[a-z][a-z0-9_/.-]*`, max 128 chars
- Names and aliases share one namespace — no collisions
- Credentials decrypted from disk per access — never cached in memory
- Rotation: no restart needed — next pool connection uses new creds
- `nopassword = true` for credential-free datasources — omit `credentials_source`

---

## RECIPE:DDL-WHITELIST

### Match — use when:
- [ ] App needs DDL/admin operations at startup

### Template — riversd.toml

```toml
[security]
ddl_whitelist = [
    "orders@f47ac10b-58cc-4372-a567-0e02b2c3d479",
]
```

### Constraints
- `{database}` = actual database name from `ConnectionParams.database`
- `{appId}` = app's UUID from `manifest.toml`
- Empty/absent `ddl_whitelist` = no DDL allowed (safe default)
- No wildcard syntax — every pair explicitly listed

---

# Filesystem & Execution

---

## RECIPE:FILESYSTEM-OPS

### Match — use when:
- [ ] Need sandboxed file operations (read, write, list, search, delete, stat)

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
default  = "."

[data.dataviews.stat_file]
datasource = "workspace"
query      = "STAT $path"

[[data.dataviews.stat_file.parameters]]
name = "path"
type = "string"
required = true
```

### Constraints
- Driver enforces chroot-like sandboxing — paths resolve relative to workspace root
- Absolute paths and `../` traversal REJECTED at driver level
- Workspace root configured as datasource — NOT hardcoded in handlers
- Search returns file references, NOT full content dumps
- Stat returns structured metadata: file size, last modified time

---

## RECIPE:EXEC-DRIVER

### Match — use when:
- [ ] Need to run whitelisted system commands

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
- Commands MUST be hash-pinned — SHA-256 match required
- Non-whitelisted command → rejected
- Output returned as STRUCTURED DATA, NOT raw stdout
- Path traversal in arguments blocked by driver

---

# Message Broker Patterns

---

## RECIPE:KAFKA-PRODUCE

### Match — use when:
- [ ] Publishing messages to Kafka from a handler

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
- Kafka datasource WITHOUT `[consumer]` block acts as producer
- Use `post_query` for publish operations

---

## RECIPE:KAFKA-CONSUME

### Match — use when:
- [ ] Processing Kafka messages asynchronously

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
- MessageConsumer has NO HTTP route
- Consumer handler receives event payload as `ctx.request.body`
- Events persisted in consumer handler, not producer

---

# Plugin Development

---

## RECIPE:PLUGIN-DRIVER

### Match — use when:
- [ ] Creating a custom driver plugin (Rust)

### Configuration — riversd.toml

```toml
[plugins]
enabled   = true
directory = "/var/rivers/plugins"
```

### Plugin Template (Rust)

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

### Constraints
- Plugin exports `_rivers_abi_version` and `_rivers_register_driver`
- ABI version must match — mismatch → plugin rejected
- `execute()` MUST call `check_admin_guard()`
- Use `DriverError::NotImplemented` for stubs, `DriverError::Unsupported` for permanent inability
- Drivers MUST NOT panic — all errors as `DriverError`

---

# Infrastructure & Server Config

---

## RECIPE:STORAGE-ENGINE

### Match — use when:
- [ ] Configuring session storage, L2 cache, polling state, CSRF tokens
- [ ] Required for any app using sessions, polling views, or L2 caching

### Template — riversd.toml

```toml
# Development
[storage_engine]
backend = "memory"

# Single-node production
[storage_engine]
backend = "sqlite"
path    = "/var/data/rivers.db"

# Multi-node production
[storage_engine]
backend      = "redis"
url          = "redis://localhost:6379"
retention_ms = 172800000    # 2 days
```

### Constraints
- `memory` — lost on restart, dev only
- `sqlite` — WAL mode, durable, single-node
- `redis` — required for multi-node (shared sessions, shared cache, shared poll state)
- Polling views REQUIRE StorageEngine to be configured — validation error otherwise

---

## RECIPE:CONNECTION-POOL

### Match — use when:
- [ ] Tuning datasource connection pool and circuit breaker

### Template — app.toml

```toml
[data.datasources.orders_db.connection_pool]
min_idle           = 2
max_size           = 20
connection_timeout = 5000      # ms
idle_timeout       = 600000    # ms
max_lifetime       = 300000    # ms
test_query         = "SELECT 1"

[data.datasources.orders_db.connection_pool.circuit_breaker]
enabled              = true
failure_threshold    = 5
window_ms            = 60000   # rolling window
open_timeout_ms      = 10000
half_open_max_trials = 2
```

### Constraints
- Circuit breaker uses rolling window model, not fixed-window
- States: Closed (normal) → Open (all fail fast) → Half-Open (test request)
- Pool changes require full restart — hot reload does NOT re-initialize pools
- `DatasourceCircuitOpened` logged at WARN, `DatasourceCircuitClosed` at INFO

---

## RECIPE:RATE-LIMITING

### Match — use when:
- [ ] Configuring request rate limits per app or per view

### App-Level Default — app.toml

```toml
[app.rate_limit]
per_minute = 120
burst_size = 60
strategy   = "ip"          # "ip" | "header" | "session"
```

### Per-View Override

```toml
[api.views.search]
rate_limit_per_minute = 60
rate_limit_burst_size = 20
```

### Constraints
- `ip` (default) — rate limit by remote IP
- `header` — rate limit by custom header value; requires `rate_limit_custom_header`
- `session` — rate limit by `session.identity.username`; falls back to `ip` on `auth = "none"` views
- Token bucket state is in-memory — resets on restart
- Per-app, not per-server — different apps can have different policies
- WebSocket rate limiting is per-connection (messages per second)

---

## RECIPE:CORS-CONFIG

### Match — use when:
- [ ] Configuring CORS for cross-origin requests
- [ ] SPA hosted on a different domain than the API

### Template — init handler

```typescript
// handlers/init.ts
export async function initialize(ctx: Rivers.InitContext): Promise<void> {
    ctx.app.cors({
        origins: ["https://app.example.com"],
        methods: ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
        headers: ["Content-Type", "Authorization", "X-Trace-Id"],
        credentials: false,
    });
}
```

### Constraints
- CORS is configured in the init handler — NOT per-view, NOT in server config
- `origins: ["*"]` is incompatible with `credentials: true` — validation rejects at startup
- Handler-set CORS headers are blocked (SEC-8) — only Rivers sets `access-control-*` headers
- CORS policy is evaluated per-request against the `Origin` header

---

## RECIPE:LOGGING-CONFIG

### Match — use when:
- [ ] Configuring log level and format

### Template — app.toml

```toml
[base.logging]
level  = "info"     # debug | info | warn | error
format = "json"     # json | text
```

### Constraints
- Two fields. Stdout only.
- Operators handle routing via journald, Docker log driver, etc.
- Handler logs via `Rivers.log.info()`, `Rivers.log.warn()`, `Rivers.log.error()` — auto-correlated with trace ID
- DO NOT log sensitive data — structured logging makes field selection explicit

---

## RECIPE:TRACING-CONFIG

### Match — use when:
- [ ] Configuring OpenTelemetry distributed tracing

### Template — riversd.toml

```toml
[performance.tracing]
enabled       = true
provider      = "otlp"                        # otlp | jaeger | datadog
endpoint      = "http://otel-collector:4317"
service_name  = "riversd"
sampling_rate = 0.1                           # 10% sampling
```

### Span Hierarchy
```
http.request (root)
  └─ view.dispatch
       ├─ dataview.execute
       │    └─ driver.execute
       └─ codecomponent.execute
```

### Constraints
- Every request gets a `trace_id` — available in handlers via `ctx.trace_id`
- Handler logs automatically include the trace ID

---

## RECIPE:ENVIRONMENT-OVERRIDES

### Match — use when:
- [ ] Different config values for prod, staging, dev

### Template — app.toml

```toml
[environment_overrides.prod.base]
host                    = "0.0.0.0"
port                    = 443
request_timeout_seconds = 60

[environment_overrides.prod.base.backpressure]
queue_depth = 1024

[environment_overrides.prod.security]
rate_limit_per_minute = 300
```

### Constraints
- Override blocks keyed by environment name
- `ctx.env` in handlers returns `"dev"` | `"staging"` | `"prod"`

---

## RECIPE:HOT-RELOAD

### Match — use when:
- [ ] Development mode — auto-reload on config changes

### Template — riversd.toml

```toml
[hot_reload]
enabled    = true
watch_path = "./app.toml"
```

### What Hot Reload Does
- Reloads View routes, DataView configs, DataView engine
- Reloads static file config, security config, GraphQL schema

### What Hot Reload Does NOT Do
- Restart HTTP server or rebind sockets
- Re-initialize connection pools
- Reload plugins
- Re-resolve LockBox credentials

### Constraints
- Dev mode ONLY — disabled in production
- Pool changes require full restart

---

# Application Patterns

---

## RECIPE:GRAPHQL-SETUP

### Match — use when:
- [ ] Enabling GraphQL endpoint alongside REST

### Template — app.toml

```toml
[graphql]
enabled        = true
path           = "/graphql"
introspection  = true
max_depth      = 10
max_complexity = 1000
```

### Constraints
- Schema auto-generated from DataViews with `return_schema` → GraphQL object types
- Mutations require CodeComponent resolvers, not DataViews
- Subscriptions route through EventBus → SSE views available as GraphQL subscriptions
- `introspection = true` for dev; consider disabling in production
- GraphQL resolvers delegate to DataViewEngine — same caching, same validation

---

## RECIPE:STREAMING-REST

### Match — use when:
- [ ] Large result sets streamed over HTTP
- [ ] Real-time data feeds over a single request

### Template

```toml
[api.views.export_data]
path             = "/api/data/export"
method           = "GET"
view_type        = "Rest"
streaming        = true
streaming_format = "ndjson"       # "ndjson" or "sse"
stream_timeout_ms = 30000

[api.views.export_data.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/export.ts"
entrypoint = "exportData"
resources  = ["db"]
```

```typescript
// handlers/export.ts — chunk protocol
export async function exportData(ctx: ViewContext): Promise<void> {
    const rows = await Rivers.view.query("get_all_records", {});

    for (const row of rows.rows) {
        yield { chunk: row };
    }

    yield { done: true };
}
```

### Constraints
- `streaming = true` enables chunked responses
- `streaming_format` required when streaming: `"ndjson"` or `"sse"`
- Handler uses `{ chunk, done }` protocol
- `{ chunk: data }` for each piece, `{ done: true }` to end
- Handler MUST be CodeComponent

---

## RECIPE:MULTI-APP-BUNDLE

### Match — use when:
- [ ] Application with frontend (app-main) + backend service (app-service)
- [ ] Microservice-style decomposition within one bundle

### Template — Directory Structure

```
my-bundle/
├── frontend/
│   ├── manifest.toml    # type = "app-main"
│   ├── resources.toml
│   ├── app.toml
│   └── dist/            # SPA build output
└── api-service/
    ├── manifest.toml    # type = "app-service"
    ├── resources.toml
    ├── app.toml
    └── schemas/
```

### frontend/manifest.toml

```toml
[app]
appName    = "frontend"
type       = "app-main"
appId      = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01"
entryPoint = "https://0.0.0.0:3000"

[app.spa_config]
root_path    = "dist/"
index_file   = "index.html"
spa_fallback = true
```

### frontend/resources.toml

```toml
[[services]]
name     = "api-service"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee02"
required = true

[[datasources]]
name    = "api"
driver  = "http"
service = "api-service"
```

### api-service/manifest.toml

```toml
[app]
appName    = "api-service"
type       = "app-service"
appId      = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee02"
entryPoint = "https://0.0.0.0:3001"
```

### Constraints
- `app-main` is the entry point — serves SPA + proxies to services
- `app-service` has no SPA config
- Each app has a unique `appId` and `entryPoint` port
- `[[services]]` in resources.toml declares service dependencies by `appId`
- HTTP proxy DataViews in app-main route to app-service endpoints
- Start services BEFORE main — main waits for service healthy

---

## RECIPE:SERVICE-TO-SERVICE

### Match — use when:
- [ ] app-main calling app-service endpoints
- [ ] One service calling another service

### Template

```toml
# In the calling app's resources.toml
[[services]]
name     = "orders-api"
appId    = "f47ac10b-58cc-4372-a567-0e02b2c3d479"
required = true

[[datasources]]
name    = "orders-proxy"
driver  = "http"
service = "orders-api"

# In the calling app's app.toml
[data.dataviews.proxy_list_orders]
datasource = "orders-proxy"
query      = "/api/orders?limit={limit}"
method     = "GET"

[[data.dataviews.proxy_list_orders.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20
```

### Constraints
- `service` field on HTTP datasource references the `[[services]]` name
- Auth scope carry-over automatically forwards `Authorization: Bearer {session_token}`
- From the View layer, proxy DataViews are identical to SQL DataViews

---

## RECIPE:OUTBOUND-HTTP

### Match — use when:
- [ ] Calling a third-party API that has no Rivers driver
- [ ] Webhook relay, external service integration
- [ ] Cannot be expressed through DataView/driver model

### Template — app.toml

```toml
[api.views.webhook_relay]
path      = "/api/webhook"
method    = "POST"
view_type = "Rest"

[api.views.webhook_relay.handler]
type                 = "codecomponent"
language             = "typescript"
module               = "handlers/webhook.ts"
entrypoint           = "relayWebhook"
resources            = []
allow_outbound_http  = true
```

```typescript
// handlers/webhook.ts
export async function relayWebhook(ctx: ViewContext): Promise<Rivers.Response> {
    const result = await Rivers.http.post(
        "https://external-service.example.com/webhook",
        ctx.request.body,
        { headers: { "Content-Type": "application/json" } }
    );

    return { status: result.status, body: await result.json() };
}
```

### Constraints
- `allow_outbound_http = true` REQUIRED — `Rivers.http` not injected without it
- Rivers logs WARNING at startup identifying views with outbound HTTP
- Each call logged at INFO with destination host and trace ID
- `Rivers.http` is an escape hatch — document why a DataView over HTTP driver doesn't suffice
- SSRF prevention via capability model — no runtime IP validation

---

## RECIPE:WASM-HANDLER

### Match — use when:
- [ ] Compute-intensive handler needs native speed
- [ ] Using Wasmtime runtime instead of V8

### Template — app.toml

```toml
[runtime.process_pools.wasm]
engine             = "wasmtime"
workers            = 4
max_memory_mb      = 128
task_timeout_ms    = 10000
epoch_interval_ms  = 10

[api.views.process_image]
path         = "/api/process"
method       = "POST"
view_type    = "Rest"
process_pool = "wasm"

[api.views.process_image.handler]
type       = "codecomponent"
language   = "wasm"
module     = "libraries/processors/image.wasm"
entrypoint = "processImage"
resources  = ["storage"]
```

### Constraints
- WASM modules cannot load secondary modules at runtime — all imports resolved at dispatch
- Same capability model as V8 — token-based datasource access
- `process_pool` on the view selects which pool executes the handler
- Omitted `process_pool` → uses `"default"` V8 pool

---

# Schema Patterns

---

## RECIPE:SCHEMA-FILE

### Match — use when:
- [ ] Creating schema files for any driver type
- [ ] Need to understand driver-specific schema attributes

### PostgreSQL Schema

```json
{
    "driver": "postgresql",
    "type": "object",
    "description": "Order record",
    "fields": [
        { "name": "id",     "type": "uuid",    "required": true },
        { "name": "amount", "type": "decimal",  "required": true, "min": 0 },
        { "name": "status", "type": "string",  "required": true, "pattern": "^(active|closed)$" }
    ]
}
```

### Redis Schema

```json
{
    "driver": "redis",
    "type": "hash",
    "description": "User session data",
    "key_pattern": "session:{session_id}",
    "fields": [
        { "name": "user_id",    "type": "string",  "required": true },
        { "name": "claims",     "type": "json",    "required": true },
        { "name": "expires_at", "type": "datetime", "required": true }
    ]
}
```

### Kafka Schema

```json
{
    "driver": "kafka",
    "type": "message",
    "topic": "orders",
    "key": { "type": "uuid" },
    "value": {
        "fields": [
            { "name": "order_id", "type": "uuid",   "required": true },
            { "name": "action",   "type": "string",  "required": true }
        ]
    }
}
```

### Faker Schema

```json
{
    "type": "object",
    "fields": [
        { "name": "id",    "type": "uuid",   "faker": "datatype.uuid",  "required": true },
        { "name": "name",  "type": "string", "faker": "name.fullName",  "required": true },
        { "name": "email", "type": "email",  "faker": "internet.email", "required": true }
    ]
}
```

### Constraints
- Every schema file includes a `driver` field — routes to driver's validation engine
- `faker` attributes on non-faker schemas → validation error
- Schema validated at both `riverpackage` build time AND `riversd` deploy time
- Schemas live in `schemas/` directory, referenced by path in DataView config
- Per-method schemas: `get_schema`, `post_schema`, `put_schema`, `delete_schema`

---

# Operations

---

## RECIPE:BUNDLE-VALIDATION

### Match — use when:
- [ ] Validating a bundle before deployment

### Command

```bash
riversctl validate <bundle_path>
riversctl validate --schema server|app|bundle    # output JSON Schema
```

### Validation Checks (9 total)
1. View types — recognized values
2. Driver names — match registered drivers
3. Datasource refs — all references resolve
4. DataView refs — all references resolve
5. Invalidates targets — targets exist
6. Duplicate names — no duplicate DataView/View/datasource names
7. Schema file existence — all referenced files exist on disk
8. Cross-app service refs — inter-app references resolve within bundle
9. TOML parse errors — line/column context

### Constraints
- Run BEFORE deployment — catch errors at build time
- Unknown driver names produce warnings, not errors
- Schema attribute violations caught at build time

---

## RECIPE:GRACEFUL-SHUTDOWN

### Match — understanding only:
- [ ] How Rivers handles SIGTERM/SIGINT

### Shutdown Sequence
1. Stop accepting new connections
2. `shutdown_guard_middleware` returns 503 for new requests
3. Wait for in-flight requests to complete (drain timeout)
4. Close datasource connections
5. Stop ProcessPool workers
6. Exit

### Constraints
- In-flight requests complete normally
- New requests receive `503 Service Unavailable`
- WebSocket connections receive close frame
- SSE connections close gracefully

---

## RECIPE:ERROR-RESPONSE-FORMAT

### Match — understanding only:
- [ ] Standard error envelope from Rivers (not from handler code)

### Format

```json
{
    "code": 500,
    "message": "human-readable error message",
    "details": "optional diagnostic info",
    "trace_id": "abc-123"
}
```

### Status Code Mapping

| Status | Condition |
|--------|-----------|
| 400 | Invalid request, parameter validation |
| 401 | Auth failed |
| 403 | RBAC denied, IP allowlist |
| 404 | View/file not found |
| 422 | Schema validation failed |
| 429 | Rate limit exceeded |
| 500 | Runtime error |
| 503 | Draining, backpressure, circuit open |

---

# Anti-Patterns

---

## ANTI:MULTI-STATEMENT-SQL

### Rule
NEVER put multiple SQL statements in a single query field.

### Wrong

```toml
post_query = """
UPDATE wip SET status='in_progress' WHERE id=$wip_id;
UPDATE projects SET active_wip_id=$wip_id WHERE id=$project_id;
"""
```

### Why
The SQLite driver (and potentially others) only executes the FIRST statement. Statements after the first `;` are silently dropped. No error, no warning — partial execution with HTTP 200.

### Right
Declare each statement as a separate DataView. Orchestrate in a handler with transactions.

```toml
[data.dataviews.update_wip_status]
datasource = "db"
query      = "UPDATE wip SET status='in_progress' WHERE id=$wip_id"

[data.dataviews.set_active_wip]
datasource = "db"
query      = "UPDATE projects SET active_wip_id=$wip_id WHERE id=$project_id"
```

---

## ANTI:RAW-SQL-IN-HANDLER

### Rule
Handlers should call DataViews by name, not write SQL.

### Wrong

```typescript
const result = await Rivers.db.query("orders_db",
    "SELECT * FROM orders WHERE id = $1", [orderId]);
```

### Right

```typescript
const result = await Rivers.view.query("get_order", { order_id: orderId });
```

### Why
`Rivers.db.query()` bypasses DataView schema validation, caching, operation inference, and the single-statement enforcement. The SQL lives in handler code instead of TOML — invisible to validation tooling.

---

## ANTI:SPLIT-TOOL-SEQUENCING

### Rule
NEVER split one logical operation into multiple MCP tools and rely on the caller to sequence them.

### Wrong

```
Tool 1: cb_archive_wip        → archives WIP items
Tool 2: cb_clear_wip           → deletes WIP items
Tool 3: cb_mark_goal_complete  → marks goal done
Tool 4: cb_clear_context       → clears project state
```
CC must call all four in order. If step 2 fails, step 1 already committed.

### Right
One MCP tool, one handler, one transaction. See `RECIPE:MCP-MULTI-STEP`.

---

## ANTI:HANDLER-FOR-SIMPLE-CRUD

### Rule
DO NOT write a handler that just passes through to a DataView.

### Wrong

```typescript
export async function getOrder(ctx: ViewContext): Promise<Rivers.Response> {
    const result = await Rivers.view.query("get_order", {
        id: ctx.request.params.id,
    });
    return { status: 200, body: result };
}
```

### Right
Bind the View directly to the DataView. No handler. See `RECIPE:SINGLE-READ` or `RECIPE:REST-CRUD`.

---

## ANTI:RETURNING-CLAUSE

### Rule
DO NOT use `RETURNING *` in write queries.

### Wrong

```toml
post_query = "INSERT INTO orders (...) VALUES (...) RETURNING *"
```

### Why
Rivers' SQLite driver dispatches writes through `execute()` which expects zero rows. `RETURNING` produces rows, causing a driver error. Other drivers may behave differently, but avoid for portability.

### Right
Use a follow-up read DataView if you need the row back. Or in a transaction:

```typescript
const tx = Rivers.db.tx.begin("db");
tx.query("insert_order", { /* params */ });
tx.query("get_order", { id: order_id });
const results = tx.commit();
const inserted = results["get_order"][0].rows[0];
```
