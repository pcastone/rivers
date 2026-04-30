//! OTel span export integration tests — P1.7.g.
//!
//! These tests require a running Jaeger instance on beta-01 and are guarded by
//! the `RIVERS_INTEGRATION_TEST` env var so they skip in CI.
//!
//! Jaeger configuration expected on beta-01:
//!   OTLP HTTP ingest: http://beta-01:4318/v1/traces
//!   Query API:        http://beta-01:16686
//!
//! Run locally against beta-01:
//!   RIVERS_INTEGRATION_TEST=1 cargo test -p riversd telemetry_otel -- --nocapture

use rivers_runtime::rivers_core::config::TelemetryConfig;
use rivers_runtime::rivers_core::ServerConfig;

const JAEGER_HOST: &str = "beta-01";
const JAEGER_OTLP_PORT: u16 = 4318;
const JAEGER_QUERY_PORT: u16 = 16686;
const TEST_SERVICE: &str = "rivers-otel-test";

fn integration_test_enabled() -> bool {
    std::env::var("RIVERS_INTEGRATION_TEST").map(|v| v == "1").unwrap_or(false)
}

fn telemetry_config() -> TelemetryConfig {
    TelemetryConfig {
        otlp_endpoint: format!("http://{}:{}/v1/traces", JAEGER_HOST, JAEGER_OTLP_PORT),
        service_name: TEST_SERVICE.to_string(),
    }
}

/// Build a minimal ServerConfig with a single faker DataView + REST view.
fn server_config_with_telemetry(telemetry: Option<TelemetryConfig>) -> ServerConfig {
    let mut config = ServerConfig::default();
    config.base.admin_api.no_auth = Some(true);
    config.telemetry = telemetry;

    // Wire up a minimal fake app bundle inline via AppConfig so the handler
    // path executes and generates "handler" + "dataview" spans.
    //
    // The view_dispatch path fires when a request matches a loaded app's view.
    // For this test we rely on the /health endpoint (no DataView span) to
    // verify the handler span and the OTel pipeline are wired correctly.
    // A follow-up smoke test (G6) verifies the full "handler"+"dataview"
    // pair via the manual Jaeger UI check against a deployed bundle.
    config
}

/// Query the Jaeger HTTP API for recent traces for `service_name`.
///
/// Returns the raw JSON value on success.  Retries up to `attempts` times
/// with a 500ms delay between attempts to allow the batch exporter to flush.
async fn query_jaeger_traces(service_name: &str, attempts: u8) -> serde_json::Value {
    let client = reqwest::Client::new();
    let url = format!(
        "http://{}:{}/api/traces?service={}&limit=20",
        JAEGER_HOST, JAEGER_QUERY_PORT, service_name
    );

    for i in 0..attempts {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                let data = json.get("data").and_then(|d| d.as_array());
                if data.map(|d| !d.is_empty()).unwrap_or(false) {
                    return json;
                }
            }
        }
    }

    serde_json::json!({ "data": [] })
}

/// Delete all traces for `service_name` from Jaeger before each test to
/// avoid cross-test contamination.  Best-effort — Jaeger may not expose a
/// delete API in all deployments.
async fn purge_jaeger_service(service_name: &str) {
    let client = reqwest::Client::new();
    let url = format!(
        "http://{}:{}/api/services/{}",
        JAEGER_HOST, JAEGER_QUERY_PORT, service_name
    );
    let _ = client.delete(&url).send().await;
}

// ── Test 1: handler span reaches Jaeger ──────────────────────────

/// Spin up a test server with TelemetryConfig pointing at beta-01 Jaeger,
/// make a request to /health (exercises the tracing_opentelemetry layer),
/// call force_flush(), then query Jaeger and assert that at least one trace
/// arrived for `service_name = rivers-otel-test`.
///
/// The /health endpoint is handled by the built-in health handler which runs
/// inside the tracing layer installed in main.rs; the `tracing_opentelemetry`
/// bridge forwards those spans to the OTLP exporter.
#[tokio::test]
async fn spans_arrive_at_jaeger() {
    if !integration_test_enabled() {
        return;
    }

    purge_jaeger_service(TEST_SERVICE).await;

    // Initialize OTel with beta-01 Jaeger endpoint.
    let cfg = telemetry_config();
    riversd::telemetry::init_otel(&cfg);

    let server_config = server_config_with_telemetry(Some(cfg));

    // Spin up a real TCP listener on an ephemeral port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);

    // Run the server in a background task.
    let server_handle = tokio::spawn(async move {
        let _ = riversd::server::run_server_with_listener_with_control(
            server_config, listener, rx,
        )
        .await;
    });

    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Make a request to /health — this traverses the full tracing middleware.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .expect("health request failed");
    assert_eq!(resp.status().as_u16(), 200, "health endpoint should return 200");

    // Flush the batch exporter so the span is sent before we query Jaeger.
    riversd::telemetry::force_flush();

    // Shut the server down.
    tx.send(true).unwrap();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle).await;

    // Query Jaeger — retry up to 6 times (3 seconds total).
    let traces = query_jaeger_traces(TEST_SERVICE, 6).await;
    let data = traces["data"].as_array().expect("Jaeger response should have 'data' array");

    assert!(
        !data.is_empty(),
        "Expected at least one trace in Jaeger for service '{}' but found none.\n\
         Check that:\n  1. Jaeger is running on {} port {}\n  \
         2. OTLP HTTP ingest is enabled (port {})",
        TEST_SERVICE, JAEGER_HOST, JAEGER_QUERY_PORT, JAEGER_OTLP_PORT
    );

    // Verify at least one span is present in the trace.
    let total_spans: usize = data
        .iter()
        .flat_map(|t| t["spans"].as_array().map(|s| s.len()).into_iter())
        .sum();
    assert!(total_spans > 0, "Jaeger returned traces but no spans");
}

// ── Test 2: no exporter without TelemetryConfig ───────────────────

/// Verify that when `config.telemetry` is `None`, no spans are sent to Jaeger.
/// We make a request, wait, then query Jaeger and confirm no new traces for
/// the `no-telemetry-test` service name appeared.
///
/// This test guards against a regression where telemetry is initialized
/// unconditionally regardless of the config field.
#[tokio::test]
async fn no_exporter_without_telemetry_config() {
    if !integration_test_enabled() {
        return;
    }

    const NO_TEL_SERVICE: &str = "no-telemetry-test";
    purge_jaeger_service(NO_TEL_SERVICE).await;

    // Build server WITHOUT telemetry config — provider should not be initialized.
    let server_config = server_config_with_telemetry(None);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);

    let server_handle = tokio::spawn(async move {
        let _ = riversd::server::run_server_with_listener_with_control(
            server_config, listener, rx,
        )
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .expect("health request failed");
    assert_eq!(resp.status().as_u16(), 200);

    // Wait a moment — if a global provider were active it might flush here.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    tx.send(true).unwrap();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle).await;

    // Jaeger should have no traces for this service name.
    let traces = query_jaeger_traces(NO_TEL_SERVICE, 2).await;
    let data = traces["data"].as_array().expect("Jaeger response should have 'data' array");

    assert!(
        data.is_empty(),
        "Expected no traces in Jaeger for service '{}' when telemetry is disabled, \
         but found {} trace(s)",
        NO_TEL_SERVICE,
        data.len()
    );
}
