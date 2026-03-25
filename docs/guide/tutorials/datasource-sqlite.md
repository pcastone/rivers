# Tutorial: SQLite Datasource

**Rivers v0.50.1**

## Overview

The SQLite driver provides an embedded relational database using `rusqlite` with bundled SQLite. It runs in WAL mode with a 5-second busy timeout. No external database server is needed -- the database is a single file on disk (or `:memory:` for ephemeral use).

Use the SQLite driver when:
- Prototyping an API with real SQL before deploying to PostgreSQL or MySQL
- Building small single-node apps that do not need a separate database server
- Running integration tests with a real SQL engine
- Storing local application state that does not need to scale across nodes

The SQLite driver is a built-in driver registered directly in `DriverFactory` at startup. No plugin loading is required.

## Prerequisites

- No external services needed -- SQLite is bundled
- No credentials needed (`nopassword = true`)
- For file-based databases, the directory must exist and be writable by the `riversd` process

## Step 1: Declare the Datasource

In your app's `resources.toml`, declare a SQLite datasource.

```toml
# resources.toml

[[datasources]]
name       = "local_db"
driver     = "sqlite"
x-type     = "sqlite"
nopassword = true
required   = true
```

- `nopassword = true` -- SQLite has no authentication; omit `lockbox` entirely

## Step 2: Configure the Datasource

In your app's `app.toml`, configure the database file path.

```toml
# app.toml

[data.datasources.local_db]
name       = "local_db"
driver     = "sqlite"
nopassword = true
database   = "data/app.db"
```

- `database` -- path to the SQLite file, relative to the app directory. Use `":memory:"` for an in-memory database that is created fresh on each startup.

### In-Memory Mode

For ephemeral databases (tests, prototypes):

```toml
[data.datasources.local_db]
name       = "local_db"
driver     = "sqlite"
nopassword = true
database   = ":memory:"
```

In-memory databases are empty on every startup. Combine with a startup migration or seed script if you need initial tables.

## Step 3: Define a Schema

Create a schema file at `schemas/task.schema.json`. For SQLite datasources, do not use the `faker` attribute.

```json
{
  "type": "object",
  "description": "Task list item",
  "fields": [
    { "name": "id",          "type": "integer",  "required": true  },
    { "name": "title",       "type": "string",   "required": true  },
    { "name": "description", "type": "string",   "required": false },
    { "name": "is_complete", "type": "boolean",  "required": true  },
    { "name": "priority",    "type": "integer",  "required": false, "min": 1, "max": 5 },
    { "name": "created_at",  "type": "datetime", "required": true  },
    { "name": "completed_at","type": "datetime", "required": false }
  ]
}
```

### SQLite Type Mapping

SQLite uses type affinity. The driver maps affinities to `QueryValue`:

| SQLite Affinity   | QueryValue          |
|-------------------|---------------------|
| INTEGER           | `Integer(i64)`      |
| REAL              | `Float(f64)`        |
| TEXT (valid JSON)  | `Json(Value)`      |
| TEXT              | `String(String)`    |
| BLOB              | `String` (UTF-8 attempt, else hex) |
| NULL              | `Null`              |

## Step 4: Create a DataView

For SQLite DataViews, the `query` field contains a SQL statement. SQLite supports named parameters: `:name`, `@name`, `$name`. Parameters without a prefix are auto-prefixed with `:`.

```toml
# app.toml (continued)

[data.dataviews.list_tasks]
name          = "list_tasks"
datasource    = "local_db"
query         = "SELECT id, title, description, is_complete, priority, created_at, completed_at FROM tasks WHERE is_complete = :is_complete ORDER BY priority ASC, created_at DESC LIMIT :limit OFFSET :offset"
return_schema = "schemas/task.schema.json"

[data.dataviews.list_tasks.caching]
ttl_seconds = 15

[[data.dataviews.list_tasks.parameters]]
name     = "is_complete"
type     = "boolean"
required = false
default  = false

[[data.dataviews.list_tasks.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 50

[[data.dataviews.list_tasks.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0

# ─────────────────────────

[data.dataviews.get_task]
name          = "get_task"
datasource    = "local_db"
query         = "SELECT id, title, description, is_complete, priority, created_at, completed_at FROM tasks WHERE id = :id"
return_schema = "schemas/task.schema.json"

[data.dataviews.get_task.caching]
ttl_seconds = 60

[[data.dataviews.get_task.parameters]]
name     = "id"
type     = "integer"
required = true

# ─────────────────────────

[data.dataviews.create_task]
name       = "create_task"
datasource = "local_db"
query      = "INSERT INTO tasks (title, description, is_complete, priority, created_at) VALUES (:title, :description, 0, :priority, datetime('now'))"

[[data.dataviews.create_task.parameters]]
name     = "title"
type     = "string"
required = true

[[data.dataviews.create_task.parameters]]
name     = "description"
type     = "string"
required = false

[[data.dataviews.create_task.parameters]]
name     = "priority"
type     = "integer"
required = false
default  = 3

# ─────────────────────────

[data.dataviews.complete_task]
name       = "complete_task"
datasource = "local_db"
query      = "UPDATE tasks SET is_complete = 1, completed_at = datetime('now') WHERE id = :id"

[[data.dataviews.complete_task.parameters]]
name     = "id"
type     = "integer"
required = true
```

Key points:
- SQLite uses named parameters (`:name`) -- not positional `$1` (PostgreSQL) or `?` (MySQL)
- `last_insert_id` is returned from `last_insert_rowid()` for INSERT operations
- SQLite uses `datetime('now')` instead of `NOW()`
- Booleans are stored as `0`/`1` in SQLite

## Step 5: Create a View

```toml
# app.toml (continued)

[api.views.list_tasks]
path            = "tasks"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_tasks.handler]
type     = "dataview"
dataview = "list_tasks"

[api.views.list_tasks.parameter_mapping.query]
is_complete = "is_complete"
limit       = "limit"
offset      = "offset"

# ─────────────────────────

[api.views.get_task]
path            = "tasks/{id}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_task.handler]
type     = "dataview"
dataview = "get_task"

[api.views.get_task.parameter_mapping.path]
id = "id"

# ─────────────────────────

[api.views.create_task]
path            = "tasks"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.create_task.handler]
type     = "dataview"
dataview = "create_task"

[api.views.create_task.parameter_mapping.body]
title       = "title"
description = "description"
priority    = "priority"

# ─────────────────────────

[api.views.complete_task]
path            = "tasks/{id}/complete"
method          = "POST"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.complete_task.handler]
type     = "dataview"
dataview = "complete_task"

[api.views.complete_task.parameter_mapping.path]
id = "id"
```

## Testing

```bash
# List incomplete tasks
curl http://localhost:8080/tasks

# List completed tasks
curl "http://localhost:8080/tasks?is_complete=true"

# Get a single task
curl http://localhost:8080/tasks/1

# Create a task
curl -X POST http://localhost:8080/tasks \
  -H "Content-Type: application/json" \
  -d '{"title":"Write documentation","description":"Tutorial for SQLite driver","priority":1}'

# Mark a task complete
curl -X POST http://localhost:8080/tasks/1/complete
```

## Configuration Reference

### resources.toml Fields

| Field        | Type    | Required | Description                                        |
|--------------|---------|----------|----------------------------------------------------|
| `name`       | string  | yes      | Datasource name, referenced in app.toml            |
| `driver`     | string  | yes      | Must be `"sqlite"`                                 |
| `x-type`     | string  | yes      | Must be `"sqlite"` for build-time validation       |
| `nopassword` | boolean | yes      | Must be `true` -- SQLite has no authentication     |
| `required`   | boolean | no       | Whether the app fails to start without this source |

### app.toml Datasource Config

| Field                                | Type    | Required | Default | Description                                    |
|--------------------------------------|---------|----------|---------|------------------------------------------------|
| `driver`                             | string  | yes      | --      | Must be `"sqlite"`                             |
| `nopassword`                         | boolean | yes      | --      | Must be `true`                                 |
| `database`                           | string  | yes      | --      | File path or `":memory:"`                      |

### Driver Behavior

| Setting          | Value                         |
|------------------|-------------------------------|
| Journal mode     | WAL (Write-Ahead Logging)     |
| Busy timeout     | 5 seconds                     |
| Named parameters | `:name`, `@name`, `$name`     |
| Auto-prefix      | Parameters without prefix get `:` |

### Driver Capabilities

| Capability              | Supported |
|-------------------------|-----------|
| Transactions            | Yes (via WAL) |
| Prepared statements     | No        |
| `last_insert_id`        | Yes (via `last_insert_rowid()`) |
| Named parameters        | `:name`, `@name`, `$name` |
| In-memory mode          | `":memory:"` |
