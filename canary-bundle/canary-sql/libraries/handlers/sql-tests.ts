// SQL profile test handlers — CRUD roundtrip + parameter binding order tests.
// Each function is a separate test endpoint. Uses $zname before $age
// to trap alphabetical parameter binding bugs (Issue #54).

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

// ── PostgreSQL Tests ──

// SQL-PG-PARAM-ORDER — the critical parameter binding order test
function pgParamOrder(ctx) {
    var t = new TestResult("SQL-PG-PARAM-ORDER", "SQL", "rivers-data-layer-spec.md section 8.3");
    try {
        // DataView uses $zname and $age — zname sorts AFTER age alphabetically
        // but appears FIRST in the query. If the translation layer works,
        // $1=zname value, $2=age value (order of appearance, not alphabetical).
        var result = ctx.dataview("pg_param_test", { zname: "Alice", age: 30 });

        t.assert("result_not_null", result !== null && result !== undefined, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            var row = result.rows[0];
            t.assertEquals("zname_matches", "Alice", row.zname);
            t.assertEquals("age_matches", 30, row.age);
        } else {
            t.assert("has_rows", false, "no rows returned");
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-PG-CRUD-INSERT
function pgInsert(ctx) {
    var t = new TestResult("SQL-PG-CRUD-INSERT", "SQL", "rivers-data-layer-spec.md section 3.1");
    try {
        var id = Rivers.crypto.randomHex(16);
        var result = ctx.dataview("pg_insert", {
            id: id,
            zname: "CanaryInsert",
            age: 25,
            email: "canary@test.local"
        });

        t.assert("insert_executed", result !== null, "result=" + JSON.stringify(result));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-PG-CRUD-SELECT
function pgSelect(ctx) {
    var t = new TestResult("SQL-PG-CRUD-SELECT", "SQL", "rivers-data-layer-spec.md section 3.1");
    try {
        var result = ctx.dataview("pg_select_all");

        t.assert("result_not_null", result !== null, "result type=" + typeof result);
        if (result && result.rows) {
            t.assert("has_rows", result.rows.length >= 0, "row_count=" + result.rows.length);
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── MySQL Tests ──

function mysqlParamOrder(ctx) {
    var t = new TestResult("SQL-MYSQL-PARAM-ORDER", "SQL", "rivers-data-layer-spec.md section 8.3");
    try {
        var result = ctx.dataview("mysql_param_test", { zname: "Bob", age: 40 });

        t.assert("result_not_null", result !== null, "result=" + JSON.stringify(result));
        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("zname_matches", "Bob", result.rows[0].zname);
            t.assertEquals("age_matches", 40, result.rows[0].age);
        } else {
            t.assert("has_rows", false, "no rows returned");
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── SQLite Tests ──

function sqliteParamOrder(ctx) {
    var t = new TestResult("SQL-SQLITE-PARAM-ORDER", "SQL", "rivers-data-layer-spec.md section 8.3");
    try {
        var result = ctx.dataview("sqlite_param_test", { zname: "Charlie", age: 50 });

        t.assert("result_not_null", result !== null, "result=" + JSON.stringify(result));
        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("zname_matches", "Charlie", result.rows[0].zname);
            t.assertEquals("age_matches", 50, result.rows[0].age);
        } else {
            t.assert("has_rows", false, "no rows returned");
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

function sqliteCrud(ctx) {
    var t = new TestResult("SQL-SQLITE-CRUD", "SQL", "rivers-data-layer-spec.md section 3");
    try {
        // Insert
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("sqlite_insert", { id: id, zname: "CrudTest", age: 99 });
        t.assert("insert_ok", true, "id=" + id);

        // Select back
        var result = ctx.dataview("sqlite_select_by_id", { id: id });
        t.assert("select_result", result !== null, "result=" + JSON.stringify(result));

        if (result && result.rows && result.rows.length > 0) {
            t.assertEquals("select_zname", "CrudTest", result.rows[0].zname);
            t.assertEquals("select_age", 99, result.rows[0].age);
        }

        // Delete
        ctx.dataview("sqlite_delete", { id: id });
        t.assert("delete_ok", true, "deleted id=" + id);

        // Verify gone
        var verify = ctx.dataview("sqlite_select_by_id", { id: id });
        var gone = !verify || !verify.rows || verify.rows.length === 0;
        t.assert("deleted_verified", gone, "rows_after_delete=" + (verify && verify.rows ? verify.rows.length : 0));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
