//! C1 + B1 contract tests — task_kind plumbing and ctx.ddl() gating.
//!
//! Covers:
//!  - C1.4: cross-task store sharing for same app_id (MessageConsumer write
//!    visible to subsequent REST read)
//!  - C1.4: empty app_id is rejected at dispatch (no silent app:default)
//!  - C1.5: SecurityHook + ValidationHook see same ctx.app_id as REST
//!  - B1.4: REST/MessageConsumer/ValidationHook/SecurityHook ctx.ddl()
//!    throws "only available during application initialization"
//!  - B1.5: ApplicationInit ctx.ddl() succeeds (against in-memory SQLite)

use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::rivers_core::storage::{InMemoryStorageEngine, StorageEngine};
use riversd::process_pool::{
    Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskError, TaskKind,
};

// ── Helpers ─────────────────────────────────────────────────────

fn manager() -> ProcessPoolManager {
    ProcessPoolManager::from_config(&HashMap::new())
}

fn js_file(name: &str, code: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rivers_taskkind_{name}_{id}.js"));
    std::fs::write(&path, code).unwrap();
    path
}

fn build_task(
    path: &std::path::Path,
    args: serde_json::Value,
    app_id: &str,
    task_kind: TaskKind,
    storage: Option<Arc<dyn StorageEngine>>,
) -> Result<rivers_runtime::process_pool::TaskContext, TaskError> {
    let mut b = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(args)
        .trace_id(format!("trace-{app_id}-{:?}", task_kind))
        .app_id(app_id.into())
        .node_id("node-test".into())
        .runtime_env("test".into())
        .task_kind(task_kind);
    if let Some(s) = storage {
        b = b.storage(s);
    }
    b.build()
}

// ── C1.4: Cross-task store sharing for same app_id ──────────────

/// MessageConsumer writes ctx.store.set("k","v"); a later REST handler with
/// the same app_id can ctx.store.get("k") and see the value. This is the
/// concrete bug the empty-app_id fallback was masking — pre-C1, the consumer
/// wrote into "app:default" while the REST handler read from
/// "app:<real_app_id>" and got null.
#[tokio::test]
async fn c1_message_consumer_store_visible_to_rest_with_same_app_id() {
    let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
    let mgr = manager();

    // Step 1: MessageConsumer writes to ctx.store.
    let writer_path = js_file(
        "writer",
        "function handler(ctx) { ctx.store.set('shared_key', { from: 'consumer' }); return { ok: true }; }",
    );
    let writer_ctx = build_task(
        &writer_path,
        serde_json::json!({}),
        "shared-app",
        TaskKind::MessageConsumer,
        Some(storage.clone()),
    )
    .unwrap();
    let result = mgr.dispatch("default", writer_ctx).await.unwrap();
    assert_eq!(result.value["ok"], true);
    let _ = std::fs::remove_file(&writer_path);

    // Step 2: REST handler reads it. Same app_id → same namespace.
    let reader_path = js_file(
        "reader",
        "function handler(ctx) { var v = ctx.store.get('shared_key'); return { read: v }; }",
    );
    let reader_ctx = build_task(
        &reader_path,
        serde_json::json!({}),
        "shared-app",
        TaskKind::Rest,
        Some(storage.clone()),
    )
    .unwrap();
    let result = mgr.dispatch("default", reader_ctx).await.unwrap();
    let _ = std::fs::remove_file(&reader_path);
    assert_eq!(
        result.value["read"]["from"], "consumer",
        "REST handler must see MessageConsumer's write under same app_id"
    );

    // Sanity: the storage backend has the key under the EXPECTED namespace,
    // not "app:default" (which would be the masked-bug behavior).
    let raw = storage.get("app:shared-app", "shared_key").await.unwrap();
    assert!(raw.is_some(), "stored under app:shared-app");
    let absent = storage.get("app:default", "shared_key").await.unwrap();
    assert!(absent.is_none(), "MUST NOT land in app:default");
}

// ── C1.4: Empty app_id is rejected ──────────────────────────────

#[tokio::test]
async fn c1_empty_app_id_dispatch_is_rejected() {
    let mgr = manager();
    let path = js_file("noop", "function handler(ctx) { return { ok: true }; }");
    // Build the TaskContext WITHOUT calling .app_id() — app_id stays empty.
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("empty-app-id-test".into())
        .task_kind(TaskKind::Rest)
        .build()
        .unwrap();
    let result = mgr.dispatch("default", ctx).await;
    let _ = std::fs::remove_file(&path);
    let err = result.expect_err("dispatch with empty app_id must fail");
    let msg = err.to_string();
    assert!(matches!(err, TaskError::Internal(_)), "expected Internal, got: {msg}");
    assert!(msg.contains("empty app_id"), "expected 'empty app_id' in: {msg}");
}

// ── C1.5: SecurityHook / ValidationHook see same ctx.app_id ─────

#[tokio::test]
async fn c1_security_and_validation_hooks_see_same_app_id() {
    let mgr = manager();
    let read_app_id_js = "function handler(ctx) { return { app_id: ctx.app_id }; }";
    let path = js_file("readid", read_app_id_js);

    let app_id = "wrapping-rest-app";
    for kind in [TaskKind::Rest, TaskKind::SecurityHook, TaskKind::ValidationHook] {
        let ctx = build_task(&path, serde_json::json!({}), app_id, kind, None).unwrap();
        let result = mgr.dispatch("default", ctx).await.unwrap();
        assert_eq!(
            result.value["app_id"], app_id,
            "{kind:?} must see ctx.app_id == {app_id}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

// ── B1.4: ctx.ddl() rejected for non-init task kinds ────────────

async fn assert_ddl_rejected(kind: TaskKind) {
    let mgr = manager();
    let path = js_file(
        "ddl_neg",
        r#"function handler(ctx) {
            try { ctx.ddl('canary-faker', 'CREATE TABLE x ()'); return { threw: false }; }
            catch (e) { return { threw: true, msg: String(e.message || e) }; }
        }"#,
    );
    let ctx = build_task(&path, serde_json::json!({}), "test-app", kind, None).unwrap();
    let result = mgr.dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(result.value["threw"], true, "{kind:?}: ctx.ddl must throw");
    let msg = result.value["msg"].as_str().unwrap_or("");
    assert!(
        msg.contains("only available during application initialization"),
        "{kind:?}: expected gate message, got: {msg}"
    );
}

#[tokio::test]
async fn b1_ctx_ddl_rejected_for_rest() {
    assert_ddl_rejected(TaskKind::Rest).await;
}

#[tokio::test]
async fn b1_ctx_ddl_rejected_for_message_consumer() {
    assert_ddl_rejected(TaskKind::MessageConsumer).await;
}

#[tokio::test]
async fn b1_ctx_ddl_rejected_for_security_hook() {
    assert_ddl_rejected(TaskKind::SecurityHook).await;
}

#[tokio::test]
async fn b1_ctx_ddl_rejected_for_validation_hook() {
    assert_ddl_rejected(TaskKind::ValidationHook).await;
}

#[tokio::test]
async fn b1_ctx_ddl_rejected_for_pre_process() {
    assert_ddl_rejected(TaskKind::PreProcess).await;
}

#[tokio::test]
async fn b1_ctx_ddl_rejected_for_post_process() {
    assert_ddl_rejected(TaskKind::PostProcess).await;
}

// ── B1.5: ApplicationInit can call ctx.ddl() ────────────────────
//
// Wire a real SQLite datasource via DataViewExecutor so ctx.ddl resolves
// the connection params and the in-memory DB receives the CREATE TABLE.

#[tokio::test]
async fn b1_ctx_ddl_succeeds_for_application_init() {
    use rivers_runtime::rivers_core::DriverFactory;
    use rivers_runtime::rivers_driver_sdk::ConnectionParams;
    use rivers_runtime::tiered_cache::NoopDataViewCache;
    use rivers_runtime::{DataViewExecutor, DataViewRegistry};

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("init_ddl.db");

    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(
        rivers_runtime::rivers_core::drivers::SqliteDriver::new(),
    ));
    let factory = Arc::new(factory);

    let mut params_map = HashMap::new();
    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "sqlite".to_string());
    params_map.insert(
        "init-app:my_db".to_string(),
        ConnectionParams {
            host: String::new(),
            port: 0,
            database: db_path.to_string_lossy().into(),
            username: String::new(),
            password: String::new(),
            options: opts,
        },
    );

    let registry = DataViewRegistry::new();
    let executor = Arc::new(DataViewExecutor::new(
        registry,
        factory.clone(),
        Arc::new(params_map),
        Arc::new(NoopDataViewCache),
    ));

    let path = js_file(
        "ddl_pos",
        r#"function handler(ctx) {
            ctx.ddl('my_db', 'CREATE TABLE init_ok (id INTEGER PRIMARY KEY, note TEXT)');
            return { ok: true };
        }"#,
    );

    let mgr = manager();
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("ddl-init-positive".into())
        .app_id("init-app".into())
        .node_id("node-init".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::ApplicationInit)
        .driver_factory(factory.clone())
        .dataview_executor(executor)
        .build()
        .unwrap();

    let result = mgr.dispatch("default", ctx).await;
    let _ = std::fs::remove_file(&path);
    let result = result.expect("ApplicationInit ctx.ddl should succeed");
    assert_eq!(result.value["ok"], true);

    // Verify the table actually exists by querying sqlite_master.
    let mut conn = factory
        .connect(
            "sqlite",
            &ConnectionParams {
                host: String::new(),
                port: 0,
                database: db_path.to_string_lossy().into(),
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
        "SELECT name FROM sqlite_master WHERE type='table' AND name='init_ok'",
    );
    let res = conn.execute(&q).await.unwrap();
    assert_eq!(res.rows.len(), 1, "init_ok table should exist after ctx.ddl()");
}
