# Tutorial: Faker Datasource

**Rivers v0.50.1**

## Overview

The faker driver generates synthetic data from JSON schema definitions. It requires no external database, no credentials, and no network access. Data is generated in-memory using faker.js-style category methods declared as schema attributes.

Use the faker driver when:
- Prototyping an API before the real database exists
- Writing integration tests that need realistic data
- Building demos or reference applications

The faker driver is a built-in driver registered directly in `DriverFactory` at startup. No plugin loading is required.

## Prerequisites

- A Rivers app bundle with a valid `manifest.toml`
- No external services or credentials needed (`nopassword = true`)

## Step 1: Declare the Datasource

In your app's `resources.toml`, declare a faker datasource. The `x-type` field enables build-time schema attribute validation by `riverpackage`.

```toml
# resources.toml

[[datasources]]
name       = "contacts"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true
```

- `nopassword = true` -- faker requires no credentials; omit `lockbox` entirely
- `x-type = "faker"` -- tells `riverpackage` to validate that schema files only use faker-compatible attributes
- `required = true` -- app will not start if the datasource fails to initialize

## Step 2: Configure the Datasource

In your app's `app.toml`, configure the faker datasource with generation options.

```toml
# app.toml

[data.datasources.contacts]
name       = "contacts"
driver     = "faker"
nopassword = true

[data.datasources.contacts.config]
locale                = "en_US"
seed                  = 42
max_records_per_query = 500
```

- `locale` -- determines the language/region for generated data (e.g., `en_US`, `de_DE`, `ja_JP`)
- `seed` -- integer seed for deterministic output; the same seed produces the same data on every query
- `max_records_per_query` -- upper bound on how many records a single DataView query can return

## Step 3: Define a Schema

Create a schema file at `schemas/contact.schema.json`. The `faker` attribute on each field uses faker.js dot notation (`"category.method"`) to specify what kind of data to generate.

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

The schema attribute key must be `"faker"` -- not `"faker_type"`. Using `"faker"` against a non-faker datasource (e.g., PostgreSQL) is a hard validation error at build time.

### Common Faker Categories

| Category   | Methods                                                        |
|------------|----------------------------------------------------------------|
| `name`     | `firstName`, `lastName`, `fullName`, `prefix`, `suffix`        |
| `internet` | `email`, `url`, `username`, `ipv4`, `domainName`               |
| `phone`    | `number`                                                       |
| `location` | `streetAddress`, `city`, `state`, `zipCode`, `country`         |
| `company`  | `name`, `catchPhrase`, `bs`                                    |
| `datatype` | `uuid`, `number`, `float`, `boolean`                           |
| `date`     | `past`, `future`, `recent`, `between`                          |
| `image`    | `avatar`, `url`                                                |
| `lorem`    | `word`, `words`, `sentence`, `sentences`, `paragraph`          |

## Step 4: Create a DataView

For faker DataViews, the `query` field is a file path to the schema -- not a SQL statement or inline JSON.

```toml
# app.toml (continued)

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

# ─────────────────────────

[data.dataviews.get_contact]
name          = "get_contact"
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[data.dataviews.get_contact.caching]
ttl_seconds = 300

[[data.dataviews.get_contact.parameters]]
name     = "id"
type     = "uuid"
required = true

# ─────────────────────────

[data.dataviews.search_contacts]
name          = "search_contacts"
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

[data.dataviews.search_contacts.caching]
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
```

Key syntax rules:
- Parameters use `[[data.dataviews.<name>.parameters]]` (array-of-tables) with an explicit `name` field
- Do NOT use named subtables like `[parameters.limit]` -- that produces the wrong data structure
- Cache TTL uses `ttl_seconds` (integer) -- not `ttl` or `ttl_ms`

## Step 5: Create a View

Views live under `[api.views.*]` -- the `api.` prefix is required. Using `[views.*]` without the prefix is silently ignored by riversd.

```toml
# app.toml (continued)

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

# ─────────────────────────

[api.views.get_contact]
path            = "contacts/{id}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_contact.handler]
type     = "dataview"
dataview = "get_contact"

[api.views.get_contact.parameter_mapping.path]
id = "id"

# ─────────────────────────

[api.views.search_contacts]
path            = "contacts/search"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.search_contacts.handler]
type     = "dataview"
dataview = "search_contacts"

[api.views.search_contacts.parameter_mapping.query]
q     = "q"
limit = "limit"
```

Parameter mapping uses segregated subtables:
- `parameter_mapping.query` -- maps query string params to DataView params
- `parameter_mapping.path` -- maps path template `{params}` to DataView params

## Testing

Start the app and test the endpoints:

```bash
# List contacts with defaults (limit=20, offset=0)
curl http://localhost:9100/contacts

# List contacts with pagination
curl "http://localhost:9100/contacts?limit=10&offset=20"

# Get a single contact by ID
curl http://localhost:9100/contacts/550e8400-e29b-41d4-a716-446655440000

# Search contacts
curl "http://localhost:9100/contacts/search?q=john&limit=5"
```

Because the seed is fixed (`seed = 42`), every request with the same parameters returns identical data. Change the seed to get a different dataset.

## Configuration Reference

### resources.toml Fields

| Field        | Type    | Required | Description                                          |
|--------------|---------|----------|------------------------------------------------------|
| `name`       | string  | yes      | Datasource name, referenced in app.toml              |
| `driver`     | string  | yes      | Must be `"faker"`                                    |
| `x-type`     | string  | yes      | Must be `"faker"` for build-time validation          |
| `nopassword` | boolean | yes      | Must be `true` -- faker has no credentials           |
| `required`   | boolean | no       | Whether the app fails to start without this source   |

### app.toml Datasource Config

| Field                                   | Type    | Required | Default | Description                                      |
|-----------------------------------------|---------|----------|---------|--------------------------------------------------|
| `data.datasources.*.driver`             | string  | yes      | --      | Must be `"faker"`                                |
| `data.datasources.*.nopassword`         | boolean | yes      | --      | Must be `true`                                   |
| `data.datasources.*.config.locale`      | string  | no       | `en_US` | Faker locale for generated data                  |
| `data.datasources.*.config.seed`        | integer | no       | random  | Seed for deterministic generation                |
| `data.datasources.*.config.max_records_per_query` | integer | no | 500 | Upper bound on records per query                 |

### Schema Faker Attributes

| Attribute | Type    | Description                                              |
|-----------|---------|----------------------------------------------------------|
| `faker`   | string  | Faker.js dot notation, e.g. `"name.firstName"`           |
| `unique`  | boolean | Generate unique values within a single batch             |
| `domain`  | string  | Domain constraint for email generation                   |
