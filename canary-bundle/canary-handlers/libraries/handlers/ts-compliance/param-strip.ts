// RT-TS-PARAM-STRIP — spec §2.2 parameter annotation erasure.
// Mirrors probe case B. If this handler dispatches, swc stripped
// the `: any` and V8 accepted the resulting function.

import { TestResult } from "../test-harness.ts";

export function paramStrip(ctx: any): void {
    const t = new TestResult("RT-TS-PARAM-STRIP", "handlers", "spec §2.2");
    t.assert("dispatched", true);
    t.assert(
        "ctx_is_object",
        typeof ctx === "object" && ctx !== null,
    );
    ctx.resdata = t.finish();
}
