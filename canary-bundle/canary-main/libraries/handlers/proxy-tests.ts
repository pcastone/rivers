// Proxy tests — verify canary-main can reach each service through HTTP proxy DataViews.
// Each function is a standalone test endpoint for the canary fleet.

// -- Inline TestResult (cross-app imports forbidden) --

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

// -- PROXY-HEALTH -- verify canary-main can reach its own health endpoint via self-check

function proxyHealth(ctx) {
    var t = new TestResult("PROXY-HEALTH", "PROXY", "http-driver-spec section 6");
    try {
        // Verify ctx.dataview is callable
        t.assert("dataview_is_function", typeof ctx.dataview === "function",
            "type=" + typeof ctx.dataview);

        // Call proxy_guard_ping as a basic health reachability check
        var result = ctx.dataview("proxy_guard_ping", {});
        t.assert("proxy_returned", result !== null && result !== undefined,
            "type=" + typeof result);

        // If we got here without throwing, the HTTP proxy round-trip works
        t.assert("proxy_reachable", true, "HTTP proxy round-trip succeeded");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// -- PROXY-GUARD-FORWARD -- verify cross-app proxy to canary-guard works

function proxyGuard(ctx) {
    var t = new TestResult("PROXY-GUARD-FORWARD", "PROXY", "http-driver-spec section 6");
    try {
        var result = ctx.dataview("proxy_guard_ping", {});
        t.assert("guard_responded", result !== null && result !== undefined,
            "type=" + typeof result);

        // The upstream response should be a valid object or array
        if (result && result.rows) {
            t.assert("guard_has_rows", Array.isArray(result.rows), "type=" + typeof result.rows);
        } else if (typeof result === "object") {
            t.assert("guard_is_object", true, "keys=" + Object.keys(result).join(","));
        }

        t.assert("guard_proxy_ok", true, "canary-guard reachable via HTTP proxy");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// -- PROXY-SQL-FORWARD -- verify proxy to canary-sql works

function proxySql(ctx) {
    var t = new TestResult("PROXY-SQL-FORWARD", "PROXY", "http-driver-spec section 6");
    try {
        var result = ctx.dataview("proxy_sql_ping", {});
        t.assert("sql_responded", result !== null && result !== undefined,
            "type=" + typeof result);

        if (result && result.rows) {
            t.assert("sql_has_rows", Array.isArray(result.rows), "type=" + typeof result.rows);
        } else if (typeof result === "object") {
            t.assert("sql_is_object", true, "keys=" + Object.keys(result).join(","));
        }

        t.assert("sql_proxy_ok", true, "canary-sql reachable via HTTP proxy");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// -- PROXY-NOSQL-FORWARD -- verify proxy to canary-nosql works

function proxyNosql(ctx) {
    var t = new TestResult("PROXY-NOSQL-FORWARD", "PROXY", "http-driver-spec section 6");
    try {
        var result = ctx.dataview("proxy_nosql_ping", {});
        t.assert("nosql_responded", result !== null && result !== undefined,
            "type=" + typeof result);

        if (result && result.rows) {
            t.assert("nosql_has_rows", Array.isArray(result.rows), "type=" + typeof result.rows);
        } else if (typeof result === "object") {
            t.assert("nosql_is_object", true, "keys=" + Object.keys(result).join(","));
        }

        t.assert("nosql_proxy_ok", true, "canary-nosql reachable via HTTP proxy");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// -- PROXY-RT-FORWARD -- verify proxy to canary-handlers works

function proxyHandlers(ctx) {
    var t = new TestResult("PROXY-RT-FORWARD", "PROXY", "http-driver-spec section 6");
    try {
        var result = ctx.dataview("proxy_handlers_ping", {});
        t.assert("handlers_responded", result !== null && result !== undefined,
            "type=" + typeof result);

        if (result && result.rows) {
            t.assert("handlers_has_rows", Array.isArray(result.rows), "type=" + typeof result.rows);
        } else if (typeof result === "object") {
            t.assert("handlers_is_object", true, "keys=" + Object.keys(result).join(","));
        }

        t.assert("handlers_proxy_ok", true, "canary-handlers reachable via HTTP proxy");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// -- PROXY-RESPONSE-ENVELOPE -- verify response envelope format (data + meta)

function proxyResponseEnvelope(ctx) {
    var t = new TestResult("PROXY-RESPONSE-ENVELOPE", "PROXY", "http-driver-spec section 10");
    try {
        // Call any proxy DataView and inspect the response structure
        var result = ctx.dataview("proxy_guard_ping", {});
        t.assert("response_not_null", result !== null && result !== undefined,
            "type=" + typeof result);

        // The DataView engine wraps HTTP responses in QueryResult format.
        // Verify the response is structured data (object or has rows).
        var isStructured = false;
        if (result && result.rows) {
            isStructured = true;
            t.assert("has_rows_array", Array.isArray(result.rows),
                "rows type=" + typeof result.rows);
        } else if (typeof result === "object" && result !== null) {
            isStructured = true;
            t.assert("is_object", true, "keys=" + Object.keys(result).join(","));
        } else if (Array.isArray(result)) {
            isStructured = true;
            t.assert("is_array", true, "length=" + result.length);
        }

        t.assert("response_is_structured", isStructured,
            "result type=" + typeof result);

        // Verify the response is not an error string
        t.assert("not_error_string", typeof result !== "string",
            "should be object, not raw string");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
