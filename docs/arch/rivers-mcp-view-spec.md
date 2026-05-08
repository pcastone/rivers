# Rivers MCP View Type Specification

**Document Type:** Spec Addition / Patch  
**Scope:** MCP view type — JSON-RPC dispatch, tool/resource/prompt exposure, instructions, session management  
**Status:** Design / Pre-Implementation  
**Patches:** `rivers-view-layer-spec.md`, `rivers-processpool-runtime-spec-v2.md`, `rivers-httpd-spec.md`  
**Depends On:** Epic 10 (DataView Engine), Epic 12 (ProcessPool), Epic 13 (View Layer), Epic 4 (EventBus), StorageEngine  
**Protocol Target:** MCP Streamable HTTP — current stable specification at implementation time

---

## Table of Contents

1. [Design Rationale](#1-design-rationale)
2. [View Declaration](#2-view-declaration)
3. [JSON-RPC Dispatch Layer](#3-json-rpc-dispatch-layer)
4. [Tools](#4-tools)
5. [Resources](#5-resources)
6. [Prompts](#6-prompts)
7. [Instructions](#7-instructions)
8. [Streaming Tools](#8-streaming-tools)
9. [Session Management](#9-session-management)
10. [Parameter Resolution](#10-parameter-resolution)
11. [Error Handling](#11-error-handling)
12. [Validation Rules](#12-validation-rules)
13. [Configuration Reference](#13-configuration-reference)
14. [Examples](#14-examples)

---

## 1. Design Rationale

### 1.1 Why a View Type

A DataView is already an MCP tool. It has a name, typed input parameters (JSON Schema via `parameters`), typed output (via `get_schema`/`post_schema`), and a deterministic execution path. The MCP `tools/list` response is a projection of the DataView registry. `tools/call` is `dataview_engine.execute(name, params)` with a JSON-RPC envelope.

The framework already does the hard part — parameter validation, schema enforcement, datasource dispatch, capability isolation. MCP is a wire protocol on top of the same execution engine.

MCP Streamable HTTP is a single-endpoint protocol: the client POSTs JSON-RPC messages to one URL, receives either a JSON response or an SSE stream. This maps to a single Rivers view with a framework-level dispatcher — same architectural level as the REST, SSE, and WebSocket dispatchers.

### 1.2 Ownership Boundary

- **Framework MUST** — JSON-RPC parsing, method routing, session lifecycle, tool/resource/prompt schema generation, instructions compilation, parameter injection into DataView engine, streaming response mode switching.
- **Developer MUST** — Whitelist which DataViews to expose, provide descriptions and hints, write prompt templates and instructions markdown.
- **Framework MUST NOT** — expose DataViews not explicitly whitelisted, interpret tool results, inject MCP concerns into the DataView layer.

### 1.3 Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Exposure model | Whitelist only | Explicit is safer; prevents accidental exposure of internal DataViews |
| Descriptions | MCP config, not DataView | Different audience (AI model vs developer), different metadata; separation of concerns |
| Prompts | Markdown templates with `{arg}` substitution | Same substitution engine as DataView path/body templating |
| Instructions | Compiled tool catalog + optional static markdown file | Two-document agentic training pattern at the framework level |
| Protocol version | Current stable at implementation time | MCP spec is still evolving; pinning risks staleness |
| `notifications/tools/list_changed` | Deferred | Only relevant for hot reload in dev mode; production tool lists are static |

---

## 2. View Declaration

```toml
[api.views.mcp]
path         = "/mcp"
view_type    = "Mcp"
guard        = "api_key_guard"
instructions = "docs/mcp-help.md"

[api.views.mcp.tools.get_orders]
dataview    = "get_orders"
description = "Retrieve orders for a customer by ID or date range"
hints       = { read_only = true }

[api.views.mcp.tools.create_order]
dataview    = "create_order"
description = "Create a new order with line items and shipping address"
hints       = { destructive = false, idempotent = false }

[api.views.mcp.tools.cancel_order]
dataview    = "cancel_order"
description = "Cancel an existing order by order ID"
hints       = { destructive = true, idempotent = true }

[api.views.mcp.resources.product_catalog]
dataview    = "get_products"
description = "Browse the product catalog with optional category filter"

[api.views.mcp.resources.inventory_status]
dataview    = "get_inventory"
description = "Check current inventory levels for a product"

[api.views.mcp.prompts.fulfill_order]
description = "Complete the order fulfillment workflow"
template    = "prompts/fulfill_order.md"

[[api.views.mcp.prompts.fulfill_order.arguments]]
name     = "order_id"
required = true

[[api.views.mcp.prompts.fulfill_order.arguments]]
name     = "priority"
required = false
default  = "normal"
```

### 2.1 Constraints

| ID | Rule |
|---|---|
| MCP-1 | `view_type = "Mcp"` activates the MCP JSON-RPC dispatcher. No other view type processes MCP traffic. |
| MCP-2 | Multiple MCP views per application are allowed. Each `view_type = "Mcp"` view registers its own JSON-RPC endpoint at its configured `path` and its own `/instructions` sibling route. MCP clients connect to one path at a time; there is no automatic tool aggregation across views. |
| MCP-3 | Every tool and resource MUST reference a DataView declared in `[data.dataviews.*]`. References to undeclared DataViews fail at config validation. |
| MCP-4 | `instructions` path is relative to the app bundle root. File must exist at startup. Missing file fails at config validation. If omitted, only the auto-generated tool catalog is served. |
| MCP-5 | `guard` is optional. If omitted, the MCP endpoint is unauthenticated. The guard follows the same contract as REST view guards. |

---

## 3. JSON-RPC Dispatch Layer

The MCP dispatcher lives inside `riversd` at the view layer — not in a CodeComponent. It receives the raw POST body, parses the JSON-RPC 2.0 envelope, and routes by method.

### 3.1 Method Routing

| JSON-RPC Method | Rivers Internal Action |
|---|---|
| `initialize` | Negotiate capabilities, create MCP session in StorageEngine, return server info + instructions |
| `ping` | Return empty result (session keepalive) |
| `tools/list` | Project exposed DataViews into MCP tool schema format |
| `tools/call` | Execute DataView via `dataview_engine.execute(name, params)` |
| `resources/list` | Project exposed read-only DataViews into MCP resource format |
| `resources/read` | Execute DataView (GET-only path) |
| `resources/templates/list` | List parameterized resource URIs |
| `prompts/list` | List declared prompt templates |
| `prompts/get` | Resolve prompt template with argument substitution |

### 3.2 Transport

MCP Streamable HTTP transport. Single endpoint, POST method.

- **Request:** `POST /mcp` with `Content-Type: application/json`, body is a JSON-RPC 2.0 message or batch.
- **Response (non-streaming):** `Content-Type: application/json`, body is a JSON-RPC 2.0 response.
- **Response (streaming):** `Content-Type: text/event-stream`, body is SSE with JSON-RPC response events.
- **Session header:** `Mcp-Session-Id` sent by server on `initialize`, echoed by client on subsequent requests.

### 3.3 Instructions Endpoint

```
GET /mcp/instructions
```

Returns the compiled instructions document as `text/markdown`. Same content the model receives during `initialize`. Available outside the JSON-RPC path for developer inspection, debugging, and integration with non-MCP AI systems.

Compiled once on app startup. Re-compiled on hot reload in dev mode.

### 3.4 Constraints

| ID | Rule |
|---|---|
| MCP-6 | The dispatcher MUST reject JSON-RPC messages with `jsonrpc` != `"2.0"` with error code `-32600` (Invalid Request). |
| MCP-7 | Unknown methods MUST return error code `-32601` (Method not found). |
| MCP-8 | Malformed JSON MUST return error code `-32700` (Parse error). |
| MCP-9 | Batch requests (JSON array of messages) MUST be supported. Responses are returned as a JSON array in the same order. |
| MCP-10 | Notifications (requests without `id`) MUST NOT produce a response. |

---

## 4. Tools

### 4.1 Input Schema

Each exposed tool's MCP `inputSchema` is provided via an explicit JSON Schema file declared on the tool:

```toml
[api.views.mcp.tools.get_orders]
dataview     = "get_orders"
description  = "Retrieve orders for a customer"
input_schema = "schemas/get_orders_input.json"   # path relative to app bundle root
```

The file must be a valid JSON Schema object. If omitted, the tool is exposed with an empty `inputSchema` (`{"type":"object","properties":{}}`).

**Note:** Automatic schema projection from DataView parameter declarations is not yet implemented. Until it is, every tool that needs a typed `inputSchema` must supply the file explicitly.

### 4.2 Tool Execution

`tools/call` request:
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "get_orders",
    "arguments": { "user_id": "42", "status": "active" }
  }
}
```

Execution path:
1. Lookup tool name in MCP tool registry → resolve to DataView name
2. Pass `arguments` as the DataView parameter map
3. DataView engine validates parameters against declarations
4. DataView engine routes each parameter to its declared `location` (path, query, body, header)
5. DataView executes against datasource
6. Result wrapped in JSON-RPC response

The MCP dispatcher calls the same `dataview_engine.execute()` path as `ctx.dataview()`. If the DataView has a handler chain, those handlers run normally. MCP is an entry point, not a bypass.

### 4.3 Full CRUD via Tools

When a DataView declares multiple methods (GET/POST/PUT/DELETE via `get_query`, `post_query`, `put_query`, `delete_query`), the MCP exposure config determines how these map to tools.

A single DataView with full CRUD can be exposed as:
- One tool per method (explicit, fine-grained control)
- A single tool where the method is inferred from the arguments (compact, but less discoverable)

The recommended pattern is one tool per method:

```toml
[api.views.mcp.tools.get_order]
dataview    = "orders"
method      = "GET"
description = "Retrieve an order by ID"
hints       = { read_only = true }

[api.views.mcp.tools.create_order]
dataview    = "orders"
method      = "POST"
description = "Create a new order"
hints       = { destructive = false, idempotent = false }

[api.views.mcp.tools.update_order]
dataview    = "orders"
method      = "PUT"
description = "Update an existing order"
hints       = { destructive = false, idempotent = true }

[api.views.mcp.tools.delete_order]
dataview    = "orders"
method      = "DELETE"
description = "Delete an order"
hints       = { destructive = true, idempotent = true }
```

When `method` is specified, only the parameters declared for that method (e.g., `get.parameters`, `post.parameters`) are projected into the tool's `inputSchema`. When `method` is omitted, all parameters across all methods are merged — this is valid only for single-method DataViews.

### 4.4 Annotations

MCP tool annotations provide hints to the model about tool behavior. Declared in the `hints` table:

| Hint | Type | Default | Meaning |
|---|---|---|---|
| `read_only` | bool | false | Tool does not modify state |
| `destructive` | bool | true | Tool may perform destructive operations |
| `idempotent` | bool | false | Safe to retry without side effects |
| `open_world` | bool | true | Tool interacts with external systems |

When `method` is declared, the framework can infer defaults:
- `method = "GET"` → `read_only = true`, `destructive = false`
- `method = "DELETE"` → `destructive = true`

Explicit hints override inferred defaults.

---

## 5. Resources

MCP resources are read-only named data the model can fetch. These map to DataViews exposed with the resource contract.

### 5.1 Resource Execution

`resources/read` always executes the DataView's GET path. Write methods (POST/PUT/DELETE) are not available through the resource interface — they require tools.

### 5.2 URI Scheme

Resources use a `rivers://` URI scheme:

```
rivers://{app_id}/{resource_name}
rivers://{app_id}/{resource_name}/{param_value}
```

For parameterized resources, the URI template is exposed via `resources/templates/list`:

```
rivers://{app_id}/inventory_status/{product_id}
```

### 5.3 MIME Types

Resource responses include a MIME type. Default is `application/json`. Override with `mime_type` in the resource config:

```toml
[api.views.mcp.resources.product_catalog]
dataview    = "get_products"
description = "Full product catalog"
mime_type   = "application/json"
```

---

## 6. Prompts

Server-provided workflow templates. The AI client discovers them via `prompts/list`, retrieves one via `prompts/get`, and uses the result as a structured instruction for accomplishing a task with the available tools.

### 6.1 Template Format

Prompt templates are markdown files with `{argument}` substitution placeholders. Same substitution syntax as DataView path and body templating.

```markdown
<!-- prompts/fulfill_order.md -->
# Order Fulfillment Workflow

Look up order **{order_id}** using `get_order`.

Verify inventory for each line item using `check_inventory`.

If all items are in stock, call `create_shipment` with priority **{priority}**.

If any items are out of stock:
1. Call `flag_backorder` for the out-of-stock items
2. Call `create_shipment` for the in-stock items as a partial shipment
3. Notify the customer using `send_notification` with the backorder details
```

### 6.2 Argument Substitution

At `prompts/get` time, the framework:
1. Reads the template file
2. Validates all required arguments are present in the request
3. Applies defaults for missing optional arguments
4. Substitutes `{argument}` placeholders with values
5. Returns the resolved markdown as the prompt content

Unrecognized placeholders (no matching argument declaration) fail at config validation, not at runtime.

### 6.3 Declaration

```toml
[api.views.mcp.prompts.fulfill_order]
description = "Complete the order fulfillment workflow"
template    = "prompts/fulfill_order.md"

[[api.views.mcp.prompts.fulfill_order.arguments]]
name        = "order_id"
description = "The order ID to fulfill"
required    = true

[[api.views.mcp.prompts.fulfill_order.arguments]]
name        = "priority"
description = "Fulfillment priority level"
required    = false
default     = "normal"
```

### 6.4 Constraints

| ID | Rule |
|---|---|
| MCP-11 | Template path is relative to app bundle root. Missing template file fails at config validation. |
| MCP-12 | Every `{placeholder}` in the template MUST have a matching argument declaration. Orphans fail at config validation. |
| MCP-13 | Every declared argument MUST appear as a `{placeholder}` in the template. Unused arguments fail at config validation with a warning (not an error). |

---

## 7. Instructions

### 7.1 Compilation

The instructions document is assembled from two sources:

**Source 1 — Static instructions file.** Developer-authored markdown with business rules, usage patterns, example workflows, domain context. Declared via `instructions = "docs/mcp-help.md"` in the MCP view config.

**Source 2 — Auto-generated tool catalog.** The framework walks every exposed tool, resource, and prompt, and renders a structured reference document including: tool name, description, parameter schemas (name, type, required, default), annotations/hints, resource URIs, and prompt names with argument lists.

### 7.2 Assembly Order

```
1. Static help.md content (verbatim, if declared)
2. ---
3. ## Tool Reference (auto-generated)
4.   - Tool entries with descriptions, parameters, hints
5. ## Resource Reference (auto-generated)
6.   - Resource entries with descriptions, URI templates
7. ## Prompt Reference (auto-generated)
8.   - Prompt entries with descriptions, arguments
```

Static file first — the developer's narrative context before the structured reference. If `instructions` is omitted, only the auto-generated sections are produced.

### 7.3 Delivery

- **MCP `initialize` response:** Instructions document included in the `instructions` field of the `ServerCapabilities` response.
- **HTTP GET endpoint:** `GET /mcp/instructions` returns the same compiled document as `text/markdown`.

Both paths read the same compiled output. Compiled once at app startup, re-compiled on hot reload in dev mode.

### 7.4 Constraints

| ID | Rule |
|---|---|
| MCP-14 | The auto-generated tool catalog MUST always be present, even if no static instructions file is declared. |
| MCP-15 | The `GET /mcp/instructions` endpoint MUST NOT require MCP session authentication. It follows the same guard as the MCP view. |
| MCP-16 | The compiled instructions MUST be regenerated on hot reload when any tool, resource, or prompt declaration changes. |

---

## 8. Streaming Tools

> **NOT YET IMPLEMENTED.** The dispatcher detects streaming DataViews but executes them synchronously. SSE response mode for MCP tools is planned. The spec below describes the intended wire format for reference.

### 8.1 Detection

When `tools/call` targets a DataView with `streaming = true`, the MCP dispatcher will automatically switch to SSE response mode. No special MCP-level configuration is required — the streaming property is on the DataView, not the tool exposure.

### 8.2 Wire Format

Streaming tool responses use the MCP Streamable HTTP SSE transport. Each chunk from the streaming DataView is wrapped in a JSON-RPC notification and sent as an SSE event:

```
data: {"jsonrpc":"2.0","method":"notifications/tools/progress","params":{"token":"...","data":{"token":"The"}}}\n\n
data: {"jsonrpc":"2.0","method":"notifications/tools/progress","params":{"token":"...","data":{"token":" answer"}}}\n\n
data: {"jsonrpc":"2.0","result":{"content":[{"type":"text","text":"complete"}]},"id":1}\n\n
```

The final event is the JSON-RPC response with the tool's result. Preceding events are progress notifications.

### 8.3 Error Mid-Stream

Same as Rivers streaming REST error handling: if the generator throws after the first yield, a poison chunk is emitted as the final SSE event, wrapped in a JSON-RPC error response:

```
data: {"jsonrpc":"2.0","error":{"code":-32000,"message":"upstream model API unavailable"},"id":1}\n\n
```

### 8.4 Constraints

| ID | Rule |
|---|---|
| MCP-17 | Streaming detection is automatic. If the target DataView has `streaming = true`, the response MUST use SSE transport. |
| MCP-18 | `stream_timeout_ms` from the DataView's streaming config applies. The MCP layer does not define its own timeout. |
| MCP-19 | Non-streaming DataViews MUST always return a single JSON-RPC response, never SSE. |

---

## 9. Session Management

### 9.1 Session Lifecycle

MCP sessions use the `Mcp-Session-Id` header for continuity.

1. Client sends `initialize` — no session header.
2. Server creates a session entry in StorageEngine with TTL, returns `Mcp-Session-Id` in response header.
3. Client echoes `Mcp-Session-Id` on all subsequent requests.
4. Server validates session on each request. Invalid/expired session returns JSON-RPC error `-32001`.
5. Session expires on TTL or explicit termination.

### 9.2 StorageEngine Key

```
mcp:session:{session_id}
```

Session data stored:
- `created_at` — ISO 8601 timestamp
- `capabilities` — negotiated capability set from `initialize`
- `identity` — guard-resolved identity (if guard is configured)

### 9.3 Configuration

```toml
[api.views.mcp.session]
ttl_seconds = 3600          # default: 3600 (1 hour)
```

### 9.4 Constraints

| ID | Rule |
|---|---|
| MCP-20 | `initialize` MUST create a new session. Re-initializing with an existing session ID replaces the session. |
| MCP-21 | Requests to any method other than `initialize` without a valid `Mcp-Session-Id` MUST return JSON-RPC error `-32001` with message "Session required". |
| MCP-22 | Session TTL is reset on every valid request (sliding expiration). |

---

## 10. Parameter Resolution

The MCP layer passes tool arguments as a flat JSON object to the DataView engine. The DataView engine routes each argument to its declared `location` — the MCP layer never interprets parameter locations.

### 10.1 Resolution Flow

```
MCP tools/call arguments:    { "user_id": "42", "status": "active" }
                                       │
                                       ▼
DataView parameter declarations:
  user_id  → location = "path"    → substituted into path template
  status   → location = "query"   → appended to query string
                                       │
                                       ▼
Resolved HTTP request:           GET /v1/users/42/orders?status=active
```

### 10.2 Type Coercion

MCP arguments arrive as JSON values. The DataView engine coerces them to match declared parameter types:

| Declared type | JSON input | Coercion |
|---|---|---|
| `string` | `"42"` | No coercion needed |
| `integer` | `42` | No coercion needed |
| `integer` | `"42"` | Parse string → integer |
| `uuid` | `"abc-123"` | Validated as UUID format |
| `boolean` | `true` | No coercion needed |
| `boolean` | `"true"` | Parse string → boolean |

Coercion failure returns JSON-RPC error `-32602` (Invalid params).

### 10.3 Constraints

| ID | Rule |
|---|---|
| MCP-23 | Arguments not matching any declared parameter MUST be rejected with `-32602`. |
| MCP-24 | Missing required parameters MUST be rejected with `-32602`. |
| MCP-25 | Default values from parameter declarations apply when an optional argument is absent. |

---

## 11. Error Handling

### 11.1 JSON-RPC Error Codes

| Code | Meaning | When |
|---|---|---|
| `-32700` | Parse error | Malformed JSON body |
| `-32600` | Invalid Request | Missing `jsonrpc: "2.0"`, missing `method` |
| `-32601` | Method not found | Unknown JSON-RPC method |
| `-32602` | Invalid params | Parameter validation failure, unknown tool/resource/prompt name |
| `-32001` | Session required | Missing or invalid `Mcp-Session-Id` |
| `-32000` | Server error | DataView execution failure, datasource errors |

### 11.2 DataView Error Mapping

DataView engine errors are mapped to JSON-RPC errors:

| DataView Error | JSON-RPC Code | Message |
|---|---|---|
| Parameter validation failure | `-32602` | Validation details |
| DataView not found | `-32602` | "Unknown tool: {name}" |
| Datasource connection failure | `-32000` | "Datasource unavailable" |
| Handler error (CodeComponent throw) | `-32000` | Handler error message |
| Timeout | `-32000` | "Execution timeout" |

### 11.3 Constraints

| ID | Rule |
|---|---|
| MCP-26 | DataView errors MUST NOT leak datasource connection details, credentials, or internal stack traces. Error messages are sanitized before inclusion in JSON-RPC responses. |
| MCP-27 | Guard authentication failures MUST return HTTP 401, not a JSON-RPC error. The guard runs before the JSON-RPC dispatcher. |

---

## 12. Validation Rules

Config validation at app startup (fail-fast):

| ID | Rule |
|---|---|
| VAL-1 | Every `tools.*.dataview` must reference a declared DataView. |
| VAL-2 | Every `resources.*.dataview` must reference a declared DataView. |
| VAL-3 | Every `tools.*.method` must match a method declared on the referenced DataView (GET/POST/PUT/DELETE). |
| VAL-4 | `instructions` file path must exist in the bundle. |
| VAL-5 | Every prompt `template` file path must exist in the bundle. |
| VAL-6 | Every `{placeholder}` in a prompt template must have a matching argument declaration. |
| VAL-7 | ~~Only one `view_type = "Mcp"` per application.~~ **Removed** — multiple MCP views per app are allowed (see MCP-2). |
| VAL-8 | Tool names must be unique within the MCP view. |
| VAL-9 | Resource names must be unique within the MCP view. |
| VAL-10 | Prompt names must be unique within the MCP view. |

---

## 13. Configuration Reference

### 13.1 MCP View

```toml
[api.views.mcp]
path         = "/mcp"              # REQUIRED — endpoint path
view_type    = "Mcp"               # REQUIRED — activates MCP dispatcher
guard        = "api_key_guard"     # optional — guard view name
instructions = "docs/mcp-help.md"  # optional — static instructions file

[api.views.mcp.session]
ttl_seconds  = 3600                # default: 3600
```

### 13.2 Tool

Tools have two mutually exclusive backends: DataView-backed or CodeComponent-backed.

**DataView-backed tool** (queries a datasource):

```toml
[api.views.mcp.tools.{tool_name}]
dataview     = "dataview_name"      # REQUIRED for DataView backend
description  = "..."                # REQUIRED — human-readable for AI model
method       = "GET"                # optional — restrict to one HTTP method
input_schema = "schemas/tool.json"  # optional — JSON Schema file for inputSchema
hints        = { ... }              # optional — MCP annotations
```

**CodeComponent-backed tool** (runs a handler view):

```toml
[api.views.mcp.tools.{tool_name}]
view         = "handler_view_name"  # REQUIRED for CodeComponent backend — references a Codecomponent view in the same app
description  = "..."                # REQUIRED
input_schema = "schemas/tool.json"  # optional — JSON Schema file for inputSchema
hints        = { ... }              # optional — MCP annotations
```

`view` and `dataview` are mutually exclusive — set exactly one. When `view` is set, the tool call dispatches through ProcessPool to the referenced handler view, receiving the tool `arguments` as the request body.

### 13.3 Resource

```toml
[api.views.mcp.resources.{resource_name}]
dataview    = "dataview_name"      # REQUIRED — DataView reference
description = "..."                # REQUIRED — human-readable for AI model
mime_type   = "application/json"   # optional — default: application/json
```

### 13.4 Prompt

```toml
[api.views.mcp.prompts.{prompt_name}]
description = "..."                # REQUIRED — human-readable for AI model
template    = "prompts/file.md"    # REQUIRED — template file path

[[api.views.mcp.prompts.{prompt_name}.arguments]]
name        = "arg_name"           # REQUIRED
description = "..."                # optional — shown to AI model
required    = true                 # default: false
default     = "value"              # optional — used when argument absent
```

---

## 14. Examples

### 14.1 Minimal MCP — Existing REST App

Add MCP to an existing app with three DataViews. No CodeComponent changes needed.

```toml
# Existing DataViews (unchanged)
[data.dataviews.get_contacts]
datasource = "primary_db"
query      = "SELECT id, name, email FROM contacts WHERE org_id = $org_id"
[[data.dataviews.get_contacts.parameters]]
name     = "org_id"
type     = "uuid"
required = true

[data.dataviews.create_contact]
datasource = "primary_db"
post_query = "INSERT INTO contacts (name, email, org_id) VALUES ($name, $email, $org_id) RETURNING *"
[[data.dataviews.create_contact.parameters]]
name = "name"
type = "string"
required = true
[[data.dataviews.create_contact.parameters]]
name = "email"
type = "string"
required = true
[[data.dataviews.create_contact.parameters]]
name = "org_id"
type = "uuid"
required = true

[data.dataviews.search_contacts]
datasource = "primary_db"
query      = "SELECT * FROM contacts WHERE name ILIKE '%' || $q || '%'"
[[data.dataviews.search_contacts.parameters]]
name = "q"
type = "string"
required = true

# MCP exposure — this is the only addition
[api.views.mcp]
path      = "/mcp"
view_type = "Mcp"
guard     = "api_key_guard"

[api.views.mcp.tools.get_contacts]
dataview    = "get_contacts"
description = "List all contacts for an organization"
hints       = { read_only = true }

[api.views.mcp.tools.create_contact]
dataview    = "create_contact"
description = "Create a new contact with name and email"
hints       = { destructive = false, idempotent = false }

[api.views.mcp.tools.search_contacts]
dataview    = "search_contacts"
description = "Search contacts by name (partial match)"
hints       = { read_only = true }
```

The AI model calls `tools/call` with `{"name": "search_contacts", "arguments": {"q": "smith"}}`. The MCP dispatcher passes `{"q": "smith"}` to the DataView engine. The engine binds `$q` in the SQL query. Results return through the JSON-RPC response. Same execution path as `GET /api/contacts?q=smith`.

### 14.2 Full-Featured MCP — Order Management

```toml
[api.views.mcp]
path         = "/mcp"
view_type    = "Mcp"
guard        = "api_key_guard"
instructions = "docs/order-api-help.md"

# ── Tools ──

[api.views.mcp.tools.get_order]
dataview    = "orders"
method      = "GET"
description = "Retrieve an order by ID, returns order details with line items"
hints       = { read_only = true }

[api.views.mcp.tools.create_order]
dataview    = "orders"
method      = "POST"
description = "Create a new order with customer ID, line items, and shipping address"
hints       = { destructive = false, idempotent = false }

[api.views.mcp.tools.update_order]
dataview    = "orders"
method      = "PUT"
description = "Update order status or shipping details"
hints       = { destructive = false, idempotent = true }

[api.views.mcp.tools.cancel_order]
dataview    = "orders"
method      = "DELETE"
description = "Cancel an order — only pending orders can be cancelled"
hints       = { destructive = true, idempotent = true }

[api.views.mcp.tools.check_inventory]
dataview    = "inventory_check"
description = "Check stock levels for a product by SKU"
hints       = { read_only = true }

# ── Resources ──

[api.views.mcp.resources.product_catalog]
dataview    = "get_products"
description = "Full product catalog with pricing and availability"

[api.views.mcp.resources.shipping_zones]
dataview    = "get_shipping_zones"
description = "Available shipping zones and delivery estimates"

# ── Prompts ──

[api.views.mcp.prompts.fulfill_order]
description = "Step-by-step order fulfillment workflow"
template    = "prompts/fulfill_order.md"

[[api.views.mcp.prompts.fulfill_order.arguments]]
name        = "order_id"
description = "The order to fulfill"
required    = true

[[api.views.mcp.prompts.fulfill_order.arguments]]
name        = "priority"
description = "Fulfillment priority: normal, rush, overnight"
required    = false
default     = "normal"

[api.views.mcp.session]
ttl_seconds = 1800
```

### 14.3 Streaming MCP — LLM Proxy

```toml
[data.datasources.anthropic]
driver      = "http"
base_url    = "https://api.anthropic.com"
protocol    = "http2"
auth        = "bearer"
credentials = "lockbox://anthropic/api_key"

[data.dataviews.generate]
datasource       = "anthropic"
method           = "POST"
path             = "/v1/messages"
streaming        = true
streaming_format = "sse"
stream_timeout_ms = 120000

[data.dataviews.generate.body_template]
model      = "{model}"
max_tokens = 1024
stream     = true
messages   = "{messages}"

[[data.dataviews.generate.parameters]]
name     = "model"
location = "body"
required = false
default  = "claude-sonnet-4-20250514"

[[data.dataviews.generate.parameters]]
name     = "messages"
location = "body"
required = true

# MCP exposure — streaming tool
[api.views.mcp]
path      = "/mcp"
view_type = "Mcp"
guard     = "api_key_guard"

[api.views.mcp.tools.generate]
dataview    = "generate"
description = "Generate text using Claude — returns a streaming response of tokens"
hints       = { read_only = true, open_world = true }
```

The model calls `tools/call` with `{"name": "generate", "arguments": {"messages": [...]}}`. The MCP dispatcher detects `streaming = true` on the DataView and switches to SSE response mode automatically. Each chunk from the upstream Anthropic API is forwarded as an MCP progress notification.

### 14.4 HTTP DataView with Query Parameters

```toml
[data.datasources.internal_api]
driver   = "http"
base_url = "https://api.internal.example.com"
auth     = "bearer"
credentials = "lockbox://internal/token"

[data.dataviews.get_orders]
datasource = "internal_api"
method     = "GET"
path       = "/v1/users/{user_id}/orders"

[[data.dataviews.get_orders.parameters]]
name     = "user_id"
location = "path"
required = true

[[data.dataviews.get_orders.parameters]]
name     = "status"
location = "query"
required = false
default  = "active"

[[data.dataviews.get_orders.parameters]]
name     = "limit"
location = "query"
required = false
default  = "50"

# MCP tool
[api.views.mcp.tools.get_orders]
dataview    = "get_orders"
description = "Get orders for a user, optionally filtered by status"
hints       = { read_only = true }
```

The model sends: `{"user_id": "42", "status": "shipped", "limit": "10"}`. The DataView engine routes `user_id` to the path, `status` and `limit` to the query string. The model never knows the difference — it sees a flat input schema.

---

## Shaping Amendments

None. Initial specification.

---

## CHANGELOG

| Date | Change |
|---|---|
| 2026-04-15 | Initial specification — MCP View Type for Rivers v1 |
