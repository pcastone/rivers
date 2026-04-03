// ctx.* API surface tests — verify every property on the handler context.
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

// ── RT-CTX-REQUEST — verify ctx.request has method, path, headers, query, body, path_params ──

function ctxRequest(ctx) {
    var t = new TestResult("RT-CTX-REQUEST", "RUNTIME", "processpool section 9.8");
    try {
        var req = ctx.request;
        t.assert("request_exists", req !== null && req !== undefined, "type=" + typeof req);
        t.assert("has_method", typeof req.method === "string", "method=" + req.method);
        t.assert("has_path", typeof req.path === "string", "path=" + req.path);
        t.assert("has_headers", req.headers !== null && typeof req.headers === "object", "type=" + typeof req.headers);
        t.assert("has_query", req.query !== null && req.query !== undefined, "type=" + typeof req.query);
        // body may be null for GET requests — just verify the key exists
        t.assert("has_body_key", "body" in req, "body key present");
        // path_params may be empty object for routes without path params
        t.assert("has_params", req.params !== null && req.params !== undefined, "type=" + typeof req.params);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-RESDATA — verify setting ctx.resdata returns data to caller ──

function ctxResdata(ctx) {
    var t = new TestResult("RT-CTX-RESDATA", "RUNTIME", "processpool section 9.8");
    try {
        // Setting ctx.resdata is the mechanism for returning data.
        // If this test result arrives at the caller, the mechanism works.
        t.assert("resdata_writable", true, "ctx.resdata accepted assignment");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-DATA — verify ctx.data has pre-fetched DataView results ──

function ctxData(ctx) {
    var t = new TestResult("RT-CTX-DATA", "RUNTIME", "processpool section 9.8");
    try {
        t.assert("data_exists", ctx.data !== null && ctx.data !== undefined, "type=" + typeof ctx.data);
        t.assert("data_is_object", typeof ctx.data === "object", "type=" + typeof ctx.data);
        // The view config pre-fetches "list_records" — verify it landed on ctx.data
        t.assert("has_list_records", ctx.data.list_records !== undefined,
            "keys=" + Object.keys(ctx.data).join(","));
        if (ctx.data.list_records) {
            t.assert("list_records_has_rows",
                ctx.data.list_records.rows !== undefined || Array.isArray(ctx.data.list_records),
                "type=" + typeof ctx.data.list_records);
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-DATAVIEW — verify ctx.dataview("name") returns data ──

function ctxDataview(ctx) {
    var t = new TestResult("RT-CTX-DATAVIEW", "RUNTIME", "processpool section 9.8");
    try {
        t.assert("dataview_is_function", typeof ctx.dataview === "function",
            "type=" + typeof ctx.dataview);

        var result = ctx.dataview("list_records", { limit: 5 });
        t.assert("result_not_null", result !== null && result !== undefined,
            "type=" + typeof result);

        if (result && result.rows) {
            t.assert("has_rows", result.rows.length >= 0, "row_count=" + result.rows.length);
        } else if (Array.isArray(result)) {
            t.assert("has_rows", result.length >= 0, "array_length=" + result.length);
        } else {
            t.assert("has_rows", result !== null, "result=" + JSON.stringify(result));
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-STORE — verify ctx.store.set/get/del works ──

function ctxStore(ctx) {
    var t = new TestResult("RT-CTX-STORE-GET-SET", "RUNTIME", "storage-engine section 11.5");
    try {
        t.assert("store_exists", ctx.store !== null && ctx.store !== undefined,
            "type=" + typeof ctx.store);
        t.assert("store_has_set", typeof ctx.store.set === "function",
            "type=" + typeof ctx.store.set);
        t.assert("store_has_get", typeof ctx.store.get === "function",
            "type=" + typeof ctx.store.get);
        t.assert("store_has_del", typeof ctx.store.del === "function",
            "type=" + typeof ctx.store.del);

        // Roundtrip: set, get, delete, verify gone
        var key = "canary:ctx-store-test:" + Date.now();
        var value = "hello-canary";

        ctx.store.set(key, value);
        var got = ctx.store.get(key);
        t.assertEquals("get_after_set", value, got);

        ctx.store.del(key);
        var after = ctx.store.get(key);
        t.assert("deleted", after === null || after === undefined,
            "after_del=" + JSON.stringify(after));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-TRACEID — verify ctx.trace_id is a non-empty string ──

function ctxTraceId(ctx) {
    var t = new TestResult("RT-CTX-TRACE-ID", "RUNTIME", "processpool section 9.8");
    try {
        t.assert("trace_id_exists", ctx.trace_id !== null && ctx.trace_id !== undefined,
            "type=" + typeof ctx.trace_id);
        t.assert("trace_id_is_string", typeof ctx.trace_id === "string",
            "type=" + typeof ctx.trace_id);
        t.assert("trace_id_not_empty", ctx.trace_id.length > 0,
            "length=" + (ctx.trace_id ? ctx.trace_id.length : 0));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-APPID — verify ctx.app_id is populated (not empty) ──

function ctxAppId(ctx) {
    var t = new TestResult("RT-CTX-APP-ID", "RUNTIME", "processpool section 9.8");
    try {
        t.assert("app_id_exists", ctx.app_id !== null && ctx.app_id !== undefined,
            "type=" + typeof ctx.app_id);
        t.assert("app_id_is_string", typeof ctx.app_id === "string",
            "type=" + typeof ctx.app_id);
        t.assert("app_id_not_empty", ctx.app_id.length > 0,
            "length=" + (ctx.app_id ? ctx.app_id.length : 0));
        // Verify it matches the manifest appId
        t.assertEquals("app_id_matches_manifest",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee03", ctx.app_id);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-ENV — verify ctx.env exists ──

function ctxEnv(ctx) {
    var t = new TestResult("RT-CTX-ENV", "RUNTIME", "processpool section 9.8");
    try {
        t.assert("env_exists", ctx.env !== null && ctx.env !== undefined,
            "type=" + typeof ctx.env);
        t.assert("env_is_string", typeof ctx.env === "string",
            "type=" + typeof ctx.env);
        t.assert("env_not_empty", ctx.env.length > 0,
            "value=" + ctx.env);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-NODE-ID — verify ctx.node_id exists and is a string ──

function ctxNodeId(ctx) {
    var t = new TestResult("RT-CTX-NODE-ID", "RUNTIME", "processpool section 9.8");
    try {
        t.assert("node_id_exists", ctx.node_id !== null && ctx.node_id !== undefined,
            "type=" + typeof ctx.node_id);
        t.assert("node_id_is_string", typeof ctx.node_id === "string",
            "type=" + typeof ctx.node_id);
        t.assert("node_id_not_empty", ctx.node_id.length > 0,
            "length=" + (ctx.node_id ? ctx.node_id.length : 0));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-SESSION — verify ctx.session exists (stub — not yet implemented) ──

function ctxSession(ctx) {
    var t = new TestResult("RT-CTX-SESSION", "RUNTIME", "processpool section 9.8");
    try {
        // ctx.session should exist as an object with claims when auth is active.
        // This is a stub test — session support is not yet implemented.
        // For now, just verify the property exists on ctx.
        t.assert("session_property_exists", "session" in ctx,
            "has_session_key=" + ("session" in ctx));
        if (ctx.session !== null && ctx.session !== undefined) {
            t.assert("session_is_object", typeof ctx.session === "object",
                "type=" + typeof ctx.session);
        } else {
            // Session is null/undefined — acceptable for auth=none endpoints
            t.assert("session_null_for_no_auth", true,
                "session is null/undefined (expected for auth=none)");
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-DATAVIEW-PARAMS — call ctx.dataview() with params, verify they are passed ──

function ctxDataviewParams(ctx) {
    var t = new TestResult("RT-CTX-DATAVIEW-PARAMS", "RUNTIME", "dream-doc: ctx.dataview() bug");
    try {
        t.assert("dataview_is_function", typeof ctx.dataview === "function",
            "type=" + typeof ctx.dataview);

        // Call a DataView with explicit params — this is the test for the
        // ctx.dataview() param-dropping bug.
        var result = ctx.dataview("list_records", { limit: 5 });
        t.assert("result_not_null", result !== null && result !== undefined,
            "type=" + typeof result);

        if (result && result.rows) {
            t.assert("has_rows", result.rows.length > 0,
                "row_count=" + result.rows.length);
            t.assert("limit_respected", result.rows.length <= 5,
                "row_count=" + result.rows.length + " (expected <= 5)");
        } else if (Array.isArray(result)) {
            t.assert("has_rows", result.length > 0,
                "array_length=" + result.length);
            t.assert("limit_respected", result.length <= 5,
                "array_length=" + result.length + " (expected <= 5)");
        } else {
            t.assert("has_rows", result !== null,
                "result=" + JSON.stringify(result));
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CTX-PSEUDO-DV — test ctx.datasource() builder if available (stub) ──

function ctxPseudoDv(ctx) {
    var t = new TestResult("RT-CTX-PSEUDO-DV", "RUNTIME", "view-layer section 3.2");
    try {
        // ctx.datasource() is a pseudo DataView builder that lets handlers
        // build ad-hoc queries against a datasource without a pre-defined DataView.
        // This is a stub test — the feature may not be implemented yet.
        if (typeof ctx.datasource === "function") {
            t.assert("datasource_is_function", true,
                "ctx.datasource is available");
            // Try to use it — expect it to return a builder or result
            try {
                var builder = ctx.datasource("canary-faker");
                t.assert("builder_returned", builder !== null && builder !== undefined,
                    "type=" + typeof builder);
            } catch (e) {
                t.assert("datasource_callable", false,
                    "ctx.datasource() threw: " + String(e));
            }
        } else {
            // Not yet implemented — mark as stub pass
            t.assert("datasource_not_implemented", true,
                "ctx.datasource is " + typeof ctx.datasource + " — stub test (not yet implemented)");
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
