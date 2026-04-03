# Canary Fleet — Changelog

## [Decision] — Initial scaffolding
**File:** manifest.toml, all app manifests
**Description:** Created 6-app bundle structure per canary-fleet-spec.md
**Spec reference:** Canary Fleet Spec §1 (Bundle Structure)
**Resolution:** Stable UUIDs assigned, startup order enforced via apps array.

## [Decision] — canary-streams aligned to spec Part 6
**File:** canary-streams/app.toml, ws-handler.ts, sse-handler.ts, streaming-rest.ts, kafka-consumer.ts, poll-handler.ts, resources.toml, manifest.toml
**Description:** Rewrote canary-streams to match spec Part 6. Added ws_echo and ws_broadcast views with on_stream handlers (onConnection/onMessage, onBroadcastConnection/onBroadcastMessage). Added sse_tick (tick-based SSE) and sse_event (EventBus-triggered SSE) views. Changed streaming REST to POST method at /canary/stream/rest/ndjson with generator-based streamNdjson (STREAM-REST-NDJSON). Added streamPoison (STREAM-REST-POISON) for SHAPE-15 poison chunk guard. Added Kafka MessageConsumer view with onMessage entrypoint and kafka_verify REST endpoint. Added poll_hash SSE view with hash diff strategy and onPollChange callback. Removed old ws_chat, sse_events, stream_chunks views. Updated resources.toml to match spec (lockbox alias, x-type fields, service dependency).
**Spec reference:** Canary Fleet Spec Part 6 (STREAM Profile)
**Resolution:** All 9 STREAM test IDs now have corresponding views and handlers.

## [Decision] — canary-main aligned to spec Part 7
**File:** canary-main/app.toml, proxy-tests.ts, resources.toml, manifest.toml
**Description:** Rewrote canary-main to match spec Part 7. Renamed proxyGuard to proxySessionPropagation (PROXY-SESSION-PROPAGATION) testing X-Rivers-Claims header at /canary/proxy/session-check. Renamed proxySql to proxySqlPassthrough (PROXY-SQL-PASSTHROUGH) proxying to pg select at /canary/proxy/sql/pg/select. Renamed proxyHandlers to proxyHandlerPassthrough (PROXY-HANDLER-PASSTHROUGH) proxying to trace-id at /canary/proxy/rt/ctx/trace-id. Added proxyErrorPropagation (PROXY-ERROR-PROPAGATION) at /canary/proxy/error. Removed old proxy_health, proxy_guard, proxy_nosql, proxy_envelope views. Updated DataViews to target actual upstream endpoints. Updated resources.toml to match spec (canary-sql-api, canary-handlers-api datasources; service deps). Updated manifest.toml with appEntryPoint and corrected source URL.
**Spec reference:** Canary Fleet Spec Part 7 (PROXY Profile)
**Resolution:** All 4 PROXY test IDs now have corresponding views and handlers.
