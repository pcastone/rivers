# Rivers Platform Standards Alignment Specification

**Document Type:** Spec Addition  
**Scope:** OpenAPI, probe semantics, OpenTelemetry export, runtime transaction completion, standards-based API auth, AsyncAPI  
**Status:** Design / Team Review  
**Patches:** `rivers-httpd-spec.md`, `rivers-view-layer-spec.md`, `rivers-logging-spec.md`, `rivers-auth-session-spec.md`, `rivers-driver-spec.md`, `rivers-data-layer-spec.md`, `rivers-streaming-rest-spec.md`  
**Depends On:** HTTPD, View Layer, DataView Engine, ProcessPool Runtime, Session/Auth, Driver SDK, EventBus

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [Goals and Non-Goals](#2-goals-and-non-goals)
3. [Standards Alignment Overview](#3-standards-alignment-overview)
4. [OpenAPI Support](#4-openapi-support)
5. [Liveness, Readiness, and Startup Probes](#5-liveness-readiness-and-startup-probes)
6. [OpenTelemetry Export](#6-opentelemetry-export)
7. [Runtime Transaction and Batch Completion](#7-runtime-transaction-and-batch-completion)
8. [Standards-Based API Authentication](#8-standards-based-api-authentication)
9. [AsyncAPI Support](#9-asyncapi-support)
10. [Cross-Cutting Validation Rules](#10-cross-cutting-validation-rules)
11. [Rollout Plan](#11-rollout-plan)
12. [Open Questions for Review](#12-open-questions-for-review)

---

## 1. Purpose

Rivers already provides a strong declarative runtime for HTTP-first application services: REST, streaming REST, SSE, WebSocket, GraphQL, sessions, CSRF, metrics, rate limiting, circuit breakers, and a broad datasource layer.

The missing pieces are not the basics of request handling. The missing pieces are the platform-standard surfaces that production teams now expect by default:

1. OpenAPI for REST contracts
2. Kubernetes-style liveness, readiness, and startup probes
3. OpenTelemetry export for traces and correlated telemetry
4. Completed runtime transaction and batch APIs for handlers
5. Standards-based API authentication for service and public API use
6. AsyncAPI for event-driven and streaming contracts

This document proposes a coherent v1 platform-alignment plan for those six areas.

---

## 2. Goals and Non-Goals

### 2.1 Goals

- Make Rivers easier to adopt in modern platform environments without changing its declarative core.
- Close the biggest production-readiness gaps identified by comparison with current microservice and web-service standards.
- Prefer generated and derived contracts from Rivers config over hand-maintained parallel documents.
- Preserve backward compatibility wherever practical.
- Keep Rivers HTTP-first while adding standards that improve interoperability and operations.

### 2.2 Non-Goals

- This document does **not** introduce gRPC support.
- This document does **not** redesign service discovery or add service mesh APIs.
- This document does **not** replace guard/session auth; it adds standards-based options alongside it.
- This document does **not** introduce full distributed cluster orchestration.
- This document does **not** require every existing app to opt into every new feature.

---

## 3. Standards Alignment Overview

| Area | Rivers Today | Proposed Change | Priority |
|---|---|---|---|
| REST contracts | Declarative views and schemas, but no first-class OpenAPI surface | Generate and serve OpenAPI from app config | P0 |
| Health semantics | `/health` and `/health/verbose` only | Add `/live`, `/ready`, `/startup` with distinct semantics | P0 |
| Telemetry export | Structured logs, trace IDs, Prometheus metrics | Add OpenTelemetry export for traces first, metrics/logs second | P1 |
| Handler runtime completeness | `Rivers.db.begin/commit/rollback/batch` stubs remain | Wire runtime APIs to real transaction and batch execution | P1 |
| API auth | Guard/session model, app-owned credential validation | Add declarative JWT, OIDC discovery, and API key verification modes | P1 |
| Event contracts | Messaging and streaming supported, but no AsyncAPI contract | Generate AsyncAPI documents from broker and stream config | P2 |

### 3.1 Design Principle

For all six features, Rivers should remain the source of truth. The operator or app author declares behavior once in Rivers config. Standards documents and runtime endpoints are derived from that declaration.

---

## 4. OpenAPI Support

### 4.1 Problem

Rivers can already describe REST endpoints declaratively, but it does not expose that metadata in the standard format used by client generators, API gateways, test tooling, and governance platforms.

This creates a tooling gap:

- no generated API contract for client SDKs
- no standard docs artifact for review
- no easy inventory of app surfaces
- no straightforward integration with API linting and contract testing tools

### 4.2 Proposal

Every app with REST views can optionally expose a generated OpenAPI document derived from:

- `app.toml` view declarations
- DataView parameter declarations
- request and response schemas
- auth requirements
- route metadata

Rivers generates the document at bundle load and serves it as a standard endpoint.

### 4.3 New Capability

Add per-app OpenAPI config:

```toml
[api.openapi]
enabled = true
path = "/openapi.json"         # default
title = "Orders Service API"   # default: appName
version = "1.2.0"              # default: app manifest version
include_playground = false     # reserved for future HTML viewer
```

### 4.4 Generation Rules

- Only `view_type = Rest` participates in OpenAPI v1.
- WebSocket, SSE, MessageConsumer, and streaming REST are excluded from the OpenAPI document and handled separately in AsyncAPI.
- Each REST view maps to one OpenAPI path + method operation.
- Request parameters derive from Rivers parameter mappings:
  - path parameters -> OpenAPI `in: path`
  - query parameters -> OpenAPI `in: query`
  - header mappings -> OpenAPI `in: header`
- Request body derives from input schema when present.
- Response body derives from output schema when present.
- `auth = "none"` maps to no security requirement.
- standards-based auth modes from Section 8 map to OpenAPI `securitySchemes`.

### 4.5 Required Metadata Additions

Add optional descriptive fields on views and apps:

```toml
[api.views.list_orders]
summary = "List recent orders"
description = "Returns the most recent orders for the authenticated tenant."
tags = ["orders"]
operation_id = "listOrders"
deprecated = false
```

If omitted:

- `summary` defaults to view name
- `operation_id` defaults to a deterministic generated identifier
- `tags` defaults to app name

### 4.6 Served Endpoints

For an app with `api.openapi.enabled = true`:

- `GET /<bundle>/<app>/openapi.json`

If `route_prefix` is configured, the endpoint is namespaced accordingly.

### 4.7 Validation Rules

- OpenAPI generation fails bundle validation only if `enabled = true` and the document cannot be generated deterministically.
- Missing schemas are allowed, but the generated operation must omit body schema details rather than invent them.
- Two views may not resolve to the same OpenAPI path + method pair.
- `operation_id` values must be unique within an app when explicitly declared.

### 4.8 Out of Scope for v1

- HTML Swagger UI/ReDoc bundling
- code sample generation
- OpenAPI callbacks
- non-REST view types

---

## 5. Liveness, Readiness, and Startup Probes

### 5.1 Problem

`/health` and `/health/verbose` are useful but do not model the semantics expected by Kubernetes and other orchestrators:

- liveness: should this process be restarted?
- readiness: should this instance receive traffic?
- startup: has boot completed successfully?

Today Rivers has one basic endpoint and one verbose endpoint. That is not enough for standard deployment behavior.

### 5.2 Proposal

Add three new system endpoints:

- `GET /live`
- `GET /ready`
- `GET /startup`

Existing `/health` and `/health/verbose` remain for backward compatibility.

### 5.3 Probe Semantics

#### `/live`

Purpose: process liveness.

Returns 200 when:

- HTTP server is running
- event loop is responsive
- process is not in terminal failure state

Returns non-200 only for unrecoverable local process failure.

Datasource state does **not** affect `/live`.

#### `/ready`

Purpose: traffic readiness.

Returns 200 when:

- startup completed
- bundle loaded successfully
- app routing is installed
- required subsystems are available enough to serve traffic
- server is not draining

Returns 503 when:

- startup has not completed
- bundle/app load failed
- required subsystems are unavailable
- the instance is draining and should be removed from rotation

#### `/startup`

Purpose: bootstrap completion.

Returns 200 only after:

- config validation completed
- bundle load completed
- all required apps and required datasources initialized
- runtime registration complete

Returns 503 before startup completes.

After startup succeeds, `/startup` remains 200 for the life of the process.

### 5.4 Relationship to Existing Endpoints

- `/health` remains a lightweight human/operator endpoint.
- `/health/verbose` remains a diagnostics endpoint.
- `/live`, `/ready`, `/startup` are machine-oriented orchestration endpoints.

### 5.5 New Config

```toml
[base.probes]
enabled = true
live_path = "/live"
ready_path = "/ready"
startup_path = "/startup"
```

Default:

- enabled = true
- default paths as above

### 5.6 Validation Rules

- Probe paths may not collide with app view routes or existing system routes.
- `/ready` must respect graceful shutdown and failed-app isolation state.
- `/startup` must not flip back to failed after successful completion in v1. Ongoing readiness degradation is represented via `/ready`, not `/startup`.

---

## 6. OpenTelemetry Export

### 6.1 Problem

Rivers has trace IDs and Prometheus metrics, but no standard telemetry export for distributed tracing and service correlation. That makes Rivers hard to operate in polyglot environments where OpenTelemetry is the default observability substrate.

### 6.2 Proposal

Add optional OpenTelemetry export with phased scope:

- v1: traces
- v1.1: metrics bridge alignment
- v1.2: structured log export hooks

Traces are the initial priority because they solve the biggest cross-service visibility problem with the smallest scope.

### 6.3 New Config

```toml
[telemetry.otel]
enabled = true
service_name = "orders-service"       # default: appName
service_version = "1.2.0"             # default: manifest version
environment = "production"            # default: base.environment
exporter = "otlp_http"                # "otlp_http" | "otlp_grpc"
endpoint = "http://otel-collector:4318"
headers = { "Authorization" = "Bearer ${OTEL_TOKEN}" }
sample_ratio = 1.0
propagate_w3c = true
```

### 6.4 Trace Model

Rivers already has request trace IDs and W3C `traceparent` support. OTel export extends that model instead of replacing it.

The following operations must produce spans:

- inbound HTTP request lifecycle
- DataView execution
- datasource execution
- handler execution
- streaming view lifetime
- broker message consumption
- outbound HTTP driver execution

### 6.5 Span Attributes

Minimum required attributes:

- `service.name`
- `service.version`
- `deployment.environment`
- `http.request.method`
- `url.path`
- `http.response.status_code`
- `rivers.app.id`
- `rivers.view.id`
- `rivers.dataview.name` when applicable
- `db.system` or messaging attributes when applicable

### 6.6 Propagation Rules

- Rivers must continue to honor inbound W3C `traceparent`.
- If a request arrives without `traceparent`, Rivers creates a root span and synthesizes standard propagation headers downstream.
- The HTTP driver must forward propagated trace context on outbound requests unless explicitly disabled.

### 6.7 Failure Policy

Telemetry export must never fail a request path.

- exporter failure -> log warning
- buffer overflow -> drop telemetry with counter increment
- malformed config when `enabled = true` -> startup validation error

### 6.8 Out of Scope for v1

- full log signal export
- collector management
- vendor-specific trace enrichment

---

## 7. Runtime Transaction and Batch Completion

### 7.1 Problem

The runtime surface exposes transaction and batch APIs, but parts remain stubbed. This creates a dangerous mismatch between documented capability and actual behavior.

The missing APIs are:

- `Rivers.db.begin`
- `Rivers.db.commit`
- `Rivers.db.rollback`
- `Rivers.db.batch`

### 7.2 Proposal

Complete the handler/runtime transaction model by wiring these APIs to real engine and DataView execution paths.

### 7.3 Transaction Model

Transactions are datasource-scoped within a single handler execution context.

Rules:

- A transaction token is created by `begin(datasource)`.
- All transactional operations for that datasource in the current handler bind to the active token.
- `commit` finalizes the transaction.
- `rollback` aborts the transaction.
- On handler failure, any open transaction in the request context is rolled back automatically.

### 7.4 Batch Model

`Rivers.db.batch(dataview, params[])` executes the same DataView multiple times under a single dispatch call.

Batch behavior:

- preserve input order
- return ordered result array
- support partial failure policy declaration

New optional API behavior:

```typescript
await Rivers.db.batch("createOrder", [
  { id: "1" },
  { id: "2" }
], { onError: "fail_fast" })   // default
```

Supported policies:

- `fail_fast`
- `collect_errors`

### 7.5 Config and Validation

Transactions require datasource capability:

- SQL datasources: allowed if driver supports transactions
- non-transactional datasources: validation/runtime error with `Unsupported`

Batch requires:

- existing DataView name
- parameter set array
- validation of each batch item against DataView input rules

### 7.6 Required Runtime Guarantees

- no fake success for unimplemented transaction operations
- no silent no-op behavior
- consistent rollback on timeout or execution termination where possible
- clear error reporting when a driver does not support transactions

---

## 8. Standards-Based API Authentication

### 8.1 Problem

Rivers intentionally treats application credential validation as app-owned, which keeps the core flexible. That remains valid.

However, for modern production APIs, teams also expect built-in support for common standards:

- JWT bearer validation
- OIDC discovery-backed JWT validation
- API key validation

Without these, every team must rebuild the same auth edge logic in handlers.

### 8.2 Proposal

Add declarative API auth modes alongside the existing guard/session model.

v1 auth modes:

- `none`
- `session`
- `jwt`
- `oidc`
- `api_key`

Guard/session remains the best fit for browser login flows. JWT, OIDC, and API key target service APIs and machine clients.

### 8.3 New View Config

```toml
[api.views.list_orders]
auth = "jwt"

[api.views.list_orders.auth_config]
provider = "internal-jwt"
required_scopes = ["orders.read"]
```

### 8.4 Provider Config

```toml
[security.auth_providers.internal-jwt]
type = "jwt"
issuer = "https://auth.example.com"
audience = "orders-api"
jwks_url = "https://auth.example.com/.well-known/jwks.json"

[security.auth_providers.company-oidc]
type = "oidc"
issuer = "https://login.example.com"
audience = "orders-api"
discovery_url = "https://login.example.com/.well-known/openid-configuration"

[security.auth_providers.partner-key]
type = "api_key"
header = "x-api-key"
storage_namespace = "api_keys"
hash = "sha256"
```

### 8.5 Runtime Behavior

#### JWT

- extract bearer token
- validate signature
- validate issuer, audience, expiry, not-before
- expose claims on `ctx.session`-like auth context or `ctx.auth`

#### OIDC

- fetch discovery metadata
- resolve JWKS
- cache signing keys with TTL
- validate token as JWT after discovery

#### API key

- extract configured header
- compare against hashed or stored key material
- resolve mapped identity and scopes

### 8.6 Authorization Surface

All standards-based modes support:

- required scopes
- required roles
- optional custom claim mapping

Example:

```toml
[api.views.list_orders.auth_config]
provider = "company-oidc"
required_scopes = ["orders.read"]
required_roles = ["support", "admin"]
subject_claim = "sub"
roles_claim = "roles"
scopes_claim = "scope"
```

### 8.7 Compatibility Rules

- `auth = "session"` remains unchanged.
- Guard views continue to use session semantics only.
- Standards-based auth and session auth are mutually exclusive per view in v1.
- Hybrid auth modes are out of scope for v1.

### 8.8 Security Rules

- remote key fetches must use TLS
- JWKS and discovery fetches must be cached
- auth provider misconfiguration is a startup validation error
- auth failure returns structured 401 or 403 envelopes

---

## 9. AsyncAPI Support

### 9.1 Problem

Rivers already supports event-driven and streaming patterns:

- Kafka
- RabbitMQ
- NATS
- SSE
- WebSocket
- streaming REST

But it lacks a standard machine-readable event contract.

That makes event consumers harder to build, review, and govern than REST consumers.

### 9.2 Proposal

Add optional AsyncAPI document generation for broker-backed and stream-backed interfaces.

AsyncAPI v1 scope includes:

- MessageConsumer views
- broker datasource topics, subjects, exchanges, queues
- SSE event names
- WebSocket message channels

Streaming REST is documented only where the stream shape is stable and declarative enough to derive.

### 9.3 New Config

```toml
[api.asyncapi]
enabled = true
path = "/asyncapi.json"
title = "Orders Events"
version = "1.0.0"
```

### 9.4 Document Sources

AsyncAPI generation derives from:

- datasource driver type
- consumer binding config
- message schemas
- event trigger names
- stream metadata declared on views

### 9.5 Coverage Rules

#### Kafka / RabbitMQ / NATS

- channels derive from topic / exchange+routing key / subject
- publish vs subscribe direction derives from Rivers role
- message payload schema derives from schema files

#### SSE

- one channel per SSE view
- event names derive from configured trigger names when declared
- payload schema derives from associated DataView/output schema when deterministic

#### WebSocket

- one channel per WebSocket view
- direct vs broadcast mode exposed as extension metadata
- payload schemas derive from message schema declarations when available

### 9.6 Validation Rules

- AsyncAPI generation is optional.
- If `enabled = true`, missing required channel metadata for a covered protocol is a validation error only when Rivers claims that protocol in the document.
- If schema shape cannot be derived safely, Rivers must omit the payload schema rather than invent one.

### 9.7 Out of Scope for v1

- complete streaming REST modeling
- CloudEvents binding generation
- callback/webhook inversion
- protocol-specific vendor extensions beyond minimal Rivers extensions

---

## 10. Cross-Cutting Validation Rules

### 10.1 No Invented Contracts

Rivers must never fabricate schema details that are not derivable from config and known runtime declarations.

### 10.2 Opt-In by Default

OpenAPI, AsyncAPI, and OTel export are opt-in in v1.

Probe endpoints are on by default.

### 10.3 Backward Compatibility

Existing apps with no new config continue to behave as they do today.

### 10.4 Structured Error Policy

For all new machine-facing endpoints and auth failures:

- use Rivers structured JSON error envelope for non-streaming errors
- use existing wire-format error rules for streaming paths

### 10.5 Validation Timing

- config shape errors -> startup validation error
- derivation conflicts -> startup validation error
- runtime backend/transient failures -> logged runtime errors, not startup failures

---

## 11. Rollout Plan

### Phase 1

- OpenAPI generation and serving
- `/live`, `/ready`, `/startup`

### Phase 2

- OTel trace export
- runtime transaction and batch completion

### Phase 3

- JWT, OIDC, and API key auth providers

### Phase 4

- AsyncAPI generation

### Phase 5

- follow-on polish:
  - OpenAPI UI
  - OTel metrics/log signal integration
  - richer AsyncAPI bindings

---

## 12. Open Questions for Review

1. Should OpenAPI and AsyncAPI documents be generated per app only, or should Rivers also support bundle-level aggregate documents?
2. Should `/ready` degrade on any required datasource failure, or only on startup-time failure plus explicit breaker/open states?
3. Should OTel traces be the only v1 requirement, with metrics/log export deferred explicitly to separate specs?
4. Should `Rivers.db.batch` expose partial-failure semantics in v1, or ship with `fail_fast` only?
5. For standards-based auth, should `ctx.auth` be introduced as a new runtime object, or should validated claims reuse `ctx.session` shape for consistency?
6. Should AsyncAPI v1 include SSE and WebSocket immediately, or start with broker-backed consumers only?
7. Should OpenAPI generation be strict enough to fail on undocumented response schemas, or permissive enough to emit partial documents?

---

## Recommended Review Outcome

Approve this document as the platform-alignment umbrella spec, then split implementation into six child execution specs in the rollout order above.
