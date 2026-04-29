//! MCP JSON-RPC method dispatcher.

use std::collections::HashMap;

use std::sync::Arc;

use rivers_runtime::view::{McpToolConfig, McpResourceConfig, McpPromptConfig};
use rivers_runtime::rivers_driver_sdk::QueryValue;
use rivers_runtime::DataViewExecutor;

use super::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::server::AppContext;

/// Dispatch a JSON-RPC request to the appropriate MCP handler.
pub async fn dispatch(
    ctx: &AppContext,
    req: &JsonRpcRequest,
    tools: &HashMap<String, McpToolConfig>,
    resources: &HashMap<String, McpResourceConfig>,
    prompts: &HashMap<String, McpPromptConfig>,
    app_id: &str,
    dv_namespace: &str,
    app_dir: &std::path::Path,
    instructions: Option<&str>,
) -> JsonRpcResponse {
    if req.jsonrpc != "2.0" {
        return JsonRpcResponse::invalid_request(req.id.clone());
    }

    match req.method.as_str() {
        "initialize" => handle_initialize(req, tools, resources, prompts, ctx, app_dir, instructions, dv_namespace).await,
        "ping" => JsonRpcResponse::success(req.id.clone(), serde_json::json!({})),
        "tools/list" => handle_tools_list(req, tools, ctx, dv_namespace).await,
        "tools/call" => handle_tools_call(req, tools, ctx, app_id, dv_namespace).await,
        "resources/list" => handle_resources_list(req, resources),
        "resources/read" => handle_resources_read(req, resources, ctx, dv_namespace).await,
        "resources/templates/list" => handle_resource_templates(req, resources, app_id),
        "prompts/list" => handle_prompts_list(req, prompts),
        "prompts/get" => handle_prompts_get(req, prompts, app_dir),
        _ => JsonRpcResponse::method_not_found(req.id.clone(), &req.method),
    }
}

async fn handle_initialize(
    req: &JsonRpcRequest,
    tools: &HashMap<String, McpToolConfig>,
    resources: &HashMap<String, McpResourceConfig>,
    prompts: &HashMap<String, McpPromptConfig>,
    ctx: &AppContext,
    app_dir: &std::path::Path,
    static_instructions: Option<&str>,
    dv_namespace: &str,
) -> JsonRpcResponse {
    let mut capabilities = serde_json::json!({
        "tools": { "listChanged": false },
    });
    if !resources.is_empty() {
        capabilities["resources"] = serde_json::json!({});
    }
    if !prompts.is_empty() {
        capabilities["prompts"] = serde_json::json!({});
    }

    // Compile instructions: static file + auto-generated catalog
    let dv_guard = ctx.dataview_executor.read().await;
    let executor_opt = dv_guard.as_ref();
    let ns = dv_namespace.to_string();
    let get_params = |dv_name: &str, method: &str| -> Vec<rivers_runtime::DataViewParameterConfig> {
        let namespaced = format!("{}:{}", ns, dv_name);
        executor_opt
            .and_then(|e| e.get_dataview_config(&namespaced))
            .map(|cfg| cfg.parameters_for_method(method).to_vec())
            .unwrap_or_default()
    };

    let instructions_doc = crate::mcp::instructions::compile_instructions(
        static_instructions,
        app_dir,
        tools,
        resources,
        prompts,
        &get_params,
    );

    let mut result = serde_json::json!({
        "protocolVersion": "2024-11-05",
        "serverInfo": {
            "name": "rivers-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": capabilities,
    });

    if !instructions_doc.is_empty() {
        result["instructions"] = serde_json::Value::String(instructions_doc);
    }

    JsonRpcResponse::success(req.id.clone(), result)
}

async fn handle_tools_list(
    req: &JsonRpcRequest,
    tools: &HashMap<String, McpToolConfig>,
    ctx: &AppContext,
    dv_namespace: &str,
) -> JsonRpcResponse {
    let dv_guard = ctx.dataview_executor.read().await;
    let executor_opt = dv_guard.as_ref();

    let tool_list: Vec<serde_json::Value> = tools.iter().map(|(name, config)| {
        // CB-P0.1: codecomponent-backed tools use an open schema for now;
        // CB-P0.2 will derive a precise schema from the TypeScript handler signature.
        let schema = if config.view.is_some() {
            serde_json::json!({"type": "object", "properties": {}})
        } else if let Some(executor) = executor_opt {
            let namespaced = format!("{}:{}", dv_namespace, config.dataview);
            let method = config.method.as_deref().unwrap_or("GET");
            if let Some(dv_config) = executor.get_dataview_config(&namespaced) {
                let params = dv_config.parameters_for_method(method);
                project_input_schema(params)
            } else {
                serde_json::json!({"type": "object", "properties": {}})
            }
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
    tools: &HashMap<String, McpToolConfig>,
    ctx: &AppContext,
    app_id: &str,
    dv_namespace: &str,
) -> JsonRpcResponse {
    let tool_name = match req.params.get("name").and_then(|n| n.as_str()) {
        Some(n) => n,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'name' in params"),
    };

    let tool_config = match tools.get(tool_name) {
        Some(c) => c,
        None => return JsonRpcResponse::invalid_params(
            req.id.clone(),
            format!("Unknown tool: {}", tool_name),
        ),
    };

    let arguments = req.params.get("arguments")
        .and_then(|a| a.as_object())
        .cloned()
        .unwrap_or_default();

    // CB-P0.1: codecomponent-backed tools dispatch through the ProcessPool.
    if let Some(ref view_name) = tool_config.view {
        return dispatch_codecomponent_tool(req, ctx, app_id, dv_namespace, view_name, arguments).await;
    }

    let params: HashMap<String, QueryValue> = arguments.into_iter().map(|(k, v)| {
        let qv = crate::view_engine::json_value_to_query_value(&v);
        (k, qv)
    }).collect();

    let method = tool_config.method.as_deref().unwrap_or("GET");
    let namespaced = format!("{}:{}", dv_namespace, tool_config.dataview);
    let trace_id = uuid::Uuid::new_v4().to_string();

    let dv_guard = ctx.dataview_executor.read().await;
    let executor: &Arc<DataViewExecutor> = match dv_guard.as_ref() {
        Some(e) => e,
        None => return JsonRpcResponse::server_error(req.id.clone(), "DataView engine not available"),
    };

    // Check if DataView supports streaming.
    // TODO: when the streaming DataView execution path supports it, switch to SSE response here.
    let is_streaming = executor.get_dataview_config(&namespaced)
        .map(|dv| dv.streaming)
        .unwrap_or(false);

    if is_streaming {
        tracing::debug!(tool = %tool_name, "streaming DataView detected — executing synchronously (SSE streaming deferred)");
    }

    match executor.execute(&namespaced, params, method, &trace_id, None).await {
        Ok(response) => {
            let text = serde_json::to_string(&response.query_result.rows).unwrap_or_default();
            JsonRpcResponse::success(req.id.clone(), serde_json::json!({
                "content": [{ "type": "text", "text": text }]
            }))
        }
        Err(e) => {
            let msg = e.to_string();
            // Map DataView errors to JSON-RPC codes
            if msg.contains("not found") || msg.contains("NotFound") {
                JsonRpcResponse::invalid_params(req.id.clone(), format!("Unknown tool: {}", tool_name))
            } else if msg.contains("Missing") || msg.contains("validation") {
                JsonRpcResponse::invalid_params(req.id.clone(), msg)
            } else {
                JsonRpcResponse::server_error(req.id.clone(), msg)
            }
        }
    }
}

/// Dispatch an MCP tool call to a codecomponent view's handler via the ProcessPool.
///
/// CB-P0.1: Locates the codecomponent entrypoint from the loaded bundle, builds a
/// TaskContext with the tool arguments as the handler's args, and dispatches it
/// through the default process pool.  The handler receives its full capabilities
/// (storage, driver_factory, dataview_executor, lockbox, keystore) via
/// task_enrichment::enrich — identical to REST, WebSocket, and SSE handlers.
async fn dispatch_codecomponent_tool(
    req: &JsonRpcRequest,
    ctx: &AppContext,
    app_id: &str,
    dv_namespace: &str,
    view_name: &str,
    arguments: serde_json::Map<String, serde_json::Value>,
) -> JsonRpcResponse {
    use crate::process_pool::{Entrypoint, TaskContextBuilder, TaskKind};
    use rivers_runtime::view::HandlerConfig;

    // Locate the app in the loaded bundle by dv_namespace (entry_point slug).
    let entrypoint = {
        let bundle = match ctx.loaded_bundle.as_ref() {
            Some(b) => b,
            None => return JsonRpcResponse::server_error(req.id.clone(), "no bundle loaded"),
        };
        let app = bundle.apps.iter().find(|a| {
            a.manifest.entry_point.as_deref() == Some(dv_namespace)
                || a.manifest.app_entry_point.as_deref() == Some(dv_namespace)
        });
        let app = match app {
            Some(a) => a,
            None => return JsonRpcResponse::server_error(
                req.id.clone(),
                format!("app '{}' not found in bundle", dv_namespace),
            ),
        };
        let view_config = match app.config.api.views.get(view_name) {
            Some(v) => v,
            None => return JsonRpcResponse::server_error(
                req.id.clone(),
                format!("view '{}' not found", view_name),
            ),
        };
        match &view_config.handler {
            HandlerConfig::Codecomponent { language, module, entrypoint, .. } => Entrypoint {
                language: language.clone(),
                module: module.clone(),
                function: entrypoint.clone(),
            },
            _ => return JsonRpcResponse::server_error(
                req.id.clone(),
                format!("view '{}' is not a codecomponent handler", view_name),
            ),
        }
    };

    let trace_id = uuid::Uuid::new_v4().to_string();
    let args = serde_json::Value::Object(arguments);

    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id(trace_id.clone());
    let builder = crate::task_enrichment::enrich(builder, app_id, TaskKind::Rest);
    let task_ctx = match builder.build() {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::server_error(
            req.id.clone(),
            format!("task context build failed: {e}"),
        ),
    };

    match ctx.pool.dispatch("default", task_ctx).await {
        Ok(result) => {
            let text = serde_json::to_string(&result.value).unwrap_or_default();
            JsonRpcResponse::success(req.id.clone(), serde_json::json!({
                "content": [{ "type": "text", "text": text }]
            }))
        }
        Err(e) => JsonRpcResponse::server_error(
            req.id.clone(),
            format!("handler error: {e}"),
        ),
    }
}

fn handle_resources_list(
    req: &JsonRpcRequest,
    resources: &HashMap<String, McpResourceConfig>,
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
    resources: &HashMap<String, McpResourceConfig>,
    ctx: &AppContext,
    dv_namespace: &str,
) -> JsonRpcResponse {
    let uri = match req.params.get("uri").and_then(|u| u.as_str()) {
        Some(u) => u,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'uri' in params"),
    };

    let resource_name = uri.rsplit('/').next().unwrap_or(uri);
    let config = match resources.get(resource_name) {
        Some(c) => c,
        None => return JsonRpcResponse::invalid_params(
            req.id.clone(),
            format!("Unknown resource: {}", resource_name),
        ),
    };

    let namespaced = format!("{}:{}", dv_namespace, config.dataview);
    let trace_id = uuid::Uuid::new_v4().to_string();

    let dv_guard = ctx.dataview_executor.read().await;
    let executor: &Arc<DataViewExecutor> = match dv_guard.as_ref() {
        Some(e) => e,
        None => return JsonRpcResponse::server_error(req.id.clone(), "DataView engine not available"),
    };

    match executor.execute(&namespaced, HashMap::new(), "GET", &trace_id, None).await {
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
        Err(e) => JsonRpcResponse::server_error(req.id.clone(), e.to_string()),
    }
}

fn handle_resource_templates(
    req: &JsonRpcRequest,
    resources: &HashMap<String, McpResourceConfig>,
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
    prompts: &HashMap<String, McpPromptConfig>,
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
    prompts: &HashMap<String, McpPromptConfig>,
    app_dir: &std::path::Path,
) -> JsonRpcResponse {
    let name = match req.params.get("name").and_then(|n| n.as_str()) {
        Some(n) => n,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'name'"),
    };
    let config = match prompts.get(name) {
        Some(c) => c,
        None => return JsonRpcResponse::invalid_params(
            req.id.clone(),
            format!("Unknown prompt: {}", name),
        ),
    };

    // Load template file
    let template_path = app_dir.join(&config.template);
    let template = match std::fs::read_to_string(&template_path) {
        Ok(t) => t,
        Err(e) => return JsonRpcResponse::server_error(
            req.id.clone(),
            format!("Failed to read template '{}': {}", config.template, e),
        ),
    };

    // Get arguments from request
    let req_args = req.params.get("arguments")
        .and_then(|a| a.as_object())
        .cloned()
        .unwrap_or_default();

    // Validate required arguments
    for arg_def in &config.arguments {
        if arg_def.required && !req_args.contains_key(&arg_def.name) {
            return JsonRpcResponse::invalid_params(
                req.id.clone(),
                format!("Missing required prompt argument: {}", arg_def.name),
            );
        }
    }

    // Build substitution map (request values + defaults)
    let mut subs: HashMap<String, String> = HashMap::new();
    for arg_def in &config.arguments {
        if let Some(val) = req_args.get(&arg_def.name).and_then(|v| v.as_str()) {
            subs.insert(arg_def.name.clone(), val.to_string());
        } else if let Some(ref default) = arg_def.default {
            subs.insert(arg_def.name.clone(), default.clone());
        }
    }

    // Substitute {placeholder} in template
    let mut resolved = template;
    for (key, value) in &subs {
        resolved = resolved.replace(&format!("{{{}}}", key), value);
    }

    JsonRpcResponse::success(req.id.clone(), serde_json::json!({
        "description": config.description,
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": resolved }
        }]
    }))
}

/// Project DataView parameters into MCP JSON Schema inputSchema.
pub fn project_input_schema(params: &[rivers_runtime::DataViewParameterConfig]) -> serde_json::Value {
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
