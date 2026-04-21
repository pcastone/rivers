// RT-TXN-UNSUPPORTED — spec §6.2 transactions on a non-transactional
// driver must throw TransactionError: datasource "X" does not support
// transactions.
//
// Faker declares supports_transactions() = false. Attempting
// ctx.transaction("canary-faker", ...) must fail at begin with the
// spec's verbatim error shape.

import { TestResult } from "../test-harness.ts";

export function txnUnsupported(ctx: any): void {
    const t = new TestResult("RT-TXN-UNSUPPORTED", "handlers", "spec §6.2");
    try {
        ctx.transaction("canary-faker", function () {
            // Unreachable — begin must fail first.
        });
        t.assert("caught_unsupported_error", false, "expected throw");
    } catch (e: any) {
        const msg = String(e && e.message ? e.message : e);
        t.assert("caught_unsupported_error", true);
        t.assert("is_TransactionError", msg.indexOf("TransactionError") >= 0, msg);
        t.assert(
            "says_does_not_support",
            msg.indexOf("does not support transactions") >= 0,
            msg,
        );
    }
    ctx.resdata = t.finish();
}
