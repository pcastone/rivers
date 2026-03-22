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
#[derive(Debug, Clone)]
pub struct EntryMetadata {
    pub name: String,
    pub entry_type: EntryType,
    /// Index into the keystore entries array (for disk-based value retrieval).
    pub entry_index: usize,
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
