// H18.2 — MySQL `BIGINT UNSIGNED` round-trip via QueryValue::UInt.
//
// Verifies the driver:
//   1. Decodes mysql_async::Value::UInt → QueryValue::UInt(u64) (no i64 cast).
//   2. Re-binds QueryValue::UInt → mysql_async::Value::UInt losslessly.
//   3. Serializes UInt to JSON per the H18.1 threshold (numbers ≤ 2⁵³−1,
//      strings above).
//
// Cluster-gated: requires `RIVERS_TEST_CLUSTER=1` and a reachable MySQL at
// 192.168.2.215 (`rivers / rivers_test / rivers`). Falls through silently
// when the cluster guard is off, matching the rest of `conformance_tests`.

use rivers_driver_sdk::types::*;

use super::conformance::*;

const SAFE_BELOW: u64 = 9_007_199_254_740_991; // 2^53 - 1
const ABOVE_SAFE: u64 = 9_007_199_254_740_992; // 2^53
const NEAR_U64_MAX: u64 = 18_446_744_073_709_551_610; // u64::MAX - 5

#[tokio::test]
async fn mysql_bigint_unsigned_round_trip() {
    if !cluster_available() {
        eprintln!("RIVERS_TEST_CLUSTER not set — skipping mysql_bigint_unsigned_round_trip");
        return;
    }
    let Some(mut conn) = make_connection("mysql").await else {
        eprintln!("could not connect to mysql cluster — skipping");
        return;
    };

    // Use a unique table name so concurrent runs don't collide.
    let table = format!("h18_uint_roundtrip_{}", std::process::id());
    let drop_stmt = format!("DROP TABLE IF EXISTS {table}");
    let create_stmt = format!(
        "CREATE TABLE {table} (id BIGINT UNSIGNED PRIMARY KEY, label VARCHAR(64) NOT NULL)"
    );

    // DDL must go through ddl_execute (the runtime DDL guard rejects
    // CREATE/DROP via the regular execute() path).
    let _ = conn
        .ddl_execute(&Query::new(&table, &drop_stmt))
        .await;
    conn.ddl_execute(&Query::new(&table, &create_stmt))
        .await
        .expect("CREATE TABLE for h18_uint_roundtrip");

    // Five representative values covering the threshold cliff and the
    // i64::MAX cliff that pre-H18 code would have silently truncated.
    let values: [(u64, &str); 5] = [
        (0, "zero"),
        (42, "small"),
        (SAFE_BELOW, "safe_below"),
        (ABOVE_SAFE, "above_safe"),
        (NEAR_U64_MAX, "near_u64_max"),
    ];

    // Insert each row.
    for (val, label) in values.iter() {
        let mut params = std::collections::HashMap::new();
        params.insert("001".to_string(), QueryValue::UInt(*val));
        params.insert("002".to_string(), QueryValue::String((*label).to_string()));
        let stmt = format!("INSERT INTO {table} (id, label) VALUES (?, ?)");
        let mut q = Query::with_operation("insert", &table, &stmt);
        q.parameters = params;
        conn.execute(&q)
            .await
            .unwrap_or_else(|e| panic!("INSERT for {label}={val}: {e:?}"));
    }

    // Read each row back. Use ordered SELECTs so we know which value we
    // got even if the driver's row HashMap iteration is unordered.
    for (val, label) in values.iter() {
        let mut params = std::collections::HashMap::new();
        params.insert("001".to_string(), QueryValue::UInt(*val));
        let stmt = format!("SELECT id, label FROM {table} WHERE id = ?");
        let mut q = Query::with_operation("select", &table, &stmt);
        q.parameters = params;
        let result = conn
            .execute(&q)
            .await
            .unwrap_or_else(|e| panic!("SELECT for {label}={val}: {e:?}"));
        assert_eq!(result.rows.len(), 1, "row not found for {label}={val}");
        let row = &result.rows[0];

        // Variant assertion — must be UInt (not Integer, not String).
        match row.get("id") {
            Some(QueryValue::UInt(got)) => {
                assert_eq!(*got, *val, "value mismatch for {label}");
            }
            other => panic!(
                "expected QueryValue::UInt({val}) for {label}, got {other:?}"
            ),
        }

        // JSON-serialization assertion — threshold check.
        let qv = QueryValue::UInt(*val);
        let json = serde_json::to_value(&qv).expect("UInt serialize");
        if *val <= SAFE_BELOW {
            assert!(
                json.is_number(),
                "{label}={val} should serialize as JSON number, got {json:?}"
            );
        } else {
            // Above the safe threshold → JSON string carrying decimal repr.
            assert_eq!(
                json,
                serde_json::Value::String(val.to_string()),
                "{label}={val} should serialize as decimal string"
            );
        }
    }

    // Cleanup.
    let _ = conn
        .ddl_execute(&Query::new(&table, &drop_stmt))
        .await;
}
