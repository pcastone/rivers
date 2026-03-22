# Rivers

Rivers is a declarative app-service framework written in Rust. Define REST APIs, WebSocket endpoints, SSE streams, and more using only TOML configuration and JSON schemas — no application code required.

The runtime (`riversd`) loads your configuration at startup and serves fully functional endpoints backed by any supported datasource.

## Features

- **Zero application code** — endpoints, datasources, caching, auth, and middleware are all declared in TOML
- **Multi-driver datasource layer** — PostgreSQL, MySQL, SQLite, Redis, MongoDB, Elasticsearch, Cassandra, CouchDB, InfluxDB, Kafka, RabbitMQ, NATS, LDAP, HTTP, and a built-in Faker driver for synthetic data
- **DataView engine** — named, parameterized queries with built-in caching, pagination, and filtering
- **Connection pooling** — per-datasource pools with circuit breaker and health checks
- **LockBox secrets** — Age-encrypted local keystore, resolved at startup, never exposed to user code
- **ProcessPool** — V8 JavaScript and WASM engines for custom business logic when you need it
- **Static & dynamic builds** — single ~80MB binary or a thin binary + shared libraries and hot-loadable plugins
- **Bundle packaging** — deploy one or many apps as a self-contained bundle

## Quick Start

### Prerequisites

- Rust 1.75+ (edition 2021)
- [just](https://github.com/casey/just) command runner (recommended)

### Build

```bash
# Clone the repository
git clone https://github.com/AquaFlare/rivers.pub.git
cd rivers.pub

# Static build (single binary)
just build

# Or with cargo directly
cargo build --release
```

The binary is output to `target/release/riversd`.

### Run the Example Bundle

Rivers ships with an **address-book** example bundle that uses the Faker driver to generate synthetic contact data — no database required.

```bash
# Start the server with the example bundle
target/release/riversd --bundle address-book-bundle
```

Then hit the API:

```bash
# List contacts
curl http://localhost:9100/contacts

# Get a single contact
curl http://localhost:9100/contacts/{id}

# Search contacts
curl http://localhost:9100/contacts/search?q=John

# Filter by city
curl http://localhost:9100/contacts/city/Portland
```

## How It Works

A Rivers application is a **bundle** — a directory of TOML config files and JSON schemas:

```
my-bundle/
├── manifest.toml              # Bundle metadata
├── my-app/
│   ├── manifest.toml          # App metadata (name, port, appId)
│   ├── resources.toml         # Datasource declarations
│   ├── app.toml               # DataViews, Views, middleware
│   └── schemas/               # JSON schema files
```

### 1. Declare a datasource

```toml
# resources.toml
[[datasources]]
name       = "contacts"
driver     = "faker"
nopassword = true
required   = true
```

### 2. Define a DataView (query)

```toml
# app.toml
[data.dataviews.list_contacts]
name       = "list_contacts"
datasource = "contacts"
query      = "schemas/contact.schema.json"

[data.dataviews.list_contacts.caching]
ttl_seconds = 60

[[data.dataviews.list_contacts.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20
```

### 3. Map it to an endpoint

```toml
# app.toml
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
limit = "limit"
```

That's it. No controller code, no ORM, no boilerplate. `riversd` loads the bundle and serves the endpoint.

## Architecture

```
┌──────────────────────────────────────────────────┐
│  View Layer  (REST, WebSocket, SSE, Consumers)   │
│  ── middleware pipeline: auth, rate limit, gzip ──│
├──────────────────────────────────────────────────┤
│  DataView Engine  (query, cache, paginate)       │
├──────────────────────────────────────────────────┤
│  Pool Manager  (per-datasource, circuit breaker) │
├───────────┬──────────────┬───────────────────────┤
│ Database  │ MessageBroker│   HTTP Driver          │
│ Driver    │ Driver       │   (upstream proxy)     │
│ PG,MySQL, │ Kafka,AMQP,  │                        │
│ SQLite,...│ NATS          │                        │
└───────────┴──────────────┴───────────────────────┘
```

### Workspace Crates

| Crate | Role |
|-------|------|
| `riversd` | Server binary — HTTP server, routing, engine loader |
| `riversctl` | CLI — start/stop, health checks, admin API |
| `rivers-runtime` | Facade crate — re-exports core, config, driver-sdk, engine-sdk |
| `rivers-core` | Config types, DriverFactory, StorageEngine, LockBox, EventBus |
| `rivers-driver-sdk` | Driver traits (DatabaseDriver, Connection, Query, QueryResult) |
| `rivers-engine-sdk` | C-ABI contract for engine plugins |
| `rivers-engine-v8` | V8 JavaScript engine (cdylib) |
| `rivers-engine-wasm` | Wasmtime WASM engine (cdylib) |
| `rivers-plugin-*` | 10 datasource driver plugins (cdylib each) |

## Build Modes

### Static (default)

```bash
just build
```

Produces a single monolithic binary with everything statically linked.

### Dynamic

```bash
just build-dynamic
```

Produces a thin binary + shared runtime + hot-loadable engine and plugin cdylibs:

```
release/dynamic/
├── bin/riversd              # Thin binary (~5-10MB)
├── lib/librivers_runtime.dylib
├── lib/librivers_engine_v8.dylib
├── lib/librivers_engine_wasm.dylib
└── plugins/librivers_plugin_*.dylib
```

## Supported Drivers

| Driver | Type | Protocol |
|--------|------|----------|
| PostgreSQL | Database | TCP |
| MySQL | Database | TCP |
| SQLite | Database | File |
| Redis | Database | TCP |
| MongoDB | Database | TCP |
| Elasticsearch | Database | HTTP |
| Cassandra | Database | TCP |
| CouchDB | Database | HTTP |
| InfluxDB | Database | HTTP |
| LDAP | Database | TCP |
| Kafka | Message Broker | TCP |
| RabbitMQ | Message Broker | AMQP |
| NATS | Message Broker | TCP |
| Redis Streams | Message Broker | TCP |
| HTTP | Upstream Proxy | HTTP/2, WebSocket, SSE |
| Faker | Synthetic Data | In-process |

## Running Tests

```bash
# Unit tests
just test

# Live integration tests require infrastructure.
# Set env vars to point at your test services:
export RIVERS_TEST_PG_HOST=localhost
export RIVERS_TEST_REDIS_HOST=localhost
export RIVERS_TEST_MYSQL_HOST=localhost
# ... etc. All default to localhost.
```

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
