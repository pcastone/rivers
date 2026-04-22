// RT-TS-NAMESPACE — spec §2.2 namespace lowering to nested object.

import { TestResult } from "../test-harness.ts";

namespace util {
    export const VERSION = "1.0";
    export function greet(who: string): string {
        return "hello " + who;
    }
}

export function namespaceTest(ctx: any): void {
    const t = new TestResult("RT-TS-NAMESPACE", "handlers", "spec §2.2");
    t.assertEquals("version_value", "1.0", util.VERSION);
    t.assertEquals("greet_runs", "hello rivers", util.greet("rivers"));
    ctx.resdata = t.finish();
}
