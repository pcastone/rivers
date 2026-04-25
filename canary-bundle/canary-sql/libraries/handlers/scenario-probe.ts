// SCENARIO-SQL-PROBE — envelope-shape validator (CS1.4).
// Trivial 1-step scenario that asserts true, used to verify the
// scenario-harness.ts port is wired correctly before CS2 ships real
// scenarios. Checks SV-1 (type="scenario"), SV-3 (steps[] shape),
// SV-5 (failed_at_step=null on full pass), SV-6 (total_steps honoured).

import { ScenarioResult } from "./scenario-harness.ts";

export function scenarioProbe(ctx: any): void {
    const sr = new ScenarioResult(
        "SCENARIO-SQL-PROBE",
        "SQL",
        "probe",
        "rivers-canary-scenarios-spec.md §3",
        1
    );

    const step = sr.beginStep("harness-smoke");
    step.assert("harness_reachable", true);
    step.assert("ctx_is_object", typeof ctx === "object" && ctx !== null);

    ctx.resdata = sr.finish();
}
