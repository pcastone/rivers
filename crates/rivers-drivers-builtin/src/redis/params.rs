//! Parameter extraction helpers for Redis operations.
//!
//! Used by both `single::RedisConnection` and `cluster::RedisClusterConnection`.

use std::collections::HashMap;

use rivers_driver_sdk::{DriverError, Query, QueryValue};

/// Parse Redis command statement into named parameters if params are empty.
///
/// Handles: `"SMEMBERS categories"` -> key=categories
///          `"GET session:user:alice"` -> key=session:user:alice
///          `"SET mykey myvalue"` -> key=mykey, value=myvalue
///          `"LRANGE orders:recent 0 -1"` -> key=orders:recent, start=0, stop=-1
///          `"DEL mykey"` -> key=mykey
pub fn inject_params_from_statement(query: &Query) -> Query {
    // Only inject if "key" param is missing (user-supplied params take precedence)
    if query.parameters.contains_key("key") || query.statement.is_empty() {
        return query.clone();
    }

    let parts: Vec<&str> = query.statement.split_whitespace().collect();
    if parts.len() < 2 {
        return query.clone();
    }

    let mut q = query.clone();
    // parts[0] is the command (already captured as operation), parts[1..] are args
    match parts.len() {
        2 => {
            // Single-arg commands: GET key, SMEMBERS key, DEL key, HGETALL key
            q.parameters.insert("key".into(), QueryValue::String(parts[1].into()));
        }
        3 => {
            // Two-arg commands: SET key value, HGET key field
            q.parameters.insert("key".into(), QueryValue::String(parts[1].into()));
            q.parameters.insert("value".into(), QueryValue::String(parts[2].into()));
        }
        4 => {
            // Three-arg commands: LRANGE key start stop, HSET key field value
            q.parameters.insert("key".into(), QueryValue::String(parts[1].into()));
            // For LRANGE: start + stop; for HSET: field + value
            let op = query.operation.to_lowercase();
            if op == "lrange" || op == "zrangebyscore" {
                q.parameters.insert("start".into(), QueryValue::String(parts[2].into()));
                q.parameters.insert("stop".into(), QueryValue::String(parts[3].into()));
            } else {
                q.parameters.insert("field".into(), QueryValue::String(parts[2].into()));
                q.parameters.insert("value".into(), QueryValue::String(parts[3].into()));
            }
        }
        _ => {
            // Multi-arg: just set key as first arg, rest as space-joined value
            q.parameters.insert("key".into(), QueryValue::String(parts[1].into()));
            q.parameters.insert("value".into(), QueryValue::String(parts[2..].join(" ")));
        }
    }

    q
}

/// Extract a string parameter from the query, converting non-string values
/// via Debug formatting.
pub fn get_str_param(query: &Query, name: &str) -> Result<String, DriverError> {
    match query.parameters.get(name) {
        Some(QueryValue::String(s)) => Ok(s.clone()),
        Some(v) => Ok(format!("{:?}", v)),
        None => Err(DriverError::Query(format!(
            "missing required parameter: {name}"
        ))),
    }
}

/// Extract an integer parameter from the query.
pub fn get_int_param(query: &Query, name: &str) -> Result<i64, DriverError> {
    match query.parameters.get(name) {
        Some(QueryValue::Integer(n)) => Ok(*n),
        Some(QueryValue::String(s)) => s.parse::<i64>().map_err(|_| {
            DriverError::Query(format!("parameter '{name}' is not a valid integer: {s}"))
        }),
        Some(_) => Err(DriverError::Query(format!(
            "parameter '{name}' must be an integer"
        ))),
        None => Err(DriverError::Query(format!(
            "missing required parameter: {name}"
        ))),
    }
}

/// Extract a list of keys for MGET from query parameters.
///
/// Accepts either a single `key` parameter (space-separated keys) or
/// an `Array` variant containing multiple key strings.
pub fn get_keys_param(query: &Query) -> Result<Vec<String>, DriverError> {
    match query.parameters.get("key") {
        Some(QueryValue::Array(arr)) => {
            let keys: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    QueryValue::String(s) => s.clone(),
                    other => format!("{:?}", other),
                })
                .collect();
            if keys.is_empty() {
                return Err(DriverError::Query(
                    "MGET requires at least one key".to_string(),
                ));
            }
            Ok(keys)
        }
        Some(QueryValue::String(s)) => {
            let keys: Vec<String> = s.split_whitespace().map(|k| k.to_string()).collect();
            if keys.is_empty() {
                return Err(DriverError::Query(
                    "MGET requires at least one key".to_string(),
                ));
            }
            Ok(keys)
        }
        _ => Err(DriverError::Query(
            "missing required parameter: key".to_string(),
        )),
    }
}

/// Build a single-row result with a `value` field.
pub fn single_value_row(value: String) -> HashMap<String, QueryValue> {
    let mut row = HashMap::new();
    row.insert("value".to_string(), QueryValue::String(value));
    row
}
