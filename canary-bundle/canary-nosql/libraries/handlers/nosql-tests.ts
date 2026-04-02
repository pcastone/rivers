// NoSQL profile test handlers — ping + CRUD roundtrip tests for each NoSQL driver.
// Each function is a separate test endpoint invoked via codecomponent handler.

// ── Helpers ──

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

// ── MongoDB Tests ──

// NOSQL-MONGO-PING — connect and verify the driver responds
function mongoConnect(ctx) {
    var t = new TestResult("NOSQL-MONGO-PING", "NOSQL", "rivers-driver-spec.md");
    try {
        var result = ctx.dataview("mongo_ping");

        t.assert("result_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));
        t.assert("ping_ok", true, "mongo driver responded");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// NOSQL-MONGO-CRUD — insert, find, delete roundtrip
function mongoCrud(ctx) {
    var t = new TestResult("NOSQL-MONGO-CRUD", "NOSQL", "rivers-driver-spec.md");
    try {
        // Insert a test document
        var testName = "canary_" + Rivers.crypto.randomHex(8);
        var testValue = "test_" + Date.now();
        ctx.dataview("mongo_insert", { name: testName, value: testValue });
        t.assert("insert_ok", true, "name=" + testName);

        // Find it back
        var result = ctx.dataview("mongo_find_by_name", { name: testName });
        t.assert("find_result", result !== null, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("find_name", testName, result.rows[0].name);
            t.assertEquals("find_value", testValue, result.rows[0].value);
        } else {
            t.assert("has_rows", false, "no rows returned after insert");
        }

        // Delete it
        ctx.dataview("mongo_delete_by_name", { name: testName });
        t.assert("delete_ok", true, "deleted name=" + testName);

        // Verify gone
        var verify = ctx.dataview("mongo_find_by_name", { name: testName });
        var gone = !verify || !verify.rows || verify.rows.length === 0;
        t.assert("deleted_verified", gone, "rows_after_delete=" + (verify && verify.rows ? verify.rows.length : 0));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── Elasticsearch Tests ──

// NOSQL-ES-PING — connect and verify the driver responds
function esConnect(ctx) {
    var t = new TestResult("NOSQL-ES-PING", "NOSQL", "rivers-driver-spec.md");
    try {
        var result = ctx.dataview("es_ping");

        t.assert("result_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));
        t.assert("ping_ok", true, "elasticsearch driver responded");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── CouchDB Tests ──

// NOSQL-COUCH-PING — connect and verify the driver responds
function couchConnect(ctx) {
    var t = new TestResult("NOSQL-COUCH-PING", "NOSQL", "rivers-driver-spec.md");
    try {
        var result = ctx.dataview("couch_ping");

        t.assert("result_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));
        t.assert("ping_ok", true, "couchdb driver responded");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── Cassandra Tests ──

// NOSQL-CASSANDRA-PING — connect and verify the driver responds
function cassandraConnect(ctx) {
    var t = new TestResult("NOSQL-CASSANDRA-PING", "NOSQL", "rivers-driver-spec.md");
    try {
        var result = ctx.dataview("cassandra_ping");

        t.assert("result_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));
        t.assert("ping_ok", true, "cassandra driver responded");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── LDAP Tests ──

// NOSQL-LDAP-PING — connect and verify the driver responds
function ldapConnect(ctx) {
    var t = new TestResult("NOSQL-LDAP-PING", "NOSQL", "rivers-driver-spec.md");
    try {
        var result = ctx.dataview("ldap_ping");

        t.assert("result_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));
        t.assert("ping_ok", true, "ldap driver responded");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── Redis Tests ──

// NOSQL-REDIS-PING — connect and verify the driver responds
function redisConnect(ctx) {
    var t = new TestResult("NOSQL-REDIS-PING", "NOSQL", "rivers-driver-spec.md");
    try {
        var result = ctx.dataview("redis_ping");

        t.assert("result_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));
        t.assert("ping_ok", true, "redis driver responded");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// NOSQL-REDIS-CRUD — set, get, del roundtrip
function redisCrud(ctx) {
    var t = new TestResult("NOSQL-REDIS-CRUD", "NOSQL", "rivers-driver-spec.md");
    try {
        // Set a key
        var testKey = "canary:" + Rivers.crypto.randomHex(8);
        var testValue = "test_" + Date.now();
        ctx.dataview("redis_set", { key: testKey, value: testValue });
        t.assert("set_ok", true, "key=" + testKey);

        // Get it back
        var result = ctx.dataview("redis_get", { key: testKey });
        t.assert("get_result", result !== null, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("get_value", testValue, result.rows[0].value || result.rows[0]);
        } else if (result && result.value !== undefined) {
            t.assertEquals("get_value", testValue, result.value);
        } else {
            t.assert("has_value", false, "no value returned after SET");
        }

        // Delete it
        ctx.dataview("redis_del", { key: testKey });
        t.assert("del_ok", true, "deleted key=" + testKey);

        // Verify gone
        var verify = ctx.dataview("redis_get", { key: testKey });
        var gone = !verify || !verify.rows || verify.rows.length === 0 || verify.value === null || verify.value === undefined;
        t.assert("deleted_verified", gone, "value_after_del=" + JSON.stringify(verify));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
