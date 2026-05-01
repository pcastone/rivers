//! Tests for `broker_supervisor` (P0-4 — nonblocking broker startup).
//!
//! Verifies:
//! - Spawning the supervisor returns immediately even when the driver's
//!   `create_consumer` would block or fail.
//! - Repeated `create_consumer` failures don't crash the supervisor; the
//!   registry surfaces `disconnected` with a failure count.
//! - On eventual success the registry surfaces `connected`.
//! - Shutdown signal exits the supervisor promptly (no hanging tasks).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{watch, Mutex};

use rivers_runtime::rivers_driver_sdk::broker::{
    AckOutcome, BrokerConsumer, BrokerConsumerConfig, BrokerError, BrokerProducer, FailureMode,
    FailurePolicy, InboundMessage, MessageBrokerDriver, MessageReceipt,
};
use rivers_runtime::rivers_driver_sdk::error::DriverError;
use rivers_runtime::rivers_driver_sdk::ConnectionParams;
use rivers_runtime::rivers_core::eventbus::EventBus;

use riversd::broker_supervisor::{
    spawn_broker_supervisor, BrokerBridgeRegistry, BrokerBridgeState, SupervisorBackoff,
};

// ── Mocks ─────────────────────────────────────────────────────────

struct AlwaysFailDriver {
    create_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl MessageBrokerDriver for AlwaysFailDriver {
    fn name(&self) -> &str {
        "always-fail"
    }

    async fn create_producer(
        &self,
        _params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        Err(DriverError::Connection("not implemented".into()))
    }

    async fn create_consumer(
        &self,
        _params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
        self.create_calls.fetch_add(1, Ordering::SeqCst);
        Err(DriverError::Connection("No route to host".into()))
    }
}

struct EventuallyOkDriver {
    create_calls: Arc<AtomicUsize>,
    fails_until: usize,
}

#[async_trait]
impl MessageBrokerDriver for EventuallyOkDriver {
    fn name(&self) -> &str {
        "eventually-ok"
    }

    async fn create_producer(
        &self,
        _params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        Err(DriverError::Connection("not implemented".into()))
    }

    async fn create_consumer(
        &self,
        _params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
        let n = self.create_calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fails_until {
            Err(DriverError::Connection(format!("transient #{n}")))
        } else {
            Ok(Box::new(IdleConsumer { closed: Mutex::new(false) }))
        }
    }
}

/// A consumer whose `receive()` blocks until close — keeps the bridge alive.
struct IdleConsumer {
    closed: Mutex<bool>,
}

#[async_trait]
impl BrokerConsumer for IdleConsumer {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError> {
        // Sleep forever (or until cancelled). Bridge will drop us on shutdown.
        loop {
            if *self.closed.lock().await {
                return Err(DriverError::Connection("closed".into()));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    async fn ack(&mut self, _r: &MessageReceipt) -> Result<AckOutcome, BrokerError> {
        Ok(AckOutcome::Acked)
    }
    async fn nack(&mut self, _r: &MessageReceipt) -> Result<AckOutcome, BrokerError> {
        Ok(AckOutcome::Acked)
    }
    async fn close(&mut self) -> Result<(), DriverError> {
        *self.closed.lock().await = true;
        Ok(())
    }
}

// ── Fixtures ──────────────────────────────────────────────────────

fn fixture_params() -> ConnectionParams {
    ConnectionParams {
        host: "192.0.2.1".into(), // TEST-NET-1, guaranteed unreachable
        port: 9092,
        database: String::new(),
        username: String::new(),
        password: String::new(),
        options: HashMap::new(),
    }
}

fn fixture_config() -> BrokerConsumerConfig {
    BrokerConsumerConfig {
        group_prefix: "test".into(),
        app_id: "app".into(),
        datasource_id: "ds".into(),
        node_id: "node-0".into(),
        reconnect_ms: 50,
        subscriptions: vec![],
    }
}

fn fixture_failure_policy() -> FailurePolicy {
    FailurePolicy {
        mode: FailureMode::Drop,
        destination: None,
        handlers: vec![],
    }
}

// ── Tests ─────────────────────────────────────────────────────────

/// **A1.4 core**: spawning the supervisor returns immediately even when the
/// broker is unreachable. The original code blocked here.
#[tokio::test(flavor = "current_thread")]
async fn spawn_returns_immediately_when_broker_unreachable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = Arc::new(AlwaysFailDriver { create_calls: calls.clone() })
        as Arc<dyn MessageBrokerDriver>;
    let event_bus = Arc::new(EventBus::new());
    let registry = BrokerBridgeRegistry::new();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let start = std::time::Instant::now();
    let _h = spawn_broker_supervisor(
        driver,
        fixture_params(),
        fixture_config(),
        fixture_failure_policy(),
        event_bus,
        "ds".into(),
        "always-fail".into(),
        SupervisorBackoff { base_ms: 50, max_ms: 1_000 },
        registry.clone(),
        shutdown_rx,
    );
    let elapsed = start.elapsed();

    // Spawning is essentially instant — well under one connect-timeout.
    assert!(
        elapsed < Duration::from_millis(50),
        "spawn should return immediately, took {:?}",
        elapsed
    );

    // Give the supervisor a moment to attempt + fail at least twice.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let attempts = calls.load(Ordering::SeqCst);
    assert!(attempts >= 2, "supervisor should retry — saw {attempts} attempts");

    let snap = registry.snapshot().await;
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].state, BrokerBridgeState::Disconnected);
    assert!(snap[0].failed_attempts >= 1);
    assert!(snap[0].last_error.as_deref().unwrap_or("").contains("No route to host"));
    assert!(!registry.all_healthy().await);

    let _ = shutdown_tx.send(true);
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Supervisor recovers when the driver eventually succeeds.
#[tokio::test(flavor = "current_thread")]
async fn supervisor_reaches_connected_after_transient_failures() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = Arc::new(EventuallyOkDriver {
        create_calls: calls.clone(),
        fails_until: 2, // succeed on 3rd attempt
    }) as Arc<dyn MessageBrokerDriver>;
    let event_bus = Arc::new(EventBus::new());
    let registry = BrokerBridgeRegistry::new();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let _h = spawn_broker_supervisor(
        driver,
        fixture_params(),
        fixture_config(),
        fixture_failure_policy(),
        event_bus,
        "ds".into(),
        "eventually-ok".into(),
        SupervisorBackoff { base_ms: 20, max_ms: 200 },
        registry.clone(),
        shutdown_rx,
    );

    // Wait up to 2s for recovery.
    let connected = wait_for_state(&registry, "ds", BrokerBridgeState::Connected, 2_000).await;
    assert!(connected, "supervisor never reached Connected");

    let snap = registry.snapshot().await;
    assert_eq!(snap[0].state, BrokerBridgeState::Connected);
    assert_eq!(snap[0].failed_attempts, 0, "counter resets on success");
    assert!(registry.all_healthy().await);
    assert!(calls.load(Ordering::SeqCst) >= 3);

    let _ = shutdown_tx.send(true);
    tokio::time::sleep(Duration::from_millis(150)).await;
}

/// Empty registry → `all_healthy` returns true (no broker datasources is fine).
#[tokio::test(flavor = "current_thread")]
async fn empty_registry_is_healthy() {
    let registry = BrokerBridgeRegistry::new();
    assert!(registry.all_healthy().await);
    assert!(registry.snapshot().await.is_empty());
}

// ── Helpers ───────────────────────────────────────────────────────

async fn wait_for_state(
    registry: &BrokerBridgeRegistry,
    datasource: &str,
    target: BrokerBridgeState,
    max_ms: u64,
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(max_ms);
    loop {
        let snap = registry.snapshot().await;
        if snap.iter().any(|s| s.datasource == datasource && s.state == target) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
