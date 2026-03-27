# Tutorial: WebSocket Views

**Rivers v0.50.1**

## Overview

WebSocket views provide full-duplex, bidirectional communication between clients and your Rivers app. Clients connect via `ws://` or `wss://` and exchange messages in real time.

Rivers WebSocket views support:
- **Lifecycle hooks** — `on_connect`, `on_message`, `on_disconnect`
- **Two modes** — `Broadcast` (all clients see all messages) or `Direct` (1:1 communication)
- **Connection management** — automatic upgrade, heartbeat, and cleanup

## When to Use

- Chat applications
- Live notifications
- Collaborative editing
- Real-time gaming
- Any use case requiring bidirectional, low-latency communication

---

## Step 1: Create the View

File: `app.toml`

```toml
[api.views.chat]
path           = "ws/chat"
method         = "GET"
view_type      = "Websocket"
auth           = "none"
websocket_mode = "Broadcast"
```

| Field | Values | Description |
|-------|--------|-------------|
| `view_type` | `"Websocket"` | Enables WebSocket upgrade |
| `method` | `"GET"` | Must be GET — WebSocket upgrade is a GET request |
| `websocket_mode` | `"Broadcast"` or `"Direct"` | Broadcast sends replies to all clients; Direct sends only to the sender |

---

## Step 2: Add Lifecycle Hooks

```toml
[api.views.chat.ws_hooks]
on_connect.module        = "libraries/handlers/chat.js"
on_connect.entrypoint    = "onConnect"
on_message.module        = "libraries/handlers/chat.js"
on_message.entrypoint    = "onMessage"
on_disconnect.module     = "libraries/handlers/chat.js"
on_disconnect.entrypoint = "onDisconnect"
```

Each hook points to a JavaScript function.

---

## Step 3: Write the Handlers

File: `libraries/handlers/chat.js`

### on_connect

Called when a client completes the WebSocket upgrade. Return non-null to send a welcome message. Return `false` to reject the connection.

```javascript
function onConnect(ctx) {
    var connId = ctx.ws.connection_id;
    var username = ctx.request.query_params.username || "anonymous";

    // Store connection metadata (TTL 1 hour)
    ctx.store.set("ws:user:" + connId, {
        username: username,
        joined: new Date().toISOString()
    }, 3600000);

    Rivers.log.info("client connected", { connection_id: connId, username: username });

    // Return welcome message — sent to the connecting client
    return {
        type: "welcome",
        message: "Hello, " + username + "!"
    };
}
```

### on_message

Called for each inbound WebSocket frame. Return non-null to reply.

```javascript
function onMessage(ctx) {
    var connId = ctx.ws.connection_id;
    var msg = ctx.ws.message;
    var user = ctx.store.get("ws:user:" + connId);

    if (msg.type === "chat") {
        // In Broadcast mode, all connected clients receive this reply
        return {
            type: "chat",
            username: user ? user.username : "anonymous",
            text: msg.text,
            timestamp: new Date().toISOString()
        };
    }

    if (msg.type === "ping") {
        return { type: "pong" };
    }

    return { type: "error", message: "unknown type" };
}
```

### on_disconnect

Called when the connection closes. Return value is ignored.

```javascript
function onDisconnect(ctx) {
    var connId = ctx.ws.connection_id;
    ctx.store.del("ws:user:" + connId);
    Rivers.log.info("client disconnected", { connection_id: connId });
}
```

---

## Step 4: Add Auth (Optional)

For session-protected WebSocket views:

```toml
[api.views.secure_chat]
path           = "ws/secure"
method         = "GET"
view_type      = "Websocket"
auth           = "session"
websocket_mode = "Broadcast"
```

When `auth = "session"`, the client must pass a valid session token. The session claims are available in `ctx.session` inside your hooks.

---

## Testing

### Connect with wscat

```bash
# Install wscat
npm install -g wscat

# Connect
wscat -c "ws://localhost:8080/ws/chat?username=alice"

# Send a message
> {"type": "chat", "text": "Hello everyone!"}

# You'll receive:
< {"type": "chat", "username": "alice", "text": "Hello everyone!", "timestamp": "..."}
```

### Connect with curl (upgrade check)

```bash
curl -i -N \
  -H "Connection: Upgrade" \
  -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
  http://localhost:8080/ws/chat?username=test
```

---

## Broadcast vs Direct Mode

| Mode | Behavior |
|------|----------|
| `Broadcast` | Reply from `on_message` is sent to **all** connected clients |
| `Direct` | Reply from `on_message` is sent **only** to the sender |

Use `Broadcast` for chat rooms, collaborative tools, and live feeds. Use `Direct` for request/response patterns and private messaging.

---

## Hook Reference

| Hook | Trigger | Return Value |
|------|---------|-------------|
| `on_connect` | Client completes WS upgrade | Non-null → send to client. `false` → reject. |
| `on_message` | Each inbound WS frame | Non-null → reply (broadcast or direct). Null → no reply. |
| `on_disconnect` | Connection closes | Ignored — use for cleanup only |

### Context Properties in WebSocket Hooks

| Property | Description |
|----------|-------------|
| `ctx.ws.connection_id` | Unique connection identifier |
| `ctx.ws.message` | Parsed inbound message (on_message only) |
| `ctx.request.query_params` | Query string from the upgrade request |
| `ctx.request.headers` | Headers from the upgrade request |
| `ctx.session` | Session claims (when auth = "session") |
| `ctx.store` | Application KV store |
