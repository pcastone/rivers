//! `Rivers.__directDispatch(name, operation, parameters)` — V8 host callback for
//! direct-dispatch datasources.
//!
//! Intended to be called only from the typed-proxy codegen (Task 29d), not
//! directly from handler code. The proxy validates arguments before calling;
//! this callback trusts its inputs but still defends against wrong shapes.
//!
//! Behavior:
//! 1. Look up `name` in `TASK_DIRECT_DATASOURCES`. Throw `TypeError` if missing.
//! 2. Lazily build a `FilesystemConnection` for the stored root; cache for
//!    subsequent ops in the same task.
//! 3. Build a `Query` with `operation` + `parameters`, run via the task's
//!    tokio runtime handle.
//! 4. Marshal `QueryResult` back to JS using the "auto-unwrap" convention
//!    (spec: task plan 29 Option B):
//!    - 0 rows → return `null`
//!    - 1 row × 1 col → unwrap to the single scalar value
//!    - 1 row × N cols → return the row as an object
//!    - N rows → return an array of row objects

use std::collections::HashMap;

use rivers_runtime::rivers_core::drivers::filesystem::FilesystemDriver;
use rivers_runtime::rivers_driver_sdk::{
    ConnectionParams, DatabaseDriver, DriverError, Query, QueryResult, QueryValue,
};

use super::datasource::json_to_query_value;
use super::http::json_to_v8;
use super::task_locals::*;

/// V8 callback for `Rivers.__directDispatch(name, operation, parameters)`.
pub(super) fn rivers_direct_dispatch_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // ── Extract args ─────────────────────────────────────────────
    let name = args.get(0).to_rust_string_lossy(scope);
    let operation = args.get(1).to_rust_string_lossy(scope);
    let params_val = args.get(2);

    if name.is_empty() || operation.is_empty() {
        throw_type_error(scope, "__directDispatch: 'name' and 'operation' are required");
        return;
    }

    // Parse parameters object → HashMap<String, QueryValue>
    let parameters: HashMap<String, QueryValue> = if params_val.is_undefined() || params_val.is_null() {
        HashMap::new()
    } else if let Some(json_str) = v8::json::stringify(scope, params_val) {
        let s = json_str.to_rust_string_lossy(scope);
        match serde_json::from_str::<serde_json::Value>(&s) {
            Ok(serde_json::Value::Object(map)) => map
                .into_iter()
                .map(|(k, v)| (k, json_to_query_value(v)))
                .collect(),
            _ => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    // ── Resolve datasource + run op ──────────────────────────────
    let rt = match get_rt_handle() {
        Ok(rt) => rt,
        Err(_) => {
            throw_type_error(scope, "__directDispatch: tokio runtime handle not available");
            return;
        }
    };

    let result = TASK_DIRECT_DATASOURCES.with(|m| -> Result<QueryResult, String> {
        let map = m.borrow();
        let entry = map.get(&name).ok_or_else(|| {
            format!("datasource '{name}' is not a direct-dispatch datasource")
        })?;

        // Lazy-init the connection on first op in this task.
        if entry.connection.borrow().is_none() {
            let conn = rt.block_on(async {
                let params = ConnectionParams {
                    host: String::new(),
                    port: 0,
                    database: entry.root.display().to_string(),
                    username: String::new(),
                    password: String::new(),
                    options: HashMap::new(),
                };
                match entry.driver.as_str() {
                    "filesystem" => FilesystemDriver.connect(&params).await,
                    other => Err(DriverError::Unsupported(format!(
                        "direct dispatch: unknown driver '{other}'"
                    ))),
                }
                .map_err(|e| format!("connect failed: {e}"))
            })?;
            *entry.connection.borrow_mut() = Some(conn);
        }

        let mut query = Query::new(&name, "");
        query.operation = operation.clone();
        query.parameters = parameters;

        let mut conn_slot = entry.connection.borrow_mut();
        let conn = conn_slot.as_mut().expect("connection initialized above");
        rt.block_on(conn.execute(&query))
            .map_err(|e| format!("{operation}: {e}"))
    });

    match result {
        Ok(qr) => {
            let json = query_result_to_json(qr);
            match json_to_v8(scope, &json) {
                Ok(v) => rv.set(v),
                Err(_) => rv.set(v8::null(scope).into()),
            }
        }
        Err(msg) => throw_type_error(scope, &msg),
    }
}

/// Apply the auto-unwrap rules to a `QueryResult`.
fn query_result_to_json(qr: QueryResult) -> serde_json::Value {
    if qr.rows.is_empty() {
        return serde_json::Value::Null;
    }
    if qr.rows.len() == 1 {
        let row = &qr.rows[0];
        if row.len() == 1 {
            // Single scalar — unwrap.
            let (_k, v) = row.iter().next().unwrap();
            return query_value_to_json(v.clone());
        }
        // Single row, multiple columns → return as object.
        return row_to_json(row);
    }
    // Multiple rows → array of objects.
    serde_json::Value::Array(qr.rows.iter().map(row_to_json).collect())
}

fn row_to_json(row: &HashMap<String, QueryValue>) -> serde_json::Value {
    let mut obj = serde_json::Map::with_capacity(row.len());
    for (k, v) in row {
        obj.insert(k.clone(), query_value_to_json(v.clone()));
    }
    serde_json::Value::Object(obj)
}

fn query_value_to_json(v: QueryValue) -> serde_json::Value {
    // Per H18: delegate to QueryValue's threshold-aware Serialize so the V8
    // direct-dispatch path never hands a JS handler a silently-rounded
    // Number for a value above 2⁵³−1 — the canonical Serialize emits a JSON
    // string in that case.
    serde_json::to_value(&v).unwrap_or(serde_json::Value::Null)
}

fn throw_type_error(scope: &mut v8::HandleScope, msg: &str) {
    if let Some(msg) = v8::String::new(scope, msg) {
        let err = v8::Exception::type_error(scope, msg);
        scope.throw_exception(err);
    }
}

#[cfg(test)]
mod unwrap_tests {
    use super::*;

    fn row(entries: &[(&str, QueryValue)]) -> HashMap<String, QueryValue> {
        entries.iter().map(|(k, v)| ((*k).into(), v.clone())).collect()
    }

    fn qr(rows: Vec<HashMap<String, QueryValue>>) -> QueryResult {
        QueryResult {
            rows,
            affected_rows: 0,
            last_insert_id: None,
            column_names: None,
        }
    }

    #[test]
    fn zero_rows_returns_null() {
        let j = query_result_to_json(qr(vec![]));
        assert_eq!(j, serde_json::Value::Null);
    }

    #[test]
    fn single_scalar_unwraps() {
        let j = query_result_to_json(qr(vec![row(&[("content", QueryValue::String("world".into()))])]));
        assert_eq!(j, serde_json::json!("world"));
    }

    #[test]
    fn single_row_multi_col_returns_object() {
        let j = query_result_to_json(qr(vec![row(&[
            ("size", QueryValue::Integer(42)),
            ("isFile", QueryValue::Boolean(true)),
        ])]));
        assert_eq!(j["size"], 42);
        assert_eq!(j["isFile"], true);
    }

    #[test]
    fn multi_row_returns_array() {
        let j = query_result_to_json(qr(vec![
            row(&[("name", QueryValue::String("a.txt".into()))]),
            row(&[("name", QueryValue::String("b.txt".into()))]),
        ]));
        assert!(j.is_array());
        assert_eq!(j.as_array().unwrap().len(), 2);
    }
}
