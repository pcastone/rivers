//! Streaming response helpers — SSE, Streaming REST, WebSocket handlers.

use std::collections::HashMap;

use axum::extract::ws::WebSocket;
use axum::extract::{FromRequestParts, Request};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use tokio_stream::StreamExt as _;

use crate::error_response;

use super::context::AppContext;
use super::view_dispatch::MatchedRoute;

// ── Streaming Response Helper ──────────────────────────────────────

/// Build a streaming HTTP response from an mpsc receiver of string chunks.
///
/// Used by SSE, Streaming REST, and WebSocket views to return chunked responses.
pub(super) fn build_streaming_response(
    content_type: &str,
    rx: tokio::sync::mpsc::Receiver<String>,
) -> axum::response::Response {
    use tokio_stream::wrappers::ReceiverStream;

    let stream = ReceiverStream::new(rx)
        .map(|chunk| Ok::<_, std::convert::Infallible>(chunk));
    let body = axum::body::Body::from_stream(stream);

    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", content_type)
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .body(body)
        .unwrap_or_else(|_| {
            error_response::internal_error("streaming response construction failed")
                .into_axum_response()
        })
}

// ── SSE View Handler ───────────────────────────────────────────────

/// Execute an SSE view — subscribe to the SSE channel and stream events.
///
/// Per spec §7: SSE views return `text/event-stream` with per-client push loop.
pub(super) async fn execute_sse_view(
    ctx: AppContext,
    request: Request,
    matched: MatchedRoute,
) -> axum::response::Response {
    let view_id = matched.view_id.clone();
    let trace_id = uuid::Uuid::new_v4().to_string();

    // Look up the SSE channel for this view
    let channel = match ctx.sse_manager.get(&view_id).await {
        Some(ch) => ch,
        None => {
            tracing::warn!(view_id = %view_id, "SSE channel not registered");
            return error_response::internal_error("SSE channel not configured")
                .with_trace_id(trace_id)
                .into_axum_response();
        }
    };

    // Subscribe this client
    let mut sse_rx = match channel.subscribe() {
        Ok(rx) => rx,
        Err(e) => {
            return error_response::service_unavailable(e.to_string())
                .with_trace_id(trace_id)
                .into_axum_response();
        }
    };

    // Extract Last-Event-ID for reconnection
    let last_event_id = request
        .headers()
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Create mpsc channel for streaming response
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);

    // Replay missed events from buffer if client is reconnecting with Last-Event-ID
    if let Some(ref last_id) = last_event_id {
        let missed = channel.replay_since(last_id);
        for event in missed {
            let wire = event.to_wire_format();
            if tx.send(wire).await.is_err() {
                channel.unsubscribe();
                return error_response::internal_error("client disconnected during replay")
                    .with_trace_id(trace_id)
                    .into_axum_response();
            }
        }
        tracing::debug!(view_id = %view_id, last_event_id = %last_id, "SSE replay complete");
    }

    // Extract session ID from request for revalidation
    let session_id = {
        let cookie_name = &ctx.config.security.session.cookie.name;
        let cookie_hdr = request.headers().get("cookie").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let auth_hdr = request.headers().get("authorization").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        crate::session::extract_session_id(cookie_hdr.as_deref(), auth_hdr.as_deref(), cookie_name)
    };

    // Spawn per-client relay task: broadcast receiver → mpsc sender → HTTP stream
    let channel_for_cleanup = channel.clone();
    let revalidation_interval = matched.config.session_revalidation_interval_s;
    let session_mgr = ctx.session_manager.clone();
    let view_id_clone = view_id.clone();
    tokio::spawn(async move {
        // Optional session revalidation timer
        let mut revalidation_tick = revalidation_interval.map(|secs| {
            tokio::time::interval(tokio::time::Duration::from_secs(secs))
        });
        // Skip the first immediate tick
        if let Some(ref mut tick) = revalidation_tick {
            tick.tick().await;
        }

        loop {
            tokio::select! {
                msg = sse_rx.recv() => {
                    match msg {
                        Ok(event) => {
                            let wire = event.to_wire_format();
                            if tx.send(wire).await.is_err() {
                                break; // Client disconnected
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            let comment = format!(": lagged {} events\n\n", n);
                            if tx.send(comment).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                _ = async {
                    if let Some(ref mut tick) = revalidation_tick {
                        tick.tick().await
                    } else {
                        std::future::pending::<tokio::time::Instant>().await
                    }
                } => {
                    // Session revalidation tick — validate against StorageEngine
                    if let (Some(ref mgr), Some(ref sid)) = (&session_mgr, &session_id) {
                        match mgr.validate_session(sid).await {
                            Ok(Some(_)) => {} // Session still valid
                            Ok(None) | Err(_) => {
                                tracing::info!(
                                    view_id = %view_id_clone,
                                    "SSE session expired — closing connection"
                                );
                                let _ = tx.send(": session expired\n\n".to_string()).await;
                                break;
                            }
                        }
                    }
                }
            }
        }
        channel_for_cleanup.unsubscribe();
    });

    tracing::debug!(view_id = %view_id, "SSE client connected");
    build_streaming_response("text/event-stream", rx)
}

// ── Streaming REST View Handler ────────────────────────────────────

/// Execute a streaming REST view — returns chunked NDJSON or SSE response.
///
/// Per spec: streaming views use CodeComponent handlers that produce chunks.
pub(super) async fn execute_streaming_rest_view(
    ctx: &AppContext,
    _parsed: crate::view_engine::ParsedRequest,
    config: &rivers_runtime::view::ApiViewConfig,
    trace_id: &str,
    app_id: &str,
) -> axum::response::Response {
    use crate::streaming::{StreamingConfig, StreamingFormat, StreamChunk, run_streaming_generator, poison_chunk_ndjson, poison_chunk_sse};
    use crate::process_pool::Entrypoint;

    // Determine streaming format
    let format = StreamingFormat::from_str_opt(config.streaming_format.as_deref())
        .unwrap_or(StreamingFormat::Ndjson);
    let content_type = format.content_type();
    let stream_timeout_ms = config.stream_timeout_ms.unwrap_or(120_000);

    let streaming_config = StreamingConfig {
        format: format.clone(),
        stream_timeout_ms,
    };

    // Extract CodeComponent entrypoint
    let entrypoint = match &config.handler {
        rivers_runtime::view::HandlerConfig::Codecomponent { language, module, entrypoint, .. } => {
            Entrypoint {
                module: module.clone(),
                function: entrypoint.clone(),
                language: language.clone(),
            }
        }
        _ => {
            return error_response::internal_error("streaming views require CodeComponent handler")
                .with_trace_id(trace_id.to_string())
                .into_axum_response();
        }
    };

    // Create channels: generator → formatter → HTTP stream
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<StreamChunk>(64);
    let (wire_tx, wire_rx) = tokio::sync::mpsc::channel::<String>(64);

    // Spawn generator task — owns chunk_tx, drops it when done
    let pool = ctx.pool.clone();
    let trace_owned = trace_id.to_string();
    let app_id_owned = app_id.to_string();
    let gen_handle = tokio::spawn(async move {
        run_streaming_generator(
            &pool,
            &entrypoint,
            &streaming_config,
            chunk_tx,
            &trace_owned,
            &app_id_owned,
        )
        .await
    });

    // Spawn formatter task — reads chunks, formats, writes to wire
    let fmt2 = format;
    tokio::spawn(async move {
        while let Some(chunk) = chunk_rx.recv().await {
            let wire = match fmt2 {
                StreamingFormat::Ndjson => chunk.to_ndjson(),
                StreamingFormat::Sse => chunk.to_sse(None),
            };
            if wire_tx.send(wire).await.is_err() {
                return; // Client disconnected
            }
        }
        // Generator is done (chunk_tx dropped). Check for error.
        if let Ok(Err(e)) = gen_handle.await {
            let poison = match fmt2 {
                StreamingFormat::Ndjson => poison_chunk_ndjson(&e.to_string()),
                StreamingFormat::Sse => poison_chunk_sse(&e.to_string()),
            };
            let _ = wire_tx.send(poison).await;
        }
    });

    build_streaming_response(content_type, wire_rx)
}

// ── WebSocket View Handler ─────────────────────────────────────────

/// Execute a WebSocket view — upgrade HTTP to WebSocket connection.
///
/// Per spec §6: bidirectional connection with read/write loop and lifecycle hooks.
pub(super) async fn execute_ws_view(
    ctx: AppContext,
    request: Request,
    matched: MatchedRoute,
) -> axum::response::Response {
    use axum::extract::ws::WebSocketUpgrade;

    let view_id = matched.view_id.clone();
    let trace_id = uuid::Uuid::new_v4().to_string();
    let config = matched.config.clone();
    // Plan G: pass entry-point slug to the WS dispatch helpers (matches
    // the RT-CTX-APP-ID parity fix REST + MCP got). `matched.app_id` is
    // the manifest UUID — kept around if any audit/log line needs it,
    // but not the value to thread into TaskContext.
    let dv_namespace = matched.app_entry_point.clone();

    // Extract WebSocketUpgrade from the request parts (before body consumption)
    let (mut parts, _body) = request.into_parts();
    let ws_upgrade: WebSocketUpgrade = match <WebSocketUpgrade as FromRequestParts<()>>::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!(view_id = %view_id, error = %e, "WebSocket upgrade failed");
            return error_response::bad_request(format!("WebSocket upgrade failed: {}", e))
                .with_trace_id(trace_id)
                .into_axum_response();
        }
    };

    // Determine WebSocket mode
    let ws_mode = crate::websocket::WebSocketMode::from_str_opt(
        config.websocket_mode.as_deref(),
    );

    let ctx_clone = ctx.clone();
    ws_upgrade
        .on_upgrade(move |socket| {
            handle_ws_connection(ctx_clone, socket, view_id, config, ws_mode, trace_id, dv_namespace)
        })
        .into_response()
}

/// Handle an active WebSocket connection — read/write loop with lifecycle hooks.
///
/// Uses a single-owner pattern: the socket stays in one task that alternates
/// between reading client frames and draining outbound messages (no split needed).
async fn handle_ws_connection(
    ctx: AppContext,
    mut socket: WebSocket,
    view_id: String,
    config: rivers_runtime::view::ApiViewConfig,
    ws_mode: crate::websocket::WebSocketMode,
    trace_id: String,
    dv_namespace: String,
) {
    use axum::extract::ws::Message;
    use crate::websocket::{
        WebSocketMode, ConnectionId, ConnectionInfo, WebSocketMessage,
        WsRateLimiter, BinaryFrameTracker, dispatch_ws_lifecycle, execute_ws_on_stream,
    };
    use crate::process_pool::Entrypoint;

    let conn_id = ConnectionId::new();
    tracing::info!(
        view_id = %view_id,
        connection_id = %conn_id.0,
        mode = ?ws_mode,
        "WebSocket connected"
    );

    // Rate limiter (per-connection)
    let rate_limiter = WsRateLimiter::new(
        config.rate_limit_per_minute,
        config.rate_limit_burst_size,
    );

    let binary_tracker = BinaryFrameTracker::new();

    // Dispatch on_connect lifecycle hook
    if let Some(ref hooks) = config.ws_hooks {
        if let Some(ref on_connect) = hooks.on_connect {
            // Plan G: snapshot executor before dispatching so codecomponent
            // hooks see the per-app datasource set + correct slug.
            let exec_guard = ctx.dataview_executor.read().await;
            let executor = exec_guard.as_deref();
            match dispatch_ws_lifecycle(
                &ctx.pool,
                &on_connect.module,
                &on_connect.entrypoint,
                &conn_id.0,
                None,
                None,
                &trace_id,
                &dv_namespace,
                executor,
            )
            .await
            {
                Ok(reply) if !reply.is_null() => {
                    let reply_str = serde_json::to_string(&reply).unwrap_or_default();
                    let _ = socket.send(Message::Text(reply_str.into())).await;
                }
                Err(e) => {
                    tracing::warn!(connection_id = %conn_id.0, error = %e, "onConnect hook failed");
                }
                _ => {}
            }
        }
    }

    // Publish WebSocket connected event
    {
        let event = rivers_runtime::rivers_core::Event::new(
            rivers_runtime::rivers_core::eventbus::events::WEBSOCKET_CONNECTED,
            serde_json::json!({"connection_id": conn_id.0, "view_id": view_id}),
        ).with_trace_id(&trace_id);
        ctx.event_bus.publish(&event).await;
    }

    // Subscribe to broadcast or register in Direct mode
    let mut broadcast_rx: Option<tokio::sync::broadcast::Receiver<WebSocketMessage>> = None;
    match ws_mode {
        WebSocketMode::Broadcast => {
            if let Some(hub) = ctx.ws_manager.get_broadcast(&view_id).await {
                broadcast_rx = hub.subscribe().ok();
            }
        }
        WebSocketMode::Direct => {
            let info = ConnectionInfo {
                id: conn_id.clone(),
                view_id: view_id.clone(),
                connected_at: chrono::Utc::now(),
                session_id: None,
                path_params: HashMap::new(),
            };
            if let Some(registry) = ctx.ws_manager.get_direct(&view_id).await {
                broadcast_rx = registry.register(info).await.ok();
            }
        }
    }

    // Extract on_stream entrypoint if configured
    let on_stream_ep = config.on_stream.as_ref().map(|os| Entrypoint {
        module: os.module.clone(),
        function: os.entrypoint.clone(),
        language: "javascript".to_string(),
    });

    // Extract on_message hook if configured
    let on_message_hook = config.ws_hooks.as_ref().and_then(|h| h.on_message.clone());

    // Main loop: owns the socket, alternates between recv and outbound sends
    loop {
        tokio::select! {
            // Read from client
            msg_opt = socket.recv() => {
                let msg = match msg_opt {
                    Some(Ok(m)) => m,
                    Some(Err(_)) | None => break, // Connection error or closed
                };

                match msg {
                    Message::Text(text) => {
                        // Rate limit check
                        if let Some(ref rl) = rate_limiter {
                            if !rl.check() {
                                tracing::debug!(connection_id = %conn_id.0, "WS rate limited");
                                continue;
                            }
                        }

                        // Publish message-in EventBus event
                        {
                            let event = rivers_runtime::rivers_core::Event::new(
                                rivers_runtime::rivers_core::eventbus::events::WEBSOCKET_MESSAGE_IN,
                                serde_json::json!({"connection_id": conn_id.0, "view_id": view_id}),
                            ).with_trace_id(&trace_id);
                            ctx.event_bus.publish(&event).await;
                        }

                        // Dispatch on_message lifecycle hook if configured
                        if let Some(ref hook) = on_message_hook {
                            let message_val: serde_json::Value =
                                serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text.to_string()));
                            let exec_guard = ctx.dataview_executor.read().await;
                            let executor = exec_guard.as_deref();
                            match dispatch_ws_lifecycle(
                                &ctx.pool,
                                &hook.module,
                                &hook.entrypoint,
                                &conn_id.0,
                                Some(&message_val),
                                None,
                                &trace_id,
                                &dv_namespace,
                                executor,
                            ).await {
                                Ok(reply) if !reply.is_null() => {
                                    let reply_str = serde_json::to_string(&reply).unwrap_or_default();
                                    if socket.send(Message::Text(reply_str.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(connection_id = %conn_id.0, error = %e, "onMessage hook failed");
                                }
                                _ => {}
                            }
                        }

                        // Dispatch on_stream handler if configured
                        if let Some(ref ep) = on_stream_ep {
                            let message_val: serde_json::Value =
                                serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text.to_string()));
                            let exec_guard = ctx.dataview_executor.read().await;
                            let executor = exec_guard.as_deref();
                            match execute_ws_on_stream(
                                &ctx.pool,
                                ep,
                                &message_val,
                                &conn_id,
                                &trace_id,
                                &dv_namespace,
                                executor,
                            )
                            .await
                            {
                                Ok(Some(reply)) => {
                                    let reply_str = serde_json::to_string(&reply)
                                        .unwrap_or_else(|_| "null".to_string());
                                    if socket.send(Message::Text(reply_str.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Ok(None) => {} // No reply
                                Err(e) => {
                                    tracing::warn!(
                                        connection_id = %conn_id.0,
                                        error = %e,
                                        "on_stream handler failed"
                                    );
                                }
                            }
                        }
                    }
                    Message::Binary(_) => {
                        if binary_tracker.record_binary_frame() {
                            tracing::warn!(
                                connection_id = %conn_id.0,
                                view_id = %view_id,
                                "binary WebSocket frame received (not supported)"
                            );
                        }
                    }
                    Message::Close(_) => break,
                    _ => {} // Ping/Pong handled by axum
                }
            }

            // Drain broadcast messages
            bcast = async {
                match broadcast_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match bcast {
                    Ok(ws_msg) => {
                        if socket.send(Message::Text(ws_msg.payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

        }
    }

    // Dispatch on_disconnect lifecycle hook
    if let Some(ref hooks) = config.ws_hooks {
        if let Some(ref on_disconnect) = hooks.on_disconnect {
            let exec_guard = ctx.dataview_executor.read().await;
            let executor = exec_guard.as_deref();
            match dispatch_ws_lifecycle(
                &ctx.pool,
                &on_disconnect.module,
                &on_disconnect.entrypoint,
                &conn_id.0,
                None,
                None,
                &trace_id,
                &dv_namespace,
                executor,
            )
            .await
            {
                Ok(reply) if !reply.is_null() => {
                    // In Broadcast mode, broadcast the farewell to remaining peers
                    if ws_mode == WebSocketMode::Broadcast {
                        if let Some(hub) = ctx.ws_manager.get_broadcast(&view_id).await {
                            let reply_str = serde_json::to_string(&reply).unwrap_or_default();
                            let _ = hub.broadcast(WebSocketMessage::text(reply_str));
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(connection_id = %conn_id.0, error = %e, "onDisconnect hook failed");
                }
                _ => {}
            }
        }
    }

    // Publish WebSocket disconnected event
    {
        let event = rivers_runtime::rivers_core::Event::new(
            rivers_runtime::rivers_core::eventbus::events::WEBSOCKET_DISCONNECTED,
            serde_json::json!({"connection_id": conn_id.0, "view_id": view_id}),
        ).with_trace_id(&trace_id);
        ctx.event_bus.publish(&event).await;
    }

    // Unregister from Direct mode registry
    if ws_mode == WebSocketMode::Direct {
        if let Some(registry) = ctx.ws_manager.get_direct(&view_id).await {
            registry.unregister(&conn_id.0).await;
        }
    }

    tracing::info!(
        view_id = %view_id,
        connection_id = %conn_id.0,
        "WebSocket disconnected"
    );
}
