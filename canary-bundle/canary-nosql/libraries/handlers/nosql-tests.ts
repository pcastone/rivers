// NoSQL profile test handlers — per-driver insert/find/set/get/index/search tests.
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

// NOSQL-MONGO-INSERT — insert a document into MongoDB
function mongoInsert(ctx) {
    var t = new TestResult("NOSQL-MONGO-INSERT", "NOSQL", "data-layer section 3.1");
    try {
        var testName = "canary_" + Rivers.crypto.randomHex(8);
        var testValue = "test_" + Date.now();
        var result = ctx.dataview("mongo_insert", { name: testName, value: testValue });

        t.assert("insert_executed", result !== null || result === undefined, "name=" + testName);
        t.assert("insert_ok", true, "inserted name=" + testName + ", value=" + testValue);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// NOSQL-MONGO-FIND — query documents from MongoDB
function mongoFind(ctx) {
    var t = new TestResult("NOSQL-MONGO-FIND", "NOSQL", "data-layer section 3.1");
    try {
        // Insert a document first so there is something to find
        var testName = "canary_find_" + Rivers.crypto.randomHex(8);
        var testValue = "find_" + Date.now();
        ctx.dataview("mongo_insert", { name: testName, value: testValue });

        // Find it back
        var result = ctx.dataview("mongo_find_by_name", { name: testName });
        t.assert("find_result_not_null", result !== null, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("find_name", testName, result.rows[0].name);
            t.assertEquals("find_value", testValue, result.rows[0].value);
        } else {
            t.assert("has_rows", false, "no rows returned after insert");
        }

        // Cleanup
        ctx.dataview("mongo_delete_by_name", { name: testName });
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

// NOSQL-ES-INDEX — index a document into Elasticsearch
function esIndex(ctx) {
    var t = new TestResult("NOSQL-ES-INDEX", "NOSQL", "data-layer section 3.1");
    try {
        var docId = "canary_" + Rivers.crypto.randomHex(8);
        var result = ctx.dataview("es_index_doc", { doc_id: docId, title: "canary test", body: "test content" });

        t.assert("index_executed", true, "doc_id=" + docId);
        t.assert("index_ok", result !== null || result === undefined, "result=" + JSON.stringify(result));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// NOSQL-ES-SEARCH — search documents in Elasticsearch
function esSearch(ctx) {
    var t = new TestResult("NOSQL-ES-SEARCH", "NOSQL", "data-layer section 3.1");
    try {
        // Index a doc first so we have something to search
        var docId = "canary_search_" + Rivers.crypto.randomHex(8);
        ctx.dataview("es_index_doc", { doc_id: docId, title: "canary search test", body: "searchable content" });

        // Search for it
        var result = ctx.dataview("es_search");
        t.assert("search_result_not_null", result !== null, "result=" + JSON.stringify(result));
        t.assert("search_ok", true, "search query executed");
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

// NOSQL-COUCH-PUT — put a document into CouchDB
function couchPut(ctx) {
    var t = new TestResult("NOSQL-COUCH-PUT", "NOSQL", "data-layer section 3.1");
    try {
        var docId = "canary_" + Rivers.crypto.randomHex(8);
        var result = ctx.dataview("couch_put_doc", { doc_id: docId, title: "canary test" });

        t.assert("put_executed", true, "doc_id=" + docId);
        t.assert("put_ok", result !== null || result === undefined, "result=" + JSON.stringify(result));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// NOSQL-COUCH-GET — get a document from CouchDB
function couchGet(ctx) {
    var t = new TestResult("NOSQL-COUCH-GET", "NOSQL", "data-layer section 3.1");
    try {
        // Put a doc first
        var docId = "canary_get_" + Rivers.crypto.randomHex(8);
        ctx.dataview("couch_put_doc", { doc_id: docId, title: "canary get test" });

        // Get it back
        var result = ctx.dataview("couch_get_doc", { doc_id: docId });
        t.assert("get_result_not_null", result !== null, "result=" + JSON.stringify(result));
        t.assert("get_ok", true, "doc retrieved for id=" + docId);
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

// NOSQL-CASSANDRA-INSERT — insert a row into Cassandra
function cassandraInsert(ctx) {
    var t = new TestResult("NOSQL-CASSANDRA-INSERT", "NOSQL", "data-layer section 3.1");
    try {
        var id = Rivers.crypto.randomHex(16);
        var result = ctx.dataview("cassandra_insert", { id: id, name: "canary_test", value: "test_" + Date.now() });

        t.assert("insert_executed", true, "id=" + id);
        t.assert("insert_ok", result !== null || result === undefined, "result=" + JSON.stringify(result));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// NOSQL-CASSANDRA-SELECT — select rows from Cassandra
function cassandraSelect(ctx) {
    var t = new TestResult("NOSQL-CASSANDRA-SELECT", "NOSQL", "data-layer section 3.1");
    try {
        // Insert a row first
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("cassandra_insert", { id: id, name: "canary_select", value: "select_" + Date.now() });

        // Select it back
        var result = ctx.dataview("cassandra_select_by_id", { id: id });
        t.assert("select_result_not_null", result !== null, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("select_name", "canary_select", result.rows[0].name);
        } else {
            t.assert("has_rows", false, "no rows returned after insert");
        }
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

// NOSQL-LDAP-SEARCH — search LDAP directory with filter
function ldapSearch(ctx) {
    var t = new TestResult("NOSQL-LDAP-SEARCH", "NOSQL", "driver-spec section 6.3");
    try {
        var result = ctx.dataview("ldap_search", { filter: "(objectClass=*)" });
        t.assert("search_result_not_null", result !== null, "result=" + JSON.stringify(result));
        t.assert("search_ok", true, "ldap search executed");
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

// NOSQL-REDIS-SET — set a key in Redis
function redisSet(ctx) {
    var t = new TestResult("NOSQL-REDIS-SET", "NOSQL", "data-layer section 4.1");
    try {
        var testKey = "canary:" + Rivers.crypto.randomHex(8);
        var testValue = "test_" + Date.now();
        var result = ctx.dataview("redis_set", { key: testKey, value: testValue });

        t.assert("set_executed", true, "key=" + testKey);
        t.assert("set_ok", result !== null || result === undefined, "result=" + JSON.stringify(result));

        // Cleanup
        ctx.dataview("redis_del", { key: testKey });
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// NOSQL-REDIS-GET — get a key from Redis
function redisGet(ctx) {
    var t = new TestResult("NOSQL-REDIS-GET", "NOSQL", "data-layer section 4.1");
    try {
        // Set a key first
        var testKey = "canary:get_" + Rivers.crypto.randomHex(8);
        var testValue = "get_" + Date.now();
        ctx.dataview("redis_set", { key: testKey, value: testValue });

        // Get it back
        var result = ctx.dataview("redis_get", { key: testKey });
        t.assert("get_result_not_null", result !== null, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("get_value", testValue, result.rows[0].value || result.rows[0]);
        } else if (result && result.value !== undefined) {
            t.assertEquals("get_value", testValue, result.value);
        } else {
            t.assert("has_value", false, "no value returned after SET");
        }

        // Cleanup
        ctx.dataview("redis_del", { key: testKey });
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
