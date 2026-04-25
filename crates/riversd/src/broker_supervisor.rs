//! Broker consumer supervisor — nonblocking startup + auto-reconnect.
//!
//! Per code review P0-4: bundle wiring must NOT block on
//! `MessageBrokerDriver::create_consumer().await`. The supervisor owns the
//! retry loop so HTTP listener bind proceeds even when brokers are
//! unreachable. Once a consumer is created, the existing
//! `BrokerConsumerBridge` runs the receive loop. When the bridge exits
//! (consumer error, broker disconnect), the supervisor reconnects with
//! bounded backoff.
//!
//! Status is published into a shared [`BrokerBridgeRegistry`] so
//! `/health/verbose` can surface broker degradation distinct from process
//! readiness.
//!
//! See `docs/code_review.md` finding P0-4 and `todo/tasks.md` Phase A1.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, RwLock};

use rivers_runtime::rivers_core::eventbus::EventBus;
use rivers_runtime::rivers_driver_sdk::broker::{
    BrokerConsumerConfig, FailurePolicy, MessageBrokerDriver,
};
use rivers_runtime::rivers_driver_sdk::ConnectionParams;

use crate::broker_bridge::BrokerConsumerBridge;

/// Per-bridge connection state surfaced by `/health/verbose`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerBridgeState {
    /// Initial state — supervisor spawned, connect not yet attempted.
    Pending,
    /// Currently attempting to create the consumer (or backing off).
    Connecting,
    /// Bridge is running successfully.
    Connected,
    /// Last connect attempt failed; supervisor is backing off.
    Disconnected,
    /// Supervisor has exited (shutdown received).
    Stopped,
}

impl BrokerBridgeState {
    /// Stable lowercase identifier for serialization (e.g. health output).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Stopped => "stopped",
        }
    }
}

/// A single bridge's status snapshot.
#[derive(Debug, Clone)]
pub struct BrokerBridgeStatus {
    /// Datasource name (matches `[data.datasources.<name>]`).
    pub datasource: String,
    /// Broker driver name (e.g. `kafka`, `rabbitmq`, `nats`).
    pub driver: String,
    /// Current connection state.
    pub state: BrokerBridgeState,
    /// Last error string, if the most recent connect or run attempt failed.
    pub last_error: Option<String>,
    /// Number of consecutive failed connect attempts (resets on success).
    pub failed_attempts: u32,
}

/// Shared registry of broker bridge states.
///
/// Populated by `spawn_broker_supervisor` and read by health handlers.
/// Cheap to clone (one Arc).
#[derive(Debug, Clone, Default)]
pub struct BrokerBridgeRegistry {
    inner: Arc<RwLock<HashMap<String, BrokerBridgeStatus>>>,
}

impl BrokerBridgeRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot all bridge statuses, sorted by datasource name for stable output.
    pub async fn snapshot(&self) -> Vec<BrokerBridgeStatus> {
        let guard = self.inner.read().await;
        let mut out: Vec<_> = guard.values().cloned().collect();
        out.sort_by(|a, b| a.datasource.cmp(&b.datasource));
        out
    }

    /// Whether every registered bridge is in `Connected` state.
    /// Returns true when the registry is empty.
    pub async fn all_healthy(&self) -> bool {
        let guard = self.inner.read().await;
        guard.values().all(|s| s.state == BrokerBridgeState::Connected)
    }

    async fn upsert(
        &self,
        datasource: &str,
        driver: &str,
        state: BrokerBridgeState,
        last_error: Option<String>,
        failed_attempts: u32,
    ) {
        let mut guard = self.inner.write().await;
        guard.insert(
            datasource.to_string(),
            BrokerBridgeStatus {
                datasource: datasource.to_string(),
                driver: driver.to_string(),
                state,
                last_error,
                failed_attempts,
            },
        );
    }
}

/// Tunables for the supervisor's reconnect loop.
///
/// Defaults: base = configured `reconnect_ms`, max cap at 60s, exponential
/// doubling per consecutive failure. The cap prevents indefinite drift to
/// hour-long delays under sustained outage.
#[derive(Debug, Clone, Copy)]
pub struct SupervisorBackoff {
    /// First retry delay (also the floor for subsequent retries).
    pub base_ms: u64,
    /// Cap on retry delay; supervisor never sleeps longer than this.
    pub max_ms: u64,
}

impl SupervisorBackoff {
    /// Build from a configured `reconnect_ms`. Floor at 100ms, cap at 60s.
    pub fn from_reconnect_ms(reconnect_ms: u64) -> Self {
        Self {
            base_ms: reconnect_ms.max(100),
            max_ms: 60_000,
        }
    }

    /// Compute delay for the Nth consecutive failure (0-indexed).
    /// Doubles up to `max_ms`.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let shift = attempt.min(20); // saturate doubling at 2^20
        let multiplied = self.base_ms.saturating_mul(1u64 << shift);
        Duration::from_millis(multiplied.min(self.max_ms))
    }
}

/// Spawn a supervisor task that owns the consumer-create + bridge lifecycle.
///
/// Returns immediately. The task runs until `shutdown_rx` flips to true.
///
/// # Behavior
/// - On entry: registers `Pending` in the registry.
/// - Loop:
///   1. Mark `Connecting`. Call `driver.create_consumer(...)`.
///   2. On success: mark `Connected`, build `BrokerConsumerBridge`, run it
///      to completion. When it exits, mark `Disconnected` and back off.
///   3. On failure: mark `Disconnected`, increment failure count, sleep
///      with exponential backoff capped at `SupervisorBackoff::max_ms`.
///   4. Shutdown signal exits the loop at any wait point.
#[allow(clippy::too_many_arguments)]
pub fn spawn_broker_supervisor(
    driver: Arc<dyn MessageBrokerDriver>,
    params: ConnectionParams,
    broker_config: BrokerConsumerConfig,
    failure_policy: FailurePolicy,
    event_bus: Arc<EventBus>,
    datasource_name: String,
    driver_name: String,
    backoff: SupervisorBackoff,
    registry: BrokerBridgeRegistry,
    mut shutdown_rx: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        registry
            .upsert(&datasource_name, &driver_name, BrokerBridgeState::Pending, None, 0)
            .await;

        let mut attempt: u32 = 0;

        loop {
            if *shutdown_rx.borrow() {
                registry
                    .upsert(
                        &datasource_name,
                        &driver_name,
                        BrokerBridgeState::Stopped,
                        None,
                        attempt,
                    )
                    .await;
                return;
            }

            registry
                .upsert(
                    &datasource_name,
                    &driver_name,
                    BrokerBridgeState::Connecting,
                    None,
                    attempt,
                )
                .await;

            match driver.create_consumer(&params, &broker_config).await {
                Ok(consumer) => {
                    attempt = 0;
                    registry
                        .upsert(
                            &datasource_name,
                            &driver_name,
                            BrokerBridgeState::Connected,
                            None,
                            0,
                        )
                        .await;

                    tracing::info!(
                        datasource = %datasource_name,
                        driver = %driver_name,
                        "broker consumer created — bridge running"
                    );

                    let bridge = BrokerConsumerBridge::new(
                        consumer,
                        event_bus.clone(),
                        failure_policy.clone(),
                        &datasource_name,
                        backoff.base_ms,
                        shutdown_rx.clone(),
                    );

                    bridge.run().await;

                    if *shutdown_rx.borrow() {
                        registry
                            .upsert(
                                &datasource_name,
                                &driver_name,
                                BrokerBridgeState::Stopped,
                                None,
                                0,
                            )
                            .await;
                        return;
                    }

                    registry
                        .upsert(
                            &datasource_name,
                            &driver_name,
                            BrokerBridgeState::Disconnected,
                            Some("bridge exited unexpectedly".to_string()),
                            attempt,
                        )
                        .await;
                }
                Err(e) => {
                    let err_str = e.to_string();
                    registry
                        .upsert(
                            &datasource_name,
                            &driver_name,
                            BrokerBridgeState::Disconnected,
                            Some(err_str.clone()),
                            attempt + 1,
                        )
                        .await;

                    let delay = backoff.delay_for(attempt);
                    tracing::warn!(
                        datasource = %datasource_name,
                        driver = %driver_name,
                        error = %err_str,
                        attempt = attempt + 1,
                        retry_in_ms = delay.as_millis() as u64,
                        "broker consumer create failed — retrying"
                    );

                    attempt = attempt.saturating_add(1);

                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = shutdown_rx.changed() => {
                            registry
                                .upsert(
                                    &datasource_name,
                                    &driver_name,
                                    BrokerBridgeState::Stopped,
                                    Some(err_str),
                                    attempt,
                                )
                                .await;
                            return;
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_then_caps() {
        let b = SupervisorBackoff { base_ms: 100, max_ms: 1_000 };
        assert_eq!(b.delay_for(0), Duration::from_millis(100));
        assert_eq!(b.delay_for(1), Duration::from_millis(200));
        assert_eq!(b.delay_for(2), Duration::from_millis(400));
        assert_eq!(b.delay_for(3), Duration::from_millis(800));
        // capped
        assert_eq!(b.delay_for(4), Duration::from_millis(1_000));
        assert_eq!(b.delay_for(20), Duration::from_millis(1_000));
    }

    #[test]
    fn backoff_from_reconnect_ms_floors_base() {
        let b = SupervisorBackoff::from_reconnect_ms(0);
        assert_eq!(b.base_ms, 100);
        let b = SupervisorBackoff::from_reconnect_ms(2_000);
        assert_eq!(b.base_ms, 2_000);
        assert_eq!(b.max_ms, 60_000);
    }
}
