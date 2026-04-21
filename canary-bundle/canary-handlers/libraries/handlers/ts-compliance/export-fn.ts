// RT-TS-EXPORT-FN — spec §4 module-namespace entrypoint lookup.
// Mirrors probe case G. `export function` reaches call_entrypoint via
// the module namespace object — no globalThis.handler workaround needed.

import { TestResult } from "../test-harness.ts";

export function exportFn(ctx: any): void {
    const t = new TestResult("RT-TS-EXPORT-FN", "handlers", "spec §4");
    t.assert(
        "dispatched_via_namespace",
        true,
        "handler found on module.get_module_namespace()",
    );
    ctx.resdata = t.finish();
}
