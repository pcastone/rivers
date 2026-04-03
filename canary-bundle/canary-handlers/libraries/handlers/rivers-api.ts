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

// ── RT-RIVERS-CRYPTO-RANDOM — Rivers.crypto.randomHex + randomBase64url ──

function cryptoRandomHex(ctx) {
    var t = new TestResult("RT-RIVERS-CRYPTO-RANDOM", "RUNTIME", "processpool section 9.10");
    try {
        // Test randomHex
        var hex = Rivers.crypto.randomHex(16);
        t.assert("hex_returns_string", typeof hex === "string", "type=" + typeof hex);
        t.assertEquals("hex_length_32", 32, hex.length);
        // Verify all characters are valid hex
        var hexPattern = /^[0-9a-f]+$/i;
        t.assert("hex_valid_chars", hexPattern.test(hex), "value=" + hex);

        // Two calls should produce different values
        var hex2 = Rivers.crypto.randomHex(16);
        t.assert("hex_not_deterministic", hex !== hex2,
            "hex1=" + hex + ", hex2=" + hex2);

        // Test randomBase64url
        if (typeof Rivers.crypto.randomBase64url === "function") {
            var b64 = Rivers.crypto.randomBase64url(16);
            t.assert("b64url_returns_string", typeof b64 === "string", "type=" + typeof b64);
            t.assert("b64url_not_empty", b64.length > 0, "length=" + b64.length);
            // Base64url charset: A-Z, a-z, 0-9, -, _
            var b64Pattern = /^[A-Za-z0-9_-]+$/;
            t.assert("b64url_valid_chars", b64Pattern.test(b64), "value=" + b64);
        } else {
            t.assert("b64url_available", false,
                "Rivers.crypto.randomBase64url is " + typeof Rivers.crypto.randomBase64url);
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-CRYPTO-HASHPASSWORD — hash then verify roundtrip ──

function cryptoHashPassword(ctx) {
    var t = new TestResult("RT-RIVERS-CRYPTO-HASH", "RUNTIME", "processpool section 9.10");
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
    var t = new TestResult("RT-RIVERS-CRYPTO-TIMING", "RUNTIME", "processpool section 9.10");
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

// ── RT-RIVERS-LOG — Rivers.log.info/warn/error don't throw ──

function logInfo(ctx) {
    var t = new TestResult("RT-RIVERS-LOG", "RUNTIME", "processpool section 9.10");
    try {
        // These should all execute without throwing
        Rivers.log.info("canary log test — info level", { test_id: "RT-RIVERS-LOG" });
        t.assert("info_no_throw", true, "Rivers.log.info executed");

        Rivers.log.warn("canary log test — warn level", { test_id: "RT-RIVERS-LOG" });
        t.assert("warn_no_throw", true, "Rivers.log.warn executed");

        Rivers.log.error("canary log test — error level", { test_id: "RT-RIVERS-LOG" });
        t.assert("error_no_throw", true, "Rivers.log.error executed");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── RT-RIVERS-CRYPTO-HMAC — Rivers.crypto.hmac produces consistent output ──

function cryptoHmac(ctx) {
    var t = new TestResult("RT-RIVERS-CRYPTO-HMAC", "RUNTIME", "processpool section 9.10");
    try {
        t.assert("hmac_is_function", typeof Rivers.crypto.hmac === "function",
            "type=" + typeof Rivers.crypto.hmac);

        var result = Rivers.crypto.hmac("canary-key", "canary-data");
        t.assert("hmac_returns_string", typeof result === "string",
            "type=" + typeof result);
        t.assert("hmac_not_empty", result.length > 0,
            "length=" + result.length);

        // Same inputs should produce the same HMAC (deterministic)
        var result2 = Rivers.crypto.hmac("canary-key", "canary-data");
        t.assert("hmac_deterministic", result === result2,
            "result1=" + result + ", result2=" + result2);

        // Different key should produce a different HMAC
        var result3 = Rivers.crypto.hmac("different-key", "canary-data");
        t.assert("hmac_key_sensitive", result !== result3,
            "same_key_result=" + result + ", diff_key_result=" + result3);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
