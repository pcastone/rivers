//! Semaphore-based backpressure.
//!
//! Per `rivers-httpd-spec.md` §11.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use tokio::sync::Semaphore;

use crate::error_response;

/// Backpressure state shared across requests.
#[derive(Clone)]
pub struct BackpressureState {
    /// Semaphore with `queue_depth` permits.
    pub semaphore: Arc<Semaphore>,
    /// Timeout for acquiring a permit.
    pub queue_timeout: Duration,
    /// Whether backpressure is enabled.
    pub enabled: bool,
}

impl BackpressureState {
    /// Create a new backpressure state.
    pub fn new(queue_depth: usize, queue_timeout_ms: u64, enabled: bool) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(queue_depth)),
            queue_timeout: Duration::from_millis(queue_timeout_ms),
            enabled,
        }
    }

    /// Number of available permits.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// Backpressure middleware.
///
/// Per spec §11.1: semaphore-based request queue.
/// Timeout or semaphore closed → 503 Service Unavailable.
pub async fn backpressure_middleware(
    State(state): State<BackpressureState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !state.enabled {
        return next.run(request).await;
    }

    let trace_id = crate::middleware::extract_trace_id(&request);

    // Try to acquire a permit within the timeout
    match tokio::time::timeout(state.queue_timeout, state.semaphore.clone().acquire_owned()).await {
        Ok(Ok(permit)) => {
            let response = next.run(request).await;
            // Permit is dropped here, releasing the slot
            drop(permit);
            response
        }
        Ok(Err(_closed)) => {
            // Semaphore closed — should not happen in normal operation
            overloaded_response(trace_id)
        }
        Err(_timeout) => {
            // Timeout waiting for a permit
            overloaded_response(trace_id)
        }
    }
}

/// 503 response for backpressure exhaustion.
///
/// Per spec §11.3.
fn overloaded_response(trace_id: Option<String>) -> Response {
    let mut err = error_response::service_unavailable("server overloaded; retry later");
    if let Some(id) = trace_id {
        err = err.with_trace_id(id);
    }
    let mut response = err.into_axum_response();
    response
        .headers_mut()
        .insert("retry-after", HeaderValue::from_static("1"));
    response
}
