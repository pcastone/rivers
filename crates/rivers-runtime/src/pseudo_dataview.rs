//! Pseudo DataView — runtime-constructed, disposable DataViews from within handlers.
//!
//! Per technology-path-spec §6.

use std::collections::HashMap;

/// Builder for constructing pseudo DataViews at runtime.
///
/// Per spec §6.2: mirrors the TOML declaration in code.
/// `.build()` creates but doesn't execute — schema is syntax-checked at build time.
#[derive(Debug, Clone)]
pub struct DatasourceBuilder {
    datasource: String,
    query: Option<String>,
    query_params: Option<Vec<serde_json::Value>>,
    schema: Option<serde_json::Value>,
    schema_params: Option<HashMap<String, serde_json::Value>>,
    get_schema: Option<serde_json::Value>,
    post_schema: Option<serde_json::Value>,
    put_schema: Option<serde_json::Value>,
    delete_schema: Option<serde_json::Value>,
}

impl DatasourceBuilder {
    pub fn new(datasource: String) -> Self {
        Self {
            datasource,
            query: None,
            query_params: None,
            schema: None,
            schema_params: None,
            get_schema: None,
            post_schema: None,
            put_schema: None,
            delete_schema: None,
        }
    }

    /// Set a raw query string with optional positional parameters.
    pub fn from_query(mut self, sql: &str, params: Option<Vec<serde_json::Value>>) -> Self {
        self.query = Some(sql.to_string());
        self.query_params = params;
        self
    }

    /// Set a schema-based query with parameters.
    pub fn from_schema(mut self, schema: serde_json::Value, params: Option<HashMap<String, serde_json::Value>>) -> Self {
        self.schema = Some(schema);
        self.schema_params = params;
        self
    }

    pub fn with_get_schema(mut self, schema: serde_json::Value) -> Self {
        self.get_schema = Some(schema);
        self
    }

    pub fn with_post_schema(mut self, schema: serde_json::Value) -> Self {
        self.post_schema = Some(schema);
        self
    }

    pub fn with_put_schema(mut self, schema: serde_json::Value) -> Self {
        self.put_schema = Some(schema);
        self
    }

    pub fn with_delete_schema(mut self, schema: serde_json::Value) -> Self {
        self.delete_schema = Some(schema);
        self
    }

    /// Build the pseudo DataView with schema syntax validation.
    ///
    /// Per spec §6.5: `.build()` produces a DataView object, doesn't run the query.
    /// Per spec §19.1: inline schemas are validated at build time.
    pub fn build(self) -> Result<PseudoDataView, PseudoDataViewError> {
        if self.query.is_none() && self.schema.is_none() {
            return Err(PseudoDataViewError::NoQueryOrSchema);
        }

        // Validate inline schemas at build time (spec §19.1)
        if let Some(ref schema) = self.get_schema {
            validate_inline_schema(schema, "get_schema")?;
        }
        if let Some(ref schema) = self.post_schema {
            validate_inline_schema(schema, "post_schema")?;
        }
        if let Some(ref schema) = self.put_schema {
            validate_inline_schema(schema, "put_schema")?;
        }
        if let Some(ref schema) = self.delete_schema {
            validate_inline_schema(schema, "delete_schema")?;
        }

        Ok(PseudoDataView {
            datasource: self.datasource,
            query: self.query,
            query_params: self.query_params,
            schema: self.schema,
            schema_params: self.schema_params,
            get_schema: self.get_schema,
            post_schema: self.post_schema,
            put_schema: self.put_schema,
            delete_schema: self.delete_schema,
        })
    }
}

/// A runtime-constructed DataView — local, disposable, single-handler scope.
///
/// Per spec §6.4: no caching, no cache invalidation, no streaming, no EventBus registration.
#[derive(Debug, Clone)]
pub struct PseudoDataView {
    pub datasource: String,
    pub query: Option<String>,
    pub query_params: Option<Vec<serde_json::Value>>,
    pub schema: Option<serde_json::Value>,
    pub schema_params: Option<HashMap<String, serde_json::Value>>,
    pub get_schema: Option<serde_json::Value>,
    pub post_schema: Option<serde_json::Value>,
    pub put_schema: Option<serde_json::Value>,
    pub delete_schema: Option<serde_json::Value>,
}

impl PseudoDataView {
    /// Check if this pseudo DataView supports streaming (always false).
    pub fn supports_streaming(&self) -> bool {
        false
    }

    /// Check if this pseudo DataView supports caching (always false).
    pub fn supports_caching(&self) -> bool {
        false
    }
}

/// Validate an inline schema JSON value at build time.
///
/// Per spec §19.1: the schema must be a JSON object and must include a "driver" field.
fn validate_inline_schema(
    schema: &serde_json::Value,
    label: &str,
) -> Result<(), PseudoDataViewError> {
    // Must be an object
    if !schema.is_object() {
        return Err(PseudoDataViewError::SchemaSyntax(format!(
            "{} must be a JSON object",
            label
        )));
    }
    // Must have a driver field
    if schema.get("driver").and_then(|v| v.as_str()).is_none() {
        return Err(PseudoDataViewError::SchemaSyntax(format!(
            "{} must include a 'driver' field",
            label
        )));
    }
    Ok(())
}

/// Errors from pseudo DataView construction.
#[derive(Debug, thiserror::Error)]
pub enum PseudoDataViewError {
    #[error("pseudo DataView requires either fromQuery() or fromSchema()")]
    NoQueryOrSchema,

    #[error("schema syntax error: {0}")]
    SchemaSyntax(String),

    #[error("unknown datasource: {0}")]
    UnknownDatasource(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_requires_query_or_schema() {
        let result = DatasourceBuilder::new("db".into()).build();
        assert!(result.is_err());
    }

    #[test]
    fn build_with_query_succeeds() {
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn build_validates_inline_schema_requires_object() {
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .with_post_schema(serde_json::json!("not an object"))
            .build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("post_schema must be a JSON object"));
    }

    #[test]
    fn build_validates_inline_schema_requires_driver() {
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .with_post_schema(serde_json::json!({"type": "object"}))
            .build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("post_schema must include a 'driver' field"));
    }

    #[test]
    fn build_validates_inline_schema_accepts_valid() {
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .with_post_schema(serde_json::json!({
                "driver": "postgresql",
                "type": "object",
                "fields": []
            }))
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn build_validates_all_method_schemas() {
        // get_schema missing driver
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .with_get_schema(serde_json::json!({"type": "object"}))
            .build();
        assert!(result.is_err());

        // put_schema missing driver
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .with_put_schema(serde_json::json!({"type": "object"}))
            .build();
        assert!(result.is_err());

        // delete_schema missing driver
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .with_delete_schema(serde_json::json!({"type": "object"}))
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_valid_schemas_for_all_methods() {
        let valid = serde_json::json!({"driver": "postgresql", "type": "object"});
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .with_get_schema(valid.clone())
            .with_post_schema(valid.clone())
            .with_put_schema(valid.clone())
            .with_delete_schema(valid)
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn build_no_schemas_still_succeeds() {
        let result = DatasourceBuilder::new("db".into())
            .from_query("SELECT 1", None)
            .build();
        assert!(result.is_ok());
    }
}
