#![warn(missing_docs)]
//! cargo-deploy — Build and deploy Rivers runtime to a target directory.
//!
//! Usage:
//!   cargo deploy <path>              # Dynamic mode (bin/ + lib/ for engines)
//!   cargo deploy <path> --static     # Static mode (single fat binary)

use std::path::{Path, PathBuf};
use std::process::Command;

use rivers_core_config::config::TlsX509Config;
use rcgen::{CertificateParams, DistinguishedName, DnType, Ia5String, SanType};

/// Dylib extension for the current platform.
#[cfg(target_os = "macos")]
const DYLIB_EXT: &str = "dylib";
#[cfg(not(target_os = "macos"))]
const DYLIB_EXT: &str = "so";

/// Binary names to deploy.
const BINARIES: &[&str] = &["riversd", "riversctl", "rivers-lockbox", "rivers-keystore", "riverpackage"];

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // cargo passes "deploy" as argv[1]
    let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (path, static_mode) = match parse_args(&args) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("error: {msg}");
            print_usage();
            std::process::exit(1);
        }
    };

    let deploy_path = PathBuf::from(path);
    let workspace_root = find_workspace_root().unwrap_or_else(|| {
        eprintln!("error: not in a cargo workspace");
        std::process::exit(1);
    });
    let target_dir = workspace_root.join("target/release");

    let version = read_workspace_version(&workspace_root);

    // ── Staging / atomic swap ────────────────────────────────────────
    // All file operations target a staging directory.  On success the staging
    // directory is atomically renamed to the final deploy path.  Any crash or
    // error between steps leaves the existing live directory untouched.
    let staging_path = PathBuf::from(format!("{}.staging", deploy_path.display()));

    // Remove a leftover staging dir from a previous interrupted run.
    if staging_path.exists() {
        println!("Removing leftover staging dir: {}", staging_path.display());
        std::fs::remove_dir_all(&staging_path).unwrap_or_else(|e| {
            eprintln!("error: failed to remove staging dir: {e}");
            std::process::exit(1);
        });
    }

    if static_mode {
        println!("=== Rivers Deploy (static) v{version} ===");
        println!("Target: {}", deploy_path.display());
        println!();
        deploy_static(&workspace_root, &target_dir, &staging_path, &version);
    } else {
        println!("=== Rivers Deploy (dynamic) v{version} ===");
        println!("Target: {}", deploy_path.display());
        println!();
        deploy_dynamic(&workspace_root, &target_dir, &staging_path, &version);
    }

    // Preserve TLS certs from the live directory into staging so redeployment
    // does not rotate the certificate. This runs after staging assembly so the
    // staging tls dir already exists, and before the atomic rename.
    preserve_tls_from_live(&deploy_path, &staging_path);

    // Atomic transition: rename live → live.old, then staging → live.
    // This ensures the live path is never absent during the swap — if the
    // process crashes between the two renames, the old version is still at
    // live.old and can be recovered manually. A crash BEFORE staging is complete
    // leaves the existing live dir untouched (staging dir is cleaned on next run).
    let old_path = PathBuf::from(format!("{}.old", deploy_path.display()));
    if old_path.exists() {
        std::fs::remove_dir_all(&old_path).unwrap_or_else(|e| {
            eprintln!("error: failed to remove leftover .old dir: {e}");
            std::process::exit(1);
        });
    }
    if deploy_path.exists() {
        std::fs::rename(&deploy_path, &old_path).unwrap_or_else(|e| {
            eprintln!("error: failed to rename live → live.old: {e}");
            std::process::exit(1);
        });
    }
    std::fs::rename(&staging_path, &deploy_path).unwrap_or_else(|e| {
        eprintln!("error: atomic rename staging → final failed: {e}");
        // Attempt to restore the old dir so the live path is not left absent.
        if old_path.exists() {
            let _ = std::fs::rename(&old_path, &deploy_path);
        }
        std::process::exit(1);
    });
    // Best-effort cleanup of the previous live dir.
    if old_path.exists() {
        let _ = std::fs::remove_dir_all(&old_path);
    }
    println!("Deployed: {}", deploy_path.display());
}

/// Parse CLI arguments. Returns (path, static_mode).
fn parse_args<'a>(args: &'a [&'a str]) -> Result<(&'a str, bool), String> {
    // argv: cargo-deploy deploy <path> [--static]
    // or:   cargo-deploy <path> [--static]
    let skip = if args.len() > 1 && args[1] == "deploy" { 2 } else { 1 };
    let rest: Vec<&&str> = args[skip..].iter().filter(|a| !a.starts_with('-')).collect();
    let flags: Vec<&&str> = args[skip..].iter().filter(|a| a.starts_with('-')).collect();

    if rest.is_empty() {
        return Err("missing <path> argument".to_string());
    }

    let path = rest[0];
    let static_mode = flags.iter().any(|f| **f == "--static");

    // Reject unknown flags
    for f in &flags {
        if **f != "--static" {
            return Err(format!("unknown flag: {f}"));
        }
    }

    Ok((path, static_mode))
}

fn print_usage() {
    eprintln!();
    eprintln!("Usage: cargo deploy <path> [--static]");
    eprintln!();
    eprintln!("  <path>       Target directory to deploy Rivers into");
    eprintln!("  --static     Build single fat binary (default: dynamic with shared libs)");
}

// ── Build helpers ───────────────────────────────────────────────

/// Run cargo build --release with the given arguments. Exits on failure.
fn cargo_build(args: &[&str]) {
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("--release");
    for a in args {
        cmd.arg(a);
    }

    let status = cmd.status().unwrap_or_else(|e| {
        eprintln!("error: failed to run cargo: {e}");
        std::process::exit(1);
    });

    if !status.success() {
        eprintln!("error: cargo build failed");
        std::process::exit(1);
    }
}

// ── Deploy: dynamic mode ────────────────────────────────────────

fn deploy_dynamic(workspace_root: &Path, target_dir: &Path, deploy_path: &Path, version: &str) {
    let _workspace_root = workspace_root; // suppress unused warning

    // Step 1: Build binaries with static drivers (no cdylib plugins)
    println!("[1/4] Building binaries (static drivers)...");
    cargo_build(&[
        "--no-default-features",
        "--features", "static-builtin-drivers",
        "-p", "riversd",
        "-p", "riversctl",
        "-p", "rivers-lockbox",
        "-p", "rivers-keystore",
        "-p", "riverpackage",
    ]);

    // Step 2: Build engine cdylibs (V8, WASM — synchronous C-ABI, safe as cdylib)
    println!("[2/4] Building engine shared libraries...");
    cargo_build(&[
        "-p", "rivers-engine-v8",
        "-p", "rivers-engine-wasm",
    ]);

    // Step 3: Assemble deploy directory (bin/ + lib/ for engines only)
    println!("[3/4] Assembling deploy directory...");
    let bin_dir = deploy_path.join("bin");
    let lib_dir = deploy_path.join("lib");

    create_dir(&bin_dir);
    create_dir(&lib_dir);

    // Copy binaries
    for name in BINARIES {
        copy_file(&target_dir.join(name), &bin_dir.join(name));
    }

    // Copy engine dylibs — required for dynamic mode; absence is fatal.
    let mut missing_engines = Vec::new();
    for engine in &["rivers_engine_v8", "rivers_engine_wasm"] {
        let filename = format!("lib{engine}.{DYLIB_EXT}");
        let src = target_dir.join(&filename);
        if src.exists() {
            copy_file(&src, &lib_dir.join(&filename));
        } else {
            missing_engines.push(filename);
        }
    }
    if !missing_engines.is_empty() {
        eprintln!("error: dynamic deploy requires engine libraries that were not built:");
        for name in &missing_engines {
            eprintln!("  missing: {name}");
        }
        eprintln!("  Run: cargo build --release -p rivers-engine-v8 -p rivers-engine-wasm");
        std::process::exit(1);
    }

    // Step 4: Runtime scaffolding
    println!("[4/4] Scaffolding runtime...");
    scaffold_runtime(deploy_path, version, "dynamic", workspace_root);

    print_summary(deploy_path, workspace_root);
}

// ── Deploy: static mode ─────────────────────────────────────────

fn deploy_static(_workspace_root: &Path, target_dir: &Path, deploy_path: &Path, version: &str) {
    // Build with default features (static-engines + static-plugins + static-builtin-drivers)
    println!("[1/3] Building binaries (static)...");
    cargo_build(&[
        "-p", "riversd",
        "-p", "riversctl",
        "-p", "rivers-lockbox",
        "-p", "rivers-keystore",
        "-p", "riverpackage",
    ]);

    // Create directory structure (no lib/ or plugins/)
    println!("[2/3] Assembling deploy directory...");
    let bin_dir = deploy_path.join("bin");
    create_dir(&bin_dir);

    // Copy binaries
    for name in BINARIES {
        copy_file(&target_dir.join(name), &bin_dir.join(name));
    }

    // Runtime scaffolding
    println!("[3/3] Scaffolding runtime...");
    scaffold_runtime(deploy_path, version, "static", _workspace_root);

    print_summary(deploy_path, _workspace_root);
}

// ── Shared helpers ──────────────────────────────────────────────

/// Create all runtime directories, config, TLS certs, lockbox, VERSION, and copy guides.
fn scaffold_runtime(deploy_path: &Path, version: &str, mode: &str, workspace_root: &Path) {
    let tls_dir = deploy_path.join("config/tls");
    let log_dir = deploy_path.join("log");
    let app_log_dir = deploy_path.join("log/apps");
    let apphome_dir = deploy_path.join("apphome");
    let data_dir = deploy_path.join("data");
    let lockbox_dir = deploy_path.join("lockbox");

    create_dir(&tls_dir);
    create_dir(&log_dir);
    create_dir(&app_log_dir);
    create_dir(&apphome_dir);
    create_dir(&data_dir);

    // TLS certificate
    println!("  generating TLS certificate...");
    generate_tls(&tls_dir);

    // Default config
    println!("  writing config/riversd.toml...");
    write_default_config(deploy_path);

    // Lockbox
    println!("  initializing lockbox...");
    init_lockbox(deploy_path, &lockbox_dir);

    // Copy guide documentation
    println!("  copying docs/guide...");
    copy_guides(workspace_root, deploy_path);

    // Copy arch specs
    println!("  copying docs/arch...");
    copy_arch_specs(workspace_root, deploy_path);

    // Copy rivers.d.ts so deployed handlers can reference types
    println!("  copying types/rivers.d.ts...");
    copy_type_definitions(workspace_root, deploy_path);

    // VERSION
    write_version_file(deploy_path, version, mode);
}

/// Copy the `docs/guide` directory from the workspace into the deploy path.
fn copy_guides(workspace_root: &Path, deploy_path: &Path) {
    let src = workspace_root.join("docs/guide");
    let dst = deploy_path.join("docs/guide");
    if !src.is_dir() {
        eprintln!("  warn: docs/guide not found at {}, skipping", src.display());
        return;
    }
    if let Err(e) = copy_dir_recursive(&src, &dst) {
        eprintln!("  warn: failed to copy guides: {e}");
    } else {
        println!("  guides: {}", dst.display());
    }
}

/// Copy `types/rivers.d.ts` into the deployed instance so handler authors
/// can reference it from their `tsconfig.json` without extra setup.
/// Spec: `rivers-javascript-typescript-spec.md §8.2`.
fn copy_type_definitions(workspace_root: &Path, deploy_path: &Path) {
    let src = workspace_root.join("types/rivers.d.ts");
    if !src.is_file() {
        eprintln!(
            "  warn: types/rivers.d.ts not found at {}, skipping",
            src.display()
        );
        return;
    }
    let dst_dir = deploy_path.join("types");
    create_dir(&dst_dir);
    let dst = dst_dir.join("rivers.d.ts");
    copy_file(&src, &dst);
    println!("  types: {}", dst.display());
}

/// Copy the `docs/arch` directory from the workspace into the deploy path.
fn copy_arch_specs(workspace_root: &Path, deploy_path: &Path) {
    let src = workspace_root.join("docs/arch");
    let dst = deploy_path.join("docs/arch");
    if !src.exists() {
        eprintln!("  warn: docs/arch not found at {}, skipping", src.display());
        return;
    }
    if let Err(e) = copy_dir_recursive(&src, &dst) {
        eprintln!("  warn: failed to copy arch specs: {e}");
    } else {
        println!("  arch specs: {}", dst.display());
    }
}

/// Recursively copy a directory tree. Used for docs/guide.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn generate_tls(tls_dir: &Path) {
    let cert_path = tls_dir.join("server.crt");
    let key_path = tls_dir.join("server.key");

    let x509 = TlsX509Config::default();
    generate_self_signed_cert(&x509, &cert_path, &key_path)
        .unwrap_or_else(|e| {
            eprintln!("error: TLS cert generation failed: {e}");
            std::process::exit(1);
        });

    println!("  cert: {}", cert_path.display());
    println!("  key:  {}", key_path.display());
}

/// If the live deploy directory already has TLS certs, copy them into staging
/// so that redeployment does not rotate the cert unexpectedly. Use
/// `riversctl tls renew` to intentionally rotate certificates.
fn preserve_tls_from_live(live_deploy_path: &Path, staging_deploy_path: &Path) {
    let live_cert = live_deploy_path.join("config/tls/server.crt");
    let live_key = live_deploy_path.join("config/tls/server.key");
    if !live_cert.exists() || !live_key.exists() {
        return;
    }
    let staging_cert = staging_deploy_path.join("config/tls/server.crt");
    let staging_key = staging_deploy_path.join("config/tls/server.key");
    if let Err(e) = std::fs::copy(&live_cert, &staging_cert) {
        eprintln!("warning: could not preserve live TLS cert: {e}");
        return;
    }
    if let Err(e) = std::fs::copy(&live_key, &staging_key) {
        eprintln!("warning: could not preserve live TLS key: {e}");
        return;
    }
    println!("  TLS: preserved existing cert from live path (use `riversctl tls renew` to rotate)");
}

/// Write the default riversd.toml config.
fn write_default_config(deploy_path: &Path) {
    let config_path = deploy_path.join("config/riversd.toml");

    // Canonicalize to get absolute path for all config references
    let root = deploy_path.canonicalize().unwrap_or_else(|_| deploy_path.to_path_buf());
    let r = root.display();

    let config = format!(
        r#"# riversd.toml — Rivers server configuration
#
# Uncomment bundle_path to load an application bundle at startup:
# bundle_path = "{r}/apphome/<your-bundle>/"

[base]
host      = "0.0.0.0"
port      = 8080
log_level = "info"

[base.logging]
level           = "info"
format          = "json"
local_file_path = "{r}/log/riversd.log"
app_log_dir     = "{r}/log/apps"

[base.tls]
cert     = "{r}/config/tls/server.crt"
key      = "{r}/config/tls/server.key"
redirect = false

[base.tls.x509]
common_name = "localhost"
san         = ["localhost", "127.0.0.1"]
days        = 365

[storage_engine]
backend = "memory"

[lockbox]
path       = "{r}/lockbox"
key_source = "file"
key_file   = "{r}/lockbox/identity.key"

[engines]
dir = "{r}/lib"

# [plugins]
# cdylib driver plugins disabled — all drivers compiled statically.
# Plugin ABI v2 (synchronous C-ABI) will re-enable dynamic loading.
# dir = "{r}/plugins"

# [base.admin_api]
# enabled = true
# host    = "127.0.0.1"
# port    = 9090
"#
    );
    std::fs::write(&config_path, &config).unwrap_or_else(|e| {
        eprintln!("error: failed to write config: {e}");
        std::process::exit(1);
    });
    println!("  config: {}", config_path.display());
}

/// Initialize a lockbox directory with identity key, entries dir, and aliases.
fn init_lockbox(deploy_path: &Path, lockbox_dir: &Path) {
    let bin_dir = deploy_path.join("bin");
    let lockbox_bin = bin_dir.join("rivers-lockbox");

    if !lockbox_bin.exists() {
        eprintln!("  warn: rivers-lockbox binary not found, skipping lockbox init");
        return;
    }

    let status = Command::new(&lockbox_bin)
        .arg("init")
        .env("RIVERS_LOCKBOX_DIR", lockbox_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("  lockbox: {}", lockbox_dir.display());
        }
        Ok(s) => {
            eprintln!("  warn: lockbox init exited with {s}");
        }
        Err(e) => {
            eprintln!("  warn: lockbox init failed: {e}");
        }
    }
}

/// Generate a self-signed certificate using rcgen, write PEM files to disk.
fn generate_self_signed_cert(
    x509: &TlsX509Config,
    cert_path: &Path,
    key_path: &Path,
) -> Result<(), String> {
    let mut params = CertificateParams::default();

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, &x509.common_name);
    if let Some(ref org) = x509.organization {
        dn.push(DnType::OrganizationName, org);
    }
    if let Some(ref country) = x509.country {
        dn.push(DnType::CountryName, country);
    }
    if let Some(ref state) = x509.state {
        dn.push(DnType::StateOrProvinceName, state);
    }
    if let Some(ref locality) = x509.locality {
        dn.push(DnType::LocalityName, locality);
    }
    params.distinguished_name = dn;

    let mut sans = Vec::new();
    for s in &x509.san {
        if let Ok(ip) = s.parse::<std::net::IpAddr>() {
            sans.push(SanType::IpAddress(ip));
        } else {
            let ia5 = Ia5String::try_from(s.clone())
                .map_err(|e| format!("invalid DNS SAN '{s}': {e}"))?;
            sans.push(SanType::DnsName(ia5));
        }
    }
    params.subject_alt_names = sans;

    let not_before = ::time::OffsetDateTime::now_utc();
    let not_after = not_before + ::time::Duration::days(x509.days as i64);
    params.not_before = not_before;
    params.not_after = not_after;

    let key_pair = rcgen::KeyPair::generate()
        .map_err(|e| format!("key generation failed: {e}"))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| format!("cert generation failed: {e}"))?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    if let Some(parent) = cert_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create cert dir {}: {e}", parent.display()))?;
    }

    std::fs::write(cert_path, cert_pem)
        .map_err(|e| format!("failed to write cert to {}: {e}", cert_path.display()))?;

    // Write the private key with 0o600 permissions from the start so there is
    // never a window where it is world-readable. On Unix, `OpenOptions::mode`
    // sets the permission bits at create time before any bytes are written.
    // On non-Unix targets we fall back to a plain write.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(key_path)
            .map_err(|e| format!("failed to create key file {}: {e}", key_path.display()))?;
        f.write_all(key_pem.as_bytes())
            .map_err(|e| format!("failed to write key to {}: {e}", key_path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(key_path, key_pem)
            .map_err(|e| format!("failed to write key to {}: {e}", key_path.display()))?;
    }

    Ok(())
}

fn write_version_file(deploy_path: &Path, version: &str, mode: &str) {
    let content = format!(
        "Rivers v{version}\n\
         Build: release ({mode})\n\
         Date: {}\n\
         Platform: {} {}\n",
        chrono_date(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    let path = deploy_path.join("VERSION");
    std::fs::write(&path, content).unwrap_or_else(|e| {
        eprintln!("error: failed to write VERSION: {e}");
        std::process::exit(1);
    });
}

fn chrono_date() -> String {
    // Simple date without pulling in chrono
    let output = Command::new("date").arg("+%Y-%m-%d").output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => "unknown".to_string(),
    }
}

fn print_summary(deploy_path: &Path, _workspace_root: &Path) {
    println!();
    println!("=== Deploy Summary ===");
    println!("Directory: {}", deploy_path.display());
    println!();

    let bin_dir = deploy_path.join("bin");
    if bin_dir.exists() {
        println!("Binaries:");
        print_dir_listing(&bin_dir);
    }

    let lib_dir = deploy_path.join("lib");
    if lib_dir.exists() {
        println!("Engine Libraries:");
        print_dir_listing(&lib_dir);
    }

    // Runtime structure
    let config_path = deploy_path.join("config/riversd.toml");
    if config_path.exists() {
        println!("Config:    config/riversd.toml");
    }
    let tls_dir = deploy_path.join("config/tls");
    if tls_dir.exists() {
        println!("TLS:       config/tls/server.crt, config/tls/server.key");
    }
    let lockbox_dir = deploy_path.join("lockbox");
    if lockbox_dir.exists() {
        println!("Lockbox:   lockbox/");
    }
    println!("Logs:      log/");
    println!("Bundles:   apphome/");

    let version_path = deploy_path.join("VERSION");
    if version_path.exists() {
        println!();
        println!("Version:");
        if let Ok(v) = std::fs::read_to_string(&version_path) {
            for line in v.lines() {
                println!("  {line}");
            }
        }
    }

    println!();
    println!("Ready to run:");
    println!("  cd {}", deploy_path.display());
    println!("  ./bin/riversctl doctor");
    println!("  ./bin/riversctl start");
}

fn print_dir_listing(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            let name = entry.file_name();
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let size_mb = size as f64 / (1024.0 * 1024.0);
            println!("  {:<45} {:>6.1} MB", name.to_string_lossy(), size_mb);
        }
    }
    println!();
}

fn create_dir(path: &Path) {
    std::fs::create_dir_all(path).unwrap_or_else(|e| {
        eprintln!("error: failed to create {}: {e}", path.display());
        std::process::exit(1);
    });
}

fn copy_file(src: &Path, dst: &Path) {
    std::fs::copy(src, dst).unwrap_or_else(|e| {
        eprintln!("error: failed to copy {} -> {}: {e}", src.display(), dst.display());
        std::process::exit(1);
    });
}

fn find_workspace_root() -> Option<PathBuf> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version=1"])
        .output()
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    json.get("workspace_root")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
}

fn read_workspace_version(workspace_root: &Path) -> String {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml).unwrap_or_default();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("version") && trimmed.contains('"') {
            if let Some(start) = trimmed.find('"') {
                if let Some(end) = trimmed[start + 1..].find('"') {
                    return trimmed[start + 1..start + 1 + end].to_string();
                }
            }
        }
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_basic() {
        let args = ["cargo-deploy", "deploy", "/tmp/rivers"];
        let (path, static_mode) = parse_args(&args).unwrap();
        assert_eq!(path, "/tmp/rivers");
        assert!(!static_mode);
    }

    #[test]
    fn parse_args_static_flag() {
        let args = ["cargo-deploy", "deploy", "/tmp/rivers", "--static"];
        let (path, static_mode) = parse_args(&args).unwrap();
        assert_eq!(path, "/tmp/rivers");
        assert!(static_mode);
    }

    #[test]
    fn parse_args_missing_path() {
        let args = ["cargo-deploy", "deploy"];
        assert!(parse_args(&args).is_err());
    }
}
