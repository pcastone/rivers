//! Circuit breaker implementation for HTTP connections.
//!
//! Implements the standard Closed -> Open -> Half-Open -> Closed model
//! to prevent cascading failures when a downstream service is unhealthy.

use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::http_driver::{CircuitBreakerConfig, HttpDriverError};

/// Circuit breaker states per the standard Closed -> Open -> Half-Open -> Closed model.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CircuitState {
    /// Current flows freely through the channel — all requests pass downstream.
    Closed,
    /// Overflow has broken the banks; no requests cross until the waters recede.
    Open { opened_at: Instant },
    /// Waters tested with careful probes before the full current is restored.
    HalfOpen { successes: u32 },
}

/// Internal circuit breaker tracking.
#[derive(Debug)]
pub(crate) struct CircuitBreaker {
    pub(crate) config: CircuitBreakerConfig,
    pub(crate) state: CircuitState,
    /// Recent failure timestamps within the rolling window.
    pub(crate) failures: Vec<Instant>,
}

impl CircuitBreaker {
    pub(crate) fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: CircuitState::Closed,
            failures: Vec::new(),
        }
    }

    /// Check if a request is allowed. Returns Err if circuit is open.
    pub(crate) fn check(&mut self) -> Result<(), HttpDriverError> {
        match &self.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open { opened_at } => {
                let elapsed = opened_at.elapsed().as_millis() as u64;
                if elapsed >= self.config.open_duration_ms {
                    // Transition to half-open
                    self.state = CircuitState::HalfOpen { successes: 0 };
                    debug!("circuit breaker transitioning to half-open");
                    Ok(())
                } else {
                    Err(HttpDriverError::CircuitOpen)
                }
            }
            CircuitState::HalfOpen { .. } => Ok(()),
        }
    }

    /// Record a successful request.
    pub(crate) fn record_success(&mut self) {
        match &self.state {
            CircuitState::HalfOpen { successes } => {
                let new_successes = successes + 1;
                if new_successes >= self.config.half_open_attempts {
                    debug!("circuit breaker closing after {} successful probes", new_successes);
                    self.state = CircuitState::Closed;
                    self.failures.clear();
                } else {
                    self.state = CircuitState::HalfOpen {
                        successes: new_successes,
                    };
                }
            }
            CircuitState::Closed => {
                // Prune old failures outside the window
                self.prune_old_failures();
            }
            _ => {}
        }
    }

    /// Record a failed request.
    pub(crate) fn record_failure(&mut self) {
        let now = Instant::now();
        match &self.state {
            CircuitState::Closed => {
                self.failures.push(now);
                self.prune_old_failures();
                if self.failures.len() as u32 >= self.config.failure_threshold {
                    warn!(
                        "circuit breaker opening after {} failures in window",
                        self.failures.len()
                    );
                    self.state = CircuitState::Open { opened_at: now };
                }
            }
            CircuitState::HalfOpen { .. } => {
                // Any failure in half-open re-opens the circuit
                warn!("circuit breaker re-opening from half-open state");
                self.state = CircuitState::Open { opened_at: now };
            }
            _ => {}
        }
    }

    fn prune_old_failures(&mut self) {
        let cutoff = Instant::now() - Duration::from_millis(self.config.window_ms);
        self.failures.retain(|t| *t > cutoff);
    }
}
