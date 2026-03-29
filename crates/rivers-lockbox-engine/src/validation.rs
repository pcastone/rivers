//! Name validation and URI parsing for lockbox:// references.

use crate::types::LockBoxError;

// ── Name Validation ─────────────────────────────────────────────────

/// Validate an entry name against the naming rules.
///
/// Per spec S3.3:
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
