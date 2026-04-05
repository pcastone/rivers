// Parameter binding conformance tests — verify parameter order independence.
//
// BUG-004 (Issue #54): PostgreSQL, MySQL, and SQLite all had different
// parameter binding bugs. Alphabetical sorting caused silent data corruption
// when param order != alpha order.

use std::collections::HashMap;

use rivers_driver_sdk::types::*;
use test_case::test_case;

use super::conformance::*;

// ── Parameter Order Independence ────────────────────────────────
// Parameters declared as [zname, age] — alphabetical order is [age, zname].
// If the driver sorts alphabetically and binds positionally, zname gets
// age's value and vice versa.

#[test_case("sqlite" ; "sqlite_param_order")]
#[tokio::test]
async fn param_binding_order_independent(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    let tag = format!("param-{}-{}", driver, std::process::id());

    // Insert with params where alpha order != declaration order
    let params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age", QueryValue::Integer(42)),
    ]);
    let insert = make_insert_query(driver, &params);
    conn.execute(&insert).await.expect("INSERT should succeed");

    // Read back and verify values are in correct columns
    let select_params = ordered_params(&[("zname", QueryValue::String(tag.clone()))]);
    let select = make_select_by_zname_query(driver, &select_params);
    let result = conn.execute(&select).await.expect("SELECT should succeed");

    assert!(!result.rows.is_empty(), "should find inserted row");
    let row = &result.rows[0];

    // THE assertion: if param binding is order-dependent,
    // zname will contain "42" and age will contain the tag (or error)
    assert_eq!(
        row.get("zname"),
        Some(&QueryValue::String(tag.clone())),
        "zname column has wrong value — param binding order bug"
    );
    assert_eq!(
        row.get("age"),
        Some(&QueryValue::Integer(42)),
        "age column has wrong value — param binding order bug"
    );

    let _ = cleanup_test_row(&mut *conn, driver, &tag).await;
}

// ── Cluster-only parameter binding tests ────────────────────────

#[test_case("postgres" ; "pg_param_order")]
#[test_case("mysql"    ; "mysql_param_order")]
#[tokio::test]
async fn param_binding_cluster(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    let tag = format!("param-{}-{}", driver, std::process::id());

    let params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age", QueryValue::Integer(77)),
    ]);
    let insert = make_insert_query(driver, &params);
    conn.execute(&insert).await.expect("INSERT should succeed");

    let select_params = ordered_params(&[("zname", QueryValue::String(tag.clone()))]);
    let select = make_select_by_zname_query(driver, &select_params);
    let result = conn.execute(&select).await.expect("SELECT should succeed");

    assert!(!result.rows.is_empty(), "should find inserted row");
    assert_eq!(
        result.rows[0].get("age"),
        Some(&QueryValue::Integer(77)),
        "age column has wrong value — param binding order bug on {}",
        driver
    );

    let _ = cleanup_test_row(&mut *conn, driver, &tag).await;
}

// ── Empty params — should not crash ─────────────────────────────

#[test_case("sqlite" ; "sqlite_empty")]
#[tokio::test]
async fn param_binding_empty_params(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };

    let query = Query {
        operation: "select".into(),
        target: "canary_records".into(),
        parameters: HashMap::new(),
        statement: "SELECT 1".into(),
    };

    let result = conn.execute(&query).await;
    assert!(
        result.is_ok(),
        "empty params should not cause error: {:?}",
        result.err()
    );
}
