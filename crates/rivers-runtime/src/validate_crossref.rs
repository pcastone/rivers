//! Layer 3 — Logical Cross-Reference Validation.
//!
//! Per `rivers-bundle-validation-spec.md` §4.3 and §11.3.
//!
//! Validates that internal references between config files resolve correctly
//! and the dependency graph is consistent. This includes:
//!
//! - DataView → datasource references (X001)
//! - View → DataView references (X002)
//! - View handler resources → datasources (X003)
//! - Invalidates targets → DataView names (X004)
//! - Service → appId within bundle (X005)
//! - appId uniqueness across apps (X006)
//! - Datasource/DataView name uniqueness within app (X007)
//! - View type constraints (X008, X009, X010)
//! - SPA on app-service (X011)
//! - Init handler completeness (X012)
//! - x-type / driver consistency (X013)
//! - No views warning (W004)
//! - Solo circuit breaker ID warning (CB001)

use std::collections::{HashMap, HashSet};

use crate::loader::{LoadedApp, LoadedBundle};
use crate::validate_result::{error_codes, ValidationResult};
use crate::view::HandlerConfig;

/// Validate cross-references within and across apps in a loaded bundle.
///
/// Returns a flat list of [`ValidationResult`] findings (pass, fail, warn)
/// for the logical cross-references layer.
pub fn validate_crossref(bundle: &LoadedBundle) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    // ── Bundle-level checks ────────────────────────────────────────

    // X006: appId uniqueness across apps
    check_appid_uniqueness(bundle, &mut results);

    // Build a set of all known appIds for service dependency resolution.
    let known_app_ids: HashSet<&str> = bundle
        .apps
        .iter()
        .map(|a| a.manifest.app_id.as_str())
        .collect();

    // ── Per-app checks ─────────────────────────────────────────────

    for app in &bundle.apps {
        // X007: Duplicate datasource names in resources.toml
        check_duplicate_datasource_names(app, &mut results);

        // X007: Duplicate DataView names within app (HashMap keys are unique
        // by construction, so this is always a pass — but we validate the
        // resource-level datasource names that feed into app.toml).
        // DataView names in app.config.data.dataviews are HashMap keys and
        // therefore unique. We still emit a pass result for completeness.
        check_duplicate_dataview_names(app, &mut results);

        // Build lookup sets for the current app.
        let resource_ds_names: HashSet<&str> = app
            .resources
            .datasources
            .iter()
            .map(|ds| ds.name.as_str())
            .collect();

        let config_ds_names: HashSet<&str> = app
            .config
            .data
            .datasources
            .keys()
            .map(|k| k.as_str())
            .collect();

        let dataview_names: HashSet<&str> = app
            .config
            .data
            .dataviews
            .keys()
            .map(|k| k.as_str())
            .collect();

        // X001: DataView → datasource references
        check_dataview_datasource_refs(app, &resource_ds_names, &config_ds_names, &mut results);

        // X004: Invalidates targets → DataView names
        check_invalidates_targets(app, &dataview_names, &mut results);

        // X002: View → DataView references
        // X003: View handler resources → datasources
        // X008: Dataview handler only valid for view_type=Rest
        // X009: WebSocket requires method=GET
        // X010: SSE requires method=GET
        check_view_refs(app, &dataview_names, &resource_ds_names, &mut results);

        // W004: No views defined
        check_views_exist(app, &mut results);

        // X005: Service → appId within bundle
        check_service_app_refs(app, &known_app_ids, &mut results);

        // X013: x-type / driver consistency
        check_xtype_driver_match(app, &mut results);

        // X011: SPA on app-service
        check_spa_app_type(app, &mut results);

        // X012: Init handler completeness
        check_init_handler(app, &mut results);

        // S006/X012 adjacent: nopassword + credentials_source
        check_nopassword_credentials(app, &mut results);

        // CB001: Circuit breaker ID referenced by only one DataView
        check_solo_circuit_breaker_ids(app, &mut results);
    }

    results
}

// ── Bundle-level checks ────────────────────────────────────────────

/// X006: No two apps share the same appId.
fn check_appid_uniqueness(bundle: &LoadedBundle, results: &mut Vec<ValidationResult>) {
    let mut seen: HashMap<&str, &str> = HashMap::new(); // appId -> first app_name

    for app in &bundle.apps {
        let app_id = app.manifest.app_id.as_str();
        let app_name = app.manifest.app_name.as_str();

        if let Some(&first_app) = seen.get(app_id) {
            results.push(
                ValidationResult::fail(
                    error_codes::X006,
                    format!("{}/manifest.toml", app_name),
                    format!(
                        "Duplicate appId '{}' in {} and {}",
                        app_id, first_app, app_name,
                    ),
                )
                .with_app(app_name)
                .with_field("appId"),
            );
        } else {
            seen.insert(app_id, app_name);
        }
    }

    // Emit pass if all unique.
    if results.iter().all(|r| {
        r.error_code.as_deref() != Some(error_codes::X006)
    }) {
        results.push(ValidationResult::pass(
            "manifest.toml",
            "All appId values are unique across bundle",
        ));
    }
}

// ── Per-app checks ─────────────────────────────────────────────────

/// X007: Duplicate datasource names in resources.toml.
fn check_duplicate_datasource_names(app: &LoadedApp, results: &mut Vec<ValidationResult>) {
    let app_name = &app.manifest.app_name;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut has_dup = false;

    for ds in &app.resources.datasources {
        if !seen.insert(ds.name.as_str()) {
            has_dup = true;
            results.push(
                ValidationResult::fail(
                    error_codes::X007,
                    format!("{}/resources.toml", app_name),
                    format!(
                        "Duplicate datasource name '{}' in {}/resources.toml",
                        ds.name, app_name,
                    ),
                )
                .with_app(app_name)
                .with_table_path("datasources")
                .with_field("name"),
            );
        }
    }

    if !has_dup && !app.resources.datasources.is_empty() {
        results.push(
            ValidationResult::pass(
                format!("{}/resources.toml", app_name),
                format!("{}: datasource names are unique", app_name),
            )
            .with_app(app_name),
        );
    }
}

/// X007: Duplicate DataView names within app.
///
/// DataView names are HashMap keys (inherently unique), so this is always
/// a pass unless the map is empty. We still emit a result for auditability.
fn check_duplicate_dataview_names(app: &LoadedApp, results: &mut Vec<ValidationResult>) {
    let app_name = &app.manifest.app_name;
    let dv_count = app.config.data.dataviews.len();

    if dv_count > 0 {
        results.push(
            ValidationResult::pass(
                format!("{}/app.toml", app_name),
                format!("{}: DataView names are unique ({} DataViews)", app_name, dv_count),
            )
            .with_app(app_name),
        );
    }
}

/// X001: Every DataView's `datasource` field must resolve to a declared datasource.
///
/// A datasource is considered declared if it appears as a key in
/// `app.config.data.datasources` OR as a `name` in `app.resources.datasources`.
fn check_dataview_datasource_refs(
    app: &LoadedApp,
    resource_ds_names: &HashSet<&str>,
    config_ds_names: &HashSet<&str>,
    results: &mut Vec<ValidationResult>,
) {
    let app_name = &app.manifest.app_name;

    for (dv_name, dv) in &app.config.data.dataviews {
        let ds_ref = dv.datasource.as_str();

        if resource_ds_names.contains(ds_ref) || config_ds_names.contains(ds_ref) {
            results.push(
                ValidationResult::pass(
                    format!("{}/app.toml", app_name),
                    format!(
                        "DataView '{}' datasource '{}' resolved",
                        dv_name, ds_ref,
                    ),
                )
                .with_app(app_name)
                .with_crossref(
                    format!("data.dataviews.{}", dv_name),
                    ds_ref,
                    "datasource",
                ),
            );
        } else {
            results.push(
                ValidationResult::fail(
                    error_codes::X001,
                    format!("{}/app.toml", app_name),
                    format!(
                        "DataView '{}' references datasource '{}' not declared in {}/resources.toml",
                        dv_name, ds_ref, app_name,
                    ),
                )
                .with_app(app_name)
                .with_crossref(
                    format!("data.dataviews.{}", dv_name),
                    ds_ref,
                    "datasource",
                ),
            );
        }
    }
}

/// X004: Invalidates targets must refer to existing DataView names.
fn check_invalidates_targets(
    app: &LoadedApp,
    dataview_names: &HashSet<&str>,
    results: &mut Vec<ValidationResult>,
) {
    let app_name = &app.manifest.app_name;

    for (dv_name, dv) in &app.config.data.dataviews {
        for target in &dv.invalidates {
            if dataview_names.contains(target.as_str()) {
                results.push(
                    ValidationResult::pass(
                        format!("{}/app.toml", app_name),
                        format!(
                            "DataView '{}' invalidates target '{}' resolved",
                            dv_name, target,
                        ),
                    )
                    .with_app(app_name)
                    .with_crossref(
                        format!("data.dataviews.{}.invalidates", dv_name),
                        target.as_str(),
                        "dataview",
                    ),
                );
            } else {
                results.push(
                    ValidationResult::fail(
                        error_codes::X004,
                        format!("{}/app.toml", app_name),
                        format!(
                            "Invalidates target '{}' does not exist in {}",
                            target, app_name,
                        ),
                    )
                    .with_app(app_name)
                    .with_crossref(
                        format!("data.dataviews.{}.invalidates", dv_name),
                        target.as_str(),
                        "dataview",
                    ),
                );
            }
        }
    }
}

/// X002, X003, X008, X009, X010: View handler references and type constraints.
fn check_view_refs(
    app: &LoadedApp,
    dataview_names: &HashSet<&str>,
    resource_ds_names: &HashSet<&str>,
    results: &mut Vec<ValidationResult>,
) {
    let app_name = &app.manifest.app_name;

    for (view_name, view) in &app.config.api.views {
        let view_type = view.view_type.as_str();

        match &view.handler {
            HandlerConfig::Dataview { dataview } => {
                // X008: Dataview handler only valid for view_type=Rest
                if view_type != "Rest" {
                    results.push(
                        ValidationResult::fail(
                            error_codes::X008,
                            format!("{}/app.toml", app_name),
                            format!(
                                "View '{}': dataview handler only valid for view_type=Rest",
                                view_name,
                            ),
                        )
                        .with_app(app_name)
                        .with_table_path(format!("api.views.{}", view_name))
                        .with_field("handler.type"),
                    );
                }

                // X002: View → DataView reference
                if dataview_names.contains(dataview.as_str()) {
                    results.push(
                        ValidationResult::pass(
                            format!("{}/app.toml", app_name),
                            format!(
                                "View '{}' dataview handler '{}' resolved",
                                view_name, dataview,
                            ),
                        )
                        .with_app(app_name)
                        .with_crossref(
                            format!("api.views.{}.handler", view_name),
                            dataview.as_str(),
                            "dataview",
                        ),
                    );
                } else {
                    results.push(
                        ValidationResult::fail(
                            error_codes::X002,
                            format!("{}/app.toml", app_name),
                            format!(
                                "View '{}' references dataview '{}' not declared in {}/app.toml",
                                view_name, dataview, app_name,
                            ),
                        )
                        .with_app(app_name)
                        .with_crossref(
                            format!("api.views.{}.handler", view_name),
                            dataview.as_str(),
                            "dataview",
                        ),
                    );
                }
            }
            HandlerConfig::Codecomponent { resources, .. } => {
                // X003: Each resource must be a declared datasource
                for res in resources {
                    if resource_ds_names.contains(res.as_str()) {
                        results.push(
                            ValidationResult::pass(
                                format!("{}/app.toml", app_name),
                                format!(
                                    "View '{}' handler resource '{}' resolved",
                                    view_name, res,
                                ),
                            )
                            .with_app(app_name)
                            .with_crossref(
                                format!("api.views.{}.handler.resources", view_name),
                                res.as_str(),
                                "datasource",
                            ),
                        );
                    } else {
                        results.push(
                            ValidationResult::fail(
                                error_codes::X003,
                                format!("{}/app.toml", app_name),
                                format!(
                                    "View '{}' handler resource '{}' not declared in {}/resources.toml",
                                    view_name, res, app_name,
                                ),
                            )
                            .with_app(app_name)
                            .with_crossref(
                                format!("api.views.{}.handler.resources", view_name),
                                res.as_str(),
                                "datasource",
                            ),
                        );
                    }
                }
            }
            HandlerConfig::None {} => {
                // No references to check.
            }
        }

        // X009: WebSocket requires method=GET
        if view_type == "Websocket" {
            if let Some(method) = &view.method {
                if method.to_uppercase() != "GET" {
                    results.push(
                        ValidationResult::fail(
                            error_codes::X009,
                            format!("{}/app.toml", app_name),
                            format!(
                                "View '{}': method must be GET when view_type=Websocket",
                                view_name,
                            ),
                        )
                        .with_app(app_name)
                        .with_table_path(format!("api.views.{}", view_name))
                        .with_field("method"),
                    );
                }
            }
        }

        // X010: SSE requires method=GET
        if view_type == "ServerSentEvents" {
            if let Some(method) = &view.method {
                if method.to_uppercase() != "GET" {
                    results.push(
                        ValidationResult::fail(
                            error_codes::X010,
                            format!("{}/app.toml", app_name),
                            format!(
                                "View '{}': method must be GET when view_type=ServerSentEvents",
                                view_name,
                            ),
                        )
                        .with_app(app_name)
                        .with_table_path(format!("api.views.{}", view_name))
                        .with_field("method"),
                    );
                }
            }
        }
    }
}

/// W004: Warn if no views are defined.
fn check_views_exist(app: &LoadedApp, results: &mut Vec<ValidationResult>) {
    let app_name = &app.manifest.app_name;

    if app.config.api.views.is_empty() {
        results.push(
            ValidationResult::warn(
                error_codes::W004,
                format!(
                    "{}: no views defined — check [api.views.*] (not [views.*])",
                    app_name,
                ),
            )
            .with_app(app_name),
        );
    }
}

/// X005: Service dependency appId must resolve within the bundle.
fn check_service_app_refs(
    app: &LoadedApp,
    known_app_ids: &HashSet<&str>,
    results: &mut Vec<ValidationResult>,
) {
    let app_name = &app.manifest.app_name;

    for svc in &app.resources.services {
        if known_app_ids.contains(svc.app_id.as_str()) {
            results.push(
                ValidationResult::pass(
                    format!("{}/resources.toml", app_name),
                    format!(
                        "Service '{}' appId '{}' resolved",
                        svc.name, svc.app_id,
                    ),
                )
                .with_app(app_name)
                .with_crossref(
                    format!("services.{}", svc.name),
                    svc.app_id.as_str(),
                    "appId",
                ),
            );
        } else {
            results.push(
                ValidationResult::fail(
                    error_codes::X005,
                    format!("{}/resources.toml", app_name),
                    format!(
                        "Service '{}' references appId '{}' not found in bundle",
                        svc.name, svc.app_id,
                    ),
                )
                .with_app(app_name)
                .with_crossref(
                    format!("services.{}", svc.name),
                    svc.app_id.as_str(),
                    "appId",
                ),
            );
        }
    }
}

/// X013: x-type must match driver name (if x-type is present).
fn check_xtype_driver_match(app: &LoadedApp, results: &mut Vec<ValidationResult>) {
    let app_name = &app.manifest.app_name;

    for ds in &app.resources.datasources {
        if let Some(x_type) = &ds.x_type {
            if x_type != &ds.driver {
                results.push(
                    ValidationResult::fail(
                        error_codes::X013,
                        format!("{}/resources.toml", app_name),
                        format!(
                            "Datasource '{}': x-type '{}' does not match driver '{}'",
                            ds.name, x_type, ds.driver,
                        ),
                    )
                    .with_app(app_name)
                    .with_table_path("datasources")
                    .with_field("x-type"),
                );
            } else {
                results.push(
                    ValidationResult::pass(
                        format!("{}/resources.toml", app_name),
                        format!(
                            "Datasource '{}': x-type matches driver '{}'",
                            ds.name, ds.driver,
                        ),
                    )
                    .with_app(app_name),
                );
            }
        }
    }
}

/// X011: SPA config is only valid on app-main.
fn check_spa_app_type(app: &LoadedApp, results: &mut Vec<ValidationResult>) {
    let app_name = &app.manifest.app_name;

    if app.manifest.spa.is_some() && app.manifest.app_type != "app-main" {
        results.push(
            ValidationResult::fail(
                error_codes::X011,
                format!("{}/manifest.toml", app_name),
                format!("SPA config is only valid on app-main in {}", app_name),
            )
            .with_app(app_name)
            .with_table_path("spa")
            .with_field("type"),
        );
    }
}

/// X012: Init handler must have both module and entrypoint.
///
/// The struct requires both fields, so this will only trigger if serde
/// defaults allow empty strings. We validate non-empty as a safety net.
fn check_init_handler(app: &LoadedApp, results: &mut Vec<ValidationResult>) {
    let app_name = &app.manifest.app_name;

    if let Some(init) = &app.manifest.init {
        if init.module.trim().is_empty() || init.entrypoint.trim().is_empty() {
            results.push(
                ValidationResult::fail(
                    error_codes::X012,
                    format!("{}/manifest.toml", app_name),
                    format!(
                        "Init handler declared but missing module or entrypoint in {}",
                        app_name,
                    ),
                )
                .with_app(app_name)
                .with_table_path("init"),
            );
        }
    }
}

/// Nopassword + credentials_source consistency check.
///
/// Per spec §4.3: `nopassword=true` with `lockbox` set is contradictory.
/// This is also covered by S006 in the structural layer, but we double-check
/// at the cross-reference layer using the resources.toml datasource declarations.
fn check_nopassword_credentials(app: &LoadedApp, results: &mut Vec<ValidationResult>) {
    let app_name = &app.manifest.app_name;

    for ds in &app.resources.datasources {
        if ds.nopassword && ds.lockbox.is_some() {
            results.push(
                ValidationResult::fail(
                    error_codes::S006,
                    format!("{}/resources.toml", app_name),
                    format!(
                        "Datasource '{}': nopassword=true but lockbox is also set",
                        ds.name,
                    ),
                )
                .with_app(app_name)
                .with_table_path("datasources")
                .with_field("nopassword"),
            );
        }
    }
}

/// CB001: Warn when a circuitBreakerId is referenced by only one DataView.
///
/// A solo breaker ID is likely a typo — circuit breakers are only useful
/// when shared across multiple DataViews for coordinated fault isolation.
/// Includes a "did you mean?" suggestion when another breaker ID is close.
fn check_solo_circuit_breaker_ids(app: &LoadedApp, results: &mut Vec<ValidationResult>) {
    let mut breaker_usage: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for (dv_name, dv_config) in &app.config.data.dataviews {
        if let Some(ref breaker_id) = dv_config.circuit_breaker_id {
            breaker_usage
                .entry(breaker_id.clone())
                .or_default()
                .push(dv_name.clone());
        }
    }

    let all_breaker_ids: Vec<&str> = breaker_usage.keys().map(|s| s.as_str()).collect();
    for (breaker_id, dataviews) in &breaker_usage {
        if dataviews.len() == 1 {
            let mut msg = format!(
                "circuitBreakerId '{}' is referenced by only one DataView ('{}')",
                breaker_id, dataviews[0]
            );
            if all_breaker_ids.len() > 1 {
                let others: Vec<&str> = all_breaker_ids
                    .iter()
                    .filter(|id| **id != breaker_id.as_str())
                    .copied()
                    .collect();
                if let Some(suggestion) = crate::validate_format::suggest_key(breaker_id, &others) {
                    msg = format!("{} \u{2014} {}", msg, suggestion);
                }
            }
            let mut result = ValidationResult::warn(error_codes::CB001, msg);
            result.file = Some(format!("{}/app.toml", app.manifest.app_name));
            results.push(result);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::*;
    use crate::datasource::DatasourceConfig;
    use crate::dataview::DataViewConfig;
    use crate::loader::{LoadedApp, LoadedBundle};
    use crate::validate_result::ValidationStatus;
    use crate::view::{ApiViewConfig, HandlerConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ── Helpers ────────────────────────────────────────────────────

    fn make_manifest(name: &str, app_type: &str, app_id: &str) -> AppManifest {
        AppManifest {
            app_name: name.into(),
            description: None,
            version: None,
            app_type: app_type.into(),
            app_id: app_id.into(),
            entry_point: None,
            app_entry_point: None,
            source: None,
            spa: None,
            init: None,
        }
    }

    fn make_resource_ds(name: &str, driver: &str) -> ResourceDatasource {
        ResourceDatasource {
            name: name.into(),
            driver: driver.into(),
            lockbox: None,
            nopassword: true,
            x_type: None,
            required: true,
        }
    }

    fn make_ds_config(name: &str, driver: &str) -> DatasourceConfig {
        DatasourceConfig {
            name: name.into(),
            driver: driver.into(),
            host: None,
            port: None,
            database: None,
            username: None,
            credentials_source: None,
            nopassword: true,
            x_type: None,
            connection_pool: Default::default(),
            consumer: None,
            event_handlers: None,
            extra: HashMap::new(),
            write_batch: None,
        }
    }

    fn make_dv_config(name: &str, datasource: &str) -> DataViewConfig {
        DataViewConfig {
            name: name.into(),
            datasource: datasource.into(),
            query: None,
            parameters: vec![],
            return_schema: None,
            get_query: None,
            post_query: None,
            put_query: None,
            delete_query: None,
            get_schema: None,
            post_schema: None,
            put_schema: None,
            delete_schema: None,
            get_parameters: vec![],
            post_parameters: vec![],
            put_parameters: vec![],
            delete_parameters: vec![],
            streaming: false,
            circuit_breaker_id: None,
            caching: None,
            invalidates: vec![],
            validate_result: false,
            strict_parameters: false,
            max_rows: 1000,
        }
    }

    fn make_view_dataview(view_type: &str, dataview: &str) -> ApiViewConfig {
        ApiViewConfig {
            view_type: view_type.into(),
            path: Some("/test".into()),
            method: Some("GET".into()),
            handler: HandlerConfig::Dataview {
                dataview: dataview.into(),
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
        }
    }

    fn make_view_codecomponent(resources: Vec<String>) -> ApiViewConfig {
        ApiViewConfig {
            view_type: "Rest".into(),
            path: Some("/test".into()),
            method: Some("POST".into()),
            handler: HandlerConfig::Codecomponent {
                language: "javascript".into(),
                module: "handlers/test.js".into(),
                entrypoint: "onRequest".into(),
                resources,
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
        }
    }

    fn make_view_none(view_type: &str, method: &str) -> ApiViewConfig {
        ApiViewConfig {
            view_type: view_type.into(),
            path: Some("/test".into()),
            method: Some(method.into()),
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
            ws_hooks: None,
            on_event: None,
        }
    }

    fn make_app(
        name: &str,
        app_type: &str,
        app_id: &str,
        resource_datasources: Vec<ResourceDatasource>,
        services: Vec<ServiceDependency>,
        datasources: HashMap<String, DatasourceConfig>,
        dataviews: HashMap<String, DataViewConfig>,
        views: HashMap<String, ApiViewConfig>,
    ) -> LoadedApp {
        LoadedApp {
            manifest: make_manifest(name, app_type, app_id),
            resources: ResourcesConfig {
                datasources: resource_datasources,
                keystores: vec![],
                services,
            },
            config: AppConfig {
                data: AppDataConfig {
                    datasources,
                    dataviews,
                    keystore: HashMap::new(),
                },
                api: AppApiConfig { views },
                static_files: None,
            },
            app_dir: PathBuf::from(format!("/tmp/{}", name)),
        }
    }

    fn make_bundle(apps: Vec<LoadedApp>) -> LoadedBundle {
        let app_names: Vec<String> = apps.iter().map(|a| a.manifest.app_name.clone()).collect();
        LoadedBundle {
            manifest: BundleManifest {
                bundle_name: "test-bundle".into(),
                bundle_version: "1.0.0".into(),
                source: None,
                apps: app_names,
            },
            apps,
        }
    }

    fn has_fail(results: &[ValidationResult], code: &str) -> bool {
        results.iter().any(|r| {
            r.status == ValidationStatus::Fail && r.error_code.as_deref() == Some(code)
        })
    }

    fn has_pass(results: &[ValidationResult]) -> bool {
        results.iter().any(|r| r.status == ValidationStatus::Pass)
    }

    fn has_warn(results: &[ValidationResult], code: &str) -> bool {
        results.iter().any(|r| {
            r.status == ValidationStatus::Warn && r.error_code.as_deref() == Some(code)
        })
    }

    // ── X001: DataView → datasource ─────────────────────────────────

    #[test]
    fn x001_dataview_references_valid_datasource() {
        let mut datasources = HashMap::new();
        datasources.insert("contacts-db".into(), make_ds_config("contacts-db", "faker"));

        let mut dataviews = HashMap::new();
        dataviews.insert("list_contacts".into(), make_dv_config("list_contacts", "contacts-db"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("contacts-db", "faker")],
            vec![],
            datasources,
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X001));
        assert!(has_pass(&results));
    }

    #[test]
    fn x001_dataview_references_missing_datasource() {
        let mut dataviews = HashMap::new();
        dataviews.insert(
            "list_contacts".into(),
            make_dv_config("list_contacts", "nonexistent-db"),
        );

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X001));
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some(error_codes::X001))
            .unwrap();
        assert!(fail.message.contains("nonexistent-db"));
        assert!(fail.target.as_deref() == Some("nonexistent-db"));
    }

    #[test]
    fn x001_dataview_resolves_via_resource_ds_only() {
        // DataView references a datasource that's only in resources.toml, not in app.toml.
        let mut dataviews = HashMap::new();
        dataviews.insert("lookup".into(), make_dv_config("lookup", "my-db"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("my-db", "postgres")],
            vec![],
            HashMap::new(), // no config-level datasources
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X001));
    }

    // ── X002: View → DataView ───────────────────────────────────────

    #[test]
    fn x002_view_references_valid_dataview() {
        let mut dataviews = HashMap::new();
        dataviews.insert("list_contacts".into(), make_dv_config("list_contacts", "db"));

        let mut views = HashMap::new();
        views.insert("get_contacts".into(), make_view_dataview("Rest", "list_contacts"));

        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X002));
    }

    #[test]
    fn x002_view_references_missing_dataview() {
        let mut views = HashMap::new();
        views.insert("get_contacts".into(), make_view_dataview("Rest", "ghost_dv"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X002));
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some(error_codes::X002))
            .unwrap();
        assert!(fail.message.contains("ghost_dv"));
    }

    // ── X003: View handler resources → datasources ──────────────────

    #[test]
    fn x003_codecomponent_resources_resolve() {
        let mut views = HashMap::new();
        views.insert(
            "create_contact".into(),
            make_view_codecomponent(vec!["contacts-db".into()]),
        );

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("contacts-db", "postgres")],
            vec![],
            HashMap::new(),
            HashMap::new(),
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X003));
    }

    #[test]
    fn x003_codecomponent_resource_not_declared() {
        let mut views = HashMap::new();
        views.insert(
            "create_contact".into(),
            make_view_codecomponent(vec!["missing-db".into()]),
        );

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X003));
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some(error_codes::X003))
            .unwrap();
        assert!(fail.message.contains("missing-db"));
    }

    // ── X004: Invalidates targets ───────────────────────────────────

    #[test]
    fn x004_invalidates_target_exists() {
        let mut dv = make_dv_config("create_contact", "db");
        dv.invalidates = vec!["list_contacts".into()];

        let mut dataviews = HashMap::new();
        dataviews.insert("create_contact".into(), dv);
        dataviews.insert("list_contacts".into(), make_dv_config("list_contacts", "db"));

        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X004));
    }

    #[test]
    fn x004_invalidates_target_missing() {
        let mut dv = make_dv_config("create_contact", "db");
        dv.invalidates = vec!["nonexistent_dv".into()];

        let mut dataviews = HashMap::new();
        dataviews.insert("create_contact".into(), dv);

        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X004));
    }

    // ── X005: Service → appId ───────────────────────────────────────

    #[test]
    fn x005_service_references_valid_app_id() {
        let svc = ServiceDependency {
            name: "backend-api".into(),
            app_id: "00000000-0000-0000-0000-000000000002".into(),
            required: true,
        };

        let app1 = make_app(
            "frontend",
            "app-main",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![svc],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let app2 = make_app(
            "backend",
            "app-service",
            "00000000-0000-0000-0000-000000000002",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app1, app2]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X005));
    }

    #[test]
    fn x005_service_references_missing_app_id() {
        let svc = ServiceDependency {
            name: "backend-api".into(),
            app_id: "00000000-0000-0000-0000-999999999999".into(),
            required: true,
        };

        let app = make_app(
            "frontend",
            "app-main",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![svc],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X005));
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some(error_codes::X005))
            .unwrap();
        assert!(fail.message.contains("999999999999"));
    }

    // ── X006: Duplicate appId ───────────────────────────────────────

    #[test]
    fn x006_duplicate_app_id() {
        let app1 = make_app(
            "app-one",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let app2 = make_app(
            "app-two",
            "app-service",
            "00000000-0000-0000-0000-000000000001", // same!
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app1, app2]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X006));
    }

    #[test]
    fn x006_unique_app_ids_pass() {
        let app1 = make_app(
            "app-one",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let app2 = make_app(
            "app-two",
            "app-service",
            "00000000-0000-0000-0000-000000000002",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app1, app2]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X006));
    }

    // ── X007: Duplicate datasource names ────────────────────────────

    #[test]
    fn x007_duplicate_datasource_names() {
        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![
                make_resource_ds("db", "postgres"),
                make_resource_ds("db", "mysql"), // duplicate name
            ],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X007));
    }

    #[test]
    fn x007_unique_datasource_names_pass() {
        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![
                make_resource_ds("pg-db", "postgres"),
                make_resource_ds("redis-db", "redis"),
            ],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X007));
    }

    // ── X008: Dataview handler on non-Rest view ─────────────────────

    #[test]
    fn x008_dataview_handler_on_websocket() {
        let mut dataviews = HashMap::new();
        dataviews.insert("feed".into(), make_dv_config("feed", "db"));

        let mut views = HashMap::new();
        views.insert("ws_feed".into(), make_view_dataview("Websocket", "feed"));

        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X008));
    }

    #[test]
    fn x008_dataview_handler_on_rest_passes() {
        let mut dataviews = HashMap::new();
        dataviews.insert("list".into(), make_dv_config("list", "db"));

        let mut views = HashMap::new();
        views.insert("get_list".into(), make_view_dataview("Rest", "list"));

        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X008));
    }

    // ── X009: WebSocket method must be GET ──────────────────────────

    #[test]
    fn x009_websocket_with_post_method() {
        let mut views = HashMap::new();
        views.insert("ws_feed".into(), make_view_none("Websocket", "POST"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X009));
    }

    #[test]
    fn x009_websocket_with_get_method_passes() {
        let mut views = HashMap::new();
        views.insert("ws_feed".into(), make_view_none("Websocket", "GET"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X009));
    }

    // ── X010: SSE method must be GET ────────────────────────────────

    #[test]
    fn x010_sse_with_post_method() {
        let mut views = HashMap::new();
        views.insert("sse_stream".into(), make_view_none("ServerSentEvents", "POST"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X010));
    }

    #[test]
    fn x010_sse_with_get_method_passes() {
        let mut views = HashMap::new();
        views.insert("sse_stream".into(), make_view_none("ServerSentEvents", "GET"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X010));
    }

    // ── X011: SPA on app-service ────────────────────────────────────

    #[test]
    fn x011_spa_on_app_service() {
        let mut manifest = make_manifest(
            "bad-service",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
        );
        manifest.spa = Some(SpaConfig {
            root: "build".into(),
            index_file: "index.html".into(),
            fallback: true,
            max_age: None,
        });

        let app = LoadedApp {
            manifest,
            resources: ResourcesConfig::default(),
            config: AppConfig::default(),
            app_dir: PathBuf::from("/tmp/bad-service"),
        };
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X011));
    }

    #[test]
    fn x011_spa_on_app_main_passes() {
        let mut manifest = make_manifest(
            "good-main",
            "app-main",
            "00000000-0000-0000-0000-000000000001",
        );
        manifest.spa = Some(SpaConfig {
            root: "build".into(),
            index_file: "index.html".into(),
            fallback: true,
            max_age: None,
        });

        let app = LoadedApp {
            manifest,
            resources: ResourcesConfig::default(),
            config: AppConfig::default(),
            app_dir: PathBuf::from("/tmp/good-main"),
        };
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X011));
    }

    // ── X012: Init handler completeness ─────────────────────────────

    #[test]
    fn x012_init_handler_empty_module() {
        let mut manifest = make_manifest(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
        );
        manifest.init = Some(InitHandlerConfig {
            module: "".into(),
            entrypoint: "init".into(),
        });

        let app = LoadedApp {
            manifest,
            resources: ResourcesConfig::default(),
            config: AppConfig::default(),
            app_dir: PathBuf::from("/tmp/test-app"),
        };
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X012));
    }

    #[test]
    fn x012_init_handler_empty_entrypoint() {
        let mut manifest = make_manifest(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
        );
        manifest.init = Some(InitHandlerConfig {
            module: "handlers/init.js".into(),
            entrypoint: "  ".into(), // whitespace only
        });

        let app = LoadedApp {
            manifest,
            resources: ResourcesConfig::default(),
            config: AppConfig::default(),
            app_dir: PathBuf::from("/tmp/test-app"),
        };
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X012));
    }

    #[test]
    fn x012_valid_init_handler_passes() {
        let mut manifest = make_manifest(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
        );
        manifest.init = Some(InitHandlerConfig {
            module: "handlers/init.js".into(),
            entrypoint: "onInit".into(),
        });

        let app = LoadedApp {
            manifest,
            resources: ResourcesConfig::default(),
            config: AppConfig::default(),
            app_dir: PathBuf::from("/tmp/test-app"),
        };
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X012));
    }

    // ── X013: x-type / driver mismatch ──────────────────────────────

    #[test]
    fn x013_xtype_driver_mismatch() {
        let mut ds = make_resource_ds("db", "postgres");
        ds.x_type = Some("mysql".into()); // mismatch!

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![ds],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X013));
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some(error_codes::X013))
            .unwrap();
        assert!(fail.message.contains("mysql"));
        assert!(fail.message.contains("postgres"));
    }

    #[test]
    fn x013_xtype_driver_match_passes() {
        let mut ds = make_resource_ds("db", "postgres");
        ds.x_type = Some("postgres".into());

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![ds],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X013));
    }

    #[test]
    fn x013_no_xtype_no_check() {
        let ds = make_resource_ds("db", "postgres");
        // x_type is None — no check needed

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![ds],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_fail(&results, error_codes::X013));
    }

    // ── W004: No views ──────────────────────────────────────────────

    #[test]
    fn w004_no_views_produces_warning() {
        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(), // no views
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_warn(&results, error_codes::W004));
    }

    #[test]
    fn w004_views_present_no_warning() {
        let mut views = HashMap::new();
        views.insert("list".into(), make_view_none("Rest", "GET"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_warn(&results, error_codes::W004));
    }

    // ── S006: nopassword + lockbox contradiction ────────────────────

    #[test]
    fn s006_nopassword_with_lockbox() {
        let ds = ResourceDatasource {
            name: "db".into(),
            driver: "postgres".into(),
            lockbox: Some("lockbox://db/test".into()),
            nopassword: true,
            x_type: None,
            required: true,
        };

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![ds],
            vec![],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::S006));
    }

    // ── Integration: full bundle ────────────────────────────────────

    #[test]
    fn valid_address_book_bundle_passes() {
        // Simulate the address-book-bundle structure.
        let mut service_datasources = HashMap::new();
        service_datasources.insert(
            "contacts-faker".into(),
            make_ds_config("contacts-faker", "faker"),
        );

        let mut service_dataviews = HashMap::new();
        service_dataviews.insert(
            "list_contacts".into(),
            make_dv_config("list_contacts", "contacts-faker"),
        );
        service_dataviews.insert(
            "get_contact".into(),
            make_dv_config("get_contact", "contacts-faker"),
        );

        let mut service_views = HashMap::new();
        service_views.insert(
            "list".into(),
            make_view_dataview("Rest", "list_contacts"),
        );
        service_views.insert(
            "get".into(),
            make_view_dataview("Rest", "get_contact"),
        );

        let service = make_app(
            "address-book-service",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("contacts-faker", "faker")],
            vec![],
            service_datasources,
            service_dataviews,
            service_views,
        );

        let svc_dep = ServiceDependency {
            name: "address-book-api".into(),
            app_id: "00000000-0000-0000-0000-000000000001".into(),
            required: true,
        };

        let main = make_app(
            "address-book-main",
            "app-main",
            "00000000-0000-0000-0000-000000000002",
            vec![],
            vec![svc_dep],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );

        let bundle = make_bundle(vec![service, main]);
        let results = validate_crossref(&bundle);

        // Should have no errors (only passes and possibly a W004 for main having no views).
        let failures: Vec<_> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Fail)
            .collect();
        assert!(
            failures.is_empty(),
            "Expected no failures, got: {:?}",
            failures,
        );
    }

    #[test]
    fn multiple_errors_in_one_app() {
        // DataView references missing datasource (X001)
        // View references missing dataview (X002)
        // Invalidates target missing (X004)
        let mut dv = make_dv_config("create_item", "ghost-db");
        dv.invalidates = vec!["phantom_dv".into()];

        let mut dataviews = HashMap::new();
        dataviews.insert("create_item".into(), dv);

        let mut views = HashMap::new();
        views.insert("get_items".into(), make_view_dataview("Rest", "missing_dv"));

        let app = make_app(
            "broken-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            dataviews,
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(has_fail(&results, error_codes::X001), "expected X001");
        assert!(has_fail(&results, error_codes::X002), "expected X002");
        assert!(has_fail(&results, error_codes::X004), "expected X004");
    }

    #[test]
    fn crossref_results_include_app_context() {
        let mut dataviews = HashMap::new();
        dataviews.insert("lookup".into(), make_dv_config("lookup", "missing-db"));

        let app = make_app(
            "my-service",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![],
            vec![],
            HashMap::new(),
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        let x001 = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some(error_codes::X001))
            .unwrap();
        assert_eq!(x001.app.as_deref(), Some("my-service"));
        assert!(x001.source.is_some());
        assert!(x001.target.is_some());
        assert!(x001.target_type.is_some());
    }

    #[test]
    fn empty_bundle_produces_no_errors() {
        let bundle = LoadedBundle {
            manifest: BundleManifest {
                bundle_name: "empty".into(),
                bundle_version: "0.1.0".into(),
                source: None,
                apps: vec![],
            },
            apps: vec![],
        };
        let results = validate_crossref(&bundle);

        let failures: Vec<_> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Fail)
            .collect();
        assert!(failures.is_empty());
    }

    #[test]
    fn result_count_sanity() {
        // A simple valid app should produce only pass/warn results.
        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        let mut dataviews = HashMap::new();
        dataviews.insert("list".into(), make_dv_config("list", "db"));

        let mut views = HashMap::new();
        views.insert("get_list".into(), make_view_dataview("Rest", "list"));

        let app = make_app(
            "simple-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            views,
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        let pass_count = results.iter().filter(|r| r.status == ValidationStatus::Pass).count();
        let fail_count = results.iter().filter(|r| r.status == ValidationStatus::Fail).count();

        assert!(pass_count > 0, "should have pass results");
        assert_eq!(fail_count, 0, "should have no failures");
    }

    // ── CB001: Solo circuit breaker ID ──────────────────────────────

    #[test]
    fn cb001_solo_breaker_id_produces_warning() {
        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        // Two DataViews: one with a shared breaker ID (used twice), one solo.
        let mut dv_shared_a = make_dv_config("list_orders", "db");
        dv_shared_a.circuit_breaker_id = Some("orders_cb".into());

        let mut dv_shared_b = make_dv_config("get_order", "db");
        dv_shared_b.circuit_breaker_id = Some("orders_cb".into());

        let mut dv_solo = make_dv_config("get_summary", "db");
        dv_solo.circuit_breaker_id = Some("summry_cb".into()); // typo: summry vs summary

        let mut dataviews = HashMap::new();
        dataviews.insert("list_orders".into(), dv_shared_a);
        dataviews.insert("get_order".into(), dv_shared_b);
        dataviews.insert("get_summary".into(), dv_solo);

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        // CB001 warning should be emitted for the solo breaker ID only.
        assert!(has_warn(&results, error_codes::CB001), "expected CB001 warning");

        let cb001 = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some(error_codes::CB001))
            .unwrap();
        assert!(
            cb001.message.contains("summry_cb"),
            "warning should name the solo breaker ID, got: {}",
            cb001.message,
        );
        // The shared breaker ID must NOT trigger a warning.
        let cb001_count = results
            .iter()
            .filter(|r| r.error_code.as_deref() == Some(error_codes::CB001))
            .count();
        assert_eq!(cb001_count, 1, "only one CB001 warning expected");
    }

    #[test]
    fn cb001_no_warning_when_breaker_shared() {
        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        let mut dv_a = make_dv_config("list_orders", "db");
        dv_a.circuit_breaker_id = Some("orders_cb".into());

        let mut dv_b = make_dv_config("get_order", "db");
        dv_b.circuit_breaker_id = Some("orders_cb".into());

        let mut dataviews = HashMap::new();
        dataviews.insert("list_orders".into(), dv_a);
        dataviews.insert("get_order".into(), dv_b);

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_warn(&results, error_codes::CB001), "no CB001 when breaker is shared");
    }

    #[test]
    fn cb001_solo_breaker_includes_did_you_mean_suggestion() {
        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        // "orders_cb" (shared) and "ordes_cb" (solo typo — 1 edit from "orders_cb").
        let mut dv_a = make_dv_config("list_orders", "db");
        dv_a.circuit_breaker_id = Some("orders_cb".into());

        let mut dv_b = make_dv_config("get_order", "db");
        dv_b.circuit_breaker_id = Some("orders_cb".into());

        let mut dv_solo = make_dv_config("get_summary", "db");
        dv_solo.circuit_breaker_id = Some("ordes_cb".into()); // 1-char typo

        let mut dataviews = HashMap::new();
        dataviews.insert("list_orders".into(), dv_a);
        dataviews.insert("get_order".into(), dv_b);
        dataviews.insert("get_summary".into(), dv_solo);

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        let cb001 = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some(error_codes::CB001))
            .unwrap();
        assert!(
            cb001.message.contains("did you mean 'orders_cb'?"),
            "expected Levenshtein suggestion in message, got: {}",
            cb001.message,
        );
    }

    #[test]
    fn cb001_no_warning_when_no_breaker_ids() {
        let mut datasources = HashMap::new();
        datasources.insert("db".into(), make_ds_config("db", "faker"));

        let mut dataviews = HashMap::new();
        dataviews.insert("list_orders".into(), make_dv_config("list_orders", "db"));

        let app = make_app(
            "test-app",
            "app-service",
            "00000000-0000-0000-0000-000000000001",
            vec![make_resource_ds("db", "faker")],
            vec![],
            datasources,
            dataviews,
            HashMap::new(),
        );
        let bundle = make_bundle(vec![app]);
        let results = validate_crossref(&bundle);

        assert!(!has_warn(&results, error_codes::CB001));
    }
}
