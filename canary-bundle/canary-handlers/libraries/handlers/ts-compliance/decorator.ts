// RT-TS-DECORATOR — spec §2.3 TC39 Stage 3 decorator syntax.
// Parser accepts the syntax; swc leaves decorators in the AST;
// V8 v130 executes Stage 3 decorators natively.

import { TestResult } from "../test-harness.ts";

function track(_originalMethod: any, context: any): any {
    // TC39 Stage 3 decorator receives (target, context). Return undefined
    // to keep the original method; we just probe that the decorator runs.
    (globalThis as any).__decorator_fired = true;
    (globalThis as any).__decorator_kind = context && context.kind;
    return undefined;
}

class Probe {
    @track
    run(): string {
        return "ok";
    }
}

export function decoratorTest(ctx: any): void {
    const t = new TestResult("RT-TS-DECORATOR", "handlers", "spec §2.3");
    const p = new Probe();
    t.assertEquals("method_returns", "ok", p.run());
    t.assert(
        "decorator_fired_at_class_init",
        (globalThis as any).__decorator_fired === true,
    );
    t.assertEquals(
        "decorator_received_method_kind",
        "method",
        (globalThis as any).__decorator_kind,
    );
    // Clean up globals so repeat calls don't carry state.
    (globalThis as any).__decorator_fired = undefined;
    (globalThis as any).__decorator_kind = undefined;
    ctx.resdata = t.finish();
}
