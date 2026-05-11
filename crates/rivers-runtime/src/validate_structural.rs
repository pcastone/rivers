//! Layer 1 — Structural TOML Validation.
//!
//! Per `rivers-bundle-validation-spec.md` §4.1 and
//! `rivers-bundle-validation-friction.md` FR-1.
//!
//! Validates that every TOML file in a bundle uses only known keys,
//! includes all required fields, and that field values conform to expected
//! formats (UUID, semver, app-type enum, nopassword/lockbox exclusion).
//!
//! **Approach:** Parse each file as `toml::Value`, then walk the value tree
//! comparing keys against known field sets. This avoids adding
//! `deny_unknown_fields` to the runtime config structs (which would break
//! the existing `load_bundle()` path).

use std::path::Path;

use crate::validate_format::suggest_key;
use crate::validate_result::{error_codes, ValidationResult};

// ── Known field sets (FR-1) ───────────────────────────────────────

/// Bundle `manifest.toml` — all required.
const BUNDLE_MANIFEST_FIELDS: &[&str] = &["bundleName", "bundleVersion", "source", "apps"];
const BUNDLE_MANIFEST_REQUIRED: &[&str] = &["bundleName", "bundleVersion", "source", "apps"];

/// App `manifest.toml`.
const APP_MANIFEST_FIELDS: &[&str] = &[
    "appName", "description", "version", "type", "appId",
    "entryPoint", "appEntryPoint", "source", "spa", "init",
];
const APP_MANIFEST_REQUIRED: &[&str] = &[
    "appName", "version", "type", "appId", "entryPoint", "source",
];

/// `spa` sub-table in app manifest.
const SPA_FIELDS: &[&str] = &["root", "indexFile", "fallback", "maxAge"];
const SPA_REQUIRED: &[&str] = &["root", "indexFile"];

/// `init` sub-table in app manifest.
const INIT_FIELDS: &[&str] = &["module", "entrypoint"];
const INIT_REQUIRED: &[&str] = &["module", "entrypoint"];

/// `resources.toml` top-level.
const RESOURCES_FIELDS: &[&str] = &["datasources", "services", "keystores"];

/// Datasource entry in `[[datasources]]`.
const DATASOURCE_DECL_FIELDS: &[&str] = &[
    "name", "driver", "x-type", "lockbox", "nopassword", "required",
    "host", "port", "database", "username", "password", "service", "introspect",
];
const DATASOURCE_DECL_REQUIRED: &[&str] = &["name", "driver", "x-type", "required"];

/// Service entry in `[[services]]`.
const SERVICE_DECL_FIELDS: &[&str] = &["name", "appId", "required"];
const SERVICE_DECL_REQUIRED: &[&str] = &["name", "appId", "required"];

/// Keystore entry in `[[keystores]]`.
const KEYSTORE_DECL_FIELDS: &[&str] = &["name", "lockbox", "required"];
const KEYSTORE_DECL_REQUIRED: &[&str] = &["name", "lockbox", "required"];

/// `app.toml` top-level.
const APP_CONFIG_FIELDS: &[&str] = &["data", "api", "static_files"];

/// `[data]` section.
const DATA_CONFIG_FIELDS: &[&str] = &["datasources", "dataviews", "keystore"];

/// DataView config.
const DATAVIEW_FIELDS: &[&str] = &[
    "name", "datasource", "query", "parameters", "caching", "max_rows",
    "invalidates", "get_schema", "post_schema", "put_schema", "delete_schema",
    "get_query", "post_query", "put_query", "delete_query",
    "return_schema", "get_parameters", "post_parameters", "put_parameters",
    "delete_parameters", "streaming", "validate_result", "strict_parameters",
    "circuitBreakerId", "prepared", "skip_introspect", "query_params", "cursor_key",
    // Composability (P2.9)
    "source_views", "compose_strategy", "join_key", "enrich_mode",
    // Transaction (TXN spec §3)
    "transaction",
];
const DATAVIEW_REQUIRED: &[&str] = &["name", "datasource"];

/// DataView parameter.
const PARAMETER_FIELDS: &[&str] = &["name", "type", "required", "default", "location"];
const PARAMETER_REQUIRED: &[&str] = &["name", "type"];

/// DataView caching config.
const CACHING_FIELDS: &[&str] = &[
    "ttl_seconds", "l1_enabled", "l1_max_bytes", "l1_max_entries",
    "l2_enabled", "l2_max_value_bytes",
];
const CACHING_REQUIRED: &[&str] = &["ttl_seconds"];

/// `[api]` section.
const API_CONFIG_FIELDS: &[&str] = &["views"];

/// View config.
const VIEW_FIELDS: &[&str] = &[
    "path", "method", "view_type", "response_format", "auth", "handler",
    "parameter_mapping", "process_pool", "libs", "datasources", "dataviews",
    "primary", "streaming", "streaming_format", "stream_timeout_ms",
    "allow_outbound_http", "allow_env_vars", "session_stage", "methods",
    "guard", "guard_config", "rate_limit_per_minute", "rate_limit_burst_size",
    "websocket_mode", "max_connections", "sse_tick_interval_ms",
    "sse_trigger_events", "sse_event_buffer_size",
    "session_revalidation_interval_s", "polling", "event_handlers",
    "on_stream", "ws_hooks", "on_event",
    "tools", "resources", "prompts", "instructions", "session", "federation",
    "response_headers", "guard_view",
    // Cron view fields (CB-P1.14 Track 3)
    "schedule", "interval_seconds", "overlap_policy", "max_concurrent",
];
const VIEW_REQUIRED: &[&str] = &["path", "method", "view_type", "handler"];

/// Canonical `view_type` values accepted by the framework. Mirrors
/// `crates/rivers-runtime/src/validate.rs::VALID_VIEW_TYPES` (the runtime
/// load path) so the bundle validator catches unknown values at structural
/// layer instead of letting them slip through to runtime as silent no-ops.
/// Sprint 2026-05-09 Track 2 + Track 3 (added `Cron`).
const VALID_VIEW_TYPES: &[&str] = &[
    "Rest", "Websocket", "ServerSentEvents", "MessageConsumer", "Mcp", "Cron",
];

/// Canonical `overlap_policy` values for Cron views (CB-P1.14 Track 3).
const VALID_OVERLAP_POLICIES: &[&str] = &["skip", "queue", "allow"];

/// Canonical `auth` values. Anything else (e.g. `"bearer"` from CB-P1.12)
/// is rejected at structural layer with `S005`. The bearer pattern is
/// expressed via `guard_view` referencing a codecomponent that returns
/// `{ allow: bool }` — see `rivers-auth-session-spec.md` §11.5.
/// Sprint 2026-05-09 Track 2.
const VALID_AUTH_MODES: &[&str] = &["none", "session"];

/// Handler config.
const HANDLER_FIELDS: &[&str] = &[
    "type", "dataview", "language", "module", "entrypoint", "resources",
];
const HANDLER_REQUIRED: &[&str] = &["type"];

/// Parameter mapping config.
const PARAM_MAPPING_FIELDS: &[&str] = &["query", "path", "body", "header"];

/// `[static_files]` section.
const STATIC_FILES_FIELDS: &[&str] = &["enabled", "root", "index_file", "spa_fallback"];

/// MCP resource config in `[api.views.*.resources.*]`.
const MCP_RESOURCE_FIELDS: &[&str] = &[
    "dataview", "description", "mime_type", "uri_template",
    "subscribable", "poll_interval_seconds",
];
const MCP_RESOURCE_REQUIRED: &[&str] = &["dataview"];

/// MCP federation entry in `[[api.views.*.federation]]` or `[api.views.*.federation.*]`.
const MCP_FEDERATION_FIELDS: &[&str] = &[
    "alias", "url", "bearer_token", "tools_filter", "resources_filter", "timeout_ms",
];
const MCP_FEDERATION_REQUIRED: &[&str] = &["alias", "url"];

// ── Public API ────────────────────────────────────────────────────

/// Validate structural correctness of all TOML files in a bundle.
///
/// Returns a `Vec<ValidationResult>` for the `structural_toml` layer.
/// The caller adds these to a `ValidationReport`.
pub fn validate_structural(bundle_dir: &Path) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    // ── 1. Bundle manifest ────────────────────────────────────────
    let manifest_path = bundle_dir.join("manifest.toml");
    let manifest_rel = "manifest.toml";

    let bundle_value = match parse_toml_file(&manifest_path, manifest_rel) {
        Ok(v) => v,
        Err(r) => {
            results.push(r);
            return results; // Cannot continue without bundle manifest
        }
    };

    // Structural checks on bundle manifest
    if let Some(table) = bundle_value.as_table() {
        check_unknown_keys(table, BUNDLE_MANIFEST_FIELDS, manifest_rel, "", &mut results);
        check_required_fields(table, BUNDLE_MANIFEST_REQUIRED, manifest_rel, "", &mut results);

        // S010 — bundleVersion semver check
        if let Some(v) = table.get("bundleVersion") {
            if let Some(s) = v.as_str() {
                if !is_valid_semver(s) {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S010,
                            manifest_rel,
                            format!("bundleVersion '{}' is not valid semver (expected X.Y.Z)", s),
                        )
                        .with_field("bundleVersion"),
                    );
                }
            } else {
                results.push(
                    ValidationResult::fail(
                        error_codes::S004,
                        manifest_rel,
                        "bundleVersion must be a string",
                    )
                    .with_field("bundleVersion"),
                );
            }
        }

        // Validate apps is an array of strings
        if let Some(apps) = table.get("apps") {
            if let Some(arr) = apps.as_array() {
                for (i, item) in arr.iter().enumerate() {
                    if !item.is_str() {
                        results.push(
                            ValidationResult::fail(
                                error_codes::S004,
                                manifest_rel,
                                format!("apps[{}] must be a string", i),
                            )
                            .with_field("apps"),
                        );
                    }
                }
            } else {
                results.push(
                    ValidationResult::fail(
                        error_codes::S004,
                        manifest_rel,
                        "apps must be an array",
                    )
                    .with_field("apps"),
                );
            }
        }
    } else {
        results.push(ValidationResult::fail(
            error_codes::S001,
            manifest_rel,
            "bundle manifest.toml root is not a table",
        ));
        return results;
    }

    // Pass for bundle manifest if no errors so far
    if results.iter().all(|r| r.status == crate::validate_result::ValidationStatus::Pass) {
        results.push(ValidationResult::pass(manifest_rel, "bundle manifest valid"));
    }

    // ── 2. Per-app files ──────────────────────────────────────────
    let apps = extract_app_names(&bundle_value);
    for app_name in &apps {
        let app_dir = bundle_dir.join(app_name);

        // App manifest
        let app_manifest_rel = format!("{}/manifest.toml", app_name);
        let app_manifest_path = app_dir.join("manifest.toml");
        match parse_toml_file(&app_manifest_path, &app_manifest_rel) {
            Ok(v) => validate_app_manifest(&v, &app_manifest_rel, app_name, &mut results),
            Err(r) => results.push(r),
        }

        // Resources
        let resources_rel = format!("{}/resources.toml", app_name);
        let resources_path = app_dir.join("resources.toml");
        match parse_toml_file(&resources_path, &resources_rel) {
            Ok(v) => validate_resources(&v, &resources_rel, app_name, &mut results),
            Err(r) => results.push(r),
        }

        // App config
        let app_toml_rel = format!("{}/app.toml", app_name);
        let app_toml_path = app_dir.join("app.toml");
        match parse_toml_file(&app_toml_path, &app_toml_rel) {
            Ok(v) => validate_app_config(&v, &app_toml_rel, app_name, &mut results),
            Err(r) => results.push(r),
        }
    }

    results
}

// ── File parsing ──────────────────────────────────────────────────

/// Parse a TOML file into a `toml::Value`. Returns `Err(ValidationResult)` on failure.
fn parse_toml_file(path: &Path, rel_path: &str) -> Result<toml::Value, ValidationResult> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        ValidationResult::fail(
            error_codes::S001,
            rel_path,
            format!("cannot read file: {}", e),
        )
    })?;

    content.parse::<toml::Value>().map_err(|e| {
        ValidationResult::fail(
            error_codes::S001,
            rel_path,
            format!("TOML parse error: {}", e),
        )
    })
}

// ── App manifest validation ───────────────────────────────────────

fn validate_app_manifest(
    value: &toml::Value,
    file: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(error_codes::S001, file, "app manifest root is not a table")
                    .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, APP_MANIFEST_FIELDS, file, "", results);
    check_required_fields(table, APP_MANIFEST_REQUIRED, file, "", results);

    // S008 — appId UUID format
    if let Some(v) = table.get("appId") {
        if let Some(s) = v.as_str() {
            if !is_valid_uuid(s) {
                results.push(
                    ValidationResult::fail(
                        error_codes::S008,
                        file,
                        format!("appId '{}' is not a valid UUID (expected 8-4-4-4-12 hex)", s),
                    )
                    .with_field("appId")
                    .with_app(app_name),
                );
            }
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "appId must be a string")
                    .with_field("appId")
                    .with_app(app_name),
            );
        }
    }

    // S009 — type enum
    if let Some(v) = table.get("type") {
        if let Some(s) = v.as_str() {
            if s != "app-main" && s != "app-service" {
                results.push(
                    ValidationResult::fail(
                        error_codes::S009,
                        file,
                        format!("type '{}' is invalid — must be 'app-main' or 'app-service'", s),
                    )
                    .with_field("type")
                    .with_app(app_name),
                );
            }
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "type must be a string")
                    .with_field("type")
                    .with_app(app_name),
            );
        }
    }

    // Validate spa sub-table if present
    if let Some(spa) = table.get("spa") {
        if let Some(spa_table) = spa.as_table() {
            check_unknown_keys(spa_table, SPA_FIELDS, file, "spa", results);
            check_required_fields(spa_table, SPA_REQUIRED, file, "spa", results);
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "spa must be a table")
                    .with_field("spa")
                    .with_app(app_name),
            );
        }
    }

    // Validate init sub-table if present
    if let Some(init) = table.get("init") {
        if let Some(init_table) = init.as_table() {
            check_unknown_keys(init_table, INIT_FIELDS, file, "init", results);
            check_required_fields(init_table, INIT_REQUIRED, file, "init", results);
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "init must be a table")
                    .with_field("init")
                    .with_app(app_name),
            );
        }
    }

    // If no errors were added for this file, emit a pass
    let has_errors = results.iter().any(|r| {
        r.file.as_deref() == Some(file)
            && r.status == crate::validate_result::ValidationStatus::Fail
    });
    if !has_errors {
        results.push(
            ValidationResult::pass(file, "app manifest valid").with_app(app_name),
        );
    }
}

// ── Resources validation ──────────────────────────────────────────

fn validate_resources(
    value: &toml::Value,
    file: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::S001,
                    file,
                    "resources.toml root is not a table",
                )
                .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, RESOURCES_FIELDS, file, "", results);

    // Validate [[datasources]]
    if let Some(ds) = table.get("datasources") {
        if let Some(arr) = ds.as_array() {
            for (i, entry) in arr.iter().enumerate() {
                let table_path = format!("datasources[{}]", i);
                if let Some(entry_table) = entry.as_table() {
                    check_unknown_keys(entry_table, DATASOURCE_DECL_FIELDS, file, &table_path, results);
                    check_required_fields(entry_table, DATASOURCE_DECL_REQUIRED, file, &table_path, results);

                    // S006 — nopassword and lockbox mutual exclusion
                    let has_nopassword = entry_table
                        .get("nopassword")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let has_lockbox = entry_table.get("lockbox").is_some();

                    if has_nopassword && has_lockbox {
                        results.push(
                            ValidationResult::fail(
                                error_codes::S006,
                                file,
                                format!(
                                    "'nopassword=true' and 'lockbox' are mutually exclusive in {}",
                                    table_path
                                ),
                            )
                            .with_table_path(&table_path)
                            .with_app(app_name),
                        );
                    }
                } else {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S004,
                            file,
                            format!("{} must be a table", table_path),
                        )
                        .with_table_path(&table_path)
                        .with_app(app_name),
                    );
                }
            }
        } else {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    "datasources must be an array of tables",
                )
                .with_field("datasources")
                .with_app(app_name),
            );
        }
    }

    // Validate [[services]]
    if let Some(svcs) = table.get("services") {
        if let Some(arr) = svcs.as_array() {
            for (i, entry) in arr.iter().enumerate() {
                let table_path = format!("services[{}]", i);
                if let Some(entry_table) = entry.as_table() {
                    check_unknown_keys(entry_table, SERVICE_DECL_FIELDS, file, &table_path, results);
                    check_required_fields(entry_table, SERVICE_DECL_REQUIRED, file, &table_path, results);

                    // S008 — appId UUID format in service declarations
                    if let Some(v) = entry_table.get("appId") {
                        if let Some(s) = v.as_str() {
                            if !is_valid_uuid(s) {
                                results.push(
                                    ValidationResult::fail(
                                        error_codes::S008,
                                        file,
                                        format!(
                                            "appId '{}' in {} is not a valid UUID",
                                            s, table_path,
                                        ),
                                    )
                                    .with_table_path(&table_path)
                                    .with_field("appId")
                                    .with_app(app_name),
                                );
                            }
                        }
                    }
                } else {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S004,
                            file,
                            format!("{} must be a table", table_path),
                        )
                        .with_table_path(&table_path)
                        .with_app(app_name),
                    );
                }
            }
        }
    }

    // Validate [[keystores]]
    if let Some(ks) = table.get("keystores") {
        if let Some(arr) = ks.as_array() {
            for (i, entry) in arr.iter().enumerate() {
                let table_path = format!("keystores[{}]", i);
                if let Some(entry_table) = entry.as_table() {
                    check_unknown_keys(entry_table, KEYSTORE_DECL_FIELDS, file, &table_path, results);
                    check_required_fields(entry_table, KEYSTORE_DECL_REQUIRED, file, &table_path, results);
                } else {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S004,
                            file,
                            format!("{} must be a table", table_path),
                        )
                        .with_table_path(&table_path)
                        .with_app(app_name),
                    );
                }
            }
        }
    }

    // Emit pass if no errors
    let has_errors = results.iter().any(|r| {
        r.file.as_deref() == Some(file)
            && r.status == crate::validate_result::ValidationStatus::Fail
    });
    if !has_errors {
        results.push(
            ValidationResult::pass(file, "resources valid").with_app(app_name),
        );
    }
}

// ── App config validation ─────────────────────────────────────────

fn validate_app_config(
    value: &toml::Value,
    file: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(error_codes::S001, file, "app.toml root is not a table")
                    .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, APP_CONFIG_FIELDS, file, "", results);

    // [data] section
    if let Some(data) = table.get("data") {
        if let Some(data_table) = data.as_table() {
            check_unknown_keys(data_table, DATA_CONFIG_FIELDS, file, "data", results);

            // data.datasources — map of datasource configs (runtime detail, not validated structurally here
            // beyond unknown key at the data level — datasource config structs are complex and runtime-owned)

            // data.dataviews — map of DataView configs
            if let Some(dvs) = data_table.get("dataviews") {
                if let Some(dvs_table) = dvs.as_table() {
                    for (dv_name, dv_value) in dvs_table {
                        let table_path = format!("data.dataviews.{}", dv_name);
                        validate_dataview(dv_value, file, &table_path, app_name, results);
                    }
                }
            }
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "data must be a table")
                    .with_field("data")
                    .with_app(app_name),
            );
        }
    }

    // [api] section
    if let Some(api) = table.get("api") {
        if let Some(api_table) = api.as_table() {
            check_unknown_keys(api_table, API_CONFIG_FIELDS, file, "api", results);

            if let Some(views) = api_table.get("views") {
                if let Some(views_table) = views.as_table() {
                    for (view_name, view_value) in views_table {
                        let table_path = format!("api.views.{}", view_name);
                        validate_view(view_value, file, &table_path, app_name, results);
                    }
                }
            }
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "api must be a table")
                    .with_field("api")
                    .with_app(app_name),
            );
        }
    }

    // [static_files] section
    if let Some(sf) = table.get("static_files") {
        if let Some(sf_table) = sf.as_table() {
            check_unknown_keys(sf_table, STATIC_FILES_FIELDS, file, "static_files", results);
        } else {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    "static_files must be a table",
                )
                .with_field("static_files")
                .with_app(app_name),
            );
        }
    }

    // Emit pass if no errors for this file
    let has_errors = results.iter().any(|r| {
        r.file.as_deref() == Some(file)
            && r.status == crate::validate_result::ValidationStatus::Fail
    });
    if !has_errors {
        results.push(
            ValidationResult::pass(file, "app config valid").with_app(app_name),
        );
    }
}

// ── DataView validation ───────────────────────────────────────────

fn validate_dataview(
    value: &toml::Value,
    file: &str,
    table_path: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    format!("{} must be a table", table_path),
                )
                .with_table_path(table_path)
                .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, DATAVIEW_FIELDS, file, table_path, results);
    check_required_fields(table, DATAVIEW_REQUIRED, file, table_path, results);

    // Validate caching sub-table
    if let Some(caching) = table.get("caching") {
        if let Some(caching_table) = caching.as_table() {
            let caching_path = format!("{}.caching", table_path);
            check_unknown_keys(caching_table, CACHING_FIELDS, file, &caching_path, results);
            check_required_fields(caching_table, CACHING_REQUIRED, file, &caching_path, results);
        }
    }

    // S-DV-1: warn when skip_introspect = true but a GET query is present —
    // read DataViews should not need to skip introspection.
    let has_skip = table.get("skip_introspect")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_get_query = table.get("get_query")
        .or_else(|| table.get("query"))
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_skip && has_get_query {
        results.push(
            ValidationResult::warn(
                error_codes::W005,
                format!(
                    "{}: skip_introspect = true on a DataView with a GET query — \
                     likely a misconfiguration; skip_introspect is intended for \
                     mutation DataViews",
                    table_path
                ),
            )
            .with_table_path(table_path)
            .with_app(app_name),
        );
    }

    // Validate parameters arrays
    for param_key in &[
        "parameters",
        "get_parameters",
        "post_parameters",
        "put_parameters",
        "delete_parameters",
    ] {
        if let Some(params) = table.get(*param_key) {
            if let Some(arr) = params.as_array() {
                for (i, p) in arr.iter().enumerate() {
                    let param_path = format!("{}.{}[{}]", table_path, param_key, i);
                    if let Some(pt) = p.as_table() {
                        check_unknown_keys(pt, PARAMETER_FIELDS, file, &param_path, results);
                        check_required_fields(pt, PARAMETER_REQUIRED, file, &param_path, results);
                    }
                }
            }
        }
    }
}

// ── View validation ───────────────────────────────────────────────

fn validate_view(
    value: &toml::Value,
    file: &str,
    table_path: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    format!("{} must be a table", table_path),
                )
                .with_table_path(table_path)
                .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, VIEW_FIELDS, file, table_path, results);

    // Enum-validate `view_type` and `auth` (Sprint 2026-05-09 Track 2).
    // Catches CB-probe-style silent passes — e.g. `view_type = "Cron"`
    // (P1.14 pending) and `auth = "bearer"` (P1.12 closed) used to slide
    // through structural and become no-ops at runtime; they now surface
    // as S005 with a did-you-mean hint.
    if let Some(vt) = table.get("view_type") {
        validate_view_type(vt, file, table_path, app_name, results);
    }
    if let Some(au) = table.get("auth") {
        validate_auth_mode(au, file, table_path, app_name, results);
    }

    // MessageConsumer views are event-driven — no HTTP route, so path/method
    // are forbidden by the runtime validator (see crates/riversd/src/
    // view_engine/validation.rs). Cron views are time-driven, same shape:
    // no HTTP route. Restrict the required-fields check to the common
    // view_type + handler pair for those views.
    let view_type = table.get("view_type").and_then(|v| v.as_str()).unwrap_or("");
    let required: &[&str] = if view_type == "MessageConsumer" || view_type == "Cron" {
        &["view_type", "handler"]
    } else {
        VIEW_REQUIRED
    };
    check_required_fields(table, required, file, table_path, results);

    // Cron-view-specific structural rules (CB-P1.14, Track 3).
    if view_type == "Cron" {
        validate_cron_view(table, file, table_path, app_name, results);
    } else {
        // Cron-only fields on non-Cron views are no-ops at runtime — flag as
        // S005 so the misuse surfaces at validation rather than silently.
        for f in &["schedule", "interval_seconds", "overlap_policy", "max_concurrent"] {
            if table.contains_key(*f) {
                results.push(
                    ValidationResult::fail(
                        error_codes::S005,
                        file,
                        format!(
                            "{}.{} is only valid when view_type=\"Cron\"",
                            table_path, f
                        ),
                    )
                    .with_table_path(table_path)
                    .with_field(*f)
                    .with_app(app_name),
                );
            }
        }
    }

    // Validate handler sub-table
    if let Some(handler) = table.get("handler") {
        if let Some(handler_table) = handler.as_table() {
            let handler_path = format!("{}.handler", table_path);
            check_unknown_keys(handler_table, HANDLER_FIELDS, file, &handler_path, results);
            check_required_fields(handler_table, HANDLER_REQUIRED, file, &handler_path, results);
        }
    }

    // Validate parameter_mapping sub-table
    if let Some(pm) = table.get("parameter_mapping") {
        if let Some(pm_table) = pm.as_table() {
            let pm_path = format!("{}.parameter_mapping", table_path);
            check_unknown_keys(pm_table, PARAM_MAPPING_FIELDS, file, &pm_path, results);
        }
    }

    // Validate MCP federation array (if present — MCP view type only).
    if let Some(fed_val) = table.get("federation") {
        match fed_val {
            toml::Value::Array(arr) => {
                for (i, entry) in arr.iter().enumerate() {
                    let fed_path = format!("{}.federation[{}]", table_path, i);
                    if let Some(fed_table) = entry.as_table() {
                        check_unknown_keys(fed_table, MCP_FEDERATION_FIELDS, file, &fed_path, results);
                        check_required_fields(fed_table, MCP_FEDERATION_REQUIRED, file, &fed_path, results);
                    } else {
                        results.push(
                            ValidationResult::fail(
                                error_codes::S004,
                                file,
                                format!("{} must be a table", fed_path),
                            )
                            .with_table_path(&fed_path)
                            .with_app(app_name),
                        );
                    }
                }
            }
            toml::Value::Table(tbl) => {
                // Allow inline table form: [api.views.*.federation.alias_name]
                for (fed_name, fed_value) in tbl {
                    let fed_path = format!("{}.federation.{}", table_path, fed_name);
                    if let Some(fed_table) = fed_value.as_table() {
                        check_unknown_keys(fed_table, MCP_FEDERATION_FIELDS, file, &fed_path, results);
                        check_required_fields(fed_table, MCP_FEDERATION_REQUIRED, file, &fed_path, results);
                    } else {
                        results.push(
                            ValidationResult::fail(
                                error_codes::S004,
                                file,
                                format!("{} must be a table", fed_path),
                            )
                            .with_table_path(&fed_path)
                            .with_app(app_name),
                        );
                    }
                }
            }
            _ => {
                results.push(
                    ValidationResult::fail(
                        error_codes::S004,
                        file,
                        format!("{}.federation must be an array or table", table_path),
                    )
                    .with_table_path(table_path)
                    .with_field("federation")
                    .with_app(app_name),
                );
            }
        }
    }

    // Validate [api.views.*.response_headers] (CB-P1.11).
    // Names must be RFC 7230 tokens; values must be ASCII-printable; a small
    // reserved set is rejected because the framework manages those headers.
    if let Some(rh) = table.get("response_headers") {
        validate_response_headers(rh, file, table_path, app_name, results);
    }

    // Validate MCP resources sub-table (if present — MCP view type only).
    // S-MCP-2: warn when a resource has subscribable = true but view method is not GET.
    if let Some(resources_val) = table.get("resources") {
        if let Some(resources_table) = resources_val.as_table() {
            for (res_name, res_value) in resources_table {
                let res_path = format!("{}.resources.{}", table_path, res_name);
                if let Some(res_table) = res_value.as_table() {
                    check_unknown_keys(res_table, MCP_RESOURCE_FIELDS, file, &res_path, results);
                    check_required_fields(res_table, MCP_RESOURCE_REQUIRED, file, &res_path, results);

                    // S-MCP-2: subscribable = true with a non-GET view method is likely wrong.
                    let subscribable = res_table
                        .get("subscribable")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if subscribable {
                        let method = table
                            .get("method")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !method.is_empty() && method.to_uppercase() != "GET" {
                            results.push(
                                ValidationResult::warn(
                                    error_codes::W006,
                                    format!(
                                        "{}: subscribable = true but view method is '{}' — \
                                         subscriptions require a GET-capable DataView; \
                                         consider setting method = \"GET\" or subscribable = false",
                                        res_path, method
                                    ),
                                )
                                .with_table_path(&res_path)
                                .with_app(app_name),
                            );
                        }
                    }
                }
            }
        }
    }
}

// ── Core helpers ──────────────────────────────────────────────────

/// Check for unknown keys in a TOML table. Emits S002 for each unknown key
/// with a "did you mean?" suggestion if a close match exists.
fn check_unknown_keys(
    table: &toml::map::Map<String, toml::Value>,
    known: &[&str],
    file: &str,
    table_path: &str,
    results: &mut Vec<ValidationResult>,
) {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            let mut result = ValidationResult::warn(
                error_codes::S002,
                format!("unknown key '{}' in [{}]", key, if table_path.is_empty() { "root" } else { table_path }),
            )
            .with_table_path(if table_path.is_empty() { "root" } else { table_path })
            .with_field(key.clone());

            result.file = Some(file.to_string());

            if let Some(suggestion) = suggest_key(key, known) {
                result = result.with_suggestion(suggestion);
            }

            results.push(result);
        }
    }
}

/// Validate Cron-view-specific structural rules (CB-P1.14, Track 3).
///
/// Walks `[api.views.<name>]` when `view_type = "Cron"` and enforces:
/// - Exactly one of `schedule` (cron expression) or `interval_seconds`.
/// - `schedule` parses via the `cron` crate.
/// - `interval_seconds >= 1`.
/// - `overlap_policy ∈ {skip, queue, allow}` if set.
/// - `path`, `method`, `auth`, `guard_view`, `response_headers` not allowed.
///
/// All errors are `S005`. The required-fields path/method check is suppressed
/// elsewhere when `view_type = "Cron"` (see [`super::validate_structural`] —
/// the per-view walker branches on `view_type` for `VIEW_REQUIRED`).
fn validate_cron_view(
    table: &toml::value::Table,
    file: &str,
    table_path: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    // Forbidden fields for Cron — they have no semantic meaning when there is
    // no caller. Each is its own S005 so all problems surface in one pass.
    const FORBIDDEN: &[&str] = &[
        "path", "method", "auth", "guard_view", "response_headers",
        "polling", "tools", "resources", "prompts", "instructions",
        "session", "federation", "websocket_mode", "max_connections",
        "sse_tick_interval_ms", "sse_trigger_events", "sse_event_buffer_size",
        "session_revalidation_interval_s", "streaming", "streaming_format",
        "stream_timeout_ms",
    ];
    for f in FORBIDDEN {
        if table.contains_key(*f) {
            results.push(
                ValidationResult::fail(
                    error_codes::S005,
                    file,
                    format!(
                        "{}.{} is not allowed on view_type=\"Cron\" — Cron views have no caller",
                        table_path, f
                    ),
                )
                .with_table_path(table_path)
                .with_field(*f)
                .with_app(app_name),
            );
        }
    }

    let has_schedule = table.contains_key("schedule");
    let has_interval = table.contains_key("interval_seconds");

    match (has_schedule, has_interval) {
        (true, true) => {
            results.push(
                ValidationResult::fail(
                    error_codes::S005,
                    file,
                    format!(
                        "{} declares both `schedule` and `interval_seconds` — exactly one is required for view_type=\"Cron\"",
                        table_path
                    ),
                )
                .with_table_path(table_path)
                .with_app(app_name),
            );
        }
        (false, false) => {
            results.push(
                ValidationResult::fail(
                    error_codes::S005,
                    file,
                    format!(
                        "{} requires exactly one of `schedule` (cron expression) or `interval_seconds` for view_type=\"Cron\"",
                        table_path
                    ),
                )
                .with_table_path(table_path)
                .with_app(app_name),
            );
        }
        (true, false) => {
            if let Some(s) = table.get("schedule").and_then(|v| v.as_str()) {
                // The `cron` crate parses 6- or 7-field expressions through
                // `Schedule::from_str`. Wrap parse failure as S005 with the
                // parse error message so users see exactly what the parser
                // didn't like.
                if let Err(e) = <cron::Schedule as std::str::FromStr>::from_str(s) {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S005,
                            file,
                            format!(
                                "{}.schedule '{}' is not a valid cron expression: {}",
                                table_path, s, e
                            ),
                        )
                        .with_table_path(table_path)
                        .with_field("schedule")
                        .with_app(app_name),
                    );
                }
            } else {
                results.push(
                    ValidationResult::fail(
                        error_codes::S004,
                        file,
                        format!("{}.schedule must be a string", table_path),
                    )
                    .with_table_path(table_path)
                    .with_field("schedule")
                    .with_app(app_name),
                );
            }
        }
        (false, true) => {
            // interval_seconds — must be a positive integer.
            let v = table.get("interval_seconds").unwrap();
            match v.as_integer() {
                Some(n) if n >= 1 => {}
                Some(n) => {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S005,
                            file,
                            format!(
                                "{}.interval_seconds must be >= 1, got {}",
                                table_path, n
                            ),
                        )
                        .with_table_path(table_path)
                        .with_field("interval_seconds")
                        .with_app(app_name),
                    );
                }
                None => {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S004,
                            file,
                            format!(
                                "{}.interval_seconds must be a positive integer",
                                table_path
                            ),
                        )
                        .with_table_path(table_path)
                        .with_field("interval_seconds")
                        .with_app(app_name),
                    );
                }
            }
        }
    }

    if let Some(op_val) = table.get("overlap_policy") {
        match op_val.as_str() {
            Some(s) if VALID_OVERLAP_POLICIES.contains(&s) => {}
            Some(s) => {
                let mut msg = format!(
                    "{}.overlap_policy '{}' is not one of [{}]",
                    table_path,
                    s,
                    VALID_OVERLAP_POLICIES.join(", "),
                );
                if let Some(hint) = suggest_key(s, VALID_OVERLAP_POLICIES) {
                    msg.push_str(" — ");
                    msg.push_str(&hint);
                }
                results.push(
                    ValidationResult::fail(error_codes::S005, file, msg)
                        .with_table_path(table_path)
                        .with_field("overlap_policy")
                        .with_app(app_name),
                );
            }
            None => {
                results.push(
                    ValidationResult::fail(
                        error_codes::S004,
                        file,
                        format!("{}.overlap_policy must be a string", table_path),
                    )
                    .with_table_path(table_path)
                    .with_field("overlap_policy")
                    .with_app(app_name),
                );
            }
        }
    }
}

/// Validate `view_type` against the canonical set (Sprint 2026-05-09 Track 2).
///
/// Emits `S005` for any value outside [`VALID_VIEW_TYPES`] with a
/// did-you-mean suggestion when the typo is close. A missing `view_type` is
/// handled by [`VIEW_REQUIRED`]'s required-fields check, not here.
fn validate_view_type(
    value: &toml::Value,
    file: &str,
    table_path: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let s = match value.as_str() {
        Some(s) => s,
        None => return,
    };
    if VALID_VIEW_TYPES.contains(&s) {
        return;
    }
    let mut msg = format!(
        "{}.view_type '{}' is not one of [{}]",
        table_path,
        s,
        VALID_VIEW_TYPES.join(", "),
    );
    if let Some(hint) = suggest_key(s, VALID_VIEW_TYPES) {
        msg.push_str(" — ");
        msg.push_str(&hint);
    }
    results.push(
        ValidationResult::fail(error_codes::S005, file, msg)
            .with_table_path(table_path)
            .with_field("view_type")
            .with_app(app_name),
    );
}

/// Validate `auth` against the canonical set (Sprint 2026-05-09 Track 2).
///
/// Emits `S005` for any value outside [`VALID_AUTH_MODES`] with a
/// did-you-mean. `auth` is optional, so `None` here is a no-op.
fn validate_auth_mode(
    value: &toml::Value,
    file: &str,
    table_path: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let s = match value.as_str() {
        Some(s) => s,
        None => return,
    };
    if VALID_AUTH_MODES.contains(&s) {
        return;
    }
    let mut msg = format!(
        "{}.auth '{}' is not one of [{}]",
        table_path,
        s,
        VALID_AUTH_MODES.join(", "),
    );
    if let Some(hint) = suggest_key(s, VALID_AUTH_MODES) {
        msg.push_str(" — ");
        msg.push_str(&hint);
    }
    results.push(
        ValidationResult::fail(error_codes::S005, file, msg)
            .with_table_path(table_path)
            .with_field("auth")
            .with_app(app_name),
    );
}

/// Validate `[api.views.*.response_headers]` (CB-P1.11).
///
/// Rules:
/// - Must be a TOML table.
/// - Header names match RFC 7230 token grammar (alphanumerics + `-`).
/// - Header values must be ASCII-printable (`\x20`–`\x7E`); no control chars.
/// - Reserved framework-managed headers are rejected (case-insensitive):
///   `Content-Type`, `Content-Length`, `Transfer-Encoding`, `Mcp-Session-Id`.
fn validate_response_headers(
    value: &toml::Value,
    file: &str,
    table_path: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    const RESERVED: &[&str] = &[
        "content-type",
        "content-length",
        "transfer-encoding",
        "mcp-session-id",
    ];

    let headers_path = format!("{}.response_headers", table_path);
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    format!("{} must be a table", headers_path),
                )
                .with_table_path(&headers_path)
                .with_app(app_name),
            );
            return;
        }
    };

    for (name, val) in table {
        let name_lc = name.to_ascii_lowercase();
        if RESERVED.iter().any(|r| *r == name_lc) {
            results.push(
                ValidationResult::fail(
                    error_codes::S005,
                    file,
                    format!(
                        "{}.\"{}\" is a framework-managed header and cannot be set via response_headers",
                        headers_path, name
                    ),
                )
                .with_table_path(&headers_path)
                .with_field(name)
                .with_app(app_name),
            );
            continue;
        }
        let name_ok = !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !name_ok {
            results.push(
                ValidationResult::fail(
                    error_codes::S005,
                    file,
                    format!(
                        "{}.\"{}\" is not a valid HTTP header name (alphanumerics and `-` only)",
                        headers_path, name
                    ),
                )
                .with_table_path(&headers_path)
                .with_field(name)
                .with_app(app_name),
            );
            continue;
        }

        let s = match val.as_str() {
            Some(s) => s,
            None => {
                results.push(
                    ValidationResult::fail(
                        error_codes::S004,
                        file,
                        format!("{}.\"{}\" must be a string", headers_path, name),
                    )
                    .with_table_path(&headers_path)
                    .with_field(name)
                    .with_app(app_name),
                );
                continue;
            }
        };
        if !s.bytes().all(|b| (0x20..=0x7E).contains(&b)) {
            results.push(
                ValidationResult::fail(
                    error_codes::S005,
                    file,
                    format!(
                        "{}.\"{}\" contains non-printable or non-ASCII bytes; HTTP header values must be ASCII-printable",
                        headers_path, name
                    ),
                )
                .with_table_path(&headers_path)
                .with_field(name)
                .with_app(app_name),
            );
        }
    }
}

/// Check for missing required fields. Emits S003 for each missing field.
fn check_required_fields(
    table: &toml::map::Map<String, toml::Value>,
    required: &[&str],
    file: &str,
    table_path: &str,
    results: &mut Vec<ValidationResult>,
) {
    for &field in required {
        if !table.contains_key(field) {
            results.push(
                ValidationResult::fail(
                    error_codes::S003,
                    file,
                    format!(
                        "missing required field '{}' in [{}]",
                        field,
                        if table_path.is_empty() { "root" } else { table_path },
                    ),
                )
                .with_table_path(if table_path.is_empty() { "root" } else { table_path })
                .with_field(field),
            );
        }
    }
}

// ── Value validators ──────────────────────────────────────────────

/// Check if a string is a valid UUID (8-4-4-4-12 hex pattern).
fn is_valid_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8, 4, 4, 4, 12];
    for (part, &expected) in parts.iter().zip(expected_lens.iter()) {
        if part.len() != expected || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

/// Check if a string is valid semver (X.Y.Z where X, Y, Z are non-negative integers).
fn is_valid_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    for part in &parts {
        if part.is_empty() {
            return false;
        }
        // No leading zeros (except "0" itself)
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    true
}

/// Extract app directory names from a parsed bundle manifest value.
fn extract_app_names(value: &toml::Value) -> Vec<String> {
    value
        .get("apps")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate_result::ValidationStatus;
    use std::path::PathBuf;

    /// Create a minimal valid bundle in a temporary directory.
    fn create_valid_bundle(dir: &Path) -> PathBuf {
        let bundle_dir = dir.join("test-bundle");
        let app_dir = bundle_dir.join("test-app");
        let schemas_dir = app_dir.join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        // Bundle manifest
        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0.0"
source        = "https://test.example.com"
apps          = ["test-app"]
"#,
        )
        .unwrap();

        // App manifest
        std::fs::write(
            app_dir.join("manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-service"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "service"
source     = "https://test.example.com"
"#,
        )
        .unwrap();

        // Resources
        std::fs::write(
            app_dir.join("resources.toml"),
            r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true
"#,
        )
        .unwrap();

        // App config
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"
query      = "schemas/item.schema.json"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();

        bundle_dir
    }

    // ── Valid bundle passes ────────────────────────────────────────

    #[test]
    fn valid_bundle_all_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        let results = validate_structural(&bundle_dir);

        let fails: Vec<_> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Fail)
            .collect();

        assert!(
            fails.is_empty(),
            "expected no failures but got: {:?}",
            fails
        );

        // Should have pass results for each file
        let passes: Vec<_> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Pass)
            .collect();
        assert!(
            passes.len() >= 4,
            "expected at least 4 pass results, got {}",
            passes.len()
        );
    }

    // ── S001: TOML parse error ────────────────────────────────────

    #[test]
    fn s001_invalid_toml_syntax() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Corrupt the bundle manifest
        std::fs::write(
            bundle_dir.join("manifest.toml"),
            "bundleName = [invalid toml",
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S001"))
            .expect("expected S001 error");

        assert_eq!(fail.file.as_deref(), Some("manifest.toml"));
        assert!(fail.message.contains("TOML parse error"));
    }

    // ── S002: unknown key with did-you-mean ───────────────────────

    #[test]
    fn s002_unknown_key_in_bundle_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Add unknown key
        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0.0"
source        = "test"
apps          = ["test-app"]
bundleNme     = "typo"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S002"))
            .expect("expected S002 error");

        assert_eq!(fail.file.as_deref(), Some("manifest.toml"));
        assert!(fail.message.contains("bundleNme"));
        assert!(
            fail.suggestion.as_ref().unwrap().contains("bundleName"),
            "suggestion should mention bundleName, got: {:?}",
            fail.suggestion
        );
    }

    #[test]
    fn s002_unknown_key_in_view() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
veiew_type = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("veiew_type")
            })
            .expect("expected S002 for veiew_type");

        assert!(fail.suggestion.as_ref().unwrap().contains("view_type"));
        assert_eq!(
            fail.table_path.as_deref(),
            Some("api.views.items")
        );
    }

    // ── S003: missing required field ──────────────────────────────

    #[test]
    fn s003_missing_required_field_bundle_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Missing apps
        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0.0"
source        = "test"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S003")
                    && r.field.as_deref() == Some("apps")
            })
            .expect("expected S003 for 'apps'");

        assert_eq!(fail.file.as_deref(), Some("manifest.toml"));
    }

    #[test]
    fn s003_missing_required_field_datasource() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Missing driver
        std::fs::write(
            bundle_dir.join("test-app/resources.toml"),
            r#"
[[datasources]]
name       = "data"
x-type     = "faker"
nopassword = true
required   = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S003")
                    && r.field.as_deref() == Some("driver")
            })
            .expect("expected S003 for missing 'driver'");

        assert_eq!(
            fail.file.as_deref(),
            Some("test-app/resources.toml")
        );
    }

    // ── S004: wrong type ──────────────────────────────────────────

    #[test]
    fn s004_apps_not_array() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0.0"
source        = "test"
apps          = "not-an-array"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S004")
                    && r.field.as_deref() == Some("apps")
            })
            .expect("expected S004 for apps type");

        assert!(fail.message.contains("must be an array"));
    }

    // ── S006: nopassword and lockbox mutual exclusion ─────────────

    #[test]
    fn s006_nopassword_and_lockbox() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/resources.toml"),
            r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
lockbox    = "lockbox://db/test"
required   = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S006"))
            .expect("expected S006 error");

        assert!(fail.message.contains("mutually exclusive"));
    }

    // ── S008: appId not valid UUID ────────────────────────────────

    #[test]
    fn s008_invalid_uuid() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-service"
appId      = "not-a-uuid"
entryPoint = "service"
source     = "test"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S008"))
            .expect("expected S008 error");

        assert!(fail.message.contains("not a valid UUID"));
        assert_eq!(fail.field.as_deref(), Some("appId"));
    }

    #[test]
    fn s008_valid_uuid_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Default test bundle has a valid UUID, so just check no S008
        let results = validate_structural(&bundle_dir);
        let s008 = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S008"));
        assert!(s008.is_none(), "valid UUID should not produce S008");
    }

    // ── S009: invalid app_type ────────────────────────────────────

    #[test]
    fn s009_invalid_app_type() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "worker"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "service"
source     = "test"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S009"))
            .expect("expected S009 error");

        assert!(fail.message.contains("worker"));
        assert!(fail.message.contains("app-main"));
        assert!(fail.message.contains("app-service"));
    }

    // ── S010: invalid semver ──────────────────────────────────────

    #[test]
    fn s010_invalid_semver() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0"
source        = "test"
apps          = ["test-app"]
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S010"))
            .expect("expected S010 error");

        assert!(fail.message.contains("not valid semver"));
    }

    #[test]
    fn s010_leading_zero_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "01.0.0"
source        = "test"
apps          = ["test-app"]
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S010"));
        assert!(fail.is_some(), "leading zeros should be invalid semver");
    }

    // ── UUID validator unit tests ─────────────────────────────────

    #[test]
    fn uuid_validator() {
        assert!(is_valid_uuid("aaaaaaaa-bbbb-cccc-dddd-000000000001"));
        assert!(is_valid_uuid("c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a"));
        assert!(is_valid_uuid("F47AC10B-58CC-4372-A567-0E02B2C3D479"));
        assert!(!is_valid_uuid("not-a-uuid"));
        assert!(!is_valid_uuid("aaaaaaaa-bbbb-cccc-dddd"));
        assert!(!is_valid_uuid("aaaaaaaa-bbbb-cccc-dddd-00000000000g"));
        assert!(!is_valid_uuid(""));
        assert!(!is_valid_uuid("aaaaaaaa_bbbb_cccc_dddd_000000000001"));
    }

    // ── Semver validator unit tests ───────────────────────────────

    #[test]
    fn semver_validator() {
        assert!(is_valid_semver("1.0.0"));
        assert!(is_valid_semver("0.0.1"));
        assert!(is_valid_semver("10.20.30"));
        assert!(!is_valid_semver("1.0"));
        assert!(!is_valid_semver("1.0.0.0"));
        assert!(!is_valid_semver("v1.0.0"));
        assert!(!is_valid_semver("1.0.0-beta"));
        assert!(!is_valid_semver("01.0.0"));
        assert!(!is_valid_semver(""));
        assert!(!is_valid_semver(".."));
    }

    // ── Address book bundle validation ────────────────────────────

    #[test]
    fn address_book_bundle_validates() {
        let bundle_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../address-book-bundle");

        if !bundle_dir.exists() {
            // Skip if address-book-bundle not present
            return;
        }

        let results = validate_structural(&bundle_dir);

        let fails: Vec<_> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Fail)
            .collect();

        // The address book bundle uses some fields not in the minimal friction-doc
        // field sets (e.g., response_format on views, config sub-table on datasources).
        // Those are intentionally allowed by the field sets — verify no unexpected failures.
        for fail in &fails {
            // Print for debugging if this test fails
            eprintln!(
                "FAIL [{}] {} — {} (field: {:?}, suggestion: {:?})",
                fail.error_code.as_deref().unwrap_or("?"),
                fail.file.as_deref().unwrap_or("?"),
                fail.message,
                fail.field,
                fail.suggestion,
            );
        }

        // We expect zero hard failures — the field sets should cover the address book
        assert!(
            fails.is_empty(),
            "address-book-bundle should pass structural validation, got {} failures",
            fails.len()
        );
    }

    // ── Nested validation ─────────────────────────────────────────

    #[test]
    fn dataview_caching_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[data.dataviews.items.caching]
ttl_seconds  = 60
ttl_secnds   = 30

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("ttl_secnds")
            })
            .expect("expected S002 for unknown 'ttl_secnds' in caching");

        assert!(
            fail.table_path
                .as_ref()
                .unwrap()
                .contains("caching"),
        );
        // "ttl_secnds" is distance 1 from "ttl_seconds", so we get a suggestion
        assert!(
            fail.suggestion
                .as_ref()
                .unwrap()
                .contains("ttl_seconds"),
        );
    }

    #[test]
    fn handler_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type      = "dataview"
dataview  = "items"
dataveiw  = "items"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("dataveiw")
            })
            .expect("expected S002 for unknown 'dataveiw' in handler");

        assert!(fail.suggestion.as_ref().unwrap().contains("dataview"));
    }

    #[test]
    fn parameter_mapping_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"

[api.views.items.parameter_mapping]
qeury = {limit = "limit"}
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("qeury")
            })
            .expect("expected S002 for unknown 'qeury' in parameter_mapping");

        assert!(fail.suggestion.as_ref().unwrap().contains("query"));
    }

    // ── CB-P1.11 response_headers validation ──────────────────────

    #[test]
    fn response_headers_rejects_reserved_names() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"

[api.views.items.response_headers]
"Deprecation"  = "true"
"content-type" = "application/json"
"Mcp-Session-Id" = "abc"
"#,
        )
        .unwrap();
        let results = validate_structural(&bundle_dir);
        let bad: Vec<_> = results.iter()
            .filter(|r| r.error_code.as_deref() == Some("S005")
                && r.message.contains("framework-managed"))
            .collect();
        assert_eq!(bad.len(), 2,
            "expected S005 on both reserved headers, got {}: {:?}",
            bad.len(),
            results.iter().map(|r| (&r.error_code, &r.message)).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn response_headers_rejects_invalid_name_and_value() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"

[api.views.items.response_headers]
"Bad Name" = "ok"
"X-Bell" = "\u0007warn"
"#,
        )
        .unwrap();
        let results = validate_structural(&bundle_dir);
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("S005")
            && r.message.contains("not a valid HTTP header name")),
            "expected S005 for invalid header name (Bad Name)");
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("S005")
            && r.message.contains("non-printable")),
            "expected S005 for non-printable header value (X-Bell)");
    }

    #[test]
    fn response_headers_accepts_valid_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"

[api.views.items.response_headers]
"Deprecation" = "true"
"Sunset"      = "Wed, 31 Dec 2026 23:59:59 GMT"
"Cache-Control" = "max-age=60"
"#,
        )
        .unwrap();
        let results = validate_structural(&bundle_dir);
        let view_failures: Vec<_> = results.iter()
            .filter(|r| r.status == ValidationStatus::Fail
                && r.table_path.as_deref()
                    .map(|p| p.contains("response_headers"))
                    .unwrap_or(false))
            .collect();
        assert!(view_failures.is_empty(),
            "expected no failures on valid response_headers, got: {:?}",
            view_failures.iter().map(|r| (&r.error_code, &r.message)).collect::<Vec<_>>(),
        );
    }

    // ── CB-PROBE Track 2: view_type / auth enum validation ────────

    #[test]
    fn view_type_rejects_unknown_string() {
        // Track 3 added `Cron` to the canonical set, so the original CB
        // probe value ('Cron') now passes view_type. Use a clearly-bogus
        // value to confirm the enum gate still rejects unknowns.
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "QuantumStreamer"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();
        let results = validate_structural(&bundle_dir);
        let bad: Vec<_> = results.iter()
            .filter(|r| r.error_code.as_deref() == Some("S005")
                && r.field.as_deref() == Some("view_type")
                && r.message.contains("'QuantumStreamer'"))
            .collect();
        assert_eq!(bad.len(), 1,
            "expected exactly one S005 on view_type='QuantumStreamer', got: {:?}",
            results.iter().map(|r| (&r.error_code, &r.field, &r.message)).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn view_type_accepts_canonical_values() {
        for vt in &["Rest", "Mcp", "Websocket", "ServerSentEvents", "MessageConsumer"] {
            let tmp = tempfile::tempdir().unwrap();
            let bundle_dir = create_valid_bundle(tmp.path());
            // MessageConsumer doesn't take path/method; skip those for it.
            let body = if *vt == "MessageConsumer" {
                format!(r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
view_type  = "{}"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#, vt)
            } else {
                format!(r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "{}"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#, vt)
            };
            std::fs::write(bundle_dir.join("test-app/app.toml"), body).unwrap();
            let results = validate_structural(&bundle_dir);
            let bad: Vec<_> = results.iter()
                .filter(|r| r.error_code.as_deref() == Some("S005")
                    && r.field.as_deref() == Some("view_type"))
                .collect();
            assert!(bad.is_empty(),
                "view_type='{}' should be accepted, got: {:?}",
                vt,
                bad.iter().map(|r| &r.message).collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn view_type_did_you_mean_suggests_canonical() {
        // Lowercase typo should produce a "did you mean 'Rest'?" hint.
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();
        let results = validate_structural(&bundle_dir);
        let hit = results.iter().find(|r|
            r.error_code.as_deref() == Some("S005")
            && r.field.as_deref() == Some("view_type")
            && r.message.contains("'Rest'")
            && r.message.contains("did you mean")
        );
        assert!(hit.is_some(),
            "expected did-you-mean Rest hint for view_type='rest', got: {:?}",
            results.iter().map(|r| &r.message).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn auth_rejects_unknown_string() {
        // CB probe Case G — `auth = "bearer"` (P1.12 closed-as-superseded)
        // should now produce S005 instead of silently passing.
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "bearer"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();
        let results = validate_structural(&bundle_dir);
        let bad: Vec<_> = results.iter()
            .filter(|r| r.error_code.as_deref() == Some("S005")
                && r.field.as_deref() == Some("auth")
                && r.message.contains("'bearer'"))
            .collect();
        assert_eq!(bad.len(), 1,
            "expected exactly one S005 on auth='bearer', got: {:?}",
            results.iter().map(|r| (&r.error_code, &r.field, &r.message)).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn auth_accepts_canonical_and_omitted() {
        // none + session + omitted (auth field absent) all valid.
        for body in &[
            r#"
[data.dataviews.items]
name = "items"
datasource = "data"

[api.views.items]
path = "items"
method = "GET"
view_type = "Rest"
auth = "none"

[api.views.items.handler]
type = "dataview"
dataview = "items"
"#,
            r#"
[data.dataviews.items]
name = "items"
datasource = "data"

[api.views.items]
path = "items"
method = "GET"
view_type = "Rest"
auth = "session"

[api.views.items.handler]
type = "dataview"
dataview = "items"
"#,
            r#"
[data.dataviews.items]
name = "items"
datasource = "data"

[api.views.items]
path = "items"
method = "GET"
view_type = "Rest"

[api.views.items.handler]
type = "dataview"
dataview = "items"
"#,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let bundle_dir = create_valid_bundle(tmp.path());
            std::fs::write(bundle_dir.join("test-app/app.toml"), body).unwrap();
            let results = validate_structural(&bundle_dir);
            let bad: Vec<_> = results.iter()
                .filter(|r| r.error_code.as_deref() == Some("S005")
                    && r.field.as_deref() == Some("auth"))
                .collect();
            assert!(bad.is_empty(),
                "expected no auth S005, got: {:?}",
                bad.iter().map(|r| &r.message).collect::<Vec<_>>(),
            );
        }
    }

    // ── CB-PROBE Track 3: Cron view validation (P1.14) ────────────

    fn write_cron_view(dir: &Path, body: &str) {
        std::fs::write(
            dir.join("test-app/app.toml"),
            format!(r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

{}
"#, body),
        ).unwrap();
    }

    #[test]
    fn cron_view_accepts_canonical_schedule() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        write_cron_view(&bundle_dir, r#"
[api.views.recompute]
view_type = "Cron"
schedule  = "0 */5 * * * *"
overlap_policy = "skip"

[api.views.recompute.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#);
        // libraries/handlers/recompute.ts isn't required for structural — that's Layer 2.
        let results = validate_structural(&bundle_dir);
        let view_failures: Vec<_> = results.iter()
            .filter(|r| r.status == ValidationStatus::Fail
                && r.table_path.as_deref()
                    .map(|p| p.contains("recompute"))
                    .unwrap_or(false))
            .collect();
        assert!(view_failures.is_empty(),
            "expected no failures on canonical Cron view, got: {:?}",
            view_failures.iter().map(|r| &r.message).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn cron_view_accepts_interval_seconds() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        write_cron_view(&bundle_dir, r#"
[api.views.recompute]
view_type        = "Cron"
interval_seconds = 300

[api.views.recompute.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#);
        let results = validate_structural(&bundle_dir);
        let view_failures: Vec<_> = results.iter()
            .filter(|r| r.status == ValidationStatus::Fail
                && r.table_path.as_deref()
                    .map(|p| p.contains("recompute"))
                    .unwrap_or(false))
            .collect();
        assert!(view_failures.is_empty(),
            "expected no failures on Cron view with interval_seconds, got: {:?}",
            view_failures.iter().map(|r| &r.message).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn cron_view_rejects_both_schedule_and_interval() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        write_cron_view(&bundle_dir, r#"
[api.views.recompute]
view_type        = "Cron"
schedule         = "*/5 * * * * *"
interval_seconds = 300

[api.views.recompute.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#);
        let results = validate_structural(&bundle_dir);
        assert!(results.iter().any(|r|
            r.error_code.as_deref() == Some("S005")
            && r.message.contains("declares both `schedule` and `interval_seconds`")
        ), "expected S005 on schedule+interval_seconds mutex, got: {:?}",
            results.iter().map(|r| &r.message).collect::<Vec<_>>());
    }

    #[test]
    fn cron_view_rejects_neither_schedule_nor_interval() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        write_cron_view(&bundle_dir, r#"
[api.views.recompute]
view_type = "Cron"

[api.views.recompute.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#);
        let results = validate_structural(&bundle_dir);
        assert!(results.iter().any(|r|
            r.error_code.as_deref() == Some("S005")
            && r.message.contains("requires exactly one of `schedule`")
        ), "expected S005 on missing schedule+interval_seconds, got: {:?}",
            results.iter().map(|r| &r.message).collect::<Vec<_>>());
    }

    #[test]
    fn cron_view_rejects_invalid_cron_expression() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        write_cron_view(&bundle_dir, r#"
[api.views.recompute]
view_type = "Cron"
schedule  = "not a cron expression"

[api.views.recompute.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#);
        let results = validate_structural(&bundle_dir);
        assert!(results.iter().any(|r|
            r.error_code.as_deref() == Some("S005")
            && r.field.as_deref() == Some("schedule")
            && r.message.contains("not a valid cron expression")
        ), "expected S005 on invalid cron expression, got: {:?}",
            results.iter().map(|r| &r.message).collect::<Vec<_>>());
    }

    #[test]
    fn cron_view_rejects_forbidden_fields() {
        // path / method / auth on a Cron view are meaningless — Cron views
        // have no caller. Each is its own S005.
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        write_cron_view(&bundle_dir, r#"
[api.views.recompute]
view_type = "Cron"
schedule  = "0 */5 * * * *"
path      = "/cron/oops"
method    = "POST"
auth      = "session"

[api.views.recompute.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#);
        let results = validate_structural(&bundle_dir);
        for forbidden in &["path", "method", "auth"] {
            assert!(results.iter().any(|r|
                r.error_code.as_deref() == Some("S005")
                && r.field.as_deref() == Some(*forbidden)
                && r.message.contains("not allowed on view_type=\"Cron\"")
            ), "expected S005 on Cron view with forbidden field '{}', got: {:?}",
                forbidden,
                results.iter().map(|r| &r.message).collect::<Vec<_>>());
        }
    }

    #[test]
    fn cron_view_rejects_invalid_overlap_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        write_cron_view(&bundle_dir, r#"
[api.views.recompute]
view_type      = "Cron"
schedule       = "0 */5 * * * *"
overlap_policy = "abandon"

[api.views.recompute.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/recompute.ts"
entrypoint = "tick"
resources  = []
"#);
        let results = validate_structural(&bundle_dir);
        assert!(results.iter().any(|r|
            r.error_code.as_deref() == Some("S005")
            && r.field.as_deref() == Some("overlap_policy")
            && r.message.contains("'abandon'")
        ), "expected S005 on overlap_policy='abandon', got: {:?}",
            results.iter().map(|r| &r.message).collect::<Vec<_>>());
    }

    #[test]
    fn cron_only_fields_rejected_on_rest_view() {
        // schedule / interval_seconds / overlap_policy / max_concurrent on a
        // non-Cron view are silently no-ops at runtime — surface them.
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name = "items"
datasource = "data"

[api.views.items]
path             = "items"
method           = "GET"
view_type        = "Rest"
auth             = "none"
schedule         = "0 */5 * * * *"
interval_seconds = 60

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        ).unwrap();
        let results = validate_structural(&bundle_dir);
        for f in &["schedule", "interval_seconds"] {
            assert!(results.iter().any(|r|
                r.error_code.as_deref() == Some("S005")
                && r.field.as_deref() == Some(*f)
                && r.message.contains("only valid when view_type=\"Cron\"")
            ), "expected S005 on Rest view with Cron-only field '{}', got: {:?}",
                f,
                results.iter().map(|r| &r.message).collect::<Vec<_>>());
        }
    }

    // ── Multiple errors collected ─────────────────────────────────

    #[test]
    fn collects_all_errors_in_one_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Introduce multiple errors
        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
type       = "worker"
appId      = "not-a-uuid"
entryPoint = "service"
source     = "test"
foo        = "bar"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);

        let error_codes: Vec<&str> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Fail)
            .filter_map(|r| r.error_code.as_deref())
            .collect();

        let warn_codes: Vec<&str> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Warn)
            .filter_map(|r| r.error_code.as_deref())
            .collect();

        // S002 (unknown key 'foo') is now a warning, S003/S008/S009 remain errors
        assert!(warn_codes.contains(&"S002"), "missing S002 warning: {:?}", warn_codes);
        assert!(error_codes.contains(&"S003"), "missing S003: {:?}", error_codes);
        assert!(error_codes.contains(&"S008"), "missing S008: {:?}", error_codes);
        assert!(error_codes.contains(&"S009"), "missing S009: {:?}", error_codes);
    }

    // ── Missing file ──────────────────────────────────────────────

    #[test]
    fn missing_bundle_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = tmp.path().join("empty-bundle");
        std::fs::create_dir_all(&bundle_dir).unwrap();

        let results = validate_structural(&bundle_dir);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].error_code.as_deref(), Some("S001"));
        assert!(results[0].message.contains("cannot read file"));
    }

    #[test]
    fn missing_app_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Remove app manifest
        std::fs::remove_file(bundle_dir.join("test-app/manifest.toml")).unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S001")
                    && r.file
                        .as_deref()
                        .map(|f| f.contains("test-app/manifest.toml"))
                        .unwrap_or(false)
            })
            .expect("expected S001 for missing app manifest");

        assert!(fail.message.contains("cannot read file"));
    }

    // ── Static files section ──────────────────────────────────────

    #[test]
    fn static_files_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[static_files]
enabled      = true
root         = "libraries/spa"
index_file   = "index.html"
spa_fallback = true
typo_field   = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("typo_field")
            })
            .expect("expected S002 for unknown field in static_files");

        assert!(fail.table_path.as_deref() == Some("static_files"));
    }

    // ── Keystore validation ───────────────────────────────────────

    #[test]
    fn keystore_unknown_key_in_resources() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/resources.toml"),
            r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true

[[keystores]]
name     = "app-keys"
lockbox  = "test/keystore-key"
required = true
unknwon  = "value"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("unknwon")
            })
            .expect("expected S002 for unknown key in keystore");

        assert!(
            fail.table_path.as_deref().unwrap().contains("keystores"),
        );
    }

    // ── Service appId UUID ────────────────────────────────────────

    #[test]
    fn service_invalid_app_id() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/resources.toml"),
            r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true

[[services]]
name     = "other-app"
appId    = "bad-uuid"
required = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S008"))
            .expect("expected S008 for bad UUID in service");

        assert!(fail.message.contains("bad-uuid"));
    }

    // ── SPA and init sub-tables ───────────────────────────────────

    #[test]
    fn spa_config_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-main"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "main"
source     = "test"

[spa]
root      = "libraries/spa"
indexFile  = "index.html"
fallback  = true
unknownFld = "oops"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("unknownFld")
            })
            .expect("expected S002 for unknown key in spa");

        assert!(fail.table_path.as_deref() == Some("spa"));
    }

    #[test]
    fn spa_config_missing_required() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-main"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "main"
source     = "test"

[spa]
fallback = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);

        let missing: Vec<_> = results
            .iter()
            .filter(|r| {
                r.error_code.as_deref() == Some("S003")
                    && r.table_path.as_deref() == Some("spa")
            })
            .collect();

        assert!(
            missing.len() >= 2,
            "expected missing root and indexFile in spa, got {} errors",
            missing.len()
        );
    }

    #[test]
    fn init_config_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-service"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "service"
source     = "test"

[init]
module = "handlers/init.ts"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S003")
                    && r.field.as_deref() == Some("entrypoint")
                    && r.table_path.as_deref() == Some("init")
            })
            .expect("expected S003 for missing entrypoint in init");

        assert!(fail.message.contains("entrypoint"));
    }
}
