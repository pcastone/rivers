# Tutorial: MongoDB Datasource

**Rivers v0.50.1**

## Overview

The MongoDB driver (`rivers-plugin-mongodb`) implements the `DatabaseDriver` trait. It supports document CRUD operations, aggregation pipelines, and collection management through the standard request/response model. All results are normalized to `QueryResult` -- each MongoDB document becomes a row with fields mapped to `QueryValue` variants.

Use MongoDB when you need a flexible document store with rich querying, aggregation, and schema-on-read flexibility. MongoDB is appropriate for applications with heterogeneous data shapes, nested documents, and workloads that benefit from document-oriented storage.

## Prerequisites

- A running MongoDB instance or replica set accessible from the Rivers host
- LockBox initialized with MongoDB credentials
- The `rivers-plugin-mongodb` plugin present in the configured plugin directory

### Store credentials in LockBox

```bash
rivers lockbox add \
    --name mongodb/prod \
    --type string
# Value: mongodb://myuser:mypassword@host:27017 (full MongoDB URI)
```

## Step 1: Declare the Datasource

In `resources.toml`, declare the MongoDB datasource.

```toml
# resources.toml
[[datasources]]
name     = "documents_db"
driver   = "mongodb"
x-type   = "database"
required = true
```

## Step 2: Configure the Datasource

In `app.toml`, configure the MongoDB connection. The `credentials_source` resolves to a full MongoDB connection URI from LockBox.

```toml
# app.toml

# ─────────────────────────────────────────────
# Datasource
# ─────────────────────────────────────────────

[data.datasources.documents_db]
driver             = "mongodb"
host               = "mongo.internal"
port               = 27017
database           = "myapp"
credentials_source = "lockbox://mongodb/prod"

[data.datasources.documents_db.extra]
collection    = "articles"
authSource    = "admin"
replicaSet    = "rs0"

[data.datasources.documents_db.connection_pool]
max_size              = 20
min_idle              = 2
connection_timeout_ms = 3000
idle_timeout_ms       = 60000

[data.datasources.documents_db.connection_pool.circuit_breaker]
enabled           = true
failure_threshold = 5
window_ms         = 60000
open_timeout_ms   = 15000
```

## Step 3: Define a Schema

Create a schema for the documents stored in MongoDB.

```json
// schemas/article.schema.json
{
  "type": "object",
  "description": "Article document",
  "fields": [
    { "name": "id",         "type": "uuid",     "required": true  },
    { "name": "title",      "type": "string",   "required": true  },
    { "name": "slug",       "type": "string",   "required": true  },
    { "name": "content",    "type": "string",   "required": true  },
    { "name": "author_id",  "type": "uuid",     "required": true  },
    { "name": "tags",       "type": "json",     "required": false },
    { "name": "published",  "type": "boolean",  "required": true  },
    { "name": "created_at", "type": "datetime", "required": true  },
    { "name": "updated_at", "type": "datetime", "required": false }
  ]
}
```

## Step 4: Create a DataView

Define DataViews for CRUD operations. MongoDB operations use `find`, `insert`, `update`, and `delete` as the `query` operation. The `Json` variant of `QueryValue` is used for MongoDB documents and filter expressions.

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# DataViews
# ─────────────────────────────────────────────

# List articles with pagination
[data.dataviews.list_articles]
datasource    = "documents_db"
query         = "find"
return_schema = "schemas/article.schema.json"

[data.dataviews.list_articles.caching]
ttl_seconds = 60

[[data.dataviews.list_articles.parameters]]
name     = "limit"
type     = "integer"
required = false

[[data.dataviews.list_articles.parameters]]
name     = "offset"
type     = "integer"
required = false

[[data.dataviews.list_articles.parameters]]
name     = "published"
type     = "boolean"
required = false

# Get a single article by ID
[data.dataviews.get_article]
datasource    = "documents_db"
query         = "find"
return_schema = "schemas/article.schema.json"

[data.dataviews.get_article.caching]
ttl_seconds = 300

[[data.dataviews.get_article.parameters]]
name     = "id"
type     = "string"
required = true

# Insert a new article
[data.dataviews.create_article]
datasource    = "documents_db"
query         = "insert"
return_schema = "schemas/article.schema.json"
invalidates   = ["list_articles"]

[[data.dataviews.create_article.parameters]]
name     = "document"
type     = "string"
required = true

# Update an article
[data.dataviews.update_article]
datasource = "documents_db"
query      = "update"
invalidates = ["list_articles", "get_article"]

[[data.dataviews.update_article.parameters]]
name     = "id"
type     = "string"
required = true

[[data.dataviews.update_article.parameters]]
name     = "document"
type     = "string"
required = true

# Delete an article
[data.dataviews.delete_article]
datasource = "documents_db"
query      = "delete"
invalidates = ["list_articles"]

[[data.dataviews.delete_article.parameters]]
name     = "id"
type     = "string"
required = true
```

## Step 5: Create a View

Expose the CRUD DataViews as REST endpoints.

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

# List articles
[api.views.list_articles]
path            = "articles"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_articles.handler]
type     = "dataview"
dataview = "list_articles"

[api.views.list_articles.parameter_mapping.query]
limit     = "limit"
offset    = "offset"
published = "published"

# Get article by ID
[api.views.get_article]
path            = "articles/{id}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_article.handler]
type     = "dataview"
dataview = "get_article"

[api.views.get_article.parameter_mapping.path]
id = "id"

# Create article
[api.views.create_article]
path      = "articles"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.create_article.handler]
type     = "dataview"
dataview = "create_article"

# Update article
[api.views.update_article]
path      = "articles/{id}"
method    = "PUT"
view_type = "Rest"
auth      = "none"

[api.views.update_article.handler]
type     = "dataview"
dataview = "update_article"

[api.views.update_article.parameter_mapping.path]
id = "id"

# Delete article
[api.views.delete_article]
path      = "articles/{id}"
method    = "DELETE"
view_type = "Rest"
auth      = "none"

[api.views.delete_article.handler]
type     = "dataview"
dataview = "delete_article"

[api.views.delete_article.parameter_mapping.path]
id = "id"
```

## Testing

List articles:

```bash
curl -k "https://localhost:8080/<bundle>/<app>/articles?limit=10&offset=0&published=true"
```

Get an article by ID:

```bash
curl -k https://localhost:8080/<bundle>/<app>/articles/550e8400-e29b-41d4-a716-446655440000
```

Create a new article:

```bash
curl -k -X POST https://localhost:8080/<bundle>/<app>/articles \
  -H "Content-Type: application/json" \
  -d '{
    "document": "{\"id\":\"550e8400-e29b-41d4-a716-446655440000\",\"title\":\"Getting Started with Rivers\",\"slug\":\"getting-started-rivers\",\"content\":\"Rivers is a declarative app-service framework...\",\"author_id\":\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\",\"tags\":[\"rivers\",\"tutorial\"],\"published\":true,\"created_at\":\"2026-03-24T10:00:00Z\"}"
  }'
```

Update an article:

```bash
curl -k -X PUT https://localhost:8080/<bundle>/<app>/articles/550e8400-e29b-41d4-a716-446655440000 \
  -H "Content-Type: application/json" \
  -d '{
    "document": "{\"title\":\"Getting Started with Rivers (Updated)\",\"updated_at\":\"2026-03-24T12:00:00Z\"}"
  }'
```

Delete an article:

```bash
curl -k -X DELETE https://localhost:8080/<bundle>/<app>/articles/550e8400-e29b-41d4-a716-446655440000
```

## Configuration Reference

### Datasource fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `driver` | string | yes | Must be `"mongodb"` |
| `host` | string | yes | MongoDB host |
| `port` | integer | yes | MongoDB port (default: 27017) |
| `database` | string | yes | Database name |
| `credentials_source` | string | yes | LockBox URI for credentials (full MongoDB URI) |

### Extra config (`[data.datasources.*.extra]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `collection` | string | -- | Default collection name for operations |
| `authSource` | string | `"admin"` | Authentication database |
| `replicaSet` | string | -- | Replica set name for cluster connections |

### Connection pool (`[data.datasources.*.connection_pool]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_size` | integer | `10` | Maximum connections in the pool |
| `min_idle` | integer | `0` | Minimum idle connections maintained |
| `connection_timeout_ms` | integer | `500` | Timeout for acquiring a connection |
| `idle_timeout_ms` | integer | `30000` | Close idle connections after this duration |
| `max_lifetime_ms` | integer | `300000` | Maximum connection lifetime before recycling |

### DatabaseDriver operations

| Operation | Description |
|-----------|-------------|
| `find` | Query documents with filter; parameters: filter fields, `limit`, `offset` |
| `insert` | Insert a document; parameter: `document` (JSON string) |
| `update` | Update a document by ID; parameters: `id`, `document` (partial update JSON) |
| `delete` | Delete a document by ID; parameter: `id` |
| `aggregate` | Run an aggregation pipeline; parameter: `pipeline` (JSON array string) |
| `count` | Count documents matching a filter |
| `ping` | Database health check |
