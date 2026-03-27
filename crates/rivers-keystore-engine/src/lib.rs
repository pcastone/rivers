//! Application Keystore Engine — Age-encrypted TOML key management.
//!
//! Per `rivers-feature-request-app-keystore.md`.
//!
//! Manages an Age-encrypted TOML file containing named AES-256 keys
//! with version history. Keys are generated, rotated, and deleted
//! through this crate. Key material is zeroized on drop.
//!
//! AES-256-GCM encrypt/decrypt operations are provided in Task 2.

use std::path::Path;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

// ── Errors ──────────────────────────────────────────────────────────

/// Application keystore errors.
#[derive(Debug, thiserror::Error)]
pub enum AppKeystoreError {
    #[error("keystore not found: {path}")]
    KeystoreNotFound { path: String },

    #[error("decryption failed")]
    DecryptionFailed,

    #[error("malformed keystore: {reason}")]
    MalformedKeystore { reason: String },

    #[error("key not found: '{name}'")]
    KeyNotFound { name: String },

    #[error("key '{name}' version {version} not found")]
    KeyVersionNotFound { name: String, version: u32 },

    #[error("invalid key type: expected '{expected}', got '{got}'")]
    InvalidKeyType { expected: String, got: String },

    #[error("invalid key length: expected {expected} bytes, got {got}")]
    InvalidKeyLength { expected: usize, got: usize },

    #[error("invalid nonce: {reason}")]
    InvalidNonce { reason: String },

    #[error("duplicate key: '{name}'")]
    DuplicateKey { name: String },

    #[error("keystore not configured")]
    KeystoreNotConfigured,

    #[error("keystore locked: lockbox alias '{alias}' not found")]
    KeystoreLocked { alias: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Types ───────────────────────────────────────────────────────────

/// Plaintext TOML schema inside the Age envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppKeystore {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub keys: Vec<AppKeystoreKey>,
}

/// A named encryption key with version history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppKeystoreKey {
    pub name: String,
    pub key_type: String,
    pub current_version: u32,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub versions: Vec<KeyVersion>,
}

/// A single version of a key's material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyVersion {
    pub version: u32,
    pub key_material: String,
    pub created: DateTime<Utc>,
}

/// Metadata returned by key_info() — never contains raw key bytes.
#[derive(Debug, Clone)]
pub struct KeyInfo {
    pub name: String,
    pub key_type: String,
    pub current_version: u32,
    pub version_count: usize,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

/// Result from an encrypt operation (actual encrypt/decrypt in Task 2).
#[derive(Debug, Clone)]
pub struct EncryptResult {
    pub ciphertext: String,
    pub nonce: String,
    pub key_version: u32,
}

fn default_version() -> u32 {
    1
}

// ── Zeroize ─────────────────────────────────────────────────────────

impl Zeroize for KeyVersion {
    fn zeroize(&mut self) {
        self.key_material.zeroize();
    }
}

impl Drop for KeyVersion {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl Zeroize for AppKeystoreKey {
    fn zeroize(&mut self) {
        for version in &mut self.versions {
            version.zeroize();
        }
        self.versions.clear();
    }
}

impl Drop for AppKeystoreKey {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl Zeroize for AppKeystore {
    fn zeroize(&mut self) {
        for key in &mut self.keys {
            key.zeroize();
        }
        self.keys.clear();
    }
}

impl Drop for AppKeystore {
    fn drop(&mut self) {
        self.zeroize();
    }
}

// ── Key size constants ──────────────────────────────────────────────

/// AES-256 key size in bytes.
const AES_256_KEY_SIZE: usize = 32;

/// The only supported key type.
const SUPPORTED_KEY_TYPE: &str = "aes-256";

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

// ── Key management ──────────────────────────────────────────────────

impl AppKeystore {
    /// Generate a new named key with the given type.
    ///
    /// Only `"aes-256"` is supported. Returns `InvalidKeyType` for
    /// anything else. Returns `DuplicateKey` if a key with this name
    /// already exists.
    pub fn generate_key(
        &mut self,
        name: &str,
        key_type: &str,
    ) -> Result<&AppKeystoreKey, AppKeystoreError> {
        // Validate key type
        if key_type != SUPPORTED_KEY_TYPE {
            return Err(AppKeystoreError::InvalidKeyType {
                expected: SUPPORTED_KEY_TYPE.to_string(),
                got: key_type.to_string(),
            });
        }

        // Check for duplicate
        if self.has_key(name) {
            return Err(AppKeystoreError::DuplicateKey {
                name: name.to_string(),
            });
        }

        // Generate 32 random bytes
        let mut raw_key = vec![0u8; AES_256_KEY_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut raw_key);

        let key_material = BASE64.encode(&raw_key);

        // Zeroize raw bytes
        raw_key.zeroize();

        let now = Utc::now();

        let version = KeyVersion {
            version: 1,
            key_material,
            created: now,
        };

        let key = AppKeystoreKey {
            name: name.to_string(),
            key_type: key_type.to_string(),
            current_version: 1,
            created: now,
            updated: now,
            versions: vec![version],
        };

        self.keys.push(key);
        Ok(self.keys.last().unwrap())
    }

    /// Find a key by name.
    pub fn get_key(&self, name: &str) -> Option<&AppKeystoreKey> {
        self.keys.iter().find(|k| k.name == name)
    }

    /// Find a specific version of a key.
    pub fn get_key_version(
        &self,
        name: &str,
        version: u32,
    ) -> Result<&KeyVersion, AppKeystoreError> {
        let key = self
            .get_key(name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        key.versions
            .iter()
            .find(|v| v.version == version)
            .ok_or_else(|| AppKeystoreError::KeyVersionNotFound {
                name: name.to_string(),
                version,
            })
    }

    /// Check if a key with the given name exists.
    pub fn has_key(&self, name: &str) -> bool {
        self.keys.iter().any(|k| k.name == name)
    }

    /// Return metadata for a key without exposing raw key bytes.
    pub fn key_info(&self, name: &str) -> Result<KeyInfo, AppKeystoreError> {
        let key = self
            .get_key(name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        Ok(KeyInfo {
            name: key.name.clone(),
            key_type: key.key_type.clone(),
            current_version: key.current_version,
            version_count: key.versions.len(),
            created: key.created,
            updated: key.updated,
        })
    }

    /// List metadata for all keys.
    pub fn list_keys(&self) -> Vec<KeyInfo> {
        self.keys
            .iter()
            .map(|key| KeyInfo {
                name: key.name.clone(),
                key_type: key.key_type.clone(),
                current_version: key.current_version,
                version_count: key.versions.len(),
                created: key.created,
                updated: key.updated,
            })
            .collect()
    }

    /// Rotate a key — generates a new version N+1, keeps old versions
    /// for decryption. Returns the new version number.
    pub fn rotate_key(&mut self, name: &str) -> Result<u32, AppKeystoreError> {
        let key = self
            .keys
            .iter_mut()
            .find(|k| k.name == name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        // Generate new key material
        let mut raw_key = vec![0u8; AES_256_KEY_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut raw_key);
        let key_material = BASE64.encode(&raw_key);
        raw_key.zeroize();

        let new_version = key.current_version + 1;
        let now = Utc::now();

        key.versions.push(KeyVersion {
            version: new_version,
            key_material,
            created: now,
        });

        key.current_version = new_version;
        key.updated = now;

        Ok(new_version)
    }

    /// Delete a key by name. Returns `KeyNotFound` if not found.
    pub fn delete_key(&mut self, name: &str) -> Result<(), AppKeystoreError> {
        let pos = self
            .keys
            .iter()
            .position(|k| k.name == name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        // Remove triggers Drop which triggers Zeroize
        self.keys.remove(pos);
        Ok(())
    }

    /// Decode the current version's key material into raw bytes.
    ///
    /// # Security
    /// The returned `Vec<u8>` contains raw key material. The caller **must**
    /// zeroize it after use (e.g. `bytes.zeroize()`).
    pub fn current_key_bytes(&self, name: &str) -> Result<Vec<u8>, AppKeystoreError> {
        let key = self
            .get_key(name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        let current = key
            .versions
            .iter()
            .find(|v| v.version == key.current_version)
            .ok_or_else(|| AppKeystoreError::KeyVersionNotFound {
                name: name.to_string(),
                version: key.current_version,
            })?;

        let bytes =
            BASE64
                .decode(&current.key_material)
                .map_err(|_| AppKeystoreError::MalformedKeystore {
                    reason: format!(
                        "key '{}' version {} has invalid base64 key material",
                        name, current.version
                    ),
                })?;

        if bytes.len() != AES_256_KEY_SIZE {
            return Err(AppKeystoreError::InvalidKeyLength {
                expected: AES_256_KEY_SIZE,
                got: bytes.len(),
            });
        }

        Ok(bytes)
    }

    /// Decode a specific version's key material into raw bytes.
    ///
    /// # Security
    /// The returned `Vec<u8>` contains raw key material. The caller **must**
    /// zeroize it after use (e.g. `bytes.zeroize()`).
    pub fn versioned_key_bytes(
        &self,
        name: &str,
        version: u32,
    ) -> Result<Vec<u8>, AppKeystoreError> {
        let kv = self.get_key_version(name, version)?;

        let bytes =
            BASE64
                .decode(&kv.key_material)
                .map_err(|_| AppKeystoreError::MalformedKeystore {
                    reason: format!(
                        "key '{}' version {} has invalid base64 key material",
                        name, version
                    ),
                })?;

        if bytes.len() != AES_256_KEY_SIZE {
            return Err(AppKeystoreError::InvalidKeyLength {
                expected: AES_256_KEY_SIZE,
                got: bytes.len(),
            });
        }

        Ok(bytes)
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
}
