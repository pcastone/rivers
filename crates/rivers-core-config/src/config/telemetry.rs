//! Telemetry configuration — OTLP span export.

use schemars::JsonSchema;
use serde::Deserialize;

/// OTel span export configuration.
///
/// When present, riversd initializes an OTLP HTTP exporter and installs a
/// `tracing_opentelemetry` layer so all tracing spans are exported to the
/// configured endpoint. When absent, no exporter is initialized.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TelemetryConfig {
    /// OTLP HTTP endpoint to export spans to (e.g. `http://collector:4318/v1/traces`).
    pub otlp_endpoint: String,

    /// Service name reported in span metadata.
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

fn default_service_name() -> String {
    "riversd".to_string()
}
