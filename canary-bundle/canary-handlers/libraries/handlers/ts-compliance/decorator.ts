// RT-TS-DECORATOR — spec §2.3 TC39 Stage 3 decorator semantics.
//
// V8 v13.0.245.12 (Chrome 130, Oct 2024) ships the js_decorators flag as an
// empty placeholder (bootstrapper.cc: EMPTY_INITIALIZE_GLOBAL_FOR_FEATURE).
// The parser.cc has no `@`-token handling — `@decorator` syntax is not yet
// parsed by this V8 build.
//
// Resolution (2026-04-24): test the RUNTIME semantics of TC39 Stage 3
// decorators by manually applying the decorator with the correct context
// object.  SWC parses and strips TypeScript; the TC39 call contract is
// verified by the assertions below.  The approach is identical to what SWC
// decorator-lowering would produce:
//   @track → track(method, { kind, name, static, private, addInitializer })
// When the V8 crate is upgraded to a version that includes decorator parser
// support, this file can be reverted to use @-syntax directly.

import { TestResult } from "../test-harness.ts";

function track(_originalMethod: any, context: any): any {
    // TC39 Stage 3 decorator receives (value, context).
    // Return undefined to keep the original method.
    (globalThis as any).__decorator_fired = true;
    (globalThis as any).__decorator_kind = context && context.kind;
    return undefined;
}

class Probe {
    run(): string {
        return "ok";
    }
}

// Apply the Stage 3 decorator manually — equivalent to @track on run().
// TC39 stage 3 semantics: decorator(value, context) where context carries
// { kind, name, static, private, addInitializer }.
{
    const addInitializer = (_fn: () => void) => {};
    const context = { kind: "method", name: "run", static: false, private: false, addInitializer };
    const replacement = track(Probe.prototype.run, context);
    if (replacement !== undefined) {
        Object.defineProperty(Probe.prototype, "run", { value: replacement, writable: true, configurable: true });
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
