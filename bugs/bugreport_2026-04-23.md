# Bug Report — 2026-04-23

## Summary

TypeScript handlers running in the ProcessPool have no way to **publish** messages to any `MessageBrokerDriver` datasource. Four shipped drivers — **kafka, rabbitmq, nats, redis-streams** — expose `publish()` at the Rust level (via `rivers-driver-sdk::broker::MessageBrokerDriver`) but the method is **not bridged to V8**. Handlers can receive broker messages via `MessageConsumer` views, but cannot produce them.

This blocks any scenario where a handler needs to originate an event: Activity Feed (CS3), any fan-out pattern, any multi-stage pipeline that spans a broker hop.

## Affected components

| Driver | Crate | Rust-side publish | TS-side publish |
|---|---|---|---|
| Kafka | `rivers-plugin-kafka` | ✓ `KafkaDriver::publish()` | ✗ absent |
| RabbitMQ | `rivers-plugin-rabbitmq` | ✓ `RabbitMqDriver::publish()` | ✗ absent |
| NATS | `rivers-plugin-nats` | ✓ `NatsDriver::publish()` | ✗ absent |
| Redis Streams | `rivers-plugin-redis-streams` | ✓ `RedisStreamsDriver::publish()` | ✗ absent |

Contrast with drivers that ARE bridged to TS:
- Database drivers (postgres/mysql/sqlite/redis/etc.) → via `ctx.dataview("name", params)` (DataView dispatch)
- HTTP driver → via `ctx.dataview("name", body)` with `query = url-path` (HTTP-as-datasource)
- Filesystem driver → via `ctx.datasource("fs").writeFile/readFile/…` (direct-dispatch method API)
- rivers-exec → via `ctx.dataview("exec_<name>", {command, args})` (hash-pinned script dispatch)

## Symptoms

1. **Scenario-level blocker (CS3 Activity Feed)**: spec `rivers-canary-scenarios-spec.md §6 AF-1` says "Kafka MUST be source of truth. Direct database inserts bypass the event pipeline and are forbidden." AF-2 requires persistence in the MessageConsumer handler. Without a TS-side `publish`, the scenario orchestrator cannot originate the events whose downstream processing it tests. The only workarounds are:
   - Direct SQL insert from the orchestrator (violates AF-1 / AF-2 / AF-8).
   - External publish via `kafkacat` from run-tests.sh (adds a host dependency, fragile shell timing, breaks the scenario-verdict envelope).
   Both are unacceptable — they don't test the actual composition the scenario is supposed to cover.

2. **Existing canary-streams MessageConsumer is commented out** with `# disabled — no broker`. Even if the consumer side were active, the test harness has no way to originate messages to flow through it from inside a handler.

3. **Fan-out scenarios infeasible**: any pattern like "HTTP request arrives → handler processes → handler publishes follow-up event" can't be expressed in Rivers handlers today, forcing business logic that conceptually belongs in Rivers to live outside it.

4. **Broker coverage gap**: the 4 MessageBrokerDriver plugins have driver-level integration tests but no end-to-end coverage that exercises them from real handler code.

## Root cause

The V8 bridge in `crates/riversd/src/process_pool/v8_engine/` wires:
- `ctx.dataview()` → DataViewExecutor (database, HTTP, exec drivers)
- `ctx.datasource("name").method()` — direct-dispatch API for self-contained drivers (filesystem)

Neither path reaches `MessageBrokerDriver::publish`. `crates/riversd/src/pool.rs` has its own internal `event_bus.publish()` used for lifecycle events, but that's a separate EventBus (not MessageBrokerDriver) and is also not exposed to handlers.

Search that establishes the gap:
```
grep -rn "MessageBrokerDriver\|publish\|produce" crates/riversd/src/process_pool/
```
Returns zero publish/produce call sites in the process-pool layer.

## Proposed fix

Mirror the filesystem driver's direct-dispatch pattern for MessageBrokerDriver:

1. Extend `DatasourceToken` or add a new variant that carries a broker-publish capability.
2. Wire a new V8 method `ctx.datasource("<broker-name>").publish({topic, payload, key?, headers?})` on datasources whose driver implements MessageBrokerDriver.
3. The publish call routes to the host side via the same direct-dispatch mechanism as `fs.writeFile`; the host invokes `driver.publish(OutboundMessage{…})` and returns the `PublishReceipt`.
4. Update `rivers-processpool-runtime-spec-v2.md` to document the new surface.
5. Add canary coverage: a STREAMS-PUBLISH atomic test that produces one message and asserts the returned receipt is well-formed.

**Alternative (simpler, less flexible)**: extend DataView dispatch to accept broker datasources, with the query field naming the topic/routing-key. Matches how HTTP-as-datasource works today. Less expressive (no structured headers, no receipt inspection) but lighter runtime change.

## Impact if not fixed

- CS3 Activity Feed scenario cannot ship.
- Any future scenario or real app needing pub/sub composition is blocked.
- The MessageBrokerDriver trait remains half-implemented in practice — one direction wired, the other stranded.

## Effort estimate

- Direct-dispatch bridge (full fix): **1-2 days** — new V8 callback, token plumbing, canary atomic tests, spec update.
- DataView-based bridge (alternative): **0.5-1 day** — smaller surface change; may be sufficient for canary + simple cases.
- CS3 implementation once fix lands: **3-4 hours** per the original CS3 plan.

## Related

- `docs/arch/rivers-canary-scenarios-spec.md §6` — Activity Feed scenario definition.
- `todo/tasks.md` CS3 tier — currently deferred pending this fix.
- `changedecisionlog.md` 2026-04-23 — CS3 deferral decision logged alongside CS0.1/CS0.2 canary scenarios.

## Resolved

**2026-04-23** — V8 broker-publish bridge landed via BR0–BR6 in `todo/tasks.md`. Shape taken: parallel-scaffolding path (BR0.1). New surface:
- `DatasourceToken::Broker { driver }` + `TASK_DIRECT_BROKER_PRODUCERS` thread-local (BR1).
- `Rivers.__brokerPublish(name, msg)` V8 callback in `broker_dispatch.rs`; `ctx.datasource("<broker>").publish(...)` proxy emitted for every Broker-tagged datasource (BR2).
- Canary atomic tests for receipt, unknown-datasource, missing-destination, publish-then-consume — `broker-publish-tests.ts` (BR4). MessageConsumer view re-enabled.
- CS3 Activity Feed scenario un-deferred and shipped structurally (BR5). Kafka infra verification pending first deploy.

Broker plugin lib-test regression: all four (kafka 15 / rabbitmq 15 / nats 12 / redis-streams 11 = 53 tests) PASS — no damage from token plumbing.

Runtime unit tests: `rivers-runtime::process_pool::types` 8/8 (3 new BR1.T1-3 cases), `riversd::process_pool::v8_engine::broker_dispatch` 8/8 (BR2.T1-8 covering outbound message shape + proxy codegen).

Follow-on tracked outside this bug: `docs/arch/rivers-processpool-runtime-spec-v2.md` + `rivers-driver-spec.md` need publish-surface documentation (BR6.1/6.2 — deferred with the CS5 SPA-source rebuild work).
