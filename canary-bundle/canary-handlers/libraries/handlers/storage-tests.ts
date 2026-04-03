// StorageEngine tests — CRUD operations and reserved prefix enforcement.
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

// ── RT-STORE-CRUD — set with TTL, get, del, verify gone ──

function storeCrud(ctx) {
    var t = new TestResult("RT-STORE-CRUD", "RUNTIME", "storage-engine section 11.5");
    try {
        var key = "canary:store-crud:" + Date.now();
        var value = { message: "canary-crud-test", ts: Date.now() };
        var valueStr = JSON.stringify(value);

        // Set with TTL (60000 ms = 60 seconds — plenty for the test)
        ctx.store.set(key, valueStr, 60000);
        t.assert("set_ok", true, "key=" + key);

        // Get
        var got = ctx.store.get(key);
        t.assertEquals("get_matches", valueStr, got);

        // Delete
        ctx.store.del(key);
        t.assert("del_ok", true, "deleted key=" + key);

        // Verify gone
        var afterDel = ctx.store.get(key);
        t.assert("verify_gone", afterDel === null || afterDel === undefined,
            "after_del=" + JSON.stringify(afterDel));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-STORE-RESERVED — writing to reserved prefix (session:, csrf:) throws error ──

function storeReservedPrefix(ctx) {
    var t = new TestResult("RT-CTX-STORE-NAMESPACE", "RUNTIME", "storage-engine section 11.3");
    try {
        // Attempt to write to "session:" prefix — must be rejected
        var sessionThrew = false;
        var sessionErr = "";
        try {
            ctx.store.set("session:hijack-attempt", "evil-data");
        } catch (e) {
            sessionThrew = true;
            sessionErr = String(e);
        }
        t.assert("session_prefix_blocked", sessionThrew,
            "threw=" + sessionThrew + ", err=" + sessionErr.substring(0, 80));

        // Attempt to write to "csrf:" prefix — must be rejected
        var csrfThrew = false;
        var csrfErr = "";
        try {
            ctx.store.set("csrf:hijack-attempt", "evil-data");
        } catch (e) {
            csrfThrew = true;
            csrfErr = String(e);
        }
        t.assert("csrf_prefix_blocked", csrfThrew,
            "threw=" + csrfThrew + ", err=" + csrfErr.substring(0, 80));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
