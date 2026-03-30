use std::path::{Path, PathBuf};

/// Locate riversd and launch it.
/// On Unix, replaces the current process (POSIX exec) so signals pass through naturally.
/// On Windows, spawns riversd as a child process and waits for it.
pub fn cmd_start(args: &[String]) -> Result<(), String> {
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
