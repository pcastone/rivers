//! View engine types — parsed requests, context, results, and errors.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── ParsedRequest ────────────────────────────────────────────────

/// A parsed HTTP request, ready for view handler consumption.
///
/// Per spec §4.3, technology-path-spec §E1.6.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedRequest {
    pub method: String,
    pub path: String,
    pub query_params: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: serde_json::Value,
    pub path_params: HashMap<String, String>,
}

impl ParsedRequest {
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            query_params: HashMap::new(),
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
    pub app_id: String,
}

impl StoreHandle {
    /// Reserved namespace prefixes that handlers cannot use.
    const RESERVED_PREFIXES: &'static [&'static str] =
        &["session:", "csrf:", "cache:", "raft:", "rivers:"];

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
    pub request: ParsedRequest,
    pub trace_id: String,
    pub session: Option<serde_json::Value>,
    /// Application ID.
    pub app_id: String,
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
    pub fn new(
        request: ParsedRequest,
        trace_id: String,
        app_id: String,
        node_id: String,
        env: String,
    ) -> Self {
        let store = StoreHandle::new(app_id.clone());
        Self {
            request,
            trace_id,
            session: None,
            app_id,
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
    pub status: u16,
    pub headers: HashMap<String, String>,
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
    #[error("not found: {0}")]
    NotFound(String),

    #[error("method not allowed: {0}")]
    MethodNotAllowed(String),

    #[error("handler error: {0}")]
    Handler(String),

    #[error("pipeline error: {0}")]
    Pipeline(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("internal error: {0}")]
    Internal(String),
}
