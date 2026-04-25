// Transaction canary tests — exercises Rivers.db.begin/commit/rollback

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

// TXN-COMMIT — begin, insert, commit, verify row exists
function txnCommit(ctx) {
    var t = new TestResult("TXN-COMMIT", "SQL", "rivers-connection-features-spec.md §3");
    try {
        Rivers.db.begin("canary-pg");
        var id = "txn-commit-" + Date.now();
        ctx.dataview("pg_insert", { id: id, zname: "TxnCommit", age: 1 });
        Rivers.db.commit("canary-pg");

        var result = ctx.dataview("pg_select_by_id", { id: id });
        var rowCount = (result && result.rows) ? result.rows.length : 0;
        t.assert("row-exists-after-commit", rowCount > 0, "row_count=" + rowCount);
        return t.finish();
    } catch(e) {
        try { Rivers.db.rollback("canary-pg"); } catch(e2) {}
        return t.fail("" + e);
    }
}

// TXN-ROLLBACK — begin, insert, rollback, verify row gone
function txnRollback(ctx) {
    var t = new TestResult("TXN-ROLLBACK", "SQL", "rivers-connection-features-spec.md §3");
    try {
        var id = "txn-rollback-" + Date.now();
        Rivers.db.begin("canary-pg");
        ctx.dataview("pg_insert", { id: id, zname: "TxnRollback", age: 2 });
        Rivers.db.rollback("canary-pg");

        var result = ctx.dataview("pg_select_by_id", { id: id });
        var rowCount = (result && result.rows) ? result.rows.length : 0;
        t.assert("row-gone-after-rollback", rowCount === 0, "row_count=" + rowCount);
        return t.finish();
    } catch(e) {
        try { Rivers.db.rollback("canary-pg"); } catch(e2) {}
        return t.fail("" + e);
    }
}

// TXN-DOUBLE-BEGIN — begin twice should throw
function txnDoubleBegin(ctx) {
    var t = new TestResult("TXN-DOUBLE-BEGIN", "SQL", "rivers-connection-features-spec.md §3 TXN-2");
    try {
        Rivers.db.begin("canary-pg");
        try {
            Rivers.db.begin("canary-pg");
            t.assert("double-begin-throws", false, "should have thrown");
        } catch(e) {
            t.assert("double-begin-throws", true, "correctly threw: " + e);
        }
        Rivers.db.rollback("canary-pg");
        return t.finish();
    } catch(e) {
        try { Rivers.db.rollback("canary-pg"); } catch(e2) {}
        return t.fail("" + e);
    }
}

// TXN-BATCH — batch insert inside a transaction
function txnBatch(ctx) {
    var t = new TestResult("TXN-BATCH", "SQL", "rivers-connection-features-spec.md §5");
    try {
        Rivers.db.begin("canary-pg");
        var results = Rivers.db.batch("pg_insert", [
            { id: "batch-1-" + Date.now(), zname: "Batch1", age: 10 },
            { id: "batch-2-" + Date.now(), zname: "Batch2", age: 20 }
        ]);
        t.assert("batch-returns-results", results && results.length === 2, "should return 2 results");
        Rivers.db.commit("canary-pg");
        return t.finish();
    } catch(e) {
        try { Rivers.db.rollback("canary-pg"); } catch(e2) {}
        return t.fail("" + e);
    }
}
