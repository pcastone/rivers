# Tutorial: Redis Datasource

**Rivers v0.50.1**

## Overview

The Redis driver is a first-class built-in driver that exposes a broad set of Redis operations through the standard `DatabaseDriver` contract. It maps Redis commands onto the Rivers five-op model (query, execute, ping, begin, stream) across multiple data structures -- strings, hashes, lists, and sets.

Use the Redis driver when:
- You need a fast key-value cache layer
- You need session storage or ephemeral state
- You need counters, queues, or lightweight data structures
- You want to use Redis as an application-level datasource (not just internal infrastructure)

The Redis driver is a built-in driver registered directly in `DriverFactory` at startup. No plugin loading is required. Redis does not support transactions in Rivers.

## Prerequisites

- A running Redis instance (6.0+)
- A LockBox keystore with Redis credentials stored (or `nopassword = true` for development instances without auth)

### Store Credentials in LockBox

```bash
# Add Redis credentials to the keystore
rivers lockbox add \
    --name redis/cache \
    --type string \
    --alias cache/prod
# Value: **** (enter the Redis AUTH password at the hidden prompt)
```

## Step 1: Declare the Datasource

In your app's `resources.toml`, declare a Redis datasource.

```toml
# resources.toml

[[datasources]]
name     = "cache"
driver   = "redis"
x-type   = "redis"
required = true
```

For development instances without authentication:

```toml
[[datasources]]
name       = "cache"
driver     = "redis"
x-type     = "redis"
nopassword = true
required   = true
```

## Step 2: Configure the Datasource

In your app's `app.toml`, configure the connection and pool.

```toml
# app.toml

[data.datasources.cache]
name               = "cache"
driver             = "redis"
host               = "cache.internal"
port               = 6379
credentials_source = "lockbox://redis/cache"

[data.datasources.cache.connection_pool]
max_size              = 10
min_idle              = 2
connection_timeout_ms = 2000
idle_timeout_ms       = 30000
max_lifetime_ms       = 300000
health_check_interval_ms = 5000

[data.datasources.cache.circuit_breaker]
enabled            = true
failure_threshold  = 5
window_ms          = 60000
open_timeout_ms    = 15000
half_open_max_trials = 1
```

- `credentials_source` -- LockBox URI resolving to the Redis AUTH password
- Redis connections use `redis::aio::Connection` with a single multiplexed connection per pool slot
- Reconnection is handled by the pool circuit breaker, not the driver

For development instances without auth, replace `credentials_source` with `nopassword = true`:

```toml
[data.datasources.cache]
name       = "cache"
driver     = "redis"
host       = "localhost"
port       = 6379
nopassword = true
```

## Step 3: Define a Schema

Create a schema file at `schemas/session.schema.json`. Redis schemas define the shape of values you store and retrieve.

```json
{
  "type": "object",
  "description": "User session data",
  "fields": [
    { "name": "user_id",    "type": "uuid",     "required": true  },
    { "name": "email",      "type": "email",    "required": true  },
    { "name": "role",       "type": "string",   "required": true  },
    { "name": "expires_at", "type": "datetime", "required": true  }
  ]
}
```

Redis schemas are used for `return_schema` validation on DataViews that return structured data (e.g., JSON values stored via `set`). For simple string get/set operations, schemas are optional.

## Step 4: Create a DataView

Redis DataViews use the `query` field to specify the Redis command. Parameters are extracted from `query.parameters` by name: `key`, `field`, `value`, `start`, `stop`, `seconds`, `increment`.

```toml
# app.toml (continued)

# ─────────────────────────
# String operations: GET / SET / DEL
# ─────────────────────────

[data.dataviews.cache_get]
name       = "cache_get"
datasource = "cache"
query      = "get"

[[data.dataviews.cache_get.parameters]]
name     = "key"
type     = "string"
required = true

# ─────────────────────────

[data.dataviews.cache_set]
name       = "cache_set"
datasource = "cache"
query      = "set"

[[data.dataviews.cache_set.parameters]]
name     = "key"
type     = "string"
required = true

[[data.dataviews.cache_set.parameters]]
name     = "value"
type     = "string"
required = true

[[data.dataviews.cache_set.parameters]]
name     = "seconds"
type     = "integer"
required = false
default  = 3600

# ─────────────────────────

[data.dataviews.cache_delete]
name       = "cache_delete"
datasource = "cache"
query      = "del"

[[data.dataviews.cache_delete.parameters]]
name     = "key"
type     = "string"
required = true

# ─────────────────────────
# Hash operations: HGET / HSET / HGETALL
# ─────────────────────────

[data.dataviews.session_get_field]
name       = "session_get_field"
datasource = "cache"
query      = "hget"

[[data.dataviews.session_get_field.parameters]]
name     = "key"
type     = "string"
required = true

[[data.dataviews.session_get_field.parameters]]
name     = "field"
type     = "string"
required = true

# ─────────────────────────

[data.dataviews.session_get_all]
name       = "session_get_all"
datasource = "cache"
query      = "hgetall"

[[data.dataviews.session_get_all.parameters]]
name     = "key"
type     = "string"
required = true

# ─────────────────────────

[data.dataviews.session_set_field]
name       = "session_set_field"
datasource = "cache"
query      = "hset"

[[data.dataviews.session_set_field.parameters]]
name     = "key"
type     = "string"
required = true

[[data.dataviews.session_set_field.parameters]]
name     = "field"
type     = "string"
required = true

[[data.dataviews.session_set_field.parameters]]
name     = "value"
type     = "string"
required = true

# ─────────────────────────
# Counter: INCR / INCRBY
# ─────────────────────────

[data.dataviews.counter_increment]
name       = "counter_increment"
datasource = "cache"
query      = "incrby"

[[data.dataviews.counter_increment.parameters]]
name     = "key"
type     = "string"
required = true

[[data.dataviews.counter_increment.parameters]]
name     = "increment"
type     = "integer"
required = false
default  = 1

# ─────────────────────────
# List operations: LPUSH / RPOP
# ─────────────────────────

[data.dataviews.queue_push]
name       = "queue_push"
datasource = "cache"
query      = "lpush"

[[data.dataviews.queue_push.parameters]]
name     = "key"
type     = "string"
required = true

[[data.dataviews.queue_push.parameters]]
name     = "value"
type     = "string"
required = true

# ─────────────────────────

[data.dataviews.queue_pop]
name       = "queue_pop"
datasource = "cache"
query      = "rpop"

[[data.dataviews.queue_pop.parameters]]
name     = "key"
type     = "string"
required = true
```

### Redis Operation Reference

| `query` value | Redis Command           | Returns                                  |
|---------------|-------------------------|------------------------------------------|
| `get`         | `GET key`               | Single row: `{value}`                    |
| `mget`        | `MGET key [key...]`     | One row per key: `{key, value}`          |
| `set`         | `SET key value [EX s]`  | `affected_rows = 1`                      |
| `setex`       | `SET key value EX s`    | `affected_rows = 1`                      |
| `del`         | `DEL key [key...]`      | `affected_rows = count deleted`          |
| `expire`      | `EXPIRE key seconds`    | `affected_rows = 1 or 0`                |
| `hget`        | `HGET key field`        | Single row: `{field, value}`             |
| `hgetall`     | `HGETALL key`           | One row per field: `{field, value}`      |
| `hset`        | `HSET key field value`  | `affected_rows = 1`                      |
| `hdel`        | `HDEL key field`        | `affected_rows = count deleted`          |
| `lpush`       | `LPUSH key value`       | `affected_rows = new length`             |
| `rpush`       | `RPUSH key value`       | `affected_rows = new length`             |
| `lpop`        | `LPOP key`              | Single row: `{value}`                    |
| `rpop`        | `RPOP key`              | Single row: `{value}`                    |
| `lrange`      | `LRANGE key start stop` | One row per element: `{index, value}`    |
| `smembers`    | `SMEMBERS key`          | One row per member: `{member}`           |
| `incr`        | `INCR key`              | Single row: `{value}`                    |
| `incrby`      | `INCRBY key increment`  | Single row: `{value}`                    |
| `ping`        | `PING`                  | Empty result                             |

## Step 5: Create a View

```toml
# app.toml (continued)

[api.views.cache_get]
path            = "cache/{key}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.cache_get.handler]
type     = "dataview"
dataview = "cache_get"

[api.views.cache_get.parameter_mapping.path]
key = "key"

# ─────────────────────────

[api.views.cache_set]
path            = "cache/{key}"
method          = "PUT"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.cache_set.handler]
type     = "dataview"
dataview = "cache_set"

[api.views.cache_set.parameter_mapping.path]
key = "key"

[api.views.cache_set.parameter_mapping.body]
value   = "value"
seconds = "seconds"

# ─────────────────────────

[api.views.cache_delete]
path            = "cache/{key}"
method          = "DELETE"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.cache_delete.handler]
type     = "dataview"
dataview = "cache_delete"

[api.views.cache_delete.parameter_mapping.path]
key = "key"

# ─────────────────────────

[api.views.counter_increment]
path            = "counters/{key}"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.counter_increment.handler]
type     = "dataview"
dataview = "counter_increment"

[api.views.counter_increment.parameter_mapping.path]
key = "key"

[api.views.counter_increment.parameter_mapping.body]
increment = "increment"
```

## Testing

```bash
# Set a cache value with 1-hour TTL
curl -X PUT http://localhost:8080/cache/greeting \
  -H "Content-Type: application/json" \
  -d '{"value":"hello world","seconds":3600}'

# Get a cache value
curl http://localhost:8080/cache/greeting

# Delete a cache value
curl -X DELETE http://localhost:8080/cache/greeting

# Increment a counter
curl -X POST http://localhost:8080/counters/page_views \
  -H "Content-Type: application/json" \
  -d '{"increment":1}'

# Increment by 5
curl -X POST http://localhost:8080/counters/page_views \
  -H "Content-Type: application/json" \
  -d '{"increment":5}'
```

## Configuration Reference

### resources.toml Fields

| Field        | Type    | Required | Description                                        |
|--------------|---------|----------|----------------------------------------------------|
| `name`       | string  | yes      | Datasource name, referenced in app.toml            |
| `driver`     | string  | yes      | Must be `"redis"`                                  |
| `x-type`     | string  | yes      | Must be `"redis"` for build-time validation        |
| `nopassword` | boolean | no       | Set `true` for development instances without auth  |
| `required`   | boolean | no       | Whether the app fails to start without this source |

### app.toml Datasource Config

| Field                                       | Type    | Required | Default | Description                                     |
|---------------------------------------------|---------|----------|---------|-------------------------------------------------|
| `driver`                                    | string  | yes      | --      | Must be `"redis"`                               |
| `host`                                      | string  | yes      | --      | Redis host                                      |
| `port`                                      | integer | no       | 6379    | Redis port                                      |
| `credentials_source`                        | string  | cond.    | --      | LockBox URI; required unless `nopassword = true`|
| `nopassword`                                | boolean | cond.    | --      | Set `true` for no-auth instances                |

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
| Transactions            | No        |
| Prepared statements     | No        |
| Key namespacing         | Manual (use explicit prefixes in keys) |
| Multiplexed connections | Yes (single multiplexed connection per pool slot) |
