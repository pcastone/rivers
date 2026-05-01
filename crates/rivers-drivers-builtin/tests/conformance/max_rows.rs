// max_rows truncation conformance tests.
//
// The driver must honor LIMIT in the SQL statement.
// (max_rows enforcement happens at the DataView engine layer via LIMIT injection;
// these tests verify the driver correctly returns at most N rows when LIMIT N is set.)

use rivers_driver_sdk::types::*;
use test_case::test_case;

use super::conformance::*;

/// LIMIT in the query must constrain the result set.
#[test_case("sqlite" ; "sqlite_max_rows")]
#[tokio::test]
async fn result_truncated_at_limit(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    // Seed 10 rows with unique tags
    let seed_tag = format!("maxrows-{}-{}", driver, std::process::id());
    for i in 0..10 {
        let tag = format!("{}-{}", seed_tag, i);
        let params = ordered_params(&[
            ("zname", QueryValue::String(tag)),
            ("age", QueryValue::Integer(i)),
        ]);
        let insert = make_insert_query(driver, &params);
        conn.execute(&insert).await.expect("seed INSERT failed");
    }

    // SELECT with LIMIT 5
    let stmt = match driver {
        "sqlite" => format!(
            "SELECT * FROM canary_records WHERE zname LIKE '{}%' LIMIT 5",
            seed_tag
        ),
        _ => format!(
            "SELECT * FROM canary_records WHERE zname LIKE '{}%' LIMIT 5",
            seed_tag
        ),
    };
    let query = Query {
        operation: "select".into(),
        target: "canary_records".into(),
        parameters: std::collections::HashMap::new(),
        statement: stmt,
    };

    let result = conn.execute(&query).await.expect("SELECT LIMIT failed");
    assert!(
        result.rows.len() <= 5,
        "LIMIT 5 must return at most 5 rows, got {}",
        result.rows.len()
    );

    // Cleanup: delete seeded rows
    for i in 0..10 {
        let tag = format!("{}-{}", seed_tag, i);
        let _ = cleanup_test_row(&mut *conn, driver, &tag).await;
    }
}

/// LIMIT 1 returns exactly one row even when multiple match.
#[test_case("sqlite" ; "sqlite_limit_one")]
#[tokio::test]
async fn limit_one_returns_single_row(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    let seed_tag = format!("limitone-{}-{}", driver, std::process::id());
    for i in 0..3 {
        let tag = format!("{}-{}", seed_tag, i);
        let params = ordered_params(&[
            ("zname", QueryValue::String(tag)),
            ("age", QueryValue::Integer(i)),
        ]);
        let insert = make_insert_query(driver, &params);
        conn.execute(&insert).await.expect("seed INSERT failed");
    }

    let stmt = format!(
        "SELECT * FROM canary_records WHERE zname LIKE '{}%' LIMIT 1",
        seed_tag
    );
    let query = Query {
        operation: "select".into(),
        target: "canary_records".into(),
        parameters: std::collections::HashMap::new(),
        statement: stmt,
    };

    let result = conn.execute(&query).await.expect("SELECT LIMIT 1 failed");
    assert_eq!(result.rows.len(), 1, "LIMIT 1 must return exactly 1 row");

    for i in 0..3 {
        let tag = format!("{}-{}", seed_tag, i);
        let _ = cleanup_test_row(&mut *conn, driver, &tag).await;
    }
}
