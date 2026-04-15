# MCP View Type Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the MCP (Model Context Protocol) view type — a JSON-RPC 2.0 endpoint that exposes DataViews as AI-consumable tools, resources, and prompts per the MCP Streamable HTTP transport specification.

**Architecture:** MCP is a new `view_type = "MCP"` that activates a JSON-RPC dispatcher in the view layer. Tools map to DataViews, resources are read-only DataViews, prompts are markdown templates. The dispatcher calls the same `dataview_engine.execute()` path as REST views. Sessions use StorageEngine. Streaming tools use SSE automatically when the target DataView has `streaming = true`.

**Tech Stack:** Rust, serde_json (JSON-RPC), Axum (HTTP POST + SSE), StorageEngine (sessions), DataView engine (tool execution)

**Spec:** `docs/arch/rivers-mcp-view-spec.md`

---

## Phase 1: MVP — Config Types + JSON-RPC Dispatcher + Tools

The minimum viable MCP endpoint: can list tools and execute them.

---

### Task 1: MCP Config Types

**Files:**
- Modify: `crates/rivers-runtime/src/view.rs`
- Modify: `crates/rivers-runtime/src/validate_structural.rs`

- [ ] **Step 1: Define MCP config structs**

In `crates/rivers-runtime/src/view.rs`, add the MCP config types after `ParameterMappingConfig`:

```rust
/// MCP tool declaration — maps a DataView to an MCP tool.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpToolConfig {
    /// Target DataView name.
    pub dataview: String,
    /// Human-readable description for the AI model.
    #[serde(default)]
    pub description: String,
    /// HTTP method (GET/POST/PUT/DELETE) when DataView supports multiple.
    #[serde(default)]
    pub method: Option<String>,
    /// Tool behavior hints for the AI model.
    #[serde(default)]
    pub hints: McpToolHints,
}

/// MCP tool behavior hints.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpToolHints {
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "default_true")]
    pub destructive: bool,
    #[serde(default)]
    pub idempotent: bool,
    #[serde(default = "default_true")]
    pub open_world: bool,
}

fn default_true() -> bool { true }

/// MCP resource declaration — read-only DataView exposure.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpResourceConfig {
    /// Target DataView name (GET method only).
    pub dataview: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// MIME type for the resource. Default: "application/json".
    #[serde(default = "default_mime")]
    pub mime_type: String,
}

fn default_mime() -> String { "application/json".into() }

/// MCP prompt declaration — markdown template with argument substitution.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpPromptConfig {
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Path to markdown template file (relative to app bundle root).
    pub template: String,
    /// Prompt arguments for template substitution.
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

/// A single prompt argument.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpPromptArgument {
    /// Argument name (matches {placeholder} in template).
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Whether this argument is required.
    #[serde(default)]
    pub required: bool,
    /// Default value when not provided.
    #[serde(default)]
    pub default: Option<String>,
}

/// MCP session configuration.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpSessionConfig {
    /// Session TTL in seconds. Default: 3600 (1 hour).
    #[serde(default = "default_mcp_ttl")]
    pub ttl_seconds: u64,
}

fn default_mcp_ttl() -> u64 { 3600 }
```

- [ ] **Step 2: Add MCP fields to ApiViewConfig**

In the `ApiViewConfig` struct, add after the polling field:

```rust
    /// MCP tool declarations — whitelisted DataViews exposed as MCP tools.
    #[serde(default)]
    pub tools: HashMap<String, McpToolConfig>,

    /// MCP resource declarations — read-only DataViews exposed as MCP resources.
    #[serde(default)]
    pub resources: HashMap<String, McpResourceConfig>,

    /// MCP prompt declarations — markdown templates for AI workflows.
    #[serde(default)]
    pub prompts: HashMap<String, McpPromptConfig>,

    /// Path to static instructions markdown file (relative to app root).
    #[serde(default)]
    pub instructions: Option<String>,

    /// MCP session configuration.
    #[serde(default)]
    pub session: Option<McpSessionConfig>,
```

- [ ] **Step 3: Add "MCP" to valid view types and known fields**

In `validate_structural.rs`:
- Add `"tools"`, `"resources"`, `"prompts"`, `"instructions"`, `"session"` to `VIEW_FIELDS`
- If there's a view type enum validation, add `"MCP"` (or `"Mcp"`) as valid

- [ ] **Step 4: Verify and commit**

Run: `cargo check -p rivers-runtime && cargo test -p rivers-runtime --lib`

```bash
git commit -m "feat(mcp): add MCP config types — tools, resources, prompts, session"
```

---

### Task 2: JSON-RPC Dispatcher Module

**Files:**
- Create: `crates/riversd/src/mcp/mod.rs`
- Create: `crates/riversd/src/mcp/jsonrpc.rs`
- Create: `crates/riversd/src/mcp/dispatch.rs`
- Modify: `crates/riversd/src/lib.rs`

- [ ] **Step 1: Create JSON-RPC types**

Create `crates/riversd/src/mcp/jsonrpc.rs`:

```rust
//! JSON-RPC 2.0 types for MCP protocol.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    pub fn error(id: Option<serde_json::Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0", id, result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }

    pub fn parse_error() -> Self {
        Self::error(None, -32700, "Parse error")
    }

    pub fn invalid_request(id: Option<serde_json::Value>) -> Self {
        Self::error(id, -32600, "Invalid Request")
    }

    pub fn method_not_found(id: Option<serde_json::Value>, method: &str) -> Self {
        Self::error(id, -32601, format!("Method not found: {}", method))
    }

    pub fn invalid_params(id: Option<serde_json::Value>, detail: impl Into<String>) -> Self {
        Self::error(id, -32602, detail)
    }

    pub fn session_required(id: Option<serde_json::Value>) -> Self {
        Self::error(id, -32001, "Session required")
    }

    pub fn server_error(id: Option<serde_json::Value>, msg: impl Into<String>) -> Self {
        Self::error(id, -32000, msg)
    }
}
```

- [ ] **Step 2: Create MCP dispatch module**

Create `crates/riversd/src/mcp/dispatch.rs`:

```rust
//! MCP JSON-RPC method dispatcher.

use std::sync::Arc;
use rivers_runtime::view::{McpToolConfig, McpResourceConfig, McpPromptConfig};

use super::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::server::context::AppContext;

/// Dispatch a single JSON-RPC request to the appropriate MCP handler.
pub async fn dispatch(
    ctx: &AppContext,
    req: &JsonRpcRequest,
    tools: &std::collections::HashMap<String, McpToolConfig>,
    resources: &std::collections::HashMap<String, McpResourceConfig>,
    prompts: &std::collections::HashMap<String, McpPromptConfig>,
    app_id: &str,
    dv_namespace: &str,
) -> JsonRpcResponse {
    // Validate JSON-RPC version
    if req.jsonrpc != "2.0" {
        return JsonRpcResponse::invalid_request(req.id.clone());
    }

    match req.method.as_str() {
        "initialize" => handle_initialize(req, tools, resources, prompts),
        "ping" => JsonRpcResponse::success(req.id.clone(), serde_json::json!({})),
        "tools/list" => handle_tools_list(req, tools, ctx, dv_namespace).await,
        "tools/call" => handle_tools_call(req, tools, ctx, dv_namespace).await,
        "resources/list" => handle_resources_list(req, resources),
        "resources/read" => handle_resources_read(req, resources, ctx, dv_namespace).await,
        "resources/templates/list" => handle_resource_templates(req, resources, app_id),
        "prompts/list" => handle_prompts_list(req, prompts),
        "prompts/get" => handle_prompts_get(req, prompts),
        _ => JsonRpcResponse::method_not_found(req.id.clone(), &req.method),
    }
}

fn handle_initialize(
    req: &JsonRpcRequest,
    tools: &std::collections::HashMap<String, McpToolConfig>,
    resources: &std::collections::HashMap<String, McpResourceConfig>,
    prompts: &std::collections::HashMap<String, McpPromptConfig>,
) -> JsonRpcResponse {
    JsonRpcResponse::success(req.id.clone(), serde_json::json!({
        "protocolVersion": "2024-11-05",
        "serverInfo": {
            "name": "rivers-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": { "listChanged": false },
            "resources": if resources.is_empty() { serde_json::Value::Null } else { serde_json::json!({}) },
            "prompts": if prompts.is_empty() { serde_json::Value::Null } else { serde_json::json!({}) },
        }
    }))
}

async fn handle_tools_list(
    req: &JsonRpcRequest,
    tools: &std::collections::HashMap<String, McpToolConfig>,
    ctx: &AppContext,
    dv_namespace: &str,
) -> JsonRpcResponse {
    let dv_guard = ctx.dataview_executor.read().await;
    let executor = match dv_guard.as_ref() {
        Some(e) => e,
        None => return JsonRpcResponse::server_error(req.id.clone(), "DataView engine not available"),
    };

    let tool_list: Vec<serde_json::Value> = tools.iter().map(|(name, config)| {
        // Project DataView parameters into MCP inputSchema
        let namespaced = format!("{}:{}", dv_namespace, config.dataview);
        let method = config.method.as_deref().unwrap_or("GET");
        let schema = if let Some(dv_config) = executor.get_dataview_config(&namespaced) {
            let params = dv_config.parameters_for_method(method);
            project_input_schema(params)
        } else {
            serde_json::json!({"type": "object", "properties": {}})
        };

        serde_json::json!({
            "name": name,
            "description": config.description,
            "inputSchema": schema,
            "annotations": {
                "readOnlyHint": config.hints.read_only,
                "destructiveHint": config.hints.destructive,
                "idempotentHint": config.hints.idempotent,
                "openWorldHint": config.hints.open_world,
            }
        })
    }).collect();

    JsonRpcResponse::success(req.id.clone(), serde_json::json!({ "tools": tool_list }))
}

async fn handle_tools_call(
    req: &JsonRpcRequest,
    tools: &std::collections::HashMap<String, McpToolConfig>,
    ctx: &AppContext,
    dv_namespace: &str,
) -> JsonRpcResponse {
    let tool_name = match req.params.get("name").and_then(|n| n.as_str()) {
        Some(n) => n,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'name' in params"),
    };

    let tool_config = match tools.get(tool_name) {
        Some(c) => c,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), format!("Unknown tool: {}", tool_name)),
    };

    let arguments = req.params.get("arguments")
        .and_then(|a| a.as_object())
        .cloned()
        .unwrap_or_default();

    // Convert JSON arguments to QueryValue params
    let params: std::collections::HashMap<String, rivers_runtime::rivers_driver_sdk::QueryValue> =
        arguments.into_iter().map(|(k, v)| {
            let qv = crate::view_engine::pipeline::json_value_to_query_value(&v);
            (k, qv)
        }).collect();

    let method = tool_config.method.as_deref().unwrap_or("GET");
    let namespaced = format!("{}:{}", dv_namespace, tool_config.dataview);
    let trace_id = uuid::Uuid::new_v4().to_string();

    let dv_guard = ctx.dataview_executor.read().await;
    let executor = match dv_guard.as_ref() {
        Some(e) => e,
        None => return JsonRpcResponse::server_error(req.id.clone(), "DataView engine not available"),
    };

    match executor.execute(&namespaced, params, method, &trace_id, None).await {
        Ok(response) => {
            let content = serde_json::json!([{
                "type": "text",
                "text": serde_json::to_string(&response.query_result.rows).unwrap_or_default()
            }]);
            JsonRpcResponse::success(req.id.clone(), serde_json::json!({ "content": content }))
        }
        Err(e) => {
            JsonRpcResponse::server_error(req.id.clone(), format!("Tool execution failed: {}", e))
        }
    }
}

fn handle_resources_list(
    req: &JsonRpcRequest,
    resources: &std::collections::HashMap<String, McpResourceConfig>,
) -> JsonRpcResponse {
    let list: Vec<serde_json::Value> = resources.iter().map(|(name, config)| {
        serde_json::json!({
            "name": name,
            "uri": format!("rivers://app/{}", name),
            "description": config.description,
            "mimeType": config.mime_type,
        })
    }).collect();
    JsonRpcResponse::success(req.id.clone(), serde_json::json!({ "resources": list }))
}

async fn handle_resources_read(
    req: &JsonRpcRequest,
    resources: &std::collections::HashMap<String, McpResourceConfig>,
    ctx: &AppContext,
    dv_namespace: &str,
) -> JsonRpcResponse {
    let uri = match req.params.get("uri").and_then(|u| u.as_str()) {
        Some(u) => u,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'uri' in params"),
    };

    // Extract resource name from URI (rivers://app/{resource_name})
    let resource_name = uri.rsplit('/').next().unwrap_or(uri);

    let config = match resources.get(resource_name) {
        Some(c) => c,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), format!("Unknown resource: {}", resource_name)),
    };

    let namespaced = format!("{}:{}", dv_namespace, config.dataview);
    let trace_id = uuid::Uuid::new_v4().to_string();

    let dv_guard = ctx.dataview_executor.read().await;
    let executor = match dv_guard.as_ref() {
        Some(e) => e,
        None => return JsonRpcResponse::server_error(req.id.clone(), "DataView engine not available"),
    };

    match executor.execute(&namespaced, std::collections::HashMap::new(), "GET", &trace_id, None).await {
        Ok(response) => {
            let text = serde_json::to_string(&response.query_result.rows).unwrap_or_default();
            JsonRpcResponse::success(req.id.clone(), serde_json::json!({
                "contents": [{
                    "uri": uri,
                    "mimeType": config.mime_type,
                    "text": text,
                }]
            }))
        }
        Err(e) => JsonRpcResponse::server_error(req.id.clone(), format!("Resource read failed: {}", e)),
    }
}

fn handle_resource_templates(
    req: &JsonRpcRequest,
    resources: &std::collections::HashMap<String, McpResourceConfig>,
    app_id: &str,
) -> JsonRpcResponse {
    let templates: Vec<serde_json::Value> = resources.iter().map(|(name, config)| {
        serde_json::json!({
            "uriTemplate": format!("rivers://{}/{}", app_id, name),
            "name": name,
            "description": config.description,
            "mimeType": config.mime_type,
        })
    }).collect();
    JsonRpcResponse::success(req.id.clone(), serde_json::json!({ "resourceTemplates": templates }))
}

fn handle_prompts_list(
    req: &JsonRpcRequest,
    prompts: &std::collections::HashMap<String, McpPromptConfig>,
) -> JsonRpcResponse {
    let list: Vec<serde_json::Value> = prompts.iter().map(|(name, config)| {
        let args: Vec<serde_json::Value> = config.arguments.iter().map(|a| {
            serde_json::json!({
                "name": a.name,
                "description": a.description,
                "required": a.required,
            })
        }).collect();
        serde_json::json!({
            "name": name,
            "description": config.description,
            "arguments": args,
        })
    }).collect();
    JsonRpcResponse::success(req.id.clone(), serde_json::json!({ "prompts": list }))
}

fn handle_prompts_get(
    req: &JsonRpcRequest,
    prompts: &std::collections::HashMap<String, McpPromptConfig>,
) -> JsonRpcResponse {
    let name = match req.params.get("name").and_then(|n| n.as_str()) {
        Some(n) => n,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'name'"),
    };
    let config = match prompts.get(name) {
        Some(c) => c,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), format!("Unknown prompt: {}", name)),
    };

    // Template loading and substitution will be added in Phase 2
    // For now, return the prompt metadata
    JsonRpcResponse::success(req.id.clone(), serde_json::json!({
        "description": config.description,
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": format!("(template: {})", config.template) }
        }]
    }))
}

/// Project DataView parameters into MCP JSON Schema inputSchema.
fn project_input_schema(params: &[rivers_runtime::dataview::DataViewParameterConfig]) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for p in params {
        let json_type = match p.param_type.as_str() {
            "integer" => "integer",
            "float" | "decimal" => "number",
            "boolean" => "boolean",
            "array" => "array",
            _ => "string",
        };

        let mut prop = serde_json::json!({ "type": json_type });
        if let Some(ref default) = p.default {
            prop["default"] = default.clone();
        }
        match p.param_type.as_str() {
            "uuid" => { prop["format"] = serde_json::json!("uuid"); }
            "date" => { prop["format"] = serde_json::json!("date"); }
            "email" => { prop["format"] = serde_json::json!("email"); }
            _ => {}
        }

        properties.insert(p.name.clone(), prop);
        if p.required {
            required.push(serde_json::Value::String(p.name.clone()));
        }
    }

    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}
```

- [ ] **Step 3: Create module entry point**

Create `crates/riversd/src/mcp/mod.rs`:

```rust
//! MCP (Model Context Protocol) view type — JSON-RPC dispatcher for AI tool access.
pub mod jsonrpc;
pub mod dispatch;
```

Register in `crates/riversd/src/lib.rs`:

```rust
/// MCP view type — JSON-RPC dispatcher for AI tool access.
pub mod mcp;
```

- [ ] **Step 4: Verify and commit**

Run: `cargo check -p riversd`

```bash
git commit -m "feat(mcp): add JSON-RPC types and MCP method dispatcher"
```

---

### Task 3: Wire MCP View into Request Dispatch

**Files:**
- Modify: `crates/riversd/src/server/view_dispatch.rs`

- [ ] **Step 1: Add MCP dispatch handler**

In `view_dispatch.rs`, find the `match view_type` block (around line 155). Add before the default `_` arm:

```rust
    "Mcp" => {
        return execute_mcp_view(ctx, request, matched).await;
    }
```

- [ ] **Step 2: Implement `execute_mcp_view`**

Add the function in the same file or in a new file:

```rust
async fn execute_mcp_view(
    ctx: AppContext,
    request: axum::http::Request<axum::body::Body>,
    matched: MatchedRoute,
) -> axum::response::Response {
    // Read POST body as JSON
    let bytes = match axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            let resp = crate::mcp::jsonrpc::JsonRpcResponse::parse_error();
            return axum::Json(resp).into_response();
        }
    };

    // Parse JSON-RPC request (single or batch)
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            let resp = crate::mcp::jsonrpc::JsonRpcResponse::parse_error();
            return axum::Json(resp).into_response();
        }
    };

    let tools = &matched.config.tools;
    let resources = &matched.config.resources;
    let prompts = &matched.config.prompts;
    let app_id = &matched.app_entry_point;
    let dv_namespace = &matched.app_entry_point;

    // Handle batch or single request
    if let Some(batch) = body.as_array() {
        // Batch request
        let mut responses = Vec::new();
        for item in batch {
            if let Ok(req) = serde_json::from_value::<crate::mcp::jsonrpc::JsonRpcRequest>(item.clone()) {
                if req.id.is_some() {
                    let resp = crate::mcp::dispatch::dispatch(&ctx, &req, tools, resources, prompts, app_id, dv_namespace).await;
                    responses.push(serde_json::to_value(&resp).unwrap_or_default());
                }
                // Notifications (no id) don't produce responses
            } else {
                responses.push(serde_json::to_value(&crate::mcp::jsonrpc::JsonRpcResponse::invalid_request(None)).unwrap_or_default());
            }
        }
        axum::Json(serde_json::Value::Array(responses)).into_response()
    } else {
        // Single request
        match serde_json::from_value::<crate::mcp::jsonrpc::JsonRpcRequest>(body) {
            Ok(req) => {
                if req.id.is_none() {
                    // Notification — no response
                    return axum::http::StatusCode::NO_CONTENT.into_response();
                }
                let resp = crate::mcp::dispatch::dispatch(&ctx, &req, tools, resources, prompts, app_id, dv_namespace).await;
                axum::Json(resp).into_response()
            }
            Err(_) => {
                axum::Json(crate::mcp::jsonrpc::JsonRpcResponse::invalid_request(None)).into_response()
            }
        }
    }
}
```

- [ ] **Step 3: Ensure MCP views route as POST**

Check `crates/riversd/src/view_engine/router.rs` — MCP views need to be registered as POST routes. The existing router may default to the method from the view config. Ensure `method = "POST"` works or add special handling for MCP.

- [ ] **Step 4: Verify and commit**

Run: `cargo check -p riversd && cargo test -p riversd --lib`

```bash
git commit -m "feat(mcp): wire MCP view type into request dispatch — tools/list and tools/call working"
```

---

## Phase 2: Full Protocol — Resources, Prompts, Instructions

---

### Task 4: Prompt Template Loading and Substitution

**Files:**
- Modify: `crates/riversd/src/mcp/dispatch.rs`

- [ ] **Step 1: Implement prompt template loading**

Replace the stub `handle_prompts_get` with real template loading:

1. Read the template file from `app_dir.join(&config.template)`
2. Validate required arguments are present in the request
3. Apply defaults for missing optional arguments
4. Substitute `{argument}` placeholders with values
5. Return resolved markdown as prompt content

The `app_dir` needs to be passed through to the dispatch function — add it as a parameter.

- [ ] **Step 2: Verify and commit**

```bash
git commit -m "feat(mcp): implement prompt template loading and argument substitution"
```

---

### Task 5: Instructions Compilation

**Files:**
- Create: `crates/riversd/src/mcp/instructions.rs`
- Modify: `crates/riversd/src/mcp/mod.rs`
- Modify: `crates/riversd/src/mcp/dispatch.rs`

- [ ] **Step 1: Implement instructions compiler**

Create a function that assembles the instructions document from:
1. Static instructions file (if declared) — read at startup
2. Auto-generated tool catalog (always present)
3. Auto-generated resource reference
4. Auto-generated prompt reference

Store the compiled result in memory (recompile on hot reload).

- [ ] **Step 2: Serve via `initialize` response and GET endpoint**

Add the compiled instructions to the `initialize` handler response. Add a GET handler for `/mcp/instructions` that returns the same content as `text/markdown`.

- [ ] **Step 3: Verify and commit**

```bash
git commit -m "feat(mcp): auto-generated instructions with tool/resource/prompt catalog"
```

---

### Task 6: Validation Rules (VAL-1 through VAL-10)

**Files:**
- Modify: `crates/rivers-runtime/src/validate_crossref.rs`

- [ ] **Step 1: Add MCP config validation**

In the per-app loop of `validate_crossref()`, add checks for:
- VAL-1/VAL-2: Tool/resource DataView references exist
- VAL-3: Tool method matches DataView's declared methods
- VAL-4: Instructions file exists
- VAL-5: Prompt template files exist
- VAL-6: Prompt template placeholders match argument declarations
- VAL-7: Only one MCP view per app
- VAL-8/9/10: Unique tool/resource/prompt names

- [ ] **Step 2: Verify and commit**

```bash
git commit -m "feat(mcp): add VAL-1 through VAL-10 MCP config validation"
```

---

## Phase 3: Production Hardening — Sessions, Streaming, Tests

---

### Task 7: MCP Session Management

**Files:**
- Create: `crates/riversd/src/mcp/session.rs`
- Modify: `crates/riversd/src/mcp/dispatch.rs`

- [ ] **Step 1: Implement MCP session lifecycle**

1. On `initialize`: create session in StorageEngine at `mcp:session:{uuid}`, return `Mcp-Session-Id` header
2. On subsequent requests: validate `Mcp-Session-Id` header, reject with `-32001` if invalid/expired
3. Sliding expiration: reset TTL on every valid request
4. Session data: `created_at`, `capabilities`, `identity` (from guard)

- [ ] **Step 2: Wire session into dispatch**

Add session validation before method dispatch (except for `initialize` which creates the session).

- [ ] **Step 3: Verify and commit**

```bash
git commit -m "feat(mcp): MCP session management with StorageEngine persistence"
```

---

### Task 8: Streaming Tools

**Files:**
- Modify: `crates/riversd/src/mcp/dispatch.rs`
- Modify: `crates/riversd/src/server/view_dispatch.rs`

- [ ] **Step 1: Detect streaming DataViews in tools/call**

When `tools/call` targets a DataView with `streaming = true`, switch the response to SSE mode:
1. Set `Content-Type: text/event-stream`
2. Wrap each chunk in a `notifications/tools/progress` JSON-RPC notification
3. Send the final result as a JSON-RPC response event

- [ ] **Step 2: Verify and commit**

```bash
git commit -m "feat(mcp): streaming tool responses via SSE for streaming DataViews"
```

---

### Task 9: Canary Tests + Documentation

**Files:**
- Create: `docs/guide/tutorials/tutorial-mcp.md`
- Modify: `canary-bundle/` (add MCP test app)

- [ ] **Step 1: Write MCP tutorial**

Cover: declaring an MCP view, exposing tools, adding descriptions, testing with curl/MCP client.

- [ ] **Step 2: Add MCP canary test profile**

Create a minimal MCP app in the canary bundle that exposes a faker DataView as an MCP tool. Test `initialize`, `tools/list`, `tools/call` in run-tests.sh.

- [ ] **Step 3: Verify and commit**

```bash
git commit -m "docs: add MCP tutorial and canary tests"
```

---

### Task 10: Final Validation

- [ ] **Step 1: Full workspace compile**

Run: `cargo check --workspace`

- [ ] **Step 2: Run all tests**

Run: `cargo test -p riversd --lib && cargo test -p rivers-runtime --lib`

- [ ] **Step 3: Validate bundles**

Run: `cargo run -p riverpackage -- validate address-book-bundle && cargo run -p riverpackage -- validate canary-bundle`

- [ ] **Step 4: Manual MCP test**

Start riversd with an MCP-enabled bundle and test with curl:

```bash
# Initialize
curl -X POST http://localhost:8080/mcp -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'

# List tools
curl -X POST http://localhost:8080/mcp -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'

# Call a tool
curl -X POST http://localhost:8080/mcp -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_contacts","arguments":{}}}'
```
