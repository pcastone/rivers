use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Universal value type crossing the driver boundary.
///
/// Every parameter and result column is a `QueryValue`.
/// The `Json` variant handles arbitrary structured payloads
/// (InfluxDB batch writes, Kafka message bodies, MongoDB documents, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QueryValue {
    /// SQL NULL or absent value.
    Null,
    /// Boolean true/false.
    Boolean(bool),
    /// Signed 64-bit integer.
    Integer(i64),
    /// 64-bit floating point number.
    Float(f64),
    /// UTF-8 string value.
    String(String),
    /// Ordered list of values.
    Array(Vec<QueryValue>),
    /// Arbitrary structured JSON payload.
    Json(serde_json::Value),
}

/// Normalized query model passed from DataView engine to driver.
///
/// `operation` is inferred from the first whitespace-delimited token of
/// `statement` (lowercased) when not set explicitly by the caller.
#[derive(Debug, Clone)]
pub struct Query {
    /// e.g. "select", "insert", "get", "set", "ping", "xadd", "publish"
    pub operation: String,
    /// Table, collection, stream, topic, or key.
    pub target: String,
    /// Named parameters for the query.
    pub parameters: HashMap<String, QueryValue>,
    /// Raw native statement or command.
    pub statement: String,
}

impl Query {
    /// Create a new query with operation inferred from the first token of `statement`.
    ///
    /// Per spec §2: "Operation is inferred from the first whitespace-delimited
    /// token of statement (lowercased) when not set explicitly."
    pub fn new(target: &str, statement: &str) -> Self {
        let operation = infer_operation(statement);
        Self {
            operation,
            target: target.to_string(),
            parameters: HashMap::new(),
            statement: statement.to_string(),
        }
    }

    /// Create a query with an explicit operation (no inference).
    pub fn with_operation(operation: &str, target: &str, statement: &str) -> Self {
        Self {
            operation: operation.to_string(),
            target: target.to_string(),
            parameters: HashMap::new(),
            statement: statement.to_string(),
        }
    }

    /// Add a parameter to this query, returning self for chaining.
    pub fn param(mut self, name: &str, value: QueryValue) -> Self {
        self.parameters.insert(name.to_string(), value);
        self
    }
}

/// Infer the operation from a statement using the full algorithm (SHAPE-7):
///
/// 1. Trim whitespace
/// 2. Strip SQL comments (`--` line comments, `/* ... */` block comments)
/// 3. First whitespace-delimited token, lowercased
/// 4. Map to canonical operation category (read/write/delete) or pass through
///
/// Returns the lowercase first token, or "unknown" if the statement is empty.
pub fn infer_operation(statement: &str) -> String {
    let trimmed = statement.trim();

    // JSON-aware path: if statement looks like JSON, extract "operation" field
    if trimmed.starts_with('{') {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(op) = json.get("operation").and_then(|v| v.as_str()) {
                return op.to_lowercase();
            }
            // JSON without "operation" field — default to "find" (read)
            return "find".to_string();
        }
        // Malformed JSON — fall through to first-token logic
    }

    let stripped = strip_sql_comments(statement);
    stripped
        .split_whitespace()
        .next()
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Canonical operation category for a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationCategory {
    /// SELECT, GET, FIND, SCAN, etc.
    Read,
    /// INSERT, UPDATE, SET, XADD, PUBLISH, etc.
    Write,
    /// DELETE, DEL, DROP, TRUNCATE, REMOVE, etc.
    Delete,
    /// Unrecognized operation — passed through to the driver.
    Other,
}

/// Classify a token into a canonical operation category.
pub fn classify_operation(token: &str) -> OperationCategory {
    match token.to_lowercase().as_str() {
        "select" | "get" | "mget" | "hget" | "hgetall" | "lrange" | "smembers"
        | "find" | "aggregate" | "search" | "scan" | "keys" | "exists"
        | "show" | "describe" | "explain" | "ping" | "info" => OperationCategory::Read,
        "insert" | "update" | "upsert" | "set" | "mset" | "hset" | "lpush"
        | "rpush" | "sadd" | "zadd" | "xadd" | "publish" | "create" | "alter"
        | "replace" | "merge" => OperationCategory::Write,
        "delete" | "del" | "hdel" | "lrem" | "srem" | "zrem" | "xdel"
        | "drop" | "truncate" | "remove" => OperationCategory::Delete,
        _ => OperationCategory::Other,
    }
}

/// Strip SQL-style comments from a statement.
///
/// Removes `--` line comments and `/* ... */` block comments.
fn strip_sql_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if i + 1 < len && chars[i] == '-' && chars[i + 1] == '-' {
            // Skip to end of line
            while i < len && chars[i] != '\n' {
                i += 1;
            }
        } else if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
            // Skip block comment
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < len {
                i += 2; // skip closing */
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Normalized result from any driver operation.
///
/// Write operations return `rows = vec![]` with `affected_rows` set.
/// Read operations set `affected_rows = rows.len()` by convention.
/// Drivers must never return rows as None — use empty vec.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Result rows — each row is a map of column name to value.
    pub rows: Vec<HashMap<String, QueryValue>>,
    /// Number of rows affected by write operations, or `rows.len()` for reads.
    pub affected_rows: u64,
    /// Auto-generated ID from an INSERT, if the driver provides one.
    pub last_insert_id: Option<String>,
}

impl QueryResult {
    /// Convenience constructor for an empty result.
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            affected_rows: 0,
            last_insert_id: None,
        }
    }

    /// Approximate heap size in bytes. Not exact — proportional estimate
    /// for memory-bounded cache eviction.
    pub fn estimated_bytes(&self) -> usize {
        let mut size = std::mem::size_of::<Self>();
        for row in &self.rows {
            size += std::mem::size_of::<HashMap<String, QueryValue>>();
            for (k, v) in row {
                size += k.len() + std::mem::size_of::<String>();
                size += v.estimated_bytes();
            }
        }
        if let Some(ref id) = self.last_insert_id {
            size += id.len();
        }
        size
    }
}

impl QueryValue {
    /// Approximate heap size in bytes.
    pub fn estimated_bytes(&self) -> usize {
        match self {
            QueryValue::Null | QueryValue::Boolean(_) => std::mem::size_of::<Self>(),
            QueryValue::Integer(_) | QueryValue::Float(_) => std::mem::size_of::<Self>(),
            QueryValue::String(s) => std::mem::size_of::<Self>() + s.len(),
            QueryValue::Array(a) => {
                std::mem::size_of::<Self>() + a.iter().map(|v| v.estimated_bytes()).sum::<usize>()
            }
            QueryValue::Json(v) => std::mem::size_of::<Self>() + estimate_json_bytes(v),
        }
    }
}

fn estimate_json_bytes(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => 16,
        serde_json::Value::String(s) => 16 + s.len(),
        serde_json::Value::Array(a) => 24 + a.iter().map(estimate_json_bytes).sum::<usize>(),
        serde_json::Value::Object(o) => {
            24 + o.iter().map(|(k, v)| k.len() + 16 + estimate_json_bytes(v)).sum::<usize>()
        }
    }
}
