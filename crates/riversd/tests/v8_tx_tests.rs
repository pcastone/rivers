//! V8 Rivers.db.tx integration tests (TXN-D.6, TXN-E.4, TXN-F.3).
//!
//! Verifies begin/query/commit/rollback/peek semantics, auto-rollback on
//! handler exit without commit, nested-begin rejection, cross-datasource
//! rejection, and peek-before-query rejection.
//!
//! Uses the built-in SQLite driver against temp-file databases so these
//! tests run without any external service.
//!
//! Run: `cargo test -p riversd --test v8_tx_tests`

use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::dataview::{DataViewConfig, DataViewParameterConfig};
use rivers_runtime::dataview_engine::{DataViewExecutor, DataViewRegistry};
use rivers_runtime::process_pool::types::{Entrypoint, ResolvedDatasource};
use rivers_runtime::rivers_core::DriverFactory;
use rivers_runtime::rivers_driver_sdk::ConnectionParams;
use rivers_runtime::tiered_cache::NoopDataViewCache;
use riversd::process_pool::{ProcessPoolManager, TaskContextBuilder, TaskKind};

// ── Helpers ─────────────────────────────────────────────────────────────────

const DS: &str = "txdb";

fn manager() -> ProcessPoolManager {
    ProcessPoolManager::from_config(&HashMap::new())
}

fn js_file(name: &str, code: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rivers_v8_tx_{name}_{id}.js"));
    std::fs::write(&path, code).unwrap();
    path
}

struct TxFixture {
    db_path: std::path::PathBuf,
    factory: Arc<DriverFactory>,
    executor: Arc<DataViewExecutor>,
}

fn make_fixture(suffix: &str) -> TxFixture {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let id = CTR.fetch_add(1, Ordering::Relaxed);
    let db_path = std::env::temp_dir().join(format!("rivers_v8_tx_{suffix}_{id}.db"));
    let _ = std::fs::remove_file(&db_path);

    // Create the items table directly — avoids needing a setup V8 dispatch.
    {
        let conn = rusqlite::Connection::open(&db_path).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, val TEXT);",
        )
        .expect("create table");
    }

    let db_str = db_path.to_string_lossy().to_string();

    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "sqlite".to_string());

    let mut params_map: HashMap<String, ConnectionParams> = HashMap::new();
    params_map.insert(
        DS.to_string(),
        ConnectionParams {
            host: String::new(),
            port: 0,
            database: db_str,
            username: String::new(),
            password: String::new(),
            options: opts,
        },
    );

    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(
        rivers_runtime::rivers_core::drivers::SqliteDriver::new(),
    ));
    let factory = Arc::new(factory);

    let mut registry = DataViewRegistry::new();

    registry.register(DataViewConfig {
        name: "insert_item".into(),
        datasource: DS.into(),
        query: Some("INSERT INTO items (val) VALUES ($val)".into()),
        parameters: vec![DataViewParameterConfig {
            name: "val".into(),
            param_type: "string".into(),
            required: true,
            default: None,
            location: None,
        }],
        ..minimal_dv()
    });

    registry.register(DataViewConfig {
        name: "count_items".into(),
        datasource: DS.into(),
        query: Some("SELECT COUNT(*) AS n FROM items".into()),
        ..minimal_dv()
    });

    let executor = Arc::new(DataViewExecutor::new(
        registry,
        factory.clone(),
        Arc::new(params_map),
        Arc::new(NoopDataViewCache),
    ));

    TxFixture { db_path, factory, executor }
}

fn minimal_dv() -> DataViewConfig {
    DataViewConfig {
        name: String::new(),
        datasource: DS.into(),
        query: None,
        parameters: vec![],
        caching: None,
        return_schema: None,
        invalidates: vec![],
        validate_result: false,
        strict_parameters: false,
        get_query: None,
        post_query: None,
        put_query: None,
        delete_query: None,
        get_schema: None,
        post_schema: None,
        put_schema: None,
        delete_schema: None,
        get_parameters: vec![],
        post_parameters: vec![],
        put_parameters: vec![],
        delete_parameters: vec![],
        streaming: false,
        circuit_breaker_id: None,
        prepared: false,
        query_params: HashMap::new(),
        max_rows: 1000,
        skip_introspect: false,
        cursor_key: None,
        source_views: vec![],
        compose_strategy: None,
        join_key: None,
        enrich_mode: "nest".into(),
        transaction: false,
    }
}

fn resolved(db_path: &std::path::Path) -> ResolvedDatasource {
    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "sqlite".to_string());
    ResolvedDatasource {
        driver_name: "sqlite".to_string(),
        params: ConnectionParams {
            host: String::new(),
            port: 0,
            database: db_path.to_string_lossy().into(),
            username: String::new(),
            password: String::new(),
            options: opts,
        },
    }
}

fn count_rows_direct(db_path: &std::path::Path) -> u64 {
    let conn = rusqlite::Connection::open(db_path).expect("open for count");
    conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as u64
}

// ── TXN-D.6.a — begin/query/commit persists write ───────────────────────────
#[tokio::test]
async fn tx_begin_query_commit_persists_write() {
    let fix = make_fixture("commit");
    let path = js_file(
        "commit",
        r#"function handler(ctx) {
            const tx = Rivers.db.tx.begin("txdb");
            tx.query("insert_item", { val: "hello" });
            const results = tx.commit();
            return { affected: results["insert_item"][0].affected_rows };
        }"#,
    );
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("tx-commit".into())
        .app_id("test-app".into())
        .node_id("node-tx".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .driver_factory(fix.factory.clone())
        .dataview_executor(fix.executor.clone())
        .datasource_config(DS.into(), resolved(&fix.db_path))
        .build()
        .unwrap();

    let result = manager().dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(result.value["affected"], 1);
    assert_eq!(count_rows_direct(&fix.db_path), 1, "committed row must persist");
    let _ = std::fs::remove_file(&fix.db_path);
}

// ── TXN-D.6.b — begin/query/rollback discards write ─────────────────────────
#[tokio::test]
async fn tx_begin_query_rollback_discards_write() {
    let fix = make_fixture("rollback");
    let path = js_file(
        "rollback",
        r#"function handler(ctx) {
            const tx = Rivers.db.tx.begin("txdb");
            tx.query("insert_item", { val: "discarded" });
            tx.rollback();
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
        .trace_id("tx-rollback".into())
        .app_id("test-app".into())
        .node_id("node-tx".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .driver_factory(fix.factory.clone())
        .dataview_executor(fix.executor.clone())
        .datasource_config(DS.into(), resolved(&fix.db_path))
        .build()
        .unwrap();

    let result = manager().dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(result.value["ok"], true);
    assert_eq!(count_rows_direct(&fix.db_path), 0, "rolled-back row must not persist");
    let _ = std::fs::remove_file(&fix.db_path);
}

// ── TXN-D.6.c — nested begin throws (TX-4) ──────────────────────────────────
#[tokio::test]
async fn tx_nested_begin_throws() {
    let fix = make_fixture("nested");
    let path = js_file(
        "nested",
        r#"function handler(ctx) {
            const tx = Rivers.db.tx.begin("txdb");
            try {
                Rivers.db.tx.begin("txdb");
                tx.rollback();
                return { threw: false };
            } catch (e) {
                tx.rollback();
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
        .trace_id("tx-nested".into())
        .app_id("test-app".into())
        .node_id("node-tx".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .driver_factory(fix.factory.clone())
        .dataview_executor(fix.executor.clone())
        .datasource_config(DS.into(), resolved(&fix.db_path))
        .build()
        .unwrap();

    let result = manager().dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&fix.db_path);
    assert_eq!(result.value["threw"], true, "nested begin must throw");
    let msg = result.value["msg"].as_str().unwrap_or("");
    assert!(
        msg.contains("nested transactions not supported"),
        "got: {msg}"
    );
}

// ── TXN-D.6.e — tx.peek before any tx.query throws (PK-2) ───────────────────
#[tokio::test]
async fn tx_peek_before_query_throws() {
    let fix = make_fixture("peek_early");
    let path = js_file(
        "peek_early",
        r#"function handler(ctx) {
            const tx = Rivers.db.tx.begin("txdb");
            try {
                tx.peek("insert_item");
                tx.rollback();
                return { threw: false };
            } catch (e) {
                tx.rollback();
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
        .trace_id("tx-peek-early".into())
        .app_id("test-app".into())
        .node_id("node-tx".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .driver_factory(fix.factory.clone())
        .dataview_executor(fix.executor.clone())
        .datasource_config(DS.into(), resolved(&fix.db_path))
        .build()
        .unwrap();

    let result = manager().dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&fix.db_path);
    assert_eq!(result.value["threw"], true, "peek before query must throw");
    let msg = result.value["msg"].as_str().unwrap_or("");
    assert!(msg.contains("no results for"), "got: {msg}");
}

// ── TXN-D.6.f — tx.peek accumulates results and is idempotent (PK-5) ─────────
#[tokio::test]
async fn tx_peek_accumulates_and_is_idempotent() {
    let fix = make_fixture("peek_accum");
    let path = js_file(
        "peek_accum",
        r#"function handler(ctx) {
            const tx = Rivers.db.tx.begin("txdb");
            tx.query("insert_item", { val: "a" });
            tx.query("insert_item", { val: "b" });
            const p1 = tx.peek("insert_item");
            const p2 = tx.peek("insert_item");
            tx.commit();
            return { p1_len: p1.length, p2_len: p2.length };
        }"#,
    );
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("tx-peek-accum".into())
        .app_id("test-app".into())
        .node_id("node-tx".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .driver_factory(fix.factory.clone())
        .dataview_executor(fix.executor.clone())
        .datasource_config(DS.into(), resolved(&fix.db_path))
        .build()
        .unwrap();

    let result = manager().dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&fix.db_path);
    assert_eq!(result.value["p1_len"], 2, "two queries must give peek array of length 2");
    assert_eq!(result.value["p2_len"], 2, "peek must be idempotent");
}

// ── TXN-E.4.a — handler exits without commit → auto-rollback (AR-1/AR-2) ────
#[tokio::test]
async fn tx_auto_rollback_on_handler_exit_without_commit() {
    let fix = make_fixture("autorollback");
    let path = js_file(
        "no_commit",
        r#"function handler(ctx) {
            const tx = Rivers.db.tx.begin("txdb");
            tx.query("insert_item", { val: "should_not_persist" });
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
        .trace_id("tx-autorollback".into())
        .app_id("test-app".into())
        .node_id("node-tx".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .driver_factory(fix.factory.clone())
        .dataview_executor(fix.executor.clone())
        .datasource_config(DS.into(), resolved(&fix.db_path))
        .build()
        .unwrap();

    let result = manager().dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    // AR-4: handler return value is preserved
    assert_eq!(result.value["ok"], true);
    // Write must have been auto-rolled back
    assert_eq!(
        count_rows_direct(&fix.db_path),
        0,
        "auto-rollback must prevent uncommitted write from persisting"
    );
    let _ = std::fs::remove_file(&fix.db_path);
}

// ── TXN-E.4.b — handler throws → auto-rollback fires ───────────────────────
#[tokio::test]
async fn tx_auto_rollback_on_handler_throw() {
    let fix = make_fixture("throw_rollback");
    let path = js_file(
        "tx_throw",
        r#"function handler(ctx) {
            const tx = Rivers.db.tx.begin("txdb");
            tx.query("insert_item", { val: "thrown_away" });
            throw new Error("handler error");
        }"#,
    );
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("tx-throw".into())
        .app_id("test-app".into())
        .node_id("node-tx".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .driver_factory(fix.factory.clone())
        .dataview_executor(fix.executor.clone())
        .datasource_config(DS.into(), resolved(&fix.db_path))
        .build()
        .unwrap();

    let result = manager().dispatch("default", ctx).await;
    let _ = std::fs::remove_file(&path);
    assert!(result.is_err(), "thrown handler must propagate as dispatch error");
    // Auto-rollback must have fired
    assert_eq!(
        count_rows_direct(&fix.db_path),
        0,
        "auto-rollback must fire on handler throw"
    );
    let _ = std::fs::remove_file(&fix.db_path);
}

// ── TXN-F.3 — tx.query with mismatched datasource throws (CD-1) ─────────────
#[tokio::test]
async fn tx_query_cross_datasource_rejected() {
    let db_a = std::env::temp_dir().join("rivers_v8_tx_cross_a.db");
    let db_b = std::env::temp_dir().join("rivers_v8_tx_cross_b.db");
    let _ = std::fs::remove_file(&db_a);
    let _ = std::fs::remove_file(&db_b);

    for db in [&db_a, &db_b] {
        let conn = rusqlite::Connection::open(db).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, val TEXT);",
        )
        .unwrap();
    }

    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(
        rivers_runtime::rivers_core::drivers::SqliteDriver::new(),
    ));
    let factory = Arc::new(factory);

    let make_opts = || {
        let mut o = HashMap::new();
        o.insert("driver".to_string(), "sqlite".to_string());
        o
    };

    let mut params_map: HashMap<String, ConnectionParams> = HashMap::new();
    params_map.insert(
        "txdb_a".to_string(),
        ConnectionParams {
            host: String::new(),
            port: 0,
            database: db_a.to_string_lossy().into(),
            username: String::new(),
            password: String::new(),
            options: make_opts(),
        },
    );
    params_map.insert(
        "txdb_b".to_string(),
        ConnectionParams {
            host: String::new(),
            port: 0,
            database: db_b.to_string_lossy().into(),
            username: String::new(),
            password: String::new(),
            options: make_opts(),
        },
    );

    let mut registry = DataViewRegistry::new();
    // insert_item_a belongs to txdb_a
    registry.register(DataViewConfig {
        name: "insert_item_a".into(),
        datasource: "txdb_a".into(),
        query: Some("INSERT INTO items (val) VALUES ($val)".into()),
        parameters: vec![DataViewParameterConfig {
            name: "val".into(),
            param_type: "string".into(),
            required: true,
            default: None,
            location: None,
        }],
        ..minimal_dv()
    });
    // insert_item_b belongs to txdb_b
    registry.register(DataViewConfig {
        name: "insert_item_b".into(),
        datasource: "txdb_b".into(),
        query: Some("INSERT INTO items (val) VALUES ($val)".into()),
        parameters: vec![DataViewParameterConfig {
            name: "val".into(),
            param_type: "string".into(),
            required: true,
            default: None,
            location: None,
        }],
        ..minimal_dv()
    });

    let executor = Arc::new(DataViewExecutor::new(
        registry,
        factory.clone(),
        Arc::new(params_map),
        Arc::new(NoopDataViewCache),
    ));

    // Open tx on txdb_a, try to query a DataView that belongs to txdb_b.
    let path = js_file(
        "cross_ds",
        r#"function handler(ctx) {
            const tx = Rivers.db.tx.begin("txdb_a");
            try {
                tx.query("insert_item_b", { val: "wrong_ds" });
                tx.rollback();
                return { threw: false };
            } catch (e) {
                try { tx.rollback(); } catch (_) {}
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
        .trace_id("cross-ds".into())
        .app_id("test-app".into())
        .node_id("node-tx".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .driver_factory(factory)
        .dataview_executor(executor)
        .datasource_config(
            "txdb_a".into(),
            ResolvedDatasource {
                driver_name: "sqlite".to_string(),
                params: ConnectionParams {
                    host: String::new(),
                    port: 0,
                    database: db_a.to_string_lossy().into(),
                    username: String::new(),
                    password: String::new(),
                    options: make_opts(),
                },
            },
        )
        .datasource_config(
            "txdb_b".into(),
            ResolvedDatasource {
                driver_name: "sqlite".to_string(),
                params: ConnectionParams {
                    host: String::new(),
                    port: 0,
                    database: db_b.to_string_lossy().into(),
                    username: String::new(),
                    password: String::new(),
                    options: make_opts(),
                },
            },
        )
        .build()
        .unwrap();

    let result = manager().dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&db_a);
    let _ = std::fs::remove_file(&db_b);

    assert_eq!(
        result.value["threw"], true,
        "tx.query on a DataView from a different datasource must throw (CD-1)"
    );
    let msg = result.value["msg"].as_str().unwrap_or("");
    assert!(
        msg.contains("datasource") || msg.contains("txdb"),
        "error must mention datasource: got {msg}"
    );
}
