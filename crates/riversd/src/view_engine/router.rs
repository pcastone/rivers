//! View router — route matching and path parsing.

use std::collections::HashMap;

use rivers_runtime::view::{ApiViewConfig, HandlerConfig};

/// A single segment of a URL path pattern.
#[derive(Debug, Clone)]
pub enum PathSegment {
    /// Literal path segment (exact match).
    Literal(String),
    /// Path parameter like `{id}`.
    Param(String),
}

/// A registered view route for matching incoming requests.
#[derive(Debug, Clone)]
pub struct ViewRoute {
    /// Unique view identifier (qualified with entry point for bundle routes).
    pub view_id: String,
    /// HTTP method this route matches.
    pub method: String,
    /// Original path pattern string (e.g. `/api/orders/{id}`).
    pub path_pattern: String,
    /// Path segments for matching. Each is either a literal or a parameter (starts with '{').
    pub segments: Vec<PathSegment>,
    /// View configuration from the app manifest.
    pub config: ApiViewConfig,
    /// App entry point — used to namespace DataView lookups.
    pub app_entry_point: String,
}

/// Build a namespaced path: `/[prefix]/<bundle>/<app>/<view_path>`.
///
/// Per spec §3.1: all app routes are namespaced by bundle and entry point.
pub fn build_namespaced_path(
    route_prefix: Option<&str>,
    bundle_name: &str,
    entry_point: &str,
    view_path: &str,
) -> String {
    let view_path = view_path.trim_start_matches('/');
    match route_prefix.filter(|p| !p.is_empty()) {
        Some(prefix) => {
            let prefix = prefix.trim_matches('/');
            if view_path.is_empty() {
                format!("/{prefix}/{bundle_name}/{entry_point}")
            } else {
                format!("/{prefix}/{bundle_name}/{entry_point}/{view_path}")
            }
        }
        None => {
            if view_path.is_empty() {
                format!("/{bundle_name}/{entry_point}")
            } else {
                format!("/{bundle_name}/{entry_point}/{view_path}")
            }
        }
    }
}

/// The view router — matches incoming requests to registered view routes.
pub struct ViewRouter {
    routes: Vec<ViewRoute>,
}

impl ViewRouter {
    /// Build a router from a loaded bundle with namespaced paths.
    ///
    /// Per spec §3.1: routes are `/<prefix>/<bundle>/<app>/<view>`.
    pub fn from_bundle(
        bundle: &rivers_runtime::LoadedBundle,
        route_prefix: Option<&str>,
    ) -> Self {
        let bundle_name = &bundle.manifest.bundle_name;
        let mut routes = Vec::new();

        for app in &bundle.apps {
            let entry_point = app
                .manifest
                .entry_point
                .as_deref()
                .unwrap_or(&app.manifest.app_name);

            for (id, config) in &app.config.api.views {
                if config.view_type == "MessageConsumer" {
                    continue;
                }

                let view_path = match &config.path {
                    Some(p) => p.clone(),
                    None => continue,
                };

                let full_path = build_namespaced_path(
                    route_prefix,
                    bundle_name,
                    entry_point,
                    &view_path,
                );

                let method = config
                    .method
                    .as_deref()
                    .unwrap_or("GET")
                    .to_uppercase();

                let segments = parse_path_pattern(&full_path);

                // Key with entry_point to prevent collisions across apps
                let qualified_id = format!("{entry_point}:{id}");

                if config.allow_outbound_http {
                    let module = match &config.handler {
                        HandlerConfig::Codecomponent { module, .. } => module.as_str(),
                        _ => "<none>",
                    };
                    tracing::warn!(
                        target: "rivers.security",
                        view = %qualified_id,
                        module = %module,
                        "view declares allow_outbound_http"
                    );
                }

                tracing::debug!(
                    method = %method,
                    path = %full_path,
                    view_id = %qualified_id,
                    "route registered"
                );

                routes.push(ViewRoute {
                    view_id: qualified_id,
                    method,
                    path_pattern: full_path,
                    segments,
                    config: config.clone(),
                    app_entry_point: entry_point.to_string(),
                });
            }
        }

        Self { routes }
    }

    /// Build a router from a set of view configs (flat, no namespacing — legacy).
    pub fn from_views(views: &HashMap<String, ApiViewConfig>) -> Self {
        let mut routes = Vec::new();

        for (id, config) in views {
            // X2.3: Warn at startup for views that declare allow_outbound_http
            if config.allow_outbound_http {
                let module = match &config.handler {
                    HandlerConfig::Codecomponent { module, .. } => module.as_str(),
                    _ => "<none>",
                };
                tracing::warn!(
                    target: "rivers.security",
                    view = %id,
                    module = %module,
                    "view declares allow_outbound_http — Rivers.http will be available in handler"
                );
            }

            // MessageConsumer views have no HTTP route
            if config.view_type == "MessageConsumer" {
                continue;
            }

            let path = match &config.path {
                Some(p) => p.clone(),
                None => continue,
            };

            let method = config
                .method
                .as_deref()
                .unwrap_or("GET")
                .to_uppercase();

            let segments = parse_path_pattern(&path);

            routes.push(ViewRoute {
                view_id: id.clone(),
                method,
                path_pattern: path,
                segments,
                config: config.clone(),
                app_entry_point: String::new(),
            });
        }

        Self { routes }
    }

    /// Match an incoming request to a view route.
    ///
    /// Returns the matched route and extracted path parameters.
    pub fn match_route(
        &self,
        method: &str,
        path: &str,
    ) -> Option<(&ViewRoute, HashMap<String, String>)> {
        let request_segments: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        for route in &self.routes {
            if route.method != method.to_uppercase() {
                continue;
            }

            if route.segments.len() != request_segments.len() {
                continue;
            }

            let mut params = HashMap::new();
            let mut matched = true;

            for (seg, req_seg) in route.segments.iter().zip(request_segments.iter()) {
                match seg {
                    PathSegment::Literal(lit) => {
                        if lit != req_seg {
                            matched = false;
                            break;
                        }
                    }
                    PathSegment::Param(name) => {
                        params.insert(name.clone(), req_seg.to_string());
                    }
                }
            }

            if matched {
                return Some((route, params));
            }
        }

        None
    }

    /// Get all registered routes.
    pub fn routes(&self) -> &[ViewRoute] {
        &self.routes
    }
}

/// Parse a path pattern like "/api/orders/{id}" into segments.
pub(crate) fn parse_path_pattern(pattern: &str) -> Vec<PathSegment> {
    pattern
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                PathSegment::Param(seg[1..seg.len() - 1].to_string())
            } else if let Some(stripped) = seg.strip_prefix(':') {
                // Also support :param syntax
                PathSegment::Param(stripped.to_string())
            } else {
                PathSegment::Literal(seg.to_string())
            }
        })
        .collect()
}
