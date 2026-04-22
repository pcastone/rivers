// RT-TXN-CROSS-DS — spec §6.2 cross-datasource dataview inside a
// transaction must throw TransactionError with the verbatim shape:
//   TransactionError: dataview "X" uses datasource "A" which differs
//     from transaction datasource "B"

import { TestResult } from "../test-harness.ts";

export function txnCrossDs(ctx: any): void {
    const t = new TestResult("RT-TXN-CROSS-DS", "handlers", "spec §6.2");
    try {
        ctx.transaction("pg", function () {
            // `txn_sqlite_ping` routes to `sqlite_cross` — a different
            // datasource than the transaction's `pg`. Must throw.
            ctx.dataview("txn_sqlite_ping");
        });
        t.assert("caught_cross_ds_error", false, "expected throw");
    } catch (e: any) {
        const msg = String(e && e.message ? e.message : e);
        t.assert("caught_cross_ds_error", true);
        t.assert("is_TransactionError", msg.indexOf("TransactionError") >= 0, msg);
        t.assert("mentions_differs_from", msg.indexOf("differs from") >= 0, msg);
        t.assert("names_dataview", msg.indexOf("txn_sqlite_ping") >= 0, msg);
    }
    ctx.resdata = t.finish();
}
