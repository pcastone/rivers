//! Rivers Driver SDK — trait contracts for database, broker, and HTTP drivers.
//!
//! This crate defines the interfaces that all Rivers drivers must implement.
//! It contains three independent driver contracts:
//!
//! - [`DatabaseDriver`] / [`Connection`] — request/response drivers for
//!   relational databases, key-value stores, and search engines.
//! - [`MessageBrokerDriver`] / [`BrokerConsumer`] / [`BrokerProducer`] —
//!   continuous-push drivers for Kafka, RabbitMQ, NATS, and Redis Streams.
//! - [`HttpDriver`](http_driver::HttpDriver) / [`HttpConnection`](http_driver::HttpConnection) —
//!   HTTP/HTTP2/SSE/WebSocket as a first-class datasource.
//!
//! Plugin crates (cdylib) depend on this SDK to implement their driver and
//! register it via [`DriverRegistrar`] at load time.

#![warn(missing_docs)]

use std::sync::Arc;

/// Message broker driver contracts — Kafka, RabbitMQ, NATS, Redis Streams.
pub mod broker;
/// Driver error types.
pub mod error;
/// HTTP driver contracts — HTTP/HTTP2/SSE/WebSocket as a datasource.
pub mod http_driver;
/// Reqwest-based HTTP driver implementation with retry and circuit breaker.
pub mod http_executor;
/// HTTP schema syntax and data validation.
pub mod http_validation;
/// Typed operation catalog types for the V8 proxy codegen framework.
pub mod operation_descriptor;
/// Core driver traits — [`DatabaseDriver`], [`Connection`], [`Driver`], and schema types.
pub mod traits;
/// Query model, result types, and operation classification.
pub mod types;
/// Shared schema validation engine for field types and constraints.
pub mod validation;

pub use broker::{
    AckOutcome, BrokerConsumer, BrokerConsumerConfig, BrokerError, BrokerMetadata, BrokerProducer,
    BrokerSemantics, BrokerSubscription, FailureMode, FailurePolicy, InboundMessage,
    MessageBrokerDriver, MessageReceipt, OutboundMessage, PublishReceipt,
};
pub use error::DriverError;
pub use operation_descriptor::{OpKind, OperationDescriptor, Param, ParamType};
pub use traits::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverType, HttpMethod, ParamStyle,
    SchemaDefinition, SchemaFieldDef, SchemaSyntaxError, ValidationDirection, ValidationError,
};
pub use types::{classify_operation, infer_operation, OperationCategory, Query, QueryResult, QueryValue};

// ── DDL / Admin Operation Guards ────────────────────────────────

/// Extract the first meaningful SQL token from a statement.
///
/// Strips leading `--` line comments and `/* */` block comments,
/// then returns the first whitespace-delimited token, uppercased.
/// Used by both `is_ddl_statement` and `infer_operation` so both
/// paths agree on the leading token (RW1.1.a).
fn first_sql_token(statement: &str) -> String {
    // Reuse the strip_sql_comments logic from types.rs via infer_operation.
    // We strip comments, then trim, then grab the first token.
    use crate::types::infer_operation;
    // infer_operation already strips comments and returns the first token
    // lowercased.  We uppercase here for DDL matching.
    infer_operation(statement).to_uppercase()
}

/// Returns true if the SQL statement is a DDL operation.
///
/// Comment-aware: strips `--` line comments and `/* */` block comments
/// before inspecting the leading token so that a comment like
/// `-- DROP TABLE\nSELECT 1` is correctly classified as a query (RW1.1.a).
pub fn is_ddl_statement(statement: &str) -> bool {
    let token = first_sql_token(statement);
    token == "CREATE"
        || token == "ALTER"
        || token == "DROP"
        || token == "TRUNCATE"
}

/// Check if a query is an admin operation (SQL DDL or driver-declared admin op).
///
/// Returns `Some(reason)` if blocked, `None` if allowed.
/// Use in `Connection::execute()` to reject admin operations.
///
/// Error messages are sanitized and never echo raw statement content
/// (which may carry credential material from connection-string payloads).
/// The full statement is logged at DEBUG level only (RW1.1.b).
pub fn check_admin_guard(query: &Query, admin_ops: &[&str]) -> Option<String> {
    if is_ddl_statement(&query.statement) {
        tracing::debug!(
            statement = %query.statement,
            "DDL statement rejected (full statement logged here only)"
        );
        let token = first_sql_token(&query.statement);
        return Some(format!(
            "DDL statement rejected (classified as: {})",
            token
        ));
    }
    if admin_ops.contains(&query.operation.as_str()) {
        return Some(format!(
            "admin operation '{}' rejected",
            query.operation
        ));
    }
    None
}

// ── Parameter Translation ──────────────────────────────────────

/// Rewrite `$name` placeholders in a query statement to the driver's native format.
///
/// Scans the statement for `$name` tokens (bare identifiers after `$`),
/// extracts them in order of appearance, and rewrites based on `ParamStyle`.
/// Returns the rewritten statement and parameters ordered for positional styles.
///
/// For `ParamStyle::None`, returns the statement unchanged.
pub fn translate_params(
    statement: &str,
    params: &std::collections::HashMap<String, QueryValue>,
    style: ParamStyle,
) -> (String, Vec<(String, QueryValue)>) {
    if style == ParamStyle::None || style == ParamStyle::DollarNamed {
        // No rewriting needed — $name is already correct or not used
        let ordered: Vec<(String, QueryValue)> = params
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        return (statement.to_string(), ordered);
    }

    // Extract $name placeholders in order of first appearance (unique list for
    // positional-index assignment) and in full appearance order (for QuestionPositional
    // binding — each occurrence of the same name needs its own bound value).
    let mut placeholders: Vec<String> = Vec::new();      // unique, first-appearance order
    let mut all_occurrences: Vec<String> = Vec::new();   // every occurrence in order
    let chars = statement.chars().peekable();
    let mut i = 0;
    let bytes = statement.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_') {
            // Found $name — extract the identifier
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            let name = String::from_utf8_lossy(&bytes[start..end]).to_string();
            all_occurrences.push(name.clone());
            if !placeholders.contains(&name) {
                placeholders.push(name);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    let _ = chars; // consumed above via bytes

    // Build ordered params matching placeholder order.
    // For QuestionPositional every occurrence of a repeated $name needs a
    // separate bound value (MySQL/SQLite require one value per '?').
    // For all other styles deduplicated placeholders are sufficient.
    let ordered: Vec<(String, QueryValue)> = if style == ParamStyle::QuestionPositional {
        all_occurrences
            .iter()
            .filter_map(|name| params.get(name).map(|v| (name.clone(), v.clone())))
            .collect()
    } else {
        placeholders
            .iter()
            .filter_map(|name| params.get(name).map(|v| (name.clone(), v.clone())))
            .collect()
    };

    // Rewrite statement
    match style {
        ParamStyle::DollarPositional => {
            // Span-based replacement: scan the statement byte-by-byte and build
            // the output in one pass.  For each $name token we write the
            // positional placeholder ($1, $2, …).  Because we never modify the
            // source string while scanning, there is no prefix-collision problem
            // (e.g. $param1 and $param10 are handled independently) (RW1.1.c).
            let bytes = statement.as_bytes();
            let mut out = String::with_capacity(statement.len() + 16);
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'$'
                    && i + 1 < bytes.len()
                    && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_')
                {
                    // Scan the full identifier
                    let start = i + 1;
                    let mut end = start;
                    while end < bytes.len()
                        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                    {
                        end += 1;
                    }
                    let name = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
                    // Look up this name's positional index (1-based)
                    if let Some(pos) = placeholders.iter().position(|p| p == name) {
                        out.push('$');
                        out.push_str(&(pos + 1).to_string());
                    } else {
                        // Name not in our placeholder list — emit verbatim
                        out.push_str(std::str::from_utf8(&bytes[i..end]).unwrap_or(""));
                    }
                    i = end;
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            (out, ordered)
        }
        ParamStyle::QuestionPositional => {
            // Replace each occurrence of $name (including repeats) with ?
            // in the order they appear so the bound-value list lines up.
            let bytes = statement.as_bytes();
            let mut out = String::with_capacity(statement.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'$'
                    && i + 1 < bytes.len()
                    && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_')
                {
                    let start = i + 1;
                    let mut end = start;
                    while end < bytes.len()
                        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                    {
                        end += 1;
                    }
                    out.push('?');
                    i = end;
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            (out, ordered)
        }
        ParamStyle::ColonNamed => {
            // Span-based: replace $name with :name
            let bytes = statement.as_bytes();
            let mut out = String::with_capacity(statement.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'$'
                    && i + 1 < bytes.len()
                    && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_')
                {
                    let start = i + 1;
                    let mut end = start;
                    while end < bytes.len()
                        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                    {
                        end += 1;
                    }
                    let name = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
                    out.push(':');
                    out.push_str(name);
                    i = end;
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            (out, ordered)
        }
        _ => (statement.to_string(), ordered),
    }
}

// ── RW1.1 Tests ────────────────────────────────────────────────

#[cfg(test)]
mod rw1_1_tests {
    use super::*;
    use crate::types::Query;

    // ── RW1.1.a — comment-aware DDL classifier ──────────────────

    #[test]
    fn ddl_leading_line_comment_classifies_as_query() {
        // A line comment mentioning DROP must not fool the classifier.
        assert!(!is_ddl_statement("-- DROP TABLE\nSELECT 1"));
    }

    #[test]
    fn ddl_leading_block_comment_classifies_as_ddl() {
        // Block comment before a real DDL token → DDL.
        assert!(is_ddl_statement("/* comment */ CREATE TABLE t (id INT)"));
    }

    #[test]
    fn ddl_block_comment_before_insert_is_not_ddl() {
        assert!(!is_ddl_statement("/* comment */ INSERT INTO t VALUES (1)"));
    }

    #[test]
    fn ddl_plain_create_is_ddl() {
        assert!(is_ddl_statement("CREATE TABLE foo (id INT)"));
        assert!(is_ddl_statement("  ALTER TABLE foo ADD COLUMN bar TEXT"));
        assert!(is_ddl_statement("DROP TABLE foo"));
        assert!(is_ddl_statement("TRUNCATE TABLE foo"));
    }

    #[test]
    fn ddl_select_is_not_ddl() {
        assert!(!is_ddl_statement("SELECT 1"));
        assert!(!is_ddl_statement("   SELECT * FROM t"));
    }

    #[test]
    fn ddl_line_comment_only_then_create_is_ddl() {
        // Comment on its own line, DDL on next line.
        assert!(is_ddl_statement("-- set up table\nCREATE TABLE t (id INT)"));
    }

    // ── RW1.1.b — sanitized forbidden-DDL rejection errors ──────

    #[test]
    fn forbidden_ddl_error_does_not_echo_raw_statement() {
        // Simulate a statement that contains credential material.
        let stmt = "DROP TABLE users; -- password=s3cr3t";
        let q = Query::new("users", stmt);
        let guard = check_admin_guard(&q, &[]);
        assert!(guard.is_some(), "DDL must be rejected");
        let msg = guard.unwrap();
        // The error message must NOT contain the raw statement or the fake password.
        assert!(
            !msg.contains("s3cr3t"),
            "error must not echo password: {msg}"
        );
        assert!(
            !msg.contains("DROP TABLE users"),
            "error must not echo raw statement: {msg}"
        );
        // But it must still tell the caller that DDL was rejected.
        assert!(
            msg.contains("DDL") || msg.contains("rejected"),
            "error must indicate rejection: {msg}"
        );
    }

    #[test]
    fn admin_op_rejection_returns_operation_name_only() {
        let q = Query::with_operation("flushall", "redis", "FLUSHALL");
        let guard = check_admin_guard(&q, &["flushall"]);
        assert!(guard.is_some());
        let msg = guard.unwrap();
        assert!(msg.contains("flushall"));
    }

    // ── RW1.1.c — $N positional parameter substitution ──────────

    #[test]
    fn dollar_positional_no_prefix_collision() {
        // $param1 and $param10 must map to independent positional slots.
        // If global string replace were used, $param1 → $1 and then $param10
        // would have already been mangled to $10 during the $param1 pass.
        let stmt = "SELECT $param1, $param10 FROM t";
        let mut params = std::collections::HashMap::new();
        params.insert("param1".to_string(), QueryValue::Integer(1));
        params.insert("param10".to_string(), QueryValue::Integer(10));

        let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::DollarPositional);

        // Both placeholders must appear as clean $N references.
        assert!(
            rewritten.contains("$1") || rewritten.contains("$2"),
            "rewritten: {rewritten}"
        );
        // $1 and $2 must be distinct — no "$10" or "$20" artefacts.
        assert!(
            !rewritten.contains("$10") && !rewritten.contains("$20"),
            "collision detected in: {rewritten}"
        );
        // The result should be something like "SELECT $1, $2 FROM t"
        // (order depends on HashMap iteration order, so we just check count).
        assert_eq!(ordered.len(), 2, "must have exactly two ordered params");
    }

    #[test]
    fn dollar_positional_basic_substitution() {
        let stmt = "INSERT INTO t VALUES ($id, $name)";
        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), QueryValue::Integer(42));
        params.insert("name".to_string(), QueryValue::String("alice".into()));

        let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::DollarPositional);
        // Both $id and $name replaced with positional placeholders
        assert!(!rewritten.contains("$id"));
        assert!(!rewritten.contains("$name"));
        assert!(rewritten.contains("$1"));
        assert!(rewritten.contains("$2"));
        assert_eq!(ordered.len(), 2);
    }

    #[test]
    fn question_positional_basic_substitution() {
        let stmt = "INSERT INTO t VALUES ($id, $name)";
        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), QueryValue::Integer(1));
        params.insert("name".to_string(), QueryValue::String("bob".into()));

        let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::QuestionPositional);
        assert_eq!(rewritten, "INSERT INTO t VALUES (?, ?)");
        assert_eq!(ordered.len(), 2);
    }

    #[test]
    fn colon_named_basic_substitution() {
        let stmt = "SELECT * FROM t WHERE id = $my_id";
        let mut params = std::collections::HashMap::new();
        params.insert("my_id".to_string(), QueryValue::Integer(7));

        let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::ColonNamed);
        assert_eq!(rewritten, "SELECT * FROM t WHERE id = :my_id");
        assert_eq!(ordered.len(), 1);
    }
}

/// ABI version for plugin compatibility checks.
///
/// Per spec §7.2 — plugins must export `_rivers_abi_version()` returning this value.
pub const ABI_VERSION: u32 = 1;

/// Trait for plugin registration callbacks.
///
/// Per spec §7.4. Plugins call methods on this trait to register
/// their driver implementations. `DriverFactory` in `rivers-core`
/// implements this trait.
pub trait DriverRegistrar {
    /// Register a database driver implementation.
    fn register_database_driver(&mut self, driver: Arc<dyn DatabaseDriver>);
    /// Register a message broker driver implementation.
    fn register_broker_driver(&mut self, driver: Arc<dyn MessageBrokerDriver>);
}
