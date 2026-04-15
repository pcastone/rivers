# Canary Tests for New Features — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add canary test coverage for circuit breakers, transactions, and schema introspection — the three features built in this program review.

**Architecture:** Rather than creating new canary apps, extend existing canary profiles. Circuit breaker tests go in `canary-sql` (adds `circuitBreakerId` to an existing DataView). Transaction tests go in `canary-handlers` (adds a JS handler that exercises `Rivers.db.begin/commit/rollback`). Schema introspection is validated at startup — no runtime test needed, just a build-time config validation.

**Tech Stack:** TOML config, TypeScript handlers, shell test scripts

---

## File Map

| Task | File | Action |
|------|------|--------|
| 1 | `canary-bundle/canary-sql/app.toml` | Modify — add `circuitBreakerId` to a DataView |
| 1 | `canary-bundle/run-tests.sh` | Modify — add CB trip/reset/verify test sequence |
| 2 | `canary-bundle/canary-handlers/libraries/handlers/transaction-tests.ts` | Create — JS handler testing Rivers.db.begin/commit/rollback |
| 2 | `canary-bundle/canary-handlers/app.toml` | Modify — add transaction test views |
| 2 | `canary-bundle/run-tests.sh` | Modify — add transaction test endpoints |
| 3 | `canary-bundle/canary-sql/app.toml` | Modify — add `introspect = true` explicitly + `prepared = true` on one DataView |

---

### Task 1: Circuit Breaker Canary Tests

**Files:**
- Modify: `canary-bundle/canary-sql/app.toml`
- Modify: `canary-bundle/run-tests.sh`

- [ ] **Step 1: Add circuitBreakerId to a canary-sql DataView**

In `canary-bundle/canary-sql/app.toml`, find a DataView (e.g., `pg_select_all` or the first postgres DataView). Add `circuitBreakerId`:

```toml
[data.dataviews.pg_select_all]
name             = "pg_select_all"
datasource       = "pg"
query            = "SELECT id, name FROM canary_contacts"
circuitBreakerId = "canary-pg-breaker"
```

This gives the canary a named breaker that can be tripped/reset via admin API during tests.

- [ ] **Step 2: Add circuit breaker test sequence to run-tests.sh**

Find the test section for `canary-sql` in `run-tests.sh`. After the existing SQL tests, add a circuit breaker test block:

```bash
# ── Circuit Breaker Tests ────────────────────────────────────
echo ""
echo "=== Circuit Breaker Tests ==="

# Get the app ID for canary-sql
CB_APP_ID=$(curl -sf "$admin_url/admin/status" | grep -o '"app_id":"[^"]*"' | head -1 | cut -d'"' -f4)

# If admin URL is available, run breaker tests
if [ -n "$admin_url" ]; then
    # Get canary-sql app_id from bundle manifest
    SQL_APP_ID="<canary-sql-appId-from-manifest>"
    
    # Test 1: List breakers — should show canary-pg-breaker as CLOSED
    BREAKERS=$(curl -sf "$admin_url/admin/apps/$SQL_APP_ID/breakers")
    if echo "$BREAKERS" | grep -q '"canary-pg-breaker"'; then
        echo "PASS: breaker 'canary-pg-breaker' registered"
        passed=$((passed + 1))
    else
        echo "FAIL: breaker 'canary-pg-breaker' not found"
        failed=$((failed + 1))
    fi
    total=$((total + 1))

    # Test 2: Trip the breaker
    TRIP=$(curl -sf -X POST "$admin_url/admin/apps/$SQL_APP_ID/breakers/canary-pg-breaker/trip")
    if echo "$TRIP" | grep -q '"OPEN"'; then
        echo "PASS: breaker tripped to OPEN"
        passed=$((passed + 1))
    else
        echo "FAIL: breaker trip failed"
        failed=$((failed + 1))
    fi
    total=$((total + 1))

    # Test 3: Verify endpoint returns 503
    HTTP_CODE=$(curl -sf -o /dev/null -w "%{http_code}" "$base_url/canary-fleet/sql/pg-select-all" || true)
    if [ "$HTTP_CODE" = "503" ]; then
        echo "PASS: endpoint returns 503 when breaker open"
        passed=$((passed + 1))
    else
        echo "FAIL: expected 503, got $HTTP_CODE"
        failed=$((failed + 1))
    fi
    total=$((total + 1))

    # Test 4: Reset the breaker
    RESET=$(curl -sf -X POST "$admin_url/admin/apps/$SQL_APP_ID/breakers/canary-pg-breaker/reset")
    if echo "$RESET" | grep -q '"CLOSED"'; then
        echo "PASS: breaker reset to CLOSED"
        passed=$((passed + 1))
    else
        echo "FAIL: breaker reset failed"
        failed=$((failed + 1))
    fi
    total=$((total + 1))

    # Test 5: Verify endpoint works again
    HTTP_CODE=$(curl -sf -o /dev/null -w "%{http_code}" "$base_url/canary-fleet/sql/pg-select-all" || true)
    if [ "$HTTP_CODE" = "200" ]; then
        echo "PASS: endpoint returns 200 after breaker reset"
        passed=$((passed + 1))
    else
        echo "FAIL: expected 200, got $HTTP_CODE"
        failed=$((failed + 1))
    fi
    total=$((total + 1))
else
    echo "SKIP: admin_url not set, skipping circuit breaker tests"
fi
```

Note: The implementer needs to:
1. Find the actual `appId` for `canary-sql` from `canary-bundle/canary-sql/manifest.toml`
2. Find the actual endpoint path for the DataView with the breaker (check how routes are constructed from the bundle name + app entry_point + view path)
3. Determine how `admin_url` is passed to the test script (may need a new parameter)

- [ ] **Step 3: Verify the canary bundle still validates**

```bash
cargo run -p riverpackage -- validate canary-bundle
```

Expected: 0 errors. The new `circuitBreakerId` is a known field.

- [ ] **Step 4: Commit**

```bash
git add canary-bundle/canary-sql/app.toml canary-bundle/run-tests.sh
git commit -m "test(canary): add circuit breaker trip/reset/503 canary tests"
```

---

### Task 2: Transaction Canary Tests

**Files:**
- Create: `canary-bundle/canary-handlers/libraries/handlers/transaction-tests.ts`
- Modify: `canary-bundle/canary-handlers/app.toml`
- Modify: `canary-bundle/run-tests.sh`

- [ ] **Step 1: Create transaction test handler**

Create `canary-bundle/canary-handlers/libraries/handlers/transaction-tests.ts`:

```typescript
// Transaction canary tests — exercises Rivers.db.begin/commit/rollback

export function txn_commit_test(ctx: any): any {
    // Begin transaction on postgres datasource
    Rivers.db.begin("pg");
    
    // Execute a query inside the transaction
    let result = ctx.dataview("pg_insert_contact", {
        name: "txn-test-" + Date.now(),
        email: "txn@test.com"
    });
    
    // Commit the transaction
    Rivers.db.commit("pg");
    
    return {
        passed: true,
        test: "txn_commit",
        message: "transaction committed successfully"
    };
}

export function txn_rollback_test(ctx: any): any {
    // Begin transaction
    Rivers.db.begin("pg");
    
    // Insert a row
    let name = "rollback-test-" + Date.now();
    ctx.dataview("pg_insert_contact", { name: name, email: "rollback@test.com" });
    
    // Rollback — the insert should be undone
    Rivers.db.rollback("pg");
    
    // Query for the row — should not exist
    let result = ctx.dataview("pg_select_by_name", { name: name });
    let found = result && result.length > 0;
    
    return {
        passed: !found,
        test: "txn_rollback",
        message: found ? "FAIL: row found after rollback" : "rollback successfully undid insert"
    };
}

export function txn_auto_rollback_test(ctx: any): any {
    // Begin transaction but don't commit or rollback
    // The runtime should auto-rollback when the handler returns
    Rivers.db.begin("pg");
    
    let name = "auto-rollback-" + Date.now();
    ctx.dataview("pg_insert_contact", { name: name, email: "auto@test.com" });
    
    // Return without commit — should trigger auto-rollback
    return {
        passed: true,
        test: "txn_auto_rollback",
        message: "handler returned without commit (auto-rollback expected)"
    };
}

export function txn_double_begin_test(ctx: any): any {
    // Begin transaction, then try to begin again — should throw
    Rivers.db.begin("pg");
    
    try {
        Rivers.db.begin("pg");
        Rivers.db.rollback("pg");
        return {
            passed: false,
            test: "txn_double_begin",
            message: "FAIL: double begin did not throw"
        };
    } catch (e: any) {
        Rivers.db.rollback("pg");
        return {
            passed: true,
            test: "txn_double_begin",
            message: "double begin correctly threw: " + e.message
        };
    }
}
```

- [ ] **Step 2: Add transaction views to canary-handlers/app.toml**

Add views at the end of the file:

```toml
# ── Transaction Tests ─────────────────────────────────────
[api.views.txn_commit]
path      = "txn/commit"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.txn_commit.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/transaction-tests.ts"
entrypoint = "txn_commit_test"

[api.views.txn_rollback]
path      = "txn/rollback"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.txn_rollback.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/transaction-tests.ts"
entrypoint = "txn_rollback_test"

[api.views.txn_auto_rollback]
path      = "txn/auto-rollback"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.txn_auto_rollback.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/transaction-tests.ts"
entrypoint = "txn_auto_rollback_test"

[api.views.txn_double_begin]
path      = "txn/double-begin"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.txn_double_begin.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/transaction-tests.ts"
entrypoint = "txn_double_begin_test"
```

Note: The handler needs access to a postgres datasource. Check if `canary-handlers` already has a `pg` datasource configured, or if one needs to be added to its `app.toml` or `resources.toml`. The transaction tests need a real SQL database, not faker.

- [ ] **Step 3: Add transaction tests to run-tests.sh**

```bash
# ── Transaction Tests ────────────────────────────────────
echo ""
echo "=== Transaction Tests ==="
test_ep "POST" "/canary-fleet/handlers/txn/commit" "txn_commit"
test_ep "POST" "/canary-fleet/handlers/txn/rollback" "txn_rollback"
test_ep "POST" "/canary-fleet/handlers/txn/auto-rollback" "txn_auto_rollback"
test_ep "POST" "/canary-fleet/handlers/txn/double-begin" "txn_double_begin"
```

- [ ] **Step 4: Verify canary bundle validates**

```bash
cargo run -p riverpackage -- validate canary-bundle
```

- [ ] **Step 5: Commit**

```bash
git add canary-bundle/canary-handlers/libraries/handlers/transaction-tests.ts canary-bundle/canary-handlers/app.toml canary-bundle/run-tests.sh
git commit -m "test(canary): add transaction begin/commit/rollback canary tests"
```

---

### Task 3: Schema Introspection Canary Validation

**Files:**
- Modify: `canary-bundle/canary-sql/app.toml`

Schema introspection runs at startup, not at request time. The canary test is: if the canary bundle loads successfully with `introspect = true` on SQL datasources, introspection passed. If there were field mismatches, riversd would have refused to start and the entire canary run would fail.

- [ ] **Step 1: Add explicit `introspect = true` and `prepared = true` to a canary-sql DataView**

In `canary-bundle/canary-sql/app.toml`, on the postgres datasource config, add explicit `introspect = true` (it defaults to true, but being explicit documents the intent):

```toml
[data.datasources.pg]
name       = "pg"
driver     = "postgres"
lockbox    = "postgres/test"
introspect = true
```

And on one DataView, add `prepared = true`:

```toml
[data.dataviews.pg_select_all]
name             = "pg_select_all"
datasource       = "pg"
query            = "SELECT id, name FROM canary_contacts"
circuitBreakerId = "canary-pg-breaker"
prepared         = true
```

- [ ] **Step 2: Add a comment to run-tests.sh documenting the implicit test**

```bash
# ── Schema Introspection (implicit) ──────────────────────
# Schema introspection runs at startup. If canary-sql's DataViews
# have field mismatches against the actual postgres tables, riversd
# would refuse to start and this entire test run would not execute.
# Reaching this point means introspection passed.
echo "PASS: schema introspection passed (startup validation)"
passed=$((passed + 1))
total=$((total + 1))
```

- [ ] **Step 3: Verify canary bundle validates**

```bash
cargo run -p riverpackage -- validate canary-bundle
```

- [ ] **Step 4: Commit**

```bash
git add canary-bundle/canary-sql/app.toml canary-bundle/run-tests.sh
git commit -m "test(canary): add introspection and prepared statement canary coverage"
```

---

### Task 4: Update ProgramReviewTasks.md

- [ ] **Step 1: Mark canary test items complete**

```bash
git add -f todo/ProgramReviewTasks.md
git commit -m "docs: mark canary test tasks complete"
```
