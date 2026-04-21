// RT-TXN-NESTED — spec §6.2 nested transactions prohibited.
// Inner ctx.transaction() call inside an outer transaction callback
// must throw TransactionError: nested transactions not supported.

import { TestResult } from "../test-harness.ts";

export function txnNested(ctx: any): void {
    const t = new TestResult("RT-TXN-NESTED", "handlers", "spec §6.2");
    try {
        ctx.transaction("pg", function () {
            ctx.transaction("pg", function () {
                // Unreachable — the inner call must throw before executing.
            });
        });
        t.assert("caught_nested_error", false, "expected throw");
    } catch (e: any) {
        const msg = String(e && e.message ? e.message : e);
        t.assert("caught_nested_error", true);
        t.assert("is_TransactionError", msg.indexOf("TransactionError") >= 0, msg);
        t.assert(
            "says_nested_not_supported",
            msg.indexOf("nested transactions not supported") >= 0,
            msg,
        );
    }
    ctx.resdata = t.finish();
}
