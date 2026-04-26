# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# Operating Rules

## Standards — how you work

These govern everything below. If a workflow step ever conflicts with these, the standard wins.

1. **Read before you propose.** When a file grounds a recommendation, read it in full. Grep and partial views are for navigation, not structural decisions. If you skimmed instead of read, say so and list what you haven't verified.
2. **State your grounding before you propose.** Briefly list what's confirmed from source vs. inferred or assumed. Hidden assumptions are the enemy.
3. **Done means the deliverable is done, not that the response sounds complete.** Don't suggest stopping, deferring, or "picking this up later" as a way of handling work still in front of you. If you hit a real blocker (missing info, genuine ambiguity, a decision only I can make), name the specific blocker and the specific question. Fatigue, length, or difficulty are not blockers.
4. **Banned exit phrases:** "for now," "as a starting point," "we can iterate tomorrow," "let's leave it here," "we can refine later," "this is a good place to pause." If work is unfinished, keep working. If you need input, ask a specific question and wait.
5. **Operate at highest standards.** Take pride in the work. Shortcuts should feel embarrassing, not efficient. Push once more when something feels "good enough" — it usually isn't. Default to more thorough, not less.
6. **Push back honestly.** If my framing is wrong, scope is off, or I'm asking for something half-baked, say so directly. Agreement is not the goal; the best outcome is. Clear disagreement is more useful than compliance.
7. **Use the context window.** Long reads, multi-step reasoning, and extended work are expected. Don't truncate to fit an imagined budget.

## Workflow — what you do

1. **Think → Plan → Check → Execute.** Read relevant files in full (per Standard 1). Identify reuse candidates before proposing new code. Write the plan to `todo/tasks.md` with small, detailed tasks and validation steps per task. Before clearing `todo/tasks.md` with unfinished items, move them to `todo/gutter.md`.
2. **One pre-flight gate.** Share the plan, get approval, then execute head-down. After approval, the plan is the permission — no per-task check-ins. Only pause mid-execution if discovery invalidates the plan; when that happens, name the specific invalidation and the specific question (per Standard 3), don't vaguely stall.
3. **Mark tasks complete as you finish them** with a high-level explanation of what changed. No ceremonial status updates between tasks.
4. **Simplicity and reuse first — with a limit.** Reuse when an existing pattern fits without contortions. Build new when reuse requires bending the pattern out of shape. "Simplicity" is not an alibi for settling (see Standard 5).
5. **Log every decision to `changedecisionlog.md`**: file affected, what was decided, spec reference, resolution method. This is CB's reference baseline for drift detection — treat it as load-bearing, not bookkeeping.
6. **Log every change to `changelog.md`**: file affected, summary of what was done, spec reference, resolution method. Also feeds CB.
7. **Update docs when behavior, public API, or developer-facing interfaces change.** Training, tutorial, and AI guide docs follow reality. Pure internal refactors with no surface-level change don't require doc updates — but anything a user or downstream agent can observe does.
8. **Git commit per logical group of completed tasks** with a summary of changes. Commit message should let CB reconstruct intent without reading the diff.

## Project Overview

Rivers is a declarative app-service framework written in Rust. Applications are defined entirely through TOML configuration and JSON schemas — not custom code. The framework runtime (`riversd`) loads these configs to serve endpoints.

The Rust runtime is implemented across a workspace of crates under `crates/`.

## Manager your human

`?` in a message means "can you find out" — investigate only, SHOULD NOT make changes.
`??` in a message means confusion — please stop and explain, DO NOT move forward until there is an understanding.

## Build philosophy

- Build so things are reusable
- Collapse complex things down to simple
- Keep it simple
- always make sure canary is working no failures it is our production. 

## Versioning

**Format:** `MAJOR.MINOR.PATCH+HHMMDDMMYY` — standard SemVer 2.0 with a UTC
build-metadata stamp. Cargo accepts `+`-prefixed build metadata; some
operator-facing surfaces (riversd banner, `riversctl status`) display the
stamp with a `.` separator instead, but the canonical form in `Cargo.toml`
uses `+`.

**Build stamp components** (10 digits, UTC, all zero-padded 2-digit):

| Position | Field |
|----------|-------|
| 1–2      | hour (00–23) |
| 3–4      | minute (00–59) |
| 5–6      | day (01–31) |
| 7–8      | month (01–12) |
| 9–10     | year (last two digits) |

Example: a PR cut at 23:25 UTC on 26 April 2026 → `2325260426`. Workspace
`Cargo.toml` records `version = "0.55.0+2325260426"`.

**Bump rules — every PR must bump the workspace version:**

| Change kind | What bumps | Recipe |
|-------------|-----------|--------|
| Any PR (default — docs, config, dependency churn, build-system tweaks) | `+build` only | `just bump` |
| Code fix (bug fix in shipped code, tightening, refactor that changes runtime behavior) | `PATCH` and `+build` | `just bump-patch` |
| Major change (new feature, breaking config change, public API surface) | `MINOR` and `+build`; `PATCH` resets to 0 | `just bump-minor` |

The bump runs from the workspace root (`./scripts/bump-version.sh`). The
script computes the UTC stamp, edits the workspace `[package]` version
in place, and prints the old → new transition.

**CI enforcement:** `.github/workflows/version-check.yml` runs on every
PR targeting `main` and fails if `Cargo.toml`'s workspace version is
unchanged versus the base branch, or if the version has no `+build`
segment. Path-ignore list is intentionally tiny (`.gitignore`, `LICENSE`)
— almost every PR must bump.

**No squash-collapse:** the squash-merge commit on `main` carries the
final bumped version; intermediate commits within a PR may bump build
stamps multiple times (e.g. via rebase or force-push) and that's fine —
only the final state matters at merge.

**Bump cadence guidance:**

- A PR full of unrelated micro-fixes is still one PR — one `bump-patch`.
- A PR that only renames a private symbol or refactors comments is a `just bump` (build-only).
- A PR that adds a new datasource type, changes the bundle layout, or alters a handler-visible API is a `bump-minor`.
- When in doubt, prefer the lower bump. CI enforces *some* bump; reviewers can ask for a higher bump if the change warrants it.

## GOAL

pushing out feature

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
- **AppLogRouter** — Per-app log file routing. Handler logs (`Rivers.log`) write to `log/apps/<app>.log`. 10MB rotation.
- **Metrics** — Optional Prometheus exporter on port 9091. Feature-gated (`metrics` feature).

### Dual Build Modes

The codebase supports two build modes via the `Justfile`:

```
Static Mode (just build — default)
  riversd [single binary, ~80MB]
    Everything statically linked (rlib)

Dynamic Mode (just build-dynamic / cargo deploy)
  riversd              [thin binary, ~5-10MB]
  librivers_engine_v8.dylib   [cdylib]
  librivers_engine_wasm.dylib [cdylib]
  librivers_plugin_*.dylib    [12 cdylib plugins]
```

**Deploy:** `cargo deploy <path>` builds and assembles a complete instance with binaries, engine/plugin dylibs, TLS certs, lockbox, config (absolute paths), log dirs, and VERSION file.

### Key Crates

| Crate | Role |
|-------|------|
| `rivers-runtime` | Facade crate — re-exports core, config, driver-sdk, engine-sdk. Contains ProcessPool shared types, `home` module for config discovery, the 4-layer bundle validation pipeline (`validate_structural`, `validate_existence`, `validate_crossref`, `validate_syntax`, `validate_engine`, `validate_pipeline`, `validate_result`, `validate_format`), and the bundle module cache types (`module_cache::{CompiledModule, BundleModuleCache}`) populated at bundle load per `rivers-javascript-typescript-spec.md §3.4`. In static mode: rlib. In dynamic mode: dylib. |
| `riversd` | Server binary — HTTP server, routing, ProcessPool dispatch, engine loader, host callbacks, per-app logging, metrics. |
| `riversctl` | CLI tool — start/stop/status riversd, doctor (--fix/--lint), admin API, TLS management (gen/renew/show/expire). |
| `rivers-core` | Config types, DriverFactory, StorageEngine, LockBox, EventBus, AppLogRouter, TLS cert generation. |
| `rivers-core-config` | Config structs (ServerConfig, LoggingConfig, MetricsConfig, etc.) and StorageEngine trait. |
| `rivers-driver-sdk` | Driver traits (DatabaseDriver, Connection, Query, QueryResult). |
| `rivers-engine-sdk` | C-ABI contract for engine cdylibs (HostCallbacks, SerializedTaskContext). |
| `rivers-engine-v8` | V8 JavaScript engine (cdylib). |
| `rivers-engine-wasm` | Wasmtime WASM engine (cdylib). |
| `rivers-keystore-engine` | Application keystore — types, AES-256-GCM encrypt/decrypt, key management. |
| `rivers-keystore` | CLI tool — init, generate, list, info, delete, rotate app keystore keys. |
| `rivers-lockbox` | CLI tool — init, add, list, show, alias, rotate, remove, rekey, validate secrets. |
| `riverpackage` | CLI tool — init (scaffold), validate, preflight, pack bundles. |
| `cargo-deploy` | CLI tool — build and deploy Rivers instances (dynamic/static). |
| `rivers-plugin-*` | 12 driver plugins (cdylib each): cassandra, couchdb, elasticsearch, exec, influxdb, kafka, ldap, mongodb, nats, neo4j, rabbitmq, redis-streams. |

### Config Discovery

Binaries find `riversd.toml` without requiring a specific CWD:
1. `$RIVERS_HOME/config/riversd.toml`
2. `<binary>/../config/riversd.toml`
3. `./config/riversd.toml`
4. `/etc/rivers/riversd.toml`

Implementation: `crates/rivers-runtime/src/home.rs`

### Engine Loader

Engine dylib filenames are `librivers_engine_v8.dylib` and `librivers_engine_wasm.dylib` (not `librivers_v8`). The engine loader checks `_rivers_engine_abi_version` (different from plugin `_rivers_abi_version`). Plugins must be built with `--features plugin-exports` to export ABI symbols.

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

Scaffold a new bundle: `riverpackage init <name> [--driver faker|postgres|sqlite|mysql]`

### Key Config Conventions

- `[[datasources]]` uses TOML array-of-tables syntax
- `[[data.dataviews.*.parameters]]` uses array-of-tables with explicit `name` field (not named subtables)
- Views use `[api.views.*]` prefix (not `[views.*]` — that silently fails)
- Parameter mapping uses `[api.views.*.parameter_mapping.query]` and `.path` subtables
- Cache config uses `ttl_seconds` (integer), not `ttl`
- Schema attribute key for faker driver is `"faker"` (not `"faker_type"`)
- UUIDs for `appId` must be stable — generate once, never regenerate
- Per-app logging: `[base.logging] app_log_dir = "/path/to/log/apps"`
- Metrics: `[metrics] enabled = true` (port 9091 default)
- Engine/plugin dirs: `[engines] dir = "/path/to/lib"`, `[plugins] dir = "/path/to/plugins"`
- All paths in deployed config are absolute (generated by `cargo deploy`)

### CLI Quick Reference

```bash
# Scaffold + deploy
riverpackage init my-app --driver faker
cargo deploy /opt/rivers

# Lifecycle
riversctl doctor --fix          # health check + auto-repair
riversctl start                  # daemon (writes PID to run/riversd.pid)
riversctl start --foreground     # interactive/systemd
riversctl status                 # show running state
riversctl stop                   # SIGTERM + wait 30s

# Bundle validation
riverpackage validate <bundle_dir>                    # text output (4-layer pipeline)
riverpackage validate <bundle_dir> --format json      # JSON output
riverpackage validate <bundle_dir> --config <path>    # specify config for engine discovery

# TLS
riversctl tls gen                # generate self-signed cert
riversctl tls renew              # regenerate cert
riversctl tls show               # display cert info
```

## Reference Implementation: Address Book Bundle

Two apps in `address-book-bundle/`:

- **address-book-service** (port 9100) — REST API using faker datasource for synthetic contacts. 4 DataViews, 4 endpoints, no auth.
- **address-book-main** (port 8080) — Svelte SPA with HTTP datasource proxying to address-book-service. Static file serving with SPA fallback.

## Specifications

All specs live in `docs/arch/`. Key documents by topic:

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
| Logging & observability | `rivers-logging-spec.md` |
| Service registry | `rivers-rps-spec-v2.md` |
| WASM runtime | `rivers-processpool-runtime-spec-v2.md` |
| Internal storage | `rivers-storage-engine-spec.md` |
| Feature inventory | `rivers-feature-inventory.md` |

## Tutorials

Key tutorials in `docs/guide/tutorials/`:

| Tutorial | File |
|----------|------|
| Getting started (zero to running) | `tutorial-getting-started.md` |
| JavaScript handlers | `tutorial-js-handlers.md` |
| Per-app logging | `tutorial-per-app-logging.md` |
| Prometheus metrics | `tutorial-metrics.md` |
| Faker datasource | `datasource-faker.md` |
| PostgreSQL datasource | `datasource-postgresql.md` |

## Test Infrastructure

Integration tests run against a Podman cluster on **192.168.2.161** (CentOS Stream 9, 128GB RAM). Full details in `sec/test-infrastructure.md`.

| Service | IPs | Port | Credentials |
|---------|-----|------|-------------|
| PostgreSQL (primary + 2 replicas) | .209-.211 | 5432 | `rivers` / `rivers_test` / db: `rivers` |
| MySQL (InnoDB Cluster, 3 nodes) | .215-.217 | 3306 | `rivers` / `rivers_test` / db: `rivers` |
| Redis (Cluster, 3 nodes) | .206-.208 | 6379 | `rivers_test` |
| MongoDB (Replica Set `rivers-rs`, 3 nodes) | .212-.214 | 27017 | `rivers` / `rivers_test` |
| Elasticsearch (Cluster `rivers-es`, 3 nodes) | .218-.220 | 9200 | security disabled |
| CouchDB (Cluster, 3 nodes) | .221-.223 | 5984 | `rivers` / `rivers_test` |
| Cassandra (Ring `rivers`, 3 nodes) | .224-.226 | 9042 | — |
| Kafka (3 brokers) | .203-.205 | 9092 | — |
| Zookeeper (3 nodes) | .200-.202 | 2181 | — |
| LDAP (single node) | .227 | 389 | `cn=admin,dc=rivers,dc=test` / `rivers_test` |

**27 containers, 9 clusters + 1 standalone.** All IPs on `192.168.2.x` subnet via macvlan.

Quick verification:
```bash
curl -s http://192.168.2.218:9200/_cluster/health?pretty    # ES
psql -h 192.168.2.209 -U rivers -d rivers -c "SELECT 1"    # PostgreSQL
redis-cli -h 192.168.2.206 -a rivers_test cluster info      # Redis
```

### Default Ports (riversd)

| Port | Purpose |
|------|---------|
| 8080 | Main HTTP/HTTPS server (configurable via `[base] port`) |
| 9090 | Admin API (configurable via `[base.admin_api] port`) |
| 9091 | Prometheus metrics exporter (configurable via `[metrics] port`) |

## Tracking

- `todo/tasks.md` — Current work items
- `todo/changelog.md` — All decisions, gaps, and amendments across rounds
- `bugs/` — Bug reports with root cause analysis
- `docs/dreams/` — Project reflection documents
- `docs/superpowers/plans/` — Implementation plans
- `sec/test-infrastructure.md` — Full test cluster details, connection strings, container management
