# Rivers Address Book — Complete Build Spec

## Version 1.3 (all amendments incorporated)

---

## What You Are Building

A Rivers app bundle containing two apps:

- **address-book-service** — pure backend app-service. Serves a RESTful contact API using the faker datasource. No frontend. Port 9100.
- **address-book-main** — app-main. Hosts a Svelte SPA and proxies API calls to address-book-service. Port 8080.

```
Browser → address-book-main (8080) → address-book-service (9100)
              ↑ SPA (Svelte)
              ↑ /api/contacts proxy
              ↑ /api/contacts/search proxy
```

No auth — all endpoints and the SPA are public. No handlers, no WASM, no CodeComponents anywhere. Fully declarative.

---

## Final Bundle Structure

```
address-book-bundle/
├── CHANGELOG.md
├── manifest.toml
├── address-book-service/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── schemas/
│   │   └── contact.schema.json
│   └── app.toml
└── address-book-main/
    ├── manifest.toml
    ├── resources.toml
    ├── app.toml
    └── libraries/
        ├── package.json
        ├── rollup.config.js
        ├── src/
        │   ├── App.svelte
        │   ├── components/
        │   │   ├── ContactList.svelte
        │   │   └── SearchBar.svelte
        │   └── main.js
        └── spa/
            ├── index.html
            ├── bundle.js          ← compiled output of npm run build
            └── bundle.css         ← compiled output of npm run build
```

---

## CHANGELOG.md

Create at `address-book-bundle/CHANGELOG.md` — same level as `manifest.toml`. Filename uppercase. Append across rounds, never replace.

Entry format:
```markdown
## [Decision|Gap|Ambiguity|Error] — <short title>
**File:** <filename>
**Description:** What you did, decided, or encountered.
**Spec reference:** Which section of this spec.
**Resolution:** How you resolved it, or "UNRESOLVED".
```

---

## Part 1 — address-book-bundle

### address-book-bundle/manifest.toml

```toml
bundleName    = "address-book"
bundleVersion = "1.0.0"
source        = "https://github.com/acme/address-book"
apps          = ["address-book-service", "address-book-main"]
```

Order in `apps` matters — app-services start before app-mains. Rivers will not start address-book-main until address-book-service is healthy.

---

## Part 2 — address-book-service

### address-book-service/manifest.toml

```toml
appName       = "address-book-service"
description   = "Address book REST API — contact management service"
version       = "1.0.0"
type          = "app-service"
appId         = "c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a"
entryPoint    = "http://0.0.0.0:9100"
appEntryPoint = "https://address-book-svc.internal.acme.com"
source        = "https://github.com/acme/address-book/address-book-service"
```

`appId` must not be regenerated. It is the stable identity used by address-book-main to declare its service dependency.

---

### address-book-service/resources.toml

```toml
[[datasources]]
name       = "contacts"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true
```

- `x-type` — declares driver contract for build-time schema attribute validation by `riverpackage`
- `nopassword = true` — faker requires no credentials; omit `lockbox` entirely
- No `[[services]]` section — address-book-service has no app-service dependencies

---

### address-book-service/schemas/contact.schema.json

```json
{
  "type": "object",
  "description": "Address book contact record",
  "fields": [
    { "name": "id",         "type": "uuid",     "faker": "datatype.uuid",          "required": true  },
    { "name": "first_name", "type": "string",   "faker": "name.firstName",         "required": true  },
    { "name": "last_name",  "type": "string",   "faker": "name.lastName",          "required": true  },
    { "name": "email",      "type": "email",    "faker": "internet.email",         "required": true  },
    { "name": "phone",      "type": "phone",    "faker": "phone.number",           "required": false },
    { "name": "company",    "type": "string",   "faker": "company.name",           "required": false },
    { "name": "street",     "type": "string",   "faker": "location.streetAddress", "required": false },
    { "name": "city",       "type": "string",   "faker": "location.city",          "required": false },
    { "name": "state",      "type": "string",   "faker": "location.state",         "required": false },
    { "name": "zip",        "type": "string",   "faker": "location.zipCode",       "required": false },
    { "name": "country",    "type": "string",   "faker": "location.country",       "required": false },
    { "name": "avatar_url", "type": "string",   "faker": "image.avatar",           "required": false },
    { "name": "created_at", "type": "datetime", "faker": "date.past",              "required": true  }
  ]
}
```

The `faker` attribute uses faker.js dot notation: `"category.method"`. It is only valid for the `faker` driver — using it against a PostgreSQL datasource is a hard validation error at build time.

---

### address-book-service/app.toml

```toml
# ─────────────────────────────────────────────
# Datasource
# ─────────────────────────────────────────────

[data.datasources.contacts]
driver     = "faker"
nopassword = true

[data.datasources.contacts.config]
locale                = "en_US"
seed                  = 42
max_records_per_query = 500

# ─────────────────────────────────────────────
# DataViews
# ─────────────────────────────────────────────

[data.dataviews.list_contacts]
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[data.dataviews.list_contacts.cache]
enabled     = true
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

# ─────────────────────────

[data.dataviews.get_contact]
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[data.dataviews.get_contact.cache]
enabled     = true
ttl_seconds = 300

[[data.dataviews.get_contact.parameters]]
name     = "id"
type     = "uuid"
required = true

# ─────────────────────────

[data.dataviews.search_contacts]
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[data.dataviews.search_contacts.cache]
enabled     = true
ttl_seconds = 30

[[data.dataviews.search_contacts.parameters]]
name     = "q"
type     = "string"
required = true

[[data.dataviews.search_contacts.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

# ─────────────────────────

[data.dataviews.contacts_by_city]
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[data.dataviews.contacts_by_city.cache]
enabled     = true
ttl_seconds = 120

[[data.dataviews.contacts_by_city.parameters]]
name     = "city"
type     = "string"
required = true

[[data.dataviews.contacts_by_city.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

[api.views.list_contacts]
path            = "/api/contacts"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.list_contacts.handler]
type     = "data_view"
dataview = "list_contacts"

[api.views.list_contacts.parameter_mapping.query]
limit  = "limit"
offset = "offset"

# ─────────────────────────

[api.views.get_contact]
path            = "/api/contacts/{id}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_contact.handler]
type     = "data_view"
dataview = "get_contact"

[api.views.get_contact.parameter_mapping.path]
id = "id"

# ─────────────────────────

[api.views.search_contacts]
path            = "/api/contacts/search"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.search_contacts.handler]
type     = "data_view"
dataview = "search_contacts"

[api.views.search_contacts.parameter_mapping.query]
q     = "q"
limit = "limit"

# ─────────────────────────

[api.views.contacts_by_city]
path            = "/api/contacts/city/{city}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.contacts_by_city.handler]
type     = "data_view"
dataview = "contacts_by_city"

[api.views.contacts_by_city.parameter_mapping.path]
city = "city"

[api.views.contacts_by_city.parameter_mapping.query]
limit = "limit"
```

**Config syntax rules — critical:**
- `query` on faker DataViews is a file path, not inline JSON
- Cache field is `ttl_seconds` (integer, seconds) — not `ttl` or `ttl_ms`
- Parameters use `[[data.dataviews.<name>.parameters]]` array-of-tables with an explicit `name` field — named subtables (`[parameters.limit]`) produce the wrong data structure
- Views live under `[api.views.*]` — the `api.` prefix is required; `[views.*]` is silently ignored by riversd
- Parameter mapping uses segregated subtables: `parameter_mapping.query`, `parameter_mapping.path`, `parameter_mapping.header`, `parameter_mapping.body`. Format: `{http_param_name} = "{dataview_param_name}"`

---

## Part 3 — address-book-main

### address-book-main/manifest.toml

```toml
appName       = "address-book-main"
description   = "Address book SPA — Svelte frontend with API proxy"
version       = "1.0.0"
type          = "app-main"
appId         = "<generate a stable UUID — do not regenerate>"
entryPoint    = "http://0.0.0.0:8080"
appEntryPoint = "https://address-book.acme.com"
source        = "https://github.com/acme/address-book/address-book-main"
```

---

### address-book-main/resources.toml

```toml
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

- `[[datasources]]` — the HTTP driver makes outbound calls to declared services
- `nopassword = true` — service-to-service calls carry session auth automatically; no lockbox needed
- `[[services]]` — declares startup dependency. `appId` must exactly match address-book-service's appId. Rivers will not start app-main until the declared service is healthy.

---

### address-book-main/app.toml

```toml
# ─────────────────────────────────────────────
# HTTP Datasource — proxy to address-book-service
# ─────────────────────────────────────────────

[data.datasources.address-book-api]
driver     = "http"
service    = "address-book-service"
nopassword = true

[data.datasources.address-book-api.config]
base_path      = "/api"
timeout_ms     = 5000
retry_attempts = 2

# ─────────────────────────────────────────────
# DataViews — proxy queries
# ─────────────────────────────────────────────

# For HTTP datasources, `query` is a URL path relative to `base_path`.
# Parameters are forwarded as query string args automatically.

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

# ─────────────────────────────────────────────
# Views — REST proxy endpoints
# ─────────────────────────────────────────────

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

# ─────────────────────────────────────────────
# SPA Config
# ─────────────────────────────────────────────

[spa]
enabled      = true
root_path    = "libraries/spa"
index_file   = "index.html"
spa_fallback = true
max_age      = 3600
```

- `service` on the HTTP datasource — logical name resolved by Rivers to the running address-book-service endpoint at startup
- `root_path` is relative to the app directory (`address-book-main/`)
- `spa_fallback = true` — all non-API routes serve `index.html`; `/api/*` routes always take precedence

---

## Part 4 — Svelte SPA

### Build Pipeline

Place in `address-book-main/libraries/`.

**package.json**
```json
{
  "name": "address-book-main",
  "version": "1.0.0",
  "private": true,
  "scripts": {
    "build": "rollup -c",
    "dev": "rollup -c -w"
  },
  "devDependencies": {
    "rollup": "^4.0.0",
    "@rollup/plugin-node-resolve": "^15.0.0",
    "rollup-plugin-svelte": "^7.0.0",
    "rollup-plugin-css-only": "^4.0.0",
    "svelte": "^4.0.0"
  }
}
```

**rollup.config.js**
```javascript
import svelte from 'rollup-plugin-svelte';
import resolve from '@rollup/plugin-node-resolve';
import css from 'rollup-plugin-css-only';

export default {
  input: 'src/main.js',
  output: {
    file: 'spa/bundle.js',
    format: 'iife',
    name: 'app'
  },
  plugins: [
    svelte({ compilerOptions: { dev: false } }),
    css({ output: 'bundle.css' }),
    resolve({ browser: true, dedupe: ['svelte'] })
  ]
};
```

**Build:**
```bash
cd address-book-main/libraries
npm install
npm run build
# outputs: spa/bundle.js, spa/bundle.css
```

`spa/bundle.js` and `spa/bundle.css` must be real compiled output. Run the build step before deploying. Hand-written JS is not acceptable.

---

### libraries/spa/index.html

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Address Book</title>
  <link rel="stylesheet" href="/bundle.css">
</head>
<body>
  <script src="/bundle.js"></script>
</body>
</html>
```

---

### libraries/src/main.js

```javascript
import App from './App.svelte';

const app = new App({
  target: document.body,
});

export default app;
```

---

### libraries/src/App.svelte

Root component. State: `mode` (`"list"` or `"search"`), `contacts`, `total`, `page`, `pageSize` (20), `query`, `loading`, `error`.

Layout:
```
┌──────────────────────────────┐
│  Address Book                │
│  [Search input]  [Clear]     │
├──────────────────────────────┤
│  Contact cards grid          │
├──────────────────────────────┤
│  ← Prev   Page 1 of N   Next→│  (list mode only)
└──────────────────────────────┘
```

API calls against same-origin app-main endpoints:
```javascript
// List (paginated)
fetch(`/api/contacts?limit=${pageSize}&offset=${page * pageSize}`)

// Search
fetch(`/api/contacts/search?q=${encodeURIComponent(query)}&limit=50`)
```

Response is Rivers envelope:
```json
{ "data": [...], "meta": { "count": 20, "total": 500, "limit": 20, "offset": 0 } }
```

Pagination only visible in `mode = "list"`. Search results show all matches with no pagination.

---

### libraries/src/components/ContactList.svelte

Props: `contacts` (array). Renders a grid of cards.

Each card:
- Full name (bold)
- Email
- Phone (if present)
- Company (if present)
- City, State (if present)
- Avatar: `<img src={avatar_url}>` if present, fallback to initials

---

### libraries/src/components/SearchBar.svelte

Props: `query` (string), `onSearch` (callback), `onClear` (callback).

- Triggers `onSearch` on Enter keypress or after 300ms debounce
- Enter cancels pending debounce timer
- `onClear` resets to list mode
- Clear button visible only when input has value

---

## Validation Checklist

```
address-book-bundle/
├── CHANGELOG.md                                    ✓ bundle root, uppercase
├── manifest.toml                                   ✓ apps: [service, main]
├── address-book-service/
│   ├── manifest.toml                               ✓ type="app-service", stable appId
│   ├── resources.toml                              ✓ [[datasources]] faker, nopassword=true
│   ├── schemas/contact.schema.json                 ✓ 13 fields, faker dot notation
│   └── app.toml                                    ✓ datasource, 4 dataviews, 4 views
└── address-book-main/
    ├── manifest.toml                               ✓ type="app-main", new stable UUID
    ├── resources.toml                              ✓ [[datasources]] http + [[services]]
    ├── app.toml                                    ✓ http datasource, 2 dataviews, 2 views, [spa]
    └── libraries/
        ├── package.json                            ✓ rollup build scripts
        ├── rollup.config.js                        ✓ src/main.js → spa/bundle.js
        ├── src/App.svelte                          ✓ state, fetch, pagination
        ├── src/components/ContactList.svelte       ✓ card grid
        ├── src/components/SearchBar.svelte         ✓ debounce + Enter
        ├── src/main.js                             ✓ mounts App to document.body
        └── spa/
            ├── index.html                          ✓ links bundle.css + bundle.js
            ├── bundle.js                           ✓ compiled (not hand-written)
            └── bundle.css                          ✓ compiled
```

---

## Expected Behavior

```bash
cd address-book-main/libraries && npm install && npm run build

# Run service first
riversd --config address-book-service/app.toml  # port 9100

# Run main (waits for service healthy)
riversd --config address-book-main/app.toml     # port 8080

# SPA
open http://localhost:8080

# API via app-main proxy
curl "http://localhost:8080/api/contacts?limit=10"
curl "http://localhost:8080/api/contacts/search?q=john"

# Direct service (debug only)
curl "http://localhost:9100/api/contacts"
```
