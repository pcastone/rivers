# Rivers Driver Schema Validation Specification

**Document Type:** Implementation Specification  
**Version:** 1.0  
**Status:** Locked — Implementation-Ready  
**Scope:** SchemaSyntaxChecker and Validator contracts per driver, per-method schema validation, validation chain  
**Depends On:** Rivers Technology Path Spec v1.0, Rivers Driver Spec, Rivers Schema Spec v2.0  
**Supersedes:** `supported_schema_attributes()` on the old Datasource trait

---

## Table of Contents

1. [Design Principles](#1-design-principles)
2. [Validation Chain](#2-validation-chain)
3. [Driver Contract — Three Responsibilities](#3-driver-contract--three-responsibilities)
4. [Per-Method Schema Model](#4-per-method-schema-model)
5. [Common Schema Fields](#5-common-schema-fields)
6. [PostgreSQL Driver](#6-postgresql-driver)
7. [MySQL Driver](#7-mysql-driver)
8. [SQLite Driver](#8-sqlite-driver)
9. [Redis Driver](#9-redis-driver)
10. [Memcached Driver](#10-memcached-driver)
11. [Faker Driver](#11-faker-driver)
12. [HTTP Driver](#12-http-driver)
13. [Kafka Driver](#13-kafka-driver)
14. [RabbitMQ Driver](#14-rabbitmq-driver)
15. [NATS Driver](#15-nats-driver)
16. [EventBus Driver](#16-eventbus-driver)
17. [Plugin Driver Requirements](#17-plugin-driver-requirements)
18. [Error Catalog](#18-error-catalog)
19. [Pseudo DataView Validation](#19-pseudo-dataview-validation)

---

## 1. Design Principles

### 1.1 The Driver Owns Its Schema Language

A Redis schema and a Postgres schema are fundamentally different shapes of data. There is no universal schema format. Each driver defines what a valid schema looks like for its data model. The framework delegates validation entirely to the driver.

### 1.2 One Format, Two Origins

Schema definitions use the same JSON format whether declared in a `.schema.json` file or constructed inline in a handler via the pseudo DataView builder. The `driver` field routes to the correct driver's validation.

### 1.3 Fail Early, Fail Clearly

Schema validation runs at build time (`riverpackage --pre-flight`) and deploy time (`riversd`). By the time a request arrives, every schema has been validated twice. Runtime validation handles data shape, not schema shape.

---

## 2. Validation Chain

### 2.1 Three Stages

```
Build time:    SchemaSyntaxChecker   →  "Is this schema well-formed for this driver?"
Deploy time:   SchemaSyntaxChecker   →  re-verified against the real registered driver
Request time:  Validator             →  "Does this data match this schema?"
               Executor              →  "Run the operation"
               Validator             →  "Do these results match the return schema?"
```

### 2.2 Stage Responsibilities

**SchemaSyntaxChecker** — examines the schema document only. Knows nothing about data, queries, or results. Pure structural validation. Catches:
- Missing required schema fields for the driver
- Invalid attribute names or values
- Structural incompatibilities (e.g., Redis `type: "string"` with `fields` array)
- `$variable` references in queries with no matching parameter declaration
- Parameter declarations with no matching `$variable` in the query

**Validator** — examines data against a schema. Runs at request time in both directions:
- **Input validation** (POST/PUT schema): incoming request data checked before the executor fires
- **Output validation** (GET schema): result data checked before it reaches the caller
- Catches: type mismatches, missing required fields, constraint violations (min/max/pattern), unexpected fields, column count mismatches

**Executor** — runs the operation. Receives validated input, returns raw results. The Validator wraps it on both sides. The Executor never sees invalid input, and its output is checked before reaching the caller.

### 2.3 Request-Time Pipeline

```
Input data  →  Validator (POST/PUT schema)  →  Executor  →  Validator (GET schema)  →  Response
```

For a GET request with no input body, only the output validator runs.  
For a DELETE with no return schema, only the input validator runs (if `delete_schema` is defined).  
For a POST with both schemas, both validators run — input before, output after.

---

## 3. Driver Contract — Three Responsibilities

### 3.1 Trait

```rust
pub trait Driver: Send + Sync {
    fn driver_type(&self) -> DriverType;

    /// Build/deploy time — is this schema structurally valid for this driver?
    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError>;

    /// Request time — does this data conform to this schema?
    fn validate(
        &self,
        data: &Value,
        schema: &SchemaDefinition,
        direction: ValidationDirection,
    ) -> Result<(), ValidationError>;

    /// Request time — execute the operation
    async fn execute(
        &self,
        dataview: &DataView,
        params: &QueryParams,
    ) -> Result<QueryResult>;

    async fn connect(&mut self, config: &DatasourceConfig) -> Result<()>;
    async fn health_check(&self) -> Result<HealthStatus>;
}
```

### 3.2 Method Parameter

`check_schema_syntax` receives the HTTP method the schema is associated with. This allows the driver to enforce method-specific rules:
- A GET schema for Postgres must describe output columns
- A POST schema for Kafka must describe a message value
- A DELETE schema for Redis needs only a key pattern

### 3.3 Direction Parameter

`validate` receives a direction indicating whether it's validating input (data flowing in from the client) or output (data flowing out from the driver).

```rust
pub enum ValidationDirection {
    Input,   // POST/PUT body → validate before execution
    Output,  // Query results → validate before response
}
```

### 3.4 SchemaDefinition

The parsed schema object that both methods receive:

```rust
pub struct SchemaDefinition {
    pub driver: String,
    pub schema_type: String,            // "object", "hash", "message", etc.
    pub description: Option<String>,
    pub fields: Option<Vec<FieldDef>>,
    pub driver_specific: serde_json::Value,  // everything else — driver parses this
}

pub struct FieldDef {
    pub name: String,
    pub field_type: String,             // Rivers primitive type
    pub required: bool,
    pub attributes: HashMap<String, serde_json::Value>,  // min, max, pattern, faker, etc.
}
```

`driver_specific` carries all driver-specific top-level keys (e.g., `key_pattern` for Redis, `topic` for Kafka, `key` for Kafka message keys). The driver parses these from the raw JSON.

---

## 4. Per-Method Schema Model

### 4.1 Four Schemas Per DataView

Each DataView can declare up to four schemas, one per HTTP method:

| TOML field | Method | Validates | Direction |
|---|---|---|---|
| `get_schema` | GET | Output — what the query returns | Output |
| `post_schema` | POST | Input — what the request body must contain | Input |
| `put_schema` | PUT | Input — what the request body must contain | Input |
| `delete_schema` | DELETE | Input — what the request must provide | Input |

### 4.2 Return Schema on Writes

A POST or PUT DataView can also declare a `get_schema` to validate the **return value** of the write operation (e.g., `RETURNING *` in Postgres). In that case, both validators run:

```
POST body → Validator (post_schema, Input) → Executor → Validator (get_schema, Output) → Response
```

### 4.3 Schema Is Optional Per Method

A DataView only declares schemas for methods it supports. A read-only DataView has `get_schema` only. A write-only endpoint has `post_schema` only. No dead config.

### 4.4 SchemaSyntaxChecker Per Method

At build/deploy time, the checker validates each declared schema against its associated method:

```rust
// For each method-schema pair on the DataView:
driver.check_schema_syntax(&get_schema, HttpMethod::GET)?;
driver.check_schema_syntax(&post_schema, HttpMethod::POST)?;
driver.check_schema_syntax(&put_schema, HttpMethod::PUT)?;
driver.check_schema_syntax(&delete_schema, HttpMethod::DELETE)?;
```

---

## 5. Common Schema Fields

### 5.1 Rivers Primitive Types

All drivers share the same set of primitive types for field definitions:

| Type | Description |
|---|---|
| `uuid` | UUID v4 string |
| `string` | UTF-8 string |
| `integer` | 64-bit signed integer |
| `float` | 64-bit float |
| `decimal` | Decimal number (string representation for precision) |
| `boolean` | true/false |
| `email` | String validated as email address |
| `phone` | String validated as phone number |
| `datetime` | ISO 8601 datetime string |
| `date` | ISO 8601 date string |
| `url` | String validated as URL |
| `json` | Arbitrary JSON value |
| `bytes` | Base64-encoded binary data |

### 5.2 Common Field Attributes

These attributes are available across all drivers unless the driver explicitly rejects them:

| Attribute | Type | Description |
|---|---|---|
| `required` | boolean | Field must be present |
| `default` | any | Default value when field is absent |
| `min` | number | Minimum numeric value |
| `max` | number | Maximum numeric value |
| `min_length` | integer | Minimum string length |
| `max_length` | integer | Maximum string length |
| `pattern` | string | Regex pattern for string validation |
| `enum` | array | Allowed values whitelist |

Drivers declare which attributes they support in their `check_schema_syntax`. An unsupported attribute on a field is a `SchemaSyntaxError`.

---

## 6. PostgreSQL Driver

### 6.1 Schema Shape

```json
{
  "driver": "postgresql",
  "type": "object",
  "fields": [
    { "name": "id",     "type": "uuid",    "required": true },
    { "name": "email",  "type": "email",   "required": true, "max_length": 255 },
    { "name": "amount", "type": "decimal",  "required": true, "min": 0 }
  ]
}
```

### 6.2 Supported Attributes

`required`, `default`, `min`, `max`, `min_length`, `max_length`, `pattern`, `enum`

### 6.3 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be `"object"` | All | `postgresql schemas must have type "object"` |
| `fields` array required | All | `postgresql schemas require a fields array` |
| `fields` must not be empty | GET | `GET schema must declare at least one field` |
| Each field must have `name` and `type` | All | `field missing name or type` |
| `type` must be a valid Rivers primitive | All | `unknown type "{type}" on field "{name}"` |
| `faker` attribute rejected | All | `"faker" is not supported by driver "postgresql"` |
| `key_pattern` rejected | All | `"key_pattern" is not supported by driver "postgresql"` |
| Query `$variables` must match parameter declarations | All | `query variable "$status" has no matching parameter` |
| Parameter declarations must match query `$variables` | All | `parameter "limit" has no matching $variable in query` |

### 6.4 Validator Rules

**Input (POST/PUT):**

| Rule | Error |
|---|---|
| Required field missing from input | `required field "{name}" is missing` |
| Field type mismatch | `field "{name}" expected {expected}, got {actual}` |
| Numeric value below `min` | `field "{name}" value {value} is below minimum {min}` |
| Numeric value above `max` | `field "{name}" value {value} exceeds maximum {max}` |
| String shorter than `min_length` | `field "{name}" length {len} below minimum {min_length}` |
| String longer than `max_length` | `field "{name}" length {len} exceeds maximum {max_length}` |
| String doesn't match `pattern` | `field "{name}" does not match pattern "{pattern}"` |
| Value not in `enum` list | `field "{name}" value "{value}" not in allowed values` |

**Output (GET):**

| Rule | Error |
|---|---|
| Required field missing from result | `required output field "{name}" missing from result` |
| Result has columns not in schema | Warning only — logged, not rejected (forward compatibility) |
| Field type cannot be coerced | `output field "{name}" cannot be coerced from {actual} to {expected}` |

### 6.5 Type Coercion (Output)

The Postgres driver applies output coercion before validation:

| Postgres type | Rivers type | Coercion |
|---|---|---|
| `int4`, `int8` | `integer` | Direct |
| `float4`, `float8` | `float` | Direct |
| `numeric` | `decimal` | String representation |
| `text`, `varchar` | `string` | Direct |
| `bool` | `boolean` | Direct |
| `uuid` | `uuid` | String |
| `timestamptz` | `datetime` | ISO 8601 format |
| `date` | `date` | ISO 8601 format |
| `jsonb` | `json` | Parsed |
| `bytea` | `bytes` | Base64 encoded |

---

## 7. MySQL Driver

### 7.1 Schema Shape

Identical to PostgreSQL. `"driver": "mysql"`.

### 7.2 Supported Attributes

Same as PostgreSQL: `required`, `default`, `min`, `max`, `min_length`, `max_length`, `pattern`, `enum`

### 7.3 SchemaSyntaxChecker Rules

Same as PostgreSQL, with one addition:

| Rule | Method | Error |
|---|---|---|
| Parameter style must be `?` not `$N` | All | Warning — MySQL uses positional `?` parameters |

### 7.4 Validator Rules

Same as PostgreSQL. Type coercion differs slightly for MySQL-specific types but follows the same pattern.

---

## 8. SQLite Driver

### 8.1 Schema Shape

Identical to PostgreSQL/MySQL. `"driver": "sqlite"`.

### 8.2 Supported Attributes

Same as PostgreSQL: `required`, `default`, `min`, `max`, `min_length`, `max_length`, `pattern`, `enum`

### 8.3 SchemaSyntaxChecker Rules

Same as PostgreSQL, with one addition:

| Rule | Method | Error |
|---|---|---|
| Named parameter prefix check | All | Parameters should use `:name`, `@name`, or `$name` style |

### 8.4 Validator Rules

Same as PostgreSQL. SQLite's dynamic typing means output coercion is more aggressive:

| SQLite affinity | Rivers type | Coercion |
|---|---|---|
| INTEGER | `integer` | Direct |
| REAL | `float` | Direct |
| TEXT (valid JSON) | `json` | Parsed |
| TEXT | `string` | Direct |
| TEXT (ISO 8601) | `datetime`/`date` | Pattern match |
| BLOB | `bytes` | Base64 encoded |
| NULL | any nullable field | `null` |

---

## 9. Redis Driver

### 9.1 Schema Shape — Varies By Structure Type

Redis has multiple data structures. The schema `type` determines the expected shape.

**Hash:**

```json
{
  "driver": "redis",
  "type": "hash",
  "key_pattern": "user:{user_id}",
  "fields": [
    { "name": "username", "type": "string", "required": true },
    { "name": "email",    "type": "email",  "required": true },
    { "name": "prefs",    "type": "json",   "required": false }
  ]
}
```

**String (simple KV):**

```json
{
  "driver": "redis",
  "type": "string",
  "key_pattern": "session:{session_id}",
  "value_type": "json"
}
```

**List:**

```json
{
  "driver": "redis",
  "type": "list",
  "key_pattern": "queue:{queue_name}",
  "element_type": "json"
}
```

**Set:**

```json
{
  "driver": "redis",
  "type": "set",
  "key_pattern": "tags:{item_id}",
  "element_type": "string"
}
```

**Sorted Set:**

```json
{
  "driver": "redis",
  "type": "sorted_set",
  "key_pattern": "leaderboard:{game_id}",
  "member_type": "string",
  "score_type": "float"
}
```

### 9.2 Supported Attributes

`required`, `default`, `min`, `max`, `min_length`, `max_length`, `pattern`, `enum`

Plus Redis-specific top-level fields: `key_pattern`, `value_type`, `element_type`, `member_type`, `score_type`

### 9.3 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be one of: `hash`, `string`, `list`, `set`, `sorted_set` | All | `unknown Redis type "{type}"` |
| `key_pattern` required | All | `Redis schemas require key_pattern` |
| `key_pattern` must contain `{variable}` placeholders | All | `key_pattern must contain at least one {variable}` |
| `{variable}` placeholders must match parameter declarations | All | `key_pattern variable "{var}" has no matching parameter` |
| `type: "hash"` requires `fields` array | GET, POST, PUT | `Redis hash schemas require fields` |
| `type: "string"` must not have `fields` | All | `Redis string schemas must not declare fields` |
| `type: "string"` requires `value_type` | All | `Redis string schemas require value_type` |
| `type: "list"` requires `element_type` | All | `Redis list schemas require element_type` |
| `type: "set"` requires `element_type` | All | `Redis set schemas require element_type` |
| `type: "sorted_set"` requires `member_type` and `score_type` | All | `Redis sorted_set requires member_type and score_type` |
| `faker` attribute rejected | All | `"faker" is not supported by driver "redis"` |

**Method-specific rules:**

| Rule | Method | Error |
|---|---|---|
| POST on `type: "hash"` requires at least one field | POST | `POST schema for Redis hash must have at least one field` |
| DELETE schema needs only `key_pattern` | DELETE | Warning if fields declared on DELETE — they are ignored |
| GET on `type: "string"` validates `value_type` | GET | `value_type must be a valid Rivers primitive` |

### 9.4 Validator Rules

**Input (POST/PUT):**

For `hash` type — validates field names, types, required fields against the schema, same as relational drivers.

For `string` type — validates the value matches `value_type`.

For `list`/`set` — validates the element matches `element_type`.

For `sorted_set` — validates member matches `member_type`, score matches `score_type`.

**Output (GET):**

Same rules applied to returned data. Redis returns strings — the validator coerces to the declared type:

| Redis return | Target type | Coercion |
|---|---|---|
| String (numeric) | `integer`/`float` | Parse |
| String (JSON) | `json` | Parse |
| String (ISO 8601) | `datetime` | Pattern match |
| String | `string`/`email`/`url` | Direct + format validation |
| `nil` | any nullable | `null` |

---

## 10. Memcached Driver

### 10.1 Schema Shape

```json
{
  "driver": "memcached",
  "type": "string",
  "key_pattern": "cache:{key}",
  "value_type": "json"
}
```

Memcached is simple KV — only `type: "string"` is valid.

### 10.2 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be `"string"` | All | `memcached only supports type "string"` |
| `key_pattern` required | All | `memcached schemas require key_pattern` |
| `value_type` required | All | `memcached schemas require value_type` |
| `fields` must not be present | All | `memcached does not support structured fields` |
| Only GET, POST, DELETE supported | PUT | `memcached does not support PUT — use POST for set` |

### 10.3 Validator Rules

Input: validates value matches `value_type`.  
Output: coerces string return to `value_type`.

---

## 11. Faker Driver

### 11.1 Schema Shape

```json
{
  "driver": "faker",
  "type": "object",
  "fields": [
    { "name": "id",         "type": "uuid",   "faker": "datatype.uuid",  "required": true },
    { "name": "first_name", "type": "string", "faker": "name.firstName", "required": true },
    { "name": "email",      "type": "email",  "faker": "internet.email", "required": true }
  ]
}
```

### 11.2 Supported Attributes

`required`, `faker` (required for each field), `unique`, `domain`

Standard validation attributes (`min`, `max`, `pattern`) are not used — faker generates, it doesn't validate input.

### 11.3 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be `"object"` | All | `faker schemas must have type "object"` |
| `fields` required | All | `faker schemas require fields` |
| Every field must have `faker` attribute | GET | `faker field "{name}" is missing the faker attribute` |
| `faker` value must be valid `category.method` notation | All | `unknown faker method "{method}" on field "{name}"` |
| Only GET supported | POST, PUT, DELETE | `faker driver is read-only — {method} schemas are not valid` |
| `min`/`max`/`pattern` attributes rejected | All | `"{attr}" is not supported by driver "faker" — faker generates, it does not validate` |

### 11.4 Validator Rules

**Output only** (faker is read-only):

The validator checks that generated records match the declared types. This is primarily a safety check on the faker generation engine — the generated `email` field should actually contain an email-formatted string.

| Rule | Error |
|---|---|
| Generated value doesn't match declared type | `faker generated "{value}" for field "{name}" but type is "{type}"` |
| Required field not generated | `faker failed to generate required field "{name}"` |

---

## 12. HTTP Driver

### 12.1 Schema Shape

```json
{
  "driver": "http",
  "type": "object",
  "content_type": "application/json",
  "fields": [
    { "name": "id",     "type": "uuid",   "required": true },
    { "name": "status", "type": "string", "required": true }
  ]
}
```

For streaming HTTP:

```json
{
  "driver": "http",
  "type": "stream_chunk",
  "content_type": "application/x-ndjson",
  "fields": [
    { "name": "token", "type": "string", "required": false },
    { "name": "done",  "type": "boolean", "required": false }
  ]
}
```

### 12.2 Supported Attributes

`required`, `default`, `min`, `max`, `min_length`, `max_length`, `pattern`, `enum`

Plus HTTP-specific: `content_type`

### 12.3 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be `"object"` or `"stream_chunk"` | All | `HTTP schemas must be type "object" or "stream_chunk"` |
| `type: "stream_chunk"` only valid on DataViews with `streaming = true` | GET | `stream_chunk type requires streaming = true on the DataView` |
| `content_type` must be recognized | All | `unknown content_type "{ct}" — expected application/json, application/x-ndjson, text/event-stream, application/xml` |
| `fields` required for `type: "object"` | All | `HTTP object schemas require fields` |
| Query `$variables` in URL path must match parameter declarations | All | `URL variable "{var}" has no matching parameter` |

**Method-specific:**

| Rule | Method | Error |
|---|---|---|
| POST/PUT schema validates request body | POST, PUT | Standard field validation |
| GET schema validates response body | GET | Standard field validation |
| DELETE may have no fields (URL params only) | DELETE | No error if fields absent |

### 12.4 Validator Rules

**Input (POST/PUT):** same as relational drivers — required fields, type checking, constraints.

**Output (GET):** validates the parsed response body against the schema. If `content_type` is `application/json`, the response is parsed as JSON and fields are validated. For streaming, each chunk is validated independently.

Non-JSON responses (XML, plain text) are validated against the `value_type` if the schema is `type: "string"`, or skipped if no field-level validation is possible.

---

## 13. Kafka Driver

### 13.1 Schema Shape

```json
{
  "driver": "kafka",
  "type": "message",
  "topic": "orders",
  "key": { "type": "uuid" },
  "value": {
    "fields": [
      { "name": "order_id",  "type": "uuid",     "required": true },
      { "name": "action",    "type": "string",   "required": true, "enum": ["created", "updated", "deleted"] },
      { "name": "timestamp", "type": "datetime", "required": true }
    ]
  }
}
```

### 13.2 Supported Attributes

On value fields: `required`, `default`, `min`, `max`, `min_length`, `max_length`, `pattern`, `enum`

Plus Kafka-specific: `topic`, `key`, `value` (container objects)

### 13.3 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be `"message"` | All | `kafka schemas must have type "message"` |
| `topic` required | All | `kafka schemas require topic` |
| `value` required | POST, GET | `kafka schemas require value definition` |
| `value.fields` required | POST, GET | `kafka value must declare fields` |
| `key` is optional — if present, must have `type` | All | `kafka key must declare a type` |
| PUT not supported | PUT | `kafka does not support PUT — messages are immutable, use POST to produce` |
| DELETE not supported | DELETE | `kafka does not support DELETE — messages are log-appended` |

**Method-specific:**

| Rule | Method | Error |
|---|---|---|
| POST schema validates produced message value | POST | `value.fields` validated against input |
| GET schema validates consumed message value | GET | `value.fields` validated against output |

### 13.4 Validator Rules

**Input (POST — produce):**
- `key` validated if declared (type check)
- `value.fields` validated same as relational fields (required, type, constraints)

**Output (GET — consume):**
- Consumed message value deserialized as JSON
- `value.fields` validated against deserialized message
- `key` validated if declared
- Missing required fields in consumed message → `ValidationError` (message rejected or logged, depending on `instance_ack`)

---

## 14. RabbitMQ Driver

### 14.1 Schema Shape

```json
{
  "driver": "rabbitmq",
  "type": "message",
  "exchange": "orders",
  "routing_key": "order.created",
  "value": {
    "fields": [
      { "name": "order_id", "type": "uuid",   "required": true },
      { "name": "amount",   "type": "decimal", "required": true }
    ]
  }
}
```

### 14.2 Supported Attributes

Same as Kafka for value fields. Plus RabbitMQ-specific: `exchange`, `routing_key`, `queue` (for GET/consume)

### 14.3 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be `"message"` | All | `rabbitmq schemas must have type "message"` |
| POST requires `exchange` | POST | `rabbitmq POST schemas require exchange` |
| GET requires `queue` | GET | `rabbitmq GET schemas require queue` |
| `value` required | POST, GET | `rabbitmq schemas require value definition` |
| `value.fields` required | POST, GET | `rabbitmq value must declare fields` |
| PUT not supported | PUT | `rabbitmq does not support PUT` |
| DELETE not supported | DELETE | `rabbitmq does not support DELETE` |

### 14.4 Validator Rules

Same as Kafka — validate `value.fields` against input (produce) or output (consume).

---

## 15. NATS Driver

### 15.1 Schema Shape

```json
{
  "driver": "nats",
  "type": "message",
  "subject": "orders.created",
  "value": {
    "fields": [
      { "name": "order_id", "type": "uuid",   "required": true },
      { "name": "payload",  "type": "json",   "required": true }
    ]
  }
}
```

### 15.2 Supported Attributes

Same as Kafka for value fields. Plus NATS-specific: `subject`

### 15.3 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be `"message"` | All | `nats schemas must have type "message"` |
| `subject` required | All | `nats schemas require subject` |
| `value` required | POST, GET | `nats schemas require value definition` |
| PUT not supported | PUT | `nats does not support PUT` |
| DELETE not supported | DELETE | `nats does not support DELETE` |

### 15.4 Validator Rules

Same as Kafka/RabbitMQ — validate `value.fields` against input/output.

---

## 16. EventBus Driver

### 16.1 Schema Shape

The EventBus driver is used when the DataView's datasource is `driver = "eventbus"`. It carries internal events.

```json
{
  "driver": "eventbus",
  "type": "event",
  "topic": "order.created",
  "value": {
    "fields": [
      { "name": "order_id", "type": "uuid",   "required": true },
      { "name": "action",   "type": "string", "required": true }
    ]
  }
}
```

### 16.2 SchemaSyntaxChecker Rules

| Rule | Method | Error |
|---|---|---|
| `type` must be `"event"` | All | `eventbus schemas must have type "event"` |
| `topic` required | All | `eventbus schemas require topic` |
| `value` required | POST, GET | `eventbus schemas require value definition` |
| PUT not supported | PUT | `eventbus does not support PUT` |
| DELETE not supported | DELETE | `eventbus does not support DELETE` |

### 16.3 Validator Rules

Same pattern as broker drivers — validate `value.fields` against published (POST) or received (GET) event payload.

---

## 17. Plugin Driver Requirements

### 17.1 Mandatory Implementation

All plugin drivers must implement `check_schema_syntax` and `validate`. A plugin that registers a driver without these methods will fail the trait contract at compile time.

### 17.2 Stub Pattern

Plugins using the honest stub pattern (`DriverError::Unsupported` on all operations) should still implement meaningful schema validation:

```rust
fn check_schema_syntax(
    &self,
    schema: &SchemaDefinition,
    _method: HttpMethod,
) -> Result<(), SchemaSyntaxError> {
    // Even stubs validate the basics
    if schema.driver != self.driver_type().as_str() {
        return Err(SchemaSyntaxError::DriverMismatch { ... });
    }
    // Stub drivers may accept any schema structure
    Ok(())
}

fn validate(
    &self,
    _data: &Value,
    _schema: &SchemaDefinition,
    _direction: ValidationDirection,
) -> Result<(), ValidationError> {
    Err(ValidationError::DriverNotImplemented {
        driver: self.driver_type().as_str().to_string()
    })
}
```

### 17.3 Plugin Schema Documentation

Plugin drivers should document their supported schema attributes, type mappings, and method restrictions in their crate README. The `check_schema_syntax` implementation is the authoritative source.

---

## 18. Error Catalog

### 18.1 SchemaSyntaxError

Returned at build/deploy time:

```rust
pub enum SchemaSyntaxError {
    DriverMismatch {
        expected: String,
        actual: String,
        schema_file: String,
    },
    MissingRequiredField {
        field: String,
        driver: String,
        schema_file: String,
    },
    UnsupportedAttribute {
        attribute: String,
        field: String,
        driver: String,
        supported: Vec<String>,
        schema_file: String,
    },
    UnsupportedType {
        schema_type: String,
        driver: String,
        supported: Vec<String>,
        schema_file: String,
    },
    UnsupportedMethod {
        method: String,
        driver: String,
        schema_file: String,
    },
    InvalidFieldType {
        field: String,
        field_type: String,
        schema_file: String,
    },
    OrphanVariable {
        variable: String,
        query: String,
        schema_file: String,
    },
    OrphanParameter {
        parameter: String,
        query: String,
        schema_file: String,
    },
    StructuralError {
        message: String,
        driver: String,
        schema_file: String,
    },
}
```

### 18.2 ValidationError

Returned at request time:

```rust
pub enum ValidationError {
    MissingRequired {
        field: String,
        direction: ValidationDirection,
    },
    TypeMismatch {
        field: String,
        expected: String,
        actual: String,
        direction: ValidationDirection,
    },
    ConstraintViolation {
        field: String,
        constraint: String,    // "min", "max", "pattern", "enum", "min_length", "max_length"
        value: String,
        limit: String,
        direction: ValidationDirection,
    },
    CoercionFailed {
        field: String,
        from_type: String,
        to_type: String,
        direction: ValidationDirection,
    },
    DriverNotImplemented {
        driver: String,
    },
}
```

### 18.3 Error Display

All errors include enough context to locate the problem without debugging:

```
SchemaSyntaxError: field "email" uses attribute "faker" which is not supported
  by driver "postgresql". Supported attributes: required, default, min, max,
  min_length, max_length, pattern, enum.
  → schema: schemas/contact.schema.json
  → dataview: list_contacts
  → datasource: orders_db (driver: postgresql)

ValidationError: required field "customer_id" is missing
  → direction: Input (POST)
  → dataview: create_order
  → trace_id: a1b2c3d4-...
```

---

## 19. Pseudo DataView Validation

### 19.1 Build-Time at `.build()`

When a handler calls `ctx.datasource("name").fromQuery(...).withPostSchema({...}).build()`, the `.build()` call triggers the SchemaSyntaxChecker against the inline schema. The handler discovers malformed schemas immediately, not when it tries to execute.

### 19.2 Runtime on Execute

When the built pseudo DataView is called with parameters, the Validator runs in both directions — same as declared DataViews. The pseudo DataView goes through the same pipeline.

### 19.3 Driver Routing

The inline schema's `driver` field determines which driver's checker and validator are used. If the `driver` field doesn't match the datasource's actual driver, `.build()` returns a `SchemaSyntaxError::DriverMismatch`.

```typescript
// This fails at .build() — datasource is redis but schema says postgresql
const bad = ctx.datasource("redis_cache")
    .fromQuery("...")
    .withGetSchema({ driver: "postgresql", ... })
    .build();  // SchemaSyntaxError: driver mismatch — datasource is "redis", schema declares "postgresql"
```

---

## Revision History

| Version | Date | Changes |
|---|---|---|
| 1.0 | 2026-03-16 | Initial specification — all drivers covered |
