// Streaming REST handler — canary-streams STREAM profile.
// Tests NDJSON streaming and poison chunk guard (SHAPE-15).

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

// ── STREAM-REST-NDJSON — generator that yields 5 NDJSON chunks then closes ──

function* streamNdjson(ctx) {
    for (var i = 0; i < 5; i++) {
        yield {
            chunk_index: i,
            test_id: "STREAM-REST-NDJSON",
            profile: "STREAM",
            data: "chunk-" + i
        };
    }
}

// ── STREAM-REST-POISON — mid-stream error handler ──
// Yields a normal chunk first, then a chunk with stream_terminated.
// SHAPE-15 guard must block the poison chunk and terminate the stream.

function* streamPoison(ctx) {
    // Yield a normal chunk first
    yield { chunk_index: 0, data: "normal" };
    // Then yield a chunk with stream_terminated — SHAPE-15 guard must block this
    yield { stream_terminated: true, data: "this should be blocked" };
    // If guard works, this is unreachable (generator terminated by runtime)
    yield { chunk_index: 2, data: "should not arrive" };
}
