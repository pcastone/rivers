//! Shared test credential helper — resolves passwords from a real LockBox keystore.
//!
//! Reads Age-encrypted secrets from the on-disk keystore at `sec/lockbox/`.
//! No passwords are stored in source code. The keystore is in `.gitignore`.
//!
//! Setup: `RIVERS_LOCKBOX_DIR=sec/lockbox rivers-lockbox add <name> --value <secret>`
//! Usage: `TestCredentials::new()` then `creds.get("postgres/test")`

use std::fs;
use std::path::PathBuf;

use age::secrecy::ExposeSecret;

/// Resolves credentials from the real LockBox keystore at `sec/lockbox/`.
pub struct TestCredentials {
    lockbox_dir: PathBuf,
    identity: age::x25519::Identity,
}

impl TestCredentials {
    /// Load the lockbox identity from `sec/lockbox/identity.key`.
    ///
    /// Searches upward from the crate directory to find the workspace root
    /// containing `sec/lockbox/`.
    pub fn new() -> Self {
        let lockbox_dir = find_lockbox_dir()
            .expect("cannot find sec/lockbox/ — run from workspace root or set RIVERS_LOCKBOX_DIR");

        let identity_path = lockbox_dir.join("identity.key");
        let key_str = fs::read_to_string(&identity_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", identity_path.display()));
        let identity: age::x25519::Identity = key_str.trim().parse()
            .expect("invalid age identity key in sec/lockbox/identity.key");

        Self { lockbox_dir, identity }
    }

    /// Decrypt and return a credential by name (e.g. "postgres/test").
    pub fn get(&self, name: &str) -> String {
        let entry_path = self.lockbox_dir.join("entries").join(format!("{name}.age"));
        if !entry_path.exists() {
            panic!("lockbox entry not found: {name} (expected at {})", entry_path.display());
        }
        let encrypted = fs::read(&entry_path)
            .unwrap_or_else(|e| panic!("cannot read lockbox entry {name}: {e}"));
        let decrypted = age::decrypt(&self.identity, &encrypted)
            .unwrap_or_else(|e| panic!("cannot decrypt lockbox entry {name}: {e}"));
        String::from_utf8(decrypted)
            .unwrap_or_else(|e| panic!("lockbox entry {name} is not valid UTF-8: {e}"))
    }
}

/// Walk up from the current directory to find the workspace root with `sec/lockbox/`.
fn find_lockbox_dir() -> Option<PathBuf> {
    // Check env var first
    if let Ok(dir) = std::env::var("RIVERS_LOCKBOX_DIR") {
        let p = PathBuf::from(&dir);
        if p.join("identity.key").exists() {
            return Some(p);
        }
    }

    // Walk up from current dir
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..10 {
        let candidate = dir.join("sec").join("lockbox");
        if candidate.join("identity.key").exists() {
            return Some(candidate);
        }
        if !dir.pop() { break; }
    }
    None
}
