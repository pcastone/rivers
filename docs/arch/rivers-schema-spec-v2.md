# Rivers DataView Schema Specification

## Version 2.0

**Supersedes:** schema.md v1.0  
**Changes in v2.0:**
- `query` field on faker DataViews is now a file path, not inline JSON
- Schema files are driver-aware — each driver declares which schema attributes it supports
- `nopassword = true` field added to resources.toml for credential-free drivers
- `x-type` field added to resources.toml to declare driver contract for build-time validation
- Schema attribute validation runs at both `riverpackage` build time and `riversd` deploy time

---

## Table of Contents

1. [Overview](#overview)
2. [Schema Files](#schema-files)
3. [Driver Schema Attributes](#driver-schema-attributes)
4. [Datasource Patterns](#datasource-patterns)
5. [DataView Query Patterns](#dataview-query-patterns)
6. [Resources Declaration](#resources-declaration)
7. [Validation Chain](#validation-chain)
8. [Implementation Reference](#implementation-reference)

---

## 1. Overview

### Design Principles

- **Unified interface:** All datasources expose consistent interfaces despite underlying protocol differences
- **File-referenced schemas:** Schema definitions live in `.json` files — never inline in TOML config
- **Driver-aware validation:** Schema attributes are driver-specific. A `faker` attribute on a PostgreSQL DataView is a hard validation error
- **Credential safety:** Credentials always through LockBox, never in config. Credential-free drivers declare `nopassword = true`
- **Fail early:** Schema attribute violations are caught at `riverpackage` build time, not at runtime

### Datasource Categories

| Category | Datasources | Query Field | Credentials |
|---|---|---|---|
| Relational | PostgreSQL, MySQL | SQL string | lockbox required |
| Directory | LDAP | LDAP filter string | lockbox required |
| Message Queue | Kafka, RabbitMQ | operation-based | lockbox required |
| Synthetic | Faker | schema file path | `nopassword = true` |

---

## 2. Schema Files

Schema definitions are stored as `.json` files within the app bundle. The `query` and `return_schema` fields on DataViews reference these files by path, relative to the app directory root.

### File Location Convention

```
address-book-service/
└── schemas/
    ├── contact.schema.json
    ├── address.schema.json
    └── phone.schema.json
```

### Schema File Format

Schema files are JSON with a `fields` array. Each field entry carries:
- `name` — field name in the output record
- `type` — Rivers primitive type (see type table below)
- `required` — whether the field must be present
- driver-specific attributes (see section 3)

```json
{
  "type": "object",
  "description": "Human-readable description of this schema",
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
|---|---|
| `uuid` | UUID v4 string |
| `string` | UTF-8 string |
| `integer` | 64-bit signed integer |
| `float` | 64-bit float |
| `boolean` | true/false |
| `email` | String validated as email address |
| `phone` | String validated as phone number |
| `datetime` | ISO 8601 datetime string |
| `date` | ISO 8601 date string |
| `url` | String validated as URL |
| `json` | Arbitrary JSON value |

---

## 3. Driver Schema Attributes

Schema attributes beyond `name`, `type`, and `required` are driver-specific. Each driver declares which attributes it supports. Using an unsupported attribute against a driver is a validation error.

### Attribute Registry

| Attribute | Supported Drivers | Description |
|---|---|---|
| `faker` | `faker` only | Faker.js dot-notation category: `"name.firstName"` |
| `min` | `postgresql`, `mysql` | Minimum numeric value for validation |
| `max` | `postgresql`, `mysql` | Maximum numeric value for validation |
| `pattern` | `postgresql`, `mysql`, `ldap` | Regex pattern for string validation |
| `format` | `postgresql`, `mysql` | Format hint: `email`, `url`, `uuid` |
| `unique` | `faker` | Generate unique values within a batch |
| `domain` | `faker` | Domain constraint, e.g. for email generation |

### The `faker` Attribute

Faker attributes use faker.js dot notation: `"category.method"`.

```json
{ "name": "first_name", "type": "string", "faker": "name.firstName" }
{ "name": "email",      "type": "email",  "faker": "internet.email" }
{ "name": "city",       "type": "string", "faker": "location.city"  }
{ "name": "avatar_url", "type": "string", "faker": "image.avatar"   }
```

Common faker categories:

| Category | Methods |
|---|---|
| `name` | `firstName`, `lastName`, `fullName`, `prefix`, `suffix` |
| `internet` | `email`, `url`, `username`, `ipv4`, `domainName` |
| `phone` | `number` |
| `location` | `streetAddress`, `city`, `state`, `zipCode`, `country`, `latitude`, `longitude` |
| `company` | `name`, `catchPhrase`, `bs` |
| `datatype` | `uuid`, `number`, `float`, `boolean` |
| `date` | `past`, `future`, `recent`, `between` |
| `image` | `avatar`, `url` |
| `lorem` | `word`, `words`, `sentence`, `sentences`, `paragraph` |

### Validation Error Examples

```
# ERROR — faker attribute on postgresql datasource
SchemaAttributeError: field "email" uses attribute "faker" which is not supported
by driver "postgresql". Supported attributes: type, format, required, min, max, pattern.
  → in: schemas/contact.schema.json
  → dataview: list_contacts
  → datasource: orders_db (driver: postgresql)

# ERROR — unknown faker method
SchemaAttributeError: field "email" has unknown faker method "internet.emailAddress".
Did you mean "internet.email"?
  → in: schemas/contact.schema.json
```

---

## 4. Datasource Patterns

### 4.1 SQL (PostgreSQL, MySQL)

```toml
[data.datasources.orders_db]
driver             = "postgresql"
host               = "${DB_HOST}"
port               = 5432
database           = "orders"
credentials_source = "lockbox://db/orders"

[data.datasources.orders_db.config]
ssl_mode         = "prefer"
statement_timeout = 30000

[data.datasources.orders_db.connection_pool]
min_idle           = 2
max_size           = 20
connection_timeout = 5000
test_query         = "SELECT 1"

[data.datasources.orders_db.features]
use_prepared_statements = true
supports_transactions   = true
```

### 4.2 Message Queue (Kafka, RabbitMQ)

```toml
[data.datasources.events]
driver             = "kafka"
bootstrap_servers  = "${KAFKA_BROKERS}"
credentials_source = "lockbox://kafka/events"

[data.datasources.events.config]
group_id          = "rivers-consumer"
enable_auto_commit = false
```

### 4.3 Directory (LDAP)

```toml
[data.datasources.directory]
driver        = "ldap"
host          = "${LDAP_HOST}"
port          = 389
use_start_tls = true

[data.datasources.directory.config]
base_dn            = "dc=company,dc=com"
credentials_source = "lockbox://ldap/service"
```

### 4.4 Synthetic (Faker)

Faker requires no credentials. Use `nopassword = true` in `resources.toml` and omit `credentials_source` from config.

```toml
[data.datasources.contacts]
driver     = "faker"
nopassword = true

[data.datasources.contacts.config]
locale                = "en_US"
seed                  = 42
max_records_per_query = 500
```

`seed` — optional. When set, faker produces the same records on every query. Omit for random data.

---

## 5. DataView Query Patterns

### 5.1 SQL DataViews

`query` is a SQL string. Schema file is referenced via `return_schema`.

```toml
[data.dataviews.list_orders]
datasource    = "orders_db"
query         = "SELECT * FROM orders WHERE status = $status ORDER BY created_at DESC LIMIT $limit"
return_schema = "schemas/order.schema.json"

[[data.dataviews.list_orders.parameters]]
name     = "status"
type     = "string"
required = false
default  = "active"

[[data.dataviews.list_orders.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[data.dataviews.list_orders.cache]
enabled     = true
ttl_seconds = 60
```

### 5.2 Message Queue DataViews

`query` is not used — operation-based config instead.

```toml
[data.dataviews.consume_events]
datasource = "events"
operation  = "consume"

[data.dataviews.consume_events.consumer_config]
topics       = ["orders", "payments"]
timeout_ms   = 1000
max_messages = 100
```

### 5.3 LDAP DataViews

`query` is an LDAP filter string with `$param` substitution.

```toml
[data.dataviews.find_user]
datasource = "directory"
query      = "(&(objectClass=person)(uid=$username))"
base_dn    = "ou=users,dc=company,dc=com"
scope      = "subtree"

[[data.dataviews.find_user.attributes]]
ldap_name  = "uid"
field_name = "username"
type       = "string"

[[data.dataviews.find_user.attributes]]
ldap_name  = "mail"
field_name = "email"
type       = "email"
```

### 5.4 Faker DataViews

`query` is a **file path** to a schema file. The schema file defines both what faker generates and what the DataView returns. The path is relative to the app directory root.

```toml
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
```

**Why the same file for both `query` and `return_schema`?** Faker generates data defined by the schema and returns records that match the same schema. They are the same contract. For SQL DataViews, `query` is the SQL string and `return_schema` is the shape of the rows returned — those are different things.

#### Complete Faker Schema File Example

`schemas/contact.schema.json`:

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

---

## 6. Resources Declaration

`resources.toml` declares what an app needs. It lives in the app directory alongside `manifest.toml`. Ops provisions what's declared here — developers never touch LockBox directly.

### Full Schema

```toml
[[datasources]]
name       = "orders-db"        # logical name — used in app.toml
driver     = "postgresql"       # driver type
x-type     = "postgresql"       # build-time contract (same as driver for built-ins)
lockbox    = "db/orders-prod"   # lockbox alias — ops provisions this
required   = true               # if true, startup fails when unavailable

[[datasources]]
name       = "contacts"
driver     = "faker"
x-type     = "faker"
nopassword = true               # omit lockbox entirely — faker needs no credentials
required   = true

[[services]]
name    = "orders-service"      # logical service name
appId   = "f47ac10b-..."        # stable UUID — must match that app's manifest.toml
required = true
```

### `nopassword` Field

| Behavior | Without `nopassword` | With `nopassword = true` |
|---|---|---|
| lockbox field | Required | Must be omitted |
| Startup credential check | Required — fails if missing | Skipped |
| `riverpackage --pre-flight` | Lists lockbox alias as required | Does not list lockbox alias |
| Drivers that use this | — | `faker`, sqlite (embedded) |

### `x-type` Field

`x-type` declares the driver contract. `riverpackage` uses it to validate schema attribute compatibility at build time — before the bundle is deployed.

For built-in drivers: `x-type` equals the `driver` value.
For plugins: `x-type` is the driver name registered by the plugin.

If `x-type` and the actual registered driver at deploy time don't match, `riversd` fails deployment with an explicit error.

---

## 7. Validation Chain

Schema attribute validation runs at two stages. Both must pass.

### Stage 1: `riverpackage --pre-flight` (Build Time)

Runs before bundle creation. Uses `x-type` from `resources.toml` to determine driver for each datasource.

Checks:
- All `query` file paths in DataViews resolve to existing files
- All `return_schema` file paths resolve to existing files
- Schema attributes in each schema file are supported by the datasource's `x-type`
- All required LockBox aliases are listed (omits aliases for `nopassword = true` datasources)

```
$ riverpackage --pre-flight ./address-book-service

Pre-flight checks for: address-book-service
─────────────────────────────────────────────
✓ manifest.toml — valid
✓ resources.toml — valid
✓ schemas/contact.schema.json — found
✓ schemas/contact.schema.json — all attributes valid for driver "faker"
✓ dataview "list_contacts" — query file resolves
✓ dataview "get_contact" — query file resolves
✓ dataview "search_contacts" — query file resolves
✓ dataview "contacts_by_city" — query file resolves
─────────────────────────────────────────────
Required LockBox aliases (ops must provision):
  (none — all datasources are nopassword)

Pre-flight: PASS
```

### Stage 2: `riversd` Deploy Time

Runs after bundle is unpacked. Uses the registered driver in the DriverFactory.

Checks:
- `x-type` matches the registered driver type
- Schema attribute validation re-run against the real driver registry
- All required datasources are reachable (except `nopassword = true` datasources)
- All required services declared in `resources.toml` are running

### Development Environment

`riversd` in development mode surfaces schema attribute errors as structured log events and HTTP error responses, not panics. The error includes schema file path, field name, attribute name, and the list of supported attributes for that driver.

```
ERROR rivers::schema: attribute_validation_failed
  schema_file = "schemas/contact.schema.json"
  field       = "email"
  attribute   = "faker"
  driver      = "postgresql"
  supported   = ["type", "format", "required", "min", "max", "pattern"]
  dataview    = "list_contacts"
  datasource  = "orders_db"
```

---

## 8. Implementation Reference

### Datasource Trait (Rust)

```rust
pub trait Datasource: Send + Sync {
    fn id(&self) -> &str;
    fn driver_type(&self) -> DriverType;

    /// Attributes this driver supports in schema files.
    /// Used by riverpackage and riversd for validation.
    fn supported_schema_attributes(&self) -> &[&str];

    async fn connect(&mut self, config: &DatasourceConfig) -> Result<(), DatasourceError>;
    async fn execute(&self, dataview: &DataView, params: QueryParams) -> Result<QueryResult>;
    async fn health_check(&self) -> Result<HealthStatus>;
}
```

### Faker Datasource — Query Resolution

The faker driver resolves `query` as a file path, loads the schema, and generates records matching that schema. The inline JSON pattern from v1.0 is not supported.

```rust
impl FakerDatasource {
    pub async fn execute_generate(
        &self,
        view: &DataView,
        params: QueryParams
    ) -> Result<QueryResult> {
        // query is a file path — load and parse the schema
        let schema: FakerSchema = self.load_schema_file(&view.query).await?;
        let count = params.get("limit").unwrap_or(20usize);

        let mut generator = self.generator.lock().await;
        if let Some(seed) = self.config.seed {
            generator.set_seed(seed);
        }

        let records = (0..count)
            .map(|_| self.generate_record(&schema, &mut generator))
            .collect::<Result<Vec<_>>>()?;

        Ok(QueryResult::Records(records))
    }

    fn generate_record(
        &self,
        schema: &FakerSchema,
        gen: &mut FakeDataGenerator
    ) -> Result<Record> {
        let mut record = Record::new();
        for field in &schema.fields {
            let value = gen.generate_by_faker_path(&field.faker)?;
            record.insert(field.name.clone(), value);
        }
        Ok(record)
    }
}
```

### DataViewError Extensions

```rust
#[derive(Debug, thiserror::Error)]
pub enum DataViewError {
    // ... existing variants ...

    #[error("Schema attribute '{attribute}' is not supported by driver '{driver}'. \
             Supported attributes: {supported:?}")]
    UnsupportedSchemaAttribute {
        attribute: String,
        driver:    String,
        supported: Vec<String>,
    },

    #[error("Schema file not found: {path}")]
    SchemaFileNotFound { path: String },

    #[error("Schema file parse error in '{path}': {reason}")]
    SchemaFileParseError { path: String, reason: String },

    #[error("Unknown faker method '{method}' on field '{field}'")]
    UnknownFakerMethod { method: String, field: String },
}
```

---

## Revision History

| Version | Date | Changes |
|---|---|---|
| 1.0 | 2024-01-20 | Initial specification |
| 2.0 | 2026-03-13 | File-referenced schemas; driver-aware attributes; `nopassword`; `x-type`; two-stage validation |
