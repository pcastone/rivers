//! Tests for `Rivers.http`, `Rivers.env`, `Rivers.log` (basic + structured fields),
//! HTTP capability gating.

use super::*;
use super::helpers::{make_js_task, make_http_js_task};

// ── P2.1: Rivers.log tests ──────────────────────────────────

#[tokio::test]
async fn execute_rivers_log_does_not_crash() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            Rivers.log.info("test message");
            Rivers.log.warn("warning");
            Rivers.log.error("error");
            return { logged: true };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["logged"], true);
}

// ── P2.3: console.log tests ─────────────────────────────────

#[tokio::test]
async fn execute_console_log_works() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            console.log("hello from console");
            console.warn("a warning");
            console.error("an error");
            return { ok: true };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["ok"], true);
}

// ── P4.3: Rivers.http ─────────────────────────────────────────

// ── V2: Rivers.http native callbacks ──────────────────────────

#[tokio::test]
async fn execute_rivers_http_get_invalid_url_throws() {
    // Invalid URL should throw an error (not panic)
    let ctx = make_http_js_task(
        r#"function handler(ctx) {
            try {
                Rivers.http.get("not-a-valid-url");
            } catch(e) {
                return { error: e.message, has_error: true };
            }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["has_error"], true);
}

#[tokio::test]
async fn execute_rivers_http_methods_exist() {
    // All HTTP methods should be callable functions (requires HttpToken for capability gating)
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .http(HttpToken)
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                return {
                    get_type: typeof Rivers.http.get,
                    post_type: typeof Rivers.http.post,
                    put_type: typeof Rivers.http.put,
                    del_type: typeof Rivers.http.del,
                };
            }"#
        }))
        .trace_id("test-http-methods".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["get_type"], "function");
    assert_eq!(result.value["post_type"], "function");
    assert_eq!(result.value["put_type"], "function");
    assert_eq!(result.value["del_type"], "function");
}

#[tokio::test]
async fn execute_rivers_http_connection_refused_throws() {
    // Connecting to a port with no listener should throw (not hang)
    let ctx = make_http_js_task(
        r#"function handler(ctx) {
            try {
                Rivers.http.get("http://127.0.0.1:19999/nonexistent");
            } catch(e) {
                return { error: e.message, caught: true };
            }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["caught"], true);
}

// ── V2: Rivers.env ──────────────────────────────────────────────

#[tokio::test]
async fn execute_rivers_env_available() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({
            "_source": "function handler(ctx) { return { db: Rivers.env.DATABASE_URL, port: Rivers.env.PORT }; }"
        }))
        .env_var("DATABASE_URL".into(), "postgres://localhost/test".into())
        .env_var("PORT".into(), "8080".into())
        .trace_id("t1".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["db"], "postgres://localhost/test");
    assert_eq!(result.value["port"], "8080");
}

#[tokio::test]
async fn execute_rivers_env_empty_by_default() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            return { has_env: typeof Rivers.env === "object", keys: Object.keys(Rivers.env).length };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["has_env"], true);
    assert_eq!(result.value["keys"], 0);
}

// ── X1: Rivers.log Structured Fields Tests ──────────────────

#[tokio::test]
async fn x1_rivers_log_info_with_fields() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            Rivers.log.info("user login", { userId: 123, action: "login" });
            return { logged: true };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["logged"], true);
}

#[tokio::test]
async fn x1_rivers_log_without_fields_still_works() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            Rivers.log.info("simple message");
            Rivers.log.warn("warning message");
            Rivers.log.error("error message");
            return { ok: true };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["ok"], true);
}

#[tokio::test]
async fn x1_console_log_with_object_fields() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            console.log("action performed", { key: "val" });
            return { ok: true };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["ok"], true);
}

// ── X2: Rivers.http Capability Gating Tests ─────────────────

#[tokio::test]
async fn x2_rivers_http_absent_without_http_token() {
    // No .http() on the builder -> Rivers.http should be undefined
    let ctx = make_js_task(
        r#"function handler(ctx) {
            return { http_type: typeof Rivers.http };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["http_type"], "undefined");
}

#[tokio::test]
async fn x2_rivers_http_present_with_http_token() {
    // With .http() on the builder -> Rivers.http should be an object
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .http(HttpToken)
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                return {
                    http_type: typeof Rivers.http,
                    get_type: typeof Rivers.http.get,
                    post_type: typeof Rivers.http.post,
                    put_type: typeof Rivers.http.put,
                    del_type: typeof Rivers.http.del,
                };
            }"#
        }))
        .trace_id("test-http-gating".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["http_type"], "object");
    assert_eq!(result.value["get_type"], "function");
    assert_eq!(result.value["post_type"], "function");
    assert_eq!(result.value["put_type"], "function");
    assert_eq!(result.value["del_type"], "function");
}

#[tokio::test]
async fn x2_rivers_http_access_without_token_is_safe() {
    // Accessing Rivers.http methods without token should not crash
    let ctx = make_js_task(
        r#"function handler(ctx) {
            try {
                Rivers.http.get("http://example.com");
                return { error: false };
            } catch(e) {
                return { error: true, message: e.message };
            }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    // Should get a TypeError because Rivers.http is undefined
    assert_eq!(result.value["error"], true);
}
