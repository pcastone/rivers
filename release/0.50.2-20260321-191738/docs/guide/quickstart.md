# Quick Start

Get a Rivers app running in under five minutes. No database required -- the bundled address-book example uses synthetic data.

---

## 1. Prerequisites

- **Rust toolchain** -- install via [rustup](https://rustup.rs/) (edition 2021, stable channel)
- **Git**

Clone the repository:

```bash
git clone https://github.com/acme/rivers.git
cd rivers
```

---

## 2. Build

```bash
cargo build --release
```

This produces three binaries in `target/release/`:

| Binary | Purpose |
|--------|---------|
| `riversd` | Application server |
| `riversctl` | CLI management tool |
| `riverpackage` | Bundle packaging tool |

For development, use debug builds (`cargo build`) -- faster compilation, slower runtime.

---

## 3. Run the Address Book Example

The repository ships with a ready-to-run bundle in `address-book-bundle/`. The server config is at `config/riversd.toml`:

```toml
bundle_path = "address-book-bundle/"

[base]
host      = "0.0.0.0"
port      = 8080
log_level = "info"

[base.tls]
redirect = false
```

TLS is mandatory. On first startup, `riversd` auto-generates a self-signed certificate into `data/tls/`. No manual cert setup needed for development.

Start the server from the project root:

```bash
./target/release/riversd --config config/riversd.toml
```

You should see startup logs confirming the bundle loaded and the server is listening on `https://0.0.0.0:8080`.

---

## 4. Test the API

The address-book-service exposes four endpoints. Routes follow the pattern `/<bundleName>/<entryPoint>/<view_path>`. The bundle is named `address-book` and the service entry point is `service`, so the base path is `/address-book/service/`.

All examples use `-k` to accept the self-signed certificate.

**List contacts** (paginated):

```bash
curl -k https://localhost:8080/address-book/service/contacts?limit=5&offset=0
```

**Get a single contact** by ID:

```bash
curl -k https://localhost:8080/address-book/service/contacts/{id}
```

Replace `{id}` with a UUID from the list response.

**Search contacts** by query string:

```bash
curl -k https://localhost:8080/address-book/service/contacts/search?q=john&limit=10
```

**Contacts by city**:

```bash
curl -k https://localhost:8080/address-book/service/contacts/city/Portland?limit=5
```

All responses use the envelope format:

```json
{
  "data": [ ... ],
  "meta": { "limit": 5, "offset": 0 }
}
```

---

## 5. Explore the Bundle

A bundle is a directory containing a `manifest.toml` and one or more app directories:

```
address-book-bundle/
  manifest.toml                        # Bundle metadata
  address-book-service/                # REST API app
    manifest.toml                      # App identity
    resources.toml                     # Datasources
    app.toml                           # DataViews + Views
    schemas/contact.schema.json        # Data schema
  address-book-main/                   # SPA frontend app
    manifest.toml
    resources.toml
    app.toml
    libraries/                         # Static assets (Svelte build)
```

### Bundle manifest (`address-book-bundle/manifest.toml`)

```toml
bundleName    = "address-book"
bundleVersion = "1.0.0"
apps          = ["address-book-service", "address-book-main"]
```

### App manifest (`address-book-service/manifest.toml`)

```toml
appName    = "address-book-service"
type       = "app-service"
appId      = "c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a"
entryPoint = "service"
```

`type` is either `app-service` (API backend) or `app-main` (frontend with static files). The `entryPoint` becomes the second segment in the route path. The `appId` is a stable UUID -- generate once, never change.

### Resources (`address-book-service/resources.toml`)

```toml
[[datasources]]
name       = "contacts"
driver     = "faker"
nopassword = true
required   = true
```

Declares a datasource named `contacts` using the `faker` driver. No connection string, no credentials -- faker generates synthetic data from the schema.

### App config (`address-book-service/app.toml`)

This file has three sections. Here is one DataView + View pair to show the pattern:

**Datasource binding:**

```toml
[data.datasources.contacts]
name       = "contacts"
driver     = "faker"
nopassword = true

[data.datasources.contacts.config]
locale                = "en_US"
seed                  = 42
max_records_per_query = 500
```

**DataView** -- a named, parameterized query:

```toml
[data.dataviews.list_contacts]
name          = "list_contacts"
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

**View** -- maps an HTTP endpoint to the DataView:

```toml
[api.views.list_contacts]
path            = "contacts"
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

`parameter_mapping.query` maps query string params to DataView parameters. Use `parameter_mapping.path` for path segments (e.g., `{id}`).

### Schema (`schemas/contact.schema.json`)

```json
{
  "type": "object",
  "description": "Address book contact record",
  "fields": [
    { "name": "id",         "type": "uuid",     "faker": "datatype.uuid",    "required": true  },
    { "name": "first_name", "type": "string",   "faker": "name.firstName",   "required": true  },
    { "name": "last_name",  "type": "string",   "faker": "name.lastName",    "required": true  },
    { "name": "email",      "type": "email",    "faker": "internet.email",   "required": true  },
    { "name": "phone",      "type": "phone",    "faker": "phone.number",     "required": false },
    { "name": "city",       "type": "string",   "faker": "location.city",    "required": false }
  ]
}
```

The `faker` attribute on each field tells the faker driver which generator to use. When you swap to a real database driver (e.g., `postgres`), the schema drives column mapping instead.

---

## 6. Add a New Endpoint

Add a "contacts by state" endpoint. Three steps: define the DataView, define the View, reload.

### Step 1: Add the DataView

Append to `address-book-service/app.toml`:

```toml
[data.dataviews.contacts_by_state]
name          = "contacts_by_state"
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[data.dataviews.contacts_by_state.caching]
ttl_seconds = 120

[[data.dataviews.contacts_by_state.parameters]]
name     = "state"
type     = "string"
required = true

[[data.dataviews.contacts_by_state.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20
```

### Step 2: Add the View

Append to the same file:

```toml
[api.views.contacts_by_state]
path            = "contacts/state/{state}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.contacts_by_state.handler]
type     = "dataview"
dataview = "contacts_by_state"

[api.views.contacts_by_state.parameter_mapping.path]
state = "state"

[api.views.contacts_by_state.parameter_mapping.query]
limit = "limit"
```

### Step 3: Reload

In development mode, `riversd` watches config files via the `notify` crate. Saving `app.toml` triggers a hot reload -- view routes and DataView configs swap atomically without restarting the server or dropping in-flight requests.

If hot reload is not active, restart the server.

### Step 4: Test

```bash
curl -k https://localhost:8080/address-book/service/contacts/state/California?limit=5
```

---

## 7. Enable GraphQL

Add the `[graphql]` section to `config/riversd.toml`:

```toml
[graphql]
enabled       = true
path          = "/graphql"
introspection = true
max_depth     = 10
max_complexity = 1000
```

Restart the server. Rivers auto-generates a GraphQL schema from all DataViews that have a `return_schema`. Each DataView becomes a query field; DataView parameters become query arguments.

### Query via curl

```bash
curl -k -X POST https://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ list_contacts(limit: 3) { id first_name last_name email city } }"}'
```

### Introspection

With `introspection = true`, you can point any GraphQL client (Altair, Insomnia, etc.) at `https://localhost:8080/graphql` to browse the schema interactively.

### What gets exposed

Every DataView with a `return_schema` generates a GraphQL object type. Fields come from the schema's `fields` array. DataView parameters map directly to GraphQL arguments with matching types:

| DataView param type | GraphQL type |
|---------------------|-------------|
| `string` | `String` |
| `integer` | `Int` |
| `uuid` | `ID` |
| `boolean` | `Boolean` |

Required parameters become non-nullable arguments. Parameters with defaults become nullable.

---

## Next Steps

- Swap the `faker` driver for `postgres`, `mysql`, or `sqlite` to use a real database
- Add authentication (`auth = "jwt"` or `auth = "session"`) to views
- Add a `CodeComponent` handler for custom business logic via WASM
- Package the bundle with `riverpackage` for deployment
- See `docs/arch/` for the full specification set
