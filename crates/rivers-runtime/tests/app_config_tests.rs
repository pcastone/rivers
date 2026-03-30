//! Integration tests for AppConfig parsing and validation.

use rivers_runtime::{validate_app_config, AppConfig};

// ── App config validation ───────────────────────────────────────────

#[test]
fn validate_app_catches_unknown_datasource_ref() {
    let toml = r#"
[data.dataviews.my_view]
name = "my_view"
datasource = "nonexistent"
query = "SELECT 1"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let errors = validate_app_config(&config).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("nonexistent")));
}

#[test]
fn validate_app_valid_config_passes() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[data.dataviews.my_view]
name = "my_view"
datasource = "mydb"
query = "SELECT * FROM users"

[api.views.list_users]
view_type = "Rest"
path = "/api/users"
method = "GET"

[api.views.list_users.handler]
type = "dataview"
dataview = "my_view"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    assert!(validate_app_config(&config).is_ok());
}

#[test]
fn validate_app_catches_unknown_dataview_in_view() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[api.views.list_users]
view_type = "Rest"
path = "/api/users"
method = "GET"

[api.views.list_users.handler]
type = "dataview"
dataview = "nonexistent_view"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let errors = validate_app_config(&config).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("nonexistent_view")));
}

// ── (C) View type validation ─────────────────────────────────────────

#[test]
fn validate_app_catches_unknown_view_type() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[api.views.bad_view]
view_type = "BadType"
path = "/api/bad"
method = "GET"

[api.views.bad_view.handler]
type = "codecomponent"
language = "javascript"
module = "handler.js"
entrypoint = "handle"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let errors = validate_app_config(&config).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("unknown view_type")));
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("BadType")));
}

#[test]
fn validate_app_accepts_all_known_view_types() {
    for vt in &["Rest", "Websocket", "ServerSentEvents", "MessageConsumer"] {
        let toml = format!(
            r#"
[api.views.v]
view_type = "{}"
path = "/api/v"
method = "GET"

[api.views.v.handler]
type = "codecomponent"
language = "javascript"
module = "handler.js"
entrypoint = "handle"
"#,
            vt
        );
        let config: AppConfig = toml::from_str(&toml).unwrap();
        assert!(
            validate_app_config(&config).is_ok(),
            "view_type '{}' should be valid",
            vt
        );
    }
}

// ── (E) Invalidates target validation ────────────────────────────────

#[test]
fn validate_app_catches_invalid_invalidates_target() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[data.dataviews.create_contact]
name = "create_contact"
datasource = "mydb"
post_query = "INSERT INTO contacts"
invalidates = ["list_contacts_typo"]
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let errors = validate_app_config(&config).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("invalidates target")));
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("list_contacts_typo")));
}

#[test]
fn validate_app_allows_valid_invalidates_target() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[data.dataviews.list_contacts]
name = "list_contacts"
datasource = "mydb"
query = "SELECT * FROM contacts"

[data.dataviews.create_contact]
name = "create_contact"
datasource = "mydb"
post_query = "INSERT INTO contacts"
invalidates = ["list_contacts"]
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    assert!(validate_app_config(&config).is_ok());
}
