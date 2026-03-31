//! Application keystore resolver — holds unlocked keystores scoped by app.
//!
//! Built at startup after LockBox credentials are resolved.
//! Key format: `"{entry_point}:{keystore_name}"`.

use std::collections::HashMap;
use std::sync::Arc;

/// Holds unlocked application keystores, scoped by app.
/// Key format: "{entry_point}:{keystore_name}"
pub struct KeystoreResolver {
    keystores: HashMap<String, Arc<rivers_keystore_engine::AppKeystore>>,
}

impl KeystoreResolver {
    /// Create an empty keystore resolver.
    pub fn new() -> Self {
        Self {
            keystores: HashMap::new(),
        }
    }

    /// Insert an unlocked keystore under the given scoped name.
    pub fn insert(&mut self, scoped_name: String, ks: rivers_keystore_engine::AppKeystore) {
        self.keystores.insert(scoped_name, Arc::new(ks));
    }

    /// Get a keystore by its scoped name.
    pub fn get(&self, scoped_name: &str) -> Option<&Arc<rivers_keystore_engine::AppKeystore>> {
        self.keystores.get(scoped_name)
    }

    /// Returns true if no keystores are registered.
    pub fn is_empty(&self) -> bool {
        self.keystores.is_empty()
    }

    /// Find the first keystore whose scoped name starts with the given entry_point prefix.
    ///
    /// Scoped names are `"{entry_point}:{keystore_name}"`. Most apps declare exactly
    /// one keystore, so this returns the first match for that app's entry_point.
    pub fn get_for_entry_point(&self, entry_point: &str) -> Option<&Arc<rivers_keystore_engine::AppKeystore>> {
        let prefix = format!("{}:", entry_point);
        self.keystores.iter()
            .find(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v)
    }
}
