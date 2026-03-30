//! In-memory secret resolver. Built at startup from decrypted keystore.
//!
//! Per SHAPE-5: stores name+alias -> entry index only. No secret values
//! in memory. Values are read from disk, decrypted, and zeroized per access.

use std::collections::HashMap;
use std::path::Path;

use crate::crypto::decrypt_keystore;
use crate::types::*;
use crate::validation::validate_entry_name;

// ── Resolver ────────────────────────────────────────────────────────

/// Entry metadata stored in the resolver. No secret values.
///
/// Per SHAPE-5: only name, type, and entry index live in memory.
/// Credential record fields (driver, username, hosts, database) are
/// non-secret connection routing metadata -- safe to hold in memory.
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
/// Per SHAPE-5: stores name+alias -> entry index only. No secret values
/// in memory. Values are read from disk, decrypted, and zeroized per access.
pub struct LockBoxResolver {
    /// Map from name-or-alias -> entry metadata (no secret values).
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
