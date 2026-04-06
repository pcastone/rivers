use std::path::{Path, PathBuf};

use super::start::find_riversd_binary;

/// Run pre-launch health checks. With `--fix`, auto-repair fixable issues.
pub fn cmd_doctor(args: &[String]) -> Result<(), String> {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut fixed = 0u32;

    // Parse flags
    let mut explicit_config: Option<PathBuf> = None;
    let mut fix_mode = false;
    let mut lint_mode = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" if i + 1 < args.len() => {
                explicit_config = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--fix" => {
                fix_mode = true;
                i += 1;
            }
            "--lint" => {
                lint_mode = true;
                i += 1;
            }
            _ => { i += 1; }
        }
    }

    if fix_mode {
        println!("  doctor --fix: will attempt to repair fixable issues");
        println!();
    }

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
                    if fix_mode {
                        match fix_init_lockbox(keystore_path) {
                            Ok(()) => {
                                println!("  [FIXED] lockbox initialized: {keystore_path}");
                                fixed += 1;
                                passed += 1;
                            }
                            Err(e) => {
                                println!("  [FAIL] lockbox not found: {keystore_path} (fix failed: {e})");
                                failed += 1;
                            }
                        }
                    } else {
                        println!("  [FAIL] lockbox keystore not found: {keystore_path}");
                        failed += 1;
                    }
                } else {
                    match check_lockbox_permissions(path) {
                        Ok(()) => {
                            println!("  [PASS] lockbox keystore permissions ok");
                            passed += 1;
                        }
                        Err(e) => {
                            if fix_mode {
                                match fix_lockbox_permissions(path) {
                                    Ok(()) => {
                                        println!("  [FIXED] lockbox keystore permissions → 0600");
                                        fixed += 1;
                                        passed += 1;
                                    }
                                    Err(fe) => {
                                        println!("  [FAIL] lockbox keystore: {e} (fix failed: {fe})");
                                        failed += 1;
                                    }
                                }
                            } else {
                                println!("  [FAIL] lockbox keystore: {e}");
                                failed += 1;
                            }
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
            let cert_path = tls.cert.as_deref();
            let key_path = tls.key.as_deref();
            let cert_exists = cert_path.map(|p| Path::new(p).is_file()).unwrap_or(false);
            let key_exists = key_path.map(|p| Path::new(p).is_file()).unwrap_or(false);

            if let Some(cert) = cert_path {
                if cert_exists {
                    println!("  [PASS] TLS cert: {cert}");
                    passed += 1;
                    // Check cert expiry
                    let expiry = rivers_runtime::rivers_core::tls::cert_expiry_summary(cert);
                    if expiry.starts_with("EXPIRED") {
                        if fix_mode {
                            if let Some(ref key) = tls.key {
                                match rivers_runtime::rivers_core::tls::generate_self_signed_cert(
                                    &tls.x509, cert, key,
                                ) {
                                    Ok(()) => {
                                        println!("  [FIXED] TLS cert renewed (was: {expiry})");
                                        fixed += 1;
                                    }
                                    Err(e) => {
                                        println!("  [FAIL] TLS cert expired ({expiry}) — renewal failed: {e}");
                                        failed += 1;
                                    }
                                }
                            }
                        } else {
                            println!("  [FAIL] TLS cert expired: {expiry}");
                            failed += 1;
                        }
                    } else {
                        println!("  [PASS] TLS cert expiry: {expiry}");
                        passed += 1;
                    }
                } else if fix_mode {
                    if let Some(key) = key_path {
                        match fix_generate_tls_cert(cert, key, &tls.x509) {
                            Ok(()) => {
                                println!("  [FIXED] TLS cert + key generated");
                                fixed += 2;
                                passed += 2;
                            }
                            Err(e) => {
                                println!("  [FAIL] TLS cert not found: {cert} (fix failed: {e})");
                                failed += 1;
                            }
                        }
                    } else {
                        println!("  [FAIL] TLS cert not found: {cert} (no key path configured)");
                        failed += 1;
                    }
                } else {
                    println!("  [FAIL] TLS cert not found: {cert}");
                    failed += 1;
                }
            } else {
                println!("  [SKIP] TLS cert not configured (will auto-generate)");
            }

            // Only check key separately if we didn't already generate both above
            if cert_exists || !fix_mode {
                if let Some(key) = key_path {
                    if key_exists {
                        println!("  [PASS] TLS key: {key}");
                        passed += 1;
                    } else {
                        println!("  [FAIL] TLS key not found: {key}");
                        failed += 1;
                    }
                } else {
                    println!("  [SKIP] TLS key not configured (will auto-generate)");
                }
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
                } else if fix_mode {
                    match std::fs::create_dir_all(log_dir) {
                        Ok(()) => {
                            println!("  [FIXED] log directory created: {}", log_dir.display());
                            fixed += 1;
                            passed += 1;
                        }
                        Err(e) => {
                            println!("  [FAIL] log directory not found: {} (fix failed: {e})", log_dir.display());
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

    // Check: App log directory (if configured)
    if let Some(ref cfg) = config {
        if let Some(ref app_log_dir) = cfg.base.logging.app_log_dir {
            let dir = Path::new(app_log_dir);
            if dir.is_dir() {
                println!("  [PASS] app log directory: {app_log_dir}");
                passed += 1;
            } else if fix_mode {
                match std::fs::create_dir_all(dir) {
                    Ok(()) => {
                        println!("  [FIXED] app log directory created: {app_log_dir}");
                        fixed += 1;
                        passed += 1;
                    }
                    Err(e) => {
                        println!("  [FAIL] app log directory: {app_log_dir} (fix failed: {e})");
                        failed += 1;
                    }
                }
            } else {
                println!("  [FAIL] app log directory not found: {app_log_dir}");
                failed += 1;
            }
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

    // Lint: bundle validation (when --lint is passed)
    if lint_mode {
        if let Some(ref cfg) = config {
            if let Some(ref bundle_path) = cfg.bundle_path {
                println!();
                println!("  === Lint: {bundle_path} ===");
                match rivers_runtime::load_bundle(std::path::Path::new(bundle_path)) {
                    Ok(bundle) => {
                        // Structural validation
                        match rivers_runtime::validate_bundle(&bundle) {
                            Ok(()) => {
                                println!("  [PASS] bundle structure valid");
                                passed += 1;
                            }
                            Err(errors) => {
                                for e in &errors {
                                    println!("  [FAIL] {e}");
                                }
                                failed += errors.len() as u32;
                            }
                        }

                        // Per-app convention checks
                        for app in &bundle.apps {
                            lint_app_conventions(app, &mut passed, &mut failed);
                        }
                    }
                    Err(e) => {
                        println!("  [FAIL] cannot load bundle: {e}");
                        failed += 1;
                    }
                }
            } else {
                println!("  [SKIP] --lint: no bundle_path configured");
            }
        }
    }

    println!();
    if fixed > 0 {
        print!("doctor: {passed} passed, {failed} failed, {fixed} fixed");
    } else {
        print!("doctor: {passed} passed, {failed} failed");
    }
    if failed > 0 && !fix_mode {
        println!(" (run with --fix to repair)");
    } else {
        println!();
    }
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn lint_app_conventions(
    app: &rivers_runtime::LoadedApp,
    passed: &mut u32,
    failed: &mut u32,
) {
    let name = &app.manifest.app_name;

    // Check: views exist (common mistake: [views.*] instead of [api.views.*])
    if app.config.api.views.is_empty() {
        println!("  [WARN] {name}: no views defined — check [api.views.*] (not [views.*])");
    } else {
        println!("  [PASS] {name}: {} views defined", app.config.api.views.len());
        *passed += 1;
    }

    // Check: schema files exist for DataViews that reference them
    for (dv_key, dv) in &app.config.data.dataviews {
        let schema_refs = [
            dv.return_schema.as_deref(),
            dv.get_schema.as_deref(),
            dv.post_schema.as_deref(),
            dv.put_schema.as_deref(),
            dv.delete_schema.as_deref(),
        ];
        for schema_ref in schema_refs.into_iter().flatten() {
            let schema_path = app.app_dir.join(schema_ref);
            if schema_path.exists() {
                *passed += 1;
            } else {
                println!("  [FAIL] {name}: schema not found: {} (expected at {})", schema_ref, schema_path.display());
                *failed += 1;
            }
        }
        let _ = dv_key; // key used for iteration; name field used in DataViewConfig
    }

    // Check: datasource references in DataViews resolve
    for dv in app.config.data.dataviews.values() {
        if app.config.data.datasources.contains_key(&dv.datasource) {
            *passed += 1;
        } else {
            println!("  [FAIL] {name}: dataview '{}' references unknown datasource '{}'", dv.name, dv.datasource);
            *failed += 1;
        }
    }
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

// ── Fix helpers (--fix) ─────────────────────────────────────────────

/// Set lockbox keystore file permissions to 0600.
#[cfg(unix)]
fn fix_lockbox_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod failed: {e}"))
}

#[cfg(not(unix))]
fn fix_lockbox_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Generate a self-signed TLS certificate and key using rivers-core.
fn fix_generate_tls_cert(
    cert_path: &str,
    key_path: &str,
    x509: &rivers_runtime::rivers_core_config::config::TlsX509Config,
) -> Result<(), String> {
    // Ensure parent directories exist
    if let Some(parent) = Path::new(cert_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    rivers_runtime::rivers_core::tls::generate_self_signed_cert(x509, cert_path, key_path)
}

/// Initialize lockbox by running the sibling `rivers-lockbox init` binary.
fn fix_init_lockbox(lockbox_path: &str) -> Result<(), String> {
    // Find rivers-lockbox binary next to riversctl
    let lockbox_bin = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("rivers-lockbox")))
        .ok_or_else(|| "cannot locate rivers-lockbox binary".to_string())?;

    if !lockbox_bin.is_file() {
        return Err(format!("rivers-lockbox not found at {}", lockbox_bin.display()));
    }

    let status = std::process::Command::new(&lockbox_bin)
        .arg("init")
        .env("RIVERS_LOCKBOX_DIR", lockbox_path)
        .status()
        .map_err(|e| format!("failed to run rivers-lockbox: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("rivers-lockbox init exited with {status}"))
    }
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
