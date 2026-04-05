use std::path::{Path, PathBuf};

/// Locate riversd and launch it as a background daemon.
pub fn cmd_start(args: &[String]) -> Result<(), String> {
    let binary = find_riversd_binary()?;

    // Parse riversctl start flags and forward to riversd serve.
    let mut riversd_args: Vec<String> = Vec::new();
    let mut explicit_config: Option<String> = None;
    let mut explicit_port: Option<String> = None;
    let mut foreground = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                i += 1;
                let path = args.get(i).ok_or("--config requires a value")?;
                explicit_config = Some(path.clone());
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
                explicit_port = Some(port.clone());
                riversd_args.push("--port".into());
                riversd_args.push(port.clone());
            }
            "--foreground" | "-f" => {
                foreground = true;
            }
            other => {
                return Err(format!("unknown option: {other}\nUsage: riversctl start [--config <path>] [--log-level <lvl>] [--no-admin-auth] [--no-ssl [--port <port>]] [--foreground]"));
            }
        }
        i += 1;
    }

    riversd_args.push("serve".into());

    // Print startup info
    print_startup_info(&binary, &explicit_config, &explicit_port);

    if foreground {
        launch_foreground(&binary, &riversd_args)
    } else {
        launch_daemon(&binary, &riversd_args)
    }
}

/// Print startup info: binary path, config, port, RIVERS_HOME.
fn print_startup_info(binary: &Path, explicit_config: &Option<String>, explicit_port: &Option<String>) {
    use super::doctor::discover_config;

    println!("rivers: starting riversd v{}", env!("CARGO_PKG_VERSION"));
    println!("  binary:  {}", binary.display());

    // Config
    let config_path = explicit_config
        .as_ref()
        .map(PathBuf::from)
        .or_else(discover_config);
    if let Some(ref cp) = config_path {
        println!("  config:  {}", cp.display());
    } else {
        println!("  config:  (defaults)");
    }

    // Port — from explicit flag or parsed config
    if let Some(ref port) = explicit_port {
        println!("  port:    {port}");
    } else if let Some(ref cp) = config_path {
        if let Ok(cfg) = rivers_runtime::loader::load_server_config(cp) {
            println!("  port:    {}", cfg.base.port);
        }
    }

    // RIVERS_HOME
    if let Ok(home) = std::env::var("RIVERS_HOME") {
        println!("  home:    {home}");
    } else if let Ok(this_exe) = std::env::current_exe() {
        if let Ok(this_exe) = this_exe.canonicalize() {
            if let Some(bin_dir) = this_exe.parent() {
                if let Some(root) = bin_dir.parent() {
                    if root.join("config").is_dir() {
                        println!("  home:    {}", root.display());
                    }
                }
            }
        }
    }

    println!();
}

/// Launch riversd as a background daemon. Prints PID on success.
fn launch_daemon(binary: &Path, args: &[String]) -> Result<(), String> {
    let child = std::process::Command::new(binary)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start riversd: {e}"))?;

    println!("rivers: riversd started (pid {})", child.id());
    Ok(())
}

/// Launch riversd in the foreground (replaces process on Unix, waits on Windows).
/// Use --foreground / -f for interactive debugging or systemd Type=simple.
#[cfg(unix)]
fn launch_foreground(binary: &Path, args: &[String]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    // SAFETY: CommandExt::exec passes args as a separate array (no shell injection).
    let err = std::process::Command::new(binary)
        .args(args)
        .exec(); // replaces this process
    Err(format!("failed to exec {}: {}", binary.display(), err))
}

#[cfg(windows)]
fn launch_foreground(binary: &Path, args: &[String]) -> Result<(), String> {
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
pub fn riversd_binary_name() -> &'static str {
    if cfg!(windows) { "riversd.exe" } else { "riversd" }
}

/// Find the riversd binary.
pub fn find_riversd_binary() -> Result<PathBuf, String> {
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
