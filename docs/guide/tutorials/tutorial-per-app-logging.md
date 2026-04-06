# Tutorial: Per-App Logging

**Rivers v0.53.0**

## Overview

Rivers supports per-application log files that isolate each app's handler output into its own log file. When a multi-app bundle runs, each app's `Rivers.log.info/warn/error` calls write to a dedicated file under the configured `app_log_dir`, making it straightforward to debug a single app without filtering through the combined server log.

Per-app logging complements the main server log -- it does not replace it. Handler log calls write to both the central tracing subscriber (stdout + `local_file_path`) and the per-app file via `AppLogRouter`. Server-level events (startup, config, driver loading, TLS) only appear in the main log.

Use per-app logging when you run multiple apps in a single bundle and need to isolate logs per service, when you want to `tail -f` a single app's output during development, or when your logging pipeline ingests files per-service.

## Prerequisites

- A deployed Rivers instance (see the [Getting Started tutorial](tutorial-getting-started.md))
- A bundle with at least two apps (this tutorial creates one)

---

## Step 1: Enable app_log_dir in riversd.toml

Open your `riversd.toml` and ensure the `[base.logging]` section includes `app_log_dir`:

```toml
[base.logging]
level           = "info"
format          = "json"
local_file_path = "/opt/rivers/log/riversd.log"
app_log_dir     = "/opt/rivers/log/apps"
```

| Field | Description |
|-------|-------------|
| `level` | Minimum log level: `trace`, `debug`, `info`, `warn`, `error` |
| `format` | Output format: `json` (structured) or `text` (human-readable) |
| `local_file_path` | Main server log file (stdout is always active) |
| `app_log_dir` | Directory for per-app log files |

If `app_log_dir` is not set, all handler logs go to the main `local_file_path` only.

If the directory does not exist, `riversctl doctor --fix` creates it automatically:

```bash
/opt/rivers/bin/riversctl doctor --fix
```

---

## Step 2: Deploy a Multi-App Bundle

Create a bundle with two apps -- an API service and a backend worker -- to demonstrate per-app log isolation.

### Bundle structure

```
multi-app-bundle/
├── manifest.toml
├── orders-api/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── schemas/
│   │   └── order.schema.json
│   └── libraries/
│       └── handlers/
│           └── orders.js
└── inventory-api/
    ├── manifest.toml
    ├── resources.toml
    ├── app.toml
    ├── schemas/
    │   └── product.schema.json
    └── libraries/
        └── handlers/
            └── products.js
```

### Bundle manifest

```toml
# multi-app-bundle/manifest.toml

bundleName    = "multi-app"
bundleVersion = "1.0.0"
apps          = ["orders-api", "inventory-api"]
```

### Orders API (app 1)

```toml
# orders-api/manifest.toml

appName    = "orders-api"
appId      = "a1b2c3d4-0001-4000-8000-000000000001"
type       = "service"
entryPoint = "orders"
```

```toml
# orders-api/resources.toml

[[datasources]]
name       = "data"
driver     = "faker"
nopassword = true
required   = true
```

```toml
# orders-api/app.toml

[data.datasources.data]
name       = "data"
driver     = "faker"
nopassword = true

[data.datasources.data.config]
locale = "en_US"
seed   = 100

[data.dataviews.list_orders]
name          = "list_orders"
datasource    = "data"
query         = "schemas/order.schema.json"
return_schema = "schemas/order.schema.json"

[[data.dataviews.list_orders.parameters]]
name    = "limit"
type    = "integer"
default = 20

[api.views.list_orders]
path            = "orders"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_orders.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/orders.js"
entrypoint = "listOrders"
resources  = ["data"]

[runtime.process_pools.default]
engine          = "v8"
workers         = 2
task_timeout_ms = 5000
max_heap_mb     = 128
```

```javascript
// orders-api/libraries/handlers/orders.js

function listOrders(ctx) {
    Rivers.log.info("fetching orders", { limit: ctx.request.query_params.limit });

    var result = ctx.dataview("list_orders", {
        limit: parseInt(ctx.request.query_params.limit || "20")
    });

    Rivers.log.info("orders fetched", { count: result.length });
    ctx.resdata = result;
}
```

### Inventory API (app 2)

```toml
# inventory-api/manifest.toml

appName    = "inventory-api"
appId      = "a1b2c3d4-0002-4000-8000-000000000002"
type       = "service"
entryPoint = "inventory"
```

```toml
# inventory-api/resources.toml

[[datasources]]
name       = "data"
driver     = "faker"
nopassword = true
required   = true
```

```toml
# inventory-api/app.toml

[data.datasources.data]
name       = "data"
driver     = "faker"
nopassword = true

[data.datasources.data.config]
locale = "en_US"
seed   = 200

[data.dataviews.list_products]
name          = "list_products"
datasource    = "data"
query         = "schemas/product.schema.json"
return_schema = "schemas/product.schema.json"

[[data.dataviews.list_products.parameters]]
name    = "limit"
type    = "integer"
default = 20

[api.views.list_products]
path            = "products"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_products.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/products.js"
entrypoint = "listProducts"
resources  = ["data"]

[runtime.process_pools.default]
engine          = "v8"
workers         = 2
task_timeout_ms = 5000
max_heap_mb     = 128
```

```javascript
// inventory-api/libraries/handlers/products.js

function listProducts(ctx) {
    Rivers.log.info("fetching products", { limit: ctx.request.query_params.limit });

    var result = ctx.dataview("list_products", {
        limit: parseInt(ctx.request.query_params.limit || "20")
    });

    if (result.length === 0) {
        Rivers.log.warn("no products found");
    }

    Rivers.log.info("products fetched", { count: result.length });
    ctx.resdata = result;
}
```

Copy the bundle to apphome and update `riversd.toml`:

```bash
cp -r multi-app-bundle /opt/rivers/apphome/
```

```toml
# In riversd.toml, set:
bundle_path = "/opt/rivers/apphome/multi-app-bundle/"
```

Start the server:

```bash
/opt/rivers/bin/riversctl start
```

---

## Step 3: Verify Per-App Log Files

After startup, check the app log directory:

```bash
ls /opt/rivers/log/apps/
```

Expected output:

```
orders.log
inventory.log
```

Each app gets a log file named after its `entryPoint` value from `manifest.toml` -- not the app name. Since the orders app has `entryPoint = "orders"`, its log file is `orders.log`.

The main server log contains startup and infrastructure events:

```bash
cat /opt/rivers/log/riversd.log
```

```json
{"timestamp":"2026-04-03T10:00:01Z","level":"INFO","message":"bundle loaded","name":"multi-app","apps":2}
{"timestamp":"2026-04-03T10:00:01Z","level":"INFO","message":"app loaded","name":"orders-api","entry_point":"orders","datasources":1,"views":1}
{"timestamp":"2026-04-03T10:00:01Z","level":"INFO","message":"app loaded","name":"inventory-api","entry_point":"inventory","datasources":1,"views":1}
{"timestamp":"2026-04-03T10:00:01Z","level":"INFO","message":"per-app logging enabled","dir":"/opt/rivers/log/apps"}
{"timestamp":"2026-04-03T10:00:01Z","level":"INFO","message":"listening on https://0.0.0.0:8080"}
```

---

## Step 4: Using Rivers.log from JavaScript Handlers

The `Rivers.log` object provides three methods that write to both the central log and the per-app log file:

| Method | Level | When to use |
|--------|-------|-------------|
| `Rivers.log.info(message, fields)` | INFO | Normal operational events |
| `Rivers.log.warn(message, fields)` | WARN | Unexpected but non-fatal conditions |
| `Rivers.log.error(message, fields)` | ERROR | Failures requiring attention |

The first argument is a message string. The second argument is an optional object of structured fields.

```javascript
function processOrder(ctx) {
    var body = ctx.request.body;

    // Simple message
    Rivers.log.info("order received");

    // Message with structured fields
    Rivers.log.info("processing order", {
        order_id: body.id,
        customer: body.customer_id,
        item_count: body.items.length
    });

    // Warning for edge cases
    if (body.items.length > 100) {
        Rivers.log.warn("large order detected", {
            order_id: body.id,
            item_count: body.items.length
        });
    }

    // Error for failures
    try {
        var result = ctx.dataview("insert_order", body);
        ctx.resdata = result;
    } catch (e) {
        Rivers.log.error("order insert failed", {
            order_id: body.id,
            error: e.message
        });
        throw e;
    }
}
```

---

## Step 5: Structured Fields in Log Output

Make some requests to generate log entries:

```bash
curl -k https://localhost:8080/multi-app/orders/orders?limit=5
curl -k https://localhost:8080/multi-app/inventory/products?limit=3
```

Check the orders app log:

```bash
cat /opt/rivers/log/apps/orders.log
```

Output (JSON format):

```json
{"timestamp":"2026-04-03T10:05:12.345Z","level":"INFO","app":"orders-api","message":"fetching orders","limit":"5"}
{"timestamp":"2026-04-03T10:05:12.348Z","level":"INFO","app":"orders-api","message":"orders fetched","count":5}
```

Check the inventory app log:

```bash
cat /opt/rivers/log/apps/inventory.log
```

Output:

```json
{"timestamp":"2026-04-03T10:05:13.102Z","level":"INFO","app":"inventory-api","message":"fetching products","limit":"3"}
{"timestamp":"2026-04-03T10:05:13.105Z","level":"INFO","app":"inventory-api","message":"products fetched","count":3}
```

Each log entry includes:

| Field | Description |
|-------|-------------|
| `timestamp` | ISO 8601 timestamp |
| `level` | Log level (`INFO`, `WARN`, `ERROR`) |
| `app` | App name that produced the entry |
| `message` | The message string from `Rivers.log.*` |
| `...` | Any structured fields passed as the second argument |

When using `format = "text"` in `riversd.toml`, entries are human-readable instead:

```
2026-04-03T10:05:12.345Z INFO  [orders-api] fetching orders limit=5
2026-04-03T10:05:12.348Z INFO  [orders-api] orders fetched count=5
```

---

## Step 6: Log Rotation

Per-app log files rotate automatically at 10 MB. When a log file reaches 10 MB:

1. The current file is renamed to `<name>.log.1`
2. A new `<name>.log` is created
3. Only one backup is kept -- the next rotation overwrites `.log.1`

```
/opt/rivers/log/apps/
├── orders.log          <- current log (writing here)
├── orders.log.1        <- previous log (rotated at 10MB)
├── inventory.log
└── inventory.log.1
```

The main server log (`local_file_path`) does not rotate automatically -- use your system's `logrotate` or equivalent for the main log file.

---

## Step 7: Monitoring Logs in Real Time

Use `tail -f` to follow a single app's log output:

```bash
# Watch orders app only
tail -f /opt/rivers/log/apps/orders.log
```

Watch all app logs simultaneously:

```bash
tail -f /opt/rivers/log/apps/*.log
```

Watch the main server log (startup, config, driver events):

```bash
tail -f /opt/rivers/log/riversd.log
```

For JSON logs, pipe through `jq` for readable output:

```bash
tail -f /opt/rivers/log/apps/orders.log | jq .
```

Filter for errors only:

```bash
tail -f /opt/rivers/log/apps/orders.log | jq 'select(.level == "ERROR")'
```

---

## Summary

This tutorial covered:

1. Enabling `app_log_dir` in `riversd.toml` to activate per-app log files
2. Deploying a multi-app bundle where each app gets its own log file
3. Log files are named after each app's `entryPoint` value
4. Using `Rivers.log.info`, `Rivers.log.warn`, and `Rivers.log.error` from JavaScript handlers
5. Structured fields in JSON log output with `app`, `timestamp`, `level`, and custom fields
6. Automatic log rotation at 10 MB with one backup
7. Monitoring logs in real time with `tail -f` and `jq`
