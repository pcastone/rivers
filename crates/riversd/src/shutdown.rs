//! Graceful shutdown coordinator.
//!
//! Per `rivers-httpd-spec.md` §13.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

/// Coordinates graceful shutdown across the server.
///
/// Per spec §13.2:
/// - `draining` flag gates new request acceptance
/// - `inflight` counter tracks in-progress requests
/// - `notify` wakes the drain-wait loop when inflight drops
pub struct ShutdownCoordinator {
    draining: AtomicBool,
    inflight: AtomicUsize,
    notify: Notify,
}

impl ShutdownCoordinator {
    pub fn new() -> Self {
        Self {
            draining: AtomicBool::new(false),
            inflight: AtomicUsize::new(0),
            notify: Notify::new(),
        }
    }

    /// Enter drain mode — new requests will be rejected with 503.
    pub fn mark_draining(&self) {
        self.draining.store(true, Ordering::Release);
        tracing::info!("shutdown signal received; entering drain mode");
    }

    /// Check if the server is draining.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    /// Increment inflight request count. Returns the new count.
    pub fn enter(&self) -> usize {
        self.inflight.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement inflight request count and notify drain waiters.
    pub fn exit(&self) {
        self.inflight.fetch_sub(1, Ordering::AcqRel);
        self.notify.notify_waiters();
    }

    /// Current inflight request count.
    pub fn inflight_count(&self) -> usize {
        self.inflight.load(Ordering::Acquire)
    }

    /// Wait until all inflight requests complete.
    ///
    /// Per spec §13.4.
    pub async fn wait_for_drain(&self) {
        while self.inflight.load(Ordering::Acquire) > 0 {
            self.notify.notified().await;
        }
        tracing::info!("all inflight requests drained");
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}
