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
    KeystoreNotFound {
        /// Filesystem path.
        path: String,
    },

    /// File permissions are not 600.
    #[error("{path} has insecure permissions (mode {mode:04o}) -- chmod 0600 {path}")]
    InsecureFilePermissions {
        /// Filesystem path.
        path: String,
        /// Actual file mode bits.
        mode: u32,
    },

    /// Age decryption failed -- wrong key or corrupted file.
    #[error("decryption failed -- check key source matches keystore")]
    DecryptionFailed,

    /// Decrypted payload is not valid TOML.
    #[error("malformed keystore: {reason}")]
    MalformedKeystore {
        /// Parse error details.
        reason: String,
    },

    /// Duplicate entry name or alias.
    #[error("\"{name}\" appears in multiple entries")]
    DuplicateEntry {
        /// The duplicate name or alias.
        name: String,
    },

    /// `lockbox://` URI references a name/alias that doesn't exist.
    #[error("\"{uri}\" referenced by datasource \"{datasource}\" -- entry not found")]
    EntryNotFound {
        /// The unresolved `lockbox://` URI.
        uri: String,
        /// Datasource that declared this reference.
        datasource: String,
    },

    /// Entry name fails naming rules.
    #[error("\"{name}\" -- must match `[a-z][a-z0-9_/.-]*` (max 128 chars)")]
    InvalidEntryName {
        /// The invalid name.
        name: String,
    },

    /// Key source is unavailable (env var missing, file unreadable, agent unreachable).
    #[error("key source unavailable: {reason}")]
    KeySourceUnavailable {
        /// Details about the unavailable key source.
        reason: String,
    },

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
///
/// `Debug` is not derived — entry values are secret material.
/// `Clone` is not derived — secret material must not be duplicated silently.
/// The manual `Debug` impl redacts entries and shows only version + count.
#[derive(Serialize, Deserialize)]
pub struct Keystore {
    /// Schema version (currently 1).
    #[serde(default = "default_version")]
    pub version: u32,
    /// Secret entries in the keystore.
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

impl std::fmt::Debug for Keystore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keystore")
            .field("version", &self.version)
            .field("entries", &format_args!("[{} entries, values redacted]", self.entries.len()))
            .finish()
    }
}

pub(crate) fn default_version() -> u32 {
    1
}

/// A single secret entry in the keystore.
///
/// Per spec S3.1. `Clone` is not derived — secret material must not be
/// duplicated silently.
#[derive(Serialize, Deserialize)]
pub struct KeystoreEntry {
    /// Registry key for this entry within the keystore. Must be unique.
    /// Used in `lockbox://` URIs to reference secrets from datasource configs.
    pub name: String,

    /// Inner secret material (plaintext inside the encrypted envelope).
    /// Zeroized on drop to prevent secret material from lingering in memory.
    pub value: String,

    /// Value type hint: `"string"`, `"base64url"`, `"pem"`, `"json"`.
    #[serde(rename = "type")]
    pub entry_type: String,

    /// Extra names that resolve to this same entry for convenience.
    #[serde(default)]
    pub aliases: Vec<String>,

    /// Records the timestamp when this entry was first added to the keystore.
    pub created: DateTime<Utc>,

    /// Signals the most recent modification timestamp for this entry.
    pub updated: DateTime<Utc>,

    // ── Credential record fields (optional, not secret) ──

    /// For driver-specific entries, indicates which driver consumes this
    /// secret (e.g. `"postgres"`, `"redis"`, `"kafka"`). Enables validation
    /// that the credential matches the expected datasource type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,

    /// Login credential — the database username associated with this secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Origin servers as `"host:port"` strings. Supports clusters
    /// (Redis, Kafka, etc.). Single-node datasources use a one-element list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,

    /// Where this credential's target database or bucket resides.
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

impl std::fmt::Debug for KeystoreEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeystoreEntry")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("entry_type", &self.entry_type)
            .field("aliases", &self.aliases)
            .field("created", &self.created)
            .field("updated", &self.updated)
            .field("driver", &self.driver)
            .field("username", &self.username)
            .field("hosts", &self.hosts)
            .field("database", &self.database)
            .finish()
    }
}

/// Supported value types for keystore entries.
///
/// Per spec S3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    /// Plain UTF-8 string (default).
    String,
    /// Base64url-encoded binary data.
    Base64Url,
    /// PEM-encoded certificate or key.
    Pem,
    /// Arbitrary JSON value.
    Json,
}

impl EntryType {
    /// Parse a type string into an `EntryType`, returning `None` for unknown values.
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
