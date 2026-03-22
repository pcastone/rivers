//! RedisDriver — full async Redis database driver implementation.
//!
//! Per `rivers-driver-spec.md` §4:
//! Redis is a first-class built-in driver with 18+ operations
//! (get, set, del, hget, hgetall, lpush, rpush, etc.).
//!
//! Uses `redis::aio::MultiplexedConnection` for non-blocking I/O.

use std::collections::HashMap;

use async_trait::async_trait;
use redis::AsyncCommands;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverError, DriverType, HttpMethod,
    Query, QueryResult, QueryValue, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

/// Redis database driver.
///
/// Stateless factory — each call to `connect()` creates a new
/// `RedisConnection` backed by a `MultiplexedConnection`.
pub struct RedisDriver;

impl RedisDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RedisDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseDriver for RedisDriver {
    fn name(&self) -> &str {
        "redis"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let is_cluster = params.options.get("cluster").map(|v| v == "true").unwrap_or(false);

        if is_cluster {
            // Cluster mode: connect to multiple nodes
            let hosts: Vec<String> = if let Some(h) = params.options.get("hosts") {
                h.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                vec![format!("{}:{}", params.host, params.port)]
            };

            let nodes: Vec<String> = hosts.iter().map(|h| {
                if params.password.is_empty() {
                    format!("redis://{h}")
                } else {
                    format!("redis://:{}@{h}", params.password)
                }
            }).collect();

            let client = redis::cluster::ClusterClient::new(nodes)
                .map_err(|e| DriverError::Connection(format!("redis cluster client: {e}")))?;

            let conn = client
                .get_async_connection()
                .await
                .map_err(|e| DriverError::Connection(format!("redis cluster connect: {e}")))?;

            Ok(Box::new(RedisClusterConnection { conn }))
        } else {
            // Single-node mode
            let db = if params.database.is_empty() {
                "0".to_string()
            } else {
                params.database.clone()
            };

            let url = if params.password.is_empty() {
                format!("redis://{}:{}/{}", params.host, params.port, db)
            } else {
                format!(
                    "redis://:{}@{}:{}/{}",
                    params.password, params.host, params.port, db
                )
            };

            let client = redis::Client::open(url.as_str())
                .map_err(|e| DriverError::Connection(format!("redis client open: {e}")))?;

            let conn = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| DriverError::Connection(format!("redis connect: {e}")))?;

            Ok(Box::new(RedisConnection { conn }))
        }
    }

    fn supports_transactions(&self) -> bool {
        false
    }

    fn supports_prepared_statements(&self) -> bool {
        false
    }
}

/// A live Redis connection wrapping `MultiplexedConnection`.
pub struct RedisConnection {
    conn: redis::aio::MultiplexedConnection,
}

// MultiplexedConnection is Send + Sync, so RedisConnection is too.

#[async_trait]
impl Connection for RedisConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Pre-process: if params are missing expected keys, parse them from statement.
        // e.g. "SMEMBERS categories" → params["key"] = "categories"
        let query = &inject_params_from_statement(query);

        match query.operation.as_str() {
            // ---------------------------------------------------------------
            // Read operations — return rows
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
            // Write operations — return affected_rows
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
                    .map_err(|e| DriverError::Query(format!("PING: {e}")))?;
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
            .map_err(|e| DriverError::Connection(format!("redis ping: {e}")))?;
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "redis"
    }
}

/// A live Redis Cluster connection wrapping `ClusterConnection`.
///
/// Same operations as `RedisConnection` but follows cluster MOVED redirects.
pub struct RedisClusterConnection {
    conn: redis::cluster_async::ClusterConnection,
}

#[async_trait]
impl Connection for RedisClusterConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let query = &inject_params_from_statement(query);

        match query.operation.as_str() {
            // ---------------------------------------------------------------
            // Read operations — return rows
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
            // Write operations — return affected_rows
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

// ---------------------------------------------------------------------------
// Unified Driver trait implementation (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

#[async_trait]
impl Driver for RedisDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "redis"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        // Type must be one of: hash, string, list, set, sorted_set
        let valid_types = ["hash", "string", "list", "set", "sorted_set"];
        if !valid_types.contains(&schema.schema_type.as_str()) {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "redis".into(),
                supported: valid_types.iter().map(|s| s.to_string()).collect(),
                schema_file: String::new(),
            });
        }

        // key_pattern required (check in extra)
        if !schema.extra.contains_key("key_pattern") {
            return Err(SchemaSyntaxError::MissingRequiredField {
                field: "key_pattern".into(),
                driver: "redis".into(),
                schema_file: String::new(),
            });
        }

        // Type-specific validation
        match schema.schema_type.as_str() {
            "hash" => {
                if (method == HttpMethod::POST || method == HttpMethod::GET || method == HttpMethod::PUT)
                    && schema.fields.is_empty()
                {
                    return Err(SchemaSyntaxError::StructuralError {
                        message: "Redis hash schemas require fields".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
            }
            "string" => {
                if !schema.fields.is_empty() {
                    return Err(SchemaSyntaxError::StructuralError {
                        message: "Redis string schemas must not declare fields".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
                if !schema.extra.contains_key("value_type") {
                    return Err(SchemaSyntaxError::MissingRequiredField {
                        field: "value_type".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
            }
            "list" | "set" => {
                if !schema.extra.contains_key("element_type") {
                    return Err(SchemaSyntaxError::MissingRequiredField {
                        field: "element_type".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
            }
            "sorted_set" => {
                if !schema.extra.contains_key("member_type") || !schema.extra.contains_key("score_type") {
                    return Err(SchemaSyntaxError::MissingRequiredField {
                        field: "member_type and score_type".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
            }
            _ => {}
        }

        // Reject faker attribute on fields
        for field in &schema.fields {
            if field.constraints.contains_key("faker") {
                return Err(SchemaSyntaxError::UnsupportedAttribute {
                    attribute: "faker".into(),
                    field: field.name.clone(),
                    driver: "redis".into(),
                    supported: vec!["required".into(), "default".into(), "min".into(), "max".into(), "min_length".into(), "max_length".into(), "pattern".into(), "enum".into()],
                    schema_file: String::new(),
                });
            }
        }

        Ok(())
    }

    fn validate(
        &self,
        data: &serde_json::Value,
        schema: &SchemaDefinition,
        direction: ValidationDirection,
    ) -> Result<(), ValidationError> {
        match schema.schema_type.as_str() {
            "hash" => {
                // Validate hash fields like relational data
                rivers_driver_sdk::validation::validate_fields(data, schema, direction)
            }
            "string" => {
                // For string type, data should be a scalar, not an object
                if data.is_object() || data.is_array() {
                    return Err(ValidationError::TypeMismatch {
                        field: "(root)".into(),
                        expected: "scalar".into(),
                        actual: rivers_driver_sdk::validation::json_type_name(data).into(),
                        direction,
                    });
                }
                Ok(())
            }
            _ => Ok(()), // list, set, sorted_set — basic validation
        }
    }

    async fn execute(
        &self,
        _query: &Query,
        _params: &HashMap<String, QueryValue>,
    ) -> Result<QueryResult, DriverError> {
        Err(DriverError::NotImplemented(
            "use DatabaseDriver::connect() + Connection::execute() for Redis".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // RedisDriver is stateless; real connection happens via DatabaseDriver::connect()
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Parse Redis command statement into named parameters if params are empty.
///
/// Handles: `"SMEMBERS categories"` → key=categories
///          `"GET session:user:alice"` → key=session:user:alice
///          `"SET mykey myvalue"` → key=mykey, value=myvalue
///          `"LRANGE orders:recent 0 -1"` → key=orders:recent, start=0, stop=-1
///          `"DEL mykey"` → key=mykey
fn inject_params_from_statement(query: &Query) -> Query {
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
fn get_str_param(query: &Query, name: &str) -> Result<String, DriverError> {
    match query.parameters.get(name) {
        Some(QueryValue::String(s)) => Ok(s.clone()),
        Some(v) => Ok(format!("{:?}", v)),
        None => Err(DriverError::Query(format!(
            "missing required parameter: {name}"
        ))),
    }
}

/// Extract an integer parameter from the query.
fn get_int_param(query: &Query, name: &str) -> Result<i64, DriverError> {
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
fn get_keys_param(query: &Query) -> Result<Vec<String>, DriverError> {
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
fn single_value_row(value: String) -> HashMap<String, QueryValue> {
    let mut row = HashMap::new();
    row.insert("value".to_string(), QueryValue::String(value));
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection};

    fn make_schema(schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        let mut extra = HashMap::new();
        extra.insert("key_pattern".to_string(), serde_json::json!("user:${id}"));
        SchemaDefinition {
            driver: "redis".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra,
        }
    }

    fn make_schema_with_extra(
        schema_type: &str,
        fields: Vec<SchemaFieldDef>,
        extra_pairs: Vec<(&str, serde_json::Value)>,
    ) -> SchemaDefinition {
        let mut extra = HashMap::new();
        for (k, v) in extra_pairs {
            extra.insert(k.to_string(), v);
        }
        SchemaDefinition {
            driver: "redis".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra,
        }
    }

    fn make_field(name: &str, field_type: &str, required: bool) -> SchemaFieldDef {
        SchemaFieldDef {
            name: name.into(),
            field_type: field_type.into(),
            required,
            constraints: HashMap::new(),
        }
    }

    fn make_field_with(
        name: &str,
        field_type: &str,
        required: bool,
        constraints: Vec<(&str, serde_json::Value)>,
    ) -> SchemaFieldDef {
        let mut c = HashMap::new();
        for (k, v) in constraints {
            c.insert(k.to_string(), v);
        }
        SchemaFieldDef {
            name: name.into(),
            field_type: field_type.into(),
            required,
            constraints: c,
        }
    }

    #[test]
    fn schema_syntax_hash_valid() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![
                make_field("name", "string", true),
                make_field("score", "integer", false),
            ],
        );
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_string_valid() {
        let driver = RedisDriver::new();
        let mut schema = make_schema("string", vec![]);
        schema.extra.insert("value_type".to_string(), serde_json::json!("string"));
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_string_rejects_fields() {
        let driver = RedisDriver::new();
        let mut schema = make_schema(
            "string",
            vec![make_field("a", "string", false)],
        );
        schema.extra.insert("value_type".to_string(), serde_json::json!("string"));
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::StructuralError { .. }));
    }

    #[test]
    fn schema_syntax_string_requires_value_type() {
        let driver = RedisDriver::new();
        let schema = make_schema("string", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "value_type"));
    }

    #[test]
    fn schema_syntax_list_requires_element_type() {
        let driver = RedisDriver::new();
        let schema = make_schema("list", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "element_type"));
    }

    #[test]
    fn schema_syntax_list_valid_with_element_type() {
        let driver = RedisDriver::new();
        let mut schema = make_schema("list", vec![]);
        schema.extra.insert("element_type".to_string(), serde_json::json!("string"));
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_sorted_set_requires_member_and_score() {
        let driver = RedisDriver::new();
        let schema = make_schema("sorted_set", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { .. }));
    }

    #[test]
    fn schema_syntax_sorted_set_valid() {
        let driver = RedisDriver::new();
        let mut schema = make_schema("sorted_set", vec![]);
        schema.extra.insert("member_type".to_string(), serde_json::json!("string"));
        schema.extra.insert("score_type".to_string(), serde_json::json!("float"));
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_object_type() {
        let driver = RedisDriver::new();
        let schema = make_schema("object", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedType { .. }));
    }

    #[test]
    fn schema_syntax_missing_key_pattern() {
        let driver = RedisDriver::new();
        let schema = make_schema_with_extra("hash", vec![make_field("name", "string", true)], vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "key_pattern"));
    }

    #[test]
    fn schema_syntax_hash_requires_fields_for_get() {
        let driver = RedisDriver::new();
        let schema = make_schema("hash", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::StructuralError { .. }));
    }

    #[test]
    fn schema_syntax_rejects_faker_attribute() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field_with("name", "string", true, vec![("faker", serde_json::json!("name"))])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedAttribute { .. }));
    }

    #[test]
    fn validate_hash_accepts_valid_object() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!({"name": "Alice"});
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_hash_rejects_missing_required() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!({"score": 42});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRequired { ref field, .. } if field == "name"));
    }

    #[test]
    fn validate_string_accepts_scalar() {
        let driver = RedisDriver::new();
        let schema = make_schema("string", vec![]);
        let data = serde_json::json!("hello");
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_string_rejects_object() {
        let driver = RedisDriver::new();
        let schema = make_schema("string", vec![]);
        let data = serde_json::json!({"key": "value"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }

    #[test]
    fn validate_hash_type_mismatch_detected() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field("count", "integer", true)],
        );
        let data = serde_json::json!({"count": "not_a_number"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }
}
