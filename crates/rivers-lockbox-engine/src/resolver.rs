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
    /// Entry name (primary key in the keystore).
    pub name: String,
    /// Value type hint for deserialization.
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
/// Values are decrypted per-access. The `value` field is wrapped in
/// `zeroize::Zeroizing` so it is automatically zeroed on drop.
///
/// `Debug` is manually implemented to redact the value; `Clone` is not
/// implemented — cloning secret material requires an explicit call to
/// `.clone_value()` to make the intent visible in code review.
pub struct ResolvedEntry {
    /// Entry name.
    pub name: String,
    /// Decrypted secret value. Zeroized on drop.
    ///
    /// Access the inner string with `.expose_secret()` (from the `zeroize`
    /// crate's `Zeroizing` wrapper via `Deref`).
    pub value: zeroize::Zeroizing<String>,
    /// Value type hint.
    pub entry_type: EntryType,
}

impl std::fmt::Debug for ResolvedEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedEntry")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("entry_type", &self.entry_type)
            .finish()
    }
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
/// Per SHAPE-5: values are decrypted per-access. The returned
/// `ResolvedEntry.value` is wrapped in `Zeroizing<String>` and will be
/// zeroed automatically on drop.
///
/// Lookup is by entry **name**, not by index, so rekey/rotation that
/// reorders entries does not produce stale results.
pub fn fetch_secret_value(
    metadata: &EntryMetadata,
    keystore_path: &Path,
    identity_str: &str,
) -> Result<ResolvedEntry, LockBoxError> {
    let keystore = decrypt_keystore(keystore_path, identity_str)?;

    // Locate by name — never by index. After rekey or rotation the entry
    // order may change, making index-based lookup stale.
    let entry = keystore
        .entries
        .iter()
        .find(|e| e.name == metadata.name)
        .ok_or_else(|| LockBoxError::MalformedKeystore {
            reason: format!(
                "entry '{}' not found in keystore (keystore has {} entries)",
                metadata.name,
                keystore.entries.len()
            ),
        })?;

    Ok(ResolvedEntry {
        name: metadata.name.clone(),
        value: zeroize::Zeroizing::new(entry.value.clone()),
        entry_type: metadata.entry_type,
    })
}
