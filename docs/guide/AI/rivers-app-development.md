# Rivers Application Development — Build Spec

**Rivers v0.53.0**

## Getting Started (v0.53.0)

The recommended way to create a new bundle is `riverpackage init`:

```bash
riverpackage init my-bundle/
```

This scaffolds the full bundle directory structure with template manifests, empty schema directories, and placeholder config files. You can then edit the generated TOML files to define your app.

---

## What You Are Building

Rivers application bundles containing one or more apps deployed to a Rivers V1 single-node environment.

```
{bundle-name}/
├── manifest.toml              ← bundle manifest
├── CHANGELOG.md               ← decisions log (create during build)
├── {app-service}/
│   ├── manifest.toml          ← app manifest (type = "app-service")
│   ├── resources.toml         ← datasources, keystores, services
│   ├── schemas/               ← JSON schema files
│   ├── app.toml               ← datasources, dataviews, views config
│   └── libraries/             ← handlers, shared code
└── {app-main}/
    ├── manifest.toml          ← app manifest (type = "app-main")
    ├── resources.toml
    ├── app.toml
    └── libraries/
        └── spa/               ← SPA assets (if applicable)
```

---

## App Types

| Type | Purpose | Starts |
|------|---------|--------|
| `app-service` | Backend API, no frontend, owns datasources | First |
| `app-main` | Frontend SPA + API gateway, proxies to app-services | After services healthy |

MUST: Bundle `apps` array lists services before mains.
MUST: `app-main` waits for `app-service` health checks before starting.
MUST NOT: Declare `spa` config on `app-service`.

---

## Bundle Manifest

File: `{bundle}/manifest.toml`

```toml
bundleName    = "{name}"
bundleVersion = "1.0.0"
source        = "{repo-url}"
apps          = ["{app-service-1}", "{app-service-2}", "{app-main}"]
```

MUST: `apps` order — services before mains. Rivers starts in this order.

---

## App Manifest

File: `{app}/manifest.toml`

### app-service

```toml
appName       = "{name}"
description   = "{description}"
version       = "1.0.0"
type          = "app-service"
appId         = "{uuid}"
entryPoint    = "{slug}"
appEntryPoint = "https://{internal-hostname}"
source        = "{repo-url}"
```

### app-main

```toml
appName       = "{name}"
description   = "{description}"
version       = "1.0.0"
type          = "app-main"
appId         = "{uuid}"
entryPoint    = "{slug}"
appEntryPoint = "https://{external-hostname}"
source        = "{repo-url}"
```

### Fields

| Field | Required | Owner | Notes |
|-------|----------|-------|-------|
| `appName` | yes | Developer | Human-readable name |
| `version` | yes | Developer | Semantic version |
| `type` | yes | Developer | `app-service` or `app-main` |
| `appId` | yes | Build tool | Stable UUID. NEVER regenerate. |
| `entryPoint` | yes | Developer | URL path slug — used as route namespace (e.g. `"service"`, `"main"`) |
| `appEntryPoint` | no | Developer | Public URL (informational) |
| `source` | yes | Build tool | Stamped at package time |

MUST: `appId` is permanent identity across redeployments. Never change it.
MUST NOT: Regenerate `appId` — it is used to match datasources and service references.

---

## Resources Declaration

File: `{app}/resources.toml`

### Datasources

```toml
[[datasources]]
name       = "{logical-name}"
driver     = "{driver-type}"
x-type     = "{driver-type}"
lockbox    = "{alias}"
required   = true

[[datasources]]
name       = "{name}"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true
```

| Field | Required | Notes |
|-------|----------|-------|
| `name` | yes | Logical name — used in app.toml |
| `driver` | yes | Driver type (postgres, redis, faker, http, etc.) |
| `x-type` | yes | Build-time contract — same as driver for built-ins |
| `lockbox` | conditional | Lockbox alias — required unless `nopassword = true` |
| `nopassword` | no | Set `true` for credential-free drivers (faker, sqlite) |
| `required` | yes | If true, startup fails when unavailable |

MUST: Use `nopassword = true` for faker, sqlite — omit `lockbox` entirely.
MUST NOT: Include `lockbox` when `nopassword = true`.

### Keystores (Application Encryption Keys)

```toml
[[keystores]]
name     = "app-keys"
lockbox  = "myapp/keystore-master-key"
required = true
```

| Field | Required | Notes |
|-------|----------|-------|
| `name` | yes | Logical name — referenced in `[data.keystore.*]` in app.toml |
| `lockbox` | yes | Lockbox alias for the Age identity that unlocks this keystore |
| `required` | yes | If true, startup fails when keystore cannot be unlocked |

Keystore file path is configured in `app.toml`:

```toml
[data.keystore.app-keys]
path = "data/app.keystore"
```

Keys are managed via `rivers-keystore` CLI. Handlers use `Rivers.crypto.encrypt/decrypt` to encrypt/decrypt data, and `Rivers.keystore.has/info` for key metadata. Key bytes never leave Rust memory.

### Services (app-main only)

```toml
[[services]]
name     = "{service-name}"
appId    = "{app-service-uuid}"
required = true
```

MUST: `appId` matches the app-service's `manifest.toml` appId exactly.

### Supported Drivers

| Driver | x-type | Credentials | Use Case |
|--------|--------|-------------|----------|
| `faker` | `faker` | `nopassword = true` | Mock data, dev/demo |
| `postgres` | `postgres` | lockbox | Relational data |
| `mysql` | `mysql` | lockbox | Relational data |
| `sqlite` | `sqlite` | `nopassword = true` | Embedded relational |
| `redis` | `redis` | lockbox | Cache, sessions, KV, streams |
| `http` | `http` | optional | External API proxy |
| `mongodb` | `mongodb` | lockbox | Document store |
| `elasticsearch` | `elasticsearch` | lockbox | Search |
| `cassandra` | `cassandra` | lockbox | Wide-column store |
| `couchdb` | `couchdb` | lockbox | Document store |
| `influxdb` | `influxdb` | lockbox | Time series |
| `ldap` | `ldap` | lockbox | Directory |
| `rivers-exec` | `exec` | `nopassword = true` | Script execution |
| `kafka` | `kafka` | lockbox | Message streaming |
| `rabbitmq` | `rabbitmq` | lockbox | Message queuing |
| `nats` | `nats` | lockbox | Message pub/sub |
| `redis-streams` | `redis-streams` | lockbox | Stream processing |

---

## Schema Files

Location: `{app}/schemas/{name}.schema.json`

```json
{
  "type": "object",
  "description": "{description}",
  "fields": [
    { "name": "id",         "type": "uuid",     "required": true  },
    { "name": "email",      "type": "email",    "required": true  },
    { "name": "created_at", "type": "datetime", "required": true  },
    { "name": "phone",      "type": "phone",    "required": false }
  ]
}
```

### Rivers Primitive Types

| Type | Description |
|------|-------------|
| `uuid` | UUID v4 string |
| `string` | UTF-8 string |
| `integer` | 64-bit signed integer |
| `float` | 64-bit float |
| `boolean` | true/false |
| `email` | String validated as email |
| `phone` | String validated as phone number |
| `datetime` | ISO 8601 datetime string |
| `date` | ISO 8601 date string |
| `url` | String validated as URL |
| `json` | Arbitrary JSON value |

### Driver-Specific Attributes

| Attribute | Supported Drivers | Description |
|-----------|-------------------|-------------|
| `faker` | `faker` only | Faker.js dot-notation: `"name.firstName"` |
| `min` | `postgres`, `mysql` | Minimum numeric value |
| `max` | `postgres`, `mysql` | Maximum numeric value |
| `pattern` | `postgres`, `mysql`, `ldap` | Regex pattern |

MUST NOT: Use `faker` attribute with non-faker drivers — validation error.

### Faker Attribute Examples

```json
{ "name": "first_name", "type": "string",   "faker": "name.firstName",         "required": true  }
{ "name": "email",      "type": "email",    "faker": "internet.email",         "required": true  }
{ "name": "phone",      "type": "phone",    "faker": "phone.number",           "required": false }
{ "name": "company",    "type": "string",   "faker": "company.name",           "required": false }
{ "name": "city",       "type": "string",   "faker": "location.city",          "required": false }
{ "name": "avatar_url", "type": "string",   "faker": "image.avatar",           "required": false }
{ "name": "created_at", "type": "datetime", "faker": "date.past",              "required": true  }
{ "name": "id",         "type": "uuid",     "faker": "datatype.uuid",          "required": true  }
```

Common faker categories: `name`, `internet`, `phone`, `location`, `company`, `datatype`, `date`, `image`, `lorem`.

---

## Datasource Configuration

File: `{app}/app.toml` — `[data.datasources.{name}]` section

### Faker Datasource

```toml
[data.datasources.contacts]
driver     = "faker"
nopassword = true

[data.datasources.contacts.config]
locale                = "en_US"
seed                  = 42
max_records_per_query = 500
```

`seed` — optional. When set, faker produces the same records on every query.

### PostgreSQL Datasource

```toml
[data.datasources.orders_db]
driver             = "postgres"
host               = "${DB_HOST}"
port               = 5432
database           = "orders"
credentials_source = "lockbox://db/orders"

[data.datasources.orders_db.config]
ssl_mode          = "prefer"
statement_timeout = 30000

[data.datasources.orders_db.connection_pool]
min_idle           = 2
max_size           = 20
connection_timeout = 5000
```

### HTTP Datasource (for app-main proxying to app-service)

```toml
[data.datasources.{name}]
driver  = "http"
service = "{service-name}"
```

`service` — logical service name from `resources.toml [[services]]`.

### ExecDriver Datasource (Script Execution)

For executing admin-declared scripts and binaries:

```toml
# resources.toml
[[datasources]]
name       = "ops_tools"
driver     = "rivers-exec"
x-type     = "exec"
nopassword = true
required   = true

# app.toml
[data.datasources.ops_tools]
name     = "ops_tools"
driver   = "rivers-exec"
run_as_user       = "rivers-exec"
working_directory = "/var/rivers/exec-scratch"
max_concurrent    = 10

[data.datasources.ops_tools.commands.network_scan]
path       = "/usr/lib/rivers/scripts/netscan.py"
sha256     = "a1b2c3d4e5f67890..."
input_mode = "stdin"
timeout_ms = 60000
```

Commands are pinned by SHA-256 hash. Input is validated against JSON Schema. Processes run as a restricted OS user.

---

## DataView Configuration

File: `{app}/app.toml` — `[data.dataviews.{name}]` section

```toml
[data.dataviews.list_contacts]
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[data.dataviews.list_contacts.caching]
ttl_seconds = 60

[[data.dataviews.list_contacts.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[[data.dataviews.list_contacts.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0
```

### DataView Cache Invalidation

DataViews that mutate data can declare which cached DataViews to invalidate on success:

```toml
[data.dataviews.create_contact]
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"
invalidates   = ["list_contacts", "search_contacts"]  # cache entries cleared on success
```

When the `create_contact` DataView executes successfully, the cache entries for `list_contacts` and `search_contacts` are evicted. Each target in `invalidates` must reference a valid DataView name — validation fails otherwise.

### DataView with Path Parameter

```toml
[data.dataviews.get_contact]
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[[data.dataviews.get_contact.parameters]]
name     = "id"
type     = "uuid"
required = true
```

### HTTP Proxy DataView (app-main)

```toml
[data.dataviews.proxy_list_contacts]
datasource = "address-book-api"
query      = "/contacts"
method     = "GET"

[[data.dataviews.proxy_list_contacts.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20
```

---

## View Configuration

File: `{app}/app.toml` — `[api.views.{name}]` section

### REST View with DataView Handler

```toml
[api.views.list_contacts]
path            = "/api/contacts"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_contacts.handler]
type     = "dataview"
dataview = "list_contacts"

[api.views.list_contacts.parameter_mapping.query]
limit  = "limit"
offset = "offset"
```

### REST View with Path Parameters

```toml
[api.views.get_contact]
path            = "/api/contacts/{id}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_contact.handler]
type     = "dataview"
dataview = "get_contact"

[api.views.get_contact.parameter_mapping.path]
id = "id"
```

### REST View with CodeComponent Handler

```toml
[api.views.create_order]
path      = "/api/orders"
method    = "POST"
view_type = "Rest"

[api.views.create_order.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/orders.ts"
entrypoint = "createOrder"
resources  = ["orders_db", "events_queue"]
```

MUST: `resources` lists all datasources the component may access.
MUST NOT: Access undeclared datasources — results in `CapabilityError`.

### View Types

| Type | Transport | HTTP Method |
|------|-----------|-------------|
| `Rest` | HTTP request/response | Any |
| `Websocket` | WS upgrade, bidirectional | GET only |
| `ServerSentEvents` | HTTP long-lived, server→client | GET only |
| `MessageConsumer` | EventBus event | No HTTP route |

### Parameter Mapping

```toml
[api.views.{name}.parameter_mapping.query]
{query-param} = "{dataview-param}"

[api.views.{name}.parameter_mapping.path]
{path-param} = "{dataview-param}"
```

### Streaming REST Views

REST views can stream responses by enabling the `streaming` flag. This is useful for large result sets or real-time data feeds over a single HTTP request.

```toml
[api.views.export_contacts]
path             = "/api/contacts/export"
method           = "GET"
view_type        = "Rest"
streaming        = true
streaming_format = "ndjson"       # "ndjson" or "sse"
stream_timeout_ms = 30000

[api.views.export_contacts.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/export.ts"
entrypoint = "exportContacts"
resources  = ["contacts"]
```

| Field | Required | Notes |
|-------|----------|-------|
| `streaming` | no | Enable chunked streaming response (default `false`) |
| `streaming_format` | conditional | Required when `streaming = true`. `"ndjson"` (newline-delimited JSON) or `"sse"` (Server-Sent Events format) |
| `stream_timeout_ms` | no | Maximum time the stream stays open (ms) |

The handler returns objects following the `{chunk, done}` protocol: each yielded value is a `{ chunk: <data> }` object, and the final value is `{ done: true }` to signal end-of-stream.

### SSE Views

Server-Sent Events views maintain a long-lived HTTP connection for server-to-client push.

```toml
[api.views.dashboard_events]
path                  = "/api/events/dashboard"
method                = "GET"
view_type             = "ServerSentEvents"
auth                  = "session"
sse_event_buffer_size = 100       # default 100, events buffered for replay

[api.views.dashboard_events.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/dashboard.ts"
entrypoint = "streamEvents"
resources  = ["events_db"]
```

| Field | Required | Notes |
|-------|----------|-------|
| `sse_event_buffer_size` | no | Number of events buffered for reconnection replay (default 100) |

Rivers supports `Last-Event-ID` reconnection: when a client reconnects with the `Last-Event-ID` header, buffered events after that ID are replayed automatically.

### WebSocket Hooks

WebSocket views can define lifecycle hooks for connection, message, and disconnection events.

```toml
[api.views.chat]
path      = "/ws/chat"
method    = "GET"
view_type = "Websocket"
auth      = "session"

[api.views.chat.ws_hooks]
on_connect.module      = "handlers/chat.js"
on_connect.entrypoint  = "onConnect"
on_message.module      = "handlers/chat.js"
on_message.entrypoint  = "onMessage"
on_disconnect.module   = "handlers/chat.js"
on_disconnect.entrypoint = "onDisconnect"
```

| Hook | Trigger | Notes |
|------|---------|-------|
| `on_connect` | Client completes WS upgrade | Return `false` to reject |
| `on_message` | Each inbound WS frame | Receives parsed message |
| `on_disconnect` | Client or server closes | Cleanup / broadcast leave |

### Polling Config

SSE and WebSocket views can use a `[polling]` section to control how Rivers polls the underlying datasource for changes to push to clients.

```toml
[api.views.dashboard]
path      = "/api/events/dashboard"
method    = "GET"
view_type = "ServerSentEvents"

[api.views.dashboard.handler]
type     = "dataview"
dataview = "dashboard_metrics"

[api.views.dashboard.polling]
tick_interval_ms = 3000
diff_strategy    = "hash"          # hash | null | change_detect
poll_state_ttl_s = 300
```

| Field | Required | Default | Notes |
|-------|----------|---------|-------|
| `tick_interval_ms` | no | 5000 | How often to poll the datasource (ms) |
| `diff_strategy` | no | `"hash"` | `"hash"` — push only when content hash changes; `"null"` — always push; `"change_detect"` — driver-level change detection |
| `poll_state_ttl_s` | no | 300 | How long to keep per-client poll state (seconds) |

### Auth & Guard Views

Views support `auth` and `guard` fields to control authentication requirements.

```toml
# Public endpoint — no authentication required
[api.views.public_health]
path      = "/api/health"
method    = "GET"
view_type = "Rest"
auth      = "none"

# Session-protected endpoint
[api.views.user_profile]
path      = "/api/profile"
method    = "GET"
view_type = "Rest"
auth      = "session"

# Login endpoint — guard view (processes credentials, issues session)
[api.views.login]
path      = "/api/login"
method    = "POST"
view_type = "Rest"
auth      = "none"
guard     = true

[api.views.login.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/auth.ts"
entrypoint = "login"
resources  = ["users_db"]
```

| Field | Values | Notes |
|-------|--------|-------|
| `auth` | `"none"`, `"session"` | `"none"` — no authentication check; `"session"` — requires valid session token |
| `guard` | `true`, `false` | When `true`, marks this view as a login/auth guard endpoint that creates sessions |

---

## SPA Configuration

File: `{app-main}/app.toml` — `[spa]` section

```toml
[spa]
enabled      = true
root_path    = "libraries/spa"
index_file   = "index.html"
spa_fallback = true
max_age      = 3600
```

| Field | Required | Notes |
|-------|----------|-------|
| `enabled` | yes | Enable SPA serving |
| `root_path` | yes | Path relative to app directory |
| `index_file` | yes | Entry point HTML file |
| `spa_fallback` | yes | All non-API routes serve index.html |
| `max_age` | no | Cache-Control max-age seconds |

MUST: `spa_fallback = true` — non-API routes serve index.html.
MUST: `/api/*` routes always take precedence over SPA.
MUST NOT: Declare `[spa]` on `app-service`.

---

## GraphQL Configuration

File: `{app}/app.toml` — `[graphql]` section

```toml
[graphql]
enabled        = true
path           = "/graphql"
introspection  = true
max_depth      = 10
max_complexity = 1000
```

| Field | Required | Default | Notes |
|-------|----------|---------|-------|
| `enabled` | yes | `false` | Enable GraphQL endpoint |
| `path` | no | `"/graphql"` | URL path for the GraphQL endpoint |
| `introspection` | no | `true` | Allow schema introspection queries |
| `max_depth` | no | 10 | Maximum query nesting depth |
| `max_complexity` | no | 1000 | Maximum query complexity score |

GraphQL queries auto-resolve from DataViews: each DataView with a `return_schema` is exposed as a queryable type. Mutations are generated from CodeComponent-backed POST/PUT/DELETE views. Subscriptions are powered by SSE trigger events via the EventBus — any `ServerSentEvents` view is available as a GraphQL subscription.

---

## Response Format

Rivers REST views return envelope format by default:

```json
{
  "data": [...],
  "meta": {
    "count": 20,
    "total": 500,
    "limit": 20,
    "offset": 0
  }
}
```

---

## CodeComponent Handler Signature

### JavaScript

```javascript
function handler(ctx) {
    // Request data
    var method = ctx.request.method;
    var body = ctx.request.body;
    var id = ctx.request.path_params.id;
    var q = ctx.request.query;

    // Pre-fetched DataView results (keyed by DataView name)
    var users = ctx.data.list_users;

    // Call DataView dynamically
    var orders = ctx.dataview("get_orders", { user_id: id });

    // Application KV store (TTL in ms)
    ctx.store.set("key", { data: 42 }, 60000);
    var cached = ctx.store.get("key");
    ctx.store.del("key");

    // WebSocket context (only in ws_hooks handlers)
    // ctx.ws.connection_id, ctx.ws.message

    // Globals — structured logging
    // When app_log_dir is configured, output goes to log/apps/<app-name>.log (v0.53.0)
    Rivers.log.info("message", { key: "value" });
    Rivers.crypto.randomHex(16);
    Rivers.crypto.hashPassword("secret");
    Rivers.crypto.verifyPassword("secret", hash);

    // Set response
    ctx.resdata = { users, orders };
}
```

### Context Properties

| Property | Type | Description |
|----------|------|-------------|
| `ctx.request` | Object | `{method, path, headers, query, body, path_params}` |
| `ctx.resdata` | Any | Set this to return response data |
| `ctx.data` | Object | Pre-fetched DataView results keyed by name |
| `ctx.store` | Object | KV store: `get(key)`, `set(key, value, ttl_ms)`, `del(key)` |
| `ctx.dataview()` | Function | Call DataView dynamically: `ctx.dataview(name, params)` |
| `ctx.ws` | Object | WebSocket: `connection_id`, `message` (only in ws_hooks) |
| `ctx.trace_id` | String | Request trace ID |
| `ctx.app_id` | String | Application ID |
| `ctx.node_id` | String | Node identifier |
| `ctx.env` | String | Runtime environment |

### Supported Languages

| Language | Aliases | Runtime |
|----------|---------|---------|
| JavaScript | `javascript`, `js`, `js_v8` | V8 |
| TypeScript | `typescript`, `ts`, `ts_v8`, `typescript_strict` | V8 |
| WASM | `wasm` | Wasmtime |

---

## CHANGELOG.md

Create at bundle root. Append during build:

```markdown
## [Decision|Gap|Ambiguity|Error] — {title}
**File:** {filename}
**Description:** {what happened}
**Spec reference:** {section}
**Resolution:** {how resolved or "UNRESOLVED"}
```

MUST: Create CHANGELOG.md at bundle root.
MUST: Append decisions, gaps, and ambiguities — never replace.

---

## New Config Fields (v0.53.0)

### Per-App Logging

```toml
[base.logging]
level           = "info"
format          = "json"
local_file_path = "log/riversd.log"
app_log_dir     = "log/apps"            # Per-app log directory
```

When `app_log_dir` is set, each app's `Rivers.log.*` calls write to `log/apps/<app-name>.log` instead of the main server log. This is useful for debugging individual apps without filtering.

### Metrics

```toml
[metrics]
enabled  = true
endpoint = "/metrics"
```

Exposes a Prometheus-compatible metrics endpoint on the main server port.

### Engines

```toml
[engines]
v8_path   = "lib/librivers_engine_v8.dylib"
wasm_path = "lib/librivers_engine_wasm.dylib"
```

Explicit paths to engine dylibs for dynamic build mode. The correct filename pattern is `librivers_engine_v8.dylib` (not `librivers_v8.dylib`).

### Plugins

```toml
[plugins]
directory = "plugins/"
```

Directory where Rivers searches for plugin dylibs (`librivers_plugin_*.dylib`).

---

## Verification

```bash
# Scaffold a new bundle (recommended starting point)
riverpackage init {bundle-name}/

# Validate bundle
riversctl validate {bundle-path}/

# Build SPA (if applicable)
cd {app-main}/libraries && npm install && npm run build

# Start server (config points to bundle)
riversd --config riversd.toml

# Test endpoints
curl "http://localhost:{port}/api/{endpoint}"
curl "http://localhost:{port}/api/{endpoint}?limit=10"

# SPA
open http://localhost:{port}
```

---

## Validation Rules

| Rule | Error |
|------|-------|
| `type` not `app-main` or `app-service` | `invalid app type` |
| `appId` missing or not UUID | `appId is required and must be a UUID` |
| Two apps share `appId` | `duplicate appId` |
| `entryPoint` port already bound | `port is already bound` |
| Required datasource lockbox alias not found | `lockbox alias not found` |
| Required service appId not deployed | `service is not running` |
| `spa` declared on `app-service` | `spa config is only valid on app-main` |
| Module path not found in bundle | `module not found` |
| Resource in view references undeclared datasource | `unknown datasource` |
| DataView handler on non-REST view | `dataview is only supported for view_type=rest` |
| WebSocket view method != GET | `method must be GET when view_type=websocket` |
| Faker attribute on non-faker driver | `attribute not supported by driver` |
| `invalidates` target doesn't exist | `invalidates target 'X' does not exist` |
| Unknown `view_type` value | `unknown view_type 'X'` |
| Unknown driver | warning: `unknown driver 'X'` |
| Duplicate datasource names | `duplicate datasource name 'X'` |
| Schema file not found | `schema file 'X' not found` |
| Unknown service appId | `service references unknown appId` |

---

## Complete Example: Address Book Bundle

See `address-book-bundle/` for a working reference implementation demonstrating:
- Bundle with app-service + app-main
- Faker datasource with schema
- DataViews with caching and parameters
- REST views with parameter mapping
- HTTP proxy datasource for service-to-service calls
- SPA configuration with Svelte
