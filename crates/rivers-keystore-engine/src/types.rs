//! Core types, error enum, constants, and Zeroize/Drop impls.

use chrono::{DateTime, Utc};
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

/// Result from an encrypt operation.
#[derive(Debug, Clone)]
pub struct EncryptResult {
    pub ciphertext: String,
    pub nonce: String,
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
