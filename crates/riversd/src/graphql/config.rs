//! GraphQL endpoint configuration.
//!
//! Per `rivers-view-layer-spec.md` §9.

use serde::Deserialize;

/// GraphQL endpoint configuration.
///
/// Per spec §9: configurable path, introspection toggle.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphqlConfig {
    /// Whether GraphQL is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Path for the GraphQL endpoint (default: "/graphql").
    #[serde(default = "default_graphql_path")]
    pub path: String,

    /// Allow introspection queries.
    #[serde(default = "default_introspection")]
    pub introspection: bool,

    /// Max query depth (default: 10).
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,

    /// Max query complexity (default: 1000).
    #[serde(default = "default_max_complexity")]
    pub max_complexity: usize,
}

fn default_graphql_path() -> String {
    "/graphql".to_string()
}

fn default_introspection() -> bool {
    true
}

fn default_max_depth() -> usize {
    10
}

fn default_max_complexity() -> usize {
    1000
}

impl Default for GraphqlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: default_graphql_path(),
            introspection: true,
            max_depth: 10,
            max_complexity: 1000,
        }
    }
}

impl From<&rivers_runtime::rivers_core::GraphqlServerConfig> for GraphqlConfig {
    fn from(server_cfg: &rivers_runtime::rivers_core::GraphqlServerConfig) -> Self {
        Self {
            enabled: server_cfg.enabled,
            path: server_cfg.path.clone(),
            introspection: server_cfg.introspection,
            max_depth: server_cfg.max_depth,
            max_complexity: server_cfg.max_complexity,
        }
    }
}
