# Git Changelog — Rivers Runtime

Generated from git commit history.

---

## Foundation

### 13e6b06 — Initial commit
Date: 2026-01-21

### 54e9df3 — Add address-book-bundle with full declarative config
Date: 2026-03-13

Reference implementation: two apps (address-book-service on port 9100, address-book-main on port 8080) with faker datasource, HTTP proxy, and Svelte SPA.

---

## Epic 1: Project Bootstrap & Workspace

### 24d50d6 — Add Cargo workspace with four crates for Rivers runtime bootstrap
Date: 2026-03-14

Created `rivers-core`, `rivers-driver-sdk`, `rivers-data`, `riversd` crates with shared dependencies (tokio, serde, axum, tracing, etc.).

### 70734ce — Reorganize project: add spec docs, remove old bundle and script stubs
Date: 2026-03-14

---

## Epic 2: Configuration Parsing & Validation

### ec02ba1 — Add configuration parsing, validation, and environment overrides
Date: 2026-03-14

ServerConfig, DatasourceConfig, DataViewConfig, ApiViewConfig structs. Bundle/app manifest parsing, config validation, environment overrides.

---

## Epic 3: EventBus

### 562279e — Add EventBus with priority-tiered dispatch and 35 event constants
Date: 2026-03-14

In-process pub/sub with Critical (awaited), Standard (awaited), Observe (fire-and-forget) priority tiers. Per-topic broadcast channels.

---

## Epic 4: StorageEngine

### 584f08b — Add StorageEngine trait and InMemory backend with KV + queue ops
Date: 2026-03-14

Namespace+key KV pattern, TTL in milliseconds, enqueue/dequeue/ack queue operations.

---

## Epic 5: Logging & Observability

### 4c9bd3d — Add structured logging, redaction, and trace ID correlation
Date: 2026-03-14

### 0ae69be — Remove OTel/OTLP dead code — spec updated to drop distributed tracing
Date: 2026-03-14

---

## Epic 6: LockBox

### 4af1309 — Add LockBox secret management — Age-encrypted keystore with O(1) resolver
Date: 2026-03-14

Age-encrypted local keystore for secrets. Resolved at startup, never enters ProcessPool.

---

## Epic 7: Driver SDK Core

### c5e3293 — Complete Driver SDK core contracts — Query inference, 15 tests
Date: 2026-03-14

DatabaseDriver trait, Query/QueryResult types, QueryValue inference.

---

## Epic 8: Broker Contracts

### 996debc — Add broker contracts — MessageBrokerDriver, consumer/producer traits
Date: 2026-03-14

MessageBrokerDriver, BrokerConsumer, BrokerProducer traits.

---

## Epic 9: Driver Factory & Plugin System

### d45ca43 — Add DriverFactory and plugin system — registry, libloading, ABI check
Date: 2026-03-14

DriverFactory registry, dynamic loading via libloading, ABI version check.

---

## Epic 10: Built-in Database Drivers

### 2aff4b4 — Epic 10: Built-in database drivers with FakerDriver and honest stubs
Date: 2026-03-14

FakerDriver generates synthetic data. PostgreSQL, MySQL, SQLite, Redis, Elasticsearch drivers as honest stubs.

---

## Epic 11: Plugin Driver Crates

### 87e54d4 — Epic 11: Plugin driver crates with ABI exports and honest stubs
Date: 2026-03-14

MongoDB, Elasticsearch, Kafka, RabbitMQ, NATS, Redis Streams, InfluxDB plugin crates.

---

## Epic 12: HTTP Driver

### 7558bb1 — Epic 12: HTTP driver types, traits, templating, and response mapping
Date: 2026-03-14

HTTP/HTTP2/WebSocket/SSE as first-class datasource. URL templating, response mapping.

---

## Epic 13: Broker Consumer Bridge

### 3b6872d — Epic 13: Broker Consumer Bridge with failure policies and graceful drain
Date: 2026-03-14

Dead-letter, retry, and skip failure policies. Graceful drain for shutdown.

---

## Epic 14: Pool Manager

### b8d5c95 — Epic 14: Pool Manager with circuit breaker, health checks, and credential rotation
Date: 2026-03-14

Per-datasource connection pooling. Circuit breaker (closed/open/half-open). Health check probes. Credential rotation.

---

## Epic 15: Schema System

### 47f3014 — Epic 15: Schema system with driver-aware attribute validation
Date: 2026-03-14

JSON schema with driver-specific attributes (faker, sql, elasticsearch, etc.).

---

## Epic 16: DataView Engine

### 9b6e45a — Epic 16: DataView Engine with parameter validation and request builder
Date: 2026-03-14

Named parameterized queries. Parameter validation, request building, datasource dispatch.

---

## Epic 17: DataView Cache

### edba080 — Epic 17: Two-tier DataView cache with L1 LRU and L2 StorageEngine
Date: 2026-03-14

L1 in-memory LRU cache + L2 StorageEngine persistence. TTL-based expiry.

---

## Epic 18: HTTP Server

### 4faeb44 — Epic 18: Axum-based HTTP server with middleware stack and graceful shutdown
Date: 2026-03-14

Axum router with 10-layer middleware stack (compression, body limit, security headers, session, rate limit, shutdown guard, backpressure, timeout, request observer, trace ID). Graceful shutdown with drain.

---

## Epic 19: Rate Limiting & Backpressure

### 652c156 — Epic 19: Token bucket rate limiting and semaphore backpressure
Date: 2026-03-14

Per-IP token bucket rate limiter. Semaphore-based backpressure with configurable max concurrent requests.

---

## Epic 20: CORS

### 942a392 — Epic 20: CORS origin matching and handler header blocklist
Date: 2026-03-14

Origin pattern matching (exact, wildcard subdomain). Preflight handling. Header blocklist enforcement.

---

## Epic 21: Static Files

### 6c0948e — Epic 21: Static file serving with SHA-256 ETag, SPA fallback, and path traversal prevention
Date: 2026-03-14

SHA-256 ETag generation. SPA fallback to index.html. Path traversal prevention. 20 MIME types. Cache-Control headers.

---

## Epic 22: Sessions

### c6522d9 — Epic 22: Session management with dual expiry, cookie/Bearer auth, and StorageEngine backing
Date: 2026-03-14

Dual expiry (absolute TTL + idle timeout). Cookie/Bearer token extraction. StorageEngine persistence. Session middleware.

---

## Epic 23: Auth & CSRF

### 68d1083 — Epic 23: Guard view detection, CSRF double-submit cookie, and per-view auth rules
Date: 2026-03-14

Guard view (single per server, CodeComponent required). CSRF double-submit cookie with constant-time comparison. Per-view auth rules (session/none).

---

## Epic 24: ProcessPool

### 4488664 — Epic 24: ProcessPool runtime types, task queue, capability model, and named pool management
Date: 2026-03-14

Opaque tokens (DatasourceToken, DataViewToken, HttpToken). TaskContext with builder pattern. Worker trait. Bounded task queue with backpressure. Named pool management. Capability validation.

---

## Epic 25: View Layer — REST

### ab5cc37 — Epic 25: View Layer — REST routing, handler pipeline, and response serialization
Date: 2026-03-14

ViewRouter with path pattern matching ({param} and :param). ParsedRequest/ViewContext. Parameter mapping. 6-stage pipeline (stubs). Response serialization. View validation (6 rules).

---

## Epic 26: WebSocket Views

### 874a8bc — Epic 26: WebSocket view layer with Broadcast/Direct modes, connection registry, and rate limiting
Date: 2026-03-14

BroadcastHub (shared channel). ConnectionRegistry (per-connection routing). Per-connection token bucket rate limiting. Connection limits. Session expired message.

---

## Epic 27: SSE Views

### 458f33b — Epic 27: SSE view layer with event wire format, broadcast channels, and route management
Date: 2026-03-14

SseEvent wire format (event:/id:/data: fields, multiline support). SseChannel with broadcast subscribers. SseRouteManager.

---

## Epic 28: MessageConsumer Views

### 7f14cc8 — Epic 28: MessageConsumer view layer with config registry, validation, and event payload types
Date: 2026-03-14

MessageConsumerConfig extraction. Registry with topic listing. MessageEventPayload. Validation (no path, on_event required, no on_stream).

---

## Epic 29: Streaming REST

### 0b7b612 — Epic 29: Streaming REST with NDJSON/SSE wire formats, poison chunks, and validation
Date: 2026-03-14

StreamingFormat (NDJSON/SSE). StreamChunk serialization. Poison chunks for mid-stream errors. Validation (REST-only, CodeComponent-only, no pipeline).

---

## Epic 30: Polling Views

### dd5ccc9 — Epic 30: Polling views with diff strategies, client deduplication, and poll loop registry
Date: 2026-03-14

DiffStrategy (Hash/Null/ChangeDetect). SHA-256 hash diff. PollLoopKey with deterministic param hashing. PollLoopRegistry (get_or_create/remove). Client fan-out via broadcast.

---

## Epic 31: Admin API

### f470ac7 — Epic 31: Admin API with RBAC, timestamp replay protection, IP allowlist, and deployment state machine
Date: 2026-03-14

AdminPermission enum with Admin-grants-all. RBAC (identity→role→permissions). Timestamp replay protection (±5 min). IP allowlist. Deployment state machine (PENDING→RUNNING/FAILED→STOPPED).

---

## Epic 32: Bundle Deployment

### 94c66ab — Epic 32: Bundle deployment lifecycle with resource resolution, startup order, and preflight checks
Date: 2026-03-14

Resource resolution (datasource/service/LockBox). Topological startup order (services before mains). Preflight checks (port conflicts, appId uniqueness, type validation). DeploymentManager.

---

## Epic 33: Health Endpoints

### 6a05235 — Epic 33: Health endpoint response types with verbose diagnostics and simulate delay
Date: 2026-03-14

HealthResponse/VerboseHealthResponse. PoolSnapshot. UptimeTracker. `?simulate_delay_ms=N` for testing.

---

## Epic 34: GraphQL

### 9f3f753 — Epic 34: GraphQL types with schema generation, resolver bridge, and config validation
Date: 2026-03-14

GraphqlConfig (path, introspection, max_depth, max_complexity). JSON schema → GraphQL type generation. ResolverMapping (field→DataView).

---

## Epic 35: Hot Reload

### 386b3f1 — Epic 35: Hot reload with atomic config swap, version tracking, and reload scope detection
Date: 2026-03-14

HotReloadState with RwLock + Arc snapshots. Atomic swap with version counter. Watch channel notifications. ReloadScope detection (host/port/TLS require restart).

---

## Epic 36: CLI Tools

### 6e8a917 — Epic 36: CLI argument parser with serve/doctor/preflight commands and flag handling
Date: 2026-03-14

CliCommand (Serve/Doctor/Preflight/Version/Help). Flags: --config, --log-level, --no-admin-auth. version_string/help_text.

---

## Epic 37: Error Response Format

### 065d04c — Epic 37: Consistent JSON error response format with status code mapping and view error bridge
Date: 2026-03-14

ErrorResponse envelope (code/message/details/trace_id). ErrorCategory → status code mapping. Convenience constructors. ViewError→ErrorResponse bridge.

---

## Maintenance

### 7575d61 — Update Cargo.lock for axum ws feature dependencies
Date: 2026-03-14
