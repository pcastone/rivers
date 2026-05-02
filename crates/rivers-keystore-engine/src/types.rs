//! Core types, error enum, constants, and Zeroize/Drop impls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

// ── Errors ──────────────────────────────────────────────────────────

/// Application keystore errors.
#[derive(Debug, thiserror::Error)]
pub enum AppKeystoreError {
    /// Keystore file not found at configured path.
    #[error("keystore not found: {path}")]
    KeystoreNotFound {
        /// Filesystem path.
        path: String,
    },

    /// Age decryption or AES-GCM authentication failed.
    #[error("decryption failed")]
    DecryptionFailed,

    /// Decrypted payload is not valid TOML or contains invalid data.
    #[error("malformed keystore: {reason}")]
    MalformedKeystore {
        /// Parse or validation error details.
        reason: String,
    },

    /// Named key does not exist in the keystore.
    #[error("key not found: '{name}'")]
    KeyNotFound {
        /// Key name.
        name: String,
    },

    /// Requested version does not exist for a key.
    #[error("key '{name}' version {version} not found")]
    KeyVersionNotFound {
        /// Key name.
        name: String,
        /// Requested version number.
        version: u32,
    },

    /// Key type is not supported (only `"aes-256"` is valid).
    #[error("invalid key type: expected '{expected}', got '{got}'")]
    InvalidKeyType {
        /// The supported key type.
        expected: String,
        /// The unsupported type that was provided.
        got: String,
    },

    /// Raw key material is not the expected length.
    #[error("invalid key length: expected {expected} bytes, got {got}")]
    InvalidKeyLength {
        /// Expected length in bytes.
        expected: usize,
        /// Actual length in bytes.
        got: usize,
    },

    /// Nonce is malformed or wrong length.
    #[error("invalid nonce: {reason}")]
    InvalidNonce {
        /// Details about the nonce error.
        reason: String,
    },

    /// A key with this name already exists.
    #[error("duplicate key: '{name}'")]
    DuplicateKey {
        /// The duplicate key name.
        name: String,
    },

    /// No keystore has been configured for this application.
    #[error("keystore not configured")]
    KeystoreNotConfigured,

    /// Keystore master key could not be resolved from LockBox.
    #[error("keystore locked: lockbox alias '{alias}' not found")]
    KeystoreLocked {
        /// The LockBox alias that was not found.
        alias: String,
    },

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Types ───────────────────────────────────────────────────────────

/// Plaintext TOML schema inside the Age envelope.
///
/// `Debug` is not derived — this type contains fields that chain to
/// `KeyVersion.key_material` which must never appear in logs or debug output.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppKeystore {
    /// Schema version (currently 1).
    #[serde(default = "default_version")]
    pub version: u32,
    /// Named encryption keys with version history.
    #[serde(default)]
    pub keys: Vec<AppKeystoreKey>,
}

impl std::fmt::Debug for AppKeystore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppKeystore")
            .field("version", &self.version)
            .field("key_count", &self.keys.len())
            .finish()
    }
}

/// A named encryption key with version history.
///
/// `Debug` is not derived — `versions` chains to `key_material` which must
/// never appear in logs or debug output.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppKeystoreKey {
    /// Unique key name within the keystore.
    pub name: String,
    /// Key algorithm (always `"aes-256"`).
    pub key_type: String,
    /// Active version number used for new encryptions.
    pub current_version: u32,
    /// When this key was first generated.
    pub created: DateTime<Utc>,
    /// When this key was last rotated or modified.
    pub updated: DateTime<Utc>,
    /// All versions of this key (old versions kept for decryption).
    pub versions: Vec<KeyVersion>,
}

impl std::fmt::Debug for AppKeystoreKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppKeystoreKey")
            .field("name", &self.name)
            .field("key_type", &self.key_type)
            .field("current_version", &self.current_version)
            .field("version_count", &self.versions.len())
            .field("created", &self.created)
            .field("updated", &self.updated)
            .finish()
    }
}

/// A single version of a key's material.
///
/// `Debug` is not derived — `key_material` must never appear in logs or
/// debug output. Use `KeyInfo` to inspect metadata without raw key bytes.
#[derive(Clone, Serialize, Deserialize)]
pub struct KeyVersion {
    /// Version number (1-based, monotonically increasing).
    pub version: u32,
    /// Base64-encoded AES-256 key material (32 bytes). Zeroized on drop.
    ///
    /// # Security
    /// Use `AppKeystore::current_key_bytes()` or `versioned_key_bytes()` rather
    /// than accessing this field directly — those methods decode into raw bytes
    /// with an explicit caller obligation to zeroize after use.
    pub(crate) key_material: String,
    /// When this version was generated.
    pub created: DateTime<Utc>,
}

impl std::fmt::Debug for KeyVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyVersion")
            .field("version", &self.version)
            .field("key_material", &"<redacted>")
            .field("created", &self.created)
            .finish()
    }
}

/// Metadata returned by key_info() — never contains raw key bytes.
#[derive(Debug, Clone)]
pub struct KeyInfo {
    /// Key name.
    pub name: String,
    /// Key algorithm (e.g. `"aes-256"`).
    pub key_type: String,
    /// Active version number.
    pub current_version: u32,
    /// Total number of versions (including old rotated versions).
    pub version_count: usize,
    /// When this key was first generated.
    pub created: DateTime<Utc>,
    /// When this key was last rotated or modified.
    pub updated: DateTime<Utc>,
}

/// Result from an encrypt operation.
#[derive(Debug, Clone)]
pub struct EncryptResult {
    /// Base64-encoded ciphertext (AES-256-GCM output including auth tag).
    pub ciphertext: String,
    /// Base64-encoded 96-bit nonce used for this encryption.
    pub nonce: String,
    /// Key version used for encryption (0 for standalone, N for keystore).
    pub key_version: u32,
}

pub(crate) fn default_version() -> u32 {
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
pub(crate) const AES_256_KEY_SIZE: usize = 32;

/// AES-GCM nonce size in bytes (96-bit).
pub(crate) const AES_GCM_NONCE_SIZE: usize = 12;

/// The only supported key type.
pub(crate) const SUPPORTED_KEY_TYPE: &str = "aes-256";
