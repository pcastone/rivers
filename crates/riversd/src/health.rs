//! Health endpoint logic.
//!
//! Per `rivers-httpd-spec.md` §14.
//!
//! - `GET /health` — always 200, basic status
//! - `GET /health/verbose` — extended diagnostics (pool snapshots, uptime, cluster state)
//! - `?simulate_delay_ms=N` for testing

use std::time::Instant;

use serde::Serialize;

// ── Health Response Types ───────────────────────────────────────

/// Basic health response for `GET /health`.
///
/// Per spec §14.1: always 200 (even during drain — load balancer pulls via drain header).
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub service: String,
    pub environment: String,
    pub version: String,
}

impl HealthResponse {
    pub fn ok(service: String, environment: String, version: String) -> Self {
        Self {
            status: "ok",
            service,
            environment,
            version,
        }
    }
}

/// Verbose health response for `GET /health/verbose`.
///
/// Per spec §14.2: pool snapshots, cluster state, uptime, datasource probes.
#[derive(Debug, Clone, Serialize)]
pub struct VerboseHealthResponse {
    pub status: &'static str,
    pub service: String,
    pub environment: String,
    pub version: String,
    pub draining: bool,
    pub inflight_requests: u64,
    pub uptime_seconds: u64,
    pub pool_snapshots: Vec<PoolSnapshot>,
    pub datasource_probes: Vec<DatasourceProbeResult>,
}

/// Result of a per-datasource connectivity probe.
#[derive(Debug, Clone, Serialize)]
pub struct DatasourceProbeResult {
    pub name: String,
    pub driver: String,
    pub status: String,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Snapshot of a connection pool's health.
#[derive(Debug, Clone, Serialize)]
pub struct PoolSnapshot {
    pub name: String,
    pub driver: String,
    pub active: u32,
    pub idle: u32,
    pub max: u32,
    pub circuit_state: String,
}

// ── Uptime Tracker ──────────────────────────────────────────────

/// Tracks server start time for uptime reporting.
pub struct UptimeTracker {
    started_at: Instant,
}

impl UptimeTracker {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

impl Default for UptimeTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Simulate Delay ──────────────────────────────────────────────

/// Parse `?simulate_delay_ms=N` from query string.
///
/// Per spec §14.3: for testing load balancer behavior.
pub fn parse_simulate_delay(query: Option<&str>) -> Option<u64> {
    let query = query?;
    for pair in query.split('&') {
        if let Some(val) = pair.strip_prefix("simulate_delay_ms=") {
            return val.parse().ok();
        }
    }
    None
}
