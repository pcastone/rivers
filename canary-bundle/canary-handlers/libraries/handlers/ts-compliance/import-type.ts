// RT-TS-IMPORT-TYPE — spec §2.2 type-only import erasure.
// Mirrors probe case D. The `type Answer` import is removed by swc's
// typescript transform; `buildAnswer` survives as a runtime import.

import { type Answer, buildAnswer } from "./import-type-helpers.ts";
import { TestResult } from "../test-harness.ts";

export function importType(ctx: any): void {
    const t = new TestResult("RT-TS-IMPORT-TYPE", "handlers", "spec §2.2");
    const a: Answer = buildAnswer(7);
    t.assertEquals("runtime_value", 7, a.value);
    ctx.resdata = t.finish();
}
