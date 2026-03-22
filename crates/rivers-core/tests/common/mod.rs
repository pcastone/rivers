//! Shared test credential helper — resolves passwords from a LockBox keystore.
//!
//! Creates an Age-encrypted keystore in a temp directory containing all
//! test infrastructure credentials. Each live test calls `TestCredentials::new()`
//! to build the keystore and then `creds.get("name")` to fetch a credential.

use std::path::PathBuf;

use age::secrecy::ExposeSecret;
use rivers_core::lockbox::{
    encrypt_keystore, fetch_secret_value, Keystore, KeystoreEntry, LockBoxResolver,
};

/// All test infrastructure credentials backed by a LockBox keystore.
pub struct TestCredentials {
    pub keystore_path: PathBuf,
    pub identity: String,
    pub resolver: LockBoxResolver,
    _tempdir: tempfile::TempDir,
}

impl TestCredentials {
    /// Build a fresh keystore with all test credentials.
    pub fn new() -> Self {
        let identity = age::x25519::Identity::generate();
        let recipient = identity.to_public();

        let entries = vec![
            entry("redis/test", "rivers_test"),
            entry("postgres/test", "postgres"),
            entry("mysql/test", "root"),
            entry("rabbitmq/test", "guest"),
            entry("couchdb/test", "admin"),
            entry("influxdb/test", "rivers-test"),
            entry("mongodb/test", ""),
            entry("redis-streams/test", "rivers_test"),
            entry("memcached/test", ""),
            entry("sqlite/test", ""),
            entry("faker/test", ""),
        ];

        let keystore = Keystore {
            version: 1,
            entries,
        };

        let tempdir = tempfile::TempDir::new().expect("failed to create temp dir for keystore");
        let keystore_path = tempdir.path().join("test.rkeystore");

        encrypt_keystore(&keystore_path, &recipient.to_string(), &keystore)
            .expect("failed to encrypt test keystore");

        let resolver = LockBoxResolver::from_entries(&keystore.entries)
            .expect("failed to build resolver from test entries");

        let identity_str = identity.to_string();

        Self {
            keystore_path,
            identity: identity_str.expose_secret().to_string(),
            resolver,
            _tempdir: tempdir,
        }
    }

    /// Resolve a credential by name, decrypting from the keystore on disk.
    pub fn get(&self, name: &str) -> String {
        let metadata = self
            .resolver
            .resolve(name)
            .unwrap_or_else(|| panic!("credential not found in test keystore: {name}"));
        let resolved = fetch_secret_value(metadata, &self.keystore_path, &self.identity)
            .unwrap_or_else(|e| panic!("failed to fetch credential {name}: {e}"));
        resolved.value
    }
}

fn entry(name: &str, value: &str) -> KeystoreEntry {
    KeystoreEntry {
        name: name.to_string(),
        value: value.to_string(),
        entry_type: "string".to_string(),
        aliases: vec![],
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
    }
}
