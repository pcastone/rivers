// V8 sandbox hardening tests — timeout, code generation blocking, error sanitization.
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

// ── RT-V8-TIMEOUT — verify the V8 watchdog terminates infinite loops ──
// This handler deliberately runs while(true){} to test the watchdog.
// The V8 watchdog thread must terminate it within task_timeout_ms.
// If the watchdog fires, the Rust side catches the timeout and returns an error.
// The handler catches the error and reports it as a pass.

function v8Timeout(ctx) {
    var t = new TestResult("RT-V8-TIMEOUT", "RUNTIME", "feature-inventory section 21.2");
    try {
        // Run an infinite loop — the V8 watchdog should terminate this.
        // If we somehow exit the loop, the timeout did NOT fire.
        while (true) {}
        // If we reach here, the watchdog failed
        t.assert("timeout_enforced", false,
            "infinite loop completed — V8 timeout not enforced");
    } catch (e) {
        // The watchdog terminated the loop and threw an error — this is the pass case
        t.assert("timeout_enforced", true,
            "watchdog terminated infinite loop: " + String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-V8-CODEGEN — verify Function constructor from string is blocked ──
// SECURITY NOTE: This test INTENTIONALLY attempts to use blocked APIs
// (Function constructor, eval) to verify the V8 sandbox rejects them.
// These calls are expected to throw — that IS the passing condition.

function v8CodeGenBlocked(ctx) {
    var t = new TestResult("RT-V8-CODEGEN", "RUNTIME", "feature-inventory section 21.2");
    try {
        // Function() constructor from string must be blocked by V8 sandbox
        var functionThrew = false;
        var functionErr = "";
        try {
            // Deliberately testing that this is BLOCKED — expected to throw
            var FnConstructor = Function;
            var fn = FnConstructor("return 1 + 1");
            fn();
        } catch (e) {
            functionThrew = true;
            functionErr = String(e);
        }
        t.assert("function_constructor_blocked", functionThrew,
            "threw=" + functionThrew + ", err=" + functionErr.substring(0, 80));

        // eval() must also be blocked by V8 sandbox
        var evalThrew = false;
        var evalErr = "";
        try {
            // Deliberately testing that eval is BLOCKED — expected to throw
            var evalFn = eval;
            evalFn("1 + 1");
        } catch (e) {
            evalThrew = true;
            evalErr = String(e);
        }
        t.assert("eval_blocked", evalThrew,
            "threw=" + evalThrew + ", err=" + evalErr.substring(0, 80));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-V8-HEAP — allocate large array, expect graceful error ──

function v8Heap(ctx) {
    var t = new TestResult("RT-V8-HEAP", "RUNTIME", "feature-inventory section 21.2");
    try {
        // Allocate massive arrays to trigger NearHeapLimitCallback.
        // Must NOT crash the process — must terminate the handler gracefully.
        var arrays = [];
        for (var i = 0; i < 1000000; i++) {
            arrays.push(new Array(100000));
        }
        t.assert("heap_limit_enforced", false,
            "massive allocation succeeded — heap limit not enforced");
    } catch (e) {
        t.assert("heap_limit_enforced", true,
            "threw: " + String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-V8-CONSOLE — verify console delegates to Rivers.log ──

function v8Console(ctx) {
    var t = new TestResult("RT-V8-CONSOLE", "RUNTIME", "processpool section 9.1");
    try {
        // Rivers provides console as a convenience that delegates to Rivers.log.
        t.assert("console_available",
            typeof console === "object" && typeof console.log === "function",
            "typeof console=" + typeof console);
        t.assert("console_has_warn", typeof console.warn === "function",
            "typeof console.warn=" + typeof console.warn);
        t.assert("console_has_error", typeof console.error === "function",
            "typeof console.error=" + typeof console.error);
        // Verify it doesn't throw
        console.log("canary console test");
        t.assert("console_log_callable", true, "console.log executed without error");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-ERROR-SANITIZE — trigger an error, verify response doesn't contain infra details ──

function errorSanitized(ctx) {
    var t = new TestResult("RT-ERROR-SANITIZE", "RUNTIME", "feature-inventory section 21.5");
    try {
        // Deliberately trigger an error by calling a nonexistent DataView
        var threw = false;
        var errMsg = "";
        try {
            ctx.dataview("nonexistent_dataview_canary_xyz");
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }

        t.assert("error_thrown", threw, "threw=" + threw);

        if (threw) {
            // Error message should NOT contain infrastructure details
            t.assert("no_hostname_leak",
                errMsg.indexOf("127.0.0.1") === -1 && errMsg.indexOf("0.0.0.0") === -1,
                "checked for IP leak");
            t.assert("no_stack_trace_leak",
                errMsg.indexOf("at ") === -1 || errMsg.indexOf(".rs:") === -1,
                "checked for Rust stack trace leak");
            t.assert("no_file_path_leak",
                errMsg.indexOf("/src/") === -1 && errMsg.indexOf("crates/") === -1,
                "checked for file path leak");
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
