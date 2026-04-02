// SSE handler — canary-streams STREAM profile.
// Tests SSE view with polling config, returns faker-generated data for SSE polling.

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

// ── STREAM-SSE-POLL — handler for SSE view with polling config ──
// Called by the SSE polling loop. Returns faker-sourced data as an SSE event payload.

function sseDataview(ctx) {
    var t = new TestResult("STREAM-SSE-POLL", "STREAM", "view-layer section 2.5");
    try {
        // Fetch data from the faker dataview
        var data = null;
        if (ctx.data && ctx.data.stream_faker) {
            data = ctx.data.stream_faker;
        } else if (typeof ctx.dataview === "function") {
            data = ctx.dataview("stream_faker", { limit: 5 });
        }

        t.assert("data_fetched", data !== null && data !== undefined,
            "type=" + typeof data);

        var hasRows = false;
        if (data && data.rows) {
            hasRows = data.rows.length > 0;
            t.assert("has_rows", hasRows, "row_count=" + data.rows.length);
        } else if (Array.isArray(data)) {
            hasRows = data.length > 0;
            t.assert("has_rows", hasRows, "array_length=" + data.length);
        } else {
            t.assert("has_rows", data !== null, "data=" + JSON.stringify(data));
        }

        t.assert("sse_context_valid", ctx !== null && ctx !== undefined,
            "ctx_keys=" + Object.keys(ctx).join(","));

        ctx.resdata = {
            event: "stream-data",
            data: data,
            timestamp: new Date().toISOString(),
            verdict: t.finish()
        };
    } catch (e) {
        ctx.resdata = {
            event: "stream-error",
            verdict: t.fail(String(e))
        };
    }
}
