// Phase 6H — source-map remapping probe.
//
// Intentionally throws at a distinctive line number. The test runner
// POSTs this endpoint, receives an error envelope, and asserts that
// `details.stack` (debug mode) includes a frame pointing at this `.ts`
// file — NOT the compiled JS position.
//
// Line 14 below is the `throw`; any change to line numbers above it
// must be reflected in the test runner's assertion. Keep this file's
// leading comment structure stable.

import { TestResult } from "../test-harness.ts";

function boom(message: string): never {
    throw new Error(message);        // ← line 14 — assertion target
}

export function sourcemapProbe(ctx: any): void {
    // Placeholder — this handler always throws. If the test ever sees
    // `sourcemapProbe` as a PASS, something went wrong.
    boom("canary sourcemap probe — expected throw");
    ctx.resdata = new TestResult("RT-TS-SOURCEMAP", "handlers", "spec §5").finish();
}
