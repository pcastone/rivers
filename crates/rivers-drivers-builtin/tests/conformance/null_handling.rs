// NULL value round-trip conformance tests.
//
// NULL must survive INSERT → SELECT without becoming empty string or zero.

use rivers_driver_sdk::types::*;
use test_case::test_case;

use super::conformance::*;

#[test_case("sqlite" ; "sqlite_null")]
#[tokio::test]
async fn null_value_round_trip(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    let tag = format!("null-{}-{}", driver, std::process::id());

    let params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age", QueryValue::Integer(1)),
        ("email", QueryValue::Null),
    ]);
    let insert = make_insert_with_email_query(driver, &params);
    conn.execute(&insert).await.expect("INSERT with NULL email failed");

    let sel_params = ordered_params(&[("zname", QueryValue::String(tag.clone()))]);
    let select = make_select_by_zname_query(driver, &sel_params);
    let result = conn.execute(&select).await.expect("SELECT failed");

    assert!(!result.rows.is_empty(), "row should exist after INSERT");
    let email = result.rows[0].get("email");
    assert!(
        email == Some(&QueryValue::Null) || email.is_none(),
        "NULL should round-trip as Null, got: {:?}", email
    );

    let _ = cleanup_test_row(&mut *conn, driver, &tag).await;
}

/// A non-NULL value must NOT be silently converted to NULL.
#[test_case("sqlite" ; "sqlite_nonnull")]
#[tokio::test]
async fn non_null_value_survives_round_trip(driver: &str) {
    let Some(mut conn) = make_connection(driver).await else { return };
    let _ = setup_test_table(&mut *conn, driver).await;

    let tag = format!("nonnull-{}-{}", driver, std::process::id());

    let params = ordered_params(&[
        ("zname", QueryValue::String(tag.clone())),
        ("age", QueryValue::Integer(42)),
        ("email", QueryValue::String("alice@example.com".into())),
    ]);
    let insert = make_insert_with_email_query(driver, &params);
    conn.execute(&insert).await.expect("INSERT with email failed");

    let sel_params = ordered_params(&[("zname", QueryValue::String(tag.clone()))]);
    let select = make_select_by_zname_query(driver, &sel_params);
    let result = conn.execute(&select).await.expect("SELECT failed");

    assert!(!result.rows.is_empty());
    let email = result.rows[0].get("email");
    assert_eq!(
        email,
        Some(&QueryValue::String("alice@example.com".into())),
        "non-NULL string email should survive round-trip"
    );

    let _ = cleanup_test_row(&mut *conn, driver, &tag).await;
}
