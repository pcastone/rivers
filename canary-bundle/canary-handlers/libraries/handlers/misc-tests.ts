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

        // Compare the two results — seeded faker should return identical data.
        // JSON key order may vary (HashMap), so compare row count and field values.
        var rows1 = result1.rows || result1;
        var rows2 = result2.rows || result2;
        t.assert("same_row_count",
            Array.isArray(rows1) && Array.isArray(rows2) && rows1.length === rows2.length,
            "rows1=" + (rows1 ? rows1.length : "?") + ", rows2=" + (rows2 ? rows2.length : "?"));
        if (Array.isArray(rows1) && Array.isArray(rows2) && rows1.length > 0) {
            // Compare sorted keys and values of first row
            var keys1 = Object.keys(rows1[0]).sort();
            var keys2 = Object.keys(rows2[0]).sort();
            t.assert("same_fields", keys1.join(",") === keys2.join(","),
                "keys1=" + keys1.join(",") + ", keys2=" + keys2.join(","));
            var match = keys1.every(function(k) {
                return JSON.stringify(rows1[0][k]) === JSON.stringify(rows2[0][k]);
            });
            t.assert("same_values", match,
                match ? "first row values match" : "first row values differ");
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
