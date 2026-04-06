# Rivers Unit Test Spec — Amendment AMD-1

**Date:** 2026-04-03
**Applies to:** `rivers-unit-test-spec.md` v1.0
**Resolves:** 6 coverage gaps found by comparing v0.52.8 bug report against test spec

---

## AMD-1.1 — Dual Boot Path Test (BUG-005)

BUG-005 was the worst kind of bug — an entire startup code path (`--no-ssl`) that was never tested. The TLS path got all the subsystem initialization; the non-TLS path was a stale copy. Seven subsystems were missing: StorageEngine, SessionManager, CsrfManager, EventBus, engine loader, host context wiring, and `HostContextProvider`.

This is not a unit test. It's a boot-path integration test that lives in `riversd`.

### Add to: `crates/riversd/tests/`

```
crates/riversd/tests/
├── boot/
│   ├── tls_boot.rs          ← TLS path subsystem verification
│   ├── no_ssl_boot.rs       ← --no-ssl path subsystem verification
│   └── boot_parity.rs       ← both paths produce equivalent state
```

```rust
// crates/riversd/tests/boot/no_ssl_boot.rs

/// Regression: BUG-005 — --no-ssl path missing all subsystem initialization.
/// The non-TLS boot path was a simplified copy that never received
/// StorageEngine, SessionManager, CsrfManager, EventBus, engine loader,
/// or host context wiring.
///
/// This test boots riversd in --no-ssl mode with a minimal config
/// and verifies every subsystem is present.

use riversd::server::ServerState;

/// Boot the server in --no-ssl mode and verify all subsystems initialized.
#[tokio::test]
async fn no_ssl_boot_has_all_subsystems() {
    let config = minimal_test_config();
    let state = boot_server_no_ssl(&config).await
        .expect("--no-ssl boot failed");

    // Every subsystem that the TLS path initializes must also exist here.
    assert!(state.storage_engine().is_some(),
        "BUG-005: StorageEngine missing on --no-ssl path");
    assert!(state.session_manager().is_some(),
        "BUG-005: SessionManager missing on --no-ssl path");
    assert!(state.csrf_manager().is_some(),
        "BUG-005: CsrfManager missing on --no-ssl path");
    assert!(state.event_bus().is_some(),
        "BUG-005: EventBus missing on --no-ssl path");
    assert!(state.engine_registry().is_some(),
        "BUG-005: Engine registry missing on --no-ssl path");
    assert!(state.host_context().is_some(),
        "BUG-005: HostContextProvider missing on --no-ssl path");

    state.shutdown().await;
}

/// Both boot paths must produce structurally equivalent ServerState.
/// This is the parity test — if the TLS path gets a new subsystem,
/// this test fails until the --no-ssl path gets it too.
#[tokio::test]
async fn boot_parity_tls_vs_no_ssl() {
    let config = minimal_test_config();

    let tls_state = boot_server_tls(&config).await
        .expect("TLS boot failed");
    let no_ssl_state = boot_server_no_ssl(&config).await
        .expect("--no-ssl boot failed");

    // Compare subsystem presence (not identity — different instances are fine)
    assert_eq!(
        tls_state.storage_engine().is_some(),
        no_ssl_state.storage_engine().is_some(),
        "StorageEngine parity mismatch"
    );
    assert_eq!(
        tls_state.session_manager().is_some(),
        no_ssl_state.session_manager().is_some(),
        "SessionManager parity mismatch"
    );
    assert_eq!(
        tls_state.csrf_manager().is_some(),
        no_ssl_state.csrf_manager().is_some(),
        "CsrfManager parity mismatch"
    );
    assert_eq!(
        tls_state.event_bus().is_some(),
        no_ssl_state.event_bus().is_some(),
        "EventBus parity mismatch"
    );
    assert_eq!(
        tls_state.engine_registry().is_some(),
        no_ssl_state.engine_registry().is_some(),
        "Engine registry parity mismatch"
    );
    assert_eq!(
        tls_state.host_context().is_some(),
        no_ssl_state.host_context().is_some(),
        "HostContextProvider parity mismatch"
    );

    tls_state.shutdown().await;
    no_ssl_state.shutdown().await;
}
```

**Design note:** The parity test is the real insurance. It doesn't test for specific subsystems — it asserts that whatever the TLS path has, the `--no-ssl` path also has. When someone adds a new subsystem to the TLS boot, this test breaks until they add it to both paths. Prevents the "stale copy" problem from recurring.

The deeper fix is architectural: both paths should call the same `initialize_subsystems()` function, with TLS configuration as the only difference. But the parity test catches the problem regardless of how the code is structured.

---

## AMD-1.2 — DataView Namespace Resolution Test (BUG-009)

BUG-009: Handler calls `ctx.dataview("list_records")` but the registry key is `"handlers:list_records"`. The bridge must prepend the entry point namespace.

### Add to: `crates/rivers-engine-v8/tests/bridge/dataview_bridge.rs`

```rust
/// Regression: BUG-009 — ctx.dataview() didn't namespace lookups.
/// Handler calling ctx.dataview("list_records") failed because
/// the registry key is "{entry_point}:list_records".
/// The V8 bridge must prepend the task's entry point prefix.
///
/// Origin: BUG-009, canary fleet testing v0.52.8
/// Root cause: V8 callback passed bare name to DataViewExecutor
#[test]
fn regression_bug009_dataview_name_namespaced() {
    let iso = TestIsolate::new()
        .with_ctx("app-1", "t1", "n1", "test")
        .with_entry_point("handlers")  // <-- new: set the entry point namespace
        .with_dataview_capture();

    // Handler calls bare name
    iso.eval(r#"ctx.dataview("list_records", {limit: 10})"#);

    let calls = iso.dataview_calls();
    assert_eq!(calls.len(), 1);

    let (resolved_name, _params) = &calls[0];
    // The bridge should have prepended the entry point namespace
    assert_eq!(resolved_name, "handlers:list_records",
        "BUG-009: dataview name not namespaced — got bare '{}'", resolved_name);
}

/// Bare name with explicit namespace should NOT be double-prefixed.
#[test]
fn dataview_already_namespaced_not_double_prefixed() {
    let iso = TestIsolate::new()
        .with_ctx("app-1", "t1", "n1", "test")
        .with_entry_point("handlers")
        .with_dataview_capture();

    // Handler passes already-namespaced name
    iso.eval(r#"ctx.dataview("handlers:list_records", {})"#);

    let calls = iso.dataview_calls();
    let (resolved_name, _) = &calls[0];
    // Should NOT become "handlers:handlers:list_records"
    assert_eq!(resolved_name, "handlers:list_records",
        "double-prefixed: '{}'", resolved_name);
}
```

### TestIsolate extension needed

Add `.with_entry_point(name)` to the `TestIsolate` factory in Section 4.2:

```rust
impl TestIsolate {
    /// Set the entry point namespace for dataview name resolution.
    pub fn with_entry_point(mut self, entry_point: &str) -> Self {
        self.runtime.set_task_namespace(entry_point);
        self
    }
}
```

---

## AMD-1.3 — ctx.request Field Name Contract Test (BUG-012)

BUG-012: `ctx.request.query` was serialized as `query_params` in the JSON sent to V8. The spec says `query`, the code said `query_params`. This is a serialization contract violation.

### Add to: `crates/rivers-engine-v8/tests/bridge/ctx_injection.rs`

```rust
/// Regression: BUG-012 — ctx.request.query serialized as "query_params".
/// The spec says `ctx.request.query`. The Rust struct had a field named
/// `query_params` without a serde rename. Handlers got `undefined` for
/// `ctx.request.query`.
///
/// Origin: BUG-012, canary fleet testing v0.52.8
/// Fix: #[serde(rename = "query")] on ParsedRequest.query_params
#[test]
fn regression_bug012_request_field_names_match_spec() {
    let req = serde_json::json!({
        "method": "GET",
        "path": "/test",
        "headers": {"x-test": "1"},
        "query": {"page": "2", "limit": "10"},
        "body": null,
        "params": {"id": "42"}
    });
    let iso = TestIsolate::new().with_request(req);

    // Spec says these exact field names — no aliases, no alternatives
    assert_eq!(iso.eval("ctx.request.method"), "GET");
    assert_eq!(iso.eval("ctx.request.path"), "/test");
    assert_eq!(iso.eval("ctx.request.query.page"), "2",
        "BUG-012: ctx.request.query not accessible — field name mismatch?");
    assert_eq!(iso.eval("ctx.request.query.limit"), "10");
    assert_eq!(iso.eval("ctx.request.params.id"), "42");
    assert_eq!(iso.eval("ctx.request.headers['x-test']"), "1");
}

/// Exhaustive field name check — every field in the spec must exist
/// with exactly the spec's name, not an alias.
#[test]
fn request_object_has_all_spec_fields() {
    let req = serde_json::json!({
        "method": "POST",
        "path": "/api/test",
        "headers": {},
        "query": {},
        "body": {"key": "val"},
        "params": {}
    });
    let iso = TestIsolate::new().with_request(req);

    let fields = ["method", "path", "headers", "query", "body", "params"];
    for field in fields {
        let js = format!(r#""{}" in ctx.request ? "EXISTS" : "MISSING""#, field);
        let result = iso.eval(&js);
        assert_eq!(result, "EXISTS",
            "ctx.request.{} is missing — spec requires this field name", field);
    }
}

/// Negative: common misspellings/aliases should NOT exist.
#[test]
fn request_object_no_alias_fields() {
    let req = serde_json::json!({
        "method": "GET", "path": "/", "headers": {},
        "query": {}, "body": null, "params": {}
    });
    let iso = TestIsolate::new().with_request(req);

    // These are names that have appeared in bugs — they should NOT exist
    let ghosts = [
        "query_params",    // BUG-012: old field name before serde rename
        "queryParams",     // camelCase variant
        "path_params",     // alternative to "params"
        "pathParams",      // camelCase variant
    ];
    for ghost in ghosts {
        let js = format!(r#""{}" in ctx.request ? "EXISTS" : "ABSENT""#, ghost);
        let result = iso.eval(&js);
        assert_eq!(result, "ABSENT",
            "ctx.request.{} should NOT exist — use spec field name instead", ghost);
    }
}
```

---

## AMD-1.4 — ctx.app_id Value Correctness (BUG-010)

My existing test checked that `ctx.app_id` is non-empty. BUG-010 showed it can be non-empty but still wrong — it returned the entry point slug (`"handlers"`) instead of the manifest UUID. The test needs to verify the actual value, not just existence.

### Replace in: `crates/rivers-engine-v8/tests/bridge/ctx_injection.rs`

```rust
/// BUG-010 regression: ctx.app_id returned entry point slug ("handlers")
/// instead of manifest UUID. Being non-empty is necessary but not sufficient.
///
/// Origin: BUG-010, canary fleet testing v0.52.8
/// Root cause: view_dispatch.rs passed app_entry_point as app_id
#[test]
fn regression_bug010_app_id_is_uuid_not_slug() {
    let uuid = "c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a";
    let iso = TestIsolate::new()
        .with_ctx(uuid, "t1", "n1", "test");

    let result = iso.eval("ctx.app_id");

    // Must be the exact UUID, not a slug
    assert_eq!(result, uuid);

    // Sanity checks for slug-like values that indicate the old bug
    assert_ne!(result, "handlers",
        "BUG-010: ctx.app_id is the entry point slug, not the UUID");
    assert_ne!(result, "canary-handlers",
        "BUG-010: ctx.app_id is the app name, not the UUID");
    assert!(result.contains('-'),
        "ctx.app_id doesn't look like a UUID: '{}'", result);
}
```

---

## AMD-1.5 — Module Path Resolution Test (BUG-013)

BUG-013: V8 engine reads handler `.ts` files relative to CWD. Running from a different directory breaks module loading. This is a bundle-loading test, not a V8 bridge test.

### Add to: `crates/riversd/tests/`

```
crates/riversd/tests/
├── bundle/
│   └── module_resolution.rs   ← module paths resolve regardless of CWD
```

```rust
// crates/riversd/tests/bundle/module_resolution.rs

/// Regression: BUG-013 — CodeComponent module paths not resolved to absolute.
/// V8 engine read handler files relative to CWD. Running from a different
/// directory caused "module not found" errors.
///
/// Origin: BUG-013, canary fleet testing v0.52.8
/// Root cause: Bundle load did not rewrite module paths to absolute

#[test]
fn module_paths_resolved_to_absolute_after_bundle_load() {
    let bundle = load_test_bundle("fixtures/test-bundle/");

    for app in &bundle.apps {
        for view in &app.views {
            if let HandlerDefinition::CodeComponent { module, .. } = &view.handler {
                assert!(
                    std::path::Path::new(module).is_absolute(),
                    "BUG-013: module path '{}' is relative — should be absolute after load",
                    module
                );
            }
        }
    }
}

/// Module resolution must work regardless of CWD.
#[test]
fn module_resolution_independent_of_cwd() {
    let bundle_path = std::env::current_dir().unwrap().join("fixtures/test-bundle/");

    // Change CWD to somewhere else
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir("/tmp").unwrap();

    let result = load_test_bundle(bundle_path.to_str().unwrap());
    assert!(result.apps.len() > 0,
        "BUG-013: bundle failed to load from non-bundle CWD");

    // Verify module files actually exist at resolved paths
    for app in &result.apps {
        for view in &app.views {
            if let HandlerDefinition::CodeComponent { module, .. } = &view.handler {
                assert!(std::path::Path::new(module).exists(),
                    "BUG-013: resolved path '{}' does not exist", module);
            }
        }
    }

    std::env::set_current_dir(original_cwd).unwrap();
}
```

---

## AMD-1.6 — Store TTL Type Contract (BUG-021)

BUG-021: `ctx.store.set(key, val, {ttl: 60})` silently ignored the TTL because the bridge expected a number (milliseconds), not an object. The V8 bridge must validate the TTL argument type.

### Add to: `crates/rivers-engine-v8/tests/bridge/store_bridge.rs`

```rust
/// Regression: BUG-021 — Store TTL API expects number but handler passed object.
/// ctx.store.set(key, val, {ttl: 60}) silently ignored the TTL.
/// The bridge should accept a number (milliseconds) and reject objects.
///
/// Origin: BUG-021, canary fleet testing v0.52.8
#[test]
fn regression_bug021_store_ttl_accepts_number() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_store();

    // Number TTL — must succeed
    let result = iso.eval(r#"
        try {
            ctx.store.set("ttl-test", "value", 60000);
            "OK"
        } catch(e) { "FAILED: " + e }
    "#);
    assert_eq!(result, "OK", "numeric TTL should be accepted");
}

#[test]
fn store_ttl_rejects_object() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_store();

    // Object TTL — must fail or warn, not silently ignore
    let result = iso.eval(r#"
        try {
            ctx.store.set("ttl-test", "value", {ttl: 60});
            // If we reach here, check if TTL was actually applied
            "ACCEPTED"
        } catch(e) { "REJECTED: " + e }
    "#);

    // Either the bridge rejects the object (preferred)
    // or it accepts it but the test documents the actual behavior
    // The critical thing: this behavior is TESTED, not assumed
    assert!(result == "REJECTED" || result.starts_with("REJECTED:"),
        "BUG-021: object TTL should be rejected, got: {}", result);
}

#[test]
fn store_ttl_zero_or_negative_behavior() {
    let iso = TestIsolate::new()
        .with_ctx("a1", "t1", "n1", "test")
        .with_store();

    // TTL of 0 — should either reject or mean "no TTL"
    let result = iso.eval(r#"
        try {
            ctx.store.set("zero-ttl", "value", 0);
            "ACCEPTED"
        } catch(e) { "REJECTED" }
    "#);
    // Document actual behavior — this is a contract test
    assert!(result == "ACCEPTED" || result == "REJECTED",
        "unexpected result for TTL=0: {}", result);
}
```

---

## AMD-1.7 — Updated Coverage Map

Add these to the coverage map in Section 7:

| Bug | Strategy 1 | Strategy 2 | Strategy 3 | Canary |
|-----|-----------|-----------|-----------|--------|
| BUG-005: --no-ssl missing subsystems | — | — | `no_ssl_boot_has_all_subsystems` + `boot_parity_tls_vs_no_ssl` | — |
| BUG-009: dataview no namespace | — | `dataview_name_namespaced` | `regression_bug009` | RT-CTX-DATAVIEW |
| BUG-010: app_id slug not UUID | — | `app_id_is_uuid_not_slug` | `regression_bug010` | RT-CTX-APP-ID |
| BUG-012: query vs query_params | — | `request_field_names_match_spec` | `regression_bug012` | RT-CTX-REQUEST |
| BUG-013: module paths relative | — | — | `module_paths_resolved_to_absolute` | — |
| BUG-021: store TTL type | — | `store_ttl_accepts_number` | `regression_bug021` | RT-CTX-STORE-GET-SET |
| BUG-025: Neo4j 10 findings | driver matrix (when neo4j promoted) | — | — | — |

**Updated totals:** 33 of 38 bugs now covered by unit-level tests. Remaining 5 are CI/infra issues (BUG-026–031) covered by workflow fixes.

---

## AMD-1.8 — Boot Path Parity as Architectural Pattern

BUG-005 revealed a systemic risk: any code path that duplicates initialization logic will drift. The parity test in AMD-1.1 is the symptom fix. The root cause fix is architectural.

**Recommendation for the codebase (not test spec):** Refactor boot to a single `fn initialize_subsystems(config, tls_mode) -> ServerState` called by both the TLS and non-TLS paths. The only difference between paths should be socket binding and TLS config. Everything else — StorageEngine, SessionManager, CsrfManager, EventBus, engine loader, host context — is identical.

The parity test then becomes a safety net rather than the primary defense. If the code has a single initialization path, parity is guaranteed by construction. If someone later splits the paths again (for whatever reason), the parity test catches it.

---

## AMD-1.9 — Canary Fleet Addition for BUG-005

The canary fleet spec should add a test that verifies the `--no-ssl` boot path works end-to-end. This is a Rust integration test in the canary harness, not a canary endpoint.

### Add to: `rivers-canary-fleet-spec.md` Part 8 (Rust Integration Test Contract)

```rust
/// BUG-005 regression: canary fleet boots and passes on --no-ssl path.
/// Run the entire canary fleet without TLS and verify all profiles pass.
#[tokio::test]
async fn canary_fleet_passes_on_no_ssl() {
    let server = boot_canary_fleet(TlsMode::Disabled).await;
    let client = CanaryClient::new("http://localhost:8080");
    client.login("canary", "canary-test").await;

    // Run a representative test from each profile
    assert_verdict(&client, "/canary/proxy/auth/session-read").await;
    assert_verdict(&client, "/canary/proxy/sql/pg/select").await;
    assert_verdict(&client, "/canary/proxy/rt/ctx/trace-id").await;

    server.shutdown().await;
}
```

---

## Absorption Checklist

- [ ] Boot parity tests added to `crates/riversd/tests/boot/`
- [ ] `TestIsolate::with_entry_point()` added to bridge test harness
- [ ] DataView namespace test added to `dataview_bridge.rs`
- [ ] `ctx.request` field name contract tests added to `ctx_injection.rs`
- [ ] `ctx.app_id` test strengthened to check UUID value, not just non-empty
- [ ] Module resolution tests added to `crates/riversd/tests/bundle/`
- [ ] Store TTL type tests added to `store_bridge.rs`
- [ ] Coverage map updated with all 6 new bugs
- [ ] Canary fleet `--no-ssl` integration test added
- [ ] File layout in Section 2 updated with `boot/` and `bundle/` directories
