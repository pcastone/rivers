#![warn(missing_docs)]
//! cargo-deploy — Build and deploy Rivers runtime to a target directory.
//!
//! Usage:
//!   cargo deploy <path>              # Dynamic mode (bin/ + lib/ + plugins/)
//!   cargo deploy <path> --static     # Static mode (single fat binary)

use std::path::{Path, PathBuf};
use std::process::Command;

use rivers_core_config::config::TlsX509Config;

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
    "rivers-plugin-influxdb",
    "rivers-plugin-kafka",
    "rivers-plugin-ldap",
    "rivers-plugin-mongodb",
    "rivers-plugin-nats",
    "rivers-plugin-rabbitmq",
    "rivers-plugin-redis-streams",
];

/// Plugin library names (underscores, matching the .dylib/.so output).
const PLUGIN_LIB_NAMES: &[&str] = &[
    "rivers_plugin_cassandra",
    "rivers_plugin_couchdb",
    "rivers_plugin_elasticsearch",
    "rivers_plugin_influxdb",
    "rivers_plugin_kafka",
    "rivers_plugin_ldap",
    "rivers_plugin_mongodb",
    "rivers_plugin_nats",
    "rivers_plugin_rabbitmq",
    "rivers_plugin_redis_streams",
];

/// Binary names to deploy.
const BINARIES: &[&str] = &["riversd", "riversctl", "rivers-lockbox", "riverpackage"];

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
    // Build binaries without static features
    println!("[1/5] Building binaries (dynamic)...");
    cargo_build(&[
        "--no-default-features",
        "-p", "riversd",
        "-p", "riversctl",
        "-p", "rivers-lockbox",
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
    let mut plugin_args: Vec<&str> = Vec::new();
    for p in PLUGINS {
        plugin_args.push("-p");
        plugin_args.push(p);
    }
    cargo_build(&plugin_args);

    // Create directory structure
    println!("[4/5] Assembling deploy directory...");
    let bin_dir = deploy_path.join("bin");
    let lib_dir = deploy_path.join("lib");
    let plugins_dir = deploy_path.join("plugins");
    let tls_dir = deploy_path.join("config/tls");

    create_dir(&bin_dir);
    create_dir(&lib_dir);
    create_dir(&plugins_dir);
    create_dir(&tls_dir);

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

    // TLS + VERSION
    println!("[5/5] Generating TLS certificate...");
    generate_tls(&tls_dir);
    write_version_file(deploy_path, version, "dynamic");

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
        "-p", "riverpackage",
    ]);

    // Create directory structure (no lib/ or plugins/)
    println!("[2/3] Assembling deploy directory...");
    let bin_dir = deploy_path.join("bin");
    let tls_dir = deploy_path.join("config/tls");

    create_dir(&bin_dir);
    create_dir(&tls_dir);

    // Copy binaries
    for name in BINARIES {
        copy_file(&target_dir.join(name), &bin_dir.join(name));
    }

    // TLS + VERSION
    println!("[3/3] Generating TLS certificate...");
    generate_tls(&tls_dir);
    write_version_file(deploy_path, version, "static");

    print_summary(deploy_path, _workspace_root);
}

// ── Shared helpers ──────────────────────────────────────────────

fn generate_tls(tls_dir: &Path) {
    let cert_path = tls_dir.join("server.crt");
    let key_path = tls_dir.join("server.key");

    let x509 = TlsX509Config::default();
    rivers_core::tls::generate_self_signed_cert(
        &x509,
        cert_path.to_str().unwrap(),
        key_path.to_str().unwrap(),
    )
    .unwrap_or_else(|e| {
        eprintln!("error: TLS cert generation failed: {e}");
        std::process::exit(1);
    });

    println!("  cert: {}", cert_path.display());
    println!("  key:  {}", key_path.display());
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

    let version_path = deploy_path.join("VERSION");
    if version_path.exists() {
        println!("Version:");
        if let Ok(v) = std::fs::read_to_string(&version_path) {
            for line in v.lines() {
                println!("  {line}");
            }
        }
    }

    println!();
    println!("Done.");
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
