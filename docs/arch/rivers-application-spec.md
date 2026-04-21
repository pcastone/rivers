# Rivers Application Specification

**Component:** Rivers Application Model  
**Version:** v1  
**Status:** Implementation-ready  
**Scope:** app-main, app-service, app-bundle structure, manifest.json, resources.json, deployment lifecycle, service resolution, preflight  
**Depends On:** LockBox v1 spec, HTTP driver spec, ProcessPool spec, riversd.conf  
**Deferred to v2:** RPS service registry, multi-node app-service routing  

---

## Table of Contents

1. [Overview](#1-overview)
2. [Application Types](#2-application-types)
3. [App Bundle Structure](#3-app-bundle-structure)
4. [Bundle Manifest](#4-bundle-manifest)
5. [App Manifest](#5-app-manifest)
6. [resources.json](#6-resourcesjson)
7. [Service Resolution](#7-service-resolution)
8. [Deployment Lifecycle](#8-deployment-lifecycle)
9. [Startup Order](#9-startup-order)
10. [Auth Scope Carry-Over](#10-auth-scope-carry-over)
11. [Optional Resources](#11-optional-resources)
12. [Preflight Check](#12-preflight-check)
13. [Module Resolution](#13-module-resolution)
14. [Validation Rules](#14-validation-rules)
15. [Examples](#15-examples)

---

## 1. Overview

A Rivers application is a deployable unit that runs inside a `riversd` process. A single `riversd` instance hosts multiple applications simultaneously. Applications are isolated from each other — datasources, routes, ProcessPool resources, and session state are all scoped per application.

Applications are packaged as an **app-bundle** — a zip file containing one or more apps and a bundle manifest. The bundle is the deployment unit. riversd deploys the entire bundle atomically.

```
Developer writes app
    │
    ▼
Build tool compiles + packages → bundle.zip
    │
    ├─ build tool --pre-flight → checklist of required resources
    │       ops provisions LockBox aliases, verifies drivers
    │
    ▼
bundle.zip deployed to riversd
    │
    ├─ riversd assigns appDeployId per app
    ├─ riversd resolves resources
    ├─ riversd starts app-services first
    └─ riversd starts app-mains after app-services healthy
```

---

## 2. Application Types

### 2.1 app-service

A backend RESTful service. Stateless. No frontend assets. Owns its own datasources. Exposes an HTTP API consumed by app-main or other app-services.

Characteristics:
- Declares datasources directly (PostgreSQL, Redis, Kafka, etc.)
- No SPA or static file serving
- Binds to a dedicated port
- Identified as `"type": "app-service"` in its manifest
- Name resolves to `{appname}-srv` at runtime

### 2.2 app-main

The primary application. Hosts the SPA and provides a service discovery endpoint. The SPA calls app-services directly — app-main does **not** proxy requests to app-services.

Characteristics:
- Serves SPA static assets
- Exposes a `services` discovery endpoint listing available app-services and their route prefixes
- Owns the auth guard — session established here carries over to app-service calls
- Identified as `"type": "app-main"` in its manifest
- Does **not** declare curl/HTTP datasources for app-services — the SPA calls service routes directly

### 2.3 Relationship

```
app-bundle.zip
    │
    ├─ app-main          (one per bundle — the entry point for users)
    │       │
    │       ├─ SPA assets
    │       ├─ Auth guard
    │       └─ /services endpoint → tells SPA where to find app-services
    │
    ├─ orders-service    (app-service)
    │       └─ postgresql, redis datasources
    │
    └─ inventory-service (app-service)
            └─ postgresql datasource
```

### 2.4 URL Routing Scheme

All routes are namespaced automatically by bundle and app entry point:

```
<host>:<port>/[route_prefix]/<bundle_entryPoint>/<entryPoint>/<view_name>
```

| Segment | Source | Required |
|---------|--------|----------|
| `route_prefix` | Operator-configured in `riversd.toml` | No (optional) |
| `bundle_entryPoint` | `bundleName` from bundle `manifest.toml` | Yes |
| `entryPoint` | `entryPoint` from app `manifest.toml` (a name, not a URL) | Yes |
| `view_name` | View name from `app.toml` (a name, not a path) | Yes |

Example (no prefix):
```
GET /address-book/service/contacts       → address-book-service list_contacts
GET /address-book/service/contacts/42    → address-book-service get_contact
GET /address-book/main/                  → SPA index.html
GET /address-book/main/services          → service discovery JSON
```

Example (with `route_prefix = "v1"`):
```
GET /v1/address-book/service/contacts    → address-book-service list_contacts
GET /v1/address-book/main/               → SPA index.html
```

This eliminates route collisions between apps — each app has its own namespace.

---

## 3. App Bundle Structure

The bundle is a standard zip file. The top-level contains `manifest.json` and one directory per app. Each app directory contains its own `manifest.json`, `resources.json`, and `libraries/` directory.

```
bundle.zip
├── manifest.json                    # bundle-level manifest
├── app-main/
│   ├── manifest.json                # app-main manifest
│   ├── resources.json               # app-main resource declarations
│   └── libraries/                   # TS/JS/WASM source or compiled files
│       ├── handlers/
│       │   ├── auth.ts
│       │   └── orders.ts
│       ├── shared/
│       │   └── utils.ts
│       └── spa/                     # SPA static assets
│           ├── index.html
│           ├── main.js
│           └── styles.css
├── orders-service/
│   ├── manifest.json
│   ├── resources.json
│   └── libraries/
│       ├── handlers/
│       │   ├── orders.ts
│       │   └── fulfillment.ts
│       └── shared/
│           └── models.ts
└── inventory-service/
    ├── manifest.json
    ├── resources.json
    └── libraries/
        └── handlers/
            └── inventory.ts
```

### 3.1 libraries/

Contains all files the app needs to execute — TypeScript source, compiled JavaScript, WASM binaries, and any supporting assets. The build tool determines what goes here. riversd treats the contents as opaque — it does not compile or transform files at deploy time. What is in `libraries/` is what runs.

WASM files (`.wasm`) are loaded directly. TypeScript files (`.ts`) are transpiled by the ProcessPool's V8 runtime at first load and cached. JavaScript files (`.js`) are loaded directly.

### 3.2 spa/

SPA static assets live inside `libraries/spa/` by convention. The app manifest declares the SPA root path and index file. riversd serves them as static files on the app's bound port.

---

## 4. Bundle Manifest

The bundle-level `manifest.json` identifies the bundle and lists the apps it contains. It is the entry point riversd reads when a bundle is deployed.

### 4.1 Schema

```json
{
  "bundleName":  "orders-platform",
  "bundleVersion": "1.4.2",
  "source":      "https://github.com/acme/orders-platform",
  "apps": [
    "app-main",
    "orders-service",
    "inventory-service"
  ]
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `bundleName` | string | yes | Human-readable bundle name |
| `bundleVersion` | string | yes | Semantic version of the bundle |
| `source` | string | yes | Origin URL — git repo, artifact URL, or file path. Stamped by build tool. Stored in riversd at deploy time for audit. |
| `apps` | string[] | yes | Directory names of apps in this bundle. Order is informational — riversd determines startup order from app types. |

### 4.2 Source field

The `source` field is stamped by the build tool at package time. It is not used by riversd for active pulling — it is provenance metadata. riversd stores it against the `appDeployId` and exposes it via the admin API for audit and traceability.

---

## 5. App Manifest

Each app directory contains its own `manifest.json` declaring the app's identity, type, and entry point.

### 5.1 Schema

```json
{
  "appName":       "orders-service",
  "description":   "Order management and fulfillment service",
  "version":       "2.1.0",
  "type":          "app-service",
  "appId":         "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "entryPoint":    "service",
  "appEntryPoint": "https://orders.internal.acme.com",
  "source":        "https://github.com/acme/orders-platform/orders-service"
}
```

```json
{
  "appName":       "app-main",
  "description":   "Orders platform main application",
  "version":       "2.1.0",
  "type":          "app-main",
  "appId":         "a3f8c21d-9b44-4e71-b823-1c04d5e6f789",
  "entryPoint":    "main",
  "appEntryPoint": "https://orders.acme.com",
  "source":        "https://github.com/acme/orders-platform/app-main",
  "spa": {
    "root":       "libraries/spa",
    "indexFile":  "index.html",
    "fallback":   true,
    "maxAge":     86400
  }
}
```

### 5.2 Fields

| Field | Type | Required | Owner | Description |
|---|---|---|---|---|
| `appName` | string | yes | Developer | Human-readable app name. Used to construct service name: `{appName}-srv`. |
| `description` | string | no | Developer | Human-readable description. |
| `version` | string | yes | Developer | Semantic version. |
| `type` | string | yes | Developer | `"app-main"` or `"app-service"`. |
| `appId` | UUID | yes | Build tool | Stable identity. Generated once by build tool. Never changes. Used by riversd to match datasources across redeployments. |
| `entryPoint` | string | yes | Developer | Route name for this app. Used as a URL segment: `/<bundle_entryPoint>/<entryPoint>/...`. Must be a simple name (no slashes, no URLs). |
| `appEntryPoint` | string | no | Developer | Public URL — load balancer or external address. Informational only. Stored at deploy time. |
| `source` | string | yes | Build tool | Stamped by build tool at package time. Stored by riversd at deploy time. |
| `spa` | object | no | Developer | SPA config. Only valid on `app-main`. |

### 5.3 entryPoint naming rules

`entryPoint` is a simple name used as a URL segment in the routing scheme. It is **not** a URL or bind address.

| entryPoint value | Resulting route prefix |
|---|---|
| `"service"` | `/<bundle>/<b>service</b>/...` |
| `"main"` | `/<bundle>/<b>main</b>/...` |
| `"orders"` | `/<bundle>/<b>orders</b>/...` |

Rules:
- Must be a non-empty string
- Must not contain `/`, `?`, `#`, or whitespace
- Must be unique within a bundle — two apps cannot share the same `entryPoint`
- Convention: `"main"` for app-main, descriptive name for app-services (e.g. `"service"`, `"orders"`, `"inventory"`)

### 5.4 appId stability

`appId` is generated by the build tool the first time an app is created and committed to source control alongside the manifest. It does not change on version bumps, refactors, or redeployments. riversd uses `appId` to:

- Match datasources from a previous deployment of the same app (preserve objectIds)
- Resolve service dependencies declared in other apps' `resources.json`
- Track deployment history per app across bundle versions

### 5.5 appDeployId

`appDeployId` is assigned by riversd at deploy time. It is unique per deployment instance — the same app redeployed gets a new `appDeployId`. riversd uses `appDeployId` internally to scope:

- Route namespacing
- ProcessPool pool assignment
- Session namespace
- Service name construction: `{appName}-srv` is backed by `appDeployId`

`appDeployId` is exposed via the admin API and stored in deployment history. Developers and ops use it to identify specific running instances.

---

## 6. resources.json

Each app declares its runtime dependencies in `resources.json`. This is the contract between the developer and the deployment environment. The developer declares what is needed. The deployment tool (via `--pre-flight`) and ops provision what is needed. riversd resolves resources at deploy time.

### 6.1 Schema

```json
{
  "datasources": [
    {
      "name":     "orders-db",
      "driver":   "postgresql",
      "lockbox":  "postgres/orders-prod",
      "required": true
    },
    {
      "name":     "cache",
      "driver":   "redis",
      "lockbox":  "redis/prod",
      "required": false
    },
    {
      "name":     "event-stream",
      "driver":   "kafka",
      "lockbox":  "kafka/prod",
      "required": true
    }
  ],
  "services": [
    {
      "name":     "orders-service",
      "appId":    "f47ac10b-58cc-4372-a567-0e02b2c3d479",
      "required": true
    },
    {
      "name":     "inventory-service",
      "appId":    "b2e9d31a-7c55-4f82-c934-2d15e6f7a890",
      "required": false
    }
  ]
}
```

### 6.2 Datasource fields

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Datasource name within this app. Used in view `resources` arrays and `Rivers.db.query()` calls. Scoped to this app — same name in another app is a different datasource. |
| `driver` | string | yes | Driver type: `postgresql`, `mysql`, `redis`, `mongodb`, `kafka`, `http`, `sqlite`, `elasticsearch`, etc. Must be a registered driver in riversd. |
| `lockbox` | string | yes | LockBox alias or name. Resolved at startup via the keystore. Format: `{category}/{identifier}` or any alias declared in the keystore. |
| `required` | bool | yes | If `true`, app will not start if this datasource cannot be resolved. If `false`, app starts but datasource is unavailable — queries return `null`. |

### 6.3 Service fields

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Logical service name. Used in view `resources` arrays to reference the service as a curl datasource. |
| `appId` | UUID | yes | The `appId` of the target app-service. riversd resolves `appId` → `appDeployId` → service endpoint at deploy time. |
| `required` | bool | yes | If `true`, app will not start if the target app-service is not running. If `false`, app starts but service calls return `null`. |

### 6.4 Datasource objectId

When riversd deploys a datasource declared in `resources.json`, it assigns an **objectId** to the datasource instance. The objectId is generated at first deploy and stored by riversd against the app's `appId` + datasource `name`. On subsequent deployments of the same app (same `appId`), riversd matches the datasource by `appId` + `name` and reuses the existing objectId — preserving pool state and avoiding unnecessary reconnections.

objectId is an internal riversd concern — it does not appear in `resources.json` or any developer-facing config. It is exposed via the admin API for diagnostics.

### 6.5 Developer vs ops boundary

`resources.json` is the developer's declaration. It says what is needed but not where it is. The actual connection parameters (host, port, database name) are the ops concern — they live in LockBox as the full connection string under the declared alias.

```
Developer declares:        driver = "postgresql", lockbox = "postgres/orders-prod"
Ops provides in LockBox:   postgres/orders-prod = "postgresql://user:pass@db.internal:5432/orders"
riversd connects using:    the resolved connection string
```

The developer never writes a host or port. The ops team never touches application code.

---

## 7. Service Resolution

### 7.1 Route-based resolution

Services are resolved by the URL routing scheme — not by HTTP proxying. Each app in a bundle is routed under its own namespace:

```
/<bundle_entryPoint>/<entryPoint>/<view_name>
```

When app-main declares a service dependency on `orders-service` in `resources.json`, riversd resolves the dependency at deploy time to verify the target app exists in the bundle. The SPA discovers service routes at runtime via the **services discovery endpoint**.

### 7.2 Services discovery endpoint

Every `app-main` automatically exposes a discovery endpoint:

```
GET /<bundle_entryPoint>/<main_entryPoint>/services
```

Response:

```json
[
  { "name": "orders-service", "url": "/orders-platform/orders" },
  { "name": "inventory-service", "url": "/orders-platform/inventory" }
]
```

The `url` is the route prefix for that service — the SPA appends view names to it. Built from `/<bundleName>/<service_entryPoint>`.

The SPA fetches this on load and calls service endpoints directly — no HTTP proxy layer, no curl datasource.

### 7.3 Service name construction

The service name in the discovery response is the `name` from the `services` array in `resources.json`. The URL is derived from the target app's `entryPoint` and the bundle's `bundleName`.

### 7.4 Direct client-to-service calls

The SPA calls app-services directly via the route namespace. There is no server-side proxy between app-main and app-services. Both run under the same `riversd` instance on the same host and port.

```
SPA loads:      GET /orders-platform/main/
SPA discovers:  GET /orders-platform/main/services → [{"name":"orders-service","url":"/orders-platform/orders"}]
SPA calls:      GET /orders-platform/orders/list    → orders-service handles directly
```

Benefits:
- No HTTP driver needed for inter-app communication
- No route collisions — each app has its own namespace
- Observable — all calls are standard HTTP to the same origin
- Simple — no proxy configuration, no curl datasources for services

### 7.5 Unresolved service at deploy time

If `required = true` and the target `appId` is not in the bundle, deployment fails:

```
RiversError::Deploy: service dependency "orders-service" (appId: f47ac10b-...)
  is not deployed in this riversd instance.
  Deploy orders-service first, or include it in the same bundle.
```

If `required = false` and the target `appId` is not in the bundle, the app starts. The service is omitted from the `/services` discovery response. A `WARN` is logged at startup.

---

## 8. Deployment Lifecycle

### 8.1 Deploy states

Each deployed app transitions through states managed by riversd:

```
PENDING → RESOLVING → STARTING → RUNNING
                │                   │
                └─ FAILED           └─ STOPPING → STOPPED
```

| State | Description |
|---|---|
| `PENDING` | Bundle received, queued for deployment |
| `RESOLVING` | riversd resolving resources — LockBox aliases, datasource objectIds, service endpoints |
| `STARTING` | Resources resolved, app initializing — connection pools warming, ProcessPool loading libraries |
| `RUNNING` | App is live and serving traffic |
| `FAILED` | Resolution or startup failed. Error logged. App not serving. |
| `STOPPING` | Graceful drain in progress |
| `STOPPED` | App stopped. Port released. Resources released. |

### 8.2 appDeployId assignment

riversd assigns `appDeployId` when the bundle is received (PENDING state). It is stable for the lifetime of that deployment — it does not change as the app moves through states.

### 8.3 Redeployment

Redeploying the same app (same `appId`) in a new bundle version:

1. New bundle received → new `appDeployId` assigned
2. riversd resolves resources for new version
3. New version enters STARTING state
4. Old version enters STOPPING (graceful drain)
5. New version enters RUNNING
6. Old version enters STOPPED

Zero-downtime by default. In-flight requests to the old version complete before it stops.

### 8.4 Admin API endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/admin/deploy` | Upload bundle zip and begin deployment |
| `GET` | `/admin/deployments` | List all deployments and their states |
| `GET` | `/admin/deployments/{appDeployId}` | Get deployment detail — state, resources, objectIds |
| `POST` | `/admin/deployments/{appDeployId}/stop` | Gracefully stop a running app |
| `DELETE` | `/admin/deployments/{appDeployId}` | Remove a stopped app |

---

## 9. Startup Order

riversd determines startup order from app types and declared service dependencies.

### 9.1 Order rules

1. All `app-service` apps start before any `app-main` app
2. Within app-services, riversd starts them in parallel unless dependency order can be inferred from service declarations (app-service A declares a service dependency on app-service B → B starts first)
3. An `app-main` starts only after all its `required = true` app-service dependencies are in RUNNING state
4. If a required app-service fails to start, dependent app-mains enter FAILED state

### 9.1.1 Single-node enforcement (without RPS)

<!-- SHAPE-8 amendment: sentinel key for single-node enforcement -->
Without RPS, Rivers is single-node only. On startup with a Redis-backed StorageEngine, `riversd` enforces single-node operation:

1. Check for existing `rivers:node:*` keys in Redis
2. If found, hard failure: `"Another Rivers node detected on this Redis instance. Multi-node requires RPS."`
3. Write `rivers:node:{node_id}` with a TTL heartbeat
4. Key expires naturally on crash or clean shutdown

This prevents accidental multi-node deployments sharing a Redis instance without RPS coordination. In-memory and SQLite StorageEngine backends are inherently single-node and do not require this check.

### 9.2 Startup sequence

```
bundle deployed
    │
    ├─ Phase 0: single-node enforcement (Redis sentinel key check) <!-- SHAPE-8 -->
    │
    ├─ Phase 1: resolve all resources for all apps
    │       any required resource unresolvable → that app enters FAILED
    │
    ├─ Phase 2: start app-services (parallel where no inter-service dependencies)
    │       orders-service → STARTING → RUNNING
    │       inventory-service → STARTING → RUNNING
    │
    └─ Phase 3: start app-mains (after required app-services RUNNING)
            app-main → STARTING → RUNNING
```

### 9.3 Health check before promotion

Before an app-service is considered RUNNING and before dependent app-mains are allowed to start, riversd verifies the app's views are registered and reachable under its route namespace:

```
GET /[route_prefix]/<bundle_entryPoint>/<entryPoint>/health
```

Standard Rivers health endpoint. Returns 200 → service is RUNNING. riversd retries with exponential backoff for up to `startup_timeout_s` (default: 30s). Timeout → FAILED.

---

## 10. Auth Scope Carry-Over

### 10.1 Session forwarding

When app-main makes an HTTP call to an app-service via a curl datasource, riversd automatically forwards the session token. The developer does not add auth headers — Rivers handles it.

What is forwarded:
- Session token as `Authorization: Bearer {session_token}` header
- Session claims as `X-Rivers-Claims: {base64-encoded-json}` header

### 10.2 App-service session validation

App-service views declared as protected (`auth = "session"`) validate the forwarded token against StorageEngine. The session was created by app-main's guard. The app-service reads the same StorageEngine (shared Redis in multi-node, shared in-process store in single-node) — the session is valid across both apps.

The app-service does not run its own guard. It validates the Bearer token and reads the claims. `Rivers.session.current` is populated in the app-service CodeComponent with the same identity that app-main established.

### 10.3 X-Rivers-Claims header

The `X-Rivers-Claims` header carries the full `IdentityClaims` payload as base64-encoded JSON. This allows app-service views that don't need to re-validate the full session (e.g., `auth = "none"` public views that still want identity context) to read the claims directly:

```typescript
// In an app-service CodeComponent
const claims = req.headers["x-rivers-claims"]
    ? JSON.parse(atob(req.headers["x-rivers-claims"]))
    : null;
```

`X-Rivers-Claims` is set by riversd on the outbound call — it cannot be spoofed by external clients. riversd strips any inbound `X-Rivers-Claims` header on the app's entryPoint listener before routing to handlers.

### 10.4 Unauthenticated requests

If app-main makes a curl datasource call without an active session (public view, `auth = "none"`), no `Authorization` header is forwarded. The app-service receives the request without session context. If the app-service view is protected, it returns 401. If `auth = "none"`, it handles the request without session context.

---

## 11. Optional Resources

### 11.1 Startup behavior

Required resources (`"required": true`) must resolve for the app to enter RUNNING state. If any required resource fails:
- App enters FAILED state
- All other resources for that app are released
- Error logged with specific resource name and failure reason

Optional resources (`"required": false`) that fail to resolve:
- App continues to RUNNING state
- `WARN` logged at startup: `optional resource '{name}' unavailable — queries will return null`
- App-service health endpoint reflects degraded state

### 11.2 Runtime behavior

When a CodeComponent queries an unavailable optional datasource or service:

```typescript
const rows = await Rivers.db.query("cache", "GET", [key]);
// cache is optional and unavailable
// rows === null
// WARN logged: "query on unavailable optional resource 'cache' — returning null"
```

The CodeComponent receives `null`. It is the developer's responsibility to handle `null` gracefully. Rivers does not throw — it returns `null` and logs.

### 11.3 Health endpoint degraded state

The app's health endpoint (`GET {entryPoint}/health`) returns degraded status when optional resources are unavailable:

```json
{
  "status":  "degraded",
  "appName": "orders-service",
  "appDeployId": "...",
  "resources": {
    "orders-db":    { "status": "ok" },
    "cache":        { "status": "unavailable", "required": false },
    "event-stream": { "status": "ok" }
  }
}
```

HTTP status is `200` for degraded (app is running). `503` is reserved for FAILED or STOPPING states.

---

## 12. Preflight Check

### 12.1 Purpose

The build tool's `--pre-flight` flag validates that all resources declared in the bundle's `resources.json` files can be provisioned before deployment. It runs against the target riversd instance and LockBox — not against a live app. Its output is a checklist for ops.

### 12.2 Invocation

```bash
rivers pack --pre-flight \
  --bundle bundle.zip \
  --lockbox /etc/rivers/lockbox.age \
  --identity /etc/rivers/lockbox.identity \
  --riversd http://localhost:8900
```

### 12.3 Output

```
Rivers Pre-flight Check — orders-platform v1.4.2
================================================

app-service: orders-service
  Datasources:
    ✓ orders-db       driver=postgresql  lockbox=postgres/orders-prod  [RESOLVED]
    ✗ cache           driver=redis       lockbox=redis/prod             [MISSING — lockbox alias not found]
    ✓ event-stream    driver=kafka       lockbox=kafka/prod             [RESOLVED]

app-service: inventory-service
  Datasources:
    ✓ inventory-db    driver=postgresql  lockbox=postgres/inventory     [RESOLVED]

app-main: app-main
  Services:
    ✓ orders-service     appId=f47ac10b-...  [FOUND — deployed]
    ✓ inventory-service  appId=b2e9d31a-...  [FOUND — deployed]
  Datasources:
    ✓ auth-db         driver=postgresql  lockbox=postgres/auth          [RESOLVED]

RESULT: 1 issue found
  ACTION REQUIRED:
    - Add lockbox alias "redis/prod" to keystore:
      lockbox add redis/prod --keystore /etc/rivers/lockbox.age
```

### 12.4 Pre-flight checks performed

| Check | Pass condition |
|---|---|
| LockBox alias exists | Alias or name found in keystore |
| Driver registered | Driver type is a registered driver in target riversd |
| Service dependency deployed | Target `appId` is in RUNNING state in target riversd |
| ~~Port available~~ | ~~`entryPoint` port not already bound by another app~~ — **removed** <!-- SHAPE-19 amendment: OS reports bind failures at startup, no preflight port check --> |
| Bundle structure valid | All declared app directories present, manifests parseable |
| appId unique | No two apps in bundle share an `appId` |

### 12.5 Pre-flight exit codes

| Code | Meaning |
|---|---|
| 0 | All checks passed |
| 1 | One or more required resources missing |
| 2 | Bundle structure invalid |
| 3 | Cannot connect to riversd or LockBox |

Pre-flight is advisory for optional resources — missing optional resources produce a warning, not a failure exit code.

---

## 13. Module Resolution

> **See also:** [`rivers-javascript-typescript-spec.md`](./rivers-javascript-typescript-spec.md) — the authoritative spec for the swc-based TypeScript compilation pipeline, module resolution algorithm (Deno-style explicit extensions, bundle-cache residency boundary), source maps, and MCP view TOML format. This section covers the app-level configuration surface; the JS/TS spec covers the runtime behaviour.

### 13.1 Base path

All module paths in view configs are relative to the app's directory root inside the bundle. The app directory root is the directory containing `manifest.json` and `resources.json`.

```toml
# In a view config for orders-service
[api.views.create_order.handler]
module     = "libraries/handlers/orders.ts"
entrypoint = "createOrder"
```

The path `libraries/handlers/orders.ts` resolves to `orders-service/libraries/handlers/orders.ts` inside the bundle zip.

### 13.2 Cross-module imports

CodeComponents within the same app may import from other files in the same app's `libraries/` directory using relative paths:

```typescript
// libraries/handlers/orders.ts
import { validateOrder } from "../shared/models.ts";
import { formatResponse } from "../shared/utils.ts";
```

Imports must resolve within the same app's `libraries/` directory. Cross-app imports are not permitted — app-service A cannot import from app-service B's libraries. Service composition is via HTTP, not code sharing.

### 13.3 WASM modules

WASM files are declared in view configs the same way as TypeScript:

```toml
[api.views.process.handler]
type       = "codecomponent"
language   = "wasm"
module     = "libraries/compute/processor.wasm"
entrypoint = "process"
```

WASM files must be self-contained — they cannot import other WASM modules at runtime. All imports are resolved at dispatch time from the same app's `libraries/` directory.

---

## 14. Validation Rules

Enforced at deploy time before any app enters STARTING state.

| Rule | Error |
|---|---|
| `type` is not `app-main` or `app-service` | `invalid app type '{type}' in {appName}/manifest.json` |
| `appId` missing or not a UUID | `appId is required and must be a UUID in {appName}/manifest.json` |
| Two apps in bundle share `appId` | `duplicate appId '{id}' in {appA} and {appB}` |
| ~~`entryPoint` port already bound~~ | ~~`port {port} is already bound by '{appName}'`~~ — **removed**, OS bind failure handles this <!-- SHAPE-19 amendment --> |
| `required` datasource LockBox alias not found | `required resource '{name}' lockbox alias '{alias}' not found in keystore` |
| `required` service `appId` not deployed | `required service '{name}' (appId: {id}) is not running` |
| `spa` declared on `app-service` | `spa config is only valid on app-main` |
| Module path not found in bundle | `module '{path}' not found in {appName}/libraries/` |
| `resources` in view references undeclared resource | `resource '{name}' not declared in {appName}/resources.json` |
| Bundle missing `manifest.json` at root | `bundle is missing root manifest.json` |
| App directory missing `manifest.json` | `{appName}/ is missing manifest.json` |
| App directory missing `resources.json` | `{appName}/ is missing resources.json` |

---

## 15. Examples

### 15.1 Complete bundle structure — orders platform

```
orders-platform-v1.4.2.zip
├── manifest.json
├── app-main/
│   ├── manifest.json
│   ├── resources.json
│   └── libraries/
│       ├── handlers/
│       │   ├── auth.ts
│       │   └── proxy.ts
│       └── spa/
│           ├── index.html
│           ├── main.js
│           └── styles.css
├── orders-service/
│   ├── manifest.json
│   ├── resources.json
│   └── libraries/
│       ├── handlers/
│       │   ├── orders.ts
│       │   └── fulfillment.ts
│       └── shared/
│           └── models.ts
└── inventory-service/
    ├── manifest.json
    ├── resources.json
    └── libraries/
        └── handlers/
            └── inventory.ts
```

### 15.2 Bundle manifest.json

```json
{
  "bundleName":    "orders-platform",
  "bundleVersion": "1.4.2",
  "source":        "https://github.com/acme/orders-platform/releases/tag/v1.4.2",
  "apps": [
    "app-main",
    "orders-service",
    "inventory-service"
  ]
}
```

### 15.3 app-main manifest.json

```json
{
  "appName":       "app-main",
  "description":   "Orders platform — main application and SPA host",
  "version":       "1.4.2",
  "type":          "app-main",
  "appId":         "a3f8c21d-9b44-4e71-b823-1c04d5e6f789",
  "entryPoint":    "https://0.0.0.0",
  "appEntryPoint": "https://orders.acme.com",
  "source":        "https://github.com/acme/orders-platform/app-main",
  "spa": {
    "root":      "libraries/spa",
    "indexFile": "index.html",
    "fallback":  true,
    "maxAge":    86400
  }
}
```

### 15.4 app-main resources.json

```json
{
  "datasources": [
    {
      "name":     "auth-db",
      "driver":   "postgresql",
      "lockbox":  "postgres/auth",
      "required": true
    }
  ],
  "services": [
    {
      "name":     "orders-service",
      "appId":    "f47ac10b-58cc-4372-a567-0e02b2c3d479",
      "required": true
    },
    {
      "name":     "inventory-service",
      "appId":    "b2e9d31a-7c55-4f82-c934-2d15e6f7a890",
      "required": false
    }
  ]
}
```

### 15.5 orders-service manifest.json

```json
{
  "appName":       "orders-service",
  "description":   "Order management and fulfillment",
  "version":       "1.4.2",
  "type":          "app-service",
  "appId":         "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "entryPoint":    "http://0.0.0.0:9001",
  "appEntryPoint": "https://orders-svc.internal.acme.com",
  "source":        "https://github.com/acme/orders-platform/orders-service"
}
```

### 15.6 orders-service resources.json

```json
{
  "datasources": [
    {
      "name":     "orders-db",
      "driver":   "postgresql",
      "lockbox":  "postgres/orders-prod",
      "required": true
    },
    {
      "name":     "cache",
      "driver":   "redis",
      "lockbox":  "redis/prod",
      "required": false
    },
    {
      "name":     "event-stream",
      "driver":   "kafka",
      "lockbox":  "kafka/prod",
      "required": true
    }
  ],
  "services": []
}
```

### 15.7 Startup log output

```
INFO  rivers::deploy: deploying bundle "orders-platform" v1.4.2
INFO  rivers::deploy: appDeployId assigned — orders-service:    deploy-7f3a1b2c
INFO  rivers::deploy: appDeployId assigned — inventory-service: deploy-4e8d9c1a
INFO  rivers::deploy: appDeployId assigned — app-main:          deploy-2b5f8e3d

INFO  rivers::deploy: resolving resources — orders-service
INFO  rivers::lockbox: resolved lockbox://postgres/orders-prod
WARN  rivers::lockbox: optional resource 'cache' lockbox alias 'redis/prod' not found — starting degraded
INFO  rivers::lockbox: resolved lockbox://kafka/prod

INFO  rivers::deploy: starting app-services (phase 2)
INFO  rivers::app: orders-service    → STARTING (port 9001)
INFO  rivers::app: inventory-service → STARTING (port 9002)
INFO  rivers::app: orders-service    → RUNNING  (health check passed)
INFO  rivers::app: inventory-service → RUNNING  (health check passed)

INFO  rivers::deploy: starting app-mains (phase 3)
INFO  rivers::app: app-main → STARTING (port 443)
INFO  rivers::app: app-main → RUNNING

INFO  rivers::deploy: bundle "orders-platform" v1.4.2 deployed successfully
  orders-service    deploy-7f3a1b2c  RUNNING  :9001  [degraded: cache unavailable]
  inventory-service deploy-4e8d9c1a  RUNNING  :9002
  app-main          deploy-2b5f8e3d  RUNNING  :443
```
