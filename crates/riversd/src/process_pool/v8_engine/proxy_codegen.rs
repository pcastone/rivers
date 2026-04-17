//! JS codegen for direct-dispatch typed proxies.
//!
//! Emits one method per `OperationDescriptor`: each method type-checks its
//! arguments, fills defaults for optional params, and calls
//! `Rivers.__directDispatch(name, operation, parameters)`.

use rivers_runtime::rivers_driver_sdk::{OperationDescriptor, Param, ParamType};

/// Build a JS IIFE that evaluates to a proxy object.
///
/// Shape:
/// ```js
/// (function() {
///     const proxy = {};
///     proxy.readFile = function(path, encoding) { … };
///     …
///     return proxy;
/// })()
/// ```
pub(super) fn build_proxy_script(ds_name: &str, catalog: &[OperationDescriptor]) -> String {
    let mut out = String::new();
    out.push_str("(function(){const proxy={};");
    for op in catalog {
        emit_method(&mut out, ds_name, op);
    }
    out.push_str("return proxy;})()");
    out
}

fn emit_method(out: &mut String, ds_name: &str, op: &OperationDescriptor) {
    let param_list: Vec<&str> = op.params.iter().map(|p| p.name).collect();
    out.push_str("proxy.");
    out.push_str(op.name);
    out.push_str("=function(");
    out.push_str(&param_list.join(","));
    out.push_str("){");

    // Required-presence checks first, then defaults for optionals, then type guards.
    for p in op.params {
        if p.required {
            out.push_str("if(");
            out.push_str(p.name);
            out.push_str("===undefined){throw new TypeError(\"");
            out.push_str(op.name);
            out.push_str(": '");
            out.push_str(p.name);
            out.push_str("' is required\");}");
        } else if let Some(default) = p.default_value {
            out.push_str("if(");
            out.push_str(p.name);
            out.push_str("===undefined){");
            out.push_str(p.name);
            out.push_str("=");
            out.push_str(&default_literal(p.param_type, default));
            out.push_str(";}");
        }
        emit_type_guard(out, op.name, p);
    }

    // Build the parameters object.
    out.push_str("return Rivers.__directDispatch(");
    push_js_string(out, ds_name);
    out.push(',');
    push_js_string(out, op.name);
    out.push_str(",{");
    for (i, p) in op.params.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(p.name);
        out.push(':');
        out.push_str(p.name);
    }
    out.push_str("});};");
}

/// Render `Param::default_value` (a string) as a JS literal appropriate to its type.
fn default_literal(ty: ParamType, raw: &str) -> String {
    match ty {
        ParamType::String => {
            let mut s = String::new();
            push_js_string(&mut s, raw);
            s
        }
        ParamType::Integer | ParamType::Float | ParamType::Boolean => raw.to_string(),
        // For Any, preserve the declared default verbatim and let V8 parse it.
        ParamType::Any => raw.to_string(),
    }
}

fn emit_type_guard(out: &mut String, op_name: &str, p: &Param) {
    let (check, expected) = match p.param_type {
        ParamType::String => ("typeof x!==\"string\"", "string"),
        ParamType::Integer => (
            "typeof x!==\"number\"||!Number.isInteger(x)",
            "integer",
        ),
        ParamType::Float => ("typeof x!==\"number\"", "number"),
        ParamType::Boolean => ("typeof x!==\"boolean\"", "boolean"),
        // Any: no guard — caller can pass whatever the operation accepts.
        ParamType::Any => return,
    };

    let check_with_var = check.replace('x', p.name);
    out.push_str("if(");
    // For optional params with a default, the default path already set a valid
    // value — but guard still runs: that's fine, defaults always pass their own check.
    // For optional params without a default, skip the guard when undefined.
    if !p.required && p.default_value.is_none() {
        out.push_str(p.name);
        out.push_str("!==undefined&&");
    }
    out.push_str(&check_with_var);
    out.push_str("){throw new TypeError(\"");
    out.push_str(op_name);
    out.push_str(": '");
    out.push_str(p.name);
    out.push_str("' must be a ");
    out.push_str(expected);
    out.push_str("\");}");
}

/// Push a JS double-quoted string literal (escaping \ and ").
fn push_js_string(out: &mut String, raw: &str) {
    out.push('"');
    for c in raw.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::rivers_driver_sdk::{OperationDescriptor, Param, ParamType};

    static READ_FILE_PARAMS: &[Param] = &[
        Param::required("path", ParamType::String),
        Param::optional("encoding", ParamType::String, "utf-8"),
    ];

    static FIND_PARAMS: &[Param] = &[
        Param::required("pattern", ParamType::String),
        Param::optional("max_results", ParamType::Integer, "1000"),
    ];

    #[test]
    fn script_starts_and_ends_with_iife_wrapper() {
        let op = OperationDescriptor::read("readFile", READ_FILE_PARAMS, "");
        let script = build_proxy_script("fs", std::slice::from_ref(&op));
        assert!(script.starts_with("(function(){"));
        assert!(script.ends_with(")()"));
        assert!(script.contains("return proxy;"));
    }

    #[test]
    fn required_string_param_emits_undefined_and_typeof_guards() {
        let op = OperationDescriptor::read("readFile", READ_FILE_PARAMS, "");
        let script = build_proxy_script("fs", std::slice::from_ref(&op));
        assert!(script.contains("path===undefined"));
        assert!(script.contains("'path' is required"));
        assert!(script.contains("typeof path!==\"string\""));
        assert!(script.contains("'path' must be a string"));
    }

    #[test]
    fn optional_string_param_injects_string_default() {
        let op = OperationDescriptor::read("readFile", READ_FILE_PARAMS, "");
        let script = build_proxy_script("fs", std::slice::from_ref(&op));
        assert!(script.contains("encoding===undefined){encoding=\"utf-8\";"));
    }

    #[test]
    fn optional_integer_param_injects_numeric_default_without_quotes() {
        let op = OperationDescriptor::read("find", FIND_PARAMS, "");
        let script = build_proxy_script("fs", std::slice::from_ref(&op));
        assert!(script.contains("max_results===undefined){max_results=1000;"));
        assert!(script.contains("Number.isInteger"));
    }

    #[test]
    fn dispatch_call_passes_datasource_and_operation_names() {
        let op = OperationDescriptor::read("readFile", READ_FILE_PARAMS, "");
        let script = build_proxy_script("fs", std::slice::from_ref(&op));
        assert!(script.contains("Rivers.__directDispatch(\"fs\",\"readFile\""));
    }

    #[test]
    fn multiple_ops_all_attached_to_proxy() {
        let a = OperationDescriptor::read("readFile", READ_FILE_PARAMS, "");
        let b = OperationDescriptor::read("find", FIND_PARAMS, "");
        let script = build_proxy_script("fs", &[a, b]);
        assert!(script.contains("proxy.readFile"));
        assert!(script.contains("proxy.find"));
    }

    #[test]
    fn datasource_name_with_quote_is_escaped() {
        let op = OperationDescriptor::read("readFile", READ_FILE_PARAMS, "");
        let script = build_proxy_script(r#"weird"name"#, std::slice::from_ref(&op));
        assert!(script.contains(r#"\""#));
    }
}
