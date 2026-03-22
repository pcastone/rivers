//! Driver SDK type and contract tests.

use std::collections::HashMap;

use rivers_driver_sdk::{
    classify_operation, infer_operation, BrokerConsumerConfig, BrokerMetadata, BrokerSubscription,
    DriverError, FailureMode, FailurePolicy, InboundMessage, MessageReceipt, OperationCategory,
    OutboundMessage, PublishReceipt, Query, QueryResult, QueryValue, ABI_VERSION,
};

// ── QueryValue ──────────────────────────────────────────────────────

#[test]
fn query_value_variants() {
    let _ = QueryValue::Null;
    let _ = QueryValue::Boolean(true);
    let _ = QueryValue::Integer(42);
    let _ = QueryValue::Float(3.14);
    let _ = QueryValue::String("hello".into());
    let _ = QueryValue::Array(vec![QueryValue::Integer(1), QueryValue::Integer(2)]);
    let _ = QueryValue::Json(serde_json::json!({"key": "value"}));
}

#[test]
fn query_value_json_roundtrip() {
    let val = QueryValue::Integer(42);
    let json = serde_json::to_string(&val).unwrap();
    let back: QueryValue = serde_json::from_str(&json).unwrap();
    match back {
        QueryValue::Integer(n) => assert_eq!(n, 42),
        _ => panic!("expected Integer"),
    }
}

// ── Query ───────────────────────────────────────────────────────────

#[test]
fn query_new_infers_operation() {
    let q = Query::new("users", "SELECT * FROM users WHERE id = $1");
    assert_eq!(q.operation, "select");
    assert_eq!(q.target, "users");
    assert_eq!(q.statement, "SELECT * FROM users WHERE id = $1");
}

#[test]
fn query_infer_insert() {
    let q = Query::new("users", "INSERT INTO users (name) VALUES ($1)");
    assert_eq!(q.operation, "insert");
}

#[test]
fn query_infer_get() {
    let q = Query::new("cache_key", "GET cache_key");
    assert_eq!(q.operation, "get");
}

#[test]
fn query_infer_empty_statement() {
    let q = Query::new("target", "");
    assert_eq!(q.operation, "unknown");
}

#[test]
fn query_with_explicit_operation() {
    let q = Query::with_operation("ping", "health", "");
    assert_eq!(q.operation, "ping");
}

#[test]
fn query_param_chaining() {
    let q = Query::new("users", "SELECT * FROM users WHERE id = $1 AND name = $2")
        .param("id", QueryValue::Integer(42))
        .param("name", QueryValue::String("Alice".into()));
    assert_eq!(q.parameters.len(), 2);
}

#[test]
fn infer_operation_various() {
    assert_eq!(infer_operation("SELECT * FROM t"), "select");
    assert_eq!(infer_operation("insert into t"), "insert");
    assert_eq!(infer_operation("UPDATE t SET x=1"), "update");
    assert_eq!(infer_operation("DELETE FROM t"), "delete");
    assert_eq!(infer_operation("PING"), "ping");
    assert_eq!(infer_operation("XADD stream * field value"), "xadd");
    assert_eq!(infer_operation(""), "unknown");
    assert_eq!(infer_operation("   "), "unknown");
}

#[test]
fn infer_operation_strips_line_comments() {
    assert_eq!(infer_operation("-- fetch users\nSELECT * FROM users"), "select");
}

#[test]
fn infer_operation_strips_block_comments() {
    assert_eq!(infer_operation("/* admin query */ DELETE FROM sessions"), "delete");
}

#[test]
fn infer_operation_strips_leading_whitespace_and_comments() {
    assert_eq!(infer_operation("  \n  -- comment\n  INSERT INTO t"), "insert");
}

#[test]
fn infer_operation_json_with_operation_field() {
    assert_eq!(infer_operation(r#"{"operation":"find","selector":{}}"#), "find");
    assert_eq!(infer_operation(r#"{"operation":"insert","doc":{"name":"test"}}"#), "insert");
}

#[test]
fn infer_operation_json_without_operation_defaults_to_find() {
    assert_eq!(infer_operation(r#"{"selector":{}}"#), "find");
    assert_eq!(infer_operation(r#"{"index":"products","body":{"query":{"match_all":{}}}}"#), "find");
}

#[test]
fn infer_operation_malformed_json_falls_through() {
    // Starts with { but isn't valid JSON — falls through to first-token
    assert_eq!(infer_operation("{broken"), "{broken");
}

#[test]
fn classify_operation_categories() {
    assert_eq!(classify_operation("select"), OperationCategory::Read);
    assert_eq!(classify_operation("GET"), OperationCategory::Read);
    assert_eq!(classify_operation("find"), OperationCategory::Read);
    assert_eq!(classify_operation("insert"), OperationCategory::Write);
    assert_eq!(classify_operation("SET"), OperationCategory::Write);
    assert_eq!(classify_operation("xadd"), OperationCategory::Write);
    assert_eq!(classify_operation("delete"), OperationCategory::Delete);
    assert_eq!(classify_operation("DROP"), OperationCategory::Delete);
    assert_eq!(classify_operation("custom_op"), OperationCategory::Other);
}

// ── QueryResult ─────────────────────────────────────────────────────

#[test]
fn query_result_empty() {
    let r = QueryResult::empty();
    assert!(r.rows.is_empty());
    assert_eq!(r.affected_rows, 0);
    assert!(r.last_insert_id.is_none());
}

#[test]
fn query_result_with_rows() {
    let mut row = HashMap::new();
    row.insert("id".to_string(), QueryValue::Integer(1));
    row.insert("name".to_string(), QueryValue::String("Alice".into()));

    let r = QueryResult {
        rows: vec![row],
        affected_rows: 1,
        last_insert_id: None,
    };
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.affected_rows, 1);
}

#[test]
fn query_result_write_operation() {
    let r = QueryResult {
        rows: Vec::new(),
        affected_rows: 5,
        last_insert_id: Some("42".to_string()),
    };
    assert!(r.rows.is_empty());
    assert_eq!(r.affected_rows, 5);
    assert_eq!(r.last_insert_id.as_deref(), Some("42"));
}

// ── DriverError ─────────────────────────────────────────────────────

#[test]
fn driver_error_variants() {
    let errors = vec![
        DriverError::UnknownDriver("foo".into()),
        DriverError::Connection("refused".into()),
        DriverError::Query("syntax error".into()),
        DriverError::Transaction("deadlock".into()),
        DriverError::Unsupported("stream".into()),
        DriverError::NotImplemented("wasm exec".into()),
        DriverError::Internal("unexpected".into()),
    ];
    // All should format without panic
    for e in &errors {
        let _ = format!("{}", e);
    }
    assert_eq!(errors.len(), 7);
}

#[test]
fn driver_error_display() {
    let e = DriverError::UnknownDriver("neo4j".into());
    assert_eq!(format!("{}", e), "unknown driver: neo4j");
}

// ── ConnectionParams ────────────────────────────────────────────────

#[test]
fn connection_params_clone() {
    use rivers_driver_sdk::ConnectionParams;

    let params = ConnectionParams {
        host: "localhost".into(),
        port: 5432,
        database: "orders".into(),
        username: "admin".into(),
        password: "secret".into(),
        options: HashMap::new(),
    };
    let cloned = params.clone();
    assert_eq!(cloned.host, "localhost");
    assert_eq!(cloned.port, 5432);
}

// ── Broker Types ────────────────────────────────────────────────────

#[test]
fn inbound_message_construction() {
    let msg = InboundMessage {
        id: "msg-001".into(),
        destination: "orders.created".into(),
        payload: b"hello".to_vec(),
        headers: HashMap::from([("content-type".into(), "application/json".into())]),
        timestamp: chrono::Utc::now(),
        receipt: MessageReceipt {
            handle: "delivery-tag-1".into(),
        },
        metadata: BrokerMetadata::Kafka {
            partition: 0,
            offset: 42,
            consumer_group: "orders-api.orders".into(),
        },
    };
    assert_eq!(msg.id, "msg-001");
    assert_eq!(msg.destination, "orders.created");
    assert_eq!(msg.payload, b"hello");
}

#[test]
fn outbound_message_construction() {
    let msg = OutboundMessage {
        destination: "events.user.created".into(),
        payload: b"{\"user_id\": 1}".to_vec(),
        headers: HashMap::new(),
        key: Some("user-1".into()),
        reply_to: None,
    };
    assert_eq!(msg.destination, "events.user.created");
    assert!(msg.key.is_some());
    assert!(msg.reply_to.is_none());
}

#[test]
fn broker_metadata_variants() {
    let kafka = BrokerMetadata::Kafka {
        partition: 3,
        offset: 100,
        consumer_group: "my-group".into(),
    };
    let rabbit = BrokerMetadata::Rabbit {
        delivery_tag: 7,
        exchange: "amq.topic".into(),
        routing_key: "order.created".into(),
    };
    let nats = BrokerMetadata::Nats {
        sequence: 55,
        stream: "ORDERS".into(),
        consumer: "worker-1".into(),
    };
    let redis = BrokerMetadata::Redis {
        stream_id: "1234-0".into(),
        group: "processors".into(),
        consumer: "node-1".into(),
    };

    // All should debug-format without panic
    let _ = format!("{:?}", kafka);
    let _ = format!("{:?}", rabbit);
    let _ = format!("{:?}", nats);
    let _ = format!("{:?}", redis);
}

#[test]
fn broker_consumer_config_construction() {
    let config = BrokerConsumerConfig {
        group_prefix: "rivers".into(),
        app_id: "orders-api".into(),
        datasource_id: "kafka-main".into(),
        node_id: "node-1".into(),
        reconnect_ms: 5000,
        subscriptions: vec![
            BrokerSubscription {
                topic: "orders.created".into(),
                event_name: Some("OrderCreated".into()),
            },
            BrokerSubscription {
                topic: "orders.updated".into(),
                event_name: None,
            },
        ],
    };
    assert_eq!(config.subscriptions.len(), 2);
    assert_eq!(config.reconnect_ms, 5000);
}

#[test]
fn failure_mode_variants() {
    assert_eq!(FailureMode::DeadLetter, FailureMode::DeadLetter);
    assert_eq!(FailureMode::Requeue, FailureMode::Requeue);
    assert_eq!(FailureMode::Redirect, FailureMode::Redirect);
    assert_eq!(FailureMode::Drop, FailureMode::Drop);
    assert_ne!(FailureMode::DeadLetter, FailureMode::Drop);
}

#[test]
fn failure_policy_construction() {
    let policy = FailurePolicy {
        mode: FailureMode::DeadLetter,
        destination: Some("dead-letters".into()),
        handlers: vec![],
    };
    assert_eq!(policy.mode, FailureMode::DeadLetter);
    assert!(policy.destination.is_some());
}

#[test]
fn publish_receipt_construction() {
    let receipt = PublishReceipt {
        id: Some("msg-123".into()),
        metadata: None,
    };
    assert_eq!(receipt.id.as_deref(), Some("msg-123"));
}

#[test]
fn message_receipt_clone() {
    let receipt = MessageReceipt {
        handle: "tag-42".into(),
    };
    let cloned = receipt.clone();
    assert_eq!(cloned.handle, "tag-42");
}

// ── ABI Version ─────────────────────────────────────────────────────

#[test]
fn abi_version_is_set() {
    assert_eq!(ABI_VERSION, 1);
}

// ── Driver Contract (Phase H) ────────────────────────────────────

use rivers_driver_sdk::traits::{
    DriverType, HttpMethod, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

#[test]
fn driver_type_variants() {
    assert_ne!(DriverType::Database, DriverType::MessageBroker);
    assert_ne!(DriverType::Database, DriverType::Http);
    assert_ne!(DriverType::MessageBroker, DriverType::Http);
}

#[test]
fn http_method_display_and_from_str() {
    assert_eq!(HttpMethod::GET.as_str(), "GET");
    assert_eq!(HttpMethod::POST.as_str(), "POST");
    assert_eq!(HttpMethod::PUT.as_str(), "PUT");
    assert_eq!(HttpMethod::DELETE.as_str(), "DELETE");
    assert_eq!(format!("{}", HttpMethod::GET), "GET");
    assert_eq!(HttpMethod::from_str("get"), Some(HttpMethod::GET));
    assert_eq!(HttpMethod::from_str("POST"), Some(HttpMethod::POST));
    assert_eq!(HttpMethod::from_str("Put"), Some(HttpMethod::PUT));
    assert_eq!(HttpMethod::from_str("DELETE"), Some(HttpMethod::DELETE));
    assert_eq!(HttpMethod::from_str("PATCH"), None);
}

#[test]
fn validation_direction_display() {
    assert_eq!(format!("{}", ValidationDirection::Input), "Input");
    assert_eq!(format!("{}", ValidationDirection::Output), "Output");
}

#[test]
fn schema_definition_deserializes() {
    let json = serde_json::json!({
        "driver": "postgresql",
        "type": "object",
        "description": "Order record",
        "fields": [
            { "name": "id", "type": "uuid", "required": true },
            { "name": "amount", "type": "decimal", "required": true, "min": 0 }
        ]
    });
    let schema: SchemaDefinition = serde_json::from_value(json).unwrap();
    assert_eq!(schema.driver, "postgresql");
    assert_eq!(schema.schema_type, "object");
    assert_eq!(schema.fields.len(), 2);
    assert_eq!(schema.fields[0].name, "id");
    assert_eq!(schema.fields[0].field_type, "uuid");
    assert!(schema.fields[0].required);
    // min is in constraints
    assert_eq!(schema.fields[1].constraints.get("min"), Some(&serde_json::json!(0)));
}

#[test]
fn schema_definition_redis_deserializes() {
    let json = serde_json::json!({
        "driver": "redis",
        "type": "hash",
        "description": "User session",
        "key_pattern": "session:{session_id}",
        "fields": [
            { "name": "user_id", "type": "string", "required": true }
        ]
    });
    let schema: SchemaDefinition = serde_json::from_value(json).unwrap();
    assert_eq!(schema.driver, "redis");
    assert_eq!(schema.schema_type, "hash");
    assert_eq!(schema.extra.get("key_pattern"), Some(&serde_json::json!("session:{session_id}")));
}

#[test]
fn schema_syntax_error_display() {
    let err = SchemaSyntaxError::InvalidStructure {
        driver: "redis".into(),
        reason: "hash type cannot have 'fields' array".into(),
    };
    assert!(err.to_string().contains("redis"));
    assert!(err.to_string().contains("hash type"));
}

#[test]
fn validation_error_display() {
    let err = ValidationError::MissingRequired {
        field: "name".into(),
        direction: ValidationDirection::Input,
    };
    assert!(err.to_string().contains("name"));
    assert!(err.to_string().contains("Input"));

    let err = ValidationError::TypeMismatch {
        field: "age".into(),
        expected: "integer".into(),
        actual: "string".into(),
        direction: ValidationDirection::Output,
    };
    assert!(err.to_string().contains("age"));
    assert!(err.to_string().contains("integer"));
    assert!(err.to_string().contains("Output"));
}
