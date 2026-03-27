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
    pub fn new() -> Self {
        Self {
            keystores: HashMap::new(),
        }
    }

    pub fn insert(&mut self, scoped_name: String, ks: rivers_keystore_engine::AppKeystore) {
        self.keystores.insert(scoped_name, Arc::new(ks));
    }

    pub fn get(&self, scoped_name: &str) -> Option<&Arc<rivers_keystore_engine::AppKeystore>> {
        self.keystores.get(scoped_name)
    }

    pub fn is_empty(&self) -> bool {
        self.keystores.is_empty()
    }
}
