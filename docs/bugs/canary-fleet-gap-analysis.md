# Canary Fleet — Gap Analysis

**Date:** 2026-04-02
**Spec:** `docs/arch/rivers-canary-fleet-spec.md`
**Implementation:** `canary-bundle/` (branch: `canary`)

---

## Summary

| Profile | Spec Tests | Implemented | Partial | Missing |
|---------|-----------|-------------|---------|---------|
| AUTH | 9 | 3 | 4 | 2 |
| SQL | 18 | 3 | 3 | 12 |
| NOSQL | 13 | 0 | 1 | 12 |
| RUNTIME | 25 | 4 | 9 | 12 |
| STREAM | 9 | 0 | 1 | 8 |
| PROXY | 4 | 0 | 0 | 4 |
| **Total** | **78** | **10** | **18** | **50** |

---

## Critical Blockers

### 1. SQL init handler absent
canary-sql `manifest.toml` has `[init]` block but no working init handler that creates tables via DDL three-gate enforcement. Without tables, all SQL CRUD tests fail at runtime.

### 2. Datasource name/LockBox alias mismatch
`resources.toml` uses `canary-pg`, `canary-mysql`, `canary-mongo` etc. but `app.toml` uses `pg`, `mysql`, `mongo`. These must match for connection resolution.

### 3. Missing files
- `canary-handlers/libraries/handlers/eventbus-tests.ts` — RT-EVENTBUS-PUBLISH
- `canary-streams/libraries/handlers/kafka-consumer.ts` — STREAM-KAFKA-CONSUME
- `canary-nosql/schemas/es-doc.schema.json`
- `canary-nosql/schemas/couch-doc.schema.json`
- `canary-nosql/schemas/cassandra-row.schema.json`
- `canary-nosql/schemas/ldap-entry.schema.json`

### 4. RT-V8-TIMEOUT neutered
Handler only asserts `handler_executes: true` — doesn't run an infinite loop. Will never actually test the watchdog timeout.

---

## Systematic Issues

### Paths missing leading `/`
Every `path = "canary/..."` should be `path = "/canary/..."`. Affects all 6 apps.

### Language mismatch
All handlers declare `language = "javascript"` but files are `.ts`. Spec says `"typescript"`.

### HTTP method mismatches
| Spec | Implementation |
|------|----------------|
| POST for param-order tests | GET |
| POST for CSRF/DDL reject | GET |
| PUT for updates | missing |
| DELETE for deletes | missing |

### Test ID mismatches (16 total)

| Spec test_id | Implementation test_id |
|-------------|----------------------|
| RT-CTX-TRACE-ID | RT-CTX-TRACEID |
| RT-CTX-APP-ID | RT-CTX-APPID |
| RT-CTX-STORE-GET-SET | RT-CTX-STORE |
| RT-CTX-STORE-NAMESPACE | RT-STORE-RESERVED |
| RT-RIVERS-LOG | RT-LOG-INFO |
| RT-RIVERS-CRYPTO-HASH | RT-CRYPTO-HASHPASSWORD |
| RT-RIVERS-CRYPTO-RANDOM | RT-CRYPTO-RANDOMHEX |
| RT-RIVERS-CRYPTO-TIMING | RT-CRYPTO-TIMINGSAFE |
| RT-ERROR-SANITIZE | RT-ERROR-SANITIZED |
| NOSQL-MONGO-INSERT | NOSQL-MONGO-CRUD (combined) |
| NOSQL-REDIS-ADMIN-REJECT | NOSQL-ADMIN-REJECTED |
| PROXY-SESSION-PROPAGATION | PROXY-GUARD-FORWARD |
| PROXY-SQL-PASSTHROUGH | PROXY-SQL-FORWARD |
| PROXY-HANDLER-PASSTHROUGH | PROXY-RT-FORWARD |
| PROXY-ERROR-PROPAGATION | (missing) |
| AUTH-GUARD-CLAIMS | (not tested separately) |

---

## Per-Profile Missing Tests

### AUTH (2 missing)
- AUTH-GUARD-CLAIMS — separate claims verification
- AUTH-LOGOUT — needs `ctx.session.invalidate()` (not implemented in V8)

### SQL (12 missing)
- SQL-PG-CRUD-UPDATE, SQL-PG-CRUD-DELETE
- SQL-PG-DDL-REJECT, SQL-PG-MAX-ROWS
- SQL-MYSQL-CRUD-INSERT/SELECT/UPDATE/DELETE
- SQL-MYSQL-DDL-REJECT
- SQL-SQLITE-PREFIX
- SQL-CACHE-L1-HIT, SQL-CACHE-INVALIDATE
- SQL-INIT-DDL-SUCCESS

### NOSQL (12 missing)
- NOSQL-MONGO-INSERT, NOSQL-MONGO-FIND, NOSQL-MONGO-ADMIN-REJECT
- NOSQL-ES-INDEX, NOSQL-ES-SEARCH
- NOSQL-COUCH-PUT, NOSQL-COUCH-GET
- NOSQL-CASSANDRA-INSERT, NOSQL-CASSANDRA-SELECT
- NOSQL-LDAP-SEARCH
- NOSQL-REDIS-SET, NOSQL-REDIS-GET

### RUNTIME (12 missing)
- RT-CTX-NODE-ID, RT-CTX-SESSION
- RT-CTX-DATAVIEW-PARAMS, RT-CTX-PSEUDO-DV
- RT-RIVERS-CRYPTO-HMAC
- RT-V8-HEAP, RT-V8-CONSOLE
- RT-EVENTBUS-PUBLISH
- RT-HEADER-BLOCKLIST, RT-FAKER-DETERMINISM

### STREAM (8 missing)
- STREAM-WS-ECHO, STREAM-WS-BROADCAST, STREAM-WS-BINARY-LOG
- STREAM-SSE-TICK, STREAM-SSE-EVENT
- STREAM-REST-NDJSON, STREAM-REST-POISON
- STREAM-KAFKA-CONSUME

### PROXY (4 missing)
- PROXY-SESSION-PROPAGATION
- PROXY-SQL-PASSTHROUGH
- PROXY-HANDLER-PASSTHROUGH
- PROXY-ERROR-PROPAGATION

---

## Tests in Implementation Not in Spec

| App | test_id | Notes |
|-----|---------|-------|
| canary-nosql | NOSQL-*-PING (5 tests) | Ping/connectivity — useful but not in spec |
| canary-nosql | NOSQL-REDIS-CRUD | Combined roundtrip |
| canary-sql | SQL-SQLITE-CRUD | Combined roundtrip |
| canary-main | PROXY-HEALTH, PROXY-RESPONSE-ENVELOPE | Not in spec inventory |
| canary-handlers | RT-STORE-CRUD | Different path/ID from spec |

---

## Remediation Priority

### P0 — Blockers (fix first)
1. Fix datasource name/alias mismatch across all apps
2. Fix missing leading `/` on all paths
3. Create SQL init handler with DDL table creation
4. Create missing files (eventbus-tests.ts, kafka-consumer.ts, 4 schemas)

### P1 — Test ID alignment
5. Rename all 16 mismatched test_ids to match spec
6. Fix HTTP methods (POST/PUT/DELETE where spec requires)
7. Fix language to "typescript"

### P2 — Missing tests
8. Add 50 missing test endpoints (split combined tests, add CRUD operations)
9. Fix RT-V8-TIMEOUT to actually run infinite loop

### P3 — Polish
10. Import from test-harness.ts instead of inlining
11. Add session TTL block to guard view
