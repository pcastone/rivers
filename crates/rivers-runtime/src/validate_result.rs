//! Validation result types for the bundle validation pipeline.
//!
//! Per `rivers-bundle-validation-spec.md` §9 and §11.
//!
//! This module defines the unified [`ValidationReport`] that collects results
//! from all four validation layers (structural TOML, resource existence,
//! logical cross-references, syntax verification) plus optional Gate 2 live
//! checks. Error codes follow the catalog in spec §11.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ── Enums ──────────────────────────────────────────────────────────

/// Severity of a validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationSeverity {
    /// Hard error — blocks deployment.
    Error,
    /// Advisory — does not affect exit code.
    Warning,
    /// Informational — context only.
    Info,
}

/// Outcome of an individual validation check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationStatus {
    /// Check passed.
    Pass,
    /// Check failed (error).
    Fail,
    /// Advisory finding (warning).
    Warn,
    /// Check was skipped (e.g., engine dylib not available).
    Skip,
}

// ── Individual result ──────────────────────────────────────────────

/// A single validation finding from any layer.
///
/// Every field beyond `status` and `message` is optional because different
/// layers produce different context. Layer 1 populates `table_path` and
/// `field`; Layer 4 populates `line`, `column`, and `exports`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Pass / Fail / Warn / Skip.
    pub status: ValidationStatus,

    /// File path relative to the bundle root (e.g., `orders-service/app.toml`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    /// Human-readable description of the finding.
    pub message: String,

    /// Error code from the catalog (e.g., `S002`, `E001`, `X005`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,

    /// TOML table path (e.g., `api.views.list_orders`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_path: Option<String>,

    /// Field name within the table (e.g., `view_type`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,

    /// "did you mean?" suggestion for unknown keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,

    /// Source line number (1-based) for syntax errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,

    /// Source column number (1-based) for syntax errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,

    /// Export names found in a compiled module (Layer 4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exports: Option<Vec<String>>,

    /// Whether the declared entrypoint was verified (Layer 4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_verified: Option<bool>,

    /// Config key that referenced a missing file (Layer 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referenced_by: Option<String>,

    /// App name for context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,

    /// Cross-reference source path (Layer 3, e.g., `data.dataviews.user_lookup`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Cross-reference target name (Layer 3, e.g., `users-db`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,

    /// Cross-reference target type (Layer 3, e.g., `datasource`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_type: Option<String>,

    /// Error type label for syntax errors (e.g., `SyntaxError`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,

    /// Engine name for skipped checks (e.g., `wasmtime`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,

    /// Reason a check was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ValidationResult {
    /// Create a passing result.
    pub fn pass(file: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: ValidationStatus::Pass,
            file: Some(file.into()),
            message: message.into(),
            error_code: None,
            table_path: None,
            field: None,
            suggestion: None,
            line: None,
            column: None,
            exports: None,
            entrypoint_verified: None,
            referenced_by: None,
            app: None,
            source: None,
            target: None,
            target_type: None,
            error_type: None,
            engine: None,
            reason: None,
        }
    }

    /// Create a failing result with an error code.
    pub fn fail(
        error_code: impl Into<String>,
        file: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: ValidationStatus::Fail,
            file: Some(file.into()),
            message: message.into(),
            error_code: Some(error_code.into()),
            table_path: None,
            field: None,
            suggestion: None,
            line: None,
            column: None,
            exports: None,
            entrypoint_verified: None,
            referenced_by: None,
            app: None,
            source: None,
            target: None,
            target_type: None,
            error_type: None,
            engine: None,
            reason: None,
        }
    }

    /// Create a warning result.
    pub fn warn(
        error_code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: ValidationStatus::Warn,
            file: None,
            message: message.into(),
            error_code: Some(error_code.into()),
            table_path: None,
            field: None,
            suggestion: None,
            line: None,
            column: None,
            exports: None,
            entrypoint_verified: None,
            referenced_by: None,
            app: None,
            source: None,
            target: None,
            target_type: None,
            error_type: None,
            engine: None,
            reason: None,
        }
    }

    /// Create a skipped result.
    pub fn skip(engine: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            status: ValidationStatus::Skip,
            file: None,
            message: String::new(),
            error_code: None,
            table_path: None,
            field: None,
            suggestion: None,
            line: None,
            column: None,
            exports: None,
            entrypoint_verified: None,
            referenced_by: None,
            app: None,
            source: None,
            target: None,
            target_type: None,
            error_type: None,
            engine: Some(engine.into()),
            reason: Some(reason.into()),
        }
    }

    /// Set the TOML table path on this result.
    pub fn with_table_path(mut self, path: impl Into<String>) -> Self {
        self.table_path = Some(path.into());
        self
    }

    /// Set the field name on this result.
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// Set a "did you mean?" suggestion.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Set source line and column for syntax errors.
    pub fn with_location(mut self, line: u32, column: u32) -> Self {
        self.line = Some(line);
        self.column = Some(column);
        self
    }

    /// Set the app name for context.
    pub fn with_app(mut self, app: impl Into<String>) -> Self {
        self.app = Some(app.into());
        self
    }

    /// Set the config key that referenced a missing file.
    pub fn with_referenced_by(mut self, key: impl Into<String>) -> Self {
        self.referenced_by = Some(key.into());
        self
    }

    /// Set cross-reference source and target.
    pub fn with_crossref(
        mut self,
        source: impl Into<String>,
        target: impl Into<String>,
        target_type: impl Into<String>,
    ) -> Self {
        self.source = Some(source.into());
        self.target = Some(target.into());
        self.target_type = Some(target_type.into());
        self
    }

    /// Set export names from a compiled module.
    pub fn with_exports(mut self, exports: Vec<String>) -> Self {
        self.exports = Some(exports);
        self
    }

    /// Set the entrypoint verification flag.
    pub fn with_entrypoint_verified(mut self, verified: bool) -> Self {
        self.entrypoint_verified = Some(verified);
        self
    }

    /// Set the error type label (e.g., `SyntaxError`).
    pub fn with_error_type(mut self, error_type: impl Into<String>) -> Self {
        self.error_type = Some(error_type.into());
        self
    }
}

// ── Layer results ──────────────────────────────────────────────────

/// Results from a single validation layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayerResults {
    /// Number of checks that passed.
    pub passed: u32,
    /// Number of checks that failed.
    pub failed: u32,
    /// Number of checks that were skipped.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped: u32,
    /// Individual check results.
    pub results: Vec<ValidationResult>,
}

/// Helper for serde `skip_serializing_if` — omit zero-valued skipped counts.
fn is_zero(v: &u32) -> bool {
    *v == 0
}

impl LayerResults {
    /// Add a result and update the pass/fail/skip counters.
    pub fn add(&mut self, result: ValidationResult) {
        match result.status {
            ValidationStatus::Pass => self.passed += 1,
            ValidationStatus::Fail => self.failed += 1,
            ValidationStatus::Skip => self.skipped += 1,
            ValidationStatus::Warn => { /* warnings counted separately in summary */ }
        }
        self.results.push(result);
    }
}

// ── Validation summary ─────────────────────────────────────────────

/// Aggregated counts across all layers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidationSummary {
    /// Total checks that passed across all layers.
    pub total_passed: u32,
    /// Total checks that failed across all layers.
    pub total_failed: u32,
    /// Total checks that were skipped across all layers.
    pub total_skipped: u32,
    /// Total warnings across all layers.
    pub total_warnings: u32,
    /// Process exit code: 0 = pass, 1 = errors, 2 = bundle not found, 3 = config error.
    pub exit_code: i32,
}

// ── Validation report ──────────────────────────────────────────────

/// Well-known layer names used as keys in the `layers` map.
pub const LAYER_STRUCTURAL_TOML: &str = "structural_toml";
/// Well-known layer name for resource existence checks.
pub const LAYER_RESOURCE_EXISTENCE: &str = "resource_existence";
/// Well-known layer name for logical cross-reference checks.
pub const LAYER_LOGICAL_CROSS_REFERENCES: &str = "logical_cross_references";
/// Well-known layer name for syntax verification.
pub const LAYER_SYNTAX_VERIFICATION: &str = "syntax_verification";

/// Unified report from all validation layers.
///
/// Created via [`ValidationReport::new`], populated with
/// [`add_result`](ValidationReport::add_result), and formatted with
/// [`format_text`](crate::validate_format::format_text) or
/// [`format_json`](crate::validate_format::format_json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Bundle name from `manifest.toml`.
    pub bundle_name: String,
    /// Bundle version from `manifest.toml`.
    pub bundle_version: String,
    /// ISO-8601 timestamp of when validation ran.
    pub timestamp: String,
    /// Results grouped by layer name.
    pub layers: BTreeMap<String, LayerResults>,
    /// Aggregated counts and exit code.
    pub summary: ValidationSummary,
}

impl ValidationReport {
    /// Create a new report for the given bundle.
    pub fn new(bundle_name: impl Into<String>, bundle_version: impl Into<String>) -> Self {
        // Use std time to generate an ISO-8601 timestamp without external deps.
        let timestamp = {
            let dur = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            // Produce a simple ISO-8601 timestamp: seconds since epoch.
            // For a proper formatted timestamp we do manual formatting.
            let secs = dur.as_secs();
            // Convert to broken-down UTC time manually.
            format_timestamp_utc(secs, dur.subsec_millis())
        };

        let mut layers = BTreeMap::new();
        layers.insert(LAYER_STRUCTURAL_TOML.to_string(), LayerResults::default());
        layers.insert(LAYER_RESOURCE_EXISTENCE.to_string(), LayerResults::default());
        layers.insert(LAYER_LOGICAL_CROSS_REFERENCES.to_string(), LayerResults::default());
        layers.insert(LAYER_SYNTAX_VERIFICATION.to_string(), LayerResults::default());

        Self {
            bundle_name: bundle_name.into(),
            bundle_version: bundle_version.into(),
            timestamp,
            layers,
            summary: ValidationSummary::default(),
        }
    }

    /// Add a result to the specified layer and update the summary.
    pub fn add_result(&mut self, layer: &str, result: ValidationResult) {
        let layer_results = self.layers.entry(layer.to_string()).or_default();

        // Update summary counters.
        match result.status {
            ValidationStatus::Pass => self.summary.total_passed += 1,
            ValidationStatus::Fail => {
                self.summary.total_failed += 1;
                self.summary.exit_code = 1;
            }
            ValidationStatus::Skip => self.summary.total_skipped += 1,
            ValidationStatus::Warn => self.summary.total_warnings += 1,
        }

        layer_results.add(result);
    }

    /// Process exit code: 0 = pass, 1 = errors, 2 = bundle not found, 3 = config error.
    pub fn exit_code(&self) -> i32 {
        self.summary.exit_code
    }

    /// Set the exit code directly (e.g., 2 for bundle not found).
    pub fn set_exit_code(&mut self, code: i32) {
        self.summary.exit_code = code;
    }

    /// Returns `true` if any check failed.
    pub fn has_errors(&self) -> bool {
        self.summary.total_failed > 0
    }

    /// Returns `true` if any check produced a warning.
    pub fn has_warnings(&self) -> bool {
        self.summary.total_warnings > 0
    }
}

/// Format a Unix timestamp as ISO-8601 UTC (`2026-04-06T14:23:01.847Z`).
///
/// Uses manual arithmetic instead of an external crate.
fn format_timestamp_utc(epoch_secs: u64, millis: u32) -> String {
    // Days from Unix epoch to a given date — Rata Die algorithm.
    let days = (epoch_secs / 86400) as i64;
    let time_secs = (epoch_secs % 86400) as u32;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    // Civil date from day count (algorithm from Howard Hinnant).
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, hours, minutes, seconds, millis,
    )
}

// ── Error code constants ───────────────────────────────────────────
//
// Per spec §11. Organized by layer prefix:
//   S = structural, E = existence, X = cross-ref,
//   C = syntax, L = live-check, W = warning.

/// Layer 1 — Structural errors.
pub mod error_codes {
    // ── S: Structural TOML (Layer 1) ────────────────────────────────

    /// TOML parse error.
    pub const S001: &str = "S001";
    /// Unknown key in TOML table.
    pub const S002: &str = "S002";
    /// Missing required field.
    pub const S003: &str = "S003";
    /// Wrong type for field.
    pub const S004: &str = "S004";
    /// Invalid value for field.
    pub const S005: &str = "S005";
    /// `nopassword=true` and `lockbox` are mutually exclusive.
    pub const S006: &str = "S006";
    /// `lockbox` is required when `nopassword` is not set.
    pub const S007: &str = "S007";
    /// `appId` is not a valid UUID.
    pub const S008: &str = "S008";
    /// `type` must be `app-main` or `app-service`.
    pub const S009: &str = "S009";
    /// `bundleVersion` is not valid semver.
    pub const S010: &str = "S010";

    // ── E: Existence (Layer 2) ──────────────────────────────────────

    /// File not found.
    pub const E001: &str = "E001";
    /// App directory listed in bundle manifest but not found.
    pub const E002: &str = "E002";
    /// Missing `manifest.toml` in app directory.
    pub const E003: &str = "E003";
    /// Missing `resources.toml` in app directory.
    pub const E004: &str = "E004";
    /// Missing `app.toml` in app directory.
    pub const E005: &str = "E005";

    // ── X: Cross-reference (Layer 3) ────────────────────────────────

    /// DataView references unknown datasource.
    pub const X001: &str = "X001";
    /// View references unknown DataView.
    pub const X002: &str = "X002";
    /// View handler resource not declared.
    pub const X003: &str = "X003";
    /// Invalidates target does not exist.
    pub const X004: &str = "X004";
    /// Service references unknown appId.
    pub const X005: &str = "X005";
    /// Duplicate appId in bundle.
    pub const X006: &str = "X006";
    /// Duplicate datasource name.
    pub const X007: &str = "X007";
    /// Dataview handler only valid for `view_type=Rest`.
    pub const X008: &str = "X008";
    /// Method must be GET for WebSocket.
    pub const X009: &str = "X009";
    /// Method must be GET for SSE.
    pub const X010: &str = "X010";
    /// SPA config only valid on `app-main`.
    pub const X011: &str = "X011";
    /// Init handler missing module or entrypoint.
    pub const X012: &str = "X012";
    /// `x-type` does not match `driver`.
    pub const X013: &str = "X013";

    // ── C: Syntax verification (Layer 4) ────────────────────────────

    /// Syntax error in TS/JS file.
    pub const C001: &str = "C001";
    /// Entrypoint not found in exports.
    pub const C002: &str = "C002";
    /// WASM validation failed.
    pub const C003: &str = "C003";
    /// Import resolves outside `libraries/` — cross-app import.
    pub const C004: &str = "C004";
    /// Import target file not found.
    pub const C005: &str = "C005";
    /// Invalid JSON in schema file.
    pub const C006: &str = "C006";
    /// Schema missing `type` field.
    pub const C007: &str = "C007";
    /// Schema `required` array references property not in `properties`.
    pub const C008: &str = "C008";
    /// Driver-specific schema constraint violated (e.g. missing subject for NATS).
    pub const C009: &str = "C009";

    // ── L: Live checks (Gate 2 only) ────────────────────────────────

    /// LockBox alias not found.
    pub const L001: &str = "L001";
    /// Driver not registered in this riversd instance.
    pub const L002: &str = "L002";
    /// Schema validation failed against live driver.
    pub const L003: &str = "L003";
    /// `x-type` does not match registered driver.
    pub const L004: &str = "L004";
    /// Required service not running.
    pub const L005: &str = "L005";

    // ── CB: Circuit breaker warnings ────────────────────────────────

    /// Circuit breaker ID referenced by only one DataView (likely a typo).
    pub const CB001: &str = "CB001";

    // ── W: Warnings (do not affect exit code) ───────────────────────

    /// Unknown driver — cannot verify at build time.
    pub const W001: &str = "W001";
    /// Layer 4 skipped — engine dylib not available.
    pub const W002: &str = "W002";
    /// Layer 4 skipped — `riversd.toml` not found.
    pub const W003: &str = "W003";
    /// No views defined — check `[api.views.*]`.
    pub const W004: &str = "W004";
    /// skip_introspect = true on a DataView that has a GET query (likely misconfiguration).
    pub const W005: &str = "W005";
    /// subscribable = true on an MCP resource whose bound DataView has no GET method.
    pub const W006: &str = "W006";
    /// cursor_key is set but the query has no ORDER BY clause (cursor pagination requires deterministic ordering).
    pub const W007: &str = "W007";
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_report_has_four_layers() {
        let report = ValidationReport::new("test-bundle", "1.0.0");
        assert_eq!(report.layers.len(), 4);
        assert!(report.layers.contains_key(LAYER_STRUCTURAL_TOML));
        assert!(report.layers.contains_key(LAYER_RESOURCE_EXISTENCE));
        assert!(report.layers.contains_key(LAYER_LOGICAL_CROSS_REFERENCES));
        assert!(report.layers.contains_key(LAYER_SYNTAX_VERIFICATION));
    }

    #[test]
    fn new_report_starts_clean() {
        let report = ValidationReport::new("test-bundle", "1.0.0");
        assert_eq!(report.exit_code(), 0);
        assert!(!report.has_errors());
        assert!(!report.has_warnings());
        assert_eq!(report.summary.total_passed, 0);
        assert_eq!(report.summary.total_failed, 0);
        assert_eq!(report.summary.total_skipped, 0);
        assert_eq!(report.summary.total_warnings, 0);
    }

    #[test]
    fn add_pass_increments_passed() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("manifest.toml", "bundle manifest valid"),
        );
        assert_eq!(report.summary.total_passed, 1);
        assert_eq!(report.summary.total_failed, 0);
        assert_eq!(report.exit_code(), 0);
        assert!(!report.has_errors());

        let layer = &report.layers[LAYER_STRUCTURAL_TOML];
        assert_eq!(layer.passed, 1);
        assert_eq!(layer.failed, 0);
        assert_eq!(layer.results.len(), 1);
    }

    #[test]
    fn add_fail_sets_exit_code_1() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::fail(
                error_codes::S002,
                "orders-service/app.toml",
                "unknown key 'veiew_type'",
            )
            .with_table_path("api.views.list_orders")
            .with_field("veiew_type")
            .with_suggestion("did you mean 'view_type'?"),
        );
        assert_eq!(report.summary.total_failed, 1);
        assert_eq!(report.exit_code(), 1);
        assert!(report.has_errors());
    }

    #[test]
    fn add_warn_does_not_set_exit_code() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_SYNTAX_VERIFICATION,
            ValidationResult::warn(error_codes::W002, "Layer 4 skipped — engine dylib not available for v8"),
        );
        assert_eq!(report.summary.total_warnings, 1);
        assert_eq!(report.exit_code(), 0);
        assert!(!report.has_errors());
        assert!(report.has_warnings());
    }

    #[test]
    fn add_skip_increments_skipped() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_SYNTAX_VERIFICATION,
            ValidationResult::skip("wasmtime", "engine dylib not available"),
        );
        assert_eq!(report.summary.total_skipped, 1);
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn mixed_results_across_layers() {
        let mut report = ValidationReport::new("orders-platform", "1.4.2");

        // Layer 1: 2 pass, 1 fail
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
            ValidationResult::fail(error_codes::S002, "orders-service/app.toml", "unknown key 'veiew_type'"),
        );

        // Layer 2: 1 pass, 1 fail
        report.add_result(
            LAYER_RESOURCE_EXISTENCE,
            ValidationResult::pass("orders-service/libraries/handlers/orders.ts", "exists"),
        );
        report.add_result(
            LAYER_RESOURCE_EXISTENCE,
            ValidationResult::fail(error_codes::E001, "orders-service/libraries/handlers/fulfillment.ts", "file not found"),
        );

        // Layer 3: 1 pass
        report.add_result(
            LAYER_LOGICAL_CROSS_REFERENCES,
            ValidationResult::pass("DataView 'order_list'", "datasource 'orders-db' resolved"),
        );

        // Layer 4: 1 skip
        report.add_result(
            LAYER_SYNTAX_VERIFICATION,
            ValidationResult::skip("wasmtime", "engine dylib not available"),
        );

        assert_eq!(report.summary.total_passed, 4);
        assert_eq!(report.summary.total_failed, 2);
        assert_eq!(report.summary.total_skipped, 1);
        assert_eq!(report.summary.total_warnings, 0);
        assert_eq!(report.exit_code(), 1);
        assert!(report.has_errors());
    }

    #[test]
    fn set_exit_code_overrides() {
        let mut report = ValidationReport::new("missing-bundle", "0.0.0");
        report.set_exit_code(2); // bundle not found
        assert_eq!(report.exit_code(), 2);
    }

    #[test]
    fn result_builder_chains() {
        let result = ValidationResult::fail(error_codes::C001, "orders.ts", "Unexpected token")
            .with_location(14, 8)
            .with_error_type("SyntaxError")
            .with_app("orders-service");

        assert_eq!(result.status, ValidationStatus::Fail);
        assert_eq!(result.error_code.as_deref(), Some("C001"));
        assert_eq!(result.line, Some(14));
        assert_eq!(result.column, Some(8));
        assert_eq!(result.error_type.as_deref(), Some("SyntaxError"));
        assert_eq!(result.app.as_deref(), Some("orders-service"));
    }

    #[test]
    fn result_with_crossref() {
        let result = ValidationResult::fail(error_codes::X001, "app.toml", "datasource 'users-db' not declared")
            .with_crossref("data.dataviews.user_lookup", "users-db", "datasource")
            .with_app("orders-service");

        assert_eq!(result.source.as_deref(), Some("data.dataviews.user_lookup"));
        assert_eq!(result.target.as_deref(), Some("users-db"));
        assert_eq!(result.target_type.as_deref(), Some("datasource"));
    }

    #[test]
    fn result_with_exports() {
        let result = ValidationResult::pass("orders.ts", "compiles, export 'onCreateOrder' found")
            .with_exports(vec!["onCreateOrder".into(), "default".into()])
            .with_entrypoint_verified(true);

        assert_eq!(result.exports.as_ref().unwrap().len(), 2);
        assert_eq!(result.entrypoint_verified, Some(true));
    }

    #[test]
    fn report_serializes_to_json() {
        let mut report = ValidationReport::new("test-bundle", "1.0.0");
        report.add_result(
            LAYER_STRUCTURAL_TOML,
            ValidationResult::pass("manifest.toml", "bundle manifest valid"),
        );

        let json = serde_json::to_string_pretty(&report).expect("serialize");
        assert!(json.contains("\"bundle_name\": \"test-bundle\""));
        assert!(json.contains("\"total_passed\": 1"));
        assert!(json.contains("\"exit_code\": 0"));
    }

    #[test]
    fn timestamp_format_is_iso8601() {
        let report = ValidationReport::new("test-bundle", "1.0.0");
        // Should match pattern: YYYY-MM-DDTHH:MM:SS.mmmZ
        assert!(report.timestamp.ends_with('Z'), "timestamp should end with Z: {}", report.timestamp);
        assert_eq!(report.timestamp.len(), 24, "timestamp length should be 24: {}", report.timestamp);
        assert_eq!(&report.timestamp[4..5], "-");
        assert_eq!(&report.timestamp[7..8], "-");
        assert_eq!(&report.timestamp[10..11], "T");
        assert_eq!(&report.timestamp[13..14], ":");
        assert_eq!(&report.timestamp[16..17], ":");
        assert_eq!(&report.timestamp[19..20], ".");
    }

    #[test]
    fn error_code_constants_are_correct() {
        assert_eq!(error_codes::S001, "S001");
        assert_eq!(error_codes::S010, "S010");
        assert_eq!(error_codes::E001, "E001");
        assert_eq!(error_codes::E005, "E005");
        assert_eq!(error_codes::X001, "X001");
        assert_eq!(error_codes::X013, "X013");
        assert_eq!(error_codes::C001, "C001");
        assert_eq!(error_codes::C008, "C008");
        assert_eq!(error_codes::L001, "L001");
        assert_eq!(error_codes::L005, "L005");
        assert_eq!(error_codes::W001, "W001");
        assert_eq!(error_codes::W004, "W004");
    }

    #[test]
    fn format_timestamp_known_epoch() {
        // 2024-01-01T00:00:00.000Z = 1704067200
        let ts = format_timestamp_utc(1704067200, 0);
        assert_eq!(ts, "2024-01-01T00:00:00.000Z");
    }

    #[test]
    fn format_timestamp_with_millis() {
        // 2026-04-06T14:23:01.847Z = 1775485381
        let ts = format_timestamp_utc(1775485381, 847);
        assert_eq!(ts, "2026-04-06T14:23:01.847Z");
    }
}
