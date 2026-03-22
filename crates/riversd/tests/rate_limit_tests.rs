//! Rate limiting and backpressure tests.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware as axum_middleware;
use axum::routing::get;
use axum::Router;
use tower::ServiceExt;

use riversd::backpressure::BackpressureState;
use riversd::rate_limit::*;

// ── Token Bucket ──────────────────────────────────────────────────

#[tokio::test]
async fn rate_limiter_allows_within_burst() {
    let config = RateLimitConfig {
        requests_per_minute: 60,
        burst_size: 5,
        strategy: RateLimitStrategy::Ip,
    };
    let limiter = RateLimiter::new(&config);

    for _ in 0..5 {
        match limiter.check("client1").await {
            RateLimitResult::Allowed => {}
            RateLimitResult::Limited { .. } => panic!("should be allowed within burst"),
        }
    }
}

#[tokio::test]
async fn rate_limiter_limits_after_burst() {
    let config = RateLimitConfig {
        requests_per_minute: 60,
        burst_size: 3,
        strategy: RateLimitStrategy::Ip,
    };
    let limiter = RateLimiter::new(&config);

    // Exhaust burst
    for _ in 0..3 {
        limiter.check("client1").await;
    }

    // Should be limited
    match limiter.check("client1").await {
        RateLimitResult::Limited { retry_after_secs } => {
            assert!(retry_after_secs >= 1, "should have retry-after");
        }
        RateLimitResult::Allowed => panic!("should be limited after burst"),
    }
}

#[tokio::test]
async fn rate_limiter_separate_keys() {
    let config = RateLimitConfig {
        requests_per_minute: 60,
        burst_size: 2,
        strategy: RateLimitStrategy::Ip,
    };
    let limiter = RateLimiter::new(&config);

    // Exhaust client1
    limiter.check("client1").await;
    limiter.check("client1").await;
    match limiter.check("client1").await {
        RateLimitResult::Limited { .. } => {}
        _ => panic!("client1 should be limited"),
    }

    // client2 should still be allowed
    match limiter.check("client2").await {
        RateLimitResult::Allowed => {}
        _ => panic!("client2 should be allowed"),
    }
}

#[tokio::test]
async fn rate_limiter_refills_over_time() {
    let config = RateLimitConfig {
        requests_per_minute: 60_000, // 1000/sec = 1/ms
        burst_size: 1,
        strategy: RateLimitStrategy::Ip,
    };
    let limiter = RateLimiter::new(&config);

    // Exhaust
    limiter.check("client1").await;
    match limiter.check("client1").await {
        RateLimitResult::Limited { .. } => {}
        _ => panic!("should be limited"),
    }

    // Wait for refill
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    match limiter.check("client1").await {
        RateLimitResult::Allowed => {}
        RateLimitResult::Limited { .. } => panic!("should have refilled"),
    }
}

#[tokio::test]
async fn rate_limiter_bucket_count() {
    let config = RateLimitConfig::default();
    let limiter = RateLimiter::new(&config);

    limiter.check("a").await;
    limiter.check("b").await;
    limiter.check("c").await;

    assert_eq!(limiter.bucket_count().await, 3);
}

#[tokio::test]
async fn rate_limiter_retry_after_is_positive() {
    let config = RateLimitConfig {
        requests_per_minute: 60,
        burst_size: 1,
        strategy: RateLimitStrategy::Ip,
    };
    let limiter = RateLimiter::new(&config);

    limiter.check("client1").await;
    match limiter.check("client1").await {
        RateLimitResult::Limited { retry_after_secs } => {
            assert!(retry_after_secs >= 1);
        }
        _ => panic!("should be limited"),
    }
}

// ── Per-View Rate Limiter ─────────────────────────────────────────

#[tokio::test]
async fn per_view_uses_global_when_no_override() {
    let global = Arc::new(RateLimiter::new(&RateLimitConfig {
        requests_per_minute: 60,
        burst_size: 2,
        ..Default::default()
    }));
    let pv = PerViewRateLimiter::new(global);

    // No view config → uses global
    match pv.check("client1", None, None).await {
        RateLimitResult::Allowed => {}
        _ => panic!("should use global and allow"),
    }
}

#[tokio::test]
async fn per_view_uses_override_when_configured() {
    let global = Arc::new(RateLimiter::new(&RateLimitConfig {
        requests_per_minute: 60,
        burst_size: 100, // generous global
        ..Default::default()
    }));
    let pv = PerViewRateLimiter::new(global);

    let view_cfg = ViewRateLimitConfig {
        rate_limit_per_minute: Some(60),
        rate_limit_burst_size: Some(1), // strict per-view
    };

    // First request allowed
    match pv.check("client1", Some("my_view"), Some(&view_cfg)).await {
        RateLimitResult::Allowed => {}
        _ => panic!("first request should be allowed"),
    }

    // Second request should be limited by per-view config
    match pv.check("client1", Some("my_view"), Some(&view_cfg)).await {
        RateLimitResult::Limited { .. } => {}
        _ => panic!("should be limited by per-view burst_size=1"),
    }
}

// ── Backpressure ──────────────────────────────────────────────────

#[tokio::test]
async fn backpressure_allows_within_capacity() {
    let state = BackpressureState::new(10, 100, true);

    let app = Router::new()
        .route("/test", get(|| async { "ok" }))
        .layer(axum_middleware::from_fn_with_state(
            state,
            riversd::backpressure::backpressure_middleware,
        ));

    let req = Request::builder()
        .uri("/test")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn backpressure_503_when_exhausted() {
    let state = BackpressureState::new(1, 1, true); // 1 permit, 1ms timeout

    // Acquire the only permit
    let _permit = state.semaphore.clone().acquire_owned().await.unwrap();

    let app = Router::new()
        .route("/test", get(|| async { "ok" }))
        .layer(axum_middleware::from_fn_with_state(
            state,
            riversd::backpressure::backpressure_middleware,
        ));

    let req = Request::builder()
        .uri("/test")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["message"], "server overloaded; retry later");
    assert_eq!(json["code"], 503);
}

#[tokio::test]
async fn backpressure_bypassed_when_disabled() {
    let state = BackpressureState::new(0, 1, false); // 0 permits but disabled

    let app = Router::new()
        .route("/test", get(|| async { "ok" }))
        .layer(axum_middleware::from_fn_with_state(
            state,
            riversd::backpressure::backpressure_middleware,
        ));

    let req = Request::builder()
        .uri("/test")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn backpressure_returns_retry_after_header() {
    let state = BackpressureState::new(1, 1, true);
    let _permit = state.semaphore.clone().acquire_owned().await.unwrap();

    let app = Router::new()
        .route("/test", get(|| async { "ok" }))
        .layer(axum_middleware::from_fn_with_state(
            state,
            riversd::backpressure::backpressure_middleware,
        ));

    let req = Request::builder()
        .uri("/test")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.headers().get("retry-after").unwrap().to_str().unwrap(),
        "1"
    );
}

// ── Config Defaults ───────────────────────────────────────────────

#[test]
fn rate_limit_config_defaults() {
    let config = RateLimitConfig::default();
    assert_eq!(config.requests_per_minute, 120);
    assert_eq!(config.burst_size, 60);
}

#[test]
fn backpressure_state_available_permits() {
    let state = BackpressureState::new(512, 100, true);
    assert_eq!(state.available_permits(), 512);
}
