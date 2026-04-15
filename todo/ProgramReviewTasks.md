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

- [ ] After pool creation, introspect actual table/collection columns from the database (e.g., `information_schema.columns` for postgres/mysql, collection sample for mongodb)
- [ ] Compare DataView schema field names against actual database columns — flag mismatches with "did you mean?" suggestions (Levenshtein, like bundle validation)
- [ ] Compare DataView schema field types against actual column types — warn on type mismatches (e.g., schema says `integer` but column is `varchar`)
- [ ] Validate per-method query references — ensure `SELECT` / `INSERT` / `UPDATE` column names exist in the target table
- [ ] Hard fail on mismatch — refuse to start with detailed error messages that clearly explain the problem (e.g., "DataView 'search_orders' field 'orderDate2' not found in table 'orders' — available columns: id, warehouseId, orderDate, locCode, qty")
- [ ] Skip introspection for drivers that don't support it (faker, exec, http) — gate behind a `Driver` trait method like `supports_schema_introspection()`

## Transactions — Request-Scoped, Handler-Driven

- [ ] Add `begin()`, `commit()`, `rollback()` to `Connection` trait in `rivers-driver-sdk`
- [ ] Implement for postgres, mysql, sqlite drivers — non-transactional drivers (faker, memcached, http) reject with clear error
- [ ] Connection hold: `PoolGuard` must hold connection for full handler lifetime when transaction is active, not release after single query
- [ ] Auto-rollback on request timeout or handler panic (PoolGuard drop when transaction is open)
- [ ] Expose `Rivers.db.begin()`, `Rivers.db.commit()`, `Rivers.db.rollback()` in ProcessPool host callbacks for JS/WASM handlers
- [ ] Multi-datasource: each datasource gets its own transaction, handler coordinates commit/rollback order

## Prepared Statements

- [ ] Add `prepare()` / `execute_prepared()` to `Connection` trait
- [ ] Implement for postgres, mysql, sqlite — lazy prepare on first use per connection
- [ ] Pool behavior: prefer returning connections to idle (reuse) over dropping, to preserve prepared statement cache
- [ ] Expose `Rivers.db.prepare()` in ProcessPool host callbacks

## Batch Operations

- [ ] Add `execute_batch()` to `Connection` trait returning `Vec<QueryResult>`
- [ ] Implement for postgres, mysql, sqlite
- [ ] Expose `Rivers.db.batch()` in ProcessPool host callbacks

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
- [ ] rivers-plugin-neo4j (no tests exist yet)
- [x] rivers-plugin-exec

## Gap: Specs Needed Before Implementation

- [x] Write `docs/arch/rivers-circuit-breaker-spec.md` — config schema, breaker registry, admin API, CLI interface, DataView dispatch behavior, error responses
- [x] Write `docs/arch/rivers-connection-features-spec.md` — unified spec for Transactions + Prepared Statements + Batch Operations, covering Connection trait changes, PoolGuard behavior, ProcessPool host callback API design
- [x] Write `docs/arch/rivers-schema-introspection-spec.md` — schema-to-database validation, per-driver introspection strategy, error messaging, startup behavior

## Gap: Batch Operations Depends on Transactions

- [ ] Plan Transactions + Prepared Statements + Batch as one coordinated implementation — they share PoolGuard connection-hold changes and Connection trait modifications

## Gap: Schema Validation Plugin Coverage

- [ ] Define introspection strategy for each plugin driver beyond postgres/mysql/mongodb:
  - Cassandra (`system_schema.columns`)
  - Elasticsearch (index mappings API)
  - InfluxDB (measurements)
  - CouchDB (schemaless — skip or sample-based)
  - Redis (key-type check only)
  - LDAP (schema subentry)

## Gap: ProcessPool Host Callback API Design

- [ ] Design the JS/WASM API for `Rivers.db.begin()`, `Rivers.db.commit()`, `Rivers.db.rollback()`, `Rivers.db.prepare()`, `Rivers.db.batch()` — what does begin() return? How does a handler reference a specific datasource's transaction? Document in the connection-features spec.

## Gap: Integration/Canary Tests for New Features

- [ ] Circuit Breaker v1 — add canary test profile or test app that exercises trip/reset/status via admin API
- [ ] Transactions — add canary tests that verify begin/commit/rollback across datasources
- [ ] Schema-to-Database Validation — add canary test that verifies startup failure on schema mismatch

## Gap: neo4j Plugin

- [ ] Write live integration tests for rivers-plugin-neo4j (connect_and_ping, create_node_query_delete_roundtrip)
- [ ] Add neo4j container to test infrastructure (192.168.2.x subnet) or document that existing neo4j node is available
- [ ] Add test documentation once tests exist

## Gap: Circuit Breaker v1 — Implementation Fixes

- [ ] Per-app breaker scoping: registry is currently global on AppContext — two apps defining a breaker called `"cache"` would collide. Namespace breaker IDs by app (e.g., `appId:breakerId` as key) and restore `{appId}` path parameter in admin API routes
- [ ] Persistence integration test: trip a breaker, restart riversd, verify breaker is still open via admin API (spec §8)
- [ ] Canary test profile: add circuit breaker to a canary DataView, test trip/reset/status via admin API during canary run

## Gap: riversd.toml Foreign Attribute Protection

- [ ] `ServerConfig` and nested structs silently ignore unknown keys (no `deny_unknown_fields`). A typo like `hostt` in `riversd.toml` is silently dropped with no warning. Either add `deny_unknown_fields` to config structs or implement a structural validation pass at startup (matching the bundle validation pattern)

## Riverbed HTTPD — Future Consideration

See `todo/RiverbedPlan.md` for full build plan with validation tasks.

## riverpackage validation (rivers-runtime)

- [x] Allow foreign/unknown attributes in bundle TOML files during `riverpackage validate` — S002 downgraded from error to warning

## riversd (Gate 2 validation gap)

- [x] Added Layer 1 structural validation at Gate 2 — `validate_structural()` now called in `load.rs` before `validate_bundle()`
