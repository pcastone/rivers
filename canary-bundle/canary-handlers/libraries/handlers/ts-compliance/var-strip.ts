// RT-TS-VAR-STRIP — spec §2.2 variable annotation erasure.
// Mirrors probe case C.

import { TestResult } from "../test-harness.ts";

export function varStrip(ctx: any): void {
    const t = new TestResult("RT-TS-VAR-STRIP", "handlers", "spec §2.2");
    const answer: number = 42;
    const name: string = "rivers";
    t.assertEquals("answer_value", 42, answer);
    t.assertEquals("name_value", "rivers", name);
    ctx.resdata = t.finish();
}
