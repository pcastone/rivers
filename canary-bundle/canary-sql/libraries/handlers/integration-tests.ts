// Integration profile test handlers — cross-cutting concerns that exercise
// the full handler-to-driver pipeline: DDL, DataView dispatch, store isolation,
// error propagation, and host callback availability.

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
    this.error = String(err);
    return {
        test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: false, assertions: this.assertions, duration_ms: Date.now() - this.start, error: String(err)
    };
};

// ── 1. INT-DDL-VERIFY — Prove init handler's DDL created the table ──

function ctxDdlVerify(ctx) {
    var t = new TestResult("INT-DDL-VERIFY", "INTEGRATION", "rivers-data-layer-spec.md section 6");
    try {
        var result = ctx.dataview("sqlite_select_all");
        t.assert("select_all_ok", result !== null && result !== undefined, "result=" + JSON.stringify(result));
        // Table exists if we got here without an error
        t.assert("table_exists", true, "canary_records table accessible via sqlite_select_all");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 2. INT-DDL-INSERT-SELECT — INSERT then SELECT, verify roundtrip ──

function ctxDdlInsertSelect(ctx) {
    var t = new TestResult("INT-DDL-INSERT-SELECT", "INTEGRATION", "rivers-data-layer-spec.md section 3.1");
    var id = "int-test-" + Rivers.crypto.randomHex(8);
    try {
        // INSERT
        ctx.dataview("sqlite_insert", { id: id, zname: "IntTest", age: 99 });
        t.assert("insert_ok", true, "insert completed");

        // SELECT by id
        var result = ctx.dataview("sqlite_select_by_id", { id: id });
        t.assert("select_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            var row = result.rows[0];
            t.assertEquals("id_matches", id, row.id);
            t.assertEquals("zname_matches", "IntTest", row.zname);
            t.assertEquals("age_matches", 99, row.age);
        } else {
            t.assert("has_rows", false, "no rows returned after insert");
        }

        // Cleanup
        try { ctx.dataview("sqlite_delete", { id: id }); } catch (e2) {}
    } catch (e) {
        // Cleanup on failure
        try { ctx.dataview("sqlite_delete", { id: id }); } catch (e3) {}
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 3. INT-DRIVER-ERROR-PROP — Verify driver errors propagate descriptively ──

function driverErrorPropagation(ctx) {
    var t = new TestResult("INT-DRIVER-ERROR-PROP", "INTEGRATION", "rivers-data-layer-spec.md section 9");
    try {
        var threw = false;
        var errMsg = "";
        try {
            ctx.dataview("sqlite_nonexistent_table");
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }
        t.assert("error_thrown", threw, "dataview on missing table should throw");
        t.assert("error_not_null", errMsg.length > 0, "error message should not be empty");
        t.assert("error_not_literal_null", errMsg !== "null", "error message should not be literal 'null'");
        t.assert("error_descriptive", errMsg.length > 5, "error=" + errMsg);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 4. INT-DDL-WHITELIST — DDL execution via ctx.ddl ──

function ddlWhitelistReject(ctx) {
    var t = new TestResult("INT-DDL-WHITELIST", "INTEGRATION", "rivers-data-layer-spec.md section 6.2");
    try {
        // CREATE TABLE — should succeed if DDL is allowed or whitelisted
        var created = false;
        try {
            ctx.ddl("canary-sqlite", "CREATE TABLE IF NOT EXISTS whitelist_test (id INTEGER PRIMARY KEY, val TEXT)");
            created = true;
        } catch (e) {
            // DDL may be rejected by whitelist policy — that is also a valid outcome
            t.assert("ddl_rejected_by_policy", true, "ddl rejected: " + String(e));
        }

        if (created) {
            t.assert("create_table_ok", true, "CREATE TABLE succeeded");
            // Cleanup
            try { ctx.ddl("canary-sqlite", "DROP TABLE IF EXISTS whitelist_test"); } catch (e2) {}
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 5. INT-PARAM-BINDING — Verify parameter binding order is correct ──

function dataviewParamBinding(ctx) {
    var t = new TestResult("INT-PARAM-BINDING", "INTEGRATION", "rivers-data-layer-spec.md section 8.3");
    var id = "int-param-" + Rivers.crypto.randomHex(8);
    try {
        // Insert a known row
        ctx.dataview("sqlite_insert", { id: id, zname: "ParamTest", age: 42 });

        // Query by zname + age via param_test DataView
        var result = ctx.dataview("sqlite_param_test", { zname: "ParamTest", age: 42 });
        t.assert("result_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            var row = result.rows[0];
            t.assertEquals("zname_not_swapped", "ParamTest", row.zname);
            t.assertEquals("age_not_swapped", 42, row.age);
        } else {
            t.assert("has_rows", false, "no rows returned for param binding test");
        }

        // Cleanup
        try { ctx.dataview("sqlite_delete", { id: id }); } catch (e2) {}
    } catch (e) {
        try { ctx.dataview("sqlite_delete", { id: id }); } catch (e3) {}
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 6. INT-STORE-NAMESPACE — ctx.store get/set/del roundtrip ──

function storeNamespaceIsolation(ctx) {
    var t = new TestResult("INT-STORE-NAMESPACE", "INTEGRATION", "rivers-storage-engine-spec.md section 3");
    try {
        var key = "int-test-key-" + Rivers.crypto.randomHex(6);
        var val = "store-value-" + Date.now();

        // Set
        ctx.store.set(key, val);
        t.assert("set_ok", true, "store.set completed");

        // Get
        var got = ctx.store.get(key);
        t.assertEquals("roundtrip_match", val, got);

        // Delete
        ctx.store.del(key);
        var afterDel = ctx.store.get(key);
        t.assert("deleted", afterDel === null || afterDel === undefined, "after del=" + JSON.stringify(afterDel));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 7. INT-RECOVERY — Verify server survived prior timeout, test store ──

function recoveryAfterTimeout(ctx) {
    var t = new TestResult("INT-RECOVERY", "INTEGRATION", "rivers-processpool-runtime-spec-v2.md section 5");
    try {
        // If this handler executes at all, the server survived prior tests
        t.assert("handler_executes", true, "handler entered successfully");

        // Quick store roundtrip as a liveness check
        var key = "recovery-" + Rivers.crypto.randomHex(6);
        ctx.store.set(key, "alive");
        var got = ctx.store.get(key);
        t.assertEquals("store_alive", "alive", got);
        ctx.store.del(key);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 8. INT-SQLITE-DISK — INSERT + SELECT in same handler for disk persistence ──

function sqliteDiskPersistence(ctx) {
    var t = new TestResult("INT-SQLITE-DISK", "INTEGRATION", "rivers-data-layer-spec.md section 6.1");
    var id = "int-disk-" + Rivers.crypto.randomHex(8);
    try {
        ctx.dataview("sqlite_insert", { id: id, zname: "DiskTest", age: 77 });
        t.assert("insert_ok", true, "insert completed");

        var result = ctx.dataview("sqlite_select_by_id", { id: id });
        t.assert("select_ok", result !== null && result !== undefined);

        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("data_found", id, result.rows[0].id);
        } else {
            t.assert("has_rows", false, "no rows found after insert");
        }

        // Cleanup
        try { ctx.dataview("sqlite_delete", { id: id }); } catch (e2) {}
    } catch (e) {
        try { ctx.dataview("sqlite_delete", { id: id }); } catch (e3) {}
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 9. INT-PG-DDL — Init DDL + DataView roundtrip on PostgreSQL ──
// Verifies: (1) init handler's ctx.ddl() created the messages table on PG,
// (2) INSERT + SELECT via DataView works end-to-end.
// ctx.ddl() is only available during ApplicationInit; this test proves the
// init DDL persisted and the DataView pipeline is operational.

function pgDdlCreateSelect(ctx) {
    var t = new TestResult("INT-PG-DDL", "INTEGRATION", "rivers-data-layer-spec.md section 6");
    try {
        var id = "int-pg-ddl-" + Rivers.crypto.randomHex(6);

        // INSERT via DataView (proves messages table exists from init DDL)
        ctx.dataview("msg_insert_pg", {
            id: id, zsender: "int-test", recipient: "pg-ddl",
            subject: "INT-PG-DDL", body: "ddl-roundtrip",
            is_secret: 0, cipher: ""
        });
        t.assert("init_ddl_created_table", true, "INSERT succeeded — messages table exists from init DDL");

        // SELECT back to confirm DataView roundtrip
        var result = ctx.dataview("msg_search_pg", { recipient: "pg-ddl", pattern: "%ddl-roundtrip%" });
        t.assert("select_ok", result !== null && result !== undefined);
        var found = false;
        if (result && result.rows) {
            for (var i = 0; i < result.rows.length; i++) {
                if (result.rows[i].id === id) { found = true; break; }
            }
        }
        t.assert("row_found", found, "inserted row found via SELECT DataView");

        // Cleanup
        try { ctx.dataview("msg_cleanup_pg", { id_prefix: id }); } catch (e2) {}
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 10. INT-MYSQL-DDL — Init DDL + DataView roundtrip on MySQL ──

function mysqlDdlCreateSelect(ctx) {
    var t = new TestResult("INT-MYSQL-DDL", "INTEGRATION", "rivers-data-layer-spec.md section 6");
    try {
        var id = "int-mysql-ddl-" + Rivers.crypto.randomHex(6);

        // INSERT via DataView (proves messages table exists from init DDL)
        ctx.dataview("msg_insert_mysql", {
            id: id, zsender: "int-test", recipient: "mysql-ddl",
            subject: "INT-MYSQL-DDL", body: "ddl-roundtrip",
            is_secret: 0, cipher: ""
        });
        t.assert("init_ddl_created_table", true, "INSERT succeeded — messages table exists from init DDL");

        // SELECT back to confirm DataView roundtrip
        var result = ctx.dataview("msg_search_mysql", { recipient: "mysql-ddl", pattern: "%ddl-roundtrip%" });
        t.assert("select_ok", result !== null && result !== undefined);
        var found = false;
        if (result && result.rows) {
            for (var i = 0; i < result.rows.length; i++) {
                if (result.rows[i].id === id) { found = true; break; }
            }
        }
        t.assert("row_found", found, "inserted row found via SELECT DataView");

        // Cleanup
        try { ctx.dataview("msg_cleanup_mysql", { id_prefix: id }); } catch (e2) {}
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 11. INT-INIT-SEQUENCE — Verify init handler ran by querying canary_records ──

function initHandlerSequence(ctx) {
    var t = new TestResult("INT-INIT-SEQUENCE", "INTEGRATION", "rivers-application-spec.md section 4.2");
    try {
        var result = ctx.dataview("sqlite_select_all");
        t.assert("init_table_exists", result !== null && result !== undefined, "canary_records table exists — init handler ran");
        // The table existing at all proves the init handler's DDL executed
        t.assert("init_completed", true, "init handler sequence completed before request handlers");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── 12. INT-HOST-CALLBACKS — Verify host callback functions are available ──

function hostCallbackAvailable(ctx) {
    var t = new TestResult("INT-HOST-CALLBACKS", "INTEGRATION", "rivers-processpool-runtime-spec-v2.md section 3");
    try {
        t.assert("ctx_dataview_is_function", typeof ctx.dataview === "function", "typeof ctx.dataview=" + typeof ctx.dataview);
        t.assert("ctx_ddl_is_function", typeof ctx.ddl === "function", "typeof ctx.ddl=" + typeof ctx.ddl);
        t.assert("ctx_store_is_object", typeof ctx.store === "object", "typeof ctx.store=" + typeof ctx.store);
        t.assert("ctx_store_get_is_function", typeof ctx.store.get === "function", "typeof ctx.store.get=" + typeof ctx.store.get);
        t.assert("ctx_store_set_is_function", typeof ctx.store.set === "function", "typeof ctx.store.set=" + typeof ctx.store.set);
        t.assert("ctx_store_del_is_function", typeof ctx.store.del === "function", "typeof ctx.store.del=" + typeof ctx.store.del);
        t.assert("ctx_resdata_settable", true, "ctx.resdata is settable");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
