//! MCP server-level configuration (`[mcp]` section in `riversd.toml`).
//!
//! Per `2026-04-29-cb-p1-1-mcp-subscriptions-design.md` §Layer 4.

use serde::Deserialize;
use schemars::JsonSchema;

/// `[mcp]` -- server-level MCP subscription limits.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct McpConfig {
    /// Maximum number of resource subscriptions a single session may hold.
    /// Requests beyond this cap return JSON-RPC error -32000.
    /// Default: 100.
    #[serde(default = "default_max_subscriptions_per_session")]
    pub max_subscriptions_per_session: u64,

    /// Minimum allowed `poll_interval_seconds` for any subscribable resource.
    /// Clamps per-resource poll intervals from below to prevent hostile bundles
    /// from spinning the DataView executor. Default: 1.
    #[serde(default = "default_min_poll_interval_seconds")]
    pub min_poll_interval_seconds: u64,
}

fn default_max_subscriptions_per_session() -> u64 { 100 }
fn default_min_poll_interval_seconds() -> u64 { 1 }

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            max_subscriptions_per_session: default_max_subscriptions_per_session(),
            min_poll_interval_seconds: default_min_poll_interval_seconds(),
        }
    }
}
