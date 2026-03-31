//! rivers-lockbox — standalone LockBox management CLI.
//!
//! Commands: init, add, list, show, alias, rotate, remove, rekey, validate

#![warn(missing_docs)]

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use age::secrecy::ExposeSecret;

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
            if args.len() < 3 { eprintln!("Usage: rivers-lockbox add <name> [--value <val>]"); std::process::exit(1); }
            let value = if args.len() >= 5 && args[3] == "--value" {
                args[4].clone()
            } else {
                match read_stdin_value() {
                    Ok(v) => v,
                    Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
                }
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
            if args.len() < 3 { eprintln!("Usage: rivers-lockbox rotate <name> [--value <val>]"); std::process::exit(1); }
            let value = if args.len() >= 5 && args[3] == "--value" {
                args[4].clone()
            } else {
                match read_stdin_value() {
                    Ok(v) => v,
                    Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
                }
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
    eprintln!("  add <name>        Add a secret (reads value from stdin or --value)");
    eprintln!("  list              List all entry names");
    eprintln!("  show <name>       Decrypt and show a secret");
    eprintln!("  alias <a> <target> Create an alias");
    eprintln!("  rotate <name>     Replace a secret value");
    eprintln!("  remove <name>     Remove a secret");
    eprintln!("  rekey             Re-encrypt all secrets with new identity");
    eprintln!("  validate          Verify keystore integrity");
    eprintln!();
    eprintln!("Environment: RIVERS_LOCKBOX_DIR (default: ./lockbox)");
}

fn read_stdin_value() -> Result<String, String> {
    eprint!("Enter value: ");
    io::stderr().flush().ok();
    let mut value = String::new();
    io::stdin().read_line(&mut value)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    Ok(value.trim().to_string())
}

fn entries_dir(lockbox_dir: &str) -> PathBuf {
    Path::new(lockbox_dir).join("entries")
}

fn identity_path(lockbox_dir: &str) -> PathBuf {
    Path::new(lockbox_dir).join("identity.key")
}

fn aliases_path(lockbox_dir: &str) -> PathBuf {
    Path::new(lockbox_dir).join("aliases.json")
}

fn load_identity(lockbox_dir: &str) -> Result<age::x25519::Identity, String> {
    let path = identity_path(lockbox_dir);
    let key_str = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read identity at {}: {e}", path.display()))?;
    let key_str = key_str.trim();
    key_str.parse::<age::x25519::Identity>()
        .map_err(|e| format!("invalid identity key: {e}"))
}

fn load_recipient(lockbox_dir: &str) -> Result<age::x25519::Recipient, String> {
    let identity = load_identity(lockbox_dir)?;
    Ok(identity.to_public())
}

fn encrypt_value(recipient: &age::x25519::Recipient, value: &[u8]) -> Result<Vec<u8>, String> {
    age::encrypt(recipient, value)
        .map_err(|e| format!("encryption: {e}"))
}

fn decrypt_value(identity: &age::x25519::Identity, encrypted: &[u8]) -> Result<Vec<u8>, String> {
    age::decrypt(identity, encrypted)
        .map_err(|e| format!("decryption: {e}"))
}

fn cmd_init(lockbox_dir: &str) -> Result<(), String> {
    let dir = Path::new(lockbox_dir);
    if dir.exists() {
        return Err(format!("lockbox directory already exists: {}", dir.display()));
    }
    fs::create_dir_all(entries_dir(lockbox_dir)).map_err(|e| format!("mkdir: {e}"))?;

    let identity = age::x25519::Identity::generate();
    let public_key = identity.to_public().to_string();

    let id_path = identity_path(lockbox_dir);
    let secret_str = identity.to_string();
    fs::write(&id_path, secret_str.expose_secret().as_bytes())
        .map_err(|e| format!("write identity: {e}"))?;

    // Set file permissions 600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&id_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod: {e}"))?;
    }

    // Empty aliases
    fs::write(aliases_path(lockbox_dir), b"{}").map_err(|e| format!("write aliases: {e}"))?;

    println!("Lockbox initialized at {}", dir.display());
    println!("Public key: {public_key}");
    Ok(())
}

fn cmd_add(lockbox_dir: &str, name: &str, value: &str) -> Result<(), String> {
    // Validate name using core library rules (rejects path traversal, special chars)
    rivers_lockbox_engine::validate_entry_name(name)
        .map_err(|e| e.to_string())?;

    let entry_path = entries_dir(lockbox_dir).join(format!("{name}.age"));
    if entry_path.exists() {
        return Err(format!("entry '{name}' already exists — use 'rotate' to update"));
    }

    let recipient = load_recipient(lockbox_dir)?;
    let encrypted = encrypt_value(&recipient, value.as_bytes())?;
    fs::write(&entry_path, &encrypted).map_err(|e| format!("write entry: {e}"))?;

    println!("Added: {name}");
    Ok(())
}

fn cmd_list(lockbox_dir: &str) -> Result<(), String> {
    let dir = entries_dir(lockbox_dir);
    if !dir.exists() {
        return Err("lockbox not initialized — run 'rivers-lockbox init'".into());
    }

    let mut entries: Vec<String> = fs::read_dir(&dir)
        .map_err(|e| format!("read dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".age").map(|n| n.to_string())
        })
        .collect();
    entries.sort();

    // Also list aliases
    let aliases: HashMap<String, String> = fs::read_to_string(aliases_path(lockbox_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    for entry in &entries {
        println!("{entry}");
    }
    if !aliases.is_empty() {
        println!();
        for (alias, target) in &aliases {
            println!("{alias} → {target}");
        }
    }
    Ok(())
}

fn cmd_show(lockbox_dir: &str, name: &str) -> Result<(), String> {
    // Check aliases first
    let aliases: HashMap<String, String> = fs::read_to_string(aliases_path(lockbox_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let resolved = aliases.get(name).map(|s| s.as_str()).unwrap_or(name);

    let entry_path = entries_dir(lockbox_dir).join(format!("{resolved}.age"));
    if !entry_path.exists() {
        return Err(format!("entry '{name}' not found"));
    }

    let identity = load_identity(lockbox_dir)?;
    let encrypted = fs::read(&entry_path).map_err(|e| format!("read entry: {e}"))?;
    let decrypted = decrypt_value(&identity, &encrypted)?;
    let value = String::from_utf8(decrypted).map_err(|e| format!("UTF-8: {e}"))?;

    println!("{value}");
    Ok(())
}

fn cmd_alias(lockbox_dir: &str, alias: &str, target: &str) -> Result<(), String> {
    let target_path = entries_dir(lockbox_dir).join(format!("{target}.age"));
    if !target_path.exists() {
        return Err(format!("target entry '{target}' not found"));
    }

    let mut aliases: HashMap<String, String> = fs::read_to_string(aliases_path(lockbox_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    aliases.insert(alias.to_string(), target.to_string());
    let json = serde_json::to_string_pretty(&aliases).map_err(|e| format!("json: {e}"))?;
    fs::write(aliases_path(lockbox_dir), json.as_bytes()).map_err(|e| format!("write: {e}"))?;

    println!("Alias: {alias} → {target}");
    Ok(())
}

fn cmd_rotate(lockbox_dir: &str, name: &str, new_value: &str) -> Result<(), String> {
    let entry_path = entries_dir(lockbox_dir).join(format!("{name}.age"));
    if !entry_path.exists() {
        return Err(format!("entry '{name}' not found — use 'add' to create"));
    }

    let recipient = load_recipient(lockbox_dir)?;
    let encrypted = encrypt_value(&recipient, new_value.as_bytes())?;
    fs::write(&entry_path, &encrypted).map_err(|e| format!("write: {e}"))?;

    println!("Rotated: {name}");
    Ok(())
}

fn cmd_remove(lockbox_dir: &str, name: &str) -> Result<(), String> {
    let entry_path = entries_dir(lockbox_dir).join(format!("{name}.age"));
    if !entry_path.exists() {
        return Err(format!("entry '{name}' not found"));
    }
    fs::remove_file(&entry_path).map_err(|e| format!("remove: {e}"))?;

    // Also remove any aliases pointing to this entry
    let mut aliases: HashMap<String, String> = fs::read_to_string(aliases_path(lockbox_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    aliases.retain(|_, v| v != name);
    let json = serde_json::to_string_pretty(&aliases).unwrap_or_else(|_| "{}".into());
    fs::write(aliases_path(lockbox_dir), json.as_bytes())
        .map_err(|e| format!("failed to update aliases: {e}"))?;

    println!("Removed: {name}");
    Ok(())
}

fn cmd_rekey(lockbox_dir: &str) -> Result<(), String> {
    let old_identity = load_identity(lockbox_dir)?;
    let new_identity = age::x25519::Identity::generate();
    let new_recipient = new_identity.to_public();

    // 1. Read + decrypt + re-encrypt ALL entries into memory first
    let dir = entries_dir(lockbox_dir);
    let entries: Vec<_> = fs::read_dir(&dir)
        .map_err(|e| format!("read dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "age"))
        .collect();

    let mut staged: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(entries.len());
    for entry in &entries {
        let encrypted = fs::read(entry.path()).map_err(|e| format!("read: {e}"))?;
        let decrypted = decrypt_value(&old_identity, &encrypted)?;
        let re_encrypted = encrypt_value(&new_recipient, &decrypted)?;
        staged.push((entry.path(), re_encrypted));
    }

    // 2. Write new identity to a temp file, then rename into place (atomic on most filesystems)
    let id_path = identity_path(lockbox_dir);
    let tmp_id_path = id_path.with_extension("key.tmp");
    let secret_str = new_identity.to_string();
    fs::write(&tmp_id_path, secret_str.expose_secret().as_bytes())
        .map_err(|e| format!("write temp identity: {e}"))?;

    // 3. Rename temp identity file to real path
    fs::rename(&tmp_id_path, &id_path)
        .map_err(|e| format!("rename identity: {e}"))?;

    // 4. Write all re-encrypted entry files
    for (path, data) in &staged {
        fs::write(path, data).map_err(|e| format!("write entry: {e}"))?;
    }

    // 5. chmod 0o600 on identity file with error propagation
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&id_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod: {e}"))?;
    }

    println!("Rekeyed {} entries", entries.len());
    println!("New public key: {}", new_recipient);
    Ok(())
}

fn cmd_validate(lockbox_dir: &str) -> Result<(), String> {
    let identity = load_identity(lockbox_dir)?;
    let dir = entries_dir(lockbox_dir);

    if !dir.exists() {
        return Err("lockbox not initialized".into());
    }

    let mut ok_count = 0;
    let mut err_count = 0;

    for entry in fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
        let entry = entry.map_err(|e| format!("entry: {e}"))?;
        if entry.path().extension().map_or(true, |ext| ext != "age") {
            continue;
        }
        let name = entry.path().file_stem().unwrap().to_string_lossy().to_string();
        let encrypted = fs::read(entry.path()).map_err(|e| format!("read {name}: {e}"))?;
        match decrypt_value(&identity, &encrypted) {
            Ok(_) => { ok_count += 1; }
            Err(e) => {
                eprintln!("FAIL: {name} — {e}");
                err_count += 1;
            }
        }
    }

    println!("{ok_count} entries OK, {err_count} errors");
    if err_count > 0 {
        Err(format!("{err_count} entries failed validation"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lockbox() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn test_init_creates_directory() {
        let tmp = temp_lockbox();
        let dir = tmp.path().join("lb");
        let result = cmd_init(dir.to_str().unwrap());
        assert!(result.is_ok());
        assert!(dir.join("identity.key").exists());
        assert!(dir.join("entries").exists());
        assert!(dir.join("aliases.json").exists());
    }

    #[test]
    fn test_init_fails_if_exists() {
        let tmp = temp_lockbox();
        let dir = tmp.path().join("lb");
        cmd_init(dir.to_str().unwrap()).unwrap();
        let result = cmd_init(dir.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_add_and_show_roundtrip() {
        let tmp = temp_lockbox();
        let dir = tmp.path().join("lb");
        cmd_init(dir.to_str().unwrap()).unwrap();
        cmd_add(dir.to_str().unwrap(), "dbpassword", "secret123").unwrap();
        // Show should succeed (actual output verification would need capture)
        let result = cmd_show(dir.to_str().unwrap(), "dbpassword");
        assert!(result.is_ok());
    }

    #[test]
    fn test_add_duplicate_fails() {
        let tmp = temp_lockbox();
        let dir = tmp.path().join("lb");
        cmd_init(dir.to_str().unwrap()).unwrap();
        cmd_add(dir.to_str().unwrap(), "mykey", "val1").unwrap();
        let result = cmd_add(dir.to_str().unwrap(), "mykey", "val2");
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_deletes_entry() {
        let tmp = temp_lockbox();
        let dir = tmp.path().join("lb");
        cmd_init(dir.to_str().unwrap()).unwrap();
        cmd_add(dir.to_str().unwrap(), "mykey", "val").unwrap();
        cmd_remove(dir.to_str().unwrap(), "mykey").unwrap();
        let result = cmd_show(dir.to_str().unwrap(), "mykey");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_on_valid_keystore() {
        let tmp = temp_lockbox();
        let dir = tmp.path().join("lb");
        cmd_init(dir.to_str().unwrap()).unwrap();
        cmd_add(dir.to_str().unwrap(), "key1", "val1").unwrap();
        let result = cmd_validate(dir.to_str().unwrap());
        assert!(result.is_ok());
    }
}
