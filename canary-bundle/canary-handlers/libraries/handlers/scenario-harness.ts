// Scenario harness — multi-step verdict builder atop test-harness.ts.
// Per `docs/arch/rivers-canary-scenarios-spec.md` §3.
// Copy this file into every app's libraries/handlers/ directory (SH-2;
// cross-app imports are forbidden, same rule as test-harness.ts).

import { TestResult, Assertion } from "./test-harness.ts";

export interface StepResult {
  step: number;
  name: string;
  passed: boolean;
  assertions: Assertion[];
  duration_ms: number;
  detail?: string;
}

export class ScenarioResult {
  test_id: string;
  profile: string;
  scenario: string;
  spec_ref: string;
  steps: StepResult[] = [];
  total_steps: number;
  error: string | null = null;
  private start: number;
  private current_step: TestResult | null = null;
  private current_step_name: string = "";
  private step_start: number = 0;
  private step_index: number = 0;
  private failed_steps: Set<number> = new Set();

  constructor(
    test_id: string,
    profile: string,
    scenario: string,
    spec_ref: string,
    total_steps: number
  ) {
    this.test_id = test_id;
    this.profile = profile;
    this.scenario = scenario;
    this.spec_ref = spec_ref;
    this.total_steps = total_steps;
    this.start = Date.now();
  }

  /// Open a new step. Implicitly closes the previous step if one is open.
  /// Returns the active TestResult — call its assert* methods directly.
  beginStep(name: string): TestResult {
    if (this.current_step) {
      this.endStep();
    }
    this.step_index++;
    this.step_start = Date.now();
    this.current_step_name = name;
    this.current_step = new TestResult(
      this.test_id + ":step-" + this.step_index,
      this.profile,
      this.spec_ref
    );
    return this.current_step;
  }

  /// Close the currently-open step. Usually called implicitly by the next
  /// beginStep() or by finish(); explicit calls are allowed but optional.
  endStep(): void {
    if (!this.current_step) return;
    var passed = this.current_step.assertions.every(function (a: Assertion) {
      return a.passed;
    });
    if (!passed) {
      this.failed_steps.add(this.step_index);
    }
    this.steps.push({
      step: this.step_index,
      name: this.current_step_name,
      passed: passed,
      assertions: this.current_step.assertions,
      duration_ms: Date.now() - this.step_start,
    });
    this.current_step = null;
    this.current_step_name = "";
  }

  /// Record a skipped step with a dependency explanation (SV-8).
  /// Caller MUST check hasFailed(N) before invoking.
  skipStep(name: string, depends_on: number): void {
    this.step_index++;
    this.steps.push({
      step: this.step_index,
      name: name,
      passed: false,
      assertions: [],
      duration_ms: 0,
      detail: "skipped — depends on step " + depends_on,
    });
    this.failed_steps.add(this.step_index);
  }

  /// Has step N failed so far?
  hasFailed(step: number): boolean {
    return this.failed_steps.has(step);
  }

  /// Finalize the scenario verdict envelope (SV-1…SV-9).
  /// Also emits a FLAT `assertions` array aggregating every step's
  /// assertions with step-prefixed IDs (e.g. `"s1:alice-sends-to-bob:insert_no_throw"`).
  /// This lets the atomic-test SPA renderer — which iterates a top-level
  /// `assertions[]` — show scenario assertion detail without a dedicated
  /// per-step view. The `steps[]` structure is preserved for future
  /// step-level rendering.
  finish(): object {
    if (this.current_step) {
      this.endStep();
    }
    var all_passed = this.steps.every(function (s) { return s.passed; });
    var first_failure: StepResult | undefined;
    for (var i = 0; i < this.steps.length; i++) {
      if (!this.steps[i].passed) {
        first_failure = this.steps[i];
        break;
      }
    }
    var flat: Assertion[] = [];
    for (var s = 0; s < this.steps.length; s++) {
      var st = this.steps[s];
      var prefix = "s" + st.step + ":" + st.name + ":";
      if (st.detail) {
        flat.push({ id: prefix + "_meta", passed: st.passed, detail: st.detail });
      }
      for (var a = 0; a < st.assertions.length; a++) {
        var orig = st.assertions[a];
        flat.push({
          id: prefix + orig.id,
          passed: orig.passed,
          detail: orig.detail
        });
      }
    }
    return {
      test_id: this.test_id,
      profile: this.profile,
      type: "scenario",
      scenario: this.scenario,
      spec_ref: this.spec_ref,
      passed: all_passed,
      steps: this.steps,
      assertions: flat,
      failed_at_step: first_failure ? first_failure.step : null,
      total_steps: this.total_steps,
      duration_ms: Date.now() - this.start,
      error: this.error,
    };
  }

  /// Terminate the scenario with an orchestrator-level error (e.g. infra
  /// connect failed before any step ran). Returns a verdict envelope
  /// marked failed at whichever step was open.
  fail(error: string): object {
    this.error = error;
    if (this.current_step) {
      this.endStep();
    }
    var first_failure: StepResult | undefined;
    for (var i = 0; i < this.steps.length; i++) {
      if (!this.steps[i].passed) {
        first_failure = this.steps[i];
        break;
      }
    }
    return {
      test_id: this.test_id,
      profile: this.profile,
      type: "scenario",
      scenario: this.scenario,
      spec_ref: this.spec_ref,
      passed: false,
      steps: this.steps,
      failed_at_step: first_failure ? first_failure.step : 1,
      total_steps: this.total_steps,
      duration_ms: Date.now() - this.start,
      error: error,
    };
  }
}
