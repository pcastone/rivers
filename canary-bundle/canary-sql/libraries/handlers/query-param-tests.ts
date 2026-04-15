// Query parameter canary tests — exercises parsing, queryAll, type coercion

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
        detail: passed ? undefined : "expected=" + JSON.stringify(expected) + ", actual=" + JSON.stringify(actual)
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

// QP-QUERY-ACCESS — verify ctx.request.query has first-value-wins
// Request: GET /qp/query-access?status=active&limit=20
function qpQueryAccess(ctx) {
    var t = new TestResult("QP-QUERY-ACCESS", "SQL", "rivers-query-param-spec §3.1");
    try {
        t.assertEquals("status-value", "active", ctx.request.query.status);
        t.assertEquals("limit-value", "20", ctx.request.query.limit);
        t.assert("missing-is-undefined", ctx.request.query.nonexistent === undefined);
        return t.finish();
    } catch(e) {
        return t.fail("" + e);
    }
}

// QP-QUERY-ALL — verify ctx.request.queryAll preserves duplicates
// Request: GET /qp/query-all?tag=a&tag=b&tag=c&single=one
function qpQueryAll(ctx) {
    var t = new TestResult("QP-QUERY-ALL", "SQL", "rivers-query-param-spec §3.2");
    try {
        // queryAll should have arrays for all keys
        var tags = ctx.request.queryAll.tag;
        t.assert("tags-is-array", Array.isArray(tags), "queryAll.tag type: " + typeof tags);
        if (Array.isArray(tags)) {
            t.assertEquals("tags-count", 3, tags.length);
            t.assertEquals("tags-order", ["a", "b", "c"], tags);
        }
        // Single value should be array of one
        var single = ctx.request.queryAll.single;
        t.assert("single-is-array", Array.isArray(single));
        if (Array.isArray(single)) {
            t.assertEquals("single-count", 1, single.length);
        }
        // First-value-wins on ctx.request.query
        t.assertEquals("query-first-wins", "a", ctx.request.query.tag);
        return t.finish();
    } catch(e) {
        return t.fail("" + e);
    }
}

// QP-PERCENT-DECODE — verify percent-encoded values are decoded
// Request: GET /qp/percent-decode?name=John%20Doe&city=S%C3%A3o%20Paulo
function qpPercentDecode(ctx) {
    var t = new TestResult("QP-PERCENT-DECODE", "SQL", "rivers-query-param-spec §2.1");
    try {
        t.assertEquals("decoded-space", "John Doe", ctx.request.query.name);
        t.assertEquals("decoded-utf8", "São Paulo", ctx.request.query.city);
        return t.finish();
    } catch(e) {
        return t.fail("" + e);
    }
}

// QP-EMPTY-VALUE — verify empty and bare keys
// Request: GET /qp/empty-value?key=&bare
function qpEmptyValue(ctx) {
    var t = new TestResult("QP-EMPTY-VALUE", "SQL", "rivers-query-param-spec §2.1");
    try {
        t.assertEquals("empty-value", "", ctx.request.query.key);
        t.assertEquals("bare-key", "", ctx.request.query.bare);
        return t.finish();
    } catch(e) {
        return t.fail("" + e);
    }
}
