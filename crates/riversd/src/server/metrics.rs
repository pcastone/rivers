//! Prometheus metrics for riversd.

use metrics::{counter, gauge, histogram};

/// Record an HTTP request with method, status, and duration.
pub fn record_request(method: &str, status: u16, duration_ms: f64) {
    counter!("rivers_http_requests_total", "method" => method.to_string(), "status" => status.to_string()).increment(1);
    histogram!("rivers_http_request_duration_ms", "method" => method.to_string()).record(duration_ms);
}

/// Set the gauge for active connections.
pub fn set_active_connections(count: usize) {
    gauge!("rivers_active_connections").set(count as f64);
}

/// Record an engine (V8/WASM) execution.
pub fn record_engine_execution(engine: &str, duration_ms: f64, success: bool) {
    counter!("rivers_engine_executions_total", "engine" => engine.to_string(), "success" => success.to_string()).increment(1);
    histogram!("rivers_engine_execution_duration_ms", "engine" => engine.to_string()).record(duration_ms);
}

/// Set the gauge for loaded apps.
pub fn set_loaded_apps(count: usize) {
    gauge!("rivers_loaded_apps").set(count as f64);
}
