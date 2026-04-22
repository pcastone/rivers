// Transaction tests (spec §6 — rivers-javascript-typescript-spec).
// Each handler probes one slice of ctx.transaction semantics without
// depending on a real database — argument validation and error-shape
// checks exercise the pre-connection code paths that run regardless
// of whether the underlying datasource exists.

import { TestResult } from "./test-harness.ts";

// RT-TXN-ARGS: ctx.transaction requires (datasource: string, fn: Function)
export function txnRequiresTwoArgs(ctx: any): void {
    var t = new TestResult("RT-TXN-ARGS", "handlers", "spec §6.1");
    try {
        (ctx as any).transaction("pg");
        t.assert("threw", false, "expected throw, got none");
    } catch (e: any) {
        var msg = String(e && e.message ? e.message : e);
        t.assert("threw", true);
        t.assert("message_mentions_args", msg.indexOf("two arguments") >= 0, msg);
    }
    ctx.resdata = t.finish();
}

// RT-TXN-CB-TYPE: second arg must be a function
export function txnRejectsNonFunction(ctx: any): void {
    var t = new TestResult("RT-TXN-CB-TYPE", "handlers", "spec §6.1");
    try {
        (ctx as any).transaction("pg", "not a function");
        t.assert("threw", false, "expected throw, got none");
    } catch (e: any) {
        var msg = String(e && e.message ? e.message : e);
        t.assert("threw", true);
        t.assert("message_mentions_fn", msg.indexOf("must be a function") >= 0, msg);
    }
    ctx.resdata = t.finish();
}

// RT-TXN-UNKNOWN-DS: datasource not in task config throws TransactionError
export function txnUnknownDatasourceThrows(ctx: any): void {
    var t = new TestResult("RT-TXN-UNKNOWN-DS", "handlers", "spec §6.2");
    try {
        ctx.transaction("not_a_real_datasource_xyz", function () {
            return { ok: true };
        });
        t.assert("threw", false, "expected throw, got none");
    } catch (e: any) {
        var msg = String(e && e.message ? e.message : e);
        t.assert("threw", true);
        t.assert("is_TransactionError", msg.indexOf("TransactionError") >= 0, msg);
        t.assert("says_not_found", msg.indexOf("not found") >= 0, msg);
        t.assert("names_datasource", msg.indexOf("not_a_real_datasource_xyz") >= 0, msg);
    }
    ctx.resdata = t.finish();
}

// RT-TXN-STATE-CLEANUP: back-to-back calls on the same handler must not
// leak thread-local state (neither call should incorrectly report "nested").
export function txnStateCleanupBetweenCalls(ctx: any): void {
    var t = new TestResult("RT-TXN-STATE-CLEANUP", "handlers", "spec §6.2");
    var firstMsg: string | null = null;
    var secondMsg: string | null = null;
    try {
        ctx.transaction("not_a_real_datasource", function () {});
    } catch (e: any) {
        firstMsg = String(e && e.message ? e.message : e);
    }
    try {
        ctx.transaction("not_a_real_datasource", function () {});
    } catch (e: any) {
        secondMsg = String(e && e.message ? e.message : e);
    }
    t.assert("first_threw", firstMsg !== null);
    t.assert("second_threw", secondMsg !== null);
    t.assert(
        "second_not_nested",
        secondMsg !== null && secondMsg.indexOf("nested") < 0,
        "second: " + String(secondMsg),
    );
    ctx.resdata = t.finish();
}

// RT-TXN-SURFACE: ctx.transaction is wired into the ctx object (smoke test).
export function txnSurfaceExists(ctx: any): void {
    var t = new TestResult("RT-TXN-SURFACE", "handlers", "spec §6.1");
    t.assert("ctx_has_transaction", typeof ctx.transaction === "function");
    t.assert(
        "transaction_is_not_dataview",
        ctx.transaction !== ctx.dataview,
    );
    ctx.resdata = t.finish();
}
