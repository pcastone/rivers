# Tutorial: PostgreSQL Datasource

**Rivers v0.50.1**

## Overview

The PostgreSQL driver connects Rivers to a PostgreSQL database using `tokio-postgres`. It supports parameterized queries with positional `$1`, `$2` binding, transactions, prepared statements, and connection pooling with circuit breaker protection.

Use the PostgreSQL driver when:
- Your application needs a production relational database
- You need transaction support and prepared statements
- You need `RETURNING` clause support for write operations

The PostgreSQL driver is a built-in driver registered directly in `DriverFactory` at startup. No plugin loading is required.

## Prerequisites

- A running PostgreSQL instance (9.6+)
- A LockBox keystore with database credentials stored
- The database and tables must already exist -- Rivers does not run migrations

### Store Credentials in LockBox

```bash
# Add PostgreSQL credentials to the keystore
rivers lockbox add \
    --name postgres/myapp \
    --type string \
    --alias db/myapp
# Value: **** (enter the password at the hidden prompt)
```

For credential records with full connection metadata:

```bash
rivers lockbox add \
    --name postgres/myapp \
    --type string \
    --driver postgres \
    --username myapp_user \
    --hosts "db.internal:5432" \
    --database myapp \
    --alias db/myapp
# Value: **** (enter the password at the hidden prompt)
```

## Step 1: Declare the Datasource

In your app's `resources.toml`, declare a PostgreSQL datasource.

```toml
# resources.toml

[[datasources]]
name     = "primary_db"
driver   = "postgres"
x-type   = "postgres"
required = true
```

No `nopassword` field -- PostgreSQL requires credentials via LockBox.

## Step 2: Configure the Datasource

In your app's `app.toml`, configure the connection, pool, and circuit breaker.

```toml
# app.toml

[data.datasources.primary_db]
name               = "primary_db"
driver             = "postgres"
host               = "db.internal"
port               = 5432
database           = "myapp"
credentials_source = "lockbox://postgres/myapp"
ssl_mode           = "prefer"

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

- `credentials_source` -- LockBox URI resolving to the database password. The secret is read from disk, decrypted, used to establish the connection, and immediately zeroized. The raw credential never enters the ProcessPool.
- `ssl_mode` -- PostgreSQL SSL mode: `"disable"`, `"prefer"`, `"require"`, `"verify-ca"`, `"verify-full"`
- `min_idle` -- minimum idle connections kept warm in the pool
- `max_lifetime_ms` -- connections are recycled after this duration, ensuring credential rotation takes effect naturally

### Circuit Breaker Behavior

```
CLOSED  --(failure_threshold failures within window_ms)-->  OPEN
OPEN    --(open_timeout_ms elapsed)-->                      HALF_OPEN
HALF_OPEN --(trial succeeds)-->                             CLOSED
HALF_OPEN --(trial fails)-->                                OPEN
```

When the circuit is OPEN, `acquire()` returns `PoolError::CircuitOpen` immediately -- no connection attempt is made.

## Step 3: Define a Schema

Create a schema file at `schemas/user.schema.json`. For PostgreSQL datasources, do not use the `faker` attribute -- it is only valid for the faker driver.

```json
{
  "type": "object",
  "description": "Application user record",
  "fields": [
    { "name": "id",         "type": "uuid",     "required": true  },
    { "name": "email",      "type": "email",    "required": true, "format": "email" },
    { "name": "username",   "type": "string",   "required": true, "pattern": "^[a-z0-9_]{3,30}$" },
    { "name": "full_name",  "type": "string",   "required": true  },
    { "name": "is_active",  "type": "boolean",  "required": true  },
    { "name": "login_count","type": "integer",  "required": false, "min": 0 },
    { "name": "created_at", "type": "datetime", "required": true  },
    { "name": "updated_at", "type": "datetime", "required": false }
  ]
}
```

PostgreSQL-supported schema attributes: `format`, `pattern`, `min`, `max`.

## Step 4: Create a DataView

For PostgreSQL DataViews, the `query` field contains a SQL statement. Parameters use positional `$1`, `$2` binding.

```toml
# app.toml (continued)

[data.dataviews.list_users]
name          = "list_users"
datasource    = "primary_db"
query         = "SELECT id, email, username, full_name, is_active, login_count, created_at, updated_at FROM users WHERE is_active = true ORDER BY created_at DESC LIMIT $1 OFFSET $2"
return_schema = "schemas/user.schema.json"

[data.dataviews.list_users.caching]
ttl_seconds = 30

[[data.dataviews.list_users.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[[data.dataviews.list_users.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0

# ─────────────────────────

[data.dataviews.get_user]
name          = "get_user"
datasource    = "primary_db"
query         = "SELECT id, email, username, full_name, is_active, login_count, created_at, updated_at FROM users WHERE id = $1"
return_schema = "schemas/user.schema.json"

[data.dataviews.get_user.caching]
ttl_seconds = 120

[[data.dataviews.get_user.parameters]]
name     = "id"
type     = "uuid"
required = true

# ─────────────────────────

[data.dataviews.create_user]
name       = "create_user"
datasource = "primary_db"
query      = "INSERT INTO users (id, email, username, full_name, is_active, created_at) VALUES (gen_random_uuid(), $1, $2, $3, true, NOW()) RETURNING id, email, username, full_name, is_active, created_at"

[[data.dataviews.create_user.parameters]]
name     = "email"
type     = "string"
required = true

[[data.dataviews.create_user.parameters]]
name     = "username"
type     = "string"
required = true

[[data.dataviews.create_user.parameters]]
name     = "full_name"
type     = "string"
required = true

# ─────────────────────────

[data.dataviews.delete_user]
name       = "delete_user"
datasource = "primary_db"
query      = "DELETE FROM users WHERE id = $1"

[[data.dataviews.delete_user.parameters]]
name     = "id"
type     = "uuid"
required = true
```

Key points:
- `RETURNING id` on INSERT allows `last_insert_id` to be extracted from the result
- Parameters are passed as `HashMap<String, QueryValue>` -- the positional binding (`$1`, `$2`) is handled by the driver based on parameter declaration order
- Operation inference: `SELECT` maps to Read, `INSERT`/`UPDATE`/`DELETE` map to Write/Delete automatically from the first token of the statement

## Step 5: Create a View

```toml
# app.toml (continued)

[api.views.list_users]
path            = "users"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_users.handler]
type     = "dataview"
dataview = "list_users"

[api.views.list_users.parameter_mapping.query]
limit  = "limit"
offset = "offset"

# ─────────────────────────

[api.views.get_user]
path            = "users/{id}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_user.handler]
type     = "dataview"
dataview = "get_user"

[api.views.get_user.parameter_mapping.path]
id = "id"

# ─────────────────────────

[api.views.create_user]
path            = "users"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.create_user.handler]
type     = "dataview"
dataview = "create_user"

[api.views.create_user.parameter_mapping.body]
email     = "email"
username  = "username"
full_name = "full_name"

# ─────────────────────────

[api.views.delete_user]
path            = "users/{id}"
method          = "DELETE"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.delete_user.handler]
type     = "dataview"
dataview = "delete_user"

[api.views.delete_user.parameter_mapping.path]
id = "id"
```

Parameter mapping subtables:
- `parameter_mapping.query` -- query string parameters
- `parameter_mapping.path` -- path template `{params}`
- `parameter_mapping.body` -- JSON body fields

## Testing

```bash
# List users
curl http://localhost:8080/users

# List users with pagination
curl "http://localhost:8080/users?limit=10&offset=0"

# Get a single user
curl http://localhost:8080/users/550e8400-e29b-41d4-a716-446655440000

# Create a user
curl -X POST http://localhost:8080/users \
  -H "Content-Type: application/json" \
  -d '{"email":"alice@example.com","username":"alice","full_name":"Alice Smith"}'

# Delete a user
curl -X DELETE http://localhost:8080/users/550e8400-e29b-41d4-a716-446655440000
```

## Configuration Reference

### resources.toml Fields

| Field      | Type    | Required | Description                                        |
|------------|---------|----------|----------------------------------------------------|
| `name`     | string  | yes      | Datasource name, referenced in app.toml            |
| `driver`   | string  | yes      | Must be `"postgres"`                               |
| `x-type`   | string  | yes      | Must be `"postgres"` for build-time validation     |
| `required` | boolean | no       | Whether the app fails to start without this source |

### app.toml Datasource Config

| Field                                       | Type    | Required | Default   | Description                                   |
|---------------------------------------------|---------|----------|-----------|-----------------------------------------------|
| `driver`                                    | string  | yes      | --        | Must be `"postgres"`                          |
| `host`                                      | string  | yes      | --        | PostgreSQL host                               |
| `port`                                      | integer | no       | 5432      | PostgreSQL port                               |
| `database`                                  | string  | yes      | --        | Database name                                 |
| `credentials_source`                        | string  | yes      | --        | LockBox URI, e.g. `"lockbox://postgres/myapp"`|
| `ssl_mode`                                  | string  | no       | `prefer`  | `disable`, `prefer`, `require`, `verify-ca`, `verify-full` |

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
| Prepared statements     | Yes       |
| `last_insert_id`        | Yes (via `RETURNING id`) |
| Positional parameters   | `$1`, `$2`, ...  |
