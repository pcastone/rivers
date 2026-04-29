//! Validates riversd.toml for unknown keys at startup.

use toml::Value;

/// Known top-level keys in ServerConfig.
const SERVER_CONFIG_KEYS: &[&str] = &[
    "base", "bundle_path", "data_dir", "app_id", "route_prefix",
    "security", "static_files", "storage_engine", "lockbox",
    "runtime", "graphql", "engines", "plugins", "metrics",
    "environment_overrides",
];

/// Known keys in [base].
const BASE_CONFIG_KEYS: &[&str] = &[
    "host", "port", "workers", "request_timeout_seconds", "log_level",
    "backpressure", "http2", "admin_api", "tls", "cluster",
    "init_timeout_s", "logging",
];

/// Check for unknown keys in a parsed TOML value tree.
/// Returns a list of warning messages for unknown keys.
pub fn check_unknown_config_keys(toml_value: &Value) -> Vec<String> {
    let mut warnings = Vec::new();

    if let Some(table) = toml_value.as_table() {
        check_table(table, SERVER_CONFIG_KEYS, "root", &mut warnings);

        // Check [base] section
        if let Some(Value::Table(base)) = table.get("base") {
            check_table(base, BASE_CONFIG_KEYS, "base", &mut warnings);
        }
    }

    warnings
}

fn check_table(
    table: &toml::map::Map<String, Value>,
    known: &[&str],
    section: &str,
    warnings: &mut Vec<String>,
) {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            let mut msg = format!("unknown key '{}' in [{}]", key, section);
            if let Some(suggestion) = suggest_key(key, known) {
                msg = format!("{} — did you mean '{}'?", msg, suggestion);
            }
            warnings.push(msg);
        }
    }
}

fn suggest_key<'a>(unknown: &str, known: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(&str, usize)> = None;
    for &k in known {
        let dist = levenshtein(unknown, k);
        if dist <= 2 {
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((k, dist));
            }
        }
    }
    best.map(|(k, _)| k)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut matrix = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 0..=a.len() {
        matrix[i][0] = i;
    }
    for j in 0..=b.len() {
        matrix[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }
    matrix[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_warnings_for_valid_config() {
        let toml_str = r#"
            bundle_path = "/opt/rivers/bundles/my-app"
            [base]
            host = "0.0.0.0"
            port = 8080
        "#;
        let value: Value = toml::from_str(toml_str).unwrap();
        let warnings = check_unknown_config_keys(&value);
        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
    }

    #[test]
    fn warns_on_top_level_typo() {
        let toml_str = r#"
            bundel_path = "/opt/rivers/bundles/my-app"
        "#;
        let value: Value = toml::from_str(toml_str).unwrap();
        let warnings = check_unknown_config_keys(&value);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bundel_path"));
        assert!(warnings[0].contains("bundle_path"));
    }

    #[test]
    fn warns_on_base_section_typo() {
        let toml_str = r#"
            [base]
            hostt = "0.0.0.0"
            portt = 9999
        "#;
        let value: Value = toml::from_str(toml_str).unwrap();
        let warnings = check_unknown_config_keys(&value);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("hostt"));
        assert!(warnings[1].contains("portt"));
    }

    #[test]
    fn no_suggestion_for_distant_key() {
        let toml_str = r#"
            zzzzz = "value"
        "#;
        let value: Value = toml::from_str(toml_str).unwrap();
        let warnings = check_unknown_config_keys(&value);
        assert_eq!(warnings.len(), 1);
        assert!(!warnings[0].contains("did you mean"));
    }
}
