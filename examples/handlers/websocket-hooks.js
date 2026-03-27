// WebSocket lifecycle hooks — on_connect, on_message, on_disconnect
//
// Config:
//   [api.views.live]
//   path           = "/ws/live"
//   method         = "GET"
//   view_type      = "Websocket"
//   websocket_mode = "Broadcast"    # or "Direct"
//
//   [api.views.live.ws_hooks]
//   on_connect.module        = "libraries/handlers/ws.js"
//   on_connect.entrypoint    = "onConnect"
//   on_message.module        = "libraries/handlers/ws.js"
//   on_message.entrypoint    = "onMessage"
//   on_disconnect.module     = "libraries/handlers/ws.js"
//   on_disconnect.entrypoint = "onDisconnect"

// Called when a client completes the WebSocket upgrade
// Return non-null  → send welcome message to client
// Return false     → reject the connection
function onConnect(ctx) {
    var connId = ctx.ws.connection_id;
    var query  = ctx.request.query_params;

    // Reject unauthenticated connections
    if (!query.token) {
        Rivers.log.warn("ws rejected — no token", { connection_id: connId });
        return false;
    }

    // Store connection metadata (TTL 1 hour)
    ctx.store.set("ws:" + connId, {
        token: query.token,
        connected_at: new Date().toISOString()
    }, 3600000);

    Rivers.log.info("ws connected", { connection_id: connId });

    return {
        type: "connected",
        connection_id: connId,
        server_time: new Date().toISOString()
    };
}

// Called for each inbound WebSocket frame
// Return non-null → send reply to client (or broadcast in Broadcast mode)
// Return null     → no reply
function onMessage(ctx) {
    var connId = ctx.ws.connection_id;
    var msg    = ctx.ws.message;

    // Route by message type
    switch (msg.type) {
        case "ping":
            return { type: "pong", timestamp: new Date().toISOString() };

        case "subscribe":
            ctx.store.set("ws:sub:" + connId, { channel: msg.channel }, 3600000);
            Rivers.log.info("ws subscribed", { connection_id: connId, channel: msg.channel });
            return { type: "subscribed", channel: msg.channel };

        case "broadcast":
            // In Broadcast mode, this goes to all connected clients
            return {
                type: "broadcast",
                from: connId,
                data: msg.data,
                timestamp: new Date().toISOString()
            };

        default:
            return { type: "error", message: "unknown type: " + msg.type };
    }
}

// Called when the client or server closes the connection
// Return value is ignored — use for cleanup only
function onDisconnect(ctx) {
    var connId = ctx.ws.connection_id;

    ctx.store.del("ws:" + connId);
    ctx.store.del("ws:sub:" + connId);

    Rivers.log.info("ws disconnected", { connection_id: connId });
}
