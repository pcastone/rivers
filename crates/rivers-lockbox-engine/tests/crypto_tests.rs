//! Crypto module tests -- encrypt/decrypt round trips, error handling, edge cases.

use std::path::Path;

use age::secrecy::ExposeSecret;
use rivers_lockbox_engine::*;
use zeroize::Zeroize;

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

// ── Encrypt/Decrypt Round Trip ───────────────────────────────────

#[test]
fn create_and_load_keystore_round_trip() {
    let (identity_str, recipient_str) = generate_keypair();

    let keystore = Keystore {
        version: 1,
        entries: vec![
            make_entry("postgres/prod", "pg://user:pass@host/db", "string", &["pg-prod"]),
            make_entry("redis/cache", "redis://secret", "string", &[]),
            make_entry("jwt-signing", "base64-encoded-key-data", "base64url", &["jwt"]),
        ],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.rkeystore");

    // Encrypt to disk
    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();
    assert!(path.exists());

    // Decrypt from disk
    let decrypted = decrypt_keystore(&path, identity_str.trim()).unwrap();

    // Verify round-trip fidelity
    assert_eq!(decrypted.version, 1);
    assert_eq!(decrypted.entries.len(), 3);
    assert_eq!(decrypted.entries[0].name, "postgres/prod");
    assert_eq!(decrypted.entries[0].value, "pg://user:pass@host/db");
    assert_eq!(decrypted.entries[0].aliases, vec!["pg-prod"]);
    assert_eq!(decrypted.entries[1].name, "redis/cache");
    assert_eq!(decrypted.entries[1].value, "redis://secret");
    assert_eq!(decrypted.entries[2].name, "jwt-signing");
    assert_eq!(decrypted.entries[2].value, "base64-encoded-key-data");
    assert_eq!(decrypted.entries[2].entry_type, "base64url");
}

// ── Wrong Key Decryption Fails ───────────────────────────────────

#[test]
fn wrong_key_decryption_fails() {
    let (_, recipient_str) = generate_keypair();
    let (wrong_identity_str, _) = generate_keypair();

    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("secret", "top-secret-value", "string", &[])],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.rkeystore");

    // Encrypt with first keypair
    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

    // Attempt to decrypt with second (wrong) keypair
    let result = decrypt_keystore(&path, wrong_identity_str.trim());
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::DecryptionFailed => {} // expected
        other => panic!("expected DecryptionFailed, got: {:?}", other),
    }
}

// ── Entry Type Validation ────────────────────────────────────────

#[test]
fn entry_type_validation() {
    assert_eq!(EntryType::parse("string"), Some(EntryType::String));
    assert_eq!(EntryType::parse("base64url"), Some(EntryType::Base64Url));
    assert_eq!(EntryType::parse("pem"), Some(EntryType::Pem));
    assert_eq!(EntryType::parse("json"), Some(EntryType::Json));
    assert_eq!(EntryType::parse("unknown"), None);
    assert_eq!(EntryType::parse(""), None);
    assert_eq!(EntryType::parse("String"), None); // case-sensitive
}

// ── File Permissions Enforced ────────────────────────────────────

#[cfg(unix)]
#[test]
fn file_permissions_enforced() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.rkeystore");
    std::fs::write(&path, b"dummy").unwrap();

    // Set insecure permissions (0o644)
    let perms = std::fs::Permissions::from_mode(0o644);
    std::fs::set_permissions(&path, perms).unwrap();

    let result = check_file_permissions(&path);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::InsecureFilePermissions { mode, .. } => {
            assert_eq!(mode, 0o644);
        }
        other => panic!("expected InsecureFilePermissions, got: {:?}", other),
    }

    // Fix permissions to 0o600 -- should pass
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(&path, perms).unwrap();
    assert!(check_file_permissions(&path).is_ok());
}

#[cfg(unix)]
#[test]
fn encrypt_keystore_sets_permissions() {
    use std::os::unix::fs::MetadataExt;

    let (_, recipient_str) = generate_keypair();
    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("test", "value", "string", &[])],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.rkeystore");

    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

    let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
    assert_eq!(mode, 0o600, "encrypt_keystore must set 0o600 permissions");
}

// ── fetch_secret_value Round Trip ────────────────────────────────

#[test]
fn fetch_secret_value_round_trip() {
    let (identity_str, recipient_str) = generate_keypair();

    let keystore = Keystore {
        version: 1,
        entries: vec![
            make_entry("first", "value-one", "string", &[]),
            make_entry("second", "value-two", "base64url", &["alias-second"]),
            make_entry("third", "value-three", "pem", &[]),
        ],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.rkeystore");
    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

    let resolver = LockBoxResolver::from_entries(&keystore.entries).unwrap();

    // Fetch first entry by name
    let meta = resolver.resolve("first").unwrap();
    let resolved = fetch_secret_value(meta, &path, identity_str.trim()).unwrap();
    assert_eq!(resolved.name, "first");
    assert_eq!(resolved.value.expose_secret().as_str(), "value-one");
    assert_eq!(resolved.entry_type, EntryType::String);

    // Fetch second entry by alias
    let meta = resolver.resolve("alias-second").unwrap();
    let resolved = fetch_secret_value(meta, &path, identity_str.trim()).unwrap();
    assert_eq!(resolved.name, "second");
    assert_eq!(resolved.value.expose_secret().as_str(), "value-two");
    assert_eq!(resolved.entry_type, EntryType::Base64Url);

    // Fetch third entry by name
    let meta = resolver.resolve("third").unwrap();
    let resolved = fetch_secret_value(meta, &path, identity_str.trim()).unwrap();
    assert_eq!(resolved.name, "third");
    assert_eq!(resolved.value.expose_secret().as_str(), "value-three");
    assert_eq!(resolved.entry_type, EntryType::Pem);
}

#[test]
fn fetch_secret_value_zeroize_after_use() {
    let (identity_str, recipient_str) = generate_keypair();

    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("api-key", "super-secret", "string", &[])],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zeroize.rkeystore");
    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

    let resolver = LockBoxResolver::from_entries(&keystore.entries).unwrap();
    let meta = resolver.resolve("api-key").unwrap();
    let mut resolved = fetch_secret_value(meta, &path, identity_str.trim()).unwrap();

    assert_eq!(resolved.value.expose_secret().as_str(), "super-secret");
    resolved.value.zeroize();
    assert_eq!(resolved.value.expose_secret().as_str(), "");
    assert_eq!(resolved.name, "api-key");
}

// ── Keystore TOML Serialization ──────────────────────────────────

#[test]
fn keystore_toml_round_trip() {
    let keystore = Keystore {
        version: 1,
        entries: vec![
            make_entry("postgres/prod", "pg://user:pass@host/db", "string", &["pg"]),
            make_entry("api-key", "sk-test-12345", "string", &[]),
        ],
    };

    let toml_str = toml::to_string_pretty(&keystore).unwrap();
    let parsed: Keystore = toml::from_str(&toml_str).unwrap();

    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.entries.len(), 2);
    assert_eq!(parsed.entries[0].name, "postgres/prod");
    assert_eq!(parsed.entries[0].value, "pg://user:pass@host/db");
    assert_eq!(parsed.entries[0].aliases, vec!["pg"]);
    assert_eq!(parsed.entries[1].name, "api-key");
    assert_eq!(parsed.entries[1].value, "sk-test-12345");
}

// ── Keystore Not Found ───────────────────────────────────────────

#[test]
fn decrypt_keystore_not_found() {
    let (identity_str, _) = generate_keypair();
    let result = decrypt_keystore(Path::new("/nonexistent/path.rkeystore"), &identity_str);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::KeystoreNotFound { path } => {
            assert_eq!(path, "/nonexistent/path.rkeystore");
        }
        other => panic!("expected KeystoreNotFound, got: {:?}", other),
    }
}

// ── Keystore Default Version ─────────────────────────────────────

#[test]
fn keystore_default_version() {
    let toml_str = r#"
[[entries]]
name    = "test"
value   = "secret"
type    = "string"
created = "2026-01-01T00:00:00Z"
updated = "2026-01-01T00:00:00Z"
"#;
    let keystore: Keystore = toml::from_str(toml_str).unwrap();
    assert_eq!(keystore.version, 1); // default_version()
    assert_eq!(keystore.entries.len(), 1);
}

// ── Empty Keystore ───────────────────────────────────────────────

#[test]
fn empty_keystore_round_trip() {
    let (identity_str, recipient_str) = generate_keypair();

    let keystore = Keystore {
        version: 1,
        entries: vec![],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.rkeystore");

    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();
    let decrypted = decrypt_keystore(&path, identity_str.trim()).unwrap();

    assert_eq!(decrypted.version, 1);
    assert!(decrypted.entries.is_empty());

    // Resolver from empty entries
    let resolver = LockBoxResolver::from_entries(&decrypted.entries).unwrap();
    assert_eq!(resolver.key_count(), 0);
    assert!(resolver.entry_names().is_empty());
}

// ── Error Variant Coverage ───────────────────────────────────────

#[test]
fn error_malformed_keystore_invalid_toml() {
    let (identity_str, recipient_str) = generate_keypair();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_toml.rkeystore");

    // Encrypt something that is valid UTF-8 but not valid TOML for Keystore
    let garbage_toml = "this is not valid [[toml structure {{{";
    let recipient: age::x25519::Recipient = recipient_str.parse().unwrap();
    let encrypted = age::encrypt(&recipient, garbage_toml.as_bytes()).unwrap();
    std::fs::write(&path, &encrypted).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let result = decrypt_keystore(&path, identity_str.trim());
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::MalformedKeystore { reason } => {
            assert!(!reason.is_empty(), "reason should describe TOML parse error");
        }
        other => panic!("expected MalformedKeystore, got: {:?}", other),
    }
}

#[test]
fn error_malformed_keystore_invalid_utf8() {
    let (identity_str, recipient_str) = generate_keypair();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_utf8.rkeystore");

    // Encrypt non-UTF8 bytes
    let garbage_bytes: Vec<u8> = vec![0xFF, 0xFE, 0x80, 0x81, 0x00, 0xC0, 0xC1];
    let recipient: age::x25519::Recipient = recipient_str.parse().unwrap();
    let encrypted = age::encrypt(&recipient, &garbage_bytes).unwrap();
    std::fs::write(&path, &encrypted).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let result = decrypt_keystore(&path, identity_str.trim());
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::MalformedKeystore { reason } => {
            assert!(reason.contains("UTF-8"), "reason was: {}", reason);
        }
        other => panic!("expected MalformedKeystore, got: {:?}", other),
    }
}

// ── fetch_secret_value Edge Cases ────────────────────────────────

#[test]
fn fetch_secret_value_name_not_found_in_keystore() {
    // After RW1.4.c: fetch_secret_value now looks up by name, not entry_index.
    // A metadata entry whose name doesn't exist in the on-disk keystore must
    // return a MalformedKeystore error mentioning the entry name.
    let (identity_str, recipient_str) = generate_keypair();

    let keystore = Keystore {
        version: 1,
        entries: vec![
            make_entry("first", "value-one", "string", &[]),
            make_entry("second", "value-two", "string", &[]),
        ],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("name_not_found.rkeystore");
    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

    // Construct metadata for a name that is absent from the keystore.
    let bad_metadata = EntryMetadata {
        name: "phantom".to_string(),
        entry_type: EntryType::String,
        entry_index: 0, // irrelevant — name-based lookup ignores this
        driver: None,
        username: None,
        hosts: vec![],
        database: None,
    };

    let result = fetch_secret_value(&bad_metadata, &path, identity_str.trim());
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::MalformedKeystore { reason } => {
            assert!(
                reason.contains("phantom"),
                "error reason should mention the missing entry name; got: {}", reason
            );
        }
        other => panic!("expected MalformedKeystore (name not found), got: {:?}", other),
    }
}

#[test]
fn fetch_secret_value_with_alias() {
    let (identity_str, recipient_str) = generate_keypair();

    let keystore = Keystore {
        version: 1,
        entries: vec![
            make_entry("postgres/prod", "pg://secret-password", "string", &["pg-prod"]),
        ],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("alias_fetch.rkeystore");
    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

    let resolver = LockBoxResolver::from_entries(&keystore.entries).unwrap();

    // Resolve via alias, then fetch
    let meta = resolver.resolve("pg-prod").unwrap();
    let resolved = fetch_secret_value(meta, &path, identity_str.trim()).unwrap();
    assert_eq!(resolved.name, "postgres/prod");
    assert_eq!(resolved.value.expose_secret().as_str(), "pg://secret-password");
    assert_eq!(resolved.entry_type, EntryType::String);
}

// ── Encryption Edge Cases ────────────────────────────────────────

#[test]
fn encrypt_with_invalid_recipient() {
    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("test", "value", "string", &[])],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_recipient.rkeystore");

    let result = encrypt_keystore(&path, "not-a-valid-recipient-string", &keystore);
    assert!(result.is_err());
}

#[test]
fn decrypt_with_invalid_identity() {
    let (_, recipient_str) = generate_keypair();

    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("test", "value", "string", &[])],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_identity.rkeystore");
    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

    let result = decrypt_keystore(&path, "garbage-identity-string");
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::DecryptionFailed => {} // expected -- parse failure maps to DecryptionFailed
        other => panic!("expected DecryptionFailed, got: {:?}", other),
    }
}

#[test]
fn encrypt_decrypt_value_with_newlines() {
    let (identity_str, recipient_str) = generate_keypair();

    let multiline_value = "line1\nline2\nline3\n";
    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("multiline-key", multiline_value, "string", &[])],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("newlines.rkeystore");

    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();
    let decrypted = decrypt_keystore(&path, identity_str.trim()).unwrap();

    assert_eq!(decrypted.entries[0].value, multiline_value);
}

#[test]
fn encrypt_decrypt_value_with_unicode() {
    let (identity_str, recipient_str) = generate_keypair();

    let unicode_value = "secret-\u{1F512}-key-\u{2603}-value-\u{00E9}\u{00F1}";
    let keystore = Keystore {
        version: 1,
        entries: vec![make_entry("unicode-key", unicode_value, "string", &[])],
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unicode.rkeystore");

    encrypt_keystore(&path, &recipient_str, &keystore).unwrap();
    let decrypted = decrypt_keystore(&path, identity_str.trim()).unwrap();

    assert_eq!(decrypted.entries[0].value, unicode_value);
}
