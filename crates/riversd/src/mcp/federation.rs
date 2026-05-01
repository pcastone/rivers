//! MCP Federation client — proxies tools/resources from upstream MCP servers.
//!
//! P2.3: Multi-Bundle MCP Federation. Each `McpFederationConfig` entry declares
//! a remote MCP upstream whose tools and resources are merged into the local
//! `tools/list` and `resources/list`, namespaced with the configured alias.
//!
//! Tool name namespacing:   `{alias}__{upstream_tool_name}`
//! Resource URI namespacing: `{alias}://{upstream_uri}`

use std::time::Duration;

use rivers_runtime::view::McpFederationConfig;

/// HTTP client wrapper for a single federated MCP upstream.
pub struct FederationClient {
    config: McpFederationConfig,
    client: reqwest::Client,
}

impl FederationClient {
    /// Create a new `FederationClient` for the given federation config.
    pub fn new(config: McpFederationConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .unwrap_or_default();
        Self { config, client }
    }

    /// Send a JSON-RPC 2.0 request to the upstream and return the `result` field.
    ///
    /// Returns `Err` if the request fails, the response is not valid JSON-RPC, or
    /// the upstream returned a JSON-RPC `error` object.
    async fn send(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let mut req = self.client.post(&self.config.url).json(&body);
        if let Some(token) = &self.config.bearer_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await.map_err(|e| e.to_string())?;
        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

        if let Some(result) = json.get("result") {
            Ok(result.clone())
        } else if let Some(err) = json.get("error") {
            Err(err.to_string())
        } else {
            Err("invalid JSON-RPC response: missing 'result' and 'error'".to_string())
        }
    }

    /// Fetch tools from the upstream and return them with namespaced names.
    ///
    /// Tool names are prefixed with `{alias}__`. If `tools_filter` is non-empty,
    /// only tools whose upstream name appears in the filter are included.
    /// Returns an empty vec on upstream error (federation is best-effort).
    pub async fn fetch_tools(&self) -> Vec<serde_json::Value> {
        match self.send("tools/list", serde_json::json!({})).await {
            Ok(result) => {
                let tools = result
                    .get("tools")
                    .and_then(|t| t.as_array())
                    .cloned()
                    .unwrap_or_default();

                tools
                    .into_iter()
                    .filter(|t| {
                        let name = t.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        self.config.tools_filter.is_empty()
                            || self.config.tools_filter.iter().any(|f| f == name)
                    })
                    .map(|mut t| {
                        if let Some(name) = t
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(|n| n.to_string())
                        {
                            t["name"] = serde_json::Value::String(
                                format!("{}__{}", self.config.alias, name),
                            );
                        }
                        t
                    })
                    .collect()
            }
            Err(e) => {
                tracing::warn!(
                    alias = %self.config.alias,
                    url = %self.config.url,
                    error = %e,
                    "MCP federation: tools/list fetch failed"
                );
                vec![]
            }
        }
    }

    /// Fetch resources from the upstream and return them with namespaced URIs.
    ///
    /// URIs are prefixed with `{alias}://`. If `resources_filter` is non-empty,
    /// only resources whose upstream URI starts with one of the filter prefixes are included.
    /// Returns an empty vec on upstream error (federation is best-effort).
    pub async fn fetch_resources(&self) -> Vec<serde_json::Value> {
        match self.send("resources/list", serde_json::json!({})).await {
            Ok(result) => {
                let resources = result
                    .get("resources")
                    .and_then(|r| r.as_array())
                    .cloned()
                    .unwrap_or_default();

                resources
                    .into_iter()
                    .filter(|r| {
                        let uri = r.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                        self.config.resources_filter.is_empty()
                            || self
                                .config
                                .resources_filter
                                .iter()
                                .any(|f| uri.starts_with(f.as_str()))
                    })
                    .map(|mut r| {
                        if let Some(uri) = r
                            .get("uri")
                            .and_then(|u| u.as_str())
                            .map(|u| u.to_string())
                        {
                            r["uri"] = serde_json::Value::String(
                                format!("{}://{}", self.config.alias, uri),
                            );
                        }
                        r
                    })
                    .collect()
            }
            Err(e) => {
                tracing::warn!(
                    alias = %self.config.alias,
                    url = %self.config.url,
                    error = %e,
                    "MCP federation: resources/list fetch failed"
                );
                vec![]
            }
        }
    }

    /// Proxy a `tools/call` for a namespaced tool to the upstream.
    ///
    /// Strips the `{alias}__` prefix from `local_tool_name` before forwarding.
    /// Returns a MCP tool result value on success or an error content block on failure.
    pub async fn proxy_tool_call(
        &self,
        local_tool_name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let prefix = format!("{}__{}", self.config.alias, "");
        let upstream_name = local_tool_name
            .strip_prefix(&prefix)
            .unwrap_or(local_tool_name);

        match self
            .send(
                "tools/call",
                serde_json::json!({"name": upstream_name, "arguments": arguments}),
            )
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!(
                    alias = %self.config.alias,
                    tool = %local_tool_name,
                    error = %e,
                    "MCP federation: tools/call proxy failed"
                );
                serde_json::json!({
                    "content": [{"type": "text", "text": format!("federation upstream error ({}): {}", self.config.alias, e)}],
                    "isError": true,
                })
            }
        }
    }

    /// Proxy a `resources/read` for a namespaced URI to the upstream.
    ///
    /// Strips the `{alias}://` prefix from `namespaced_uri` before forwarding.
    /// Returns the upstream result on success or an error object on failure.
    pub async fn proxy_resource_read(&self, namespaced_uri: &str) -> serde_json::Value {
        let prefix = format!("{}://", self.config.alias);
        let upstream_uri = namespaced_uri
            .strip_prefix(&prefix)
            .unwrap_or(namespaced_uri);

        match self
            .send(
                "resources/read",
                serde_json::json!({"uri": upstream_uri}),
            )
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!(
                    alias = %self.config.alias,
                    uri = %namespaced_uri,
                    error = %e,
                    "MCP federation: resources/read proxy failed"
                );
                serde_json::json!({
                    "error": format!("federation upstream error ({}): {}", self.config.alias, e),
                })
            }
        }
    }

    /// Return `true` if `tool_name` belongs to this federation upstream (i.e. carries
    /// the `{alias}__` namespace prefix).
    pub fn owns_tool(&self, tool_name: &str) -> bool {
        tool_name.starts_with(&format!("{}__{}", self.config.alias, ""))
    }

    /// Return `true` if `uri` belongs to this federation upstream (i.e. carries the
    /// `{alias}://` scheme prefix).
    pub fn owns_resource(&self, uri: &str) -> bool {
        uri.starts_with(&format!("{}://", self.config.alias))
    }

    /// Return the alias for this federation upstream.
    pub fn alias(&self) -> &str {
        &self.config.alias
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_client(alias: &str) -> FederationClient {
        let config = McpFederationConfig {
            alias: alias.into(),
            url: "http://localhost:9090/mcp/test".into(),
            bearer_token: None,
            tools_filter: vec![],
            resources_filter: vec![],
            timeout_ms: 5000,
        };
        FederationClient::new(config)
    }

    #[test]
    fn owns_tool_matches_alias_prefix() {
        let client = make_client("cb");
        assert!(client.owns_tool("cb__search"));
        assert!(client.owns_tool("cb__get_contacts"));
        assert!(!client.owns_tool("local_tool"));
        assert!(!client.owns_tool("cb_search")); // missing double underscore
        assert!(!client.owns_tool("other__search")); // different alias
    }

    #[test]
    fn owns_resource_matches_alias_scheme() {
        let client = make_client("cb");
        assert!(client.owns_resource("cb://rivers://app/decisions"));
        assert!(client.owns_resource("cb://anything"));
        assert!(!client.owns_resource("rivers://app/local"));
        assert!(!client.owns_resource("other://resource"));
    }

    #[test]
    fn alias_accessor_returns_configured_alias() {
        let client = make_client("my_service");
        assert_eq!(client.alias(), "my_service");
    }

    #[test]
    fn owns_tool_empty_alias_does_not_match_arbitrary_tools() {
        // An alias of "" would match any tool starting with "__" — not a realistic config
        // (MCP-VAL-FED-1 rejects empty aliases at validation time), but verify the
        // owns_tool predicate is well-behaved regardless.
        let client = make_client("");
        assert!(client.owns_tool("__anything")); // prefix "__" is present
        assert!(!client.owns_tool("no_double_underscore"));
    }

    #[test]
    fn owns_resource_empty_alias_does_not_match_arbitrary_uris() {
        let client = make_client("");
        assert!(client.owns_resource("://anything")); // prefix "://" is present
        assert!(!client.owns_resource("no_scheme"));
    }
}
