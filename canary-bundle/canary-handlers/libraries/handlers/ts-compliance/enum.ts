// RT-TS-ENUM — spec §2.2 enum lowering to IIFE-wrapped object.

import { TestResult } from "../test-harness.ts";

enum Status {
    Active = 0,
    Inactive = 1,
    Pending = 2,
}

export function enumTest(ctx: any): void {
    const t = new TestResult("RT-TS-ENUM", "handlers", "spec §2.2");
    t.assertEquals("active_value", 0, Status.Active);
    t.assertEquals("inactive_value", 1, Status.Inactive);
    t.assertEquals("pending_value", 2, Status.Pending);
    // Reverse lookup (enum lowering adds this).
    t.assertEquals("reverse_lookup_0", "Active", Status[0]);
    ctx.resdata = t.finish();
}
