//! LockBox tests — entry validation, alias resolution, URI parsing, duplicate detection.

use rivers_core::lockbox::{
    collect_lockbox_references, is_lockbox_uri, parse_lockbox_uri, resolve_all_references,
    validate_entry_name, EntryType, KeystoreEntry, LockBoxError, LockBoxResolver,
};

fn make_entry(name: &str, value: &str, aliases: &[&str]) -> KeystoreEntry {
    KeystoreEntry {
        name: name.to_string(),
        value: value.to_string(),
        entry_type: "string".to_string(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
    }
}

// ── Name Validation ─────────────────────────────────────────────────

#[test]
fn valid_entry_names() {
    assert!(validate_entry_name("postgres/orders-prod").is_ok());
    assert!(validate_entry_name("db/orders").is_ok());
    assert!(validate_entry_name("anthropic/api_key").is_ok());
    assert!(validate_entry_name("jwt_signing_key").is_ok());
    assert!(validate_entry_name("a").is_ok());
    assert!(validate_entry_name("my.service.key").is_ok());
    assert!(validate_entry_name("key-with-dashes").is_ok());
}

#[test]
fn invalid_entry_name_uppercase() {
    assert!(validate_entry_name("PostgresKey").is_err());
}

#[test]
fn invalid_entry_name_starts_with_digit() {
    assert!(validate_entry_name("1password").is_err());
}

#[test]
fn invalid_entry_name_starts_with_underscore() {
    assert!(validate_entry_name("_hidden").is_err());
}

#[test]
fn invalid_entry_name_empty() {
    assert!(validate_entry_name("").is_err());
}

#[test]
fn invalid_entry_name_too_long() {
    let name = "a".repeat(129);
    assert!(validate_entry_name(&name).is_err());
}

#[test]
fn valid_entry_name_max_length() {
    let name = "a".repeat(128);
    assert!(validate_entry_name(&name).is_ok());
}

#[test]
fn invalid_entry_name_special_chars() {
    assert!(validate_entry_name("key@host").is_err());
    assert!(validate_entry_name("key space").is_err());
    assert!(validate_entry_name("key=value").is_err());
}

// ── URI Parsing ─────────────────────────────────────────────────────

#[test]
fn parse_lockbox_uri_valid() {
    assert_eq!(
        parse_lockbox_uri("lockbox://postgres/orders-prod"),
        Some("postgres/orders-prod".to_string())
    );
}

#[test]
fn parse_lockbox_uri_alias() {
    assert_eq!(
        parse_lockbox_uri("lockbox://orders-db"),
        Some("orders-db".to_string())
    );
}

#[test]
fn parse_lockbox_uri_not_lockbox() {
    assert_eq!(parse_lockbox_uri("env://DB_PASSWORD"), None);
    assert_eq!(parse_lockbox_uri("plain-string"), None);
}

#[test]
fn parse_lockbox_uri_empty_name() {
    assert_eq!(parse_lockbox_uri("lockbox://"), None);
}

#[test]
fn is_lockbox_uri_works() {
    assert!(is_lockbox_uri("lockbox://test"));
    assert!(!is_lockbox_uri("env://test"));
}

// ── Resolver ────────────────────────────────────────────────────────

#[test]
fn resolver_lookup_by_name() {
    let entries = vec![make_entry("postgres/prod", "pg://secret", &["pg-prod"])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let metadata = resolver.resolve("postgres/prod").unwrap();
    assert_eq!(metadata.name, "postgres/prod");
    assert_eq!(metadata.entry_type, EntryType::String);
    assert_eq!(metadata.entry_index, 0);
}

#[test]
fn resolver_lookup_by_alias() {
    let entries = vec![make_entry("postgres/prod", "pg://secret", &["pg-prod"])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let metadata = resolver.resolve("pg-prod").unwrap();
    assert_eq!(metadata.name, "postgres/prod");
    assert_eq!(metadata.entry_index, 0);
}

#[test]
fn resolver_not_found() {
    let entries = vec![make_entry("postgres/prod", "pg://secret", &[])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    assert!(resolver.resolve("missing").is_none());
}

#[test]
fn resolver_key_count() {
    let entries = vec![
        make_entry("postgres/prod", "pg://secret", &["pg-prod", "db/orders"]),
        make_entry("redis/prod", "redis://secret", &["cache"]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    // 2 names + 3 aliases = 5 keys
    assert_eq!(resolver.key_count(), 5);
}

#[test]
fn resolver_entry_names() {
    let entries = vec![
        make_entry("postgres/prod", "pg://secret", &["pg-prod"]),
        make_entry("redis/prod", "redis://secret", &["cache"]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let names = resolver.entry_names();
    assert_eq!(names, vec!["postgres/prod", "redis/prod"]);
}

#[test]
fn resolver_stores_no_values_in_memory() {
    // Per SHAPE-5: resolver only stores metadata, not secret values
    let entries = vec![make_entry("postgres/prod", "pg://secret", &["pg-prod"])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let metadata = resolver.resolve("postgres/prod").unwrap();
    // EntryMetadata has no `value` field — this is enforced at compile time
    assert_eq!(metadata.entry_index, 0);
    assert_eq!(metadata.entry_type, EntryType::String);
}

#[test]
fn resolver_entry_indices_are_correct() {
    let entries = vec![
        make_entry("first", "value1", &[]),
        make_entry("second", "value2", &[]),
        make_entry("third", "value3", &[]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    assert_eq!(resolver.resolve("first").unwrap().entry_index, 0);
    assert_eq!(resolver.resolve("second").unwrap().entry_index, 1);
    assert_eq!(resolver.resolve("third").unwrap().entry_index, 2);
}

// ── Duplicate Detection ─────────────────────────────────────────────

#[test]
fn duplicate_entry_names() {
    let entries = vec![
        make_entry("postgres/prod", "value1", &[]),
        make_entry("postgres/prod", "value2", &[]),
    ];
    let result = LockBoxResolver::from_entries(&entries);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, LockBoxError::DuplicateEntry { .. }));
}

#[test]
fn alias_conflicts_with_name() {
    let entries = vec![
        make_entry("postgres/prod", "value1", &[]),
        make_entry("redis/prod", "value2", &["postgres/prod"]),
    ];
    let result = LockBoxResolver::from_entries(&entries);
    assert!(result.is_err());
}

#[test]
fn alias_conflicts_with_alias() {
    let entries = vec![
        make_entry("postgres/prod", "value1", &["shared-alias"]),
        make_entry("redis/prod", "value2", &["shared-alias"]),
    ];
    let result = LockBoxResolver::from_entries(&entries);
    assert!(result.is_err());
}

// ── Reference Collection ────────────────────────────────────────────

#[test]
fn collect_references_filters_lockbox_uris() {
    let datasources = vec![
        ("primary_db", "lockbox://postgres/prod"),
        ("cache", "lockbox://redis/prod"),
        ("contacts", "none"),
    ];
    let refs = collect_lockbox_references(&datasources);
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].datasource, "primary_db");
    assert_eq!(refs[0].name, "postgres/prod");
    assert_eq!(refs[1].datasource, "cache");
}

#[test]
fn collect_references_empty_when_no_lockbox() {
    let datasources = vec![("contacts", "none"), ("faker", "inline")];
    let refs = collect_lockbox_references(&datasources);
    assert!(refs.is_empty());
}

// ── Reference Resolution ────────────────────────────────────────────

#[test]
fn resolve_all_references_success() {
    let entries = vec![
        make_entry("postgres/prod", "pg://secret", &["pg-prod"]),
        make_entry("redis/prod", "redis://secret", &["cache"]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let datasources = vec![
        ("primary_db", "lockbox://postgres/prod"),
        ("cache", "lockbox://cache"),
    ];
    let refs = collect_lockbox_references(&datasources);
    let resolved = resolve_all_references(&resolver, &refs).unwrap();

    assert_eq!(resolved.len(), 2);
    // Per SHAPE-5: resolved metadata has no value, only index
    assert_eq!(resolved["primary_db"].name, "postgres/prod");
    assert_eq!(resolved["primary_db"].entry_index, 0);
    assert_eq!(resolved["cache"].name, "redis/prod");
    assert_eq!(resolved["cache"].entry_index, 1);
}

#[test]
fn resolve_all_references_fails_on_missing() {
    let entries = vec![make_entry("postgres/prod", "pg://secret", &[])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let datasources = vec![("primary_db", "lockbox://missing/key")];
    let refs = collect_lockbox_references(&datasources);
    let result = resolve_all_references(&resolver, &refs);

    assert!(result.is_err());
    let err = result.unwrap_err();
    match err {
        LockBoxError::EntryNotFound { uri, datasource } => {
            assert_eq!(uri, "lockbox://missing/key");
            assert_eq!(datasource, "primary_db");
        }
        _ => panic!("expected EntryNotFound"),
    }
}

// ── EntryType ───────────────────────────────────────────────────────

#[test]
fn entry_type_parsing() {
    assert_eq!(EntryType::parse("string"), Some(EntryType::String));
    assert_eq!(EntryType::parse("base64url"), Some(EntryType::Base64Url));
    assert_eq!(EntryType::parse("pem"), Some(EntryType::Pem));
    assert_eq!(EntryType::parse("json"), Some(EntryType::Json));
    assert_eq!(EntryType::parse("unknown"), None);
}

// ── Keystore TOML Parsing ───────────────────────────────────────────

#[test]
fn keystore_toml_parsing() {
    use rivers_core::lockbox::Keystore;

    let toml_str = r#"
version = 1

[[entries]]
name    = "postgres/orders-prod"
value   = "postgresql://user:password@host:5432/orders"
type    = "string"
aliases = ["db/orders", "orders-db"]
created = "2026-01-15T10:00:00Z"
updated = "2026-01-15T10:00:00Z"

[[entries]]
name    = "anthropic/api_key"
value   = "sk-ant-test"
type    = "string"
aliases = ["llm_key"]
created = "2026-01-15T10:00:00Z"
updated = "2026-01-15T10:00:00Z"
"#;

    let keystore: Keystore = toml::from_str(toml_str).unwrap();
    assert_eq!(keystore.version, 1);
    assert_eq!(keystore.entries.len(), 2);
    assert_eq!(keystore.entries[0].name, "postgres/orders-prod");
    assert_eq!(keystore.entries[0].aliases, vec!["db/orders", "orders-db"]);
    assert_eq!(keystore.entries[1].name, "anthropic/api_key");
    assert_eq!(keystore.entries[1].entry_type, "string");
}

// ── Encrypt / Decrypt Round Trip ────────────────────────────────────

#[test]
fn encrypt_decrypt_round_trip() {
    use rivers_core::lockbox::{decrypt_keystore, encrypt_keystore, Keystore};

    // Generate a fresh keypair for the test
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();

    let keystore = Keystore {
        version: 1,
        entries: vec![
            make_entry("postgres/prod", "pg://secret-value", &["pg-prod"]),
            make_entry("redis/cache", "redis://cache-pw", &[]),
        ],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.rkeystore");

    // Encrypt
    encrypt_keystore(&path, &recipient.to_string(), &keystore).unwrap();

    // Verify file exists
    assert!(path.exists());

    // Verify permissions are 0o600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    // Decrypt and verify round-trip
    use age::secrecy::ExposeSecret;
    let identity_str = identity.to_string();
    let decrypted = decrypt_keystore(&path, identity_str.expose_secret().trim()).unwrap();
    assert_eq!(decrypted.version, 1);
    assert_eq!(decrypted.entries.len(), 2);
    assert_eq!(decrypted.entries[0].name, "postgres/prod");
    assert_eq!(decrypted.entries[0].value, "pg://secret-value");
    assert_eq!(decrypted.entries[0].aliases, vec!["pg-prod"]);
    assert_eq!(decrypted.entries[1].name, "redis/cache");
    assert_eq!(decrypted.entries[1].value, "redis://cache-pw");
}

// ── Config Parsing ──────────────────────────────────────────────────

#[test]
fn lockbox_config_in_server_config() {
    let toml_str = r#"
[lockbox]
path       = "/etc/rivers/riversd.rkeystore"
key_source = "file"
key_file   = "/etc/rivers/lockbox.key"
"#;

    let config: rivers_core::ServerConfig = toml::from_str(toml_str).unwrap();
    let lockbox = config.lockbox.unwrap();
    assert_eq!(lockbox.path, Some("/etc/rivers/riversd.rkeystore".to_string()));
    assert_eq!(lockbox.key_source, "file");
    assert_eq!(lockbox.key_file, Some("/etc/rivers/lockbox.key".to_string()));
}

#[test]
fn lockbox_config_defaults() {
    let toml_str = r#"
[lockbox]
path = "/etc/rivers/riversd.rkeystore"
"#;

    let config: rivers_core::ServerConfig = toml::from_str(toml_str).unwrap();
    let lockbox = config.lockbox.unwrap();
    assert_eq!(lockbox.key_source, "env");
    assert_eq!(lockbox.key_env_var, "RIVERS_LOCKBOX_KEY");
}

#[test]
fn server_config_without_lockbox() {
    let toml_str = r#"
[base]
port = 9090
"#;

    let config: rivers_core::ServerConfig = toml::from_str(toml_str).unwrap();
    assert!(config.lockbox.is_none());
}

#[test]
fn create_test_keystore_file() {
    use rivers_core::lockbox::{encrypt_keystore, decrypt_keystore, Keystore, KeystoreEntry};
    use std::path::PathBuf;

    // Use absolute paths — resolve from CARGO_MANIFEST_DIR or CWD
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()   // crates/
        .parent().unwrap()   // workspace root
        .join("test/lockbox");

    let identity_path = base.join("identity.key");
    if !identity_path.exists() {
        eprintln!("SKIP: {} not found", identity_path.display());
        return;
    }

    let now = chrono::Utc::now();
    let entries = ["postgres-test", "mysql-test", "redis-test", "mongo-test", "couchdb-test"];
    let keystore = Keystore {
        version: 1,
        entries: entries.iter().map(|name| KeystoreEntry {
            name: name.to_string(),
            value: "rivers_test".to_string(),
            entry_type: "string".to_string(),
            aliases: vec![],
            created: now,
            updated: now,
        }).collect(),
    };

    let identity_str = std::fs::read_to_string(&identity_path).unwrap();
    let identity: age::x25519::Identity = identity_str.trim().parse().unwrap();
    let recipient = identity.to_public().to_string();

    let keystore_path = base.join("keystore.age");
    encrypt_keystore(&keystore_path, &recipient, &keystore).unwrap();
    eprintln!("Created {}", keystore_path.display());

    // Verify roundtrip
    let decrypted = decrypt_keystore(&keystore_path, identity_str.trim()).unwrap();
    assert_eq!(decrypted.entries.len(), 5);
    assert_eq!(decrypted.entries[0].name, "postgres-test");
    assert_eq!(decrypted.entries[0].value, "rivers_test");
}
