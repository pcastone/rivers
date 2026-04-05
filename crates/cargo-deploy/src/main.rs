#![warn(missing_docs)]
//! cargo-deploy — Build and deploy Rivers runtime to a target directory.
//!
//! Usage:
//!   cargo deploy <path>              # Dynamic mode (bin/ + lib/ + plugins/)
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

/// All plugin crate names.
const PLUGINS: &[&str] = &[
    "rivers-plugin-cassandra",
    "rivers-plugin-couchdb",
    "rivers-plugin-elasticsearch",
    "rivers-plugin-exec",
    "rivers-plugin-influxdb",
    "rivers-plugin-kafka",
    "rivers-plugin-ldap",
    "rivers-plugin-mongodb",
    "rivers-plugin-nats",
    "rivers-plugin-neo4j",
    "rivers-plugin-rabbitmq",
    "rivers-plugin-redis-streams",
];

/// Plugin library names (underscores, matching the .dylib/.so output).
const PLUGIN_LIB_NAMES: &[&str] = &[
    "rivers_plugin_cassandra",
    "rivers_plugin_couchdb",
    "rivers_plugin_elasticsearch",
    "rivers_plugin_exec",
    "rivers_plugin_influxdb",
    "rivers_plugin_kafka",
    "rivers_plugin_ldap",
    "rivers_plugin_mongodb",
    "rivers_plugin_nats",
    "rivers_plugin_neo4j",
    "rivers_plugin_rabbitmq",
    "rivers_plugin_redis_streams",
];

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

    if static_mode {
        println!("=== Rivers Deploy (static) v{version} ===");
        println!("Target: {}", deploy_path.display());
        println!();
        deploy_static(&workspace_root, &target_dir, &deploy_path, &version);
    } else {
        println!("=== Rivers Deploy (dynamic) v{version} ===");
        println!("Target: {}", deploy_path.display());
        println!();
        deploy_dynamic(&workspace_root, &target_dir, &deploy_path, &version);
    }
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
    // Build binaries — disable static engines/plugins but keep builtin drivers (sqlite, faker, etc.)
    println!("[1/5] Building binaries (dynamic)...");
    cargo_build(&[
        "--no-default-features",
        "--features", "static-builtin-drivers",
        "-p", "riversd",
        "-p", "riversctl",
        "-p", "rivers-lockbox",
        "-p", "rivers-keystore",
        "-p", "riverpackage",
    ]);

    // Build engine cdylibs
    println!("[2/5] Building engine shared libraries...");
    cargo_build(&[
        "-p", "rivers-engine-v8",
        "-p", "rivers-engine-wasm",
    ]);

    // Build plugin cdylibs
    println!("[3/5] Building plugin shared libraries...");
    cargo_build(&plugin_build_args());

    // Create directory structure
    println!("[4/5] Assembling deploy directory...");
    let bin_dir = deploy_path.join("bin");
    let lib_dir = deploy_path.join("lib");
    let plugins_dir = deploy_path.join("plugins");

    create_dir(&bin_dir);
    create_dir(&lib_dir);
    create_dir(&plugins_dir);

    // Copy binaries
    for name in BINARIES {
        copy_file(&target_dir.join(name), &bin_dir.join(name));
    }

    // Copy engine dylibs
    for engine in &["rivers_engine_v8", "rivers_engine_wasm"] {
        let filename = format!("lib{engine}.{DYLIB_EXT}");
        let src = target_dir.join(&filename);
        if src.exists() {
            copy_file(&src, &lib_dir.join(&filename));
        } else {
            eprintln!("  warn: {filename} not found, skipping");
        }
    }

    // Copy plugin dylibs
    for plugin_lib in PLUGIN_LIB_NAMES {
        let filename = format!("lib{plugin_lib}.{DYLIB_EXT}");
        let src = target_dir.join(&filename);
        if src.exists() {
            copy_file(&src, &plugins_dir.join(&filename));
        } else {
            eprintln!("  warn: {filename} not found, skipping");
        }
    }

    // Runtime scaffolding
    println!("[5/5] Scaffolding runtime...");
    scaffold_runtime(deploy_path, version, "dynamic");

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
    scaffold_runtime(deploy_path, version, "static");

    print_summary(deploy_path, _workspace_root);
}

// ── Shared helpers ──────────────────────────────────────────────

/// Create all runtime directories, config, TLS certs, lockbox, and VERSION.
fn scaffold_runtime(deploy_path: &Path, version: &str, mode: &str) {
    let tls_dir = deploy_path.join("config/tls");
    let log_dir = deploy_path.join("log");
    let apphome_dir = deploy_path.join("apphome");
    let data_dir = deploy_path.join("data");
    let lockbox_dir = deploy_path.join("lockbox");

    create_dir(&tls_dir);
    create_dir(&log_dir);
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

    // VERSION
    write_version_file(deploy_path, version, mode);
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

[plugins]
dir = "{r}/plugins"

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
    std::fs::write(key_path, key_pem)
        .map_err(|e| format!("failed to write key to {}: {e}", key_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod key file {}: {e}", key_path.display()))?;
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

    let plugins_dir = deploy_path.join("plugins");
    if plugins_dir.exists() {
        println!("Plugin Libraries:");
        print_dir_listing(&plugins_dir);
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

/// Build the cargo args for plugin cdylibs. Extracted for testability.
fn plugin_build_args() -> Vec<&'static str> {
    let mut args: Vec<&str> = vec!["--features", "plugin-exports"];
    for p in PLUGINS {
        args.push("-p");
        args.push(p);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_build_args_include_plugin_exports_feature() {
        let args = plugin_build_args();
        assert!(
            args.windows(2).any(|w| w == ["--features", "plugin-exports"]),
            "plugin build must pass --features plugin-exports to export ABI symbols"
        );
    }

    #[test]
    fn plugin_build_args_include_all_plugins() {
        let args = plugin_build_args();
        for plugin in PLUGINS {
            assert!(
                args.contains(plugin),
                "missing plugin in build args: {plugin}"
            );
        }
    }

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

    /// E2E: build a plugin with plugin-exports and verify the dylib exports _rivers_abi_version.
    /// Run with: cargo test -p cargo-deploy -- --ignored
    #[test]
    #[ignore]
    fn plugin_dylib_exports_abi_version_symbol() {
        let workspace = find_workspace_root().expect("must run inside workspace");
        let target_dir = workspace.join("target/release");

        // Build one plugin with the feature flag
        let status = Command::new("cargo")
            .args(["build", "--release", "--features", "plugin-exports", "-p", "rivers-plugin-nats"])
            .status()
            .expect("cargo build failed to start");
        assert!(status.success(), "cargo build failed");

        let dylib = target_dir.join(format!("librivers_plugin_nats.{DYLIB_EXT}"));
        assert!(dylib.exists(), "plugin dylib not found at {}", dylib.display());

        // Check for the ABI symbol using nm
        let output = Command::new("nm")
            .args(["-g", dylib.to_str().unwrap()])
            .output()
            .expect("nm failed to start");
        let symbols = String::from_utf8_lossy(&output.stdout);

        assert!(
            symbols.contains("_rivers_abi_version"),
            "plugin dylib missing _rivers_abi_version symbol.\n\
             This means --features plugin-exports was not applied.\n\
             nm output:\n{symbols}"
        );
        assert!(
            symbols.contains("_rivers_register_driver"),
            "plugin dylib missing _rivers_register_driver symbol.\n\
             nm output:\n{symbols}"
        );
    }
}
