//! Crypto tests — encrypt/decrypt with/without AAD, tampered data, wrong key, key length.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::RngCore;
use rivers_keystore_engine::*;

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
