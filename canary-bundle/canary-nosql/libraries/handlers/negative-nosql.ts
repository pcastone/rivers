// Negative NoSQL tests — admin operation rejection per driver.

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

// NOSQL-REDIS-ADMIN-REJECT — FLUSHDB on Redis is blocked by Gate 1
function adminOpRejected(ctx) {
    var t = new TestResult("NOSQL-REDIS-ADMIN-REJECT", "NOSQL", "feature-inventory section 21.1");
    try {
        // This DataView has a FLUSHDB command — should be rejected by Gate 1
        var threw = false;
        var errMsg = "";
        try {
            ctx.dataview("redis_flushdb_trap");
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }

        t.assert("admin_blocked", threw, "threw=" + threw);
        t.assert("error_contains_forbidden",
            errMsg.toLowerCase().indexOf("forbidden") >= 0,
            "error=" + errMsg.substring(0, 80)
        );
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// NOSQL-MONGO-ADMIN-REJECT — drop_collection on MongoDB is blocked by Gate 1
function mongoAdminReject(ctx) {
    var t = new TestResult("NOSQL-MONGO-ADMIN-REJECT", "NOSQL", "feature-inventory section 21.1");
    try {
        var threw = false;
        var errMsg = "";
        try {
            ctx.dataview("mongo_drop_trap");
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }

        t.assert("admin_blocked", threw, "threw=" + threw);
        t.assert("error_contains_forbidden",
            errMsg.toLowerCase().indexOf("forbidden") >= 0,
            "error=" + errMsg.substring(0, 80)
        );
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
