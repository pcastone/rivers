# Tasks — Epic 1: Foundation — ValidationReport + Error Codes + Formatters

> **Branch:** `feature/art-of-possible`
> **Source:** `docs/arch/rivers-bundle-validation-spec.md` (Sections 8, 9, 11, Appendix A)
> **Goal:** Create foundational types and formatters for the 4-layer bundle validation pipeline

---

## Sprint 1.1 — ValidationReport types (`validate_result.rs`)

- [x] 1. Create `validate_result.rs` with `ValidationSeverity` enum (Error, Warning, Info)
- [x] 2. `ValidationStatus` enum (Pass, Fail, Warn, Skip) for individual results
- [x] 3. `ValidationResult` struct (status, file, message, error_code, table_path, field, suggestion, line, column, exports, etc.)
- [x] 4. `LayerResults` struct (passed, failed, skipped count + results vec)
- [x] 5. `ValidationReport` struct (bundle_name, bundle_version, layers map, summary)
- [x] 6. `ValidationSummary` struct (total_passed, total_failed, total_skipped, total_warnings, exit_code)
- [x] 7. Error code constants: S001-S010, E001-E005, X001-X013, C001-C008, L001-L005, W001-W004
- [x] 8. Builder methods: `report.add_result(layer, result)`, `report.exit_code()`, `report.has_errors()`
- [x] 9. Unit tests for report builder

## Sprint 1.2 — Text + JSON formatters (`validate_format.rs`)

- [x] 10. Text formatter matching spec section 8.1 output format
- [x] 11. JSON formatter matching spec section 8.2 contract
- [x] 12. `did_you_mean()` Levenshtein helper (distance <= 2)
- [x] 13. Unit tests for both formatters and Levenshtein helper

## Integration

- [x] 14. Export modules from `lib.rs`
- [x] 15. `cargo check -p rivers-runtime` passes
- [x] 16. `cargo test -p rivers-runtime -- validate_result validate_format` passes

---

## Validation

- `cargo check -p rivers-runtime` — compiles clean
- `cargo test -p rivers-runtime -- validate_result validate_format` — all tests pass

---

# Platform Standards Alignment — Task Plan

**Spec:** `docs/arch/rivers-platform-standards-alignment-spec.md`
**Status:** Planning — tasks organized by spec rollout phases

---

## Phase 1 — OpenAPI + Probes (P0)

### OpenAPI Support (spec §4)

- [ ] Write child execution spec `docs/arch/rivers-openapi-spec.md` from §4
- [ ] Add `OpenApiConfig` struct (`enabled`, `path`, `title`, `version`, `include_playground`) to `rivers-runtime/src/view.rs`
- [ ] Add view metadata fields: `summary`, `description`, `tags`, `operation_id`, `deprecated` to `ApiViewConfig`
- [ ] Add to structural validation known fields in `validate_structural.rs`
- [ ] Create `crates/riversd/src/openapi.rs` — walk REST views, DataView params, schemas → produce OpenAPI 3.1 JSON
- [ ] Map DataView parameter types to OpenAPI `in: path/query/header` from parameter_mapping; map schemas to request/response bodies
- [ ] Register `GET /<bundle>/<app>/openapi.json` route when `api.openapi.enabled = true`
- [ ] Validation: unique `operation_id` per app; no duplicate path+method; fail if enabled but cannot generate
- [ ] Unit tests for OpenAPI generation; integration test with address-book-bundle
- [ ] Tutorial: `docs/guide/tutorials/tutorial-openapi.md`

### Liveness/Readiness/Startup Probes (spec §5)

- [ ] Write child execution spec `docs/arch/rivers-probes-spec.md` from §5
- [ ] Add `ProbesConfig` struct (`enabled`, `live_path`, `ready_path`, `startup_path`) to `rivers-core-config`
- [ ] Add `probes` to known `[base]` fields in structural validation
- [ ] Implement `/live` handler — always 200 unless catastrophic (process alive, not deadlocked)
- [ ] Implement `/ready` handler — 200 when bundle loaded, required datasources connected, pools healthy; 503 otherwise
- [ ] Implement `/startup` handler — 503 until initialization complete, then 200
- [ ] Add startup-complete flag to `AppContext`, set after bundle wiring completes
- [ ] Tests: each probe response; failing datasource → /ready returns 503
- [ ] Add probe configuration to admin guide

---

## Phase 2 — OTel + Transaction Completion (P1)

### OpenTelemetry Trace Export (spec §6)

- [ ] Write child execution spec `docs/arch/rivers-otel-spec.md` from §6
- [ ] Add `OtelConfig` struct (`enabled`, `service_name`, `service_version`, `environment`, `exporter`, `endpoint`, `headers`, `sample_ratio`, `propagate_w3c`) to `rivers-core-config`
- [ ] Add `opentelemetry`, `opentelemetry-otlp`, `tracing-opentelemetry` to workspace dependencies
- [ ] Create spans: HTTP receive → route match → guard/auth → DataView execute → response write
- [ ] Span attributes: `http.method`, `http.route`, `http.status_code`, `rivers.app`, `rivers.dataview`, `rivers.driver`, `rivers.trace_id`
- [ ] W3C propagation: extract `traceparent`/`tracestate` inbound, inject on outbound HTTP driver requests
- [ ] Failure policy: OTel export failures log warning, never block requests
- [ ] Initialize OTel exporter at startup in `server/lifecycle.rs`
- [ ] Tests: verify spans created for request lifecycle; verify W3C headers propagated
- [ ] Tutorial: `docs/guide/tutorials/tutorial-otel.md`

### Runtime Transaction & Batch Completion (spec §7)

- [ ] Gap analysis: compare §7 against current implementation (Connection trait, TransactionMap, Rivers.db.batch stubs)
- [ ] Wire `host_db_begin/commit/rollback/batch` callbacks to actual pool acquisition and TransactionMap
- [ ] Implement batch `onError` policy: `fail_fast` (default) and `continue` modes per §7.4
- [ ] Verify auto-rollback on handler exit without commit
- [ ] Integration tests: Postgres transaction roundtrip via handler; batch insert with partial failure
- [ ] Verify existing canary transaction tests pass end-to-end

---

## Phase 3 — Standards-Based Auth (P1)

### JWT / OIDC / API Key Auth Providers (spec §8)

- [ ] Write child execution spec `docs/arch/rivers-auth-providers-spec.md` from §8
- [ ] Add `AuthProviderConfig` enum (JWT, OIDC, APIKey) to `rivers-core-config`
- [ ] Add `auth_config` to `ApiViewConfig` with `provider`, `required_scopes`, `required_roles`, claim fields
- [ ] JWT provider: validate signature (RS256/ES256), check `iss`/`aud`/`exp`, extract claims → `ctx.auth`
- [ ] OIDC provider: discover JWKS from `/.well-known/openid-configuration`, cache keys, validate tokens
- [ ] API key provider: lookup hashed key in StorageEngine
- [ ] Authorization: check `required_scopes` and `required_roles` against token claims
- [ ] Add `ctx.auth` object to handler context (subject, scopes, roles, claims)
- [ ] Compatibility: `auth = "none"` / `auth = "session"` unchanged; new `auth = "jwt"` / `"oidc"` / `"api_key"`
- [ ] Security: HTTPS required for JWT/OIDC; tokens never logged; JWKS cached with TTL
- [ ] Tests: JWT validation with test keys; OIDC discovery mock; API key lookup
- [ ] Tutorial: `docs/guide/tutorials/tutorial-api-auth.md`

---

## Phase 4 — AsyncAPI (P2)

### AsyncAPI Support (spec §9)

- [ ] Write child execution spec `docs/arch/rivers-asyncapi-spec.md` from §9
- [ ] Add `AsyncApiConfig` struct (`enabled`, `path`, `title`, `version`)
- [ ] Create `crates/riversd/src/asyncapi.rs` — walk MessageConsumer, SSE, WebSocket views → produce AsyncAPI 3.0 JSON
- [ ] Kafka/RabbitMQ/NATS: map consumer subscriptions to AsyncAPI channels with message schemas
- [ ] SSE: map SSE views to AsyncAPI channels (optional in v1)
- [ ] WebSocket: map WebSocket views to AsyncAPI channels (optional in v1)
- [ ] Register `GET /<bundle>/<app>/asyncapi.json` when enabled
- [ ] Validation: broker consumers must have schemas; SSE/WS optional
- [ ] Tests: unit tests for AsyncAPI generation from broker configs
- [ ] Add to developer guide

---

## Phase 5 — Polish (Future)

- [ ] OpenAPI HTML playground (Swagger UI / ReDoc)
- [ ] OTel metrics signal (bridge Prometheus → OTel)
- [ ] OTel log signal (bridge tracing → OTel logs)
- [ ] Richer AsyncAPI bindings (Kafka headers, AMQP routing keys)

---

## Cross-Cutting Rules (spec §10)

- [ ] All new features opt-in by default (`enabled = false` or absent)
- [ ] No new feature breaks existing bundles
- [ ] All new config fields have sensible defaults
- [ ] Error responses follow existing `ErrorResponse` envelope format
- [ ] Validation runs at startup (fail-fast), not at request time

---

## Open Questions (spec §12)

Decisions for implementation:

1. Bundle-level aggregate OpenAPI/AsyncAPI → defer to v2
2. `/ready` degradation → fail on any required datasource failure + open circuit breakers
3. OTel v1 → traces only; metrics/logs deferred to Phase 5
4. `Rivers.db.batch` partial failure → `fail_fast` only in v1
5. `ctx.auth` vs `ctx.session` → introduce `ctx.auth` as new object
6. AsyncAPI SSE/WS → start with brokers only, SSE/WS optional
7. OpenAPI strictness → permissive (omit missing schemas, don't invent them)
