//! DriverFactory tests — registration, lookup, plugin load results.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rivers_core::driver_factory::{DriverFactory, DriverRegistrar, PluginLoadResult};
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, MessageBrokerDriver,
    BrokerConsumer, BrokerConsumerConfig, BrokerProducer, Query, QueryResult,
};

// ── Test Driver Implementations ─────────────────────────────────────

struct TestDriver {
    driver_name: String,
}

struct TestConnection {
    driver_name: String,
}

#[async_trait]
impl DatabaseDriver for TestDriver {
    fn name(&self) -> &str {
        &self.driver_name
    }
    async fn connect(&self, _params: &ConnectionParams) -> Result<Box<dyn Connection>, DriverError> {
        Ok(Box::new(TestConnection {
            driver_name: self.driver_name.clone(),
        }))
    }
}

#[async_trait]
impl Connection for TestConnection {
    async fn execute(&mut self, _query: &Query) -> Result<QueryResult, DriverError> {
        Ok(QueryResult::empty())
    }
    async fn ping(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn driver_name(&self) -> &str {
        &self.driver_name
    }
}

struct TestBrokerDriver;

#[async_trait]
impl MessageBrokerDriver for TestBrokerDriver {
    fn name(&self) -> &str {
        "test-broker"
    }
    async fn create_producer(
        &self,
        _params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        Err(DriverError::NotImplemented("test stub".into()))
    }
    async fn create_consumer(
        &self,
        _params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
        Err(DriverError::NotImplemented("test stub".into()))
    }
}

fn test_params() -> ConnectionParams {
    ConnectionParams {
        host: "localhost".into(),
        port: 5432,
        database: "test".into(),
        username: "admin".into(),
        password: "secret".into(),
        options: HashMap::new(),
    }
}

// ── Factory Registration ────────────────────────────────────────────

#[test]
fn empty_factory() {
    let factory = DriverFactory::new();
    assert_eq!(factory.total_count(), 0);
    assert!(factory.driver_names().is_empty());
    assert!(factory.broker_driver_names().is_empty());
}

#[test]
fn register_database_driver() {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "postgres".into(),
    }));
    assert_eq!(factory.driver_names(), vec!["postgres"]);
    assert_eq!(factory.total_count(), 1);
}

#[test]
fn register_multiple_drivers() {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "postgres".into(),
    }));
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "mysql".into(),
    }));
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "faker".into(),
    }));
    assert_eq!(factory.driver_names(), vec!["faker", "mysql", "postgres"]);
    assert_eq!(factory.total_count(), 3);
}

#[test]
fn register_broker_driver() {
    let mut factory = DriverFactory::new();
    factory.register_broker_driver(Arc::new(TestBrokerDriver));
    assert_eq!(factory.broker_driver_names(), vec!["test-broker"]);
    assert_eq!(factory.total_count(), 1);
}

#[test]
fn register_both_types() {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "redis".into(),
    }));
    factory.register_broker_driver(Arc::new(TestBrokerDriver));
    assert_eq!(factory.total_count(), 2);
    assert_eq!(factory.driver_names(), vec!["redis"]);
    assert_eq!(factory.broker_driver_names(), vec!["test-broker"]);
}

// ── Driver Lookup ───────────────────────────────────────────────────

#[test]
fn get_driver_found() {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "postgres".into(),
    }));
    assert!(factory.get_driver("postgres").is_some());
}

#[test]
fn get_driver_not_found() {
    let factory = DriverFactory::new();
    assert!(factory.get_driver("neo4j").is_none());
}

#[test]
fn get_broker_driver_found() {
    let mut factory = DriverFactory::new();
    factory.register_broker_driver(Arc::new(TestBrokerDriver));
    assert!(factory.get_broker_driver("test-broker").is_some());
}

// ── Connect ─────────────────────────────────────────────────────────

#[tokio::test]
async fn connect_success() {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "postgres".into(),
    }));
    let conn = factory.connect("postgres", &test_params()).await;
    assert!(conn.is_ok());
    assert_eq!(conn.unwrap().driver_name(), "postgres");
}

#[tokio::test]
async fn connect_unknown_driver() {
    let factory = DriverFactory::new();
    let result = factory.connect("neo4j", &test_params()).await;
    match result {
        Err(DriverError::UnknownDriver(name)) => assert_eq!(name, "neo4j"),
        Err(e) => panic!("expected UnknownDriver, got: {}", e),
        Ok(_) => panic!("expected error"),
    }
}

// ── DriverRegistrar Trait ───────────────────────────────────────────

#[test]
fn registrar_trait_on_factory() {
    let mut factory = DriverFactory::new();
    let registrar: &mut dyn DriverRegistrar = &mut factory;

    registrar.register_database_driver(Arc::new(TestDriver {
        driver_name: "custom".into(),
    }));
    registrar.register_broker_driver(Arc::new(TestBrokerDriver));

    assert_eq!(factory.total_count(), 2);
}

// ── Driver name override ────────────────────────────────────────────

#[test]
fn duplicate_name_replaces() {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "postgres".into(),
    }));
    factory.register_database_driver(Arc::new(TestDriver {
        driver_name: "postgres".into(),
    }));
    // Second registration replaces first
    assert_eq!(factory.driver_names(), vec!["postgres"]);
    assert_eq!(factory.total_count(), 1);
}

// ── Plugin Load Results ─────────────────────────────────────────────

#[test]
fn plugin_load_result_success() {
    let result = PluginLoadResult::Success {
        path: "/usr/lib/rivers/plugins/neo4j.so".into(),
        driver_names: vec!["neo4j".into()],
    };
    let _ = format!("{:?}", result);
}

#[test]
fn plugin_load_result_failed() {
    let result = PluginLoadResult::Failed {
        path: "/usr/lib/rivers/plugins/bad.so".into(),
        reason: "ABI version mismatch: expected 1, got 2".into(),
    };
    let _ = format!("{:?}", result);
}

// ── Default trait ───────────────────────────────────────────────────

#[test]
fn factory_default() {
    let factory = DriverFactory::default();
    assert_eq!(factory.total_count(), 0);
}
