// RT-TS-MULTIMOD — spec §3 multi-module import resolution.
// Mirrors probe case F. The handler imports from a sibling file;
// V8's module resolver must look up the helper in BundleModuleCache.

import { double, MODULE_MARKER } from "./multimod-helpers.ts";
import { TestResult } from "../test-harness.ts";

export function multimod(ctx: any): void {
    const t = new TestResult("RT-TS-MULTIMOD", "handlers", "spec §3");
    t.assertEquals("helper_returns_double", 20, double(10));
    t.assertEquals("helper_marker_imported", "multimod-helper", MODULE_MARKER);
    ctx.resdata = t.finish();
}
