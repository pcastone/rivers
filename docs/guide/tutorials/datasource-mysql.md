# Tutorial: MySQL Datasource

**Rivers v0.50.1**

## Overview

The MySQL driver connects Rivers to a MySQL database using `mysql_async`. It supports parameterized queries with positional `?` binding, transactions, and connection pooling with circuit breaker protection.

Use the MySQL driver when:
- Your application runs on MySQL or MariaDB
- You need transaction support
- You need `last_insert_id()` from write operations

The MySQL driver is a built-in driver registered directly in `DriverFactory` at startup. No plugin loading is required.

## Prerequisites

- A running MySQL instance (5.7+ or MariaDB 10.3+)
- A LockBox keystore with database credentials stored
- The database and tables must already exist -- Rivers does not run migrations

### Store Credentials in LockBox

```bash
# Add MySQL credentials to the keystore
rivers lockbox add \
    --name mysql/myapp \
    --type string \
    --alias db/myapp
# Value: **** (enter the password at the hidden prompt)
```

For credential records with full connection metadata:

```bash
rivers lockbox add \
    --name mysql/myapp \
    --type string \
    --driver mysql \
    --username myapp_user \
    --hosts "mysql.internal:3306" \
    --database myapp \
    --alias db/myapp
# Value: **** (enter the password at the hidden prompt)
```

## Step 1: Declare the Datasource

In your app's `resources.toml`, declare a MySQL datasource.

```toml
# resources.toml

[[datasources]]
name     = "primary_db"
driver   = "mysql"
x-type   = "mysql"
required = true
```

No `nopassword` field -- MySQL requires credentials via LockBox.

## Step 2: Configure the Datasource

In your app's `app.toml`, configure the connection, pool, and circuit breaker.

```toml
# app.toml

[data.datasources.primary_db]
name               = "primary_db"
driver             = "mysql"
host               = "mysql.internal"
port               = 3306
database           = "myapp"
credentials_source = "lockbox://mysql/myapp"

[data.datasources.primary_db.connection_pool]
max_size              = 10
min_idle              = 2
connection_timeout_ms = 5000
idle_timeout_ms       = 30000
max_lifetime_ms       = 300000
health_check_interval_ms = 5000

[data.datasources.primary_db.circuit_breaker]
enabled            = true
failure_threshold  = 5
window_ms          = 60000
open_timeout_ms    = 30000
half_open_max_trials = 1
```

- `credentials_source` -- LockBox URI resolving to the database password. The secret is read from disk, decrypted, used to establish the connection, and immediately zeroized.
- `min_idle` -- minimum idle connections kept warm in the pool
- `max_lifetime_ms` -- connections are recycled after this duration, ensuring credential rotation takes effect naturally without a pool drain

## Step 3: Define a Schema

Create a schema file at `schemas/product.schema.json`. For MySQL datasources, do not use the `faker` attribute -- it is only valid for the faker driver.

```json
{
  "type": "object",
  "description": "Product catalog entry",
  "fields": [
    { "name": "id",          "type": "integer",  "required": true  },
    { "name": "sku",         "type": "string",   "required": true, "pattern": "^[A-Z]{3}-[0-9]{6}$" },
    { "name": "name",        "type": "string",   "required": true  },
    { "name": "description", "type": "string",   "required": false },
    { "name": "price",       "type": "float",    "required": true, "min": 0 },
    { "name": "quantity",    "type": "integer",  "required": true, "min": 0 },
    { "name": "is_active",   "type": "boolean",  "required": true  },
    { "name": "created_at",  "type": "datetime", "required": true  },
    { "name": "updated_at",  "type": "datetime", "required": false }
  ]
}
```

MySQL-supported schema attributes: `format`, `pattern`, `min`, `max`.

## Step 4: Create a DataView

For MySQL DataViews, the `query` field contains a SQL statement. Parameters use positional `?` binding.

```toml
# app.toml (continued)

[data.dataviews.list_products]
name          = "list_products"
datasource    = "primary_db"
query         = "SELECT id, sku, name, description, price, quantity, is_active, created_at, updated_at FROM products WHERE is_active = true ORDER BY created_at DESC LIMIT ? OFFSET ?"
return_schema = "schemas/product.schema.json"

[data.dataviews.list_products.caching]
ttl_seconds = 30

[[data.dataviews.list_products.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[[data.dataviews.list_products.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0

# ─────────────────────────

[data.dataviews.get_product]
name          = "get_product"
datasource    = "primary_db"
query         = "SELECT id, sku, name, description, price, quantity, is_active, created_at, updated_at FROM products WHERE id = ?"
return_schema = "schemas/product.schema.json"

[data.dataviews.get_product.caching]
ttl_seconds = 120

[[data.dataviews.get_product.parameters]]
name     = "id"
type     = "integer"
required = true

# ─────────────────────────

[data.dataviews.create_product]
name       = "create_product"
datasource = "primary_db"
query      = "INSERT INTO products (sku, name, description, price, quantity, is_active, created_at) VALUES (?, ?, ?, ?, ?, true, NOW())"

[[data.dataviews.create_product.parameters]]
name     = "sku"
type     = "string"
required = true

[[data.dataviews.create_product.parameters]]
name     = "name"
type     = "string"
required = true

[[data.dataviews.create_product.parameters]]
name     = "description"
type     = "string"
required = false

[[data.dataviews.create_product.parameters]]
name     = "price"
type     = "float"
required = true

[[data.dataviews.create_product.parameters]]
name     = "quantity"
type     = "integer"
required = true

# ─────────────────────────

[data.dataviews.update_product]
name       = "update_product"
datasource = "primary_db"
query      = "UPDATE products SET name = ?, price = ?, quantity = ?, updated_at = NOW() WHERE id = ?"

[[data.dataviews.update_product.parameters]]
name     = "name"
type     = "string"
required = true

[[data.dataviews.update_product.parameters]]
name     = "price"
type     = "float"
required = true

[[data.dataviews.update_product.parameters]]
name     = "quantity"
type     = "integer"
required = true

[[data.dataviews.update_product.parameters]]
name     = "id"
type     = "integer"
required = true
```

Key points:
- MySQL uses `?` for positional parameter binding (not `$1`, `$2` like PostgreSQL)
- `last_insert_id` is automatically returned from `last_insert_id()` on the result for INSERT operations
- Operation inference: `SELECT` maps to Read, `INSERT`/`UPDATE`/`DELETE` map to Write/Delete from the first token of the statement

## Step 5: Create a View

```toml
# app.toml (continued)

[api.views.list_products]
path            = "products"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_products.handler]
type     = "dataview"
dataview = "list_products"

[api.views.list_products.parameter_mapping.query]
limit  = "limit"
offset = "offset"

# ─────────────────────────

[api.views.get_product]
path            = "products/{id}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_product.handler]
type     = "dataview"
dataview = "get_product"

[api.views.get_product.parameter_mapping.path]
id = "id"

# ─────────────────────────

[api.views.create_product]
path            = "products"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.create_product.handler]
type     = "dataview"
dataview = "create_product"

[api.views.create_product.parameter_mapping.body]
sku         = "sku"
name        = "name"
description = "description"
price       = "price"
quantity    = "quantity"

# ─────────────────────────

[api.views.update_product]
path            = "products/{id}"
method          = "PUT"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.update_product.handler]
type     = "dataview"
dataview = "update_product"

[api.views.update_product.parameter_mapping.path]
id = "id"

[api.views.update_product.parameter_mapping.body]
name     = "name"
price    = "price"
quantity = "quantity"
```

## Testing

```bash
# List products
curl http://localhost:8080/products

# List products with pagination
curl "http://localhost:8080/products?limit=10&offset=0"

# Get a single product
curl http://localhost:8080/products/42

# Create a product
curl -X POST http://localhost:8080/products \
  -H "Content-Type: application/json" \
  -d '{"sku":"WDG-000001","name":"Widget","description":"A fine widget","price":9.99,"quantity":100}'

# Update a product
curl -X PUT http://localhost:8080/products/42 \
  -H "Content-Type: application/json" \
  -d '{"name":"Widget Pro","price":14.99,"quantity":200}'
```

## Configuration Reference

### resources.toml Fields

| Field      | Type    | Required | Description                                        |
|------------|---------|----------|----------------------------------------------------|
| `name`     | string  | yes      | Datasource name, referenced in app.toml            |
| `driver`   | string  | yes      | Must be `"mysql"`                                  |
| `x-type`   | string  | yes      | Must be `"mysql"` for build-time validation        |
| `required` | boolean | no       | Whether the app fails to start without this source |

### app.toml Datasource Config

| Field                                       | Type    | Required | Default | Description                                     |
|---------------------------------------------|---------|----------|---------|-------------------------------------------------|
| `driver`                                    | string  | yes      | --      | Must be `"mysql"`                               |
| `host`                                      | string  | yes      | --      | MySQL host                                      |
| `port`                                      | integer | no       | 3306    | MySQL port                                      |
| `database`                                  | string  | yes      | --      | Database name                                   |
| `credentials_source`                        | string  | yes      | --      | LockBox URI, e.g. `"lockbox://mysql/myapp"`     |

### Connection Pool Config

| Field                      | Type    | Default  | Description                                        |
|----------------------------|---------|----------|----------------------------------------------------|
| `max_size`                 | integer | 10       | Maximum connections in pool                        |
| `min_idle`                 | integer | 0        | Minimum idle connections kept warm                  |
| `connection_timeout_ms`    | integer | 500      | Timeout for acquiring a connection                  |
| `idle_timeout_ms`          | integer | 30000    | Idle connections closed after this duration          |
| `max_lifetime_ms`          | integer | 300000   | Connections recycled after this duration             |
| `health_check_interval_ms` | integer | 5000    | Interval between health check pings                 |

### Circuit Breaker Config

| Field                  | Type    | Default | Description                                         |
|------------------------|---------|---------|-----------------------------------------------------|
| `enabled`              | boolean | false   | Enable circuit breaker                              |
| `failure_threshold`    | integer | 5       | Failures within window before opening               |
| `window_ms`            | integer | 60000   | Rolling failure window in milliseconds              |
| `open_timeout_ms`      | integer | 30000   | Time circuit stays open before half-open probe      |
| `half_open_max_trials`  | integer | 1      | Probe attempts before closing                       |

### Driver Capabilities

| Capability              | Supported |
|-------------------------|-----------|
| Transactions            | Yes       |
| Prepared statements     | No        |
| `last_insert_id`        | Yes (via `last_insert_id()`) |
| Positional parameters   | `?`       |
