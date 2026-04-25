// Kafka MessageConsumer handler — canary-streams STREAM profile.
// Tests Kafka message consumption and verdict retrieval.

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

// ── STREAM-KAFKA-CONSUME — MessageConsumer view handler ──
// Invoked by the framework when a Kafka message arrives on topic "canary.kafka.test".
// Processes the message and stores the verdict in ctx.store for later retrieval.

function onMessage(ctx) {
    var t = new TestResult("STREAM-KAFKA-CONSUME", "STREAM", "rivers-view-layer-spec.md section 2.6");
    try {
        var msg = ctx.request.body;
        t.assert("message_received", msg !== null && msg !== undefined, "body type=" + typeof msg);

        if (msg) {
            t.assert("has_payload", msg.payload !== undefined || typeof msg === "string",
                "payload present");
        }

        // Store verdict for retrieval by the test harness (existing behaviour).
        ctx.store.set("canary:kafka:last_verdict", t.finish(), 60000);

        // CS3 — if this message is a scenario event (JSON payload with the
        // AF-6 shape), persist it to the events table. Messages from other
        // producers without that shape are skipped silently.
        try {
            var body;
            if (typeof msg === "string") {
                body = JSON.parse(msg);
            } else if (msg && msg.payload) {
                body = typeof msg.payload === "string" ? JSON.parse(msg.payload) : msg.payload;
            } else {
                body = msg;
            }
            if (body && body.id && body.actor && body.target_user && body.event_type && body.published_at) {
                ctx.dataview("events_insert", {
                    id: body.id,
                    actor: body.actor,
                    target_user: body.target_user,
                    event_type: body.event_type,
                    payload: typeof body.payload === "string" ? body.payload : JSON.stringify(body.payload || null),
                    published_at: body.published_at,
                    consumed_at: new Date().toISOString(),
                });
            }
        } catch (persistErr) {
            Rivers.log.warn("canary-kafka consumer: scenario-event persist skipped — " + String(persistErr));
        }
    } catch (e) {
        ctx.store.set("canary:kafka:last_verdict", t.fail(String(e)), 60000);
    }

    ctx.resdata = { processed: true };
}

// ── STREAM-KAFKA-VERIFY — REST endpoint to retrieve the last Kafka consume verdict ──

function kafkaVerify(ctx) {
    var t = new TestResult("STREAM-KAFKA-VERIFY", "STREAM", "rivers-view-layer-spec.md section 2.6");
    try {
        var verdict = ctx.store.get("canary:kafka:last_verdict");
        if (verdict) {
            ctx.resdata = verdict;
            return;
        }
        t.assert("verdict_found", false, "no kafka consume verdict in store");
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}
