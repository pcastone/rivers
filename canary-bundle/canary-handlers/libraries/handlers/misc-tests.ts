// Miscellaneous RUNTIME profile tests — header blocklist, faker determinism.
// Each function is a standalone test endpoint for the canary fleet.

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

// ── RT-HEADER-BLOCKLIST — verify response headers are controlled by the framework ──
// Rivers does not expose ctx.response.setHeader to handlers.
// Security headers (X-Content-Type-Options, X-Frame-Options, etc.) are injected
// by the middleware pipeline. Handlers control response data via ctx.resdata only.

function headerBlocklist(ctx) {
    var t = new TestResult("RT-HEADER-BLOCKLIST", "RUNTIME", "feature-inventory section 1.5");
    try {
        // Verify handlers cannot set response headers directly.
        // This is by design — the framework controls all HTTP headers.
        t.assert("no_response_setHeader",
            !ctx.response || typeof ctx.response.setHeader !== "function",
            "ctx.response.setHeader must not be available to handlers");

        // Verify resdata is the only output mechanism
        t.assert("resdata_is_output", "resdata" in ctx,
            "ctx.resdata is the handler output mechanism");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-FAKER-DETERMINISM — call faker DataView twice with same seed, verify same results ──

function fakerDeterminism(ctx) {
    var t = new TestResult("RT-FAKER-DETERMINISM", "RUNTIME", "data-layer section 4.1");
    try {
        // Call the faker DataView twice with the same parameters.
        // With the same seed configured on the datasource, results should be identical.
        var result1 = ctx.dataview("list_records", { limit: 5 });
        var result2 = ctx.dataview("list_records", { limit: 5 });

        t.assert("result1_not_null", result1 !== null && result1 !== undefined,
            "type=" + typeof result1);
        t.assert("result2_not_null", result2 !== null && result2 !== undefined,
            "type=" + typeof result2);

        // Compare the two results — seeded faker should return identical data
        var str1 = JSON.stringify(result1);
        var str2 = JSON.stringify(result2);
        t.assert("results_identical", str1 === str2,
            str1 === str2
                ? "both calls returned identical data"
                : "result1=" + str1.substring(0, 100) + ", result2=" + str2.substring(0, 100));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
