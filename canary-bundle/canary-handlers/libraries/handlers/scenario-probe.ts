// SCENARIO-RUNTIME-PROBE — envelope-shape validator (CS1.4).
// Trivial 1-step scenario that asserts true, used to verify the
// scenario-harness.ts port is wired correctly before CS4 ships the
// Document Pipeline scenario. Profile="RUNTIME" per spec §4 test-id
// convention (scenario is hosted in canary-handlers per CS0.1).

import { ScenarioResult } from "./scenario-harness.ts";

export function scenarioProbe(ctx: any): void {
    const sr = new ScenarioResult(
        "SCENARIO-RUNTIME-PROBE",
        "RUNTIME",
        "probe",
        "rivers-canary-scenarios-spec.md §3",
        1
    );

    const step = sr.beginStep("harness-smoke");
    step.assert("harness_reachable", true);
    step.assert("ctx_is_object", typeof ctx === "object" && ctx !== null);

    ctx.resdata = sr.finish();
}
