// P1.13 — MCP `view = "..."` capability propagation regression probe.
//
// This handler is dispatched two ways during the canary run:
//   1. Direct REST POST  → /canary/sql/p113/probe              (control)
//   2. MCP tools/call    → name = "p113_view_probe"            (experiment)
//
// Both paths call the same V8 module + entrypoint. The probe asserts that
// `Rivers.db.query("canary-sqlite", ...)` succeeds — which can only happen
// if the dispatcher wired `canary-sqlite` into TASK_DS_CONFIGS for this
// task. Before P1.13 was fixed, path #1 worked but path #2 threw
// `CapabilityError: datasource 'canary-sqlite' not declared in view config`.
//
// Spec: rivers-mcp-view-spec.md §13.2; case-rivers-mcp-view-capability-propagation.md.

function P113Result(test_id) {
    this.test_id = test_id;
    this.profile = "MCP";
    this.spec_ref = "rivers-mcp-view-spec.md §13.2";
    this.assertions = [];
    this.error = null;
    this.start = Date.now();
}
P113Result.prototype.assert = function(id, passed, detail) {
    this.assertions.push({ id: id, passed: passed, detail: detail || undefined });
};
P113Result.prototype.finish = function() {
    return {
        test_id: this.test_id,
        profile: this.profile,
        spec_ref: this.spec_ref,
        passed: this.assertions.every(function(a) { return a.passed; }),
        assertions: this.assertions,
        duration_ms: Date.now() - this.start,
        error: this.error,
    };
};
P113Result.prototype.fail = function(err) {
    this.error = "" + err;
    return {
        test_id: this.test_id,
        profile: this.profile,
        spec_ref: this.spec_ref,
        passed: false,
        assertions: this.assertions,
        duration_ms: Date.now() - this.start,
        error: this.error,
    };
};

// p113Probe — runs an in-bundle SELECT against canary-sqlite.
//
// The test_id is "P113-MCP-VIEW" so the canary harness can pin this single
// regression. A successful return through MCP tools/call proves the inner
// view's capability list propagated through dispatch_codecomponent_tool.
function p113Probe(ctx) {
    var t = new P113Result("P113-MCP-VIEW");
    try {
        var r = Rivers.db.query("canary-sqlite", "SELECT 1 AS answer", []);
        var rows = (r && r.rows) ? r.rows : [];
        var answer = rows.length > 0 ? rows[0].answer : null;

        t.assert("rivers-db-query-succeeded", rows.length === 1,
            "expected 1 row, got " + rows.length);
        t.assert("answer-equals-1", answer === 1 || answer === "1",
            "expected answer=1, got " + JSON.stringify(answer));

        return t.finish();
    } catch (e) {
        return t.fail(e);
    }
}
