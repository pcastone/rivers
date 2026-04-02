// Test harness — shared assertion framework for canary fleet.
// Copy this file into every app's libraries/handlers/ directory.

export interface Assertion {
  id: string;
  passed: boolean;
  detail?: string;
}

export class TestResult {
  test_id: string;
  profile: string;
  spec_ref: string;
  assertions: Assertion[] = [];
  error: string | null = null;
  private start: number;

  constructor(test_id: string, profile: string, spec_ref: string) {
    this.test_id = test_id;
    this.profile = profile;
    this.spec_ref = spec_ref;
    this.start = Date.now();
  }

  assert(id: string, passed: boolean, detail?: string) {
    this.assertions.push({ id, passed, detail: detail || undefined });
  }

  assertEquals(id: string, expected: any, actual: any) {
    var passed = JSON.stringify(expected) === JSON.stringify(actual);
    this.assertions.push({
      id,
      passed,
      detail: passed
        ? "expected=" + JSON.stringify(expected)
        : "expected=" + JSON.stringify(expected) + ", actual=" + JSON.stringify(actual)
    });
  }

  assertExists(id: string, value: any) {
    var passed = value !== undefined && value !== null;
    this.assertions.push({
      id,
      passed,
      detail: passed ? "type=" + typeof value : "value was " + value
    });
  }

  assertType(id: string, value: any, expectedType: string) {
    var actual = typeof value;
    var passed = actual === expectedType;
    this.assertions.push({
      id,
      passed,
      detail: passed ? "type=" + actual : "expected type=" + expectedType + ", actual=" + actual
    });
  }

  assertThrows(id: string, fn: () => any) {
    var threw = false;
    var errMsg = "";
    try { fn(); } catch (e) { threw = true; errMsg = String(e); }
    this.assertions.push({
      id,
      passed: threw,
      detail: threw ? "threw: " + errMsg : "did not throw"
    });
  }

  assertNotContains(id: string, haystack: string, needle: string) {
    var passed = haystack.toLowerCase().indexOf(needle.toLowerCase()) === -1;
    this.assertions.push({
      id,
      passed,
      detail: passed ? '"' + needle + '" not found' : '"' + needle + '" found in response'
    });
  }

  finish(): object {
    return {
      test_id: this.test_id,
      profile: this.profile,
      spec_ref: this.spec_ref,
      passed: this.assertions.every(function(a) { return a.passed; }),
      assertions: this.assertions,
      duration_ms: Date.now() - this.start,
      error: this.error
    };
  }

  fail(error: string): object {
    this.error = error;
    return {
      test_id: this.test_id,
      profile: this.profile,
      spec_ref: this.spec_ref,
      passed: false,
      assertions: this.assertions,
      duration_ms: Date.now() - this.start,
      error: error
    };
  }
}
