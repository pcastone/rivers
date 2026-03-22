use riversd::health::{
    parse_simulate_delay, DatasourceProbeResult, HealthResponse, PoolSnapshot, UptimeTracker,
    VerboseHealthResponse,
};

// ── HealthResponse ──────────────────────────────────────────────

#[test]
fn health_response_ok() {
    let resp = HealthResponse::ok("my-app".into(), "production".into(), "1.0.0".into());
    assert_eq!(resp.status, "ok");
    assert_eq!(resp.service, "my-app");

    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["service"], "my-app");
    assert_eq!(json["environment"], "production");
    assert_eq!(json["version"], "1.0.0");
}

// ── VerboseHealthResponse ───────────────────────────────────────

#[test]
fn verbose_health_serialization() {
    let resp = VerboseHealthResponse {
        status: "ok",
        service: "my-app".into(),
        environment: "staging".into(),
        version: "1.0.0".into(),
        draining: false,
        inflight_requests: 5,
        uptime_seconds: 3600,
        pool_snapshots: vec![PoolSnapshot {
            name: "postgres".into(),
            driver: "postgresql".into(),
            active: 3,
            idle: 7,
            max: 10,
            circuit_state: "closed".into(),
        }],
        datasource_probes: vec![],
    };

    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["draining"], false);
    assert_eq!(json["inflight_requests"], 5);
    assert_eq!(json["uptime_seconds"], 3600);
    assert_eq!(json["pool_snapshots"][0]["name"], "postgres");
    assert_eq!(json["pool_snapshots"][0]["circuit_state"], "closed");
}

// ── UptimeTracker ───────────────────────────────────────────────

#[test]
fn uptime_starts_at_zero() {
    let tracker = UptimeTracker::new();
    // Should be very close to 0
    assert!(tracker.uptime_seconds() < 2);
}

// ── Simulate Delay ──────────────────────────────────────────────

#[test]
fn parse_delay_present() {
    assert_eq!(parse_simulate_delay(Some("simulate_delay_ms=500")), Some(500));
}

#[test]
fn parse_delay_with_other_params() {
    assert_eq!(
        parse_simulate_delay(Some("foo=bar&simulate_delay_ms=100&baz=1")),
        Some(100)
    );
}

#[test]
fn parse_delay_missing() {
    assert_eq!(parse_simulate_delay(Some("foo=bar")), None);
}

#[test]
fn parse_delay_none_query() {
    assert_eq!(parse_simulate_delay(None), None);
}

#[test]
fn parse_delay_invalid_value() {
    assert_eq!(parse_simulate_delay(Some("simulate_delay_ms=abc")), None);
}

// ── DatasourceProbeResult (AX2) ──────────────────────────────────

#[test]
fn probe_result_ok_serialization() {
    let probe = DatasourceProbeResult {
        name: "mydb".into(),
        driver: "postgres".into(),
        status: "ok".into(),
        latency_ms: 12,
        error: None,
    };
    let json = serde_json::to_value(&probe).unwrap();
    assert_eq!(json["name"], "mydb");
    assert_eq!(json["driver"], "postgres");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["latency_ms"], 12);
    assert!(json.get("error").is_none(), "error should be skipped when None");
}

#[test]
fn probe_result_error_serialization() {
    let probe = DatasourceProbeResult {
        name: "broken_db".into(),
        driver: "mysql".into(),
        status: "error".into(),
        latency_ms: 5000,
        error: Some("connection refused".into()),
    };
    let json = serde_json::to_value(&probe).unwrap();
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"], "connection refused");
}

#[test]
fn verbose_health_includes_probes() {
    let resp = VerboseHealthResponse {
        status: "ok",
        service: "riversd".into(),
        environment: "test".into(),
        version: "0.1.0".into(),
        draining: false,
        inflight_requests: 0,
        uptime_seconds: 10,
        pool_snapshots: vec![],
        datasource_probes: vec![
            DatasourceProbeResult {
                name: "pg".into(),
                driver: "postgres".into(),
                status: "ok".into(),
                latency_ms: 3,
                error: None,
            },
            DatasourceProbeResult {
                name: "redis".into(),
                driver: "redis".into(),
                status: "error".into(),
                latency_ms: 5000,
                error: Some("timeout".into()),
            },
        ],
    };
    let json = serde_json::to_value(&resp).unwrap();
    let probes = json["datasource_probes"].as_array().unwrap();
    assert_eq!(probes.len(), 2);
    assert_eq!(probes[0]["name"], "pg");
    assert_eq!(probes[0]["status"], "ok");
    assert_eq!(probes[1]["name"], "redis");
    assert_eq!(probes[1]["status"], "error");
}
