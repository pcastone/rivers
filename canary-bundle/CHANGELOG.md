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

## [Decision] — Scenario testing layer landed (partial, CS3 deferred)
**File:** canary-sql/libraries/handlers/{scenario-harness,scenario-probe,scenario-messaging}.ts, canary-streams/libraries/handlers/{scenario-harness,scenario-probe}.ts, canary-handlers/libraries/handlers/{scenario-harness,scenario-probe,scenario-doc-pipeline}.ts, canary-handlers/libraries/scripts/wc-json.sh, init.ts, app.toml (several), resources.toml (canary-handlers), run-tests.sh, canary-main/libraries/spa/bundle.js
**Description:** Added scenario-based integration tests per rivers-canary-scenarios-spec.md. ScenarioResult harness ported to three hosting apps (canary-sql, canary-streams, canary-handlers) — per-step verdict envelope with `type:"scenario"`, `steps[]`, `failed_at_step`. Scenarios shipped: (A) Messaging in canary-sql across PG/MySQL/SQLite (12 steps, placeholder XOR cipher pending app keystore wiring — MSG-4 documented deviation); (C) Document Pipeline in canary-handlers (14 steps, 11 live filesystem ops + 3 via rivers-exec driver using hash-pinned wc-json.sh wrapper, CS4.9 delivered). Dashboard integration: new SCENARIOS profile in canary-main/libraries/spa/bundle.js; scenario envelopes also expose a flat `assertions[]` aggregate so existing atomic-renderer shows full detail without new UI code.
**Spec reference:** rivers-canary-scenarios-spec.md §1-10
**Resolution:** CS3 (Activity Feed) deferred — blocked on bugs/bugreport_2026-04-23.md. Rivers TS handlers have no MessageBrokerDriver publish surface; the two workaround paths (direct SQL insert / external kafkacat) were rejected as hollow. Fix requires a 1-2 day V8 bridge in crates/riversd/src/process_pool/v8_engine/ following the filesystem direct-dispatch pattern. CS3 ships in ~3-4 hours after that. Secondary deferrals: dedicated per-step SPA UI (CS5.2/5.3/5.4 — blocked on empty libraries/src/components/ Svelte tree); run-tests.sh step-summary pretty-print (CS6.3 — polish).

## [Decision] — CG plan: Canary Green Again — 4 fixes, expected +9 tests (deploy-gated)

**File:** `crates/riversd/src/message_consumer.rs`, `crates/riversd/src/bundle_loader/wire.rs`, `crates/riversd/src/broker_bridge.rs`, `crates/rivers-drivers-builtin/src/mysql.rs`

**Description:** Four focused fixes targeting specific canary failure categories, landed 2026-04-24. Runtime verification (deploy + `run-tests.sh` pass) is pending.

**CG1 — MessageConsumer app identity fix:** Added `entry_point: String` to `MessageConsumerConfig`; `dispatch_message_event` now calls `enrich(builder, &config.entry_point)` instead of `enrich(builder, "")`. Kafka consumer `ctx.store` writes land in the owning app's namespace instead of `app:default`. Directly unblocks 2 canary KAFKA-CONSUMER-STORE failures.

**CG2 — Broker subscription topic from `on_event.topic`:** Wire.rs now reads `view_cfg.on_event.topic` for the subscription topic instead of the view ID. Consumer and per-destination publish now agree on the topic name.

**CG3 — Non-blocking broker consumer startup:** `BrokerBridgeSpec` + `run_with_retry` supervisor with bounded exponential backoff (base=reconnect_ms, cap=30s, ±50% jitter). `tokio::spawn(run_with_retry(spec))` replaces the inline `create_consumer().await` that caused the startup hang when Kafka was unreachable. HTTP listener can bind even when every broker is unreachable.

**CG4 — MySQL pool restored:** `crates/rivers-drivers-builtin/src/mysql.rs` — process-global `OnceLock<Mutex<HashMap<String, mysql_async::Pool>>>` pool cache; `connect()` reuses `pool.get_conn()` instead of paying a full per-call handshake. Unblocks 7 canary MySQL CRUD failures.

**Expected canary delta (pending deploy verification):** +9 tests minimum (2 Kafka consumer-store, 7 MySQL CRUD). Startup should never hang on broker.

**Spec reference:** `docs/canary_codereivew.md` CG plan; `docs/dreams/dream-2026-04-22.md`
**Resolution:** All 4 code changes landed and pass 347/347 riversd lib + 200+ integration tests. Deploy-gated items (CG4.3, CG4.4, CG5.1–CG5.6) require `cargo deploy` + live canary run.

---

## [Decision] — BR-2026-04-23: MessageBrokerDriver TS bridge (CS3 unblocked)
**File:** crates/rivers-runtime/src/process_pool/types.rs, crates/riversd/src/process_pool/v8_engine/{broker_dispatch.rs (new), task_locals.rs, context.rs, rivers_global.rs, mod.rs}, crates/rivers-runtime/src/process_pool/bridge.rs, crates/rivers-runtime/src/validate_structural.rs, types/rivers.d.ts, canary-bundle/canary-streams/{resources.toml, manifest.toml, app.toml, libraries/handlers/{init.ts, scenario-activity-feed.ts, broker-publish-tests.ts, kafka-consumer.ts}}, canary-bundle/canary-main/libraries/spa/bundle.js, canary-bundle/run-tests.sh
**Description:** Bridged MessageBrokerDriver::publish into V8. New `DatasourceToken::Broker { driver }` variant, `TASK_DIRECT_BROKER_PRODUCERS` per-task producer cache, `Rivers.__brokerPublish(name, msg)` V8 callback, and `ctx.datasource("<broker>").publish(msg)` proxy codegen. Four broker plugins (kafka/rabbitmq/nats/redis-streams) auto-detected by name. Re-enabled canary-streams kafka_consume MessageConsumer view (was commented out "no broker"). Shipped 4 atomic tests + CS3 Activity Feed scenario (11 steps). Fixed adjacent validator gap: `validate_structural` now exempts MessageConsumer views from path/method required-fields.
**Spec reference:** bugs/bugreport_2026-04-23.md (Resolved)
**Resolution:** Path (a) parallel scaffolding chosen over (b) unified DatabaseDriver synthesis + (c) DataView-based publish. Rationale in changedecisionlog.md 2026-04-23 BR0.1. Producer lifecycle: per-task lazy cache, closed in TaskLocals::drop before RT_HANDLE clears (mirrors auto_rollback_all pattern). TS API: `{destination, payload, headers?, key?, reply_to?} → {id, metadata}` with payload auto-stringify for objects. 61 tests added/verified (53 broker plugin regression + 3 BR1 token + 8 BR2 broker-dispatch). 0 runtime regressions. `riverpackage validate canary-bundle` → 0 errors, 90 warnings (all pre-existing patterns, none introduced by this work).
