//! Startup resolution sequence -- collecting, validating, and resolving
//! lockbox:// references during `riversd` startup.

use std::collections::HashMap;
use std::path::Path;

use zeroize::Zeroize;

use crate::crypto::*;
use crate::key_source::*;
use crate::resolver::*;
use crate::types::*;
use crate::validation::parse_lockbox_uri;

// ── Startup Resolution ──────────────────────────────────────────────

/// Credential reference found in config -- a lockbox:// URI tied to a datasource.
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
/// Per SHAPE-5: only validates existence -- no values loaded into memory.
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
/// Per spec S8.1 -- the 12-step startup sequence:
/// 1. Collect lockbox:// URIs (done by caller, passed as `references`)
/// 2. Check `[lockbox]` config present
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
