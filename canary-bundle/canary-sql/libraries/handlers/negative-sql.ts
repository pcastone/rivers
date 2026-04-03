// Negative SQL tests — DDL rejection, error sanitization.

function TestResult(test_id, profile, spec_ref) {
    this.test_id = test_id; this.profile = profile; this.spec_ref = spec_ref;
    this.assertions = []; this.error = null; this.start = Date.now();
}
TestResult.prototype.assert = function(id, passed, detail) {
    this.assertions.push({ id: id, passed: passed, detail: detail || undefined });
};
TestResult.prototype.finish = function() {
    return { test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: this.assertions.every(function(a) { return a.passed; }),
        assertions: this.assertions, duration_ms: Date.now() - this.start, error: this.error };
};
TestResult.prototype.fail = function(err) {
    this.error = err;
    return { test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: false, assertions: this.assertions, duration_ms: Date.now() - this.start, error: err };
};

// SQL-PG-DDL-REJECT — DDL via execute() is blocked by Gate 1 (PostgreSQL)
function pgDdlReject(ctx) {
    var t = new TestResult("SQL-PG-DDL-REJECT", "SQL", "feature-inventory section 21.1");
    try {
        // This DataView has a DROP TABLE statement — should be rejected
        var threw = false;
        var errMsg = "";
        try {
            ctx.dataview("pg_ddl_trap");
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }

        t.assert("ddl_blocked", threw, "threw=" + threw);
        t.assert("error_contains_forbidden",
            errMsg.toLowerCase().indexOf("forbidden") >= 0 ||
            errMsg.toLowerCase().indexOf("ddl") >= 0,
            "error=" + errMsg.substring(0, 80)
        );
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-MYSQL-DDL-REJECT — DDL via execute() is blocked by Gate 1 (MySQL)
function mysqlDdlReject(ctx) {
    var t = new TestResult("SQL-MYSQL-DDL-REJECT", "SQL", "feature-inventory section 21.1");
    try {
        // Attempt DDL through mysql datasource — should be rejected
        var threw = false;
        var errMsg = "";
        try {
            // Use a dynamic query to attempt DROP TABLE on mysql
            var ds = ctx.datasource("canary-mysql");
            var dv = ds.fromQuery("DROP TABLE canary_records").build();
            ctx.dataview(dv);
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }

        t.assert("ddl_blocked", threw, "threw=" + threw);
        t.assert("error_contains_forbidden",
            errMsg.toLowerCase().indexOf("forbidden") >= 0 ||
            errMsg.toLowerCase().indexOf("ddl") >= 0,
            "error=" + errMsg.substring(0, 80)
        );
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-PG-DDL-REJECT (legacy endpoint) — kept for backward compatibility
function ddlRejected(ctx) {
    var t = new TestResult("SQL-PG-DDL-REJECT", "SQL", "rivers-ddl-security-spec.md section 4");
    try {
        // This DataView has a DROP TABLE statement — should be rejected
        var threw = false;
        var errMsg = "";
        try {
            ctx.dataview("pg_ddl_trap");
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }

        t.assert("ddl_blocked", threw, "threw=" + threw);
        t.assert("error_contains_forbidden",
            errMsg.toLowerCase().indexOf("forbidden") >= 0 ||
            errMsg.toLowerCase().indexOf("ddl") >= 0,
            "error=" + errMsg.substring(0, 80)
        );
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}

// SQL-ERROR-SANITIZED — error responses don't leak driver details in production
function errorSanitized(ctx) {
    var t = new TestResult("SQL-ERROR-SANITIZED", "SQL", "feature-inventory section 21.5");
    try {
        // Deliberately cause a query error
        var threw = false;
        var errMsg = "";
        try {
            ctx.dataview("pg_bad_query");
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }

        t.assert("query_threw", threw, "threw=" + threw);
        // Error message should NOT contain hostname/port/connection string
        if (threw) {
            t.assert("no_hostname_leak",
                errMsg.indexOf("192.168") === -1,
                "checked for IP leak"
            );
            t.assert("no_port_leak",
                errMsg.indexOf("5432") === -1,
                "checked for port leak"
            );
        }
    } catch (e) {
        return t.fail(String(e));
    }
    ctx.resdata = t.finish();
}
