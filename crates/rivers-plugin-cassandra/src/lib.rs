#![warn(missing_docs)]
//! Cassandra plugin driver (DatabaseDriver).
//!
//! Implements `DatabaseDriver` using the `scylla` crate (pure Rust async driver).
//! Compatible with Apache Cassandra 4.x and ScyllaDB.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use scylla::frame::response::result::CqlValue;
use scylla::transport::session::Session;
use scylla::SessionBuilder;
use tracing::debug;

use rivers_driver_sdk::{
    ABI_VERSION, Connection, ConnectionParams, DatabaseDriver, DriverError, DriverRegistrar,
    Query, QueryResult, QueryValue, read_max_rows,
};

// ── Driver ─────────────────────────────────────────────────────────────

/// Cassandra driver factory — creates connections via the `scylla` async driver.
pub struct CassandraDriver;

#[async_trait]
impl DatabaseDriver for CassandraDriver {
    fn name(&self) -> &str {
        "cassandra"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let port = if params.port == 0 { 9042 } else { params.port };
        let addr = format!("{}:{}", params.host, port);

        let mut builder = SessionBuilder::new().known_node(&addr);

        if !params.database.is_empty() {
            builder = builder.use_keyspace(&params.database, false);
        }

        if !params.username.is_empty() {
            builder = builder.user(&params.username, &params.password);
        }

        let session = builder
            .build()
            .await
            .map_err(|e| DriverError::Connection(format!("cassandra connect: {e}")))?;

        debug!(host = %params.host, port = %port, keyspace = %params.database, "cassandra: connected");

        let max_rows = read_max_rows(params);
        Ok(Box::new(CassandraConnection { session, max_rows }))
    }

    fn supports_transactions(&self) -> bool { false }
    fn supports_prepared_statements(&self) -> bool { true }
    fn param_style(&self) -> rivers_driver_sdk::ParamStyle {
        rivers_driver_sdk::ParamStyle::ColonNamed
    }
    /// G_R7.2: cdylib plugins ship their own statically-linked tokio
    /// runtime — the host must isolate connect() in a fresh runtime.
    fn needs_isolated_runtime(&self) -> bool { true }
}

// ── Connection ─────────────────────────────────────────────────────────

/// Active Cassandra connection wrapping a scylla session.
pub struct CassandraConnection {
    session: Session,
    max_rows: usize,
}

#[async_trait]
impl Connection for CassandraConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL/admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        match query.operation.as_str() {
            "select" | "query" | "get" | "find" => self.exec_query(query).await,
            "insert" | "create" | "update" | "delete" | "remove" | "del" => {
                self.exec_write(query).await
            }
            "ping" => {
                self.ping().await?;
                Ok(QueryResult { rows: Vec::new(), affected_rows: 0, last_insert_id: None, column_names: None })
            }
            other => Err(DriverError::Unsupported(format!(
                "cassandra: unsupported operation '{other}'"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        self.session
            .query_unpaged("SELECT now() FROM system.local", &[])
            .await
            .map_err(|e| DriverError::Connection(format!("cassandra ping: {e}")))?;
        Ok(())
    }

    fn driver_name(&self) -> &str { "cassandra" }
}

impl CassandraConnection {
    async fn exec_query(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let values = build_named_values(&query.parameters)?;

        let prepared = self
            .session
            .prepare(query.statement.as_str())
            .await
            .map_err(|e| DriverError::Query(format!("cassandra prepare: {e}")))?;

        let result = self
            .session
            .execute_unpaged(&prepared, &values)
            .await
            .map_err(|e| DriverError::Query(format!("cassandra query: {e}")))?;

        let col_specs: Vec<String> = result
            .col_specs()
            .iter()
            .map(|c| c.name.clone())
            .collect();

        let raw_rows = result.rows_or_empty();
        let mut rows = Vec::with_capacity(raw_rows.len().min(self.max_rows));

        for row in raw_rows {
            if rows.len() >= self.max_rows {
                tracing::warn!(
                    max_rows = self.max_rows,
                    "cassandra: result set truncated to max_rows limit"
                );
                break;
            }
            let mut map = HashMap::new();
            for (i, col_name) in col_specs.iter().enumerate() {
                let val = row.columns.get(i).and_then(|c| c.as_ref());
                map.insert(col_name.clone(), cql_value_to_query_value(val));
            }
            rows.push(map);
        }

        let count = rows.len() as u64;
        Ok(QueryResult { rows, affected_rows: count, last_insert_id: None, column_names: None })
    }

    async fn exec_write(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let values = build_named_values(&query.parameters)?;

        let prepared = self
            .session
            .prepare(query.statement.as_str())
            .await
            .map_err(|e| DriverError::Query(format!("cassandra prepare: {e}")))?;

        self.session
            .execute_unpaged(&prepared, &values)
            .await
            .map_err(|e| DriverError::Query(format!("cassandra write: {e}")))?;

        // CQL does not return affected row counts for non-LWT writes; report 0 (unknown).
        Ok(QueryResult { rows: Vec::new(), affected_rows: 0, last_insert_id: None, column_names: None })
    }
}

// ── Type Conversion ──────────────────────────────────────────────────

/// Build a named-value map for prepared-statement binding.
///
/// Uses `HashMap<String, CqlValue>` which implements `SerializeRow` via
/// column-name matching in scylla 0.14+. This avoids the previous
/// alphabetical-sort approach that silently corrupted data when CQL
/// positional `?` placeholders didn't match alphabetical parameter order. (AP16)
fn build_named_values(
    parameters: &HashMap<String, QueryValue>,
) -> Result<HashMap<String, CqlValue>, DriverError> {
    parameters
        .iter()
        .map(|(k, v)| query_value_to_cql(v).map(|cql| (k.clone(), cql)))
        .collect()
}

fn query_value_to_cql(val: &QueryValue) -> Result<CqlValue, DriverError> {
    Ok(match val {
        QueryValue::Null => CqlValue::Empty,
        QueryValue::Boolean(b) => CqlValue::Boolean(*b),
        QueryValue::Integer(i) => CqlValue::BigInt(*i),
        // Cassandra has no native unsigned 64. Bind as BigInt(i64) when it
        // fits, else error rather than silently truncating.
        QueryValue::UInt(u) => {
            let i = i64::try_from(*u).map_err(|_| {
                DriverError::Connection(format!(
                    "cassandra binding overflow: u64 value {u} exceeds i64 range \
                     — Cassandra `bigint` is 64-bit signed; use a `varint` or text column"
                ))
            })?;
            CqlValue::BigInt(i)
        }
        QueryValue::Float(f) => CqlValue::Double(*f),
        QueryValue::String(s) => CqlValue::Text(s.clone()),
        QueryValue::Array(_) => CqlValue::Text(serde_json::to_string(val).unwrap_or_default()),
        QueryValue::Json(v) => CqlValue::Text(serde_json::to_string(v).unwrap_or_default()),
    })
}

fn cql_value_to_query_value(val: Option<&CqlValue>) -> QueryValue {
    match val {
        None | Some(CqlValue::Empty) => QueryValue::Null,
        Some(CqlValue::Boolean(b)) => QueryValue::Boolean(*b),
        Some(CqlValue::Int(i)) => QueryValue::Integer(*i as i64),
        Some(CqlValue::BigInt(i)) => QueryValue::Integer(*i),
        Some(CqlValue::SmallInt(i)) => QueryValue::Integer(*i as i64),
        Some(CqlValue::TinyInt(i)) => QueryValue::Integer(*i as i64),
        Some(CqlValue::Float(f)) => QueryValue::Float(*f as f64),
        Some(CqlValue::Double(d)) => QueryValue::Float(*d),
        Some(CqlValue::Text(s)) => QueryValue::String(s.clone()),
        Some(CqlValue::Ascii(s)) => QueryValue::String(s.clone()),
        Some(CqlValue::Uuid(u)) => QueryValue::String(u.to_string()),
        Some(CqlValue::Timeuuid(u)) => QueryValue::String(u.as_ref().to_string()),
        Some(CqlValue::Timestamp(ts)) => QueryValue::Integer(ts.0),
        Some(CqlValue::Date(d)) => QueryValue::Integer(d.0 as i64),
        Some(CqlValue::Counter(c)) => QueryValue::Integer(c.0),
        Some(CqlValue::Blob(b)) => QueryValue::String(hex::encode(b)),
        Some(CqlValue::List(items)) => {
            QueryValue::Array(items.iter().map(|v| cql_value_to_query_value(Some(v))).collect())
        }
        Some(CqlValue::Set(items)) => {
            QueryValue::Array(items.iter().map(|v| cql_value_to_query_value(Some(v))).collect())
        }
        Some(other) => QueryValue::String(format!("{:?}", other)),
    }
}

// ── Plugin ABI ─────────────────────────────────────────────────────────

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 { ABI_VERSION }

#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(CassandraDriver));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::DatabaseDriver;

    #[test]
    fn driver_name() { assert_eq!(CassandraDriver.name(), "cassandra"); }

    #[test]
    fn abi_version_matches() { assert_eq!(ABI_VERSION, 1); }

    #[test]
    fn query_value_to_cql_string() {
        assert!(matches!(
            query_value_to_cql(&QueryValue::String("hi".into())).unwrap(),
            CqlValue::Text(ref s) if s == "hi",
        ));
    }

    #[test]
    fn query_value_to_cql_integer() {
        assert!(matches!(
            query_value_to_cql(&QueryValue::Integer(42)).unwrap(),
            CqlValue::BigInt(42),
        ));
    }

    #[test]
    fn query_value_to_cql_uint_in_range() {
        assert!(matches!(
            query_value_to_cql(&QueryValue::UInt(42)).unwrap(),
            CqlValue::BigInt(42),
        ));
    }

    #[test]
    fn query_value_to_cql_uint_overflow_errors() {
        let err = query_value_to_cql(&QueryValue::UInt(u64::MAX)).unwrap_err();
        match err {
            DriverError::Connection(msg) => {
                assert!(msg.contains("u64 value"), "expected overflow message, got: {msg}");
            }
            other => panic!("expected DriverError::Connection, got {other:?}"),
        }
    }

    #[test]
    fn cql_to_query_value_null() {
        assert_eq!(cql_value_to_query_value(None), QueryValue::Null);
    }

    #[test]
    fn cql_to_query_value_text() {
        assert_eq!(cql_value_to_query_value(Some(&CqlValue::Text("x".into()))), QueryValue::String("x".into()));
    }

    #[test]
    fn build_named_values_preserves_keys() {
        let mut p = HashMap::new();
        p.insert("z".into(), QueryValue::Integer(2));
        p.insert("a".into(), QueryValue::Integer(1));
        let v = build_named_values(&p).unwrap();
        assert!(matches!(v.get("a"), Some(CqlValue::BigInt(1))));
        assert!(matches!(v.get("z"), Some(CqlValue::BigInt(2))));
    }

    #[tokio::test]
    async fn connect_bad_host_returns_error() {
        let params = ConnectionParams {
            host: "127.0.0.1".into(), port: 1, database: "".into(),
            username: "".into(), password: "".into(), options: HashMap::new(),
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), CassandraDriver.connect(&params)).await;
        match result {
            Ok(Err(DriverError::Connection(_))) => {}
            Ok(Ok(_)) => panic!("expected error"),
            _ => {} // timeout OK
        }
    }

    // ── RW4.2.b: max_rows default is 10_000 ─────────────────────────────
    #[test]
    fn max_rows_default_is_read_from_sdk() {
        let params = ConnectionParams {
            host: "h".into(), port: 9042, database: "".into(),
            username: "".into(), password: "".into(), options: HashMap::new(),
        };
        assert_eq!(rivers_driver_sdk::read_max_rows(&params), 10_000);
    }

    #[test]
    fn max_rows_from_option_overrides_default() {
        let mut opts = HashMap::new();
        opts.insert("max_rows".into(), "50".into());
        let params = ConnectionParams {
            host: "h".into(), port: 9042, database: "".into(),
            username: "".into(), password: "".into(), options: opts,
        };
        assert_eq!(rivers_driver_sdk::read_max_rows(&params), 50);
    }
}
