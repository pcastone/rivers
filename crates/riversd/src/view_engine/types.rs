//! View engine types — parsed requests, context, results, and errors.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── ParsedRequest ────────────────────────────────────────────────

/// A parsed HTTP request, ready for view handler consumption.
///
/// Per spec §4.3, technology-path-spec §E1.6.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedRequest {
    /// HTTP method (GET, POST, etc.).
    pub method: String,
    /// Request path.
    pub path: String,
    /// Parsed query string parameters.
    /// Serialized as "query" per spec — `ctx.request.query` in handlers.
    #[serde(rename = "query")]
    pub query_params: HashMap<String, String>,
    /// All query string values per key (preserves duplicates).
    /// Serialized as "queryAll" — `ctx.request.queryAll` in handlers.
    #[serde(rename = "queryAll")]
    pub query_all: HashMap<String, Vec<String>>,
    /// HTTP headers.
    pub headers: HashMap<String, String>,
    /// Deserialized request body.
    pub body: serde_json::Value,
    /// Extracted path parameters (e.g. `{id}` segments).
    pub path_params: HashMap<String, String>,
}

impl ParsedRequest {
    /// Create a new parsed request with the given method and path.
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            query_params: HashMap::new(),
            query_all: HashMap::new(),
            headers: HashMap::new(),
            body: serde_json::Value::Null,
            path_params: HashMap::new(),
        }
    }
}

// ── StoreHandle ─────────────────────────────────────────────────

/// Application KV store handle — wraps StorageEngine with app namespace.
///
/// Per technology-path-spec §2.4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreHandle {
    /// Application identifier used as the store namespace.
    pub app_id: String,
}

impl StoreHandle {
    /// Reserved namespace prefixes that handlers cannot use.
    const RESERVED_PREFIXES: &'static [&'static str] =
        &["session:", "csrf:", "cache:", "raft:", "rivers:"];

    /// Create a store handle for the given application.
    pub fn new(app_id: String) -> Self {
        Self { app_id }
    }

    /// Check if a key uses a reserved namespace prefix.
    pub fn is_reserved_key(key: &str) -> bool {
        Self::RESERVED_PREFIXES.iter().any(|p| key.starts_with(p))
    }
}

// ── ViewContext ──────────────────────────────────────────────────

/// Shared context for a view execution, passed through all pipeline stages.
///
/// Per technology-path-spec §E1.1: enriched ViewContext with app identity,
/// pre-fetched data map, mutable response payload, and store handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewContext {
    /// The parsed incoming HTTP request.
    pub request: ParsedRequest,
    /// Distributed trace identifier.
    pub trace_id: String,
    /// Session data, if authenticated.
    pub session: Option<serde_json::Value>,
    /// Application ID (stable UUID from manifest).
    pub app_id: String,
    /// Entry-point slug — used to namespace DataView lookups.
    pub dv_namespace: String,
    /// Node identifier.
    pub node_id: String,
    /// Environment: "dev" | "staging" | "prod".
    pub env: String,
    /// Pre-fetched DataView results keyed by DataView name.
    pub data: HashMap<String, serde_json::Value>,
    /// Mutable response payload (replaces former `sources["primary"]`).
    pub resdata: serde_json::Value,
    /// Application KV store handle.
    pub store: StoreHandle,
}

impl ViewContext {
    /// Create a new view context for the given request and app identity.
    pub fn new(
        request: ParsedRequest,
        trace_id: String,
        app_id: String,
        dv_namespace: String,
        node_id: String,
        env: String,
    ) -> Self {
        let store = StoreHandle::new(app_id.clone());
        Self {
            request,
            trace_id,
            session: None,
            app_id,
            dv_namespace,
            node_id,
            env,
            data: HashMap::new(),
            resdata: serde_json::Value::Null,
            store,
        }
    }
}

// ── ViewResult ──────────────────────────────────────────────────

/// Result of executing a view handler pipeline.
#[derive(Debug, Serialize)]
pub struct ViewResult {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: HashMap<String, String>,
    /// Response body.
    pub body: serde_json::Value,
}

impl Default for ViewResult {
    fn default() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: serde_json::Value::Null,
        }
    }
}

// ── ViewError ───────────────────────────────────────────────────

/// View execution errors.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    /// Resource not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// HTTP method not allowed for this route.
    #[error("method not allowed: {0}")]
    MethodNotAllowed(String),

    /// Error raised by the view handler.
    #[error("handler error: {0}")]
    Handler(String),

    /// Handler threw an uncaught exception, with the remapped `.ts` stack
    /// preserved for the error response. Spec §5.3. The stack is always
    /// routed to the per-app log; it is exposed in the response envelope
    /// only in debug builds (`cfg!(debug_assertions)`) or when the app's
    /// `[base] debug = true` is wired through (future work).
    #[error("handler error: {message}")]
    HandlerWithStack {
        /// Short error message (Error.toString() output).
        message: String,
        /// Remapped stack — `.ts:line:col` positions.
        stack: String,
    },

    /// `ctx.transaction()` callback returned cleanly but `commit_transaction`
    /// failed. Transaction outcome is **unknown** — writes may or may not
    /// have persisted. Client retry policy differs from a handler throw; the
    /// response envelope flags this explicitly. Spec §6 +
    /// financial-correctness gate.
    #[error("transaction commit failed on datasource '{datasource}': {message}")]
    TransactionCommitFailed {
        /// Datasource the transaction was scoped to.
        datasource: String,
        /// Driver-layer error message.
        message: String,
    },

    /// Error in the middleware pipeline.
    #[error("pipeline error: {0}")]
    Pipeline(String),

    /// Request validation failure.
    #[error("validation error: {0}")]
    Validation(String),

    /// Internal server error.
    #[error("internal error: {0}")]
    Internal(String),
}
