//! LockBox — Age-encrypted local secret resolver.
//!
//! Per `rivers-lockbox-spec.md`, amended by SHAPE-5.
//!
//! Manages an Age-encrypted TOML keystore. At startup, `riversd`
//! validates entries and builds an in-memory name+alias → entry index
//! for O(1) lookup. Secret values are never held in memory — they are
//! read from disk, decrypted, used, and zeroized on every access.
//!
//! CodeComponent isolates never receive raw credentials — only opaque
//! datasource tokens. Credentials stay host-side.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

// ── Errors ──────────────────────────────────────────────────────────

/// LockBox-specific errors.
///
/// Per spec §12 — validation rules and exit codes.
#[derive(Debug, thiserror::Error)]
pub enum LockBoxError {
    /// `[lockbox]` section absent but `lockbox://` URIs present in config.
    #[error("lockbox reference found but [lockbox] is not configured — add [lockbox] section to riversd.conf")]
    ConfigMissing,

    /// Keystore file not found at configured path.
    #[error("keystore not found: {path}")]
    KeystoreNotFound { path: String },

    /// File permissions are not 600.
    #[error("{path} has insecure permissions (mode {mode:04o}) — chmod 0600 {path}")]
    InsecureFilePermissions { path: String, mode: u32 },

    /// Age decryption failed — wrong key or corrupted file.
    #[error("decryption failed — check key source matches keystore")]
    DecryptionFailed,

    /// Decrypted payload is not valid TOML.
    #[error("malformed keystore: {reason}")]
    MalformedKeystore { reason: String },

    /// Duplicate entry name or alias.
    #[error("\"{name}\" appears in multiple entries")]
    DuplicateEntry { name: String },

    /// lockbox:// URI references a name/alias that doesn't exist.
    #[error("\"{uri}\" referenced by datasource \"{datasource}\" — entry not found")]
    EntryNotFound { uri: String, datasource: String },

    /// Entry name fails naming rules.
    #[error("\"{name}\" — must match [a-z][a-z0-9_/.-]* (max 128 chars)")]
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
/// Per spec §5.
// LockBoxConfig is defined in rivers-core-config and re-exported from rivers-core::lib.
pub use rivers_core_config::LockBoxConfig;

// LockBoxConfig struct moved to rivers-core-config/src/lockbox_config.rs
// Default functions moved there too.

// ── Keystore Model ──────────────────────────────────────────────────

/// Plaintext TOML schema inside the Age envelope.
///
/// Per spec §2.2.
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

fn default_version() -> u32 {
    1
}

/// A single secret entry in the keystore.
///
/// Per spec §3.1.
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
/// Per spec §3.2.
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

// ── Name Validation ─────────────────────────────────────────────────

/// Validate an entry name against the naming rules.
///
/// Per spec §3.3:
/// - Must match `[a-z][a-z0-9_/.-]*`
/// - Maximum 128 characters
pub fn validate_entry_name(name: &str) -> Result<(), LockBoxError> {
    if name.is_empty() || name.len() > 128 {
        return Err(LockBoxError::InvalidEntryName {
            name: name.to_string(),
        });
    }

    let mut chars = name.chars();

    // First char must be lowercase letter
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => {
            return Err(LockBoxError::InvalidEntryName {
                name: name.to_string(),
            });
        }
    }

    // Remaining chars: lowercase alphanumeric + _ / . -
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '/' | '.' | '-')) {
            return Err(LockBoxError::InvalidEntryName {
                name: name.to_string(),
            });
        }
    }

    Ok(())
}

// ── URI Parsing ─────────────────────────────────────────────────────

/// Parse a `lockbox://` URI, returning the name-or-alias.
///
/// Returns `None` if the string is not a lockbox URI.
pub fn parse_lockbox_uri(uri: &str) -> Option<String> {
    uri.strip_prefix("lockbox://")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Check if a string is a lockbox:// URI.
pub fn is_lockbox_uri(s: &str) -> bool {
    s.starts_with("lockbox://")
}

// ── Resolver ────────────────────────────────────────────────────────

/// Entry metadata stored in the resolver. No secret values.
///
/// Per SHAPE-5: only name, type, and entry index live in memory.
/// Credential record fields (driver, username, hosts, database) are
/// non-secret connection routing metadata — safe to hold in memory.
#[derive(Debug, Clone)]
pub struct EntryMetadata {
    pub name: String,
    pub entry_type: EntryType,
    /// Index into the keystore entries array (for disk-based value retrieval).
    pub entry_index: usize,

    // ── Credential record metadata (non-secret) ──

    /// Driver name (e.g. "postgres", "redis", "kafka").
    pub driver: Option<String>,
    /// Database username.
    pub username: Option<String>,
    /// Host list as "host:port" strings.
    pub hosts: Vec<String>,
    /// Database or bucket name.
    pub database: Option<String>,
}

impl EntryMetadata {
    /// True if this entry carries full connection metadata (not just a password).
    pub fn is_credential_record(&self) -> bool {
        self.driver.is_some()
    }
}

/// Resolved credential fetched on demand from disk.
///
/// Values are decrypted per-access and should be zeroized after use.
#[derive(Debug, Clone)]
pub struct ResolvedEntry {
    pub name: String,
    pub value: String,
    pub entry_type: EntryType,
}

/// In-memory secret resolver. Built at startup from decrypted keystore.
///
/// Per SHAPE-5: stores name+alias → entry index only. No secret values
/// in memory. Values are read from disk, decrypted, and zeroized per access.
pub struct LockBoxResolver {
    /// Map from name-or-alias → entry metadata (no secret values).
    entries: HashMap<String, EntryMetadata>,
}

impl std::fmt::Debug for LockBoxResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockBoxResolver")
            .field("key_count", &self.entries.len())
            .finish()
    }
}

impl LockBoxResolver {
    /// Build a resolver from keystore entries.
    ///
    /// Validates:
    /// - All entry names match naming rules
    /// - No duplicate names or aliases across entries
    /// - All aliases are valid names
    ///
    /// Per SHAPE-5: only metadata is stored, not values.
    pub fn from_entries(entries: &[KeystoreEntry]) -> Result<Self, LockBoxError> {
        let mut map: HashMap<String, EntryMetadata> = HashMap::new();

        for (index, entry) in entries.iter().enumerate() {
            // Validate entry name
            validate_entry_name(&entry.name)?;

            let entry_type = EntryType::parse(&entry.entry_type).unwrap_or(EntryType::String);

            let metadata = EntryMetadata {
                name: entry.name.clone(),
                entry_type,
                entry_index: index,
                driver: entry.driver.clone(),
                username: entry.username.clone(),
                hosts: entry.hosts.clone(),
                database: entry.database.clone(),
            };

            // Insert name
            if map.contains_key(&entry.name) {
                return Err(LockBoxError::DuplicateEntry {
                    name: entry.name.clone(),
                });
            }
            map.insert(entry.name.clone(), metadata.clone());

            // Insert aliases
            for alias in &entry.aliases {
                validate_entry_name(alias)?;
                if map.contains_key(alias) {
                    return Err(LockBoxError::DuplicateEntry {
                        name: alias.clone(),
                    });
                }
                map.insert(alias.clone(), metadata.clone());
            }
        }

        Ok(Self { entries: map })
    }

    /// Resolve a name or alias to its entry metadata (no value).
    ///
    /// Per SHAPE-5: returns metadata only. Use `fetch_secret_value()`
    /// to decrypt the actual value from disk when needed.
    pub fn resolve(&self, name_or_alias: &str) -> Option<&EntryMetadata> {
        self.entries.get(name_or_alias)
    }

    /// Number of unique keys (names + aliases) in the resolver.
    pub fn key_count(&self) -> usize {
        self.entries.len()
    }

    /// Check if a name or alias exists.
    pub fn contains(&self, name_or_alias: &str) -> bool {
        self.entries.contains_key(name_or_alias)
    }

    /// List all unique entry names (not aliases).
    pub fn entry_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .entries
            .values()
            .map(|e| e.name.as_str())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        names.sort();
        names
    }
}

/// Fetch a secret value from the keystore on disk.
///
/// Per SHAPE-5: values are decrypted per-access. The caller must
/// zeroize the returned `ResolvedEntry.value` after use.
pub fn fetch_secret_value(
    metadata: &EntryMetadata,
    keystore_path: &Path,
    identity_str: &str,
) -> Result<ResolvedEntry, LockBoxError> {
    let keystore = decrypt_keystore(keystore_path, identity_str)?;

    let entry = keystore
        .entries
        .get(metadata.entry_index)
        .ok_or_else(|| LockBoxError::MalformedKeystore {
            reason: format!(
                "entry index {} out of bounds (keystore has {} entries)",
                metadata.entry_index,
                keystore.entries.len()
            ),
        })?;

    Ok(ResolvedEntry {
        name: metadata.name.clone(),
        value: entry.value.clone(),
        entry_type: metadata.entry_type,
    })
}

// ── Keystore Decryption ─────────────────────────────────────────────

/// Decrypt and parse a keystore file.
///
/// Per spec §8.1, steps 4–9.
pub fn decrypt_keystore(
    keystore_path: &Path,
    identity_str: &str,
) -> Result<Keystore, LockBoxError> {
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
/// Per spec §7.1: encrypt with Age x25519 recipient, write with 0o600 permissions.
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

// ── Key Source Resolution ───────────────────────────────────────────

/// Read the Age identity string from the configured key source.
///
/// Per spec §6.
pub fn resolve_key_source(config: &LockBoxConfig) -> Result<String, LockBoxError> {
    match config.key_source.as_str() {
        "env" => {
            let var_name = &config.key_env_var;
            let value = std::env::var(var_name).map_err(|_| LockBoxError::KeySourceUnavailable {
                reason: format!("environment variable {} is not set", var_name),
            })?;

            // NOTE: Previously called `std::env::remove_var` here, but that is
            // UB in Rust 1.77+ when other threads may read env vars. The Tokio
            // runtime is already running at this point so we cannot guarantee
            // single-threaded access. The env var remains set; callers should
            // avoid persisting secrets in environment variables in production.

            if value.is_empty() {
                return Err(LockBoxError::KeySourceUnavailable {
                    reason: format!("environment variable {} is empty", var_name),
                });
            }

            Ok(value)
        }

        "file" => {
            let key_file = config.key_file.as_deref().ok_or_else(|| {
                LockBoxError::KeySourceUnavailable {
                    reason: "key_source = \"file\" but key_file is not set".to_string(),
                }
            })?;

            // Check file permissions
            check_file_permissions(Path::new(key_file))?;

            std::fs::read_to_string(key_file).map_err(|e| LockBoxError::KeySourceUnavailable {
                reason: format!("cannot read key file {}: {}", key_file, e),
            })
        }

        "agent" => {
            // Agent support is stubbed — requires SSH agent integration
            Err(LockBoxError::KeySourceUnavailable {
                reason: "key_source = \"agent\" is not yet supported".to_string(),
            })
        }

        other => Err(LockBoxError::KeySourceUnavailable {
            reason: format!("unknown key_source: \"{}\" — must be env, file, or agent", other),
        }),
    }
}

// ── File Permission Checks ──────────────────────────────────────────

/// Check that a file has mode 600 (owner read+write only).
///
/// Per spec §6 — enforced on both .rkeystore and key_file.
#[cfg(unix)]
pub fn check_file_permissions(path: &Path) -> Result<(), LockBoxError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            LockBoxError::KeystoreNotFound {
                path: path.display().to_string(),
            }
        } else {
            LockBoxError::Io(e)
        }
    })?;

    let mode = metadata.mode() & 0o777;
    if mode != 0o600 {
        return Err(LockBoxError::InsecureFilePermissions {
            path: path.display().to_string(),
            mode,
        });
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn check_file_permissions(_path: &Path) -> Result<(), LockBoxError> {
    // File permission checks only apply on Unix
    Ok(())
}

// ── Startup Resolution ──────────────────────────────────────────────

/// Credential reference found in config — a lockbox:// URI tied to a datasource.
#[derive(Debug, Clone)]
pub struct LockBoxReference {
    /// The full lockbox:// URI.
    pub uri: String,
    /// The name-or-alias extracted from the URI.
    pub name: String,
    /// The datasource that references this credential.
    pub datasource: String,
}

/// Collect all lockbox:// references from datasource credential_source fields.
///
/// Takes a list of (datasource_name, credentials_source_value) pairs.
pub fn collect_lockbox_references(
    datasources: &[(&str, &str)],
) -> Vec<LockBoxReference> {
    datasources
        .iter()
        .filter_map(|(ds_name, cred_source)| {
            parse_lockbox_uri(cred_source).map(|name| LockBoxReference {
                uri: cred_source.to_string(),
                name,
                datasource: ds_name.to_string(),
            })
        })
        .collect()
}

/// Validate all lockbox:// references against the resolver.
///
/// Per SHAPE-5: only validates existence — no values loaded into memory.
/// Returns entry metadata keyed by datasource name.
pub fn resolve_all_references(
    resolver: &LockBoxResolver,
    references: &[LockBoxReference],
) -> Result<HashMap<String, EntryMetadata>, LockBoxError> {
    let mut resolved = HashMap::new();

    for reference in references {
        match resolver.resolve(&reference.name) {
            Some(metadata) => {
                resolved.insert(reference.datasource.clone(), metadata.clone());
            }
            None => {
                return Err(LockBoxError::EntryNotFound {
                    uri: reference.uri.clone(),
                    datasource: reference.datasource.clone(),
                });
            }
        }
    }

    Ok(resolved)
}

/// Full startup resolution sequence.
///
/// Per spec §8.1 — the 12-step startup sequence:
/// 1. Collect lockbox:// URIs (done by caller, passed as `references`)
/// 2. Check [lockbox] config present
/// 3. Check file permissions
/// 4. Resolve key source
/// 5. Decrypt keystore
/// 6. Parse TOML (done inside decrypt_keystore)
/// 7. Validate entries
/// 8. Build resolver
/// 9. Resolve all references
/// 10. Zeroize plaintext (done inside decrypt_keystore)
///
/// Returns the resolver for runtime credential access.
pub fn startup_resolve(
    config: &LockBoxConfig,
    references: &[LockBoxReference],
) -> Result<(LockBoxResolver, HashMap<String, EntryMetadata>), LockBoxError> {
    // Step: get lockbox path
    let keystore_path = config.path.as_deref().ok_or(LockBoxError::ConfigMissing)?;
    let keystore_path = Path::new(keystore_path);

    // Validate absolute path
    if !keystore_path.is_absolute() {
        return Err(LockBoxError::MalformedKeystore {
            reason: format!(
                "lockbox.path must be an absolute path, got \"{}\"",
                keystore_path.display()
            ),
        });
    }

    // Step: check file permissions on keystore
    check_file_permissions(keystore_path)?;

    // Step: resolve key source
    let mut identity_str = resolve_key_source(config)?;

    // Step: decrypt keystore
    let keystore = decrypt_keystore(keystore_path, identity_str.trim())?;

    // Zeroize identity string
    identity_str.zeroize();

    // Step: build resolver (validates entries internally)
    let resolver = LockBoxResolver::from_entries(&keystore.entries)?;

    // Step: resolve all references
    let resolved = resolve_all_references(&resolver, references)?;

    Ok((resolver, resolved))
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::ExposeSecret;

    /// Helper: build a KeystoreEntry with sensible defaults.
    fn make_entry(name: &str, value: &str, entry_type: &str, aliases: &[&str]) -> KeystoreEntry {
        KeystoreEntry {
            name: name.to_string(),
            value: value.to_string(),
            entry_type: entry_type.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
            driver: None,
            username: None,
            hosts: vec![],
            database: None,
        }
    }

    /// Helper: generate an Age keypair, returning (identity_string, recipient_string).
    fn generate_keypair() -> (String, String) {
        let identity = age::x25519::Identity::generate();
        let identity_str = identity.to_string().expose_secret().to_string();
        let recipient_str = identity.to_public().to_string();
        (identity_str, recipient_str)
    }

    // ── Encrypt/Decrypt Round Trip ───────────────────────────────────

    #[test]
    fn create_and_load_keystore_round_trip() {
        let (identity_str, recipient_str) = generate_keypair();

        let keystore = Keystore {
            version: 1,
            entries: vec![
                make_entry("postgres/prod", "pg://user:pass@host/db", "string", &["pg-prod"]),
                make_entry("redis/cache", "redis://secret", "string", &[]),
                make_entry("jwt-signing", "base64-encoded-key-data", "base64url", &["jwt"]),
            ],
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.rkeystore");

        // Encrypt to disk
        encrypt_keystore(&path, &recipient_str, &keystore).unwrap();
        assert!(path.exists());

        // Decrypt from disk
        let decrypted = decrypt_keystore(&path, identity_str.trim()).unwrap();

        // Verify round-trip fidelity
        assert_eq!(decrypted.version, 1);
        assert_eq!(decrypted.entries.len(), 3);
        assert_eq!(decrypted.entries[0].name, "postgres/prod");
        assert_eq!(decrypted.entries[0].value, "pg://user:pass@host/db");
        assert_eq!(decrypted.entries[0].aliases, vec!["pg-prod"]);
        assert_eq!(decrypted.entries[1].name, "redis/cache");
        assert_eq!(decrypted.entries[1].value, "redis://secret");
        assert_eq!(decrypted.entries[2].name, "jwt-signing");
        assert_eq!(decrypted.entries[2].value, "base64-encoded-key-data");
        assert_eq!(decrypted.entries[2].entry_type, "base64url");
    }

    // ── Entry Lookup by Name ─────────────────────────────────────────

    #[test]
    fn entry_lookup_by_name() {
        let entries = vec![
            make_entry("postgres/prod", "secret1", "string", &[]),
            make_entry("redis/prod", "secret2", "string", &[]),
            make_entry("kafka/prod", "secret3", "string", &[]),
        ];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        let meta = resolver.resolve("redis/prod").unwrap();
        assert_eq!(meta.name, "redis/prod");
        assert_eq!(meta.entry_index, 1);

        let meta = resolver.resolve("kafka/prod").unwrap();
        assert_eq!(meta.name, "kafka/prod");
        assert_eq!(meta.entry_index, 2);

        assert!(resolver.resolve("missing").is_none());
    }

    // ── Alias Resolution ─────────────────────────────────────────────

    #[test]
    fn alias_resolution() {
        let entries = vec![
            make_entry("postgres/orders-prod", "pg://secret", "string", &["db/orders", "orders-db"]),
            make_entry("redis/sessions", "redis://secret", "string", &["cache"]),
        ];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        // Resolve via alias
        let meta = resolver.resolve("db/orders").unwrap();
        assert_eq!(meta.name, "postgres/orders-prod");
        assert_eq!(meta.entry_index, 0);

        let meta = resolver.resolve("orders-db").unwrap();
        assert_eq!(meta.name, "postgres/orders-prod");
        assert_eq!(meta.entry_index, 0);

        let meta = resolver.resolve("cache").unwrap();
        assert_eq!(meta.name, "redis/sessions");
        assert_eq!(meta.entry_index, 1);

        // Canonical name still works
        let meta = resolver.resolve("postgres/orders-prod").unwrap();
        assert_eq!(meta.name, "postgres/orders-prod");
    }

    // ── Duplicate Name Detected ──────────────────────────────────────

    #[test]
    fn duplicate_name_detected() {
        let entries = vec![
            make_entry("postgres/prod", "value1", "string", &[]),
            make_entry("postgres/prod", "value2", "string", &[]),
        ];
        let result = LockBoxResolver::from_entries(&entries);
        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::DuplicateEntry { name } => {
                assert_eq!(name, "postgres/prod");
            }
            other => panic!("expected DuplicateEntry, got: {:?}", other),
        }
    }

    // ── Duplicate Alias Detected ─────────────────────────────────────

    #[test]
    fn duplicate_alias_detected() {
        // Alias on second entry collides with name of first entry
        let entries = vec![
            make_entry("postgres/prod", "value1", "string", &[]),
            make_entry("redis/prod", "value2", "string", &["postgres/prod"]),
        ];
        let result = LockBoxResolver::from_entries(&entries);
        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::DuplicateEntry { name } => {
                assert_eq!(name, "postgres/prod");
            }
            other => panic!("expected DuplicateEntry, got: {:?}", other),
        }
    }

    #[test]
    fn duplicate_alias_across_entries() {
        // Two entries share the same alias
        let entries = vec![
            make_entry("postgres/prod", "value1", "string", &["shared"]),
            make_entry("redis/prod", "value2", "string", &["shared"]),
        ];
        let result = LockBoxResolver::from_entries(&entries);
        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::DuplicateEntry { name } => {
                assert_eq!(name, "shared");
            }
            other => panic!("expected DuplicateEntry, got: {:?}", other),
        }
    }

    // ── Invalid Entry Name Rejected ──────────────────────────────────

    #[test]
    fn invalid_entry_name_rejected() {
        // Uppercase
        assert!(validate_entry_name("PostgresKey").is_err());
        // Starts with digit
        assert!(validate_entry_name("1password").is_err());
        // Starts with underscore
        assert!(validate_entry_name("_hidden").is_err());
        // Empty
        assert!(validate_entry_name("").is_err());
        // Too long (129 chars)
        assert!(validate_entry_name(&"a".repeat(129)).is_err());
        // Special characters
        assert!(validate_entry_name("key@host").is_err());
        assert!(validate_entry_name("key space").is_err());
        assert!(validate_entry_name("key=value").is_err());
        // Unicode
        assert!(validate_entry_name("key\u{00e9}").is_err());
    }

    #[test]
    fn valid_entry_names_accepted() {
        assert!(validate_entry_name("a").is_ok());
        assert!(validate_entry_name("postgres/orders-prod").is_ok());
        assert!(validate_entry_name("db/orders").is_ok());
        assert!(validate_entry_name("anthropic/api_key").is_ok());
        assert!(validate_entry_name("jwt_signing_key").is_ok());
        assert!(validate_entry_name("my.service.key").is_ok());
        assert!(validate_entry_name("key-with-dashes").is_ok());
        // Max length (128 chars)
        assert!(validate_entry_name(&"a".repeat(128)).is_ok());
    }

    #[test]
    fn invalid_entry_name_in_resolver() {
        let entries = vec![make_entry("INVALID", "value", "string", &[])];
        let result = LockBoxResolver::from_entries(&entries);
        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::InvalidEntryName { name } => {
                assert_eq!(name, "INVALID");
            }
            other => panic!("expected InvalidEntryName, got: {:?}", other),
        }
    }

    #[test]
    fn invalid_alias_name_in_resolver() {
        let entries = vec![make_entry("valid-name", "value", "string", &["BAD_ALIAS"])];
        let result = LockBoxResolver::from_entries(&entries);
        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::InvalidEntryName { name } => {
                assert_eq!(name, "BAD_ALIAS");
            }
            other => panic!("expected InvalidEntryName, got: {:?}", other),
        }
    }

    // ── Resolver Metadata Only ───────────────────────────────────────

    #[test]
    fn resolver_metadata_only() {
        let entries = vec![
            make_entry("postgres/prod", "super-secret-pg-password", "string", &["pg"]),
        ];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        let meta = resolver.resolve("postgres/prod").unwrap();

        // EntryMetadata has name, entry_type, entry_index — but NO value field.
        // This is enforced at compile time by the struct definition.
        assert_eq!(meta.name, "postgres/prod");
        assert_eq!(meta.entry_type, EntryType::String);
        assert_eq!(meta.entry_index, 0);

        // Alias resolves to same metadata
        let alias_meta = resolver.resolve("pg").unwrap();
        assert_eq!(alias_meta.name, "postgres/prod");
        assert_eq!(alias_meta.entry_index, 0);

        // Key count = 1 name + 1 alias = 2
        assert_eq!(resolver.key_count(), 2);

        // entry_names() returns only canonical names
        assert_eq!(resolver.entry_names(), vec!["postgres/prod"]);
    }

    // ── Wrong Key Decryption Fails ───────────────────────────────────

    #[test]
    fn wrong_key_decryption_fails() {
        let (_, recipient_str) = generate_keypair();
        let (wrong_identity_str, _) = generate_keypair();

        let keystore = Keystore {
            version: 1,
            entries: vec![make_entry("secret", "top-secret-value", "string", &[])],
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.rkeystore");

        // Encrypt with first keypair
        encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

        // Attempt to decrypt with second (wrong) keypair
        let result = decrypt_keystore(&path, wrong_identity_str.trim());
        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::DecryptionFailed => {} // expected
            other => panic!("expected DecryptionFailed, got: {:?}", other),
        }
    }

    // ── Entry Type Validation ────────────────────────────────────────

    #[test]
    fn entry_type_validation() {
        assert_eq!(EntryType::parse("string"), Some(EntryType::String));
        assert_eq!(EntryType::parse("base64url"), Some(EntryType::Base64Url));
        assert_eq!(EntryType::parse("pem"), Some(EntryType::Pem));
        assert_eq!(EntryType::parse("json"), Some(EntryType::Json));
        assert_eq!(EntryType::parse("unknown"), None);
        assert_eq!(EntryType::parse(""), None);
        assert_eq!(EntryType::parse("String"), None); // case-sensitive
    }

    #[test]
    fn entry_types_stored_correctly_in_resolver() {
        let entries = vec![
            make_entry("key-string", "val", "string", &[]),
            make_entry("key-b64", "val", "base64url", &[]),
            make_entry("key-pem", "val", "pem", &[]),
            make_entry("key-json", "val", "json", &[]),
        ];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        assert_eq!(resolver.resolve("key-string").unwrap().entry_type, EntryType::String);
        assert_eq!(resolver.resolve("key-b64").unwrap().entry_type, EntryType::Base64Url);
        assert_eq!(resolver.resolve("key-pem").unwrap().entry_type, EntryType::Pem);
        assert_eq!(resolver.resolve("key-json").unwrap().entry_type, EntryType::Json);
    }

    // ── File Permissions Enforced ────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn file_permissions_enforced() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.rkeystore");
        std::fs::write(&path, b"dummy").unwrap();

        // Set insecure permissions (0o644)
        let perms = std::fs::Permissions::from_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();

        let result = check_file_permissions(&path);
        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::InsecureFilePermissions { mode, .. } => {
                assert_eq!(mode, 0o644);
            }
            other => panic!("expected InsecureFilePermissions, got: {:?}", other),
        }

        // Fix permissions to 0o600 — should pass
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms).unwrap();
        assert!(check_file_permissions(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn encrypt_keystore_sets_permissions() {
        use std::os::unix::fs::MetadataExt;

        let (_, recipient_str) = generate_keypair();
        let keystore = Keystore {
            version: 1,
            entries: vec![make_entry("test", "value", "string", &[])],
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.rkeystore");

        encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

        let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600, "encrypt_keystore must set 0o600 permissions");
    }

    // ── fetch_secret_value Round Trip ────────────────────────────────

    #[test]
    fn fetch_secret_value_round_trip() {
        let (identity_str, recipient_str) = generate_keypair();

        let keystore = Keystore {
            version: 1,
            entries: vec![
                make_entry("first", "value-one", "string", &[]),
                make_entry("second", "value-two", "base64url", &["alias-second"]),
                make_entry("third", "value-three", "pem", &[]),
            ],
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.rkeystore");
        encrypt_keystore(&path, &recipient_str, &keystore).unwrap();

        let resolver = LockBoxResolver::from_entries(&keystore.entries).unwrap();

        // Fetch first entry by name
        let meta = resolver.resolve("first").unwrap();
        let resolved = fetch_secret_value(meta, &path, identity_str.trim()).unwrap();
        assert_eq!(resolved.name, "first");
        assert_eq!(resolved.value, "value-one");
        assert_eq!(resolved.entry_type, EntryType::String);

        // Fetch second entry by alias
        let meta = resolver.resolve("alias-second").unwrap();
        let resolved = fetch_secret_value(meta, &path, identity_str.trim()).unwrap();
        assert_eq!(resolved.name, "second");
        assert_eq!(resolved.value, "value-two");
        assert_eq!(resolved.entry_type, EntryType::Base64Url);

        // Fetch third entry by name
        let meta = resolver.resolve("third").unwrap();
        let resolved = fetch_secret_value(meta, &path, identity_str.trim()).unwrap();
        assert_eq!(resolved.name, "third");
        assert_eq!(resolved.value, "value-three");
        assert_eq!(resolved.entry_type, EntryType::Pem);
    }

    // ── URI Parsing ──────────────────────────────────────────────────

    #[test]
    fn uri_parsing() {
        assert_eq!(
            parse_lockbox_uri("lockbox://postgres/prod"),
            Some("postgres/prod".to_string())
        );
        assert_eq!(
            parse_lockbox_uri("lockbox://simple"),
            Some("simple".to_string())
        );
        assert_eq!(parse_lockbox_uri("lockbox://"), None);
        assert_eq!(parse_lockbox_uri("env://DB_PASSWORD"), None);
        assert_eq!(parse_lockbox_uri("plain-string"), None);

        assert!(is_lockbox_uri("lockbox://test"));
        assert!(!is_lockbox_uri("env://test"));
        assert!(!is_lockbox_uri(""));
    }

    // ── Lockbox Reference Collection ─────────────────────────────────

    #[test]
    fn collect_and_resolve_references() {
        let entries = vec![
            make_entry("postgres/prod", "pg://secret", "string", &["pg-prod"]),
            make_entry("redis/prod", "redis://secret", "string", &["cache"]),
        ];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        let datasources = vec![
            ("primary_db", "lockbox://postgres/prod"),
            ("cache_store", "lockbox://cache"),
            ("contacts", "none"),
        ];
        let refs = collect_lockbox_references(&datasources);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].name, "postgres/prod");
        assert_eq!(refs[0].datasource, "primary_db");
        assert_eq!(refs[1].name, "cache");
        assert_eq!(refs[1].datasource, "cache_store");

        let resolved = resolve_all_references(&resolver, &refs).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved["primary_db"].name, "postgres/prod");
        assert_eq!(resolved["primary_db"].entry_index, 0);
        assert_eq!(resolved["cache_store"].name, "redis/prod");
        assert_eq!(resolved["cache_store"].entry_index, 1);
    }

    #[test]
    fn resolve_references_fails_on_missing_entry() {
        let entries = vec![make_entry("postgres/prod", "secret", "string", &[])];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        let datasources = vec![("db", "lockbox://missing")];
        let refs = collect_lockbox_references(&datasources);
        let result = resolve_all_references(&resolver, &refs);

        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::EntryNotFound { uri, datasource } => {
                assert_eq!(uri, "lockbox://missing");
                assert_eq!(datasource, "db");
            }
            other => panic!("expected EntryNotFound, got: {:?}", other),
        }
    }

    // ── Keystore TOML Serialization ──────────────────────────────────

    #[test]
    fn keystore_toml_round_trip() {
        let keystore = Keystore {
            version: 1,
            entries: vec![
                make_entry("postgres/prod", "pg://user:pass@host/db", "string", &["pg"]),
                make_entry("api-key", "sk-test-12345", "string", &[]),
            ],
        };

        let toml_str = toml::to_string_pretty(&keystore).unwrap();
        let parsed: Keystore = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].name, "postgres/prod");
        assert_eq!(parsed.entries[0].value, "pg://user:pass@host/db");
        assert_eq!(parsed.entries[0].aliases, vec!["pg"]);
        assert_eq!(parsed.entries[1].name, "api-key");
        assert_eq!(parsed.entries[1].value, "sk-test-12345");
    }

    // ── Credential Record Metadata ───────────────────────────────────

    #[test]
    fn credential_record_metadata() {
        let mut entry = make_entry("postgres/prod", "secret", "string", &[]);
        entry.driver = Some("postgres".to_string());
        entry.username = Some("app_user".to_string());
        entry.hosts = vec!["db.example.com:5432".to_string()];
        entry.database = Some("orders".to_string());

        let entries = vec![entry];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        let meta = resolver.resolve("postgres/prod").unwrap();
        assert!(meta.is_credential_record());
        assert_eq!(meta.driver, Some("postgres".to_string()));
        assert_eq!(meta.username, Some("app_user".to_string()));
        assert_eq!(meta.hosts, vec!["db.example.com:5432"]);
        assert_eq!(meta.database, Some("orders".to_string()));
    }

    #[test]
    fn non_credential_record_metadata() {
        let entries = vec![make_entry("api-key", "secret", "string", &[])];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        let meta = resolver.resolve("api-key").unwrap();
        assert!(!meta.is_credential_record());
        assert_eq!(meta.driver, None);
        assert_eq!(meta.username, None);
        assert!(meta.hosts.is_empty());
        assert_eq!(meta.database, None);
    }

    // ── Keystore Not Found ───────────────────────────────────────────

    #[test]
    fn decrypt_keystore_not_found() {
        let (identity_str, _) = generate_keypair();
        let result = decrypt_keystore(Path::new("/nonexistent/path.rkeystore"), &identity_str);
        assert!(result.is_err());
        match result.unwrap_err() {
            LockBoxError::KeystoreNotFound { path } => {
                assert_eq!(path, "/nonexistent/path.rkeystore");
            }
            other => panic!("expected KeystoreNotFound, got: {:?}", other),
        }
    }

    // ── Resolver contains() ──────────────────────────────────────────

    #[test]
    fn resolver_contains() {
        let entries = vec![make_entry("postgres/prod", "secret", "string", &["pg"])];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();

        assert!(resolver.contains("postgres/prod"));
        assert!(resolver.contains("pg"));
        assert!(!resolver.contains("missing"));
    }

    // ── Keystore Default Version ─────────────────────────────────────

    #[test]
    fn keystore_default_version() {
        let toml_str = r#"
[[entries]]
name    = "test"
value   = "secret"
type    = "string"
created = "2026-01-01T00:00:00Z"
updated = "2026-01-01T00:00:00Z"
"#;
        let keystore: Keystore = toml::from_str(toml_str).unwrap();
        assert_eq!(keystore.version, 1); // default_version()
        assert_eq!(keystore.entries.len(), 1);
    }

    // ── Unknown Entry Type Defaults to String ────────────────────────

    #[test]
    fn unknown_entry_type_defaults_to_string() {
        let entries = vec![make_entry("test", "value", "custom_type", &[])];
        let resolver = LockBoxResolver::from_entries(&entries).unwrap();
        let meta = resolver.resolve("test").unwrap();
        // Unknown types default to String via unwrap_or
        assert_eq!(meta.entry_type, EntryType::String);
    }

    // ── Empty Keystore ───────────────────────────────────────────────

    #[test]
    fn empty_keystore_round_trip() {
        let (identity_str, recipient_str) = generate_keypair();

        let keystore = Keystore {
            version: 1,
            entries: vec![],
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.rkeystore");

        encrypt_keystore(&path, &recipient_str, &keystore).unwrap();
        let decrypted = decrypt_keystore(&path, identity_str.trim()).unwrap();

        assert_eq!(decrypted.version, 1);
        assert!(decrypted.entries.is_empty());

        // Resolver from empty entries
        let resolver = LockBoxResolver::from_entries(&decrypted.entries).unwrap();
        assert_eq!(resolver.key_count(), 0);
        assert!(resolver.entry_names().is_empty());
    }
}
