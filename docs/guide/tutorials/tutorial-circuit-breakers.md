# Tutorial: Circuit Breakers

**Rivers v0.54.0**

## Overview

Circuit breakers provide operators with manual control over traffic flow to groups of DataViews. By assigning a `circuitBreakerId` to one or more DataViews, you can instantly trip (disable) or reset (re-enable) them without restarting the server.

Circuit breakers enable three key operational patterns:

- **Isolate failing backends** — immediately stop all requests to a DataView group during an outage, preventing cascading failures and resource exhaustion
- **Staged restoration** — bring back parts of your application one breaker at a time after an incident, controlling the pace of traffic recovery
- **Maintenance windows** — disable specific services during planned maintenance without affecting the rest of the application

This tutorial covers adding circuit breakers to your app configuration, managing them with the `riversctl` CLI, and understanding the behavior when a breaker is tripped.

## Prerequisites

- A running Rivers instance (see the [Getting Started tutorial](tutorial-getting-started.md))
- A bundle with DataViews configured (or use the address-book bundle from the Getting Started tutorial)

---

## Step 1: Add Circuit Breakers to Your DataViews

Open your app's `app.toml` file and add the `circuitBreakerId` field to one or more DataViews:

```toml
[data.dataviews.search_inventory]
name              = "search_inventory"
datasource        = "warehouse-kafka"
circuitBreakerId  = "Warehouse_Transaction"

[data.dataviews.update_inventory]
name              = "update_inventory"
datasource        = "warehouse-kafka"
circuitBreakerId  = "Warehouse_Transaction"

[data.dataviews.product_lookup]
name              = "product_lookup"
datasource        = "postgres-catalog"
circuitBreakerId  = "Product_Catalog"
```

### Key Concepts

| Concept | Description |
|---------|-------------|
| **Breaker ID** | A free-form string you choose to group related DataViews (e.g., `"Warehouse_Transaction"`, `"Product_Catalog"`). IDs are scoped per app — two apps can use the same breaker ID without conflict. |
| **Grouping** | Multiple DataViews can share the same `circuitBreakerId`. When you trip the breaker, all DataViews in the group are affected. |
| **Optional** | The `circuitBreakerId` field is optional. DataViews without it are never affected by breakers. |

For the address-book example, let's add a single breaker to group the search and list operations:

```toml
[data.dataviews.search_contacts]
name              = "search_contacts"
datasource        = "faker"
circuitBreakerId  = "Contact_Service"

[data.dataviews.list_contacts]
name              = "list_contacts"
datasource        = "faker"
circuitBreakerId  = "Contact_Service"

[data.dataviews.get_contact]
name              = "get_contact"
datasource        = "faker"
# No breaker — this endpoint is always available
```

Reload the bundle to apply the changes:

```bash
/opt/rivers/bin/riversctl doctor --fix
/opt/rivers/bin/riversctl stop
/opt/rivers/bin/riversctl start
```

Verify the breakers were registered:

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --list
```

Expected output:

```
Contact_Service    CLOSED   (2 dataviews)
```

---

## Step 2: Understand Breaker States

Circuit breakers have two operational states:

| State | Behavior | Usage |
|-------|----------|-------|
| **Closed** | Requests pass through normally. The backend receives traffic. | Default state — normal operation. |
| **Open** | Requests are rejected immediately with 503 Service Unavailable. No backend calls are made. | Trip the breaker to stop traffic during incidents or maintenance. |

Breakers start in the **Closed** state unless you explicitly tripped them before the server restarted (breaker state is persisted and survives restarts).

---

## Step 3: Manage Breakers with riversctl

The `riversctl breaker` command lets you list, check, trip, and reset breakers.

### List All Breakers for an App

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --list
```

Output:

```
Contact_Service    CLOSED   (2 dataviews)
```

The `--app` flag accepts either the app name or its UUID.

### Check a Specific Breaker

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --name=Contact_Service
```

Output:

```
Contact_Service    CLOSED
DataViews: search_contacts, list_contacts
```

### Trip a Breaker (Disable Traffic)

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --name=Contact_Service --trip
```

Output:

```
Contact_Service    OPEN
DataViews: search_contacts, list_contacts
```

All requests to `search_contacts` and `list_contacts` now return 503 Service Unavailable. The backend is not called.

### Reset a Breaker (Re-enable Traffic)

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --name=Contact_Service --reset
```

Output:

```
Contact_Service    CLOSED
DataViews: search_contacts, list_contacts
```

Requests now pass through to the backend normally.

---

## Step 4: Test a Tripped Breaker

When a breaker is **open**, requests return a JSON error response with a `Retry-After` header.

### Create a test scenario

With the breaker in the **Open** state, make a request to one of the protected DataViews:

```bash
curl -k https://localhost:8080/my-app/my-app/search_contacts?query=john
```

### Expected Response

**Status:** 503 Service Unavailable

**Headers:**
```
Retry-After: 30
Content-Type: application/json
```

**Body:**
```json
{
  "error": "circuit breaker 'Contact_Service' is open",
  "breakerId": "Contact_Service",
  "retryable": true
}
```

The `retryable: true` field signals to clients that this is a transient condition — the client can retry after waiting.

### Reset and Verify Normal Operation

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --name=Contact_Service --reset
```

Now the same request works:

```bash
curl -k https://localhost:8080/my-app/my-app/search_contacts?query=john
```

**Status:** 200 OK

**Body:**
```json
[
  {
    "id": "contact-123",
    "name": "John Doe",
    "email": "john@example.com"
  }
]
```

---

## Step 5: Breaker Persistence and Restarts

Breaker state is **persisted** — a tripped breaker remains tripped even after `riversd` restarts.

### Test persistence

1. Trip a breaker:

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --name=Contact_Service --trip
```

2. Restart Rivers:

```bash
/opt/rivers/bin/riversctl stop
/opt/rivers/bin/riversctl start
```

3. Verify the breaker is still open:

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --name=Contact_Service
```

Output:

```
Contact_Service    OPEN
DataViews: search_contacts, list_contacts
```

The breaker is still tripped. You must explicitly reset it to restore traffic:

```bash
/opt/rivers/bin/riversctl breaker --app=my-app --name=Contact_Service --reset
```

This is intentional — circuit breakers are an explicit operator control, not automatic. Once tripped, they stay tripped until you decide to bring them back online.

---

## Step 6: Validation and Best Practices

### Validation During Bundle Load

When you deploy a bundle, `riverpackage validate` checks for potential issues. If a `circuitBreakerId` is referenced by only one DataView, you receive a warning:

```
WARN: circuitBreakerId 'Contact_Service' is referenced by only one DataView
      — did you mean 'Contact_Services'?
```

This suggests a possible typo (the Levenshtein distance algorithm compares your ID against other breaker IDs in the app). A single-DataView breaker is allowed but unusual — it usually indicates a configuration mistake.

**Fix:** Either group multiple DataViews under the same breaker ID, or remove the unused ID.

### Best Practices

1. **Group related operations** — Use the same breaker ID for DataViews that should fail or recover together. For example, all `Warehouse_Transaction` reads and writes.

2. **Use descriptive names** — Choose breaker IDs that reflect the backend or service being controlled: `"Search_Service"`, `"Kafka_Orders"`, `"Postgres_Catalog"`.

3. **Document your breakers** — Add comments to `app.toml` explaining why each breaker exists and what operations it protects.

4. **Test trip/reset procedures** — Periodically test tripping and resetting breakers in non-production environments to ensure your team is familiar with the process.

5. **Leave non-critical operations unprotected** — Not every DataView needs a circuit breaker. Protect backend calls but leave health check endpoints or metadata queries unprotected.

---

## Step 7: Automated Trip/Reset via Admin API

The `riversctl breaker` CLI commands internally call the admin API. You can also trip and reset breakers programmatically:

### List breakers

```bash
curl -k https://localhost:9090/admin/apps/my-app/breakers
```

Response:

```json
[
  {
    "breakerId": "Contact_Service",
    "state": "CLOSED",
    "dataviews": ["search_contacts", "list_contacts"]
  }
]
```

### Trip a breaker

```bash
curl -k -X POST https://localhost:9090/admin/apps/my-app/breakers/Contact_Service/trip
```

### Reset a breaker

```bash
curl -k -X POST https://localhost:9090/admin/apps/my-app/breakers/Contact_Service/reset
```

The admin API uses the same authentication as the main admin interface (configured in `riversd.toml`).

---

## Summary

This tutorial covered:

1. **Adding circuit breakers** — Use `circuitBreakerId` in `app.toml` to group DataViews under a single breaker
2. **Managing breakers with `riversctl`** — List, check, trip, and reset breakers via CLI
3. **Understanding breaker states** — Closed (normal) vs. Open (503 rejected)
4. **Testing tripped breakers** — Verify the 503 response and `Retry-After` header
5. **Persistence** — Breaker state survives server restarts
6. **Validation** — Single-DataView breakers trigger a warning (possible typo)
7. **Admin API** — Automate trip/reset operations programmatically

Circuit breakers give you fine-grained control over traffic during incidents, making them essential for operating reliable distributed systems with Rivers.
