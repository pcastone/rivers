//! Integration tests for DatasourceToken::Direct typed-proxy dispatch (29e/29f).
//!
//! Exercise the full chain: ctx.datasource("fs") → proxy method →
//! Rivers.__directDispatch → thread-local lookup → FilesystemConnection.

use super::*;

fn task_with_fs_datasource(root: &std::path::Path, src: &str) -> TaskContext {
    TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .datasource(
            "fs".into(),
            DatasourceToken::direct("filesystem", root.to_path_buf()),
        )
        .args(serde_json::json!({ "_source": src }))
        .trace_id("direct-dispatch".into())
        .app_id("test-app".into())
        .task_kind(TaskKind::Rest)
        .build()
        .unwrap()
}

#[tokio::test]
async fn typed_proxy_readfile_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "world").unwrap();

    let ctx = task_with_fs_datasource(
        dir.path(),
        "function handler(ctx) { return ctx.datasource('fs').readFile('hello.txt'); }",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value, serde_json::json!("world"));
}

#[tokio::test]
async fn typed_proxy_readfile_base64_encoding() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b.bin"), [0xff, 0x00, 0xfe]).unwrap();

    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            return ctx.datasource('fs').readFile('b.bin', 'base64');
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value, serde_json::json!("/wD+"));
}

#[tokio::test]
async fn typed_proxy_write_then_read() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            var fs = ctx.datasource('fs');
            fs.writeFile('out.txt', 'hello there');
            return fs.readFile('out.txt');
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value, serde_json::json!("hello there"));
}

#[tokio::test]
async fn typed_proxy_exists_returns_bool() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("present.txt"), "").unwrap();
    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            var fs = ctx.datasource('fs');
            return { present: fs.exists('present.txt'), absent: fs.exists('nope.txt') };
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value["present"], serde_json::json!(true));
    assert_eq!(result.value["absent"], serde_json::json!(false));
}

#[tokio::test]
async fn typed_proxy_readdir_returns_array() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            var entries = ctx.datasource('fs').readDir('.');
            return entries.map(function(e){ return e.name; }).sort();
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value, serde_json::json!(["a.txt", "b.txt"]));
}

#[tokio::test]
async fn typed_proxy_find_returns_results_and_truncated() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "").unwrap();
    }
    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            return ctx.datasource('fs').find('*.txt');
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert!(result.value["results"].is_array());
    assert_eq!(result.value["truncated"], serde_json::json!(false));
}

// ── 29f — ParamType validation ──────────────────────────────────

#[tokio::test]
async fn typed_proxy_rejects_non_string_path() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            try {
                ctx.datasource('fs').readFile(42);
                return { threw: false };
            } catch (e) {
                return { threw: true, msg: String(e.message || e) };
            }
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value["threw"], serde_json::json!(true));
    let msg = result.value["msg"].as_str().unwrap();
    assert!(msg.contains("must be a string"), "msg: {msg}");
}

#[tokio::test]
async fn typed_proxy_rejects_missing_required_param() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            try {
                ctx.datasource('fs').readFile();
                return { threw: false };
            } catch (e) {
                return { threw: true, msg: String(e.message || e) };
            }
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value["threw"], serde_json::json!(true));
    let msg = result.value["msg"].as_str().unwrap();
    assert!(msg.contains("is required"), "msg: {msg}");
}

#[tokio::test]
async fn typed_proxy_rejects_non_integer_max_results() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            try {
                ctx.datasource('fs').find('*.txt', 'ten');
                return { threw: false };
            } catch (e) {
                return { threw: true, msg: String(e.message || e) };
            }
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value["threw"], serde_json::json!(true));
    let msg = result.value["msg"].as_str().unwrap();
    assert!(msg.contains("must be a integer"), "msg: {msg}");
}

#[tokio::test]
async fn typed_proxy_optional_default_applied() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..2 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "").unwrap();
    }
    // max_results omitted → default 1000; dispatch should succeed.
    let ctx = task_with_fs_datasource(
        dir.path(),
        r#"function handler(ctx) {
            var r = ctx.datasource('fs').find('*.txt');
            return { count: r.results.length, truncated: r.truncated };
        }"#,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None)
        .await
        .unwrap();
    assert_eq!(result.value["count"], serde_json::json!(2));
    assert_eq!(result.value["truncated"], serde_json::json!(false));
}
