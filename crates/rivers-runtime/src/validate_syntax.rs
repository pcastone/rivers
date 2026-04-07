//! Layer 4 — Syntax verification for schema JSON, handler modules, and imports.
//!
//! Per `rivers-bundle-validation-spec.md` §4.4.
//!
//! This module validates:
//! - Schema JSON files are structurally valid (C006-C008)
//! - Handler modules compile (C001-C003) — via engine dylib FFI
//! - Handler entrypoints exist in exports (C002)
//! - Relative import paths resolve within the app boundary (C004-C005)

use std::path::Path;

use crate::loader::LoadedBundle;
use crate::validate_engine::EngineHandles;
use crate::validate_result::{error_codes, ValidationResult};
use crate::view::HandlerConfig;

/// Validate syntax of all code artifacts and schema files in a bundle.
///
/// Runs schema JSON validation (pure Rust), handler compile checks (via engine
/// FFI if available), and import path resolution.
pub fn validate_syntax(
    bundle_dir: &Path,
    bundle: &LoadedBundle,
    engines: &EngineHandles,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    for app in &bundle.apps {
        let app_name = &app.manifest.app_name;
        let app_dir = &app.app_dir;

        // ── Schema JSON validation (C006-C008) ──────────────────────
        for (dv_name, dv) in &app.config.data.dataviews {
            let schema_refs = [
                dv.return_schema.as_deref(),
                dv.get_schema.as_deref(),
                dv.post_schema.as_deref(),
                dv.put_schema.as_deref(),
                dv.delete_schema.as_deref(),
            ];
            for schema_ref in schema_refs.into_iter().flatten() {
                let schema_path = app_dir.join(schema_ref);
                let display_path = format!("{}/{}", app_name, schema_ref);
                if schema_path.exists() {
                    let schema_results =
                        validate_schema_json(&schema_path, &display_path, app_name, dv_name);
                    results.extend(schema_results);
                }
                // Missing files are handled by Layer 2 (existence checks).
            }
        }

        // ── Handler compile checks (C001-C003) ─────────────────────
        for (_view_name, view) in &app.config.api.views {
            if let HandlerConfig::Codecomponent {
                language,
                module,
                entrypoint,
                ..
            } = &view.handler
            {
                let module_path = app_dir.join(module);
                let display_path = format!("{}/{}", app_name, module);

                if !module_path.exists() {
                    continue; // Layer 2 handles missing files
                }

                // Determine engine type from language
                let is_wasm = matches!(language.as_str(), "wasm");
                let is_js_ts = matches!(
                    language.as_str(),
                    "typescript"
                        | "ts"
                        | "typescript_strict"
                        | "ts_strict"
                        | "javascript"
                        | "js"
                        | "javascript_v8"
                        | "js_v8"
                );

                if is_js_ts {
                    if let Some(ref v8) = engines.v8 {
                        match std::fs::read(&module_path) {
                            Ok(source) => {
                                let filename =
                                    module_path.file_name().unwrap_or_default().to_string_lossy();
                                match v8.compile_check(&source, &filename) {
                                    Ok(check_result) => {
                                        // Check entrypoint exists in exports
                                        if check_result.exports.contains(&entrypoint.to_string())
                                        {
                                            results.push(
                                                ValidationResult::pass(
                                                    &display_path,
                                                    format!(
                                                        "compiles, export '{}' found",
                                                        entrypoint
                                                    ),
                                                )
                                                .with_app(app_name)
                                                .with_exports(check_result.exports)
                                                .with_entrypoint_verified(true),
                                            );
                                        } else {
                                            results.push(
                                                ValidationResult::fail(
                                                    error_codes::C002,
                                                    &display_path,
                                                    format!(
                                                        "entrypoint '{}' not found in exports — available: [{}]",
                                                        entrypoint,
                                                        check_result.exports.join(", ")
                                                    ),
                                                )
                                                .with_app(app_name)
                                                .with_exports(check_result.exports)
                                                .with_entrypoint_verified(false),
                                            );
                                        }
                                    }
                                    Err(err) => {
                                        let mut result = ValidationResult::fail(
                                            error_codes::C001,
                                            &display_path,
                                            format!("SyntaxError: {}", err.message),
                                        )
                                        .with_app(app_name)
                                        .with_error_type("SyntaxError");
                                        if let Some(line) = err.line {
                                            if let Some(col) = err.column {
                                                result = result.with_location(line, col);
                                            }
                                        }
                                        results.push(result);
                                    }
                                }

                                // Import path resolution (C004-C005)
                                if let Ok(source_str) = std::str::from_utf8(&source) {
                                    let import_results = validate_imports(
                                        source_str,
                                        &module_path,
                                        app_dir,
                                        &display_path,
                                        app_name,
                                    );
                                    results.extend(import_results);
                                }
                            }
                            Err(_) => {
                                // Can't read file — Layer 2 would have caught this
                            }
                        }
                    } else {
                        results.push(
                            ValidationResult::skip(
                                "v8",
                                "V8 engine dylib not available — TS/JS syntax check skipped",
                            )
                            .with_app(app_name),
                        );
                    }
                } else if is_wasm {
                    if let Some(ref wasm) = engines.wasmtime {
                        match std::fs::read(&module_path) {
                            Ok(bytes) => {
                                let filename =
                                    module_path.file_name().unwrap_or_default().to_string_lossy();
                                match wasm.compile_check(&bytes, &filename) {
                                    Ok(check_result) => {
                                        if check_result.exports.contains(&entrypoint.to_string())
                                        {
                                            results.push(
                                                ValidationResult::pass(
                                                    &display_path,
                                                    format!(
                                                        "valid WASM, export '{}' found",
                                                        entrypoint
                                                    ),
                                                )
                                                .with_app(app_name)
                                                .with_exports(check_result.exports)
                                                .with_entrypoint_verified(true),
                                            );
                                        } else {
                                            results.push(
                                                ValidationResult::fail(
                                                    error_codes::C002,
                                                    &display_path,
                                                    format!(
                                                        "entrypoint '{}' not found in WASM exports — available: [{}]",
                                                        entrypoint,
                                                        check_result.exports.join(", ")
                                                    ),
                                                )
                                                .with_app(app_name)
                                                .with_exports(check_result.exports)
                                                .with_entrypoint_verified(false),
                                            );
                                        }
                                    }
                                    Err(err) => {
                                        results.push(
                                            ValidationResult::fail(
                                                error_codes::C003,
                                                &display_path,
                                                format!("WASM validation failed: {}", err.message),
                                            )
                                            .with_app(app_name),
                                        );
                                    }
                                }
                            }
                            Err(_) => {}
                        }
                    } else {
                        results.push(
                            ValidationResult::skip(
                                "wasmtime",
                                "Wasmtime engine dylib not available — WASM check skipped",
                            )
                            .with_app(app_name),
                        );
                    }
                }

                // Run import resolution for JS/TS even without engine (file existence)
                if is_js_ts && engines.v8.is_none() {
                    if let Ok(source) = std::fs::read_to_string(&module_path) {
                        let import_results =
                            validate_imports(&source, &module_path, app_dir, &display_path, app_name);
                        results.extend(import_results);
                    }
                }
            }

            // Also check event handler pipeline stages
            if let Some(ref eh) = view.event_handlers {
                for stages in [&eh.pre_process, &eh.handlers, &eh.post_process, &eh.on_error] {
                    for stage in stages {
                        let module_path = app_dir.join(&stage.module);
                        if module_path.exists() {
                            if let Ok(source) = std::fs::read_to_string(&module_path) {
                                let display_path =
                                    format!("{}/{}", app_name, stage.module);
                                let import_results = validate_imports(
                                    &source,
                                    &module_path,
                                    app_dir,
                                    &display_path,
                                    app_name,
                                );
                                results.extend(import_results);
                            }
                        }
                    }
                }
            }
        }
    }

    // If no engines are available and no checks ran, add info
    let _ = bundle_dir; // used for future engine discovery

    results
}

// ── Schema JSON Validation ──────────────────────────────────────

/// Validate a schema JSON file for structural correctness.
///
/// Checks:
/// - Valid JSON (C006)
/// - Has `type` field (C007)
/// - `required` array entries match `properties` keys (C008)
pub fn validate_schema_json(
    path: &Path,
    display_path: &str,
    app_name: &str,
    _dv_name: &str,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return results, // File existence checked in Layer 2
    };

    // C006: Valid JSON
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            results.push(
                ValidationResult::fail(
                    error_codes::C006,
                    display_path,
                    format!("invalid JSON in schema — {e}"),
                )
                .with_app(app_name),
            );
            return results;
        }
    };

    let obj = match json.as_object() {
        Some(o) => o,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::C006,
                    display_path,
                    "schema root is not a JSON object",
                )
                .with_app(app_name),
            );
            return results;
        }
    };

    // C007: Has `type` field
    if !obj.contains_key("type") {
        results.push(
            ValidationResult::fail(
                error_codes::C007,
                display_path,
                "schema missing 'type' field",
            )
            .with_app(app_name),
        );
    }

    // C008: required entries match properties keys
    if let Some(required) = obj.get("required").and_then(|v| v.as_array()) {
        if let Some(properties) = obj.get("properties").and_then(|v| v.as_object()) {
            for req_val in required {
                if let Some(req_name) = req_val.as_str() {
                    if !properties.contains_key(req_name) {
                        results.push(
                            ValidationResult::fail(
                                error_codes::C008,
                                display_path,
                                format!(
                                    "schema 'required' array references property '{}' not in 'properties'",
                                    req_name
                                ),
                            )
                            .with_app(app_name),
                        );
                    }
                }
            }
        }
    }

    // If no errors, it passes
    if results.is_empty() {
        results.push(
            ValidationResult::pass(display_path, "schema JSON valid").with_app(app_name),
        );
    }

    results
}

// ── Import Path Resolution ──────────────────────────────────────

/// Extract relative import paths from JS/TS source.
///
/// Per spec FR-10: only checks relative paths (`./` or `../`).
/// Bare specifiers are skipped. Absolute paths are errors.
fn extract_relative_imports(source: &str) -> Vec<(String, bool)> {
    let mut imports = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // Match: import ... from "..." or import ... from '...'
        // Also match: export ... from "..."
        if let Some(from_idx) = trimmed.find(" from ") {
            let after_from = &trimmed[from_idx + 6..];
            if let Some(path) = extract_string_literal(after_from) {
                if path.starts_with("./") || path.starts_with("../") {
                    imports.push((path.to_string(), false));
                } else if path.starts_with('/') {
                    imports.push((path.to_string(), true)); // absolute = error
                }
                // Bare specifiers silently skipped
            }
        }
    }

    imports
}

/// Extract a string literal from the start of a string slice.
fn extract_string_literal(s: &str) -> Option<&str> {
    let s = s.trim();
    let (quote, rest) = if s.starts_with('"') {
        ('"', &s[1..])
    } else if s.starts_with('\'') {
        ('\'', &s[1..])
    } else {
        return None;
    };

    rest.find(quote).map(|end| &rest[..end])
}

/// Validate import paths in a JS/TS source file.
fn validate_imports(
    source: &str,
    source_path: &Path,
    app_dir: &Path,
    display_path: &str,
    app_name: &str,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();
    let source_dir = source_path.parent().unwrap_or(app_dir);
    let libraries_dir = app_dir.join("libraries");

    for (import_path, is_absolute) in extract_relative_imports(source) {
        if is_absolute {
            results.push(
                ValidationResult::fail(
                    error_codes::C004,
                    display_path,
                    format!(
                        "import '{}' uses absolute path — paths must be relative",
                        import_path
                    ),
                )
                .with_app(app_name),
            );
            continue;
        }

        // Resolve relative to the importing file's directory
        let resolved = source_dir.join(&import_path);

        // Try with common extensions if no extension provided
        let candidates = if resolved.extension().is_some() {
            vec![resolved.clone()]
        } else {
            vec![
                resolved.with_extension("ts"),
                resolved.with_extension("js"),
                resolved.with_extension("mjs"),
                resolved.clone(),
            ]
        };

        let found = candidates.iter().any(|c| c.exists());

        if !found {
            results.push(
                ValidationResult::fail(
                    error_codes::C005,
                    display_path,
                    format!("import '{}' target file not found", import_path),
                )
                .with_app(app_name),
            );
            continue;
        }

        // Check that the resolved path stays within libraries/
        if libraries_dir.exists() {
            if let Ok(canonical) = resolved.canonicalize().or_else(|_| {
                candidates
                    .iter()
                    .find_map(|c| c.canonicalize().ok())
                    .ok_or(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "not found",
                    ))
            }) {
                if let Ok(canonical_libs) = libraries_dir.canonicalize() {
                    if !canonical.starts_with(&canonical_libs) {
                        results.push(
                            ValidationResult::fail(
                                error_codes::C004,
                                display_path,
                                format!(
                                    "import '{}' resolves outside {}/libraries/ — cross-app imports not permitted",
                                    import_path, app_name
                                ),
                            )
                            .with_app(app_name),
                        );
                    }
                }
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Schema JSON tests ───────────────────────────────────────────

    #[test]
    fn valid_schema_passes() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("item.schema.json");
        std::fs::write(
            &schema_path,
            r#"{"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}"#,
        )
        .unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/item.schema.json", "app", "items");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, crate::validate_result::ValidationStatus::Pass);
    }

    #[test]
    fn c006_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("bad.json");
        std::fs::write(&schema_path, "{ not valid json }").unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/bad.json", "app", "dv");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].error_code.as_deref(), Some("C006"));
    }

    #[test]
    fn c006_non_object_root() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("array.json");
        std::fs::write(&schema_path, "[1, 2, 3]").unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/array.json", "app", "dv");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].error_code.as_deref(), Some("C006"));
    }

    #[test]
    fn c007_missing_type_field() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("notype.json");
        std::fs::write(&schema_path, r#"{"properties": {"id": {"type": "integer"}}}"#).unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/notype.json", "app", "dv");
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("C007")));
    }

    #[test]
    fn c008_required_not_in_properties() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("badreq.json");
        std::fs::write(
            &schema_path,
            r#"{"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id", "name"]}"#,
        )
        .unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/badreq.json", "app", "dv");
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("C008")));
        assert!(results.iter().any(|r| r.message.contains("name")));
    }

    // ── Import extraction tests ─────────────────────────────────────

    #[test]
    fn extract_relative_imports_basic() {
        let source = r#"
import { helper } from "./utils/helper";
import data from "../shared/data";
import lodash from "lodash";
export { foo } from './local';
"#;
        let imports = extract_relative_imports(source);
        assert_eq!(imports.len(), 3);
        assert_eq!(imports[0].0, "./utils/helper");
        assert!(!imports[0].1);
        assert_eq!(imports[1].0, "../shared/data");
        assert!(!imports[1].1);
        assert_eq!(imports[2].0, "./local");
        assert!(!imports[2].1);
    }

    #[test]
    fn extract_absolute_import_flagged() {
        let source = r#"import bad from "/absolute/path";"#;
        let imports = extract_relative_imports(source);
        assert_eq!(imports.len(), 1);
        assert!(imports[0].1); // is_absolute = true
    }

    #[test]
    fn bare_specifiers_skipped() {
        let source = r#"
import lodash from "lodash";
import React from "react";
import { foo } from "@org/pkg";
"#;
        let imports = extract_relative_imports(source);
        assert!(imports.is_empty());
    }

    #[test]
    fn validate_imports_missing_target() {
        let dir = tempfile::tempdir().unwrap();
        let handler = dir.path().join("handler.ts");
        std::fs::write(&handler, r#"import { foo } from "./missing";"#).unwrap();

        let results = validate_imports(
            &std::fs::read_to_string(&handler).unwrap(),
            &handler,
            dir.path(),
            "app/handler.ts",
            "app",
        );
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("C005")));
    }

    #[test]
    fn validate_imports_absolute_path_error() {
        let dir = tempfile::tempdir().unwrap();
        let handler = dir.path().join("handler.ts");
        std::fs::write(&handler, r#"import { foo } from "/etc/passwd";"#).unwrap();

        let results = validate_imports(
            &std::fs::read_to_string(&handler).unwrap(),
            &handler,
            dir.path(),
            "app/handler.ts",
            "app",
        );
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("C004")));
    }

    #[test]
    fn validate_imports_existing_relative() {
        let dir = tempfile::tempdir().unwrap();
        let libs = dir.path().join("libraries");
        std::fs::create_dir_all(&libs).unwrap();
        let handler = libs.join("handler.ts");
        let helper = libs.join("helper.ts");
        std::fs::write(&handler, r#"import { foo } from "./helper";"#).unwrap();
        std::fs::write(&helper, "export const foo = 1;").unwrap();

        let results = validate_imports(
            &std::fs::read_to_string(&handler).unwrap(),
            &handler,
            dir.path(),
            "app/libraries/handler.ts",
            "app",
        );
        // No errors — import resolves correctly
        assert!(
            results.is_empty() || results.iter().all(|r| r.status != crate::validate_result::ValidationStatus::Fail),
            "valid import should not produce errors: {:?}",
            results
        );
    }
}
