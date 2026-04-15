//! MCP instructions compiler — assembles static + auto-generated documentation.

use std::collections::HashMap;
use rivers_runtime::view::{McpToolConfig, McpResourceConfig, McpPromptConfig};
use rivers_runtime::dataview::DataViewParameterConfig;

/// Compile the instructions document from static file + auto-generated catalog.
pub fn compile_instructions(
    static_file: Option<&str>,
    app_dir: &std::path::Path,
    tools: &HashMap<String, McpToolConfig>,
    resources: &HashMap<String, McpResourceConfig>,
    prompts: &HashMap<String, McpPromptConfig>,
    get_params: &dyn Fn(&str, &str) -> Vec<DataViewParameterConfig>,
) -> String {
    let mut doc = String::new();

    // Source 1: Static instructions file (if declared)
    if let Some(path) = static_file {
        let full_path = app_dir.join(path);
        if let Ok(content) = std::fs::read_to_string(&full_path) {
            doc.push_str(&content);
            doc.push_str("\n\n---\n\n");
        }
    }

    // Source 2: Auto-generated tool catalog
    if !tools.is_empty() {
        doc.push_str("## Tool Reference\n\n");
        let mut tool_names: Vec<&String> = tools.keys().collect();
        tool_names.sort();
        for name in tool_names {
            let config = &tools[name];
            doc.push_str(&format!("### `{}`\n\n", name));
            if !config.description.is_empty() {
                doc.push_str(&format!("{}\n\n", config.description));
            }

            let method = config.method.as_deref().unwrap_or("GET");
            let params = get_params(&config.dataview, method);
            if !params.is_empty() {
                doc.push_str("**Parameters:**\n\n");
                doc.push_str("| Name | Type | Required | Default |\n");
                doc.push_str("|------|------|----------|---------|\n");
                for p in &params {
                    let default = p.default.as_ref()
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "—".into());
                    doc.push_str(&format!(
                        "| `{}` | {} | {} | {} |\n",
                        p.name, p.param_type,
                        if p.required { "yes" } else { "no" },
                        default,
                    ));
                }
                doc.push('\n');
            }

            // Hints
            let mut hints = Vec::new();
            if config.hints.read_only { hints.push("read-only"); }
            if config.hints.destructive { hints.push("destructive"); }
            if config.hints.idempotent { hints.push("idempotent"); }
            if !hints.is_empty() {
                doc.push_str(&format!("*Hints: {}*\n\n", hints.join(", ")));
            }
        }
    }

    // Source 3: Auto-generated resource reference
    if !resources.is_empty() {
        doc.push_str("## Resource Reference\n\n");
        let mut resource_names: Vec<&String> = resources.keys().collect();
        resource_names.sort();
        for name in resource_names {
            let config = &resources[name];
            doc.push_str(&format!("### `{}`\n\n", name));
            if !config.description.is_empty() {
                doc.push_str(&format!("{}\n\n", config.description));
            }
            doc.push_str(&format!("- MIME type: `{}`\n\n", config.mime_type));
        }
    }

    // Source 4: Auto-generated prompt reference
    if !prompts.is_empty() {
        doc.push_str("## Prompt Reference\n\n");
        let mut prompt_names: Vec<&String> = prompts.keys().collect();
        prompt_names.sort();
        for name in prompt_names {
            let config = &prompts[name];
            doc.push_str(&format!("### `{}`\n\n", name));
            if !config.description.is_empty() {
                doc.push_str(&format!("{}\n\n", config.description));
            }
            if !config.arguments.is_empty() {
                doc.push_str("**Arguments:**\n\n");
                for arg in &config.arguments {
                    let req = if arg.required { " (required)" } else { "" };
                    let def = arg.default.as_ref()
                        .map(|d| format!(" [default: {}]", d))
                        .unwrap_or_default();
                    doc.push_str(&format!("- `{}`{}{}\n", arg.name, req, def));
                }
                doc.push('\n');
            }
        }
    }

    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::view::McpToolHints;

    #[test]
    fn empty_instructions() {
        let tools = HashMap::new();
        let resources = HashMap::new();
        let prompts = HashMap::new();
        let doc = compile_instructions(
            None, std::path::Path::new("."),
            &tools, &resources, &prompts,
            &|_, _| vec![],
        );
        assert!(doc.is_empty());
    }

    #[test]
    fn tool_catalog_generated() {
        let mut tools = HashMap::new();
        tools.insert("search".into(), McpToolConfig {
            dataview: "search_dv".into(),
            description: "Search records".into(),
            method: None,
            hints: McpToolHints::default(),
        });
        let resources = HashMap::new();
        let prompts = HashMap::new();
        let doc = compile_instructions(
            None, std::path::Path::new("."),
            &tools, &resources, &prompts,
            &|_, _| vec![],
        );
        assert!(doc.contains("## Tool Reference"));
        assert!(doc.contains("### `search`"));
        assert!(doc.contains("Search records"));
    }
}
