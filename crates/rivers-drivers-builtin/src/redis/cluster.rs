//! Redis Cluster connection implementation.

use std::collections::HashMap;

use async_trait::async_trait;
use redis::AsyncCommands;
use rivers_driver_sdk::{Connection, DriverError, Query, QueryResult, QueryValue};

use super::params::*;

/// A live Redis Cluster connection wrapping `ClusterConnection`.
///
/// Same operations as `RedisConnection` but follows cluster MOVED redirects.
pub struct RedisClusterConnection {
    pub(super) conn: redis::cluster_async::ClusterConnection,
}

#[async_trait]
impl Connection for RedisClusterConnection {
    fn admin_operations(&self) -> &[&str] {
        &["flushdb", "flushall", "config_set", "config_rewrite"]
    }

    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        let query = &inject_params_from_statement(query);

        match query.operation.as_str() {
            // ---------------------------------------------------------------
            // Read operations -- return rows
            // ---------------------------------------------------------------
            "get" => {
                let key = get_str_param(query, "key")?;
                let val: Option<String> = self
                    .conn
                    .get(&key)
                    .await
                    .map_err(|e| DriverError::Query(format!("GET: {e}")))?;
                match val {
                    Some(v) => Ok(QueryResult {
                        rows: vec![single_value_row(v)],
                        affected_rows: 1,
                        last_insert_id: None,
                    }),
                    None => Ok(QueryResult::empty()),
                }
            }

            "mget" => {
                let keys = get_keys_param(query)?;
                let vals: Vec<Option<String>> = self
                    .conn
                    .get(&keys[..])
                    .await
                    .map_err(|e| DriverError::Query(format!("MGET: {e}")))?;
                let rows: Vec<HashMap<String, QueryValue>> = keys
                    .iter()
                    .zip(vals.iter())
                    .map(|(k, v)| {
                        let mut row = HashMap::new();
                        row.insert("key".to_string(), QueryValue::String(k.clone()));
                        row.insert(
                            "value".to_string(),
                            match v {
                                Some(s) => QueryValue::String(s.clone()),
                                None => QueryValue::Null,
                            },
                        );
                        row
                    })
                    .collect();
                let count = rows.len() as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "hget" => {
                let key = get_str_param(query, "key")?;
                let field = get_str_param(query, "field")?;
                let val: Option<String> = self
                    .conn
                    .hget(&key, &field)
                    .await
                    .map_err(|e| DriverError::Query(format!("HGET: {e}")))?;
                match val {
                    Some(v) => Ok(QueryResult {
                        rows: vec![single_value_row(v)],
                        affected_rows: 1,
                        last_insert_id: None,
                    }),
                    None => Ok(QueryResult::empty()),
                }
            }

            "hgetall" => {
                let key = get_str_param(query, "key")?;
                let map: HashMap<String, String> = self
                    .conn
                    .hgetall(&key)
                    .await
                    .map_err(|e| DriverError::Query(format!("HGETALL: {e}")))?;
                let rows: Vec<HashMap<String, QueryValue>> = map
                    .into_iter()
                    .map(|(f, v)| {
                        let mut row = HashMap::new();
                        row.insert("field".to_string(), QueryValue::String(f));
                        row.insert("value".to_string(), QueryValue::String(v));
                        row
                    })
                    .collect();
                let count = rows.len() as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "lrange" => {
                let key = get_str_param(query, "key")?;
                let start = get_int_param(query, "start").unwrap_or(0);
                let stop = get_int_param(query, "stop").unwrap_or(-1);
                let vals: Vec<String> = self
                    .conn
                    .lrange(&key, start as isize, stop as isize)
                    .await
                    .map_err(|e| DriverError::Query(format!("LRANGE: {e}")))?;
                let rows: Vec<HashMap<String, QueryValue>> =
                    vals.into_iter().map(single_value_row).collect();
                let count = rows.len() as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "smembers" => {
                let key = get_str_param(query, "key")?;
                let vals: Vec<String> = self
                    .conn
                    .smembers(&key)
                    .await
                    .map_err(|e| DriverError::Query(format!("SMEMBERS: {e}")))?;
                let rows: Vec<HashMap<String, QueryValue>> =
                    vals.into_iter().map(single_value_row).collect();
                let count = rows.len() as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "exists" => {
                let key = get_str_param(query, "key")?;
                let exists: bool = self
                    .conn
                    .exists(&key)
                    .await
                    .map_err(|e| DriverError::Query(format!("EXISTS: {e}")))?;
                let mut row = HashMap::new();
                row.insert(
                    "exists".to_string(),
                    QueryValue::Integer(if exists { 1 } else { 0 }),
                );
                Ok(QueryResult {
                    rows: vec![row],
                    affected_rows: 1,
                    last_insert_id: None,
                })
            }

            "keys" => {
                let pattern = get_str_param(query, "pattern").unwrap_or_else(|_| "*".to_string());
                let vals: Vec<String> = self
                    .conn
                    .keys(&pattern)
                    .await
                    .map_err(|e| DriverError::Query(format!("KEYS: {e}")))?;
                let rows: Vec<HashMap<String, QueryValue>> = vals
                    .into_iter()
                    .map(|k| {
                        let mut row = HashMap::new();
                        row.insert("key".to_string(), QueryValue::String(k));
                        row
                    })
                    .collect();
                let count = rows.len() as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            // ---------------------------------------------------------------
            // Write operations -- return affected_rows
            // ---------------------------------------------------------------
            "set" => {
                let key = get_str_param(query, "key")?;
                let value = get_str_param(query, "value")?;
                match get_int_param(query, "seconds") {
                    Ok(secs) => {
                        self.conn
                            .set_ex::<_, _, ()>(&key, &value, secs as u64)
                            .await
                            .map_err(|e| DriverError::Query(format!("SET EX: {e}")))?;
                    }
                    Err(_) => {
                        self.conn
                            .set::<_, _, ()>(&key, &value)
                            .await
                            .map_err(|e| DriverError::Query(format!("SET: {e}")))?;
                    }
                }
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: 1,
                    last_insert_id: None,
                })
            }

            "hset" => {
                let key = get_str_param(query, "key")?;
                let field = get_str_param(query, "field")?;
                let value = get_str_param(query, "value")?;
                let count: u64 = self
                    .conn
                    .hset(&key, &field, &value)
                    .await
                    .map_err(|e| DriverError::Query(format!("HSET: {e}")))?;
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "lpush" => {
                let key = get_str_param(query, "key")?;
                let value = get_str_param(query, "value")?;
                let len: u64 = self
                    .conn
                    .lpush(&key, &value)
                    .await
                    .map_err(|e| DriverError::Query(format!("LPUSH: {e}")))?;
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: len,
                    last_insert_id: None,
                })
            }

            "rpush" => {
                let key = get_str_param(query, "key")?;
                let value = get_str_param(query, "value")?;
                let len: u64 = self
                    .conn
                    .rpush(&key, &value)
                    .await
                    .map_err(|e| DriverError::Query(format!("RPUSH: {e}")))?;
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: len,
                    last_insert_id: None,
                })
            }

            "sadd" => {
                let key = get_str_param(query, "key")?;
                let member = get_str_param(query, "member")?;
                let count: u64 = self
                    .conn
                    .sadd(&key, &member)
                    .await
                    .map_err(|e| DriverError::Query(format!("SADD: {e}")))?;
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "expire" => {
                let key = get_str_param(query, "key")?;
                let seconds = get_int_param(query, "seconds")?;
                let ok: bool = self
                    .conn
                    .expire(&key, seconds)
                    .await
                    .map_err(|e| DriverError::Query(format!("EXPIRE: {e}")))?;
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: if ok { 1 } else { 0 },
                    last_insert_id: None,
                })
            }

            "incr" => {
                let key = get_str_param(query, "key")?;
                let val: i64 = self
                    .conn
                    .incr(&key, 1i64)
                    .await
                    .map_err(|e| DriverError::Query(format!("INCR: {e}")))?;
                let mut row = HashMap::new();
                row.insert("value".to_string(), QueryValue::Integer(val));
                Ok(QueryResult {
                    rows: vec![row],
                    affected_rows: 1,
                    last_insert_id: None,
                })
            }

            "incrby" => {
                let key = get_str_param(query, "key")?;
                let increment = get_int_param(query, "increment")?;
                let val: i64 = self
                    .conn
                    .incr(&key, increment)
                    .await
                    .map_err(|e| DriverError::Query(format!("INCRBY: {e}")))?;
                let mut row = HashMap::new();
                row.insert("value".to_string(), QueryValue::Integer(val));
                Ok(QueryResult {
                    rows: vec![row],
                    affected_rows: 1,
                    last_insert_id: None,
                })
            }

            // ---------------------------------------------------------------
            // Delete operations
            // ---------------------------------------------------------------
            "del" => {
                let key = get_str_param(query, "key")?;
                let count: u64 = self
                    .conn
                    .del(&key)
                    .await
                    .map_err(|e| DriverError::Query(format!("DEL: {e}")))?;
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "hdel" => {
                let key = get_str_param(query, "key")?;
                let field = get_str_param(query, "field")?;
                let count: u64 = self
                    .conn
                    .hdel(&key, &field)
                    .await
                    .map_err(|e| DriverError::Query(format!("HDEL: {e}")))?;
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            // ---------------------------------------------------------------
            // Other
            // ---------------------------------------------------------------
            "ping" => {
                redis::cmd("PING")
                    .query_async::<String>(&mut self.conn)
                    .await
                    .map_err(|e| DriverError::Connection(format!("redis cluster ping: {e}")))?;
                Ok(QueryResult::empty())
            }

            op => Err(DriverError::Unsupported(format!(
                "redis driver does not support operation: {op}"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        redis::cmd("PING")
            .query_async::<String>(&mut self.conn)
            .await
            .map_err(|e| DriverError::Connection(format!("redis cluster ping: {e}")))?;
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "redis"
    }
}
