# Rivers Spec Editing — Session Handoff

## State

Two amendments are complete and baked into the zip. Two remain.

---

## Completed This Session

| AMD | File | What was done |
|-----|------|---------------|
| AMD-2 | `rivers-streaming-rest-spec.md` | Removed `headers: { "x-api-key": await Rivers.lockbox.get("anthropic_key") }` from the `### 10.1 LLM token streaming` example (was line 467 in source) |
| AMD-7 | `rivers-streaming-rest-spec.md` | Inserted new `### 4.5 Rivers.view.stream()` section between existing 4.4 and old 4.5. Contains `StreamChunk<T>` type + API signature + LLM proxy example. Old 4.5 → 4.6, old 4.6 → 4.7. |

---

## Remaining Work (Execute Only — All Design Resolved)

### AMD-18 — `rivers-streaming-rest-spec.md`
**Insert a security callout block immediately before `### 10.1 LLM token streaming`.**

Content (verbatim from session):

> **Security note — CSRF on streaming POST**
>
> CSRF validation applies to streaming endpoints. Browser-mode session requests using POST to a streaming view require an `X-CSRF-Token` header — the streaming response does not affect CSRF validation of the request. The session middleware runs before the handler regardless of response type. API and mobile clients authenticating with `Authorization: Bearer` are exempt.

Follow the prose with a SPA `fetch()` example showing `X-CSRF-Token` alongside the streaming response reader:

```typescript
// Browser SPA — streaming POST with CSRF token
const response = await fetch("/api/llm/generate", {
    method: "POST",
    headers: {
        "Content-Type": "application/json",
        "X-CSRF-Token": getCsrfToken()   // required for browser-mode sessions
    },
    body: JSON.stringify({ prompt: "Hello", model: "claude-opus-4-5" })
});

const reader = response.body.getReader();
const decoder = new TextDecoder();

while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    const lines = decoder.decode(value).split("\n").filter(Boolean);
    for (const line of lines) {
        const chunk = JSON.parse(line);
        if (!chunk.done) process(chunk.token);
    }
}
```

This is purely documentary — AMD-6 already established CSRF validation in the session middleware. AMD-18 closes the ambiguity for readers who might assume streaming exempts the request from CSRF enforcement.

---

### AMD-17 — `rivers-http-driver-spec.md`
**Add `resources` enforcement rule to Section 11 (`## 11. Validation Rules`).**

1. Add this row to the validation table:

| Rule | Error message |
|---|---|
| `resources` names a datasource not declared in `data.datasources` | `unknown datasource '{name}' in resources` |

2. Add this prose block immediately after the table (before the `---` separator):

> **`resources` capability enforcement**
>
> The `resources` list on a handler config is validated at startup, not at dispatch time. Each name in `resources` must correspond to a key in `data.datasources`. If any name is unknown, Rivers refuses to start and emits the error above. At dispatch, the runtime enforces access: a handler that calls `Rivers.db` or `Rivers.resources.*` against a datasource not in its `resources` list receives a `CapabilityError` — the call does not proceed. This is the same enforcement described in the view layer spec (§3.2).

---

## File to Upload

Upload `rivers-docs-session-handoff.zip` — it contains all 16 spec files with AMD-2 and AMD-7 already applied.

## Relevant Grep Anchors

- AMD-18 insertion point: `grep -n "### 10.1 LLM token streaming" rivers-streaming-rest-spec.md`
- AMD-17 insertion point: `grep -n "^## 11\. Validation Rules" rivers-http-driver-spec.md`
- Section 11 table ends and `---` begins around line 506 — verify before editing.
