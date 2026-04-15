# Rivers Circuit Breaker Specification — v1 (Manual Control)

**Document Type:** Implementation Specification
**Scope:** App-level manual circuit breakers for DataView dispatch
**Status:** Approved
**Version:** 1.0

---

## Table of Contents

1. [Overview](#1-overview)
2. [Config Schema](#2-config-schema)
3. [Breaker Registry & State](#3-breaker-registry--state)
4. [DataView Dispatch Behavior](#4-dataview-dispatch-behavior)
5. [Admin API](#5-admin-api)
6. [CLI Interface](#6-cli-interface)
7. [Validation & Error Handling](#7-validation--error-handling)
8. [Testing Strategy](#8-testing-strategy)
9. [Future: Auto-Trip (v2)](#9-future-auto-trip-v2)

---

## 1. Overview

Circuit breakers provide operators with manual control over which DataViews accept traffic. An operator can **trip** a breaker to immediately stop all requests to a group of DataViews, and **reset** it to restore traffic. This enables:

- **Targeted isolation** — trip `Warehouse_Transaction` on west coast app-servers without affecting east coast, even though both run the same bundle.
- **Staged restoration** — bring back parts of an application one breaker at a time after an outage.
- **Maintenance windows** — disable specific backends during planned work.

### Design Decisions

- **App-scoped** — each app has its own breaker namespace. Unique key is `appId:breakerId`.
- **Manual control only (v1)** — operators trip and reset explicitly via CLI or admin API. No automatic threshold-based tripping.
- **Persisted state** — breaker state survives riversd restarts via StorageEngine. A tripped breaker stays tripped until an operator resets it.
- **Breakers start closed** — all breakers default to closed (traffic flows) unless persisted state says otherwise.

---

## 2. Config Schema

DataViews opt into circuit breaker control via an optional `circuitBreakerId` attribute in `app.toml`:

```toml
[data.dataviews.search_inventory]
name              = "search_inventory"
datasource        = "kafka-west"
circuitBreakerId  = "Warehouse_Transaction"

[data.dataviews.update_inventory]
name              = "update_inventory"
datasource        = "kafka-west"
circuitBreakerId  = "Warehouse_Transaction"

[data.dataviews.product_lookup]
name              = "product_lookup"
datasource        = "postgres-catalog"
circuitBreakerId  = "Product_Catalog"
```

### Constraints

| ID | Rule |
|---|---|
| CFG-1 | `circuitBreakerId` is optional. DataViews without it are never affected by breakers. |
| CFG-2 | The ID is a free-form string chosen by the operator (e.g., `"Warehouse_Transaction"`, `"kafka-search"`). |
| CFG-3 | Multiple DataViews MAY share the same `circuitBreakerId`. Tripping the ID affects all of them. |
| CFG-4 | No separate breaker definition section exists. Breakers are implicitly registered when referenced by a DataView. |
| CFG-5 | `circuitBreakerId` values are scoped to the app. Two apps MAY use the same breaker ID string without conflict. |

---

## 3. Breaker Registry & State

### Registry Structure

At bundle load time, riversd scans all DataViews for `circuitBreakerId` values and builds a per-app registry:

- **Key:** `breakerId` (string)
- **Value:** `BreakerEntry` containing:
  - `state`: `Open` or `Closed`
  - `dataviews`: list of DataView names associated with this breaker

### Startup Sequence

1. Scan all DataViews in the app, collect unique `circuitBreakerId` values.
2. For each breaker ID, check StorageEngine for persisted state at key `rivers:breaker:{appId}:{breakerId}`.
3. If persisted state exists and is `Open`, start the breaker as open.
4. If no persisted state or state is `Closed`, start closed.
5. Log each breaker and its state: `breaker 'Warehouse_Transaction' loaded: CLOSED (3 dataviews)`.

### State Changes

- **Trip:** Set state to `Open`, write `"open"` to StorageEngine at `rivers:breaker:{appId}:{breakerId}` (no TTL), log the change.
- **Reset:** Set state to `Closed`, write `"closed"` to StorageEngine, log the change.

### Concurrency

The registry is wrapped in `Arc<RwLock<>>`. Request dispatch takes a read lock (cheap, concurrent). Trip/reset operations take a write lock (rare, brief). Contention is minimal because trips are infrequent operator actions.

### Constraints

| ID | Rule |
|---|---|
| REG-1 | Breaker IDs are registered implicitly from DataView config. No explicit breaker definition is required. |
| REG-2 | Breaker state MUST be persisted to StorageEngine on every state change. |
| REG-3 | Persisted state MUST be read at startup to restore breaker state across restarts. |
| REG-4 | StorageEngine keys use the reserved `rivers:` namespace prefix, which CodeComponents cannot access. |
| REG-5 | State writes use no TTL — breaker state persists until explicitly changed by an operator. |

---

## 4. DataView Dispatch Behavior

When a request hits a DataView that has a `circuitBreakerId`:

1. **Before** pool acquisition or handler execution, check the breaker registry for that ID.
2. If breaker is `Closed` — proceed normally.
3. If breaker is `Open` — short-circuit immediately:
   - HTTP status: **503 Service Unavailable**
   - Response body:
     ```json
     {
       "error": "circuit breaker 'Warehouse_Transaction' is open",
       "breakerId": "Warehouse_Transaction",
       "retryable": true
     }
     ```
   - Header: `Retry-After: 30`
   - Do NOT acquire a connection from the pool.
   - Do NOT execute any handler code.

DataViews without `circuitBreakerId` are completely unaffected — no registry lookup, no overhead.

### Constraints

| ID | Rule |
|---|---|
| DSP-1 | Breaker check MUST occur before pool connection acquisition. A tripped breaker MUST NOT consume pool resources. |
| DSP-2 | Breaker check MUST occur before handler/CodeComponent execution. |
| DSP-3 | Response MUST be 503 with `Retry-After: 30` header. |
| DSP-4 | Response body MUST include `breakerId` and `retryable: true` for client automation. |
| DSP-5 | DataViews without `circuitBreakerId` MUST NOT incur any breaker-related overhead. |

---

## 5. Admin API

Four endpoints, all under the admin API (default port 9090):

### List all breakers for an app

```
GET /admin/apps/{appId}/breakers
```

Response (200):
```json
[
  {
    "breakerId": "Warehouse_Transaction",
    "state": "CLOSED",
    "dataviews": ["search_inventory", "update_inventory", "update_stock"]
  },
  {
    "breakerId": "Product_Catalog",
    "state": "CLOSED",
    "dataviews": ["product_lookup"]
  }
]
```

### Get single breaker status

```
GET /admin/apps/{appId}/breakers/{breakerId}
```

Response (200):
```json
{
  "breakerId": "Warehouse_Transaction",
  "state": "CLOSED",
  "dataviews": ["search_inventory", "update_inventory", "update_stock"]
}
```

### Trip a breaker

```
POST /admin/apps/{appId}/breakers/{breakerId}/trip
```

Response (200):
```json
{
  "breakerId": "Warehouse_Transaction",
  "state": "OPEN",
  "dataviews": ["search_inventory", "update_inventory", "update_stock"]
}
```

### Reset a breaker

```
POST /admin/apps/{appId}/breakers/{breakerId}/reset
```

Response (200):
```json
{
  "breakerId": "Warehouse_Transaction",
  "state": "CLOSED",
  "dataviews": ["search_inventory", "update_inventory", "update_stock"]
}
```

### Error responses

- **404** if `appId` does not match any loaded app (by UUID or name).
- **404** if `breakerId` does not exist in the app's breaker registry.

### Constraints

| ID | Rule |
|---|---|
| API-1 | `{appId}` MUST accept either the app's UUID or its `appName`. |
| API-2 | Trip and reset MUST be idempotent — tripping an already-open breaker returns 200 with current state. |
| API-3 | All responses MUST include the `dataviews` list so the operator knows what is affected. |
| API-4 | Admin API authentication (if configured) applies to breaker endpoints. |

---

## 6. CLI Interface

```bash
# List all breakers for an app
riversctl breaker --app=us-west-inventory --list
# Output:
#   Warehouse_Transaction   CLOSED   (3 dataviews)
#   Product_Catalog          CLOSED   (1 dataview)

# Check one breaker
riversctl breaker --app=us-west-inventory --name=Warehouse_Transaction
# Output:
#   Warehouse_Transaction   CLOSED
#   DataViews: search_inventory, update_inventory, update_stock

# Trip it
riversctl breaker --app=us-west-inventory --name=Warehouse_Transaction --trip
# Output:
#   Warehouse_Transaction   OPEN
#   DataViews: search_inventory, update_inventory, update_stock

# Reset it
riversctl breaker --app=us-west-inventory --name=Warehouse_Transaction --reset
# Output:
#   Warehouse_Transaction   CLOSED
#   DataViews: search_inventory, update_inventory, update_stock
```

### Constraints

| ID | Rule |
|---|---|
| CLI-1 | `--app` is required for all breaker operations. Accepts appId (UUID) or appName. |
| CLI-2 | `--list` shows all breakers with state and DataView count. |
| CLI-3 | `--name` without `--trip` or `--reset` shows the breaker's current state and associated DataViews. |
| CLI-4 | `--trip` and `--reset` show the resulting state as confirmation. |
| CLI-5 | Exit code 0 on success, non-zero on error (unknown app, unknown breaker, admin API unreachable). |

---

## 7. Validation & Error Handling

### Bundle Validation (riverpackage validate + Gate 2)

If a `circuitBreakerId` is referenced by only one DataView in an app, emit a **warning** (not error):

```
WARN: circuitBreakerId 'Warehous_Transaction' is referenced by only one DataView
      — did you mean 'Warehouse_Transaction'?
```

The suggestion uses Levenshtein distance against other breaker IDs in the same app, matching the existing S002 "did you mean?" pattern. If no close match exists (distance too large), the warning omits the suggestion:

```
WARN: circuitBreakerId 'Maintenance_Lock' is referenced by only one DataView
```

This is a warning, not a hard fail — a single-DataView breaker is unusual but valid.

### Runtime Error Handling

| Scenario | Behavior |
|----------|----------|
| Trip a breaker that's already open | 200 OK, return current state (idempotent) |
| Reset a breaker that's already closed | 200 OK, return current state (idempotent) |
| StorageEngine unavailable on trip/reset | 500 error, log error, breaker state unchanged in memory |
| StorageEngine unavailable on startup | Log warning, start all breakers as closed (safe default) |
| Unknown appId in admin API | 404 |
| Unknown breakerId in admin API | 404 |

### Constraints

| ID | Rule |
|---|---|
| VAL-1 | Solo `circuitBreakerId` references MUST produce a warning with Levenshtein suggestion when a close match exists. |
| VAL-2 | Solo breaker warnings MUST NOT cause validation failure. |
| VAL-3 | Trip and reset MUST be idempotent — no error on redundant operations. |
| VAL-4 | StorageEngine failure on trip/reset MUST NOT change in-memory breaker state. The operation fails atomically. |
| VAL-5 | StorageEngine failure on startup MUST result in all breakers starting closed with a logged warning. |

---

## 8. Testing Strategy

### Unit Tests

- **Registry:** Create registry from DataView config, verify breaker IDs and associated DataView lists.
- **State transitions:** Trip sets state to Open, reset sets state to Closed.
- **Persistence round-trip:** Trip a breaker, write to StorageEngine, read back, verify state is Open.
- **Idempotent operations:** Trip twice returns same state both times. Reset twice returns same state both times.
- **Dispatch check:** Verify 503 response when breaker is open, normal dispatch when closed.
- **Validation:** Solo breaker ID triggers warning with Levenshtein suggestion. Multiple DataViews sharing an ID produces no warning.

### Integration Tests

- **Admin API:** All 4 endpoints return correct JSON structure. 404 for unknown app/breaker.
- **CLI:** `--list`, `--name`, `--trip`, `--reset` produce correct formatted output.
- **Persistence across restart:** Trip a breaker, restart riversd, verify breaker is still open via admin API.
- **Dispatch under breaker:** Send HTTP request to a DataView behind a tripped breaker. Verify 503 status, correct JSON body, `Retry-After` header.

### Canary Test Profile

- Add a `circuitBreakerId` to an existing canary DataView (e.g., in `canary-sql`).
- Canary endpoint that verifies breaker status via admin API.
- Canary endpoint that verifies 503 response when breaker is tripped.
- Trip/reset cycle via admin API during canary run.

---

## 9. Future: Auto-Trip (v2)

Not in scope for v1. Planned additions:

- Threshold-based auto-tripping (failure count/rate within time window).
- Config for trip thresholds, recovery strategy, half-open probing.
- `mode = "auto" | "manual" | "both"` attribute on breaker config.
- Auto-tripped breakers still show in `riversctl breaker --list` with an `AUTO` indicator.
- Manual reset overrides auto-trip state.
