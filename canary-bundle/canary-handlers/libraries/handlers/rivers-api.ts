// Rivers.* global API tests — crypto utilities and logging.
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

// ── RT-CRYPTO-RANDOMHEX — Rivers.crypto.randomHex(16) returns 32 hex chars ──

function cryptoRandomHex(ctx) {
    var t = new TestResult("RT-CRYPTO-RANDOMHEX", "RUNTIME", "processpool section 9.10");
    try {
        var hex = Rivers.crypto.randomHex(16);
        t.assert("returns_string", typeof hex === "string", "type=" + typeof hex);
        t.assertEquals("length_32", 32, hex.length);
        // Verify all characters are valid hex
        var hexPattern = /^[0-9a-f]+$/i;
        t.assert("valid_hex_chars", hexPattern.test(hex), "value=" + hex);

        // Two calls should produce different values
        var hex2 = Rivers.crypto.randomHex(16);
        t.assert("not_deterministic", hex !== hex2,
            "hex1=" + hex + ", hex2=" + hex2);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CRYPTO-HASHPASSWORD — hash then verify roundtrip ──

function cryptoHashPassword(ctx) {
    var t = new TestResult("RT-CRYPTO-HASHPASSWORD", "RUNTIME", "processpool section 9.10");
    try {
        var password = "canary-test-password-" + Date.now();
        var hash = Rivers.crypto.hashPassword(password);

        t.assert("hash_returns_string", typeof hash === "string", "type=" + typeof hash);
        t.assert("hash_not_empty", hash.length > 0, "length=" + hash.length);
        t.assert("hash_not_plaintext", hash !== password, "hash should differ from input");

        // Verify the hash matches the original password
        var verified = Rivers.crypto.verifyPassword(password, hash);
        t.assert("verify_correct_password", verified === true, "verified=" + verified);

        // Verify a wrong password does NOT match
        var wrongVerified = Rivers.crypto.verifyPassword("wrong-password", hash);
        t.assert("reject_wrong_password", wrongVerified === false, "wrongVerified=" + wrongVerified);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CRYPTO-TIMINGSAFE — timingSafeEqual correct for equal/unequal ──

function cryptoTimingSafe(ctx) {
    var t = new TestResult("RT-CRYPTO-TIMINGSAFE", "RUNTIME", "processpool section 9.10");
    try {
        var a = "canary-timing-test-value";
        var b = "canary-timing-test-value";
        var c = "canary-timing-different";

        // Equal strings should return true
        var eqResult = Rivers.crypto.timingSafeEqual(a, b);
        t.assert("equal_strings_true", eqResult === true, "result=" + eqResult);

        // Unequal strings should return false
        var neqResult = Rivers.crypto.timingSafeEqual(a, c);
        t.assert("unequal_strings_false", neqResult === false, "result=" + neqResult);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-LOG-INFO — Rivers.log.info doesn't throw ──

function logInfo(ctx) {
    var t = new TestResult("RT-LOG-INFO", "RUNTIME", "processpool section 9.10");
    try {
        // These should all execute without throwing
        Rivers.log.info("canary log test — info level", { test_id: "RT-LOG-INFO" });
        t.assert("info_no_throw", true, "Rivers.log.info executed");

        Rivers.log.warn("canary log test — warn level", { test_id: "RT-LOG-INFO" });
        t.assert("warn_no_throw", true, "Rivers.log.warn executed");

        Rivers.log.error("canary log test — error level", { test_id: "RT-LOG-INFO" });
        t.assert("error_no_throw", true, "Rivers.log.error executed");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
