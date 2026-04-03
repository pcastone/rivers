//! Driver conformance test harness — shared infrastructure for cross-driver matrix tests.
//!
//! CI sets `RIVERS_TEST_CLUSTER=1` to enable live driver tests.
//! Without it, only SQLite (in-memory) and Faker run.

use std::collections::HashMap;

use rivers_driver_sdk::traits::*;
use rivers_driver_sdk::types::*;
use rivers_driver_sdk::DriverError;

// ── Cluster Guard ───────────────────────────────────────────────

/// Skip this test if the podman test cluster is not available.
/// Tests that only need SQLite/Faker should NOT call this.
pub fn skip_unless_cluster() {
    if std::env::var("RIVERS_TEST_CLUSTER").is_err() {
        eprintln!("RIVERS_TEST_CLUSTER not set — skipping cluster driver test");
        return;
    }
}

/// Returns true if the test cluster is available.
pub fn cluster_available() -> bool {
    std::env::var("RIVERS_TEST_CLUSTER").is_ok()
}

// ── Connection Factory ──────────────────────────────────────────

/// Connection params for each test cluster driver.
pub fn test_connection_params(driver: &str) -> Option<ConnectionParams> {
    match driver {
        "postgres" => Some(ConnectionParams {
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
        _ => None,
    }
}

/// Create a live connection for a named driver.
/// Returns None if the driver requires the cluster and it's unavailable.
pub async fn make_connection(driver: &str) -> Option<Box<dyn Connection>> {
    let params = test_connection_params(driver)?;

    // SQLite doesn't need cluster
    if driver != "sqlite" && !cluster_available() {
        return None;
    }

    let drv: Box<dyn DatabaseDriver> = match driver {
        "sqlite" => Box::new(rivers_drivers_builtin::SqliteDriver),
        "postgres" => Box::new(rivers_drivers_builtin::PostgresDriver),
        "mysql" => Box::new(rivers_drivers_builtin::MysqlDriver),
        "redis" => Box::new(rivers_drivers_builtin::RedisDriver),
        _ => return None,
    };

    drv.connect(&params).await.ok()
}

// ── Parameter Helpers ───────────────────────────────────────────

/// Build a HashMap from ordered pairs. HashMap doesn't preserve order —
/// that's the point: the driver must NOT depend on iteration order.
pub fn ordered_params(pairs: &[(&str, QueryValue)]) -> HashMap<String, QueryValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

// ── SQL Helpers ─────────────────────────────────────────────────

/// Create the canary_records test table if it doesn't exist.
pub async fn setup_test_table(conn: &mut dyn Connection, driver: &str) -> Result<(), DriverError> {
    let ddl = match driver {
        "postgres" => {
            "CREATE TABLE IF NOT EXISTS canary_records (\
             id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text, \
             zname TEXT NOT NULL, \
             age INTEGER NOT NULL, \
             email TEXT, \
             created_at TIMESTAMPTZ DEFAULT NOW())"
        }
        "mysql" => {
            "CREATE TABLE IF NOT EXISTS canary_records (\
             id CHAR(36) PRIMARY KEY, \
             zname VARCHAR(255) NOT NULL, \
             age INT NOT NULL, \
             email VARCHAR(255), \
             created_at DATETIME DEFAULT CURRENT_TIMESTAMP)"
        }
        "sqlite" => {
            "CREATE TABLE IF NOT EXISTS canary_records (\
             id TEXT PRIMARY KEY, \
             zname TEXT NOT NULL, \
             age INTEGER NOT NULL, \
             email TEXT, \
             created_at TEXT DEFAULT (datetime('now')))"
        }
        _ => return Ok(()),
    };

    conn.ddl_execute(&Query::new("canary_records", ddl)).await?;
    Ok(())
}

/// Build an INSERT query for the canary_records table.
pub fn make_insert_query(
    driver: &str,
    params: &HashMap<String, QueryValue>,
) -> Query {
    let stmt = match driver {
        "postgres" => "INSERT INTO canary_records (zname, age) VALUES ($zname, $age) RETURNING *",
        "mysql" | "sqlite" => "INSERT INTO canary_records (id, zname, age) VALUES ($id, $zname, $age)",
        _ => "",
    };

    let mut p = params.clone();
    // Add ID for MySQL/SQLite (no auto-gen)
    if driver == "mysql" || driver == "sqlite" {
        if !p.contains_key("id") {
            p.insert("id".into(), QueryValue::String(uuid::Uuid::new_v4().to_string()));
        }
    }

    Query {
        operation: "insert".into(),
        target: "canary_records".into(),
        parameters: p,
        statement: stmt.into(),
    }
}

/// Build a SELECT query filtering by zname.
pub fn make_select_by_zname_query(
    driver: &str,
    params: &HashMap<String, QueryValue>,
) -> Query {
    let stmt = match driver {
        "postgres" | "mysql" | "sqlite" => "SELECT * FROM canary_records WHERE zname = $zname",
        _ => "",
    };

    Query {
        operation: "select".into(),
        target: "canary_records".into(),
        parameters: params.clone(),
        statement: stmt.into(),
    }
}

/// Build an UPDATE query setting age where zname matches.
pub fn make_update_query(
    driver: &str,
    params: &HashMap<String, QueryValue>,
) -> Query {
    let stmt = match driver {
        "postgres" | "mysql" | "sqlite" => "UPDATE canary_records SET age = $age WHERE zname = $zname",
        _ => "",
    };

    Query {
        operation: "update".into(),
        target: "canary_records".into(),
        parameters: params.clone(),
        statement: stmt.into(),
    }
}

/// Build a DELETE query by zname.
pub fn make_delete_query(
    driver: &str,
    params: &HashMap<String, QueryValue>,
) -> Query {
    let stmt = match driver {
        "postgres" | "mysql" | "sqlite" => "DELETE FROM canary_records WHERE zname = $zname",
        _ => "",
    };

    Query {
        operation: "delete".into(),
        target: "canary_records".into(),
        parameters: params.clone(),
        statement: stmt.into(),
    }
}

/// Delete a test row by zname (cleanup).
pub async fn cleanup_test_row(
    conn: &mut dyn Connection,
    driver: &str,
    zname: &str,
) -> Result<(), DriverError> {
    let params = ordered_params(&[("zname", QueryValue::String(zname.into()))]);
    let q = make_delete_query(driver, &params);
    conn.execute(&q).await.map(|_| ())
}
