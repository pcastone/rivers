//! `inject_rivers_global()` -- Rivers.log, Rivers.crypto, Rivers.keystore,
//! Rivers.env, Rivers.db bindings and their callbacks.

use super::super::types::*;
use super::task_locals::*;
use super::init::v8_str;
use super::http::{
    json_to_v8,
    rivers_http_get_callback, rivers_http_post_callback,
    rivers_http_put_callback, rivers_http_del_callback,
};

/// Extract optional structured fields from a V8 value for `Rivers.log`.
///
/// Per spec SS5.2: `Rivers.log.info(msg, fields?)` supports an optional
/// second argument containing a fields object for structured logging.
/// Returns a JSON string of the fields, or empty string if no fields.
fn extract_log_fields(scope: &mut v8::HandleScope, val: v8::Local<v8::Value>) -> String {
    if val.is_undefined() || val.is_null() {
        return String::new();
    }
    if let Ok(obj) = v8::Local::<v8::Object>::try_from(val) {
        if let Some(json_str) = v8::json::stringify(scope, obj.into()) {
            return json_str.to_rust_string_lossy(scope);
        }
    }
    String::new()
}

fn current_app_name() -> String {
    super::task_locals::TASK_APP_NAME.with(|c| {
        c.borrow().clone().unwrap_or_else(|| "unknown".to_string())
    })
}

/// Build a JSON log line for the per-app log.
///
/// H15/T3-1: previously constructed JSON via `format!` with manual quoting,
/// which produced malformed lines whenever `app`, `level`, or `msg`
/// contained a `"`, `\n`, or any control character. We now build the
/// outer object with `serde_json::json!` so escaping is correct by
/// construction. `fields` is already a JSON-serialized object string
/// produced by V8's `JSON.stringify`, so we parse it back into a
/// `serde_json::Value` and embed it as a nested value rather than
/// concatenating it as text.
fn build_app_log_line(timestamp: &str, app: &str, level: &str, msg: &str, fields: &str) -> String {
    if fields.is_empty() {
        serde_json::json!({
            "timestamp": timestamp,
            "level": level,
            "app": app,
            "message": msg,
        })
        .to_string()
    } else {
        // `fields` is JSON produced by V8's JSON.stringify. If it somehow
        // fails to parse (shouldn't happen, but don't drop the log line),
        // fall back to embedding it as a string so the line is still
        // valid JSON.
        let fields_value: serde_json::Value = serde_json::from_str(fields)
            .unwrap_or_else(|_| serde_json::Value::String(fields.to_string()));
        serde_json::json!({
            "timestamp": timestamp,
            "level": level,
            "app": app,
            "message": msg,
            "fields": fields_value,
        })
        .to_string()
    }
}

/// Write a structured log line to the app's per-app log file (in addition to tracing).
fn write_to_app_log(app: &str, level: &str, msg: &str, fields: &str) {
    if let Some(router) = rivers_runtime::rivers_core::app_log_router::global_router() {
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let line = build_app_log_line(&timestamp, app, level, msg, fields);
        router.write(app, &line);
    }
}

#[cfg(test)]
mod tests {
    use super::{build_app_log_line, rewrite_positional_placeholders};

    // ── Bug 2: Rivers.db.query / Rivers.db.execute placeholder rewriter ──
    // The rewriter is the only piece of new logic that doesn't already
    // have coverage via the DataView engine's `translate_params` path
    // (`crates/rivers-driver-sdk/tests/param_translation_tests.rs`).
    // These tests pin down the contract: `?` and `$N` map to engine-
    // canonical `$_pN`, string literals are not rewritten, identifier-
    // adjacent `$` is left alone, and the count return value tracks
    // the maximum index seen — including sparse indices like `$1, $3`.

    #[test]
    fn rewrites_question_marks_to_underscored_named_placeholders() {
        let (out, n) =
            rewrite_positional_placeholders("SELECT * FROM t WHERE a = ? AND b = ?");
        assert_eq!(out, "SELECT * FROM t WHERE a = $_p1 AND b = $_p2");
        assert_eq!(n, 2);
    }

    #[test]
    fn rewrites_dollar_numeric_to_underscored_named_placeholders() {
        let (out, n) =
            rewrite_positional_placeholders("SELECT * FROM t WHERE id = $1 AND x = $2");
        assert_eq!(out, "SELECT * FROM t WHERE id = $_p1 AND x = $_p2");
        assert_eq!(n, 2);
    }

    #[test]
    fn preserves_string_literals_with_question_marks_and_dollars() {
        // `?` and `$1` inside `'...'` must be left alone.
        let (out, n) = rewrite_positional_placeholders(
            "INSERT INTO t (msg) VALUES ('what ? $1 ?') /* trailing */",
        );
        assert_eq!(
            out,
            "INSERT INTO t (msg) VALUES ('what ? $1 ?') /* trailing */"
        );
        assert_eq!(n, 0);
    }

    #[test]
    fn handles_doubled_quote_escape_inside_string_literal() {
        // SQL `''` is an escaped quote inside a string. The `?` here
        // is still inside the literal, so must not be rewritten.
        let (out, n) = rewrite_positional_placeholders(
            "SELECT * FROM t WHERE name = 'O''Rourke?' AND id = ?",
        );
        assert_eq!(
            out,
            "SELECT * FROM t WHERE name = 'O''Rourke?' AND id = $_p1"
        );
        assert_eq!(n, 1);
    }

    #[test]
    fn leaves_identifier_adjacent_dollar_alone() {
        // `col$1` is an identifier, not a placeholder. Same for `t.$1`.
        // The simple identifier-adjacency rule treats `_$1` as a
        // continuation of the identifier and leaves it alone.
        let (out, n) = rewrite_positional_placeholders("SELECT col$1 FROM t");
        assert_eq!(out, "SELECT col$1 FROM t");
        assert_eq!(n, 0);
    }

    #[test]
    fn sparse_dollar_numeric_tracks_max_index() {
        // Handler uses `$1` and `$3` but skips `$2`. Rewriter must
        // report a count of 3 so the caller fills the gap with Null.
        let (out, n) = rewrite_positional_placeholders("SELECT $1, $3 FROM t");
        assert_eq!(out, "SELECT $_p1, $_p3 FROM t");
        assert_eq!(n, 3);
    }

    #[test]
    fn empty_sql_yields_no_placeholders() {
        let (out, n) = rewrite_positional_placeholders("");
        assert_eq!(out, "");
        assert_eq!(n, 0);
    }

    // ── Bug 2: integration tests for Rivers.db.query / Rivers.db.execute ──
    // Drive `db_query_or_execute_core` directly against an SQLite tempfile
    // so we exercise the full SQL parameter pipeline (rewrite ↦
    // translate_params ↦ SqliteConnection::execute ↦ result marshal)
    // without needing a V8 isolate. SQLite is registered as a real
    // built-in driver, so the path is identical to production minus
    // the V8 argument parsing layer.

    use std::sync::Arc;

    fn rivers_db_test_factory()
        -> Arc<rivers_runtime::rivers_core::DriverFactory>
    {
        let mut factory = rivers_runtime::rivers_core::DriverFactory::new();
        factory.register_database_driver(Arc::new(
            rivers_runtime::rivers_core::drivers::SqliteDriver,
        ));
        Arc::new(factory)
    }

    fn rivers_db_test_resolved(db_path: &str)
        -> rivers_runtime::process_pool::types::ResolvedDatasource
    {
        let mut options = std::collections::HashMap::new();
        options.insert("driver".to_string(), "sqlite".to_string());
        let params = rivers_runtime::rivers_driver_sdk::ConnectionParams {
            host: String::new(),
            port: 0,
            database: db_path.to_string(),
            username: String::new(),
            password: String::new(),
            options,
        };
        rivers_runtime::process_pool::types::ResolvedDatasource {
            driver_name: "sqlite".to_string(),
            params,
        }
    }

    fn rivers_db_test_db_with_table() -> tempfile::NamedTempFile {
        let f = tempfile::Builder::new()
            .prefix("rivers-db-bug2-")
            .suffix(".sqlite")
            .tempfile()
            .expect("tempfile");
        let conn = rusqlite::Connection::open(f.path()).expect("open");
        conn.execute(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            [],
        )
        .expect("create table");
        drop(conn);
        f
    }

    /// Rivers.db.query returns rows after seed INSERTs round-trip.
    /// Validates the Query→QueryResult marshal path: rows present,
    /// affected_rows tracks count, last_insert_id is None for SELECT.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rivers_db_query_returns_rows() {
        let temp = rivers_db_test_db_with_table();
        // Seed 3 rows out-of-band so the SELECT result is deterministic.
        let conn = rusqlite::Connection::open(temp.path()).expect("open");
        conn.execute("INSERT INTO t (name) VALUES ('alice')", [])
            .unwrap();
        conn.execute("INSERT INTO t (name) VALUES ('bob')", []).unwrap();
        conn.execute("INSERT INTO t (name) VALUES ('carol')", [])
            .unwrap();
        drop(conn);

        let factory = rivers_db_test_factory();
        let resolved = rivers_db_test_resolved(temp.path().to_str().unwrap());

        let result = super::db_query_or_execute_core(
            factory,
            resolved,
            "ds",
            "SELECT id, name FROM t ORDER BY id",
            vec![],
            None,
            super::DbCallKind::Query,
        )
        .await
        .expect("query ok");

        let rows = result["rows"].as_array().expect("rows array");
        assert_eq!(rows.len(), 3, "three seeded rows expected");
        assert_eq!(rows[0]["name"], "alice");
        assert_eq!(rows[1]["name"], "bob");
        assert_eq!(rows[2]["name"], "carol");
        assert_eq!(result["affected_rows"], 3);
        assert!(result.get("last_insert_id").is_some());
    }

    /// Rivers.db.execute INSERTs and returns affected_rows + last_insert_id.
    /// Also validates the positional-array → `?`-rewrite path: SQLite
    /// uses DollarNamed style, so `?` becomes `$_p1, $_p2` and binds
    /// straight through `bind_params`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rivers_db_execute_inserts_and_returns_affected() {
        use rivers_runtime::rivers_driver_sdk::types::QueryValue;

        let temp = rivers_db_test_db_with_table();
        let factory = rivers_db_test_factory();
        let resolved = rivers_db_test_resolved(temp.path().to_str().unwrap());

        let result = super::db_query_or_execute_core(
            factory,
            resolved,
            "ds",
            "INSERT INTO t (id, name) VALUES (?, ?)",
            vec![QueryValue::Integer(42), QueryValue::String("dave".into())],
            None,
            super::DbCallKind::Execute,
        )
        .await
        .expect("execute ok");

        // execute() drops `rows`; only affected_rows + last_insert_id.
        assert!(
            result.get("rows").is_none(),
            "Rivers.db.execute must not include 'rows' (Bug 2 contract)"
        );
        assert_eq!(result["affected_rows"], 1);
        assert_eq!(result["last_insert_id"], "42");

        // Verify the row landed via a fresh out-of-band reader.
        let conn = rusqlite::Connection::open(temp.path()).expect("open");
        let name: String = conn
            .query_row("SELECT name FROM t WHERE id = ?", [42i64], |r| r.get(0))
            .expect("select");
        assert_eq!(name, "dave");
    }

    /// Inside an active transaction on the same datasource, an INSERT
    /// via Rivers.db.execute must route through the txn connection;
    /// rolling back must discard the row. An out-of-band reader is
    /// the ground-truth oracle: if the row reaches disk after rollback,
    /// the txn-routing was wrong.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rivers_db_execute_in_transaction_uses_txn_conn_and_rolls_back() {
        use rivers_runtime::rivers_driver_sdk::types::QueryValue;

        let temp = rivers_db_test_db_with_table();
        let db_path = temp.path().to_owned();
        let factory = rivers_db_test_factory();
        let resolved = rivers_db_test_resolved(db_path.to_str().unwrap());

        // Open a connection through the factory and BEGIN a transaction
        // on it — this is the connection Rivers.db.execute must reuse.
        let txn_map = Arc::new(crate::transaction::TransactionMap::new());
        {
            let conn = factory
                .connect(&resolved.driver_name, &resolved.params)
                .await
                .expect("connect");
            txn_map.begin("ds", conn).await.expect("begin");
        }

        // Execute INSERT inside the transaction. txn argument forces
        // the core to route through the held connection rather than
        // acquire a fresh one from the factory.
        let result = super::db_query_or_execute_core(
            Arc::clone(&factory),
            resolved.clone(),
            "ds",
            "INSERT INTO t (id, name) VALUES (?, ?)",
            vec![QueryValue::Integer(7), QueryValue::String("eve".into())],
            Some((Arc::clone(&txn_map), "ds".to_string())),
            super::DbCallKind::Execute,
        )
        .await
        .expect("txn execute ok");
        assert_eq!(result["affected_rows"], 1);

        // Pre-rollback: an out-of-band reader must see ZERO rows
        // (the txn connection holds the write — txn isolation).
        let pre = rusqlite::Connection::open(&db_path).unwrap();
        let pre_count: i64 = pre
            .query_row("SELECT COUNT(*) FROM t WHERE id = ?", [7i64], |r| r.get(0))
            .unwrap();
        assert_eq!(
            pre_count, 0,
            "pre-rollback: outside reader must not see uncommitted row"
        );
        drop(pre);

        // Rollback discards the write.
        txn_map.rollback("ds").await.expect("rollback");

        // Post-rollback: the row never existed.
        let post = rusqlite::Connection::open(&db_path).unwrap();
        let post_count: i64 = post
            .query_row("SELECT COUNT(*) FROM t WHERE id = ?", [7i64], |r| r.get(0))
            .unwrap();
        assert_eq!(
            post_count, 0,
            "post-rollback: row must be discarded — txn-routing failed if 1"
        );
    }

    /// H15/T3-1: log line round-trips through serde_json even when the
    /// message and app name contain quotes, newlines, and control chars.
    #[test]
    fn build_app_log_line_no_fields_round_trips_with_problematic_chars() {
        let line = build_app_log_line(
            "2026-04-25T00:00:00.000Z",
            "app\"with\\quote",
            "INFO",
            "msg with \"quotes\", newline\n, and tab\t and \x01 control",
            "",
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&line).expect("log line must be valid JSON");
        assert_eq!(parsed["level"], "INFO");
        assert_eq!(parsed["app"], "app\"with\\quote");
        assert_eq!(
            parsed["message"],
            "msg with \"quotes\", newline\n, and tab\t and \x01 control"
        );
        assert!(parsed.get("fields").is_none());
    }

    #[test]
    fn build_app_log_line_with_fields_embeds_object_not_string() {
        let line = build_app_log_line(
            "2026-04-25T00:00:00.000Z",
            "app",
            "WARN",
            "msg",
            r#"{"k":"v with \"quote\" and \n newline","n":42}"#,
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&line).expect("log line must be valid JSON");
        assert_eq!(parsed["fields"]["k"], "v with \"quote\" and \n newline");
        assert_eq!(parsed["fields"]["n"], 42);
    }

    #[test]
    fn build_app_log_line_with_malformed_fields_falls_back_to_string() {
        // If `fields` is somehow not valid JSON, the helper must still
        // produce a parseable line — embedding the raw text as a string.
        let line = build_app_log_line(
            "2026-04-25T00:00:00.000Z",
            "app",
            "ERROR",
            "msg",
            "this is not json",
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&line).expect("log line must be valid JSON");
        assert_eq!(parsed["fields"], "this is not json");
    }
}

/// Inject the `Rivers` global utility namespace.
///
/// - `Rivers.log.{info,warn,error}` -- native V8 callbacks -> Rust `tracing` (P2.1).
///   Supports optional structured fields: `Rivers.log.info(msg, { key: val })`.
/// - `Rivers.crypto.randomHex` -- real randomness via `rand` (P2.2).
/// - `Rivers.crypto.hashPassword/verifyPassword` -- bcrypt cost 12 (P3.6).
/// - `Rivers.crypto.timingSafeEqual` -- constant-time comparison (P3.6).
/// - `Rivers.crypto.randomBase64url` -- real random base64url (P3.6).
/// - `Rivers.crypto.hmac` -- real HMAC-SHA256 via `hmac` crate (V2).
/// - `Rivers.http.{get,post,put,del}` -- real outbound HTTP via reqwest + async bridge (V2).
///   Only injected when `TaskContext.http` is `Some` (capability gating per spec SS10.5).
/// - `Rivers.env` -- task environment variables from `TaskContext.env` (V2).
/// - `console.{log,warn,error}` -- delegates to `Rivers.log` (P2.3).
pub(super) fn inject_rivers_global(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
) -> Result<(), TaskError> {
    let global = scope.get_current_context().global(scope);

    // ── Rivers object ────────────────────────────────────────────
    let rivers_key = v8::String::new(scope, "Rivers")
        .ok_or_else(|| TaskError::Internal("failed to create 'Rivers' key".into()))?;
    let rivers_obj = v8::Object::new(scope);

    // ── Rivers.log (native V8 -> tracing, with optional structured fields) ──
    let log_obj = v8::Object::new(scope);

    let info_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            let app = current_app_name();
            if fields.is_empty() {
                tracing::info!(target: "rivers.handler", app = %app, "{}", msg);
            } else {
                tracing::info!(target: "rivers.handler", app = %app, fields = %fields, "{}", msg);
            }
            write_to_app_log(&app, "INFO", &msg, &fields);
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.info".into()))?;
    let info_key = v8_str(scope, "info")?;
    log_obj.set(scope, info_key.into(), info_fn.into());

    let warn_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            let app = current_app_name();
            if fields.is_empty() {
                tracing::warn!(target: "rivers.handler", app = %app, "{}", msg);
            } else {
                tracing::warn!(target: "rivers.handler", app = %app, fields = %fields, "{}", msg);
            }
            write_to_app_log(&app, "WARN", &msg, &fields);
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.warn".into()))?;
    let warn_key = v8_str(scope, "warn")?;
    log_obj.set(scope, warn_key.into(), warn_fn.into());

    let error_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            let app = current_app_name();
            if fields.is_empty() {
                tracing::error!(target: "rivers.handler", app = %app, "{}", msg);
            } else {
                tracing::error!(target: "rivers.handler", app = %app, fields = %fields, "{}", msg);
            }
            write_to_app_log(&app, "ERROR", &msg, &fields);
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.error".into()))?;
    let error_key = v8_str(scope, "error")?;
    log_obj.set(scope, error_key.into(), error_fn.into());

    let log_key = v8_str(scope, "log")?;
    rivers_obj.set(scope, log_key.into(), log_obj.into());

    // ── Rivers.crypto (native implementations) ───────────────────
    let crypto_obj = v8::Object::new(scope);

    // Rivers.crypto.randomHex -- real randomness via rand (P2.2)
    let random_hex_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use rand::Rng;
            let len = args.get(0).int32_value(scope).unwrap_or(16) as usize;
            let len = len.min(1024); // cap to prevent abuse
            let bytes: Vec<u8> = (0..len).map(|_| rand::thread_rng().gen()).collect();
            let hex_str = hex::encode(&bytes);
            if let Some(v8_str) = v8::String::new(scope, &hex_str) {
                rv.set(v8_str.into());
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.randomHex".into()))?;
    let random_hex_key = v8_str(scope, "randomHex")?;
    crypto_obj.set(scope, random_hex_key.into(), random_hex_fn.into());

    // Rivers.crypto.hashPassword -- bcrypt cost 12 (P3.6)
    let hash_pw_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let pw = args.get(0).to_rust_string_lossy(scope);
            match bcrypt::hash(pw, 12) {
                Ok(hashed) => {
                    if let Some(v8_str) = v8::String::new(scope, &hashed) {
                        rv.set(v8_str.into());
                    }
                }
                Err(e) => {
                    let msg = v8::String::new(scope, &format!("hashPassword failed: {e}")).unwrap();
                    let exc = v8::Exception::error(scope, msg);
                    scope.throw_exception(exc);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.hashPassword".into()))?;
    let hash_pw_key = v8_str(scope, "hashPassword")?;
    crypto_obj.set(scope, hash_pw_key.into(), hash_pw_fn.into());

    // Rivers.crypto.verifyPassword -- bcrypt verify (P3.6)
    let verify_pw_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let pw = args.get(0).to_rust_string_lossy(scope);
            let hash = args.get(1).to_rust_string_lossy(scope);
            match bcrypt::verify(pw, &hash) {
                Ok(valid) => rv.set(v8::Boolean::new(scope, valid).into()),
                Err(_) => rv.set(v8::Boolean::new(scope, false).into()),
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.verifyPassword".into()))?;
    let verify_pw_key = v8_str(scope, "verifyPassword")?;
    crypto_obj.set(scope, verify_pw_key.into(), verify_pw_fn.into());

    // Rivers.crypto.timingSafeEqual -- constant-time comparison (P3.6)
    let timing_safe_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let a = args.get(0).to_rust_string_lossy(scope);
            let b = args.get(1).to_rust_string_lossy(scope);
            // Constant-time comparison: always compare all bytes
            let equal = a.len() == b.len()
                && a.as_bytes()
                    .iter()
                    .zip(b.as_bytes())
                    .fold(0u8, |acc, (x, y)| acc | (x ^ y))
                    == 0;
            rv.set(v8::Boolean::new(scope, equal).into());
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.timingSafeEqual".into()))?;
    let timing_safe_key = v8_str(scope, "timingSafeEqual")?;
    crypto_obj.set(scope, timing_safe_key.into(), timing_safe_fn.into());

    // Rivers.crypto.randomBase64url -- real random base64url (P3.6)
    let random_b64_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use base64::Engine;
            use rand::Rng;
            let len = args.get(0).int32_value(scope).unwrap_or(16) as usize;
            let len = len.min(1024); // cap to prevent abuse
            let bytes: Vec<u8> = (0..len).map(|_| rand::thread_rng().gen()).collect();
            let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes);
            if let Some(v8_str) = v8::String::new(scope, &encoded) {
                rv.set(v8_str.into());
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.randomBase64url".into()))?;
    let random_b64_key = v8_str(scope, "randomBase64url")?;
    crypto_obj.set(scope, random_b64_key.into(), random_b64_fn.into());

    // Rivers.crypto.hmac -- HMAC-SHA256 with LockBox alias resolution (Wave 9)
    //
    // Arg 0: alias name (resolved via LockBox) or raw key (fallback when no lockbox)
    // Arg 1: data string to HMAC
    // Returns: hex-encoded HMAC-SHA256
    let hmac_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use hmac::{Hmac, Mac};
            use sha2::Sha256;
            type HmacSha256 = Hmac<Sha256>;

            let alias_or_key = args.get(0).to_rust_string_lossy(scope);
            let data = args.get(1).to_rust_string_lossy(scope);

            // Try LockBox resolution first, fall back to raw key
            let key_result: Result<String, String> = TASK_LOCKBOX.with(|lb| {
                let lb = lb.borrow();
                match lb.as_ref() {
                    Some(ctx) => {
                        let metadata = ctx.resolver.resolve(&alias_or_key)
                            .ok_or_else(|| format!("lockbox alias not found: '{alias_or_key}'"))?;
                        let resolved = rivers_runtime::rivers_core::lockbox::fetch_secret_value(
                            metadata, &ctx.keystore_path, &ctx.identity_str,
                        ).map_err(|e| format!("lockbox fetch failed: {e}"))?;
                        Ok(resolved.value.as_str().to_string())
                    }
                    None => {
                        // No lockbox configured -- use as raw key (dev/test mode)
                        Ok(alias_or_key.clone())
                    }
                }
            });

            match key_result {
                Ok(key) => {
                    match HmacSha256::new_from_slice(key.as_bytes()) {
                        Ok(mut mac) => {
                            mac.update(data.as_bytes());
                            let result = hex::encode(mac.finalize().into_bytes());
                            if let Some(v8_str) = v8::String::new(scope, &result) {
                                rv.set(v8_str.into());
                            }
                        }
                        Err(e) => {
                            let msg = v8::String::new(
                                scope,
                                &format!("Rivers.crypto.hmac() key error: {e}"),
                            )
                            .unwrap();
                            let exception = v8::Exception::error(scope, msg);
                            scope.throw_exception(exception);
                        }
                    }
                }
                Err(msg) => {
                    let err_msg = v8::String::new(scope, &msg).unwrap();
                    let exception = v8::Exception::error(scope, err_msg);
                    scope.throw_exception(exception);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.hmac".into()))?;
    let hmac_key = v8_str(scope, "hmac")?;
    crypto_obj.set(scope, hmac_key.into(), hmac_fn.into());

    // Rivers.crypto.encrypt -- AES-256-GCM encrypt via app keystore (App Keystore feature)
    //
    // Args:
    //   0: keyName (string) -- name of the key in the app keystore
    //   1: plaintext (string) -- data to encrypt
    //   2: options (optional object) -- { aad?: string }
    // Returns: { ciphertext: string, nonce: string, key_version: number }
    let encrypt_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let key_name = args.get(0).to_rust_string_lossy(scope);
            let plaintext = args.get(1).to_rust_string_lossy(scope);

            // Extract optional AAD from options object
            let aad: Option<String> = if args.length() > 2 && args.get(2).is_object() {
                let opts = args.get(2).to_object(scope).unwrap();
                let aad_key = v8::String::new(scope, "aad").unwrap();
                let aad_val = opts.get(scope, aad_key.into());
                aad_val.and_then(|v| {
                    if v.is_undefined() || v.is_null() { None }
                    else { Some(v.to_rust_string_lossy(scope)) }
                })
            } else {
                None
            };

            let result = TASK_KEYSTORE.with(|ks| {
                let ks = ks.borrow();
                match ks.as_ref() {
                    Some(ctx) => {
                        let aad_bytes = aad.as_ref().map(|a| a.as_bytes());
                        ctx.keystore.encrypt_with_key(&key_name, plaintext.as_bytes(), aad_bytes)
                            .map_err(|e| e.to_string())
                    }
                    None => Err("keystore not configured: no [[keystores]] resource declared".to_string()),
                }
            });

            match result {
                Ok(enc) => {
                    let obj = v8::Object::new(scope);

                    let ct_key = v8::String::new(scope, "ciphertext").unwrap();
                    let ct_val = v8::String::new(scope, &enc.ciphertext).unwrap();
                    obj.set(scope, ct_key.into(), ct_val.into());

                    let nonce_key = v8::String::new(scope, "nonce").unwrap();
                    let nonce_val = v8::String::new(scope, &enc.nonce).unwrap();
                    obj.set(scope, nonce_key.into(), nonce_val.into());

                    let ver_key = v8::String::new(scope, "key_version").unwrap();
                    let ver_val = v8::Integer::new(scope, enc.key_version as i32);
                    obj.set(scope, ver_key.into(), ver_val.into());

                    rv.set(obj.into());
                }
                Err(msg) => {
                    let err_msg = v8::String::new(scope, &msg).unwrap();
                    let exception = v8::Exception::error(scope, err_msg);
                    scope.throw_exception(exception);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.encrypt".into()))?;
    let encrypt_key = v8_str(scope, "encrypt")?;
    crypto_obj.set(scope, encrypt_key.into(), encrypt_fn.into());

    // Rivers.crypto.decrypt -- AES-256-GCM decrypt via app keystore (App Keystore feature)
    //
    // Args:
    //   0: keyName (string) -- name of the key in the app keystore
    //   1: ciphertext (string) -- base64 ciphertext from encrypt()
    //   2: nonce (string) -- base64 nonce from encrypt()
    //   3: options (object) -- { key_version: number, aad?: string }
    // Returns: plaintext string
    let decrypt_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let key_name = args.get(0).to_rust_string_lossy(scope);
            let ciphertext = args.get(1).to_rust_string_lossy(scope);
            let nonce = args.get(2).to_rust_string_lossy(scope);

            // Extract key_version (required) and aad (optional) from options
            let (key_version, aad): (Option<u32>, Option<String>) = if args.length() > 3 && args.get(3).is_object() {
                let opts = args.get(3).to_object(scope).unwrap();

                let ver_key = v8::String::new(scope, "key_version").unwrap();
                let ver_val = opts.get(scope, ver_key.into())
                    .and_then(|v| v.int32_value(scope))
                    .map(|v| v as u32);

                let aad_key = v8::String::new(scope, "aad").unwrap();
                let aad_val = opts.get(scope, aad_key.into())
                    .and_then(|v| {
                        if v.is_undefined() || v.is_null() { None }
                        else { Some(v.to_rust_string_lossy(scope)) }
                    });

                (ver_val, aad_val)
            } else {
                (None, None)
            };

            let key_version = match key_version {
                Some(v) => v,
                None => {
                    let msg = v8::String::new(scope, "Rivers.crypto.decrypt: options.key_version is required").unwrap();
                    let exc = v8::Exception::error(scope, msg);
                    scope.throw_exception(exc);
                    return;
                }
            };

            let result = TASK_KEYSTORE.with(|ks| {
                let ks = ks.borrow();
                match ks.as_ref() {
                    Some(ctx) => {
                        let aad_bytes = aad.as_ref().map(|a| a.as_bytes());
                        ctx.keystore.decrypt_with_key(&key_name, &ciphertext, &nonce, key_version, aad_bytes)
                            .map_err(|e| {
                                // Generic error for auth failures -- no oracle
                                match e {
                                    rivers_keystore_engine::AppKeystoreError::KeyNotFound { .. } => e.to_string(),
                                    rivers_keystore_engine::AppKeystoreError::KeyVersionNotFound { .. } => e.to_string(),
                                    _ => "decryption failed".to_string(),
                                }
                            })
                    }
                    None => Err("keystore not configured: no [[keystores]] resource declared".to_string()),
                }
            });

            match result {
                Ok(plaintext_bytes) => {
                    let plaintext = String::from_utf8_lossy(&plaintext_bytes);
                    if let Some(v8_str) = v8::String::new(scope, &plaintext) {
                        rv.set(v8_str.into());
                    }
                }
                Err(msg) => {
                    let err_msg = v8::String::new(scope, &msg).unwrap();
                    let exception = v8::Exception::error(scope, err_msg);
                    scope.throw_exception(exception);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.decrypt".into()))?;
    let decrypt_key = v8_str(scope, "decrypt")?;
    crypto_obj.set(scope, decrypt_key.into(), decrypt_fn.into());

    // Rivers.crypto.sha256(input: string): string
    //
    // Returns the lowercase hex-encoded SHA-256 digest of the UTF-8 bytes of
    // `input`. Pure helper — no key material involved (use Rivers.crypto.hmac
    // for keyed authentication).
    //
    // Per docs/bugs/case-rivers-crypto-textencoder-gap.md (Bug 1).
    let sha256_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use sha2::{Digest, Sha256};
            let input = args.get(0).to_rust_string_lossy(scope);
            let result = hex::encode(Sha256::digest(input.as_bytes()));
            if let Some(v8_str) = v8::String::new(scope, &result) {
                rv.set(v8_str.into());
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.sha256".into()))?;
    let sha256_key = v8_str(scope, "sha256")?;
    crypto_obj.set(scope, sha256_key.into(), sha256_fn.into());

    // Rivers.crypto.sha512(input: string): string
    //
    // Returns the lowercase hex-encoded SHA-512 digest of the UTF-8 bytes of
    // `input`. Pure helper — no key material involved.
    //
    // Per docs/bugs/case-rivers-crypto-textencoder-gap.md (Bug 1).
    let sha512_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use sha2::{Digest, Sha512};
            let input = args.get(0).to_rust_string_lossy(scope);
            let result = hex::encode(Sha512::digest(input.as_bytes()));
            if let Some(v8_str) = v8::String::new(scope, &result) {
                rv.set(v8_str.into());
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.sha512".into()))?;
    let sha512_key = v8_str(scope, "sha512")?;
    crypto_obj.set(scope, sha512_key.into(), sha512_fn.into());

    let crypto_key = v8_str(scope, "crypto")?;
    rivers_obj.set(scope, crypto_key.into(), crypto_obj.into());

    // ── Rivers.keystore (key metadata -- App Keystore feature) ────
    let ks_available = TASK_KEYSTORE.with(|ks| ks.borrow().is_some());
    if ks_available {
        let keystore_obj = v8::Object::new(scope);

        // Rivers.keystore.has(name) -- returns boolean
        let has_fn = v8::Function::new(
            scope,
            |scope: &mut v8::HandleScope,
             args: v8::FunctionCallbackArguments,
             mut rv: v8::ReturnValue| {
                let name = args.get(0).to_rust_string_lossy(scope);
                let result = TASK_KEYSTORE.with(|ks| {
                    ks.borrow().as_ref()
                        .map(|ctx| ctx.keystore.has_key(&name))
                        .unwrap_or(false)
                });
                rv.set(v8::Boolean::new(scope, result).into());
            },
        )
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.keystore.has".into()))?;
        let has_key = v8_str(scope, "has")?;
        keystore_obj.set(scope, has_key.into(), has_fn.into());

        // Rivers.keystore.info(name) -- returns {name, type, version, created_at} or throws
        let info_fn = v8::Function::new(
            scope,
            |scope: &mut v8::HandleScope,
             args: v8::FunctionCallbackArguments,
             mut rv: v8::ReturnValue| {
                let name = args.get(0).to_rust_string_lossy(scope);
                let result = TASK_KEYSTORE.with(|ks| {
                    let ks = ks.borrow();
                    match ks.as_ref() {
                        Some(ctx) => ctx.keystore.key_info(&name)
                            .map_err(|e| e.to_string()),
                        None => Err("keystore not configured".to_string()),
                    }
                });

                match result {
                    Ok(info) => {
                        // Build a V8 object with the metadata
                        let obj = v8::Object::new(scope);

                        let name_key = v8::String::new(scope, "name").unwrap();
                        let name_val = v8::String::new(scope, &info.name).unwrap();
                        obj.set(scope, name_key.into(), name_val.into());

                        let type_key = v8::String::new(scope, "type").unwrap();
                        let type_val = v8::String::new(scope, &info.key_type).unwrap();
                        obj.set(scope, type_key.into(), type_val.into());

                        let ver_key = v8::String::new(scope, "version").unwrap();
                        let ver_val = v8::Integer::new(scope, info.current_version as i32);
                        obj.set(scope, ver_key.into(), ver_val.into());

                        let created_key = v8::String::new(scope, "created_at").unwrap();
                        let created_val = v8::String::new(scope, &info.created.to_rfc3339()).unwrap();
                        obj.set(scope, created_key.into(), created_val.into());

                        rv.set(obj.into());
                    }
                    Err(msg) => {
                        let err_msg = v8::String::new(scope, &msg).unwrap();
                        let exception = v8::Exception::error(scope, err_msg);
                        scope.throw_exception(exception);
                    }
                }
            },
        )
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.keystore.info".into()))?;
        let info_key = v8_str(scope, "info")?;
        keystore_obj.set(scope, info_key.into(), info_fn.into());

        let ks_key = v8_str(scope, "keystore")?;
        rivers_obj.set(scope, ks_key.into(), keystore_obj.into());
    }

    // ── Rivers.http -- real outbound HTTP via async bridge (V2) ──
    // Per spec SS10.5: only injected when allow_outbound_http = true (capability gating).
    // When not injected, `Rivers.http` is undefined in JS -- natural V8 behavior.
    let http_enabled = TASK_HTTP_ENABLED.with(|h| *h.borrow());
    if http_enabled {
        let http_obj = v8::Object::new(scope);

        let http_get_fn = v8::Function::new(scope, rivers_http_get_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.get".into()))?;
        let get_key = v8_str(scope, "get")?;
        http_obj.set(scope, get_key.into(), http_get_fn.into());

        let http_post_fn = v8::Function::new(scope, rivers_http_post_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.post".into()))?;
        let post_key = v8_str(scope, "post")?;
        http_obj.set(scope, post_key.into(), http_post_fn.into());

        let http_put_fn = v8::Function::new(scope, rivers_http_put_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.put".into()))?;
        let put_key = v8_str(scope, "put")?;
        http_obj.set(scope, put_key.into(), http_put_fn.into());

        let http_del_fn = v8::Function::new(scope, rivers_http_del_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.del".into()))?;
        let del_key = v8_str(scope, "del")?;
        http_obj.set(scope, del_key.into(), http_del_fn.into());

        let http_key = v8_str(scope, "http")?;
        rivers_obj.set(scope, http_key.into(), http_obj.into());
    }

    // ── Rivers.__directDispatch -- typed-proxy dispatch for Direct datasources ──
    // Called only by the typed-proxy codegen (Task 29d). Handlers reach the
    // typed proxy via `ctx.datasource(name)`, not this raw entrypoint.
    let direct_dispatch_fn = v8::Function::new(
        scope,
        super::direct_dispatch::rivers_direct_dispatch_callback,
    )
    .ok_or_else(|| TaskError::Internal("failed to create __directDispatch".into()))?;
    let direct_key = v8_str(scope, "__directDispatch")?;
    rivers_obj.set(scope, direct_key.into(), direct_dispatch_fn.into());

    // ── Rivers.__brokerPublish -- broker producer dispatch (BR-2026-04-23) ──
    // Called by broker-proxy codegen; handlers reach it via
    // `ctx.datasource("<broker>").publish(msg)`. See
    // bugs/bugreport_2026-04-23.md.
    let broker_publish_fn = v8::Function::new(
        scope,
        super::broker_dispatch::rivers_broker_publish_callback,
    )
    .ok_or_else(|| TaskError::Internal("failed to create __brokerPublish".into()))?;
    let broker_key = v8_str(scope, "__brokerPublish")?;
    rivers_obj.set(scope, broker_key.into(), broker_publish_fn.into());

    // ── Rivers.db (imperative transaction API — spec §6 alternate form) ──
    // begin/commit/rollback/batch mirror the ctx.transaction() RAII form
    // but give handlers explicit control over the transaction boundary.
    // ctx.dataview() calls inside an open Rivers.db transaction are routed
    // through the held connection (same TransactionMap as ctx.transaction).
    let db_obj = v8::Object::new(scope);

    let db_begin_fn = v8::Function::new(scope, db_begin_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.begin".into()))?;
    let db_begin_key = v8_str(scope, "begin")?;
    db_obj.set(scope, db_begin_key.into(), db_begin_fn.into());

    let db_commit_fn = v8::Function::new(scope, db_commit_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.commit".into()))?;
    let db_commit_key = v8_str(scope, "commit")?;
    db_obj.set(scope, db_commit_key.into(), db_commit_fn.into());

    let db_rollback_fn = v8::Function::new(scope, db_rollback_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.rollback".into()))?;
    let db_rollback_key = v8_str(scope, "rollback")?;
    db_obj.set(scope, db_rollback_key.into(), db_rollback_fn.into());

    let db_batch_fn = v8::Function::new(scope, db_batch_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.batch".into()))?;
    let db_batch_key = v8_str(scope, "batch")?;
    db_obj.set(scope, db_batch_key.into(), db_batch_fn.into());

    // Bug 2 (case-rivers-db-query-missing.md) — install Rivers.db.query
    // and Rivers.db.execute. Both are documented in
    // rivers-processpool-runtime-spec-v2.md §5.2 but were never installed.
    let db_query_fn = v8::Function::new(scope, db_query_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.query".into()))?;
    let db_query_key = v8_str(scope, "query")?;
    db_obj.set(scope, db_query_key.into(), db_query_fn.into());

    let db_execute_fn = v8::Function::new(scope, db_execute_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.execute".into()))?;
    let db_execute_key = v8_str(scope, "execute")?;
    db_obj.set(scope, db_execute_key.into(), db_execute_fn.into());

    let db_key = v8_str(scope, "db")?;
    rivers_obj.set(scope, db_key.into(), db_obj.into());

    // ── Rivers.env -- task environment variables (V2) ─────────────
    let env_map = TASK_ENV.with(|e| e.borrow().clone()).unwrap_or_default();
    let env_json = serde_json::to_value(&env_map)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let env_val = json_to_v8(scope, &env_json)?;
    let env_key = v8_str(scope, "env")?;
    rivers_obj.set(scope, env_key.into(), env_val);

    // Set Rivers on global
    global.set(scope, rivers_key.into(), rivers_obj.into());

    // ── console.{log,warn,error} via JS eval ─────────────────────
    // X1.2: console delegates forward structured fields when the last argument is an object.
    let js_extras = r#"
        // console.{log,warn,error} -> Rivers.log (P2.3)
        var console = {
            log: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.info(args.join(' '), last);
            },
            warn: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.warn(args.join(' '), last);
            },
            error: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.error(args.join(' '), last);
            },
        };
    "#;
    let js_src = v8::String::new(scope, js_extras)
        .ok_or_else(|| TaskError::Internal("failed to create extras source string".into()))?;
    let script = v8::Script::compile(scope, js_src, None)
        .ok_or_else(|| TaskError::Internal("failed to compile Rivers extras".into()))?;
    script
        .run(scope)
        .ok_or_else(|| TaskError::Internal("failed to run Rivers extras".into()))?;

    Ok(())
}

// ── Rivers.db callback helpers ────────────────────────────────────────────

fn db_throw(scope: &mut v8::HandleScope, message: &str) {
    if let Some(msg) = v8::String::new(scope, message) {
        let exc = v8::Exception::error(scope, msg);
        scope.throw_exception(exc);
    }
}

/// `Rivers.db.begin(datasource)` — begin an explicit transaction.
///
/// Spec §6: stores the connection in `TASK_TRANSACTION` so subsequent
/// `ctx.dataview()` calls route through the held connection.
fn db_begin_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    use std::sync::Arc;
    use rivers_runtime::rivers_driver_sdk::DriverError;

    let ds_name = args.get(0).to_rust_string_lossy(scope);
    if ds_name.is_empty() {
        db_throw(scope, "Rivers.db.begin: datasource name is required");
        return;
    }

    // Reject nested transactions (spec §6.2)
    let already_active = TASK_TRANSACTION.with(|t| t.borrow().is_some());
    if already_active {
        db_throw(scope, "TransactionError: nested transactions not supported");
        return;
    }

    // Resolve datasource config
    let resolved = TASK_DS_CONFIGS.with(|c| c.borrow().get(&ds_name).cloned());
    let resolved = match resolved {
        Some(r) => r,
        None => {
            db_throw(scope, &format!("TransactionError: datasource \"{ds_name}\" not found in task config"));
            return;
        }
    };

    // Get driver factory
    let factory = TASK_DRIVER_FACTORY.with(|f| f.borrow().clone());
    let factory = match factory {
        Some(f) => f,
        None => {
            db_throw(scope, "TransactionError: driver factory not available");
            return;
        }
    };

    // Get tokio runtime
    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(e) => {
            db_throw(scope, &format!("TransactionError: {e}"));
            return;
        }
    };

    // Connect and begin transaction
    let txn_map = Arc::new(crate::transaction::TransactionMap::new());
    let txn_map_ref = txn_map.clone();
    let ds_for_begin = ds_name.clone();
    let begin_outcome: Result<(), DriverError> = rt.block_on(async move {
        let conn = factory.connect(&resolved.driver_name, &resolved.params).await?;
        txn_map_ref.begin(&ds_for_begin, conn).await
    });

    match begin_outcome {
        Ok(()) => {
            TASK_TRANSACTION.with(|t| {
                *t.borrow_mut() = Some(TaskTransactionState {
                    map: txn_map,
                    datasource: ds_name,
                });
            });
        }
        Err(e) => {
            let msg = match &e {
                DriverError::Unsupported(_) => {
                    format!("TransactionError: datasource \"{ds_name}\" does not support transactions")
                }
                _ => format!("TransactionError: begin failed: {e}"),
            };
            db_throw(scope, &msg);
        }
    }
}

/// `Rivers.db.commit(datasource)` — commit the active explicit transaction.
fn db_commit_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let ds_name = args.get(0).to_rust_string_lossy(scope);

    // Take the transaction state
    let state = TASK_TRANSACTION.with(|t| t.borrow_mut().take());
    let state = match state {
        Some(s) => s,
        None => {
            db_throw(scope, "TransactionError: no active transaction to commit");
            return;
        }
    };

    // Validate datasource matches (if caller passed one)
    if !ds_name.is_empty() && state.datasource != ds_name {
        let msg = format!(
            "TransactionError: active transaction is on \"{}\", not \"{ds_name}\"",
            state.datasource
        );
        // Restore state before throwing
        TASK_TRANSACTION.with(|t| *t.borrow_mut() = Some(state));
        db_throw(scope, &msg);
        return;
    }

    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(e) => {
            // Restore state so caller can rollback
            TASK_TRANSACTION.with(|t| *t.borrow_mut() = Some(state));
            db_throw(scope, &format!("TransactionError: {e}"));
            return;
        }
    };

    let ds = state.datasource.clone();
    let commit_res = rt.block_on(state.map.commit(&ds));
    // Connection drops → pool slot released

    if let Err(e) = commit_res {
        let driver_msg = format!("{e}");
        // Spec §6 + financial-correctness: stash for TaskError::TransactionCommitFailed upgrade.
        TASK_COMMIT_FAILED.with(|c| {
            *c.borrow_mut() = Some((ds.clone(), driver_msg.clone()));
        });
        db_throw(
            scope,
            &format!("TransactionError: commit failed on datasource '{ds}': {driver_msg}"),
        );
    }
}

/// `Rivers.db.rollback(datasource)` — rollback the active explicit transaction.
fn db_rollback_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let ds_name = args.get(0).to_rust_string_lossy(scope);

    let state = TASK_TRANSACTION.with(|t| t.borrow_mut().take());
    let state = match state {
        Some(s) => s,
        None => {
            // No active transaction — silently succeed (idempotent rollback).
            return;
        }
    };

    if !ds_name.is_empty() && state.datasource != ds_name {
        let msg = format!(
            "TransactionError: active transaction is on \"{}\", not \"{ds_name}\"",
            state.datasource
        );
        TASK_TRANSACTION.with(|t| *t.borrow_mut() = Some(state));
        db_throw(scope, &msg);
        return;
    }

    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(_) => return, // best-effort
    };

    let ds = state.datasource.clone();
    if let Err(e) = rt.block_on(state.map.rollback(&ds)) {
        tracing::warn!(
            target: "rivers.handler",
            datasource = %ds,
            error = %e,
            "Rivers.db.rollback: rollback failed"
        );
    }
}

/// `Rivers.db.batch(dataview, [...params])` — execute a DataView once per
/// param entry and return an array of results.
///
/// Routes each execution through the active `TASK_TRANSACTION` connection
/// (if any) exactly as `ctx.dataview()` does — the TransactionMap
/// take/return protocol is used per-iteration so the connection is
/// exclusively held for each call.
fn db_batch_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    use rivers_runtime::rivers_driver_sdk::types::QueryValue;

    let dv_name = args.get(0).to_rust_string_lossy(scope);

    // Parse the params array from arg[1]
    let params_val = args.get(1);
    let params_json: Vec<serde_json::Map<String, serde_json::Value>> =
        if params_val.is_array() || params_val.is_object() {
            if let Some(json_str) = v8::json::stringify(scope, params_val) {
                let s = json_str.to_rust_string_lossy(scope);
                match serde_json::from_str::<serde_json::Value>(&s) {
                    Ok(serde_json::Value::Array(arr)) => arr
                        .into_iter()
                        .filter_map(|v| {
                            if let serde_json::Value::Object(m) = v { Some(m) } else { None }
                        })
                        .collect(),
                    _ => vec![],
                }
            } else {
                vec![]
            }
        } else {
            vec![]
        };

    let executor = TASK_DV_EXECUTOR.with(|e| e.borrow().clone());
    let executor = match executor {
        Some(e) => e,
        None => {
            db_throw(scope, &format!("Rivers.db.batch: no DataViewExecutor available"));
            return;
        }
    };

    // Namespace the dataview name (same logic as ctx_dataview_callback)
    let namespaced = TASK_DV_NAMESPACE.with(|n| {
        n.borrow().as_ref()
            .filter(|ns| !ns.is_empty() && !dv_name.contains(':'))
            .map(|ns| format!("{ns}:{dv_name}"))
            .unwrap_or_else(|| dv_name.clone())
    });

    let trace_id = TASK_TRACE_ID.with(|t| t.borrow().clone()).unwrap_or_default();

    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(e) => {
            db_throw(scope, &format!("Rivers.db.batch: {e}"));
            return;
        }
    };

    // Execute each param set, routing through the active transaction if present.
    let results: Vec<serde_json::Value> = {
        let mut out = Vec::with_capacity(params_json.len());
        for entry in params_json {
            let qp: std::collections::HashMap<String, QueryValue> = entry
                .into_iter()
                .map(|(k, v)| (k, super::datasource::json_to_query_value(v)))
                .collect();

            let txn_state: Option<(std::sync::Arc<crate::transaction::TransactionMap>, String)> =
                TASK_TRANSACTION.with(|t| {
                    t.borrow().as_ref().map(|s| (s.map.clone(), s.datasource.clone()))
                });

            let exec_res = rt.block_on(async {
                if let Some((map, ds)) = txn_state {
                    if let Some(mut conn) = map.take_connection(&ds).await {
                        let res = executor
                            .execute(&namespaced, qp, "GET", &trace_id, Some(&mut conn))
                            .await;
                        map.return_connection(&ds, conn).await;
                        res
                    } else {
                        Err(rivers_runtime::dataview_engine::DataViewError::Driver(
                            format!("transaction connection for '{ds}' unavailable"),
                        ))
                    }
                } else {
                    executor.execute(&namespaced, qp, "GET", &trace_id, None).await
                }
            });

            match exec_res {
                Ok(response) => {
                    out.push(serde_json::json!({
                        "rows": response.query_result.rows,
                        "affected_rows": response.query_result.affected_rows,
                        "last_insert_id": response.query_result.last_insert_id,
                    }));
                }
                Err(e) => {
                    // On any execution error, throw JS exception immediately
                    let msg = format!("Rivers.db.batch('{}') error: {e}", dv_name);
                    db_throw(scope, &msg);
                    return;
                }
            }
        }
        out
    };

    // Convert results array to V8
    let json_str = serde_json::to_string(&results).unwrap_or_else(|_| "[]".into());
    if let Some(v8_str) = v8::String::new(scope, &json_str) {
        if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
            rv.set(parsed);
        }
    }
}

// ── Rivers.db.query / Rivers.db.execute (Bug 2) ───────────────────────────
// Documented in rivers-processpool-runtime-spec-v2.md §5.2 but never
// installed prior to this change. Closes
// docs/bugs/case-rivers-db-query-missing.md.
//
// Both callbacks accept (datasource, sql, params?). `params` is a
// positional JS array. Index N (0-based) becomes parameter `_pN+1`,
// then translate_params (the same helper the DataView engine uses
// in `dataview_engine.rs:850-872`) rewrites the SQL into the driver's
// native placeholder style and rebuilds the parameters HashMap with
// zero-padded numeric keys for positional drivers.
//
// To bridge the gap between the user's natural placeholder choice
// (`?` for MySQL/SQLite, `$1`/`$2` for Postgres) and the engine's
// `$name` convention, we rewrite both forms into `$_pN` *before*
// calling `translate_params`. This means a handler can write SQL in
// the dialect of its target driver and the bindings line up.
//
// Connection routing mirrors `db_batch_callback` exactly:
//   1. If TASK_TRANSACTION is active and matches the call's
//      datasource → use the txn connection via TransactionMap
//      take/return.
//   2. If TASK_TRANSACTION is active but the datasource MISMATCHES
//      → throw a JS TransactionError (cross-datasource inside txn).
//   3. Otherwise → acquire a fresh connection via
//      `factory.connect(driver, params)`.
//
// Returns are sync values, not Promises. Spec §5.2's `Promise<...>`
// annotation is aspirational — `db_batch_callback` already returns
// sync values, and we match it to keep the surface internally
// consistent.

/// Convert a JS positional placeholder SQL string into the engine's
/// `$name` form by replacing every `?` and `$N` (where N is a positive
/// integer literal) with `$_pK` (K = 1..=count). Returns the rewritten
/// SQL plus the number of placeholders detected.
///
/// Rules (kept narrow on purpose so handlers see predictable behavior):
/// - `?` is a placeholder *unless* it appears inside a single-quoted
///   string literal. We track quote state to avoid rewriting `'?'` etc.
/// - `$N` is a placeholder *unless* preceded by an identifier
///   character (treats `$column$tag` and identifier-style dollars as
///   non-placeholders) or immediately followed by an alphanumeric
///   character (e.g. `$tag$ ... $tag$` in Postgres dollar-quoted
///   strings remains untouched because `$tag` starts with a letter,
///   not a digit; this rewriter only matches `$<digits>`).
/// - String literals (`'...'`) suppress placeholder detection. SQL
///   `''`-escape sequences inside literals stay inside the literal.
///
/// This is intentionally simpler than a full SQL parser. Drivers that
/// need exotic placeholder forms can use the existing
/// `ctx.datasource(name).fromQuery(sql).build({...})` named-object API.
fn rewrite_positional_placeholders(sql: &str) -> (String, usize) {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 8);
    let mut count: usize = 0;
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            out.push(c as char);
            if c == b'\'' {
                // SQL escapes a literal quote by doubling: `''`.
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    out.push('\'');
                    i += 2;
                    continue;
                }
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == b'\'' {
            in_string = true;
            out.push('\'');
            i += 1;
            continue;
        }
        if c == b'?' {
            count += 1;
            out.push_str(&format!("$_p{count}"));
            i += 1;
            continue;
        }
        if c == b'$' {
            // Only digits → numeric placeholder. Letter/underscore →
            // user-named placeholder, leave alone (lets handlers mix
            // positional with named if they really want).
            let prev_is_ident = i > 0
                && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if !prev_is_ident {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    // Got `$<digits>` — treat as positional.
                    let n_str = std::str::from_utf8(&bytes[i + 1..j]).unwrap_or("0");
                    let n: usize = n_str.parse().unwrap_or(0);
                    if n >= 1 {
                        // `count` tracks the maximum index seen so the
                        // params array can be sized correctly. Numeric
                        // `$N` is allowed to skip indexes (`$1, $3`)
                        // — handlers that do that get `Null` for the
                        // missing slot.
                        if n > count {
                            count = n;
                        }
                        out.push_str(&format!("$_p{n}"));
                        i = j;
                        continue;
                    }
                }
            }
        }
        out.push(c as char);
        i += 1;
    }
    (out, count)
}

/// Common implementation behind Rivers.db.query and Rivers.db.execute.
///
/// `kind` selects the Result shape — `Query` keeps `rows`, `Execute`
/// drops it for the INSERT/UPDATE/DELETE callers.
#[derive(Clone, Copy)]
pub(crate) enum DbCallKind {
    Query,
    Execute,
}

/// Async core of `Rivers.db.query` / `Rivers.db.execute`. Pulls the
/// V8 host context out of the public surface so it can be exercised
/// directly against a real SQLite database in unit tests.
///
/// `txn` (if Some) is the `(map, datasource)` of an active transaction
/// for the current task. The caller is responsible for the cross-DS
/// reject — passing a mismatched (txn_ds, ds_name) here will simply
/// fail to find the connection and surface a "connection unavailable"
/// error. The V8 wrapper performs the cross-DS check before calling
/// in to keep the user-facing error message specific.
pub(crate) async fn db_query_or_execute_core(
    factory: std::sync::Arc<rivers_runtime::rivers_core::DriverFactory>,
    resolved: rivers_runtime::process_pool::types::ResolvedDatasource,
    ds_name: &str,
    sql_in: &str,
    positional_params: Vec<rivers_runtime::rivers_driver_sdk::types::QueryValue>,
    txn: Option<(std::sync::Arc<crate::transaction::TransactionMap>, String)>,
    kind: DbCallKind,
) -> Result<serde_json::Value, String> {
    use rivers_runtime::rivers_driver_sdk::types::{Query, QueryValue};
    use std::collections::HashMap;

    if sql_in.trim().is_empty() {
        return Err("sql is required".into());
    }

    // Build the params HashMap from the positional vec.
    let mut params: HashMap<String, QueryValue> = positional_params
        .into_iter()
        .enumerate()
        .map(|(i, v)| (format!("_p{}", i + 1), v))
        .collect();

    // Rewrite `?` and `$N` to engine-canonical `$_pN` so `translate_params`
    // (which only matches `$alpha`) can do the per-driver finalization.
    let (rewritten_sql, max_index) = rewrite_positional_placeholders(sql_in);

    // Backfill missing positions (e.g. handler used `$3` without `$2`)
    // with explicit nulls so the bound list is dense.
    for n in 1..=max_index {
        let key = format!("_p{n}");
        params.entry(key).or_insert(QueryValue::Null);
    }

    // Per-driver placeholder translation. Mirrors the DataView engine
    // pre-execute step at dataview_engine.rs:850-872.
    let mut final_sql = rewritten_sql;
    let mut final_params: HashMap<String, QueryValue> = params;
    if let Some(driver) = factory.get_driver(&resolved.driver_name) {
        let style = driver.param_style();
        if style != rivers_runtime::rivers_driver_sdk::ParamStyle::None {
            let (rewritten, ordered) = rivers_runtime::rivers_driver_sdk::translate_params(
                &final_sql,
                &final_params,
                style,
            );
            final_sql = rewritten;
            if style == rivers_runtime::rivers_driver_sdk::ParamStyle::DollarPositional
                || style == rivers_runtime::rivers_driver_sdk::ParamStyle::QuestionPositional
            {
                final_params.clear();
                for (i, (_k, v)) in ordered.into_iter().enumerate() {
                    final_params.insert(format!("{:03}", i + 1), v);
                }
            }
        }
    }

    let mut query = Query::new(ds_name, &final_sql);
    query.parameters = final_params;

    let query_result = if let Some((map, ds)) = txn {
        // Take the connection out of the txn map for the duration
        // of the call, then return it so the same map can drive
        // subsequent calls or commit/rollback. (Same protocol as
        // db_batch_callback uses.)
        if let Some(mut conn) = map.take_connection(&ds).await {
            let res = conn
                .execute(&query)
                .await
                .map_err(|e| format!("query failed: {e}"));
            map.return_connection(&ds, conn).await;
            res?
        } else {
            return Err(format!(
                "TransactionError: connection for datasource '{ds}' is unavailable \
                 (race with commit/rollback?)"
            ));
        }
    } else {
        let mut conn = factory
            .connect(&resolved.driver_name, &resolved.params)
            .await
            .map_err(|e| format!("connection failed: {e}"))?;
        conn.execute(&query)
            .await
            .map_err(|e| format!("query failed: {e}"))?
    };

    let json = match kind {
        DbCallKind::Query => serde_json::json!({
            "rows": query_result.rows,
            "affected_rows": query_result.affected_rows,
            "last_insert_id": query_result.last_insert_id,
        }),
        DbCallKind::Execute => serde_json::json!({
            "affected_rows": query_result.affected_rows,
            "last_insert_id": query_result.last_insert_id,
        }),
    };
    Ok(json)
}

fn db_query_or_execute(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    rv: &mut v8::ReturnValue,
    kind: DbCallKind,
) {
    let cb_name = match kind {
        DbCallKind::Query => "Rivers.db.query",
        DbCallKind::Execute => "Rivers.db.execute",
    };

    // Arg 0: datasource name.
    let ds_name = args.get(0).to_rust_string_lossy(scope);
    if ds_name.is_empty() {
        db_throw(scope, &format!("{cb_name}: datasource name is required"));
        return;
    }

    // Arg 1: SQL statement.
    let sql_val = args.get(1);
    if sql_val.is_undefined() || sql_val.is_null() {
        db_throw(scope, &format!("{cb_name}: sql is required"));
        return;
    }
    let sql_in = sql_val.to_rust_string_lossy(scope);
    if sql_in.trim().is_empty() {
        db_throw(scope, &format!("{cb_name}: sql is required"));
        return;
    }

    // Capability: datasource must be declared in the task's view config.
    let is_declared = TASK_DS_CONFIGS.with(|c| c.borrow().contains_key(&ds_name));
    if !is_declared {
        db_throw(
            scope,
            &format!("CapabilityError: datasource '{ds_name}' not declared in view config"),
        );
        return;
    }

    // Cross-datasource reject inside an active transaction. Done at the
    // V8 layer (not the core) so the user-facing message is specific
    // about which datasource is mismatched. Same wording style as the
    // existing db_commit/rollback callbacks.
    let txn_state: Option<(std::sync::Arc<crate::transaction::TransactionMap>, String)> =
        TASK_TRANSACTION.with(|t| {
            t.borrow().as_ref().map(|s| (s.map.clone(), s.datasource.clone()))
        });
    if let Some((_, ref txn_ds)) = txn_state {
        if txn_ds != &ds_name {
            db_throw(
                scope,
                &format!(
                    "TransactionError: active transaction is on \"{txn_ds}\", not \"{ds_name}\" — \
                     {cb_name} cannot route across datasources inside a transaction"
                ),
            );
            return;
        }
    }

    // Arg 2: optional positional params array.
    let params_val = args.get(2);
    let positional = build_positional_params_vec_from_v8(scope, params_val);

    let resolved = match TASK_DS_CONFIGS.with(|c| c.borrow().get(&ds_name).cloned()) {
        Some(r) => r,
        None => {
            db_throw(scope, &format!("{cb_name}: datasource \"{ds_name}\" not found in task config"));
            return;
        }
    };
    let factory = match TASK_DRIVER_FACTORY.with(|f| f.borrow().clone()) {
        Some(f) => f,
        None => {
            db_throw(scope, &format!("{cb_name}: driver factory not available"));
            return;
        }
    };

    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(e) => {
            db_throw(scope, &format!("{cb_name}: {e}"));
            return;
        }
    };

    let result = rt.block_on(db_query_or_execute_core(
        factory,
        resolved,
        &ds_name,
        &sql_in,
        positional,
        txn_state,
        kind,
    ));

    let json = match result {
        Ok(j) => j,
        Err(e) => {
            db_throw(scope, &format!("{cb_name} error: {e}"));
            return;
        }
    };

    let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "null".into());
    if let Some(v8_s) = v8::String::new(scope, &json_str) {
        if let Some(parsed) = v8::json::parse(scope, v8_s.into()) {
            rv.set(parsed);
        } else {
            rv.set(v8::null(scope).into());
        }
    }
}

/// Materialize a V8 array into a Vec<QueryValue> in array order.
/// Non-array / nullish args yield an empty vec — same surface as
/// omitting the argument.
fn build_positional_params_vec_from_v8(
    scope: &mut v8::HandleScope,
    val: v8::Local<v8::Value>,
) -> Vec<rivers_runtime::rivers_driver_sdk::types::QueryValue> {
    if val.is_undefined() || val.is_null() {
        return Vec::new();
    }
    if !val.is_array() {
        return Vec::new();
    }
    if let Some(json_str) = v8::json::stringify(scope, val) {
        let s = json_str.to_rust_string_lossy(scope);
        if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(&s) {
            return arr
                .into_iter()
                .map(super::datasource::json_to_query_value)
                .collect();
        }
    }
    Vec::new()
}

/// `Rivers.db.query(datasource, sql, params?)` — execute a SQL statement
/// and return `{ rows, affected_rows, last_insert_id }`.
///
/// Documented in rivers-processpool-runtime-spec-v2.md §5.2; closes
/// docs/bugs/case-rivers-db-query-missing.md (Bug 2).
fn db_query_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    db_query_or_execute(scope, args, &mut rv, DbCallKind::Query);
}

/// `Rivers.db.execute(datasource, sql, params?)` — same as `query` but
/// returns `{ affected_rows, last_insert_id }` (no rows). For
/// INSERT/UPDATE/DELETE.
fn db_execute_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    db_query_or_execute(scope, args, &mut rv, DbCallKind::Execute);
}
