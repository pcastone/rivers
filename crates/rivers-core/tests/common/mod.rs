//! Shared test credential helper — resolves full connection info from a LockBox keystore.
//!
//! Reads Age-encrypted passwords and plaintext `.meta.json` sidecars from `sec/lockbox/`.
//! No passwords, IPs, or usernames are stored in source code. The keystore is in `.gitignore`.
//!
//! Usage:
//!   `TestCredentials::new().connection_params("postgres/test")`
//!   Returns a complete `ConnectionParams` — no hardcoded values needed in tests.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use rivers_driver_sdk::ConnectionParams;

/// Credential metadata from a `.meta.json` sidecar file.
#[derive(serde::Deserialize, Default)]
struct CredentialMeta {
    #[serde(default)]
    driver: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    database: Option<String>,
    #[serde(default)]
    options: HashMap<String, String>,
}

/// Resolves full connection credentials from the LockBox keystore at `sec/lockbox/`.
pub struct TestCredentials {
    lockbox_dir: PathBuf,
    identity: age::x25519::Identity,
}

impl TestCredentials {
    /// Load the lockbox identity from `sec/lockbox/identity.key`.
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

    /// Decrypt and return just the password by name (e.g. "postgres/test").
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

    /// Read the `.meta.json` sidecar for an entry. Returns None if absent.
    fn get_meta(&self, name: &str) -> Option<CredentialMeta> {
        let meta_path = self.lockbox_dir.join("entries").join(format!("{name}.meta.json"));
        if !meta_path.exists() {
            return None;
        }
        let json = fs::read_to_string(&meta_path)
            .unwrap_or_else(|e| panic!("cannot read meta for {name}: {e}"));
        Some(serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("invalid meta JSON for {name}: {e}")))
    }

    /// Build a complete `ConnectionParams` from the lockbox entry + sidecar.
    ///
    /// Reads password from `.age`, connection info from `.meta.json`.
    /// No hardcoded IPs, usernames, or driver names needed in test code.
    pub fn connection_params(&self, name: &str) -> ConnectionParams {
        let password = self.get(name);
        let meta = self.get_meta(name).unwrap_or_default();

        let (host, port) = meta.hosts.first()
            .map(|h| parse_host_port(h))
            .unwrap_or_else(|| ("".into(), 0));

        let mut options = meta.options;
        if let Some(ref driver) = meta.driver {
            options.insert("driver".into(), driver.clone());
        }
        if meta.hosts.len() > 1 {
            options.insert("hosts".into(), meta.hosts.join(","));
            options.insert("cluster".into(), "true".into());
        }

        ConnectionParams {
            host,
            port,
            database: meta.database.unwrap_or_default(),
            username: meta.username.unwrap_or_default(),
            password,
            options,
        }
    }
}

/// Parse "host:port" into (host, port). Returns port=0 if no port specified.
fn parse_host_port(s: &str) -> (String, u16) {
    match s.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(0)),
        None => (s.to_string(), 0),
    }
}

/// Walk up from the current directory to find the workspace root with `sec/lockbox/`.
pub fn find_lockbox_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("RIVERS_LOCKBOX_DIR") {
        let p = PathBuf::from(&dir);
        if p.join("identity.key").exists() {
            return Some(p);
        }
    }

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
