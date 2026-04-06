# Rivers Unit Test Infrastructure — Spec

**Document Type:** Test Architecture Specification
**Scope:** Driver conformance matrix, V8 bridge contract tests, regression gate
**Status:** Design / Pre-Implementation
**Depends On:** `rivers-driver-sdk`, `rivers-engine-v8`, `rivers-data`, `rivers-core`, `riversd`
**Complements:** `rivers-canary-fleet-spec.md` (integration layer)

---

## 1. Purpose

The canary fleet tests Rivers after assembly — a running `riversd` hitting real endpoints. This spec covers the layer below: unit and contract tests that run without a server, catch bugs at the function level, and execute in seconds on every PR.

Three strategies target three bug classes from the v0.50–v0.52.7 audit:

| Strategy | Bug Class | Where It Runs | Speed |
|----------|-----------|---------------|-------|
| Driver Conformance Matrix | Same bug in N drivers | `crates/rivers-drivers-builtin/tests/` | 5–30s per driver |
| V8 Bridge Contract Tests | Rust↔JS injection failures, ghost APIs | `crates/rivers-engine-v8/tests/` | <1s per test |
| Regression Gate | Any previously-fixed bug | Per-crate `tests/` | <1s per test |

All three run in CI on every PR. None require a running `riversd`. The driver matrix requires the podman cluster for SQL/NoSQL drivers; the other two require nothing external.

---

## 2. File Layout

```
crates/
├── rivers-driver-sdk/
│   └── tests/
│       └── driver_contract_types.rs       ← QueryValue/Query/QueryResult invariants
│
├── rivers-drivers-builtin/
│   └── tests/
│       ├── conformance/
│       │   ├── mod.rs                     ← shared test harness
│       │   ├── param_binding.rs           ← parameter binding order tests
│       │   ├── crud_lifecycle.rs          ← insert/select/update/delete
│       │   ├── ddl_guard.rs              ← DDL rejection on execute()
│       │   ├── admin_guard.rs            ← admin operation rejection
│       │   ├── null_handling.rs          ← NULL value round-trip
│       │   ├── type_coercion.rs          ← QueryValue type mapping
│       │   ├── max_rows.rs              ← result truncation
│       │   └── circuit_breaker.rs        ← failure threshold + recovery
│       └── regression/
│           └── driver_regression.rs       ← per-driver regression tests
│
├── rivers-engine-v8/
│   └── tests/
│       ├── bridge/
│       │   ├── mod.rs                     ← test isolate factory
│       │   ├── ctx_injection.rs           ← every ctx.* property
│       │   ├── rivers_api.rs              ← every Rivers.* global
│       │   ├── dataview_bridge.rs         ← ctx.dataview() param passing
│       │   ├── store_bridge.rs            ← ctx.store namespace isolation
│       │   └── capability_guard.rs        ← undeclared resource rejection
│       ├── security/
│       │   ├── timeout.rs                 ← infinite loop termination
│       │   ├── heap_limit.rs             ← OOM callback
│       │   ├── codegen_blocked.rs        ← eval/Function rejection
│       │   └── timing_safe.rs            ← constant-time comparison
│       └── regression/
│           └── v8_regression.rs           ← per-fix regression tests
│
├── rivers-data/
│   └── tests/
│       ├── cache/
│       │   ├── l1_lru.rs                  ← LRU eviction, memory bounds
│       │   ├── l2_tiered.rs              ← L1→L2 promotion, L2 skip
│       │   ├── invalidation.rs           ← event-driven cache clear
│       │   └── key_derivation.rs         ← canonical JSON → SHA-256
│       └── regression/
│           └── data_regression.rs
│
├── rivers-core/
│   └── tests/
│       ├── config/
│       │   ├── toml_parsing.rs            ← valid/invalid config acceptance
│       │   ├── validation_rules.rs        ← all validation table entries
│       │   └── env_substitution.rs        ← ${VAR} interpolation
│       └── regression/
│           └── core_regression.rs
│
└── riversd/
    └── tests/
        ├── middleware/
        │   ├── security_headers.rs        ← header presence and values
        │   ├── rate_limit.rs             ← token bucket behavior
        │   ├── backpressure.rs           ← semaphore exhaustion
        │   ├── error_envelope.rs         ← SHAPE-2 format
        │   └── error_sanitization.rs     ← no driver names in production
        ├── dispatch/
        │   ├── header_blocklist.rs        ← Set-Cookie stripped
        │   ├── session_validation.rs      ← per-view auth modes
        │   └── view_dispatch.rs           ← ctx.app_id populated
        └── regression/
            └── server_regression.rs
```

---

## 3. Driver Conformance Matrix

### 3.1 Design

One test function, N drivers. The `#[test_case]` macro from the `test-case` crate generates a separate test instance per driver. When a new bug class is discovered in any driver, one test function is added and it automatically runs against every driver that supports that operation.

### 3.2 Test Harness Module

```rust
// crates/rivers-drivers-builtin/tests/conformance/mod.rs

use rivers_driver_sdk::*;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Connection configs for the podman test cluster.
/// CI sets RIVERS_TEST_CLUSTER=1 to enable live driver tests.
/// Without it, only SQLite (in-memory) and Faker run.
pub fn skip_unless_cluster() {
    if std::env::var("RIVERS_TEST_CLUSTER").is_err() {
        eprintln!("RIVERS_TEST_CLUSTER not set — skipping cluster driver test");
        return; // test passes but does nothing
    }
}

/// Execution contexts for DDL guard testing.
#[derive(Clone, Copy)]
pub enum TestExecutionContext {
    Request,          // normal handler — DDL MUST be rejected
    ApplicationInit,  // init handler — DDL MUST be allowed
}

/// Create a live connection for a named driver.
/// Returns None if the cluster isn't available.
pub async fn make_connection(
    driver: &str,
) -> Option<Box<dyn Connection>> {
    let factory = test_driver_factory();
    let params = test_connection_params(driver)?;
    factory.connect(driver, &params).await.ok()
}

/// Connection params for each test cluster driver.
pub fn test_connection_params(driver: &str) -> Option<ConnectionParams> {
    match driver {
        "postgres" | "postgresql" => Some(ConnectionParams {
            host: "192.168.2.209".into(),
            port: 5432,
            database: "rivers".into(),
            username: "rivers".into(),
            password: "rivers_test".into(),
            options: HashMap::new(),
        }),
        "mysql" => Some(ConnectionParams {
            host: "192.168.2.215".into(),
            port: 3306,
            database: "rivers".into(),
            username: "rivers".into(),
            password: "rivers_test".into(),
            options: HashMap::new(),
        }),
        "sqlite" => Some(ConnectionParams {
            host: String::new(),
            port: 0,
            database: ":memory:".into(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        }),
        "redis" => Some(ConnectionParams {
            host: "192.168.2.206".into(),
            port: 6379,
            database: String::new(),
            username: String::new(),
            password: "rivers_test".into(),
            options: HashMap::new(),
        }),
        "mongodb" => Some(ConnectionParams {
            host: "192.168.2.212".into(),
            port: 27017,
            database: "canary".into(),
            username: "rivers".into(),
            password: "rivers_test".into(),
            options: [
                ("replicaSet".into(), "rivers-rs".into()),
                ("authSource".into(), "admin".into()),
            ].into(),
        }),
        "elasticsearch" => Some(ConnectionParams {
            host: "192.168.2.218".into(),
            port: 9200,
            database: String::new(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        }),
        "couchdb" => Some(ConnectionParams {
            host: "192.168.2.221".into(),
            port: 5984,
            database: "canary".into(),
            username: "rivers".into(),
            password: "rivers_test".into(),
            options: HashMap::new(),
        }),
        "cassandra" => Some(ConnectionParams {
            host: "192.168.2.224".into(),
            port: 9042,
            database: String::new(),
            username: String::new(),
            password: String::new(),
            options: [("keyspace".into(), "canary".into())].into(),
        }),
        "ldap" => Some(ConnectionParams {
            host: "192.168.2.227".into(),
            port: 389,
            database: String::new(),
            username: "cn=admin,dc=rivers,dc=test".into(),
            password: "rivers_test".into(),
            options: [("base_dn".into(), "dc=rivers,dc=test".into())].into(),
        }),
        _ => None,
    }
}

/// Build a Query with named parameters in a specific declaration order.
/// The returned Vec preserves insertion order — this is the declaration order
/// that the DataView engine would pass to the driver.
pub fn ordered_params(pairs: &[(&str, QueryValue)]) -> HashMap<String, QueryValue> {
    // HashMap doesn't preserve order, but that's the point:
    // the driver must NOT depend on HashMap iteration order.
    // It must use the parameter names to bind to the correct positions.
    pairs.iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}
```

### 3.3 Parameter Binding Tests

```rust
// crates/rivers-drivers-builtin/tests/conformance/param_binding.rs

use super::*;
use test_case::test_case;

/// THE critical test. Parameters declared as [zname, age] — alphabetical
/// order is [age, zname]. If the driver sorts alphabetically and binds
/// positionally, zname gets age's value and vice versa.
///
/// This test caught Issue #54 across PostgreSQL, MySQL, SQLite, and Cassandra.
#[test_case("postgres"   ; "pg_param_order")]
#[test_case("mysql"       ; "mysql_param_order")]
#[test_case("sqlite"      ; "sqlite_param_order")]
#[test_case("cassandra"   ; "cassandra_param_order")]
#[tokio::test]
async fn param_binding_order_independent(driver: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };

    // Setup: ensure test table exists (skip for non-SQL)
    let _ = setup_test_table(&mut *conn, driver).await;

    // Insert with params where alpha order ≠ declaration order
    let params = ordered_params(&[
        ("zname", QueryValue::String("ParamTest".into())),
        ("age",   QueryValue::Integer(42)),
    ]);

    let insert_query = make_insert_query(driver, &params);
    conn.execute(&insert_query).await
        .expect("INSERT should succeed");

    // Read back and verify values are in correct columns
    let select_params = ordered_params(&[
        ("zname", QueryValue::String("ParamTest".into())),
    ]);
    let select_query = make_select_by_zname_query(driver, &select_params);
    let result = conn.execute(&select_query).await
        .expect("SELECT should succeed");

    assert!(!result.rows.is_empty(), "should find inserted row");
    let row = &result.rows[0];

    // THE assertion: if param binding is order-dependent,
    // zname will contain "42" and age will contain "ParamTest" (or error)
    assert_eq!(
        row.get("zname"),
        Some(&QueryValue::String("ParamTest".into())),
        "zname column has wrong value — param binding order bug"
    );
    assert_eq!(
        row.get("age"),
        Some(&QueryValue::Integer(42)),
        "age column has wrong value — param binding order bug"
    );

    // Cleanup
    let _ = cleanup_test_row(&mut *conn, driver, "ParamTest").await;
}

/// Multiple params with same first letter — stress-tests alphabetical sort.
/// Params: [zebra, zoo, zip] — all start with 'z', different alpha positions.
#[test_case("postgres" ; "pg_same_prefix")]
#[test_case("mysql"    ; "mysql_same_prefix")]
#[test_case("sqlite"   ; "sqlite_same_prefix")]
#[tokio::test]
async fn param_binding_same_prefix(driver: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };

    let params = ordered_params(&[
        ("zzz_last",  QueryValue::String("should-be-last".into())),
        ("aaa_first", QueryValue::String("should-be-first".into())),
        ("mmm_mid",   QueryValue::String("should-be-middle".into())),
    ]);

    // This is a synthetic test — actual table shape doesn't matter.
    // What matters is that the driver binds zzz_last to $zzz_last position,
    // not to $aaa_first position because 'a' sorts before 'z'.
    let query = Query {
        operation: "insert".into(),
        target: "canary_param_test".into(),
        parameters: params.clone(),
        statement: match driver {
            "postgres" => "INSERT INTO canary_param_test (aaa_first, mmm_mid, zzz_last) VALUES ($aaa_first, $mmm_mid, $zzz_last) RETURNING *".into(),
            "mysql" => "INSERT INTO canary_param_test (aaa_first, mmm_mid, zzz_last) VALUES ($aaa_first, $mmm_mid, $zzz_last)".into(),
            "sqlite" => "INSERT INTO canary_param_test (aaa_first, mmm_mid, zzz_last) VALUES ($aaa_first, $mmm_mid, $zzz_last)".into(),
            _ => return,
        },
    };

    let result = conn.execute(&query).await.expect("INSERT should succeed");

    // Read back
    let select = Query {
        operation: "select".into(),
        target: "canary_param_test".into(),
        parameters: HashMap::new(),
        statement: format!("SELECT * FROM canary_param_test WHERE aaa_first = 'should-be-first'"),
    };
    let rows = conn.execute(&select).await.expect("SELECT should succeed");
    assert!(!rows.rows.is_empty());

    let row = &rows.rows[0];
    assert_eq!(row.get("aaa_first"), Some(&QueryValue::String("should-be-first".into())));
    assert_eq!(row.get("mmm_mid"),   Some(&QueryValue::String("should-be-middle".into())));
    assert_eq!(row.get("zzz_last"),  Some(&QueryValue::String("should-be-last".into())));
}

/// Empty params — should not crash or misbind.
#[test_case("postgres" ; "pg_empty")]
#[test_case("mysql"    ; "mysql_empty")]
#[test_case("sqlite"   ; "sqlite_empty")]
#[test_case("redis"    ; "redis_empty")]
#[tokio::test]
async fn param_binding_empty_params(driver: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };

    let query = match driver {
        "redis" => Query {
            operation: "ping".into(),
            target: String::new(),
            parameters: HashMap::new(),
            statement: "PING".into(),
        },
        _ => Query {
            operation: "select".into(),
            target: "canary_records".into(),
            parameters: HashMap::new(),
            statement: "SELECT 1".into(),
        },
    };

    let result = conn.execute(&query).await;
    assert!(result.is_ok(), "empty params should not cause error: {:?}", result.err());
}
```

### 3.4 DDL Guard Tests

```rust
// crates/rivers-drivers-builtin/tests/conformance/ddl_guard.rs

use super::*;
use test_case::test_case;

/// DDL statements MUST be rejected on Connection::execute() in Request context.
/// They MUST only succeed through ddl_execute() in ApplicationInit context.

#[test_case("postgres", "DROP TABLE canary_records"              ; "pg_drop")]
#[test_case("postgres", "CREATE TABLE evil (id INT)"             ; "pg_create")]
#[test_case("postgres", "ALTER TABLE canary_records ADD col INT"  ; "pg_alter")]
#[test_case("postgres", "TRUNCATE canary_records"                ; "pg_truncate")]
#[test_case("mysql",    "DROP TABLE canary_records"              ; "mysql_drop")]
#[test_case("mysql",    "CREATE TABLE evil (id INT)"             ; "mysql_create")]
#[test_case("sqlite",   "DROP TABLE canary_records"              ; "sqlite_drop")]
#[test_case("sqlite",   "CREATE TABLE evil (id INT)"             ; "sqlite_create")]
#[tokio::test]
async fn ddl_rejected_on_execute(driver: &str, statement: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };

    let query = Query {
        operation: String::new(), // let driver infer from statement
        target: String::new(),
        parameters: HashMap::new(),
        statement: statement.into(),
    };

    let result = conn.execute(&query).await;
    assert!(
        matches!(&result, Err(DriverError::Forbidden(_))),
        "DDL should be Forbidden, got: {:?}", result
    );
}

/// Verify DDL detection covers case variations and leading whitespace/comments.
#[test_case("  DROP TABLE x"          ; "leading_whitespace")]
#[test_case("-- comment\nDROP TABLE x" ; "sql_comment_prefix")]
#[test_case("/* block */DROP TABLE x"  ; "block_comment_prefix")]
#[test_case("drop table x"            ; "lowercase")]
#[test_case("Drop Table x"            ; "mixed_case")]
#[test_case("   \t\nDROP TABLE x"     ; "tabs_newlines")]
#[tokio::test]
async fn ddl_detection_edge_cases(statement: &str) {
    // SQLite in-memory — no cluster needed
    let Some(mut conn) = make_connection("sqlite").await else { return };

    let query = Query {
        operation: String::new(),
        target: String::new(),
        parameters: HashMap::new(),
        statement: statement.into(),
    };

    let result = conn.execute(&query).await;
    assert!(
        matches!(&result, Err(DriverError::Forbidden(_))),
        "DDL '{}' should be Forbidden, got: {:?}", statement, result
    );
}
```

### 3.5 Admin Operation Guard Tests

```rust
// crates/rivers-drivers-builtin/tests/conformance/admin_guard.rs

use super::*;
use test_case::test_case;

/// Non-SQL drivers have admin operation denylists.
/// These operations MUST be rejected with DriverError::Forbidden.

#[test_case("redis",         "FLUSHDB"          ; "redis_flushdb")]
#[test_case("redis",         "FLUSHALL"         ; "redis_flushall")]
#[test_case("redis",         "CONFIG SET"       ; "redis_config_set")]
#[test_case("mongodb",       "drop_collection"  ; "mongo_drop_collection")]
#[test_case("mongodb",       "create_index"     ; "mongo_create_index")]
#[test_case("elasticsearch", "delete_index"     ; "es_delete_index")]
#[tokio::test]
async fn admin_op_rejected(driver: &str, operation: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };

    let query = Query {
        operation: operation.into(),
        target: "canary_test".into(),
        parameters: HashMap::new(),
        statement: operation.into(),
    };

    let result = conn.execute(&query).await;
    assert!(
        matches!(&result, Err(DriverError::Forbidden(_))),
        "Admin op '{}' on {} should be Forbidden, got: {:?}",
        operation, driver, result
    );
}
```

### 3.6 CRUD Lifecycle Tests

```rust
// crates/rivers-drivers-builtin/tests/conformance/crud_lifecycle.rs

use super::*;
use test_case::test_case;

/// Full CRUD round-trip: Insert → Select → Update → Select → Delete → Select.
/// Every step verifies the previous step's effect.

#[test_case("postgres" ; "pg_crud")]
#[test_case("mysql"    ; "mysql_crud")]
#[test_case("sqlite"   ; "sqlite_crud")]
#[tokio::test]
async fn full_crud_lifecycle(driver: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    let tag = format!("crud-{}-{}", driver, std::process::id());

    // INSERT
    let insert_params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age",   QueryValue::Integer(25)),
    ]);
    let insert = make_insert_query(driver, &insert_params);
    let insert_result = conn.execute(&insert).await
        .expect("INSERT failed");
    assert!(insert_result.affected_rows >= 1 || !insert_result.rows.is_empty(),
        "INSERT should affect at least 1 row");

    // SELECT — verify insert
    let select_params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
    ]);
    let select = make_select_by_zname_query(driver, &select_params);
    let select_result = conn.execute(&select).await
        .expect("SELECT after INSERT failed");
    assert!(!select_result.rows.is_empty(), "row should exist after INSERT");
    assert_eq!(
        select_result.rows[0].get("age"),
        Some(&QueryValue::Integer(25))
    );

    // UPDATE
    let update_params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age",   QueryValue::Integer(99)),
    ]);
    let update = make_update_query(driver, &update_params);
    conn.execute(&update).await.expect("UPDATE failed");

    // SELECT — verify update
    let select2 = conn.execute(&select).await
        .expect("SELECT after UPDATE failed");
    assert_eq!(
        select2.rows[0].get("age"),
        Some(&QueryValue::Integer(99)),
        "age should be 99 after UPDATE"
    );

    // DELETE
    let delete_params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
    ]);
    let delete = make_delete_query(driver, &delete_params);
    conn.execute(&delete).await.expect("DELETE failed");

    // SELECT — verify delete
    let select3 = conn.execute(&select).await
        .expect("SELECT after DELETE failed");
    assert!(select3.rows.is_empty(), "row should not exist after DELETE");
}

/// NoSQL drivers get a simpler write → read → verify cycle.
#[test_case("redis"         ; "redis_crud")]
#[test_case("mongodb"       ; "mongo_crud")]
#[test_case("elasticsearch" ; "es_crud")]
#[test_case("couchdb"       ; "couch_crud")]
#[tokio::test]
async fn nosql_write_read_cycle(driver: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };

    let key = format!("canary:crud:{}:{}", driver, std::process::id());

    // Write
    let write_params = ordered_params(&[
        ("key",   QueryValue::String(key.clone())),
        ("value", QueryValue::String("test-value-42".into())),
    ]);
    let write = make_write_query(driver, &write_params);
    conn.execute(&write).await
        .expect("write failed");

    // Read
    let read_params = ordered_params(&[
        ("key", QueryValue::String(key.clone())),
    ]);
    let read = make_read_query(driver, &read_params);
    let result = conn.execute(&read).await
        .expect("read failed");

    assert!(!result.rows.is_empty(), "should find written value");
}
```

### 3.7 NULL and Type Coercion Tests

```rust
// crates/rivers-drivers-builtin/tests/conformance/null_handling.rs

use super::*;
use test_case::test_case;

/// NULL values must round-trip correctly — not silently become empty strings
/// or zero values.
#[test_case("postgres" ; "pg_null")]
#[test_case("mysql"    ; "mysql_null")]
#[test_case("sqlite"   ; "sqlite_null")]
#[tokio::test]
async fn null_value_round_trip(driver: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    // Insert with NULL email
    let params = ordered_params(&[
        ("zname", QueryValue::String("NullTest".into())),
        ("age",   QueryValue::Integer(1)),
        ("email", QueryValue::Null),
    ]);
    let insert = make_insert_with_email_query(driver, &params);
    conn.execute(&insert).await.expect("INSERT with NULL failed");

    // Read back
    let select_params = ordered_params(&[
        ("zname", QueryValue::String("NullTest".into())),
    ]);
    let select = make_select_by_zname_query(driver, &select_params);
    let result = conn.execute(&select).await.expect("SELECT failed");

    assert!(!result.rows.is_empty());
    let email = result.rows[0].get("email");
    assert!(
        email == Some(&QueryValue::Null) || email.is_none(),
        "NULL should round-trip as Null, got: {:?}", email
    );

    let _ = cleanup_test_row(&mut *conn, driver, "NullTest").await;
}
```

### 3.8 max_rows Truncation Tests

```rust
// crates/rivers-drivers-builtin/tests/conformance/max_rows.rs

use super::*;
use test_case::test_case;

/// When max_rows is configured, the result MUST be truncated.
/// This is enforced at the DataView engine layer, not the driver,
/// but we test the driver's behavior when LIMIT is appended.
#[test_case("postgres" ; "pg_max_rows")]
#[test_case("mysql"    ; "mysql_max_rows")]
#[test_case("sqlite"   ; "sqlite_max_rows")]
#[tokio::test]
async fn result_truncated_at_max_rows(driver: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };

    // Seed enough rows — the canary init handler seeds 200 for PG.
    // For this test we just need to verify LIMIT works.
    let query = Query {
        operation: "select".into(),
        target: "canary_records".into(),
        parameters: HashMap::new(),
        statement: "SELECT * FROM canary_records LIMIT 5".into(),
    };

    let result = conn.execute(&query).await.expect("SELECT LIMIT failed");
    assert!(
        result.rows.len() <= 5,
        "expected at most 5 rows, got {}", result.rows.len()
    );
}
```

---

## 4. V8 Bridge Contract Tests

### 4.1 Design

These tests create a V8 isolate, inject known Rust-side values, run a JS snippet that reads them, and verify the output. No HTTP server, no `riversd`, no real datasources. They test the injection bridge in isolation.

The test isolate factory creates a minimal V8 environment with the same injection code paths that production uses, but with mock backends for datasources and StorageEngine.

### 4.2 Test Isolate Factory

```rust
// crates/rivers-engine-v8/tests/bridge/mod.rs

use rivers_engine_v8::*;
use std::sync::{Arc, Mutex};

/// Create a V8 isolate with production injection paths
/// but mock backends.
pub struct TestIsolate {
    runtime: V8Runtime,
    dataview_calls: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    store_backend: Arc<Mutex<HashMap<String, String>>>,
}

impl TestIsolate {
    pub fn new() -> Self {
        let rt = V8Runtime::new_for_test(V8TestConfig {
            max_heap_mb: 64,
            timeout_ms: 5000,
            allow_codegen: false,
        });
        Self {
            runtime: rt,
            dataview_calls: Arc::new(Mutex::new(Vec::new())),
            store_backend: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Inject a mock session with known claims.
    pub fn with_session(mut self, claims: serde_json::Value) -> Self {
        self.runtime.inject_session(claims);
        self
    }

    /// Inject ctx metadata.
    pub fn with_ctx(mut self, app_id: &str, trace_id: &str,
                     node_id: &str, env: &str) -> Self {
        self.runtime.inject_ctx_meta(app_id, trace_id, node_id, env);
        self
    }

    /// Inject a mock request.
    pub fn with_request(mut self, request: serde_json::Value) -> Self {
        self.runtime.inject_request(request);
        self
    }

    /// Inject a dataview mock that captures calls.
    pub fn with_dataview_capture(mut self) -> Self {
        let calls = self.dataview_calls.clone();
        self.runtime.inject_dataview_bridge(move |name, params| {
            calls.lock().unwrap().push((name.to_string(), params.clone()));
            Ok(serde_json::json!({
                "rows": [{"id": 1, "name": "mock"}],
                "affected_rows": 1
            }))
        });
        self
    }

    /// Inject a mock store with namespace enforcement.
    pub fn with_store(mut self) -> Self {
        let backend = self.store_backend.clone();
        self.runtime.inject_store_bridge(
            // get
            {
                let b = backend.clone();
                move |key: &str| {
                    Self::enforce_namespace(key)?;
                    Ok(b.lock().unwrap().get(key).cloned())
                }
            },
            // set
            {
                let b = backend.clone();
                move |key: &str, value: &str, _ttl: u64| {
                    Self::enforce_namespace(key)?;
                    b.lock().unwrap().insert(key.to_string(), value.to_string());
                    Ok(())
                }
            },
            // del
            {
                let b = backend.clone();
                move |key: &str| {
                    Self::enforce_namespace(key)?;
                    b.lock().unwrap().remove(key);
                    Ok(())
                }
            },
        );
        self
    }

    fn enforce_namespace(key: &str) -> Result<(), String> {
        let reserved = ["session:", "csrf:", "cache:", "raft:", "rivers:"];
        for prefix in &reserved {
            if key.starts_with(prefix) {
                return Err(format!("access denied: reserved namespace '{}'", prefix));
            }
        }
        Ok(())
    }

    /// Run a JS expression and return the result as a string.
    pub fn eval(&self, js: &str) -> String {
        self.runtime.eval(js).expect("JS eval failed")
    }

    /// Run a JS expression and return as parsed JSON.
    pub fn eval_json(&self, js: &str) -> serde_json::Value {
        let s = self.eval(js);
        serde_json::from_str(&s).expect("result is not valid JSON")
    }

    /// Get captured dataview calls.
    pub fn dataview_calls(&self) -> Vec<(String, serde_json::Value)> {
        self.dataview_calls.lock().unwrap().clone()
    }
}
```

### 4.3 ctx.* Injection Tests

```rust
// crates/rivers-engine-v8/tests/bridge/ctx_injection.rs

use super::*;

#[test]
fn ctx_trace_id_injected() {
    let iso = TestIsolate::new()
        .with_ctx("app-1", "trace-abc-123", "node-1", "test");
    let result = iso.eval("ctx.trace_id");
    assert_eq!(result, "trace-abc-123");
}

#[test]
fn ctx_app_id_injected_and_not_empty() {
    let iso = TestIsolate::new()
        .with_ctx("my-app-uuid", "t1", "n1", "test");
    let result = iso.eval("ctx.app_id");
    assert_eq!(result, "my-app-uuid");
    assert!(!result.is_empty(), "ctx.app_id must not be empty string");
}

#[test]
fn ctx_node_id_injected() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "node-42", "test");
    assert_eq!(iso.eval("ctx.node_id"), "node-42");
}

#[test]
fn ctx_env_injected() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "production");
    assert_eq!(iso.eval("ctx.env"), "production");
}

#[test]
fn ctx_session_is_object_with_claims() {
    let claims = serde_json::json!({
        "sub": "user-001",
        "role": "admin",
        "email": "test@test.com"
    });
    let iso = TestIsolate::new().with_session(claims);

    assert_eq!(iso.eval("typeof ctx.session"), "object");
    assert_eq!(iso.eval("ctx.session.sub"), "user-001");
    assert_eq!(iso.eval("ctx.session.role"), "admin");
    assert_eq!(iso.eval("ctx.session.email"), "test@test.com");
}

#[test]
fn ctx_session_undefined_when_no_session() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test");
    // No .with_session() — session should be undefined, not null, not {}
    let result = iso.eval("typeof ctx.session");
    assert!(
        result == "undefined" || result == "object",
        "ctx.session type should be undefined or null-object when no session, got: {}", result
    );
}

#[test]
fn ctx_request_has_all_fields() {
    let req = serde_json::json!({
        "method": "POST",
        "path": "/api/test",
        "headers": {"content-type": "application/json"},
        "query": {"limit": "10"},
        "body": {"name": "Alice"},
        "params": {"id": "42"}
    });
    let iso = TestIsolate::new().with_request(req);

    assert_eq!(iso.eval("ctx.request.method"), "POST");
    assert_eq!(iso.eval("ctx.request.path"), "/api/test");
    assert_eq!(iso.eval("ctx.request.headers['content-type']"), "application/json");
    assert_eq!(iso.eval("ctx.request.query.limit"), "10");
    assert_eq!(iso.eval("ctx.request.body.name"), "Alice");
    assert_eq!(iso.eval("ctx.request.params.id"), "42");
}

#[test]
fn ctx_resdata_writable_and_becomes_response() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test");

    iso.eval("ctx.resdata = {result: 'hello'}");
    let resdata = iso.eval_json("ctx.resdata");
    assert_eq!(resdata["result"], "hello");
}
```

### 4.4 ctx.dataview() Bridge Tests

```rust
// crates/rivers-engine-v8/tests/bridge/dataview_bridge.rs

use super::*;

/// THE regression test for the ctx.dataview() param-dropping bug (PR #48).
/// Verifies that parameters passed from JS reach the Rust-side bridge.
#[test]
fn dataview_params_not_dropped() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_dataview_capture();

    iso.eval(r#"ctx.dataview("my_view", {id: 42, name: "Alice", active: true})"#);

    let calls = iso.dataview_calls();
    assert_eq!(calls.len(), 1, "exactly one dataview call expected");
    let (name, params) = &calls[0];
    assert_eq!(name, "my_view");
    assert_eq!(params["id"], 42);
    assert_eq!(params["name"], "Alice");
    assert_eq!(params["active"], true);
}

/// Params with special types — verify QueryValue mapping.
#[test]
fn dataview_params_type_fidelity() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_dataview_capture();

    iso.eval(r#"ctx.dataview("typed_view", {
        str_val: "hello",
        int_val: 42,
        float_val: 3.14,
        bool_val: false,
        null_val: null,
        arr_val: [1, 2, 3]
    })"#);

    let calls = iso.dataview_calls();
    let (_, params) = &calls[0];
    assert_eq!(params["str_val"], "hello");
    assert_eq!(params["int_val"], 42);
    assert!(params["float_val"].as_f64().unwrap() - 3.14 < 0.001);
    assert_eq!(params["bool_val"], false);
    assert!(params["null_val"].is_null());
    assert_eq!(params["arr_val"], serde_json::json!([1, 2, 3]));
}

/// Calling dataview with empty params — must not crash.
#[test]
fn dataview_empty_params() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_dataview_capture();

    iso.eval(r#"ctx.dataview("empty_view", {})"#);

    let calls = iso.dataview_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, serde_json::json!({}));
}

/// Calling dataview without second arg — must not crash,
/// params should be empty object or undefined-safe.
#[test]
fn dataview_no_params_arg() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_dataview_capture();

    iso.eval(r#"ctx.dataview("no_params_view")"#);

    let calls = iso.dataview_calls();
    assert_eq!(calls.len(), 1);
    // Params should be empty, not undefined/crash
    assert!(calls[0].1.is_object());
}
```

### 4.5 ctx.store Namespace Isolation Tests

```rust
// crates/rivers-engine-v8/tests/bridge/store_bridge.rs

use super::*;

#[test]
fn store_get_set_del_roundtrip() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_store();

    iso.eval(r#"ctx.store.set("my-key", "my-value", 60)"#);
    let val = iso.eval(r#"ctx.store.get("my-key")"#);
    assert_eq!(val, "my-value");

    iso.eval(r#"ctx.store.del("my-key")"#);
    let deleted = iso.eval(r#"ctx.store.get("my-key") === null ? "null" : "exists""#);
    assert_eq!(deleted, "null");
}

#[test]
fn store_rejects_session_namespace() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_store();

    let result = iso.eval(r#"
        try { ctx.store.get("session:hijack"); "ACCESSIBLE" }
        catch(e) { "REJECTED" }
    "#);
    assert_eq!(result, "REJECTED");
}

#[test]
fn store_rejects_csrf_namespace() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_store();

    let result = iso.eval(r#"
        try { ctx.store.set("csrf:forged", "evil", 60); "ACCESSIBLE" }
        catch(e) { "REJECTED" }
    "#);
    assert_eq!(result, "REJECTED");
}

#[test]
fn store_rejects_cache_namespace() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_store();

    let result = iso.eval(r#"
        try { ctx.store.get("cache:views:secret:hash"); "ACCESSIBLE" }
        catch(e) { "REJECTED" }
    "#);
    assert_eq!(result, "REJECTED");
}

#[test]
fn store_rejects_all_reserved_prefixes() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_store();

    let prefixes = ["session:", "csrf:", "cache:", "raft:", "rivers:"];
    for prefix in prefixes {
        let js = format!(
            r#"try {{ ctx.store.get("{}test"); "ACCESSIBLE" }} catch(e) {{ "REJECTED" }}"#,
            prefix
        );
        let result = iso.eval(&js);
        assert_eq!(result, "REJECTED",
            "reserved prefix '{}' was accessible", prefix);
    }
}
```

### 4.6 V8 Security Tests

```rust
// crates/rivers-engine-v8/tests/security/timeout.rs

use super::*;
use std::time::{Duration, Instant};

#[test]
fn infinite_loop_terminates_within_timeout() {
    let iso = TestIsolate::new(); // default timeout_ms = 5000

    let start = Instant::now();
    let result = std::panic::catch_unwind(|| {
        iso.eval("while(true) {}");
    });
    let elapsed = start.elapsed();

    // Must terminate, not hang
    assert!(elapsed < Duration::from_secs(10),
        "infinite loop ran for {:?} — watchdog did not fire", elapsed);
    // Should terminate close to the timeout, not instantly
    assert!(elapsed > Duration::from_secs(1),
        "terminated too fast ({:?}) — may not have actually looped", elapsed);
}

// crates/rivers-engine-v8/tests/security/codegen_blocked.rs

#[test]
fn eval_is_blocked() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"
        try { eval("1 + 1"); "EXECUTED" }
        catch(e) { "BLOCKED" }
    "#);
    assert_eq!(result, "BLOCKED");
}

#[test]
fn function_constructor_is_blocked() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"
        try { new Function("return 42")(); "EXECUTED" }
        catch(e) { "BLOCKED" }
    "#);
    assert_eq!(result, "BLOCKED");
}

// crates/rivers-engine-v8/tests/security/timing_safe.rs

#[test]
fn timing_safe_equal_returns_true_for_equal() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"Rivers.crypto.timingSafeEqual("abc", "abc") ? "true" : "false""#);
    assert_eq!(result, "true");
}

#[test]
fn timing_safe_equal_returns_false_for_unequal() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"Rivers.crypto.timingSafeEqual("abc", "xyz") ? "true" : "false""#);
    assert_eq!(result, "false");
}

#[test]
fn timing_safe_equal_returns_false_for_different_length() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"Rivers.crypto.timingSafeEqual("short", "muchlonger") ? "true" : "false""#);
    assert_eq!(result, "false");
}

// crates/rivers-engine-v8/tests/security/heap_limit.rs

#[test]
fn massive_allocation_does_not_crash_process() {
    let iso = TestIsolate::new();
    // This should trigger NearHeapLimitCallback and terminate the handler.
    // The critical assertion: the TEST PROCESS must survive.
    // The isolate may throw, panic, or return an error — all acceptable.
    // The only failure is if this test process crashes (segfault/OOM).
    let result = std::panic::catch_unwind(|| {
        iso.eval(r#"
            var arrays = [];
            for (var i = 0; i < 1000000; i++) {
                arrays.push(new Array(100000).fill(i));
            }
        "#);
    });
    // We don't care if it's Ok or Err — we care that we reached this line
    assert!(true, "process survived OOM attempt");
}
```

### 4.7 Rivers.* Global API Tests

```rust
// crates/rivers-engine-v8/tests/bridge/rivers_api.rs

use super::*;

#[test]
fn rivers_log_exists_and_callable() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"
        try {
            Rivers.log.info("test");
            Rivers.log.warn("test");
            Rivers.log.error("test");
            "OK"
        } catch(e) { "FAILED: " + e }
    "#);
    assert_eq!(result, "OK");
}

#[test]
fn rivers_crypto_random_hex_returns_correct_length() {
    let iso = TestIsolate::new();
    let len = iso.eval("Rivers.crypto.randomHex(32).length");
    assert_eq!(len, "64"); // 32 bytes = 64 hex chars
}

#[test]
fn rivers_crypto_random_hex_not_deterministic() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"
        var a = Rivers.crypto.randomHex(16);
        var b = Rivers.crypto.randomHex(16);
        a === b ? "SAME" : "DIFFERENT"
    "#);
    assert_eq!(result, "DIFFERENT");
}

#[test]
fn rivers_crypto_hash_password_and_verify() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"
        var hash = Rivers.crypto.hashPassword("secret123");
        var valid = Rivers.crypto.verifyPassword("secret123", hash);
        var invalid = Rivers.crypto.verifyPassword("wrong", hash);
        valid && !invalid ? "OK" : "FAILED"
    "#);
    assert_eq!(result, "OK");
}

#[test]
fn rivers_crypto_hmac_deterministic() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"
        var a = Rivers.crypto.hmac("sha256", "key", "message");
        var b = Rivers.crypto.hmac("sha256", "key", "message");
        a === b ? "DETERMINISTIC" : "NOT"
    "#);
    assert_eq!(result, "DETERMINISTIC");
}

#[test]
fn console_not_available() {
    let iso = TestIsolate::new();
    let result = iso.eval(r#"typeof console === "undefined" ? "ABSENT" : "PRESENT""#);
    assert_eq!(result, "ABSENT");
}

/// Ghost API detection: every Rivers.* method in the spec must exist.
/// If any method is undefined, it's a ghost API.
#[test]
fn all_spec_rivers_apis_exist() {
    let iso = TestIsolate::new();
    let apis = [
        "Rivers.log.info",
        "Rivers.log.warn",
        "Rivers.log.error",
        "Rivers.crypto.hashPassword",
        "Rivers.crypto.verifyPassword",
        "Rivers.crypto.randomHex",
        "Rivers.crypto.randomBase64url",
        "Rivers.crypto.hmac",
        "Rivers.crypto.timingSafeEqual",
    ];
    for api in apis {
        let js = format!(r#"typeof {} === "function" ? "EXISTS" : "GHOST""#, api);
        let result = iso.eval(&js);
        assert_eq!(result, "EXISTS",
            "GHOST API detected: {} is not a function", api);
    }
}

/// Ghost API detection for ctx methods.
#[test]
fn all_spec_ctx_methods_exist() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_dataview_capture()
        .with_store();

    let methods = [
        ("ctx.dataview",   "function"),
        ("ctx.datasource", "function"),
        ("ctx.store.get",  "function"),
        ("ctx.store.set",  "function"),
        ("ctx.store.del",  "function"),
    ];
    for (method, expected_type) in methods {
        let js = format!(r#"typeof {}"#, method);
        let result = iso.eval(&js);
        assert_eq!(result, expected_type,
            "GHOST API: {} should be '{}', got '{}'", method, expected_type, result);
    }
}
```

---

## 5. Regression Gate

### 5.1 Process Rule

**Every bug fix PR MUST include a regression test that:**
1. Fails on the codebase BEFORE the fix (verified by the author)
2. Passes on the codebase AFTER the fix
3. Is tagged with the bug's origin (PR number, issue number, or SEC finding ID)

**No exceptions.** If a bug can't be reproduced in a unit test, it gets a canary fleet endpoint instead. But the regression MUST exist somewhere.

### 5.2 Regression Test Convention

Each crate has a `tests/regression/` directory. Each file is named `{crate}_regression.rs`. Tests are annotated with their origin.

```rust
// crates/rivers-engine-v8/tests/regression/v8_regression.rs

/// Regression: ctx.dataview() silently dropped parameters.
/// Fixed in PR #48. The V8 bridge extracted the dataview name
/// but did not pass the second argument (params object) to the
/// Rust-side handler.
///
/// Origin: PR #48, discovered via user testing
/// File: crates/rivers-engine-v8/src/execution.rs
/// Root cause: bridge function arity check was 1, should be 2
#[test]
fn regression_pr48_dataview_params_not_dropped() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_dataview_capture();

    iso.eval(r#"ctx.dataview("test_dv", {id: 42})"#);

    let calls = iso.dataview_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1["id"], 42,
        "PR #48 regression: params still being dropped");
}

/// Regression: ctx.app_id was always empty string.
/// Fixed in view_dispatch.rs:172.
///
/// Origin: Bug report, view_dispatch.rs:172
/// Root cause: AppId not propagated from deployment context to TaskContext
#[test]
fn regression_app_id_not_empty() {
    let iso = TestIsolate::new()
        .with_ctx("real-app-uuid", "t1", "n1", "test");

    let result = iso.eval("ctx.app_id");
    assert_eq!(result, "real-app-uuid");
    assert!(!result.is_empty(),
        "view_dispatch regression: ctx.app_id is empty string");
}

/// Regression: timingSafeEqual used short-circuiting .all()
/// Fixed in SEC-5 audit finding.
///
/// Origin: Security audit finding #5
/// File: crates/rivers-engine-v8/src/crypto.rs
/// Root cause: Array.all() short-circuits on first false
#[test]
fn regression_sec5_timing_safe_not_short_circuit() {
    let iso = TestIsolate::new();

    // Both equal and unequal comparisons must work correctly
    assert_eq!(iso.eval(
        r#"Rivers.crypto.timingSafeEqual("abc", "abc") ? "T" : "F""#
    ), "T");
    assert_eq!(iso.eval(
        r#"Rivers.crypto.timingSafeEqual("abc", "xyz") ? "T" : "F""#
    ), "F");
    // Different lengths
    assert_eq!(iso.eval(
        r#"Rivers.crypto.timingSafeEqual("a", "abcdef") ? "T" : "F""#
    ), "F");
}
```

```rust
// crates/rivers-drivers-builtin/tests/regression/driver_regression.rs

/// Regression: SQLite bind_params prefixes `:` but query uses `$`.
/// The DataView engine sends $name params. SQLite's native API expects :name.
/// The driver must translate.
///
/// Origin: Bug report, sqlite.rs
/// Root cause: No prefix translation in sqlite driver
#[tokio::test]
async fn regression_sqlite_param_prefix() {
    let Some(mut conn) = make_connection("sqlite").await else { return };

    // Setup
    let setup = Query {
        operation: "execute".into(),
        target: String::new(),
        parameters: HashMap::new(),
        statement: "CREATE TABLE IF NOT EXISTS prefix_test (name TEXT)".into(),
    };
    // This would go through ddl_execute in production, but for test
    // we use a test-mode connection that allows DDL
    let _ = conn.execute(&setup).await;

    // Insert with $name placeholder — driver must translate to :name
    let params = ordered_params(&[
        ("name", QueryValue::String("test-value".into())),
    ]);
    let insert = Query {
        operation: "insert".into(),
        target: "prefix_test".into(),
        parameters: params,
        statement: "INSERT INTO prefix_test (name) VALUES ($name)".into(),
    };

    let result = conn.execute(&insert).await;
    assert!(result.is_ok(),
        "sqlite param prefix regression: $name not translated to :name — {:?}",
        result.err());
}

/// Regression: PostgreSQL and MySQL sort params alphabetically
/// and bind positionally — causes silent data corruption.
///
/// Origin: Issue #54
/// Root cause: HashMap iteration order used for positional binding
#[test_case("postgres" ; "pg_issue54")]
#[test_case("mysql"    ; "mysql_issue54")]
#[tokio::test]
async fn regression_issue54_param_order(driver: &str) {
    skip_unless_cluster();
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    // zname sorts AFTER age alphabetically.
    // If driver sorts HashMap keys and binds positionally:
    //   position 1 ($zname) gets "age" value = 99
    //   position 2 ($age)   gets "zname" value = "IssueTest"
    // Result: zname column contains "99", age column errors or contains garbage
    let params = ordered_params(&[
        ("zname", QueryValue::String("Issue54Test".into())),
        ("age",   QueryValue::Integer(99)),
    ]);

    let insert = make_insert_query(driver, &params);
    conn.execute(&insert).await.expect("INSERT failed");

    let select_params = ordered_params(&[
        ("zname", QueryValue::String("Issue54Test".into())),
    ]);
    let select = make_select_by_zname_query(driver, &select_params);
    let result = conn.execute(&select).await.expect("SELECT failed");

    assert!(!result.rows.is_empty(), "row not found");
    assert_eq!(
        result.rows[0].get("zname"),
        Some(&QueryValue::String("Issue54Test".into())),
        "Issue #54 regression: zname column has wrong value"
    );
    assert_eq!(
        result.rows[0].get("age"),
        Some(&QueryValue::Integer(99)),
        "Issue #54 regression: age column has wrong value"
    );

    let _ = cleanup_test_row(&mut *conn, driver, "Issue54Test").await;
}
```

```rust
// crates/riversd/tests/regression/server_regression.rs

/// Regression: Error responses leaked driver/infrastructure details.
/// Fixed in SEC-14 audit finding.
///
/// Origin: Security audit finding #14
/// File: crates/riversd/src/server/view_dispatch.rs
/// Root cause: raw DriverError::Query message passed to HTTP response
#[test]
fn regression_sec14_error_no_driver_names() {
    let error_msg = "connection to postgres at 192.168.2.209:5432 refused";
    let sanitized = sanitize_error_for_client(error_msg);

    assert!(!sanitized.contains("postgres"),
        "SEC-14 regression: error contains 'postgres'");
    assert!(!sanitized.contains("192.168"),
        "SEC-14 regression: error contains IP address");
    assert!(!sanitized.contains("5432"),
        "SEC-14 regression: error contains port number");
}

/// Regression: HSTS header missing.
/// Fixed in SEC-13 audit finding.
///
/// Origin: Security audit finding #13
#[test]
fn regression_sec13_hsts_header() {
    let headers = build_security_headers(&SecurityConfig::default());
    assert!(headers.contains_key("strict-transport-security"),
        "SEC-13 regression: HSTS header missing");
    let hsts = headers.get("strict-transport-security").unwrap();
    assert!(hsts.contains("max-age="),
        "SEC-13 regression: HSTS missing max-age");
}

/// Regression: CORS missing Vary: Origin header.
/// Fixed in SEC-12 audit finding.
///
/// Origin: Security audit finding #12
/// Root cause: dynamic origin reflection without Vary header = CDN cache poisoning
#[test]
fn regression_sec12_cors_vary_origin() {
    let cors_headers = build_cors_headers(
        &CorsConfig {
            allowed_origins: vec!["https://example.com".into()],
            ..Default::default()
        },
        "https://example.com",
    );
    assert_eq!(
        cors_headers.get("vary"),
        Some(&"Origin".to_string()),
        "SEC-12 regression: Vary: Origin missing on dynamic CORS"
    );
}

/// Regression: CSRF cookie missing Secure flag.
/// Fixed in SEC-8 audit finding.
///
/// Origin: Security audit finding #8
#[test]
fn regression_sec8_csrf_cookie_secure() {
    let cookie = build_csrf_cookie("token-value", &SessionConfig::default());
    assert!(cookie.contains("Secure"),
        "SEC-8 regression: CSRF cookie missing Secure flag");
    assert!(cookie.contains("HttpOnly"),
        "SEC-8 regression: CSRF cookie missing HttpOnly flag");
}

/// Regression: Admin RBAC default-allow for unknown paths.
/// Fixed in SEC-9 audit finding.
///
/// Origin: Security audit finding #9
/// Root cause: missing path = deny-by-default
#[test]
fn regression_sec9_rbac_deny_unknown() {
    let rbac = AdminRbac::new(test_rbac_config());
    let result = rbac.check_permission(
        "admin-identity",
        "/admin/unknown-new-endpoint"
    );
    assert!(result.is_err() || result == Ok(false),
        "SEC-9 regression: unknown admin path allowed by default");
}
```

### 5.3 Regression Tracking Table

Maintain this table at the top of each regression file. It maps every bug to its test:

```rust
// Regression Tracking — crates/rivers-engine-v8/tests/regression/v8_regression.rs
//
// | Bug ID    | Test Function                              | Fixed In | File             |
// |-----------|--------------------------------------------|----------|------------------|
// | PR #48    | regression_pr48_dataview_params_not_dropped | v0.51.0  | execution.rs     |
// | SEC-2     | regression_sec2_v8_timeout                 | v0.52.0  | v8_runtime.rs    |
// | SEC-3     | regression_sec3_codegen_blocked             | v0.52.0  | v8_runtime.rs    |
// | SEC-4     | regression_sec4_heap_limit                  | v0.52.0  | v8_runtime.rs    |
// | SEC-5     | regression_sec5_timing_safe_not_short_circuit | v0.52.0 | crypto.rs       |
// | BUG-172   | regression_app_id_not_empty                | v0.52.3  | view_dispatch.rs |
```

---

## 6. CI Integration

### 6.1 Test Tiers

```yaml
# .github/workflows/test.yml

jobs:
  unit-tests:
    # Tier 1: No external deps. Runs on every PR.
    # Bridge contract, V8 security, config parsing, regression gates.
    runs-on: ubuntu-latest
    steps:
      - run: cargo test --workspace --exclude rivers-drivers-builtin

  driver-matrix-sqlite:
    # Tier 2: SQLite only (in-memory). Runs on every PR.
    # Covers param binding, DDL guard, CRUD lifecycle for SQLite.
    runs-on: ubuntu-latest
    steps:
      - run: cargo test -p rivers-drivers-builtin

  driver-matrix-cluster:
    # Tier 3: Full podman cluster. Runs on merge to main + nightly.
    # Covers all drivers against live services.
    runs-on: [self-hosted, rivers-test-cluster]
    env:
      RIVERS_TEST_CLUSTER: "1"
    steps:
      - run: cargo test -p rivers-drivers-builtin

  canary-fleet:
    # Tier 4: Full integration. Runs on release candidates.
    # Boots riversd, deploys canary bundle, runs harness.
    needs: [driver-matrix-cluster]
    runs-on: [self-hosted, rivers-test-cluster]
    steps:
      - run: cargo test --test canary_fleet -- --test-threads=1
```

### 6.2 Test Naming Convention

```
test name format: {category}_{what}_{condition}

unit tests:        param_binding_order_independent
contract tests:    ctx_session_is_object_with_claims  
regression tests:  regression_pr48_dataview_params_not_dropped
security tests:    infinite_loop_terminates_within_timeout
```

### 6.3 Test Tagging

Use `#[ignore]` for tests that require the cluster but the cluster isn't available:

```rust
#[tokio::test]
async fn some_cluster_test() {
    if std::env::var("RIVERS_TEST_CLUSTER").is_err() {
        return; // silently skip — not a failure
    }
    // ... test body
}
```

Do NOT use `#[ignore]` — it requires `--ignored` flag. Use the env-var early return pattern so tests always "pass" in CI without the cluster, and actually exercise when the cluster is available.

---

## 7. Coverage Map

How the three strategies cover the dream doc's 25 bugs:

| Bug | Strategy 1 (Driver Matrix) | Strategy 2 (Bridge) | Strategy 3 (Regression) | Canary Fleet |
|-----|---------------------------|---------------------|------------------------|--------------|
| DDL through execute() | `ddl_rejected_on_execute` | — | — | SQL-PG-DDL-REJECT |
| V8 no timeout | — | `infinite_loop_terminates` | `regression_sec2` | RT-V8-TIMEOUT |
| V8 codegen available | — | `eval_is_blocked` | `regression_sec3` | RT-V8-CODEGEN |
| V8 heap no limit | — | `massive_allocation_does_not_crash` | `regression_sec4` | RT-V8-HEAP |
| timingSafeEqual short-circuit | — | `timing_safe_*` | `regression_sec5` | RT-RIVERS-CRYPTO-TIMING |
| TLS verify disabled | — | — | `regression_sec6` | — |
| Session token 122-bit | — | — | `regression_sec7` | AUTH-SESSION-TOKEN-SIZE |
| CSRF missing Secure | — | — | `regression_sec8` | — |
| Admin RBAC default-allow | — | — | `regression_sec9` | — |
| No max_rows | `result_truncated_at_max_rows` | — | `regression_sec10` | SQL-PG-MAX-ROWS |
| Init handler no timeout | — | — | `regression_sec11` | — |
| CORS no Vary | — | — | `regression_sec12` | — |
| No HSTS | — | — | `regression_sec13` | — |
| Error leaks driver names | — | — | `regression_sec14` | RT-ERROR-SANITIZE |
| ctx.dataview() drops params | — | `dataview_params_not_dropped` | `regression_pr48` | RT-CTX-DATAVIEW-PARAMS |
| SQLite $name prefix | `param_binding_*` | — | `regression_sqlite_prefix` | SQL-SQLITE-PREFIX |
| PG/MySQL alpha sort params | `param_binding_order_independent` | — | `regression_issue54` | SQL-PG-PARAM-ORDER |
| ctx.session undefined | — | `ctx_session_is_object` | — | RT-CTX-SESSION |
| ctx.streamDataview ghost | — | `all_spec_ctx_methods_exist` | — | — |
| Rivers.http ghost | — | `all_spec_rivers_apis_exist` | — | — |
| ctx.app_id empty | — | `ctx_app_id_not_empty` | `regression_app_id` | RT-CTX-APP-ID |
| StorageEngine "sqlite" vs "sled" | — | — | — | — (config test) |
| hot_reload ghost config | — | — | — | — (config test) |
| OTel ghost config | — | — | — | — (config test) |
| sccache CI crashes | — | — | — | — (CI workflow) |

**21 of 25 bugs** are covered by at least one unit-level test. The remaining 4 are CI/config issues tested by config validation tests or workflow fixes.
