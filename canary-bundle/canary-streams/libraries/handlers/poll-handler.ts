// Poll handler — canary-streams STREAM profile.
// Tests polling with hash diff detection and on_change callback.

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

// ── pollData — REST endpoint returning data for polling diff detection ──
// Used by the poll_data REST view. Also used as the DataView for the SSE poll_hash view.

function pollData(ctx) {
    var t = new TestResult("STREAM-POLL-HASH", "STREAM", "polling-views §10.2");
    try {
        var data = null;
        if (ctx.data && ctx.data.poll_data) {
            data = ctx.data.poll_data;
        } else if (ctx.data && ctx.data.poll_faker) {
            data = ctx.data.poll_faker;
        } else if (typeof ctx.dataview === "function") {
            data = ctx.dataview("poll_data", { limit: 3 });
        }

        t.assert("poll_data_fetched", data !== null && data !== undefined,
            "type=" + typeof data);

        var hasContent = false;
        if (data && data.rows) {
            hasContent = data.rows.length > 0;
            t.assert("poll_has_rows", hasContent, "row_count=" + data.rows.length);
        } else if (Array.isArray(data)) {
            hasContent = data.length > 0;
            t.assert("poll_has_rows", hasContent, "array_length=" + data.length);
        } else {
            t.assert("poll_has_rows", data !== null, "data=" + JSON.stringify(data));
        }

        // Include a timestamp so the hash changes on each poll tick
        ctx.resdata = {
            poll_data: data,
            polled_at: new Date().toISOString(),
            verdict: t.finish()
        };
    } catch (e) {
        ctx.resdata = { verdict: t.fail(String(e)) };
    }
}

// ── onPollChange — called when the polling hash changes ──
// This is the on_change callback for the poll_hash SSE view.
// Receives the new data after a hash change is detected.

function onPollChange(ctx) {
    var t = new TestResult("STREAM-POLL-HASH", "STREAM", "polling-views §10.2");
    try {
        t.assert("change_detected", true, "polling hash changed — on_change fired");

        var data = null;
        if (ctx.data) {
            data = ctx.data;
        } else if (ctx.request && ctx.request.body) {
            data = ctx.request.body;
        }

        t.assert("change_data_present", data !== null && data !== undefined,
            "type=" + typeof data);

        ctx.resdata = {
            event: "poll-change",
            data: data,
            changed_at: new Date().toISOString(),
            verdict: t.finish()
        };
    } catch (e) {
        ctx.resdata = {
            event: "poll-change-error",
            verdict: t.fail(String(e))
        };
    }
}
