# Rivers Canary Fleet — Amendment AMD-2

**Date:** 2026-04-03
**Applies to:** `rivers-canary-fleet-spec.md` v1.0 → v1.1
**Resolves:** v0.53.0 feature conformance coverage
**Instruction:** Already absorbed into source spec (v1.1). This file is historical reference.

---

## AMD-2.0 — Summary

v0.53.0 introduced nine new feature areas that had zero canary coverage. This amendment adds:

- **1 new profile:** canary-ops (port 9105) with 24 tests
- **3 new tests in canary-handlers:** per-app logging (RT-LOG-APP-ROUTER, RT-LOG-STRUCTURED, RT-LOG-LEVELS)
- **4 new tests in canary-sql:** SQLite path fallback (SQL-SQLITE-PATH-DATABASE, SQL-SQLITE-PATH-HOST, SQL-SQLITE-PATH-MKDIR, SQL-SQLITE-PATH-EMPTY)
- **New riversd.toml sections:** `[metrics]` and `[logging]` config for test infrastructure
- **Updated totals:** 75 → 107 tests across 7 profiles (was 6)

---

## AMD-2.1 — New Profile: OPS (canary-ops, port 9105)

**Rationale:** v0.53.0 added operational infrastructure (PID files, metrics, logging, doctor, TLS, config discovery, riverpackage, engine loader) that is invisible to application handlers but critical for production. These features cross-cut all profiles and need their own isolation domain.

**appId:** `aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee06`

**24 tests total:**
- 8 handler-based tests (PID, metrics, logging, config discovery)
- 16 harness-only tests (riversctl stop/status, doctor --lint/--fix, TLS renew, riverpackage init/validate, engine loader naming, plugin ABI)

**Key design decisions:**
1. canary-ops has no external datasources — it tests infrastructure, not data flow
2. Metrics tests use an inline HTTP datasource to scrape `http://127.0.0.1:9091/metrics`
3. Doctor/TLS/riverpackage tests are harness-only because they exercise CLI commands outside the running server
4. Log rotation stress test uses repeated calls (50K entries per call, 20+ calls) to approach the 10MB threshold

---

## AMD-2.2 — Per-App Logging Tests in canary-handlers

**Rationale:** AppLogRouter routes `Rivers.log.*` calls to per-app log files. The existing RT-RIVERS-LOG test only verified that log calls don't throw. The new tests verify the actual file output.

**3 new tests:**

| Test ID | What It Tests |
|---------|---------------|
| RT-LOG-APP-ROUTER | `Rivers.log.info` writes to `log/apps/canary-handlers.log` |
| RT-LOG-STRUCTURED | `Rivers.log.info("msg", { key: "value" })` includes structured fields |
| RT-LOG-LEVELS | info/warn/error each produce correct level tag in output |

**Handler location:** Added to `canary-handlers/libraries/handlers/rivers-api.ts`
**View config:** 3 new views in `canary-handlers/app.toml`

---

## AMD-2.3 — SQLite Path Fallback Tests in canary-sql

**Rationale:** v0.53.0 added `host=` as a fallback for SQLite file paths, auto-creation of parent directories, and clear errors when both `database=` and `host=` are empty.

**4 new tests:**

| Test ID | What It Tests |
|---------|---------------|
| SQL-SQLITE-PATH-DATABASE | `database=` config works for file path |
| SQL-SQLITE-PATH-HOST | `host=` accepted as fallback when `database=` absent |
| SQL-SQLITE-PATH-MKDIR | Parent directories created automatically for new DB files |
| SQL-SQLITE-PATH-EMPTY | Clear error when both `database=` and `host=` are empty |

**Handler location:** Added to `canary-sql/libraries/handlers/sql-tests.ts`
**View config:** 4 new views in `canary-sql/app.toml`

---

## AMD-2.4 — Metrics Infrastructure Config

**New riversd.toml section:**

```toml
[metrics]
enabled = true
port    = 9091
```

Required for OPS-METRICS-ENDPOINT, OPS-METRICS-REQUEST-COUNTER, OPS-METRICS-DURATION-HISTOGRAM tests.

---

## AMD-2.5 — Logging Infrastructure Config

**New riversd.toml section:**

```toml
[logging]
app_log_dir     = "log/apps"
rotation_max_mb = 10
```

Required for all per-app logging tests in canary-ops and canary-handlers.

---

## AMD-2.6 — canary-main Updated Dependencies

canary-main now depends on canary-ops (appId `aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee06`) in addition to all existing service dependencies. The SPA dashboard includes the OPS profile in its conformance matrix.

---

## AMD-2.7 — Harness-Only Tests (New Category)

The OPS profile introduces a pattern not seen in previous profiles: tests that have no handler endpoint and run entirely in the Rust integration test harness. These are marked `(harness)` in the Method column and documented with Rust test code examples in the spec.

**Harness-only tests (16 total):**
- OPS-PID-CLEANUP — PID file removed after `riversctl stop`
- OPS-RIVERSCTL-STATUS — `riversctl status` reports running state
- OPS-RIVERSCTL-STOP — `riversctl stop` sends SIGTERM
- OPS-DOCTOR-LINT-PASS — `riversctl doctor --lint` passes on valid bundle
- OPS-DOCTOR-LINT-FAIL — `riversctl doctor --lint` fails on broken bundle
- OPS-DOCTOR-FIX-LOCKBOX — `--fix` auto-repairs missing lockbox
- OPS-DOCTOR-FIX-LOGDIRS — `--fix` creates missing log directories
- OPS-DOCTOR-FIX-PERMS — `--fix` corrects file permissions
- OPS-DOCTOR-FIX-TLS — `--fix` auto-repairs missing TLS certs
- OPS-TLS-CERT-RENEW — `riversctl tls renew` regenerates cert
- OPS-TLS-CERT-EXPIRY — Doctor detects near-expiry and auto-renews
- OPS-RIVERPACKAGE-INIT — `riverpackage init <name>` scaffolds bundle
- OPS-RIVERPACKAGE-VALIDATE — `riverpackage validate` passes on scaffold
- OPS-ENGINE-LOADER-NAMING — Engine dylib `librivers_engine_v8.dylib` loads
- OPS-PLUGIN-ABI-EXPORTS — Plugin dylib has required ABI symbols

These tests are fundamentally different from handler-based tests because they exercise processes, files, and CLI tools rather than HTTP request/response cycles.

---

## AMD-2.8 — Startup Order Update

Startup order changed from:
```
canary-guard → canary-sql → canary-nosql → canary-handlers → canary-streams → canary-main
```

To:
```
canary-guard → canary-sql → canary-nosql → canary-handlers → canary-streams → canary-ops → canary-main
```

canary-ops starts after canary-streams because it verifies log files that other profiles produce during their startup.

---

## Test Count Reconciliation

| Profile | v1.0 Total | v1.1 Added | v1.1 Total |
|---------|-----------|------------|------------|
| AUTH | 9 | 0 | 9 |
| SQL | 17 | +4 (SQLite path) +1 (init) | 22 |
| NOSQL | 11 | 0 | 11 |
| RUNTIME | 25 | +3 (per-app logging) | 28 |
| STREAM | 9 | 0 | 9 |
| OPS | — | +24 (new profile) | 24 |
| PROXY | 4 | 0 | 4 |
| **Total** | **75** | **+32** | **107** |
