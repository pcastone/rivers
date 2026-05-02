//! OTel span export initialization — P1.7.
//!
//! Called once at startup in `main.rs` before the tracing subscriber is
//! installed. `init_otel` builds the OTLP provider and returns the tracer
//! so `main.rs` can wire it directly into `OpenTelemetryLayer::new(tracer)`
//! rather than relying on `global::tracer()` which may capture a no-op.

use std::sync::OnceLock;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::{SdkTracerProvider, SdkTracer};
use opentelemetry_sdk::Resource;
use rivers_runtime::rivers_core::config::TelemetryConfig;

// Retained so force_flush() and shutdown() can reach it without going through
// the global tracer provider (which doesn't expose those methods).
static PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

/// Initialize the OTel OTLP exporter.
///
/// Returns the SDK tracer to wire into `OpenTelemetryLayer::new()`.
/// Returns `None` if the exporter fails to build (OTel disabled for this run).
pub fn init_otel(cfg: &TelemetryConfig) -> Option<SdkTracer> {
    let resource = Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", cfg.service_name.clone()),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ])
        .build();

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(&cfg.otlp_endpoint)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "telemetry: failed to build OTLP span exporter ({}): {} — OTel disabled",
                cfg.otlp_endpoint, e
            );
            return None;
        }
    };

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    let tracer = provider.tracer("riversd");

    // Store for force_flush / shutdown access.
    let _ = PROVIDER.set(provider.clone());
    // Also register globally so any code using global::tracer() picks it up.
    opentelemetry::global::set_tracer_provider(provider);

    Some(tracer)
}

/// Flush all pending spans synchronously.
///
/// Called in integration tests after making a request to drain the batch
/// exporter before querying the collector for the emitted traces.
pub fn force_flush() {
    if let Some(p) = PROVIDER.get() {
        if let Err(e) = p.force_flush() {
            tracing::warn!(error = %e, "telemetry: force_flush failed");
        }
    }
}

/// Flush and shut down the tracer provider.
///
/// Called during graceful shutdown after all in-flight requests have drained,
/// ensuring the final span batch is exported before the process exits.
pub fn shutdown() {
    if let Some(p) = PROVIDER.get() {
        if let Err(e) = p.shutdown() {
            tracing::warn!(error = %e, "telemetry: provider shutdown failed");
        }
    }
}
