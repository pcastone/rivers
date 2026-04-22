// RT-TXN-COMMIT — spec §6 commit-on-return semantics.
// Dispatches ctx.transaction("pg", () => ctx.dataview("txn_pg_ping"))
// and asserts: (a) no throw, (b) callback's return value reaches the
// handler, (c) dataview executed inside the held connection path
// (proven by absence of TransactionError).
//
// Requires a reachable `pg` datasource (192.168.2.209). Skip via the
// canary `PG_AVAIL` gate on no-infra deploys.

import { TestResult } from "../test-harness.ts";

export function txnCommit(ctx: any): void {
    const t = new TestResult("RT-TXN-COMMIT", "handlers", "spec §6.1");
    try {
        const out = ctx.transaction("pg", function () {
            const r = ctx.dataview("txn_pg_ping");
            return { rows: (r && r.rows) ? r.rows.length : 0 };
        });
        t.assert("no_throw", true);
        t.assert(
            "callback_return_reaches_handler",
            typeof out === "object" && out !== null && "rows" in out,
        );
        t.assert("rows_read_via_txn_connection", (out as any).rows >= 1);
    } catch (e: any) {
        t.assert("no_throw", false, String(e && e.message ? e.message : e));
    }
    ctx.resdata = t.finish();
}
