use std::path::{Path, PathBuf};

use super::start::find_riversd_binary;

/// Run pre-launch health checks.
pub fn cmd_doctor(args: &[String]) -> Result<(), String> {
    let mut passed = 0u32;
    let mut failed = 0u32;

    // Parse --config flag
    let explicit_config: Option<PathBuf> = {
        let mut cfg = None;
        let mut i = 0;
        while i < args.len() {
            if (args[i] == "--config" || args[i] == "-c") && i + 1 < args.len() {
                cfg = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            } else {
                i += 1;
            }
        }
        cfg
    };

    // Check 1: riversd binary exists
    match find_riversd_binary() {
        Ok(path) => {
            println!("  [PASS] riversd binary: {}", path.display());
            passed += 1;
        }
        Err(e) => {
            println!("  [FAIL] riversd binary: {e}");
            failed += 1;
        }
    }

    // Check 2: Config file found + parses
    let config_path = explicit_config.or_else(discover_config);
    let config = match &config_path {
        Some(path) => {
            match rivers_runtime::loader::load_server_config(path) {
                Ok(cfg) => {
                    println!("  [PASS] config: {} (parsed)", path.display());
                    passed += 1;
                    Some(cfg)
                }
                Err(e) => {
                    println!("  [FAIL] config: {} — {e}", path.display());
                    failed += 1;
                    None
                }
            }
        }
        None => {
            println!("  [SKIP] config: no file found — using defaults");
            Some(rivers_runtime::rivers_core_config::ServerConfig::default())
        }
    };

    // Check 3: Config validates
    if let Some(ref cfg) = config {
        match rivers_runtime::validate_server_config(cfg) {
            Ok(()) => {
                println!("  [PASS] config validates");
                passed += 1;
            }
            Err(errors) => {
                println!("  [FAIL] config validation: {} error(s)", errors.len());
                for e in &errors {
                    println!("         {e}");
                }
                failed += 1;
            }
        }
    }

    // Check 4: Storage engine (in-memory always works)
    {
        let _storage = rivers_runtime::rivers_core_config::InMemoryStorageEngine::new();
        println!("  [PASS] storage engine available");
        passed += 1;
    }

    // Check 5: LockBox permissions (if configured)
    if let Some(ref cfg) = config {
        if let Some(ref lockbox_cfg) = cfg.lockbox {
            if let Some(ref keystore_path) = lockbox_cfg.path {
                let path = Path::new(keystore_path);
                if !path.exists() {
                    println!("  [FAIL] lockbox keystore not found: {keystore_path}");
                    failed += 1;
                } else {
                    match check_lockbox_permissions(path) {
                        Ok(()) => {
                            println!("  [PASS] lockbox keystore permissions ok");
                            passed += 1;
                        }
                        Err(e) => {
                            println!("  [FAIL] lockbox keystore: {e}");
                            failed += 1;
                        }
                    }
                }
            } else {
                println!("  [SKIP] lockbox not configured");
            }
        } else {
            println!("  [SKIP] lockbox not configured");
        }
    }

    println!();
    if failed > 0 {
        println!("doctor: {passed} passed, {failed} failed");
        std::process::exit(1);
    } else {
        println!("doctor: {passed} passed, 0 failed");
    }
    Ok(())
}

/// Check that a file has mode 600 (owner read+write only).
#[cfg(unix)]
fn check_lockbox_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let mode = metadata.mode() & 0o777;
    if mode != 0o600 {
        return Err(format!("{} has insecure permissions (mode {mode:04o}) — chmod 0600 {}", path.display(), path.display()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_lockbox_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Discover a config file from conventional locations.
pub fn discover_config() -> Option<PathBuf> {
    rivers_runtime::home::discover_config()
}

#[cfg(feature = "tls")]
pub fn load_config_for_tls() -> Result<rivers_runtime::rivers_core_config::ServerConfig, String> {
    let path = discover_config()
        .ok_or_else(|| "no config file found — run from a directory with config/riversd.toml".to_string())?;
    rivers_runtime::loader::load_server_config(&path)
        .map_err(|e| format!("failed to load config: {e}"))
}
