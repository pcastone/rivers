# Program Review Tasks
## Circuit Breaker — App-Level Manual Control (v1)

- [x] Define config schema: `circuitBreakerId` optional attribute on DataView in `app.toml`
- [x] Breaker registry: app-scoped registration of breaker IDs at bundle load time (unique key = `appId:breakerId`)
- [x] DataView dispatch: when a breaker is tripped, all DataViews sharing that ID return a circuit-open error response (503)
- [x] `riversctl` CLI: `breaker` subcommand with `--list`, `--name=<breakerId>`, `--trip`, `--reset`
- [x] Admin API endpoints: GET/POST `/admin/breakers[/:id[/trip|/reset]]`
- [x] Validation: CB001 warning if a DataView references a solo `circuitBreakerId` with Levenshtein suggestion
- [x] Documentation: circuit breaker tutorial at `docs/guide/tutorials/tutorial-circuit-breakers.md`

## Circuit Breaker — Auto-Trip (v2, future)

- [ ] Threshold-based auto-tripping (failure count/rate within time window)
- [ ] Config for trip thresholds, recovery strategy, half-open probing
- [ ] Spec: `mode = "auto" | "manual" | "both"` on breaker config

## cargo-deploy

- [x] Copy `docs/arch` alongside `docs/guide` into deployed instance — arch specs are already public via `dist-source`, no reason to omit them from deploy

## riverpackage

- [x] Remove `hex` dependency — replace `hex::encode(Sha256::digest(&bytes))` with `format!("{:x}", Sha256::digest(&bytes))` in `main.rs:443`

## Schema-to-Database Validation at Startup

- [x] `supports_introspection()` on DatabaseDriver trait — SQL drivers return true, others false
- [x] `column_names: Option<Vec<String>>` added to QueryResult — populated by SQL drivers on empty results
- [x] Postgres/MySQL/SQLite implement introspection via LIMIT 0 + column metadata
- [x] `introspect = false` opt-out on datasource config (defaults to true)
- [x] Hard fail at startup — queries validated via LIMIT 0, errors collected with Levenshtein suggestions
- [x] `schema_introspection` module with `check_fields_against_columns()` and error formatting
- [ ] Schema field-to-column comparison (pending schema file loading integration)

## Transactions — Request-Scoped, Handler-Driven

- [x] Add `begin()`, `commit()`, `rollback()` to `Connection` trait in `rivers-driver-sdk`
- [x] Implement for postgres, mysql, sqlite, mongodb, neo4j drivers — non-transactional drivers inherit default `Unsupported`
- [x] PoolGuard returns connections to idle on drop (preserves prepared statement caches)
- [x] TransactionMap for per-request transaction state with auto-rollback
- [x] Expose `Rivers.db.begin()`, `Rivers.db.commit()`, `Rivers.db.rollback()` in ProcessPool host callbacks + V8 bindings
- [x] Transaction-aware DataView engine — uses transaction connection when active

## Prepared Statements

- [x] Add `prepare()` / `execute_prepared()` / `has_prepared()` to `Connection` trait
- [x] `prepared = true` config-driven on DataView — transparent to handlers
- [x] Pool behavior: PoolGuard returns connections to idle on drop (reuse)
- [x] No handler API needed — config-driven, no `Rivers.db.prepare()` callback

## Batch Operations

- [x] `Rivers.db.batch()` host callback + V8 binding
- [x] Single-DataView bulk execution with multiple parameter sets
- [x] Inherits transaction state — uses active connection if transaction open

## Test Documentation — Plugin Crates

Add `///` doc comments to every `#[test]` / `#[tokio::test]` function explaining what the test validates. One line per test — what it proves, not what it does.

- [x] rivers-plugin-mongodb (2 tests: connect_and_ping, insert_find_delete_roundtrip)
- [x] rivers-plugin-elasticsearch (2 tests: connect_and_ping, index_search_delete_roundtrip)
- [x] rivers-plugin-kafka (1 test: produce_and_consume)
- [x] rivers-plugin-rabbitmq
- [x] rivers-plugin-nats
- [x] rivers-plugin-redis-streams
- [x] rivers-plugin-cassandra
- [x] rivers-plugin-couchdb
- [x] rivers-plugin-influxdb
- [x] rivers-plugin-ldap
- [x] rivers-plugin-neo4j (2 tests: connect_and_ping, create_query_delete_roundtrip)
- [x] rivers-plugin-exec

## Gap: Specs Needed Before Implementation

- [x] Write `docs/arch/rivers-circuit-breaker-spec.md` — config schema, breaker registry, admin API, CLI interface, DataView dispatch behavior, error responses
- [x] Write `docs/arch/rivers-connection-features-spec.md` — unified spec for Transactions + Prepared Statements + Batch Operations, covering Connection trait changes, PoolGuard behavior, ProcessPool host callback API design
- [x] Write `docs/arch/rivers-schema-introspection-spec.md` — schema-to-database validation, per-driver introspection strategy, error messaging, startup behavior

## Gap: Batch Operations Depends on Transactions

- [x] Implemented as one coordinated effort — Connection trait, PoolGuard, and host callbacks built together

## Gap: Schema Validation Plugin Coverage

- [ ] Define introspection strategy for each plugin driver beyond postgres/mysql/mongodb:
  - Cassandra (`system_schema.columns`)
  - Elasticsearch (index mappings API)
  - InfluxDB (measurements)
  - CouchDB (schemaless — skip or sample-based)
  - Redis (key-type check only)
  - LDAP (schema subentry)

## Gap: ProcessPool Host Callback API Design

- [x] Implemented: `Rivers.db.begin(datasource)`, `Rivers.db.commit(datasource)`, `Rivers.db.rollback(datasource)`, `Rivers.db.batch(dataview, params)` — implicit per-datasource transactions, no tokens. Host callbacks in riversd + V8 bindings in rivers-engine-v8. No `Rivers.db.prepare()` — prepared statements are config-driven.

## Gap: Integration/Canary Tests for New Features

- [ ] Circuit Breaker v1 — add canary test profile or test app that exercises trip/reset/status via admin API
- [ ] Transactions — add canary tests that verify begin/commit/rollback across datasources
- [ ] Schema-to-Database Validation — add canary test that verifies startup failure on schema mismatch

## Gap: neo4j Plugin

- [x] Write live integration tests for rivers-plugin-neo4j (connect_and_ping, create_query_delete_roundtrip)
- [x] Neo4j container deployed: `192.168.2.240:7687` (Bolt), auth `neo4j/rivers_test`, lockbox entry at `sec/lockbox/entries/neo4j/`
- [x] Test documentation included in test file

## Gap: Circuit Breaker v1 — Implementation Fixes

- [x] Per-app breaker scoping: registry namespaced by `appId:breakerId`, admin API routes updated to `/admin/apps/:app_id/breakers/...`
- [ ] Persistence integration test: trip a breaker, restart riversd, verify breaker is still open via admin API (spec §8)
- [ ] Canary test profile: add circuit breaker to a canary DataView, test trip/reset/status via admin API during canary run

## Gap: riversd.toml Foreign Attribute Protection

- [x] Unknown key warnings at startup — `check_unknown_config_keys()` walks top-level and `[base]` section, logs warnings with Levenshtein suggestions. Wired into `load_server_config()` in rivers-runtime

## Riverbed HTTPD — Future Consideration

See `todo/RiverbedPlan.md` for full build plan with validation tasks.

## riverpackage validation (rivers-runtime)

- [x] Allow foreign/unknown attributes in bundle TOML files during `riverpackage validate` — S002 downgraded from error to warning

## riversd (Gate 2 validation gap)

- [x] Added Layer 1 structural validation at Gate 2 — `validate_structural()` now called in `load.rs` before `validate_bundle()`
