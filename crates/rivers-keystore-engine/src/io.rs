//! File I/O — create, load, save keystore files with Age encryption.

use std::path::Path;

use zeroize::Zeroize;

use crate::types::*;

// ── File operations ─────────────────────────────────────────────────

impl AppKeystore {
    /// Create a new empty keystore file at `path`, encrypted with the
    /// given Age recipient public key string.
    pub fn create(path: &Path, recipient_key: &str) -> Result<(), AppKeystoreError> {
        let keystore = AppKeystore {
            version: 1,
            keys: Vec::new(),
        };
        keystore.save(path, recipient_key)
    }

    /// Load and decrypt a keystore file from `path` using the given
    /// Age identity (private key) string.
    pub fn load(path: &Path, identity_str: &str) -> Result<AppKeystore, AppKeystoreError> {
        // Read encrypted file
        let encrypted = std::fs::read(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppKeystoreError::KeystoreNotFound {
                    path: path.display().to_string(),
                }
            } else {
                AppKeystoreError::Io(e)
            }
        })?;

        // Parse Age identity
        let identity = identity_str
            .parse::<age::x25519::Identity>()
            .map_err(|_| AppKeystoreError::DecryptionFailed)?;

        // Decrypt
        let mut decrypted =
            age::decrypt(&identity, &encrypted).map_err(|_| AppKeystoreError::DecryptionFailed)?;

        // Parse TOML
        let toml_str =
            std::str::from_utf8(&decrypted).map_err(|_| AppKeystoreError::MalformedKeystore {
                reason: "decrypted payload is not valid UTF-8".to_string(),
            })?;

        let keystore: AppKeystore =
            toml::from_str(toml_str).map_err(|e| AppKeystoreError::MalformedKeystore {
                reason: e.to_string(),
            })?;

        // Zeroize decrypted bytes
        decrypted.zeroize();

        Ok(keystore)
    }

    /// Serialize and encrypt this keystore, writing it to `path`.
    pub fn save(&self, path: &Path, recipient_key: &str) -> Result<(), AppKeystoreError> {
        // Serialize to TOML
        let mut toml_str =
            toml::to_string_pretty(self).map_err(|e| AppKeystoreError::MalformedKeystore {
                reason: format!("serialization failed: {}", e),
            })?;

        // Parse recipient
        let recipient: age::x25519::Recipient =
            recipient_key
                .parse()
                .map_err(|_| AppKeystoreError::MalformedKeystore {
                    reason: "invalid recipient public key".to_string(),
                })?;

        // Encrypt
        let encrypted =
            age::encrypt(&recipient, toml_str.as_bytes()).map_err(|_| {
                AppKeystoreError::MalformedKeystore {
                    reason: "encryption failed".to_string(),
                }
            })?;

        // Zeroize plaintext
        toml_str.zeroize();

        // Atomic write: tempfile + rename to avoid torn keystore on crash
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        std::io::Write::write_all(&mut tmp, &encrypted)?;

        // Set permissions to 0o600 on Unix before rename
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(tmp.path(), perms)?;
        }

        tmp.persist(path).map_err(|e| AppKeystoreError::Io(e.error))?;

        Ok(())
    }
}
