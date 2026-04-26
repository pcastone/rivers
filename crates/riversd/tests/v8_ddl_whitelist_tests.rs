//! H1 contract tests — V8 `ctx.ddl()` consults the DDL whitelist.
//!
//! Phase B1 gated `ctx.ddl()` to `TaskKind::ApplicationInit`. H1 closes the
//! remaining gap: even when the gate allows the call through, the V8 callback
//! must enforce `[security].ddl_whitelist` the same way the dynamic-engine
//! callback (`engine_loader::host_callbacks::host_ddl_execute`) does.
//!
//! Lives in its own integration-test binary so the whitelist `OnceLock`s in
//! `engine_loader::host_context` (set once per process) cannot contaminate
//! the B1.5 success-path test in `task_kind_dispatch_tests.rs`, which runs
//! with no whitelist configured.
//!
//! Both tests in this binary share a single `DDL_WHITELIST` `OnceLock` value
//! installed via `init_whitelist_once` — running them in any order is safe.
//!
//! Run with: `cargo test -p riversd --test v8_ddl_whitelist_tests`

use std::collections::HashMap;
use std::sync::{Arc, Once};

use rivers_runtime::rivers_core::DriverFactory;
use rivers_runtime::rivers_driver_sdk::ConnectionParams;
use rivers_runtime::tiered_cache::NoopDataViewCache;
use rivers_runtime::{DataViewExecutor, DataViewRegistry};
use riversd::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskKind};

// ── Test fixture: shared whitelist + deterministic db paths ─────
//
// Both tests need to call `set_ddl_whitelist`, but it backs onto a `OnceLock`
// — only the first call wins, so we must install one combined whitelist
// up-front that satisfies the positive test and intentionally does NOT
// cover the negative test.

const APP_ID: &str = "h1-app";

fn positive_db_path() -> std::path::PathBuf {
    std::env::temp_dir().join("rivers_h1_positive.db")
}

fn negative_db_path() -> std::path::PathBuf {
    std::env::temp_dir().join("rivers_h1_negative.db")
}

fn init_whitelist_once() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Whitelist covers the positive test's db only.
        let entry = format!(
            "{}@{}",
            positive_db_path().to_string_lossy(),
            APP_ID,
        );
        riversd::engine_loader::set_ddl_whitelist(vec![entry]);
        // No app_id_map registered → V8 callback falls back to entry_point
        // as app_id, matching engine_loader/host_callbacks.rs:743-745.
    });
}

// ── Helpers ─────────────────────────────────────────────────────

fn manager() -> ProcessPoolManager {
    ProcessPoolManager::from_config(&HashMap::new())
}

fn js_file(name: &str, code: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rivers_h1_{name}_{id}.js"));
    std::fs::write(&path, code).unwrap();
    path
}

/// Build a SQLite-backed `DataViewExecutor` with a single datasource.
///
/// The datasource is registered under `"<APP_ID>:my_db"` so the B1 namespaced
/// resolver in `ctx_ddl_callback` finds it via the bare-name lookup path.
fn sqlite_executor(db_path: &std::path::Path) -> (Arc<DriverFactory>, Arc<DataViewExecutor>) {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(
        rivers_runtime::rivers_core::drivers::SqliteDriver::new(),
    ));
    let factory = Arc::new(factory);

    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "sqlite".to_string());
    let mut params_map = HashMap::new();
    params_map.insert(
        format!("{APP_ID}:my_db"),
        ConnectionParams {
            host: String::new(),
            port: 0,
            database: db_path.to_string_lossy().into(),
            username: String::new(),
            password: String::new(),
            options: opts,
        },
    );

    let executor = Arc::new(DataViewExecutor::new(
        DataViewRegistry::new(),
        factory.clone(),
        Arc::new(params_map),
        Arc::new(NoopDataViewCache),
    ));

    (factory, executor)
}

// ── H1.1: Whitelisted DDL succeeds ──────────────────────────────
//
// With a matching `database@app_id` entry configured, ApplicationInit
// `ctx.ddl()` runs the statement and the table actually persists in SQLite.
// This is the positive control proving the new gate doesn't false-reject
// permitted DDL.
#[tokio::test]
async fn h1_whitelisted_ddl_succeeds_for_application_init() {
    init_whitelist_once();

    let db_path = positive_db_path();
    // Clean any leftovers from a previous run so the table-existence check
    // at the end is meaningful.
    let _ = std::fs::remove_file(&db_path);
    let db_str: String = db_path.to_string_lossy().into();

    let (factory, executor) = sqlite_executor(&db_path);

    let path = js_file(
        "ddl_pos",
        r#"function handler(ctx) {
            ctx.ddl('my_db', 'CREATE TABLE h1_ok (id INTEGER PRIMARY KEY, note TEXT)');
            return { ok: true };
        }"#,
    );

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("h1-positive".into())
        .app_id(APP_ID.into())
        .node_id("node-h1".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::ApplicationInit)
        .driver_factory(factory.clone())
        .dataview_executor(executor)
        .build()
        .unwrap();

    let mgr = manager();
    let result = mgr.dispatch("default", ctx).await;
    let _ = std::fs::remove_file(&path);
    let result = result.expect("whitelisted ApplicationInit ctx.ddl should succeed");
    assert_eq!(result.value["ok"], true);

    // Sanity check — the table really got created on disk, proving the
    // whitelist check didn't short-circuit a no-op success.
    let mut conn = factory
        .connect(
            "sqlite",
            &ConnectionParams {
                host: String::new(),
                port: 0,
                database: db_str,
                username: String::new(),
                password: String::new(),
                options: {
                    let mut o = HashMap::new();
                    o.insert("driver".to_string(), "sqlite".to_string());
                    o
                },
            },
        )
        .await
        .unwrap();
    let q = rivers_runtime::rivers_driver_sdk::Query::new(
        "check",
        "SELECT name FROM sqlite_master WHERE type='table' AND name='h1_ok'",
    );
    let res = conn.execute(&q).await.unwrap();
    assert_eq!(res.rows.len(), 1, "h1_ok table must exist post-DDL");

    // Cleanup so re-runs start clean.
    drop(conn);
    let _ = std::fs::remove_file(&db_path);
}

// ── H1.2: Unwhitelisted DDL rejected ────────────────────────────
//
// Even though task_kind == ApplicationInit (B1 gate satisfied), DDL targeting
// a database NOT in the whitelist must throw. The error message must be
// byte-identical to what `host_ddl_execute` returns from the dynamic-engine
// path — operators see the same string regardless of which engine runs the
// init handler.
#[tokio::test]
async fn h1_unwhitelisted_ddl_rejected_for_application_init() {
    init_whitelist_once();

    let db_path = negative_db_path();
    let _ = std::fs::remove_file(&db_path);
    let db_str: String = db_path.to_string_lossy().into();

    let (factory, executor) = sqlite_executor(&db_path);

    let path = js_file(
        "ddl_neg",
        r#"function handler(ctx) {
            try {
                ctx.ddl('my_db', 'CREATE TABLE h1_blocked (id INTEGER PRIMARY KEY)');
                return { threw: false };
            } catch (e) {
                return { threw: true, msg: String(e.message || e) };
            }
        }"#,
    );

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("h1-negative".into())
        .app_id(APP_ID.into())
        .node_id("node-h1".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::ApplicationInit)
        .driver_factory(factory.clone())
        .dataview_executor(executor)
        .build()
        .unwrap();

    let mgr = manager();
    let result = mgr.dispatch("default", ctx).await;
    let _ = std::fs::remove_file(&path);
    let result = result.expect("dispatch itself must not error — JS catches the throw");

    assert_eq!(
        result.value["threw"], true,
        "ctx.ddl must throw when database not in whitelist"
    );
    let msg = result.value["msg"].as_str().unwrap_or("");

    // Must match host_ddl_execute (engine_loader/host_callbacks.rs:808-811)
    // verbatim. If this format ever drifts, fix the V8 callback to match —
    // never the test — because operators rely on a single error string for
    // alerting and log-search across both engine paths.
    let expected_fragment = format!(
        "DDL not permitted for database '{}' (datasource '{}') in app '{}'",
        db_str, "my_db", APP_ID,
    );
    assert!(
        msg.contains(&expected_fragment),
        "expected dynamic-engine error format, got: {msg}\n  expected to contain: {expected_fragment}"
    );

    // The rejected DDL must not have run. Connecting to the (now-empty)
    // SQLite file and listing tables proves the connect/execute path was
    // never reached for the rejected statement.
    let mut conn = factory
        .connect(
            "sqlite",
            &ConnectionParams {
                host: String::new(),
                port: 0,
                database: db_str,
                username: String::new(),
                password: String::new(),
                options: {
                    let mut o = HashMap::new();
                    o.insert("driver".to_string(), "sqlite".to_string());
                    o
                },
            },
        )
        .await
        .unwrap();
    let q = rivers_runtime::rivers_driver_sdk::Query::new(
        "check",
        "SELECT name FROM sqlite_master WHERE type='table' AND name='h1_blocked'",
    );
    let res = conn.execute(&q).await.unwrap();
    assert_eq!(
        res.rows.len(),
        0,
        "h1_blocked table must NOT exist — whitelist should reject before connect"
    );

    drop(conn);
    let _ = std::fs::remove_file(&db_path);
}
