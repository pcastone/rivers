// Kafka MessageConsumer handler for STREAM profile.

function TestResult(test_id, profile, spec_ref) {
    this.test_id = test_id; this.profile = profile; this.spec_ref = spec_ref;
    this.assertions = []; this.error = null; this.start = Date.now();
}
TestResult.prototype.assert = function(id, passed, detail) {
    this.assertions.push({ id: id, passed: passed, detail: detail || undefined });
};
TestResult.prototype.finish = function() {
    return { test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: this.assertions.every(function(a) { return a.passed; }),
        assertions: this.assertions, duration_ms: Date.now() - this.start, error: this.error };
};
TestResult.prototype.fail = function(err) {
    this.error = err;
    return { test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: false, assertions: this.assertions, duration_ms: Date.now() - this.start, error: err };
};

// STREAM-KAFKA-CONSUME — MessageConsumer view handler
// This handler is invoked by the framework when a Kafka message arrives.
// It processes the message and stores the verdict in ctx.store for later retrieval.
function kafkaConsume(ctx) {
    var t = new TestResult("STREAM-KAFKA-CONSUME", "STREAM", "rivers-view-layer-spec.md section 2.6");
    try {
        var msg = ctx.request.body;
        t.assert("message_received", msg !== null && msg !== undefined, "body type=" + typeof msg);

        if (msg) {
            t.assert("has_payload", msg.payload !== undefined || typeof msg === "string",
                "payload present");
        }

        // Store verdict for retrieval by the test harness
        ctx.store.set("canary:kafka:last_verdict", t.finish(), 60000);
    } catch (e) {
        ctx.store.set("canary:kafka:last_verdict", t.fail(String(e)), 60000);
    }

    ctx.resdata = { processed: true };
}

// STREAM-KAFKA-VERIFY — retrieve the last Kafka consume verdict
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
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
