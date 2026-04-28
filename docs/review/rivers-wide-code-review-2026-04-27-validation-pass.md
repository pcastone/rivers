# Rivers-Wide Code Review Validation Pass

**Date:** 2026-04-27
**Source report:** `docs/review/rivers-wide-code-review-2026-04-27.md`
**Goal:** second-pass confirmation that the consolidated report is at least 95% accurate.

## Verdict

The report is source-backed after small corrections. I found no finding that should be fully removed. I did correct two severity-table counts, downgraded one Kafka item from a defect to an architectural observation, and tightened one CouchDB wording that was directionally correct but too broad.

Post-correction confidence: **>=95%**.

## Corrections Applied To Source Report

- `rivers-lockbox` severity count changed from `10 / 2 / 7 / 1` to `11 / 2 / 8 / 1`; the per-crate section already listed 11 findings.
- `rivers-plugin-influxdb` severity count changed from `3 / 1 / 2 / 0` to `4 / 1 / 3 / 0`; the per-crate section already listed 4 findings.
- `rivers-plugin-kafka` changed from `2 findings` to `1 confirmed finding plus 1 architectural observation`. The plugin does use `rskafka`, but that fact is not itself a defect.
- `rivers-plugin-couchdb` selector wording was narrowed: the real issue is raw string substitution into JSON source without correct escaping/encoding, not that every string path is always unquoted.

## Validation Summary By Crate

| Crate | Status | Notes |
|---|---|---|
| `rivers-plugin-exec` | Confirmed | Timeout window, TOCTOU, privilege-drop, env parsing, stderr slicing, and process-group findings match source. |
| `rivers-lockbox-engine` | Confirmed | Plaintext `String` exposure, success-only zeroization, stale entry index, and missing runtime permission check match source. |
| `rivers-keystore-engine` | Confirmed | Durability, concurrent save risk, secret `Debug`, public accessors, and overflow findings match source. |
| `rivers-lockbox` | Confirmed with count fix | All listed findings match source; table count was wrong. |
| `rivers-keystore` | Confirmed | Existing target overwrite, plain identity `String`, and unlocked load-mutate-save match source. |
| `rivers-driver-sdk` | Confirmed | Comment-bypass DDL guard, raw statement prefix in errors, global dollar replacement, and backoff overflow risk match source. |
| `rivers-engine-sdk` | Confirmed clean | No focused findings were identified in the report. |
| `rivers-plugin-kafka` | Confirmed with downgrade | Offset-before-ack is confirmed; `rskafka`/no FFI is an observation, not a defect. |
| `rivers-core-config` | Confirmed | Unknown-key depth, field-name allowlist drift, hot-reload validation bypass, and parsed-but-unused storage policy are source-backed. |
| `riversctl` | Confirmed | Admin stop fallback, ignored kill failures, no HTTP timeout, malformed-key ignore, deploy semantics, log payload mismatch, and TLS key chmod gap match source. |
| `cargo-deploy` | Confirmed | Missing dynamic library hard failure, live-target writes, TLS regeneration, chmod-after-write, and hard-coded `target/release` match source. |
| `riverpackage` | Confirmed | `--config` unused, invalid generated templates, and zip-vs-tar behavior match source. |
| `rivers-plugin-ldap` | Confirmed | Plain LDAP URL, direct network awaits without driver timeout, and unbounded search materialization match source. |
| `rivers-plugin-neo4j` | Confirmed | Transactions stored but not used by `execute`, ping stream error swallowed, static registration gap, lossy parameter mapping, and dropped unsupported result values match source. |
| `rivers-plugin-cassandra` | Confirmed | Unpaged query materialization and synthetic write row-count match source. |
| `rivers-plugin-mongodb` | Confirmed | Transactions not attached to CRUD calls, unbounded cursor drain, and broad update/delete defaults match source. |
| `rivers-plugin-elasticsearch` | Confirmed | Initial ping bypasses auth helper, default index is unused, admin ops lack `ddl_execute`, no request timeout, unbounded response reads, and unescaped IDs match source. |
| `rivers-plugin-couchdb` | Confirmed with wording fix | The JSON substitution, URL segment interpolation, unbounded response collection, and insert status-ordering findings match source. |
| `rivers-plugin-influxdb` | Confirmed with count fix | Buffer-clear-before-send, missing batch bucket, incomplete line-protocol escaping, and timeout/unbounded response findings match source. |
| `rivers-plugin-redis-streams` | Confirmed | PEL reclaim gap, unbounded `XADD`, and ignored outbound headers match source. |
| `rivers-plugin-nats` | Confirmed | Ack/nack no-op success, plain subscribe despite group identity, first-subscription-only, ignored key suffix, and unwired schema checker match source. |
| `rivers-plugin-rabbitmq` | Confirmed | Missing prefetch, unbounded publish-confirm wait, and unwired schema checker match source. |

## Residual Judgment Calls

- `rivers-plugin-cassandra` synthetic `affected_rows: 1` is source-true and contract-relevant, but severity could reasonably be T3 if callers do not rely on affected rows for CQL writes.
- `rivers-core-config` storage policy enforcement is source-true as a wiring gap; its severity depends on whether retention/max-events/cache fields are already meant to be production-enforced.
- Broker schema checker functions are confirmed unwired by repository search; if the intended architecture is plugin-local optional validation, downgrade those from defect to design debt.

## Bottom Line

After the corrections above, the consolidated report is accurate enough to drive remediation. The highest-value next pass is implementation, not more review: fix the shared failure classes first, especially broker ack/nack semantics, secret lifecycle, timeout/size caps, and static registration/schema wiring.
