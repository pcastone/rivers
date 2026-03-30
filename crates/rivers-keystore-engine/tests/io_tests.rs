//! I/O tests — create/load/save roundtrip, file permissions, error paths.

use rivers_keystore_engine::*;
use std::path::Path;

/// Generate a fresh Age keypair, returning (identity_str, recipient_str).
fn generate_age_keypair() -> (String, String) {
    use age::secrecy::ExposeSecret;
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();
    let identity_str = identity.to_string().expose_secret().to_string();
    let recipient_str = recipient.to_string();
    (identity_str, recipient_str)
}

#[test]
fn create_and_load_round_trip() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.akeystore");
    let (identity_str, recipient_str) = generate_age_keypair();

    // Create empty keystore
    AppKeystore::create(&path, &recipient_str).unwrap();
    assert!(path.exists());

    // Load it back
    let ks = AppKeystore::load(&path, &identity_str).unwrap();
    assert_eq!(ks.version, 1);
    assert!(ks.keys.is_empty());
}

#[test]
fn create_generate_save_load_round_trip() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.akeystore");
    let (identity_str, recipient_str) = generate_age_keypair();

    // Create, generate a key, save
    AppKeystore::create(&path, &recipient_str).unwrap();
    let mut ks = AppKeystore::load(&path, &identity_str).unwrap();
    ks.generate_key("credential-key", "aes-256").unwrap();
    ks.save(&path, &recipient_str).unwrap();

    // Load back and verify
    let ks2 = AppKeystore::load(&path, &identity_str).unwrap();
    assert_eq!(ks2.keys.len(), 1);
    assert_eq!(ks2.keys[0].name, "credential-key");
    assert_eq!(ks2.keys[0].key_type, "aes-256");
    assert_eq!(ks2.keys[0].current_version, 1);
    assert_eq!(ks2.keys[0].versions.len(), 1);
}

#[test]
fn load_with_wrong_identity_fails() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.akeystore");

    let (_identity_str, recipient_str) = generate_age_keypair();
    let (wrong_identity, _) = generate_age_keypair();

    AppKeystore::create(&path, &recipient_str).unwrap();

    let err = AppKeystore::load(&path, &wrong_identity).unwrap_err();
    assert!(matches!(err, AppKeystoreError::DecryptionFailed));
}

#[test]
fn load_missing_file_errors() {
    let err = AppKeystore::load(Path::new("/nonexistent/path.akeystore"), "fake-identity")
        .unwrap_err();
    assert!(matches!(err, AppKeystoreError::KeystoreNotFound { .. }));
}

#[test]
fn full_lifecycle_with_persistence() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("lifecycle.akeystore");
    let (identity_str, recipient_str) = generate_age_keypair();

    // Create
    AppKeystore::create(&path, &recipient_str).unwrap();

    // Generate keys
    let mut ks = AppKeystore::load(&path, &identity_str).unwrap();
    ks.generate_key("primary", "aes-256").unwrap();
    ks.generate_key("secondary", "aes-256").unwrap();
    ks.save(&path, &recipient_str).unwrap();

    // Rotate primary
    let mut ks = AppKeystore::load(&path, &identity_str).unwrap();
    let v = ks.rotate_key("primary").unwrap();
    assert_eq!(v, 2);
    ks.save(&path, &recipient_str).unwrap();

    // Delete secondary
    let mut ks = AppKeystore::load(&path, &identity_str).unwrap();
    ks.delete_key("secondary").unwrap();
    ks.save(&path, &recipient_str).unwrap();

    // Final verification
    let ks = AppKeystore::load(&path, &identity_str).unwrap();
    assert_eq!(ks.keys.len(), 1);
    assert_eq!(ks.keys[0].name, "primary");
    assert_eq!(ks.keys[0].current_version, 2);
    assert_eq!(ks.keys[0].versions.len(), 2);

    // Both versions accessible
    let v1 = ks.versioned_key_bytes("primary", 1).unwrap();
    let v2 = ks.versioned_key_bytes("primary", 2).unwrap();
    assert_eq!(v1.len(), 32);
    assert_eq!(v2.len(), 32);
    assert_ne!(v1, v2);
}

#[cfg(unix)]
#[test]
fn file_permissions_are_0600() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("perms.akeystore");
    let (_, recipient_str) = generate_age_keypair();

    AppKeystore::create(&path, &recipient_str).unwrap();

    let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
    assert_eq!(mode, 0o600, "keystore should have 0600 permissions");
}
