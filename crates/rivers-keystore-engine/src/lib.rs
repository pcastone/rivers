//! Application Keystore Engine — Age-encrypted TOML key management.
//!
//! Per `rivers-feature-request-app-keystore.md`.
//!
//! Manages an Age-encrypted TOML file containing named AES-256 keys
//! with version history. Keys are generated, rotated, and deleted
//! through this crate. Key material is zeroized on drop.
//!
//! AES-256-GCM encrypt/decrypt operations are standalone functions
//! (`encrypt`, `decrypt`) plus convenience wrappers on `AppKeystore`.

#![warn(missing_docs)]

mod types;
mod io;
mod key_management;
mod crypto;

// Re-export everything at crate root so external callers see no change.
pub use types::*;
pub use crypto::{encrypt, decrypt};

// ── Test Helpers ────────────────────────────────────────────────────

/// Create a test keystore in memory with one AES-256 key.
///
/// Returns the keystore ready for use — no file I/O needed.
/// Useful for integration tests that need a keystore without the full
/// Lockbox/Age-encrypted-file lifecycle.
pub fn create_test_keystore(key_name: &str) -> AppKeystore {
    let mut ks = AppKeystore {
        version: 1,
        keys: Vec::new(),
    };
    ks.generate_key(key_name, "aes-256").unwrap();
    ks
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use rand::RngCore;
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

    // ── AES-256-GCM crypto tests ───────────────────────────────────

    /// Helper: generate a valid 32-byte key for standalone encrypt/decrypt tests.
    fn generate_raw_key() -> Vec<u8> {
        let mut key = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        key
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = generate_raw_key();
        let plaintext = b"hello rivers keystore";

        let enc = encrypt(&key, plaintext, None).unwrap();
        let dec = decrypt(&key, &enc.ciphertext, &enc.nonce, None).unwrap();

        assert_eq!(dec, plaintext);
        assert_eq!(enc.key_version, 0); // standalone sets 0
    }

    #[test]
    fn encrypt_decrypt_round_trip_with_aad() {
        let key = generate_raw_key();
        let plaintext = b"sensitive data";
        let aad = b"device-1";

        let enc = encrypt(&key, plaintext, Some(aad)).unwrap();
        let dec = decrypt(&key, &enc.ciphertext, &enc.nonce, Some(aad)).unwrap();

        assert_eq!(dec, plaintext);
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let key1 = generate_raw_key();
        let key2 = generate_raw_key();
        let plaintext = b"secret";

        let enc = encrypt(&key1, plaintext, None).unwrap();
        let err = decrypt(&key2, &enc.ciphertext, &enc.nonce, None).unwrap_err();

        assert!(matches!(err, AppKeystoreError::DecryptionFailed));
    }

    #[test]
    fn tampered_ciphertext_fails_decrypt() {
        let key = generate_raw_key();
        let plaintext = b"original data";

        let enc = encrypt(&key, plaintext, None).unwrap();

        // Decode, flip a byte, re-encode
        let mut ct_bytes = BASE64.decode(&enc.ciphertext).unwrap();
        ct_bytes[0] ^= 0xFF;
        let tampered = BASE64.encode(&ct_bytes);

        let err = decrypt(&key, &tampered, &enc.nonce, None).unwrap_err();
        assert!(matches!(err, AppKeystoreError::DecryptionFailed));
    }

    #[test]
    fn aad_mismatch_fails_decrypt() {
        let key = generate_raw_key();
        let plaintext = b"data";

        let enc = encrypt(&key, plaintext, Some(b"device-1")).unwrap();
        let err = decrypt(&key, &enc.ciphertext, &enc.nonce, Some(b"device-2")).unwrap_err();

        assert!(matches!(err, AppKeystoreError::DecryptionFailed));
    }

    #[test]
    fn two_encrypts_produce_different_ciphertexts() {
        let key = generate_raw_key();
        let plaintext = b"same input";

        let enc1 = encrypt(&key, plaintext, None).unwrap();
        let enc2 = encrypt(&key, plaintext, None).unwrap();

        // Nonces must differ (overwhelmingly likely with random 96-bit nonces)
        assert_ne!(enc1.nonce, enc2.nonce);
        // Ciphertexts must differ because nonces differ
        assert_ne!(enc1.ciphertext, enc2.ciphertext);
    }

    #[test]
    fn invalid_nonce_too_short() {
        let key = generate_raw_key();
        let plaintext = b"data";

        let enc = encrypt(&key, plaintext, None).unwrap();

        // Use a 6-byte nonce (too short)
        let short_nonce = BASE64.encode(&[0u8; 6]);
        let err = decrypt(&key, &enc.ciphertext, &short_nonce, None).unwrap_err();
        assert!(matches!(err, AppKeystoreError::InvalidNonce { .. }));
    }

    #[test]
    fn invalid_nonce_too_long() {
        let key = generate_raw_key();
        let plaintext = b"data";

        let enc = encrypt(&key, plaintext, None).unwrap();

        // Use a 16-byte nonce (too long)
        let long_nonce = BASE64.encode(&[0u8; 16]);
        let err = decrypt(&key, &enc.ciphertext, &long_nonce, None).unwrap_err();
        assert!(matches!(err, AppKeystoreError::InvalidNonce { .. }));
    }

    #[test]
    fn invalid_key_length_16_bytes() {
        let short_key = vec![0u8; 16];
        let err = encrypt(&short_key, b"data", None).unwrap_err();
        assert!(
            matches!(err, AppKeystoreError::InvalidKeyLength { expected: 32, got: 16 })
        );
    }

    #[test]
    fn invalid_key_length_64_bytes() {
        let long_key = vec![0u8; 64];
        let err = encrypt(&long_key, b"data", None).unwrap_err();
        assert!(
            matches!(err, AppKeystoreError::InvalidKeyLength { expected: 32, got: 64 })
        );
    }

    #[test]
    fn invalid_key_length_on_decrypt() {
        let short_key = vec![0u8; 16];
        let err = decrypt(&short_key, "AAAA", "AAAAAAAAAAAAAAAAAA==", None).unwrap_err();
        assert!(
            matches!(err, AppKeystoreError::InvalidKeyLength { expected: 32, got: 16 })
        );
    }

    #[test]
    fn encrypt_with_key_decrypt_with_key_round_trip() {
        let mut ks = AppKeystore {
            version: 1,
            keys: Vec::new(),
        };
        ks.generate_key("crypto-key", "aes-256").unwrap();

        let plaintext = b"keystore round trip";
        let enc = ks.encrypt_with_key("crypto-key", plaintext, None).unwrap();

        assert_eq!(enc.key_version, 1);

        let dec = ks
            .decrypt_with_key("crypto-key", &enc.ciphertext, &enc.nonce, enc.key_version, None)
            .unwrap();

        assert_eq!(dec, plaintext);
    }

    #[test]
    fn encrypt_with_key_decrypt_with_key_aad() {
        let mut ks = AppKeystore {
            version: 1,
            keys: Vec::new(),
        };
        ks.generate_key("aad-key", "aes-256").unwrap();

        let plaintext = b"with context";
        let aad = b"app-id-123";

        let enc = ks
            .encrypt_with_key("aad-key", plaintext, Some(aad))
            .unwrap();

        let dec = ks
            .decrypt_with_key("aad-key", &enc.ciphertext, &enc.nonce, enc.key_version, Some(aad))
            .unwrap();

        assert_eq!(dec, plaintext);
    }

    #[test]
    fn encrypt_with_key_nonexistent_key() {
        let ks = AppKeystore {
            version: 1,
            keys: Vec::new(),
        };

        let err = ks
            .encrypt_with_key("no-such-key", b"data", None)
            .unwrap_err();

        assert!(
            matches!(err, AppKeystoreError::KeyNotFound { name } if name == "no-such-key")
        );
    }

    #[test]
    fn decrypt_with_key_wrong_version() {
        let mut ks = AppKeystore {
            version: 1,
            keys: Vec::new(),
        };
        ks.generate_key("ver-key", "aes-256").unwrap();

        let enc = ks.encrypt_with_key("ver-key", b"data", None).unwrap();

        // Try to decrypt with version 99 which doesn't exist
        let err = ks
            .decrypt_with_key("ver-key", &enc.ciphertext, &enc.nonce, 99, None)
            .unwrap_err();

        assert!(
            matches!(err, AppKeystoreError::KeyVersionNotFound { name, version }
                if name == "ver-key" && version == 99)
        );
    }

    #[test]
    fn encrypt_with_rotated_key_decrypt_with_old_version() {
        let mut ks = AppKeystore {
            version: 1,
            keys: Vec::new(),
        };
        ks.generate_key("rotate-crypto", "aes-256").unwrap();

        // Encrypt with v1
        let enc_v1 = ks
            .encrypt_with_key("rotate-crypto", b"v1 data", None)
            .unwrap();
        assert_eq!(enc_v1.key_version, 1);

        // Rotate
        ks.rotate_key("rotate-crypto").unwrap();

        // Encrypt with v2 (now current)
        let enc_v2 = ks
            .encrypt_with_key("rotate-crypto", b"v2 data", None)
            .unwrap();
        assert_eq!(enc_v2.key_version, 2);

        // Decrypt v1 ciphertext with v1 key
        let dec_v1 = ks
            .decrypt_with_key(
                "rotate-crypto",
                &enc_v1.ciphertext,
                &enc_v1.nonce,
                1,
                None,
            )
            .unwrap();
        assert_eq!(dec_v1, b"v1 data");

        // Decrypt v2 ciphertext with v2 key
        let dec_v2 = ks
            .decrypt_with_key(
                "rotate-crypto",
                &enc_v2.ciphertext,
                &enc_v2.nonce,
                2,
                None,
            )
            .unwrap();
        assert_eq!(dec_v2, b"v2 data");

        // Decrypt v1 ciphertext with v2 key should fail
        let err = ks
            .decrypt_with_key(
                "rotate-crypto",
                &enc_v1.ciphertext,
                &enc_v1.nonce,
                2,
                None,
            )
            .unwrap_err();
        assert!(matches!(err, AppKeystoreError::DecryptionFailed));
    }

    #[test]
    fn encrypt_empty_plaintext() {
        let key = generate_raw_key();
        let enc = encrypt(&key, b"", None).unwrap();
        let dec = decrypt(&key, &enc.ciphertext, &enc.nonce, None).unwrap();
        assert!(dec.is_empty());
    }
}
