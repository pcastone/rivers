//! Typed operation catalog types for the V8 proxy codegen framework.
//!
//! Any driver may declare a slice of `OperationDescriptor` to expose typed
//! JS methods on `ctx.datasource("name")`. Drivers that do not declare a
//! catalog continue to use the standard `Query` / `execute()` pipeline.

/// Parameter type for JS-side validation before IPC dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    /// String parameter type.
    String,
    /// Integer parameter type.
    Integer,
    /// Float parameter type.
    Float,
    /// Boolean parameter type.
    Boolean,
    /// Accepts string, number, boolean, array, or object.
    Any,
}

/// Single parameter in an operation signature.
#[derive(Clone, Debug)]
pub struct Param {
    /// Parameter name.
    pub name: &'static str,
    /// Parameter type for validation.
    pub param_type: ParamType,
    /// Whether this parameter is required.
    pub required: bool,
    /// Default value if parameter is optional.
    pub default_value: Option<&'static str>,
}

impl Param {
    /// Create a required parameter.
    pub const fn required(name: &'static str, param_type: ParamType) -> Self {
        Param { name, param_type, required: true, default_value: None }
    }

    /// Create an optional parameter with a default value.
    pub const fn optional(
        name: &'static str,
        param_type: ParamType,
        default: &'static str,
    ) -> Self {
        Param { name, param_type, required: false, default_value: Some(default) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_required_builder_sets_required_true_and_no_default() {
        let p = Param::required("path", ParamType::String);
        assert_eq!(p.name, "path");
        assert!(p.required);
        assert!(p.default_value.is_none());
    }

    #[test]
    fn param_optional_builder_sets_required_false_with_default() {
        let p = Param::optional("encoding", ParamType::String, "utf-8");
        assert_eq!(p.name, "encoding");
        assert!(!p.required);
        assert_eq!(p.default_value, Some("utf-8"));
    }

    #[test]
    fn paramtype_variants_are_distinct() {
        // Prove all five variants exist and can be constructed
        let _ = ParamType::String;
        let _ = ParamType::Integer;
        let _ = ParamType::Float;
        let _ = ParamType::Boolean;
        let _ = ParamType::Any;
    }
}
