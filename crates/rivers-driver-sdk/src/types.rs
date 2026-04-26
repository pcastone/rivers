use std::collections::HashMap;

use serde::Deserialize;

/// Universal value type crossing the driver boundary.
///
/// Every parameter and result column is a `QueryValue`.
/// The `Json` variant handles arbitrary structured payloads
/// (InfluxDB batch writes, Kafka message bodies, MongoDB documents, etc.).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum QueryValue {
    /// SQL NULL or absent value.
    Null,
    /// Boolean true/false.
    Boolean(bool),
    /// Signed 64-bit integer.
    ///
    /// JSON representation: emitted as a JSON number if `|v| ≤ 2⁵³−1`
    /// (`Number.MAX_SAFE_INTEGER`), otherwise as a JSON string. This avoids
    /// silent precision loss in JS clients (IEEE-754 double rounds above 2⁵³).
    /// Per Twitter / Stripe / GitHub / Discord convention.
    Integer(i64),
    /// 64-bit floating point number.
    Float(f64),
    /// Unsigned 64-bit integer, used for `BIGINT UNSIGNED` columns and
    /// other unsigned-source values that don't fit in `Integer(i64)`.
    ///
    /// JSON representation: same threshold as `Integer` — emitted as a
    /// JSON number when `v ≤ 2⁵³−1`, else as a JSON string. Per H18.
    UInt(u64),
    /// UTF-8 string value.
    String(String),
    /// Ordered list of values.
    Array(Vec<QueryValue>),
    /// Arbitrary structured JSON payload.
    Json(serde_json::Value),
}

/// JS `Number.MAX_SAFE_INTEGER` (2⁵³−1). Integers whose magnitude exceeds
/// this value lose precision in JavaScript clients, so we serialize them
/// as JSON strings. Per Twitter snowflake / Stripe ID / GitHub ID
/// convention; see `todo/tasks.md` H18 + `docs/code_review.md` T2-1.
///
/// The signed and unsigned forms are kept side-by-side to document the
/// symmetry; `Integer` checks magnitude via `i64::unsigned_abs()` against
/// `SAFE_UINT_MAX`, so `SAFE_INT_MAX` is reference-only.
#[allow(dead_code)]
const SAFE_INT_MAX: i64 = 9_007_199_254_740_991;
const SAFE_UINT_MAX: u64 = 9_007_199_254_740_991;

impl serde::Serialize for QueryValue {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            QueryValue::Null => ser.serialize_none(),
            QueryValue::Boolean(b) => ser.serialize_bool(*b),
            QueryValue::Integer(v) => {
                if v.unsigned_abs() > SAFE_UINT_MAX {
                    ser.serialize_str(&v.to_string())
                } else {
                    ser.serialize_i64(*v)
                }
            }
            QueryValue::UInt(v) => {
                if *v > SAFE_UINT_MAX {
                    ser.serialize_str(&v.to_string())
                } else {
                    ser.serialize_u64(*v)
                }
            }
            QueryValue::Float(v) => ser.serialize_f64(*v),
            QueryValue::String(s) => ser.serialize_str(s),
            QueryValue::Array(arr) => {
                use serde::ser::SerializeSeq;
                let mut seq = ser.serialize_seq(Some(arr.len()))?;
                for v in arr {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            QueryValue::Json(v) => v.serialize(ser),
        }
    }
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
    /// Column names from the query result metadata. Populated by SQL drivers.
    /// Present when rows are empty (e.g. LIMIT 0 introspection queries) so
    /// callers can discover schema without any row data.
    pub column_names: Option<Vec<String>>,
}

impl QueryResult {
    /// Convenience constructor for an empty result.
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            affected_rows: 0,
            last_insert_id: None,
            column_names: None,
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
            QueryValue::Integer(_) | QueryValue::Float(_) | QueryValue::UInt(_) => {
                std::mem::size_of::<Self>()
            }
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

#[cfg(test)]
mod h18_serialize_tests {
    use super::*;

    fn ser(v: &QueryValue) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    #[test]
    fn integer_below_safe_max_emits_number() {
        assert_eq!(ser(&QueryValue::Integer(0)), serde_json::json!(0));
        assert_eq!(ser(&QueryValue::Integer(42)), serde_json::json!(42));
        assert_eq!(
            ser(&QueryValue::Integer(9_007_199_254_740_991)), // 2^53 - 1
            serde_json::json!(9_007_199_254_740_991_i64),
        );
    }

    #[test]
    fn integer_above_safe_max_emits_string() {
        assert_eq!(
            ser(&QueryValue::Integer(9_007_199_254_740_992)), // 2^53
            serde_json::json!("9007199254740992"),
        );
        assert_eq!(
            ser(&QueryValue::Integer(i64::MAX)),
            serde_json::json!(i64::MAX.to_string()),
        );
    }

    #[test]
    fn integer_below_negative_safe_max_emits_string() {
        assert_eq!(
            ser(&QueryValue::Integer(-9_007_199_254_740_992)),
            serde_json::json!("-9007199254740992"),
        );
        assert_eq!(
            ser(&QueryValue::Integer(i64::MIN)),
            serde_json::json!(i64::MIN.to_string()),
        );
    }

    #[test]
    fn integer_at_negative_safe_max_emits_number() {
        // -(2^53 - 1) is still safe.
        assert_eq!(
            ser(&QueryValue::Integer(-9_007_199_254_740_991)),
            serde_json::json!(-9_007_199_254_740_991_i64),
        );
    }

    #[test]
    fn uint_below_safe_max_emits_number() {
        assert_eq!(ser(&QueryValue::UInt(0)), serde_json::json!(0_u64));
        assert_eq!(
            ser(&QueryValue::UInt(9_007_199_254_740_991)),
            serde_json::json!(9_007_199_254_740_991_u64),
        );
    }

    #[test]
    fn uint_above_safe_max_emits_string() {
        assert_eq!(
            ser(&QueryValue::UInt(9_007_199_254_740_992)),
            serde_json::json!("9007199254740992"),
        );
        assert_eq!(
            ser(&QueryValue::UInt(u64::MAX)),
            serde_json::json!("18446744073709551615"),
        );
    }

    #[test]
    fn other_variants_unchanged() {
        assert_eq!(ser(&QueryValue::Null), serde_json::json!(null));
        assert_eq!(ser(&QueryValue::Boolean(true)), serde_json::json!(true));
        assert_eq!(ser(&QueryValue::Float(1.5)), serde_json::json!(1.5));
        assert_eq!(ser(&QueryValue::String("hi".into())), serde_json::json!("hi"));
        assert_eq!(
            ser(&QueryValue::Array(vec![QueryValue::Integer(1), QueryValue::Integer(2)])),
            serde_json::json!([1, 2]),
        );
        assert_eq!(
            ser(&QueryValue::Json(serde_json::json!({"k": "v"}))),
            serde_json::json!({"k": "v"}),
        );
    }

    #[test]
    fn array_of_large_uints_stringifies_per_element() {
        // Each element gets the threshold check independently.
        assert_eq!(
            ser(&QueryValue::Array(vec![
                QueryValue::UInt(42),
                QueryValue::UInt(u64::MAX),
            ])),
            serde_json::json!([42, "18446744073709551615"]),
        );
    }
}
