//! Integration tests for the Application Keystore feature.
//!
//! Exercises the full keystore flow: creation, key generation, encrypt/decrypt
//! round-trip, key rotation with versioning, AAD, persistence, and error cases.

use rivers_keystore_engine::*;
use tempfile::TempDir;

// ── Helpers ─────────────────────────────────────────────────────────

/// Generate a fresh Age keypair for testing, returning (identity_str, recipient_str).
fn generate_age_keypair() -> (String, String) {
    use age::secrecy::ExposeSecret;
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();
    let identity_str = identity.to_string().expose_secret().to_string();
    let recipient_str = recipient.to_string();
    (identity_str, recipient_str)
}

// ── T12.2: Full encrypt/decrypt round-trip ──────────────────────────

#[test]
fn full_encrypt_decrypt_round_trip() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("test.akeystore");
    let (identity, recipient) = generate_age_keypair();

    // Create and populate keystore
    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("test-key", "aes-256").unwrap();
    ks.save(&ks_path, &recipient).unwrap();

    // Encrypt
    let enc = ks
        .encrypt_with_key("test-key", b"hello world", None)
        .unwrap();
    assert!(!enc.ciphertext.is_empty());
    assert!(!enc.nonce.is_empty());
    assert_eq!(enc.key_version, 1);

    // Decrypt
    let dec = ks
        .decrypt_with_key("test-key", &enc.ciphertext, &enc.nonce, enc.key_version, None)
        .unwrap();
    assert_eq!(dec, b"hello world");
}

#[test]
fn create_load_verify_empty_keystore() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("empty.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    assert!(ks_path.exists());

    let ks = AppKeystore::load(&ks_path, &identity).unwrap();
    assert_eq!(ks.version, 1);
    assert!(ks.keys.is_empty());
}

#[test]
fn generate_key_populates_keystore() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("gen.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();

    let key = ks.generate_key("my-key", "aes-256").unwrap();
    assert_eq!(key.name, "my-key");
    assert_eq!(key.key_type, "aes-256");
    assert_eq!(key.current_version, 1);
    assert_eq!(key.versions.len(), 1);
}

// ── T12.3: Key rotation flow ────────────────────────────────────────

#[test]
fn key_rotation_preserves_old_versions_and_both_decrypt() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("rotate.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("rotate-key", "aes-256").unwrap();

    // Step 1: Encrypt data with current key (v1)
    let enc_v1 = ks
        .encrypt_with_key("rotate-key", b"v1-secret", None)
        .unwrap();
    assert_eq!(enc_v1.key_version, 1);

    // Step 2: Rotate key
    let new_version = ks.rotate_key("rotate-key").unwrap();
    assert_eq!(new_version, 2);

    // Step 3: Encrypt new data (now uses v2 automatically)
    let enc_v2 = ks
        .encrypt_with_key("rotate-key", b"v2-secret", None)
        .unwrap();
    assert_eq!(enc_v2.key_version, 2);

    // Step 4: Decrypt v1 data using key_version: 1 -> succeeds
    let dec_v1 = ks
        .decrypt_with_key(
            "rotate-key",
            &enc_v1.ciphertext,
            &enc_v1.nonce,
            1,
            None,
        )
        .unwrap();
    assert_eq!(dec_v1, b"v1-secret");

    // Step 5: Decrypt v2 data using key_version: 2 -> succeeds
    let dec_v2 = ks
        .decrypt_with_key(
            "rotate-key",
            &enc_v2.ciphertext,
            &enc_v2.nonce,
            2,
            None,
        )
        .unwrap();
    assert_eq!(dec_v2, b"v2-secret");

    // Step 6: Verify v1 and v2 ciphertexts differ (different keys)
    assert_ne!(
        enc_v1.ciphertext, enc_v2.ciphertext,
        "v1 and v2 ciphertexts should differ (different key material)"
    );

    // Also verify cross-version decryption fails (v1 ciphertext with v2 key)
    let cross_err = ks
        .decrypt_with_key(
            "rotate-key",
            &enc_v1.ciphertext,
            &enc_v1.nonce,
            2,
            None,
        )
        .unwrap_err();
    assert!(matches!(cross_err, AppKeystoreError::DecryptionFailed));
}

#[test]
fn multi_rotation_three_versions() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("multi-rotate.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("multi", "aes-256").unwrap();

    // Encrypt with v1
    let enc1 = ks.encrypt_with_key("multi", b"data-1", None).unwrap();
    assert_eq!(enc1.key_version, 1);

    // Rotate to v2
    ks.rotate_key("multi").unwrap();
    let enc2 = ks.encrypt_with_key("multi", b"data-2", None).unwrap();
    assert_eq!(enc2.key_version, 2);

    // Rotate to v3
    ks.rotate_key("multi").unwrap();
    let enc3 = ks.encrypt_with_key("multi", b"data-3", None).unwrap();
    assert_eq!(enc3.key_version, 3);

    // All three versions decrypt correctly
    let dec1 = ks
        .decrypt_with_key("multi", &enc1.ciphertext, &enc1.nonce, 1, None)
        .unwrap();
    assert_eq!(dec1, b"data-1");

    let dec2 = ks
        .decrypt_with_key("multi", &enc2.ciphertext, &enc2.nonce, 2, None)
        .unwrap();
    assert_eq!(dec2, b"data-2");

    let dec3 = ks
        .decrypt_with_key("multi", &enc3.ciphertext, &enc3.nonce, 3, None)
        .unwrap();
    assert_eq!(dec3, b"data-3");

    // Verify key info reflects 3 versions
    let info = ks.key_info("multi").unwrap();
    assert_eq!(info.current_version, 3);
    assert_eq!(info.version_count, 3);
}

// ── T12.4: Error cases ──────────────────────────────────────────────

#[test]
fn encrypt_with_nonexistent_key_returns_key_not_found() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("err.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let ks = AppKeystore::load(&ks_path, &identity).unwrap();

    let err = ks
        .encrypt_with_key("nonexistent", b"data", None)
        .unwrap_err();
    assert!(
        matches!(err, AppKeystoreError::KeyNotFound { ref name } if name == "nonexistent"),
        "expected KeyNotFound, got: {:?}",
        err
    );
}

#[test]
fn decrypt_with_wrong_version_returns_key_version_not_found() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("ver-err.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("ver-key", "aes-256").unwrap();

    let enc = ks.encrypt_with_key("ver-key", b"data", None).unwrap();

    let err = ks
        .decrypt_with_key("ver-key", &enc.ciphertext, &enc.nonce, 99, None)
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppKeystoreError::KeyVersionNotFound { ref name, version }
            if name == "ver-key" && version == 99
        ),
        "expected KeyVersionNotFound, got: {:?}",
        err
    );
}

#[test]
fn decrypt_with_tampered_ciphertext_returns_decryption_failed() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("tamper.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("tamper-key", "aes-256").unwrap();

    let enc = ks
        .encrypt_with_key("tamper-key", b"original data", None)
        .unwrap();

    // Decode ciphertext, flip a byte, re-encode
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    let mut ct_bytes = BASE64.decode(&enc.ciphertext).unwrap();
    ct_bytes[0] ^= 0xFF;
    let tampered = BASE64.encode(&ct_bytes);

    let err = ks
        .decrypt_with_key("tamper-key", &tampered, &enc.nonce, enc.key_version, None)
        .unwrap_err();
    assert!(
        matches!(err, AppKeystoreError::DecryptionFailed),
        "expected DecryptionFailed, got: {:?}",
        err
    );
}

#[test]
fn decrypt_with_wrong_aad_returns_decryption_failed() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("aad-err.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("aad-key", "aes-256").unwrap();

    // Encrypt with one AAD
    let enc = ks
        .encrypt_with_key("aad-key", b"data", Some(b"correct-context"))
        .unwrap();

    // Decrypt with different AAD
    let err = ks
        .decrypt_with_key(
            "aad-key",
            &enc.ciphertext,
            &enc.nonce,
            enc.key_version,
            Some(b"wrong-context"),
        )
        .unwrap_err();
    assert!(
        matches!(err, AppKeystoreError::DecryptionFailed),
        "expected DecryptionFailed for wrong AAD, got: {:?}",
        err
    );
}

// ── T12.5: AAD flow ─────────────────────────────────────────────────

#[test]
fn aad_encrypt_decrypt_round_trip() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("aad.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("aad-key", "aes-256").unwrap();

    // Encrypt with AAD
    let enc = ks
        .encrypt_with_key("aad-key", b"sensitive payload", Some(b"context-1"))
        .unwrap();
    assert_eq!(enc.key_version, 1);

    // Decrypt with same AAD -> succeeds
    let dec = ks
        .decrypt_with_key(
            "aad-key",
            &enc.ciphertext,
            &enc.nonce,
            enc.key_version,
            Some(b"context-1"),
        )
        .unwrap();
    assert_eq!(dec, b"sensitive payload");
}

#[test]
fn aad_mismatch_fails() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("aad-mismatch.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("aad-test", "aes-256").unwrap();

    let enc = ks
        .encrypt_with_key("aad-test", b"data", Some(b"context-A"))
        .unwrap();

    // Different AAD
    let err = ks
        .decrypt_with_key(
            "aad-test",
            &enc.ciphertext,
            &enc.nonce,
            enc.key_version,
            Some(b"context-B"),
        )
        .unwrap_err();
    assert!(matches!(err, AppKeystoreError::DecryptionFailed));

    // No AAD at all (was encrypted with AAD)
    let err2 = ks
        .decrypt_with_key(
            "aad-test",
            &enc.ciphertext,
            &enc.nonce,
            enc.key_version,
            None,
        )
        .unwrap_err();
    assert!(matches!(err2, AppKeystoreError::DecryptionFailed));
}

#[test]
fn no_aad_vs_aad_fails() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("no-aad.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("no-aad-key", "aes-256").unwrap();

    // Encrypt without AAD
    let enc = ks
        .encrypt_with_key("no-aad-key", b"data", None)
        .unwrap();

    // Try to decrypt with an AAD -> should fail
    let err = ks
        .decrypt_with_key(
            "no-aad-key",
            &enc.ciphertext,
            &enc.nonce,
            enc.key_version,
            Some(b"unexpected-context"),
        )
        .unwrap_err();
    assert!(matches!(err, AppKeystoreError::DecryptionFailed));
}

// ── T12.6: Persistence round-trip ───────────────────────────────────

#[test]
fn persistence_round_trip_encrypt_save_load_decrypt() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("persist.akeystore");
    let (identity, recipient) = generate_age_keypair();

    // Create keystore, generate key, encrypt data, save
    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("persist-key", "aes-256").unwrap();

    let enc = ks
        .encrypt_with_key("persist-key", b"survive the reload", None)
        .unwrap();
    assert_eq!(enc.key_version, 1);

    ks.save(&ks_path, &recipient).unwrap();
    drop(ks); // Explicitly drop to prove we rely on loaded state

    // Load keystore from same file
    let ks2 = AppKeystore::load(&ks_path, &identity).unwrap();

    // Decrypt with loaded keystore -> succeeds (proves persistence works)
    let dec = ks2
        .decrypt_with_key(
            "persist-key",
            &enc.ciphertext,
            &enc.nonce,
            enc.key_version,
            None,
        )
        .unwrap();
    assert_eq!(dec, b"survive the reload");
}

#[test]
fn persistence_with_rotation_survives_reload() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("persist-rotate.akeystore");
    let (identity, recipient) = generate_age_keypair();

    // Create, generate, encrypt v1
    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("pr-key", "aes-256").unwrap();

    let enc_v1 = ks
        .encrypt_with_key("pr-key", b"v1 payload", None)
        .unwrap();

    // Rotate, encrypt v2, save
    ks.rotate_key("pr-key").unwrap();
    let enc_v2 = ks
        .encrypt_with_key("pr-key", b"v2 payload", None)
        .unwrap();
    ks.save(&ks_path, &recipient).unwrap();
    drop(ks);

    // Load fresh
    let ks2 = AppKeystore::load(&ks_path, &identity).unwrap();

    // Both versions still decrypt
    let dec_v1 = ks2
        .decrypt_with_key("pr-key", &enc_v1.ciphertext, &enc_v1.nonce, 1, None)
        .unwrap();
    assert_eq!(dec_v1, b"v1 payload");

    let dec_v2 = ks2
        .decrypt_with_key("pr-key", &enc_v2.ciphertext, &enc_v2.nonce, 2, None)
        .unwrap();
    assert_eq!(dec_v2, b"v2 payload");

    // Verify key info
    let info = ks2.key_info("pr-key").unwrap();
    assert_eq!(info.current_version, 2);
    assert_eq!(info.version_count, 2);
}

#[test]
fn persistence_multiple_keys_survive_reload() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("multi-key.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();

    // Generate three keys
    ks.generate_key("alpha", "aes-256").unwrap();
    ks.generate_key("beta", "aes-256").unwrap();
    ks.generate_key("gamma", "aes-256").unwrap();

    // Encrypt with each
    let enc_a = ks
        .encrypt_with_key("alpha", b"alpha-data", None)
        .unwrap();
    let enc_b = ks
        .encrypt_with_key("beta", b"beta-data", Some(b"beta-ctx"))
        .unwrap();
    let enc_g = ks
        .encrypt_with_key("gamma", b"gamma-data", None)
        .unwrap();

    ks.save(&ks_path, &recipient).unwrap();
    drop(ks);

    // Load and decrypt all
    let ks2 = AppKeystore::load(&ks_path, &identity).unwrap();
    assert_eq!(ks2.list_keys().len(), 3);

    let dec_a = ks2
        .decrypt_with_key("alpha", &enc_a.ciphertext, &enc_a.nonce, 1, None)
        .unwrap();
    assert_eq!(dec_a, b"alpha-data");

    let dec_b = ks2
        .decrypt_with_key("beta", &enc_b.ciphertext, &enc_b.nonce, 1, Some(b"beta-ctx"))
        .unwrap();
    assert_eq!(dec_b, b"beta-data");

    let dec_g = ks2
        .decrypt_with_key("gamma", &enc_g.ciphertext, &enc_g.nonce, 1, None)
        .unwrap();
    assert_eq!(dec_g, b"gamma-data");
}

// ── Full lifecycle: create -> generate -> rotate -> delete -> persist ─

#[test]
fn full_lifecycle_integration() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("lifecycle.akeystore");
    let (identity, recipient) = generate_age_keypair();

    // Phase 1: Create and populate
    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("primary", "aes-256").unwrap();
    ks.generate_key("secondary", "aes-256").unwrap();
    assert_eq!(ks.list_keys().len(), 2);

    // Phase 2: Encrypt with both keys
    let enc_p = ks
        .encrypt_with_key("primary", b"primary-secret", None)
        .unwrap();
    let _enc_s = ks
        .encrypt_with_key("secondary", b"secondary-secret", Some(b"app-ctx"))
        .unwrap();

    // Phase 3: Rotate primary, encrypt new data
    ks.rotate_key("primary").unwrap();
    let enc_p2 = ks
        .encrypt_with_key("primary", b"primary-v2-secret", None)
        .unwrap();
    assert_eq!(enc_p2.key_version, 2);

    // Phase 4: Delete secondary
    ks.delete_key("secondary").unwrap();
    assert_eq!(ks.list_keys().len(), 1);
    assert!(!ks.has_key("secondary"));

    // Phase 5: Save and reload
    ks.save(&ks_path, &recipient).unwrap();
    drop(ks);

    let ks2 = AppKeystore::load(&ks_path, &identity).unwrap();

    // Primary still works for both versions
    let dec_p1 = ks2
        .decrypt_with_key("primary", &enc_p.ciphertext, &enc_p.nonce, 1, None)
        .unwrap();
    assert_eq!(dec_p1, b"primary-secret");

    let dec_p2 = ks2
        .decrypt_with_key("primary", &enc_p2.ciphertext, &enc_p2.nonce, 2, None)
        .unwrap();
    assert_eq!(dec_p2, b"primary-v2-secret");

    // Secondary is gone
    assert!(!ks2.has_key("secondary"));
    let err = ks2
        .encrypt_with_key("secondary", b"gone", None)
        .unwrap_err();
    assert!(matches!(err, AppKeystoreError::KeyNotFound { .. }));
}

// ── Edge cases ──────────────────────────────────────────────────────

#[test]
fn encrypt_decrypt_empty_plaintext() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("empty-pt.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("empty-key", "aes-256").unwrap();

    let enc = ks
        .encrypt_with_key("empty-key", b"", None)
        .unwrap();

    let dec = ks
        .decrypt_with_key("empty-key", &enc.ciphertext, &enc.nonce, enc.key_version, None)
        .unwrap();
    assert!(dec.is_empty());
}

#[test]
fn encrypt_decrypt_large_payload() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("large.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("large-key", "aes-256").unwrap();

    // 1 MB payload
    let large_data = vec![0xABu8; 1_000_000];
    let enc = ks
        .encrypt_with_key("large-key", &large_data, None)
        .unwrap();

    let dec = ks
        .decrypt_with_key("large-key", &enc.ciphertext, &enc.nonce, enc.key_version, None)
        .unwrap();
    assert_eq!(dec, large_data);
}

#[test]
fn same_plaintext_produces_different_ciphertexts() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("nonce.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("nonce-key", "aes-256").unwrap();

    let enc1 = ks
        .encrypt_with_key("nonce-key", b"identical", None)
        .unwrap();
    let enc2 = ks
        .encrypt_with_key("nonce-key", b"identical", None)
        .unwrap();

    // Different nonces => different ciphertexts (random nonce each time)
    assert_ne!(enc1.nonce, enc2.nonce);
    assert_ne!(enc1.ciphertext, enc2.ciphertext);

    // Both decrypt to same plaintext
    let dec1 = ks
        .decrypt_with_key("nonce-key", &enc1.ciphertext, &enc1.nonce, 1, None)
        .unwrap();
    let dec2 = ks
        .decrypt_with_key("nonce-key", &enc2.ciphertext, &enc2.nonce, 1, None)
        .unwrap();
    assert_eq!(dec1, dec2);
    assert_eq!(dec1, b"identical");
}

#[test]
fn load_with_wrong_identity_fails() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("wrong-id.akeystore");
    let (_identity, recipient) = generate_age_keypair();
    let (wrong_identity, _) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();

    let err = AppKeystore::load(&ks_path, &wrong_identity).unwrap_err();
    assert!(matches!(err, AppKeystoreError::DecryptionFailed));
}

#[test]
fn duplicate_key_name_returns_error() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("dup.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();
    ks.generate_key("dup-key", "aes-256").unwrap();

    let err = ks.generate_key("dup-key", "aes-256").unwrap_err();
    assert!(matches!(err, AppKeystoreError::DuplicateKey { ref name } if name == "dup-key"));
}

#[test]
fn invalid_key_type_returns_error() {
    let tmp = TempDir::new().unwrap();
    let ks_path = tmp.path().join("bad-type.akeystore");
    let (identity, recipient) = generate_age_keypair();

    AppKeystore::create(&ks_path, &recipient).unwrap();
    let mut ks = AppKeystore::load(&ks_path, &identity).unwrap();

    let err = ks.generate_key("bad", "aes-128").unwrap_err();
    assert!(matches!(
        err,
        AppKeystoreError::InvalidKeyType {
            ref expected,
            ref got
        } if expected == "aes-256" && got == "aes-128"
    ));
}
