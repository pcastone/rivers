//! Tests for `ctx.dataview`, `ctx.store` (in-memory + StorageEngine),
//! `ctx.datasource().build()`, DataViewExecutor.

use super::*;
use super::helpers::{make_js_task, make_js_task_with_storage};

// ── P3: Host Function Binding Tests ──────────────────────────

#[tokio::test]
async fn execute_ctx_dataview_returns_prefetched() {
    // ctx.dataview() returns data from ctx.data when pre-fetched
    let ctx = make_js_task(
        r#"function handler(ctx) {
            ctx.data.orders = [{ id: 1, name: "test" }];
            var result = ctx.dataview("orders");
            return { got: result };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["got"][0]["id"], 1);
    assert_eq!(result.value["got"][0]["name"], "test");
}

#[tokio::test]
async fn execute_ctx_dataview_missing_throws() {
    // ctx.dataview() throws when data not pre-fetched (error sanitization)
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var threw = false;
            try {
                ctx.dataview("nonexistent");
            } catch(e) {
                threw = true;
            }
            return { threw: threw };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
}

// ── V2.3: ctx.streamDataview -- mock iterator protocol ─────────

#[tokio::test]
async fn execute_stream_dataview_array() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            ctx.data.items = [1, 2, 3];
            var stream = ctx.streamDataview("items");
            var result = [];
            var chunk;
            while (!(chunk = stream.next()).done) {
                result.push(chunk.value);
            }
            return { items: result };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["items"], serde_json::json!([1, 2, 3]));
}

#[tokio::test]
async fn execute_stream_dataview_single_value() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            ctx.data.record = { name: "alice" };
            var stream = ctx.streamDataview("record");
            var chunk = stream.next();
            return { value: chunk.value, done: chunk.done };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["value"]["name"], "alice");
    assert_eq!(result.value["done"], false);
}

#[tokio::test]
async fn execute_stream_dataview_missing_returns_done() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var stream = ctx.streamDataview("nonexistent");
            var chunk = stream.next();
            return { done: chunk.done };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["done"], true);
}

// ── V2.4.4: ctx.store -- native V8 callbacks ─────────────────

#[tokio::test]
async fn execute_store_native_reserved_prefix_rejected() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            try { ctx.store.set("session:abc", "val"); return { error: false }; }
            catch(e) { return { error: true, msg: e.message }; }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["error"], true);
    assert!(result.value["msg"].as_str().unwrap().contains("reserved"));
}

#[tokio::test]
async fn execute_store_native_crud() {
    let ctx = make_js_task_with_storage(
        r#"function handler(ctx) {
            ctx.store.set("user:1", { name: "alice", age: 30 });
            var val = ctx.store.get("user:1");
            ctx.store.del("user:1");
            var after = ctx.store.get("user:1");
            return { val: val, after: after };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["val"]["name"], "alice");
    assert_eq!(result.value["val"]["age"], 30);
    assert!(result.value["after"].is_null());
}

#[tokio::test]
async fn execute_ctx_store_get_set_del() {
    // In-memory per-task store: set, get, del all work within a single handler.
    // B2 (P1-5): a StorageEngine must be configured — the silent in-memory
    // fallback was removed.
    let ctx = make_js_task_with_storage(
        r#"function handler(ctx) {
            ctx.store.set("mykey", { count: 42 });
            var val = ctx.store.get("mykey");
            ctx.store.del("mykey");
            var after = ctx.store.get("mykey");
            return { val: val, after: after };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["val"]["count"], 42);
    assert!(result.value["after"].is_null());
}

#[tokio::test]
async fn execute_ctx_store_get_missing_returns_null() {
    let ctx = make_js_task_with_storage(
        r#"function handler(ctx) {
            var val = ctx.store.get("nonexistent");
            return { val: val };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert!(result.value["val"].is_null());
}

#[tokio::test]
async fn execute_ctx_store_overwrite() {
    let ctx = make_js_task_with_storage(
        r#"function handler(ctx) {
            ctx.store.set("k", "first");
            ctx.store.set("k", "second");
            return { val: ctx.store.get("k") };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["val"], "second");
}

#[tokio::test]
async fn execute_ctx_datasource_builder_chain() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var builder = ctx.datasource("primary_db");
            builder = builder.fromQuery("SELECT 1");
            builder = builder.withPostSchema({});
            return { datasource: builder._datasource, query: builder._query };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["datasource"], "primary_db");
    assert_eq!(result.value["query"], "SELECT 1");
}

#[tokio::test]
async fn execute_ctx_datasource_build_throws() {
    // X7: .build() on an undeclared datasource should throw CapabilityError
    let ctx = make_js_task(
        r#"function handler(ctx) {
            try {
                ctx.datasource("db").fromQuery("SELECT 1").build();
            } catch(e) { return { error: e.message }; }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert!(result.value["error"].as_str().unwrap().contains("CapabilityError"));
}

// ── X3: ctx.store StorageEngine Tests ───────────────────────

#[tokio::test]
async fn x3_store_with_storage_engine_round_trip() {
    let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .storage(engine.clone())
        .app_id("test-app".into())
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                ctx.store.set("mykey", { count: 42 });
                var val = ctx.store.get("mykey");
                return { stored: val };
            }"#
        }))
        .trace_id("x3-test".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["stored"]["count"], 42);
}

#[tokio::test]
async fn x3_store_with_storage_engine_del() {
    let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .storage(engine.clone())
        .app_id("test-app".into())
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                ctx.store.set("temp", "hello");
                ctx.store.del("temp");
                var val = ctx.store.get("temp");
                return { deleted: val === null };
            }"#
        }))
        .trace_id("x3-del".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["deleted"], true);
}

#[tokio::test]
async fn x3_store_with_ttl() {
    let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .storage(engine.clone())
        .app_id("test-app".into())
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                ctx.store.set("ttl_key", "value", 60000);
                var val = ctx.store.get("ttl_key");
                return { has_value: val !== null };
            }"#
        }))
        .trace_id("x3-ttl".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["has_value"], true);
}

#[tokio::test]
async fn x3_store_reserved_prefix_with_engine() {
    let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .storage(engine.clone())
        .app_id("test-app".into())
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                try {
                    ctx.store.set("session:evil", "hack");
                    return { blocked: false };
                } catch(e) {
                    return { blocked: true, msg: e.message };
                }
            }"#
        }))
        .trace_id("x3-reserved".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["blocked"], true);
}

#[tokio::test]
async fn x3_store_persists_across_engine() {
    // Verify the StorageEngine actually received the data
    let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .storage(engine.clone())
        .app_id("myapp".into())
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                ctx.store.set("persistent", { data: "hello" });
                return { ok: true };
            }"#
        }))
        .trace_id("x3-persist".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["ok"], true);

    // Read directly from engine to confirm persistence
    let stored = engine.get("app:myapp", "persistent").await.unwrap();
    assert!(stored.is_some(), "StorageEngine should have the value");
    let bytes = stored.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["data"], "hello");
}

#[tokio::test]
async fn b2_store_throws_without_engine() {
    // B2 (P1-5): without a configured StorageEngine, ctx.store.set must throw
    // a JS exception instead of silently buffering into the in-memory
    // TASK_STORE map. (The previous test x3_store_fallback_without_engine
    // asserted the silent fallback that B2 explicitly removes.)
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var threw = false;
            var msg = null;
            try { ctx.store.set("fallback", { n: 99 }); }
            catch(e) { threw = true; msg = String(e.message || e); }
            return { threw: threw, msg: msg };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true,
        "B2: ctx.store.set must throw when no StorageEngine is configured");
    let msg = result.value["msg"].as_str().unwrap_or("");
    assert!(
        msg.contains("no StorageEngine configured"),
        "B2: error message should explain root cause, got: {msg}"
    );
}

// ── X4: ctx.dataview Pre-fetch Tests ────────────────────────

#[tokio::test]
async fn x4_dataview_prefetch_returns_data() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                // Pre-populate ctx.data in the handler
                ctx.data.contacts = [{ name: "Alice" }, { name: "Bob" }];
                return ctx.dataview("contacts");
            }"#
        }))
        .trace_id("x4-prefetch".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value[0]["name"], "Alice");
    assert_eq!(result.value[1]["name"], "Bob");
}

#[tokio::test]
async fn x4_dataview_missing_throws() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var threw = false;
            try {
                ctx.dataview("nonexistent");
            } catch(e) {
                threw = true;
            }
            return { threw: threw };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
}

// ── X7: ctx.datasource().build() Tests ──────────────────────

#[tokio::test]
async fn x7_datasource_build_undeclared_throws() {
    // Attempting to build with a datasource not in TaskContext should throw CapabilityError
    let ctx = make_js_task(
        r#"function handler(ctx) {
            try {
                ctx.datasource("nonexistent").fromQuery("SELECT 1").build();
                return { error: false };
            } catch(e) {
                return { error: true, msg: e.message };
            }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["error"], true);
    assert!(
        result.value["msg"].as_str().unwrap().contains("CapabilityError"),
        "expected CapabilityError, got: {}",
        result.value["msg"]
    );
}

#[tokio::test]
async fn x7_datasource_build_without_query_throws() {
    // .build() without .fromQuery() should throw
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .datasource("mydb".into(), DatasourceToken::pooled("mydb"))
        .datasource_config("mydb".into(), ResolvedDatasource {
            driver_name: "faker".into(),
            params: rivers_runtime::rivers_driver_sdk::ConnectionParams {
                host: String::new(),
                port: 0,
                database: String::new(),
                username: String::new(),
                password: String::new(),
                options: HashMap::new(),
            },
        })
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                try {
                    ctx.datasource("mydb").build();
                    return { error: false };
                } catch(e) {
                    return { error: true, msg: e.message };
                }
            }"#
        }))
        .trace_id("x7-no-query".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["error"], true);
    assert!(
        result.value["msg"].as_str().unwrap().contains("fromQuery"),
        "expected fromQuery hint, got: {}",
        result.value["msg"]
    );
}

#[tokio::test]
async fn x7_datasource_build_no_factory_throws() {
    // Declared datasource but no DriverFactory -> should throw
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .datasource("mydb".into(), DatasourceToken::pooled("mydb"))
        .datasource_config("mydb".into(), ResolvedDatasource {
            driver_name: "faker".into(),
            params: rivers_runtime::rivers_driver_sdk::ConnectionParams {
                host: String::new(),
                port: 0,
                database: String::new(),
                username: String::new(),
                password: String::new(),
                options: HashMap::new(),
            },
        })
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                try {
                    ctx.datasource("mydb").fromQuery("SELECT 1").build();
                    return { error: false };
                } catch(e) {
                    return { error: true, msg: e.message };
                }
            }"#
        }))
        .trace_id("x7-no-factory".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["error"], true);
    assert!(
        result.value["msg"].as_str().unwrap().contains("DriverFactory"),
        "expected DriverFactory error, got: {}",
        result.value["msg"]
    );
}

#[tokio::test]
async fn x7_datasource_builder_chain_preserves_state() {
    // Builder chain should preserve state across calls
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var builder = ctx.datasource("test_ds");
            builder = builder.fromQuery("SELECT * FROM users", { limit: 10 });
            builder = builder.withGetSchema({ type: "object" });
            return {
                ds: builder._datasource,
                query: builder._query,
                has_params: builder._params !== null,
                has_schema: builder._getSchema !== undefined,
            };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["ds"], "test_ds");
    assert_eq!(result.value["query"], "SELECT * FROM users");
    assert_eq!(result.value["has_params"], true);
    assert_eq!(result.value["has_schema"], true);
}

#[tokio::test]
async fn x7_datasource_build_with_faker_driver() {
    // Full end-to-end: wire a real faker driver through DriverFactory
    use rivers_runtime::rivers_core::DriverFactory;

    let mut factory = DriverFactory::new();
    // Register the faker driver
    let faker = Arc::new(rivers_runtime::rivers_core::drivers::FakerDriver::new());
    factory.register_database_driver(faker);

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .datasource("faker-ds".into(), DatasourceToken::pooled("faker-ds"))
        .datasource_config("faker-ds".into(), ResolvedDatasource {
            driver_name: "faker".into(),
            params: rivers_runtime::rivers_driver_sdk::ConnectionParams {
                host: String::new(),
                port: 0,
                database: String::new(),
                username: String::new(),
                password: String::new(),
                options: HashMap::new(),
            },
        })
        .driver_factory(Arc::new(factory))
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                var result = ctx.datasource("faker-ds").fromQuery("SELECT name, email FROM contacts LIMIT 3").build();
                return { has_rows: result.rows !== undefined, row_count: result.rows.length };
            }"#
        }))
        .trace_id("x7-faker".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["has_rows"], true);
    // Faker driver returns synthetic data
    assert!(result.value["row_count"].as_u64().unwrap() > 0);
}

// ── X4: ctx.dataview with DataViewExecutor Tests ────────────

#[tokio::test]
async fn x4_dataview_executor_end_to_end() {
    use rivers_runtime::rivers_core::DriverFactory;
    use rivers_runtime::dataview_engine::DataViewRegistry;
    use rivers_runtime::DataViewExecutor;
    use rivers_runtime::dataview::DataViewConfig;

    // Set up a faker driver in the factory
    let mut factory = DriverFactory::new();
    let faker = Arc::new(rivers_runtime::rivers_core::drivers::FakerDriver::new());
    factory.register_database_driver(faker);

    // Set up a DataView config pointing to the faker datasource
    let mut registry = DataViewRegistry::new();
    let dv_config = DataViewConfig {
        name: "contacts".into(),
        datasource: "faker-ds".into(),
        query: Some("SELECT name, email FROM contacts".into()),
        parameters: vec![],
        return_schema: None,
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
        caching: None,
        circuit_breaker_id: None,
        prepared: false,
        query_params: HashMap::new(),
        invalidates: Vec::new(),
        validate_result: false,
        strict_parameters: false,
        max_rows: 1000,
        skip_introspect: false,
        cursor_key: None,
        source_views: vec![],
        compose_strategy: None,
        join_key: None,
        enrich_mode: "nest".to_string(),
            transaction: false,
    };
    registry.register(dv_config);

    // Set up datasource params with driver hint
    let mut ds_params = HashMap::new();
    let mut params = rivers_runtime::rivers_driver_sdk::ConnectionParams {
        host: String::new(),
        port: 0,
        database: String::new(),
        username: String::new(),
        password: String::new(),
        options: HashMap::new(),
    };
    params.options.insert("driver".into(), "faker".into());
    ds_params.insert("faker-ds".into(), params);

    let executor = Arc::new(DataViewExecutor::new(
        registry,
        Arc::new(factory),
        Arc::new(ds_params),
        Arc::new(NoopDataViewCache),
    ));

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .dataview("contacts".into(), DataViewToken("contacts".into()))
        .dataview_executor(executor)
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                var result = ctx.dataview("contacts");
                return { has_rows: result.rows !== undefined, row_count: result.rows.length };
            }"#
        }))
        .trace_id("x4-executor".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["has_rows"], true);
    assert!(result.value["row_count"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn x4_dataview_executor_not_found_throws() {
    use rivers_runtime::rivers_core::DriverFactory;
    use rivers_runtime::dataview_engine::DataViewRegistry;
    use rivers_runtime::DataViewExecutor;

    let factory = DriverFactory::new();
    let registry = DataViewRegistry::new();
    let executor = Arc::new(DataViewExecutor::new(
        registry,
        Arc::new(factory),
        Arc::new(HashMap::new()),
        Arc::new(NoopDataViewCache),
    ));

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .dataview_executor(executor)
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                try {
                    ctx.dataview("nonexistent");
                    return { error: false };
                } catch(e) {
                    return { error: true, msg: e.message };
                }
            }"#
        }))
        .trace_id("x4-notfound".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["error"], true);
    assert!(
        result.value["msg"].as_str().unwrap().contains("not found"),
        "expected 'not found' error, got: {}",
        result.value["msg"]
    );
}

#[tokio::test]
async fn x4_dataview_prefetch_takes_priority_over_executor() {
    use rivers_runtime::rivers_core::DriverFactory;
    use rivers_runtime::dataview_engine::DataViewRegistry;
    use rivers_runtime::DataViewExecutor;

    // Even with an executor, pre-fetched data should win
    let factory = DriverFactory::new();
    let registry = DataViewRegistry::new();
    let executor = Arc::new(DataViewExecutor::new(
        registry,
        Arc::new(factory),
        Arc::new(HashMap::new()),
        Arc::new(NoopDataViewCache),
    ));

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .dataview_executor(executor)
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                ctx.data.cached_view = [{ id: 1, name: "pre-fetched" }];
                var result = ctx.dataview("cached_view");
                return { from_prefetch: result[0].name === "pre-fetched" };
            }"#
        }))
        .trace_id("x4-priority".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["from_prefetch"], true);
}

// ── AU8: ctx.store persistence within single handler (complex values) ──

#[tokio::test]
async fn au8_store_complex_objects() {
    let ctx = make_js_task_with_storage(
        r#"function handler(ctx) {
            ctx.store.set("config", { nested: { deep: [1, 2, 3] }, flag: true });
            var loaded = ctx.store.get("config");
            return { deep_val: loaded.nested.deep[2], flag: loaded.flag };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["deep_val"], 3);
    assert_eq!(result.value["flag"], true);
}

// ── AU13: ctx.dataview() with real DataViewExecutor + faker driver ──

#[tokio::test]
async fn au13_ctx_dataview_dynamic_with_executor() {
    // Build a real DataViewExecutor with faker driver
    let mut factory = DriverFactory::new();
    let faker = Arc::new(rivers_runtime::rivers_core::drivers::FakerDriver::new());
    factory.register_database_driver(faker);

    let mut registry = rivers_runtime::DataViewRegistry::new();
    registry.register(rivers_runtime::DataViewConfig {
        name: "dynamic_contacts".into(),
        datasource: "faker-ds".into(),
        query: Some("schemas/contact.schema.json".into()),
        parameters: vec![],
        return_schema: None,
        invalidates: Vec::new(),
        validate_result: false,
        strict_parameters: false,
        caching: None,
        circuit_breaker_id: None,
        prepared: false,
        get_query: None, post_query: None, put_query: None, delete_query: None,
        get_schema: None, post_schema: None, put_schema: None, delete_schema: None,
        get_parameters: Vec::new(), post_parameters: Vec::new(),
        put_parameters: Vec::new(), delete_parameters: Vec::new(),
        streaming: false,
        query_params: HashMap::new(),
        max_rows: 1000,
        skip_introspect: false,
        cursor_key: None,
        source_views: vec![],
        compose_strategy: None,
        join_key: None,
        enrich_mode: "nest".to_string(),
            transaction: false,
    });

    let mut ds_params = HashMap::new();
    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "faker".to_string());
    ds_params.insert("faker-ds".to_string(), rivers_runtime::rivers_driver_sdk::ConnectionParams {
        host: String::new(), port: 0, database: String::new(),
        username: String::new(), password: String::new(), options: opts,
    });

    let executor = Arc::new(DataViewExecutor::new(
        registry,
        Arc::new(factory),
        Arc::new(ds_params),
        Arc::new(NoopDataViewCache),
    ));

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .dataview_executor(executor)
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                var data = ctx.dataview("dynamic_contacts");
                return { has_data: data !== null, type: typeof data };
            }"#
        }))
        .trace_id("au13".into()).app_id("test-app".into())
        .build()
        .unwrap();

    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["has_data"], true);
    // Faker returns an array or object
    assert!(
        result.value["type"] == "object",
        "expected object from faker, got type: {}",
        result.value["type"]
    );
}
