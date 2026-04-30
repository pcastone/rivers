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
/// Logs a warning and returns without error on provider install failure.
pub fn init_otel(cfg: &TelemetryConfig) {
    let resource = Resource::new(vec![
        KeyValue::new("service.name", cfg.service_name.clone()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ]);

    let pipeline = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .http()
                .with_endpoint(&cfg.otlp_endpoint),
        )
        .with_trace_config(
            opentelemetry_sdk::trace::Config::default().with_resource(resource),
        );

    match pipeline.install_batch(opentelemetry_sdk::runtime::Tokio) {
        Ok(tracer) => {
            let _ = tracer; // provider is now global
            tracing::info!(
                endpoint = %cfg.otlp_endpoint,
                service = %cfg.service_name,
                "telemetry: OTel OTLP exporter initialized"
            );
        }
        Err(e) => {
            tracing::warn!(
                endpoint = %cfg.otlp_endpoint,
                error = %e,
                "telemetry: failed to install OTel OTLP exporter — OTel disabled"
            );
        }
    }
}
