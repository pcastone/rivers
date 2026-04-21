// RT-TS-GENERIC — spec §2.2 generic parameter erasure.
// Mirrors probe case E.

import { TestResult } from "../test-harness.ts";

function identity<T>(x: T): T {
    return x;
}

export function generic(ctx: any): void {
    const t = new TestResult("RT-TS-GENERIC", "handlers", "spec §2.2");
    t.assertEquals("identity_number", 42, identity<number>(42));
    t.assertEquals("identity_string", "rivers", identity<string>("rivers"));
    ctx.resdata = t.finish();
}
