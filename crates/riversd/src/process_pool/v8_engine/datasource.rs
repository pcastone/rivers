//! `ctx_datasource_build_callback` and `json_to_query_value()`.

use std::collections::HashMap;

use super::task_locals::*;

/// Native V8 callback for `__ds_build(datasource_name, sql, params)` (X7).
///
/// Called by `ctx.datasource(name).fromQuery(sql).build()`.
/// Resolves the datasource token -> DriverFactory -> Connection -> execute.
/// Returns the query result as a V8 value.
///
/// V8 callback -- cannot return Result.
pub(super) fn ctx_datasource_build_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let ds_name = args.get(0).to_rust_string_lossy(scope);
    let sql_val = args.get(1);
    let params_val = args.get(2);

    // Check capability: datasource must be declared in TaskContext.datasources
    let is_declared = TASK_DS_CONFIGS.with(|c| c.borrow().contains_key(&ds_name));
    if !is_declared {
        let msg = v8::String::new(
            scope,
            &format!("CapabilityError: datasource '{}' not declared in view config", ds_name),
        ).unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // Require a SQL statement from .fromQuery()
    if sql_val.is_undefined() || sql_val.is_null() {
        let msg = v8::String::new(scope, "ctx.datasource().build(): call .fromQuery(sql) before .build()").unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }
    let sql = sql_val.to_rust_string_lossy(scope);

    // Extract params if provided
    let query_params: HashMap<String, rivers_runtime::rivers_driver_sdk::types::QueryValue> =
        if params_val.is_undefined() || params_val.is_null() {
            HashMap::new()
        } else if let Some(json_str) = v8::json::stringify(scope, params_val) {
            let json_string = json_str.to_rust_string_lossy(scope);
            // Try to parse as a JSON object and convert values to QueryValue
            match serde_json::from_str::<serde_json::Value>(&json_string) {
                Ok(serde_json::Value::Object(map)) => {
                    map.into_iter()
                        .map(|(k, v)| (k, json_to_query_value(v)))
                        .collect()
                }
                _ => HashMap::new(),
            }
        } else {
            HashMap::new()
        };

    // Get the DriverFactory and resolved config
    let factory = TASK_DRIVER_FACTORY.with(|f| f.borrow().clone());
    let ds_config = TASK_DS_CONFIGS.with(|c| c.borrow().get(&ds_name).cloned());

    let (factory, config) = match (factory, ds_config) {
        (Some(f), Some(c)) => (f, c),
        _ => {
            let msg = v8::String::new(
                scope,
                &format!("ctx.datasource('{}').build(): DriverFactory not available", ds_name),
            ).unwrap();
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    // Execute via async bridge: connect -> build query -> execute
    let rt = match get_rt_handle() {
        Ok(rt) => rt,
        Err(_) => {
            let msg = v8::String::new(scope, "ctx.datasource().build(): runtime handle not available").unwrap();
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    let result = rt.block_on(async {
        let mut conn = factory.connect(&config.driver_name, &config.params).await
            .map_err(|e| format!("connection failed: {e}"))?;

        let mut query = rivers_runtime::rivers_driver_sdk::types::Query::new(&ds_name, &sql);
        for (k, v) in query_params {
            query.parameters.insert(k, v);
        }

        conn.execute(&query).await
            .map_err(|e| format!("query failed: {e}"))
    });

    match result {
        Ok(query_result) => {
            // Convert QueryResult to JSON
            let json = serde_json::json!({
                "rows": query_result.rows,
                "affected_rows": query_result.affected_rows,
                "last_insert_id": query_result.last_insert_id,
            });
            let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "null".into());
            let v8_str = v8::String::new(scope, &json_str).unwrap();
            if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                rv.set(parsed);
            } else {
                rv.set(v8::null(scope).into());
            }
        }
        Err(e) => {
            let msg = v8::String::new(scope, &format!("ctx.datasource().build() error: {e}")).unwrap();
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
        }
    }
}

/// Convert serde_json::Value to QueryValue for driver execution.
pub(super) fn json_to_query_value(v: serde_json::Value) -> rivers_runtime::rivers_driver_sdk::types::QueryValue {
    match v {
        serde_json::Value::Null => rivers_runtime::rivers_driver_sdk::types::QueryValue::Null,
        serde_json::Value::Bool(b) => rivers_runtime::rivers_driver_sdk::types::QueryValue::Boolean(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rivers_runtime::rivers_driver_sdk::types::QueryValue::Integer(i)
            } else {
                rivers_runtime::rivers_driver_sdk::types::QueryValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => rivers_runtime::rivers_driver_sdk::types::QueryValue::String(s),
        serde_json::Value::Array(a) => rivers_runtime::rivers_driver_sdk::types::QueryValue::Array(
            a.into_iter().map(json_to_query_value).collect(),
        ),
        serde_json::Value::Object(_) => rivers_runtime::rivers_driver_sdk::types::QueryValue::Json(v),
    }
}
