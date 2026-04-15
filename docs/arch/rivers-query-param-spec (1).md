# Rivers URL Query Parameter Specification

**Document Type:** Spec Patch  
**Scope:** Inbound URL query string parsing, parameter mapping, type coercion, validation  
**Status:** Design / Pre-Implementation  
**Patches:** `rivers-view-layer-spec.md` §5.2, `rivers-technology-path-spec.md` §2.3  
**Depends On:** Epic 10 (DataView Engine), Epic 13 (View Layer)

---

## 1. Overview

URL query parameters flow through three layers: HTTP parsing, view-layer extraction, and DataView parameter binding. This spec formalizes the full lifecycle from `?key=value` on the wire to `$key` in a SQL query or `{key}` in an HTTP path template.

---

## 2. Wire Parsing

### 2.1 Extraction

Riverbed HTTPD parses the query string from the request URI after the `?` delimiter. The raw query string is preserved on the `RbRequest` for consumer use. Parsing follows RFC 3986 §3.4.

Key-value pairs are delimited by `&`. Keys and values are percent-decoded (RFC 3986 §2.1). Empty values (`?key=`) produce an empty string. Bare keys (`?key`) produce an empty string. The `?` itself is not part of the query string.

### 2.2 Duplicate Keys

When a key appears multiple times (`?tag=a&tag=b`), all values are preserved in declaration order. The parsed representation is `HashMap<String, Vec<String>>` at the HTTP layer.

### 2.3 Constraints

| ID | Rule |
|---|---|
| QP-1 | Query string parsing MUST follow RFC 3986 §3.4. Percent-encoded characters MUST be decoded. |
| QP-2 | Keys and values are always strings at the HTTP layer. Type coercion happens at the DataView parameter binding layer, not at parse time. |
| QP-3 | Query strings exceeding the configured `max_query_string_bytes` (default: 8192) MUST return 414 URI Too Long. Configurable via `EngineFactory`. |

---

## 3. Handler Access

### 3.1 `ctx.request.query`

Query parameters are available in handlers as `ctx.request.query` — a `Record<string, string>`. This is the spec-canonical field name.

```typescript
// Request: GET /api/orders?status=active&limit=20
export async function listOrders(ctx): Promise<void> {
    const status = ctx.request.query.status;   // "active"
    const limit  = ctx.request.query.limit;    // "20" (string)
}
```

**Naming history:** The Rust struct originally used `query_params`. BUG-012 added `#[serde(rename = "query")]` to match the spec. The canonical name is `query`. The field `query_params` MUST NOT exist on the serialized request object.

### 3.2 Multi-Value Access

For duplicate keys, `ctx.request.query` returns the **first** value. To access all values, use `ctx.request.queryAll`:

```typescript
// Request: GET /api/products?tag=electronics&tag=sale
ctx.request.query.tag           // "electronics" (first value)
ctx.request.queryAll.tag        // ["electronics", "sale"]
```

`ctx.request.queryAll` is `Record<string, string[]>`. Every key has an array, even single-value keys (array of one).

### 3.3 Constraints

| ID | Rule |
|---|---|
| QP-4 | `ctx.request.query` MUST be `Record<string, string>` with first-value-wins for duplicate keys. |
| QP-5 | `ctx.request.queryAll` MUST be `Record<string, string[]>` preserving all values in declaration order. |
| QP-6 | Missing keys return `undefined`, not empty string. Handlers test with `if (ctx.request.query.page)`. |

---

## 4. Parameter Mapping

### 4.1 Declaration

Parameter mapping connects inbound HTTP request data to DataView parameters. Four source locations are supported: `path`, `query`, `body`, and `header`.

```toml
[api.views.search_orders.parameter_mapping.query]
status = "status"
limit  = "limit"
offset = "offset"

[api.views.search_orders.parameter_mapping.path]
id = "id"

[api.views.search_orders.parameter_mapping.body]
customer_id = "customer_id"

[api.views.search_orders.parameter_mapping.header]
x-tenant-id = "tenant_id"
```

Format: `{http_param_name} = "{dataview_param_name}"`

The left side is the name as it appears in the HTTP request (query string key, path segment name, body field name, header name). The right side is the DataView parameter name it maps to.

### 4.2 Resolution Order

When a request arrives, the view layer extracts parameters from each declared source and assembles a flat parameter map for the DataView engine:

```
1. Path parameters    — extracted from URL path segments matching {name} in route
2. Query parameters   — extracted from URL query string
3. Body parameters    — extracted from parsed JSON request body (POST/PUT/PATCH)
4. Header parameters  — extracted from request headers
```

All four sources merge into a single `HashMap<String, QueryValue>` passed to the DataView engine. If the same DataView parameter name appears in multiple sources, the **last source in resolution order wins** (header > body > query > path). This is intentional — it allows header-based overrides for multi-tenant patterns.

### 4.3 Query Parameter Mapping Rules

```toml
[api.views.list_contacts.parameter_mapping.query]
q      = "q"
limit  = "limit"
offset = "offset"
sort   = "sort_field"
```

| HTTP request | DataView param map |
|---|---|
| `GET /api/contacts?q=smith&limit=10` | `{ q: "smith", limit: "10" }` |
| `GET /api/contacts?q=smith` | `{ q: "smith" }` — limit/offset absent |
| `GET /api/contacts` | `{}` — all query params absent |
| `GET /api/contacts?q=smith&extra=ignored` | `{ q: "smith" }` — unmapped params discarded |

Unmapped query parameters (those not declared in `parameter_mapping.query`) are silently discarded. They are still available in `ctx.request.query` for handler access but do not flow into the DataView engine.

### 4.4 Combined Path + Query Mapping

The common REST pattern: path identifies the resource, query params filter or paginate.

```toml
[api.views.contacts_by_city]
path   = "/api/contacts/city/{city}"
method = "GET"

[api.views.contacts_by_city.parameter_mapping.path]
city = "city"

[api.views.contacts_by_city.parameter_mapping.query]
limit = "limit"
```

Request: `GET /api/contacts/city/Detroit?limit=25`  
DataView params: `{ city: "Detroit", limit: "25" }`

### 4.5 Renaming

The left and right sides don't need to match. This allows decoupling the HTTP API contract from the DataView parameter names:

```toml
[api.views.search.parameter_mapping.query]
q       = "search_term"       # HTTP ?q= maps to DataView $search_term
per     = "limit"              # HTTP ?per= maps to DataView $limit
pg      = "offset"             # HTTP ?pg= maps to DataView $offset
```

Request: `GET /api/search?q=rivers&per=10&pg=2`  
DataView params: `{ search_term: "rivers", limit: "10", offset: "2" }`

---

## 5. Type Coercion

All URL query parameters arrive as strings. The DataView parameter declaration includes a `type` field. The DataView engine coerces the string value to the declared type before passing it to the driver.

### 5.1 Coercion Rules

| Declared type | Input string | Result | Error |
|---|---|---|---|
| `string` | `"smith"` | `"smith"` | Never fails |
| `integer` | `"42"` | `42` | Non-numeric string → 400 |
| `decimal` | `"19.99"` | `19.99` | Non-numeric string → 400 |
| `boolean` | `"true"` / `"false"` / `"1"` / `"0"` | `true` / `false` | Other values → 400 |
| `uuid` | `"550e8400-..."` | `"550e8400-..."` (validated format) | Invalid UUID → 400 |
| `date` | `"2026-04-15"` | `"2026-04-15"` (validated ISO 8601) | Invalid date → 400 |
| `array` | `"a,b,c"` | `["a","b","c"]` | Never fails (split on comma) |

### 5.2 Array Parameters

Two patterns for array-valued query parameters:

**Pattern 1 — Repeated key:** `?tag=a&tag=b&tag=c`  
When the DataView parameter declares `type = "array"`, the view layer collects all values from `queryAll` and passes them as a JSON array.

**Pattern 2 — Comma-separated:** `?tags=a,b,c`  
When the DataView parameter declares `type = "array"` and the value is a single string containing commas, it is split on `,` and passed as a JSON array.

Pattern 1 takes precedence — if multiple values exist for the key, comma splitting is not applied to individual values.

### 5.3 Constraints

| ID | Rule |
|---|---|
| QP-7 | Type coercion failures MUST return HTTP 400 with the standard `ErrorResponse` envelope, including the parameter name and expected type in the `details` field. |
| QP-8 | Coercion is applied after parameter mapping, before DataView execution. The DataView engine receives typed `QueryValue` values, not raw strings. |
| QP-9 | If no `type` is declared on the DataView parameter, the value is passed as a string (no coercion). |

---

## 6. Default Values and Required Parameters

### 6.1 Defaults

DataView parameter declarations support `default` values for optional parameters:

```toml
[[data.dataviews.list_contacts.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = "25"
```

When the query parameter is absent from the URL, the default value is used. Default values go through the same type coercion as request values.

### 6.2 Required Parameters

```toml
[[data.dataviews.get_contact.parameters]]
name     = "id"
type     = "uuid"
required = true
```

If a required parameter is absent from all mapped sources (path, query, body, header), the request fails with HTTP 400:

```json
{
  "code": 400,
  "message": "Missing required parameter: id",
  "details": "Parameter 'id' is required but was not found in path, query, body, or header sources",
  "trace_id": "abc-123"
}
```

### 6.3 Constraints

| ID | Rule |
|---|---|
| QP-10 | Required parameter validation runs after all sources are merged. A parameter is "present" if any mapped source provides it. |
| QP-11 | Default values MUST go through type coercion. A default of `"25"` on an `integer` parameter produces `QueryValue::Integer(25)`, not `QueryValue::String("25")`. |
| QP-12 | Default values are applied at the DataView engine layer, not the view layer. The parameter map passed to the engine may have absent keys — the engine applies defaults before execution. |

---

## 7. Outbound Query Parameters (HTTP Driver)

When Rivers makes outbound HTTP requests via the HTTP driver, query parameters flow in the opposite direction — from DataView parameter declarations to the outbound URL.

### 7.1 Static Query Parameters

```toml
[data.dataviews.search_records]
datasource = "salesforce"
method     = "GET"
path       = "/services/data/v57.0/query"

[data.dataviews.search_records.query_params]
format = "json"
```

Static query params are appended to every outbound request for this DataView, regardless of input parameters.

### 7.2 Dynamic Query Parameters

Parameters declared with `location = "query"` are appended to the outbound URL at execution time:

```toml
[[data.dataviews.search_records.parameters]]
name     = "q"
location = "query"
required = true
```

Produces: `GET /services/data/v57.0/query?format=json&q=SELECT+Id+FROM+Account`

### 7.3 Parameter Encoding

Outbound query parameter values are percent-encoded per RFC 3986 §2.1. Spaces are encoded as `%20`, not `+`. The `+` encoding (RFC 1866 / HTML form encoding) is accepted on inbound parsing but never produced on outbound requests.

### 7.4 Merge Order

Static `query_params` are applied first. Dynamic parameters with `location = "query"` are appended after. If both declare the same key, the dynamic value wins.

### 7.5 Constraints

| ID | Rule |
|---|---|
| QP-13 | Outbound query values MUST be percent-encoded per RFC 3986 §2.1. |
| QP-14 | Dynamic query parameters override static query parameters with the same key. |
| QP-15 | Empty string values produce `?key=` (key present, empty value). Null/absent values omit the key entirely. |

---

## 8. MCP Integration

MCP `tools/call` arguments are a flat JSON object. The MCP dispatcher passes them directly to the DataView engine as the parameter map. The DataView's existing parameter declarations (including `location`) determine where each value goes on the wire.

```
MCP: { "user_id": "42", "status": "active" }
  → DataView engine reads parameter declarations
  → user_id: location = "path"  → /v1/users/42/orders
  → status:  location = "query" → ?status=active
```

The MCP layer is unaware of parameter locations. This is the same pipeline used by REST views — MCP is just another entry point.

See `rivers-mcp-view-spec.md` §10 for the full MCP parameter resolution specification.

---

## 9. End-to-End Resolution Flow

### 9.1 Inbound REST

```
Client: GET /api/contacts/city/Detroit?limit=25&sort=name

1. Riverbed HTTPD parses URL
   → path = "/api/contacts/city/Detroit"
   → query_string = "limit=25&sort=name"

2. Router matches path pattern /api/contacts/city/{city}
   → path_params = { city: "Detroit" }

3. Query string parsed
   → query = { limit: "25", sort: "name" }

4. Parameter mapping applied:
   [parameter_mapping.path]   city  = "city"       → { city: "Detroit" }
   [parameter_mapping.query]  limit = "limit"      → { limit: "25" }
                              sort  = "sort_field"  → { sort_field: "name" }

5. Merged param map: { city: "Detroit", limit: "25", sort_field: "name" }

6. Type coercion (from DataView parameter declarations):
   city:       type = "string"   → "Detroit"   (no coercion)
   limit:      type = "integer"  → 25          (string → int)
   sort_field:  type = "string"   → "name"     (no coercion)

7. DataView engine executes:
   SELECT * FROM contacts WHERE city = $city ORDER BY $sort_field LIMIT $limit
   bindings: { city: "Detroit", sort_field: "name", limit: 25 }
```

### 9.2 Outbound HTTP Driver

```
Handler calls: ctx.dataview("get_orders", { user_id: "42", status: "active" })

1. DataView engine receives params: { user_id: "42", status: "active" }

2. Parameter declarations consulted:
   user_id: location = "path",  required = true
   status:  location = "query", required = false, default = "active"

3. Path template resolved:
   "/v1/users/{user_id}/orders" → "/v1/users/42/orders"

4. Query params assembled:
   → "?status=active"

5. Static query_params merged:
   [query_params] format = "json" → "?status=active&format=json"

6. Final outbound request:
   GET /v1/users/42/orders?status=active&format=json
```

---

## 10. Validation Rules

Config validation at app startup (fail-fast):

| ID | Rule |
|---|---|
| VAL-QP-1 | Every key in `parameter_mapping.query` must map to a declared DataView parameter. Orphan mappings fail at startup. |
| VAL-QP-2 | Every key in `parameter_mapping.path` must correspond to a `{name}` segment in the view's `path`. Orphan path mappings fail at startup. |
| VAL-QP-3 | If a DataView parameter is `required = true` and has no mapping from any source (path, query, body, header) and no `default`, a startup warning is emitted. The parameter will always fail at runtime. |
| VAL-QP-4 | Parameter mapping keys are case-sensitive. `Status` and `status` are different query parameter names. |
| VAL-QP-5 | A DataView parameter MUST NOT be mapped from more than one source within the same mapping section. Duplicate right-side values within a single source section fail at startup. Cross-source duplicates are allowed (last-source-wins). |

---

## CHANGELOG

| Date | Change |
|---|---|
| 2026-04-15 | Initial specification — URL Query Parameter lifecycle |
