// DDL guard conformance tests — verify DDL is rejected on Connection::execute().
//
// BUG-001: DDL statements executed unchecked through Connection::execute().
// Gate 1 of the three-gate DDL enforcement model.

use std::collections::HashMap;

use rivers_driver_sdk::types::*;
use rivers_driver_sdk::DriverError;
use test_case::test_case;

use super::conformance::*;

// ── DDL Rejection on execute() — SQLite (no cluster needed) ─────

#[test_case("sqlite", "DROP TABLE canary_records"              ; "sqlite_drop")]
#[test_case("sqlite", "CREATE TABLE evil (id INT)"             ; "sqlite_create")]
#[test_case("sqlite", "ALTER TABLE canary_records ADD col INT"  ; "sqlite_alter")]
#[tokio::test]
async fn ddl_rejected_on_execute(driver: &str, statement: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };

    let query = Query {
        operation: String::new(),
        target: String::new(),
        parameters: HashMap::new(),
        statement: statement.into(),
    };

    let result = conn.execute(&query).await;
    assert!(
        matches!(&result, Err(DriverError::Forbidden(_))),
        "DDL '{}' should be Forbidden on {}, got: {:?}",
        statement, driver, result
    );
}

// ── DDL Detection Edge Cases (SQLite only — no cluster needed) ──

#[test_case("  DROP TABLE x"                ; "leading_whitespace")]
#[test_case("drop table x"                  ; "lowercase")]
#[test_case("Drop Table x"                  ; "mixed_case")]
#[test_case("   \t\nDROP TABLE x"           ; "tabs_newlines")]
#[test_case("TRUNCATE TABLE x"              ; "truncate")]
#[tokio::test]
async fn ddl_detection_edge_cases(statement: &str) {
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
        "DDL edge case '{}' should be Forbidden, got: {:?}",
        statement, result
    );
}

// ── Cluster-only DDL tests ──────────────────────────────────────

#[test_case("postgres", "DROP TABLE canary_records"              ; "pg_drop")]
#[test_case("postgres", "CREATE TABLE evil (id INT)"             ; "pg_create")]
#[test_case("mysql",    "DROP TABLE canary_records"              ; "mysql_drop")]
#[test_case("mysql",    "CREATE TABLE evil (id INT)"             ; "mysql_create")]
#[tokio::test]
async fn ddl_rejected_cluster_drivers(driver: &str, statement: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };

    let query = Query {
        operation: String::new(),
        target: String::new(),
        parameters: HashMap::new(),
        statement: statement.into(),
    };

    let result = conn.execute(&query).await;
    assert!(
        matches!(&result, Err(DriverError::Forbidden(_))),
        "DDL '{}' should be Forbidden on {}, got: {:?}",
        statement, driver, result
    );
}
