//! GraphQL type definitions generated from DataView schemas.
//!
//! Per `rivers-view-layer-spec.md` §9.2: return_schema -> GraphQL object types.

use std::collections::HashMap;

use async_graphql::dynamic;
use serde::Serialize;

/// Maps a GraphQL query field to a DataView.
///
/// Per spec §9.2: GraphQL query field -> DataView name, arguments -> DataView parameters.
#[derive(Debug, Clone, Serialize)]
pub struct ResolverMapping {
    /// GraphQL field name.
    pub field_name: String,
    /// DataView to execute.
    pub dataview: String,
    /// Argument -> DataView parameter mapping.
    pub argument_mapping: HashMap<String, String>,
    /// Whether this is a list (returns array) or scalar (returns single object).
    pub is_list: bool,
}

/// A GraphQL type generated from a DataView return schema.
#[derive(Debug, Clone, Serialize)]
pub struct GraphqlType {
    /// GraphQL type name (PascalCase).
    pub name: String,
    /// Fields belonging to this type.
    pub fields: Vec<GraphqlField>,
}

/// A field in a GraphQL type.
#[derive(Debug, Clone, Serialize)]
pub struct GraphqlField {
    /// Field name.
    pub name: String,
    /// Field type.
    pub field_type: GraphqlFieldType,
    /// Whether the field is nullable.
    pub nullable: bool,
}

/// GraphQL field type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum GraphqlFieldType {
    /// UTF-8 string scalar.
    String,
    /// 32-bit integer scalar.
    Int,
    /// 64-bit floating-point scalar.
    Float,
    /// Boolean scalar.
    Boolean,
    /// Unique identifier scalar.
    ID,
    /// Reference to another GraphQL type.
    Object(std::string::String),
    /// List of another type.
    List(Box<GraphqlFieldType>),
}

impl GraphqlFieldType {
    /// Map a JSON schema type to a GraphQL field type.
    pub fn from_json_schema_type(type_str: &str) -> Self {
        match type_str {
            "string" => GraphqlFieldType::String,
            "integer" => GraphqlFieldType::Int,
            "number" => GraphqlFieldType::Float,
            "boolean" => GraphqlFieldType::Boolean,
            _ => GraphqlFieldType::String, // fallback
        }
    }

    /// Convert to an async-graphql `TypeRef`.
    #[allow(dead_code)] // Reserved for GraphQL resolver support (spec §9.2)
    pub(crate) fn to_type_ref(&self, nullable: bool) -> dynamic::TypeRef {
        let inner = match self {
            GraphqlFieldType::String => dynamic::TypeRef::named(dynamic::TypeRef::STRING),
            GraphqlFieldType::Int => dynamic::TypeRef::named(dynamic::TypeRef::INT),
            GraphqlFieldType::Float => dynamic::TypeRef::named(dynamic::TypeRef::FLOAT),
            GraphqlFieldType::Boolean => dynamic::TypeRef::named(dynamic::TypeRef::BOOLEAN),
            GraphqlFieldType::ID => dynamic::TypeRef::named(dynamic::TypeRef::ID),
            GraphqlFieldType::Object(name) => dynamic::TypeRef::named(name.as_str()),
            GraphqlFieldType::List(inner_type) => {
                // Inner list elements are non-null by default
                let element_ref = inner_type.to_type_ref(false);
                dynamic::TypeRef::List(Box::new(element_ref))
            }
        };

        if nullable {
            inner
        } else {
            dynamic::TypeRef::NonNull(Box::new(inner))
        }
    }
}

/// Generate GraphQL type definitions from DataView return schemas.
///
/// Per spec §9.2: return_schema -> GraphQL object types.
pub fn generate_graphql_types(
    dataview_schemas: &HashMap<String, serde_json::Value>,
) -> Vec<GraphqlType> {
    let mut types = Vec::new();

    for (name, schema) in dataview_schemas {
        if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
            let fields: Vec<GraphqlField> = properties
                .iter()
                .map(|(field_name, field_schema)| {
                    let type_str = field_schema
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("string");

                    let field_type = if type_str == "array" {
                        let item_type = field_schema
                            .get("items")
                            .and_then(|i| i.get("type"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("string");
                        GraphqlFieldType::List(Box::new(
                            GraphqlFieldType::from_json_schema_type(item_type),
                        ))
                    } else {
                        GraphqlFieldType::from_json_schema_type(type_str)
                    };

                    let required = schema
                        .get("required")
                        .and_then(|r| r.as_array())
                        .map(|arr| {
                            arr.iter()
                                .any(|v| v.as_str() == Some(field_name.as_str()))
                        })
                        .unwrap_or(false);

                    GraphqlField {
                        name: field_name.clone(),
                        field_type,
                        nullable: !required,
                    }
                })
                .collect();

            // Convert DataView name to PascalCase for GraphQL type name
            let type_name = to_pascal_case(name);

            types.push(GraphqlType {
                name: type_name,
                fields,
            });
        }
    }

    types
}

/// Convert snake_case or kebab-case to PascalCase.
pub(crate) fn to_pascal_case(s: &str) -> String {
    s.split(|c: char| c == '_' || c == '-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut s = first.to_uppercase().to_string();
                    s.extend(chars);
                    s
                }
            }
        })
        .collect()
}
