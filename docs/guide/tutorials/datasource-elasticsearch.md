# Tutorial: Elasticsearch Datasource

**Rivers v0.50.1**

## Overview

The Elasticsearch driver (`rivers-plugin-elasticsearch`) implements the `DatabaseDriver` trait. It supports full-text search, index management, and document CRUD operations through the standard request/response model. Results are normalized to `QueryResult` like all Rivers drivers.

Use Elasticsearch when you need full-text search, faceted search, aggregations, or log/event analytics. Elasticsearch is appropriate for search-heavy applications where inverted index queries, relevance scoring, and complex aggregations are required.

## Prerequisites

- A running Elasticsearch cluster accessible from the Rivers host
- LockBox initialized with Elasticsearch credentials
- The `rivers-plugin-elasticsearch` plugin present in the configured plugin directory

### Store credentials in LockBox

```bash
rivers lockbox add \
    --name elasticsearch/prod \
    --type string
# Value: elastic:mypassword (username:password or API key string)
```

## Step 1: Declare the Datasource

In `resources.toml`, declare the Elasticsearch datasource.

```toml
# resources.toml
[[datasources]]
name     = "search_engine"
driver   = "elasticsearch"
x-type   = "database"
required = true
```

## Step 2: Configure the Datasource

In `app.toml`, configure the Elasticsearch connection. The `host` and `port` fields point to the cluster endpoint.

```toml
# app.toml

# ─────────────────────────────────────────────
# Datasource
# ─────────────────────────────────────────────

[data.datasources.search_engine]
driver             = "elasticsearch"
host               = "es.internal"
port               = 9200
credentials_source = "lockbox://elasticsearch/prod"

[data.datasources.search_engine.extra]
index  = "products"
scheme = "https"

[data.datasources.search_engine.connection_pool]
max_size              = 10
connection_timeout_ms = 3000
idle_timeout_ms       = 60000

[data.datasources.search_engine.connection_pool.circuit_breaker]
enabled           = true
failure_threshold = 5
window_ms         = 60000
open_timeout_ms   = 15000
```

## Step 3: Define a Schema

Create a schema for the documents stored in Elasticsearch. Schema fields map to the document structure returned by search queries.

```json
// schemas/product.schema.json
{
  "type": "object",
  "description": "Product document for search index",
  "fields": [
    { "name": "id",          "type": "uuid",    "required": true  },
    { "name": "name",        "type": "string",  "required": true  },
    { "name": "description", "type": "string",  "required": true  },
    { "name": "category",    "type": "string",  "required": true  },
    { "name": "price",       "type": "float",   "required": true  },
    { "name": "in_stock",    "type": "boolean",  "required": true  },
    { "name": "tags",        "type": "json",    "required": false },
    { "name": "created_at",  "type": "datetime","required": true  }
  ]
}
```

## Step 4: Create a DataView

Define DataViews for searching and indexing documents. The `query` field contains the Elasticsearch query DSL as a JSON string, and the `operation` is inferred from the first token (see the driver spec operation inference algorithm).

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# DataViews
# ─────────────────────────────────────────────

# Full-text search across products
[data.dataviews.search_products]
datasource    = "search_engine"
query         = "search"
return_schema = "schemas/product.schema.json"

[data.dataviews.search_products.caching]
ttl_seconds = 30

[[data.dataviews.search_products.parameters]]
name     = "q"
type     = "string"
required = true

[[data.dataviews.search_products.parameters]]
name     = "category"
type     = "string"
required = false

[[data.dataviews.search_products.parameters]]
name     = "limit"
type     = "integer"
required = false

[[data.dataviews.search_products.parameters]]
name     = "offset"
type     = "integer"
required = false

# Get a single document by ID
[data.dataviews.get_product]
datasource    = "search_engine"
query         = "get"
return_schema = "schemas/product.schema.json"

[data.dataviews.get_product.caching]
ttl_seconds = 120

[[data.dataviews.get_product.parameters]]
name     = "id"
type     = "string"
required = true

# Index (upsert) a document
[data.dataviews.index_product]
datasource    = "search_engine"
query         = "index"
return_schema = "schemas/product.schema.json"
invalidates   = ["search_products"]

[[data.dataviews.index_product.parameters]]
name     = "id"
type     = "string"
required = true

[[data.dataviews.index_product.parameters]]
name     = "document"
type     = "string"
required = true

# Delete a document
[data.dataviews.delete_product]
datasource = "search_engine"
query      = "delete"
invalidates = ["search_products"]

[[data.dataviews.delete_product.parameters]]
name     = "id"
type     = "string"
required = true
```

## Step 5: Create a View

Expose the search and CRUD DataViews as REST endpoints.

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

# Search products
[api.views.search_products]
path            = "products/search"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.search_products.handler]
type     = "dataview"
dataview = "search_products"

[api.views.search_products.parameter_mapping.query]
q        = "q"
category = "category"
limit    = "limit"
offset   = "offset"

# Get product by ID
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

# Index a product
[api.views.index_product]
path      = "products/{id}"
method    = "PUT"
view_type = "Rest"
auth      = "none"

[api.views.index_product.handler]
type     = "dataview"
dataview = "index_product"

[api.views.index_product.parameter_mapping.path]
id = "id"

# Delete a product
[api.views.delete_product]
path      = "products/{id}"
method    = "DELETE"
view_type = "Rest"
auth      = "none"

[api.views.delete_product.handler]
type     = "dataview"
dataview = "delete_product"

[api.views.delete_product.parameter_mapping.path]
id = "id"
```

## Testing

Search for products:

```bash
curl -k "https://localhost:8080/<bundle>/<app>/products/search?q=wireless+headphones&category=electronics&limit=10"
```

Get a product by ID:

```bash
curl -k https://localhost:8080/<bundle>/<app>/products/550e8400-e29b-41d4-a716-446655440000
```

Index a new product:

```bash
curl -k -X PUT https://localhost:8080/<bundle>/<app>/products/550e8400-e29b-41d4-a716-446655440000 \
  -H "Content-Type: application/json" \
  -d '{
    "document": "{\"id\":\"550e8400-e29b-41d4-a716-446655440000\",\"name\":\"Wireless Headphones\",\"description\":\"Noise-cancelling Bluetooth headphones\",\"category\":\"electronics\",\"price\":79.99,\"in_stock\":true,\"tags\":[\"audio\",\"bluetooth\"],\"created_at\":\"2026-03-24T10:00:00Z\"}"
  }'
```

Delete a product:

```bash
curl -k -X DELETE https://localhost:8080/<bundle>/<app>/products/550e8400-e29b-41d4-a716-446655440000
```

## Configuration Reference

### Datasource fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `driver` | string | yes | Must be `"elasticsearch"` |
| `host` | string | yes | Elasticsearch cluster host |
| `port` | integer | yes | Elasticsearch port (default: 9200) |
| `credentials_source` | string | yes | LockBox URI for credentials (`username:password` or API key) |

### Extra config (`[data.datasources.*.extra]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `index` | string | -- | Default index name for operations |
| `scheme` | string | `"https"` | Connection scheme (`"http"` or `"https"`) |

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
| `search` | Full-text search with query DSL; parameters: `q`, `category`, `limit`, `offset` |
| `get` | Retrieve a document by ID |
| `index` | Index (upsert) a document |
| `delete` | Delete a document by ID |
| `create_index` | Create an index with mappings |
| `delete_index` | Delete an index |
| `ping` | Cluster health check |
