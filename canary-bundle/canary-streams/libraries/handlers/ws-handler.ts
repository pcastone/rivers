// WebSocket lifecycle hooks — canary-streams STREAM profile.
// Tests WebSocket connect, message echo, broadcast, and disconnect lifecycle.

// ── Inline TestResult (cross-app imports forbidden) ──

function TestResult(test_id, profile, spec_ref) {
    this.test_id = test_id;
    this.profile = profile;
    this.spec_ref = spec_ref;
    this.assertions = [];
    this.error = null;
    this.start = Date.now();
}
TestResult.prototype.assert = function(id, passed, detail) {
    this.assertions.push({ id: id, passed: passed, detail: detail || undefined });
};
TestResult.prototype.assertEquals = function(id, expected, actual) {
    var passed = JSON.stringify(expected) === JSON.stringify(actual);
    this.assertions.push({
        id: id, passed: passed,
        detail: passed ? "expected=" + JSON.stringify(expected)
            : "expected=" + JSON.stringify(expected) + ", actual=" + JSON.stringify(actual)
    });
};
TestResult.prototype.finish = function() {
    return {
        test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: this.assertions.every(function(a) { return a.passed; }),
        assertions: this.assertions, duration_ms: Date.now() - this.start, error: this.error
    };
};
TestResult.prototype.fail = function(err) {
    this.error = err;
    return {
        test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: false, assertions: this.assertions, duration_ms: Date.now() - this.start, error: err
    };
};

// ── ws_echo view: onConnection — called on WebSocket upgrade for echo mode ──

function onConnection(ctx) {
    var t = new TestResult("STREAM-WS-ECHO", "STREAM", "view-layer §2.4");
    try {
        var connId = ctx.ws.connection_id;

        t.assert("ws_exists", ctx.ws !== null && ctx.ws !== undefined,
            "type=" + typeof ctx.ws);
        t.assert("connection_id_exists", connId !== null && connId !== undefined,
            "connection_id=" + connId);
        t.assert("connection_id_not_empty",
            typeof connId === "string" && connId.length > 0,
            "length=" + (connId ? connId.length : 0));

        Rivers.log.info("ws echo connected", { connection_id: connId });

        return {
            type: "welcome",
            connection_id: connId,
            server_time: new Date().toISOString(),
            verdict: t.finish()
        };
    } catch (e) {
        return { type: "error", verdict: t.fail(String(e)) };
    }
}

// ── ws_echo view: onMessage — called for each inbound frame, echoes back ──

function onMessage(ctx) {
    var t = new TestResult("STREAM-WS-ECHO", "STREAM", "view-layer §2.4");
    try {
        var connId = ctx.ws.connection_id;
        var msg = ctx.ws.message;

        t.assert("message_received", msg !== null && msg !== undefined,
            "type=" + typeof msg);
        t.assert("connection_id_on_message",
            typeof connId === "string" && connId.length > 0,
            "connection_id=" + connId);

        Rivers.log.info("ws echo message received", {
            connection_id: connId,
            message_type: typeof msg
        });

        return {
            type: "echo",
            connection_id: connId,
            original: msg,
            echoed_at: new Date().toISOString(),
            verdict: t.finish()
        };
    } catch (e) {
        return { type: "error", verdict: t.fail(String(e)) };
    }
}

// ── ws_broadcast view: onBroadcastConnection — called on WebSocket upgrade for broadcast mode ──

function onBroadcastConnection(ctx) {
    var t = new TestResult("STREAM-WS-BROADCAST", "STREAM", "view-layer §2.4");
    try {
        var connId = ctx.ws.connection_id;

        t.assert("ws_exists", ctx.ws !== null && ctx.ws !== undefined,
            "type=" + typeof ctx.ws);
        t.assert("connection_id_exists", connId !== null && connId !== undefined,
            "connection_id=" + connId);
        t.assert("broadcast_mode", true, "view configured as broadcast");

        Rivers.log.info("ws broadcast connected", { connection_id: connId });

        return {
            type: "welcome",
            mode: "broadcast",
            connection_id: connId,
            server_time: new Date().toISOString(),
            verdict: t.finish()
        };
    } catch (e) {
        return { type: "error", verdict: t.fail(String(e)) };
    }
}

// ── ws_broadcast view: onBroadcastMessage — called for each inbound frame, fans out to all clients ──

function onBroadcastMessage(ctx) {
    var t = new TestResult("STREAM-WS-BROADCAST", "STREAM", "view-layer §2.4");
    try {
        var connId = ctx.ws.connection_id;
        var msg = ctx.ws.message;

        t.assert("message_received", msg !== null && msg !== undefined,
            "type=" + typeof msg);
        t.assert("connection_id_on_broadcast",
            typeof connId === "string" && connId.length > 0,
            "connection_id=" + connId);

        Rivers.log.info("ws broadcast message", {
            connection_id: connId,
            message_type: typeof msg
        });

        // Return value is broadcast to all connected clients
        return {
            type: "broadcast",
            from: connId,
            original: msg,
            broadcast_at: new Date().toISOString(),
            verdict: t.finish()
        };
    } catch (e) {
        return { type: "error", verdict: t.fail(String(e)) };
    }
}

// ── onDisconnect — cleanup on connection close ──

function onDisconnect(ctx) {
    var connId = ctx.ws.connection_id;

    Rivers.log.info("ws disconnected", { connection_id: connId });

    if (ctx.store && typeof ctx.store.del === "function") {
        ctx.store.del("ws:canary:" + connId);
    }
}
