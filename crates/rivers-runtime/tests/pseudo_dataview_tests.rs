//! Tests for pseudo DataView builder.

use rivers_runtime::pseudo_dataview::{DatasourceBuilder, PseudoDataViewError};

#[test]
fn builder_from_query_builds_successfully() {
    let pdv = DatasourceBuilder::new("primary_db".into())
        .from_query("INSERT INTO transfers (from_id, to_id) VALUES ($1, $2)", None)
        .build()
        .unwrap();
    assert_eq!(pdv.datasource, "primary_db");
    assert_eq!(pdv.query.as_deref(), Some("INSERT INTO transfers (from_id, to_id) VALUES ($1, $2)"));
}

#[test]
fn builder_from_schema_builds_successfully() {
    let schema = serde_json::json!({
        "driver": "postgresql",
        "type": "object",
        "fields": [
            { "name": "amount", "type": "decimal", "required": true }
        ]
    });
    let pdv = DatasourceBuilder::new("db".into())
        .from_schema(schema.clone(), None)
        .build()
        .unwrap();
    assert_eq!(pdv.schema, Some(schema));
}

#[test]
fn builder_with_schemas_builds_successfully() {
    let pdv = DatasourceBuilder::new("db".into())
        .from_query("SELECT 1", None)
        .with_post_schema(serde_json::json!({"driver": "postgresql", "type": "object"}))
        .with_get_schema(serde_json::json!({"driver": "postgresql", "type": "object"}))
        .build()
        .unwrap();
    assert!(pdv.post_schema.is_some());
    assert!(pdv.get_schema.is_some());
    assert!(pdv.put_schema.is_none());
    assert!(pdv.delete_schema.is_none());
}

#[test]
fn builder_no_query_or_schema_fails() {
    let result = DatasourceBuilder::new("db".into()).build();
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), PseudoDataViewError::NoQueryOrSchema));
}

#[test]
fn pseudo_dataview_no_streaming() {
    let pdv = DatasourceBuilder::new("db".into())
        .from_query("SELECT 1", None)
        .build()
        .unwrap();
    assert!(!pdv.supports_streaming());
}

#[test]
fn pseudo_dataview_no_caching() {
    let pdv = DatasourceBuilder::new("db".into())
        .from_query("SELECT 1", None)
        .build()
        .unwrap();
    assert!(!pdv.supports_caching());
}
