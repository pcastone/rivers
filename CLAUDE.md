# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Workflow

1. **Think-Plan-Check-Execute**: Read the codebase for relevant files, look for reuse vs create, then write a plan to `todo/tasks.md` with small, detailed tasks and validation steps. If clearing `todo/tasks.md` with unfinished tasks, copy them to `todo/gutter.md` first.
2. **Check in before executing** — share the plan and get approval before starting work.
3. **Mark tasks complete as you go** and give high-level explanations of changes at each step.
4. **Simplicity first** — every change should impact as little code as possible. Reuse existing code, scripts, and patterns before creating new ones.
5. **Log every decision** in `changedecisionlog.md` with: file affected, what was decided, spec reference, and resolution method.
6. **Log high level changes** in `changedecisionlog.md` with: file affected, what was decided, spec reference, and resolution method. 
7. **Git commit** after each logical group of completed tasks with a summary of changes.

## Project Overview

Rivers is a declarative app-service framework written in Rust. Applications are defined entirely through TOML configuration and JSON schemas — not custom code. The framework runtime (`riversd`) loads these configs to serve endpoints.

The Rust runtime is implemented across a workspace of crates under `crates/`.

## Manager you human 
? in a message mean can you find out, SHOULD NOT make changes investigate only.  
?? in a message mean confusion, please stop and explain DO NOT move forward until there is an understanding. 

## Build philosophy
- build so things are resuable 
- Collaspse complex things down to simple 
- keep it simple

## GOAL 
The current goals of this sprint
- create smaller binary 
- identifiy and replace libraries not needed
- follow are build philosophy 
## Architecture

Rivers uses layered isolation with clear boundaries:

- **View Layer** — REST routes, WebSocket, SSE, MessageConsumer handlers with middleware pipeline (auth, rate limiting, compression). Views map HTTP endpoints to DataViews.
- **DataView Engine** — Named, parameterized queries with caching, pagination, and filtering. Resolves to a datasource via the Pool Manager at dispatch time.
- **Pool Manager** — Per-datasource connection pooling with circuit breaker and health checks.
- **Driver Layer** — Three independent driver contracts:
  - `DatabaseDriver` — request/response (PostgreSQL, MySQL, SQLite, Redis, Elasticsearch, Faker)
  - `MessageBrokerDriver` — continuous push (Kafka, RabbitMQ, NATS)
  - `HttpDriver` — HTTP/HTTP2/WebSocket/SSE as a first-class datasource
- **StorageEngine** — Internal KV + queue infrastructure for DataView caching and message buffering.
- **LockBox** — Age-encrypted local keystore for secrets. Resolved at startup, never enters ProcessPool.
- **Application Keystore** — Per-app AES-256-GCM encryption keys. Master key from LockBox. Exposed to handlers via `Rivers.keystore` and `Rivers.crypto.encrypt/decrypt`. Key bytes stay in Rust memory.
- **ProcessPool** — V8 + WASM CodeComponent execution for custom business logic.
- **RPS** — Service registry for inter-app discovery and dependency resolution.

### Dual Build Modes

The codebase supports two build modes via the `Justfile`:

```
Static Mode (just build — default)
  riversd [single binary, ~80MB]
    Everything statically linked (rlib)

Dynamic Mode (just build-dynamic)
  riversd              [thin binary, ~5-10MB]
  librivers_runtime.dylib  [shared runtime, ~25MB]  ← THE one Rust dylib
  librivers_engine_v8.dylib   [cdylib]
  librivers_engine_wasm.dylib [cdylib]
  librivers_plugin_*.dylib    [10 cdylib plugins]
```

**One dylib rule**: `rivers-runtime` is the sole Rust `dylib` crate. Engines and plugins are `cdylib` (self-contained, loaded via `libloading`).

### Key Crates

| Crate | Role |
|-------|------|
| `rivers-runtime` | Facade crate — re-exports core, config, driver-sdk, engine-sdk. Contains ProcessPool shared types. In static mode: rlib. In dynamic mode: dylib. |
| `riversd` | Server binary — HTTP server, routing, ProcessPool dispatch, engine loader, host callbacks. |
| `riversctl` | CLI tool — start/stop riversd, health checks, admin API, TLS management. |
| `rivers-core` | Config types, DriverFactory, StorageEngine, LockBox, EventBus. |
| `rivers-core-config` | Config structs (ServerConfig, etc.) and StorageEngine trait. |
| `rivers-driver-sdk` | Driver traits (DatabaseDriver, Connection, Query, QueryResult). |
| `rivers-engine-sdk` | C-ABI contract for engine cdylibs (HostCallbacks, SerializedTaskContext). |
| `rivers-engine-v8` | V8 JavaScript engine (cdylib). |
| `rivers-engine-wasm` | Wasmtime WASM engine (cdylib). |
| `rivers-keystore-engine` | Application keystore — types, AES-256-GCM encrypt/decrypt, key management. |
| `rivers-keystore` | CLI tool — init, generate, list, info, delete, rotate app keystore keys. |
| `rivers-plugin-*` | 10 driver plugins (cdylib each). |

### Bundle Structure

A Rivers app is packaged as a **bundle** containing one or more apps:

```
bundle/
├── manifest.toml              # Bundle metadata, lists apps
├── app-name/
│   ├── manifest.toml          # App metadata (appId UUID, type, port)
│   ├── resources.toml         # Datasources, services, dependencies
│   ├── app.toml               # DataViews, Views, static file config
│   ├── schemas/               # JSON schema files with driver-specific attributes
│   └── libraries/             # Static assets (SPA builds, etc.)
```

### Key Config Conventions

- `[[datasources]]` uses TOML array-of-tables syntax
- `[[data.dataviews.*.parameters]]` uses array-of-tables with explicit `name` field (not named subtables)
- Views use `[api.views.*]` prefix (not `[views.*]` — that silently fails)
- Parameter mapping uses `[api.views.*.parameter_mapping.query]` and `.path` subtables
- Cache config uses `ttl_seconds` (integer), not `ttl`
- Schema attribute key for faker driver is `"faker"` (not `"faker_type"`)
- UUIDs for `appId` must be stable — generate once, never regenerate

## Reference Implementation: Address Book Bundle

Two apps in `address-book-bundle/`:

- **address-book-service** (port 9100) — REST API using faker datasource for synthetic contacts. 4 DataViews, 4 endpoints, no auth.
- **address-book-main** (port 8080) — Svelte SPA with HTTP datasource proxying to address-book-service. Static file serving with SPA fallback.

## Specifications

All specs live in `docs/`. Key documents by topic:

| Topic | File |
|-------|------|
| App bundling & lifecycle | `rivers-application-spec.md` |
| Address book build spec | `rivers-address-book-spec-v1.3.md` |
| Routes, handlers, pipeline | `rivers-view-layer-spec.md` |
| Datasources, drivers, DataViews | `rivers-data-layer-spec.md` |
| Driver contracts | `rivers-driver-spec.md` |
| HTTP datasource driver | `rivers-http-driver-spec.md` |
| HTTP server, TLS, CORS | `rivers-httpd-spec.md` |
| Auth & sessions | `rivers-auth-session-spec.md` |
| Schema system | `rivers-schema-spec-v2.md` |
| Secrets management | `rivers-lockbox-spec.md` |
| App keystore & encryption | `rivers-feature-request-app-keystore.md` |
| Logging & observability | `rivers-logging-spec.md` |
| Service registry | `rivers-rps-spec-v2.md` |
| WASM runtime | `rivers-processpool-runtime-spec-v2.md` |
| Internal storage | `rivers-storage-engine-spec.md` |

Pending amendments are tracked in `docs/NEXT-SESSION-HANDOFF.md`.

## Tracking

- `todo/tasks.md` — Current work items
- `todo/changelog.md` — All decisions, gaps, and amendments across rounds
