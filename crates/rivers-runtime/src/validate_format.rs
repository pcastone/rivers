//! Output formatters for bundle validation reports.
//!
//! Per `rivers-bundle-validation-spec.md` §8.
//!
//! Two formats are supported:
//! - **Text** (`format_text`) — human-readable, intended for terminal output.
//! - **JSON** (`format_json`) — machine-readable, stable contract for CI/CD.
//!
//! Also provides a Levenshtein distance helper for "did you mean?" suggestions
//! on unknown TOML keys (spec Appendix A).

use crate::validate_result::{
    ValidationReport, ValidationResult, ValidationStatus,
    LAYER_LOGICAL_CROSS_REFERENCES, LAYER_RESOURCE_EXISTENCE,
    LAYER_STRUCTURAL_TOML, LAYER_SYNTAX_VERIFICATION,
};

// ── Layer display names ────────────────────────────────────────────

/// Human-readable layer names in display order.
const LAYER_DISPLAY_ORDER: &[(&str, &str)] = &[
    (LAYER_STRUCTURAL_TOML, "Layer 1: Structural TOML"),
    (LAYER_RESOURCE_EXISTENCE, "Layer 2: Resource Existence"),
    (LAYER_LOGICAL_CROSS_REFERENCES, "Layer 3: Logical Cross-References"),
    (LAYER_SYNTAX_VERIFICATION, "Layer 4: Syntax Verification"),
];

// ── Text formatter ─────────────────────────────────────────────────

/// Format a validation report as human-readable text.
///
/// Output matches the spec §8.1 format:
///
/// ```text
/// Rivers Bundle Validation — orders-platform v1.4.2
/// ==================================================
///
/// Layer 1: Structural TOML
///   [PASS] manifest.toml — bundle manifest valid
///   [FAIL] orders-service/app.toml — unknown key 'veiew_type'
///
/// RESULT: 4 errors, 0 warnings
/// ```
pub fn format_text(report: &ValidationReport) -> String {
    let mut out = String::with_capacity(2048);

    // Header
    let title = format!(
        "Rivers Bundle Validation \u{2014} {} v{}",
        report.bundle_name, report.bundle_version,
    );
    out.push_str(&title);
    out.push('\n');
    out.push_str(&"=".repeat(title.len()));
    out.push('\n');

    // Layers in display order
    for &(key, display_name) in LAYER_DISPLAY_ORDER {
        if let Some(layer) = report.layers.get(key) {
            if layer.results.is_empty() {
                continue;
            }
            out.push('\n');
            out.push_str(display_name);
            out.push('\n');
            for result in &layer.results {
                format_text_result(&mut out, result);
            }
        }
    }

    // Summary line
    out.push('\n');
    let errors = report.summary.total_failed;
    let warnings = report.summary.total_warnings;
    out.push_str(&format!("RESULT: {} error{}, {} warning{}",
        errors,
        if errors == 1 { "" } else { "s" },
        warnings,
        if warnings == 1 { "" } else { "s" },
    ));
    out.push('\n');

    out
}

/// Format a single result as an indented text line.
fn format_text_result(out: &mut String, result: &ValidationResult) {
    let tag = match result.status {
        ValidationStatus::Pass => "[PASS]",
        ValidationStatus::Fail => "[FAIL]",
        ValidationStatus::Warn => "[WARN]",
        ValidationStatus::Skip => "[SKIP]",
    };

    // Primary line
    out.push_str("  ");
    out.push_str(tag);
    out.push(' ');

    if let Some(ref file) = result.file {
        out.push_str(file);
        if !result.message.is_empty() {
            out.push_str(" \u{2014} ");
            out.push_str(&result.message);
        }
    } else if let Some(ref engine) = result.engine {
        // Skip results show engine + reason
        out.push_str(&format!("{} checks", engine));
        if let Some(ref reason) = result.reason {
            out.push_str(" \u{2014} ");
            out.push_str(reason);
        }
    } else {
        out.push_str(&result.message);
    }
    out.push('\n');

    // Supplementary lines (indented further)
    if let Some(ref referenced_by) = result.referenced_by {
        out.push_str(&format!("         referenced by: {}\n", referenced_by));
    }
    if let Some(ref suggestion) = result.suggestion {
        out.push_str(&format!("         {}\n", suggestion));
    }
}

// ── JSON formatter ─────────────────────────────────────────────────

/// Format a validation report as JSON.
///
/// The output matches the stable contract in spec §8.2. Fields may be added
/// but never removed or renamed.
pub fn format_json(report: &ValidationReport) -> String {
    serde_json::to_string_pretty(report).expect("ValidationReport serialization cannot fail")
}

// ── Levenshtein distance ───────────────────────────────────────────

/// Compute the Levenshtein edit distance between two strings.
///
/// Returns the minimum number of single-character insertions, deletions,
/// or substitutions needed to transform `a` into `b`.
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    // Use two rows instead of a full matrix.
    let mut prev = vec![0usize; n + 1];
    let mut curr = vec![0usize; n + 1];

    for j in 0..=n {
        prev[j] = j;
    }

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1)             // deletion
                .min(curr[j - 1] + 1)            // insertion
                .min(prev[j - 1] + cost);        // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Find the best "did you mean?" suggestion from a list of known keys.
///
/// Returns the closest match if the Levenshtein distance is at most 2.
/// If multiple keys tie at the same distance, the first in the list wins.
pub fn did_you_mean<'a>(unknown: &str, known_keys: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(&str, usize)> = None;

    for &key in known_keys {
        let dist = levenshtein_distance(unknown, key);
        if dist == 0 {
            // Exact match — shouldn't happen if the key is truly unknown,
            // but return it as a safety measure.
            return Some(key);
        }
        if dist <= 2 {
            match best {
                None => best = Some((key, dist)),
                Some((_, best_dist)) if dist < best_dist => best = Some((key, dist)),
                _ => {}
            }
        }
    }

    best.map(|(key, _)| key)
}

/// Format a "did you mean?" suggestion string.
///
/// Returns `Some("did you mean 'view_type'?")` if a close match is found,
/// `None` otherwise.
pub fn suggest_key<'a>(unknown: &str, known_keys: &[&'a str]) -> Option<String> {
    did_you_mean(unknown, known_keys)
        .map(|key| format!("did you mean '{}'?", key))
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate_result::{error_codes, ValidationReport, ValidationResult};

    // ── Levenshtein tests ───────────────────────────────────────────

    #[test]
    fn levenshtein_identical() {
        assert_eq!(levenshtein_distance("view_type", "view_type"), 0);
    }

    #[test]
    fn levenshtein_single_substitution() {
        assert_eq!(levenshtein_distance("view_type", "view_tipe"), 1);
    }

    #[test]
    fn levenshtein_single_insertion() {
        // "viewtype" -> "view_type" = 1 insertion
        assert_eq!(levenshtein_distance("viewtype", "view_type"), 1);
    }

    #[test]
    fn levenshtein_single_deletion() {
        // "view__type" -> "view_type" = 1 deletion
        assert_eq!(levenshtein_distance("view__type", "view_type"), 1);
    }

    #[test]
    fn levenshtein_two_edits() {
        // "veiew_type" -> "view_type" = 1 deletion (extra 'e')
        assert_eq!(levenshtein_distance("veiew_type", "view_type"), 1);
    }

    #[test]
    fn levenshtein_transposition_is_two() {
        // "tpye" -> "type" = 2 (substitute p->y, substitute y->p)
        assert_eq!(levenshtein_distance("tpye", "type"), 2);
    }

    #[test]
    fn levenshtein_empty_strings() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("", "xyz"), 3);
    }

    #[test]
    fn levenshtein_completely_different() {
        assert_eq!(levenshtein_distance("abc", "xyz"), 3);
    }

    // ── did_you_mean tests ──────────────────────────────────────────

    #[test]
    fn did_you_mean_finds_close_match() {
        let known = &["view_type", "method", "path", "handler"];
        assert_eq!(did_you_mean("veiew_type", known), Some("view_type"));
    }

    #[test]
    fn did_you_mean_no_match_when_too_far() {
        let known = &["view_type", "method", "path"];
        assert_eq!(did_you_mean("completely_different", known), None);
    }

    #[test]
    fn did_you_mean_picks_closest() {
        let known = &["path", "pat", "paths"];
        // "pth" is distance 1 from "pat" and "path", picks first (pat)
        // Actually: "pth" -> "pat" = 2 (sub a for nothing? No: p-t-h vs p-a-t = 2 subs)
        // "pth" -> "path" = 1 (insert 'a')
        assert_eq!(did_you_mean("pth", known), Some("path"));
    }

    #[test]
    fn did_you_mean_empty_known_keys() {
        let known: &[&str] = &[];
        assert_eq!(did_you_mean("anything", known), None);
    }

    #[test]
    fn suggest_key_formats_suggestion() {
        let known = &["view_type", "method", "path"];
        let suggestion = suggest_key("veiew_type", known);
        assert_eq!(suggestion, Some("did you mean 'view_type'?".to_string()));
    }

    #[test]
    fn suggest_key_none_when_no_match() {
        let known = &["view_type", "method", "path"];
        assert_eq!(suggest_key("zzzzz", known), None);
    }

    // ── Text formatter tests ────────────────────────────────────────

    #[test]
    fn text_format_header() {
        let report = ValidationReport::new("orders-platform", "1.4.2");
        let text = format_text(&report);
        assert!(text.starts_with("Rivers Bundle Validation \u{2014} orders-platform v1.4.2\n"));
        // Second line is = signs
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[1].chars().all(|c| c == '='));
    }

    #[test]
    fn text_format_pass_line() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("manifest.toml", "bundle manifest valid"),
        );
        let text = format_text(&report);
        assert!(text.contains("  [PASS] manifest.toml \u{2014} bundle manifest valid"), "text was: {}", text);
    }

    #[test]
    fn text_format_fail_with_suggestion() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::fail(error_codes::S002, "app.toml", "unknown key 'veiew_type'")
                .with_suggestion("did you mean 'view_type'?"),
        );
        let text = format_text(&report);
        assert!(text.contains("  [FAIL] app.toml \u{2014} unknown key 'veiew_type'"), "text was: {}", text);
        assert!(text.contains("         did you mean 'view_type'?"), "text was: {}", text);
    }

    #[test]
    fn text_format_fail_with_referenced_by() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_RESOURCE_EXISTENCE,
            ValidationResult::fail(error_codes::E001, "handlers/fulfillment.ts", "file not found")
                .with_referenced_by("api.views.fulfill_order.handler.module"),
        );
        let text = format_text(&report);
        assert!(text.contains("         referenced by: api.views.fulfill_order.handler.module"), "text was: {}", text);
    }

    #[test]
    fn text_format_skip_line() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_SYNTAX_VERIFICATION,
            ValidationResult::skip("wasmtime", "engine dylib not available"),
        );
        let text = format_text(&report);
        assert!(text.contains("  [SKIP] wasmtime checks \u{2014} engine dylib not available"), "text was: {}", text);
    }

    #[test]
    fn text_format_summary_line() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::fail(error_codes::S002, "app.toml", "unknown key"),
        );
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::fail(error_codes::S003, "app.toml", "missing field"),
        );
        let text = format_text(&report);
        assert!(text.contains("RESULT: 2 errors, 0 warnings"), "text was: {}", text);
    }

    #[test]
    fn text_format_summary_singular() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::fail(error_codes::S002, "app.toml", "unknown key"),
        );
        let text = format_text(&report);
        assert!(text.contains("RESULT: 1 error, 0 warnings"), "text was: {}", text);
    }

    #[test]
    fn text_format_empty_layers_omitted() {
        let report = ValidationReport::new("test-bundle", "1.0.0");
        let text = format_text(&report);
        // Empty layers should not produce section headers
        assert!(!text.contains("Layer 1:"), "text was: {}", text);
        assert!(!text.contains("Layer 2:"), "text was: {}", text);
    }

    #[test]
    fn text_format_layer_ordering() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        // Add to layers out of order
        report.add_result(
            LAYER_SYNTAX_VERIFICATION,
            ValidationResult::pass("orders.ts", "compiles"),
        );
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("manifest.toml", "valid"),
        );

        let text = format_text(&report);
        let l1_pos = text.find("Layer 1:").expect("Layer 1 missing");
        let l4_pos = text.find("Layer 4:").expect("Layer 4 missing");
        assert!(l1_pos < l4_pos, "Layer 1 should come before Layer 4");
    }

    // ── JSON formatter tests ────────────────────────────────────────

    #[test]
    fn json_format_stable_contract() {
        let mut report = ValidationReport::new("orders-platform", "1.4.2");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("manifest.toml", "bundle manifest valid"),
        );
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::fail(error_codes::S002, "orders-service/app.toml", "unknown key 'veiew_type'")
                .with_table_path("api.views.list_orders")
                .with_field("veiew_type")
                .with_suggestion("did you mean 'view_type'?"),
        );

        let json = format_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

        // Top-level fields
        assert_eq!(parsed["bundle_name"], "orders-platform");
        assert_eq!(parsed["bundle_version"], "1.4.2");
        assert!(parsed["timestamp"].is_string());

        // Layers
        assert!(parsed["layers"]["structural_toml"]["results"].is_array());
        let results = &parsed["layers"]["structural_toml"]["results"];
        assert_eq!(results.as_array().unwrap().len(), 2);

        // First result: pass
        assert_eq!(results[0]["status"], "pass");
        assert_eq!(results[0]["file"], "manifest.toml");

        // Second result: fail with suggestion
        assert_eq!(results[1]["status"], "fail");
        assert_eq!(results[1]["field"], "veiew_type");
        assert_eq!(results[1]["suggestion"], "did you mean 'view_type'?");

        // Summary
        assert_eq!(parsed["summary"]["total_passed"], 1);
        assert_eq!(parsed["summary"]["total_failed"], 1);
        assert_eq!(parsed["summary"]["exit_code"], 1);
    }

    #[test]
    fn json_format_skip_serializing_none_fields() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("manifest.toml", "valid"),
        );

        let json = format_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let result = &parsed["layers"]["structural_toml"]["results"][0];

        // Optional fields that were not set should be absent
        assert!(result.get("table_path").is_none(), "table_path should be omitted");
        assert!(result.get("field").is_none(), "field should be omitted");
        assert!(result.get("suggestion").is_none(), "suggestion should be omitted");
        assert!(result.get("line").is_none(), "line should be omitted");
        assert!(result.get("column").is_none(), "column should be omitted");
        assert!(result.get("exports").is_none(), "exports should be omitted");
    }

    #[test]
    fn json_format_roundtrip() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("manifest.toml", "valid"),
        );
        report.add_result(
            LAYER_RESOURCE_EXISTENCE,
            ValidationResult::fail(error_codes::E001, "missing.ts", "file not found")
                .with_referenced_by("api.views.handler.module")
                .with_app("my-app"),
        );

        let json = format_json(&report);
        let deserialized: ValidationReport = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.bundle_name, "test-bundle");
        assert_eq!(deserialized.summary.total_passed, 1);
        assert_eq!(deserialized.summary.total_failed, 1);
        assert_eq!(deserialized.summary.exit_code, 1);
    }

    // ── Full spec example test ──────────────────────────────────────

    #[test]
    fn text_format_matches_spec_example_structure() {
        // Build the example from spec §8.1
        let mut report = ValidationReport::new("orders-platform", "1.4.2");

        // Layer 1
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("manifest.toml", "bundle manifest valid"),
        );
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("orders-service/manifest.toml", "app manifest valid"),
        );
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::fail(
                error_codes::S002,
                "orders-service/app.toml",
                "unknown key 'veiew_type' in [api.views.list_orders]",
            ),
        );
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::fail(
                error_codes::S003,
                "orders-service/resources.toml",
                "missing required field 'driver' in [[datasources]][1]",
            ),
        );

        // Layer 2
        report.add_result(
            LAYER_RESOURCE_EXISTENCE,
            ValidationResult::pass(
                "orders-service/libraries/handlers/orders.ts",
                "exists",
            ),
        );
        report.add_result(
            LAYER_RESOURCE_EXISTENCE,
            ValidationResult::fail(
                error_codes::E001,
                "orders-service/libraries/handlers/fulfillment.ts",
                "file not found",
            )
            .with_referenced_by("api.views.fulfill_order.handler.module"),
        );

        // Layer 3
        report.add_result(
            LAYER_LOGICAL_CROSS_REFERENCES,
            ValidationResult::pass(
                "DataView 'order_list'",
                "datasource 'orders-db' resolved",
            ),
        );
        report.add_result(
            LAYER_LOGICAL_CROSS_REFERENCES,
            ValidationResult::fail(
                error_codes::X001,
                "DataView 'user_lookup'",
                "datasource 'users-db' not declared in orders-service/resources.toml",
            ),
        );

        // Layer 4
        report.add_result(
            LAYER_SYNTAX_VERIFICATION,
            ValidationResult::pass(
                "orders-service/libraries/handlers/orders.ts",
                "compiles, export 'onCreateOrder' found",
            ),
        );
        report.add_result(
            LAYER_SYNTAX_VERIFICATION,
            ValidationResult::fail(
                error_codes::C001,
                "orders-service/libraries/handlers/init.ts",
                "SyntaxError: Unexpected token at line 14, column 8",
            ),
        );
        report.add_result(
            LAYER_SYNTAX_VERIFICATION,
            ValidationResult::skip("wasmtime", "engine dylib not available"),
        );

        let text = format_text(&report);

        // Verify structure
        assert!(text.contains("Rivers Bundle Validation"));
        assert!(text.contains("Layer 1: Structural TOML"));
        assert!(text.contains("Layer 2: Resource Existence"));
        assert!(text.contains("Layer 3: Logical Cross-References"));
        assert!(text.contains("Layer 4: Syntax Verification"));
        assert!(text.contains("RESULT: 5 errors, 0 warnings"));
    }
}
