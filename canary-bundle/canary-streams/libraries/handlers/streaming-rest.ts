// Streaming REST handler — canary-streams STREAM profile.
// Tests the {chunk, done} streaming protocol with NDJSON wire format.

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

// ── STREAM-REST-CHUNKS — streaming handler using {chunk, done} protocol ──
// Returns 5 chunks then done:true. Uses __args.iteration to track position.
// Rivers calls this function repeatedly — iteration increments each call.

function streamChunks(ctx) {
    var iteration = __args.iteration || 0;
    var totalChunks = 5;

    // After all chunks are sent, signal completion
    if (iteration >= totalChunks) {
        var t = new TestResult("STREAM-REST-CHUNKS", "STREAM", "streaming-rest section 2.7");
        t.assert("all_chunks_sent", true, "total=" + totalChunks);
        t.assertEquals("final_iteration", totalChunks, iteration);

        return {
            chunk: { type: "complete", verdict: t.finish() },
            done: true
        };
    }

    // Build chunk payload with test evidence
    var chunkData = {
        chunk_index: iteration,
        test_id: "STREAM-REST-CHUNKS",
        profile: "STREAM",
        message: "chunk-" + iteration + "-of-" + totalChunks,
        timestamp: new Date().toISOString()
    };

    return {
        chunk: chunkData,
        done: false
    };
}
