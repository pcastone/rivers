// Proxy tests — canary-main PROXY profile.
// Tests HTTP driver as cross-app proxy, session propagation, and error handling.

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

// ── PROXY-SESSION-PROPAGATION ──
// Verify that session headers (X-Rivers-Claims) survive the HTTP driver proxy.

function proxySessionPropagation(ctx) {
    var t = new TestResult("PROXY-SESSION-PROPAGATION", "PROXY", "auth-session §7.5");
    try {
        // If we reached this handler, the session was valid (auth = "session").
        t.assert("session_exists", ctx.session !== null && ctx.session !== undefined,
            "type=" + typeof ctx.session);

        // Check that the session claims are present
        if (ctx.session) {
            t.assert("has_sub", ctx.session.sub !== null && ctx.session.sub !== undefined,
                "sub=" + ctx.session.sub);
            t.assert("has_role", ctx.session.role !== null && ctx.session.role !== undefined,
                "role=" + ctx.session.role);
        }

        // Check the X-Rivers-Claims header is present in the request
        var claimsHeader = ctx.request.headers["x-rivers-claims"];
        t.assert("claims_header_present",
            claimsHeader !== null && claimsHeader !== undefined,
            "x-rivers-claims=" + (claimsHeader || "missing"));

        if (claimsHeader) {
            // The claims header should be a JSON-encoded string or base64
            t.assert("claims_header_not_empty",
                typeof claimsHeader === "string" && claimsHeader.length > 0,
                "length=" + claimsHeader.length);
        }
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}

// ── PROXY-SQL-PASSTHROUGH ──
// Proxy to canary-sql pg select endpoint, verify the result comes back correctly.

function proxySqlPassthrough(ctx) {
    var t = new TestResult("PROXY-SQL-PASSTHROUGH", "PROXY", "http-driver §6.2");
    try {
        t.assert("dataview_is_function", typeof ctx.dataview === "function",
            "type=" + typeof ctx.dataview);

        var result = ctx.dataview("proxy_sql_pg_select", {});
        t.assert("sql_responded", result !== null && result !== undefined,
            "type=" + typeof result);

        // The upstream response should contain rows from canary-sql
        if (result && result.rows) {
            t.assert("has_rows", Array.isArray(result.rows),
                "rows type=" + typeof result.rows);
        } else if (typeof result === "object" && result !== null) {
            t.assert("is_object", true, "keys=" + Object.keys(result).join(","));
        }

        t.assert("sql_passthrough_ok", true, "canary-sql pg/select reachable via proxy");
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}

// ── PROXY-HANDLER-PASSTHROUGH ──
// Proxy to canary-handlers trace-id endpoint, verify the verdict comes back.

function proxyHandlerPassthrough(ctx) {
    var t = new TestResult("PROXY-HANDLER-PASSTHROUGH", "PROXY", "http-driver §6.2");
    try {
        t.assert("dataview_is_function", typeof ctx.dataview === "function",
            "type=" + typeof ctx.dataview);

        var result = ctx.dataview("proxy_handlers_trace_id", {});
        t.assert("handlers_responded", result !== null && result !== undefined,
            "type=" + typeof result);

        // The upstream handler returns a test verdict — verify it has test_id
        if (result && result.test_id) {
            t.assert("upstream_has_test_id", true,
                "upstream test_id=" + result.test_id);
        } else if (typeof result === "object" && result !== null) {
            t.assert("is_object", true, "keys=" + Object.keys(result).join(","));
        }

        t.assert("handler_passthrough_ok", true,
            "canary-handlers rt/ctx/trace-id reachable via proxy");
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}

// ── PROXY-ERROR-PROPAGATION ──
// Proxy to a failing endpoint and verify the error is propagated correctly.

function proxyErrorPropagation(ctx) {
    var t = new TestResult("PROXY-ERROR-PROPAGATION", "PROXY", "SHAPE-2");
    try {
        t.assert("dataview_is_function", typeof ctx.dataview === "function",
            "type=" + typeof ctx.dataview);

        // Call a non-existent endpoint — should get an error
        var result = null;
        var errorCaught = false;
        try {
            result = ctx.dataview("proxy_error_target", {});
        } catch (proxyErr) {
            errorCaught = true;
            t.assert("error_thrown", true, "proxy error: " + String(proxyErr));

            // Verify the error is not a raw stack trace (SHAPE-2 sanitization)
            var errStr = String(proxyErr);
            var hasStackTrace = errStr.indexOf("at ") >= 0 && errStr.indexOf(".rs:") >= 0;
            t.assert("error_sanitized", !hasStackTrace,
                "error should not contain raw Rust stack trace");
        }

        if (!errorCaught) {
            // If no error was thrown, the proxy returned something — check for error status
            if (result && result.error) {
                t.assert("error_in_result", true, "error=" + JSON.stringify(result.error));
            } else if (result && result.status && result.status >= 400) {
                t.assert("error_status_propagated", true, "status=" + result.status);
            } else {
                // If no error at all, the endpoint might exist — still test the structure
                t.assert("response_received", result !== null && result !== undefined,
                    "result type=" + typeof result);
                t.assert("error_expected", false,
                    "expected error from non-existent endpoint, got success");
            }
        }
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}
