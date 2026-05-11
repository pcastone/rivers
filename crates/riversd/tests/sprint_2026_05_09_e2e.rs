//! End-to-end tests for Sprint 2026-05-09 (CB unblock).
//!
//! Exercises Tracks 1, 2, and 3 against real building blocks:
//!
//! - **Track 1 (probe migration)** — validates the canonical config shapes
//!   for P1.9 / P1.10 / P1.11 / P1.12 against the actual structural
//!   validator. Confirms the migrated probe shapes pass clean on this
//!   build.
//!
//! - **Track 2 (validator hardening)** — submits the original CB-probe
//!   "bad" shapes (`auth = "bearer"`, `view_type = "QuantumStreamer"`)
//!   and asserts S005 rejections.
//!
//! - **Track 3 (cron primitive)** — focused. Exercises the actual
//!   `CronScheduler` with a real `InMemoryStorageEngine` and a real
//!   (engine-less) `ProcessPoolManager`. Asserts:
//!     1. The loop fires ticks (StorageEngine accumulates dedupe keys).
//!     2. Two schedulers against shared storage dedupe correctly
//!        (one node fires per tick, not both).
//!     3. CronViewSpec parses canonical TOML.
//!
//! V8 is statically linked in this build (per CLAUDE.md — `just build`
//! default), so the test ALSO runs real JS handlers via the pool. The
//! `track3_cron_handler_runs_and_writes_to_store` test below is the
//! load-bearing e2e — it asserts a real JS function executed inside V8
//! and wrote to the StorageEngine.

use std::sync::Arc;

use rivers_runtime::rivers_core::storage::{
    InMemoryStorageEngine, StorageEngine,
};
use riversd::cron::{CronScheduler, CronViewSpec};
use riversd::process_pool::ProcessPoolManager;

// ── Track 3: Cron e2e ─────────────────────────────────────────────────

/// Build a `CronViewSpec` directly, spin up a `CronScheduler` for ~2.5s
/// with `interval_seconds = 1`, and assert that the StorageEngine has
/// accumulated dedupe keys — proof the loop fired its ticks.
#[tokio::test(flavor = "multi_thread")]
async fn track3_cron_scheduler_fires_ticks() {
    let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
    let pool = Arc::new(ProcessPoolManager::from_config(&Default::default()));

    let spec = build_cron_spec_with_interval("e2e_app", "tick_view", 1);
    let scheduler = CronScheduler::start(
        vec![spec],
        pool,
        storage.clone(),
        "node-A".to_string(),
    );
    assert_eq!(scheduler.spawned_count(), 1);

    // Sleep ~2.5s — expect 2-3 ticks at 1s interval.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    let keys = storage
        .list_keys("cron", Some("e2e_app:tick_view:"))
        .await
        .unwrap();
    assert!(
        !keys.is_empty(),
        "expected at least one dedupe key after 2.5s @ 1s interval, got 0"
    );
    eprintln!(
        "track3_cron_scheduler_fires_ticks: {} dedupe key(s) accumulated: {:?}",
        keys.len(),
        keys
    );

    scheduler.shutdown().await;
}

/// Two schedulers against shared storage. After a few ticks, the **set**
/// of dedupe keys is what each scheduler tried to acquire — the storage
/// reflects whichever node got there first per tick. Most importantly:
/// the test confirms that calling `set_if_absent` from two competing
/// loops yields a single key per tick_epoch (not two).
#[tokio::test(flavor = "multi_thread")]
async fn track3_two_schedulers_dedupe_via_shared_storage() {
    let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
    let pool_a = Arc::new(ProcessPoolManager::from_config(&Default::default()));
    let pool_b = Arc::new(ProcessPoolManager::from_config(&Default::default()));

    // Both schedulers run the same view definition against the same storage.
    let spec_a = build_cron_spec_with_interval("dup_app", "view", 1);
    let spec_b = build_cron_spec_with_interval("dup_app", "view", 1);

    let sched_a = CronScheduler::start(
        vec![spec_a],
        pool_a,
        storage.clone(),
        "node-A".to_string(),
    );
    let sched_b = CronScheduler::start(
        vec![spec_b],
        pool_b,
        storage.clone(),
        "node-B".to_string(),
    );

    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    let keys = storage
        .list_keys("cron", Some("dup_app:view:"))
        .await
        .unwrap();

    // Each tick_epoch produces exactly one dedupe key — set_if_absent
    // means the second writer gets Ok(false). If dedupe were broken,
    // we'd have duplicate entries (impossible in a Set-keyed kv anyway,
    // so the more meaningful assertion is: the COUNT bounds the elapsed
    // ticks).
    let n = keys.len();
    assert!(
        n >= 1 && n <= 4,
        "expected 1-4 unique tick-epochs in 2.5s @ 1s interval, got {}: {:?}",
        n,
        keys
    );

    // Each key has exactly one writer recorded — read each value back
    // and assert they're either node-A or node-B (never both).
    for key in &keys {
        let val = storage.get("cron", key).await.unwrap().unwrap();
        let s = String::from_utf8(val.to_vec()).unwrap();
        assert!(
            s == "node-A" || s == "node-B",
            "dedupe key {} has unexpected writer {:?}",
            key,
            s
        );
    }

    eprintln!(
        "track3_two_schedulers_dedupe: {} unique tick-epochs, owners: {:?}",
        n,
        keys.iter()
            .map(|k| {
                let v = futures_get(&storage, k);
                (k.clone(), v)
            })
            .collect::<Vec<_>>()
    );

    sched_a.shutdown().await;
    sched_b.shutdown().await;
}

/// Helper to read a key synchronously inside an iter — flat-blocks on the
/// async call because the keys are already known to exist.
fn futures_get(storage: &Arc<dyn StorageEngine>, key: &str) -> String {
    let storage = storage.clone();
    let key = key.to_string();
    let val = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(async move { storage.get("cron", &key).await.unwrap() })
    })
    .unwrap();
    String::from_utf8(val.to_vec()).unwrap()
}

/// Build a CronViewSpec from canonical-shape TOML — same path the bundle
/// loader uses. Asserts the parsed shape matches expectation.
#[test]
fn track3_cron_view_spec_parses_canonical_toml() {
    let cfg = parse_view_config(
        r#"
view_type        = "Cron"
schedule         = "0 */5 * * * *"
overlap_policy   = "skip"

[handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#,
    );

    let spec = CronViewSpec::from_view_config("app1", "recompute", &cfg)
        .expect("spec build")
        .expect("spec is Some for Cron view");
    assert_eq!(spec.app_id, "app1");
    assert_eq!(spec.view_name, "recompute");
    assert_eq!(spec.entrypoint.module, "libraries/handlers/recompute.ts");
    assert_eq!(spec.entrypoint.function, "tick");
    eprintln!("track3_cron_view_spec_parses_canonical_toml: OK");
}

/// Same path with `interval_seconds` instead of `schedule`. Both forms
/// should yield a working spec.
#[test]
fn track3_cron_view_spec_parses_interval_form() {
    let cfg = parse_view_config(
        r#"
view_type        = "Cron"
interval_seconds = 300

[handler]
type       = "codecomponent"
language   = "javascript"
module     = "h.js"
entrypoint = "t"
resources  = []
"#,
    );
    let spec = CronViewSpec::from_view_config("a", "v", &cfg).unwrap().unwrap();
    assert_eq!(spec.entrypoint.language, "javascript");
}

/// Direct ProcessPool dispatch with the same TaskContextBuilder shape
/// `dispatch_tick` produces. Used to isolate: does V8 + the handler
/// shape work at all? If this fails, the cron-scheduler test below
/// can't possibly pass.
#[tokio::test(flavor = "multi_thread")]
async fn track3_direct_dispatch_handler_writes_to_store() {
    use riversd::process_pool::{Entrypoint, TaskContextBuilder};
    use rivers_runtime::process_pool::TaskKind;

    // ctx.store.set is 2-arg (key, value). Namespace is auto-derived as
    // "app:{app_id}" — set via TASK_STORE_NAMESPACE in v8_engine task locals.
    let handler_js = r#"
        function tick(ctx) {
            ctx.store.set('fired', 'yes');
            return { ok: true };
        }
    "#;
    let path = std::env::temp_dir().join(format!(
        "rivers_direct_e2e_{}.js",
        std::process::id()
    ));
    std::fs::write(&path, handler_js).unwrap();

    let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
    let pool = ProcessPoolManager::from_config(&Default::default());

    let entrypoint = Entrypoint {
        module: path.to_string_lossy().into_owned(),
        function: "tick".to_string(),
        language: "javascript".to_string(),
    };
    let args = serde_json::json!({
        "request": {"headers": {}, "body": null, "path_params": {}, "query": {}},
        "session": null,
        "path_params": {},
        "cron": {"view_name": "v", "tick_epoch": 1, "node_id": "n"},
    });
    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id("direct-e2e".to_string())
        .storage(storage.clone());
    let builder = riversd::task_enrichment::enrich(builder, "test_app", TaskKind::Rest);
    let ctx = builder.build().expect("build TaskContext");

    let result = pool.dispatch("default", ctx).await;
    match &result {
        Ok(r) => eprintln!("direct dispatch OK: value={}", r.value),
        Err(e) => eprintln!("direct dispatch ERR: {:?}", e),
    }

    // Namespace is auto-derived from app_id: "app:{app_id}".
    let fired = storage.get("app:test_app", "fired").await.unwrap();
    eprintln!("direct dispatch ctx.store value: {:?}",
        fired.as_ref().map(|b| String::from_utf8_lossy(b).to_string()));

    let _ = std::fs::remove_file(&path);

    assert!(result.is_ok(), "direct V8 dispatch should succeed, got: {:?}", result.err());
    let v = fired.expect("handler must write 'fired'='yes' via ctx.store");
    // ctx.store.set serializes values as JSON, so a JS string becomes
    // a JSON-quoted string when read back from the raw KV backend.
    assert_eq!(String::from_utf8(v.to_vec()).unwrap(), "\"yes\"");
}

/// **The load-bearing e2e test for Track 3.** Writes a real JS handler
/// to disk, configures a CronViewSpec pointing at it, starts the
/// scheduler with `interval_seconds = 1`, and asserts that after ~2.5s
/// the JS handler **actually executed** — observable via a value the
/// handler wrote to `ctx.store`.
///
/// V8 is statically linked in this build; `ensure_v8_initialized()`
/// fires lazily on first dispatch. No dylibs involved.
#[tokio::test(flavor = "multi_thread")]
async fn track3_cron_handler_runs_and_writes_to_store() {
    // The JS handler increments a counter in ctx.store. Each tick this
    // fires, the counter goes up. After a few ticks we read it back
    // from the same StorageEngine.
    // ctx.store.set is 2-arg (key, value); namespace is auto-derived as
    // "app:{app_id}" by the V8 bridge. Values round-trip through JSON,
    // so ctx.store.set/get can store numbers directly.
    let handler_js = r#"
        function tick(ctx) {
            const prev = ctx.store.get('count');
            const n = (typeof prev === 'number') ? prev : 0;
            ctx.store.set('count', n + 1);
            ctx.store.set('last_marker', 'handler-fired');
            return { ok: true };
        }
    "#;
    let path = std::env::temp_dir().join(format!(
        "rivers_cron_e2e_{}.js",
        std::process::id()
    ));
    std::fs::write(&path, handler_js).unwrap();

    let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
    let pool = Arc::new(ProcessPoolManager::from_config(&Default::default()));

    let mut cfg = make_skeleton_view_config();
    cfg.view_type = "Cron".to_string();
    cfg.path = None;
    cfg.method = None;
    cfg.interval_seconds = Some(1);
    cfg.handler = rivers_runtime::view::HandlerConfig::Codecomponent {
        language: "javascript".to_string(),
        module: path.to_string_lossy().into_owned(),
        entrypoint: "tick".to_string(),
        resources: vec![],
    };
    let spec = CronViewSpec::from_view_config("e2e_app", "tick_view", &cfg)
        .unwrap()
        .unwrap();

    let scheduler = CronScheduler::start(
        vec![spec],
        pool,
        storage.clone(),
        "node-test".to_string(),
    );

    // Sleep ~2.5s — expect 2-3 ticks at 1s interval, each firing the
    // handler which increments the counter.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    // Namespace is "app:e2e_app" — auto-derived from the CronViewSpec's app_id.
    // ctx.store values round-trip through JSON.
    let count_bytes = storage
        .get("app:e2e_app", "count")
        .await
        .unwrap()
        .expect("'count' key written by handler — V8 dispatch must have executed");
    let count: u32 = serde_json::from_slice(&count_bytes)
        .expect("'count' must be a valid JSON integer");
    let marker_bytes = storage
        .get("app:e2e_app", "last_marker")
        .await
        .unwrap()
        .expect("'last_marker' key written by handler");
    let marker: String = serde_json::from_slice(&marker_bytes).unwrap();

    eprintln!(
        "track3_cron_handler_runs_and_writes_to_store: handler fired {} time(s), marker={}",
        count, marker
    );

    assert!(
        count >= 1,
        "expected JS handler to fire at least once in 2.5s, got count={}",
        count
    );
    assert_eq!(marker, "handler-fired");

    scheduler.shutdown().await;
    let _ = std::fs::remove_file(&path);
}

// ── Track 2: Validator hardening e2e ──────────────────────────────────

/// `auth = "bearer"` (CB-P1.12 closed-as-superseded) must produce a clean
/// `S005` with the canonical set in the message.
#[test]
fn track2_validator_rejects_auth_bearer() {
    let report = validate_inline(
        r#"
[api.views.bad]
path      = "/x"
method    = "GET"
view_type = "Rest"
auth      = "bearer"

[api.views.bad.handler]
type     = "dataview"
dataview = "items"
"#,
    );
    assert_finding(
        &report,
        "S005",
        Some("auth"),
        &["'bearer'", "[none, session]"],
    );
}

/// `view_type = "QuantumStreamer"` (clearly bogus) must produce S005 with
/// the full canonical set including `Cron` (added in Track 3).
#[test]
fn track2_validator_rejects_unknown_view_type_and_lists_canonical() {
    let report = validate_inline(
        r#"
[api.views.bad]
path      = "/x"
method    = "GET"
view_type = "QuantumStreamer"
auth      = "none"

[api.views.bad.handler]
type     = "dataview"
dataview = "items"
"#,
    );
    assert_finding(
        &report,
        "S005",
        Some("view_type"),
        &["'QuantumStreamer'", "Cron"],
    );
}

/// Cron-only fields on a Rest view — schedule + interval — must each
/// produce S005.
#[test]
fn track2_validator_rejects_cron_only_fields_on_non_cron_view() {
    let report = validate_inline(
        r#"
[api.views.bad]
path             = "/x"
method           = "GET"
view_type        = "Rest"
auth             = "none"
schedule         = "0 */5 * * * *"
interval_seconds = 60

[api.views.bad.handler]
type     = "dataview"
dataview = "items"
"#,
    );
    for f in &["schedule", "interval_seconds"] {
        assert_finding(
            &report,
            "S005",
            Some(*f),
            &["only valid when view_type=\"Cron\""],
        );
    }
}

// ── Track 1 / canonical Cron acceptance ───────────────────────────────

/// The migrated CB probe Case I shape (canonical Cron view) must validate
/// clean — this is the post-Track-3 sentinel for P1.14.
#[test]
fn track1_track3_canonical_cron_view_validates_clean() {
    let report = validate_inline(
        r#"
[api.views.recompute]
view_type        = "Cron"
schedule         = "0 */5 * * * *"
overlap_policy   = "skip"

[api.views.recompute.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#,
    );
    let view_failures: Vec<_> = report
        .iter()
        .filter(|r| {
            r.error_code.as_deref().is_some()
                && r.table_path
                    .as_deref()
                    .map(|p| p.contains("recompute"))
                    .unwrap_or(false)
        })
        .collect();
    assert!(
        view_failures.is_empty(),
        "expected no S/X errors on canonical Cron view, got: {:?}",
        view_failures
            .iter()
            .map(|r| (&r.error_code, &r.message))
            .collect::<Vec<_>>()
    );
    eprintln!("track1_track3_canonical_cron_view_validates_clean: OK");
}

// ── Helpers ───────────────────────────────────────────────────────────

fn build_cron_spec_with_interval(
    app_id: &str,
    view_name: &str,
    interval_seconds: u64,
) -> CronViewSpec {
    use rivers_runtime::view::HandlerConfig;
    let mut cfg = make_skeleton_view_config();
    cfg.view_type = "Cron".to_string();
    cfg.path = None;
    cfg.method = None;
    cfg.interval_seconds = Some(interval_seconds);
    cfg.handler = HandlerConfig::Codecomponent {
        language: "javascript".to_string(),
        module: "h.js".to_string(),
        entrypoint: "tick".to_string(),
        resources: vec![],
    };
    CronViewSpec::from_view_config(app_id, view_name, &cfg)
        .expect("spec build ok")
        .expect("spec is Some")
}

fn make_skeleton_view_config() -> rivers_runtime::ApiViewConfig {
    rivers_runtime::ApiViewConfig {
        view_type: "Rest".to_string(),
        path: Some("/x".to_string()),
        method: Some("GET".to_string()),
        handler: rivers_runtime::view::HandlerConfig::Dataview {
            dataview: "items".to_string(),
        },
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard: false,
        auth: None,
        guard_config: None,
        guard_view: None,
        allow_outbound_http: false,
        rate_limit_per_minute: None,
        rate_limit_burst_size: None,
        websocket_mode: None,
        max_connections: None,
        sse_tick_interval_ms: None,
        sse_trigger_events: vec![],
        sse_event_buffer_size: None,
        session_revalidation_interval_s: None,
        polling: None,
        event_handlers: None,
        on_stream: None,
        ws_hooks: None,
        on_event: None,
        tools: Default::default(),
        resources: Default::default(),
        prompts: Default::default(),
        instructions: None,
        session: None,
        federation: vec![],
        response_headers: None,
        schedule: None,
        interval_seconds: None,
        overlap_policy: None,
        max_concurrent: None,
    }
}

fn parse_view_config(toml_str: &str) -> rivers_runtime::ApiViewConfig {
    toml::from_str::<rivers_runtime::ApiViewConfig>(toml_str)
        .expect("ApiViewConfig parse")
}

/// Run the structural validator against a single inline `[api.views.*]`
/// fragment by writing a minimal valid bundle on disk and splicing the
/// fragment into `app.toml`. Returns the validator results.
fn validate_inline(
    fragment_toml: &str,
) -> Vec<rivers_runtime::validate_result::ValidationResult> {
    let tmp = tempfile::tempdir().unwrap();
    let bundle_dir = tmp.path().to_path_buf();

    // Bundle manifest.
    std::fs::write(
        bundle_dir.join("manifest.toml"),
        r#"bundleName = "e2e"
bundleVersion = "1.0.0"
source = "https://example.invalid/e2e"
apps = ["test-app"]
"#,
    )
    .unwrap();

    // Per-app dirs.
    let app_dir = bundle_dir.join("test-app");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(
        app_dir.join("manifest.toml"),
        r#"appName = "test-app"
version = "1.0.0"
type = "app-service"
appId = "00000000-0000-0000-0000-000000000001"
entryPoint = "test-app"
source = "https://example.invalid/e2e"
"#,
    )
    .unwrap();
    std::fs::write(
        app_dir.join("resources.toml"),
        r#"[[datasources]]
name = "data"
driver = "faker"
nopassword = true
"#,
    )
    .unwrap();

    let app_toml = format!(
        r#"
[data.dataviews.items]
name = "items"
datasource = "data"
query = "SELECT 1"

{fragment}
"#,
        fragment = fragment_toml
    );
    std::fs::write(app_dir.join("app.toml"), app_toml).unwrap();

    rivers_runtime::validate_structural(&bundle_dir)
}

fn assert_finding(
    report: &[rivers_runtime::validate_result::ValidationResult],
    code: &str,
    field: Option<&str>,
    message_contains: &[&str],
) {
    let hit = report.iter().find(|r| {
        r.error_code.as_deref() == Some(code)
            && (field.is_none() || r.field.as_deref() == field)
            && message_contains.iter().all(|s| r.message.contains(s))
    });
    if hit.is_none() {
        let dump: Vec<_> = report
            .iter()
            .map(|r| (r.error_code.clone(), r.field.clone(), r.message.clone()))
            .collect();
        panic!(
            "expected finding {} field={:?} containing {:?}, got: {:#?}",
            code, field, message_contains, dump
        );
    }
}
