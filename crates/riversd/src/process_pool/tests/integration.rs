//! JS/WASM gap tests (AU tests), async/promises, complex data types, file loading.

use super::*;
use super::helpers::make_js_task;

// ── T4: ES Module + Async Support Tests ──────────────────────

#[tokio::test]
async fn execute_async_function() {
    let ctx = make_js_task(
        r#"async function handler(ctx) { return { async: true }; }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["async"], true);
}

#[test]
fn execute_module_syntax_detected() {
    assert!(is_module_syntax("export function handler(ctx) {}"));
    assert!(is_module_syntax("import { foo } from 'bar'"));
    assert!(!is_module_syntax("function handler(ctx) {}"));
    assert!(is_module_syntax("export default function handler(ctx) {}"));
}

#[tokio::test]
async fn execute_async_with_promise_chain() {
    let ctx = make_js_task(
        r#"async function handler(ctx) {
            var result = await Promise.resolve({ resolved: true });
            return result;
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["resolved"], true);
}

#[tokio::test]
async fn execute_async_rejection() {
    let ctx = make_js_task(
        r#"async function handler(ctx) {
            throw new Error("async boom");
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("async") || err_msg.contains("boom"),
        "expected async error details in: {err_msg}"
    );
}

// ── AU1: JS file loading from disk ──

#[tokio::test]
async fn au1_execute_js_file_from_disk() {
    let dir = std::env::temp_dir();
    let js_path = dir.join("au1_handler.js");
    std::fs::write(&js_path, r#"function onRequest(ctx) { return { from_file: true, got: __args.name }; }"#).unwrap();

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "onRequest".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({"name": "disk-test"}))
        .trace_id("au1".into()).app_id("test-app".into())
        .build()
        .unwrap();

    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["from_file"], true);
    assert_eq!(result.value["got"], "disk-test");

    let _ = std::fs::remove_file(&js_path);
}

// AU2: Multiple JS functions in same source -- call correct entrypoint
#[tokio::test]
async fn au2_execute_correct_entrypoint_from_multi_function_source() {
    let ctx = make_js_task(
        r#"
        function alpha(ctx) { return { fn: "alpha" }; }
        function beta(ctx) { return { fn: "beta" }; }
        function gamma(ctx) { return { fn: "gamma" }; }
        "#,
        "beta",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["fn"], "beta");
}

// AU3: Promise.all with multiple concurrent resolutions
#[tokio::test]
async fn au3_promise_all_multiple_values() {
    let ctx = make_js_task(
        r#"async function handler(ctx) {
            var results = await Promise.all([
                Promise.resolve(1),
                Promise.resolve(2),
                Promise.resolve(3),
            ]);
            return { sum: results[0] + results[1] + results[2] };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["sum"], 6);
}

// AU4: Promise.race -- first resolver wins
#[tokio::test]
async fn au4_promise_race() {
    let ctx = make_js_task(
        r#"async function handler(ctx) {
            var result = await Promise.race([
                Promise.resolve("first"),
                new Promise(function(r) { r("second"); }),
            ]);
            return { winner: result };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["winner"], "first");
}

// AU5: JS complex data structures -- nested objects, arrays, nulls
#[tokio::test]
async fn au5_complex_data_round_trip() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            return {
                nested: { a: { b: { c: 42 } } },
                array: [1, "two", null, true, [5, 6]],
                empty_obj: {},
                empty_arr: [],
                null_val: null,
            };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["nested"]["a"]["b"]["c"], 42);
    assert_eq!(result.value["array"][0], 1);
    assert_eq!(result.value["array"][1], "two");
    assert!(result.value["array"][2].is_null());
    assert_eq!(result.value["array"][3], true);
    assert_eq!(result.value["array"][4][1], 6);
    assert_eq!(result.value["empty_arr"], serde_json::json!([]));
    assert!(result.value["null_val"].is_null());
}

// AU6: JS error types -- TypeError, RangeError, custom errors
#[tokio::test]
async fn au6_error_types_captured() {
    let ctx = make_js_task(
        "function handler(ctx) { null.property; }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Cannot read") || msg.contains("null") || msg.contains("TypeError"),
        "expected TypeError details in: {msg}"
    );
}

// AU7: JS closures and higher-order functions
#[tokio::test]
async fn au7_closures_and_higher_order_functions() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var items = [3, 1, 4, 1, 5, 9];
            var sorted = items.slice().sort(function(a, b) { return a - b; });
            var doubled = items.map(function(x) { return x * 2; });
            var sum = items.reduce(function(acc, x) { return acc + x; }, 0);
            return { sorted: sorted, doubled: doubled, sum: sum };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["sorted"], serde_json::json!([1, 1, 3, 4, 5, 9]));
    assert_eq!(result.value["sum"], 23);
    assert_eq!(result.value["doubled"][0], 6);
}

// AU11: JS string operations and JSON
#[tokio::test]
async fn au11_string_and_json_operations() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var obj = { name: "alice", age: 30 };
            var json_str = JSON.stringify(obj);
            var parsed = JSON.parse(json_str);
            var upper = "hello".toUpperCase();
            var split = "a,b,c".split(",");
            return { json_str: json_str, name: parsed.name, upper: upper, split: split };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert!(result.value["json_str"].as_str().unwrap().contains("alice"));
    assert_eq!(result.value["name"], "alice");
    assert_eq!(result.value["upper"], "HELLO");
    assert_eq!(result.value["split"], serde_json::json!(["a", "b", "c"]));
}

// AU12: JS Date and Math builtins
#[tokio::test]
async fn au12_date_and_math_builtins() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var now = Date.now();
            var pi = Math.PI;
            var floor = Math.floor(3.7);
            var random = Math.random();
            return {
                now_is_number: typeof now === "number",
                now_positive: now > 0,
                pi: pi,
                floor: floor,
                random_in_range: random >= 0 && random < 1,
            };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["now_is_number"], true);
    assert_eq!(result.value["now_positive"], true);
    assert!((result.value["pi"].as_f64().unwrap() - std::f64::consts::PI).abs() < 0.0001);
    assert_eq!(result.value["floor"], 3);
    assert_eq!(result.value["random_in_range"], true);
}
