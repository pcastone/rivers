//! EventBusDriver — built-in driver that reads/writes via the in-process EventBus.
//!
//! Write operations (`publish`, `insert`, `set`, `xadd`) publish events to
//! EventBus topics. Read operations return empty results (EventBus is pub/sub,
//! not queryable). `ping` always succeeds.
//!
//! The driver is registered in DriverFactory at startup alongside other
//! built-in drivers.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverError, DriverType, HttpMethod,
    Query, QueryResult, QueryValue, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

/// Callback type for publishing events to the EventBus from the driver.
///
/// Parameters: (topic: String, payload: serde_json::Value)
/// Higher layers wire this to `EventBus::publish()`.
pub type EventBusPublisher = Arc<dyn Fn(String, serde_json::Value) + Send + Sync>;

/// Built-in driver that bridges DataView queries to the EventBus.
///
/// Write operations publish events; read operations return empty results.
pub struct EventBusDriver;

impl EventBusDriver {
    /// Create a new EventBus driver instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for EventBusDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseDriver for EventBusDriver {
    fn name(&self) -> &str {
        "eventbus"
    }

    async fn connect(
        &self,
        _params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        // EventBus connections are lightweight — no real connection to establish.
        // The publisher callback is set after creation via `set_publisher()`.
        Ok(Box::new(EventBusConnection {
            publisher: Arc::new(Mutex::new(None)),
        }))
    }
}

// ---------------------------------------------------------------------------
// Unified Driver trait implementation (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

#[async_trait]
impl Driver for EventBusDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "eventbus"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        // type must be "event"
        if schema.schema_type != "event" {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "eventbus".into(),
                supported: vec!["event".into()],
                schema_file: String::new(),
            });
        }
        // topic required
        if !schema.extra.contains_key("topic") {
            return Err(SchemaSyntaxError::MissingRequiredField {
                field: "topic".into(),
                driver: "eventbus".into(),
                schema_file: String::new(),
            });
        }
        // PUT/DELETE not supported
        if method == HttpMethod::PUT || method == HttpMethod::DELETE {
            return Err(SchemaSyntaxError::UnsupportedMethod {
                method: method.to_string(),
                driver: "eventbus".into(),
                schema_file: String::new(),
            });
        }
        Ok(())
    }

    fn validate(
        &self,
        data: &serde_json::Value,
        schema: &SchemaDefinition,
        direction: ValidationDirection,
    ) -> Result<(), ValidationError> {
        rivers_driver_sdk::validation::validate_fields(data, schema, direction)
    }

    async fn execute(
        &self,
        _query: &Query,
        _params: &std::collections::HashMap<String, QueryValue>,
    ) -> Result<QueryResult, DriverError> {
        Err(DriverError::NotImplemented(
            "use DatabaseDriver::connect() + Connection::execute() for EventBus".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // EventBusDriver is stateless
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}

/// A connection to the EventBus driver.
///
/// Write operations extract topic from `query.target` and payload from
/// parameters, then publish to the EventBus. Read operations return empty.
pub struct EventBusConnection {
    publisher: Arc<Mutex<Option<EventBusPublisher>>>,
}

impl EventBusConnection {
    /// Create a connection pre-wired with a publisher callback.
    pub fn with_publisher(publisher: EventBusPublisher) -> Self {
        Self {
            publisher: Arc::new(Mutex::new(Some(publisher))),
        }
    }

    /// Set the publisher callback after creation.
    pub async fn set_publisher(&self, publisher: EventBusPublisher) {
        *self.publisher.lock().await = Some(publisher);
    }

    /// Build a JSON payload from query parameters for publishing.
    ///
    /// Delegates to `QueryValue`'s threshold-aware `Serialize` impl (H18.1)
    /// so large integers (`|v| > 2⁵³−1`) are stringified rather than emitted
    /// as JSON numbers that JS consumers would silently round.
    fn build_payload(query: &Query) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (k, v) in &query.parameters {
            let json_val = serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
            map.insert(k.clone(), json_val);
        }
        serde_json::Value::Object(map)
    }
}

#[async_trait]
impl Connection for EventBusConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        match query.operation.as_str() {
            // Write operations: publish to EventBus topic
            "publish" | "insert" | "set" | "xadd" | "create" => {
                let topic = &query.target;
                let payload = Self::build_payload(query);

                let publisher = self.publisher.lock().await;
                if let Some(ref publish_fn) = *publisher {
                    publish_fn(topic.clone(), payload);
                    Ok(QueryResult {
                        rows: Vec::new(),
                        affected_rows: 1,
                        last_insert_id: None,
                        column_names: None,
                    })
                } else {
                    Err(DriverError::Internal(
                        "eventbus publisher not configured".to_string(),
                    ))
                }
            }

            // Read operations: EventBus is pub/sub, not queryable
            "select" | "get" | "find" | "query" | "scan" => Ok(QueryResult::empty()),

            // Ping always succeeds
            "ping" => Ok(QueryResult::empty()),

            op => Err(DriverError::Unsupported(format!(
                "eventbus driver does not support operation: {}",
                op
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "eventbus"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection};

    fn make_schema_with_extra(
        schema_type: &str,
        fields: Vec<SchemaFieldDef>,
        extra_pairs: Vec<(&str, serde_json::Value)>,
    ) -> SchemaDefinition {
        let mut extra = HashMap::new();
        for (k, v) in extra_pairs {
            extra.insert(k.to_string(), v);
        }
        SchemaDefinition {
            driver: "eventbus".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra,
        }
    }

    fn make_valid_schema() -> SchemaDefinition {
        make_schema_with_extra(
            "event",
            vec![
                SchemaFieldDef {
                    name: "message".into(),
                    field_type: "string".into(),
                    required: true,
                    constraints: HashMap::new(),
                },
            ],
            vec![("topic", serde_json::json!("my_topic"))],
        )
    }

    #[test]
    fn schema_syntax_valid() {
        let driver = EventBusDriver::new();
        let schema = make_valid_schema();
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_valid_post() {
        let driver = EventBusDriver::new();
        let schema = make_valid_schema();
        assert!(driver.check_schema_syntax(&schema, HttpMethod::POST).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_non_event_type() {
        let driver = EventBusDriver::new();
        let schema = make_schema_with_extra(
            "object",
            vec![],
            vec![("topic", serde_json::json!("t"))],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedType { .. }));
    }

    #[test]
    fn schema_syntax_requires_topic() {
        let driver = EventBusDriver::new();
        let schema = make_schema_with_extra("event", vec![], vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "topic"));
    }

    #[test]
    fn schema_syntax_rejects_put_method() {
        let driver = EventBusDriver::new();
        let schema = make_valid_schema();
        let err = driver.check_schema_syntax(&schema, HttpMethod::PUT).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedMethod { .. }));
    }

    #[test]
    fn schema_syntax_rejects_delete_method() {
        let driver = EventBusDriver::new();
        let schema = make_valid_schema();
        let err = driver.check_schema_syntax(&schema, HttpMethod::DELETE).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedMethod { .. }));
    }

    #[test]
    fn validate_accepts_valid_data() {
        let driver = EventBusDriver::new();
        let schema = make_valid_schema();
        let data = serde_json::json!({"message": "hello world"});
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        let driver = EventBusDriver::new();
        let schema = make_valid_schema();
        let data = serde_json::json!({"other": "value"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRequired { ref field, .. } if field == "message"));
    }

    #[tokio::test]
    async fn test_eventbus_driver_name() {
        let driver = EventBusDriver::new();
        assert_eq!(DatabaseDriver::name(&driver), "eventbus");
    }

    #[tokio::test]
    async fn test_eventbus_connect_succeeds() {
        let driver = EventBusDriver::new();
        let params = ConnectionParams {
            host: String::new(),
            port: 0,
            database: String::new(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        };
        let conn = driver.connect(&params).await;
        assert!(conn.is_ok());
    }

    #[tokio::test]
    async fn test_eventbus_write_publishes_event() {
        let published: Arc<StdMutex<Vec<(String, serde_json::Value)>>> =
            Arc::new(StdMutex::new(Vec::new()));

        let published_clone = published.clone();
        let publisher: EventBusPublisher = Arc::new(move |topic, payload| {
            published_clone.lock().unwrap().push((topic, payload));
        });

        let mut conn = EventBusConnection::with_publisher(publisher);

        let query = Query::with_operation("publish", "my_topic", "")
            .param("message", QueryValue::String("hello".into()))
            .param("severity", QueryValue::Integer(1));

        let result = conn.execute(&query).await.unwrap();
        assert_eq!(result.affected_rows, 1);

        let events = published.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "my_topic");
        assert_eq!(events[0].1["message"], "hello");
        assert_eq!(events[0].1["severity"], 1);
    }

    #[tokio::test]
    async fn test_eventbus_read_returns_empty() {
        let publisher: EventBusPublisher = Arc::new(|_, _| {});
        let mut conn = EventBusConnection::with_publisher(publisher);

        let query = Query::with_operation("select", "some_topic", "");
        let result = conn.execute(&query).await.unwrap();
        assert!(result.rows.is_empty());
        assert_eq!(result.affected_rows, 0);
    }

    #[tokio::test]
    async fn test_eventbus_ping_succeeds() {
        let publisher: EventBusPublisher = Arc::new(|_, _| {});
        let mut conn = EventBusConnection::with_publisher(publisher);
        assert!(conn.ping().await.is_ok());
    }

    #[tokio::test]
    async fn test_eventbus_write_without_publisher_fails() {
        let mut conn = EventBusConnection {
            publisher: Arc::new(Mutex::new(None)),
        };

        let query = Query::with_operation("publish", "topic", "");
        let result = conn.execute(&query).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("publisher not configured"));
    }

    #[tokio::test]
    async fn test_eventbus_unsupported_operation() {
        let publisher: EventBusPublisher = Arc::new(|_, _| {});
        let mut conn = EventBusConnection::with_publisher(publisher);

        let query = Query::with_operation("truncate", "topic", "");
        let result = conn.execute(&query).await;
        assert!(result.is_err());
    }
}
