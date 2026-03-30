//! Key management tests — generate, rotate, delete, metadata, versioning.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rivers_keystore_engine::*;

#[test]
fn generate_key_validates_type_and_material_length() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };

    // Valid type
    let key = ks.generate_key("test-key", "aes-256").unwrap();
    assert_eq!(key.key_type, "aes-256");
    assert_eq!(key.current_version, 1);
    assert_eq!(key.versions.len(), 1);

    // Verify key material is 32 bytes when decoded
    let bytes = BASE64.decode(&key.versions[0].key_material).unwrap();
    assert_eq!(bytes.len(), 32);

    // Invalid type
    let err = ks.generate_key("bad-key", "aes-128").unwrap_err();
    assert!(
        matches!(err, AppKeystoreError::InvalidKeyType { expected, got }
            if expected == "aes-256" && got == "aes-128")
    );
}

#[test]
fn duplicate_key_name_errors() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };

    ks.generate_key("my-key", "aes-256").unwrap();
    let err = ks.generate_key("my-key", "aes-256").unwrap_err();
    assert!(matches!(err, AppKeystoreError::DuplicateKey { name } if name == "my-key"));
}

#[test]
fn rotate_key_increments_version() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    ks.generate_key("rotate-me", "aes-256").unwrap();

    // Get original bytes
    let v1_bytes = ks.current_key_bytes("rotate-me").unwrap();

    // Rotate
    let new_version = ks.rotate_key("rotate-me").unwrap();
    assert_eq!(new_version, 2);

    let key = ks.get_key("rotate-me").unwrap();
    assert_eq!(key.current_version, 2);
    assert_eq!(key.versions.len(), 2);

    // Old version still accessible
    let old = ks.get_key_version("rotate-me", 1).unwrap();
    assert_eq!(old.version, 1);

    // New version accessible
    let new = ks.get_key_version("rotate-me", 2).unwrap();
    assert_eq!(new.version, 2);

    // Current bytes should differ from v1 (overwhelmingly likely)
    let v2_bytes = ks.current_key_bytes("rotate-me").unwrap();
    assert_ne!(v1_bytes, v2_bytes);

    // Versioned bytes for v1 should match original
    let v1_again = ks.versioned_key_bytes("rotate-me", 1).unwrap();
    assert_eq!(v1_bytes, v1_again);

    // Versioned bytes for v2 should match current
    let v2_again = ks.versioned_key_bytes("rotate-me", 2).unwrap();
    assert_eq!(v2_bytes, v2_again);
}

#[test]
fn delete_key_removes_it() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    ks.generate_key("delete-me", "aes-256").unwrap();
    assert!(ks.has_key("delete-me"));

    ks.delete_key("delete-me").unwrap();
    assert!(!ks.has_key("delete-me"));
    assert!(ks.get_key("delete-me").is_none());
}

#[test]
fn delete_missing_key_errors() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };

    let err = ks.delete_key("nope").unwrap_err();
    assert!(matches!(err, AppKeystoreError::KeyNotFound { name } if name == "nope"));
}

#[test]
fn rotate_missing_key_errors() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };

    let err = ks.rotate_key("nope").unwrap_err();
    assert!(matches!(err, AppKeystoreError::KeyNotFound { name } if name == "nope"));
}

#[test]
fn key_info_returns_metadata_without_raw_bytes() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    ks.generate_key("info-key", "aes-256").unwrap();
    ks.rotate_key("info-key").unwrap();

    let info = ks.key_info("info-key").unwrap();
    assert_eq!(info.name, "info-key");
    assert_eq!(info.key_type, "aes-256");
    assert_eq!(info.current_version, 2);
    assert_eq!(info.version_count, 2);

    // KeyInfo has no key_material field — verified at compile time by the struct definition.
}

#[test]
fn key_info_missing_key_errors() {
    let ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    let err = ks.key_info("nope").unwrap_err();
    assert!(matches!(err, AppKeystoreError::KeyNotFound { name } if name == "nope"));
}

#[test]
fn list_keys_returns_correct_count() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };

    assert_eq!(ks.list_keys().len(), 0);

    ks.generate_key("key-a", "aes-256").unwrap();
    ks.generate_key("key-b", "aes-256").unwrap();
    ks.generate_key("key-c", "aes-256").unwrap();

    let infos = ks.list_keys();
    assert_eq!(infos.len(), 3);

    let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
    assert!(names.contains(&"key-a"));
    assert!(names.contains(&"key-b"));
    assert!(names.contains(&"key-c"));
}

#[test]
fn has_key_returns_correct_bool() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };

    assert!(!ks.has_key("test"));
    ks.generate_key("test", "aes-256").unwrap();
    assert!(ks.has_key("test"));
    assert!(!ks.has_key("other"));
}

#[test]
fn current_key_bytes_returns_32_bytes() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    ks.generate_key("bytes-test", "aes-256").unwrap();

    let bytes = ks.current_key_bytes("bytes-test").unwrap();
    assert_eq!(bytes.len(), 32);
}

#[test]
fn current_key_bytes_missing_key_errors() {
    let ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    let err = ks.current_key_bytes("nope").unwrap_err();
    assert!(matches!(err, AppKeystoreError::KeyNotFound { .. }));
}

#[test]
fn versioned_key_bytes_per_version() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    ks.generate_key("versioned", "aes-256").unwrap();
    ks.rotate_key("versioned").unwrap();
    ks.rotate_key("versioned").unwrap();

    // All three versions should return 32 bytes
    for v in 1..=3 {
        let bytes = ks.versioned_key_bytes("versioned", v).unwrap();
        assert_eq!(bytes.len(), 32, "version {} should be 32 bytes", v);
    }

    // Different versions should have different key material
    let v1 = ks.versioned_key_bytes("versioned", 1).unwrap();
    let v2 = ks.versioned_key_bytes("versioned", 2).unwrap();
    let v3 = ks.versioned_key_bytes("versioned", 3).unwrap();
    assert_ne!(v1, v2);
    assert_ne!(v2, v3);
    assert_ne!(v1, v3);
}

#[test]
fn versioned_key_bytes_missing_version_errors() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    ks.generate_key("vtest", "aes-256").unwrap();

    let err = ks.versioned_key_bytes("vtest", 99).unwrap_err();
    assert!(
        matches!(err, AppKeystoreError::KeyVersionNotFound { name, version }
            if name == "vtest" && version == 99)
    );
}

#[test]
fn get_key_version_for_existing() {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    ks.generate_key("gkv-test", "aes-256").unwrap();

    let kv = ks.get_key_version("gkv-test", 1).unwrap();
    assert_eq!(kv.version, 1);
    assert!(!kv.key_material.is_empty());
}
