//! Integration tests for `view_type = "OTLP"` HTTP-layer dispatch.
//!
//! Exercises the OTLP view's pre-handler stages end-to-end through the Axum
//! router: body extraction, size enforcement, gzip/deflate decompression,
//! Content-Type negotiation (including protobuf via the P1.6 transcoder),
//! path-tail signal routing, and error response shaping. Stops short of
//! actually executing a JS handler — process pool dispatch is reached but
//! fails because no engine cdylib is loaded in unit tests; that surfaces as
//! the framework-level 500 / "handler dispatch failed" branch which is also
//! covered as a positive signal that the dispatch path was reached.
//!
//! The pure-function pieces (decompress_body, signal_from_path,
//! shape_response_body, decode_body) are unit-tested in
//! `crates/riversd/src/server/otlp_view.rs` — this file covers the
//! HTTP-layer integration that requires the Axum router + a populated
//! AppContext.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use rivers_runtime::rivers_core::ServerConfig;
use rivers_runtime::view::ApiViewConfig;
use riversd::server::{build_main_router, AppContext};
use riversd::shutdown::ShutdownCoordinator;
use riversd::view_engine::ViewRouter;

// ── Helpers ──────────────────────────────────────────────────────

fn otlp_ctx() -> AppContext {
    let mut config = ServerConfig::default();
    config.base.admin_api.no_auth = Some(true);
    AppContext::new(config, Arc::new(ShutdownCoordinator::new()))
}

fn otlp_view_multi_handler() -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "OTLP",
        "path": "/otel",
        "handlers": {
            "metrics": { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestMetrics", "resources": [] },
            "logs":    { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestLogs",    "resources": [] },
            "traces":  { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestTraces",  "resources": [] }
        }
    }))
    .expect("valid OTLP view config")
}

fn otlp_view_metrics_only() -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "OTLP",
        "path": "/otel",
        "handlers": {
            "metrics": { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestMetrics", "resources": [] }
        }
    }))
    .expect("valid OTLP view config")
}

fn otlp_view_with_max_body(mb: u32) -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "OTLP",
        "path": "/otel",
        "max_body_mb": mb,
        "handlers": {
            "metrics": { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestMetrics", "resources": [] }
        }
    }))
    .expect("valid OTLP view config")
}

async fn wire_view(ctx: &AppContext, name: &str, view: ApiViewConfig) {
    let mut views = HashMap::new();
    views.insert(name.to_string(), view);
    let router_obj = ViewRouter::from_views(&views);
    let mut vr = ctx.view_router.write().await;
    *vr = Some(router_obj);
}

fn otlp_post(path: &str, content_type: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .uri(path)
        .method("POST")
        .header("content-type", content_type)
        .body(Body::from(body))
        .unwrap()
}

fn otlp_post_with_encoding(
    path: &str,
    content_type: &str,
    encoding: &str,
    body: Vec<u8>,
) -> Request<Body> {
    Request::builder()
        .uri(path)
        .method("POST")
        .header("content-type", content_type)
        .header("content-encoding", encoding)
        .body(Body::from(body))
        .unwrap()
}

async fn body_to_json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::Value::Null)
}

// ── Tests ────────────────────────────────────────────────────────

/// Path with an unrecognised OTLP signal suffix (`/otel/v1/wat`) does not
/// match any of the three POST routes the OTLP view registers (one per
/// canonical signal), so the request falls through to the router-level
/// catchall 404. This proves the per-signal mount surface is exactly what
/// the spec describes — no wildcard catchall that the handler would later
/// have to reject. The catchall body shape is the framework default
/// (`{"code":404,"message":"not found"}`).
#[tokio::test]
async fn unknown_signal_path_returns_router_level_404() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "otel_ingest", otlp_view_multi_handler()).await;
    let router = build_main_router(ctx);
    let req = otlp_post("/otel/v1/wat", "application/json", b"{}".to_vec());
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A view that declares only `handlers.metrics` returns 404 for /v1/logs.
#[tokio::test]
async fn metrics_only_view_returns_404_for_logs() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "metrics_only", otlp_view_metrics_only()).await;
    let router = build_main_router(ctx);
    let req = otlp_post("/otel/v1/logs", "application/json", b"{}".to_vec());
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_to_json(resp).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("not configured"),
        "got body: {}",
        body
    );
}

/// Content-Encoding the framework doesn't support (br, zstd) → 415.
#[tokio::test]
async fn unsupported_content_encoding_returns_415() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "otel_ingest", otlp_view_multi_handler()).await;
    let router = build_main_router(ctx);
    let req = otlp_post_with_encoding(
        "/otel/v1/metrics",
        "application/json",
        "br",
        b"{}".to_vec(),
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body = body_to_json(resp).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("not supported"),
        "got body: {}",
        body
    );
}

/// Content-Type the framework doesn't support → 415.
#[tokio::test]
async fn unsupported_content_type_returns_415() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "otel_ingest", otlp_view_multi_handler()).await;
    let router = build_main_router(ctx);
    let req = otlp_post("/otel/v1/metrics", "text/plain", b"hello".to_vec());
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

/// Malformed JSON body → 400 (not 415 — body label was honoured).
#[tokio::test]
async fn malformed_json_body_returns_400() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "otel_ingest", otlp_view_multi_handler()).await;
    let router = build_main_router(ctx);
    let req = otlp_post(
        "/otel/v1/metrics",
        "application/json",
        b"{not-json".to_vec(),
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Garbage protobuf body → 415 with the spec-mandated decode-failure
/// message shape (matches what CB's probe surfaced).
#[tokio::test]
async fn malformed_protobuf_body_returns_415() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "otel_ingest", otlp_view_multi_handler()).await;
    let router = build_main_router(ctx);
    let req = otlp_post(
        "/otel/v1/metrics",
        "application/x-protobuf",
        b"\x06\x06\x06garbage".to_vec(),
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body = body_to_json(resp).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("protobuf decode failed"),
        "got body: {}",
        body
    );
}

/// Body larger than `max_body_mb` → 413.
#[tokio::test]
async fn oversized_body_returns_413() {
    let ctx = otlp_ctx();
    // max_body_mb = 1 — keeps the test cheap. Body is 2MB.
    wire_view(&ctx, "otel_ingest", otlp_view_with_max_body(1)).await;
    let router = build_main_router(ctx);
    let big_body = b"a".repeat(2 * 1024 * 1024);
    let req = otlp_post("/otel/v1/metrics", "application/json", big_body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// Gzipped body decodes and reaches the handler dispatch. No engine is
/// loaded in tests, so the dispatch itself fails — but the framework should
/// surface this as a 500 with the "handler dispatch failed" error string,
/// which is the positive signal that gzip → JSON parsing succeeded and the
/// downstream code path was reached.
#[tokio::test]
async fn gzipped_json_reaches_handler_dispatch() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "otel_ingest", otlp_view_multi_handler()).await;
    let router = build_main_router(ctx);

    let payload = br#"{"resourceMetrics":[]}"#;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(payload).unwrap();
    let gz = enc.finish().unwrap();

    let req = otlp_post_with_encoding(
        "/otel/v1/metrics",
        "application/json",
        "gzip",
        gz,
    );
    let resp = router.oneshot(req).await.unwrap();
    // 500 = framework reached process_pool dispatch but no engine cdylib is
    // loaded in this test harness, so dispatch fails. This proves the
    // decompression + content-type + path-routing stages all succeeded.
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_to_json(resp).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("dispatch failed"),
        "got body: {}",
        body
    );
}

/// Same as gzipped path but with deflate, to exercise the second decoder.
#[tokio::test]
async fn deflate_encoded_json_decodes_and_dispatches() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "otel_ingest", otlp_view_multi_handler()).await;
    let router = build_main_router(ctx);

    let payload = br#"{"resourceLogs":[]}"#;
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(payload).unwrap();
    let df = enc.finish().unwrap();

    let req = otlp_post_with_encoding("/otel/v1/logs", "application/json", "deflate", df);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Identity-encoded JSON also reaches the dispatch stage.
#[tokio::test]
async fn identity_json_reaches_handler_dispatch_per_signal() {
    let ctx = otlp_ctx();
    wire_view(&ctx, "otel_ingest", otlp_view_multi_handler()).await;
    let router = build_main_router(ctx);

    for path in &["/otel/v1/metrics", "/otel/v1/logs", "/otel/v1/traces"] {
        let req = otlp_post(path, "application/json", br#"{"x":1}"#.to_vec());
        let resp = router.clone().oneshot(req).await.unwrap();
        // 500 because no engine; positive signal that we reached dispatch.
        assert_eq!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected dispatch reach for {}",
            path
        );
    }
}
