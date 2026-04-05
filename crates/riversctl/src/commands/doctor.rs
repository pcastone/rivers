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

    // Check 6: TLS cert/key files exist
    if let Some(ref cfg) = config {
        if let Some(ref tls) = cfg.base.tls {
            if let Some(ref cert) = tls.cert {
                if Path::new(cert).is_file() {
                    println!("  [PASS] TLS cert: {cert}");
                    passed += 1;
                } else {
                    println!("  [FAIL] TLS cert not found: {cert}");
                    failed += 1;
                }
            } else {
                println!("  [SKIP] TLS cert not configured (will auto-generate)");
            }
            if let Some(ref key) = tls.key {
                if Path::new(key).is_file() {
                    println!("  [PASS] TLS key: {key}");
                    passed += 1;
                } else {
                    println!("  [FAIL] TLS key not found: {key}");
                    failed += 1;
                }
            } else {
                println!("  [SKIP] TLS key not configured (will auto-generate)");
            }
        } else {
            println!("  [SKIP] TLS not configured");
        }
    }

    // Check 7: Log directory writable
    if let Some(ref cfg) = config {
        if let Some(ref log_path) = cfg.base.logging.local_file_path {
            if let Some(log_dir) = Path::new(log_path).parent() {
                if log_dir.is_dir() {
                    // Try creating a temp file to verify write access
                    let test_file = log_dir.join(".rivers-doctor-test");
                    match std::fs::write(&test_file, b"") {
                        Ok(()) => {
                            let _ = std::fs::remove_file(&test_file);
                            println!("  [PASS] log directory writable: {}", log_dir.display());
                            passed += 1;
                        }
                        Err(e) => {
                            println!("  [FAIL] log directory not writable: {} — {e}", log_dir.display());
                            failed += 1;
                        }
                    }
                } else {
                    println!("  [FAIL] log directory not found: {}", log_dir.display());
                    failed += 1;
                }
            }
        } else {
            println!("  [SKIP] log file not configured");
        }
    }

    // Check 8: Engine libraries directory
    if let Some(ref cfg) = config {
        let engines_dir = &cfg.engines.dir;
        let engines_path = Path::new(engines_dir);
        if engines_path.is_dir() {
            let dylib_count = std::fs::read_dir(engines_path)
                .map(|entries| entries.filter_map(|e| e.ok())
                    .filter(|e| {
                        let name = e.file_name();
                        let s = name.to_string_lossy();
                        s.ends_with(".dylib") || s.ends_with(".so")
                    })
                    .count())
                .unwrap_or(0);
            if dylib_count > 0 {
                println!("  [PASS] engines directory: {engines_dir} ({dylib_count} libraries)");
                passed += 1;
            } else {
                println!("  [WARN] engines directory empty: {engines_dir}");
                passed += 1; // Not a hard failure — static mode has no engine dylibs
            }
        } else {
            println!("  [SKIP] engines directory not found: {engines_dir}");
        }
    }

    // Check 9: Plugins directory
    if let Some(ref cfg) = config {
        let plugins_dir = &cfg.plugins.dir;
        let plugins_path = Path::new(plugins_dir);
        if plugins_path.is_dir() {
            let dylib_count = std::fs::read_dir(plugins_path)
                .map(|entries| entries.filter_map(|e| e.ok())
                    .filter(|e| {
                        let name = e.file_name();
                        let s = name.to_string_lossy();
                        s.ends_with(".dylib") || s.ends_with(".so")
                    })
                    .count())
                .unwrap_or(0);
            if dylib_count > 0 {
                println!("  [PASS] plugins directory: {plugins_dir} ({dylib_count} plugins)");
                passed += 1;
            } else {
                println!("  [WARN] plugins directory empty: {plugins_dir}");
                passed += 1;
            }
        } else {
            println!("  [SKIP] plugins directory not found: {plugins_dir}");
        }
    }

    // Check 10: Bundle/apphome directory
    if let Some(ref cfg) = config {
        if let Some(ref bundle_path) = cfg.bundle_path {
            let bp = Path::new(bundle_path);
            if bp.is_dir() {
                println!("  [PASS] bundle path: {bundle_path}");
                passed += 1;
            } else {
                println!("  [FAIL] bundle path not found: {bundle_path}");
                failed += 1;
            }
        } else {
            println!("  [SKIP] bundle_path not configured");
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
