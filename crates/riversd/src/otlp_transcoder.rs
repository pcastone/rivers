//! OTLP protobuf → JSON transcoder — P1.6.
//!
//! Incoming OTLP requests with `Content-Type: application/x-protobuf` are
//! decoded here to their corresponding prost message type and re-encoded as
//! JSON, allowing downstream handler code to work uniformly with JSON bodies.
//!
//! Supported signals (mapped by request path):
//!   /v1/traces   → ExportTraceServiceRequest
//!   /v1/metrics  → ExportMetricsServiceRequest
//!   /v1/logs     → ExportLogsServiceRequest
//!
//! Paths that do not match a known signal are passed through unchanged
//! (`UnknownSignal`). Decode failures return `DecodeFailed`.

use prost::Message;

/// Errors from [`transcode_otlp_protobuf`].
#[derive(Debug, thiserror::Error)]
pub enum TranscodeError {
    /// Path does not map to a known OTLP signal — caller should pass through.
    #[error("unknown OTLP signal path: {0}")]
    UnknownSignal(String),
    /// Binary protobuf could not be decoded for the matched signal.
    #[error("protobuf decode failed for {signal}: {reason}")]
    DecodeFailed {
        /// The matched signal name (e.g. "traces").
        signal: String,
        /// Underlying decode or serialization error.
        reason: String,
    },
}

/// Decode a binary OTLP protobuf body and re-encode it as JSON.
///
/// `path` — the HTTP request path (e.g. `/v1/traces`). Only the trailing
/// segment is matched; a prefix like `/otlp/v1/traces` also works.
///
/// Returns the JSON bytes on success, `UnknownSignal` when the path is not
/// a recognised OTLP endpoint, or `DecodeFailed` when prost can't parse the
/// payload.
pub fn transcode_otlp_protobuf(path: &str, body: &[u8]) -> Result<Vec<u8>, TranscodeError> {
    use opentelemetry_proto::tonic::collector;

    if path.ends_with("/v1/traces") {
        let msg = collector::trace::v1::ExportTraceServiceRequest::decode(body)
            .map_err(|e| TranscodeError::DecodeFailed {
                signal: "traces".into(),
                reason: e.to_string(),
            })?;
        serde_json::to_vec(&msg).map_err(|e| TranscodeError::DecodeFailed {
            signal: "traces".into(),
            reason: e.to_string(),
        })
    } else if path.ends_with("/v1/metrics") {
        let msg = collector::metrics::v1::ExportMetricsServiceRequest::decode(body)
            .map_err(|e| TranscodeError::DecodeFailed {
                signal: "metrics".into(),
                reason: e.to_string(),
            })?;
        serde_json::to_vec(&msg).map_err(|e| TranscodeError::DecodeFailed {
            signal: "metrics".into(),
            reason: e.to_string(),
        })
    } else if path.ends_with("/v1/logs") {
        let msg = collector::logs::v1::ExportLogsServiceRequest::decode(body)
            .map_err(|e| TranscodeError::DecodeFailed {
                signal: "logs".into(),
                reason: e.to_string(),
            })?;
        serde_json::to_vec(&msg).map_err(|e| TranscodeError::DecodeFailed {
            signal: "logs".into(),
            reason: e.to_string(),
        })
    } else {
        Err(TranscodeError::UnknownSignal(path.to_string()))
    }
}
