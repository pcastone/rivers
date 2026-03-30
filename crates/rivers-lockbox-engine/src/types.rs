//! LockBox type definitions — errors, keystore model, entry types.
//!
//! Security-sensitive structs carry `Zeroize` + `Drop` impls to prevent
//! secret material from lingering in memory.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

// ── Errors ──────────────────────────────────────────────────────────

/// LockBox-specific errors.
///
/// Per spec S12 -- validation rules and exit codes.
#[derive(Debug, thiserror::Error)]
pub enum LockBoxError {
    /// `[lockbox]` section absent but `lockbox://` URIs present in config.
    #[error("lockbox reference found but [lockbox] is not configured -- add [lockbox] section to riversd.conf")]
    ConfigMissing,

    /// Keystore file not found at configured path.
    #[error("keystore not found: {path}")]
    KeystoreNotFound { path: String },

    /// File permissions are not 600.
    #[error("{path} has insecure permissions (mode {mode:04o}) -- chmod 0600 {path}")]
    InsecureFilePermissions { path: String, mode: u32 },

    /// Age decryption failed -- wrong key or corrupted file.
    #[error("decryption failed -- check key source matches keystore")]
    DecryptionFailed,

    /// Decrypted payload is not valid TOML.
    #[error("malformed keystore: {reason}")]
    MalformedKeystore { reason: String },

    /// Duplicate entry name or alias.
    #[error("\"{name}\" appears in multiple entries")]
    DuplicateEntry { name: String },

    /// lockbox:// URI references a name/alias that doesn't exist.
    #[error("\"{uri}\" referenced by datasource \"{datasource}\" -- entry not found")]
    EntryNotFound { uri: String, datasource: String },

    /// Entry name fails naming rules.
    #[error("\"{name}\" -- must match [a-z][a-z0-9_/.-]* (max 128 chars)")]
    InvalidEntryName { name: String },

    /// Key source is unavailable (env var missing, file unreadable, agent unreachable).
    #[error("key source unavailable: {reason}")]
    KeySourceUnavailable { reason: String },

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Config ──────────────────────────────────────────────────────────

/// `[lockbox]` section in `riversd.conf`.
///
/// Per spec S5.
// LockBoxConfig is defined in rivers-core-config and re-exported from rivers-core::lib.
pub use rivers_core_config::LockBoxConfig;

// ── Keystore Model ──────────────────────────────────────────────────

/// Plaintext TOML schema inside the Age envelope.
///
/// Per spec S2.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keystore {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<KeystoreEntry>,
}

impl Zeroize for Keystore {
    fn zeroize(&mut self) {
        for entry in &mut self.entries {
            entry.zeroize();
        }
        self.entries.clear();
    }
}

impl Drop for Keystore {
    fn drop(&mut self) {
        self.zeroize();
    }
}

pub(crate) fn default_version() -> u32 {
    1
}

/// A single secret entry in the keystore.
///
/// Per spec S3.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeystoreEntry {
    /// Canonical name. Unique within keystore. Used in lockbox:// URIs.
    pub name: String,

    /// The secret value (plaintext inside the encrypted envelope).
    /// Zeroized on drop to prevent secret material from lingering in memory.
    pub value: String,

    /// Value type hint: "string", "base64url", "pem", "json".
    #[serde(rename = "type")]
    pub entry_type: String,

    /// Alternative names that resolve to this entry.
    #[serde(default)]
    pub aliases: Vec<String>,

    /// Creation timestamp.
    pub created: DateTime<Utc>,

    /// Last update timestamp.
    pub updated: DateTime<Utc>,

    // ── Credential record fields (optional, not secret) ──

    /// Driver name (e.g. "postgres", "redis", "kafka"). Enables validation
    /// that the credential matches the expected datasource type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,

    /// Database username.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Host list as "host:port" strings. Supports clusters (Redis, Kafka, etc.).
    /// Single-node datasources use a one-element list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,

    /// Database or bucket name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

impl Zeroize for KeystoreEntry {
    fn zeroize(&mut self) {
        self.value.zeroize();
    }
}

impl Drop for KeystoreEntry {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Supported value types for keystore entries.
///
/// Per spec S3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    String,
    Base64Url,
    Pem,
    Json,
}

impl EntryType {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "string" => Some(EntryType::String),
            "base64url" => Some(EntryType::Base64Url),
            "pem" => Some(EntryType::Pem),
            "json" => Some(EntryType::Json),
            _ => None,
        }
    }
}
