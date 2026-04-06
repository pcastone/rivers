use std::path::PathBuf;

pub fn cmd_stop(_args: &[String]) -> Result<(), String> {
    let pid = read_pid_file()?;
    if !is_process_alive(pid) {
        cleanup_pid_file();
        return Err(format!("riversd (pid {pid}) is not running"));
    }
    println!("rivers: stopping riversd (pid {pid})");
    #[cfg(unix)]
    unsafe { libc::kill(pid as i32, libc::SIGTERM); }
    #[cfg(windows)]
    { let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string()]).status(); }

    for i in 0..30 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if !is_process_alive(pid) {
            cleanup_pid_file();
            println!("rivers: riversd stopped (took {}s)", i + 1);
            return Ok(());
        }
    }
    eprintln!("rivers: riversd did not stop after 30s — sending SIGKILL");
    #[cfg(unix)]
    unsafe { libc::kill(pid as i32, libc::SIGKILL); }
    cleanup_pid_file();
    Ok(())
}

pub(crate) fn read_pid_file() -> Result<u32, String> {
    let pid_path = find_pid_file().ok_or("PID file not found — is riversd running?")?;
    let content = std::fs::read_to_string(&pid_path).map_err(|e| format!("cannot read PID file: {e}"))?;
    content.trim().parse::<u32>().map_err(|e| format!("invalid PID: {e}"))
}

pub(crate) fn find_pid_file() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(exe) = exe.canonicalize() {
            if let Some(root) = exe.parent().and_then(|b| b.parent()) {
                let p = root.join("run/riversd.pid");
                if p.is_file() { return Some(p); }
            }
        }
    }
    if let Ok(home) = std::env::var("RIVERS_HOME") {
        let p = PathBuf::from(home).join("run/riversd.pid");
        if p.is_file() { return Some(p); }
    }
    let p = PathBuf::from("run/riversd.pid");
    if p.is_file() { return Some(p); }
    None
}

pub(crate) fn cleanup_pid_file() {
    if let Some(p) = find_pid_file() { let _ = std::fs::remove_file(p); }
}

#[cfg(unix)]
pub(crate) fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub(crate) fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}
