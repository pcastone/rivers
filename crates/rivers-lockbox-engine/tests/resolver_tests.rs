//! Resolver module tests -- entry lookup, alias resolution, validation, metadata.

use rivers_lockbox_engine::*;

/// Helper: build a KeystoreEntry with sensible defaults.
fn make_entry(name: &str, value: &str, entry_type: &str, aliases: &[&str]) -> KeystoreEntry {
    KeystoreEntry {
        name: name.to_string(),
        value: value.to_string(),
        entry_type: entry_type.to_string(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        driver: None,
        username: None,
        hosts: vec![],
        database: None,
    }
}

// ── Entry Lookup by Name ─────────────────────────────────────────

#[test]
fn entry_lookup_by_name() {
    let entries = vec![
        make_entry("postgres/prod", "secret1", "string", &[]),
        make_entry("redis/prod", "secret2", "string", &[]),
        make_entry("kafka/prod", "secret3", "string", &[]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let meta = resolver.resolve("redis/prod").unwrap();
    assert_eq!(meta.name, "redis/prod");
    assert_eq!(meta.entry_index, 1);

    let meta = resolver.resolve("kafka/prod").unwrap();
    assert_eq!(meta.name, "kafka/prod");
    assert_eq!(meta.entry_index, 2);

    assert!(resolver.resolve("missing").is_none());
}

// ── Alias Resolution ─────────────────────────────────────────────

#[test]
fn alias_resolution() {
    let entries = vec![
        make_entry("postgres/orders-prod", "pg://secret", "string", &["db/orders", "orders-db"]),
        make_entry("redis/sessions", "redis://secret", "string", &["cache"]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    // Resolve via alias
    let meta = resolver.resolve("db/orders").unwrap();
    assert_eq!(meta.name, "postgres/orders-prod");
    assert_eq!(meta.entry_index, 0);

    let meta = resolver.resolve("orders-db").unwrap();
    assert_eq!(meta.name, "postgres/orders-prod");
    assert_eq!(meta.entry_index, 0);

    let meta = resolver.resolve("cache").unwrap();
    assert_eq!(meta.name, "redis/sessions");
    assert_eq!(meta.entry_index, 1);

    // Canonical name still works
    let meta = resolver.resolve("postgres/orders-prod").unwrap();
    assert_eq!(meta.name, "postgres/orders-prod");
}

// ── Duplicate Name Detected ──────────────────────────────────────

#[test]
fn duplicate_name_detected() {
    let entries = vec![
        make_entry("postgres/prod", "value1", "string", &[]),
        make_entry("postgres/prod", "value2", "string", &[]),
    ];
    let result = LockBoxResolver::from_entries(&entries);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::DuplicateEntry { name } => {
            assert_eq!(name, "postgres/prod");
        }
        other => panic!("expected DuplicateEntry, got: {:?}", other),
    }
}

// ── Duplicate Alias Detected ─────────────────────────────────────

#[test]
fn duplicate_alias_detected() {
    // Alias on second entry collides with name of first entry
    let entries = vec![
        make_entry("postgres/prod", "value1", "string", &[]),
        make_entry("redis/prod", "value2", "string", &["postgres/prod"]),
    ];
    let result = LockBoxResolver::from_entries(&entries);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::DuplicateEntry { name } => {
            assert_eq!(name, "postgres/prod");
        }
        other => panic!("expected DuplicateEntry, got: {:?}", other),
    }
}

#[test]
fn duplicate_alias_across_entries() {
    // Two entries share the same alias
    let entries = vec![
        make_entry("postgres/prod", "value1", "string", &["shared"]),
        make_entry("redis/prod", "value2", "string", &["shared"]),
    ];
    let result = LockBoxResolver::from_entries(&entries);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::DuplicateEntry { name } => {
            assert_eq!(name, "shared");
        }
        other => panic!("expected DuplicateEntry, got: {:?}", other),
    }
}

// ── Invalid Entry Name Rejected ──────────────────────────────────

#[test]
fn invalid_entry_name_rejected() {
    // Uppercase
    assert!(validate_entry_name("PostgresKey").is_err());
    // Starts with digit
    assert!(validate_entry_name("1password").is_err());
    // Starts with underscore
    assert!(validate_entry_name("_hidden").is_err());
    // Empty
    assert!(validate_entry_name("").is_err());
    // Too long (129 chars)
    assert!(validate_entry_name(&"a".repeat(129)).is_err());
    // Special characters
    assert!(validate_entry_name("key@host").is_err());
    assert!(validate_entry_name("key space").is_err());
    assert!(validate_entry_name("key=value").is_err());
    // Unicode
    assert!(validate_entry_name("key\u{00e9}").is_err());
}

#[test]
fn valid_entry_names_accepted() {
    assert!(validate_entry_name("a").is_ok());
    assert!(validate_entry_name("postgres/orders-prod").is_ok());
    assert!(validate_entry_name("db/orders").is_ok());
    assert!(validate_entry_name("anthropic/api_key").is_ok());
    assert!(validate_entry_name("jwt_signing_key").is_ok());
    assert!(validate_entry_name("my.service.key").is_ok());
    assert!(validate_entry_name("key-with-dashes").is_ok());
    // Max length (128 chars)
    assert!(validate_entry_name(&"a".repeat(128)).is_ok());
}

#[test]
fn invalid_entry_name_in_resolver() {
    let entries = vec![make_entry("INVALID", "value", "string", &[])];
    let result = LockBoxResolver::from_entries(&entries);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::InvalidEntryName { name } => {
            assert_eq!(name, "INVALID");
        }
        other => panic!("expected InvalidEntryName, got: {:?}", other),
    }
}

#[test]
fn invalid_alias_name_in_resolver() {
    let entries = vec![make_entry("valid-name", "value", "string", &["BAD_ALIAS"])];
    let result = LockBoxResolver::from_entries(&entries);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::InvalidEntryName { name } => {
            assert_eq!(name, "BAD_ALIAS");
        }
        other => panic!("expected InvalidEntryName, got: {:?}", other),
    }
}

// ── Resolver Metadata Only ───────────────────────────────────────

#[test]
fn resolver_metadata_only() {
    let entries = vec![
        make_entry("postgres/prod", "super-secret-pg-password", "string", &["pg"]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let meta = resolver.resolve("postgres/prod").unwrap();

    // EntryMetadata has name, entry_type, entry_index -- but NO value field.
    // This is enforced at compile time by the struct definition.
    assert_eq!(meta.name, "postgres/prod");
    assert_eq!(meta.entry_type, EntryType::String);
    assert_eq!(meta.entry_index, 0);

    // Alias resolves to same metadata
    let alias_meta = resolver.resolve("pg").unwrap();
    assert_eq!(alias_meta.name, "postgres/prod");
    assert_eq!(alias_meta.entry_index, 0);

    // Key count = 1 name + 1 alias = 2
    assert_eq!(resolver.key_count(), 2);

    // entry_names() returns only canonical names
    assert_eq!(resolver.entry_names(), vec!["postgres/prod"]);
}

// ── Entry Types Stored Correctly ─────────────────────────────────

#[test]
fn entry_types_stored_correctly_in_resolver() {
    let entries = vec![
        make_entry("key-string", "val", "string", &[]),
        make_entry("key-b64", "val", "base64url", &[]),
        make_entry("key-pem", "val", "pem", &[]),
        make_entry("key-json", "val", "json", &[]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    assert_eq!(resolver.resolve("key-string").unwrap().entry_type, EntryType::String);
    assert_eq!(resolver.resolve("key-b64").unwrap().entry_type, EntryType::Base64Url);
    assert_eq!(resolver.resolve("key-pem").unwrap().entry_type, EntryType::Pem);
    assert_eq!(resolver.resolve("key-json").unwrap().entry_type, EntryType::Json);
}

// ── Credential Record Metadata ───────────────────────────────────

#[test]
fn credential_record_metadata() {
    let mut entry = make_entry("postgres/prod", "secret", "string", &[]);
    entry.driver = Some("postgres".to_string());
    entry.username = Some("app_user".to_string());
    entry.hosts = vec!["db.example.com:5432".to_string()];
    entry.database = Some("orders".to_string());

    let entries = vec![entry];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let meta = resolver.resolve("postgres/prod").unwrap();
    assert!(meta.is_credential_record());
    assert_eq!(meta.driver, Some("postgres".to_string()));
    assert_eq!(meta.username, Some("app_user".to_string()));
    assert_eq!(meta.hosts, vec!["db.example.com:5432"]);
    assert_eq!(meta.database, Some("orders".to_string()));
}

#[test]
fn non_credential_record_metadata() {
    let entries = vec![make_entry("api-key", "secret", "string", &[])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let meta = resolver.resolve("api-key").unwrap();
    assert!(!meta.is_credential_record());
    assert_eq!(meta.driver, None);
    assert_eq!(meta.username, None);
    assert!(meta.hosts.is_empty());
    assert_eq!(meta.database, None);
}

// ── Resolver contains() ──────────────────────────────────────────

#[test]
fn resolver_contains() {
    let entries = vec![make_entry("postgres/prod", "secret", "string", &["pg"])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    assert!(resolver.contains("postgres/prod"));
    assert!(resolver.contains("pg"));
    assert!(!resolver.contains("missing"));
}

// ── Unknown Entry Type Defaults to String ────────────────────────

#[test]
fn unknown_entry_type_defaults_to_string() {
    let entries = vec![make_entry("test", "value", "custom_type", &[])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();
    let meta = resolver.resolve("test").unwrap();
    // Unknown types default to String via unwrap_or
    assert_eq!(meta.entry_type, EntryType::String);
}

// ── URI Parsing ──────────────────────────────────────────────────

#[test]
fn uri_parsing() {
    assert_eq!(
        parse_lockbox_uri("lockbox://postgres/prod"),
        Some("postgres/prod".to_string())
    );
    assert_eq!(
        parse_lockbox_uri("lockbox://simple"),
        Some("simple".to_string())
    );
    assert_eq!(parse_lockbox_uri("lockbox://"), None);
    assert_eq!(parse_lockbox_uri("env://DB_PASSWORD"), None);
    assert_eq!(parse_lockbox_uri("plain-string"), None);

    assert!(is_lockbox_uri("lockbox://test"));
    assert!(!is_lockbox_uri("env://test"));
    assert!(!is_lockbox_uri(""));
}

// ── Empty Resolver ───────────────────────────────────────────────

#[test]
fn empty_resolver_key_count() {
    let resolver = LockBoxResolver::from_entries(&[]).unwrap();
    assert_eq!(resolver.key_count(), 0);
}

#[test]
fn empty_resolver_contains() {
    let resolver = LockBoxResolver::from_entries(&[]).unwrap();
    assert!(!resolver.contains("anything"));
    assert!(!resolver.contains(""));
    assert!(!resolver.contains("lockbox://test"));
}
