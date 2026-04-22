// RT-TXN-ROLLBACK — spec §6 rollback-on-throw.
// Callback throws after executing a dataview; the handler receives the
// re-thrown exception unchanged.

import { TestResult } from "../test-harness.ts";

export function txnRollback(ctx: any): void {
    const t = new TestResult("RT-TXN-ROLLBACK", "handlers", "spec §6.1");
    const distinctive = "canary-rollback-probe-" + String(Date.now());
    try {
        ctx.transaction("pg", function () {
            ctx.dataview("txn_pg_ping");
            throw new Error(distinctive);
        });
        t.assert("caught_rethrow", false, "expected exception, got none");
    } catch (e: any) {
        const msg = String(e && e.message ? e.message : e);
        t.assert("caught_rethrow", true);
        t.assert(
            "original_exception_preserved",
            msg.indexOf(distinctive) >= 0,
            msg,
        );
    }
    ctx.resdata = t.finish();
}
