//! OTel span export initialization — P1.7.
//!
//! Called once at startup when `[telemetry]` is present in `riversd.toml`.
//! Installs an OTLP HTTP exporter and sets the global tracer provider.
//! The `tracing_opentelemetry` layer (wired in `main.rs`) bridges all
//! existing `tracing::` spans into the OTel pipeline automatically.

use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use rivers_runtime::rivers_core::config::TelemetryConfig;

/// Initialize the OTel OTLP exporter and set the global tracer provider.
///
/// Idempotent in practice — called at most once per process from lifecycle.
/// Logs a warning and returns without error on exporter build failure.
pub fn init_otel(cfg: &TelemetryConfig) {
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
            tracing::warn!(
                endpoint = %cfg.otlp_endpoint,
                error = %e,
                "telemetry: failed to build OTLP span exporter — OTel disabled"
            );
            return;
        }
    };

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    opentelemetry::global::set_tracer_provider(provider);

    tracing::info!(
        endpoint = %cfg.otlp_endpoint,
        service = %cfg.service_name,
        "telemetry: OTel OTLP exporter initialized"
    );
}
