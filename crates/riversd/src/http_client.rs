//! Shared outbound HTTP client used by V8 and dynamic-engine host
//! callbacks (`Rivers.http.*` and `host_http_request`).
//!
//! One process-wide client → shared connection pool, single timeout
//! policy. Without timeouts, a stalled upstream service would pin a
//! V8/engine worker indefinitely.
//!
//! Phase H — H6 (V8 path) + H7 (dynamic-engine path), code-review
//! tracking T2-6 / T2-7.
//!
//! TODO(H6/H7 follow-up): expose `[base.outbound_http] timeout_ms`
//! and `connect_timeout_ms` config knobs so operators can tune.
//! Hard-coded today; H2 used the same 30s default for host bridges.

use std::sync::OnceLock;
use std::time::Duration;

/// Total request timeout (connect + headers + body). Mirrors H2's
/// 30s host-bridge ceiling.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Inner bound on TCP/TLS handshake. A reachable-but-silent host
/// must fail fast rather than burn the full request budget.
const CONNECT_TIMEOUT_SECS: u64 = 5;

static OUTBOUND_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Process-wide outbound `reqwest::Client`. Built lazily on first use,
/// reused thereafter. `reqwest::Client` is internally `Arc`-wrapped, so
/// callers may `.clone()` cheaply when an owned handle is needed.
pub(crate) fn outbound_client() -> &'static reqwest::Client {
    OUTBOUND_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .build()
            .expect("default reqwest::Client::builder configuration is always valid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `outbound_client()` returns a reference to the same shared client
    /// across calls — proves the OnceLock wiring, not the timeout firing.
    #[test]
    fn outbound_client_is_shared() {
        let a = outbound_client();
        let b = outbound_client();
        assert!(std::ptr::eq(a, b), "outbound_client() must return the same &'static client");
    }

    /// Integration-level proof that the timeout policy is wired:
    /// TEST-NET-3 (203.0.113.1) is reserved-for-documentation and
    /// will not respond. The connect_timeout (5s) must fire well
    /// inside 35s. Adds ~5s of test wall time.
    #[tokio::test]
    async fn outbound_http_times_out_on_unreachable_endpoint() {
        let client = outbound_client();
        let start = std::time::Instant::now();
        let res = client.get("http://203.0.113.1/").send().await;
        let elapsed = start.elapsed();
        assert!(res.is_err(), "request to TEST-NET-3 should not succeed");
        assert!(
            elapsed < Duration::from_secs(35),
            "elapsed {:?} > 35s — outbound timeout did not fire",
            elapsed
        );
    }
}
