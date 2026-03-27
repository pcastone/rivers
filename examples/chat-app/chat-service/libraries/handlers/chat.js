// WebSocket lifecycle hook: client connects
// Return non-null to send welcome message; return false to reject connection
function onConnect(ctx) {
    var connId = ctx.ws.connection_id;
    var username = ctx.request.query.username || "anonymous";

    // Store connection info
    ctx.store.set("ws:user:" + connId, { username: username, joined: new Date().toISOString() }, 3600000);

    Rivers.log.info("client connected", { connection_id: connId, username: username });

    return {
        type: "welcome",
        message: "Welcome to the chat, " + username + "!",
        connection_id: connId
    };
}

// WebSocket lifecycle hook: message received
// Return non-null to send a reply back to the client
function onMessage(ctx) {
    var connId = ctx.ws.connection_id;
    var msg = ctx.ws.message;
    var user = ctx.store.get("ws:user:" + connId);
    var username = user ? user.username : "anonymous";

    Rivers.log.info("message received", { connection_id: connId, username: username });

    // Handle different message types
    if (msg.type === "ping") {
        return { type: "pong", timestamp: new Date().toISOString() };
    }

    if (msg.type === "chat") {
        // Store recent message
        var msgId = Rivers.crypto.randomHex(8);
        ctx.store.set("ws:msg:" + msgId, {
            id: msgId,
            username: username,
            text: msg.text,
            timestamp: new Date().toISOString()
        }, 3600000);

        // Echo back with metadata (in Broadcast mode, all clients receive this)
        return {
            type: "chat",
            id: msgId,
            username: username,
            text: msg.text,
            timestamp: new Date().toISOString()
        };
    }

    return { type: "error", message: "unknown message type: " + msg.type };
}

// WebSocket lifecycle hook: client disconnects
// Use for cleanup — return value is ignored
function onDisconnect(ctx) {
    var connId = ctx.ws.connection_id;
    var user = ctx.store.get("ws:user:" + connId);
    var username = user ? user.username : "unknown";

    // Clean up stored connection
    ctx.store.del("ws:user:" + connId);

    Rivers.log.info("client disconnected", { connection_id: connId, username: username });
}
