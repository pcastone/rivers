//! JSON-RPC 2.0 types for MCP protocol.

use serde::{Deserialize, Serialize};

/// A JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    /// Must be "2.0".
    pub jsonrpc: String,
    /// Request ID (null for notifications).
    pub id: Option<serde_json::Value>,
    /// Method name.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    /// Always "2.0".
    pub jsonrpc: &'static str,
    /// Echoed request ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    /// Result (present on success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error (present on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i32,
    /// Error message.
    pub message: String,
    /// Optional additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    /// Create a success response.
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    /// Create an error response.
    pub fn error(id: Option<serde_json::Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0", id, result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }

    /// -32700: Parse error.
    pub fn parse_error() -> Self { Self::error(None, -32700, "Parse error") }

    /// -32600: Invalid Request.
    pub fn invalid_request(id: Option<serde_json::Value>) -> Self {
        Self::error(id, -32600, "Invalid Request")
    }

    /// -32601: Method not found.
    pub fn method_not_found(id: Option<serde_json::Value>, method: &str) -> Self {
        Self::error(id, -32601, format!("Method not found: {}", method))
    }

    /// -32602: Invalid params.
    pub fn invalid_params(id: Option<serde_json::Value>, detail: impl Into<String>) -> Self {
        Self::error(id, -32602, detail)
    }

    /// -32001: Session required.
    pub fn session_required(id: Option<serde_json::Value>) -> Self {
        Self::error(id, -32001, "Session required")
    }

    /// -32000: Server error.
    pub fn server_error(id: Option<serde_json::Value>, msg: impl Into<String>) -> Self {
        Self::error(id, -32000, msg)
    }
}
