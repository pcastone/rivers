//! rivers-lockbox — standalone LockBox management CLI.
//!
//! Commands: init, add, list, show, alias, rotate, remove, rekey, validate
//!
//! Storage: a single Age-encrypted TOML keystore (`keystore.rkeystore`) managed
//! via `rivers-lockbox-engine`. Replaces the previous per-entry `.age` file store.

#![warn(missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};

use age::secrecy::ExposeSecret;
use chrono::Utc;
use rivers_lockbox_engine::{
    Keystore, KeystoreEntry,
    decrypt_keystore, encrypt_keystore,
    validate_entry_name,
};
use zeroize::Zeroizing;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    let lockbox_dir = std::env::var("RIVERS_LOCKBOX_DIR")
        .unwrap_or_else(|_| "lockbox".into());

    let result = match args[1].as_str() {
        "--version" | "-V" | "version" => {
            println!("rivers-lockbox {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH);
            return;
        }
        "init" => cmd_init(&lockbox_dir),
        "add" => {
            if args.len() < 3 { eprintln!("Usage: rivers-lockbox add <name>"); std::process::exit(1); }
            let value = match read_secret_value(&format!("Enter value for '{}': ", args[2])) {
                Ok(v) => v,
                Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
            };
            cmd_add(&lockbox_dir, &args[2], &value)
        }
        "list" => cmd_list(&lockbox_dir),
        "show" => {
            if args.len() < 3 { eprintln!("Usage: rivers-lockbox show <name>"); std::process::exit(1); }
            cmd_show(&lockbox_dir, &args[2])
        }
        "alias" => {
            if args.len() < 4 { eprintln!("Usage: rivers-lockbox alias <alias> <target>"); std::process::exit(1); }
            cmd_alias(&lockbox_dir, &args[2], &args[3])
        }
        "rotate" => {
            if args.len() < 3 { eprintln!("Usage: rivers-lockbox rotate <name>"); std::process::exit(1); }
            let value = match read_secret_value(&format!("Enter new value for '{}': ", args[2])) {
                Ok(v) => v,
                Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
            };
            cmd_rotate(&lockbox_dir, &args[2], &value)
        }
        "remove" => {
            if args.len() < 3 { eprintln!("Usage: rivers-lockbox remove <name>"); std::process::exit(1); }
            cmd_remove(&lockbox_dir, &args[2])
        }
        "rekey" => cmd_rekey(&lockbox_dir),
        "validate" => cmd_validate(&lockbox_dir),
        "help" | "--help" | "-h" => { print_usage(); Ok(()) }
        other => {
            eprintln!("Unknown command: {other}");
            print_usage();
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!("rivers-lockbox — Rivers secret management");
    eprintln!();
    eprintln!("Usage: rivers-lockbox <command> [args]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  init              Create new lockbox with age keypair");
    eprintln!("  add <name>        Add a secret (prompts for value via hidden TTY input)");
    eprintln!("  list              List all entry names and aliases");
    eprintln!("  show <name>       Decrypt and show a secret");
    eprintln!("  alias <a> <target> Add an alias for an existing entry");
    eprintln!("  rotate <name>     Replace a secret value (prompts for value via hidden TTY input)");
    eprintln!("  remove <name>     Remove a secret entry and its aliases");
    eprintln!("  rekey             Re-encrypt keystore with a new identity (transactional)");
    eprintln!("  validate          Verify keystore integrity");
    eprintln!();
    eprintln!("Environment: RIVERS_LOCKBOX_DIR (default: ./lockbox)");
}

fn read_secret_value(prompt: &str) -> Result<String, String> {
    rpassword::prompt_password(prompt)
        .map_err(|e| format!("failed to read secret: {e}"))
}

fn identity_path(lockbox_dir: &str) -> PathBuf {
    Path::new(lockbox_dir).join("identity.key")
}

fn keystore_path(lockbox_dir: &str) -> PathBuf {
    Path::new(lockbox_dir).join("keystore.rkeystore")
}

fn load_identity_str(lockbox_dir: &str) -> Result<Zeroizing<String>, String> {
    let path = identity_path(lockbox_dir);
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read identity at {}: {e}", path.display()))?;
    Ok(Zeroizing::new(raw.trim().to_string()))
}

fn load_identity(lockbox_dir: &str) -> Result<age::x25519::Identity, String> {
    let id_str = load_identity_str(lockbox_dir)?;
    id_str.parse::<age::x25519::Identity>()
        .map_err(|_| "invalid identity key".to_string())
}

fn load_keystore(lockbox_dir: &str) -> Result<Keystore, String> {
    let id_str = load_identity_str(lockbox_dir)?;
    let ks_path = keystore_path(lockbox_dir);
    decrypt_keystore(&ks_path, &id_str)
        .map_err(|e| format!("cannot decrypt keystore: {e}"))
}

fn save_keystore_atomic(lockbox_dir: &str, keystore: &Keystore) -> Result<(), String> {
    let ks_path = keystore_path(lockbox_dir);
    let tmp_path = ks_path.with_extension("rkeystore.tmp");
    let identity = load_identity(lockbox_dir)?;
    let recipient_str = identity.to_public().to_string();
    encrypt_keystore(&tmp_path, &recipient_str, keystore)
        .map_err(|e| format!("encrypt keystore: {e}"))?;
    fs::rename(&tmp_path, &ks_path)
        .map_err(|e| format!("atomic rename failed: {e}"))?;
    Ok(())
}

fn cmd_init(lockbox_dir: &str) -> Result<(), String> {
    let dir = Path::new(lockbox_dir);
    if dir.exists() {
        return Err(format!("lockbox directory already exists: {}", dir.display()));
    }
    fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;

    let identity = age::x25519::Identity::generate();
    let public_key = identity.to_public().to_string();
    let recipient_str = public_key.clone();

    let id_path = identity_path(lockbox_dir);
    let secret_str = identity.to_string();
    fs::write(&id_path, secret_str.expose_secret().as_bytes())
        .map_err(|e| format!("write identity: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&id_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod identity: {e}"))?;
    }

    let empty_keystore = Keystore { version: 1, entries: vec![] };
    let ks_path = keystore_path(lockbox_dir);
    encrypt_keystore(&ks_path, &recipient_str, &empty_keystore)
        .map_err(|e| format!("init keystore: {e}"))?;

    println!("Lockbox initialized at {}", dir.display());
    println!("Public key: {public_key}");
    Ok(())
}

fn cmd_add(lockbox_dir: &str, name: &str, value: &str) -> Result<(), String> {
    validate_entry_name(name).map_err(|e| e.to_string())?;

    let mut keystore = load_keystore(lockbox_dir)?;

    if keystore.entries.iter().any(|e| e.name == name) {
        return Err(format!("entry '{name}' already exists — use 'rotate' to update"));
    }

    let now = Utc::now();
    keystore.entries.push(KeystoreEntry {
        name: name.to_string(),
        value: value.to_string(),
        entry_type: "string".to_string(),
        aliases: vec![],
        created: now,
        updated: now,
        driver: None,
        username: None,
        hosts: vec![],
        database: None,
    });

    save_keystore_atomic(lockbox_dir, &keystore)?;
    println!("Added: {name}");
    Ok(())
}

fn cmd_list(lockbox_dir: &str) -> Result<(), String> {
    let keystore = load_keystore(lockbox_dir)?;

    for entry in &keystore.entries {
        println!("{}", entry.name);
        for alias in &entry.aliases {
            println!("  alias: {alias} → {}", entry.name);
        }
    }
    if keystore.entries.is_empty() {
        println!("(no entries)");
    }
    Ok(())
}

fn cmd_show(lockbox_dir: &str, name: &str) -> Result<(), String> {
    let keystore = load_keystore(lockbox_dir)?;

    let entry = keystore.entries.iter()
        .find(|e| e.name == name || e.aliases.iter().any(|a| a == name))
        .ok_or_else(|| format!("entry '{name}' not found"))?;

    println!("{}", entry.value);
    Ok(())
}

fn cmd_alias(lockbox_dir: &str, alias: &str, target: &str) -> Result<(), String> {
    validate_entry_name(alias).map_err(|e| e.to_string())?;

    let mut keystore = load_keystore(lockbox_dir)?;

    // Reject if alias name already exists as an entry name or alias elsewhere.
    let alias_conflict = keystore.entries.iter().any(|e| {
        e.name == alias || e.aliases.iter().any(|a| a == alias)
    });
    if alias_conflict {
        return Err(format!("'{alias}' already exists as an entry or alias"));
    }

    let entry = keystore.entries.iter_mut()
        .find(|e| e.name == target)
        .ok_or_else(|| format!("target entry '{target}' not found"))?;

    entry.aliases.push(alias.to_string());
    entry.updated = Utc::now();

    save_keystore_atomic(lockbox_dir, &keystore)?;
    println!("Alias: {alias} → {target}");
    Ok(())
}

fn cmd_rotate(lockbox_dir: &str, name: &str, new_value: &str) -> Result<(), String> {
    let mut keystore = load_keystore(lockbox_dir)?;

    let entry = keystore.entries.iter_mut()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("entry '{name}' not found — use 'add' to create"))?;

    entry.value = new_value.to_string();
    entry.updated = Utc::now();

    save_keystore_atomic(lockbox_dir, &keystore)?;
    println!("Rotated: {name}");
    Ok(())
}

fn cmd_remove(lockbox_dir: &str, name: &str) -> Result<(), String> {
    let mut keystore = load_keystore(lockbox_dir)?;

    let before = keystore.entries.len();
    keystore.entries.retain(|e| e.name != name);
    if keystore.entries.len() == before {
        return Err(format!("entry '{name}' not found"));
    }

    save_keystore_atomic(lockbox_dir, &keystore)?;
    println!("Removed: {name}");
    Ok(())
}

fn cmd_rekey(lockbox_dir: &str) -> Result<(), String> {
    // Transactional rekey via a staging directory:
    //   1. Load current keystore
    //   2. Generate new identity
    //   3. Write staging/identity.key + staging/keystore.rkeystore with new key
    //   4. Rename staging dir → lockbox dir (atomic swap)
    let keystore = load_keystore(lockbox_dir)?;

    let new_identity = age::x25519::Identity::generate();
    let new_recipient_str = new_identity.to_public().to_string();

    let staging_dir = format!("{lockbox_dir}.staging");
    if Path::new(&staging_dir).exists() {
        fs::remove_dir_all(&staging_dir)
            .map_err(|e| format!("cannot remove leftover staging dir: {e}"))?;
    }
    fs::create_dir_all(&staging_dir).map_err(|e| format!("mkdir staging: {e}"))?;

    let new_id_path = Path::new(&staging_dir).join("identity.key");
    let secret_str = new_identity.to_string();
    fs::write(&new_id_path, secret_str.expose_secret().as_bytes())
        .map_err(|e| format!("write staging identity: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new_id_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod staging identity: {e}"))?;
    }

    let new_ks_path = Path::new(&staging_dir).join("keystore.rkeystore");
    encrypt_keystore(&new_ks_path, &new_recipient_str, &keystore)
        .map_err(|e| format!("encrypt staging keystore: {e}"))?;

    // Atomic swap: rename old → backup, staging → live, remove backup
    let backup_dir = format!("{lockbox_dir}.old");
    if Path::new(&backup_dir).exists() {
        fs::remove_dir_all(&backup_dir)
            .map_err(|e| format!("cannot remove old backup: {e}"))?;
    }
    fs::rename(lockbox_dir, &backup_dir)
        .map_err(|e| format!("rename current → backup: {e}"))?;
    fs::rename(&staging_dir, lockbox_dir)
        .map_err(|e| format!("rename staging → live: {e}"))?;
    let _ = fs::remove_dir_all(&backup_dir);

    println!("Rekeyed {} entries", keystore.entries.len());
    println!("New public key: {new_recipient_str}");
    Ok(())
}

fn cmd_validate(lockbox_dir: &str) -> Result<(), String> {
    match load_keystore(lockbox_dir) {
        Ok(keystore) => {
            println!("{} entries validated OK", keystore.entries.len());
            Ok(())
        }
        Err(e) => Err(format!("keystore validation failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_tmp() -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("lb");
        let dir_str = dir.to_str().unwrap().to_string();
        cmd_init(&dir_str).expect("init failed");
        (tmp, dir_str)
    }

    #[test]
    fn init_creates_keystore_and_identity() {
        let (_tmp, dir) = init_tmp();
        assert!(Path::new(&dir).join("identity.key").exists());
        assert!(Path::new(&dir).join("keystore.rkeystore").exists());
    }

    #[test]
    fn init_fails_if_dir_exists() {
        let (_tmp, dir) = init_tmp();
        let result = cmd_init(&dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn add_and_show_roundtrip() {
        let (_tmp, dir) = init_tmp();
        cmd_add(&dir, "dbpassword", "secret123").unwrap();
        let result = cmd_show(&dir, "dbpassword");
        assert!(result.is_ok());
    }

    #[test]
    fn add_duplicate_fails() {
        let (_tmp, dir) = init_tmp();
        cmd_add(&dir, "mykey", "val1").unwrap();
        let result = cmd_add(&dir, "mykey", "val2");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn remove_deletes_entry() {
        let (_tmp, dir) = init_tmp();
        cmd_add(&dir, "mykey", "val").unwrap();
        cmd_remove(&dir, "mykey").unwrap();
        let result = cmd_show(&dir, "mykey");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn remove_nonexistent_fails() {
        let (_tmp, dir) = init_tmp();
        let result = cmd_remove(&dir, "ghost");
        assert!(result.is_err());
    }

    #[test]
    fn rotate_updates_value() {
        let (_tmp, dir) = init_tmp();
        cmd_add(&dir, "k", "old").unwrap();
        cmd_rotate(&dir, "k", "new").unwrap();
        // Verify via raw keystore inspection
        let ks = load_keystore(&dir).unwrap();
        assert_eq!(ks.entries[0].value, "new");
    }

    #[test]
    fn alias_resolves_to_entry() {
        let (_tmp, dir) = init_tmp();
        cmd_add(&dir, "postgres/prod", "pg-secret").unwrap();
        cmd_alias(&dir, "pg", "postgres/prod").unwrap();
        let ks = load_keystore(&dir).unwrap();
        assert!(ks.entries[0].aliases.contains(&"pg".to_string()));
    }

    #[test]
    fn alias_duplicate_rejected() {
        let (_tmp, dir) = init_tmp();
        cmd_add(&dir, "postgres/prod", "pg-secret").unwrap();
        cmd_alias(&dir, "pg", "postgres/prod").unwrap();
        let result = cmd_alias(&dir, "pg", "postgres/prod");
        assert!(result.is_err());
    }

    #[test]
    fn validate_on_valid_keystore() {
        let (_tmp, dir) = init_tmp();
        cmd_add(&dir, "key1", "val1").unwrap();
        let result = cmd_validate(&dir);
        assert!(result.is_ok());
    }

    #[test]
    fn rekey_produces_new_identity_and_preserves_entries() {
        let (_tmp, dir) = init_tmp();
        cmd_add(&dir, "secret/key", "my-value").unwrap();
        let old_id = fs::read_to_string(Path::new(&dir).join("identity.key")).unwrap();
        cmd_rekey(&dir).unwrap();
        let new_id = fs::read_to_string(Path::new(&dir).join("identity.key")).unwrap();
        assert_ne!(old_id, new_id, "identity must change after rekey");
        // Entries survive rekey
        let ks = load_keystore(&dir).unwrap();
        assert_eq!(ks.entries.len(), 1);
        assert_eq!(ks.entries[0].name, "secret/key");
        assert_eq!(ks.entries[0].value, "my-value");
    }

    #[test]
    fn invalid_name_rejected() {
        let (_tmp, dir) = init_tmp();
        let result = cmd_add(&dir, "INVALID-UPPERCASE", "val");
        assert!(result.is_err());
    }
}
