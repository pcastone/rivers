# Bug Report — 2026-04-04

## Summary
`cargo deploy` dynamic mode missing exec/neo4j plugins and all builtin drivers (sqlite, faker, postgres, mysql, redis, http).

## Symptoms
After deploying with `cargo deploy <path>` (dynamic mode), the IPAM bundle reported:
- `unknown driver 'sqlite'` — SQLite not in known driver list
- `unknown driver 'plugin:rivers-exec'` — exec driver not in known driver list
- Known drivers only listed the 10 plugin drivers (cassandra, couchdb, elasticsearch, influxdb, ldap, mongodb, kafka, nats, rabbitmq, redis-streams)
- V8/WASM engine dylibs logged `missing _rivers_abi_version symbol` warnings (cosmetic — plugin loader scanning `lib/` directory, engines use `_rivers_engine_abi_version`)

## Environment
Rivers v0.52.10, macOS (Darwin), IPAM bundle, dynamic deploy via `cargo deploy`.

## Root Cause
Two issues in `crates/cargo-deploy/src/main.rs`:

1. **Missing plugins**: `rivers-plugin-exec` and `rivers-plugin-neo4j` were not listed in the `PLUGINS` or `PLUGIN_LIB_NAMES` constants. The `static-plugins` feature in `riversd/Cargo.toml` lists 12 plugins, but cargo-deploy only built 10.

2. **Builtin drivers stripped**: The dynamic binary build used `--no-default-features` which disabled the `static-builtin-drivers` feature. This feature gates `rivers-drivers-builtin` (sqlite, postgres, mysql, redis, faker, http) — all core drivers that should always be compiled into `riversd` regardless of build mode.

Same root file: `crates/cargo-deploy/src/main.rs`
Same root pattern: cargo-deploy plugin/feature lists out of sync with `riversd/Cargo.toml`.

Additionally, the same exec/neo4j omission existed in:
- `Justfile` (`_build-dynamic-crates` recipe)
- `scripts/build-release.sh` (plugin build and copy lists)

## Fix Applied
1. Added `rivers-plugin-exec` and `rivers-plugin-neo4j` to `PLUGINS` and `PLUGIN_LIB_NAMES` in `crates/cargo-deploy/src/main.rs`
2. Changed dynamic binary build from `--no-default-features` to `--no-default-features --features static-builtin-drivers` so builtin drivers (sqlite, faker, etc.) remain compiled into `riversd`
3. Updated `Justfile` `_build-dynamic-crates` recipe to include exec and neo4j plugins
4. Updated `scripts/build-release.sh` plugin build and copy lists to include exec and neo4j

## Occurrence Log
| Date | Context | Notes |
|------|---------|-------|
| 2026-04-04 | IPAM team deploy from rivers-0.52.10-source.zip | Second cargo-deploy bug in same session — first was missing `--features plugin-exports` (fixed in v0.52.9) |
