# Rivers Cross-Crate Consolidation Report

**Date:** 2026-04-30
**Source basis:** Fallback consolidation — 22 per-crate reports not available. Sourced from:
- `docs/review/rivers-wide-code-review-2026-04-27.md` (primary — post-RW consolidation pass)
- `docs/code_review.md` (April 24 pre-RW review)
- Resolution work in `todo/changelog.md` and `todo/changedecisionlog.md`

---

## 1. Repeated Cross-Crate Patterns

Findings that appear in 3 or more crates. Each row shows the class, affected crates (count), current resolution state, and the relevant task IDs.

| # | Pattern | Crates (count) | Resolved | Task refs |
|---|---------|---------------|----------|-----------|
| P1 | Secret material in plain `String`/`Vec<u8>`, debug-printable, not zeroized | 6 | Partial | RW1.4.b, RW1.4.h |
| P2 | Unbounded result sets / response materialization | 7 | Partial | RW4.2.b |
| P3 | Driver-level timeout missing or inconsistent | 6 | Partial | RW4 (PR #96) |
| P4 | Config fields that parse but do not affect runtime behavior | 4 | Partial | RW3.3.b |
| P5 | Broker ack/nack semantics diverge from SDK contract | 4 | Partial | RW2 (PR #96), RW-CI.2 |
| P6 | URL path segments interpolated without percent-encoding | 3 | Partial | RW4 (PR #96) |
| P7 | Public functions (schema checkers, admin ops) with no production caller | 3 | Partial | RW3 (PR #96) |
| P8 | Atomic write missing: temp+rename not used for secret/live-file writes | 3 | Partial | RW1.4.h |

### P1 — Secret Lifecycle Is Manual And Easy To Get Wrong

**Affected crates (6):** rivers-lockbox-engine, rivers-keystore-engine, rivers-lockbox, rivers-keystore, cargo-deploy, riversctl

**Pattern:** Secret-bearing types derive `Debug`/`Clone` or have public plaintext fields. Zeroization happens on success paths only. CLI tools put secret material into ordinary `String`s.

**Resolution state:**
- `rivers-lockbox-engine`: `SecretBox<String>` for `ResolvedEntry.value`; `Clone` removed from `Keystore`/`KeystoreEntry`; `Zeroizing::new(toml_str)` on all paths (RW1.4.b, 2026-04-30).
- `rivers-lockbox` CLI: rewritten to route through lockbox-engine; `rpassword` TTY-only input; no `--value` argv (RW1.4.h, 2026-04-30).
- `rivers-keystore-engine`: `Secret<T>` wrapper applied to key material; `Debug` removed from `KeystoreEntry` (RW1.4, PR #96).
- `cargo-deploy`: TLS private key created with `0600` from first write (RW5, PR #96).
- **Still open:** `rivers-keystore` CLI argv zeroization; concurrent-save locking in keystore-engine; per-access permission recheck in lockbox-engine.

### P2 — Unbounded Reads And Result Materialization

**Affected crates (7):** ldap, cassandra, mongodb, elasticsearch, couchdb, influxdb, rabbitmq

**Pattern:** Drivers read full response bodies or cursor results into memory with no cap. Some have no page size or prefetch limit.

**Resolution state:**
- Row caps via `read_max_rows(params)` now applied to: elasticsearch (PR #96), cassandra, mongodb, couchdb, influxdb (RW4.2.b, 2026-04-30).
- LDAP: timeout added (PR #96); row cap and paged search deferred.
- RabbitMQ: prefetch (`basic_qos`) deferred.
- **Still open:** streaming pagination, response-byte caps, LDAP paged search, RabbitMQ prefetch.

### P3 — Timeout Policy Inconsistent

**Affected crates (6):** exec, riversctl, ldap, elasticsearch, rabbitmq, influxdb

**Pattern:** `Client::new()` with no timeout; long I/O outside timeout window; broker confirm waits that can hang indefinitely.

**Resolution state:**
- `rivers-driver-sdk/src/defaults.rs`: shared `DEFAULT_TIMEOUT_MS` constant introduced (PR #96).
- elasticsearch, influxdb, ldap: `DEFAULT_TIMEOUT_MS` applied (PR #96, RW4).
- `rivers-plugin-exec`: stdin/stdout/wait unified under one timeout block (PR #96, RW1.2).
- **Still open:** riversctl admin request timeout; RabbitMQ confirm timeout; LDAP per-operation timeout completeness.

### P4 — Config Fields Parse But Do Nothing

**Affected crates (4):** rivers-core-config, riverpackage, riversctl, rivers-driver-sdk

**Pattern:** Fields are deserialized from TOML successfully but no runtime code reads them for enforcement.

**Resolution state:**
- `rivers-core-config`: unenforced storage fields (`retention_ms`, `max_events`, `cache.datasources`, `cache.dataviews`) now emit `tracing::warn!` at startup when non-default (RW3.3.b, 2026-04-30).
- `riverpackage --config`: wired into engine discovery (PR #96, RW5).
- `riversctl` private key config: corrected key name and loading path (PR #96, RW1.3).
- **Still open:** nested unknown-key validation depth in rivers-core-config; `init_timeout_s` field name typo in allowlist; `SessionCookieConfig::validate()` not bound to hot-reload path.

### P5 — Broker Ack/Nack Contract Divergence

**Affected crates (4):** nats, kafka, redis-streams, rabbitmq

**Pattern:** `nack()` semantics vary per driver; some return `Ok(())` without broker disposition; consumer-group identity built but not used.

**Resolution state:**
- `AckOutcome` enum and `BrokerSubscription` SDK contract defined (PR #96, RW2).
- Shared contract fixtures in `rivers-driver-sdk/src/broker_contract_fixtures.rs` (RW-CI.2, 2026-04-30).
- rabbitmq: AMQP-406 double-ack detection; `basic_reject` for nack (PR #96).
- nats: per-subject `mpsc` channels for fair dispatch (PR #96).
- **Still open:** NATS queue subscriptions / JetStream durable consumers; Kafka pre-commit offset-advance TOCTOU; redis-streams PEL reclaim; multi-subscription support.

### P6 — URL Path Segments Unescaped

**Affected crates (3):** elasticsearch, couchdb, influxdb (driver URLs)

**Pattern:** Document IDs, index names, view names, and query values interpolated into URL strings without percent-encoding.

**Resolution state:**
- Shared `url_encode_path_segment` helper added in `rivers-driver-sdk` (PR #96).
- Applied to elasticsearch document IDs and index segments (PR #96).
- **Still open:** couchdb document ID / design doc / view name segments; influxdb bucket name in batching URL.

### P7 — Public Functions With No Production Caller

**Affected crates (3):** nats, rabbitmq, elasticsearch

**Pattern:** `check_*_schema`, `ddl_execute`, and admin operation functions are public and tested but not called from the production wiring path.

**Resolution state:**
- NATS/RabbitMQ schema checkers wired into `validate_syntax.rs` (PR #96, RW3).
- Elasticsearch `ddl_execute` returns `Unsupported` with a clear message (PR #96, RW3).
- **Still open:** Neo4j static plugin registration; CI enforcement heuristic (rg for unwired public functions).

### P8 — Non-Atomic Secret/Live-File Writes

**Affected crates (3):** rivers-lockbox, rivers-keystore-engine, cargo-deploy

**Pattern:** Write directly into live path; no temp+rename; no fsync; partial writes observable.

**Resolution state:**
- `rivers-lockbox`: atomic temp+rename on all writes (RW1.4.h, 2026-04-30).
- `cargo-deploy`: staging directory pattern for deploy target (PR #96, RW5).
- **Still open:** rivers-keystore-engine fsync + concurrent-save locking; rivers-keystore concurrent-update locking.

---

## 2. SDK/Runtime Contract Violations

Comparing findings against the `rivers-driver-sdk`, engine host callback, and runtime datasource contracts.

| Contract | Violation | Crate | Status |
|----------|-----------|-------|--------|
| `BrokerConsumer::ack/nack` must return `Result<AckOutcome, BrokerError>` | Return type was `Result<(), DriverError>` | riversd test mocks | Fixed 2026-04-30 (broker_bridge_tests, broker_supervisor_tests) |
| `BrokerConsumer::nack` must signal redelivery or return `Unsupported` | nack() returns Ok without broker disposition | nats, kafka, redis-streams | Open |
| `DatabaseDriver::max_rows` via `read_max_rows(params)` | Local constants used instead of SDK defaults | mongodb (was 1,000 not 10,000) | Fixed RW4.2.b |
| `check_schema_syntax` must be wired into `validate_syntax.rs` | Schema checkers tested but never called from validation pipeline | nats, rabbitmq (pre-PR #96) | Fixed PR #96 |
| Engine dylib ABI version check must pass before driver registration | No panic containment on ABI version FFI call | rivers-core | Fixed H3 (PR #95) |
| DDL guard must strip SQL comments before token classification | Leading SQL comments bypass `is_ddl_statement()` | rivers-driver-sdk | Fixed RW1.1 (PR #96) |
| Handler `ctx.store.set/get` must propagate backend errors | Storage errors silently swallowed when backend unavailable | riversd | Fixed B2.1 |
| Module cache miss in production mode must error, not compile live | Cache miss triggered live compilation in production | riversd | Fixed B3.1 |
| V8 host callbacks must not block indefinitely on `recv()` | `recv()` without timeout on synchronous host bridge | riversd engine_loader | Fixed H2 (PR #95) |

---

## 3. Cross-Crate Wiring Gaps

Paths where implementation exists in one crate but the production caller path is missing, stubbed, or bypassed.

| Gap | Implementing Crate | Caller Crate | Status |
|-----|-------------------|--------------|--------|
| Neo4j static plugin not registered in static driver inventory | rivers-plugin-neo4j | rivers-core (static registry) | Open |
| MongoDB `ClientSession` not passed to CRUD methods during active transaction | rivers-plugin-mongodb | — | Open (T1) |
| Neo4j transaction path routes through non-transactional query path | rivers-plugin-neo4j | — | Open (T1) |
| NATS consumer uses plain `subscribe` instead of queue subscription with group | rivers-plugin-nats | — | Open |
| Redis Streams consumer reads only `>` (new messages); PEL/XCLAIM path absent | rivers-plugin-redis-streams | — | Open |
| Kafka `nack()` cannot redeliver: offset committed before ack | rivers-plugin-kafka | — | Open (architecture deferred) |
| `SessionCookieConfig::validate()` not bound to config hot-reload path | rivers-core-config | rivers-core (config reload) | Open |
| `riversctl tls import` does not restrict imported private-key file permissions | riversctl | — | Open |
| `riversctl deploy` only creates a pending deployment (lifecycle not driven) | riversctl | — | Open |

---

## 4. Severity Distribution (Current State)

Based on the April 27 wide review. Strikethrough = resolved. Numbers reflect original findings count.

| Crate | Original T1 | Original T2 | Original T3 | T1 Remaining | T2 Remaining |
|-------|------------|------------|------------|-------------|-------------|
| `rivers-lockbox` | 2 | 8 | 1 | 0 | 3 |
| `rivers-plugin-exec` | 3 | 4 | 1 | 0 | 0 |
| `rivers-keystore-engine` | 1 | 6 | 0 | 1 | 4 |
| `riversctl` | 2 | 5 | 0 | 0 | 3 |
| `rivers-plugin-elasticsearch` | 1 | 5 | 0 | 1 | 1 |
| `rivers-plugin-neo4j` | 2 | 3 | 0 | 2 | 2 |
| `rivers-plugin-nats` | 2 | 3 | 0 | 2 | 1 |
| `cargo-deploy` | 1 | 4 | 0 | 0 | 0 |
| `rivers-lockbox-engine` | 0 | 4 | 0 | 0 | 2 |
| `rivers-driver-sdk` | 1 | 3 | 0 | 0 | 0 |
| `rivers-core-config` | 0 | 4 | 0 | 0 | 3 |
| `rivers-keystore` | 1 | 2 | 0 | 1 | 1 |
| `rivers-plugin-ldap` | 1 | 2 | 0 | 1 | 0 |
| `riverpackage` | 0 | 1 | 2 | 0 | 0 |
| `rivers-plugin-rabbitmq` | 1 | 2 | 0 | 1 | 1 |
| `rivers-plugin-mongodb` | 1 | 2 | 0 | 1 | 1 |
| `rivers-plugin-influxdb` | 1 | 3 | 0 | 0 | 1 |
| `rivers-plugin-redis-streams` | 1 | 2 | 0 | 1 | 1 |
| `rivers-plugin-cassandra` | 1 | 1 | 0 | 0 | 1 |
| `rivers-plugin-couchdb` | 1 | 3 | 0 | 1 | 2 |
| `rivers-plugin-kafka` | 1 | 0 | 0 | 1 | 0 |
| `rivers-engine-sdk` | 0 | 0 | 0 | 0 | 0 |
| **Totals** | **23** | **67** | **4** | **13** | **27** |

Resolved since April 27: 10 T1, 40 T2. Remaining: ~13 T1, ~27 T2.

---

## 5. Recommended Next Remediations (Priority Order)

Items still open, ordered by impact × ease:

1. **MongoDB transaction session attachment** (T1 — CRUD routes outside active session)
2. **Neo4j transaction path** (T1 — queries bypass active Txn)
3. **NATS queue subscriptions / JetStream consumers** (T1 — consumer-group not actually used)
4. **rivers-keystore-engine fsync + locking** (T1 — key rotation silently losable)
5. **rivers-keystore zeroization** (T1 — Age identity in plain String)
6. **Redis Streams PEL reclaim** (T1 — nack leaves messages in PEL with no reclaim path)
7. **riversctl deploy lifecycle** (T2 — only creates pending deployment)
8. **rivers-core-config nested key validation** (T2 — typos in nested sections accepted)
9. **CouchDB selector JSON safety** (T1 — string splice can break JSON structure)
10. **Elasticsearch authenticated-cluster ping** (T1 — ping without auth fails at connect)

---

*Source: `docs/review/rivers-wide-code-review-2026-04-27.md` + `docs/code_review.md`.*
*Resolution tracking: `todo/changelog.md` + `todo/changedecisionlog.md`.*
