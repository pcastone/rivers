//! Age-based keystore encryption and decryption.

use std::path::Path;

use zeroize::Zeroize;

use crate::key_source::check_file_permissions;
use crate::types::*;

// ── Keystore Decryption ─────────────────────────────────────────────

/// Decrypt and parse a keystore file.
///
/// Per spec S8.1, steps 4-9.
///
/// Permission check runs on every call — a runtime `chmod` after startup
/// will be caught here and not silently bypass security.
pub fn decrypt_keystore(
    keystore_path: &Path,
    identity_str: &str,
) -> Result<Keystore, LockBoxError> {
    // Check permissions on every decrypt, not just at startup.
    check_file_permissions(keystore_path)?;

    // Read encrypted file
    let encrypted = std::fs::read(keystore_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            LockBoxError::KeystoreNotFound {
                path: keystore_path.display().to_string(),
            }
        } else {
            LockBoxError::Io(e)
        }
    })?;

    // Parse Age identity
    let identity = identity_str
        .parse::<age::x25519::Identity>()
        .map_err(|_| LockBoxError::DecryptionFailed)?;

    // Decrypt using age simple API (handles armored and binary formats)
    let mut decrypted =
        age::decrypt(&identity, &encrypted).map_err(|_| LockBoxError::DecryptionFailed)?;

    // Parse TOML
    let toml_str = std::str::from_utf8(&decrypted).map_err(|_| LockBoxError::MalformedKeystore {
        reason: "decrypted payload is not valid UTF-8".to_string(),
    })?;

    let keystore: Keystore =
        toml::from_str(toml_str).map_err(|e| LockBoxError::MalformedKeystore {
            reason: e.to_string(),
        })?;

    // Zeroize decrypted bytes
    decrypted.zeroize();

    Ok(keystore)
}

/// Serialize and encrypt a keystore, writing it to disk.
///
/// Per spec S7.1: encrypt with Age x25519 recipient, write with 0o600 permissions.
pub fn encrypt_keystore(
    keystore_path: &Path,
    recipient_str: &str,
    keystore: &Keystore,
) -> Result<(), LockBoxError> {
    // Serialize to TOML
    let mut toml_str = toml::to_string_pretty(keystore).map_err(|e| LockBoxError::MalformedKeystore {
        reason: format!("serialization failed: {}", e),
    })?;

    // Parse recipient
    let recipient: age::x25519::Recipient = recipient_str
        .parse()
        .map_err(|_| LockBoxError::KeySourceUnavailable {
            reason: "invalid recipient public key".to_string(),
        })?;

    // Encrypt
    let encrypted = age::encrypt(&recipient, toml_str.as_bytes())
        .map_err(|_| LockBoxError::MalformedKeystore {
            reason: "encryption failed".to_string(),
        })?;

    // Zeroize plaintext
    toml_str.zeroize();

    // Write to disk
    std::fs::write(keystore_path, &encrypted)?;

    // Set permissions to 0o600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(keystore_path, perms)?;
    }

    Ok(())
}
