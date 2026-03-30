//! AES-256-GCM encrypt/decrypt — standalone functions + AppKeystore wrappers.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::Aes256Gcm;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use zeroize::Zeroize;

use crate::types::*;

// ── AES-256-GCM encrypt / decrypt ───────────────────────────────────

/// Encrypt plaintext with AES-256-GCM using the given key bytes.
///
/// Generates a random 96-bit nonce via OsRng. Never accepts a caller-supplied nonce.
/// Returns base64-encoded ciphertext and nonce.
pub fn encrypt(
    key_bytes: &[u8],
    plaintext: &[u8],
    aad: Option<&[u8]>,
) -> Result<EncryptResult, AppKeystoreError> {
    if key_bytes.len() != AES_256_KEY_SIZE {
        return Err(AppKeystoreError::InvalidKeyLength {
            expected: AES_256_KEY_SIZE,
            got: key_bytes.len(),
        });
    }

    let cipher = Aes256Gcm::new_from_slice(key_bytes)
        .expect("key length already validated as 32 bytes");

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext_bytes = match aad {
        Some(aad_bytes) => cipher
            .encrypt(&nonce, Payload { msg: plaintext, aad: aad_bytes })
            .map_err(|_| AppKeystoreError::DecryptionFailed)?,
        None => cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| AppKeystoreError::DecryptionFailed)?,
    };

    Ok(EncryptResult {
        ciphertext: BASE64.encode(&ciphertext_bytes),
        nonce: BASE64.encode(&nonce),
        key_version: 0, // Caller sets the real version
    })
}

/// Decrypt AES-256-GCM ciphertext using the given key bytes.
///
/// On any failure returns a generic `DecryptionFailed` error (no oracle).
pub fn decrypt(
    key_bytes: &[u8],
    ciphertext_b64: &str,
    nonce_b64: &str,
    aad: Option<&[u8]>,
) -> Result<Vec<u8>, AppKeystoreError> {
    if key_bytes.len() != AES_256_KEY_SIZE {
        return Err(AppKeystoreError::InvalidKeyLength {
            expected: AES_256_KEY_SIZE,
            got: key_bytes.len(),
        });
    }

    let ciphertext_bytes = BASE64
        .decode(ciphertext_b64)
        .map_err(|_| AppKeystoreError::DecryptionFailed)?;

    let nonce_bytes = BASE64
        .decode(nonce_b64)
        .map_err(|_| AppKeystoreError::InvalidNonce {
            reason: "invalid base64 nonce".to_string(),
        })?;

    if nonce_bytes.len() != AES_GCM_NONCE_SIZE {
        return Err(AppKeystoreError::InvalidNonce {
            reason: format!(
                "expected {} bytes, got {}",
                AES_GCM_NONCE_SIZE,
                nonce_bytes.len()
            ),
        });
    }

    let cipher = Aes256Gcm::new_from_slice(key_bytes)
        .expect("key length already validated as 32 bytes");

    let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);

    let plaintext = match aad {
        Some(aad_bytes) => cipher
            .decrypt(nonce, Payload { msg: ciphertext_bytes.as_ref(), aad: aad_bytes })
            .map_err(|_| AppKeystoreError::DecryptionFailed)?,
        None => cipher
            .decrypt(nonce, ciphertext_bytes.as_ref())
            .map_err(|_| AppKeystoreError::DecryptionFailed)?,
    };

    Ok(plaintext)
}

// ── AppKeystore convenience wrappers ────────────────────────────────

impl AppKeystore {
    /// Encrypt using the current version of the named key.
    pub fn encrypt_with_key(
        &self,
        key_name: &str,
        plaintext: &[u8],
        aad: Option<&[u8]>,
    ) -> Result<EncryptResult, AppKeystoreError> {
        let mut key_bytes = self.current_key_bytes(key_name)?;

        let key_meta = self
            .get_key(key_name)
            .expect("key exists — current_key_bytes succeeded");
        let version = key_meta.current_version;

        let result = encrypt(&key_bytes, plaintext, aad);

        // Zeroize key material regardless of success/failure
        key_bytes.zeroize();

        let mut enc = result?;
        enc.key_version = version;
        Ok(enc)
    }

    /// Decrypt using a specific version of the named key.
    pub fn decrypt_with_key(
        &self,
        key_name: &str,
        ciphertext_b64: &str,
        nonce_b64: &str,
        key_version: u32,
        aad: Option<&[u8]>,
    ) -> Result<Vec<u8>, AppKeystoreError> {
        let mut key_bytes = self.versioned_key_bytes(key_name, key_version)?;

        let result = decrypt(&key_bytes, ciphertext_b64, nonce_b64, aad);

        // Zeroize key material regardless of success/failure
        key_bytes.zeroize();

        result
    }
}
