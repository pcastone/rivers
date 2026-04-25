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
    /// Status string, always `"ok"`.
    pub status: &'static str,
    /// Service name from config.
    pub service: String,
    /// Deployment environment (e.g. `"production"`).
    pub environment: String,
    /// Application version string.
    pub version: String,
}

impl HealthResponse {
    /// Create a healthy response with status `"ok"`.
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
    /// Status string, always `"ok"`.
    pub status: &'static str,
    /// Service name from config.
    pub service: String,
    /// Deployment environment (e.g. `"production"`).
    pub environment: String,
    /// Application version string.
    pub version: String,
    /// Whether the server is in graceful-drain mode.
    pub draining: bool,
    /// Number of currently in-flight requests.
    pub inflight_requests: u64,
    /// Seconds since the server started.
    pub uptime_seconds: u64,
    /// Per-pool connection statistics.
    pub pool_snapshots: Vec<PoolSnapshot>,
    /// Per-datasource connectivity probe results.
    pub datasource_probes: Vec<DatasourceProbeResult>,
    /// Per-broker bridge connection states. Empty when no broker datasources
    /// are configured. Surfaces broker readiness independently of process
    /// readiness — see code review P0-4.
    pub broker_bridges: Vec<BrokerBridgeHealth>,
}

/// Broker bridge state surfaced via `/health/verbose`.
///
/// Mirrors `broker_supervisor::BrokerBridgeStatus` but uses string state
/// for stable JSON output.
#[derive(Debug, Clone, Serialize)]
pub struct BrokerBridgeHealth {
    /// Datasource name.
    pub datasource: String,
    /// Broker driver name (e.g. `kafka`).
    pub driver: String,
    /// Connection state: `pending` | `connecting` | `connected` | `disconnected` | `stopped`.
    pub state: &'static str,
    /// Most recent error string, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Consecutive failed connect attempts since last success.
    pub failed_attempts: u32,
}

/// Result of a per-datasource connectivity probe.
#[derive(Debug, Clone, Serialize)]
pub struct DatasourceProbeResult {
    /// Datasource name from config.
    pub name: String,
    /// Driver type used by this datasource.
    pub driver: String,
    /// Probe result status (e.g. `"ok"` or `"error"`).
    pub status: String,
    /// Round-trip probe latency in milliseconds.
    pub latency_ms: u64,
    /// Error message if the probe failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Snapshot of a connection pool's health.
#[derive(Debug, Clone, Serialize)]
pub struct PoolSnapshot {
    /// Pool / datasource name.
    pub name: String,
    /// Driver type for this pool.
    pub driver: String,
    /// Number of active (in-use) connections.
    pub active: u32,
    /// Number of idle connections.
    pub idle: u32,
    /// Maximum pool size.
    pub max: u32,
    /// Circuit breaker state (e.g. `"closed"`, `"open"`).
    pub circuit_state: String,
}

// ── Uptime Tracker ──────────────────────────────────────────────

/// Tracks server start time for uptime reporting.
pub struct UptimeTracker {
    started_at: Instant,
}

impl UptimeTracker {
    /// Create a new tracker anchored to the current instant.
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }

    /// Return elapsed seconds since the tracker was created.
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
