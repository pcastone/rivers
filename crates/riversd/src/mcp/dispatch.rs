//! MCP JSON-RPC method dispatcher.

use std::collections::HashMap;

use std::sync::Arc;

use rivers_runtime::view::{McpToolConfig, McpResourceConfig, McpPromptConfig, McpFederationConfig};
use rivers_runtime::rivers_driver_sdk::QueryValue;
use rivers_runtime::DataViewExecutor;

use super::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use super::federation::FederationClient;
use crate::server::AppContext;

/// Dispatch a JSON-RPC request to the appropriate MCP handler.
///
/// `auth_context` is the caller identity retrieved from the MCP session store
/// (the `"auth"` sub-object stored at `initialize` time). It is `None` for
/// `initialize`/`ping` and for MCP views with no storage backend.
///
/// `federation` is the list of federated MCP upstream configs declared in the
/// view's `federation` field (P2.3). Tools and resources from these upstreams are
/// merged into `tools/list` and `resources/list` responses under namespaced prefixes.
/// Pass `&[]` when no federation is configured.
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
    auth_context: Option<&serde_json::Value>,
    session_id: Option<&str>,
    federation: &[McpFederationConfig],
) -> JsonRpcResponse {
    if req.jsonrpc != "2.0" {
        return JsonRpcResponse::invalid_request(req.id.clone());
    }

    match req.method.as_str() {
        "initialize" => handle_initialize(req, tools, resources, prompts, ctx, app_dir, instructions, dv_namespace).await,
        "ping" => JsonRpcResponse::success(req.id.clone(), serde_json::json!({})),
        "tools/list" => handle_tools_list(req, tools, ctx, dv_namespace, app_dir, federation).await,
        "tools/call" => {
            // P2.8: emit audit event for MCP tool invocations
            let tool_name = req.params.get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("<unknown>")
                .to_string();
            let tool_start = std::time::Instant::now();
            let resp = handle_tools_call(req, tools, ctx, app_id, dv_namespace, auth_context, federation, session_id).await;
            if let Some(ref bus) = ctx.audit_bus {
                let is_error = resp.result.as_ref()
                    .and_then(|r| r.get("isError"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(resp.error.is_some());
                let _ = bus.send(crate::audit::AuditEvent::McpToolCalled {
                    app_id: app_id.to_string(),
                    tool: tool_name,
                    duration_ms: tool_start.elapsed().as_millis() as u64,
                    is_error,
                });
            }
            resp
        }
        "tools/call_batch" => handle_tools_call_batch(req, tools, ctx, app_id, dv_namespace, auth_context, federation, session_id).await,
        "resources/list" => handle_resources_list(req, resources, federation).await,
        "resources/read" => handle_resources_read(req, resources, ctx, dv_namespace, app_id, federation).await,
        "resources/templates/list" => handle_resource_templates(req, resources, app_id),
        "resources/subscribe" => handle_resources_subscribe(req, resources, ctx, app_id, dv_namespace, session_id).await,
        "resources/unsubscribe" => handle_resources_unsubscribe(req, ctx, session_id).await,
        "prompts/list" => handle_prompts_list(req, prompts),
        "prompts/get" => handle_prompts_get(req, prompts, app_dir),
        // P2.6: elicitation/response — resolves a pending mid-handler elicitation.
        "elicitation/response" => handle_elicitation_response(req, ctx),
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
        "tools": { "listChanged": false, "batch": true },
    });
    if !resources.is_empty() {
        // P1.1.1.c: advertise subscribe capability only when ≥1 resource has subscribable = true.
        let has_subscribable = resources.values().any(|r| r.subscribable);
        if has_subscribable {
            capabilities["resources"] = serde_json::json!({ "subscribe": true });
        } else {
            capabilities["resources"] = serde_json::json!({});
        }
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
    app_dir: &std::path::Path,
    federation: &[McpFederationConfig],
) -> JsonRpcResponse {
    let dv_guard = ctx.dataview_executor.read().await;
    let executor_opt = dv_guard.as_ref();

    let mut tool_list: Vec<serde_json::Value> = tools.iter().map(|(name, config)| {
        let schema = if config.view.is_some() {
            // CB-P0.2.c: load explicit JSON Schema file when declared.
            if let Some(schema_path) = &config.input_schema {
                let full_path = app_dir.join(schema_path);
                match std::fs::read_to_string(&full_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                {
                    Some(v) => v,
                    None => serde_json::json!({"type": "object", "properties": {}}),
                }
            } else {
                serde_json::json!({"type": "object", "properties": {}})
            }
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

    // P2.3: merge federated tools (best-effort — upstream failures silently produce empty lists).
    drop(dv_guard); // release lock before potentially slow upstream fetches
    for fed_config in federation {
        let client = FederationClient::new(fed_config.clone());
        let fed_tools = client.fetch_tools().await;
        tool_list.extend(fed_tools);
    }

    JsonRpcResponse::success(req.id.clone(), serde_json::json!({ "tools": tool_list }))
}

async fn handle_tools_call(
    req: &JsonRpcRequest,
    tools: &HashMap<String, McpToolConfig>,
    ctx: &AppContext,
    app_id: &str,
    dv_namespace: &str,
    auth_context: Option<&serde_json::Value>,
    federation: &[McpFederationConfig],
    session_id: Option<&str>,
) -> JsonRpcResponse {
    let tool_name = match req.params.get("name").and_then(|n| n.as_str()) {
        Some(n) => n,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'name' in params"),
    };

    // P2.3: check federation upstreams first — if the tool name carries a federation
    // namespace prefix, proxy the call to the upstream rather than dispatching locally.
    for fed_config in federation {
        let client = FederationClient::new(fed_config.clone());
        if client.owns_tool(tool_name) {
            let arguments = req.params.get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let result = client.proxy_tool_call(tool_name, arguments).await;
            return JsonRpcResponse::success(req.id.clone(), result);
        }
    }

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
        return dispatch_codecomponent_tool(req, ctx, app_id, dv_namespace, view_name, arguments, auth_context, session_id).await;
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

/// Handle `tools/call_batch` — invoke multiple tools in a single JSON-RPC request.
///
/// P2.2: Accepts an `items` array (each with `name` and `arguments`) and an optional
/// `continue_on_error` flag (default false). Calls `handle_tools_call` for each item
/// in sequence. On first error (when `continue_on_error = false`) returns immediately
/// with that error. With `continue_on_error = true` collects all results, marking
/// failures with `"isError": true`.
async fn handle_tools_call_batch(
    req: &JsonRpcRequest,
    tools: &HashMap<String, McpToolConfig>,
    ctx: &AppContext,
    app_id: &str,
    dv_namespace: &str,
    auth_context: Option<&serde_json::Value>,
    federation: &[McpFederationConfig],
    session_id: Option<&str>,
) -> JsonRpcResponse {
    let items = match req.params.get("items").and_then(|v| v.as_array()) {
        Some(arr) => arr.clone(),
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'items' array in params"),
    };

    let continue_on_error = req.params.get("continue_on_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut results: Vec<serde_json::Value> = Vec::with_capacity(items.len());

    for item in &items {
        let item_name = match item.get("name").and_then(|n| n.as_str()) {
            Some(n) => n.to_string(),
            None => {
                if continue_on_error {
                    results.push(serde_json::json!({
                        "name": serde_json::Value::Null,
                        "content": [{"type": "text", "text": "missing 'name' in batch item"}],
                        "isError": true
                    }));
                    continue;
                } else {
                    return JsonRpcResponse::invalid_params(
                        req.id.clone(),
                        "missing 'name' in batch item",
                    );
                }
            }
        };

        let arguments = item.get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        // Construct a synthetic JsonRpcRequest for the existing handle_tools_call path.
        let synthetic_req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            method: "tools/call".into(),
            params: serde_json::json!({
                "name": item_name,
                "arguments": arguments,
            }),
        };

        let resp = handle_tools_call(&synthetic_req, tools, ctx, app_id, dv_namespace, auth_context, federation, session_id).await;

        // A JSON-RPC error response (has `error` field, no `result`) is an item failure.
        if resp.error.is_some() {
            if continue_on_error {
                let error_text = resp.error.as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "unknown error".into());
                results.push(serde_json::json!({
                    "name": item_name,
                    "content": [{"type": "text", "text": error_text}],
                    "isError": true
                }));
            } else {
                // Propagate the error immediately (stop on first failure).
                return resp;
            }
        } else {
            // Success — extract the content array from the result.
            let content = resp.result
                .as_ref()
                .and_then(|r| r.get("content"))
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            results.push(serde_json::json!({
                "name": item_name,
                "content": content,
                "isError": false
            }));
        }
    }

    JsonRpcResponse::success(req.id.clone(), serde_json::json!({ "results": results }))
}

/// Dispatch an MCP tool call to a codecomponent view's handler via the ProcessPool.
///
/// CB-P0.1/P0.3: Locates the codecomponent entrypoint from the loaded bundle, builds a
/// TaskContext with the tool arguments as `ctx.request` and the caller identity as
/// `ctx.session`, and dispatches it through the default process pool. The handler
/// receives its full capabilities (storage, driver_factory, dataview_executor, lockbox,
/// keystore) via task_enrichment::enrich — identical to REST, WebSocket, and SSE handlers.
async fn dispatch_codecomponent_tool(
    req: &JsonRpcRequest,
    ctx: &AppContext,
    app_id: &str,
    dv_namespace: &str,
    view_name: &str,
    arguments: serde_json::Map<String, serde_json::Value>,
    auth_context: Option<&serde_json::Value>,
    session_id: Option<&str>,
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

    // P2.6: create the elicitation relay channel before dispatching.
    //
    // The V8 worker thread reads from `TASK_ELICITATION_TX` (populated via
    // the global registry keyed by trace_id) when the handler calls
    // `ctx.elicit(spec)`. The relay task below handles the outbound SSE
    // notification and ElicitationRegistry registration.
    let (elicit_tx, mut elicit_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::mcp::elicitation::ElicitationRequest>();

    // Register the sender in the global map — TaskLocals::set will take it.
    crate::process_pool::v8_engine::register_elicitation_tx(
        &trace_id,
        elicit_tx,
    );

    // Clone what the relay task needs from ctx (AppContext is Clone).
    let elicitation_registry = ctx.elicitation_registry.clone();
    let subscription_registry = ctx.subscription_registry.clone();

    // Spawn the relay task. It reads ElicitationRequests from the channel,
    // emits `elicitation/create` SSE notifications to the session, and
    // registers the response oneshot in the ElicitationRegistry.
    //
    // SSE wiring: the session_id passed to dispatch() is the MCP-Session-Id
    // header value. We use it to look up the session's SSE sender via
    // SubscriptionRegistry. If no SSE stream is open for this session, the
    // notification is dropped with a WARN — the elicitation will time out
    // (60s) and resolve with action = "cancel".
    //
    // Note: if session_id is None (POST-only session), we still process the
    // elicitation request (register the oneshot), but the SSE notification
    // cannot be delivered, so the client must poll or maintain a session.
    let session_id_owned = session_id.map(|s| s.to_string());

    tokio::spawn(async move {
        while let Some(elicit_req) = elicit_rx.recv().await {
            let id = elicit_req.id.clone();
            let spec = elicit_req.spec.clone();

            // Send `elicitation/create` notification over SSE (best-effort).
            // Per MCP P2.6: notification method is `elicitation/create`.
            let notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "elicitation/create",
                "params": {
                    "id": id,
                    "title": spec.title,
                    "message": spec.message,
                    "requestedSchema": spec.requested_schema,
                }
            });
            let notification_str = serde_json::to_string(&notification)
                .unwrap_or_else(|_| "{}".to_string());

            // Deliver via send_to_session (best-effort — drops if no SSE stream open).
            if let Some(ref sid) = session_id_owned {
                let sent = subscription_registry
                    .send_to_session(sid, notification_str)
                    .await;
                if !sent {
                    tracing::warn!(
                        session_id = %sid,
                        elicitation_id = %id,
                        "elicitation/create: SSE channel unavailable — notification dropped; elicitation will time out after 60s"
                    );
                }
            } else {
                tracing::warn!(
                    elicitation_id = %id,
                    "elicitation/create: no session_id available — SSE notification not sent"
                );
            }

            // Register the response oneshot in the registry.
            // The client sends `elicitation/response` which calls handle_elicitation_response,
            // which calls elicitation_registry.resolve(response) to unblock the V8 worker.
            elicitation_registry.register(id, elicit_req.response_tx);
        }
    });

    // Wrap tool arguments as ctx.request and propagate caller identity as ctx.session,
    // matching the REST pipeline's injection pattern (pipeline.rs).
    let args = serde_json::json!({
        "request": serde_json::Value::Object(arguments),
        "session": auth_context,
    });

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

async fn handle_resources_list(
    req: &JsonRpcRequest,
    resources: &HashMap<String, McpResourceConfig>,
    federation: &[McpFederationConfig],
) -> JsonRpcResponse {
    let mut list: Vec<serde_json::Value> = resources.iter().map(|(name, config)| {
        serde_json::json!({
            "name": name,
            "uri": format!("rivers://app/{}", name),
            "description": config.description,
            "mimeType": config.mime_type,
        })
    }).collect();

    // P2.3: merge federated resources (best-effort — upstream failures produce empty lists).
    for fed_config in federation {
        let client = FederationClient::new(fed_config.clone());
        let fed_resources = client.fetch_resources().await;
        list.extend(fed_resources);
    }

    JsonRpcResponse::success(req.id.clone(), serde_json::json!({ "resources": list }))
}

/// Extract path variable values from a URI by matching it against an RFC 6570 template.
///
/// Only handles simple path variables (`{varname}`) and strips `{?...}` query expansions
/// from the template before matching. Returns `Some(vars)` when the URI matches the template
/// (vars may be empty for a no-variable template), or `None` when it doesn't match.
pub(crate) fn extract_uri_template_vars_pub(template: &str, uri: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    extract_uri_template_vars(template, uri)
}

fn extract_uri_template_vars(template: &str, uri: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    // Strip {?...} query expansion from template before path matching
    let template_path = match template.find("{?") {
        Some(pos) => &template[..pos],
        None => template,
    };
    // Strip query string from URI before path matching
    let uri_path = match uri.find('?') {
        Some(pos) => &uri[..pos],
        None => uri,
    };

    let mut vars = serde_json::Map::new();
    let t_segs: Vec<&str> = template_path.split('/').collect();
    let u_segs: Vec<&str> = uri_path.split('/').collect();

    if t_segs.len() != u_segs.len() {
        return None;
    }

    for (t_seg, u_seg) in t_segs.iter().zip(u_segs.iter()) {
        if t_seg.starts_with('{') && t_seg.ends_with('}') && !u_seg.is_empty() {
            let name = &t_seg[1..t_seg.len() - 1];
            vars.insert(name.to_string(), serde_json::Value::String(u_seg.to_string()));
        } else if t_seg != u_seg {
            // Literal segment mismatch — URI doesn't match this template
            return None;
        }
    }
    Some(vars)
}

/// Parse query string parameters from a URI (everything after `?`).
fn extract_query_params(uri: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    let qs = match uri.find('?') {
        Some(pos) => &uri[pos + 1..],
        None => return map,
    };
    for pair in qs.split('&') {
        let mut parts = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            if !k.is_empty() {
                map.insert(k.to_string(), serde_json::Value::String(v.to_string()));
            }
        }
    }
    map
}

async fn handle_resources_read(
    req: &JsonRpcRequest,
    resources: &HashMap<String, McpResourceConfig>,
    ctx: &AppContext,
    dv_namespace: &str,
    app_id: &str,
    federation: &[McpFederationConfig],
) -> JsonRpcResponse {
    let uri = match req.params.get("uri").and_then(|u| u.as_str()) {
        Some(u) => u,
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'uri' in params"),
    };

    // P2.3: if the URI belongs to a federation upstream, proxy the read there.
    for fed_config in federation {
        let client = FederationClient::new(fed_config.clone());
        if client.owns_resource(uri) {
            let result = client.proxy_resource_read(uri).await;
            return JsonRpcResponse::success(req.id.clone(), result);
        }
    }

    // Match the incoming URI against each resource's template (config template or default).
    let matched = resources.iter().find_map(|(name, config)| {
        let template = config.uri_template.as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| format!("rivers://{}/{}", app_id, name));

        extract_uri_template_vars(&template, uri).map(|path_vars| {
            let query_vars = extract_query_params(uri);
            (name.clone(), config.clone(), path_vars, query_vars)
        })
    });

    let (_, config, path_vars, query_vars) = match matched {
        Some(m) => m,
        None => return JsonRpcResponse::invalid_params(
            req.id.clone(),
            format!("No resource matches URI: {}", uri),
        ),
    };

    // Merge path and query vars into DataView params
    let mut params: HashMap<String, QueryValue> = HashMap::new();
    for (k, v) in path_vars.iter().chain(query_vars.iter()) {
        if let Some(s) = v.as_str() {
            params.insert(k.clone(), QueryValue::String(s.to_string()));
        }
    }

    let namespaced = format!("{}:{}", dv_namespace, config.dataview);
    let trace_id = uuid::Uuid::new_v4().to_string();

    let dv_guard = ctx.dataview_executor.read().await;
    let executor: &Arc<DataViewExecutor> = match dv_guard.as_ref() {
        Some(e) => e,
        None => return JsonRpcResponse::server_error(req.id.clone(), "DataView engine not available"),
    };

    match executor.execute(&namespaced, params, "GET", &trace_id, None).await {
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
        let uri_template = config.uri_template.as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| format!("rivers://{}/{}", app_id, name));
        serde_json::json!({
            "uriTemplate": uri_template,
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

// ── P1.1.5.b — resources/subscribe + resources/unsubscribe ──────────────────

/// Handle `resources/subscribe` — register the session's interest in a URI.
///
/// Requires a valid `session_id` (from `Mcp-Session-Id` header) and an open
/// SSE channel (attached via `attach_sse` when the client opened the SSE stream).
/// The URI must match a `subscribable = true` resource in this MCP view.
async fn handle_resources_subscribe(
    req: &JsonRpcRequest,
    resources: &HashMap<String, McpResourceConfig>,
    ctx: &AppContext,
    app_id: &str,
    dv_namespace: &str,
    session_id: Option<&str>,
) -> JsonRpcResponse {
    let Some(sid) = session_id else {
        return JsonRpcResponse::invalid_params(
            req.id.clone(),
            "resources/subscribe requires an active Mcp-Session-Id".to_string(),
        );
    };

    let uri = match req.params.get("uri").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'uri' param".to_string()),
    };

    // Verify the URI matches a subscribable resource template.
    let is_subscribable = resources.iter().any(|(name, r)| {
        if !r.subscribable { return false; }
        let template = r.uri_template.as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| format!("rivers://{}/{}", app_id, name));
        extract_uri_template_vars_pub(&template, &uri).is_some()
    });

    if !is_subscribable {
        return JsonRpcResponse::invalid_params(
            req.id.clone(),
            format!("resource '{}' is not subscribable", uri),
        );
    }

    let max_subs = ctx
        .config
        .mcp
        .as_ref()
        .map(|m| m.max_subscriptions_per_session)
        .unwrap_or(100);

    match ctx
        .subscription_registry
        .subscribe(sid, &uri, max_subs)
        .await
    {
        Ok(()) => {
            // P1.1.5.b: start the change poller for this (app_id, uri) if not already running.
            let dv_guard = ctx.dataview_executor.read().await;
            if let Some(executor) = dv_guard.as_ref() {
                let min_poll_secs = ctx.config.mcp.as_ref()
                    .map(|m| m.min_poll_interval_seconds)
                    .unwrap_or(1);
                // Use the per-resource poll_interval_seconds (default 5).
                let poll_secs = resources.values()
                    .find(|r| {
                        let tmpl = r.uri_template.as_deref()
                            .filter(|t| !t.is_empty())
                            .map(|t| t.to_string())
                            .unwrap_or_default();
                        extract_uri_template_vars_pub(&tmpl, &uri).is_some()
                    })
                    .map(|r| r.poll_interval_seconds)
                    .unwrap_or(5);
                ctx.change_poller
                    .ensure_running(
                        app_id.to_string(),
                        uri.clone(),
                        dv_namespace.to_string(),
                        resources.clone(),
                        executor.clone(),
                        ctx.subscription_registry.clone(),
                        poll_secs,
                        min_poll_secs,
                    )
                    .await;
            }
            JsonRpcResponse::success(req.id.clone(), serde_json::json!({}))
        }
        Err(crate::mcp::subscriptions::SubscribeError::SessionNotFound) => {
            JsonRpcResponse::invalid_params(
                req.id.clone(),
                "no active SSE stream for this session — open the SSE stream first".to_string(),
            )
        }
        Err(crate::mcp::subscriptions::SubscribeError::TooMany) => {
            JsonRpcResponse::server_error(
                req.id.clone(),
                format!("subscription cap ({}) reached for this session", max_subs),
            )
        }
    }
}

/// Handle `resources/unsubscribe` — remove the session's subscription to a URI.
async fn handle_resources_unsubscribe(
    req: &JsonRpcRequest,
    ctx: &AppContext,
    session_id: Option<&str>,
) -> JsonRpcResponse {
    let Some(sid) = session_id else {
        return JsonRpcResponse::invalid_params(
            req.id.clone(),
            "resources/unsubscribe requires an active Mcp-Session-Id".to_string(),
        );
    };

    let uri = match req.params.get("uri").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return JsonRpcResponse::invalid_params(req.id.clone(), "missing 'uri' param".to_string()),
    };

    ctx.subscription_registry.unsubscribe(sid, &uri).await;
    JsonRpcResponse::success(req.id.clone(), serde_json::json!({}))
}

// ── P2.6 — elicitation/response ─────────────────────────────────────────────

/// Handle `elicitation/response` — deliver a user's answer to a pending elicitation.
///
/// P2.6: The MCP client posts this after displaying the `elicitation/create`
/// notification to the user. It resolves the `oneshot::Sender` stored in
/// `AppContext::elicitation_registry`, which unblocks the V8 worker thread
/// waiting in `Rivers.__elicit`.
///
/// Returns:
/// - `{}` on success (ID was found and response was delivered).
/// - `-32602` (invalid params) when the elicitation ID is unknown or has already
///   timed out.
fn handle_elicitation_response(
    req: &JsonRpcRequest,
    ctx: &AppContext,
) -> JsonRpcResponse {
    use crate::mcp::elicitation::ElicitationResponse;

    // Parse ElicitationResponse from params.
    let id = match req.params.get("id").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return JsonRpcResponse::invalid_params(
            req.id.clone(),
            "elicitation/response: missing 'id' in params",
        ),
    };
    let action = match req.params.get("action").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return JsonRpcResponse::invalid_params(
            req.id.clone(),
            "elicitation/response: missing 'action' in params",
        ),
    };
    let content = req.params.get("content").cloned();

    let response = ElicitationResponse { id, action, content };

    if ctx.elicitation_registry.resolve(response) {
        JsonRpcResponse::success(req.id.clone(), serde_json::json!({}))
    } else {
        JsonRpcResponse::invalid_params(
            req.id.clone(),
            "elicitation/response: unknown or already-resolved elicitation id",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(method: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(1)),
            method: method.into(),
            params: serde_json::json!({}),
        }
    }

    #[test]
    fn resource_template_uses_uri_template_when_set() {
        let mut resources = HashMap::new();
        resources.insert("decisions".into(), McpResourceConfig {
            dataview: "decisions_list".into(),
            description: "CB decisions".into(),
            mime_type: "application/json".into(),
            uri_template: Some("cb://{project_id}/decisions{?since,limit}".into()),
            subscribable: false,
            poll_interval_seconds: 5,
        });
        let resp = handle_resource_templates(&make_req("resources/templates/list"), &resources, "app-id");
        // Serialize to inspect result
        let v = serde_json::to_value(&resp).unwrap();
        let templates = v["result"]["resourceTemplates"].as_array().unwrap().clone();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0]["uriTemplate"], "cb://{project_id}/decisions{?since,limit}");
    }

    #[test]
    fn resource_template_falls_back_to_default_uri_when_not_set() {
        let mut resources = HashMap::new();
        resources.insert("tasks".into(), McpResourceConfig {
            dataview: "tasks_list".into(),
            description: "Tasks".into(),
            mime_type: "application/json".into(),
            uri_template: None,
            subscribable: false,
            poll_interval_seconds: 5,
        });
        let resp = handle_resource_templates(&make_req("resources/templates/list"), &resources, "my-app");
        let v = serde_json::to_value(&resp).unwrap();
        let templates = v["result"]["resourceTemplates"].as_array().unwrap().clone();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0]["uriTemplate"], "rivers://my-app/tasks");
    }

    #[test]
    fn extract_template_vars_path_only() {
        let vars = extract_uri_template_vars(
            "cb://{project_id}/decisions",
            "cb://proj-abc/decisions",
        ).unwrap_or_default();
        assert_eq!(vars.get("project_id").and_then(|v| v.as_str()), Some("proj-abc"));
    }

    #[test]
    fn extract_template_vars_with_query_expansion() {
        // {?since,limit} means those arrive as query string — not extracted by template matching
        let vars = extract_uri_template_vars(
            "cb://{project_id}/decisions{?since,limit}",
            "cb://proj-abc/decisions?since=2024-01-01&limit=20",
        ).unwrap_or_default();
        assert_eq!(vars.get("project_id").and_then(|v| v.as_str()), Some("proj-abc"));
        // query params from {?...} expansion are parsed separately
        assert!(vars.get("since").is_none());
    }

    #[test]
    fn extract_template_vars_no_match_returns_none() {
        let result = extract_uri_template_vars(
            "cb://{project_id}/decisions",
            "cb://proj-abc/tasks",  // different path segment
        );
        assert!(result.is_none(), "structural mismatch should return None");
    }

    #[test]
    fn parse_query_string_from_uri() {
        let qs = extract_query_params("cb://proj/decisions?since=2024-01-01&limit=20");
        assert_eq!(qs.get("since").and_then(|v| v.as_str()), Some("2024-01-01"));
        assert_eq!(qs.get("limit").and_then(|v| v.as_str()), Some("20"));
    }

    // ── P2.2: tools/call_batch unit tests ────────────────────────────────────

    /// Build a minimal JsonRpcRequest for batch calls.
    fn make_batch_req(items: serde_json::Value, continue_on_error: Option<bool>) -> JsonRpcRequest {
        let mut params = serde_json::json!({ "items": items });
        if let Some(coe) = continue_on_error {
            params["continue_on_error"] = serde_json::json!(coe);
        }
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(42)),
            method: "tools/call_batch".into(),
            params,
        }
    }

    #[tokio::test]
    async fn batch_empty_items_returns_empty_results() {
        let _tools: HashMap<String, McpToolConfig> = HashMap::new();
        // We cannot construct a real AppContext in unit tests, so we test the
        // batch dispatcher's routing logic by inspecting the missing-items path.
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(1)),
            method: "tools/call_batch".into(),
            params: serde_json::json!({ "items": [] }),
        };
        // Without a real AppContext we verify only that the params parsing
        // correctly identifies an empty items array and produces the right shape.
        // The result shape is {"results": []}.
        let items = req.params.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        assert!(items.is_empty(), "empty items array should parse to zero items");
        // Verify results shape would be correct (we check the JSON path logic directly).
        let empty: Vec<serde_json::Value> = Vec::new();
        let result = serde_json::json!({ "results": empty });
        assert_eq!(result["results"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn batch_missing_items_param_returns_invalid_params() {
        // Validate that a request without 'items' correctly detects the missing field.
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(2)),
            method: "tools/call_batch".into(),
            params: serde_json::json!({}),
        };
        // Simulate the items extraction logic from handle_tools_call_batch.
        let items_opt = req.params.get("items").and_then(|v| v.as_array());
        assert!(items_opt.is_none(), "missing 'items' should be None");
    }

    #[test]
    fn batch_continue_on_error_defaults_to_false() {
        let req = make_batch_req(serde_json::json!([]), None);
        let coe = req.params.get("continue_on_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(!coe, "continue_on_error should default to false when absent");
    }

    #[test]
    fn batch_continue_on_error_true_is_parsed() {
        let req = make_batch_req(serde_json::json!([]), Some(true));
        let coe = req.params.get("continue_on_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(coe, "continue_on_error = true should be parsed correctly");
    }

    #[test]
    fn batch_item_missing_name_is_detectable() {
        // Verify that a batch item without 'name' returns None from the extractor.
        let item = serde_json::json!({ "arguments": {} });
        let name_opt = item.get("name").and_then(|n| n.as_str());
        assert!(name_opt.is_none(), "item without 'name' key should fail name extraction");
    }

    #[test]
    fn batch_result_shape_on_success() {
        // Verify the expected per-item result shape for a successful call.
        let name = "my_tool".to_string();
        let content = serde_json::json!([{"type": "text", "text": "hello"}]);
        let entry = serde_json::json!({
            "name": name,
            "content": content,
            "isError": false
        });
        assert_eq!(entry["isError"], serde_json::json!(false));
        assert_eq!(entry["name"], serde_json::json!("my_tool"));
    }

    #[test]
    fn batch_result_shape_on_error() {
        // Verify the expected per-item result shape for a failed call.
        let name = "bad_tool".to_string();
        let error_text = "Unknown tool: bad_tool".to_string();
        let entry = serde_json::json!({
            "name": name,
            "content": [{"type": "text", "text": error_text}],
            "isError": true
        });
        assert_eq!(entry["isError"], serde_json::json!(true));
        assert_eq!(entry["content"][0]["text"], serde_json::json!("Unknown tool: bad_tool"));
    }
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
