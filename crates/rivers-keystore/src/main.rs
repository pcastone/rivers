//! rivers-keystore — CLI for managing application keystores.
//!
//! Commands: init, generate, list, info, delete, rotate

#![warn(missing_docs)]

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use rivers_keystore_engine::AppKeystore;

// ── CLI definition ──────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "rivers-keystore", about = "Rivers application keystore management")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new application keystore
    Init {
        /// Path to the keystore file
        #[arg(long)]
        path: PathBuf,
    },

    /// Generate and store a new encryption key
    Generate {
        /// Name for the key
        name: String,

        /// Key type (only aes-256 supported)
        #[arg(long = "type", default_value = "aes-256")]
        key_type: String,

        /// Path to the keystore file
        #[arg(long)]
        path: PathBuf,
    },

    /// List keys (names and metadata only)
    List {
        /// Path to the keystore file
        #[arg(long)]
        path: PathBuf,
    },

    /// Show key metadata
    Info {
        /// Name of the key
        name: String,

        /// Path to the keystore file
        #[arg(long)]
        path: PathBuf,
    },

    /// Delete a key
    Delete {
        /// Name of the key
        name: String,

        /// Path to the keystore file
        #[arg(long)]
        path: PathBuf,
    },

    /// Rotate a key (create new version)
    Rotate {
        /// Name of the key
        name: String,

        /// Path to the keystore file
        #[arg(long)]
        path: PathBuf,
    },
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Read and validate the Age identity from the env var.
/// Returns (raw key string, parsed identity).
fn read_identity() -> Result<(String, age::x25519::Identity), String> {
    let key = std::env::var("RIVERS_KEYSTORE_KEY")
        .map_err(|_| "RIVERS_KEYSTORE_KEY env var not set".to_string())?;
    let trimmed = key.trim().to_string();
    let identity = trimmed
        .parse::<age::x25519::Identity>()
        .map_err(|_| "RIVERS_KEYSTORE_KEY is not a valid Age identity".to_string())?;
    Ok((trimmed, identity))
}

fn load_keystore(path: &Path) -> Result<AppKeystore, String> {
    let (key_str, _identity) = read_identity()?;
    AppKeystore::load(path, &key_str)
        .map_err(|e| format!("failed to load keystore: {e}"))
}

fn save_keystore(keystore: &AppKeystore, path: &Path) -> Result<(), String> {
    let (_key_str, identity) = read_identity()?;
    let recipient = identity.to_public();
    keystore
        .save(path, &recipient.to_string())
        .map_err(|e| format!("failed to save keystore: {e}"))
}

// ── Commands ────────────────────────────────────────────────────────

fn cmd_init(path: &Path) -> Result<(), String> {
    let (_key_str, identity) = read_identity()?;
    let recipient = identity.to_public();
    AppKeystore::create(path, &recipient.to_string())
        .map_err(|e| format!("failed to create keystore: {e}"))?;
    println!("keystore created: {}", path.display());
    Ok(())
}

fn cmd_generate(path: &Path, name: &str, key_type: &str) -> Result<(), String> {
    let mut keystore = load_keystore(path)?;
    keystore
        .generate_key(name, key_type)
        .map_err(|e| format!("failed to generate key: {e}"))?;
    save_keystore(&keystore, path)?;
    println!("generated key \"{name}\" ({key_type}, v1)");
    Ok(())
}

fn cmd_list(path: &Path) -> Result<(), String> {
    let keystore = load_keystore(path)?;
    let keys = keystore.list_keys();
    if keys.is_empty() {
        println!("(no keys)");
        return Ok(());
    }
    println!("{:<20} {:<10} {:<10} {}", "name", "type", "version", "created");
    for info in &keys {
        println!(
            "{:<20} {:<10} v{:<9} {}",
            info.name,
            info.key_type,
            info.current_version,
            info.created.format("%Y-%m-%d %H:%M:%S"),
        );
    }
    Ok(())
}

fn cmd_info(path: &Path, name: &str) -> Result<(), String> {
    let keystore = load_keystore(path)?;
    let info = keystore
        .key_info(name)
        .map_err(|e| format!("{e}"))?;
    println!("name:            {}", info.name);
    println!("type:            {}", info.key_type);
    println!("current version: v{}", info.current_version);
    println!("total versions:  {}", info.version_count);
    println!("created:         {}", info.created.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("updated:         {}", info.updated.format("%Y-%m-%d %H:%M:%S UTC"));
    Ok(())
}

fn cmd_delete(path: &Path, name: &str) -> Result<(), String> {
    let mut keystore = load_keystore(path)?;
    keystore
        .delete_key(name)
        .map_err(|e| format!("{e}"))?;
    save_keystore(&keystore, path)?;
    println!("deleted key \"{name}\"");
    Ok(())
}

fn cmd_rotate(path: &Path, name: &str) -> Result<(), String> {
    let mut keystore = load_keystore(path)?;

    // Get current version before rotation
    let old_version = keystore
        .key_info(name)
        .map_err(|e| format!("{e}"))?
        .current_version;

    let new_version = keystore
        .rotate_key(name)
        .map_err(|e| format!("failed to rotate key: {e}"))?;

    save_keystore(&keystore, path)?;
    println!("rotated key \"{name}\" (v{old_version} -> v{new_version})");
    Ok(())
}

// ── Main ────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Init { ref path } => cmd_init(path),
        Command::Generate {
            ref name,
            ref key_type,
            ref path,
        } => cmd_generate(path, name, key_type),
        Command::List { ref path } => cmd_list(path),
        Command::Info { ref name, ref path } => cmd_info(path, name),
        Command::Delete { ref name, ref path } => cmd_delete(path, name),
        Command::Rotate { ref name, ref path } => cmd_rotate(path, name),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
