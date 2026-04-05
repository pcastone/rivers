// CRUD lifecycle conformance tests — insert/select/update/delete round-trip.
//
// Verifies full data lifecycle for each driver.

use rivers_driver_sdk::types::*;
use test_case::test_case;

use super::conformance::*;

// ── Full CRUD Round-Trip (SQL drivers) ──────────────────────────

#[test_case("sqlite" ; "sqlite_crud")]
#[tokio::test]
async fn full_crud_lifecycle(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    let tag = format!("crud-{}-{}", driver, std::process::id());

    // INSERT
    let insert_params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age", QueryValue::Integer(25)),
    ]);
    let insert = make_insert_query(driver, &insert_params);
    let insert_result = conn
        .execute(&insert)
        .await
        .expect("INSERT failed");
    assert!(
        insert_result.affected_rows >= 1 || !insert_result.rows.is_empty(),
        "INSERT should affect at least 1 row"
    );

    // SELECT — verify insert
    let select_params = ordered_params(&[("zname", QueryValue::String(tag.clone()))]);
    let select = make_select_by_zname_query(driver, &select_params);
    let select_result = conn.execute(&select).await.expect("SELECT after INSERT failed");
    assert!(!select_result.rows.is_empty(), "row should exist after INSERT");
    assert_eq!(
        select_result.rows[0].get("age"),
        Some(&QueryValue::Integer(25))
    );

    // UPDATE
    let update_params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age", QueryValue::Integer(99)),
    ]);
    let update = make_update_query(driver, &update_params);
    conn.execute(&update).await.expect("UPDATE failed");

    // SELECT — verify update
    let select2 = conn.execute(&select).await.expect("SELECT after UPDATE failed");
    assert_eq!(
        select2.rows[0].get("age"),
        Some(&QueryValue::Integer(99)),
        "age should be 99 after UPDATE"
    );

    // DELETE
    let delete_params = ordered_params(&[("zname", QueryValue::String(tag.clone()))]);
    let delete = make_delete_query(driver, &delete_params);
    conn.execute(&delete).await.expect("DELETE failed");

    // SELECT — verify delete
    let select3 = conn.execute(&select).await.expect("SELECT after DELETE failed");
    assert!(select3.rows.is_empty(), "row should not exist after DELETE");
}

// ── Cluster-only CRUD tests ─────────────────────────────────────

#[test_case("postgres" ; "pg_crud")]
#[test_case("mysql"    ; "mysql_crud")]
#[tokio::test]
async fn full_crud_cluster(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    let tag = format!("crud-{}-{}", driver, std::process::id());

    let insert_params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age", QueryValue::Integer(33)),
    ]);
    let insert = make_insert_query(driver, &insert_params);
    conn.execute(&insert).await.expect("INSERT failed");

    let select_params = ordered_params(&[("zname", QueryValue::String(tag.clone()))]);
    let select = make_select_by_zname_query(driver, &select_params);
    let result = conn.execute(&select).await.expect("SELECT failed");
    assert!(!result.rows.is_empty(), "row should exist after INSERT");

    let _ = cleanup_test_row(&mut *conn, driver, &tag).await;
}
