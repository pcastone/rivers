# OTLP End-to-End Smoke Probe

End-to-end smoke for `view_type = "OTLP"` (CB-OTLP, Track O5.6).

This is the inverse of CB's `cb-rivers-otlp-feature-request/run-probe.sh`:
their probe targets the REST-workaround bundle and expects
PASS / FAIL / FAIL. Ours targets the new `view_type = "OTLP"` form and
expects every scenario to PASS.

## Layout

```
tests/fixtures/otlp-probe/
├── README.md
├── run-probe.sh                          — executable smoke runner
└── bundle/
    ├── manifest.toml
    └── otlp/
        ├── manifest.toml
        ├── resources.toml                — sqlite probe_db
        ├── app.toml                      — single view_type = "OTLP" view
        └── libraries/handlers/otel.ts    — minimal TS handler
```

## What it covers

12 scenarios in one run (no infra needed beyond a built `riversd` + a
free TCP port):

| # | Scenario | Expected |
|---|---|---|
| 01 | JSON metrics | 200 `{"ingested":1, …}` |
| 02 | JSON logs | 200 `{"ingested":1, …}` |
| 03 | JSON traces | 200 `{"ingested":1, …}` |
| 04 | JSON metrics, `Content-Encoding: gzip` | 200 `{"ingested":1, …}` |
| 05 | JSON metrics, `Content-Encoding: deflate` | 200 `{"ingested":1, …}` |
| 06 | Empty protobuf body (valid empty `ExportMetricsServiceRequest`) | 200 `{"ingested":0, …}` |
| 07 | Handler-side partial reject | 200 `{"partialSuccess":{"rejectedDataPoints":1, …}}` |
| 08 | Oversized body (3 MB > 2 MB `max_body_mb` cap) | 413 |
| 09 | `Content-Encoding: br` | 415 |
| 10 | `/otlp/v1/wat` (unknown signal) | 404 |
| 11 | `Content-Type: text/plain` | 415 |
| 12 | Malformed JSON body | 400 |

## Usage

```bash
# Build the binaries (one time, ~5 min cold)
cargo build -p riversd -p riverpackage

# Run the smoke
tests/fixtures/otlp-probe/run-probe.sh
```

Exit code 0 → all 12 scenarios passed. Exit code 1 → at least one regression.
Exit code 2 → environment problem (binaries missing, port collision, riversd didn't boot).

Useful env overrides:

| Env var | Default | Notes |
|---|---|---|
| `RIVERSD` | `target/debug/riversd` | Path to the daemon binary |
| `RIVERPACKAGE` | `target/debug/riverpackage` | Path to the validator CLI |
| `PORT` | `8197` | Listen port (override on collision) |
| `HOST` | `127.0.0.1` | Listen host |
| `KEEP_TMP` | unset | Set to keep the per-run tempdir for inspection |

The smoke writes nothing into the repo — the bundle gets copied to a
`mktemp -d` working dir and the `data/` directory is initialised
there. Cleanup runs on EXIT/INT/TERM unless `KEEP_TMP` is set.

## Why this lives in `tests/fixtures/otlp-probe/`

Mirrors the existing `tests/fixtures/ts-pipeline-probe/run-probe.sh`
location convention. Not wired into `cargo test` because:

- Booting riversd + V8 in a `cargo test` would couple the smoke to the
  test harness and slow `cargo test` runs.
- The smoke needs a network port, which is awkward in CI without
  per-runner port allocation.

Run it manually as a pre-merge gate when shipping changes that touch
the OTLP dispatcher, V8 `ctx.otel` injection, or the per-signal
router registration. The unit + integration tests in
`crates/riversd/{src,tests}` cover the deterministic parts.

## Spec reference

`docs/arch/rivers-otlp-view-spec.md` — every assertion in `run-probe.sh`
maps to a section: §4 (wire format), §5 (routing), §6 (handler
envelope), §7 (response shapes), §9 (validation rules).
