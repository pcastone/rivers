//! Startup resolution tests -- full sequence, error paths, reference collection.

use age::secrecy::ExposeSecret;
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

/// Helper: generate an Age keypair, returning (identity_string, recipient_string).
fn generate_keypair() -> (String, String) {
    let identity = age::x25519::Identity::generate();
    let identity_str = identity.to_string().expose_secret().to_string();
    let recipient_str = identity.to_public().to_string();
    (identity_str, recipient_str)
}

// ── Lockbox Reference Collection ─────────────────────────────────

#[test]
fn collect_and_resolve_references() {
    let entries = vec![
        make_entry("postgres/prod", "pg://secret", "string", &["pg-prod"]),
        make_entry("redis/prod", "redis://secret", "string", &["cache"]),
    ];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let datasources = vec![
        ("primary_db", "lockbox://postgres/prod"),
        ("cache_store", "lockbox://cache"),
        ("contacts", "none"),
    ];
    let refs = collect_lockbox_references(&datasources);
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].name, "postgres/prod");
    assert_eq!(refs[0].datasource, "primary_db");
    assert_eq!(refs[1].name, "cache");
    assert_eq!(refs[1].datasource, "cache_store");

    let resolved = resolve_all_references(&resolver, &refs).unwrap();
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved["primary_db"].name, "postgres/prod");
    assert_eq!(resolved["primary_db"].entry_index, 0);
    assert_eq!(resolved["cache_store"].name, "redis/prod");
    assert_eq!(resolved["cache_store"].entry_index, 1);
}

#[test]
fn resolve_references_fails_on_missing_entry() {
    let entries = vec![make_entry("postgres/prod", "secret", "string", &[])];
    let resolver = LockBoxResolver::from_entries(&entries).unwrap();

    let datasources = vec![("db", "lockbox://missing")];
    let refs = collect_lockbox_references(&datasources);
    let result = resolve_all_references(&resolver, &refs);

    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::EntryNotFound { uri, datasource } => {
            assert_eq!(uri, "lockbox://missing");
            assert_eq!(datasource, "db");
        }
        other => panic!("expected EntryNotFound, got: {:?}", other),
    }
}

// ── Startup Resolve Integration ──────────────────────────────────

#[cfg(unix)]
#[test]
fn startup_resolve_complete_sequence() {
    let (identity_str, recipient_str) = generate_keypair();

    // Create keystore on disk
    let dir = tempfile::tempdir().unwrap();
    let keystore_path = dir.path().join("startup.rkeystore");

    let keystore = Keystore {
        version: 1,
        entries: vec![
            make_entry("postgres/prod", "pg://secret", "string", &["pg-prod"]),
            make_entry("redis/cache", "redis://secret", "string", &[]),
        ],
    };
    encrypt_keystore(&keystore_path, &recipient_str, &keystore).unwrap();

    // Set env var for key source
    let env_var = "TEST_LOCKBOX_KEY_STARTUP_3";
    unsafe { std::env::set_var(env_var, &identity_str); }

    let config = LockBoxConfig {
        path: Some(keystore_path.to_str().unwrap().to_string()),
        key_source: "env".to_string(),
        key_env_var: env_var.to_string(),
        ..Default::default()
    };

    let references = vec![
        LockBoxReference {
            uri: "lockbox://postgres/prod".to_string(),
            name: "postgres/prod".to_string(),
            datasource: "primary_db".to_string(),
        },
        LockBoxReference {
            uri: "lockbox://redis/cache".to_string(),
            name: "redis/cache".to_string(),
            datasource: "cache_store".to_string(),
        },
    ];

    let (resolver, resolved) = startup_resolve(&config, &references).unwrap();

    assert_eq!(resolver.key_count(), 3); // 2 names + 1 alias
    assert!(resolver.contains("postgres/prod"));
    assert!(resolver.contains("pg-prod"));
    assert!(resolver.contains("redis/cache"));
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved["primary_db"].name, "postgres/prod");
    assert_eq!(resolved["cache_store"].name, "redis/cache");
}

#[test]
fn startup_resolve_relative_path_rejected() {
    let env_var = "TEST_LOCKBOX_KEY_RELPATH_4";
    unsafe { std::env::set_var(env_var, "AGE-SECRET-KEY-DUMMY"); }

    let config = LockBoxConfig {
        path: Some("relative/path.rkeystore".to_string()),
        key_source: "env".to_string(),
        key_env_var: env_var.to_string(),
        ..Default::default()
    };

    let result = startup_resolve(&config, &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::MalformedKeystore { reason } => {
            assert!(reason.contains("absolute"), "reason was: {}", reason);
        }
        other => panic!("expected MalformedKeystore (relative path), got: {:?}", other),
    }
}

#[cfg(unix)]
#[test]
fn startup_resolve_file_not_found() {
    let env_var = "TEST_LOCKBOX_KEY_NOTFOUND_5";
    unsafe { std::env::set_var(env_var, "AGE-SECRET-KEY-DUMMY"); }

    let config = LockBoxConfig {
        path: Some("/tmp/nonexistent_lockbox_test.rkeystore".to_string()),
        key_source: "env".to_string(),
        key_env_var: env_var.to_string(),
        ..Default::default()
    };

    let result = startup_resolve(&config, &[]);
    assert!(result.is_err());
    // Could be KeystoreNotFound or InsecureFilePermissions depending on
    // whether check_file_permissions runs first (file doesn't exist).
    match result.unwrap_err() {
        LockBoxError::KeystoreNotFound { .. } => {} // expected
        other => panic!("expected KeystoreNotFound, got: {:?}", other),
    }
}

#[cfg(unix)]
#[test]
fn startup_resolve_insecure_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let keystore_path = dir.path().join("insecure.rkeystore");
    std::fs::write(&keystore_path, "not-used").unwrap();
    std::fs::set_permissions(&keystore_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let config = LockBoxConfig {
        path: Some(keystore_path.to_str().unwrap().to_string()),
        key_source: "env".to_string(),
        key_env_var: "TEST_LOCKBOX_KEY_INSECURE_PERMS".to_string(),
        ..Default::default()
    };

    let result = startup_resolve(&config, &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::InsecureFilePermissions { mode, .. } => {
            assert_eq!(mode, 0o644);
        }
        other => panic!("expected InsecureFilePermissions, got: {:?}", other),
    }
}

#[test]
fn error_config_missing() {
    let config = LockBoxConfig::default();

    let result = startup_resolve(&config, &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::ConfigMissing => {}
        other => panic!("expected ConfigMissing, got: {:?}", other),
    }
}

#[cfg(unix)]
#[test]
fn startup_resolve_wrong_key() {
    let (_, recipient_str) = generate_keypair();
    let (wrong_identity_str, _) = generate_keypair();

    // Create keystore encrypted with one key
    let dir = tempfile::tempdir().unwrap();
    let keystore_path = dir.path().join("wrongkey.rkeystore");

    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("secret", "value", "string", &[])],
    };
    encrypt_keystore(&keystore_path, &recipient_str, &keystore).unwrap();

    // Provide the wrong key via env
    let env_var = "TEST_LOCKBOX_KEY_WRONGKEY_6";
    unsafe { std::env::set_var(env_var, &wrong_identity_str); }

    let config = LockBoxConfig {
        path: Some(keystore_path.to_str().unwrap().to_string()),
        key_source: "env".to_string(),
        key_env_var: env_var.to_string(),
        ..Default::default()
    };

    let result = startup_resolve(&config, &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::DecryptionFailed => {} // expected
        other => panic!("expected DecryptionFailed, got: {:?}", other),
    }
}

#[cfg(unix)]
#[test]
fn startup_resolve_missing_reference() {
    let (identity_str, recipient_str) = generate_keypair();

    let dir = tempfile::tempdir().unwrap();
    let keystore_path = dir.path().join("missingref.rkeystore");

    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("postgres/prod", "pg://secret", "string", &[])],
    };
    encrypt_keystore(&keystore_path, &recipient_str, &keystore).unwrap();

    let env_var = "TEST_LOCKBOX_KEY_MISSINGREF_7";
    unsafe { std::env::set_var(env_var, &identity_str); }

    let config = LockBoxConfig {
        path: Some(keystore_path.to_str().unwrap().to_string()),
        key_source: "env".to_string(),
        key_env_var: env_var.to_string(),
        ..Default::default()
    };

    let references = vec![LockBoxReference {
        uri: "lockbox://nonexistent".to_string(),
        name: "nonexistent".to_string(),
        datasource: "some_ds".to_string(),
    }];

    let result = startup_resolve(&config, &references);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::EntryNotFound { uri, datasource } => {
            assert_eq!(uri, "lockbox://nonexistent");
            assert_eq!(datasource, "some_ds");
        }
        other => panic!("expected EntryNotFound, got: {:?}", other),
    }
}

// ── Reference Collection Edge Cases ──────────────────────────────

#[test]
fn collect_references_empty_datasources() {
    let refs = collect_lockbox_references(&[]);
    assert!(refs.is_empty());
}

#[test]
fn collect_references_no_lockbox_uris() {
    let datasources = vec![
        ("db", "env://DB_PASSWORD"),
        ("cache", "plain-connection-string"),
        ("broker", ""),
    ];
    let refs = collect_lockbox_references(&datasources);
    assert!(refs.is_empty());
}

#[test]
fn collect_references_mixed_uris() {
    let datasources = vec![
        ("db", "lockbox://postgres/prod"),
        ("cache", "env://REDIS_URL"),
        ("broker", "lockbox://kafka/cluster"),
        ("static", "hardcoded-value"),
    ];
    let refs = collect_lockbox_references(&datasources);
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].name, "postgres/prod");
    assert_eq!(refs[0].datasource, "db");
    assert_eq!(refs[1].name, "kafka/cluster");
    assert_eq!(refs[1].datasource, "broker");
}
