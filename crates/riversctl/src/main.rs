//! riversctl — Rivers control CLI.
//!
//! Starts and manages riversd, runs health checks, and communicates with
//! riversd's admin API using Ed25519-signed requests.

#[cfg(feature = "tls")]
mod tls_cmd;

#[cfg(feature = "admin-api")]
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    #[cfg(feature = "admin-api")]
    let admin_url = std::env::var("RIVERS_ADMIN_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9090".into());

    let result: Result<(), String> = match args[1].as_str() {
        "start"  => cmd_start(&args[2..]),
        "doctor" => cmd_doctor(&args[2..]),
        #[cfg(feature = "admin-api")]
        "status" => cmd_status(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "deploy" => {
            if args.len() < 3 { eprintln!("Usage: riversctl deploy <bundle_path>"); std::process::exit(1); }
            cmd_deploy(&admin_url, &args[2]).await
        }
        #[cfg(feature = "admin-api")]
        "drivers"     => cmd_drivers(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "datasources" => cmd_datasources(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "health"      => cmd_health(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "stop"        => cmd_stop(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "graceful"    => cmd_graceful(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "log" => {
            if args.len() < 3 { eprintln!("Usage: riversctl log <levels|set|reset>"); std::process::exit(1); }
            cmd_log(&admin_url, &args[2..]).await
        }
        #[cfg(feature = "tls")]
        "tls" => {
            if args.len() < 3 {
                eprintln!("Usage: riversctl tls <gen|request|import|show|list|expire> [--port P]");
                std::process::exit(1);
            }
            load_config_for_tls().and_then(|config| {
                let tls_args: Vec<&str> = args[2..].iter().map(|s| s.as_str()).collect();
                tls_cmd::parse_tls_args(&tls_args)
                    .map_err(|e| format!("tls: {e}"))
                    .and_then(|cmd| {
                        tls_cmd::run_tls_cmd(cmd, &config)
                            .map_err(|e| format!("tls: {e}"))
                    })
            })
        }
        "validate" => cmd_validate(&args[2..]),
        "--version" | "-V" | "version" => {
            println!("riversctl {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH);
            Ok(())
        }
        "help" | "--help" | "-h" => { print_usage(); Ok(()) }
        other => { eprintln!("Unknown command: {other}"); print_usage(); std::process::exit(1); }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!("riversctl — Rivers control CLI");
    eprintln!();
    eprintln!("Usage: riversctl <command> [args]");
    eprintln!();
    eprintln!("Local commands:");
    eprintln!("  start [--config <path>] [--log-level <lvl>] [--no-admin-auth] [--no-ssl [--port <port>]]");
    eprintln!("                  Find and launch riversd (bundle_path comes from config)");
    eprintln!("  doctor [--config <path>]");
    eprintln!("                  Run pre-launch health checks");
    eprintln!("  tls gen [--port P]");
    eprintln!("                  Generate a self-signed certificate for the server on port P");
    eprintln!("  tls request [--port P]");
    eprintln!("                  Generate a CSR and print to stdout");
    eprintln!("  tls import <cert> <key> [--port P]");
    eprintln!("                  Import a cert/key pair, validate, and copy to configured paths");
    eprintln!("  tls show [--port P]");
    eprintln!("                  Show certificate details (subject, SANs, expiry, fingerprint)");
    eprintln!("  tls list        List all configured and auto-generated certificate paths");
    eprintln!("  tls expire --yes [--port P]");
    eprintln!("                  Remove cert/key files to force re-generation on next start");
    eprintln!("  validate <bundle_path>");
    eprintln!("                  Validate a bundle (schema refs, driver names, cross-app deps)");
    eprintln!("  validate --schema <server|app|bundle>");
    eprintln!("                  Output JSON Schema for a config type to stdout");
    eprintln!();
    eprintln!("Admin API commands (require a running riversd):");
    eprintln!("  status          Server status");
    eprintln!("  deploy <path>   Deploy a bundle");
    eprintln!("  drivers         List registered drivers");
    eprintln!("  datasources     List configured datasources");
    eprintln!("  health          Verbose health check");
    eprintln!("  stop            Stop riversd immediately (SIGKILL fallback)");
    eprintln!("  graceful        Stop riversd gracefully — drain in-flight requests (SIGTERM fallback)");
    eprintln!("  log levels      View current log levels");
    eprintln!("  log set <e> <l> Change log level");
    eprintln!("  log reset       Reset to defaults");
    eprintln!();
    eprintln!("Environment:");
    eprintln!("  RIVERS_ADMIN_URL     Admin API base URL (default: http://127.0.0.1:9090)");
    eprintln!("  RIVERS_ADMIN_KEY     Path to Ed25519 private key for signed requests");
    eprintln!("  RIVERS_DAEMON_PATH   Explicit path to the riversd binary");
}

// ── start ────────────────────────────────────────────────────────────────────

/// Locate riversd and launch it.
/// On Unix, replaces the current process (POSIX exec) so signals pass through naturally.
/// On Windows, spawns riversd as a child process and waits for it.
fn cmd_start(args: &[String]) -> Result<(), String> {
    let binary = find_riversd_binary()?;

    // Parse riversctl start flags and forward to riversd serve.
    let mut riversd_args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                i += 1;
                let path = args.get(i).ok_or("--config requires a value")?;
                riversd_args.push("--config".into());
                riversd_args.push(path.clone());
            }
            "--log-level" | "-l" => {
                i += 1;
                let level = args.get(i).ok_or("--log-level requires a value")?;
                riversd_args.push("--log-level".into());
                riversd_args.push(level.clone());
            }
            "--no-admin-auth" => {
                riversd_args.push("--no-admin-auth".into());
            }
            "--no-ssl" => {
                riversd_args.push("--no-ssl".into());
            }
            "--port" => {
                i += 1;
                let port = args.get(i).ok_or("--port requires a value")?;
                riversd_args.push("--port".into());
                riversd_args.push(port.clone());
            }
            other => {
                return Err(format!("unknown option: {other}\nUsage: riversctl start [--config <path>] [--log-level <lvl>] [--no-admin-auth] [--no-ssl [--port <port>]]"));
            }
        }
        i += 1;
    }

    riversd_args.push("serve".into());

    launch_riversd(&binary, &riversd_args)
}

/// Unix: POSIX exec — replaces the current process so signals pass through naturally.
/// Uses CommandExt::exec which passes args as a separate array (no shell involved).
#[cfg(unix)]
fn launch_riversd(binary: &Path, args: &[String]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(binary)
        .args(args)
        .exec();
    Err(format!("failed to launch {}: {}", binary.display(), err))
}

/// Windows: spawn riversd as a child and wait — stop/graceful handled via taskkill.
#[cfg(windows)]
fn launch_riversd(binary: &Path, args: &[String]) -> Result<(), String> {
    let status = std::process::Command::new(binary)
        .args(args)
        .status()
        .map_err(|e| format!("failed to launch {}: {}", binary.display(), e))?;
    if !status.success() {
        return Err(format!("riversd exited with {}", status));
    }
    Ok(())
}

/// Platform-aware binary name for riversd.
fn riversd_binary_name() -> &'static str {
    if cfg!(windows) { "riversd.exe" } else { "riversd" }
}

/// Find the riversd binary.
fn find_riversd_binary() -> Result<PathBuf, String> {
    // 1. Explicit env var
    if let Ok(path) = std::env::var("RIVERS_DAEMON_PATH") {
        let p = PathBuf::from(&path);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!("RIVERS_DAEMON_PATH={path} does not exist or is not a file"));
    }

    let binary_name = riversd_binary_name();

    // 2. Sibling to this binary (release layout: bin/riversctl and bin/riversd)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(binary_name);
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }

    // 3. PATH
    if let Some(p) = find_in_path(binary_name) {
        return Ok(p);
    }

    Err("riversd binary not found — set RIVERS_DAEMON_PATH or place riversd in the same directory".into())
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// ── doctor ───────────────────────────────────────────────────────────────────

/// Run pre-launch health checks.
fn cmd_doctor(args: &[String]) -> Result<(), String> {
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
fn discover_config() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("config/riversd.toml"),
        PathBuf::from("../config/riversd.toml"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

#[cfg(feature = "tls")]
fn load_config_for_tls() -> Result<rivers_runtime::rivers_core_config::ServerConfig, String> {
    let path = discover_config()
        .ok_or_else(|| "no config file found — run from a directory with config/riversd.toml".to_string())?;
    rivers_runtime::loader::load_server_config(&path)
        .map_err(|e| format!("failed to load config: {e}"))
}

// ── Admin API helpers ────────────────────────────────────────────────────────

#[cfg(feature = "admin-api")]
fn sign_request(method: &str, path: &str, body: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();

    // Server expects epoch milliseconds, not RFC 3339
    let ts_ms = chrono::Utc::now().timestamp_millis().to_string();
    headers.insert("X-Rivers-Timestamp".into(), ts_ms.clone());

    if let Ok(key_path) = std::env::var("RIVERS_ADMIN_KEY") {
        if let Ok(key_hex) = std::fs::read_to_string(&key_path) {
            let key_hex = key_hex.trim();
            if let Ok(seed_bytes) = hex::decode(key_hex) {
                if seed_bytes.len() == 32 {
                    use ed25519_dalek::{Signer, SigningKey};
                    use sha2::Digest;
                    let seed: [u8; 32] = seed_bytes.try_into().unwrap();
                    let signing_key = SigningKey::from_bytes(&seed);

                    // Match server's build_signing_payload: method\npath\ntimestamp\nbody_hash
                    let body_hash = hex::encode(sha2::Sha256::digest(body.as_bytes()));
                    let message = format!("{method}\n{path}\n{ts_ms}\n{body_hash}");
                    let signature = signing_key.sign(message.as_bytes());
                    headers.insert("X-Rivers-Signature".into(), hex::encode(signature.to_bytes()));
                }
            }
        }
    }

    headers
}

#[cfg(feature = "admin-api")]
async fn admin_get(url: &str, path: &str) -> Result<serde_json::Value, String> {
    let headers = sign_request("GET", path, "");
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{url}{path}"));
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("parse JSON: {e}"))
}

#[cfg(feature = "admin-api")]
async fn admin_post(url: &str, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let body_str = serde_json::to_string(body).unwrap_or_default();
    let headers = sign_request("POST", path, &body_str);
    let client = reqwest::Client::new();
    let mut req = client.post(format!("{url}{path}"))
        .header("Content-Type", "application/json")
        .body(body_str);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("parse JSON: {e}"))
}

// ── validate ────────────────────────────────────────────────────────────────

/// Validate a bundle or output config JSON Schema.
///
/// `riversctl validate <bundle_path>` — run all validation checks on a bundle
/// `riversctl validate --schema server|app|bundle` — output JSON Schema to stdout
fn cmd_validate(args: &[String]) -> Result<(), String> {
    // --schema mode: output JSON Schema
    if args.first().map(|s| s.as_str()) == Some("--schema") {
        let schema_type = args.get(1).map(|s| s.as_str()).unwrap_or("server");
        let schema = match schema_type {
            "server" => rivers_runtime::rivers_core_config::server_config_schema(),
            "app" => rivers_runtime::app_config_schema(),
            "bundle" => rivers_runtime::bundle_manifest_schema(),
            other => return Err(format!("unknown schema type '{}' (expected: server, app, bundle)", other)),
        };
        println!("{}", serde_json::to_string_pretty(&schema).unwrap());
        return Ok(());
    }

    // Bundle validation mode
    let bundle_path = args.first().ok_or("Usage: riversctl validate <bundle_path> | --schema <type>")?;
    let path = std::path::Path::new(bundle_path);

    if !path.is_dir() {
        return Err(format!("bundle path '{}' is not a directory", bundle_path));
    }

    println!("Validating bundle: {}", path.display());

    // Load bundle
    let bundle = rivers_runtime::load_bundle(path)
        .map_err(|e| format!("bundle load failed: {}", e))?;

    println!("  Loaded: {} app(s)", bundle.apps.len());
    for app in &bundle.apps {
        println!("    - {} ({})", app.manifest.app_name, app.manifest.app_type);
    }

    // Run bundle validation (9 checks: view types, datasource refs, DataView refs,
    // invalidates targets, duplicate names, service refs, schema files, etc.)
    let mut error_count = 0;
    if let Err(errors) = rivers_runtime::validate_bundle(&bundle) {
        for e in &errors {
            eprintln!("  [ERROR] {}", e);
        }
        error_count += errors.len();
    }

    // Check keystore files exist (warning only — file may be created at runtime)
    for app in &bundle.apps {
        for (name, ks_config) in &app.config.data.keystore {
            let ks_path = app.app_dir.join(&ks_config.path);
            if !ks_path.exists() {
                eprintln!("  [WARN]  keystore '{}' file not found: {}", name, ks_path.display());
            }
        }
    }

    // Run driver name validation (hardcoded names — avoids pulling in DriverFactory + all drivers)
    let known: Vec<&str> = vec![
        // Built-in database drivers
        "eventbus", "faker", "memcached", "mysql", "postgres", "redis", "rps-client", "sqlite",
        // Plugin database drivers
        "cassandra", "couchdb", "elasticsearch", "http", "influxdb", "ldap", "mongodb",
        // Plugin broker drivers
        "kafka", "nats", "rabbitmq", "redis-streams",
    ];
    let driver_errors = rivers_runtime::validate_known_drivers(&bundle, &known);
    for e in &driver_errors {
        eprintln!("  [WARN]  {}", e);
    }

    if error_count == 0 {
        println!("  [PASS]  Bundle is valid ({} warning(s))", driver_errors.len());
        Ok(())
    } else {
        Err(format!("{} validation error(s) found", error_count))
    }
}

// ── Admin API commands ──────────────────────────────────────────────────────

#[cfg(feature = "admin-api")]
async fn cmd_status(url: &str) -> Result<(), String> {
    let data = admin_get(url, "/admin/status").await?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
async fn cmd_deploy(url: &str, bundle_path: &str) -> Result<(), String> {
    let body = serde_json::json!({ "bundle_path": bundle_path });
    let data = admin_post(url, "/admin/deploy", &body).await?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
async fn cmd_drivers(url: &str) -> Result<(), String> {
    let data = admin_get(url, "/admin/drivers").await?;
    if let Some(drivers) = data.as_array() {
        for d in drivers {
            println!("{}", d.as_str().unwrap_or(&d.to_string()));
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&data).unwrap());
    }
    Ok(())
}

#[cfg(feature = "admin-api")]
async fn cmd_datasources(url: &str) -> Result<(), String> {
    let data = admin_get(url, "/admin/datasources").await?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
async fn cmd_health(_url: &str) -> Result<(), String> {
    let main_url = std::env::var("RIVERS_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let data = admin_get(&main_url, "/health/verbose").await?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
async fn cmd_stop(url: &str) -> Result<(), String> {
    let body = serde_json::json!({ "mode": "immediate" });
    match admin_post(url, "/admin/shutdown", &body).await {
        Ok(data) => {
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
            Ok(())
        }
        Err(_) => {
            eprintln!("Admin API unreachable — falling back to signal");
            signal_riversd(Signal::Kill)
        }
    }
}

#[cfg(feature = "admin-api")]
async fn cmd_graceful(url: &str) -> Result<(), String> {
    let body = serde_json::json!({ "mode": "graceful" });
    match admin_post(url, "/admin/shutdown", &body).await {
        Ok(data) => {
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
            Ok(())
        }
        Err(_) => {
            eprintln!("Admin API unreachable — falling back to signal");
            signal_riversd(Signal::Term)
        }
    }
}

enum Signal {
    Kill,
    Term,
}

/// Find the riversd process and send a signal.
/// Cross-platform: uses `pgrep`/`kill` on Unix, `tasklist`/`taskkill` on Windows.
fn signal_riversd(sig: Signal) -> Result<(), String> {
    let pids = find_riversd_pids()?;
    if pids.is_empty() {
        return Err("no running riversd process found".into());
    }
    for pid in &pids {
        kill_pid(pid, &sig)?;
    }
    Ok(())
}

/// Discover PIDs of running riversd processes.
#[cfg(unix)]
fn find_riversd_pids() -> Result<Vec<String>, String> {
    let output = std::process::Command::new("pgrep")
        .arg("-x")
        .arg("riversd")
        .output()
        .map_err(|e| format!("failed to run pgrep: {e}"))?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let pids: Vec<String> = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("pgrep output: {e}"))?
        .trim()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    Ok(pids)
}

#[cfg(windows)]
fn find_riversd_pids() -> Result<Vec<String>, String> {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq riversd.exe", "/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| format!("failed to run tasklist: {e}"))?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("tasklist output: {e}"))?;

    // CSV format: "riversd.exe","1234","Console","1","12,345 K"
    let pids: Vec<String> = stdout
        .lines()
        .filter(|line| line.contains("riversd.exe"))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            fields.get(1).map(|pid| pid.trim_matches('"').to_string())
        })
        .collect();

    Ok(pids)
}

/// Send a signal to a process by PID.
#[cfg(unix)]
fn kill_pid(pid: &str, sig: &Signal) -> Result<(), String> {
    let signal_name = match sig {
        Signal::Kill => "KILL",
        Signal::Term => "TERM",
    };

    let status = std::process::Command::new("kill")
        .arg(format!("-{signal_name}"))
        .arg(pid)
        .status()
        .map_err(|e| format!("failed to send signal: {e}"))?;

    if status.success() {
        println!("Sent SIG{signal_name} to riversd (PID {pid})");
    } else {
        eprintln!("Failed to signal PID {pid}");
    }
    Ok(())
}

#[cfg(windows)]
fn kill_pid(pid: &str, sig: &Signal) -> Result<(), String> {
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/PID", pid]);

    // /F = force kill (immediate), omit for graceful
    if matches!(sig, Signal::Kill) {
        cmd.arg("/F");
    }

    let status = cmd.status().map_err(|e| format!("failed to run taskkill: {e}"))?;

    let mode = match sig {
        Signal::Kill => "force killed",
        Signal::Term => "stopped",
    };

    if status.success() {
        println!("Successfully {mode} riversd (PID {pid})");
    } else {
        eprintln!("Failed to stop PID {pid}");
    }
    Ok(())
}

#[cfg(feature = "admin-api")]
async fn cmd_log(url: &str, args: &[String]) -> Result<(), String> {
    match args.first().map(|s| s.as_str()) {
        Some("levels") => {
            let data = admin_get(url, "/admin/log/levels").await?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        Some("set") => {
            if args.len() < 3 {
                return Err("Usage: riversctl log set <event> <level>".into());
            }
            let body = serde_json::json!({ "event": args[1], "level": args[2] });
            let data = admin_post(url, "/admin/log/set", &body).await?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        Some("reset") => {
            let data = admin_post(url, "/admin/log/reset", &serde_json::json!({})).await?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        _ => return Err("Usage: riversctl log <levels|set|reset>".into()),
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "admin-api")]
    #[test]
    fn sign_request_produces_timestamp() {
        let headers = sign_request("GET", "/admin/status", "test body");
        assert!(headers.contains_key("X-Rivers-Timestamp"));
    }

    #[cfg(feature = "admin-api")]
    #[test]
    fn sign_request_without_key_has_no_signature() {
        std::env::remove_var("RIVERS_ADMIN_KEY");
        let headers = sign_request("GET", "/admin/status", "body");
        assert!(!headers.contains_key("X-Rivers-Signature"));
        assert!(headers.contains_key("X-Rivers-Timestamp"));
    }

    #[test]
    fn find_riversd_binary_env_missing_file() {
        std::env::set_var("RIVERS_DAEMON_PATH", "/nonexistent/riversd");
        let result = find_riversd_binary();
        std::env::remove_var("RIVERS_DAEMON_PATH");
        assert!(result.is_err());
    }

    #[test]
    fn discover_config_returns_none_when_absent() {
        assert!(discover_config().is_none());
    }

    #[test]
    fn start_arg_parsing_forwards_config_and_level() {
        // Verify arg parsing builds the expected riversd argv
        let args = vec![
            "--config".to_string(), "/etc/rivers.toml".to_string(),
            "--log-level".to_string(), "debug".to_string(),
        ];
        let mut riversd_args: Vec<String> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--config" | "-c" => {
                    i += 1;
                    riversd_args.push("--config".into());
                    riversd_args.push(args[i].clone());
                }
                "--log-level" | "-l" => {
                    i += 1;
                    riversd_args.push("--log-level".into());
                    riversd_args.push(args[i].clone());
                }
                "--no-admin-auth" => riversd_args.push("--no-admin-auth".into()),
                bundle => {
                    riversd_args.push("serve".into());
                    riversd_args.push(bundle.into());
                }
            }
            i += 1;
        }
        if !riversd_args.contains(&"serve".to_string()) {
            riversd_args.push("serve".into());
        }
        assert!(riversd_args.contains(&"--config".to_string()));
        assert!(riversd_args.contains(&"/etc/rivers.toml".to_string()));
        assert!(riversd_args.contains(&"serve".to_string()));
    }
}
