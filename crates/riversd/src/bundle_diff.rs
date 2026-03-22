//! Bundle diff — compares old and new bundles for hot reload logging.
//!
//! Produces a human-readable summary of what changed between two bundle versions.

use std::collections::{HashMap, HashSet};
use std::fmt;

use rivers_runtime::LoadedBundle;

/// Summary of differences between two bundle versions.
#[derive(Debug)]
pub struct BundleDiff {
    pub dataviews: SectionDiff,
    pub views: SectionDiff,
    pub datasources: DatasourceDiff,
}

/// Added/removed/changed items in a section.
#[derive(Debug, Default)]
pub struct SectionDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

/// Datasource-specific diff with restart detection.
#[derive(Debug, Default)]
pub struct DatasourceDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<DatasourceChange>,
}

/// A single datasource change with restart requirement.
#[derive(Debug)]
pub struct DatasourceChange {
    pub name: String,
    pub fields_changed: Vec<String>,
    pub requires_restart: bool,
}

impl BundleDiff {
    pub fn is_empty(&self) -> bool {
        self.dataviews.added.is_empty()
            && self.dataviews.removed.is_empty()
            && self.dataviews.changed.is_empty()
            && self.views.added.is_empty()
            && self.views.removed.is_empty()
            && self.views.changed.is_empty()
            && self.datasources.added.is_empty()
            && self.datasources.removed.is_empty()
            && self.datasources.changed.is_empty()
    }
}

impl fmt::Display for BundleDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(f, "no changes detected");
        }

        let mut parts = Vec::new();

        let dv = &self.dataviews;
        if !dv.added.is_empty() {
            parts.push(format!("{} DataView(s) added", dv.added.len()));
        }
        if !dv.removed.is_empty() {
            parts.push(format!("{} DataView(s) removed", dv.removed.len()));
        }
        if !dv.changed.is_empty() {
            parts.push(format!("{} DataView(s) changed", dv.changed.len()));
        }

        let v = &self.views;
        if !v.added.is_empty() {
            parts.push(format!("{} view(s) added", v.added.len()));
        }
        if !v.removed.is_empty() {
            parts.push(format!("{} view(s) removed", v.removed.len()));
        }
        if !v.changed.is_empty() {
            parts.push(format!("{} view(s) changed", v.changed.len()));
        }

        let ds = &self.datasources;
        if !ds.added.is_empty() {
            parts.push(format!("{} datasource(s) added", ds.added.len()));
        }
        if !ds.removed.is_empty() {
            parts.push(format!("{} datasource(s) removed", ds.removed.len()));
        }
        for change in &ds.changed {
            if change.requires_restart {
                parts.push(format!(
                    "datasource '{}' {} changed (requires restart)",
                    change.name,
                    change.fields_changed.join(", "),
                ));
            } else {
                parts.push(format!(
                    "datasource '{}' {} changed",
                    change.name,
                    change.fields_changed.join(", "),
                ));
            }
        }

        write!(f, "{}", parts.join("; "))
    }
}

/// Compare two bundles and return a diff summary.
pub fn diff_bundles(old: &LoadedBundle, new: &LoadedBundle) -> BundleDiff {
    BundleDiff {
        dataviews: diff_dataviews(old, new),
        views: diff_views(old, new),
        datasources: diff_datasources(old, new),
    }
}

fn diff_dataviews(old: &LoadedBundle, new: &LoadedBundle) -> SectionDiff {
    let old_names = collect_dataview_names(old);
    let new_names = collect_dataview_names(new);
    set_diff(&old_names, &new_names)
}

fn diff_views(old: &LoadedBundle, new: &LoadedBundle) -> SectionDiff {
    let old_names = collect_view_names(old);
    let new_names = collect_view_names(new);
    set_diff(&old_names, &new_names)
}

fn diff_datasources(old: &LoadedBundle, new: &LoadedBundle) -> DatasourceDiff {
    let old_map = collect_datasource_map(old);
    let new_map = collect_datasource_map(new);

    let old_keys: HashSet<&String> = old_map.keys().collect();
    let new_keys: HashSet<&String> = new_map.keys().collect();

    let added: Vec<String> = new_keys.difference(&old_keys).map(|k| (*k).clone()).collect();
    let removed: Vec<String> = old_keys.difference(&new_keys).map(|k| (*k).clone()).collect();

    let mut changed = Vec::new();
    for key in old_keys.intersection(&new_keys) {
        let old_ds = &old_map[*key];
        let new_ds = &new_map[*key];

        let mut fields = Vec::new();
        let mut restart = false;

        if old_ds.host != new_ds.host {
            fields.push("host".to_string());
            restart = true;
        }
        if old_ds.port != new_ds.port {
            fields.push("port".to_string());
            restart = true;
        }
        if old_ds.driver != new_ds.driver {
            fields.push("driver".to_string());
            restart = true;
        }
        if old_ds.database != new_ds.database {
            fields.push("database".to_string());
        }
        if old_ds.username != new_ds.username {
            fields.push("username".to_string());
        }

        if !fields.is_empty() {
            changed.push(DatasourceChange {
                name: (*key).clone(),
                fields_changed: fields,
                requires_restart: restart,
            });
        }
    }

    DatasourceDiff { added, removed, changed }
}

/// Collect namespaced DataView names from all apps.
fn collect_dataview_names(bundle: &LoadedBundle) -> HashSet<String> {
    let mut names = HashSet::new();
    for app in &bundle.apps {
        let ep = app.manifest.entry_point.as_deref().unwrap_or(&app.manifest.app_name);
        for dv_name in app.config.data.dataviews.keys() {
            names.insert(format!("{}:{}", ep, dv_name));
        }
    }
    names
}

/// Collect namespaced view names from all apps.
fn collect_view_names(bundle: &LoadedBundle) -> HashSet<String> {
    let mut names = HashSet::new();
    for app in &bundle.apps {
        let ep = app.manifest.entry_point.as_deref().unwrap_or(&app.manifest.app_name);
        for view_name in app.config.api.views.keys() {
            names.insert(format!("{}:{}", ep, view_name));
        }
    }
    names
}

/// Collect namespaced datasource configs from all apps.
fn collect_datasource_map(bundle: &LoadedBundle) -> HashMap<String, &rivers_runtime::DatasourceConfig> {
    let mut map = HashMap::new();
    for app in &bundle.apps {
        let ep = app.manifest.entry_point.as_deref().unwrap_or(&app.manifest.app_name);
        for (ds_name, ds_config) in &app.config.data.datasources {
            map.insert(format!("{}:{}", ep, ds_name), ds_config);
        }
    }
    map
}

/// Simple set diff: added, removed, changed (items in both sets count as "changed" placeholder).
fn set_diff(old: &HashSet<String>, new: &HashSet<String>) -> SectionDiff {
    SectionDiff {
        added: new.difference(old).cloned().collect(),
        removed: old.difference(new).cloned().collect(),
        changed: Vec::new(), // field-level comparison would require PartialEq on configs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_bundle() -> LoadedBundle {
        LoadedBundle {
            manifest: rivers_runtime::BundleManifest {
                bundle_name: "test".into(),
                bundle_version: "1.0".into(),
                source: None,
                apps: vec![],
            },
            apps: vec![],
        }
    }

    fn bundle_with_app(dv_names: &[&str], view_names: &[&str]) -> LoadedBundle {
        use rivers_runtime::datasource::DatasourceConfig;

        let mut dataviews = HashMap::new();
        for name in dv_names {
            dataviews.insert(name.to_string(), rivers_runtime::DataViewConfig {
                name: name.to_string(),
                datasource: "db".into(),
                query: None,
                parameters: vec![],
                return_schema: None,
                invalidates: vec![],
                validate_result: false,
                strict_parameters: false,
                caching: None,
                get_query: None, post_query: None, put_query: None, delete_query: None,
                get_schema: None, post_schema: None, put_schema: None, delete_schema: None,
                get_parameters: vec![], post_parameters: vec![],
                put_parameters: vec![], delete_parameters: vec![],
                streaming: false,
            });
        }

        let mut views = HashMap::new();
        for name in view_names {
            views.insert(name.to_string(), rivers_runtime::ApiViewConfig {
                view_type: "Rest".into(),
                path: Some(format!("/{}", name)),
                method: Some("GET".into()),
                handler: rivers_runtime::view::HandlerConfig::None {},
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
            });
        }

        LoadedBundle {
            manifest: rivers_runtime::BundleManifest {
                bundle_name: "test".into(),
                bundle_version: "1.0".into(),
                source: None,
                apps: vec![],
            },
            apps: vec![rivers_runtime::LoadedApp {
                manifest: rivers_runtime::AppManifest {
                    app_name: "app".into(),
                    description: None,
                    version: None,
                    app_type: "app-service".into(),
                    app_id: "id-1".into(),
                    entry_point: Some("svc".into()),
                    app_entry_point: None,
                    source: None,
                    spa: None,
                },
                resources: rivers_runtime::ResourcesConfig::default(),
                config: rivers_runtime::AppConfig {
                    data: rivers_runtime::bundle::AppDataConfig {
                        datasources: HashMap::new(),
                        dataviews,
                    },
                    api: rivers_runtime::bundle::AppApiConfig { views },
                    static_files: None,
                },
                app_dir: std::path::PathBuf::from("/tmp"),
            }],
        }
    }

    #[test]
    fn identical_bundles_no_diff() {
        let b = empty_bundle();
        let diff = diff_bundles(&b, &b);
        assert!(diff.is_empty());
        assert_eq!(format!("{}", diff), "no changes detected");
    }

    #[test]
    fn added_dataviews_detected() {
        let old = empty_bundle();
        let new = bundle_with_app(&["list_users", "get_user"], &[]);
        let diff = diff_bundles(&old, &new);
        assert_eq!(diff.dataviews.added.len(), 2);
        assert!(diff.dataviews.removed.is_empty());
    }

    #[test]
    fn removed_views_detected() {
        let old = bundle_with_app(&[], &["list", "get", "create"]);
        let new = bundle_with_app(&[], &["list"]);
        let diff = diff_bundles(&old, &new);
        assert_eq!(diff.views.removed.len(), 2);
        assert!(diff.views.added.is_empty());
    }

    #[test]
    fn display_format() {
        let old = empty_bundle();
        let new = bundle_with_app(&["dv1"], &["v1", "v2"]);
        let diff = diff_bundles(&old, &new);
        let display = format!("{}", diff);
        assert!(display.contains("DataView(s) added"));
        assert!(display.contains("view(s) added"));
    }
}
