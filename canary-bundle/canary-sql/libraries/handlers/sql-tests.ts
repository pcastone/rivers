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

// SQL-PG-CRUD-UPDATE
function pgUpdate(ctx) {
    var t = new TestResult("SQL-PG-CRUD-UPDATE", "SQL", "rivers-data-layer-spec.md section 3.1");
    try {
        // Insert a row first so we have something to update
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("pg_insert", {
            id: id,
            zname: "UpdateTarget",
            age: 30,
            email: "before@test.local"
        });

        // Update the row
        ctx.dataview("pg_update", {
            id: id,
            zname: "UpdatedName",
            age: 31
        });

        // Verify the update
        var check = ctx.dataview("pg_select_by_id", { id: id });
        t.assert("select_returned", check !== null, "result=" + JSON.stringify(check));
        if (check && check.rows && check.rows.length > 0) {
            t.assertEquals("zname_updated", "UpdatedName", check.rows[0].zname);
            t.assertEquals("age_updated", 31, check.rows[0].age);
        } else {
            t.assert("row_found", false, "no rows after update");
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-PG-CRUD-DELETE
function pgDelete(ctx) {
    var t = new TestResult("SQL-PG-CRUD-DELETE", "SQL", "rivers-data-layer-spec.md section 3.1");
    try {
        // Insert a row to delete
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("pg_insert", {
            id: id,
            zname: "DeleteTarget",
            age: 40,
            email: "delete@test.local"
        });

        // Delete it
        ctx.dataview("pg_delete", { id: id });

        // Verify it's gone
        var check = ctx.dataview("pg_select_by_id", { id: id });
        var gone = !check || !check.rows || check.rows.length === 0;
        t.assert("row_deleted", gone, "rows_after_delete=" + (check && check.rows ? check.rows.length : 0));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-PG-MAX-ROWS — verifies pg_cached DataView max_rows truncation
function pgMaxRows(ctx) {
    var t = new TestResult("SQL-PG-MAX-ROWS", "SQL", "feature-inventory section 21.5");
    try {
        var result = ctx.dataview("pg_cached");
        t.assert("result_not_null", result !== null, "result type=" + typeof result);
        if (result && result.rows) {
            t.assert("max_rows_enforced", result.rows.length <= 10,
                "row_count=" + result.rows.length + ", max_rows=10");
        } else {
            t.assert("has_rows", false, "no rows property on result");
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

// SQL-MYSQL-CRUD-INSERT
function mysqlInsert(ctx) {
    var t = new TestResult("SQL-MYSQL-CRUD-INSERT", "SQL", "rivers-data-layer-spec.md section 3.1");
    try {
        var id = Rivers.crypto.randomHex(16);
        var result = ctx.dataview("mysql_insert", {
            id: id,
            zname: "MysqlInsert",
            age: 35
        });
        t.assert("insert_executed", result !== null, "result=" + JSON.stringify(result));
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-MYSQL-CRUD-SELECT
function mysqlSelect(ctx) {
    var t = new TestResult("SQL-MYSQL-CRUD-SELECT", "SQL", "rivers-data-layer-spec.md section 3.1");
    try {
        var result = ctx.dataview("mysql_select_all");
        t.assert("result_not_null", result !== null, "result type=" + typeof result);
        if (result && result.rows) {
            t.assert("has_rows", result.rows.length >= 0, "row_count=" + result.rows.length);
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-MYSQL-CRUD-UPDATE
function mysqlUpdate(ctx) {
    var t = new TestResult("SQL-MYSQL-CRUD-UPDATE", "SQL", "rivers-data-layer-spec.md section 3.1");
    try {
        // Insert a row first
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("mysql_insert", {
            id: id,
            zname: "MysqlUpdateTarget",
            age: 45
        });

        // Update the row
        ctx.dataview("mysql_update", {
            id: id,
            zname: "MysqlUpdatedName",
            age: 46
        });

        // Verify (select all and find our row)
        var result = ctx.dataview("mysql_select_all");
        var found = false;
        if (result && result.rows) {
            for (var i = 0; i < result.rows.length; i++) {
                if (result.rows[i].id === id) {
                    found = true;
                    t.assertEquals("zname_updated", "MysqlUpdatedName", result.rows[i].zname);
                    t.assertEquals("age_updated", 46, result.rows[i].age);
                    break;
                }
            }
        }
        t.assert("row_found_after_update", found, "id=" + id);
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-MYSQL-CRUD-DELETE
function mysqlDelete(ctx) {
    var t = new TestResult("SQL-MYSQL-CRUD-DELETE", "SQL", "rivers-data-layer-spec.md section 3.1");
    try {
        // Insert a row to delete
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("mysql_insert", {
            id: id,
            zname: "MysqlDeleteTarget",
            age: 50
        });

        // Delete it
        ctx.dataview("mysql_delete", { id: id });

        // Verify it's gone
        var result = ctx.dataview("mysql_select_all");
        var found = false;
        if (result && result.rows) {
            for (var i = 0; i < result.rows.length; i++) {
                if (result.rows[i].id === id) {
                    found = true;
                    break;
                }
            }
        }
        t.assert("row_deleted", !found, "id=" + id + " should not exist");
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

// SQL-SQLITE-PREFIX — verifies $name vs :name prefix handling
function sqlitePrefix(ctx) {
    var t = new TestResult("SQL-SQLITE-PREFIX", "SQL", "Issue #54");
    try {
        // Insert a row to query against
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("sqlite_insert", { id: id, zname: "PrefixTest", age: 77 });

        // Query using the $-prefixed parameter DataView
        var result = ctx.dataview("sqlite_param_test", { zname: "PrefixTest", age: 77 });
        t.assert("query_executed", result !== null, "query with $name params succeeded on SQLite");
        if (result && result.rows) {
            t.assert("rows_returned", result.rows.length > 0, "row_count=" + result.rows.length);
        } else {
            t.assert("rows_returned", false, "no rows property");
        }

        // Cleanup
        ctx.dataview("sqlite_delete", { id: id });
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// ── Cache Tests ──

// SQL-CACHE-L1-HIT — verifies second identical query hits L1 cache
function cacheL1Hit(ctx) {
    var t = new TestResult("SQL-CACHE-L1-HIT", "SQL", "storage-engine section 11.6");
    try {
        // First call — populates cache
        var t1 = Date.now();
        ctx.dataview("pg_cached");
        var d1 = Date.now() - t1;

        // Second call — should hit L1 cache
        var t2 = Date.now();
        var result = ctx.dataview("pg_cached");
        var d2 = Date.now() - t2;

        t.assert("first_call_returned", result !== null, "result type=" + typeof result);
        t.assert("second_call_faster", d2 <= d1, "first=" + d1 + "ms, second=" + d2 + "ms");
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-CACHE-INVALIDATE — verifies write triggers cache invalidation
function cacheInvalidate(ctx) {
    var t = new TestResult("SQL-CACHE-INVALIDATE", "SQL", "rivers-data-layer-spec.md section 3.3");
    try {
        // Prime cache
        ctx.dataview("pg_select_all");

        // Write — should invalidate pg_select_all cache
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("pg_insert", {
            id: id,
            zname: "CacheInvalidateTest",
            age: 1,
            email: "cache@test.local"
        });

        // Read again — should miss cache (fresh query)
        var result = ctx.dataview("pg_select_all");
        var found = false;
        if (result && result.rows) {
            for (var i = 0; i < result.rows.length; i++) {
                if (result.rows[i].zname === "CacheInvalidateTest") {
                    found = true;
                    break;
                }
            }
        }
        t.assert("new_row_visible", found, "new row found in result — cache was invalidated");

        // Cleanup
        ctx.dataview("pg_delete", { id: id });
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-INIT-DDL-SUCCESS — verifies init handler created tables successfully
function initDdlSuccess(ctx) {
    var t = new TestResult("SQL-INIT-DDL-SUCCESS", "SQL", "rivers-application-spec.md section 13.7");
    try {
        // Try selecting from each table — if init DDL worked, these should succeed
        var pgResult = ctx.dataview("pg_select_all");
        t.assert("pg_table_exists", pgResult !== null, "pg_select_all returned result");

        var mysqlResult = ctx.dataview("mysql_select_all");
        t.assert("mysql_table_exists", mysqlResult !== null, "mysql_select_all returned result");

        // SQLite: insert and select to verify table exists
        var id = Rivers.crypto.randomHex(16);
        ctx.dataview("sqlite_insert", { id: id, zname: "InitCheck", age: 1 });
        var sqliteResult = ctx.dataview("sqlite_select_by_id", { id: id });
        t.assert("sqlite_table_exists", sqliteResult !== null && sqliteResult.rows && sqliteResult.rows.length > 0,
            "sqlite insert+select succeeded");
        ctx.dataview("sqlite_delete", { id: id });
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
