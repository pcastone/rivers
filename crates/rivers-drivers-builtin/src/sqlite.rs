//! SqliteDriver — SQLite database driver via rusqlite.
//!
//! Per `rivers-driver-spec.md` §3.4:
//! - WAL mode enabled, 5-second busy timeout
//! - Named parameters with `:name` prefix
//! - Supports `:memory:` via `database = ":memory:"`
//! - `last_insert_id` from `last_insert_rowid()`
//! - Type mapping: INTEGER -> i64, REAL -> f64, TEXT -> String, BLOB -> hex, NULL -> Null

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverError, DriverType, HttpMethod,
    Query, QueryResult, QueryValue, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

// ── G_R8: SQLite path policy ────────────────────────────────────────
//
// Two operator-controlled knobs governed by environment variables, mirrored
// after the B2 / B3 / F2 OnceLock pattern (see `process_pool::v8_config`):
//
//   * `RIVERS_SQLITE_ALLOWED_ROOT` (path; unset = no restriction). When set,
//     any non-`:memory:` database path that resolves outside the configured
//     root is rejected with a clear `DriverError::Connection`. Prevents a
//     misconfigured `database = "/etc/passwd"` from creating SQLite files in
//     arbitrary locations.
//   * `RIVERS_SQLITE_CREATE_PARENT_DIRS` (`1` = allow auto-mkdir; default
//     `0`). The previous implementation called `std::fs::create_dir_all`
//     unconditionally, which masked typos and let SQLite scribble nested
//     directory trees wherever the path pointed. With the new default,
//     missing parent directories produce a clear connect error.

const RIVERS_SQLITE_ALLOWED_ROOT_ENV: &str = "RIVERS_SQLITE_ALLOWED_ROOT";
const RIVERS_SQLITE_CREATE_PARENT_DIRS_ENV: &str = "RIVERS_SQLITE_CREATE_PARENT_DIRS";

/// Resolve the active allowed-root for SQLite database paths. Reads the env
/// var exactly once via `OnceLock` so toggling mid-process has no effect —
/// operators must set it before riversd starts. Test helpers may bypass via
/// the override hook below.
fn allowed_root() -> Option<PathBuf> {
    if let Some(test_override) = test_override_allowed_root() {
        return test_override;
    }
    static CACHED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            std::env::var(RIVERS_SQLITE_ALLOWED_ROOT_ENV)
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .clone()
}

/// Whether the driver may auto-create missing parent directories. Default
/// is `false` — set `RIVERS_SQLITE_CREATE_PARENT_DIRS=1` to opt in.
fn create_parent_dirs() -> bool {
    if let Some(test_override) = test_override_create_parent_dirs() {
        return test_override;
    }
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var(RIVERS_SQLITE_CREATE_PARENT_DIRS_ENV)
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

#[cfg(test)]
thread_local! {
    static SQLITE_ALLOWED_ROOT_OVERRIDE: std::cell::RefCell<Option<Option<PathBuf>>> =
        const { std::cell::RefCell::new(None) };
    static SQLITE_CREATE_PARENT_DIRS_OVERRIDE: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn test_override_allowed_root() -> Option<Option<PathBuf>> {
    SQLITE_ALLOWED_ROOT_OVERRIDE.with(|c| c.borrow().clone())
}

#[cfg(not(test))]
fn test_override_allowed_root() -> Option<Option<PathBuf>> {
    None
}

#[cfg(test)]
fn test_override_create_parent_dirs() -> Option<bool> {
    SQLITE_CREATE_PARENT_DIRS_OVERRIDE.with(|c| c.get())
}

#[cfg(not(test))]
fn test_override_create_parent_dirs() -> Option<bool> {
    None
}

/// Test-only RAII guard for the allowed-root override. `None` means
/// "no restriction" (matches an unset env var); `Some(path)` restricts.
#[cfg(test)]
pub(crate) struct SqliteAllowedRootOverride;

#[cfg(test)]
impl SqliteAllowedRootOverride {
    pub(crate) fn new(root: Option<PathBuf>) -> Self {
        SQLITE_ALLOWED_ROOT_OVERRIDE.with(|c| *c.borrow_mut() = Some(root));
        Self
    }
}

#[cfg(test)]
impl Drop for SqliteAllowedRootOverride {
    fn drop(&mut self) {
        SQLITE_ALLOWED_ROOT_OVERRIDE.with(|c| *c.borrow_mut() = None);
    }
}

/// Test-only RAII guard for the create-parent-dirs override.
#[cfg(test)]
pub(crate) struct SqliteCreateParentDirsOverride;

#[cfg(test)]
impl SqliteCreateParentDirsOverride {
    pub(crate) fn new(enabled: bool) -> Self {
        SQLITE_CREATE_PARENT_DIRS_OVERRIDE.with(|c| c.set(Some(enabled)));
        Self
    }
}

#[cfg(test)]
impl Drop for SqliteCreateParentDirsOverride {
    fn drop(&mut self) {
        SQLITE_CREATE_PARENT_DIRS_OVERRIDE.with(|c| c.set(None));
    }
}

/// Redact a SQLite database path for log lines.
///
/// G_R8.2: the previous `db_path = %path` log emitted the full host
/// absolute path, leaking deployment-internal directory structure. This
/// helper produces a stable, redacted label suitable for production logs:
///
/// * `:memory:` → returned verbatim.
/// * Anything containing `/libraries/` → reported as
///   `<libraries>/<rest-after-libraries>` (mirrors the B4 redaction style
///   used by the V8 module loader, but inlined here because
///   `rivers-drivers-builtin` cannot depend on `riversd`).
/// * Anything else → `<path>/<basename>`.
fn redact_db_path(path: &str) -> String {
    if path == ":memory:" {
        return path.to_string();
    }
    if let Some(idx) = path.find("/libraries/") {
        let tail = &path[idx + "/libraries/".len()..];
        return format!("<libraries>/{tail}");
    }
    let basename = Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<unnamed>".into());
    format!("<path>/{basename}")
}

/// Validate a candidate SQLite path against the configured allowed root and
/// the auto-mkdir policy. Returns `Ok(())` if the path is acceptable, or a
/// `DriverError::Connection` describing the violation.
fn check_sqlite_path(path: &str) -> Result<(), DriverError> {
    if path == ":memory:" {
        return Ok(());
    }
    let candidate = Path::new(path);

    // Allowed-root check.
    if let Some(root) = allowed_root() {
        // Canonicalize the root (it must exist if it's been configured).
        let root_canon = root.canonicalize().unwrap_or(root.clone());
        // The candidate file may not exist yet, so canonicalize the parent
        // directory and append the basename. This resolves macOS symlinks
        // like `/var/folders/...` → `/private/var/folders/...` so a
        // tempdir-rooted candidate matches a tempdir-rooted root.
        let candidate_abs = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(candidate))
                .unwrap_or_else(|_| candidate.to_path_buf())
        };
        let candidate_canon = match (candidate_abs.parent(), candidate_abs.file_name()) {
            (Some(parent), Some(file)) if parent.exists() => {
                parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf()).join(file)
            }
            _ => normalise_path(&candidate_abs),
        };
        if !candidate_canon.starts_with(&root_canon) {
            return Err(DriverError::Connection(format!(
                "sqlite: path '{}' is outside allowed root '{}' (set RIVERS_SQLITE_ALLOWED_ROOT to widen)",
                redact_db_path(path),
                root_canon.display()
            )));
        }
    }

    // Parent-dir policy.
    if let Some(parent) = candidate.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            if create_parent_dirs() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    DriverError::Connection(format!(
                        "sqlite: failed to create parent directory for '{}': {e}",
                        redact_db_path(path)
                    ))
                })?;
            } else {
                return Err(DriverError::Connection(format!(
                    "sqlite: parent directory does not exist for '{}' (set RIVERS_SQLITE_CREATE_PARENT_DIRS=1 to auto-create)",
                    redact_db_path(path)
                )));
            }
        }
    }

    Ok(())
}

/// Lexically normalise a path: collapse `.` and `..` components without
/// hitting the filesystem. Used by the allowed-root check because the
/// candidate file does not exist yet.
fn normalise_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Supported field types for the SQLite driver.
const SQLITE_TYPES: &[&str] = &[
    "uuid", "string", "text", "integer", "float", "real", "decimal",
    "boolean", "datetime", "date", "json", "blob", "bytes",
    "email", "phone", "url",
];

/// SQLite database driver.
///
/// Creates `SqliteConnection` instances backed by rusqlite. Each connection
/// opens its own database file (or `:memory:` instance) with WAL mode and a
/// 5-second busy timeout.
///
/// See `rivers-driver-spec.md` §3.4.
pub struct SqliteDriver;

impl SqliteDriver {
    /// Create a new SQLite driver instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SqliteDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseDriver for SqliteDriver {
    fn name(&self) -> &str {
        "sqlite"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        // Resolve path: database field first, fall back to host (common for SQLite configs)
        let path = if !params.database.is_empty() {
            params.database.clone()
        } else if !params.host.is_empty() {
            params.host.clone()
        } else {
            return Err(DriverError::Connection(
                "sqlite: no database path — set 'database' or 'host' in datasource config".into(),
            ));
        };

        // G_R8.2: log the redacted path, never the host-absolute string.
        tracing::info!(
            target: "rivers.sqlite",
            db_path = %redact_db_path(&path),
            from_field = if !params.database.is_empty() { "database" } else { "host" },
            "sqlite: opening connection"
        );

        // G_R8.1 + G_R8.3: enforce the allowed-root and parent-dir policies
        // before opening. `:memory:` short-circuits inside `check_sqlite_path`.
        check_sqlite_path(&path)?;

        // Open connection on a blocking thread since rusqlite is synchronous.
        let conn = tokio::task::spawn_blocking(move || -> Result<rusqlite::Connection, DriverError> {
            let conn = rusqlite::Connection::open(&path)
                .map_err(|e| {
                    DriverError::Connection(format!(
                        "sqlite open '{}': {}",
                        redact_db_path(&path),
                        e
                    ))
                })?;

            // WAL mode for concurrent read performance (§3.4).
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|e| DriverError::Connection(format!("sqlite WAL pragma: {}", e)))?;

            // 5-second busy timeout (§3.4).
            conn.busy_timeout(Duration::from_secs(5))
                .map_err(|e| DriverError::Connection(format!("sqlite busy_timeout: {}", e)))?;

            Ok(conn)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking join: {}", e)))?
        ?;

        Ok(Box::new(SqliteConnection {
            conn: Arc::new(Mutex::new(conn)),
        }))
    }

    fn supports_transactions(&self) -> bool {
        true
    }

    fn param_style(&self) -> rivers_driver_sdk::ParamStyle {
        rivers_driver_sdk::ParamStyle::DollarNamed
    }

    fn supports_introspection(&self) -> bool {
        true
    }
}

/// A live SQLite connection wrapping `rusqlite::Connection` behind `Arc<Mutex>`.
///
/// All operations are dispatched via `tokio::task::spawn_blocking` to avoid
/// blocking the async runtime. See `rivers-driver-spec.md` §3.4.
pub struct SqliteConnection {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

#[async_trait]
impl Connection for SqliteConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        let conn = Arc::clone(&self.conn);
        let statement = query.statement.clone();
        let operation = query.operation.clone();
        let parameters = query.parameters.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| DriverError::Internal(format!("sqlite mutex poisoned: {}", e)))?;

            match operation.as_str() {
                "select" | "query" | "get" | "find" => {
                    execute_query(&conn, &statement, &parameters)
                }
                "insert" | "create" => {
                    execute_insert(&conn, &statement, &parameters)
                }
                "update" => {
                    execute_write(&conn, &statement, &parameters)
                }
                "delete" | "del" | "remove" | "drop" | "truncate" => {
                    execute_write(&conn, &statement, &parameters)
                }
                "ping" => {
                    conn.execute_batch("SELECT 1")
                        .map_err(|e| DriverError::Query(format!("sqlite ping: {}", e)))?;
                    Ok(QueryResult::empty())
                }
                op => Err(DriverError::Unsupported(format!(
                    "sqlite driver does not support operation: {}",
                    op
                ))),
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking join: {}", e)))?
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| DriverError::Internal(format!("sqlite mutex poisoned: {}", e)))?;
            conn.execute_batch("SELECT 1")
                .map_err(|e| DriverError::Query(format!("sqlite ping: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking join: {}", e)))?
    }

    async fn begin_transaction(&mut self) -> Result<(), DriverError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| DriverError::Internal(format!("sqlite mutex: {e}")))?;
            conn.execute_batch("BEGIN")
                .map_err(|e| DriverError::Query(format!("sqlite BEGIN: {e}")))
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking: {e}")))?
    }

    async fn commit_transaction(&mut self) -> Result<(), DriverError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| DriverError::Internal(format!("sqlite mutex: {e}")))?;
            conn.execute_batch("COMMIT")
                .map_err(|e| DriverError::Query(format!("sqlite COMMIT: {e}")))
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking: {e}")))?
    }

    async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|e| DriverError::Internal(format!("sqlite mutex: {e}")))?;
            conn.execute_batch("ROLLBACK")
                .map_err(|e| DriverError::Query(format!("sqlite ROLLBACK: {e}")))
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking: {e}")))?
    }

    async fn ddl_execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let conn = Arc::clone(&self.conn);
        let statement = query.statement.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| DriverError::Internal(format!("sqlite lock: {e}")))?;
            conn.execute_batch(&statement)
                .map_err(|e| DriverError::Query(format!("sqlite ddl: {e}")))?;
            Ok(QueryResult::empty())
        })
        .await
        .map_err(|e| DriverError::Internal(format!("sqlite spawn: {e}")))?
    }

    fn driver_name(&self) -> &str {
        "sqlite"
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — all run inside spawn_blocking (no async)
// ---------------------------------------------------------------------------

/// Build a vector of `(name, value)` pairs for rusqlite named-parameter binding.
/// Each parameter key is prefixed with `:` if not already present.
/// Keys are sorted to ensure deterministic binding order (HashMap iteration is
/// unordered; sorted keys make positional `$001, $002, …` bindings correct).
fn bind_params(parameters: &HashMap<String, QueryValue>) -> Vec<(String, Box<dyn rusqlite::types::ToSql>)> {
    let mut keys: Vec<&String> = parameters.keys().collect();
    keys.sort();
    keys.into_iter()
        .map(|key| {
            let val = &parameters[key];
            let name = if key.starts_with(':') || key.starts_with('@') || key.starts_with('$') {
                key.clone()
            } else {
                format!("${}", key)
            };
            let boxed: Box<dyn rusqlite::types::ToSql> = match val {
                QueryValue::Null => Box::new(rusqlite::types::Null),
                QueryValue::Boolean(b) => Box::new(*b),
                QueryValue::Integer(i) => Box::new(*i),
                QueryValue::Float(f) => Box::new(*f),
                QueryValue::String(s) => Box::new(s.clone()),
                QueryValue::Array(arr) => {
                    Box::new(serde_json::to_string(arr).unwrap_or_default())
                }
                QueryValue::Json(v) => {
                    Box::new(serde_json::to_string(v).unwrap_or_default())
                }
            };
            (name, boxed)
        })
        .collect()
}

/// Execute a SELECT statement and return rows as `Vec<HashMap<String, QueryValue>>`.
fn execute_query(
    conn: &rusqlite::Connection,
    statement: &str,
    parameters: &HashMap<String, QueryValue>,
) -> Result<QueryResult, DriverError> {
    let mut stmt = conn
        .prepare(statement)
        .map_err(|e| DriverError::Query(format!("sqlite prepare: {}", e)))?;

    let bound = bind_params(parameters);
    let param_slice: Vec<(&str, &dyn rusqlite::types::ToSql)> = bound
        .iter()
        .map(|(name, val)| (name.as_str(), val.as_ref() as &dyn rusqlite::types::ToSql))
        .collect();

    let column_names: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let rows_result = stmt
        .query_map(param_slice.as_slice(), |row| {
            let mut map = HashMap::new();
            for (i, col_name) in column_names.iter().enumerate() {
                let value = row_value_at(row, i);
                map.insert(col_name.clone(), value);
            }
            Ok(map)
        })
        .map_err(|e| DriverError::Query(format!("sqlite query: {}", e)))?;

    let mut rows = Vec::new();
    for row_result in rows_result {
        let row = row_result
            .map_err(|e| DriverError::Query(format!("sqlite row: {}", e)))?;
        rows.push(row);
    }

    let affected = rows.len() as u64;
    let result_column_names = if rows.is_empty() {
        Some(column_names)
    } else {
        None
    };
    Ok(QueryResult {
        rows,
        affected_rows: affected,
        last_insert_id: None,
        column_names: result_column_names,
    })
}

/// Execute an INSERT/CREATE statement, returning affected_rows and last_insert_id.
fn execute_insert(
    conn: &rusqlite::Connection,
    statement: &str,
    parameters: &HashMap<String, QueryValue>,
) -> Result<QueryResult, DriverError> {
    let mut stmt = conn
        .prepare(statement)
        .map_err(|e| DriverError::Query(format!("sqlite prepare: {}", e)))?;

    let bound = bind_params(parameters);
    let param_slice: Vec<(&str, &dyn rusqlite::types::ToSql)> = bound
        .iter()
        .map(|(name, val)| (name.as_str(), val.as_ref() as &dyn rusqlite::types::ToSql))
        .collect();

    let affected = stmt
        .execute(param_slice.as_slice())
        .map_err(|e| DriverError::Query(format!("sqlite execute: {}", e)))?;

    let last_id = conn.last_insert_rowid();

    Ok(QueryResult {
        rows: Vec::new(),
        affected_rows: affected as u64,
        last_insert_id: Some(last_id.to_string()),
        column_names: None,
    })
}

/// Execute an UPDATE/DELETE statement, returning affected_rows only.
fn execute_write(
    conn: &rusqlite::Connection,
    statement: &str,
    parameters: &HashMap<String, QueryValue>,
) -> Result<QueryResult, DriverError> {
    let mut stmt = conn
        .prepare(statement)
        .map_err(|e| DriverError::Query(format!("sqlite prepare: {}", e)))?;

    let bound = bind_params(parameters);
    let param_slice: Vec<(&str, &dyn rusqlite::types::ToSql)> = bound
        .iter()
        .map(|(name, val)| (name.as_str(), val.as_ref() as &dyn rusqlite::types::ToSql))
        .collect();

    let affected = stmt
        .execute(param_slice.as_slice())
        .map_err(|e| DriverError::Query(format!("sqlite execute: {}", e)))?;

    Ok(QueryResult {
        rows: Vec::new(),
        affected_rows: affected as u64,
        last_insert_id: None,
        column_names: None,
    })
}

// ---------------------------------------------------------------------------
// Unified Driver trait implementation (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

#[async_trait]
impl Driver for SqliteDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "sqlite"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        // SQLite schemas must be type "object"
        if schema.schema_type != "object" {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "sqlite".into(),
                supported: vec!["object".into()],
                schema_file: String::new(),
            });
        }
        // GET schema must have at least one field
        if method == HttpMethod::GET && schema.fields.is_empty() {
            return Err(SchemaSyntaxError::StructuralError {
                message: "GET schema must declare at least one field".into(),
                driver: "sqlite".into(),
                schema_file: String::new(),
            });
        }
        // Validate field types and attributes
        for field in &schema.fields {
            if !SQLITE_TYPES.contains(&field.field_type.as_str()) {
                return Err(SchemaSyntaxError::InvalidFieldType {
                    field: field.name.clone(),
                    field_type: field.field_type.clone(),
                    schema_file: String::new(),
                });
            }
            // Reject unsupported attributes (e.g., "faker", "key_pattern")
            rivers_driver_sdk::validation::check_supported_attributes(
                field, "sqlite", rivers_driver_sdk::validation::RELATIONAL_ATTRIBUTES, ""
            )?;
        }
        Ok(())
    }

    fn validate(
        &self,
        data: &serde_json::Value,
        schema: &SchemaDefinition,
        direction: ValidationDirection,
    ) -> Result<(), ValidationError> {
        rivers_driver_sdk::validation::validate_fields(data, schema, direction)
    }

    async fn execute(
        &self,
        _query: &Query,
        _params: &HashMap<String, QueryValue>,
    ) -> Result<QueryResult, DriverError> {
        // Delegate to DatabaseDriver::connect + Connection::execute pattern
        Err(DriverError::NotImplemented(
            "use DatabaseDriver::connect() + Connection::execute() for SQLite".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // SQLiteDriver is stateless; real connection happens via DatabaseDriver::connect()
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}

/// Extract a single column value from a rusqlite row, mapping to `QueryValue`.
///
/// Type mapping per `rivers-driver-spec.md` §3.4:
/// - INTEGER -> `QueryValue::Integer(i64)`
/// - REAL    -> `QueryValue::Float(f64)`
/// - TEXT    -> `QueryValue::String(String)`
/// - BLOB    -> `QueryValue::String(String)` (hex-encoded)
/// - NULL    -> `QueryValue::Null`
fn row_value_at(row: &rusqlite::Row<'_>, idx: usize) -> QueryValue {
    // Try types in order of specificity. rusqlite returns Err for type mismatches,
    // so we try each variant until one succeeds.
    if let Ok(v) = row.get::<_, rusqlite::types::Value>(idx) {
        match v {
            rusqlite::types::Value::Null => QueryValue::Null,
            rusqlite::types::Value::Integer(i) => QueryValue::Integer(i),
            rusqlite::types::Value::Real(f) => QueryValue::Float(f),
            rusqlite::types::Value::Text(s) => QueryValue::String(s),
            rusqlite::types::Value::Blob(b) => {
                // Hex-encode blob bytes without pulling in the `hex` crate.
                let hex_str: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
                QueryValue::String(hex_str)
            }
        }
    } else {
        QueryValue::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection};

    fn make_schema(schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        SchemaDefinition {
            driver: "sqlite".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra: HashMap::new(),
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
    fn schema_syntax_valid_object() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![
                make_field("id", "uuid", true),
                make_field("name", "string", true),
                make_field("age", "integer", false),
                make_field("price", "decimal", false),
                make_field("data", "bytes", false),
            ],
        );
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_non_object_type() {
        let driver = SqliteDriver::new();
        let schema = make_schema("array", vec![make_field("id", "integer", true)]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(
            matches!(err, SchemaSyntaxError::UnsupportedType { .. }),
            "expected UnsupportedType, got {:?}",
            err,
        );
    }

    #[test]
    fn schema_syntax_rejects_unknown_field_type() {
        let driver = SqliteDriver::new();
        let schema = make_schema("object", vec![make_field("data", "xml", false)]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(
            matches!(err, SchemaSyntaxError::InvalidFieldType { .. }),
            "expected InvalidFieldType, got {:?}",
            err,
        );
    }

    #[test]
    fn schema_syntax_get_requires_fields() {
        let driver = SqliteDriver::new();
        let schema = make_schema("object", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(
            matches!(err, SchemaSyntaxError::StructuralError { .. }),
            "expected StructuralError, got {:?}",
            err,
        );
    }

    #[test]
    fn schema_syntax_post_allows_empty_fields() {
        let driver = SqliteDriver::new();
        let schema = make_schema("object", vec![]);
        assert!(driver.check_schema_syntax(&schema, HttpMethod::POST).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_faker_attribute() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field_with("name", "text", true, vec![("faker", serde_json::json!("name"))])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedAttribute { .. }));
    }

    #[test]
    fn validate_accepts_valid_data() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!({"name": "Alice"});
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!({"age": 30});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(
            matches!(err, ValidationError::MissingRequired { ref field, .. } if field == "name"),
            "expected MissingRequired for 'name', got {:?}",
            err,
        );
    }

    #[test]
    fn validate_rejects_non_object_data_with_fields() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!("just a string");
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(
            matches!(err, ValidationError::TypeMismatch { .. }),
            "expected TypeMismatch, got {:?}",
            err,
        );
    }

    #[test]
    fn validate_type_mismatch_detected() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field("active", "boolean", true)],
        );
        let data = serde_json::json!({"active": "yes"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }

    #[test]
    fn validate_constraint_violation_detected() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field_with("score", "integer", true, vec![("min", serde_json::json!(0))])],
        );
        let data = serde_json::json!({"score": -5});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::ConstraintViolation { .. }));
    }

    // ── Connection path resolution tests ────────────────────────────

    fn make_params(host: &str, database: &str) -> ConnectionParams {
        ConnectionParams {
            host: host.into(),
            port: 0,
            database: database.into(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        }
    }

    fn q(statement: &str, params: Vec<(&str, QueryValue)>) -> Query {
        let mut query = Query::new("t", statement);
        query.parameters = params.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        query
    }

    #[tokio::test]
    async fn connect_uses_database_field() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        let mut conn = driver.connect(&params).await.unwrap();
        conn.ddl_execute(&q("CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])).await.unwrap();

        assert!(db_path.exists(), "SQLite file should exist on disk");
    }

    #[tokio::test]
    async fn connect_falls_back_to_host_when_database_empty() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("fallback.db");
        let driver = SqliteDriver::new();
        let params = make_params(db_path.to_str().unwrap(), "");

        let mut conn = driver.connect(&params).await.unwrap();
        conn.ddl_execute(&q("CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])).await.unwrap();

        assert!(db_path.exists(), "SQLite file should exist via host fallback");
    }

    #[tokio::test]
    async fn connect_errors_when_both_empty() {
        let driver = SqliteDriver::new();
        let params = make_params("", "");

        match driver.connect(&params).await {
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(msg.contains("no database path"), "should mention 'no database path', got: {msg}");
            }
            Ok(_) => panic!("should error when both host and database are empty"),
        }
    }

    #[tokio::test]
    async fn connect_creates_parent_directories() {
        // G_R8.3: parent-dir auto-creation is now opt-in. Without the
        // override the test below (`connect_errors_on_missing_parent_dir`)
        // proves the new default — we keep this happy-path coverage by
        // setting the override.
        let _create_guard = SqliteCreateParentDirsOverride::new(true);
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested/deep/dir/test.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        let mut conn = driver.connect(&params).await.unwrap();
        conn.ddl_execute(&q("CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])).await.unwrap();

        assert!(db_path.exists(), "SQLite file should exist in nested directory");
    }

    /// G_R8.3: by default, missing parent directories MUST cause connect()
    /// to fail with a clear message. The previous implementation called
    /// `std::fs::create_dir_all` unconditionally, which masked
    /// misconfigured database paths and let SQLite scribble files into
    /// arbitrary locations (e.g., the daemon's CWD).
    #[tokio::test]
    async fn connect_errors_on_missing_parent_dir_by_default() {
        // Explicitly disable the override; default behaviour is "no auto-mkdir".
        let _create_guard = SqliteCreateParentDirsOverride::new(false);
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nonexistent/deep/path/test.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        match driver.connect(&params).await {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("parent directory does not exist"),
                    "expected parent-dir error, got: {msg}"
                );
            }
            Err(other) => panic!("expected DriverError::Connection, got {other:?}"),
            Ok(_) => panic!("connect should fail when parent dir is missing and auto-mkdir is off"),
        }
        assert!(!db_path.exists(), "no file should have been created");
    }

    /// G_R8.1: when `RIVERS_SQLITE_ALLOWED_ROOT` is set, paths outside the
    /// root MUST be rejected with a clear error.
    #[tokio::test]
    async fn connect_rejects_path_outside_allowed_root() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let _root_guard = SqliteAllowedRootOverride::new(Some(
            allowed.path().to_path_buf(),
        ));
        let _create_guard = SqliteCreateParentDirsOverride::new(true);

        let db_path = outside.path().join("escape.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        match driver.connect(&params).await {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("outside allowed root"),
                    "expected allowed-root error, got: {msg}"
                );
            }
            Err(other) => panic!("expected DriverError::Connection, got {other:?}"),
            Ok(_) => panic!("connect should fail when path escapes allowed root"),
        }
    }

    /// G_R8.1: paths inside the allowed root succeed.
    #[tokio::test]
    async fn connect_accepts_path_inside_allowed_root() {
        let allowed = tempfile::tempdir().unwrap();
        let _root_guard = SqliteAllowedRootOverride::new(Some(
            allowed.path().to_path_buf(),
        ));
        let _create_guard = SqliteCreateParentDirsOverride::new(true);

        let db_path = allowed.path().join("inside.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        let mut conn = driver.connect(&params).await.unwrap();
        conn.ddl_execute(&q("CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])).await.unwrap();
        assert!(db_path.exists());
    }

    /// G_R8.1: `:memory:` is always allowed regardless of the configured
    /// allowed root.
    #[tokio::test]
    async fn connect_memory_db_bypasses_allowed_root() {
        let allowed = tempfile::tempdir().unwrap();
        let _root_guard = SqliteAllowedRootOverride::new(Some(
            allowed.path().to_path_buf(),
        ));

        let driver = SqliteDriver::new();
        let params = make_params("", ":memory:");
        let _conn = driver.connect(&params).await.expect(":memory: should always be allowed");
    }

    /// G_R8.2: the path-redaction helper produces a stable label that hides
    /// host-specific prefixes. Anything containing `/libraries/` is reported
    /// relative to that anchor; otherwise the basename is returned.
    #[test]
    fn redact_db_path_hides_host_prefix() {
        assert_eq!(
            redact_db_path("/Users/alice/work/myapp/libraries/data/app.db"),
            "<libraries>/data/app.db"
        );
        assert_eq!(
            redact_db_path("/var/lib/rivers/instance/libraries/x.db"),
            "<libraries>/x.db"
        );
        // No /libraries/ anchor — fall back to basename only.
        assert_eq!(redact_db_path("/srv/data/random.db"), "<path>/random.db");
        assert_eq!(redact_db_path(":memory:"), ":memory:");
    }

    #[tokio::test]
    async fn ddl_persists_to_disk_not_memory() {
        // Verify that DDL (CREATE TABLE) writes to the on-disk file, not an
        // in-memory database.  The test creates a table via ddl_execute(),
        // drops the connection, opens a FRESH connection to the same file,
        // and queries sqlite_master to confirm the table exists.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("ddl_persist.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        // Connection 1: CREATE TABLE via ddl_execute
        {
            let mut conn = driver.connect(&params).await.unwrap();
            conn.ddl_execute(&q(
                "CREATE TABLE orders (id INTEGER PRIMARY KEY, total REAL, customer TEXT)",
                vec![],
            ))
            .await
            .unwrap();
        }
        // conn is dropped — if this was :memory:, the table is gone

        // File must exist on disk
        assert!(db_path.exists(), "DDL should create file on disk");
        assert!(
            std::fs::metadata(&db_path).unwrap().len() > 0,
            "DB file should not be empty after DDL"
        );

        // Connection 2: verify schema survived by querying sqlite_master
        let mut conn2 = driver.connect(&params).await.unwrap();
        let result = conn2
            .execute(&q(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='orders'",
                vec![],
            ))
            .await
            .unwrap();

        assert_eq!(result.rows.len(), 1, "CREATE TABLE must persist across connections");
        assert_eq!(
            result.rows[0].get("name"),
            Some(&QueryValue::String("orders".into())),
            "table name should be 'orders'"
        );

        // Connection 2: verify we can INSERT + SELECT (schema is usable, not just metadata)
        conn2
            .execute(&q(
                "INSERT INTO orders (id, total, customer) VALUES (1, 99.95, 'acme')",
                vec![],
            ))
            .await
            .unwrap();
        let row = conn2
            .execute(&q("SELECT customer FROM orders WHERE id = 1", vec![]))
            .await
            .unwrap();
        assert_eq!(row.rows.len(), 1, "INSERT+SELECT on persisted DDL table should work");
        assert_eq!(
            row.rows[0].get("customer"),
            Some(&QueryValue::String("acme".into())),
        );
    }

    #[tokio::test]
    async fn ddl_multiple_statements_persist() {
        // Verify that ddl_execute with multiple statements (execute_batch)
        // all persist to disk.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("ddl_multi.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        // Create two tables + an index in a single ddl_execute batch
        {
            let mut conn = driver.connect(&params).await.unwrap();
            conn.ddl_execute(&q(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
                 CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id INTEGER REFERENCES users(id));
                 CREATE INDEX idx_sessions_user ON sessions(user_id);",
                vec![],
            ))
            .await
            .unwrap();
        }

        // Fresh connection: all objects must exist
        let mut conn2 = driver.connect(&params).await.unwrap();
        let tables = conn2
            .execute(&q(
                "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
                vec![],
            ))
            .await
            .unwrap();
        let table_names: Vec<&str> = tables
            .rows
            .iter()
            .filter_map(|r| r.get("name").and_then(|v| match v {
                QueryValue::String(s) => Some(s.as_str()),
                _ => None,
            }))
            .collect();
        assert!(table_names.contains(&"users"), "users table must persist");
        assert!(table_names.contains(&"sessions"), "sessions table must persist");

        let indexes = conn2
            .execute(&q(
                "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_sessions_user'",
                vec![],
            ))
            .await
            .unwrap();
        assert_eq!(indexes.rows.len(), 1, "index must persist across connections");
    }

    #[tokio::test]
    async fn connect_insert_then_select_across_connections() {
        // Regression test: INSERT + SELECT on SEPARATE connections to the SAME file
        // must return data (not null). This catches the in-memory DB bug.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("persist.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        // Connection 1: create table + insert a row
        let mut conn1 = driver.connect(&params).await.unwrap();
        conn1.ddl_execute(&q("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)", vec![])).await.unwrap();
        conn1.execute(&q("INSERT INTO items (id, name) VALUES (42, 'test-item')", vec![])).await.unwrap();

        // Connection 2: select (different connection, same file)
        let mut conn2 = driver.connect(&params).await.unwrap();
        let result = conn2.execute(&q("SELECT name FROM items WHERE id = 42", vec![])).await.unwrap();

        assert_eq!(result.rows.len(), 1, "SELECT on conn2 should see conn1's INSERT");
        assert_eq!(
            result.rows[0].get("name"),
            Some(&QueryValue::String("test-item".into())),
            "should read back the inserted value"
        );
    }
}
