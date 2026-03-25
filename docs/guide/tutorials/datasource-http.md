# Tutorial: HTTP Datasource

**Rivers v0.50.1**

## Overview

The HTTP driver treats external HTTP services as first-class datasources. It is how an `app-main` talks to an `app-service`, and how any Rivers app makes outbound API calls. The driver handles connection pooling, path templating, auth token lifecycle, and protocol negotiation.

Use the HTTP driver when:
- An `app-main` needs to proxy API requests to an `app-service`
- Your app needs to call an external REST API as a datasource
- You need inter-service communication within a bundle

The HTTP driver is a built-in driver with a purpose-built `HttpDriver` trait -- separate from `DatabaseDriver` and `MessageBrokerDriver`. It is registered alongside them in the `DriverRegistrar`. No plugin loading is required.

This tutorial focuses on the inter-service proxy pattern -- the most common use case. For the full HTTP driver spec including SSE, WebSocket, OAuth2, and external API patterns, see `docs/arch/rivers-http-driver-spec.md`.

## Prerequisites

- A running `app-service` that exposes REST endpoints (e.g., `address-book-service`)
- The `app-service`'s `appId` from its `manifest.toml`

## Step 1: Declare the Datasource

In your `app-main`'s `resources.toml`, declare the HTTP datasource and the service dependency.

```toml
# resources.toml

[[datasources]]
name       = "address-book-api"
driver     = "http"
x-type     = "http"
nopassword = true
required   = true

[[services]]
name     = "address-book-service"
appId    = "c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a"
required = true
```

- `[[datasources]]` -- the HTTP driver makes outbound calls to declared services
- `nopassword = true` -- service-to-service calls carry session auth automatically; no lockbox needed for intra-bundle traffic
- `[[services]]` -- declares a startup dependency. `appId` must exactly match the target service's `appId` from its `manifest.toml`. Rivers will not start `app-main` until the declared service is healthy.

## Step 2: Configure the Datasource

In your `app-main`'s `app.toml`, configure the HTTP datasource to point at the service.

```toml
# app.toml

[data.datasources.address-book-api]
driver     = "http"
service    = "address-book-service"
nopassword = true

[data.datasources.address-book-api.config]
base_path      = "/api"
timeout_ms     = 5000
retry_attempts = 2
```

- `service` -- logical service name. Rivers resolves this to the running service's endpoint at startup using the service registry.
- `base_path` -- all DataView query paths are relative to this prefix
- `timeout_ms` -- per-request timeout for outbound calls
- `retry_attempts` -- number of retries on transient failure

The `service` field replaces `base_url` for intra-bundle communication. Rivers handles endpoint discovery automatically -- you never hardcode ports or hostnames for internal services.

### External API Pattern

For calling external APIs outside your bundle, use `base_url` with `credentials_source` instead of `service`:

```toml
[data.datasources.external_api]
driver             = "http"
base_url           = "https://api.example.com"
auth               = "bearer"
credentials_source = "lockbox://external/api_key"
pool_size          = 10
connect_timeout_ms = 5000
request_timeout_ms = 30000
```

## Step 3: Define a Schema

For HTTP proxy DataViews, the schema defines the shape of the upstream response. Create `schemas/contact.schema.json` if you want return schema validation.

```json
{
  "type": "object",
  "description": "Contact record from address-book-service",
  "fields": [
    { "name": "id",         "type": "uuid",     "required": true  },
    { "name": "first_name", "type": "string",   "required": true  },
    { "name": "last_name",  "type": "string",   "required": true  },
    { "name": "email",      "type": "email",    "required": true  },
    { "name": "phone",      "type": "phone",    "required": false },
    { "name": "company",    "type": "string",   "required": false },
    { "name": "city",       "type": "string",   "required": false }
  ]
}
```

Schema validation is optional on HTTP DataViews. If `return_schema` is set, the upstream JSON response is validated against it. Omit it to pass through the upstream response without validation.

## Step 4: Create a DataView

For HTTP DataViews, the `query` field is a URL path relative to `base_path`. Parameters are forwarded as query string arguments automatically.

```toml
# app.toml (continued)

[data.dataviews.proxy_list_contacts]
datasource = "address-book-api"
query      = "/contacts"
method     = "GET"

[data.dataviews.proxy_list_contacts.cache]
enabled     = true
ttl_seconds = 60

[[data.dataviews.proxy_list_contacts.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[[data.dataviews.proxy_list_contacts.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0

# ─────────────────────────

[data.dataviews.proxy_search_contacts]
datasource = "address-book-api"
query      = "/contacts/search"
method     = "GET"

[data.dataviews.proxy_search_contacts.cache]
enabled     = true
ttl_seconds = 30

[[data.dataviews.proxy_search_contacts.parameters]]
name     = "q"
type     = "string"
required = true

[[data.dataviews.proxy_search_contacts.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20
```

Key points:
- `query = "/contacts"` -- relative to `base_path` (`/api`), so the full upstream path is `/api/contacts`
- `method = "GET"` -- the HTTP method for the outbound request
- Parameters with `required = false` and a `default` are forwarded as query string parameters
- The upstream response is wrapped in `QueryResult` for compatibility with the DataView engine: JSON object becomes a single row, JSON array becomes one row per element

### HTTP DataView with Path Parameters

For proxying to endpoints with path parameters:

```toml
[data.dataviews.proxy_get_contact]
datasource = "address-book-api"
query      = "/contacts/{id}"
method     = "GET"

[[data.dataviews.proxy_get_contact.parameters]]
name     = "id"
type     = "uuid"
required = true
```

Path parameters use `{param}` syntax in the query path. They are substituted before the request is sent.

## Step 5: Create a View

```toml
# app.toml (continued)

[api.views.list_contacts]
path            = "/api/contacts"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_contacts.handler]
type     = "data_view"
dataview = "proxy_list_contacts"

[api.views.list_contacts.parameter_mapping.query]
limit  = "limit"
offset = "offset"

# ─────────────────────────

[api.views.search_contacts]
path            = "/api/contacts/search"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.search_contacts.handler]
type     = "data_view"
dataview = "proxy_search_contacts"

[api.views.search_contacts.parameter_mapping.query]
q     = "q"
limit = "limit"
```

### Request Flow

```
Browser
  --> GET /api/contacts?limit=10
  --> app-main (port 8080)
      --> View: list_contacts
          --> DataView: proxy_list_contacts
              --> HTTP driver
                  --> GET /api/contacts?limit=10
                  --> address-book-service (port 9100)
                      --> DataView: list_contacts
                          --> faker driver
                          <-- generated contacts
                  <-- JSON response
          <-- QueryResult
      <-- envelope response
  <-- JSON to browser
```

### SPA Configuration

If your `app-main` serves a frontend SPA alongside the proxy endpoints, add the static files config:

```toml
# app.toml (continued)

[static_files]
enabled      = true
root         = "libraries/spa"
index_file   = "index.html"
spa_fallback = true
```

- `spa_fallback = true` -- all non-API routes serve `index.html`; `/api/*` routes always take precedence

## Testing

```bash
# List contacts through the proxy
curl http://localhost:8080/api/contacts

# List contacts with pagination
curl "http://localhost:8080/api/contacts?limit=10&offset=20"

# Search contacts through the proxy
curl "http://localhost:8080/api/contacts/search?q=john&limit=5"
```

These requests hit `app-main` on port 8080, which proxies them through the HTTP datasource to `address-book-service` on port 9100.

## Configuration Reference

### resources.toml Fields

| Field        | Type    | Required | Description                                          |
|--------------|---------|----------|------------------------------------------------------|
| `name`       | string  | yes      | Datasource name, referenced in app.toml              |
| `driver`     | string  | yes      | Must be `"http"`                                     |
| `x-type`     | string  | yes      | Must be `"http"` for build-time validation           |
| `nopassword` | boolean | cond.    | `true` for intra-bundle; omit for credentialed APIs  |
| `required`   | boolean | no       | Whether the app fails to start without this source   |

### resources.toml Service Dependency

| Field      | Type    | Required | Description                                              |
|------------|---------|----------|----------------------------------------------------------|
| `name`     | string  | yes      | Logical service name matching the target's `appName`     |
| `appId`    | string  | yes      | UUID matching the target service's `manifest.toml` appId |
| `required` | boolean | no       | Whether the app fails to start without this service      |

### app.toml Datasource Config (Intra-Bundle)

| Field                              | Type    | Required | Default | Description                                     |
|------------------------------------|---------|----------|---------|-------------------------------------------------|
| `driver`                           | string  | yes      | --      | Must be `"http"`                                |
| `service`                          | string  | yes      | --      | Logical service name for endpoint resolution    |
| `nopassword`                       | boolean | yes      | --      | `true` for intra-bundle                         |
| `config.base_path`                 | string  | no       | `""`    | Path prefix for all DataView queries            |
| `config.timeout_ms`               | integer | no       | 30000   | Per-request timeout                             |
| `config.retry_attempts`           | integer | no       | 1       | Retry count on transient failure                |

### app.toml Datasource Config (External API)

| Field                              | Type    | Required | Default | Description                                     |
|------------------------------------|---------|----------|---------|-------------------------------------------------|
| `driver`                           | string  | yes      | --      | Must be `"http"`                                |
| `base_url`                         | string  | yes      | --      | Full base URL of the external API               |
| `auth`                             | string  | no       | --      | `"bearer"`, `"basic"`, `"api_key"`, `"oauth2_client_credentials"` |
| `auth_header`                      | string  | cond.    | --      | Required when `auth = "api_key"`                |
| `credentials_source`               | string  | cond.    | --      | LockBox URI; required when `auth` is set        |
| `pool_size`                        | integer | no       | 10      | Max concurrent connections                      |
| `connect_timeout_ms`               | integer | no       | 5000    | Connection establishment timeout                |
| `request_timeout_ms`               | integer | no       | 30000   | Per-request timeout                             |

### HTTP DataView Config

| Field           | Type    | Required | Default        | Description                                   |
|-----------------|---------|----------|----------------|-----------------------------------------------|
| `datasource`    | string  | yes      | --             | Name of the HTTP datasource                   |
| `query`         | string  | yes      | --             | URL path (relative to `base_path`)            |
| `method`        | string  | yes      | --             | HTTP method: `GET`, `POST`, `PUT`, `DELETE`   |
| `return_schema` | string  | no       | --             | Schema file for response validation           |
| `timeout_ms`    | integer | no       | from datasource| Per-DataView timeout override                 |

### Auth Models

| Auth Type                    | LockBox Value Format                                    | Header Injected                              |
|------------------------------|--------------------------------------------------------|----------------------------------------------|
| `bearer`                     | Token string                                           | `Authorization: Bearer <token>`              |
| `basic`                      | JSON: `{"username":"...","password":"..."}`             | `Authorization: Basic <base64>`              |
| `api_key`                    | Key string                                             | `<auth_header>: <key>`                       |
| `oauth2_client_credentials`  | JSON: `{"client_id":"...","client_secret":"...","token_url":"..."}` | `Authorization: Bearer <managed_token>` |
