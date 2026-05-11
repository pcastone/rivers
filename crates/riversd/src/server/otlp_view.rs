//! OTLP/HTTP view dispatcher (CB-OTLP Track O2).
//!
//! Per `docs/arch/rivers-otlp-view-spec.md`. The framework owns content-type
//! negotiation, gzip/deflate decompression, path-based per-signal dispatch,
//! and OTLP partial-success response shaping; handlers receive a decoded
//! `ctx.otel.{kind, payload, encoding}` envelope and may return
//! `{ rejected, errorMessage }` to drive the spec response.
//!
//! Pure helpers (`decompress_body`, `signal_from_path`, `shape_response_body`)
//! are unit-tested without a running server; the orchestrator
//! `execute_otlp_view` integrates them and dispatches via the existing
//! ProcessPool path.
//!
//! Wire-format reuse: the existing P1.6 protobuf → JSON transcoder
//! (`crate::otlp_transcoder`) handles `application/x-protobuf` inputs.

use std::io::Read;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use flate2::read::{GzDecoder, ZlibDecoder};
use rivers_runtime::process_pool::{Entrypoint, TaskContextBuilder};
use rivers_runtime::view::HandlerConfig;
use serde_json::{json, Value};

use crate::server::context::AppContext;
use crate::server::view_dispatch::MatchedRoute;

// ── Prometheus metrics (CB-OTLP Track O5, spec §11) ──────────────
//
// Metric names follow the existing `rivers_*` prefix used elsewhere
// (`rivers_http_*`, `rivers_cron_*`, etc.). Registration is automatic via
// the `metrics` crate facade when riversd is built with the `metrics`
// feature — same pattern as `cron::cron_metrics`.

mod otlp_metrics {
    use metrics::{counter, histogram};

    /// Strip the per-signal suffix from a router-internal view id (e.g.
    /// `"otel_ingest:metrics"` → `"otel_ingest"`). The router registers one
    /// route per signal underneath each OTLP view; the metric `view` label
    /// should be the operator-facing view name, not the synthesised id.
    pub fn strip_signal_suffix(view_id: &str) -> &str {
        view_id.rsplit_once(':').map(|(v, _)| v).unwrap_or(view_id)
    }

    pub fn record_request(view: &str, signal: &str, encoding: &str, status: u16) {
        counter!(
            "rivers_otlp_requests_total",
            "view" => view.to_string(),
            "signal" => signal.to_string(),
            "encoding" => encoding.to_string(),
            "status" => status.to_string(),
        )
        .increment(1);
    }

    pub fn record_decode_failure(view: &str, signal: &str, reason: &str) {
        counter!(
            "rivers_otlp_decode_failures_total",
            "view" => view.to_string(),
            "signal" => signal.to_string(),
            "reason" => reason.to_string(),
        )
        .increment(1);
    }

    pub fn record_partial_success(view: &str, signal: &str) {
        counter!(
            "rivers_otlp_partial_success_total",
            "view" => view.to_string(),
            "signal" => signal.to_string(),
        )
        .increment(1);
    }

    pub fn record_rejected_points(view: &str, signal: &str, n: u64) {
        counter!(
            "rivers_otlp_rejected_points_total",
            "view" => view.to_string(),
            "signal" => signal.to_string(),
        )
        .increment(n);
    }

    pub fn observe_request_bytes(view: &str, signal: &str, encoding: &str, n: usize) {
        histogram!(
            "rivers_otlp_request_bytes",
            "view" => view.to_string(),
            "signal" => signal.to_string(),
            "encoding" => encoding.to_string(),
        )
        .record(n as f64);
    }

    pub fn observe_decoded_bytes(view: &str, signal: &str, n: usize) {
        histogram!(
            "rivers_otlp_decoded_bytes",
            "view" => view.to_string(),
            "signal" => signal.to_string(),
        )
        .record(n as f64);
    }

    pub fn observe_dispatch_duration_ms(view: &str, signal: &str, ms: f64) {
        histogram!(
            "rivers_otlp_dispatch_duration_ms",
            "view" => view.to_string(),
            "signal" => signal.to_string(),
        )
        .record(ms);
    }
}

/// Default body-size cap (megabytes) when the view does not set `max_body_mb`.
/// Matches the OTLP/HTTP recommendation per `rivers-otlp-view-spec.md` §4.3.
const DEFAULT_MAX_BODY_MB: u32 = 4;

/// Post-decompression body amplification cap as a multiplier of the raw cap.
/// Guards against zip-bomb-style inputs per `rivers-otlp-view-spec.md` §4.2.
const DECOMPRESSED_AMPLIFICATION_FACTOR: u64 = 3 / 2 + 1; // 1.5×, ceiling

/// Errors surfaced from the OTLP wire-format stages. Each variant maps to a
/// concrete HTTP response shape per `rivers-otlp-view-spec.md` §7.3.
#[derive(Debug)]
pub(crate) enum OtlpError {
    /// Body exceeded the size limit (pre- or post-decompression). → 413.
    BodyTooLarge { observed: usize, limit_bytes: u64 },
    /// `Content-Encoding` was not `identity`, `gzip`, or `deflate`. → 415.
    UnsupportedEncoding(String),
    /// `Content-Type` was not `application/json` or `application/x-protobuf`. → 415.
    UnsupportedContentType(String),
    /// gzip/deflate decode failure. → 415.
    DecompressionFailed(String),
    /// JSON parse failure on a JSON body. → 400.
    JsonParseFailed(String),
    /// prost protobuf decode failure on a protobuf body. → 415.
    ProtobufDecodeFailed(String),
    /// Path did not match a known OTLP signal. → 404.
    UnknownSignal(String),
    /// View declares only a subset of `handlers.{metrics,logs,traces}`
    /// and the request hit a signal that wasn't configured. → 404.
    SignalNotConfigured(String),
}

impl OtlpError {
    /// HTTP status code per spec §7.3 mapping.
    fn status(&self) -> StatusCode {
        match self {
            OtlpError::BodyTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            OtlpError::UnsupportedEncoding(_)
            | OtlpError::UnsupportedContentType(_)
            | OtlpError::DecompressionFailed(_)
            | OtlpError::ProtobufDecodeFailed(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            OtlpError::JsonParseFailed(_) => StatusCode::BAD_REQUEST,
            OtlpError::UnknownSignal(_) | OtlpError::SignalNotConfigured(_) => StatusCode::NOT_FOUND,
        }
    }

    /// Human-readable message embedded in the `{"error": "..."}` body.
    fn message(&self) -> String {
        match self {
            OtlpError::BodyTooLarge { observed, limit_bytes } => format!(
                "body exceeds OTLP size limit ({} bytes > {} bytes)",
                observed, limit_bytes
            ),
            OtlpError::UnsupportedEncoding(s) => {
                format!("OTLP Content-Encoding '{}' not supported", s)
            }
            OtlpError::UnsupportedContentType(s) => format!(
                "OTLP requires application/json or application/x-protobuf, got '{}'",
                s
            ),
            OtlpError::DecompressionFailed(s) => format!("OTLP decompression failed: {}", s),
            OtlpError::JsonParseFailed(s) => format!("OTLP JSON parse failed: {}", s),
            OtlpError::ProtobufDecodeFailed(s) => format!("protobuf decode failed: {}", s),
            OtlpError::UnknownSignal(p) => format!(
                "OTLP path '{}' does not match /v1/{{metrics,logs,traces}}",
                p
            ),
            OtlpError::SignalNotConfigured(s) => {
                format!("OTLP signal '{}' not configured on this view", s)
            }
        }
    }

    fn into_response(self) -> Response<Body> {
        let body = json!({ "error": self.message() }).to_string();
        Response::builder()
            .status(self.status())
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap_or_else(|_| Response::new(Body::empty()))
    }
}

/// Extract the OTLP signal name (`metrics` | `logs` | `traces`) from a request
/// path. Returns the signal slug or [`OtlpError::UnknownSignal`].
///
/// Matches the trailing `/v1/<signal>` segment, allowing any prefix the
/// operator declared (e.g. `path = "otel"` → `/otel/v1/metrics`).
pub(crate) fn signal_from_path(path: &str) -> Result<&'static str, OtlpError> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.ends_with("/v1/metrics") {
        Ok("metrics")
    } else if trimmed.ends_with("/v1/logs") {
        Ok("logs")
    } else if trimmed.ends_with("/v1/traces") {
        Ok("traces")
    } else {
        Err(OtlpError::UnknownSignal(path.to_string()))
    }
}

/// Pick the OTLP partial-success field name for the given signal kind.
/// Spec §7.2.
pub(crate) fn rejected_field_for_signal(kind: &str) -> &'static str {
    match kind {
        "logs" => "rejectedLogRecords",
        "traces" => "rejectedSpans",
        _ => "rejectedDataPoints",
    }
}

/// Decompress an inbound body per `Content-Encoding`.
///
/// `encoding` is the header value (case-insensitive). `cap_bytes` is the
/// post-decompression size cap; reads beyond it return
/// [`OtlpError::BodyTooLarge`].
pub(crate) fn decompress_body(
    body: &[u8],
    encoding: Option<&str>,
    cap_bytes: u64,
) -> Result<Vec<u8>, OtlpError> {
    let enc = encoding.map(|s| s.trim().to_ascii_lowercase());
    let mut decoded = Vec::new();
    match enc.as_deref() {
        None | Some("") | Some("identity") => {
            return Ok(body.to_vec());
        }
        Some("gzip") => {
            let limited = (&body[..]).take(cap_bytes.saturating_add(1));
            let mut decoder = GzDecoder::new(limited);
            decoder
                .read_to_end(&mut decoded)
                .map_err(|e| OtlpError::DecompressionFailed(format!("gzip: {}", e)))?;
        }
        Some("deflate") => {
            // RFC 9110 §8.4.1.2: Content-Encoding `deflate` means the
            // zlib data format (RFC 1950) — NOT raw RFC 1951 DEFLATE.
            // flate2::ZlibDecoder reads the 2-byte zlib header + checksum;
            // DeflateDecoder would reject the zlib wrapper as a "corrupt
            // deflate stream". CB-OTLP Track O5.6 caught the divergence.
            let limited = (&body[..]).take(cap_bytes.saturating_add(1));
            let mut decoder = ZlibDecoder::new(limited);
            decoder
                .read_to_end(&mut decoded)
                .map_err(|e| OtlpError::DecompressionFailed(format!("deflate: {}", e)))?;
        }
        Some(other) => return Err(OtlpError::UnsupportedEncoding(other.to_string())),
    }
    if decoded.len() as u64 > cap_bytes {
        return Err(OtlpError::BodyTooLarge {
            observed: decoded.len(),
            limit_bytes: cap_bytes,
        });
    }
    Ok(decoded)
}

/// Decode a body to a JSON `Value` based on `Content-Type`.
///
/// `application/json` → `serde_json::from_slice`.
/// `application/x-protobuf` → reuses the P1.6 transcoder
/// (`crate::otlp_transcoder::transcode_otlp_protobuf`) which decodes prost
/// types and re-encodes as JSON.
pub(crate) fn decode_body(
    body: &[u8],
    content_type: Option<&str>,
    path: &str,
) -> Result<(Value, &'static str), OtlpError> {
    let ct = content_type
        .map(|s| s.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .unwrap_or_default();
    match ct.as_str() {
        "application/json" => {
            let v: Value = serde_json::from_slice(body)
                .map_err(|e| OtlpError::JsonParseFailed(e.to_string()))?;
            Ok((v, "json"))
        }
        "application/x-protobuf" => {
            let json_bytes = crate::otlp_transcoder::transcode_otlp_protobuf(path, body)
                .map_err(|e| match e {
                    crate::otlp_transcoder::TranscodeError::UnknownSignal(p) => {
                        OtlpError::UnknownSignal(p)
                    }
                    crate::otlp_transcoder::TranscodeError::DecodeFailed { reason, .. } => {
                        OtlpError::ProtobufDecodeFailed(reason)
                    }
                })?;
            let v: Value = serde_json::from_slice(&json_bytes)
                .map_err(|e| OtlpError::JsonParseFailed(format!("post-transcode: {}", e)))?;
            Ok((v, "protobuf"))
        }
        other => Err(OtlpError::UnsupportedContentType(other.to_string())),
    }
}

/// Shape the OTLP response body from a handler's return value.
///
/// Per spec §7.1 / §7.2: empty `{}` on success, `{"partialSuccess": {...}}`
/// when the handler reports `rejected > 0`. The framework selects the
/// rejection-field name from the matched signal kind.
pub(crate) fn shape_response_body(handler_value: &Value, kind: &str) -> Value {
    let rejected = handler_value
        .get("rejected")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if rejected == 0 {
        return json!({});
    }
    let mut partial = serde_json::Map::new();
    partial.insert(
        rejected_field_for_signal(kind).to_string(),
        json!(rejected),
    );
    if let Some(msg) = handler_value.get("errorMessage").and_then(|v| v.as_str()) {
        if !msg.is_empty() {
            partial.insert("errorMessage".to_string(), json!(msg));
        }
    }
    json!({ "partialSuccess": Value::Object(partial) })
}

/// Pick the handler for the requested signal. Multi-handler form
/// (`handlers.{kind}`) wins when present; otherwise falls back to the
/// single `handler` block (single-handler form). Returns
/// `SignalNotConfigured` when the multi-form is declared but missing the
/// requested kind. Spec §5.
pub(crate) fn pick_handler<'a>(
    matched: &'a MatchedRoute,
    kind: &str,
) -> Result<&'a HandlerConfig, OtlpError> {
    if let Some(handlers) = &matched.config.handlers {
        if let Some(h) = handlers.get(kind) {
            return Ok(h);
        }
        // Multi-handler form was declared but doesn't cover this signal.
        return Err(OtlpError::SignalNotConfigured(kind.to_string()));
    }
    // Fall through to single-handler form. The structural validator
    // ([X-OTLP-2]) guarantees one form is present.
    Ok(&matched.config.handler)
}

/// Map an `OtlpError` to the `reason` label used by
/// `rivers_otlp_decode_failures_total`. Each reason corresponds to a
/// rejection class enumerated in `rivers-otlp-view-spec.md` §11.
fn decode_failure_reason(err: &OtlpError) -> &'static str {
    match err {
        OtlpError::BodyTooLarge { .. } => "size_pre",
        OtlpError::UnsupportedEncoding(_) => "encoding",
        OtlpError::UnsupportedContentType(_) => "content_type",
        OtlpError::DecompressionFailed(_) => "decompress",
        OtlpError::JsonParseFailed(_) => "json",
        OtlpError::ProtobufDecodeFailed(_) => "protobuf",
        OtlpError::UnknownSignal(_) | OtlpError::SignalNotConfigured(_) => "signal",
    }
}

/// Execute an OTLP/HTTP request against the matched OTLP view.
pub(crate) async fn execute_otlp_view(
    ctx: AppContext,
    request: Request<Body>,
    matched: MatchedRoute,
) -> Response<Body> {
    let path = request.uri().path().to_string();
    let headers: std::collections::HashMap<String, String> = request
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|val| (k.to_string(), val.to_string())))
        .collect();

    let view_name = otlp_metrics::strip_signal_suffix(&matched.view_id).to_string();

    // The signal is determined by the request path — we resolve it early so
    // metric labels are populated even when we reject the request before
    // dispatch. If the path doesn't match a canonical signal we use
    // "unknown" as a placeholder so we still emit a request counter.
    let kind_for_metrics: &str = signal_from_path(&path).unwrap_or("unknown");

    let max_body_mb = matched.config.max_body_mb.unwrap_or(DEFAULT_MAX_BODY_MB);
    let raw_cap: u64 = (max_body_mb as u64) * 1024 * 1024;
    let decompressed_cap: u64 = raw_cap.saturating_mul(DECOMPRESSED_AMPLIFICATION_FACTOR);

    // ── 1. Read body. We give axum a generous absolute ceiling (the spec's
    //      max + 16 MiB headroom) so an oversized request gets *read* and
    //      we can emit a clean 413, instead of axum aborting the connection
    //      before our error response goes out. The real OTLP-level size
    //      check runs immediately after on the returned bytes.
    let axum_ceiling = (raw_cap as usize).saturating_add(16 * 1024 * 1024);
    let body_bytes = match axum::body::to_bytes(request.into_body(), axum_ceiling).await {
        Ok(b) => b,
        Err(_) => {
            // Truly enormous body (over axum's ceiling). Still a 413.
            let err = OtlpError::BodyTooLarge {
                observed: 0,
                limit_bytes: raw_cap,
            };
            otlp_metrics::record_decode_failure(&view_name, kind_for_metrics, decode_failure_reason(&err));
            otlp_metrics::record_request(&view_name, kind_for_metrics, "unknown", err.status().as_u16());
            return err.into_response();
        }
    };
    if body_bytes.len() as u64 > raw_cap {
        let err = OtlpError::BodyTooLarge {
            observed: body_bytes.len(),
            limit_bytes: raw_cap,
        };
        otlp_metrics::record_decode_failure(&view_name, kind_for_metrics, decode_failure_reason(&err));
        otlp_metrics::record_request(&view_name, kind_for_metrics, "unknown", err.status().as_u16());
        return err.into_response();
    }

    // ── 2. Decompress per Content-Encoding ──
    let encoding_header = headers.get("content-encoding").map(|s| s.as_str());
    let decompressed = match decompress_body(&body_bytes, encoding_header, decompressed_cap) {
        Ok(b) => b,
        Err(e) => {
            otlp_metrics::record_decode_failure(&view_name, kind_for_metrics, decode_failure_reason(&e));
            otlp_metrics::record_request(&view_name, kind_for_metrics, "unknown", e.status().as_u16());
            return e.into_response();
        }
    };

    // ── 3. Decode per Content-Type (json passthrough or protobuf transcode) ──
    let content_type_header = headers.get("content-type").map(|s| s.as_str());
    let (payload, encoding_label) = match decode_body(&decompressed, content_type_header, &path) {
        Ok(p) => p,
        Err(e) => {
            otlp_metrics::record_decode_failure(&view_name, kind_for_metrics, decode_failure_reason(&e));
            otlp_metrics::record_request(&view_name, kind_for_metrics, "unknown", e.status().as_u16());
            return e.into_response();
        }
    };

    // ── 4. Signal routing (now strict — path must match a canonical signal) ──
    let kind = match signal_from_path(&path) {
        Ok(k) => k,
        Err(e) => {
            otlp_metrics::record_decode_failure(&view_name, kind_for_metrics, decode_failure_reason(&e));
            otlp_metrics::record_request(&view_name, kind_for_metrics, encoding_label, e.status().as_u16());
            return e.into_response();
        }
    };

    // ── 5. Pick handler ──
    let handler = match pick_handler(&matched, kind) {
        Ok(h) => h,
        Err(e) => {
            otlp_metrics::record_decode_failure(&view_name, kind, decode_failure_reason(&e));
            otlp_metrics::record_request(&view_name, kind, encoding_label, e.status().as_u16());
            return e.into_response();
        }
    };

    // Pre-dispatch byte-size observations (records inbound shape regardless
    // of handler outcome).
    otlp_metrics::observe_request_bytes(&view_name, kind, encoding_label, body_bytes.len());
    otlp_metrics::observe_decoded_bytes(&view_name, kind, decompressed.len());

    let (module, entrypoint, language, _resources) = match handler {
        HandlerConfig::Codecomponent {
            module,
            entrypoint,
            language,
            resources,
        } => (module.clone(), entrypoint.clone(), language.clone(), resources.clone()),
        HandlerConfig::Dataview { .. } | HandlerConfig::None {} => {
            // [X-OTLP-2] guard: dataview / none handlers aren't valid for OTLP.
            // Treat as a misconfiguration that slipped past structural validation.
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"error": "OTLP view handler must be a codecomponent"}).to_string(),
                ))
                .unwrap_or_else(|_| Response::new(Body::empty()));
        }
    };

    // ── 6. Build dispatch envelope. The V8 engine exposes every top-level
    //      `args` key as `ctx.<key>`, so we put `otel` alongside `request`. ──
    let trace_id = uuid::Uuid::new_v4().to_string();
    let dv_namespace = matched.app_entry_point.clone();
    let request_envelope = json!({
        "method": "POST",
        "path": path,
        "headers": headers,
        "body": payload,
        "path_params": serde_json::Map::<String, Value>::new(),
        "query": serde_json::Map::<String, Value>::new(),
    });
    let otel_envelope = json!({
        "kind": kind,
        "payload": payload,
        "encoding": encoding_label,
    });
    let args = json!({
        "request": request_envelope,
        "session": Value::Null,
        "otel": otel_envelope,
        "_dv_namespace": dv_namespace,
        "_source": null,
    });

    let entry = Entrypoint {
        module,
        function: entrypoint,
        language,
    };
    let dv_guard = ctx.dataview_executor.read().await;
    let dv_ref = dv_guard.as_deref();

    let builder = TaskContextBuilder::new()
        .entrypoint(entry)
        .args(args)
        .trace_id(trace_id.clone());
    let builder = crate::task_enrichment::wire_datasources(builder, dv_ref, &dv_namespace);
    let builder = crate::task_enrichment::enrich(
        builder,
        &dv_namespace,
        rivers_runtime::process_pool::TaskKind::Rest,
    );
    drop(dv_guard);

    let task_ctx = match builder.build() {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(trace_id = %trace_id, kind = %kind, "OTLP task build failed: {}", e);
            otlp_metrics::record_request(&view_name, kind, encoding_label, 500);
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"error": format!("OTLP dispatch build failed: {}", e)}).to_string(),
                ))
                .unwrap_or_else(|_| Response::new(Body::empty()));
        }
    };

    let dispatch_start = std::time::Instant::now();
    let result = match ctx.pool.dispatch("default", task_ctx).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(trace_id = %trace_id, kind = %kind, "OTLP handler error: {}", e);
            otlp_metrics::record_request(&view_name, kind, encoding_label, 500);
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"error": format!("OTLP handler dispatch failed: {}", e)}).to_string(),
                ))
                .unwrap_or_else(|_| Response::new(Body::empty()));
        }
    };
    let dispatch_ms = dispatch_start.elapsed().as_millis() as f64;
    otlp_metrics::observe_dispatch_duration_ms(&view_name, kind, dispatch_ms);
    tracing::info!(
        trace_id = %trace_id,
        kind = %kind,
        encoding = %encoding_label,
        body_bytes = body_bytes.len(),
        decoded_bytes = decompressed.len(),
        duration_ms = dispatch_ms as u64,
        "OTLP request handled"
    );

    // Handler-reported rejection metrics — only meaningful when the handler
    // ran to completion and returned a numeric `rejected`.
    let rejected = result
        .value
        .get("rejected")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if rejected > 0 {
        otlp_metrics::record_partial_success(&view_name, kind);
        otlp_metrics::record_rejected_points(&view_name, kind, rejected);
    }

    otlp_metrics::record_request(&view_name, kind, encoding_label, 200);

    let response_body = shape_response_body(&result.value, kind);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(response_body.to_string()))
        .unwrap_or_else(|_| Response::new(Body::empty()))
        .into_response()
}

// ── Unit tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn signal_from_path_matches_canonical_endpoints() {
        assert_eq!(signal_from_path("/v1/metrics").unwrap(), "metrics");
        assert_eq!(signal_from_path("/v1/logs").unwrap(), "logs");
        assert_eq!(signal_from_path("/v1/traces").unwrap(), "traces");
        assert_eq!(signal_from_path("/otel/v1/metrics").unwrap(), "metrics");
        assert_eq!(signal_from_path("/telemetry/otel/v1/traces").unwrap(), "traces");
        // Trailing slash tolerated.
        assert_eq!(signal_from_path("/v1/metrics/").unwrap(), "metrics");
    }

    #[test]
    fn signal_from_path_rejects_unknown_paths() {
        assert!(matches!(
            signal_from_path("/v1/foo"),
            Err(OtlpError::UnknownSignal(_))
        ));
        assert!(matches!(
            signal_from_path("/otel"),
            Err(OtlpError::UnknownSignal(_))
        ));
        assert!(matches!(
            signal_from_path("/v1/metricsx"),
            Err(OtlpError::UnknownSignal(_))
        ));
    }

    #[test]
    fn rejected_field_per_signal_matches_otlp_spec() {
        assert_eq!(rejected_field_for_signal("metrics"), "rejectedDataPoints");
        assert_eq!(rejected_field_for_signal("logs"), "rejectedLogRecords");
        assert_eq!(rejected_field_for_signal("traces"), "rejectedSpans");
        // Unknown signals default to the metrics field — defensive only;
        // signal_from_path filters out anything not in the canonical set.
        assert_eq!(rejected_field_for_signal("wat"), "rejectedDataPoints");
    }

    #[test]
    fn decompress_identity_passes_body_through() {
        let body = b"hello";
        assert_eq!(decompress_body(body, None, 16).unwrap(), body);
        assert_eq!(decompress_body(body, Some(""), 16).unwrap(), body);
        assert_eq!(decompress_body(body, Some("identity"), 16).unwrap(), body);
    }

    #[test]
    fn decompress_gzip_round_trips() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"{\"ok\":true}").unwrap();
        let gz = enc.finish().unwrap();
        let got = decompress_body(&gz, Some("gzip"), 1024).unwrap();
        assert_eq!(got, b"{\"ok\":true}");
    }

    #[test]
    fn decompress_deflate_round_trips() {
        // RFC 9110: Content-Encoding `deflate` = zlib format (RFC 1950),
        // NOT raw RFC 1951 deflate. Use ZlibEncoder to produce the
        // wire-format the framework actually accepts.
        let mut enc =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"abc").unwrap();
        let df = enc.finish().unwrap();
        let got = decompress_body(&df, Some("deflate"), 1024).unwrap();
        assert_eq!(got, b"abc");
    }

    #[test]
    fn decompress_unknown_encoding_returns_415_class() {
        let err = decompress_body(b"x", Some("br"), 1024).unwrap_err();
        assert!(matches!(err, OtlpError::UnsupportedEncoding(_)));
        assert_eq!(err.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[test]
    fn decompress_zip_bomb_caught_by_cap() {
        // 1 byte repeated 10000 times compresses to ~30 bytes; cap at 50 bytes.
        let payload = b"A".repeat(10_000);
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(&payload).unwrap();
        let gz = enc.finish().unwrap();
        let err = decompress_body(&gz, Some("gzip"), 50).unwrap_err();
        assert!(matches!(err, OtlpError::BodyTooLarge { .. }));
        assert_eq!(err.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn decode_json_body_round_trips() {
        let (v, enc) =
            decode_body(b"{\"a\":1}", Some("application/json"), "/v1/metrics").unwrap();
        assert_eq!(v, json!({"a": 1}));
        assert_eq!(enc, "json");
    }

    #[test]
    fn decode_json_with_charset_param_accepted() {
        let (v, enc) = decode_body(
            b"{\"a\":1}",
            Some("application/json; charset=utf-8"),
            "/v1/metrics",
        )
        .unwrap();
        assert_eq!(v, json!({"a": 1}));
        assert_eq!(enc, "json");
    }

    #[test]
    fn decode_unknown_content_type_returns_415_class() {
        let err = decode_body(b"x", Some("text/plain"), "/v1/metrics").unwrap_err();
        assert!(matches!(err, OtlpError::UnsupportedContentType(_)));
        assert_eq!(err.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[test]
    fn decode_missing_content_type_returns_415_class() {
        let err = decode_body(b"x", None, "/v1/metrics").unwrap_err();
        assert!(matches!(err, OtlpError::UnsupportedContentType(_)));
    }

    #[test]
    fn decode_protobuf_rejects_invalid_bytes() {
        // Garbage bytes labeled protobuf → ProtobufDecodeFailed → 415.
        let err =
            decode_body(b"\x06\x06\x06garbage", Some("application/x-protobuf"), "/v1/metrics")
                .unwrap_err();
        assert!(matches!(err, OtlpError::ProtobufDecodeFailed(_)));
        assert_eq!(err.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[test]
    fn shape_response_empty_on_no_rejections() {
        let v = shape_response_body(&json!({"unrelated": "field"}), "metrics");
        assert_eq!(v, json!({}));
    }

    #[test]
    fn shape_response_partial_success_picks_field_per_signal() {
        let v = shape_response_body(
            &json!({"rejected": 3, "errorMessage": "boom"}),
            "metrics",
        );
        assert_eq!(
            v,
            json!({"partialSuccess": {"rejectedDataPoints": 3, "errorMessage": "boom"}})
        );
        let v = shape_response_body(&json!({"rejected": 2}), "logs");
        assert_eq!(v, json!({"partialSuccess": {"rejectedLogRecords": 2}}));
        let v = shape_response_body(&json!({"rejected": 7}), "traces");
        assert_eq!(v, json!({"partialSuccess": {"rejectedSpans": 7}}));
    }

    #[test]
    fn shape_response_omits_empty_error_message() {
        let v = shape_response_body(&json!({"rejected": 1, "errorMessage": ""}), "metrics");
        assert_eq!(v, json!({"partialSuccess": {"rejectedDataPoints": 1}}));
    }

    #[test]
    fn error_response_carries_json_error_body() {
        let resp = OtlpError::JsonParseFailed("bad token".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json"
        );
    }

    // ── pick_handler: single- vs multi-handler form (Track O3) ──────

    fn matched_with(config: rivers_runtime::view::ApiViewConfig) -> crate::server::view_dispatch::MatchedRoute {
        crate::server::view_dispatch::MatchedRoute {
            config,
            app_entry_point: String::new(),
            app_id: String::new(),
            path_params: std::collections::HashMap::new(),
            guard_view_path: None,
            view_id: "otel_ingest:metrics".into(),
        }
    }

    fn cc_handler(entrypoint: &str) -> rivers_runtime::view::HandlerConfig {
        rivers_runtime::view::HandlerConfig::Codecomponent {
            language: "javascript".into(),
            module: "otel.js".into(),
            entrypoint: entrypoint.into(),
            resources: vec![],
        }
    }

    #[test]
    fn pick_handler_prefers_multi_handler_form_when_signal_present() {
        // Multi-handler form: handlers.{metrics,logs,traces} → each signal
        // routes to its own entrypoint. Spec §3.1.
        let mut handlers = std::collections::HashMap::new();
        handlers.insert("metrics".to_string(), cc_handler("ingestMetrics"));
        handlers.insert("logs".to_string(), cc_handler("ingestLogs"));
        handlers.insert("traces".to_string(), cc_handler("ingestTraces"));
        let cfg = serde_json::from_value::<rivers_runtime::view::ApiViewConfig>(
            serde_json::json!({
                "view_type": "OTLP",
                "path": "/otel",
                "handlers": {
                    "metrics": { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestMetrics", "resources": [] },
                    "logs":    { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestLogs", "resources": [] },
                    "traces":  { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestTraces", "resources": [] }
                }
            })
        ).unwrap();
        let matched = matched_with(cfg);

        for (signal, expected) in &[
            ("metrics", "ingestMetrics"),
            ("logs", "ingestLogs"),
            ("traces", "ingestTraces"),
        ] {
            let h = pick_handler(&matched, signal).expect("handler present");
            match h {
                rivers_runtime::view::HandlerConfig::Codecomponent { entrypoint, .. } => {
                    assert_eq!(entrypoint, expected,
                        "signal '{}' routed to wrong handler", signal);
                }
                other => panic!("expected codecomponent, got {:?}", other),
            }
        }
    }

    #[test]
    fn pick_handler_falls_back_to_single_handler_form() {
        // Single-handler form: only `handler:` is declared, no `handlers.*`.
        // All three signals route to the same handler; handler is expected
        // to read ctx.otel.kind to discriminate. Spec §3.2.
        let cfg = serde_json::from_value::<rivers_runtime::view::ApiViewConfig>(
            serde_json::json!({
                "view_type": "OTLP",
                "path": "/otel",
                "handler": { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestAny", "resources": [] }
            })
        ).unwrap();
        let matched = matched_with(cfg);

        for signal in &["metrics", "logs", "traces"] {
            let h = pick_handler(&matched, signal).expect("single handler picked");
            match h {
                rivers_runtime::view::HandlerConfig::Codecomponent { entrypoint, .. } => {
                    assert_eq!(entrypoint, "ingestAny",
                        "single-handler form must dispatch to same entrypoint for all signals");
                }
                other => panic!("expected codecomponent, got {:?}", other),
            }
        }
    }

    #[test]
    fn strip_signal_suffix_handles_synthesised_and_bare_ids() {
        // Router-internal IDs are `<view>:<signal>` (see view_engine::router
        // from_views — registers 3 routes per OTLP view). The metric `view`
        // label should be the operator-facing view name without the suffix.
        assert_eq!(
            otlp_metrics::strip_signal_suffix("otel_ingest:metrics"),
            "otel_ingest"
        );
        assert_eq!(
            otlp_metrics::strip_signal_suffix("otel_ingest:traces"),
            "otel_ingest"
        );
        // Bare id with no signal suffix (defensive — not produced by our
        // router but exercised here so the helper is forgiving).
        assert_eq!(otlp_metrics::strip_signal_suffix("bare"), "bare");
        // Multi-colon: only the last `:` is the signal separator.
        assert_eq!(
            otlp_metrics::strip_signal_suffix("ns:view:logs"),
            "ns:view"
        );
    }

    #[test]
    fn decode_failure_reason_covers_every_error_variant() {
        // Each OtlpError variant must map to a stable `reason` label so
        // operators can build alerts on specific failure classes.
        assert_eq!(
            decode_failure_reason(&OtlpError::BodyTooLarge {
                observed: 0,
                limit_bytes: 0
            }),
            "size_pre"
        );
        assert_eq!(
            decode_failure_reason(&OtlpError::UnsupportedEncoding("br".into())),
            "encoding"
        );
        assert_eq!(
            decode_failure_reason(&OtlpError::UnsupportedContentType("text/plain".into())),
            "content_type"
        );
        assert_eq!(
            decode_failure_reason(&OtlpError::DecompressionFailed("oops".into())),
            "decompress"
        );
        assert_eq!(
            decode_failure_reason(&OtlpError::JsonParseFailed("oops".into())),
            "json"
        );
        assert_eq!(
            decode_failure_reason(&OtlpError::ProtobufDecodeFailed("oops".into())),
            "protobuf"
        );
        assert_eq!(
            decode_failure_reason(&OtlpError::UnknownSignal("/v1/wat".into())),
            "signal"
        );
        assert_eq!(
            decode_failure_reason(&OtlpError::SignalNotConfigured("logs".into())),
            "signal"
        );
    }

    #[test]
    fn pick_handler_returns_signal_not_configured_on_partial_multi_form() {
        // handlers.metrics declared, but no handlers.logs / handlers.traces.
        // A request to /v1/logs picks the missing handler → SignalNotConfigured.
        let cfg = serde_json::from_value::<rivers_runtime::view::ApiViewConfig>(
            serde_json::json!({
                "view_type": "OTLP",
                "path": "/otel",
                "handlers": {
                    "metrics": { "type": "codecomponent", "language": "javascript", "module": "otel.js", "entrypoint": "ingestMetrics", "resources": [] }
                }
            })
        ).unwrap();
        let matched = matched_with(cfg);

        // Metrics resolves.
        assert!(pick_handler(&matched, "metrics").is_ok());

        // Logs and traces don't.
        for signal in &["logs", "traces"] {
            let err = pick_handler(&matched, signal).unwrap_err();
            match err {
                OtlpError::SignalNotConfigured(s) => {
                    assert_eq!(&s, signal,
                        "SignalNotConfigured should carry the missing signal name");
                }
                other => panic!("expected SignalNotConfigured for '{}', got {:?}", signal, other),
            }
        }
    }
}
