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

/// Classifies an operation as read or write for DDL security alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    /// Read operation (non-mutating).
    Read,
    /// Write operation (mutating).
    Write,
}

/// Describes a single typed operation a driver exposes to handlers.
#[derive(Clone, Debug)]
pub struct OperationDescriptor {
    /// Operation name as exposed to handlers.
    pub name: &'static str,
    /// Read or write classification.
    pub kind: OpKind,
    /// Parameter slice.
    pub params: &'static [Param],
    /// Description for documentation.
    pub description: &'static str,
}

impl OperationDescriptor {
    /// Create a read operation descriptor.
    pub const fn read(
        name: &'static str,
        params: &'static [Param],
        description: &'static str,
    ) -> Self {
        OperationDescriptor { name, kind: OpKind::Read, params, description }
    }

    /// Create a write operation descriptor.
    pub const fn write(
        name: &'static str,
        params: &'static [Param],
        description: &'static str,
    ) -> Self {
        OperationDescriptor { name, kind: OpKind::Write, params, description }
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

    #[test]
    fn operation_descriptor_read_builder_sets_kind_read() {
        static PARAMS: &[Param] = &[
            Param::required("path", ParamType::String),
        ];
        let desc = OperationDescriptor::read("readFile", PARAMS, "Read file contents");
        assert_eq!(desc.name, "readFile");
        assert_eq!(desc.kind, OpKind::Read);
        assert_eq!(desc.params.len(), 1);
        assert_eq!(desc.description, "Read file contents");
    }

    #[test]
    fn operation_descriptor_write_builder_sets_kind_write() {
        static PARAMS: &[Param] = &[
            Param::required("path", ParamType::String),
            Param::required("content", ParamType::String),
        ];
        let desc = OperationDescriptor::write("writeFile", PARAMS, "Write file");
        assert_eq!(desc.kind, OpKind::Write);
        assert_eq!(desc.params.len(), 2);
    }

    #[test]
    fn opkind_eq() {
        assert_eq!(OpKind::Read, OpKind::Read);
        assert_ne!(OpKind::Read, OpKind::Write);
    }
}
