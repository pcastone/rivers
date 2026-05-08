//! Layer 2 — Resource existence validation.
//!
//! Verifies that all file paths referenced in a loaded bundle actually exist
//! on disk. This includes handler modules, schema files, init handlers, SPA
//! directories, app directories, and required config files.
//!
//! Per `rivers-bundle-validation-spec.md` §Layer 2.

use std::collections::HashSet;
use std::path::Path;

use crate::loader::{LoadedApp, LoadedBundle};
use crate::validate_result::{error_codes, ValidationResult};
use crate::view::HandlerConfig;

/// Validate that all file references in the bundle exist on disk.
///
/// Takes a loaded bundle and checks file existence for:
/// - CodeComponent handler modules (E001)
/// - Pipeline stage handler modules (E001)
/// - WebSocket/SSE stream handler modules (E001)
/// - Init handler modules (E001)
/// - Schema JSON files (E001)
/// - SPA root directory (E002) and index file (E001)
pub fn validate_existence(bundle_dir: &Path, bundle: &LoadedBundle) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    for app in &bundle.apps {
        let app_name = &app.manifest.app_name;
        let app_dir = bundle_dir.join(
            bundle
                .manifest
                .apps
                .iter()
                .find(|a| {
                    // Match app directory name — try the app_name first,
                    // but the actual directory name comes from manifest.apps[]
                    // which is the directory name on disk.
                    let dir = bundle_dir.join(a);
                    dir == app.app_dir
                })
                .cloned()
                .unwrap_or_else(|| app_name.clone()),
        );

        // Handler modules
        validate_handler_modules(app, &app_dir, app_name, &mut results);

        // Pipeline stage handler modules
        validate_pipeline_modules(app, &app_dir, app_name, &mut results);

        // Schema files
        validate_schema_files(app, &app_dir, app_name, &mut results);

        // Init handler module
        validate_init_handler(app, &app_dir, app_name, &mut results);

        // SPA config
        validate_spa(app, &app_dir, app_name, &mut results);
    }

    results
}

/// Pre-load existence checks: verify app directories and required files exist.
///
/// Call this BEFORE `load_bundle()` — it catches errors that would cause
/// `load_bundle()` to fail with unhelpful messages.
///
/// Checks:
/// - App directory presence (E002)
/// - `manifest.toml` per app (E003)
/// - `resources.toml` per app (E004)
/// - `app.toml` per app (E005)
pub fn validate_existence_preload(
    bundle_dir: &Path,
    app_names: &[String],
) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    for app_name in app_names {
        let app_dir = bundle_dir.join(app_name);

        // E002: App directory must exist
        if !app_dir.is_dir() {
            results.push(
                ValidationResult::fail(
                    error_codes::E002,
                    format!("{}/", app_name),
                    format!("app directory '{}' listed in bundle manifest but not found", app_name),
                )
                .with_app(app_name.clone())
                .with_referenced_by("manifest.toml → apps[]"),
            );
            // Skip file checks — directory doesn't exist
            continue;
        }

        results.push(
            ValidationResult::pass(
                format!("{}/", app_name),
                format!("app directory '{}' exists", app_name),
            )
            .with_app(app_name.clone()),
        );

        // E003: manifest.toml
        let manifest_path = app_dir.join("manifest.toml");
        if !manifest_path.is_file() {
            results.push(
                ValidationResult::fail(
                    error_codes::E003,
                    format!("{}/manifest.toml", app_name),
                    format!("missing manifest.toml in app directory '{}'", app_name),
                )
                .with_app(app_name.clone()),
            );
        } else {
            results.push(
                ValidationResult::pass(
                    format!("{}/manifest.toml", app_name),
                    "manifest.toml exists",
                )
                .with_app(app_name.clone()),
            );
        }

        // E004: resources.toml
        let resources_path = app_dir.join("resources.toml");
        if !resources_path.is_file() {
            results.push(
                ValidationResult::fail(
                    error_codes::E004,
                    format!("{}/resources.toml", app_name),
                    format!("missing resources.toml in app directory '{}'", app_name),
                )
                .with_app(app_name.clone()),
            );
        } else {
            results.push(
                ValidationResult::pass(
                    format!("{}/resources.toml", app_name),
                    "resources.toml exists",
                )
                .with_app(app_name.clone()),
            );
        }

        // E005: app.toml
        let app_toml_path = app_dir.join("app.toml");
        if !app_toml_path.is_file() {
            results.push(
                ValidationResult::fail(
                    error_codes::E005,
                    format!("{}/app.toml", app_name),
                    format!("missing app.toml in app directory '{}'", app_name),
                )
                .with_app(app_name.clone()),
            );
        } else {
            results.push(
                ValidationResult::pass(
                    format!("{}/app.toml", app_name),
                    "app.toml exists",
                )
                .with_app(app_name.clone()),
            );
        }
    }

    results
}

// ── Internal helpers ──────────────────────────────────────────────

/// Check CodeComponent handler `module` files referenced by views.
fn validate_handler_modules(
    app: &LoadedApp,
    app_dir: &Path,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    for (view_name, view) in &app.config.api.views {
        if let HandlerConfig::Codecomponent { ref module, .. } = view.handler {
            check_file_exists(
                app_dir,
                module,
                app_name,
                &format!("api.views.{}.handler.module", view_name),
                results,
            );
        }
    }
}

/// Check pipeline stage handler modules (event_handlers, on_stream, ws_hooks, polling).
fn validate_pipeline_modules(
    app: &LoadedApp,
    app_dir: &Path,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    // Track checked modules to avoid duplicate results for the same file
    let mut checked = HashSet::new();

    for (view_name, view) in &app.config.api.views {
        // event_handlers stages
        if let Some(ref eh) = view.event_handlers {
            let stages: &[(&str, &[crate::view::HandlerStageConfig])] = &[
                ("pre_process", &eh.pre_process),
                ("handlers", &eh.handlers),
                ("post_process", &eh.post_process),
                ("on_error", &eh.on_error),
            ];
            for (stage_name, handlers) in stages {
                for (i, handler) in handlers.iter().enumerate() {
                    let ref_key = format!(
                        "api.views.{}.event_handlers.{}[{}].module",
                        view_name, stage_name, i,
                    );
                    let module_key = format!("{}:{}", app_dir.join(&handler.module).display(), &ref_key);
                    if checked.insert(module_key) {
                        check_file_exists(app_dir, &handler.module, app_name, &ref_key, results);
                    }
                }
            }
        }

        // on_stream handler
        if let Some(ref on_stream) = view.on_stream {
            let ref_key = format!("api.views.{}.on_stream.module", view_name);
            check_file_exists(app_dir, &on_stream.module, app_name, &ref_key, results);
        }

        // ws_hooks
        if let Some(ref hooks) = view.ws_hooks {
            let hook_checks: &[(&str, &Option<crate::view::HandlerStageConfig>)] = &[
                ("on_connect", &hooks.on_connect),
                ("on_message", &hooks.on_message),
                ("on_disconnect", &hooks.on_disconnect),
            ];
            for (hook_name, handler_opt) in hook_checks {
                if let Some(handler) = handler_opt {
                    let ref_key = format!(
                        "api.views.{}.ws_hooks.{}.module",
                        view_name, hook_name,
                    );
                    check_file_exists(app_dir, &handler.module, app_name, &ref_key, results);
                }
            }
        }

        // polling on_change / change_detect
        if let Some(ref polling) = view.polling {
            if let Some(ref on_change) = polling.on_change {
                let ref_key = format!("api.views.{}.polling.on_change.module", view_name);
                check_file_exists(app_dir, &on_change.module, app_name, &ref_key, results);
            }
            if let Some(ref change_detect) = polling.change_detect {
                let ref_key = format!("api.views.{}.polling.change_detect.module", view_name);
                check_file_exists(app_dir, &change_detect.module, app_name, &ref_key, results);
            }
        }
    }
}

/// Check schema file references in all DataViews.
fn validate_schema_files(
    app: &LoadedApp,
    app_dir: &Path,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    for (dv_name, dv) in &app.config.data.dataviews {
        let schema_fields: &[(&str, &Option<String>)] = &[
            ("return_schema", &dv.return_schema),
            ("get_schema", &dv.get_schema),
            ("post_schema", &dv.post_schema),
            ("put_schema", &dv.put_schema),
            ("delete_schema", &dv.delete_schema),
        ];

        for (field_name, schema_opt) in schema_fields {
            if let Some(schema_ref) = schema_opt {
                let ref_key = format!("data.dataviews.{}.{}", dv_name, field_name);
                check_file_exists(app_dir, schema_ref, app_name, &ref_key, results);
            }
        }
    }
}

/// Check init handler module existence.
///
/// Init handler modules are resolved relative to `{app_dir}/libraries/`.
fn validate_init_handler(
    app: &LoadedApp,
    app_dir: &Path,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    if let Some(ref init) = app.manifest.init {
        let libraries_dir = app_dir.join("libraries");
        let module_path = libraries_dir.join(&init.module);
        let rel_path = format!("libraries/{}", init.module);

        let app_dir_name = app_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(app_name);
        let file_ref = format!("{}/{}", app_dir_name, rel_path);

        if module_path.is_file() {
            results.push(
                ValidationResult::pass(
                    &file_ref,
                    format!("init handler module '{}' exists", init.module),
                )
                .with_app(app_name),
            );
        } else {
            results.push(
                ValidationResult::fail(
                    error_codes::E001,
                    &file_ref,
                    format!("init handler module '{}' not found", init.module),
                )
                .with_app(app_name)
                .with_referenced_by("manifest.toml → init.module"),
            );
        }
    }
}

/// Check SPA root directory and index file existence.
fn validate_spa(
    app: &LoadedApp,
    app_dir: &Path,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    if let Some(ref spa) = app.manifest.spa {
        let app_dir_name = app_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(app_name);

        // SPA root directory — E002
        let root_path = app_dir.join(&spa.root);
        let root_ref = format!("{}/{}", app_dir_name, spa.root);

        if root_path.is_dir() {
            results.push(
                ValidationResult::pass(
                    &root_ref,
                    format!("SPA root directory '{}' exists", spa.root),
                )
                .with_app(app_name),
            );

            // SPA index_file — E001 (only check if root exists)
            let index_path = root_path.join(&spa.index_file);
            let index_ref = format!("{}/{}/{}", app_dir_name, spa.root, spa.index_file);

            if index_path.is_file() {
                results.push(
                    ValidationResult::pass(
                        &index_ref,
                        format!("SPA index file '{}' exists", spa.index_file),
                    )
                    .with_app(app_name),
                );
            } else {
                results.push(
                    ValidationResult::fail(
                        error_codes::E001,
                        &index_ref,
                        format!("SPA index file '{}' not found", spa.index_file),
                    )
                    .with_app(app_name)
                    .with_referenced_by("manifest.toml → spa.index_file"),
                );
            }
        } else {
            results.push(
                ValidationResult::fail(
                    error_codes::E002,
                    &root_ref,
                    format!("SPA root directory '{}' not found", spa.root),
                )
                .with_app(app_name)
                .with_referenced_by("manifest.toml → spa.root"),
            );
        }
    }
}

/// Check that a file exists relative to the app directory.
///
/// Emits a Pass or Fail (E001) result.
fn check_file_exists(
    app_dir: &Path,
    relative_path: &str,
    app_name: &str,
    referenced_by: &str,
    results: &mut Vec<ValidationResult>,
) {
    let full_path = app_dir.join(relative_path);
    let app_dir_name = app_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(app_name);
    let file_ref = format!("{}/{}", app_dir_name, relative_path);

    if full_path.is_file() {
        results.push(
            ValidationResult::pass(&file_ref, format!("file '{}' exists", relative_path))
                .with_app(app_name),
        );
    } else {
        results.push(
            ValidationResult::fail(
                error_codes::E001,
                &file_ref,
                format!("file '{}' not found", relative_path),
            )
            .with_app(app_name)
            .with_referenced_by(referenced_by),
        );
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::*;
    use crate::dataview::DataViewConfig;
    use crate::loader::{LoadedApp, LoadedBundle};
    use crate::validate_result::ValidationStatus;
    use crate::view::{ApiViewConfig, HandlerConfig, HandlerStageConfig, OnStreamConfig, ViewEventHandlers, WebSocketHooks};
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    // ── Fixture helpers ──────────────────────────────────────────────

    /// Create a minimal valid bundle on disk and return it as a LoadedBundle.
    fn create_test_bundle(dir: &Path) -> (LoadedBundle, std::path::PathBuf) {
        let bundle_dir = dir.join("test-bundle");
        let app_dir = bundle_dir.join("test-app");
        let schemas_dir = app_dir.join("schemas");
        fs::create_dir_all(&schemas_dir).unwrap();

        // Bundle manifest
        fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName = "test-bundle"
bundleVersion = "1.0.0"
apps = ["test-app"]
"#,
        )
        .unwrap();

        // App manifest
        fs::write(
            app_dir.join("manifest.toml"),
            r#"
appName = "test-app"
type = "app-service"
appId = "00000000-0000-0000-0000-000000000001"
"#,
        )
        .unwrap();

        // Resources
        fs::write(
            app_dir.join("resources.toml"),
            r#"
[[datasources]]
name = "db"
driver = "faker"
nopassword = true
"#,
        )
        .unwrap();

        // App config (minimal)
        fs::write(app_dir.join("app.toml"), "").unwrap();

        // Schema file
        fs::write(
            schemas_dir.join("contact.json"),
            r#"{"type":"object","properties":{"name":{"type":"string"}}}"#,
        )
        .unwrap();

        let bundle = LoadedBundle {
            manifest: BundleManifest {
                bundle_name: "test-bundle".into(),
                bundle_version: "1.0.0".into(),
                source: None,
                apps: vec!["test-app".into()],
            },
            apps: vec![LoadedApp {
                manifest: AppManifest {
                    app_name: "test-app".into(),
                    description: None,
                    version: None,
                    app_type: "app-service".into(),
                    app_id: "00000000-0000-0000-0000-000000000001".into(),
                    entry_point: None,
                    app_entry_point: None,
                    source: None,
                    spa: None,
                    init: None,
                },
                resources: ResourcesConfig {
                    datasources: vec![ResourceDatasource {
                        name: "db".into(),
                        driver: "faker".into(),
                        lockbox: None,
                        nopassword: true,
                        x_type: None,
                        required: true,
                    }],
                    keystores: vec![],
                    services: vec![],
                },
                config: AppConfig::default(),
                app_dir: app_dir.clone(),
            }],
        };

        (bundle, bundle_dir)
    }

    /// Add a DataView with a schema reference to a loaded bundle's first app.
    fn add_dataview_with_schema(
        bundle: &mut LoadedBundle,
        dv_name: &str,
        schema_path: &str,
    ) {
        bundle.apps[0].config.data.dataviews.insert(
            dv_name.to_string(),
            DataViewConfig {
                name: dv_name.to_string(),
                datasource: "db".into(),
                query: None,
                parameters: vec![],
                return_schema: None,
                get_query: Some("SELECT 1".into()),
                post_query: None,
                put_query: None,
                delete_query: None,
                get_schema: Some(schema_path.to_string()),
                post_schema: None,
                put_schema: None,
                delete_schema: None,
                get_parameters: vec![],
                post_parameters: vec![],
                put_parameters: vec![],
                delete_parameters: vec![],
                streaming: false,
                circuit_breaker_id: None,
                prepared: false,
                query_params: std::collections::HashMap::new(),
                caching: None,
                invalidates: vec![],
                validate_result: false,
                strict_parameters: false,
                max_rows: 1000,
                skip_introspect: false,
                cursor_key: None,
                source_views: vec![],
                compose_strategy: None,
                join_key: None,
                enrich_mode: "nest".into(),
            transaction: false,
            },
        );
    }

    /// Add a CodeComponent view to a loaded bundle's first app.
    fn add_codecomponent_view(
        bundle: &mut LoadedBundle,
        view_name: &str,
        module_path: &str,
    ) {
        bundle.apps[0].config.api.views.insert(
            view_name.to_string(),
            ApiViewConfig {
                view_type: "Rest".into(),
                path: Some("/test".into()),
                method: Some("GET".into()),
                handler: HandlerConfig::Codecomponent {
                    language: "javascript".into(),
                    module: module_path.to_string(),
                    entrypoint: "handler".into(),
                    resources: vec![],
                },
                parameter_mapping: None,
                dataviews: vec![],
                primary: None,
                streaming: None,
                streaming_format: None,
                stream_timeout_ms: None,
                guard: false,
                auth: None,
                guard_config: None,
                allow_outbound_http: false,
                rate_limit_per_minute: None,
                rate_limit_burst_size: None,
                websocket_mode: None,
                max_connections: None,
                sse_tick_interval_ms: None,
                sse_trigger_events: vec![],
                sse_event_buffer_size: None,
                session_revalidation_interval_s: None,
                polling: None,
                event_handlers: None,
                on_stream: None,
                ws_hooks: None,
                on_event: None,
                tools: HashMap::new(),
                resources: HashMap::new(),
                prompts: HashMap::new(),
                instructions: None,
                session: None,
                federation: vec![],
            response_headers: None,
            guard_view: None,
            },
        );
    }

    fn count_by_status(results: &[ValidationResult], status: ValidationStatus) -> usize {
        results.iter().filter(|r| r.status == status).count()
    }

    fn find_fail_with_code<'a>(
        results: &'a [ValidationResult],
        code: &str,
    ) -> Option<&'a ValidationResult> {
        results.iter().find(|r| {
            r.status == ValidationStatus::Fail
                && r.error_code.as_deref() == Some(code)
        })
    }

    // ── Preload tests ────────────────────────────────────────────────

    #[test]
    fn preload_all_present() {
        let tmp = TempDir::new().unwrap();
        let (_, bundle_dir) = create_test_bundle(tmp.path());

        let results = validate_existence_preload(&bundle_dir, &["test-app".into()]);

        // Should have 4 passes: dir, manifest, resources, app.toml
        assert_eq!(count_by_status(&results, ValidationStatus::Pass), 4);
        assert_eq!(count_by_status(&results, ValidationStatus::Fail), 0);
    }

    #[test]
    fn preload_missing_app_directory() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("empty-bundle");
        fs::create_dir_all(&bundle_dir).unwrap();

        let results =
            validate_existence_preload(&bundle_dir, &["missing-app".into()]);

        assert_eq!(count_by_status(&results, ValidationStatus::Fail), 1);
        let fail = find_fail_with_code(&results, error_codes::E002).unwrap();
        assert!(fail.message.contains("missing-app"));
        assert_eq!(fail.app.as_deref(), Some("missing-app"));
    }

    #[test]
    fn preload_missing_manifest() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("bundle");
        let app_dir = bundle_dir.join("my-app");
        fs::create_dir_all(&app_dir).unwrap();

        // Write resources.toml and app.toml but NOT manifest.toml
        fs::write(app_dir.join("resources.toml"), "").unwrap();
        fs::write(app_dir.join("app.toml"), "").unwrap();

        let results =
            validate_existence_preload(&bundle_dir, &["my-app".into()]);

        // 1 fail for missing manifest, 2 pass for resources and app.toml, 1 pass for dir
        assert_eq!(count_by_status(&results, ValidationStatus::Fail), 1);
        let fail = find_fail_with_code(&results, error_codes::E003).unwrap();
        assert!(fail.message.contains("manifest.toml"));
    }

    #[test]
    fn preload_missing_resources() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("bundle");
        let app_dir = bundle_dir.join("my-app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(app_dir.join("manifest.toml"), "").unwrap();
        fs::write(app_dir.join("app.toml"), "").unwrap();
        // No resources.toml

        let results =
            validate_existence_preload(&bundle_dir, &["my-app".into()]);

        let fail = find_fail_with_code(&results, error_codes::E004).unwrap();
        assert!(fail.message.contains("resources.toml"));
    }

    #[test]
    fn preload_missing_app_toml() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("bundle");
        let app_dir = bundle_dir.join("my-app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(app_dir.join("manifest.toml"), "").unwrap();
        fs::write(app_dir.join("resources.toml"), "").unwrap();
        // No app.toml

        let results =
            validate_existence_preload(&bundle_dir, &["my-app".into()]);

        let fail = find_fail_with_code(&results, error_codes::E005).unwrap();
        assert!(fail.message.contains("app.toml"));
    }

    #[test]
    fn preload_missing_all_files() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("bundle");
        let app_dir = bundle_dir.join("my-app");
        fs::create_dir_all(&app_dir).unwrap();
        // Dir exists but no files

        let results =
            validate_existence_preload(&bundle_dir, &["my-app".into()]);

        // 1 pass (dir), 3 fails (manifest, resources, app.toml)
        assert_eq!(count_by_status(&results, ValidationStatus::Pass), 1);
        assert_eq!(count_by_status(&results, ValidationStatus::Fail), 3);
        assert!(find_fail_with_code(&results, error_codes::E003).is_some());
        assert!(find_fail_with_code(&results, error_codes::E004).is_some());
        assert!(find_fail_with_code(&results, error_codes::E005).is_some());
    }

    // ── Schema file tests ────────────────────────────────────────────

    #[test]
    fn schema_file_exists() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        add_dataview_with_schema(&mut bundle, "contacts", "schemas/contact.json");

        let results = validate_existence(&bundle_dir, &bundle);

        // Schema check should pass
        let schema_results: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("schemas/contact.json"))
            .collect();
        assert_eq!(schema_results.len(), 1);
        assert_eq!(schema_results[0].status, ValidationStatus::Pass);
    }

    #[test]
    fn schema_file_missing() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        add_dataview_with_schema(&mut bundle, "contacts", "schemas/nonexistent.json");

        let results = validate_existence(&bundle_dir, &bundle);

        let fail = results
            .iter()
            .find(|r| {
                r.status == ValidationStatus::Fail
                    && r.message.contains("nonexistent.json")
            })
            .expect("expected a failure for missing schema");

        assert_eq!(fail.error_code.as_deref(), Some(error_codes::E001));
        assert!(fail.referenced_by.as_ref().unwrap().contains("get_schema"));
    }

    #[test]
    fn multiple_schema_fields_checked() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        // Write a second schema file
        fs::write(
            bundle_dir
                .join("test-app")
                .join("schemas")
                .join("post.json"),
            "{}",
        )
        .unwrap();

        // DataView with both get_schema and post_schema
        let dv = DataViewConfig {
            name: "multi".into(),
            datasource: "db".into(),
            query: None,
            parameters: vec![],
            return_schema: None,
            get_query: None,
            post_query: None,
            put_query: None,
            delete_query: None,
            get_schema: Some("schemas/contact.json".into()),
            post_schema: Some("schemas/post.json".into()),
            put_schema: Some("schemas/missing.json".into()),
            delete_schema: None,
            get_parameters: vec![],
            post_parameters: vec![],
            put_parameters: vec![],
            delete_parameters: vec![],
            streaming: false,
            circuit_breaker_id: None,
            prepared: false,
            query_params: std::collections::HashMap::new(),
            caching: None,
            invalidates: vec![],
            validate_result: false,
            strict_parameters: false,
            max_rows: 1000,
            skip_introspect: false,
            cursor_key: None,
            source_views: vec![],
            compose_strategy: None,
            join_key: None,
            enrich_mode: "nest".into(),
            transaction: false,
        };
        bundle.apps[0]
            .config
            .data
            .dataviews
            .insert("multi".into(), dv);

        let results = validate_existence(&bundle_dir, &bundle);

        // 2 pass (contact.json, post.json) + 1 fail (missing.json)
        let schema_results: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("schemas/"))
            .collect();
        assert_eq!(
            schema_results.iter().filter(|r| r.status == ValidationStatus::Pass).count(),
            2,
        );
        assert_eq!(
            schema_results.iter().filter(|r| r.status == ValidationStatus::Fail).count(),
            1,
        );
    }

    // ── Handler module tests ─────────────────────────────────────────

    #[test]
    fn handler_module_exists() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        let handler_dir = bundle_dir
            .join("test-app")
            .join("libraries")
            .join("handlers");
        fs::create_dir_all(&handler_dir).unwrap();
        fs::write(handler_dir.join("greet.js"), "export function handler(){}").unwrap();

        add_codecomponent_view(
            &mut bundle,
            "greet",
            "libraries/handlers/greet.js",
        );

        let results = validate_existence(&bundle_dir, &bundle);

        let handler_results: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("greet.js"))
            .collect();
        assert_eq!(handler_results.len(), 1);
        assert_eq!(handler_results[0].status, ValidationStatus::Pass);
    }

    #[test]
    fn handler_module_missing() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        add_codecomponent_view(
            &mut bundle,
            "missing_handler",
            "libraries/handlers/nope.js",
        );

        let results = validate_existence(&bundle_dir, &bundle);

        let fail = results
            .iter()
            .find(|r| {
                r.status == ValidationStatus::Fail
                    && r.message.contains("nope.js")
            })
            .expect("expected failure for missing handler module");

        assert_eq!(fail.error_code.as_deref(), Some(error_codes::E001));
        assert!(fail.referenced_by.as_ref().unwrap().contains("handler.module"));
    }

    // ── Init handler tests ───────────────────────────────────────────

    #[test]
    fn init_handler_exists() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        let lib_dir = bundle_dir.join("test-app").join("libraries");
        fs::create_dir_all(&lib_dir).unwrap();
        fs::write(lib_dir.join("init.js"), "export function init(){}").unwrap();

        bundle.apps[0].manifest.init = Some(InitHandlerConfig {
            module: "init.js".into(),
            entrypoint: "init".into(),
        });

        let results = validate_existence(&bundle_dir, &bundle);

        let init_results: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("init"))
            .collect();
        assert_eq!(init_results.len(), 1);
        assert_eq!(init_results[0].status, ValidationStatus::Pass);
    }

    #[test]
    fn init_handler_missing() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        bundle.apps[0].manifest.init = Some(InitHandlerConfig {
            module: "missing_init.js".into(),
            entrypoint: "init".into(),
        });

        let results = validate_existence(&bundle_dir, &bundle);

        let fail = results
            .iter()
            .find(|r| {
                r.status == ValidationStatus::Fail
                    && r.message.contains("missing_init.js")
            })
            .expect("expected failure for missing init handler");

        assert_eq!(fail.error_code.as_deref(), Some(error_codes::E001));
        assert!(fail.referenced_by.as_ref().unwrap().contains("init.module"));
    }

    // ── SPA tests ────────────────────────────────────────────────────

    #[test]
    fn spa_root_and_index_exist() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        let spa_dir = bundle_dir.join("test-app").join("public");
        fs::create_dir_all(&spa_dir).unwrap();
        fs::write(spa_dir.join("index.html"), "<html></html>").unwrap();

        bundle.apps[0].manifest.spa = Some(SpaConfig {
            root: "public".into(),
            index_file: "index.html".into(),
            fallback: false,
            max_age: None,
        });
        bundle.apps[0].manifest.app_type = "app-main".into();

        let results = validate_existence(&bundle_dir, &bundle);

        let spa_results: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("SPA"))
            .collect();
        assert_eq!(spa_results.len(), 2); // root dir + index file
        assert!(spa_results.iter().all(|r| r.status == ValidationStatus::Pass));
    }

    #[test]
    fn spa_root_missing() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        bundle.apps[0].manifest.spa = Some(SpaConfig {
            root: "nonexistent-dir".into(),
            index_file: "index.html".into(),
            fallback: false,
            max_age: None,
        });

        let results = validate_existence(&bundle_dir, &bundle);

        let fail = find_fail_with_code(&results, error_codes::E002)
            .expect("expected E002 for missing SPA root");
        assert!(fail.message.contains("SPA root directory"));
        assert!(fail.referenced_by.as_ref().unwrap().contains("spa.root"));
    }

    #[test]
    fn spa_root_exists_index_missing() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        let spa_dir = bundle_dir.join("test-app").join("dist");
        fs::create_dir_all(&spa_dir).unwrap();
        // No index.html

        bundle.apps[0].manifest.spa = Some(SpaConfig {
            root: "dist".into(),
            index_file: "index.html".into(),
            fallback: false,
            max_age: None,
        });

        let results = validate_existence(&bundle_dir, &bundle);

        // Root passes, index fails
        let root_pass = results
            .iter()
            .find(|r| r.status == ValidationStatus::Pass && r.message.contains("SPA root"))
            .expect("root should pass");
        assert!(root_pass.status == ValidationStatus::Pass);

        let fail = results
            .iter()
            .find(|r| {
                r.status == ValidationStatus::Fail
                    && r.message.contains("SPA index file")
            })
            .expect("expected E001 for missing index file");
        assert_eq!(fail.error_code.as_deref(), Some(error_codes::E001));
    }

    // ── Pipeline handler tests ───────────────────────────────────────

    #[test]
    fn event_handler_module_checked() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        // Create the handler file
        let handler_dir = bundle_dir
            .join("test-app")
            .join("libraries")
            .join("handlers");
        fs::create_dir_all(&handler_dir).unwrap();
        fs::write(handler_dir.join("pre.js"), "export function pre(){}").unwrap();

        // Add a view with event_handlers
        let view = ApiViewConfig {
            view_type: "Rest".into(),
            path: Some("/test".into()),
            method: Some("POST".into()),
            handler: HandlerConfig::None {},
            parameter_mapping: None,
            dataviews: vec![],
            primary: None,
            streaming: None,
            streaming_format: None,
            stream_timeout_ms: None,
            guard: false,
            auth: None,
            guard_config: None,
            allow_outbound_http: false,
            rate_limit_per_minute: None,
            rate_limit_burst_size: None,
            websocket_mode: None,
            max_connections: None,
            sse_tick_interval_ms: None,
            sse_trigger_events: vec![],
            sse_event_buffer_size: None,
            session_revalidation_interval_s: None,
            polling: None,
            event_handlers: Some(ViewEventHandlers {
                pre_process: vec![HandlerStageConfig {
                    module: "libraries/handlers/pre.js".into(),
                    entrypoint: "pre".into(),
                    key: None,
                    on_failure: None,
                }],
                handlers: vec![HandlerStageConfig {
                    module: "libraries/handlers/missing_handler.js".into(),
                    entrypoint: "handle".into(),
                    key: None,
                    on_failure: None,
                }],
                post_process: vec![],
                on_error: vec![],
            }),
            on_stream: None,
            ws_hooks: None,
            on_event: None,
            tools: HashMap::new(),
            resources: HashMap::new(),
            prompts: HashMap::new(),
            instructions: None,
            session: None,
            federation: vec![],
            response_headers: None,
            guard_view: None,
        };
        bundle.apps[0]
            .config
            .api
            .views
            .insert("pipeline_view".into(), view);

        let results = validate_existence(&bundle_dir, &bundle);

        // pre.js should pass
        let pre_pass = results
            .iter()
            .find(|r| r.message.contains("pre.js") && r.status == ValidationStatus::Pass);
        assert!(pre_pass.is_some(), "pre.js should pass");

        // missing_handler.js should fail
        let fail = results
            .iter()
            .find(|r| {
                r.status == ValidationStatus::Fail
                    && r.message.contains("missing_handler.js")
            })
            .expect("expected failure for missing pipeline handler");
        assert_eq!(fail.error_code.as_deref(), Some(error_codes::E001));
    }

    #[test]
    fn on_stream_module_checked() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        let view = ApiViewConfig {
            view_type: "Websocket".into(),
            path: Some("/ws".into()),
            method: Some("GET".into()),
            handler: HandlerConfig::None {},
            parameter_mapping: None,
            dataviews: vec![],
            primary: None,
            streaming: None,
            streaming_format: None,
            stream_timeout_ms: None,
            guard: false,
            auth: None,
            guard_config: None,
            allow_outbound_http: false,
            rate_limit_per_minute: None,
            rate_limit_burst_size: None,
            websocket_mode: Some("Broadcast".into()),
            max_connections: None,
            sse_tick_interval_ms: None,
            sse_trigger_events: vec![],
            sse_event_buffer_size: None,
            session_revalidation_interval_s: None,
            polling: None,
            event_handlers: None,
            on_stream: Some(OnStreamConfig {
                module: "libraries/handlers/stream.js".into(),
                entrypoint: "onStream".into(),
                handler_mode: None,
            }),
            ws_hooks: None,
            on_event: None,
            tools: HashMap::new(),
            resources: HashMap::new(),
            prompts: HashMap::new(),
            instructions: None,
            session: None,
            federation: vec![],
            response_headers: None,
            guard_view: None,
        };
        bundle.apps[0]
            .config
            .api
            .views
            .insert("ws_view".into(), view);

        let results = validate_existence(&bundle_dir, &bundle);

        let fail = results
            .iter()
            .find(|r| {
                r.status == ValidationStatus::Fail
                    && r.message.contains("stream.js")
            })
            .expect("expected failure for missing on_stream module");
        assert_eq!(fail.error_code.as_deref(), Some(error_codes::E001));
    }

    #[test]
    fn ws_hooks_modules_checked() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        // Create one hook file, leave others missing
        let handler_dir = bundle_dir
            .join("test-app")
            .join("libraries")
            .join("hooks");
        fs::create_dir_all(&handler_dir).unwrap();
        fs::write(handler_dir.join("connect.js"), "").unwrap();

        let view = ApiViewConfig {
            view_type: "Websocket".into(),
            path: Some("/ws".into()),
            method: Some("GET".into()),
            handler: HandlerConfig::None {},
            parameter_mapping: None,
            dataviews: vec![],
            primary: None,
            streaming: None,
            streaming_format: None,
            stream_timeout_ms: None,
            guard: false,
            auth: None,
            guard_config: None,
            allow_outbound_http: false,
            rate_limit_per_minute: None,
            rate_limit_burst_size: None,
            websocket_mode: None,
            max_connections: None,
            sse_tick_interval_ms: None,
            sse_trigger_events: vec![],
            sse_event_buffer_size: None,
            session_revalidation_interval_s: None,
            polling: None,
            event_handlers: None,
            on_stream: None,
            ws_hooks: Some(WebSocketHooks {
                on_connect: Some(HandlerStageConfig {
                    module: "libraries/hooks/connect.js".into(),
                    entrypoint: "onConnect".into(),
                    key: None,
                    on_failure: None,
                }),
                on_message: Some(HandlerStageConfig {
                    module: "libraries/hooks/message.js".into(),
                    entrypoint: "onMessage".into(),
                    key: None,
                    on_failure: None,
                }),
                on_disconnect: None,
            }),
            on_event: None,
            tools: HashMap::new(),
            resources: HashMap::new(),
            prompts: HashMap::new(),
            instructions: None,
            session: None,
            federation: vec![],
            response_headers: None,
            guard_view: None,
        };
        bundle.apps[0]
            .config
            .api
            .views
            .insert("ws_hooks_view".into(), view);

        let results = validate_existence(&bundle_dir, &bundle);

        // connect.js should pass
        assert!(results.iter().any(|r| {
            r.status == ValidationStatus::Pass && r.message.contains("connect.js")
        }));

        // message.js should fail
        let fail = results
            .iter()
            .find(|r| {
                r.status == ValidationStatus::Fail
                    && r.message.contains("message.js")
            })
            .expect("expected failure for missing ws_hooks.on_message module");
        assert_eq!(fail.error_code.as_deref(), Some(error_codes::E001));
    }

    // ── Empty bundle tests ───────────────────────────────────────────

    #[test]
    fn empty_bundle_no_results() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("empty");
        fs::create_dir_all(&bundle_dir).unwrap();

        let bundle = LoadedBundle {
            manifest: BundleManifest {
                bundle_name: "empty".into(),
                bundle_version: "0.0.0".into(),
                source: None,
                apps: vec![],
            },
            apps: vec![],
        };

        let results = validate_existence(&bundle_dir, &bundle);
        assert!(results.is_empty());
    }

    #[test]
    fn preload_empty_app_list_no_results() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("empty");
        fs::create_dir_all(&bundle_dir).unwrap();

        let results = validate_existence_preload(&bundle_dir, &[]);
        assert!(results.is_empty());
    }

    // ── Combined test ────────────────────────────────────────────────

    #[test]
    fn combined_checks_multiple_issues() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        // Add a schema that exists and one that doesn't
        add_dataview_with_schema(&mut bundle, "good", "schemas/contact.json");
        add_dataview_with_schema(&mut bundle, "bad", "schemas/nope.json");

        // Add a handler that doesn't exist
        add_codecomponent_view(&mut bundle, "nope_view", "libraries/nope.js");

        let results = validate_existence(&bundle_dir, &bundle);

        let passes = count_by_status(&results, ValidationStatus::Pass);
        let fails = count_by_status(&results, ValidationStatus::Fail);

        // 1 pass for existing schema, 1 fail for missing schema, 1 fail for missing handler
        assert!(passes >= 1, "expected at least 1 pass, got {}", passes);
        assert_eq!(fails, 2, "expected 2 fails, got {}: {:?}", fails,
            results.iter().filter(|r| r.status == ValidationStatus::Fail).collect::<Vec<_>>());
    }

    // ── Result metadata tests ────────────────────────────────────────

    #[test]
    fn results_have_app_name() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        add_dataview_with_schema(&mut bundle, "dv", "schemas/contact.json");

        let results = validate_existence(&bundle_dir, &bundle);

        // All results should have an app name set
        for r in &results {
            assert_eq!(
                r.app.as_deref(),
                Some("test-app"),
                "result {:?} missing app name",
                r.message,
            );
        }
    }

    #[test]
    fn fail_results_have_referenced_by() {
        let tmp = TempDir::new().unwrap();
        let (mut bundle, bundle_dir) = create_test_bundle(tmp.path());

        add_dataview_with_schema(&mut bundle, "dv", "schemas/nope.json");

        let results = validate_existence(&bundle_dir, &bundle);
        let fail = results
            .iter()
            .find(|r| r.status == ValidationStatus::Fail)
            .expect("expected at least one failure");

        assert!(
            fail.referenced_by.is_some(),
            "fail result should have referenced_by",
        );
    }
}
