# Rivers Canary Fleet — Scenario Testing Spec

**Document Type:** Implementation Specification
**Scope:** Scenario-based integration tests for the canary fleet
**Status:** Draft
**Version:** 1.0
**Depends on:** `rivers-canary-fleet-spec.md` v1.1

---

## 1. Purpose

The canary fleet's atomic test endpoints verify individual spec assertions in isolation. A SQLite parameter binding test proves `bind_params()` works. A session read test proves `ctx.session` is populated. Neither proves that a handler can insert a record into SQLite using session-derived identity, read it back with query parameters, and return a shaped response — which is what every real application does.

Scenarios are multi-step integration tests that exercise Rivers features in composition. Each scenario describes a realistic mini-application use case. The implementation decides all structural details — DataView names, schemas, endpoint paths, handler organization. The spec defines only the use case, the workflow steps, and the behavioral constraints.

### Why Loose Specs

Tight specs test Rivers' ability to execute a predetermined design. Loose specs test Rivers' ability to support a developer building something from a description. Real users write loose specs. If Claude Code builds a message app from this spec and gets the wiring wrong, that is a Rivers usability problem — the same mistake a real developer would make. The scenario becomes both a regression test and a developer experience test.

### Relationship to Atomic Tests

Scenarios are additive. The existing atomic test endpoints remain unchanged. Scenarios do not replace them — they cover a different failure class:

| Layer | Catches | Example |
|---|---|---|
| Atomic tests | Single spec assertion violations | `bind_params()` defaults to `:` instead of `$` |
| Scenarios | Composition failures across subsystems | Init handler can't create tables because `datasource_configs` isn't populated |

A green atomic suite with a red scenario suite means the pieces work but the seams don't.

---

## 2. Scenario Verdict Protocol

Scenarios use an extended version of the self-reporting verdict envelope. The key addition is a `steps` array that reports per-step pass/fail, allowing the harness (and dashboard) to identify exactly where a multi-step workflow broke.

```json
{
  "test_id": "SCENARIO-SQL-MESSAGING-PG",
  "profile": "SQL",
  "type": "scenario",
  "scenario": "messaging",
  "spec_ref": "rivers-canary-scenarios-spec.md §4",
  "passed": false,
  "steps": [
    {
      "step": 1,
      "name": "alice-sends-message",
      "passed": true,
      "assertions": [
        { "id": "insert_returned_id", "passed": true, "detail": "id=a1b2c3" }
      ],
      "duration_ms": 8
    },
    {
      "step": 2,
      "name": "bob-checks-inbox",
      "passed": false,
      "assertions": [
        { "id": "inbox_not_empty", "passed": true, "detail": "count=1" },
        { "id": "message_from_alice", "passed": false,
          "detail": "expected sender='alice', got sender=null" }
      ],
      "duration_ms": 5
    }
  ],
  "failed_at_step": 2,
  "total_steps": 12,
  "duration_ms": 13,
  "error": null
}
```

### Verdict Rules

| ID | Rule |
|---|---|
| SV-1 | `type` MUST be `"scenario"` to distinguish from atomic verdicts. |
| SV-2 | `scenario` MUST be the scenario name (lowercase, hyphenated). |
| SV-3 | `steps` MUST be an ordered array. Each step has `step` (1-indexed), `name` (snake-case), `passed`, `assertions`, and `duration_ms`. |
| SV-4 | `passed` at the top level is `true` only if ALL steps passed. |
| SV-5 | `failed_at_step` is the 1-indexed step number of the first failure, or `null` if all passed. |
| SV-6 | `total_steps` is the total number of steps defined in the scenario, regardless of how many executed. |
| SV-7 | When a step fails, subsequent steps MUST still execute unless they have an explicit data dependency on the failed step. This maximizes diagnostic information. |
| SV-8 | Steps with a data dependency on a failed step MUST be reported as skipped: `"passed": false, "detail": "skipped — depends on step N"`. |
| SV-9 | HTTP status is always `200` for a completed scenario (check `passed` for result). `500` means the scenario handler itself crashed. |

---

## 3. Scenario Test Harness

The scenario harness extends the existing `TestResult` class. Each profile that hosts scenarios copies this into its `libraries/handlers/` directory alongside the existing `test-harness.ts`.

```typescript
// libraries/handlers/scenario-harness.ts

import { TestResult, Assertion } from './test-harness';

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

  beginStep(name: string): TestResult {
    if (this.current_step) {
      this.endStep();
    }
    this.step_index++;
    this.step_start = Date.now();
    this.current_step = new TestResult(
      `${this.test_id}:step-${this.step_index}`,
      this.profile,
      this.spec_ref
    );
    (this.current_step as any)._step_name = name;
    return this.current_step;
  }

  endStep(): void {
    if (!this.current_step) return;
    const passed = this.current_step.assertions
      ? this.current_step.assertions.every((a: Assertion) => a.passed)
      : true;
    if (!passed) {
      this.failed_steps.add(this.step_index);
    }
    this.steps.push({
      step: this.step_index,
      name: (this.current_step as any)._step_name,
      passed,
      assertions: this.current_step.assertions || [],
      duration_ms: Date.now() - this.step_start,
    });
    this.current_step = null;
  }

  skipStep(name: string, depends_on: number): void {
    this.step_index++;
    this.steps.push({
      step: this.step_index,
      name,
      passed: false,
      assertions: [],
      duration_ms: 0,
      detail: `skipped — depends on step ${depends_on}`,
    });
    this.failed_steps.add(this.step_index);
  }

  hasFailed(step: number): boolean {
    return this.failed_steps.has(step);
  }

  finish(): object {
    if (this.current_step) {
      this.endStep();
    }
    const all_passed = this.steps.every(s => s.passed);
    const first_failure = this.steps.find(s => !s.passed);
    return {
      test_id: this.test_id,
      profile: this.profile,
      type: "scenario",
      scenario: this.scenario,
      spec_ref: this.spec_ref,
      passed: all_passed,
      steps: this.steps,
      failed_at_step: first_failure ? first_failure.step : null,
      total_steps: this.total_steps,
      duration_ms: Date.now() - this.start,
      error: this.error,
    };
  }

  fail(error: string): object {
    this.error = error;
    if (this.current_step) {
      this.endStep();
    }
    return {
      test_id: this.test_id,
      profile: this.profile,
      type: "scenario",
      scenario: this.scenario,
      spec_ref: this.spec_ref,
      passed: false,
      steps: this.steps,
      failed_at_step: this.steps.find(s => !s.passed)?.step || 1,
      total_steps: this.total_steps,
      duration_ms: Date.now() - this.start,
      error,
    };
  }
}
```

### Harness Constraints

| ID | Rule |
|---|---|
| SH-1 | `scenario-harness.ts` imports from `test-harness.ts`. Each step reuses the atomic `TestResult` assertion methods (`assertEquals`, `assertExists`, `assertThrows`, etc.). |
| SH-2 | Each profile that hosts scenarios gets its own copy of `scenario-harness.ts`. Cross-app imports are forbidden (same rule as `test-harness.ts`). |
| SH-3 | `beginStep()` returns the active `TestResult` for that step. The handler calls assertion methods on it directly. |
| SH-4 | `endStep()` is called implicitly by the next `beginStep()` or by `finish()`. Explicit calls are allowed but not required. |
| SH-5 | `skipStep()` records a failed step with a dependency explanation. The handler MUST check `hasFailed(N)` before skipping. |

---

## 4. Scenario Hosting

Scenarios live within existing profiles. Each scenario gets its own handler file under the profile's `libraries/handlers/` directory and its own endpoint(s) under a `/canary/scenarios/` path prefix.

### Path Convention

```
/canary/scenarios/{profile}/{scenario-name}
```

Examples:
- `/canary/scenarios/sql/messaging`
- `/canary/scenarios/stream/activity-feed`
- `/canary/scenarios/rt/doc-pipeline`

### Test ID Convention

```
SCENARIO-{PROFILE}-{SCENARIO-NAME}[-{DRIVER}]
```

The optional driver suffix applies when a scenario runs against multiple backends:
- `SCENARIO-SQL-MESSAGING-PG`
- `SCENARIO-SQL-MESSAGING-MYSQL`
- `SCENARIO-SQL-MESSAGING-SQLITE`

### Profile Assignment

| Scenario | Profile | Rationale |
|---|---|---|
| Messaging | canary-sql | Primary concern is SQL CRUD, parameter binding, init handler DDL. Auth and crypto are secondary. |
| Activity Feed | canary-streams | Primary concern is Kafka publish, SSE delivery, streaming lifecycle. SQL for history is secondary. |
| Document Pipeline | canary-handlers | Primary concern is filesystem driver, exec driver, handler context surface. |

### What Claude Code Decides

For each scenario, the implementation chooses:

- DataView names, count, and configuration
- Schema shapes and field names
- Handler file organization and function names
- Endpoint paths (within the `/canary/scenarios/` prefix)
- Init handler DDL (table structures, indexes)
- Error response shapes
- Internal data flow (how data moves between DataViews and handlers)
- Parameter naming (but MUST use the `zname`-style non-alphabetical trap for any SQL scenario)

### What the Spec Decides

- The use case narrative
- The workflow steps (what must happen, in what order)
- The behavioral constraints (security rules, data invariants)
- The verdict protocol (envelope shape, step reporting)

---

## 5. Scenario A — Messaging

**Profile:** canary-sql
**Drivers:** PostgreSQL, MySQL, SQLite (one scenario instance per driver)
**Touches:** Init handler DDL, SQL CRUD, parameter binding, session identity, `Rivers.crypto`, query parameters, DataView dispatch

### Use Case

Users send messages to other users. A user can check their inbox, search through received messages, and delete messages they own. Some messages are marked as secret — their body is encrypted at rest and decrypted only when the intended recipient reads them.

Three test users exist in the session layer: Alice (`sub: "alice"`), Bob (`sub: "bob"`), and Carol (`sub: "carol"`). The scenario handler simulates their sessions by setting identity claims before each operation.

### Workflows

| Step | Action | Verification |
|---|---|---|
| 1 | Alice sends a message to Bob | Insert succeeds, returns a message ID |
| 2 | Bob checks his inbox | Alice's message appears with correct sender, subject, body |
| 3 | Alice checks her inbox | Empty — she is not the recipient of any message |
| 4 | Bob searches messages by a keyword from Alice's message body | Search returns exactly one result matching Alice's message |
| 5 | Bob searches with a keyword not in any message | Search returns empty results |
| 6 | Alice sends Bob a secret message (body marked as secret) | Insert succeeds, returns a message ID |
| 7 | Bob reads the secret message | Decrypted plaintext body is returned |
| 8 | Direct database read of the secret message row | Stored body is NOT plaintext (encryption verified at rest) |
| 9 | Carol checks Bob's inbox | Returns nothing — inbox is scoped to the authenticated user |
| 10 | Bob deletes Alice's first (non-secret) message | Delete succeeds |
| 11 | Bob checks his inbox again | Only the secret message remains |
| 12 | Carol attempts to delete Bob's secret message | Rejected — Carol is neither sender nor recipient |

### Constraints

| ID | Rule |
|---|---|
| MSG-1 | Sender identity MUST come from the session (`ctx.session.sub`), not the request body. The handler MUST NOT accept a `sender` field in the request payload. |
| MSG-2 | Inbox MUST be filtered server-side via DataView parameters. The handler MUST NOT fetch all messages and filter in JavaScript. |
| MSG-3 | Search MUST support at minimum: keyword match on message body and sender filter. Implementation may add date range, pagination, or other filters. |
| MSG-4 | Secret message encryption MUST use `Rivers.crypto.encrypt()` / `Rivers.crypto.decrypt()` (or equivalent `Rivers.crypto` API). Encryption must happen before the INSERT, decryption on read. |
| MSG-5 | Deletion MUST be restricted to the message sender OR the message recipient. All other users receive an error. |
| MSG-6 | All SQL queries MUST use parameterized DataViews. String concatenation for query construction is forbidden. |
| MSG-7 | The message table's column naming MUST use the `zname`-style parameter binding trap — at least one column name must sort alphabetically before a column declared earlier in the parameter list. |
| MSG-8 | The init handler MUST create all required tables via DDL three-gate enforcement. The scenario MUST NOT assume tables exist. |
| MSG-9 | Step 8 (encryption-at-rest verification) MUST perform a raw SELECT against the database — not through the handler's decryption path — to prove the stored value is ciphertext. |
| MSG-10 | The scenario MUST run independently against each SQL driver (PG, MySQL, SQLite) using driver-specific datasources. Schema differences across drivers (UUID generation, datetime types) are the implementation's concern. |

### Step Dependencies

Steps 2–12 depend on Step 1 (Alice's message must exist). Steps 7–8 depend on Step 6 (secret message must exist). Steps 10–11 depend on Step 1 (delete target must exist). If a dependency step fails, dependent steps MUST be reported as skipped per SV-8.

---

## 6. Scenario B — Activity Feed

**Profile:** canary-streams
**Touches:** Kafka publish, Kafka consume (MessageConsumer), SSE delivery, EventBus, REST history endpoint, session scoping, query parameters, event ordering

### Use Case

A system tracks user activity by publishing events to Kafka. Events are delivered in real-time via SSE to connected users and are also persisted so users can query their activity history via a REST endpoint. Each user only sees their own events.

Three test users: Alice, Bob, Carol.

### Workflows

| Step | Action | Verification |
|---|---|---|
| 1 | Publish an event for Bob to Kafka ("Alice commented on your post") | Kafka produce succeeds |
| 2 | Consume the event from Kafka via MessageConsumer | Consumer handler receives the event, persists it |
| 3 | Bob requests activity history via REST | One event returned, content matches published event |
| 4 | Publish three more events for Bob in rapid succession | All three produce calls succeed |
| 5 | Wait for consumer processing, then Bob requests history | Four events total, in publish order |
| 6 | Bob requests history with a date range filter (before the last three events) | Returns only the first event |
| 7 | Carol requests activity history | Empty — no events for Carol |
| 8 | Publish an event for Carol | Produce succeeds |
| 9 | Carol requests history | One event — only Carol's |
| 10 | Bob requests history | Still four events — Carol's event does not appear |
| 11 | Bob requests history with pagination (limit 2, offset 0, then offset 2) | Two pages, two events each, all four events covered, no duplicates |

### Constraints

| ID | Rule |
|---|---|
| AF-1 | Kafka is the source of truth. Events MUST be published to Kafka and consumed via a MessageConsumer view. Direct database inserts bypass the event pipeline and are forbidden. |
| AF-2 | Event persistence MUST happen in the MessageConsumer handler, not in the producer or the REST handler. |
| AF-3 | The REST history endpoint MUST filter events server-side by authenticated user. The handler MUST NOT return other users' events. |
| AF-4 | History MUST support pagination (limit + offset or cursor) and date range filtering. |
| AF-5 | Event ordering MUST be preserved — events returned by history MUST be in publish order (or reverse chronological, as long as consistent). |
| AF-6 | Events MUST have at minimum: event type, actor (who performed the action), target user, timestamp, and a payload field. |
| AF-7 | The consumer MUST store events in a SQL table (using one of the canary SQL datasources) so that the REST history endpoint can query them with DataViews. |
| AF-8 | The Kafka topic name is implementation's choice but MUST use the `canary-kafka` datasource from the fleet's LockBox aliases. |
| AF-9 | The init handler MUST create the events table. The scenario MUST NOT assume tables exist. |

### Step Dependencies

Steps 2–11 depend on Kafka connectivity (if Kafka is unreachable, all steps after 1 are skipped). Steps 3 and 5–11 depend on the consumer having processed events (Step 2). The scenario SHOULD include a brief poll/wait for consumer processing before querying history — eventual delivery is acceptable, indefinite hanging is not. A reasonable timeout (5 seconds) with a retry loop is sufficient.

### SSE Note

Real-time SSE delivery (Bob receives events on an open stream) is valuable but difficult to test from a synchronous scenario handler. The scenario focuses on the durable path: publish → consume → persist → query. SSE delivery is covered by the atomic `STREAM-SSE-*` tests. If the implementation can test SSE delivery within the scenario (e.g., by opening an internal SSE connection), it MAY add steps for it, but this is not required.

---

## 7. Scenario C — Document Processing Pipeline

**Profile:** canary-handlers
**Touches:** Filesystem driver (sandboxed), ExecDriver (hash-pinned commands), handler context surface, path traversal security, structured command output

### Use Case

A user works with text documents in a sandboxed workspace. They can create, read, list, search, and delete documents. They can also run whitelisted analysis commands against their documents and receive structured results. The system enforces workspace isolation — no file operations can escape the sandbox.

### Workflows

| Step | Action | Verification |
|---|---|---|
| 1 | Create a workspace directory for the test session | Directory created successfully |
| 2 | Write a markdown document ("report.md") into the workspace | Write succeeds, file exists |
| 3 | Read the document back | Content matches exactly what was written |
| 4 | List workspace contents | "report.md" appears in listing |
| 5 | Write a second document ("notes.md") with different content | Write succeeds |
| 6 | Search workspace for a keyword that appears only in "report.md" | Search returns "report.md" only, not "notes.md" |
| 7 | Search workspace for a keyword that appears in neither document | Search returns empty results |
| 8 | Run a whitelisted command (`wc` — word count) against "report.md" | Structured result returned with line/word/byte counts |
| 9 | Run a non-whitelisted command against "report.md" | Rejected with an error — command not in hash-pinned allowlist |
| 10 | Run `wc` with a path targeting a file outside the workspace (`../../etc/passwd` or equivalent) | Rejected — path traversal blocked |
| 11 | Write a document with a filename containing `../` ("../../escape.md") | Rejected or sanitized — path traversal blocked at write |
| 12 | Delete "notes.md" | Delete succeeds |
| 13 | List workspace contents | Only "report.md" remains |
| 14 | Stat "report.md" | Returns file metadata (size, modified time) |

### Constraints

| ID | Rule |
|---|---|
| DOC-1 | All filesystem operations MUST use the filesystem driver's DataView interface. The handler MUST NOT use shell commands (e.g., `exec("ls")`) for file operations. |
| DOC-2 | The filesystem driver MUST enforce chroot-like sandboxing. All paths MUST resolve relative to the configured workspace root. Absolute paths and `..` traversal MUST be rejected or neutralized at the driver level. |
| DOC-3 | ExecDriver commands MUST be hash-pinned — only commands whose SHA-256 hash matches the configured allowlist are executed. The implementation MUST configure at least one whitelisted command (`wc` or equivalent) and demonstrate rejection of a non-whitelisted command. |
| DOC-4 | Command output MUST be returned as structured data (parsed into fields), not raw stdout text. For `wc`, this means fields like `lines`, `words`, `bytes` — not `"  42 156 1024 report.md"`. |
| DOC-5 | The `grep`/search operation MUST return file references (filename, match context), not full file content dumps. |
| DOC-6 | Path traversal rejection (Steps 10, 11) MUST be enforced by the filesystem driver, not by handler-level string checks. The handler MAY additionally validate, but the driver MUST be the security boundary. |
| DOC-7 | The workspace root MUST be configured as a datasource in `resources.toml`, not hardcoded in handler logic. |
| DOC-8 | The scenario handler MUST clean up the workspace directory at the end of execution (delete test files). Cleanup failure MUST NOT cause the scenario to fail — it is logged as a warning. |
| DOC-9 | ExecDriver process isolation MUST use privilege drop if configured. The scenario SHOULD verify that the command runs in the restricted context. |
| DOC-10 | The `stat` operation (Step 14) MUST return structured metadata: at minimum file size and last modified time. |

### Step Dependencies

Steps 2–14 depend on Step 1 (workspace must exist). Steps 3, 6, 8, 10, 14 depend on Step 2 ("report.md" must exist). Steps 12–13 depend on Step 5 ("notes.md" must exist). Step 6 depends on both Steps 2 and 5 (both documents must exist for the differential search test).

---

## 8. Scenario Integration

### Bundle Structure Changes

Each profile that hosts scenarios adds:

```
canary-{profile}/
└── libraries/
    └── handlers/
        ├── test-harness.ts            ← existing
        ├── scenario-harness.ts        ← NEW: shared scenario verdict builder
        ├── {existing handlers}        ← unchanged
        └── scenario-{name}.ts         ← NEW: one file per scenario
```

The implementation MAY split a scenario across multiple handler files if the complexity warrants it, but each scenario MUST have a single entry point that runs the complete workflow and returns the verdict.

### DataView and Schema Organization

Scenario DataViews and schemas are added to the existing profile's `app.toml` and `schemas/` directory. They share the profile's datasources. The implementation chooses names — there is no requirement to prefix or namespace scenario DataViews, but collisions with existing atomic test DataViews MUST be avoided.

### Init Handler Impact

If a scenario requires DDL (table creation), the SQL is added to the profile's existing init handler. The init handler runs once at app startup — it must be idempotent (`CREATE TABLE IF NOT EXISTS`).

### Dashboard Integration

`canary-main`'s SPA dashboard MUST display scenario verdicts. Scenarios appear as a separate section or tab, showing the step-by-step results with expand/collapse per step. The `type: "scenario"` field in the verdict distinguishes them from atomic tests.

The dashboard MUST show:
- Scenario name and overall pass/fail
- Step list with per-step pass/fail indicators
- `failed_at_step` highlighted for failed scenarios
- Skipped steps visually distinguished from failed steps
- Per-step assertion details on expand

---

## 9. Test Count Summary

| Profile | Scenario | Steps | Drivers | Total Step-Executions |
|---|---|---|---|---|
| SQL | Messaging | 12 | 3 (PG, MySQL, SQLite) | 36 |
| STREAM | Activity Feed | 11 | 1 (Kafka + SQL) | 11 |
| RUNTIME | Document Pipeline | 14 | 1 (filesystem + exec) | 14 |
| **Total** | **3 scenarios** | **37 steps** | | **61** |

Combined with the existing 107 atomic test endpoints, the canary fleet covers 168 total test points.

---

## 10. Implementation Notes

These are non-normative guidelines for the implementor.

### Simulating Multiple Users

Scenarios require multiple user identities (Alice, Bob, Carol). The implementation has two options:

1. **Session injection** — if the scenario handler has access to `ctx.session` manipulation (e.g., via a test-only API or by calling the guard view internally), it can switch identity between steps.
2. **Pre-seeded sessions** — the scenario creates three sessions during setup and includes the appropriate session cookie/token on each step's internal request.

The choice depends on what the runtime supports. Either approach is valid as long as identity isolation is verifiable.

### Timing in the Activity Feed

The Kafka consumer processes messages asynchronously. The scenario handler needs to wait for events to be persisted before querying history. A simple approach:

```
publish → poll history endpoint with exponential backoff (100ms, 200ms, 400ms, ...) → timeout at 5s
```

If the consumer hasn't processed within 5 seconds, the step fails with a timeout detail rather than hanging.

### Cleanup

The Messaging scenario should delete its test messages at the end. The Document Pipeline must delete its workspace. Cleanup failures are logged but do not affect the verdict — the scenario tests the application, not the cleanup.

### Parameter Binding Traps

The Messaging scenario's SQL tables MUST use the `zname`-style trap from the atomic tests. At least one column name must sort alphabetically before a column declared earlier in the parameter list. This trap has already caught one real bug (Issue #54) and must remain active in scenarios.

---

## CHANGELOG.md

Append to `canary-bundle/CHANGELOG.md`:

```markdown
## [Decision] — Scenario testing layer added
**File:** rivers-canary-scenarios-spec.md
**Description:** Added three scenario-based integration tests (Messaging, Activity Feed, Document Pipeline) that exercise Rivers features in composition. Scenarios are additive to existing atomic tests.
**Spec reference:** rivers-canary-scenarios-spec.md §1
**Resolution:** Scenarios use a loose-spec model — implementation decides all structural details. Verdicts use an extended envelope with per-step reporting.
```
